//! Claude price table and cost arithmetic, shared by the relay (soft caps,
//! `/v1/usage`) and the phone (local spending limit in direct mode).
//!
//! Money is integer micro-USD (1 USD = 1,000,000) so sums are exact. Prices
//! are *estimates* used for limits and the usage screen; the Anthropic Console
//! is the source of truth for billing.

use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;

/// Default prices, USD per 1M tokens (Claude API list prices, 2026-11).
/// Haiku 5.5 rates apply to prompts up to 100K tokens, which covers this app.
pub const DEFAULT_PRICES_JSON: &str = r#"{
  "claude-haiku-5-5":  {"input": 0.10, "output": 0.50, "cache_read": 0.01, "cache_write": 0.125},
  "claude-sonnet-5-5": {"input": 2.00, "output": 10.00, "cache_read": 0.20, "cache_write": 2.50},
  "claude-opus-5-5":   {"input": 4.00, "output": 20.00, "cache_read": 0.20, "cache_write": 5.00}
}"#;

/// Price of one model in micro-USD per 1M tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrice {
    /// Uncached input.
    pub input: u64,
    /// Output (including thinking).
    pub output: u64,
    /// Cache reads.
    pub cache_read: u64,
    /// Cache writes.
    pub cache_write: u64,
}

/// Token counts of one response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Uncached input.
    pub input: u64,
    /// Output.
    pub output: u64,
    /// Read from cache.
    pub cache_read: u64,
    /// Written to cache.
    pub cache_write: u64,
}

/// A price table could not be parsed.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "price table must be a JSON object {{model: {{input, output, cache_read?, cache_write?}}}} in USD per 1M tokens"
)]
pub struct PriceTableError;

/// Model id → price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceTable(BTreeMap<String, ModelPrice>);

impl Default for PriceTable {
    fn default() -> Self {
        // The built-in table is valid (tested); fall back to empty defensively.
        Self::parse(DEFAULT_PRICES_JSON).unwrap_or(Self(BTreeMap::new()))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PriceWire {
    input: f64,
    output: f64,
    #[serde(default)]
    cache_read: Option<f64>,
    #[serde(default)]
    cache_write: Option<f64>,
}

impl PriceTable {
    /// Parse a JSON price table (USD per 1M tokens). Missing cache prices
    /// default to Anthropic's usual ratios (read 0.1×, write 1.25× input).
    pub fn parse(json: &str) -> Result<Self, PriceTableError> {
        let wire: BTreeMap<String, PriceWire> =
            serde_json::from_str(json).map_err(|_| PriceTableError)?;
        let to_micros = |usd: f64| -> Result<u64, PriceTableError> {
            if usd.is_finite() && (0.0..=1_000_000.0).contains(&usd) {
                // Bounded and non-negative, so the cast cannot wrap.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Ok((usd * 1_000_000.0).round() as u64)
            } else {
                Err(PriceTableError)
            }
        };
        wire.into_iter()
            .map(|(model, p)| {
                let input = to_micros(p.input)?;
                Ok((
                    model,
                    ModelPrice {
                        input,
                        output: to_micros(p.output)?,
                        cache_read: p
                            .cache_read
                            .map(to_micros)
                            .transpose()?
                            .unwrap_or(input / 10),
                        cache_write: p
                            .cache_write
                            .map(to_micros)
                            .transpose()?
                            .unwrap_or(input * 5 / 4),
                    },
                ))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    /// Price for `model`. Unknown models are charged at the most expensive
    /// configured rate, so limits fail safe rather than under-count.
    pub fn price_for(&self, model: &str) -> ModelPrice {
        if let Some(p) = self.0.get(model) {
            return *p;
        }
        let max = |f: fn(&ModelPrice) -> u64| self.0.values().map(f).max().unwrap_or(0);
        ModelPrice {
            input: max(|p| p.input),
            output: max(|p| p.output),
            cache_read: max(|p| p.cache_read),
            cache_write: max(|p| p.cache_write),
        }
    }

    /// Estimated cost of a response in micro-USD.
    pub fn cost(&self, model: &str, t: &Tokens) -> u64 {
        cost_micros(t, &self.price_for(model))
    }
}

/// Cost in micro-USD, rounded up per component (never 0 for non-zero use).
pub fn cost_micros(t: &Tokens, p: &ModelPrice) -> u64 {
    let part = |tokens: u64, per_mtok: u64| -> u64 {
        let micros = (u128::from(tokens) * u128::from(per_mtok)).div_ceil(1_000_000);
        u64::try_from(micros).unwrap_or(u64::MAX)
    };
    part(t.input, p.input)
        .saturating_add(part(t.output, p.output))
        .saturating_add(part(t.cache_read, p.cache_read))
        .saturating_add(part(t.cache_write, p.cache_write))
}

/// Parse a decimal USD amount ("5", "2.50", ".5") into micro-USD without
/// floating point.
pub fn usd_to_micros(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('$');
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    if frac.len() > 6
        || !whole
            .bytes()
            .chain(frac.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut frac_micros: u64 = if frac.is_empty() {
        0
    } else {
        frac.parse().ok()?
    };
    for _ in frac.len()..6 {
        frac_micros *= 10;
    }
    whole.checked_mul(1_000_000)?.checked_add(frac_micros)
}

/// Format micro-USD as `$1.234567` trimmed to cents where exact (`$2.00`).
pub fn format_usd(micros: u64) -> String {
    let whole = micros / 1_000_000;
    let frac = micros % 1_000_000;
    if frac.is_multiple_of(10_000) {
        format!("${whole}.{:02}", frac / 10_000)
    } else {
        format!("${whole}.{frac:06}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table() {
        let t = PriceTable::default();
        assert_eq!(t.price_for("claude-haiku-5-5").input, 100_000);
        assert_eq!(t.price_for("claude-opus-5-5").output, 20_000_000);
        assert_eq!(t.price_for("mystery"), t.price_for("claude-opus-5-5"));
    }

    #[test]
    fn cost_math() {
        let h = PriceTable::default().price_for("claude-haiku-5-5");
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        assert_eq!(cost_micros(&t, &h), 600_000);
        assert_eq!(
            cost_micros(
                &Tokens {
                    input: 1,
                    ..Default::default()
                },
                &h
            ),
            1
        );
        assert_eq!(cost_micros(&Tokens::default(), &h), 0);
        let max = ModelPrice {
            input: u64::MAX,
            output: u64::MAX,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(
            cost_micros(
                &Tokens {
                    input: u64::MAX,
                    output: u64::MAX,
                    ..Default::default()
                },
                &max
            ),
            u64::MAX
        );
    }

    #[test]
    fn parsing() {
        let t = PriceTable::parse(r#"{"m":{"input":1,"output":2}}"#).unwrap();
        let p = t.price_for("m");
        assert_eq!((p.cache_read, p.cache_write), (100_000, 1_250_000));
        assert!(PriceTable::parse(r#"{"m":{"input":-1,"output":1}}"#).is_err());
        assert!(PriceTable::parse("[]").is_err());
    }

    #[test]
    fn usd() {
        assert_eq!(usd_to_micros("5"), Some(5_000_000));
        assert_eq!(usd_to_micros("$2.5"), Some(2_500_000));
        assert_eq!(usd_to_micros(".5"), Some(500_000));
        assert_eq!(usd_to_micros("0.000001"), Some(1));
        assert_eq!(usd_to_micros("1.0000001"), None);
        assert_eq!(usd_to_micros("-1"), None);
        assert_eq!(usd_to_micros("."), None);
        assert_eq!(format_usd(2_000_000), "$2.00");
        assert_eq!(format_usd(1_234), "$0.001234");
    }
}

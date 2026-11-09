//! Relay configuration, read from `wrangler.toml` `[vars]`.
//!
//! Every value has a safe default so a relay works with an empty `[vars]`
//! table. Secrets (`ANTHROPIC_API_KEY`, `PAIRING_CODE`) are *not* read here;
//! they are fetched only by the handlers that need them.

use std::collections::BTreeMap;

use pocket_agent_protocol::Caps;
use serde::Deserialize;
use thiserror::Error;

/// Default upstream. The only non-loopback upstream the relay will talk to.
pub const ANTHROPIC_API: &str = "https://api.anthropic.com";
/// Default `anthropic-version` header when the client does not send one.
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2026-10-21";

const DEFAULT_MODELS: &str = "claude-haiku-5-5,claude-sonnet-5-5,claude-opus-5-5";
const DEFAULT_BETAS: &str = "server-side-fallback-2026-07-01";
const DEFAULT_MAX_TOKENS_CAP: u64 = 16_000;
const DEFAULT_PAIRING_MAX_FAILURES: u32 = 10;
const DEFAULT_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Default prices, USD per 1M tokens.
const DEFAULT_PRICES: &str = r#"{
  "claude-haiku-5-5":  {"input": 0.10, "output": 0.50, "cache_read": 0.01, "cache_write": 0.125},
  "claude-sonnet-5-5": {"input": 2.00, "output": 10.00, "cache_read": 0.20, "cache_write": 2.50},
  "claude-opus-5-5":   {"input": 4.00, "output": 20.00, "cache_read": 0.20, "cache_write": 5.00}
}"#;

/// A configuration value was present but invalid.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid relay configuration `{name}`: {reason}")]
pub struct ConfigError {
    /// Variable name.
    pub name: &'static str,
    /// What was wrong (never echoes the value).
    pub reason: &'static str,
}

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

/// Parsed relay configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Models the relay will forward.
    pub model_allowlist: Vec<String>,
    /// `anthropic-beta` values the relay will forward.
    pub beta_allowlist: Vec<String>,
    /// Requests asking for more output tokens are clamped to this.
    pub max_tokens_cap: u64,
    /// Soft spending caps.
    pub caps: Caps,
    /// Price table keyed by model id.
    pub prices: BTreeMap<String, ModelPrice>,
    /// Failed pairing attempts before pairing locks.
    pub pairing_max_failures: u32,
    /// Maximum accepted request body.
    pub max_body_bytes: usize,
    /// Upstream base URL (Anthropic, or loopback for local tests).
    pub upstream: String,
}

impl Config {
    /// Build from a variable lookup (`|name| env.var(name)`). Missing or
    /// empty variables use defaults.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let var = |name: &str| {
            get(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };

        let model_allowlist = csv(&var("MODEL_ALLOWLIST").unwrap_or_else(|| DEFAULT_MODELS.into()));
        if model_allowlist.is_empty() {
            return Err(ConfigError {
                name: "MODEL_ALLOWLIST",
                reason: "must list at least one model",
            });
        }
        let beta_allowlist = csv(&var("BETA_ALLOWLIST").unwrap_or_else(|| DEFAULT_BETAS.into()));

        let max_tokens_cap = match var("MAX_TOKENS_CAP") {
            None => DEFAULT_MAX_TOKENS_CAP,
            Some(v) => v
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or(ConfigError {
                    name: "MAX_TOKENS_CAP",
                    reason: "must be a positive integer",
                })?,
        };

        let caps = Caps {
            daily_usd_micros: usd_var(var("DAILY_USD_CAP"), "DAILY_USD_CAP")?,
            monthly_usd_micros: usd_var(var("MONTHLY_USD_CAP"), "MONTHLY_USD_CAP")?,
        };

        let prices = parse_prices(&var("PRICES").unwrap_or_else(|| DEFAULT_PRICES.into()))?;

        let pairing_max_failures = match var("PAIRING_MAX_FAILURES") {
            None => DEFAULT_PAIRING_MAX_FAILURES,
            Some(v) => v
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or(ConfigError {
                    name: "PAIRING_MAX_FAILURES",
                    reason: "must be a positive integer",
                })?,
        };

        let max_body_bytes = match var("MAX_BODY_BYTES") {
            None => DEFAULT_MAX_BODY_BYTES,
            Some(v) => v
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1024)
                .ok_or(ConfigError {
                    name: "MAX_BODY_BYTES",
                    reason: "must be an integer >= 1024",
                })?,
        };

        let upstream = match var("UPSTREAM_URL") {
            None => ANTHROPIC_API.to_owned(),
            Some(u) => validate_upstream(&u)?,
        };

        Ok(Self {
            model_allowlist,
            beta_allowlist,
            max_tokens_cap,
            caps,
            prices,
            pairing_max_failures,
            max_body_bytes,
            upstream,
        })
    }

    /// Price for `model`. Unknown models are charged at the most expensive
    /// configured rate, so caps fail safe rather than under-count.
    pub fn price_for(&self, model: &str) -> ModelPrice {
        if let Some(p) = self.prices.get(model) {
            return *p;
        }
        let max = |f: fn(&ModelPrice) -> u64| self.prices.values().map(f).max().unwrap_or(0);
        ModelPrice {
            input: max(|p| p.input),
            output: max(|p| p.output),
            cache_read: max(|p| p.cache_read),
            cache_write: max(|p| p.cache_write),
        }
    }

    /// Whether `model` may be forwarded.
    pub fn model_allowed(&self, model: &str) -> bool {
        self.model_allowlist.iter().any(|m| m == model)
    }
}

/// The upstream may only be the real Anthropic API, or a loopback address
/// (reachable only under `wrangler dev`, used by integration tests). This
/// stops a misconfiguration from sending the API key anywhere else.
fn validate_upstream(u: &str) -> Result<String, ConfigError> {
    let u = u.trim_end_matches('/');
    let ok = u == ANTHROPIC_API
        || ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| {
            u.strip_prefix(p)
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        });
    if ok {
        Ok(u.to_owned())
    } else {
        Err(ConfigError {
            name: "UPSTREAM_URL",
            reason: "must be https://api.anthropic.com or a loopback http://127.0.0.1:<port>",
        })
    }
}

fn csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Parse a decimal USD amount ("5", "2.50") into micro-USD. `None` => 0 (no cap).
fn usd_var(v: Option<String>, name: &'static str) -> Result<u64, ConfigError> {
    match v {
        None => Ok(0),
        Some(v) => usd_to_micros(&v).ok_or(ConfigError {
            name,
            reason: "must be a non-negative USD amount like 5 or 2.50",
        }),
    }
}

/// Parse "12.345678" into micros without floating point.
pub fn usd_to_micros(s: &str) -> Option<u64> {
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

fn parse_prices(json: &str) -> Result<BTreeMap<String, ModelPrice>, ConfigError> {
    const ERR: ConfigError = ConfigError {
        name: "PRICES",
        reason: "must be a JSON object {model: {input, output, cache_read?, cache_write?}} in USD per 1M tokens",
    };
    let wire: BTreeMap<String, PriceWire> = serde_json::from_str(json).map_err(|_| ERR)?;
    let to_micros = |usd: f64| -> Result<u64, ConfigError> {
        if usd.is_finite() && (0.0..=1_000_000.0).contains(&usd) {
            // Bounded and non-negative, so the cast cannot wrap.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Ok((usd * 1_000_000.0).round() as u64)
        } else {
            Err(ERR)
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
                    // Anthropic's usual ratios when not given explicitly.
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_are_sane() {
        let c = cfg(&[]).unwrap();
        assert!(c.model_allowed("claude-haiku-5-5"));
        assert!(c.model_allowed("claude-sonnet-5-5"));
        assert!(!c.model_allowed("claude-fable-5-1"));
        assert_eq!(c.max_tokens_cap, 16_000);
        assert_eq!(c.caps, Caps::default());
        assert_eq!(c.upstream, ANTHROPIC_API);
        assert_eq!(c.price_for("claude-haiku-5-5").input, 100_000);
        assert_eq!(c.price_for("claude-opus-5-5").output, 20_000_000);
        assert_eq!(c.beta_allowlist, vec!["server-side-fallback-2026-07-01"]);
    }

    #[test]
    fn unknown_model_priced_at_max() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.price_for("mystery"), c.price_for("claude-opus-5-5"));
    }

    #[test]
    fn usd_parsing() {
        assert_eq!(usd_to_micros("5"), Some(5_000_000));
        assert_eq!(usd_to_micros("2.5"), Some(2_500_000));
        assert_eq!(usd_to_micros("0.000001"), Some(1));
        assert_eq!(usd_to_micros(".5"), Some(500_000));
        assert_eq!(usd_to_micros("1.0000001"), None);
        assert_eq!(usd_to_micros("-1"), None);
        assert_eq!(usd_to_micros("abc"), None);
        assert_eq!(usd_to_micros("."), None);
    }

    #[test]
    fn caps_and_overrides() {
        let c = cfg(&[
            ("DAILY_USD_CAP", "1.50"),
            ("MONTHLY_USD_CAP", "20"),
            ("MAX_TOKENS_CAP", "4096"),
            ("MODEL_ALLOWLIST", " a , b ,"),
        ])
        .unwrap();
        assert_eq!(c.caps.daily_usd_micros, 1_500_000);
        assert_eq!(c.caps.monthly_usd_micros, 20_000_000);
        assert_eq!(c.max_tokens_cap, 4096);
        assert_eq!(c.model_allowlist, vec!["a", "b"]);
    }

    #[test]
    fn rejects_bad_values() {
        assert_eq!(
            cfg(&[("MAX_TOKENS_CAP", "0")]).unwrap_err().name,
            "MAX_TOKENS_CAP"
        );
        assert_eq!(
            cfg(&[("DAILY_USD_CAP", "lots")]).unwrap_err().name,
            "DAILY_USD_CAP"
        );
        assert_eq!(
            cfg(&[("PRICES", "{\"m\":{\"input\":-1,\"output\":1}}")])
                .unwrap_err()
                .name,
            "PRICES"
        );
        assert_eq!(
            cfg(&[("MODEL_ALLOWLIST", ",")]).unwrap_err().name,
            "MODEL_ALLOWLIST"
        );
    }

    #[test]
    fn upstream_must_be_anthropic_or_loopback() {
        assert!(cfg(&[("UPSTREAM_URL", "http://127.0.0.1:9999")]).is_ok());
        assert!(cfg(&[("UPSTREAM_URL", "https://api.anthropic.com/")]).is_ok());
        for bad in [
            "https://evil.example",
            "http://127.0.0.1.evil.example:80",
            "http://127.0.0.1:",
            "http://localhost:80/x",
        ] {
            assert_eq!(
                cfg(&[("UPSTREAM_URL", bad)]).unwrap_err().name,
                "UPSTREAM_URL",
                "{bad}"
            );
        }
    }

    #[test]
    fn price_defaults_for_cache() {
        let c = cfg(&[("PRICES", r#"{"m":{"input":1,"output":2}}"#)]).unwrap();
        let p = c.price_for("m");
        assert_eq!((p.cache_read, p.cache_write), (100_000, 1_250_000));
    }
}

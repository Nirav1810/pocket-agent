//! Local spending limit.
//!
//! Direct mode has no relay in front of the API key, so the phone enforces
//! its own daily and monthly limits (relay mode gets these too, on top of the
//! relay's caps). Every response's estimated cost (shared price table in
//! `pocket_agent_protocol::pricing`) is added to a small ledger file; before
//! each request the agent checks it. Limits are soft: the request that crosses
//! the line completes, the next one is refused.
//!
//! This is a safety net, not a guarantee: estimates can drift from real
//! billing, and a leaked key can be used elsewhere. The hard limit is the
//! spend limit in the Anthropic Console.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use pocket_agent_protocol::pricing::{PriceTable, Tokens};
use pocket_agent_protocol::time::{month_start, utc_day};
use serde::{Deserialize, Serialize};

use crate::api::Usage;
use crate::settings::atomic_write;

/// Spend so far and the configured limits, for the usage screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct BudgetStatus {
    /// Estimated spend today (UTC), micro-USD.
    pub today_usd_micros: u64,
    /// Estimated spend this month (UTC), micro-USD.
    pub month_usd_micros: u64,
    /// Daily limit, micro-USD (0 = none).
    pub daily_limit_usd_micros: u64,
    /// Monthly limit, micro-USD (0 = none).
    pub monthly_limit_usd_micros: u64,
}

/// Which limit was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitHit {
    /// Daily limit.
    Daily,
    /// Monthly limit.
    Monthly,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Ledger {
    schema_version: u32,
    /// `YYYY-MM-DD` the daily figure belongs to.
    day: String,
    /// `YYYY-MM-01` the monthly figure belongs to.
    month: String,
    day_usd_micros: u64,
    month_usd_micros: u64,
}

impl Ledger {
    /// Reset counters that belong to an earlier day/month.
    fn roll(&mut self, today: &str) {
        let month = month_start(today);
        if self.month != month {
            self.month = month;
            self.month_usd_micros = 0;
        }
        if self.day != today {
            self.day = today.to_owned();
            self.day_usd_micros = 0;
        }
    }
}

/// Persistent spend ledger (`usage.json` next to `settings.json`).
#[derive(Debug)]
pub struct Budget {
    path: PathBuf,
    ledger: Mutex<Ledger>,
    prices: PriceTable,
}

/// Today's UTC date.
pub fn today() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    utc_day(ms)
}

impl Budget {
    /// Open (or start) the ledger in `dir`. A corrupt ledger starts from zero
    /// (spend is an estimate anyway; the Console limit is the backstop).
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("usage.json");
        let ledger = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            ledger: Mutex::new(ledger),
            prices: PriceTable::default(),
        }
    }

    /// Whether a new request is allowed under the given limits (0 = none).
    pub fn check(&self, today: &str, daily_limit: u64, monthly_limit: u64) -> Result<(), LimitHit> {
        let Ok(mut l) = self.ledger.lock() else {
            return Ok(());
        };
        l.roll(today);
        if daily_limit > 0 && l.day_usd_micros >= daily_limit {
            Err(LimitHit::Daily)
        } else if monthly_limit > 0 && l.month_usd_micros >= monthly_limit {
            Err(LimitHit::Monthly)
        } else {
            Ok(())
        }
    }

    /// Add one response's estimated cost and persist. Returns the cost.
    pub fn record(&self, today: &str, model: &str, usage: &Usage) -> u64 {
        let cost = self.prices.cost(
            model,
            &Tokens {
                input: usage.input_tokens,
                output: usage.output_tokens,
                cache_read: usage.cache_read_input_tokens,
                cache_write: usage.cache_creation_input_tokens,
            },
        );
        let Ok(mut l) = self.ledger.lock() else {
            return cost;
        };
        l.roll(today);
        l.schema_version = 1;
        l.day_usd_micros = l.day_usd_micros.saturating_add(cost);
        l.month_usd_micros = l.month_usd_micros.saturating_add(cost);
        if let Ok(json) = serde_json::to_vec(&*l)
            && let Err(e) = atomic_write(&self.path, &json)
        {
            tracing::warn!("could not save usage ledger: {e}");
        }
        cost
    }

    /// Current spend and limits.
    pub fn status(&self, today: &str, daily_limit: u64, monthly_limit: u64) -> BudgetStatus {
        let (d, m) = self
            .ledger
            .lock()
            .map(|mut l| {
                l.roll(today);
                (l.day_usd_micros, l.month_usd_micros)
            })
            .unwrap_or_default();
        BudgetStatus {
            today_usd_micros: d,
            month_usd_micros: m,
            daily_limit_usd_micros: daily_limit,
            monthly_limit_usd_micros: monthly_limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Default::default()
        }
    }

    #[test]
    fn enforces_daily_then_resets_next_day() {
        let dir = tempfile::tempdir().unwrap();
        let b = Budget::open(dir.path());
        // Haiku: 1M in + 1M out = $0.60.
        assert_eq!(b.check("2026-11-09", 1_000_000, 0), Ok(()));
        assert_eq!(
            b.record(
                "2026-11-09",
                "claude-haiku-5-5",
                &usage(1_000_000, 1_000_000)
            ),
            600_000
        );
        assert_eq!(
            b.check("2026-11-09", 1_000_000, 0),
            Ok(()),
            "soft limit: still under"
        );
        b.record(
            "2026-11-09",
            "claude-haiku-5-5",
            &usage(1_000_000, 1_000_000),
        );
        assert_eq!(b.check("2026-11-09", 1_000_000, 0), Err(LimitHit::Daily));
        assert_eq!(b.check("2026-11-10", 1_000_000, 0), Ok(()), "new day");
        assert_eq!(
            b.check("2026-11-10", 0, 1_000_000),
            Err(LimitHit::Monthly),
            "month keeps counting"
        );
        assert_eq!(b.check("2026-12-01", 0, 1_000_000), Ok(()), "new month");
    }

    #[test]
    fn persists_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        Budget::open(dir.path()).record("2026-11-09", "claude-sonnet-5-5", &usage(100_000, 10_000));
        let s = Budget::open(dir.path()).status("2026-11-09", 5, 6);
        assert_eq!(s.today_usd_micros, 300_000); // $0.20 + $0.10
        assert_eq!(s.month_usd_micros, 300_000);
        assert_eq!(
            (s.daily_limit_usd_micros, s.monthly_limit_usd_micros),
            (5, 6)
        );
    }

    #[test]
    fn zero_means_unlimited_and_corrupt_file_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("usage.json"), b"garbage").unwrap();
        let b = Budget::open(dir.path());
        b.record(
            "2026-11-09",
            "claude-opus-5-5",
            &usage(10_000_000, 1_000_000),
        );
        assert_eq!(b.check("2026-11-09", 0, 0), Ok(()));
    }

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }
}

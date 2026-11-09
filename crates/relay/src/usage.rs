//! Cost estimation, usage aggregation and spending-cap checks.

use pocket_agent_protocol::{Caps, UsageSummary, UsageTotals};

use crate::config::ModelPrice;
use crate::inspect::Usage;

/// Estimated cost of one response in micro-USD, rounded up per component.
pub fn cost_micros(u: &Usage, p: &ModelPrice) -> u64 {
    pocket_agent_protocol::pricing::cost_micros(
        &pocket_agent_protocol::pricing::Tokens {
            input: u.input_tokens,
            output: u.output_tokens,
            cache_read: u.cache_read_input_tokens,
            cache_write: u.cache_creation_input_tokens,
        },
        p,
    )
}

/// One row of the `usage` table (per day, device and model).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageRow {
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    /// Device id.
    pub device_id: String,
    /// Totals for that day/device/model.
    pub totals: UsageTotals,
}

fn add(a: &mut UsageTotals, b: &UsageTotals) {
    a.input_tokens += b.input_tokens;
    a.output_tokens += b.output_tokens;
    a.cache_read_input_tokens += b.cache_read_input_tokens;
    a.cache_creation_input_tokens += b.cache_creation_input_tokens;
    a.usd_micros += b.usd_micros;
    a.requests += b.requests;
}

/// Aggregate this month's rows into a [`UsageSummary`].
pub fn summarize(rows: &[UsageRow], today: &str, device_id: &str, caps: Caps) -> UsageSummary {
    let mut s = UsageSummary {
        day: today.to_owned(),
        today: UsageTotals::default(),
        month: UsageTotals::default(),
        device_today: UsageTotals::default(),
        device_month: UsageTotals::default(),
        caps,
    };
    for r in rows {
        let is_today = r.day == today;
        let is_device = r.device_id == device_id;
        add(&mut s.month, &r.totals);
        if is_today {
            add(&mut s.today, &r.totals);
        }
        if is_device {
            add(&mut s.device_month, &r.totals);
            if is_today {
                add(&mut s.device_today, &r.totals);
            }
        }
    }
    s
}

/// Which cap, if any, blocks a new request.
#[derive(Debug, PartialEq, Eq)]
pub enum CapHit {
    /// Daily cap reached.
    Daily,
    /// Monthly cap reached.
    Monthly,
}

/// Check soft caps against spend so far. A cap of 0 means unlimited.
/// Caps are "soft": the request that crosses the line is allowed, the next is not.
pub fn check_caps(caps: Caps, spent_today: u64, spent_month: u64) -> Option<CapHit> {
    if caps.daily_usd_micros > 0 && spent_today >= caps.daily_usd_micros {
        Some(CapHit::Daily)
    } else if caps.monthly_usd_micros > 0 && spent_month >= caps.monthly_usd_micros {
        Some(CapHit::Monthly)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAIKU: ModelPrice = ModelPrice {
        input: 100_000,
        output: 500_000,
        cache_read: 10_000,
        cache_write: 125_000,
    };

    #[test]
    fn cost_math() {
        let u = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        assert_eq!(cost_micros(&u, &HAIKU), 600_000); // $0.10 + $0.50
        let small = Usage {
            input_tokens: 1,
            ..Default::default()
        };
        assert_eq!(cost_micros(&small, &HAIKU), 1); // rounds up, never 0 for non-zero use
        assert_eq!(cost_micros(&Usage::default(), &HAIKU), 0);
        let huge = Usage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            ..Default::default()
        };
        assert_eq!(
            cost_micros(
                &huge,
                &ModelPrice {
                    input: u64::MAX,
                    output: u64::MAX,
                    cache_read: 0,
                    cache_write: 0
                }
            ),
            u64::MAX
        );
    }

    fn row(day: &str, dev: &str, usd: u64) -> UsageRow {
        UsageRow {
            day: day.into(),
            device_id: dev.into(),
            totals: UsageTotals {
                usd_micros: usd,
                requests: 1,
                ..Default::default()
            },
        }
    }

    #[test]
    fn summary_splits_by_day_and_device() {
        let rows = [
            row("2026-11-09", "a", 10),
            row("2026-11-09", "b", 20),
            row("2026-11-01", "a", 5),
        ];
        let s = summarize(&rows, "2026-11-09", "a", Caps::default());
        assert_eq!(s.today.usd_micros, 30);
        assert_eq!(s.month.usd_micros, 35);
        assert_eq!(s.device_today.usd_micros, 10);
        assert_eq!(s.device_month.usd_micros, 15);
        assert_eq!(s.month.requests, 3);
    }

    #[test]
    fn caps() {
        let caps = Caps {
            daily_usd_micros: 100,
            monthly_usd_micros: 1000,
        };
        assert_eq!(check_caps(caps, 99, 999), None);
        assert_eq!(check_caps(caps, 100, 100), Some(CapHit::Daily));
        assert_eq!(check_caps(caps, 0, 1000), Some(CapHit::Monthly));
        assert_eq!(check_caps(Caps::default(), u64::MAX, u64::MAX), None);
    }
}

//! UTC calendar helpers (no chrono: keeps the WASM small).

/// `YYYY-MM-DD` for a Unix timestamp in milliseconds.
pub fn utc_day(unix_ms: u64) -> String {
    let days = i64::try_from(unix_ms / 86_400_000).unwrap_or(i64::MAX);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// First day of the month containing `day` (`YYYY-MM-DD` → `YYYY-MM-01`).
pub fn month_start(day: &str) -> String {
    format!("{}-01", day.get(..7).unwrap_or(day))
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    // d is 1..=31 and m is 1..=12 by construction.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (y, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(951_782_400_000), "2000-02-29");
        assert_eq!(utc_day(1_794_268_799_000), "2026-11-09"); // 2026-11-09T23:59:59Z
        assert_eq!(utc_day(1_794_268_800_000), "2026-11-10");
        assert_eq!(utc_day(1_798_761_600_000), "2027-01-01");
    }

    #[test]
    fn month() {
        assert_eq!(month_start("2026-11-09"), "2026-11-01");
    }
}

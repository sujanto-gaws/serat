//! Calendar arithmetic without a dependency.
//!
//! Dates in the vault are derived from the seed, never from the system clock —
//! otherwise "byte-identical output from one seed" (NFR-MNT-02) would be false
//! the moment the day rolled over.

/// Days from the civil epoch 1970-01-01 to `(y, m, d)`. Hinnant's algorithm.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // Mar = 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD`.
pub fn iso_date(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DDTHH:MM:SS` — local-naive, no zone, which is what note
/// frontmatter written by hand actually looks like.
pub fn iso_datetime(day: i64, seconds_of_day: u32) -> String {
    let (y, m, d) = civil_from_days(day);
    let h = seconds_of_day / 3600;
    let mi = (seconds_of_day % 3600) / 60;
    let s = seconds_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
}

/// Mon..Sun for a civil day number (1970-01-01 was a Thursday).
pub fn weekday(day: i64) -> &'static str {
    const NAMES: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    NAMES[(day.rem_euclid(7)) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(iso_date(days_from_civil(2026, 9, 3)), "2026-09-03");
        assert_eq!(weekday(days_from_civil(1970, 1, 1)), "Thu");
        assert_eq!(weekday(days_from_civil(2026, 9, 3)), "Thu");
        // Leap-year boundary, the classic off-by-one.
        assert_eq!(iso_date(days_from_civil(2024, 2, 28) + 1), "2024-02-29");
        assert_eq!(iso_date(days_from_civil(2100, 2, 28) + 1), "2100-03-01");
    }

    #[test]
    fn every_day_of_a_decade_round_trips() {
        let start = days_from_civil(2020, 1, 1);
        let end = days_from_civil(2030, 1, 1);
        for day in start..end {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
    }
}

//! AIRAC cycles (OA-ASP-072). **Core** (B1, B7): pure date arithmetic. NASR (effective date) and
//! CIFP (header cycle `YYRR`) are both published on the 28-day AIRAC cycle; this maps a date to
//! the cycle that contains it, so the two sources can be checked against each other.

/// Reference epoch: the AIRAC date of cycle 2601, 2026-01-22. Verified against the files: CIFP
/// 2610 is effective 2026-10-01 (nine cycles on) and 2611 is effective 2026-10-29.
pub const AIRAC_EPOCH: (i32, u32, u32) = (2026, 1, 22);
/// Days in one AIRAC cycle.
pub const AIRAC_DAYS: i64 = 28;

/// Years `days_from_civil` accepts, inclusive.
pub const CIVIL_YEARS: (i32, i32) = (1, 9999);

fn is_leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Days in month `m` (1 to 12) of year `y`; `None` for another month.
fn days_in_month(y: i32, m: u32) -> Option<u32> {
    Some(match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + u32::from(is_leap(y)),
        _ => return None,
    })
}

/// Days since 1970-01-01 of a proleptic Gregorian date; `None` unless the year is within
/// [`CIVIL_YEARS`], the month is 1 to 12 and the day exists in that month.
fn days_from_civil(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(CIVIL_YEARS.0..=CIVIL_YEARS.1).contains(&y) || d < 1 || d > days_in_month(y, m)? {
        return None;
    }
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The year of a day count (days since 1970-01-01).
fn year_of_days(z: i64) -> i64 {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(m <= 2)
}

/// The AIRAC cycle that contains the date `yyyymmdd`, as `YYRR` (2611 is release 11 of 2026);
/// `None` for an invalid date or a year outside 2000 to 2099.
pub fn airac_cycle(yyyymmdd: u32) -> Option<u32> {
    let (y, m, d) = (
        i32::try_from(yyyymmdd / 10_000).ok()?,
        (yyyymmdd / 100) % 100,
        yyyymmdd % 100,
    );
    if !(2000..=2099).contains(&y) {
        return None;
    }
    let day = days_from_civil(y, m, d)?;
    let epoch = days_from_civil(AIRAC_EPOCH.0, AIRAC_EPOCH.1, AIRAC_EPOCH.2)?;
    let start = epoch + (day - epoch).div_euclid(AIRAC_DAYS) * AIRAC_DAYS;
    let year = year_of_days(start);
    let jan1 = days_from_civil(i32::try_from(year).ok()?, 1, 1)?;
    let first = epoch + (jan1 - epoch + AIRAC_DAYS - 1).div_euclid(AIRAC_DAYS) * AIRAC_DAYS;
    let release = (start - first) / AIRAC_DAYS + 1;
    u32::try_from((year - 2000) * 100 + release).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-ASP-078
    #[test]
    fn days_from_civil_matches_known_dates_and_refuses_impossible_ones() {
        for (y, m, d, days) in [
            (1970, 1, 1, 0),
            (1969, 12, 31, -1),
            (2000, 2, 29, 11_016),
            (2000, 3, 1, 11_017),
            (2024, 2, 29, 19_782),
            (2026, 1, 22, 20_475),
            (2026, 10, 29, 20_755),
            (1, 1, 1, -719_162),
            (9999, 12, 31, 2_932_896),
            (1900, 3, 1, -25_508),
        ] {
            assert_eq!(days_from_civil(y, m, d), Some(days), "{y}-{m}-{d}");
        }
        for (y, m, d) in [
            (1900, 2, 29),
            (2100, 2, 29),
            (2023, 2, 29),
            (2026, 4, 31),
            (2026, 0, 1),
            (2026, 13, 1),
            (2026, 1, 0),
            (2026, 1, 32),
            (0, 1, 1),
            (10_000, 1, 1),
            (-5, 6, 1),
            (i32::MAX, 1, 1),
        ] {
            assert_eq!(days_from_civil(y, m, d), None, "{y}-{m}-{d}");
        }
    }

    // REQ: OA-ASP-078
    #[test]
    fn airac_cycle_refuses_dates_that_do_not_exist() {
        assert_eq!(airac_cycle(20_261_029), Some(2611));
        assert_eq!(airac_cycle(20_260_431), None);
        assert_eq!(airac_cycle(20_260_229), None);
        assert!(airac_cycle(20_240_229).is_some());
        assert_eq!(airac_cycle(20_261_300), None);
    }
}

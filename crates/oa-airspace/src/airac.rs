//! AIRAC cycles (OA-ASP-072). **Core** (B1): pure date arithmetic. NASR (effective date) and
//! CIFP (header cycle `YYRR`) are both published on the 28-day AIRAC cycle; this maps a date to
//! the cycle that contains it, so the two sources can be checked against each other.

/// Reference epoch: the AIRAC date of cycle 2601, 2026-01-22. Verified against the files: CIFP
/// 2610 is effective 2026-10-01 (nine cycles on) and 2611 is effective 2026-10-29.
pub const AIRAC_EPOCH: (i32, u32, u32) = (2026, 1, 22);
/// Days in one AIRAC cycle.
pub const AIRAC_DAYS: i64 = 28;

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
    if !(1..=12).contains(&m) || !(2000..=2099).contains(&y) {
        return None;
    }
    let next = if m == 12 {
        days_from_civil(y + 1, 1, 1)
    } else {
        days_from_civil(y, m + 1, 1)
    };
    if d < 1 || i64::from(d) > next - days_from_civil(y, m, 1) {
        return None;
    }
    let epoch = days_from_civil(AIRAC_EPOCH.0, AIRAC_EPOCH.1, AIRAC_EPOCH.2);
    let day = days_from_civil(y, m, d);
    let start = epoch + (day - epoch).div_euclid(AIRAC_DAYS) * AIRAC_DAYS;
    let year = year_of_days(start);
    let jan1 = days_from_civil(i32::try_from(year).ok()?, 1, 1);
    let first = epoch + (jan1 - epoch + AIRAC_DAYS - 1).div_euclid(AIRAC_DAYS) * AIRAC_DAYS;
    let release = (start - first) / AIRAC_DAYS + 1;
    u32::try_from((year - 2000) * 100 + release).ok()
}

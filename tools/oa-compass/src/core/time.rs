//! Core (pure, B1): UTC time of day in 1/128 s and the calendar. A day is a count of days since
//! 1970-01-01; a date is checked against the calendar before it is used (E8).

/// Counts of 1/128 s in one UTC day (I062/070 LSB, 5.2.5).
pub const UNITS_PER_DAY: i64 = 86_400 * 128;

/// A point in time as the UTC day and the I062/070 count of 1/128 s since midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct DayTime {
    /// Days since 1970-01-01 (UTC).
    pub day: i64,
    /// 1/128 s since 00:00:00 UTC of `day`, below [`UNITS_PER_DAY`].
    pub units: u32,
}

/// Floor division and the matching non-negative remainder, for a positive divisor.
fn floor_div(a: i64, d: i64) -> (i64, i64) {
    let q = a / d;
    let r = a % d;
    if r < 0 {
        (q - 1, r + d)
    } else {
        (q, r)
    }
}

/// The day and time of day of `t_ns` (Unix nanoseconds), the time rounded to the nearest 1/128 s,
/// halves up. The day is that of the rounded time, so a time within 1/256 s before midnight
/// belongs to the next day and never to a time of day of 24:00:00. `None` if the count does not
/// fit (it always does for an `i64` of nanoseconds; kept for the checked form).
pub fn day_time_of_ns(t_ns: i64) -> Option<DayTime> {
    // t_ns * 128 does not fit an i64 for late dates; 128-bit arithmetic cannot overflow here.
    let scaled = i128::from(t_ns) * 128 + 500_000_000;
    let q = scaled / 1_000_000_000;
    let units = i64::try_from(if scaled % 1_000_000_000 < 0 { q - 1 } else { q }).ok()?;
    let (day, rem) = floor_div(units, UNITS_PER_DAY);
    Some(DayTime {
        day,
        units: u32::try_from(rem).ok()?,
    })
}

/// Whether the instant `t_ns` is on UTC day `day` by its raw time (floor division) or by its
/// rounded time ([`day_time_of_ns`], the day an exported row is filed under): a time within 1/256 s
/// before midnight is on the day before by its raw time and on the day after once rounded, and
/// both count.
pub fn touches_day(t_ns: i64, day: i64) -> bool {
    floor_div(t_ns, 86_400_000_000_000).0 == day
        || day_time_of_ns(t_ns).is_some_and(|d| d.day == day)
}

/// Days since 1970-01-01 of the civil date, `None` unless the date exists in the proleptic
/// Gregorian calendar (year 1 to 9999).
pub fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let dim = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    };
    if day < 1 || day > dim {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = i64::from(year) - i64::from(month <= 2);
    let era = floor_div(y, 400).0;
    let yoe = y - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The civil date of a day count; the inverse of [`days_from_civil`]. `None` outside years 1 to
/// 9999.
pub fn civil_from_days(days: i64) -> Option<(i32, u32, u32)> {
    let z = days.checked_add(719_468)?;
    let (era, doe) = floor_div(z, 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).ok()?;
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).ok()?;
    let year = i32::try_from(y + i64::from(m <= 2)).ok()?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    Some((year, m, d))
}

//! Strict ISO-8601 UTC timestamps to Unix milliseconds. Core (pure); allocates nothing.
//!
//! Accepted form, exactly: `YYYY-MM-DDTHH:MM:SS` then an optional `.` with one to three digits
//! then `Z`. Nothing else: no offsets, no lower case, no spaces, no leap second (`:60`), no
//! date-only or basic (compact) form. The date must exist (leap years honoured) and the year
//! lies in 1970..=9999. A fraction of one or two digits is read as tenths or hundredths of a
//! second; more than three digits is refused rather than rounded.

/// Why a timestamp was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeError {
    /// Wrong length or a separator in the wrong place.
    Shape,
    /// A non-digit where a digit belongs.
    Digit,
    /// A fraction of more than three digits.
    Fraction,
    /// Month, day, hour, minute or second out of range, or the year outside 1970..=9999.
    Range,
}

fn digit(b: Option<&u8>) -> Result<i64, TimeError> {
    match b {
        Some(&d) if d.is_ascii_digit() => Ok(i64::from(d - b'0')),
        Some(_) => Err(TimeError::Digit),
        None => Err(TimeError::Shape),
    }
}

fn num(s: &[u8], at: usize, n: usize) -> Result<i64, TimeError> {
    let mut v = 0i64;
    for i in at..at + n {
        v = v * 10 + digit(s.get(i))?;
    }
    Ok(v)
}

fn sep(s: &[u8], at: usize, want: u8) -> Result<(), TimeError> {
    if s.get(at) == Some(&want) {
        Ok(())
    } else {
        Err(TimeError::Shape)
    }
}

/// Whether `y` is a Gregorian leap year.
pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in month `m` (1..=12) of year `y`; 0 for any other month.
pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses a timestamp (see the module comment) to Unix milliseconds.
pub fn parse_iso_ms(s: &[u8]) -> Result<i64, TimeError> {
    // The shortest form is `YYYY-MM-DDTHH:MM:SSZ` (20 bytes); the longest valid one has `.mmm`
    // (24). Longer input is read on so that an over-long fraction is named as such.
    if s.len() < 20 || s.len() > 64 {
        return Err(TimeError::Shape);
    }
    sep(s, 4, b'-')?;
    sep(s, 7, b'-')?;
    sep(s, 10, b'T')?;
    sep(s, 13, b':')?;
    sep(s, 16, b':')?;
    let y = num(s, 0, 4)?;
    let mo = num(s, 5, 2)?;
    let d = num(s, 8, 2)?;
    let h = num(s, 11, 2)?;
    let mi = num(s, 14, 2)?;
    let sec = num(s, 17, 2)?;
    let mut ms = 0i64;
    let mut at = 19;
    if s.get(at) == Some(&b'.') {
        let digits = s.len().saturating_sub(at + 2);
        if digits == 0 {
            return Err(TimeError::Shape);
        }
        if digits > 3 {
            return Err(TimeError::Fraction);
        }
        let frac = num(s, at + 1, digits)?;
        ms = frac * 10i64.pow(3 - digits as u32);
        at += 1 + digits;
    }
    sep(s, at, b'Z')?;
    if s.len() != at + 1 {
        return Err(TimeError::Shape);
    }
    if !(1970..=9999).contains(&y)
        || !(1..=12).contains(&mo)
        || d < 1
        || d > days_in_month(y, mo)
        || h > 23
        || mi > 59
        || sec > 59
    {
        return Err(TimeError::Range);
    }
    let days = days_from_civil(y, mo, d);
    Ok(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

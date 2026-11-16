//! Strict scalar parsers for AIXM and FNS values. Core (pure); allocate nothing. Every parser
//! returns `None` for anything it does not fully understand: no defaults, no partial reads. The
//! only leniency is the surrounding white space that [`trim`] removes, which the caller opts in
//! to for short code fields (a real purpose code reads `BO ` with a trailing space).

use crate::fields::{ALT_UNLIMITED, REF_AGL, REF_FL, REF_MSL};

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`), for years 1970
/// to 9999 and dates that exist; `None` for any other year, month or day.
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// `n` ASCII digits starting at `at`, as a number.
fn digits(s: &[u8], at: usize, n: usize) -> Option<i64> {
    let part = s.get(at..at.checked_add(n)?)?;
    let mut v = 0i64;
    for &b in part {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(i64::from(b - b'0'))?;
    }
    Some(v)
}

/// A date and time of day that must exist, as Unix milliseconds (no seconds fraction).
fn civil_ms(year: i64, mon: i64, day: i64, hour: i64, min: i64, sec: i64) -> Option<i64> {
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&mon)
        || day < 1
        || day > days_in_month(year, mon)
        || hour > 23
        || min > 59
        || sec > 59
    {
        return None;
    }
    let days = days_from_civil(year, mon, day)?;
    Some(((days * 24 + hour) * 60 + min) * 60_000 + sec * 1000)
}

/// Strict ISO-8601 UTC timestamp `YYYY-MM-DDThh:mm:ss[.f{1,3}]Z` as Unix milliseconds: the
/// designator must be `Z` (no offsets), the date must exist, the year is 1970 to 9999, the
/// second is 0 to 59 (no leap second), and a fraction of more than three digits is refused
/// rather than rounded.
pub fn parse_iso_ms(s: &[u8]) -> Option<i64> {
    if *s.get(4)? != b'-' || *s.get(7)? != b'-' || *s.get(10)? != b'T' {
        return None;
    }
    if *s.get(13)? != b':' || *s.get(16)? != b':' {
        return None;
    }
    let mut ms = 0i64;
    let mut at = 19;
    if s.get(at) == Some(&b'.') {
        let n = s.len().checked_sub(at + 2)?;
        if n == 0 || n > 3 {
            return None;
        }
        ms = digits(s, at + 1, n)? * 10i64.pow(3 - n as u32);
        at += 1 + n;
    }
    if s.get(at) != Some(&b'Z') || at + 1 != s.len() {
        return None;
    }
    let base = civil_ms(
        digits(s, 0, 4)?,
        digits(s, 5, 2)?,
        digits(s, 8, 2)?,
        digits(s, 11, 2)?,
        digits(s, 14, 2)?,
        digits(s, 17, 2)?,
    )?;
    Some(base + ms)
}

/// The compact NOTAM time `YYYYMMDDhhmm` (twelve digits, UTC, whole minutes) as Unix
/// milliseconds.
pub fn parse_compact_ms(s: &[u8]) -> Option<i64> {
    if s.len() != 12 {
        return None;
    }
    civil_ms(
        digits(s, 0, 4)?,
        digits(s, 4, 2)?,
        digits(s, 6, 2)?,
        digits(s, 8, 2)?,
        digits(s, 10, 2)?,
        0,
    )
}

/// How the end of the effective period is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffEnd {
    /// `YYYYMMDDhhmm`.
    At(i64),
    /// `YYYYMMDDhhmmEST`, an estimated end.
    Estimated(i64),
    /// `PERM`.
    Permanent,
}

/// The `effectiveEnd` forms: a compact time, a compact time followed by `EST`, or `PERM`. The
/// last two are the ICAO NOTAM item C forms; neither occurs in the surveyed data.
pub fn parse_eff_end(s: &[u8]) -> Option<EffEnd> {
    if s == b"PERM" {
        return Some(EffEnd::Permanent);
    }
    match s.strip_suffix(b"EST") {
        Some(t) => parse_compact_ms(t).map(EffEnd::Estimated),
        None => parse_compact_ms(s).map(EffEnd::At),
    }
}

/// An unsigned decimal integer, digits only, at most `max_digits` of them.
pub fn parse_uint(s: &[u8], max_digits: usize) -> Option<u64> {
    if s.is_empty() || s.len() > max_digits || max_digits > 19 {
        return None;
    }
    let mut v: u64 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    Some(v)
}

/// A UUID in its 8-4-4-4-12 hexadecimal form as sixteen bytes (either letter case).
pub fn parse_uuid(s: &[u8]) -> Option<[u8; 16]> {
    if s.len() != 36 {
        return None;
    }
    let mut v: u128 = 0;
    for (i, &b) in s.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if b != b'-' {
                return None;
            }
            continue;
        }
        v = (v << 4) | u128::from(char::from(b).to_digit(16)?);
    }
    Some(v.to_be_bytes())
}

/// Copies `s` into a zero-padded array when it is 1 to `N` ASCII letters, digits or the
/// characters in `extra`.
pub fn parse_code<const N: usize>(s: &[u8], extra: &[u8]) -> Option<[u8; N]> {
    if s.is_empty() || s.len() > N {
        return None;
    }
    let mut out = [0u8; N];
    for (o, &b) in out.iter_mut().zip(s) {
        if !(b.is_ascii_alphanumeric() || extra.contains(&b)) {
            return None;
        }
        *o = b;
    }
    Some(out)
}

/// `s` without leading and trailing ASCII white space.
pub fn trim(s: &[u8]) -> &[u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && s.get(a).is_some_and(u8::is_ascii_whitespace) {
        a += 1;
    }
    while b > a && s.get(b - 1).is_some_and(u8::is_ascii_whitespace) {
        b -= 1;
    }
    s.get(a..b).unwrap_or(&[])
}

/// Altitude range kept, feet: the model's, plus the flight-level ceiling `FL999`.
const ALT_MIN_FT: i64 = -2_000;
const ALT_MAX_FT: i64 = 100_000;

fn feet(v: i64) -> Option<i32> {
    if (ALT_MIN_FT..=ALT_MAX_FT).contains(&v) {
        i32::try_from(v).ok()
    } else {
        None
    }
}

/// A flight level as three digits (`000` to `999`): feet and the [`REF_FL`] reference.
pub fn parse_fl3(s: &[u8]) -> Option<(i32, u8)> {
    if s.len() != 3 {
        return None;
    }
    Some((feet(i64::try_from(parse_uint(s, 3)?).ok()? * 100)?, REF_FL))
}

/// An altitude limit as the FNS writes it, to feet and a reference:
/// * `SFC` and `GND`: 0 feet above ground;
/// * `UNL`: [`ALT_UNLIMITED`] with the flight-level reference;
/// * `FLnnn`: a flight level;
/// * `nnnFT AGL` or `nnnM AGL`, and the same with `AMSL` or `MSL`: feet or metres (3.048 dm
///   per foot, rounded to the nearest foot) above ground or above mean sea level.
///
/// The forms are those of the surveyed data; their meaning is unconfirmed.
pub fn parse_limit(s: &[u8]) -> Option<(i32, u8)> {
    match s {
        b"SFC" | b"GND" => return Some((0, REF_AGL)),
        b"UNL" => return Some((ALT_UNLIMITED, REF_FL)),
        _ => {}
    }
    if let Some(d) = s.strip_prefix(b"FL") {
        return parse_fl3(d);
    }
    let (num, rest) = {
        let n = s.iter().take_while(|b| b.is_ascii_digit()).count();
        (s.get(..n)?, s.get(n..)?)
    };
    let v = i64::try_from(parse_uint(num, 6)?).ok()?;
    let (unit, reference) = match rest {
        b"FT AGL" => (1, REF_AGL),
        b"FT AMSL" | b"FT MSL" => (1, REF_MSL),
        b"M AGL" => (2, REF_AGL),
        b"M AMSL" | b"M MSL" => (2, REF_MSL),
        _ => return None,
    };
    let ft = if unit == 1 {
        v
    } else {
        // metres to feet: v * 10000 / 3048, rounded half up (v is not negative)
        (v.checked_mul(20_000)?.checked_add(3048)?) / 6096
    };
    Some((feet(ft)?, reference))
}

/// A coordinate pair `DDMM[NS]DDDMM[EW]` (whole minutes, as the NOTAM centre is written) to
/// latitude and longitude in 1e-7 degree. Minutes are 00 to 59, latitude at most 90 degrees and
/// longitude at most 180, and a pole or the antimeridian takes no minutes.
pub fn parse_coord(s: &[u8]) -> Option<(i32, i32)> {
    if s.len() != 11 {
        return None;
    }
    let lat_d = digits(s, 0, 2)?;
    let lat_m = digits(s, 2, 2)?;
    let lon_d = digits(s, 5, 3)?;
    let lon_m = digits(s, 8, 2)?;
    if lat_m > 59 || lon_m > 59 || lat_d > 90 || lon_d > 180 {
        return None;
    }
    let lat_neg = match *s.get(4)? {
        b'N' => false,
        b'S' => true,
        _ => return None,
    };
    let lon_neg = match *s.get(10)? {
        b'E' => false,
        b'W' => true,
        _ => return None,
    };
    if (lat_d == 90 && lat_m != 0) || (lon_d == 180 && lon_m != 0) {
        return None;
    }
    // minutes to 1e-7 degree: m * 1e7 / 60, rounded half up
    let e7 = |d: i64, m: i64, neg: bool| -> Option<i32> {
        let v = d
            .checked_mul(60)?
            .checked_add(m)?
            .checked_mul(10_000_000)?
            .checked_add(30)?
            / 60;
        i32::try_from(if neg { -v } else { v }).ok()
    };
    Some((e7(lat_d, lat_m, lat_neg)?, e7(lon_d, lon_m, lon_neg)?))
}

/// Strict decimal `[-]digits[.digits]` scaled by `10^scale` and rounded half away from zero.
/// At most 12 integer digits and 20 fraction digits (the feed writes 13 to 16; digits past the rounding digit are validated and dropped); no sign `+`, no exponent, no spaces.
pub fn parse_scaled(s: &[u8], scale: u32) -> Option<i64> {
    let (neg, body) = match s.split_first()? {
        (b'-', rest) => (true, rest),
        _ => (false, s),
    };
    let (int, frac) = match body.iter().position(|&b| b == b'.') {
        Some(p) => (body.get(..p)?, body.get(p + 1..)?),
        None => (body, &[][..]),
    };
    if int.is_empty() || int.len() > 12 || frac.len() > 20 {
        return None;
    }
    if body.contains(&b'.') && frac.is_empty() {
        return None;
    }
    let mut v: i64 = 0;
    for &b in int {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(i64::from(b - b'0'))?;
    }
    let mut round_up = false;
    for (i, &b) in frac.iter().enumerate() {
        if !b.is_ascii_digit() {
            return None;
        }
        if (i as u32) < scale {
            v = v.checked_mul(10)?.checked_add(i64::from(b - b'0'))?;
        } else if i as u32 == scale {
            round_up = b >= b'5';
        }
    }
    for _ in frac.len()..scale as usize {
        v = v.checked_mul(10)?;
    }
    if round_up {
        v = v.checked_add(1)?;
    }
    Some(if neg { -v } else { v })
}

/// A latitude and a longitude as two decimal numbers in degrees to 1e-7 degree, range checked
/// (latitude within 90 degrees, longitude within 180).
pub fn parse_lat_lon(lat: &[u8], lon: &[u8]) -> Option<(i32, i32)> {
    let la = parse_scaled(lat, 7)?;
    let lo = parse_scaled(lon, 7)?;
    if !(-900_000_000..=900_000_000).contains(&la)
        || !(-1_800_000_000..=1_800_000_000).contains(&lo)
    {
        return None;
    }
    Some((i32::try_from(la).ok()?, i32::try_from(lo).ok()?))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod calendar_tests {
    use super::*;

    // (year, month, day, Unix day or None): the same table is applied in every crate that has
    // its own calendar helper.
    const VECTORS: [(i64, i64, i64, Option<i64>); 20] = [
        (1970, 1, 1, Some(0)),
        (2000, 2, 29, Some(11_016)),
        (2024, 2, 29, Some(19_782)),
        (2026, 11, 15, Some(20_772)),
        (9999, 12, 31, Some(2_932_896)),
        (2023, 2, 29, None),
        (2100, 2, 29, None),
        (2026, 4, 31, None),
        (2026, 6, 31, None),
        (2026, 9, 31, None),
        (2026, 1, 32, None),
        (2026, 1, 0, None),
        (2026, 0, 1, None),
        (2026, 13, 1, None),
        (-1, 1, 1, None),
        (1969, 12, 31, None),
        (10_000, 1, 1, None),
        (i64::MAX, 1, 1, None),
        (i64::MIN, 1, 1, None),
        (2026, i64::MAX, 1, None),
    ];

    // REQ: OA-NOTAM-025
    #[test]
    fn impossible_dates_and_years_out_of_range_are_refused() {
        for (y, m, d, want) in VECTORS {
            assert_eq!(days_from_civil(y, m, d), want, "{y}-{m}-{d}");
        }
        assert_eq!(civil_ms(2026, 2, 29, 0, 0, 0), None);
        assert_eq!(civil_ms(2026, 4, 31, 0, 0, 0), None);
        assert_eq!(civil_ms(2026, 11, 15, 24, 0, 0), None);
        assert_eq!(civil_ms(1969, 12, 31, 23, 59, 59), None);
        assert_eq!(civil_ms(10_000, 1, 1, 0, 0, 0), None);
        assert_eq!(civil_ms(2026, 11, 15, 0, 0, 0), Some(20_772 * 86_400_000));
    }
}

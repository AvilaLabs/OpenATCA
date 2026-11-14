//! Strict scalar parsers for FIXM attribute and text values. Core (pure); allocate nothing.
//! Every parser returns `None` for anything it does not fully understand: no trimming, no
//! defaults, no partial reads.

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
        v = v * 10 + i64::from(b - b'0');
    }
    Some(v)
}

/// Strict ISO-8601 UTC timestamp `YYYY-MM-DDThh:mm:ss[.f{1,9}]Z` as Unix milliseconds. The
/// designator must be `Z` (no offsets), the date must exist, the year is 1970 to 9999, the
/// second is 0 to 59 (no leap second), and a fraction finer than a millisecond is truncated.
pub fn parse_iso_ms(s: &[u8]) -> Option<i64> {
    let year = digits(s, 0, 4)?;
    if *s.get(4)? != b'-' || *s.get(7)? != b'-' || *s.get(10)? != b'T' {
        return None;
    }
    if *s.get(13)? != b':' || *s.get(16)? != b':' {
        return None;
    }
    let mon = digits(s, 5, 2)?;
    let day = digits(s, 8, 2)?;
    let hour = digits(s, 11, 2)?;
    let min = digits(s, 14, 2)?;
    let sec = digits(s, 17, 2)?;
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
    let mut ms = 0i64;
    let mut at = 19;
    if s.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while s.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let n = at - start;
        if n == 0 || n > 9 {
            return None;
        }
        let mut scale = 100;
        for &b in s.get(start..start + n.min(3))? {
            ms += i64::from(b - b'0') * scale;
            scale /= 10;
        }
    }
    if s.get(at) != Some(&b'Z') || at + 1 != s.len() {
        return None;
    }
    let days = days_from_civil(year, mon, day);
    Some(((days * 24 + hour) * 60 + min) * 60_000 + sec * 1000 + ms)
}

/// Strict decimal `[-]digits[.digits]` scaled by `10^scale` and rounded half away from zero.
/// At most 12 integer digits and 12 fraction digits; no sign `+`, no exponent, no spaces.
pub fn parse_scaled(s: &[u8], scale: u32) -> Option<i64> {
    let (neg, body) = match s.split_first()? {
        (b'-', rest) => (true, rest),
        _ => (false, s),
    };
    let (int, frac) = match body.iter().position(|&b| b == b'.') {
        Some(p) => (body.get(..p)?, body.get(p + 1..)?),
        None => (body, &[][..]),
    };
    if int.is_empty() || int.len() > 12 || frac.len() > 12 {
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
        v = v * 10 + i64::from(b - b'0');
    }
    let mut round_up = false;
    for (i, &b) in frac.iter().enumerate() {
        if !b.is_ascii_digit() {
            return None;
        }
        if (i as u32) < scale {
            v = v * 10 + i64::from(b - b'0');
        } else if i as u32 == scale {
            round_up = b >= b'5';
        }
    }
    for _ in frac.len()..scale as usize {
        v *= 10;
    }
    if round_up {
        v += 1;
    }
    Some(if neg { -v } else { v })
}

/// `v * num / den` rounded half away from zero (`den` positive).
pub fn rescale(v: i64, num: i64, den: i64) -> Option<i64> {
    let p = v.checked_mul(num)?;
    let half = den / 2;
    let r = if p >= 0 {
        p.checked_add(half)?
    } else {
        p.checked_sub(half)?
    };
    Some(r / den)
}

/// A UUID in its 8-4-4-4-12 hexadecimal form as a 128-bit number (either letter case).
pub fn parse_uuid(s: &[u8]) -> Option<u128> {
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
    Some(v)
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

/// A transponder code: exactly four octal digits, kept as the digits read as a decimal number.
pub fn parse_beacon(s: &[u8]) -> Option<u16> {
    if s.len() != 4 || s.iter().any(|b| !(b'0'..=b'7').contains(b)) {
        return None;
    }
    let mut v = 0u16;
    for &b in s {
        v = v * 10 + u16::from(b - b'0');
    }
    Some(v)
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

/// `s` without leading and trailing ASCII whitespace.
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

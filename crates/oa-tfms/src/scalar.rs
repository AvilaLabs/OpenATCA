//! Strict scalar parsers for attribute and element values. Core (pure); allocate nothing. Every
//! parser returns the [`FieldErrorKind`] it refuses with; none trims, defaults or reads part of
//! a value.

use crate::fields::FieldErrorKind;
use crate::rows::{spd_unit, NO_NUM};
use crate::time::parse_iso_ms;

/// A name: printable ASCII without spaces (0x21 to 0x7E), at most `N` bytes, zero padded.
pub fn parse_name<const N: usize>(v: &[u8]) -> Result<[u8; N], FieldErrorKind> {
    let mut out = [0u8; N];
    if v.len() > N {
        return Err(FieldErrorKind::TooLong);
    }
    for (d, &b) in out.iter_mut().zip(v) {
        if !(0x21..=0x7e).contains(&b) {
            return Err(FieldErrorKind::BadChar);
        }
        *d = b;
    }
    Ok(out)
}

/// Words: printable ASCII with single interior spaces (no leading, trailing or doubled space),
/// at most `N` bytes, zero padded.
pub fn parse_words<const N: usize>(v: &[u8]) -> Result<[u8; N], FieldErrorKind> {
    let mut out = [0u8; N];
    if v.len() > N {
        return Err(FieldErrorKind::TooLong);
    }
    let mut prev = b' ';
    for (d, &b) in out.iter_mut().zip(v) {
        if !(0x20..=0x7e).contains(&b) || (b == b' ' && prev == b' ') {
            return Err(FieldErrorKind::BadChar);
        }
        prev = b;
        *d = b;
    }
    if prev == b' ' && !v.is_empty() {
        return Err(FieldErrorKind::BadChar);
    }
    Ok(out)
}

/// Plain decimal digits, one to `max_len` of them.
pub fn parse_digits(v: &[u8], max_len: usize) -> Result<u32, FieldErrorKind> {
    if v.is_empty() || v.len() > max_len || !v.iter().all(u8::is_ascii_digit) {
        return Err(FieldErrorKind::Format);
    }
    let mut n = 0u32;
    for &b in v {
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(u32::from(b - b'0')))
            .ok_or(FieldErrorKind::Format)?;
    }
    Ok(n)
}

/// Like [`parse_digits`], for an identifier of up to 19 digits that does not fit `u32` (the
/// flight reference and the restriction identifiers are ten digits wide).
pub fn parse_digits64(v: &[u8], max_len: usize) -> Result<u64, FieldErrorKind> {
    if v.is_empty() || v.len() > max_len || !v.iter().all(u8::is_ascii_digit) {
        return Err(FieldErrorKind::Format);
    }
    let mut n = 0u64;
    for &b in v {
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(b - b'0')))
            .ok_or(FieldErrorKind::Format)?;
    }
    Ok(n)
}

/// A timestamp, [`parse_iso_ms`].
pub fn parse_time(v: &[u8]) -> Result<i64, FieldErrorKind> {
    parse_iso_ms(v).map_err(FieldErrorKind::Time)
}

/// `true` or `false`, lower case, as the tri-state code 2 or 1.
pub fn parse_bool(v: &[u8]) -> Result<u16, FieldErrorKind> {
    match v {
        b"true" => Ok(2),
        b"false" => Ok(1),
        _ => Err(FieldErrorKind::Vocabulary),
    }
}

/// A word from `names` (index 0 is the placeholder for "absent" and never matches); returns its
/// index.
pub fn parse_vocab(v: &[u8], names: &[&str]) -> Result<u8, FieldErrorKind> {
    names
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, n)| n.as_bytes() == v)
        .and_then(|(i, _)| u8::try_from(i).ok())
        .ok_or(FieldErrorKind::Vocabulary)
}

/// One upper-case letter.
pub fn parse_letter(v: &[u8]) -> Result<u8, FieldErrorKind> {
    match v {
        [b] if b.is_ascii_uppercase() => Ok(*b),
        [_] => Err(FieldErrorKind::BadChar),
        _ => Err(FieldErrorKind::Format),
    }
}

/// An altitude as the feed writes it: one to three digits, in hundreds of feet (the survey's
/// reading, unconfirmed), then optionally one upper-case letter. Returns feet and the letter
/// (0 when absent).
pub fn parse_altitude(v: &[u8]) -> Result<(i32, u8), FieldErrorKind> {
    let (digits, sfx) = match v.split_last() {
        Some((&l, rest)) if l.is_ascii_uppercase() => (rest, l),
        _ => (v, 0),
    };
    let n = parse_digits(digits, 3)?;
    let ft = i32::try_from(n * 100).map_err(|_| FieldErrorKind::Format)?;
    Ok((ft, sfx))
}

/// A whole number of knots or similar: one to four digits.
pub fn parse_speed(v: &[u8]) -> Result<i32, FieldErrorKind> {
    i32::try_from(parse_digits(v, 4)?).map_err(|_| FieldErrorKind::Format)
}

/// A speed with its unit, in thousandths: `(value, unit)` with a [`spd_unit`] code. One to four
/// digits, or two to four digits and a decimal, is knots; one digit and two or three decimals
/// (`0.82`) and the `M` form (`M082`, hundredths) are a Mach number; `N` and up to five digits
/// (`N0450`) is knots. Any other shape built of digits, `.`, `M` and `N` is
/// [`spd_unit::UNKNOWN`] with the value kept when it reads as a number and [`NO_NUM`] otherwise.
/// Anything with another byte is a format error (the message is refused).
pub fn parse_speed_unit(v: &[u8]) -> Result<(i32, u8), FieldErrorKind> {
    if v.is_empty()
        || !v
            .iter()
            .all(|&b| b.is_ascii_digit() || matches!(b, b'.' | b'M' | b'N'))
    {
        return Err(FieldErrorKind::Format);
    }
    let unknown = |milli: Option<i32>| Ok((milli.unwrap_or(NO_NUM), spd_unit::UNKNOWN));
    let (prefix, body) = match v.split_first() {
        Some((&p @ (b'M' | b'N'), rest)) => (p, rest),
        _ => (0, v),
    };
    let (int, frac, dot) = match body.iter().position(|&b| b == b'.') {
        Some(i) => (
            body.get(..i).unwrap_or(&[]),
            body.get(i + 1..).unwrap_or(&[]),
            true,
        ),
        None => (body, &[][..], false),
    };
    let digits = |d: &[u8]| !d.is_empty() && d.iter().all(u8::is_ascii_digit);
    if !digits(int) || (dot && !digits(frac)) || int.len() > 6 || frac.len() > 3 {
        return unknown(None);
    }
    let i = i64::from(parse_digits(int, 6)?);
    let f = if dot {
        i64::from(parse_digits(frac, 3)?) * 10i64.pow(3 - frac.len() as u32)
    } else {
        0
    };
    let milli = i32::try_from(i * 1000 + f).ok();
    match (prefix, dot, int.len(), frac.len()) {
        // `M082` is hundredths of a Mach number
        (b'M', false, 3, _) => Ok((
            i32::try_from(i * 10).map_err(|_| FieldErrorKind::Format)?,
            spd_unit::MACH,
        )),
        (b'N', false, 1..=5, _) => Ok((milli.unwrap_or(NO_NUM), spd_unit::KNOTS)),
        (0, false, 1..=4, _) | (0, true, 2..=4, 1..=3) => {
            Ok((milli.unwrap_or(NO_NUM), spd_unit::KNOTS))
        }
        (0, true, 1, 2..=3) => Ok((milli.unwrap_or(NO_NUM), spd_unit::MACH)),
        _ => unknown(if prefix == 0 { milli } else { None }),
    }
}

/// Latitude or longitude as signed decimal degrees to 1e-7 degree: an optional `-`, one to
/// three digits, `.`, one to eighteen digits (the eighth rounds half up, the rest are read but
/// not kept). `max_deg` is the bound (90 or 180).
pub fn parse_decimal_e7(v: &[u8], max_deg: i64) -> Result<i32, FieldErrorKind> {
    let (neg, body) = match v.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, v),
    };
    let dot = body
        .iter()
        .position(|&b| b == b'.')
        .ok_or(FieldErrorKind::Format)?;
    let (int, frac) = (
        body.get(..dot).unwrap_or(&[]),
        body.get(dot + 1..).unwrap_or(&[]),
    );
    let whole = i64::from(parse_digits(int, 3)?);
    if frac.is_empty() || frac.len() > 18 || !frac.iter().all(u8::is_ascii_digit) {
        return Err(FieldErrorKind::Format);
    }
    let mut kept = 0i64;
    for i in 0..7 {
        let d = frac.get(i).map_or(0, |b| i64::from(b - b'0'));
        kept = kept * 10 + d;
    }
    let round = i64::from(frac.get(7).is_some_and(|&b| b >= b'5'));
    let mag = whole * 10_000_000 + kept + round;
    if mag > max_deg * 10_000_000 {
        return Err(FieldErrorKind::Range);
    }
    let v = if neg { -mag } else { mag };
    i32::try_from(v).map_err(|_| FieldErrorKind::Range)
}

/// Degrees, minutes and seconds to 1e-7 degree, rounded to the nearest unit (half up). Minutes
/// and seconds are below 60; the total may not pass `max_deg` degrees.
pub fn dms_e7(
    deg: u32,
    min: u32,
    sec: u32,
    neg: bool,
    max_deg: u32,
) -> Result<i32, FieldErrorKind> {
    if min > 59 || sec > 59 || deg > max_deg || (deg == max_deg && (min > 0 || sec > 0)) {
        return Err(FieldErrorKind::Range);
    }
    let total = i64::from(deg) * 3600 + i64::from(min) * 60 + i64::from(sec);
    // 1e7 / 3600 = 25000 / 9
    let mag = (total * 25_000 + 4) / 9;
    let v = if neg { -mag } else { mag };
    i32::try_from(v).map_err(|_| FieldErrorKind::Range)
}

/// Whether `hay` holds `needle` as a whole word: the byte before and after it (if any) is not
/// an ASCII letter or digit.
pub fn contains_word(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    for start in 0..=hay.len() - needle.len() {
        if hay.get(start..start + needle.len()) != Some(needle) {
            continue;
        }
        let before = start
            .checked_sub(1)
            .and_then(|i| hay.get(i))
            .is_none_or(|b| !b.is_ascii_alphanumeric());
        let after = hay
            .get(start + needle.len())
            .is_none_or(|b| !b.is_ascii_alphanumeric());
        if before && after {
            return true;
        }
    }
    false
}

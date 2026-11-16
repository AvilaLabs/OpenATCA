//! Units, time and identifiers. Core (pure).
//!
//! Units used by the tables (the names carry them):
//! * pressure: hectopascals in tenths (`_dhpa`); the API's `altim` is hPa with one decimal.
//!   `rawOb` altimeters are `A3014` (inches of mercury times 100) or `Q1021` (hPa), converted
//!   with 1 inHg = 33.8639 hPa.
//! * flight level: hundreds of feet (`_fl`), as the API reports it; feet are `_ft`.
//! * temperature: tenths of a degree Celsius (`_dc`) for METAR, whole degrees for PIREP.
//! * visibility: hundredths of a statute mile (`_sm100`); the API's `"10+"` is 1000 with the plus
//!   flag set, meaning "10 or more".
//! * times: Unix seconds (`_s`) or milliseconds (`_ms`), UTC.

/// Hectopascals per inch of mercury (conversion used for `A`-format altimeters).
pub const HPA_PER_INHG: f64 = 33.8639;
/// ISA sea-level pressure, hPa.
pub const ISA_SEA_LEVEL_HPA: f64 = 1013.25;
/// Bound: altimeter setting, hPa. World records are 870.0 and 1086.8; this is a little wider.
pub const ALTIM_MIN_HPA: f64 = 850.0;
/// Bound: altimeter setting, hPa (upper).
pub const ALTIM_MAX_HPA: f64 = 1090.0;
/// Bound: earliest accepted timestamp, 2000-01-01T00:00:00Z.
pub const MIN_TIME_S: i64 = 946_684_800;
/// Bound: latest accepted timestamp, 2100-01-01T00:00:00Z.
pub const MAX_TIME_S: i64 = 4_102_444_800;
/// The sentinel for an absent signed time (seconds or milliseconds).
pub const NO_TIME: i64 = i64::MIN;
/// The sentinel for an absent unsigned 16-bit value.
pub const NONE_U16: u16 = u16::MAX;
/// The sentinel for an absent temperature in tenths of a degree.
pub const NONE_I16: i16 = i16::MIN;
/// The sentinel for an absent altitude in feet.
pub const NO_ALT: i32 = i32::MIN;
/// Station and aircraft-type identifiers are packed into a `u32`; this value means "none".
pub const NO_IDENT: u32 = 0;

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`), for years 1970
/// to 9999 and dates that exist; `None` for any other year, month or day.
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn digits(b: &[u8], at: usize, n: usize) -> Option<i64> {
    let mut v = 0i64;
    for i in at..at.checked_add(n)? {
        let d = *b.get(i)?;
        if !d.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(i64::from(d - b'0'))?;
    }
    Some(v)
}

/// Parses `YYYY-MM-DDTHH:MM:SSZ` with an optional fraction of 1 to 3 digits (`.123`) to Unix
/// milliseconds. The year lies in 2000..2100; nothing else is accepted (no offsets, no `:60`).
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b.len() > 24 {
        return None;
    }
    let ok = b.get(4) == Some(&b'-')
        && b.get(7) == Some(&b'-')
        && b.get(10) == Some(&b'T')
        && b.get(13) == Some(&b':')
        && b.get(16) == Some(&b':');
    if !ok {
        return None;
    }
    let (y, mo, d) = (digits(b, 0, 4)?, digits(b, 5, 2)?, digits(b, 8, 2)?);
    let (h, mi, sec) = (digits(b, 11, 2)?, digits(b, 14, 2)?, digits(b, 17, 2)?);
    if !(2000..2100).contains(&y)
        || d < 1
        || d > days_in_month(y, mo)
        || h > 23
        || mi > 59
        || sec > 59
    {
        return None;
    }
    let mut ms = 0i64;
    let mut at = 19;
    if b.get(at) == Some(&b'.') {
        let n = b.len().checked_sub(21)?;
        if !(1..=3).contains(&n) {
            return None;
        }
        let frac = digits(b, 20, n)?;
        ms = frac * 10i64.pow(3 - n as u32);
        at = 20 + n;
    }
    if b.get(at) != Some(&b'Z') || at + 1 != b.len() {
        return None;
    }
    let days = days_from_civil(y, mo, d)?;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

/// Whether a Unix-seconds timestamp lies in the accepted years.
pub fn time_in_bounds(t_s: i64) -> bool {
    (MIN_TIME_S..MAX_TIME_S).contains(&t_s)
}

/// Packs an identifier of `min_len..=4` upper-case letters or digits into a `u32` (big-endian,
/// zero-padded on the right, so packed values sort like the text). `None` if it does not fit.
pub fn pack_ident(s: &str, min_len: usize) -> Option<u32> {
    let b = s.as_bytes();
    if b.len() < min_len || b.len() > 4 {
        return None;
    }
    let mut v = 0u32;
    for i in 0..4 {
        let c = b.get(i).copied().unwrap_or(0);
        if i < b.len() && !(c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return None;
        }
        v = (v << 8) | u32::from(c);
    }
    Some(v)
}

/// The text of a packed identifier (empty for [`NO_IDENT`]).
pub fn unpack_ident(v: u32) -> String {
    v.to_be_bytes()
        .iter()
        .filter(|&&c| c != 0)
        .map(|&c| c as char)
        .collect()
}

/// An altimeter in hPa to tenths of a hectopascal, `None` outside the bounds.
pub fn hpa_to_dhpa(hpa: f64) -> Option<u16> {
    if !(ALTIM_MIN_HPA..=ALTIM_MAX_HPA).contains(&hpa) {
        return None;
    }
    Some((hpa * 10.0).round() as u16)
}

/// Finds the altimeter group in a METAR text (before `RMK`) and returns it in tenths of hPa.
/// `A3014` is 30.14 inHg; `Q1021` is 1021 hPa. `None` if the text has no altimeter group or the
/// value is outside the bounds. The first group wins.
pub fn raw_altimeter_dhpa(raw: &str) -> Option<u16> {
    for tok in raw.split_ascii_whitespace() {
        if tok == "RMK" {
            return None;
        }
        let b = tok.as_bytes();
        if b.len() != 5 {
            continue;
        }
        let first = *b.first()?;
        let n = digits(b, 1, 4);
        match (first, n) {
            (b'A', Some(n)) => return hpa_to_dhpa(n as f64 / 100.0 * HPA_PER_INHG),
            (b'Q', Some(n)) => return hpa_to_dhpa(n as f64),
            _ => {}
        }
    }
    None
}

/// Visibility text or number to (hundredths of a statute mile, "or more" flag). `"10+"` is
/// (1000, true). Bound: 0 to 100 SM.
pub fn parse_visib(s: &str) -> Option<(u16, bool)> {
    let (body, plus) = match s.strip_suffix('+') {
        Some(b) => (b, true),
        None => (s, false),
    };
    let v: f64 = body.parse().ok()?;
    if !v.is_finite() || !(0.0..=100.0).contains(&v) {
        return None;
    }
    Some(((v * 100.0).round() as u16, plus))
}

/// Flight level (hundreds of feet) to feet.
pub fn fl_to_ft(fl: u16) -> i32 {
    i32::from(fl) * 100
}

/// Converts a pressure altitude (feet, the standard-setting altitude an ADS-B or Mode C report
/// carries) to the indicated altitude an altimeter set to `qnh_hpa` would show, using the ISA
/// pressure-height relation (exponent 0.190263, 44330.77 m). `None` if the setting is outside
/// the altimeter bounds or the pressure altitude is outside -2 000..=36 000 ft (the model is the
/// troposphere only). Valid below the transition altitude only; the caller owns that decision.
pub fn indicated_altitude_ft(pressure_alt_ft: f64, qnh_hpa: f64) -> Option<f64> {
    const H0_M: f64 = 44_330.77;
    const EXP: f64 = 0.190_263;
    const FT_PER_M: f64 = 3.280_839_895;
    if !(ALTIM_MIN_HPA..=ALTIM_MAX_HPA).contains(&qnh_hpa)
        || !(-2000.0..=36_000.0).contains(&pressure_alt_ft)
    {
        return None;
    }
    let ratio = (1.0 - pressure_alt_ft / FT_PER_M / H0_M).powf(1.0 / EXP);
    let p = ISA_SEA_LEVEL_HPA * ratio;
    Some(H0_M * (1.0 - (p / qnh_hpa).powf(EXP)) * FT_PER_M)
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

    // REQ: OA-AWC-024
    #[test]
    fn impossible_dates_and_years_out_of_range_are_refused() {
        for (y, m, d, want) in VECTORS {
            assert_eq!(days_from_civil(y, m, d), want, "{y}-{m}-{d}");
        }
        assert_eq!(parse_iso_ms("2026-02-29T00:00:00Z"), None);
        assert_eq!(parse_iso_ms("2026-04-31T00:00:00Z"), None);
        assert_eq!(parse_iso_ms("2026-11-15T24:00:00Z"), None);
        assert_eq!(parse_iso_ms("1999-12-31T23:59:59Z"), None);
        assert_eq!(parse_iso_ms("2100-01-01T00:00:00Z"), None);
        assert_eq!(
            parse_iso_ms("2026-11-15T00:00:00Z"),
            Some(20_772 * 86_400_000)
        );
    }
}

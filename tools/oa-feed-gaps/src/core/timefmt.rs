//! Core (B1): ISO times and durations as text.

const NS: i64 = 1_000_000_000;

fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    if d > dim {
        return None;
    }
    // Hinnant's days_from_civil; the year is 1970 or later, so no negative era.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `YYYY-MM-DDTHH:MM:SSZ` (1970 or later) as Unix nanoseconds; `None` for anything else.
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    if b.get(10) != Some(&b'T') || b.get(13) != Some(&b':') || b.get(16) != Some(&b':') {
        return None;
    }
    if b.get(19) != Some(&b'Z') {
        return None;
    }
    let (y, mo, d) = (num(s.get(0..4)?)?, num(s.get(5..7)?)?, num(s.get(8..10)?)?);
    let (h, mi, se) = (
        num(s.get(11..13)?)?,
        num(s.get(14..16)?)?,
        num(s.get(17..19)?)?,
    );
    if h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let days = days_from_civil(y, mo, d)?;
    (days * 86_400 + h * 3600 + mi * 60 + se).checked_mul(NS)
}

/// Unix nanoseconds as `YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`; negative times print as `before-1970`.
pub fn iso(ns: i64) -> String {
    if ns < 0 {
        return "before-1970".to_string();
    }
    let secs = ns / NS;
    let frac = ns % NS;
    let (y, m, d) = civil_from_days(secs / 86_400);
    let r = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:09}Z",
        r / 3600,
        r % 3600 / 60,
        r % 60
    )
}

/// A duration in nanoseconds as seconds with nine decimals, e.g. `600.001000000 s`.
pub fn secs(ns: i64) -> String {
    let sign = if ns < 0 { "-" } else { "" };
    let a = ns.unsigned_abs();
    format!("{sign}{}.{:09} s", a / 1_000_000_000, a % 1_000_000_000)
}

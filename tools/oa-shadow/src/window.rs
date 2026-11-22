//! The comparison window: the two recordings rarely cover the same time. A system is not
//! "missing" at a time its recording does not cover, so both sides are cut to the overlap of the
//! two recorded extents before anything is compared. Pure.

use std::collections::BTreeMap;

use crate::model::{EramFlight, OaAircraft};

/// The overlap of two closed time extents (Unix ms), `None` when they do not meet.
pub fn overlap(a: (i64, i64), b: (i64, i64)) -> Option<(i64, i64)> {
    let (lo, hi) = (a.0.max(b.0), a.1.min(b.1));
    (lo <= hi).then_some((lo, hi))
}

/// The first and last of a set of times, `None` for none.
pub fn extent(times: impl Iterator<Item = i64>) -> Option<(i64, i64)> {
    times.fold(None, |e: Option<(i64, i64)>, t| {
        Some(e.map_or((t, t), |(a, b)| (a.min(t), b.max(t))))
    })
}

/// The original extent and sample counts outside a closed comparison window.
pub fn coverage(
    times: impl Iterator<Item = i64>,
    lo: i64,
    hi: i64,
) -> (Option<(i64, i64)>, u64, u64) {
    let mut extent = None;
    let (mut before, mut after) = (0u64, 0u64);
    for t in times {
        extent = Some(extent.map_or((t, t), |(a, b): (i64, i64)| (a.min(t), b.max(t))));
        if t < lo {
            before = before.saturating_add(1);
        } else if t > hi {
            after = after.saturating_add(1);
        }
    }
    (extent, before, after)
}

/// Keeps the ERAM samples inside `[lo, hi]` and drops the flights left with none.
pub fn restrict_eram(flights: &mut Vec<EramFlight>, lo: i64, hi: i64) {
    for f in flights.iter_mut() {
        f.samples.retain(|s| s.t_ms >= lo && s.t_ms <= hi);
    }
    flights.retain(|f| !f.samples.is_empty());
}

/// Keeps the OpenATCA rows inside `[lo, hi]` and drops the aircraft left with none.
pub fn restrict_oa(oa: &mut BTreeMap<u32, OaAircraft>, lo: i64, hi: i64) {
    for a in oa.values_mut() {
        a.samples.retain(|s| s.t_ms >= lo && s.t_ms <= hi);
    }
    oa.retain(|_, a| !a.samples.is_empty());
}

/// `YYYY-MM-DD HH:MM:SS` UTC of a Unix time in milliseconds (proleptic Gregorian).
pub fn utc_text(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

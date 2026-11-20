//! Core (B1): the excluded intervals of P-007: every interval over 600 s in either feed, and the
//! 300 s after it, merged where they overlap.

use super::gaps::{Gap, Stats, Window};

/// The margin after an interval over the limit, nanoseconds (P-007 section 2).
pub const MARGIN_NS: i64 = 300_000_000_000;

/// The excluded time over which the day is damaged, nanoseconds: more than 60 minutes.
pub const LIMIT_NS: i64 = 3_600_000_000_000;

/// The excluded intervals of the given feeds, each with its margin and cut at the window end,
/// sorted and merged where they overlap or touch.
pub fn excluded(window: Window, feeds: &[&Stats]) -> Vec<Gap> {
    let mut v: Vec<Gap> = Vec::new();
    for st in feeds {
        for g in &st.over {
            v.push(Gap {
                start_ns: g.start_ns,
                end_ns: g.end_ns.saturating_add(MARGIN_NS).min(window.to_ns),
            });
        }
    }
    v.sort_unstable_by_key(|g| (g.start_ns, g.end_ns));
    let mut out: Vec<Gap> = Vec::new();
    for g in v {
        match out.last_mut() {
            Some(l) if g.start_ns <= l.end_ns => l.end_ns = l.end_ns.max(g.end_ns),
            _ => out.push(g),
        }
    }
    out
}

/// The time the merged intervals cover, nanoseconds.
pub fn total_ns(merged: &[Gap]) -> i64 {
    merged
        .iter()
        .fold(0i64, |a, g| a.saturating_add(g.len_ns()))
}

/// Whether the excluded time is past the limit.
pub fn too_much(merged: &[Gap]) -> bool {
    total_ns(merged) > LIMIT_NS
}

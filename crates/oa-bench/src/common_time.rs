//! Common-time comparison of two systems' paired samples. Core (pure).
//!
//! Per-report statistics (T1 to T4) weight each system by how often it publishes: a system that
//! reports every second while an error lasts and every ten seconds when it is easy has more of its
//! reports in the hard part than a system that reports at one rate throughout, with the same error
//! at every instant. That is a valid description of the reports each system emits (the native
//! number), but not a comparison of the two. Here both systems are scored at the same truth
//! times: a grid of whole multiples of [`CommonTimeConfig::grid_ms`], and a grid time counts for an
//! aircraft only when both systems have a paired sample within [`CommonTimeConfig::tol_ms`] of
//! it. The error at a grid time is the error of the nearest sample; nothing is interpolated, so a
//! system is never credited with reports it did not publish. Each aircraft then counts in
//! proportion to the time both systems cover it, and the per-aircraft RMS values give a
//! distribution over aircraft that no long flight can dominate. The error is the horizontal one,
//! the root of the squares of the cross-track and along-track errors of the pairing.

use std::collections::BTreeMap;

use crate::pair::PairTable;
use crate::stats::{new_result, stats_of, MetricResult, Row};

/// Settings of the common-time comparison (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommonTimeConfig {
    /// Spacing of the shared truth-time grid, ms.
    pub grid_ms: i64,
    /// A system counts at a grid time only with a sample this close to it, ms.
    pub tol_ms: i64,
}

impl Default for CommonTimeConfig {
    fn default() -> CommonTimeConfig {
        CommonTimeConfig {
            grid_ms: 10_000,
            tol_ms: 2_000,
        }
    }
}

/// Horizontal error samples `(time, metres)` per truth aircraft, sorted by time; samples without
/// both a cross-track and an along-track error are left out.
fn by_aircraft(p: &PairTable) -> BTreeMap<u32, Vec<(i64, f64)>> {
    let mut m: BTreeMap<u32, Vec<(i64, f64)>> = BTreeMap::new();
    for i in 0..p.t_ms.len() {
        let (Some(&a), Some(&t), Some(&c), Some(&l)) = (
            p.addr.get(i),
            p.t_ms.get(i),
            p.cross_m.get(i),
            p.along_m.get(i),
        ) else {
            continue;
        };
        if c.is_finite() && l.is_finite() {
            m.entry(a)
                .or_default()
                .push((t, f64::from(c).hypot(f64::from(l))));
        }
    }
    for v in m.values_mut() {
        v.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
    }
    m
}

/// The error of the sample nearest to `g` within `tol` (the earlier on a tie).
fn nearest(v: &[(i64, f64)], g: i64, tol: i64) -> Option<f64> {
    let i = v.partition_point(|s| s.0 < g);
    let before = i.checked_sub(1).and_then(|j| v.get(j));
    let after = v.get(i);
    let pick = match (before, after) {
        (Some(b), Some(a)) => {
            if g - b.0 <= a.0 - g {
                b
            } else {
                a
            }
        }
        (Some(b), None) => b,
        (None, Some(a)) => a,
        (None, None) => return None,
    };
    ((pick.0 - g).abs() <= tol).then_some(pick.1)
}

/// Compares system `a` with system `b` at the same truth times (see the module documentation).
/// Rows: the horizontal error over all shared grid times of each system, and the distribution over
/// aircraft of each system's RMS at those times.
pub fn common_time_compare(a: &PairTable, b: &PairTable, cfg: &CommonTimeConfig) -> MetricResult {
    let (ma, mb) = (by_aircraft(a), by_aircraft(b));
    let grid = cfg.grid_ms.max(1);
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    let (mut fa, mut fb) = (Vec::new(), Vec::new());
    let (mut scored, mut without, mut aircraft) = (0u64, 0u64, 0u64);
    for (addr, va) in &ma {
        let Some(vb) = mb.get(addr) else {
            continue;
        };
        let (Some(a0), Some(a1), Some(b0), Some(b1)) =
            (va.first(), va.last(), vb.first(), vb.last())
        else {
            continue;
        };
        let (lo, hi) = (a0.0.max(b0.0), a1.0.min(b1.0));
        let mut g = lo.saturating_add(grid - 1).div_euclid(grid) * grid;
        let (mut xa, mut xb) = (Vec::new(), Vec::new());
        while g <= hi {
            match (nearest(va, g, cfg.tol_ms), nearest(vb, g, cfg.tol_ms)) {
                (Some(x), Some(y)) => {
                    xa.push(x);
                    xb.push(y);
                }
                _ => without += 1,
            }
            g += grid;
        }
        if xa.is_empty() {
            continue;
        }
        aircraft += 1;
        scored += xa.len() as u64;
        fa.push(stats_of(&xa).rms);
        fb.push(stats_of(&xb).rms);
        ea.extend(xa);
        eb.extend(xb);
    }
    let mut r = new_result(
        "C1",
        "horizontal position error of two systems at the same truth times",
        "m",
    );
    for (label, vals) in [
        ("horizontal, system A", &ea),
        ("horizontal, system B", &eb),
        ("per-flight RMS, system A", &fa),
        ("per-flight RMS, system B", &fb),
    ] {
        r.rows.push(Row {
            label: label.to_string(),
            stats: stats_of(vals),
        });
    }
    r.scalars
        .push(("aircraft compared".into(), aircraft as f64));
    r.scalars.push(("grid times scored".into(), scored as f64));
    r.scalars.push((
        "grid times without a report of both systems".into(),
        without as f64,
    ));
    r.scalars
        .push(("grid spacing, s".into(), cfg.grid_ms as f64 / 1000.0));
    r.scalars
        .push(("report tolerance, s".into(), cfg.tol_ms as f64 / 1000.0));
    r.notes.push(
        "both systems are scored at the same truth times: a grid time counts only when both have a paired sample within the tolerance, and the error is that of the nearest sample (nothing is interpolated or credited that was not published); per-report tables (T1 to T4) weight each system by how often it publishes, so a comparison of two systems cites this table".into(),
    );
    r
}

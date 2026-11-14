//! Metrics T1 to T4 over the paired-sample table. Core (pure): free functions from a
//! [`PairTable`] to [`MetricResult`]s.
//!
//! Along-track error is reported twice: raw, and after fitting one constant time offset `tau`
//! (seconds) over the whole run. The fit is the least squares solution of `along = tau * v`
//! over samples whose truth ground speed `v` is at least [`OffsetConfig::min_speed_mps`]:
//! `tau = sum(along * v) / sum(v * v)`; a positive `tau` means ERAM's positions are ahead of the
//! truth at the same time stamp. The corrected error is `along - tau * v`.
//!
//! **Drift.** The offset is estimated again inside each time bin of [`OffsetConfig::bin_ms`] and
//! for each aircraft, using groups of at least [`OffsetConfig::min_group_n`] samples. After
//! dropping the lowest and highest 5% of the aircraft groups (floor of 5% of the group count,
//! so a few mismatched tracks do not decide), the offset drifts if any remaining group estimate
//! differs from the overall `tau` by more than [`OffsetConfig::drift_limit_s`]. A drifting
//! offset withholds the corrected numbers with the reason; the fitted offset is still published.
//! With fewer than two usable time bins the time stability cannot be tested; that is stated.

use std::collections::BTreeMap;

use crate::pair::PairTable;
use crate::stats::{new_result, split_result, withheld_result, MetricResult, Splits};

/// Offset fit settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffsetConfig {
    /// Samples below this truth ground speed do not enter the fit, metres per second.
    pub min_speed_mps: f64,
    /// Width of a time bin, ms.
    pub bin_ms: i64,
    /// Fewest samples a bin or an aircraft needs to be tested for stability.
    pub min_group_n: u64,
    /// The largest accepted deviation of a group offset from the overall one, seconds.
    pub drift_limit_s: f64,
}

impl Default for OffsetConfig {
    fn default() -> OffsetConfig {
        OffsetConfig {
            min_speed_mps: 50.0,
            bin_ms: 3_600_000,
            min_group_n: 30,
            drift_limit_s: 1.0,
        }
    }
}

/// The fitted offset and the stability test.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetFit {
    /// Samples in the fit.
    pub n: u64,
    /// Fitted offset, seconds; `None` when there is nothing to fit.
    pub tau_s: Option<f64>,
    /// Time bins tested.
    pub bins_tested: u64,
    /// Aircraft groups tested.
    pub groups_tested: u64,
    /// Largest deviation of a tested bin from the overall offset, seconds.
    pub max_bin_dev_s: f64,
    /// Largest deviation of a tested (trimmed) aircraft group, seconds.
    pub max_group_dev_s: f64,
    /// The reason the offset drifts, if it does.
    pub drift: Option<String>,
}

#[derive(Default)]
struct Acc {
    n: u64,
    sev: f64,
    svv: f64,
}

fn tau_of(a: &Acc) -> Option<f64> {
    (a.svv > 0.0).then(|| a.sev / a.svv)
}

/// Fits the offset and tests its stability.
pub fn fit_offset(p: &PairTable, cfg: &OffsetConfig) -> OffsetFit {
    let mut all = Acc::default();
    let mut bins: BTreeMap<i64, Acc> = BTreeMap::new();
    let mut groups: BTreeMap<u32, Acc> = BTreeMap::new();
    let t0 = p.t_ms.iter().copied().min().unwrap_or(0);
    for i in 0..p.t_ms.len() {
        let (Some(&e), Some(&v), Some(&t), Some(&a)) = (
            p.along_m.get(i),
            p.truth_speed_mps.get(i),
            p.t_ms.get(i),
            p.addr.get(i),
        ) else {
            continue;
        };
        let (e, v) = (f64::from(e), f64::from(v));
        if !e.is_finite() || v < cfg.min_speed_mps {
            continue;
        }
        let bin = (t - t0).div_euclid(cfg.bin_ms.max(1));
        for acc in [
            &mut all,
            bins.entry(bin).or_default(),
            groups.entry(a).or_default(),
        ] {
            acc.n += 1;
            acc.sev += e * v;
            acc.svv += v * v;
        }
    }
    let mut fit = OffsetFit {
        n: all.n,
        tau_s: tau_of(&all),
        bins_tested: 0,
        groups_tested: 0,
        max_bin_dev_s: 0.0,
        max_group_dev_s: 0.0,
        drift: None,
    };
    let Some(tau) = fit.tau_s else { return fit };
    for b in bins.values().filter(|b| b.n >= cfg.min_group_n) {
        if let Some(t) = tau_of(b) {
            fit.bins_tested += 1;
            fit.max_bin_dev_s = fit.max_bin_dev_s.max((t - tau).abs());
        }
    }
    let mut gt: Vec<f64> = groups
        .values()
        .filter(|g| g.n >= cfg.min_group_n)
        .filter_map(tau_of)
        .collect();
    gt.sort_by(f64::total_cmp);
    fit.groups_tested = gt.len() as u64;
    let trim = gt.len() / 20;
    for &t in gt.get(trim..gt.len().saturating_sub(trim)).unwrap_or(&[]) {
        fit.max_group_dev_s = fit.max_group_dev_s.max((t - tau).abs());
    }
    if fit.max_bin_dev_s > cfg.drift_limit_s {
        fit.drift = Some(format!(
            "the offset drifts across the run: a time bin differs from the overall {tau:.3} s by {:.3} s (limit {:.3} s)",
            fit.max_bin_dev_s, cfg.drift_limit_s
        ));
    } else if fit.max_group_dev_s > cfg.drift_limit_s {
        fit.drift = Some(format!(
            "the offset differs across aircraft: a group differs from the overall {tau:.3} s by {:.3} s (limit {:.3} s)",
            fit.max_group_dev_s, cfg.drift_limit_s
        ));
    }
    fit
}

type Cols = (Vec<f64>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

/// The finite values of `f` over the table with their split codes.
fn collect(p: &PairTable, f: impl Fn(usize) -> f32) -> Cols {
    let mut c: Cols = (vec![], vec![], vec![], vec![], vec![]);
    for i in 0..p.t_ms.len() {
        let v = f(i);
        if v.is_finite() {
            c.0.push(f64::from(v));
            c.1.push(p.phase.get(i).copied().unwrap_or(0));
            c.2.push(p.band.get(i).copied().unwrap_or(0));
            c.3.push(p.coast.get(i).copied().unwrap_or(0));
            c.4.push(p.method.get(i).copied().unwrap_or(0));
        }
    }
    c
}

fn result_of(id: &'static str, name: &'static str, unit: &'static str, c: &Cols) -> MetricResult {
    let sp = Splits {
        phase: &c.1,
        band: &c.2,
        coast: &c.3,
        method: &c.4,
    };
    split_result(id, name, unit, &c.0, &sp)
}

fn at(v: &[f32], i: usize) -> f32 {
    v.get(i).copied().unwrap_or(f32::NAN)
}

/// T1: cross-track position error, metres (positive to the right of the truth track).
pub fn t1_cross_track(p: &PairTable) -> MetricResult {
    let c = collect(p, |i| at(&p.cross_m, i));
    result_of("T1", "cross-track position error", "m", &c)
}

/// T2 raw: along-track error, metres (positive when ERAM is ahead of the truth).
pub fn t2_along_raw(p: &PairTable) -> MetricResult {
    let c = collect(p, |i| at(&p.along_m, i));
    result_of("T2-raw", "along-track error, raw", "m", &c)
}

/// T2 corrected: along-track error after removing the fitted constant time offset. The offset,
/// its sample count and the stability test are scalars; a drifting offset withholds the rows.
pub fn t2_along_corrected(p: &PairTable, cfg: &OffsetConfig) -> MetricResult {
    let fit = fit_offset(p, cfg);
    let Some(tau) = fit.tau_s else {
        return withheld_result(
            "T2-offset",
            "along-track error, offset corrected",
            "m",
            "no sample with a defined along-track error and truth speed above the fit minimum",
        );
    };
    let mut r = if let Some(why) = &fit.drift {
        withheld_result("T2-offset", "along-track error, offset corrected", "m", why)
    } else {
        let c = collect(p, |i| {
            at(&p.along_m, i) - (tau as f32) * at(&p.truth_speed_mps, i)
        });
        result_of("T2-offset", "along-track error, offset corrected", "m", &c)
    };
    r.scalars.push(("fitted_offset_s".to_string(), tau));
    r.scalars.push(("fit_n".to_string(), fit.n as f64));
    r.scalars
        .push(("bins_tested".to_string(), fit.bins_tested as f64));
    r.scalars
        .push(("groups_tested".to_string(), fit.groups_tested as f64));
    r.scalars
        .push(("max_bin_dev_s".to_string(), fit.max_bin_dev_s));
    r.scalars
        .push(("max_group_dev_s".to_string(), fit.max_group_dev_s));
    if fit.bins_tested < 2 {
        r.notes.push(
            "time stability not tested: fewer than two time bins have enough samples".to_string(),
        );
    }
    r
}

/// T3 speed: ERAM track speed minus truth ground speed, knots.
pub fn t3_speed(p: &PairTable) -> MetricResult {
    let c = collect(p, |i| at(&p.speed_err_kt, i));
    result_of("T3-speed", "speed error", "kt", &c)
}

/// T3 heading: ERAM track heading minus truth heading, degrees in (-180, 180].
pub fn t3_heading(p: &PairTable) -> MetricResult {
    let c = collect(p, |i| at(&p.hdg_err_deg, i));
    result_of("T3-heading", "heading error", "deg", &c)
}

/// T4: ERAM reported altitude minus truth barometric altitude, feet.
pub fn t4_altitude(p: &PairTable) -> MetricResult {
    let c = collect(p, |i| at(&p.alt_err_ft, i));
    result_of("T4", "altitude error", "ft", &c)
}

/// An empty result placeholder used when no pairs exist.
pub fn empty_pairs_result(
    id: &'static str,
    name: &'static str,
    unit: &'static str,
) -> MetricResult {
    let mut r = new_result(id, name, unit);
    r.notes.push("no paired samples".to_string());
    r
}

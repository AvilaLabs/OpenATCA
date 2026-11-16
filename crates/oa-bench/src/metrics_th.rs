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
//!
//! **Raw is primary.** The fitted offset cannot tell a clock offset from real ERAM lag, so the
//! raw along-track error is the primary number and the corrected one is secondary. The offset is
//! published with two standard errors: the sample one (`sqrt(rss / (n - 1) / sum(v * v))`, which
//! treats samples as independent and is too small for tracks) and the between-aircraft one
//! (spread of the aircraft group offsets over the square root of their number), and a 95%
//! half-width of 1.96 times the larger. The trim of the drift test is kept (a few mismatched
//! tracks should not decide it) but the untrimmed largest deviation and the worst groups are
//! published too. Every scored source takes this one path.

use std::collections::BTreeMap;

use crate::pair::PairTable;
use crate::stats::{
    new_result, split_result, stats_of, withheld_result, ExactSum, MetricResult, Row, Splits,
};

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
    /// Standard error of the offset treating samples as independent, seconds.
    pub se_s: f64,
    /// Standard error from the spread of the aircraft group offsets, seconds; NaN with fewer than
    /// two groups.
    pub between_se_s: f64,
    /// Largest deviation of a tested aircraft group with nothing trimmed, seconds; NaN when no
    /// group was tested.
    pub max_group_dev_untrimmed_s: f64,
    /// The worst groups by deviation: `(address, samples, offset, deviation)`, at most
    /// [`WORST_GROUPS`].
    pub worst_groups: Vec<(u32, u64, f64, f64)>,
}

/// Aircraft groups listed in the untrimmed report.
pub const WORST_GROUPS: usize = 5;

#[derive(Default)]
struct Acc {
    n: u64,
    sev: ExactSum,
    svv: ExactSum,
    see: ExactSum,
}

fn tau_of(a: &Acc) -> Option<f64> {
    let svv = a.svv.value();
    (svv > 0.0).then(|| a.sev.value() / svv)
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
            acc.sev.add(e * v);
            acc.svv.add(v * v);
            acc.see.add(e * e);
        }
    }
    let mut fit = OffsetFit {
        n: all.n,
        tau_s: tau_of(&all),
        bins_tested: 0,
        groups_tested: 0,
        max_bin_dev_s: f64::NAN,
        max_group_dev_s: f64::NAN,
        drift: None,
        se_s: f64::NAN,
        between_se_s: f64::NAN,
        max_group_dev_untrimmed_s: f64::NAN,
        worst_groups: Vec::new(),
    };
    let Some(tau) = fit.tau_s else { return fit };
    if all.n > 1 {
        let rss = (all.see.value() - tau * all.sev.value()).max(0.0);
        fit.se_s = (rss / (all.n - 1) as f64 / all.svv.value()).sqrt();
    }
    let mut max_bin = 0.0f64;
    for b in bins.values().filter(|b| b.n >= cfg.min_group_n) {
        if let Some(t) = tau_of(b) {
            fit.bins_tested += 1;
            max_bin = max_bin.max((t - tau).abs());
        }
    }
    if fit.bins_tested > 0 {
        fit.max_bin_dev_s = max_bin;
    }
    let mut gl: Vec<(u32, u64, f64)> = groups
        .iter()
        .filter(|(_, g)| g.n >= cfg.min_group_n)
        .filter_map(|(&a, g)| tau_of(g).map(|t| (a, g.n, t)))
        .collect();
    gl.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
    fit.groups_tested = gl.len() as u64;
    let trim = gl.len() / 20;
    let mut max_trim = 0.0f64;
    for &(_, _, t) in gl.get(trim..gl.len().saturating_sub(trim)).unwrap_or(&[]) {
        max_trim = max_trim.max((t - tau).abs());
    }
    if fit.groups_tested > 0 {
        fit.max_group_dev_s = max_trim;
        fit.max_group_dev_untrimmed_s = gl.iter().map(|g| (g.2 - tau).abs()).fold(0.0f64, f64::max);
    }
    if gl.len() > 1 {
        let m = gl.iter().map(|g| g.2).sum::<f64>() / gl.len() as f64;
        let var = gl.iter().map(|g| (g.2 - m).powi(2)).sum::<f64>() / (gl.len() - 1) as f64;
        fit.between_se_s = (var / gl.len() as f64).sqrt();
    }
    let mut worst: Vec<(u32, u64, f64, f64)> = gl
        .iter()
        .map(|&(a, n, t)| (a, n, t, (t - tau).abs()))
        .collect();
    worst.sort_by(|a, b| b.3.total_cmp(&a.3).then(a.0.cmp(&b.0)));
    worst.truncate(WORST_GROUPS);
    fit.worst_groups = worst;
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
    let mut r = split_result(id, name, unit, &c.0, &sp);
    r.notes.push(NATIVE_NOTE.to_string());
    r
}

/// Marks the per-report tables: each published sample counts once.
pub const NATIVE_NOTE: &str = "native per-report statistics: every published sample counts once, so a system that publishes more often in a stretch weights it more; to compare two systems use the common-time table (C1, oa_bench::common_time)";

fn at(v: &[f32], i: usize) -> f32 {
    v.get(i).copied().unwrap_or(f32::NAN)
}

/// Label of the headline row that holds every associated flight, the flagged ones too.
pub const ALL_WITH_FLAGGED: &str = "all associated including flagged";

/// Adds to a headline table (T1, T2-raw, T4) what the identity check took out of it: the number
/// of flagged flights and of their points, and a row over the primary and the flagged points
/// together. The headline rows themselves are unchanged. `pick` is the column the table is of.
pub fn add_flagged_view(
    r: &mut MetricResult,
    p: &PairTable,
    flagged: &PairTable,
    flagged_flights: u64,
    pick: fn(&PairTable) -> &[f32],
) {
    let finite = |t: &PairTable| -> Vec<f64> {
        pick(t)
            .iter()
            .filter(|v| v.is_finite())
            .map(|&v| f64::from(v))
            .collect()
    };
    let mut all = finite(p);
    let fv = finite(flagged);
    r.scalars.push((
        "flagged_flights_excluded".to_string(),
        flagged_flights as f64,
    ));
    r.scalars
        .push(("flagged_points_excluded".to_string(), fv.len() as f64));
    all.extend(fv);
    r.rows.push(Row {
        label: ALL_WITH_FLAGGED.to_string(),
        stats: stats_of(&all),
    });
    r.notes.push(
        "the rows above leave out flights whose identity check failed (their count and points are the scalars); the last row adds them back, so nothing is hidden"
            .to_string(),
    );
}

/// T1: cross-track position error, metres (positive to the right of the truth track).
pub fn t1_cross_track(p: &PairTable) -> MetricResult {
    let c = collect(p, |i| at(&p.cross_m, i));
    result_of("T1", "cross-track position error", "m", &c)
}

/// T2 raw: along-track error, metres (positive when ERAM is ahead of the truth).
pub fn t2_along_raw(p: &PairTable) -> MetricResult {
    let c = collect(p, |i| at(&p.along_m, i));
    result_of("T2-raw", "along-track error, raw (primary)", "m", &c)
}

/// T2 corrected: along-track error after removing the fitted constant time offset. The offset,
/// its sample count and the stability test are scalars; a drifting offset withholds the rows.
pub fn t2_along_corrected(p: &PairTable, cfg: &OffsetConfig) -> MetricResult {
    let fit = fit_offset(p, cfg);
    let Some(tau) = fit.tau_s else {
        return withheld_result(
            "T2-offset",
            "along-track error, offset corrected (secondary)",
            "m",
            "no sample with a defined along-track error and truth speed above the fit minimum",
        );
    };
    let mut r = if let Some(why) = &fit.drift {
        withheld_result(
            "T2-offset",
            "along-track error, offset corrected (secondary)",
            "m",
            why,
        )
    } else {
        let c = collect(p, |i| {
            at(&p.along_m, i) - (tau as f32) * at(&p.truth_speed_mps, i)
        });
        result_of(
            "T2-offset",
            "along-track error, offset corrected (secondary)",
            "m",
            &c,
        )
    };
    r.scalars.push(("fitted_offset_s".to_string(), tau));
    r.scalars.push(("fit_n".to_string(), fit.n as f64));
    r.scalars.push(("fitted_offset_se_s".to_string(), fit.se_s));
    r.scalars.push((
        "fitted_offset_between_aircraft_se_s".to_string(),
        fit.between_se_s,
    ));
    r.scalars.push((
        "fitted_offset_u95_s".to_string(),
        1.96 * fit.se_s.max(fit.between_se_s),
    ));
    r.scalars
        .push(("bins_tested".to_string(), fit.bins_tested as f64));
    r.scalars
        .push(("groups_tested".to_string(), fit.groups_tested as f64));
    r.scalars
        .push(("max_bin_dev_s".to_string(), fit.max_bin_dev_s));
    r.scalars
        .push(("max_group_dev_s".to_string(), fit.max_group_dev_s));
    r.scalars.push((
        "max_group_dev_untrimmed_s".to_string(),
        fit.max_group_dev_untrimmed_s,
    ));
    let worst: Vec<String> = fit
        .worst_groups
        .iter()
        .map(|(a, n, t, d)| format!("{a:06X} (n={n}, offset {t:.3} s, deviation {d:.3} s)"))
        .collect();
    r.notes.push(format!(
        "worst aircraft groups, untrimmed: {}",
        if worst.is_empty() {
            "none tested".to_string()
        } else {
            worst.join("; ")
        }
    ));
    r.notes.push(
        "the fitted offset cannot be told apart from real ERAM lag: the raw along-track error is the primary number and this corrected one is secondary"
            .to_string(),
    );
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

/// Least-squares line of `y` on `x` over `(x, y)` pairs given by `get` for `0..n`: `(slope,
/// standard error of the slope, n)`, or `None` with fewer than three points or no spread in `x`.
/// Three passes, no allocation, so a run of millions of pairs costs nothing extra.
fn ols_slope(n: usize, get: impl Fn(usize) -> Option<(f64, f64)>) -> Option<(f64, f64, u64)> {
    let (mut cnt, mut mx, mut my) = (0u64, 0.0, 0.0);
    for (x, y) in (0..n).filter_map(&get) {
        cnt += 1;
        mx += x;
        my += y;
    }
    if cnt < 3 {
        return None;
    }
    let (mx, my) = (mx / cnt as f64, my / cnt as f64);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in (0..n).filter_map(&get) {
        let (dx, dy) = (x - mx, y - my);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    if sxx <= 0.0 || !sxx.is_finite() {
        return None;
    }
    let slope = sxy / sxx;
    let sse = (syy - slope * sxy).max(0.0);
    Some((slope, (sse / (cnt - 2) as f64 / sxx).sqrt(), cnt))
}

/// The widest longitude span, degrees, of the samples of [`t3_heading_grid`]: past it the
/// regression is refused and counted. The term `(lon - lon_ref) * sin(lat)` is a plane
/// approximation of the convergence, and a cohort spread round the globe has no single reference.
pub const HEADING_GRID_MAX_SPAN_DEG: f64 = 90.0;

/// 1e-7 degree units in a half turn.
const HALF_TURN_E7: i64 = 1_800_000_000;

/// T3 heading against grid convergence: the heading error (degrees) regressed on
/// `c = (lon - lon_ref) * sin(lat)`, with `lon_ref` the mean longitude of the samples used.
/// A slope near 1 says the scored velocity is reported in a plane grid rotated against true north
/// by the meridian convergence rather than in true east and north; a slope near 0 says it is not.
/// The result does not depend on the order of the pairs: longitudes are taken relative to the
/// smallest longitude of the samples (wrapped, so a run across the antimeridian is one block),
/// and the samples are summed in sorted order. A cohort wider than
/// [`HEADING_GRID_MAX_SPAN_DEG`] is refused: scalar `refused_span` is 1 and no slope is given.
/// Scalars: `slope_deg_per_deg`, `slope_se` (sample standard error, treating samples as
/// independent), `n` and `lon_ref_deg`.
pub fn t3_heading_grid(p: &PairTable) -> MetricResult {
    let mut r = new_result(
        "T3-heading-grid",
        "heading error against (lon - mean lon) * sin(lat)",
        "deg per deg",
    );
    let used = |i: usize| -> Option<(i32, i32, f32)> {
        let h = p.hdg_err_deg.get(i).copied()?;
        let (la, lo) = (p.lat_e7.get(i).copied()?, p.lon_e7.get(i).copied()?);
        h.is_finite().then_some((la, lo, h))
    };
    let anchor = (0..p.hdg_err_deg.len())
        .filter_map(used)
        .map(|(_, lo, _)| lo)
        .min();
    let mut rows: Vec<(i32, i64, u32)> = Vec::new();
    if let Some(anchor) = anchor {
        let wrap = |lo: i32| -> i64 {
            (i64::from(lo) - i64::from(anchor) + HALF_TURN_E7).rem_euclid(2 * HALF_TURN_E7)
                - HALF_TURN_E7
        };
        rows.extend(
            (0..p.hdg_err_deg.len())
                .filter_map(used)
                .map(|(la, lo, h)| {
                    // The bits of the error as a key that sorts as the value does.
                    let b = h.to_bits();
                    (la, wrap(lo), if b >> 31 == 0 { b | 1 << 31 } else { !b })
                }),
        );
        rows.sort_unstable();
    }
    let cnt = rows.len() as u64;
    let (lo_e7, hi_e7) = rows
        .iter()
        .fold((i64::MAX, i64::MIN), |(l, h), x| (l.min(x.1), h.max(x.1)));
    if cnt > 0 && (hi_e7 - lo_e7) as f64 * 1e-7 > HEADING_GRID_MAX_SPAN_DEG {
        r.scalars.push(("n".to_string(), cnt as f64));
        r.scalars.push(("refused_span".to_string(), 1.0));
        r.notes.push(format!(
            "slope not computed: the samples span {:.1} degrees of longitude, more than {HEADING_GRID_MAX_SPAN_DEG} degrees",
            (hi_e7 - lo_e7) as f64 * 1e-7
        ));
        return r;
    }
    let sum: i64 = rows.iter().map(|x| x.1).sum();
    let mean = if cnt > 0 {
        sum as f64 / cnt as f64 * 1e-7
    } else {
        0.0
    };
    let fit = ols_slope(rows.len(), |i| {
        let &(la, dlon, key) = rows.get(i)?;
        let b = if key >> 31 == 1 {
            key & !(1 << 31)
        } else {
            !key
        };
        let h = f64::from(f32::from_bits(b));
        let lat = f64::from(la) * 1e-7;
        Some(((dlon as f64 * 1e-7 - mean) * lat.to_radians().sin(), h))
    });
    match fit {
        Some((slope, se, n)) => {
            r.scalars.push(("slope_deg_per_deg".to_string(), slope));
            r.scalars.push(("slope_se".to_string(), se));
            r.scalars.push(("n".to_string(), n as f64));
            r.scalars.push((
                "lon_ref_deg".to_string(),
                crate::geo::wrap180(f64::from(anchor.unwrap_or(0)) * 1e-7 + mean),
            ));
        }
        None => {
            r.scalars.push(("n".to_string(), cnt as f64));
            r.notes
                .push("slope not computed: fewer than three samples or no spread in c".to_string());
        }
    }
    r.notes.push(
        "tests whether the scored velocity is grid-relative: a slope near 1 means the heading error follows the meridian convergence, near 0 means true north".to_string(),
    );
    r
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

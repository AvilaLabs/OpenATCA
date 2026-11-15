//! Leave-one-out accuracy (design section 1.5). Core (pure).
//!
//! Every `loo_stride`-th report of a leg (by position in the leg, so the hidden set does not
//! depend on anything else) is hidden, the leg is smoothed again from the rest, and each hidden
//! report is predicted at its own time with the same interpolation and the same bracket rule
//! as a query: a real remaining report within the bracket on both sides, no extrapolation.

use oa_adsb::NONE_I32;

use crate::smooth::{interp, sigma95_h_m, smooth, LegView, SIGMA95};
use crate::{Dist, Fraction, LooStats, TruthConfig};

const NS_PER_MS: i64 = 1_000_000;

/// Residuals collected over all legs. A7.2: cold, transient accumulator.
#[derive(Default)]
pub(crate) struct LooAcc {
    hidden: u64,
    no_bracket: u64,
    hz: Vec<f64>,
    cross: Vec<f64>,
    along: Vec<f64>,
    alt: Vec<f64>,
    under_gate: Fraction,
    coverage: Fraction,
}

/// Hides every `cfg.loo_stride`-th report of the leg, predicts it from the rest and adds the
/// residuals to `acc`. The slices are the leg's columns and have equal lengths.
#[allow(clippy::too_many_arguments)]
pub(crate) fn loo_leg(
    acc: &mut LooAcc,
    cfg: &TruthConfig,
    t: &[i64],
    ze: &[f64],
    zn: &[f64],
    sigma: &[f64],
    alt: &[i32],
    vrate: &[i32],
) {
    let n = t.len();
    let stride = cfg.loo_stride as usize;
    let phase = stride / 2;
    let mut keep: Vec<usize> = Vec::with_capacity(n);
    let mut hide: Vec<usize> = Vec::with_capacity(n / stride + 1);
    for k in 0..n {
        if k % stride == phase {
            hide.push(k);
        } else {
            keep.push(k);
        }
    }
    acc.hidden += hide.len() as u64;
    if keep.len() < cfg.min_leg_points as usize {
        acc.no_bracket += hide.len() as u64;
        return;
    }
    let pick_i64 =
        |col: &[i64]| -> Vec<i64> { keep.iter().filter_map(|&k| col.get(k).copied()).collect() };
    let pick_i32 =
        |col: &[i32]| -> Vec<i32> { keep.iter().filter_map(|&k| col.get(k).copied()).collect() };
    let pick_f64 =
        |col: &[f64]| -> Vec<f64> { keep.iter().filter_map(|&k| col.get(k).copied()).collect() };
    let (rt, ra, rv) = (pick_i64(t), pick_i32(alt), pick_i32(vrate));
    let (re, rn, rs) = (pick_f64(ze), pick_f64(zn), pick_f64(sigma));
    let sm = smooth(&rt, &re, &rn, &rs, cfg.accel_q);
    let view = LegView {
        t_ns: &rt,
        alt_ft: &ra,
        vrate_fpm: &rv,
        xf: &sm.xf,
        pf: &sm.pf,
        xs: &sm.xs,
        ps: &sm.ps,
    };
    let bracket = cfg.bracket_ms * NS_PER_MS;
    for &k in &hide {
        let (Some(&tk), Some(&ek), Some(&nk), Some(&sk)) =
            (t.get(k), ze.get(k), zn.get(k), sigma.get(k))
        else {
            continue;
        };
        // The remaining reports strictly before and after tk (hidden times are not in them).
        let j = rt.partition_point(|&x| x <= tk);
        let (Some(left), Some(&t_l), Some(&t_r)) =
            (j.checked_sub(1), rt.get(j.wrapping_sub(1)), rt.get(j))
        else {
            acc.no_bracket += 1;
            continue;
        };
        if tk.saturating_sub(t_l) > bracket || t_r.saturating_sub(tk) > bracket {
            acc.no_bracket += 1;
            continue;
        }
        let Some((x, p)) = interp(&view, left, tk, cfg.accel_q) else {
            acc.no_bracket += 1;
            continue;
        };
        // A prediction with a non-finite state or variance is not a prediction: counted as such.
        if !x.iter().all(|v| v.is_finite()) || !p[0].is_finite() {
            acc.no_bracket += 1;
            continue;
        }
        let [pe, ve, _, pn, vn, _] = x;
        let (dx, dy) = (ek - pe, nk - pn);
        let hz = (dx * dx + dy * dy).sqrt();
        acc.hz.push(hz);
        let speed = (ve * ve + vn * vn).sqrt();
        if speed >= 1.0 {
            let (ux, uy) = (ve / speed, vn / speed);
            acc.along.push(dx * ux + dy * uy);
            acc.cross.push(dx * uy - dy * ux);
        }
        let var = p[0].max(0.0);
        acc.under_gate.n += 1;
        if sigma95_h_m(var).is_some_and(|s| s <= cfg.max_sigma95_h_m) {
            acc.under_gate.hit += 1;
        }
        acc.coverage.n += 1;
        if hz <= SIGMA95 * (var + sk * sk).sqrt() {
            acc.coverage.hit += 1;
        }
        // Altitude: neighbours in the rest, all three must have one.
        if let (Some(&a), Some(&a0), Some(&a1)) = (alt.get(k), ra.get(left), ra.get(j)) {
            if a != NONE_I32 && a0 != NONE_I32 && a1 != NONE_I32 {
                let w = (tk - t_l) as f64 / (t_r - t_l) as f64;
                let pred = f64::from(a0) + (f64::from(a1) - f64::from(a0)) * w;
                acc.alt.push(f64::from(a) - pred);
            }
        }
    }
}

/// Turns the collected residuals into statistics.
pub(crate) fn loo_finish(acc: &mut LooAcc, cfg: &TruthConfig) -> LooStats {
    LooStats {
        stride: cfg.loo_stride,
        hidden: acc.hidden,
        no_bracket: acc.no_bracket,
        horizontal: dist_of(&acc.hz),
        cross: dist_of(&acc.cross),
        along: dist_of(&acc.along),
        alt: dist_of(&acc.alt),
        under_gate: acc.under_gate,
        coverage95: acc.coverage,
    }
}

/// Statistics of signed values: signed mean, RMS, and median, p95, p99 (nearest rank) and
/// maximum of the absolute values.
pub(crate) fn dist_of(v: &[f64]) -> Dist {
    let n = v.len();
    if n == 0 {
        return Dist::default();
    }
    let nf = n as f64;
    let mean = v.iter().sum::<f64>() / nf;
    let rms = (v.iter().map(|x| x * x).sum::<f64>() / nf).sqrt();
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    a.sort_by(|x, y| x.total_cmp(y));
    let rank = |p: f64| -> f64 {
        let r = ((p * nf).ceil() as usize).clamp(1, n);
        a.get(r - 1).copied().unwrap_or(0.0)
    };
    Dist {
        n: n as u64,
        mean,
        rms,
        median: rank(0.5),
        p95: rank(0.95),
        p99: rank(0.99),
        max: a.last().copied().unwrap_or(0.0),
    }
}

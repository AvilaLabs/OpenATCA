//! Leave-one-out accuracy (design section 1.5). Core (pure, B7).
//!
//! Every `loo_stride`-th report of a leg (by position in the leg, so the hidden set does not
//! depend on anything else) is hidden, the leg is smoothed again from the rest, and each hidden
//! report is predicted at its own time with the same interpolation and the same bracket rule
//! as a query: a real remaining report within the bracket on both sides, no extrapolation.

use super::smooth::{interp, sigma95_h_m, smooth, LegView, SIGMA95};
use super::types::{Dist, Fraction, LooStats, TruthConfig, NONE_I32};

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
        // A difference past the range of an i64 is no bracket.
        let near = |a: i64, b: i64| b.checked_sub(a).is_some_and(|d| d <= bracket);
        if !near(t_l, tk) || !near(tk, t_r) {
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
        let hz = oa_ieee::sqrt(dx * dx + dy * dy);
        acc.hz.push(hz);
        let speed = oa_ieee::sqrt(ve * ve + vn * vn);
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
        if hz <= SIGMA95 * oa_ieee::sqrt(var + sk * sk) {
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
    let rms = oa_ieee::sqrt(v.iter().map(|x| x * x).sum::<f64>() / nf);
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    a.sort_by(|x, y| x.total_cmp(y));
    let rank = |p: f64| -> f64 {
        let r = (oa_ieee::ceil(p * nf) as usize).clamp(1, n);
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

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::core::types::DEFAULT_CONFIG;

    const S: i64 = 1_000_000_000;

    /// A leg's columns: times in s, east, north, sigma, altitude, vertical rate.
    struct Leg {
        t: Vec<i64>,
        ze: Vec<f64>,
        zn: Vec<f64>,
        sg: Vec<f64>,
        alt: Vec<i32>,
        vr: Vec<i32>,
    }

    /// `n` reports 1 s apart on the line 100 m/s east and 100 m/s north, sigma 5 m, altitude
    /// 10000 ft.
    fn line(n: usize) -> Leg {
        Leg {
            t: (0..n as i64).map(|k| k * S).collect(),
            ze: (0..n).map(|k| 100.0 * k as f64).collect(),
            zn: (0..n).map(|k| 100.0 * k as f64).collect(),
            sg: vec![5.0; n],
            alt: vec![10_000; n],
            vr: vec![0; n],
        }
    }

    fn run(l: &Leg, cfg: &TruthConfig) -> LooAcc {
        let mut acc = LooAcc::default();
        loo_leg(&mut acc, cfg, &l.t, &l.ze, &l.zn, &l.sg, &l.alt, &l.vr);
        acc
    }

    fn near(a: f64, b: f64, what: &str) {
        assert!((a - b).abs() < 1e-6, "{what}: {a} vs {b}");
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn every_fifth_report_from_the_third_is_hidden_and_the_last_has_no_right_neighbour() {
        // n = 13 hides positions 2, 7 and 12 (k % 5 == 2). Altitude markers tell them apart.
        let mut l = line(13);
        l.alt[2] += 1000;
        l.alt[7] += 2000;
        l.alt[12] += 4000;
        let acc = run(&l, &DEFAULT_CONFIG);
        assert_eq!(acc.hidden, 3);
        // Position 12 is the last report: nothing to its right, so it is not predicted.
        assert_eq!(acc.no_bracket, 1);
        assert_eq!(acc.hz.len(), 2);
        // Neighbours are 10000 ft on both sides, so the residual is the marker.
        assert_eq!(acc.alt, vec![1000.0, 2000.0]);
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn a_leg_with_fewer_than_min_leg_points_left_after_hiding_is_counted_not_predicted() {
        // Three reports: position 2 is hidden, two remain (under 3): not smoothed.
        let acc = run(&line(3), &DEFAULT_CONFIG);
        assert_eq!((acc.hidden, acc.no_bracket, acc.hz.len()), (1, 1, 0));
        // Four reports: three remain, exactly the minimum: predicted.
        let acc = run(&line(4), &DEFAULT_CONFIG);
        assert_eq!((acc.hidden, acc.no_bracket, acc.hz.len()), (1, 0, 1));
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn a_hidden_report_needs_a_remaining_report_within_the_bracket_on_both_sides() {
        let shifted = |from: usize, extra_s: i64| {
            let mut l = line(9);
            for t in &mut l.t[from..] {
                *t += extra_s * S;
            }
            l
        };
        // Right neighbour (position 3) 12 s after the hidden one: no bracket; position 7 is fine.
        let acc = run(&shifted(3, 11), &DEFAULT_CONFIG);
        assert_eq!((acc.hidden, acc.no_bracket, acc.hz.len()), (2, 1, 1));
        // Left neighbour (position 1) 12 s before the hidden one.
        let acc = run(&shifted(2, 11), &DEFAULT_CONFIG);
        assert_eq!((acc.hidden, acc.no_bracket, acc.hz.len()), (2, 1, 1));
        // Left neighbour exactly 10 s before: inside the bracket.
        let acc = run(&shifted(2, 9), &DEFAULT_CONFIG);
        assert_eq!((acc.no_bracket, acc.hz.len()), (0, 2));
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn a_prediction_that_is_not_finite_is_counted_and_adds_no_residual() {
        // A NaN position poisons the whole filter.
        let mut l = line(9);
        l.ze[0] = f64::NAN;
        let acc = run(&l, &DEFAULT_CONFIG);
        assert_eq!((acc.hidden, acc.no_bracket), (2, 2));
        assert!(acc.hz.is_empty() && acc.along.is_empty() && acc.alt.is_empty());
        assert_eq!((acc.under_gate.n, acc.coverage.n), (0, 0));
        // An unbounded sigma on the report before the first hidden one leaves its filtered state
        // finite and its variance NaN: the prediction has a finite state and a non-finite variance.
        let mut l = line(9);
        l.sg[1] = 1e200;
        let acc = run(&l, &DEFAULT_CONFIG);
        assert_eq!((acc.hidden, acc.no_bracket), (2, 2));
        assert!(acc.hz.is_empty());
    }

    // Reference values for `line(9)` with the hidden position 2 moved by (dx, dy), computed
    // independently from the filter and smoother equations on the seven remaining reports.
    // Position 2 is predicted at t = 2 s from reports at 1 s and 3 s; position 7 at 7 s from
    // 6 s and 8 s.
    const VAR2: f64 = 6.504303403933136;
    const VAR7: f64 = 9.750431367413661;

    fn offset_leg(dx: f64, dy: f64) -> Leg {
        let mut l = line(9);
        l.ze[2] += dx;
        l.zn[2] += dy;
        l
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn residuals_split_into_horizontal_along_and_cross_track() {
        // Offset (30, 40) from a track heading north-east: hz 50, along (30 + 40) / sqrt 2 less the
        // prediction's own millimetre-level drift, cross (30 - 40) / sqrt 2 (negative: left).
        let acc = run(&offset_leg(30.0, 40.0), &DEFAULT_CONFIG);
        assert_eq!(acc.hz.len(), 2);
        near(acc.hz[0], 50.003327534033886, "hz[0]");
        near(acc.hz[1], 0.0011821500587057034, "hz[1]");
        assert_eq!(acc.along.len(), 2);
        near(acc.along[0], 49.50083599774726, "along[0]");
        near(acc.along[1], -0.0011821500587057031, "along[1]");
        assert_eq!(acc.cross.len(), 2);
        near(acc.cross[0], -7.071067811865472, "cross[0]");
        near(acc.cross[1], 0.0, "cross[1]");
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn coverage_compares_the_residual_with_the_prediction_variance_plus_the_report_variance() {
        // Offset (12, 16): hz = 20.0033 m. The 95 % radius of variance plus report variance is
        // 2.4477 * sqrt(6.5043 + 25) = 13.74 m (a miss); of the product it would be 31.2 m. The
        // unperturbed report at position 7 (hz 0.001 m) is a hit.
        let acc = run(&offset_leg(12.0, 16.0), &DEFAULT_CONFIG);
        assert_eq!((acc.coverage.n, acc.coverage.hit), (2, 1));
        // Predicted 95 % radii 6.2426 m and 7.6433 m (2.4477 * sqrt(VAR)): both under the 50 m
        // gate; with a 7 m gate only the first.
        assert_eq!((acc.under_gate.n, acc.under_gate.hit), (2, 2));
        let cfg = TruthConfig {
            max_sigma95_h_m: 7.0,
            ..DEFAULT_CONFIG
        };
        let acc = run(&offset_leg(12.0, 16.0), &cfg);
        assert_eq!((acc.under_gate.n, acc.under_gate.hit), (2, 1));
        assert!((SIGMA95 * VAR2.sqrt() - 6.2426).abs() < 1e-3);
        assert!((SIGMA95 * VAR7.sqrt() - 7.6433).abs() < 1e-3);
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn the_altitude_residual_is_against_the_time_weighted_neighbours_and_needs_all_three() {
        // Times 0 1 2 4 5 6 7 8 9: position 2 (t = 2 s) sits a third of the way from the report at
        // 1 s to the one at 4 s: 10000 + 300 / 3 = 10100; reported 10150: residual 50.
        let base = || {
            let mut l = line(9);
            l.t = [0, 1, 2, 4, 5, 6, 7, 8, 9].iter().map(|s| s * S).collect();
            l.alt[1] = 10_000;
            l.alt[3] = 10_300;
            l.alt[2] = 10_150;
            l
        };
        let acc = run(&base(), &DEFAULT_CONFIG);
        assert_eq!(acc.alt.len(), 2);
        near(acc.alt[0], 50.0, "alt[0]");
        // Position 7: neighbours 10000 and 10000, reported 10000.
        near(acc.alt[1], 0.0, "alt[1]");
        // Any one of the three missing: no altitude residual for position 2 (position 7 stays).
        for missing in [2usize, 1, 3] {
            let mut l = base();
            l.alt[missing] = NONE_I32;
            let acc = run(&l, &DEFAULT_CONFIG);
            assert_eq!(acc.alt.len(), 1, "alt missing at {missing}");
        }
    }

    // REQ: OA-TRUTH-057
    #[test]
    fn distributions_are_signed_mean_rms_and_nearest_rank_of_the_absolute_values() {
        // Absolute values 1..=20, signs alternating: mean of 1 - 2 + 3 - ... - 20 over 20 is -0.5.
        let v: Vec<f64> = (1..=20)
            .map(|k| {
                if k % 2 == 0 {
                    -f64::from(k)
                } else {
                    f64::from(k)
                }
            })
            .collect();
        let d = dist_of(&v);
        assert_eq!(d.n, 20);
        assert!((d.mean + 0.5).abs() < 1e-12);
        // sum of squares 1..=20 = 2870.
        assert!((d.rms - (2870.0f64 / 20.0).sqrt()).abs() < 1e-12);
        // Nearest rank: ceil(p n)-th smallest: 10th, 19th, 20th.
        assert_eq!((d.median, d.p95, d.p99, d.max), (10.0, 19.0, 20.0, 20.0));
        // One value: every rank is it.
        let d = dist_of(&[-3.0]);
        assert_eq!(
            (d.n, d.mean, d.median, d.p95, d.p99, d.max),
            (1, -3.0, 3.0, 3.0, 3.0, 3.0)
        );
        assert_eq!(dist_of(&[]), Dist::default());
    }
}

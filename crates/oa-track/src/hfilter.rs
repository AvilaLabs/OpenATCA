//! The horizontal interacting-multiple-model filter: a nearly-constant-velocity model and a
//! coordinated-turn model with the turn rate as a state, in a local east-north plane. Core
//! (pure).
//!
//! Both models share one state `[east, north, east velocity, north velocity, turn rate]` so the
//! mixing step is a plain weighted sum. The straight model keeps the turn rate exactly zero
//! (its row and column of the transition matrix are zero). The turn model predicts the nonlinear
//! coordinated-turn map with third-degree spherical-radial cubature over its five-dimensional
//! Gaussian state. Ten equally weighted points from the validated input covariance determine the
//! mean. Ten points from the covariance with the turn-rate floor determine the spread, centered on
//! that mean, before the existing process noise is added. The map uses series for small `theta`,
//! so the straight-flight limit is smooth. A strict generalized Cholesky factor accepts positive-
//! semidefinite covariances, including exact zero pivots, and reports invalid or non-finite
//! covariances instead of repairing them or falling back.
//!
//! A `mode` other than 0 replaces the pair by one straight-model filter with a fixed process
//! noise ([`crate::params::CV_ONLY_Q`]): the single-filter baseline the interacting pair is
//! measured against.
//!
//! One step is mixing, per-model prediction, a position update and a velocity update where the
//! measurement has one (the two are independent, so the sequential update is exact), then the
//! model probabilities from the likelihoods. The Markov transition between models depends on
//! the time step: the stay probability is `exp(-dt / tau)` with a mean dwell time per model
//! ([`H_TAU_CV_S`], [`H_TAU_CT_S`]), so a 1 s and a 17 s step are both consistent.

// E3: matrix indexes come from fixed 5-state/2-model bounds; cubature has exactly 2*N points.
// These loops never take an index or collection length from external input.
#![allow(clippy::indexing_slicing, clippy::needless_range_loop)]

use crate::linalg::{
    add, ident, matvec, mul, outer, sandwich, scale, symmetrise, transpose, M5, N, V5, ZERO,
};
use oa_geo::small::sin_cos_near_zero;

use crate::params::{
    cv_only_q, CT_OMEGA_STD_FLOOR, H_MU0_CV, H_PRIOR_VEL_STD, H_Q_CT, H_Q_CV, H_Q_OMEGA,
    H_TAU_CT_S, H_TAU_CV_S, MU_FLOOR, OMEGA_MAX, V_TAU_CLIMB_S, V_TAU_LEVEL_S,
};

/// One model's state and covariance. Hot (A7): 5 + 25 doubles.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct HModel {
    pub x: V5,
    pub p: M5,
}
const _: () = assert!(core::mem::size_of::<HModel>() == 240);

/// The two-model filter with its model probabilities `[straight, turn]`. Hot (A7).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct HFilter {
    pub m: [HModel; 2],
    pub mu: [f64; 2],
}
const _: () = assert!(core::mem::size_of::<HFilter>() == 496);

/// The turn model's prior turn rate variance floor.
fn omega_var_floor() -> f64 {
    CT_OMEGA_STD_FLOOR * CT_OMEGA_STD_FLOOR
}

/// A filter started at the origin of its frame from one measurement: position covariance
/// `pos_cov = [ee, en, nn]`, and a velocity with covariance if the measurement has one.
pub(crate) fn h_init(pos_cov: &[f64; 3], vel: Option<([f64; 2], [f64; 3])>, mode: u8) -> HFilter {
    let (v, vc) = vel.unwrap_or((
        [0.0, 0.0],
        [
            H_PRIOR_VEL_STD * H_PRIOR_VEL_STD,
            0.0,
            H_PRIOR_VEL_STD * H_PRIOR_VEL_STD,
        ],
    ));
    let mut p = ZERO;
    let [pee, pen, pnn] = *pos_cov;
    set_block(&mut p, 0, 1, pee, pen, pnn);
    let [vee, ven, vnn] = vc;
    set_block(&mut p, 2, 3, vee, ven, vnn);
    let x = [0.0, 0.0, v[0], v[1], 0.0];
    let cv = HModel { x, p };
    let mut p_ct = p;
    set(&mut p_ct, 4, 4, omega_var_floor());
    let ct = HModel { x, p: p_ct };
    HFilter {
        m: [cv, ct],
        mu: if mode == 0 {
            [H_MU0_CV, 1.0 - H_MU0_CV]
        } else {
            [1.0, 0.0]
        },
    }
}

fn set(p: &mut M5, i: usize, j: usize, v: f64) {
    if let Some(c) = p.get_mut(i).and_then(|r| r.get_mut(j)) {
        *c = v;
    }
}

fn set_block(p: &mut M5, i: usize, j: usize, aa: f64, ab: f64, bb: f64) {
    set(p, i, i, aa);
    set(p, i, j, ab);
    set(p, j, i, ab);
    set(p, j, j, bb);
}

/// The model transition probabilities over `dt` seconds: `[[cv to cv, cv to ct], [ct to cv,
/// ct to ct]]`.
///
/// `u` is [`dwell_decay`] of the step. The unit is a whole multiple of both dwell times, so the
/// stay probabilities are powers of `u`: `exp(-dt / tau) = u^n` for `n = unit / tau`.
pub(crate) fn h_transition(u: f64) -> [[f64; 2]; 2] {
    let u2 = u * u;
    let u3 = u2 * u;
    let u4 = u2 * u2;
    let u6 = u4 * u2;
    let u12 = u6 * u6;
    let s0 = u6;
    let s1 = u12 * u3;
    [[s0, 1.0 - s0], [1.0 - s1, s1]]
}

/// The time of which every model dwell time is a whole part, seconds: [`H_TAU_CV_S`] is a sixth of
/// it, [`H_TAU_CT_S`] a fifteenth and each vertical dwell time a tenth.
pub(crate) const DWELL_UNIT_S: f64 = 600.0;
const _: () = assert!(
    H_TAU_CV_S * 6.0 == DWELL_UNIT_S
        && H_TAU_CT_S * 15.0 == DWELL_UNIT_S
        && V_TAU_LEVEL_S * 10.0 == DWELL_UNIT_S
        && V_TAU_CLIMB_S * 10.0 == DWELL_UNIT_S
);

/// `exp(-dt / DWELL_UNIT_S)`: the one exponential from which the horizontal and vertical model
/// transitions of a step of `dt` seconds are taken.
pub(crate) fn dwell_decay(dt: f64) -> f64 {
    oa_math::exp(-dt / DWELL_UNIT_S)
}

/// Adds the white-acceleration noise of density `q` over `dt` to the position and velocity
/// blocks.
fn add_accel_noise(p: &mut M5, dt: f64, q: f64) {
    let d2 = dt * dt;
    let d3 = d2 * dt;
    for (a, b) in [(0usize, 2usize), (1, 3)] {
        add_to(p, a, a, q * d3 / 3.0);
        add_to(p, a, b, q * d2 / 2.0);
        add_to(p, b, a, q * d2 / 2.0);
        add_to(p, b, b, q * dt);
    }
}

fn add_to(p: &mut M5, i: usize, j: usize, v: f64) {
    if let Some(c) = p.get_mut(i).and_then(|r| r.get_mut(j)) {
        *c += v;
    }
}

fn g(v: &V5, i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}

/// Straight model prediction over `dt`.
fn cv_predict(m: &HModel, dt: f64, q: f64) -> HModel {
    let mut f = ident();
    set(&mut f, 0, 2, dt);
    set(&mut f, 1, 3, dt);
    set(&mut f, 4, 4, 0.0);
    let x = matvec(&f, &m.x);
    let mut p = sandwich(&f, &m.p);
    add_accel_noise(&mut p, dt, q);
    HModel {
        x,
        p: symmetrise(&p),
    }
}

/// `(sin t / t, (1 - cos t) / t)`; `s` and `c` are `sin t` and `cos t`.
fn ct_position_terms(theta: f64, s: f64, c: f64) -> (f64, f64) {
    if theta.abs() < 1.0e-4 {
        let t2 = theta * theta;
        (1.0 - t2 / 6.0, theta / 2.0 - theta * t2 / 24.0)
    } else {
        (s / theta, (1.0 - c) / theta)
    }
}

/// Deterministic coordinated-turn state map; moments are integrated separately.
fn ct_map(x: &V5, dt: f64) -> Option<V5> {
    if !dt.is_finite() || dt < 0.0 || x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let (px, py, vx, vy) = (x[0], x[1], x[2], x[3]);
    let w = x[4].clamp(-OMEGA_MAX, OMEGA_MAX);
    let theta = w * dt;
    if !theta.is_finite() {
        return None;
    }
    let (s, c) = sin_cos_near_zero(theta);
    let (a1, b1) = ct_position_terms(theta, s, c);
    let (a, b) = (dt * a1, dt * b1);
    let mapped = [
        px + vx * a - vy * b,
        py + vx * b + vy * a,
        vx * c - vy * s,
        vx * s + vy * c,
        w,
    ];
    mapped.iter().all(|v| v.is_finite()).then_some(mapped)
}

/// Exact-symmetry generalized lower Cholesky factor for a positive-semidefinite matrix.
/// A zero pivot is accepted only when its remaining Schur-complement cross residual is exactly
/// zero. Negative pivots are refused; no tolerance, jitter, or covariance repair is used.
fn cholesky_psd(p: &M5) -> Option<M5> {
    for i in 0..N {
        for j in 0..N {
            if !p[i][j].is_finite() || p[i][j] != p[j][i] {
                return None;
            }
        }
    }

    let mut l = ZERO;
    for i in 0..N {
        for j in 0..=i {
            let mut residual = p[i][j];
            for k in 0..j {
                let term = l[i][k] * l[j][k];
                if !term.is_finite() {
                    return None;
                }
                residual -= term;
                if !residual.is_finite() {
                    return None;
                }
            }
            if i == j {
                if residual < 0.0 {
                    return None;
                }
                l[i][j] = residual.sqrt();
            } else if l[j][j] == 0.0 {
                if residual != 0.0 {
                    return None;
                }
                l[i][j] = 0.0;
            } else {
                l[i][j] = residual / l[j][j];
                if !l[i][j].is_finite() {
                    return None;
                }
            }
        }
    }
    Some(l)
}

/// Split-floor third-degree spherical-radial cubature, with ten equal-weight points in 5D.
/// The original covariance determines the mean; its floored version determines the covariance
/// spread, centered on the original mean. Returns `None` for invalid input or non-finite results.
fn ct_predict(m: &HModel, dt: f64) -> Option<HModel> {
    if !dt.is_finite() || dt < 0.0 || m.x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    // The original covariance determines the nominal forecast. Apply the existing discovery
    // floor only to the covariance point cloud, then measure its spread around that forecast.
    let original_l = cholesky_psd(&m.p)?;
    let mut floored_p = m.p;
    floored_p[4][4] = floored_p[4][4].max(omega_var_floor());
    let floored_l = cholesky_psd(&floored_p)?;
    let radius = (N as f64).sqrt();
    let mut mean_points = [[0.0; N]; 2 * N];
    let mut covariance_points = [[0.0; N]; 2 * N];
    let mut point_ix = 0;
    for axis in 0..N {
        for sign in [-1.0, 1.0] {
            let mut mean_point = m.x;
            let mut covariance_point = m.x;
            for row in 0..N {
                mean_point[row] += sign * radius * original_l[row][axis];
                covariance_point[row] += sign * radius * floored_l[row][axis];
            }
            mean_points[point_ix] = ct_map(&mean_point, dt)?;
            covariance_points[point_ix] = ct_map(&covariance_point, dt)?;
            point_ix += 1;
        }
    }

    let weight = 1.0 / (2 * N) as f64;
    let mut mean = [0.0; N];
    for point in mean_points.iter() {
        for i in 0..N {
            mean[i] += weight * point[i];
        }
    }
    if mean.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let mut p = ZERO;
    for point in covariance_points.iter() {
        let mut delta = [0.0; N];
        for i in 0..N {
            delta[i] = point[i] - mean[i];
        }
        for i in 0..N {
            for j in 0..N {
                p[i][j] += weight * delta[i] * delta[j];
            }
        }
    }
    add_accel_noise(&mut p, dt, H_Q_CT);
    add_to(&mut p, 4, 4, H_Q_OMEGA * dt);
    p = symmetrise(&p);
    if p.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    Some(HModel { x: mean, p })
}

/// Mixes the two models for the next step and predicts each over `dt`. The returned filter's
/// `mu` is the predicted model probability. In IMM mode, `None` reports an invalid CT covariance
/// or non-finite mapped moment; callers must surface that failure.
pub(crate) fn h_predict(h: &HFilter, dt: f64, u: f64, mode: u8) -> Option<HFilter> {
    if mode != 0 {
        // Single straight-model filter (the comparison baseline): no mixing, no turn model.
        return Some(HFilter {
            m: [cv_predict(&h.m[0], dt, cv_only_q(mode)), h.m[1]],
            mu: [1.0, 0.0],
        });
    }
    let pi = h_transition(u);
    let mut mixed = [h.m[0], h.m[1]];
    let mut c = [0.0f64; 2];
    for j in 0..2 {
        let cj: f64 = (0..2).map(|i| pi[i][j] * h.mu[i]).sum();
        c[j] = cj;
        let w: [f64; 2] = if cj > 0.0 {
            [pi[0][j] * h.mu[0] / cj, pi[1][j] * h.mu[1] / cj]
        } else {
            [1.0, 0.0]
        };
        let mut x = [0.0; 5];
        for i in 0..2 {
            for (xk, hk) in x.iter_mut().zip(h.m[i].x.iter()) {
                *xk += w[i] * hk;
            }
        }
        let mut p = ZERO;
        for i in 0..2 {
            let mut d = h.m[i].x;
            for (dk, xk) in d.iter_mut().zip(x.iter()) {
                *dk -= xk;
            }
            p = add(&p, &scale(&add(&h.m[i].p, &outer(&d)), w[i]));
        }
        mixed[j] = HModel {
            x,
            p: symmetrise(&p),
        };
    }
    Some(HFilter {
        m: [
            cv_predict(&mixed[0], dt, H_Q_CV),
            ct_predict(&mixed[1], dt)?,
        ],
        mu: c,
    })
}

/// Mean and covariance of the filter's output: the probability-weighted moments of the two
/// models.
pub(crate) fn h_combine(h: &HFilter) -> (V5, M5) {
    let mut x = [0.0; 5];
    for (m, w) in h.m.iter().zip(h.mu.iter()) {
        for (xk, mk) in x.iter_mut().zip(m.x.iter()) {
            *xk += w * mk;
        }
    }
    let mut p = ZERO;
    for (m, w) in h.m.iter().zip(h.mu.iter()) {
        let mut d = m.x;
        for (dk, xk) in d.iter_mut().zip(x.iter()) {
            *dk -= xk;
        }
        p = add(&p, &scale(&add(&m.p, &outer(&d)), *w));
    }
    (x, symmetrise(&p))
}

/// The predicted position and its covariance `[ee, en, nn]` of the combined estimate.
pub(crate) fn h_pos_moments(h: &HFilter) -> ([f64; 2], [f64; 3]) {
    let (x, p) = h_combine(h);
    let pe = |i: usize, j: usize| p.get(i).and_then(|r| r.get(j)).copied().unwrap_or(0.0);
    ([g(&x, 0), g(&x, 1)], [pe(0, 0), pe(0, 1), pe(1, 1)])
}

/// The normalised squared innovation of a position `z` with covariance `r` against the
/// predicted combined estimate; infinite if the innovation covariance is singular.
pub(crate) fn h_pos_nis(pred: &HFilter, z: &[f64; 2], r: &[f64; 3]) -> f64 {
    let (p, c) = h_pos_moments(pred);
    let (s00, s01, s11) = (c[0] + r[0], c[1] + r[1], c[2] + r[2]);
    let det = s00 * s11 - s01 * s01;
    if det.is_nan() || det <= 0.0 {
        return f64::INFINITY;
    }
    let (y0, y1) = (z[0] - p[0], z[1] - p[1]);
    (y0 * (s11 * y0 - s01 * y1) + y1 * (s00 * y1 - s01 * y0)) / det
}

/// The gating distance of a position `z`: the smallest normalised squared innovation over the
/// two models, each against its own predicted covariance. A measurement is acceptable if any
/// model explains it, which is wider than the moment-matched mixture when the turn model has a
/// small probability and the aircraft starts to turn.
pub(crate) fn h_gate_d2(pred: &HFilter, z: &[f64; 2], r: &[f64; 3]) -> f64 {
    let mut best = f64::INFINITY;
    for m in pred.m.iter() {
        let p = &m.p;
        let (s00, s01, s11) = (p[0][0] + r[0], p[0][1] + r[1], p[1][1] + r[2]);
        let det = s00 * s11 - s01 * s01;
        if det.is_nan() || det <= 0.0 {
            continue;
        }
        let (y0, y1) = (z[0] - m.x[0], z[1] - m.x[1]);
        let d2 = (y0 * (s11 * y0 - s01 * y1) + y1 * (s00 * y1 - s01 * y0)) / det;
        best = best.min(d2);
    }
    best
}

/// The gating distance of a velocity `v` with covariance `vr`: the smallest normalised squared
/// innovation over the two models, each against its own predicted velocity covariance. Mirrors
/// [`h_gate_d2`].
pub(crate) fn h_vel_gate_d2(pred: &HFilter, v: &[f64; 2], vr: &[f64; 3]) -> f64 {
    let mut best = f64::INFINITY;
    for m in pred.m.iter() {
        let p = &m.p;
        let (s00, s01, s11) = (p[2][2] + vr[0], p[2][3] + vr[1], p[3][3] + vr[2]);
        let det = s00 * s11 - s01 * s01;
        if det.is_nan() || det <= 0.0 {
            continue;
        }
        let (y0, y1) = (v[0] - m.x[2], v[1] - m.x[3]);
        let d2 = (y0 * (s11 * y0 - s01 * y1) + y1 * (s00 * y1 - s01 * y0)) / det;
        best = best.min(d2);
    }
    best
}

/// One two-component measurement of the states `i0` and `i1` (`z = x[i0], x[i1]` plus noise of
/// covariance `r`): updates `x` and `p` and returns the quadratic form and the determinant of
/// the innovation covariance, from which the log likelihood is `-d2 / 2 - ln(det) / 2` (without
/// the constant shared by every model); `None`, with `x` and `p` unchanged, if the innovation
/// covariance is singular.
fn update_sel(
    x: &mut V5,
    p: &mut M5,
    i: [usize; 2],
    z: &[f64; 2],
    r: &[f64; 3],
) -> Option<(f64, f64)> {
    let [i0, i1] = i;
    let pe =
        |p: &M5, a: usize, b: usize| p.get(a).and_then(|row| row.get(b)).copied().unwrap_or(0.0);
    let y0 = z[0] - g(x, i0);
    let y1 = z[1] - g(x, i1);
    let s00 = pe(p, i0, i0) + r[0];
    let s01 = pe(p, i0, i1) + r[1];
    let s11 = pe(p, i1, i1) + r[2];
    let det = s00 * s11 - s01 * s01;
    if det.is_nan() || det <= 0.0 {
        return None;
    }
    let (n00, n01, n11) = (s11 / det, -s01 / det, s00 / det);
    let d2 = y0 * (n00 * y0 + n01 * y1) + y1 * (n01 * y0 + n11 * y1);
    let old = *p;
    let mut k = [[0.0f64; 2]; 5];
    for (row, kr) in k.iter_mut().enumerate() {
        let (a, b) = (pe(&old, row, i0), pe(&old, row, i1));
        *kr = [a * n00 + b * n01, a * n01 + b * n11];
    }
    for (row, kr) in k.iter().enumerate() {
        if let Some(xr) = x.get_mut(row) {
            *xr += kr[0] * y0 + kr[1] * y1;
        }
    }
    let mut pn = old;
    for (row, kr) in k.iter().enumerate() {
        for col in 0..5 {
            add_to(
                &mut pn,
                row,
                col,
                -(kr[0] * pe(&old, i0, col) + kr[1] * pe(&old, i1, col)),
            );
        }
    }
    *p = symmetrise(&pn);
    Some((d2, det))
}

/// The model probabilities after a measurement, before the floor and the normalisation: the
/// prior `prior[j]` times the likelihood, scaled so the larger is 1. Each model's `fit[j]` is
/// the sum `q` of the quadratic forms and the product `det` of the innovation determinants of
/// its updates (`None` if one was singular), so its log weight is
/// `lw[j] = ln(prior[j]) - q / 2 - ln(det) / 2`, and the result is `exp(lw[j] - max(lw))`.
///
/// The two models need only the difference `t = lw[1] - lw[0]`, which is
/// `ln((prior[1] / prior[0]) sqrt(det0 / det1)) - (q1 - q0) / 2` in real arithmetic: one `ln`
/// of one quotient where the log weights took four to six, and one `exp` of `-|t|` where they
/// took two. `t >= 0` gives `[exp(-t), 1]`, otherwise `[1, exp(t)]`, which are those same
/// numbers. A quotient or difference that is not finite (a determinant that over- or underflows,
/// an infinite quadratic form) takes the log weights themselves, as does a singular model.
/// A prior is floored at 1e-300 first. If no log weight is finite the prior comes back.
pub(crate) fn imm_weights(prior: [f64; 2], fit: [Option<(f64, f64)>; 2]) -> [f64; 2] {
    let p = [prior[0].max(1.0e-300), prior[1].max(1.0e-300)];
    if let [Some((q0, d0)), Some((q1, d1))] = fit {
        let ratio = p[1] / p[0] * (d0 / d1).sqrt();
        if ratio.is_finite() && ratio > 0.0 {
            let t = oa_math::ln(ratio) - 0.5 * (q1 - q0);
            if t.is_finite() {
                return if t >= 0.0 {
                    [oa_math::exp(-t), 1.0]
                } else {
                    [1.0, oa_math::exp(t)]
                };
            }
        }
    }
    let lw = [0, 1].map(|j| {
        let ll = fit[j].map_or(f64::NEG_INFINITY, |(q, d)| -0.5 * q - 0.5 * oa_math::ln(d));
        oa_math::ln(p[j]) + ll
    });
    let mx = lw[0].max(lw[1]);
    if mx.is_finite() {
        [oa_math::exp(lw[0] - mx), oa_math::exp(lw[1] - mx)]
    } else {
        prior
    }
}

/// Updates the predicted filter with a position (and a velocity if given), then the model
/// probabilities from the likelihoods and the predicted probabilities.
pub(crate) fn h_update(
    pred: &HFilter,
    z: &[f64; 2],
    r: &[f64; 3],
    vel: Option<([f64; 2], [f64; 3])>,
    mode: u8,
) -> HFilter {
    let mut out = *pred;
    if mode != 0 {
        let m = &mut out.m[0];
        update_sel(&mut m.x, &mut m.p, [0, 1], z, r);
        if let Some((v, vr)) = vel {
            update_sel(&mut m.x, &mut m.p, [2, 3], &v, &vr);
        }
        m.x[4] = 0.0;
        return out;
    }
    let mut fit = [None; 2];
    for (j, m) in out.m.iter_mut().enumerate() {
        let mut f = update_sel(&mut m.x, &mut m.p, [0, 1], z, r);
        if let Some((v, vr)) = vel {
            let g = update_sel(&mut m.x, &mut m.p, [2, 3], &v, &vr);
            f = f.zip(g).map(|((q, d), (q2, d2))| (q + q2, d * d2));
        }
        fit[j] = f;
        if let Some(w) = m.x.get_mut(4) {
            *w = w.clamp(-OMEGA_MAX, OMEGA_MAX);
        }
        if j == 0 {
            // The straight model keeps its turn rate at zero by construction.
            if let Some(w) = m.x.get_mut(4) {
                *w = 0.0;
            }
        }
    }
    let mut mu = imm_weights(pred.mu, fit);
    let s = mu[0] + mu[1];
    mu = [mu[0] / s, mu[1] / s];
    mu = [mu[0].max(MU_FLOOR), mu[1].max(MU_FLOOR)];
    let s = mu[0] + mu[1];
    out.mu = [mu[0] / s, mu[1] / s];
    out
}

/// A frame change at the mean position: every model's state and covariance are carried to the
/// new frame. Positions go to `to + d (q - from)`, velocities to `d v`, and the covariance to
/// `T P T^T` with `T = diag(d, d, 1)`, so the position and velocity blocks and their cross terms
/// are all transformed; `d` is the differential of the change of frame at the point.
pub(crate) fn h_change_frame(h: &mut HFilter, from: [f64; 2], to: [f64; 2], d: &[[f64; 2]; 2]) {
    let mut t = ident();
    for ix in [0usize, 2] {
        set(&mut t, ix, ix, d[0][0]);
        set(&mut t, ix, ix + 1, d[0][1]);
        set(&mut t, ix + 1, ix, d[1][0]);
        set(&mut t, ix + 1, ix + 1, d[1][1]);
    }
    for m in h.m.iter_mut() {
        let rel = [m.x[0] - from[0], m.x[1] - from[1], m.x[2], m.x[3], m.x[4]];
        let mut x = matvec(&t, &rel);
        x[0] += to[0];
        x[1] += to[1];
        m.x = x;
        m.p = symmetrise(&mul(&mul(&t, &m.p), &transpose(&t)));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // REQ: OA-TRK-262
    #[test]
    fn a_frame_change_transforms_the_whole_covariance_and_every_mean() {
        let mut h = h_init(
            &[10000.0, 30.0, 1.0],
            Some(([20.0, -5.0], [4.0, 0.5, 0.25])),
            0,
        );
        // Give the position/velocity block a cross term and the models different positions.
        for (k, m) in h.m.iter_mut().enumerate() {
            m.p[0][2] = 3.0;
            m.p[2][0] = 3.0;
            m.p[1][3] = -2.0;
            m.p[3][1] = -2.0;
            m.x[0] = 5.0 + k as f64;
        }
        let d = [[0.0, -1.0], [1.0, 0.0]];
        let before = h;
        h_change_frame(&mut h, [5.5, 0.0], [100.0, 200.0], &d);
        for (b, a) in before.m.iter().zip(h.m.iter()) {
            // Position: to + d (q - from); velocity: d v; turn rate unchanged.
            let (qx, qy) = (b.x[0] - 5.5, b.x[1]);
            assert!((a.x[0] - (100.0 - qy)).abs() < 1e-12 && (a.x[1] - (200.0 + qx)).abs() < 1e-12);
            assert!((a.x[2] - 5.0).abs() < 1e-12 && (a.x[3] - 20.0).abs() < 1e-12);
            assert_eq!(a.x[4], b.x[4]);
            // A quarter turn: (x, y) -> (-y, x) in both the position and velocity blocks, with
            // the cross terms following.
            assert!((a.p[0][0] - b.p[1][1]).abs() < 1e-12 && (a.p[1][1] - b.p[0][0]).abs() < 1e-12);
            assert!((a.p[0][1] + b.p[0][1]).abs() < 1e-12);
            assert!((a.p[2][2] - b.p[3][3]).abs() < 1e-12 && (a.p[3][3] - b.p[2][2]).abs() < 1e-12);
            // Position-velocity: new (E_pos, E_vel) = (-N_pos, -N_vel) ...
            assert!(
                (a.p[0][2] - b.p[1][3]).abs() < 1e-12,
                "{} {}",
                a.p[0][2],
                b.p[1][3]
            );
            assert!((a.p[1][3] - b.p[0][2]).abs() < 1e-12);
            assert!((a.p[0][3] + b.p[1][2]).abs() < 1e-12);
        }
    }

    /// The model probabilities from the log weights themselves, as the filter took them before
    /// the difference form.
    fn imm_by_log_weights(prior: [f64; 2], fit: [Option<(f64, f64)>; 2]) -> [f64; 2] {
        let lw = [0, 1].map(|j| {
            let ll = fit[j].map_or(f64::NEG_INFINITY, |(q, d)| -0.5 * q - 0.5 * oa_math::ln(d));
            oa_math::ln(prior[j].max(1.0e-300)) + ll
        });
        let mx = lw[0].max(lw[1]);
        if mx.is_finite() {
            [oa_math::exp(lw[0] - mx), oa_math::exp(lw[1] - mx)]
        } else {
            prior
        }
    }

    // REQ: OA-TRK-294
    #[test]
    fn the_model_probabilities_are_the_log_weight_softmax_in_every_regime() {
        let mut worst = 0.0f64;
        let mut n = 0;
        for p0 in [0.9, 0.5, 0.01, 1.0e-3, 0.0] {
            let prior = [p0, 1.0 - p0];
            for (q0, q1) in [
                (0.0, 0.0),
                (1.0, 40.0),
                (900.0, 3.0),
                (2.0e3, 1.0e-3),
                (1.0e5, 0.5),
            ] {
                for (d0, d1) in [
                    (1.0, 1.0),
                    (4.0e6, 9.0e3),
                    (2.0e-9, 5.0e3),
                    (1.0e-30, 3.0e12),
                ] {
                    let fit = [Some((q0, d0)), Some((q1, d1))];
                    let (a, b) = (imm_weights(prior, fit), imm_by_log_weights(prior, fit));
                    for (x, y) in a.iter().zip(b.iter()) {
                        worst = worst.max((x - y).abs() / y.abs().max(1.0e-300));
                    }
                    // The larger of the two is exactly 1.
                    assert!(a[0].max(a[1]) == 1.0 && a[0].min(a[1]) >= 0.0, "{a:?}");
                    n += 1;
                }
            }
        }
        assert!(worst < 1.0e-12, "{worst} over {n}");
        // A singular model, both singular, an infinite form, and a determinant that overflows.
        for (prior, fit) in [
            ([0.5, 0.5], [None, Some((1.0, 2.0))]),
            ([0.5, 0.5], [Some((1.0, 2.0)), None]),
            ([0.5, 0.5], [None, None]),
            ([0.3, 0.7], [Some((f64::INFINITY, 2.0)), Some((1.0, 2.0))]),
            ([0.3, 0.7], [Some((1.0, f64::MAX)), Some((1.0, 1.0e-300))]),
            ([0.3, 0.7], [Some((f64::NAN, 2.0)), Some((1.0, 2.0))]),
        ] {
            let (a, b) = (imm_weights(prior, fit), imm_by_log_weights(prior, fit));
            assert!(
                a == b || (a[0].is_nan() && b[0].is_nan()),
                "{fit:?}: {a:?} vs {b:?}"
            );
        }
    }

    // REQ: OA-TRK-294
    #[test]
    fn the_stay_probabilities_are_the_exponentials_of_the_dwell_times() {
        for k in 0..400 {
            let dt = 0.25 * f64::from(k);
            let u = dwell_decay(dt);
            let (pi, pv) = (h_transition(u), crate::vfilter::v_transition(u));
            let (e0, e1) = ((-dt / H_TAU_CV_S).exp(), (-dt / H_TAU_CT_S).exp());
            let e2 = (-dt / V_TAU_LEVEL_S).exp();
            assert!(
                (pi[0][0] - e0).abs() <= 4.0e-15 && (pi[1][1] - e1).abs() <= 4.0e-15,
                "{dt}"
            );
            assert!(
                (pv[0][0] - e2).abs() <= 4.0e-15 && pv[1][1] == pv[0][0],
                "{dt}"
            );
            assert_eq!(pi[0][1], 1.0 - pi[0][0]);
        }
    }

    // REQ: OA-TRK-329
    #[test]
    fn ct_prediction_integrates_the_turn_rate_spread_in_the_state_mean() {
        let mut p = ZERO;
        p[4][4] = 0.04 * 0.04;
        let state = [0.0, 0.0, 230.0, 0.0, 0.0];
        let h = HFilter {
            m: [HModel { x: state, p: ZERO }, HModel { x: state, p }],
            mu: [0.0, 1.0],
        };
        let dt = 4.8;
        let predicted = h_predict(&h, dt, dwell_decay(dt), 0)
            .expect("valid semidefinite CT covariance predicts");

        // Eight of ten cubature points have omega=0; the remaining pair has
        // omega=+/-sqrt(5)*0.04. Average those exact CT maps independently.
        let theta = 5.0_f64.sqrt() * 0.04 * dt;
        let sinc = theta.sin() / theta;
        let expected_x = 230.0 * dt * (0.8 + 0.2 * sinc);
        let expected_vx = 230.0 * (0.8 + 0.2 * theta.cos());
        assert!((predicted.m[1].x[0] - expected_x).abs() < 1.0e-12);
        assert!((predicted.m[1].x[2] - expected_vx).abs() < 1.0e-12);
        assert!(predicted.m[1].x[1].abs() < 1.0e-12);
        assert!(predicted.m[1].x[3].abs() < 1.0e-12);
        assert!(predicted.m[1].x[4].abs() < 1.0e-12);
    }

    // REQ: OA-TRK-329
    #[test]
    fn cubature_applies_the_existing_turn_limit_to_each_mapped_point() {
        let mut p = ZERO;
        p[4][4] = 0.04 * 0.04;
        for sign in [-1.0, 1.0] {
            let m = HModel {
                x: [0.0, 0.0, 100.0, 0.0, sign * 0.49],
                p,
            };
            let actual = ct_predict(&m, 4.8).expect("valid covariance at the turn limit");
            // Eight points retain mean omega; one of the spread pair reaches the limit.
            let expected = sign * (8.0 * 0.49 + 0.5 + (0.49 - 5.0_f64.sqrt() * 0.04)) / 10.0;
            assert!((actual.x[4] - expected).abs() < 1.0e-15);
            assert!(actual.x[4].abs() <= OMEGA_MAX);
            assert!(actual.x.iter().all(|x| x.is_finite()));
        }
    }

    // REQ: OA-TRK-329
    #[test]
    fn cubature_matches_independent_dense_mean_and_covariance_golden() {
        let lower = [
            [30.0, 0.0, 0.0, 0.0, 0.0],
            [3.0, 20.0, 0.0, 0.0, 0.0],
            [1.0, 0.4, 3.0, 0.0, 0.0],
            [-0.6, 1.0, 0.2, 2.0, 0.0],
            [0.0002, -0.0004, 0.001, 0.002, 0.02],
        ];
        let mut p = ZERO;
        for i in 0..N {
            for j in 0..N {
                for k in 0..N {
                    p[i][j] += lower[i][k] * lower[j][k];
                }
            }
        }
        let state = [12.0, -4.0, 120.0, -50.0, 0.02];
        let h = HFilter {
            m: [HModel { x: state, p: ZERO }, HModel { x: state, p }],
            mu: [0.0, 1.0],
        };
        let actual = h_predict(&h, 4.8, dwell_decay(4.8), 0)
            .expect("dense positive-definite covariance predicts")
            .m[1];
        let expected_x: V5 = [
            597.6637704174037,
            -215.66527333107612,
            123.64336971413496,
            -38.076679239988074,
            0.02,
        ];
        let expected_p: M5 = [
            [
                1910.402100494927,
                1156.497134201323,
                269.47430111690176,
                436.1253776982784,
                0.7864098117549173,
            ],
            [
                1156.497134201323,
                3899.5779055551016,
                401.9871211981368,
                1363.1331766430071,
                2.242093301893808,
            ],
            [
                269.47430111690176,
                401.9871211981368,
                89.4974379197418,
                160.95228958089635,
                0.28767944213523294,
            ],
            [
                436.1253776982784,
                1363.1331766430071,
                160.95228958089635,
                552.3214348529035,
                0.9292618899328312,
            ],
            [
                0.7864098117549173,
                2.242093301893808,
                0.28767944213523294,
                0.9292618899328312,
                0.0016144,
            ],
        ];
        for i in 0..N {
            let x_tol = 128.0 * f64::EPSILON * expected_x[i].abs().max(1.0);
            assert!(
                (actual.x[i] - expected_x[i]).abs() <= x_tol,
                "mean[{i}] {} != {}",
                actual.x[i],
                expected_x[i]
            );
            assert!(actual.x[i].is_finite());
            for j in 0..N {
                let tol = 128.0 * f64::EPSILON * expected_p[i][j].abs().max(1.0);
                assert!(
                    (actual.p[i][j] - expected_p[i][j]).abs() <= tol,
                    "p[{i}][{j}] {} != {}",
                    actual.p[i][j],
                    expected_p[i][j]
                );
                assert!(actual.p[i][j].is_finite());
                assert_eq!(actual.p[i][j], actual.p[j][i]);
            }
        }
    }

    // REQ: OA-TRK-329
    #[test]
    fn zero_turn_variance_floor_changes_covariance_without_moving_the_mean() {
        let model = HModel {
            x: [0.0, 0.0, 230.0, 0.0, 0.0],
            p: ZERO,
        };
        let actual =
            ct_predict(&model, 17.0).expect("zero covariance with discovery floor predicts");
        let expected_x: V5 = [3910.0, 0.0, 230.0, 0.0, 0.0];
        let expected_p: M5 = [
            [361707.1803187536, 0.0, 58764.498164412995, 0.0, 0.0],
            [
                0.0,
                1194568.4947803435,
                0.0,
                112346.72892929845,
                43.68854912882645,
            ],
            [58764.498164412995, 0.0, 9560.446624909415, 0.0, 0.0],
            [
                0.0,
                112346.72892929845,
                0.0,
                10570.285974350756,
                4.109167501935302,
            ],
            [0.0, 43.68854912882645, 0.0, 4.109167501935302, 0.001651],
        ];
        for i in 0..N {
            let x_tol = 128.0 * f64::EPSILON * expected_x[i].abs().max(1.0);
            assert!(
                (actual.x[i] - expected_x[i]).abs() <= x_tol,
                "mean[{i}] {} != {}",
                actual.x[i],
                expected_x[i]
            );
            for j in 0..N {
                let tol = 128.0 * f64::EPSILON * expected_p[i][j].abs().max(1.0);
                assert!(
                    (actual.p[i][j] - expected_p[i][j]).abs() <= tol,
                    "p[{i}][{j}] {} != {}",
                    actual.p[i][j],
                    expected_p[i][j]
                );
                assert_eq!(actual.p[i][j], actual.p[j][i]);
            }
        }
    }

    // REQ: OA-TRK-329
    #[test]
    fn strict_cholesky_reconstructs_psd_and_refuses_invalid_covariances() {
        let mut rank_one = ZERO;
        rank_one[0][0] = 4.0;
        rank_one[0][1] = 2.0;
        rank_one[1][0] = 2.0;
        rank_one[1][1] = 1.0;
        let l = cholesky_psd(&rank_one).expect("rank-one PSD covariance is valid");
        for i in 0..N {
            for j in 0..N {
                let mut reconstructed = 0.0;
                for k in 0..N {
                    reconstructed += l[i][k] * l[j][k];
                }
                assert_eq!(reconstructed, rank_one[i][j]);
            }
        }

        let mut negative = ZERO;
        negative[4][4] = -f64::EPSILON;
        assert!(cholesky_psd(&negative).is_none());
        let mut bad_zero_pivot = ZERO;
        bad_zero_pivot[0][1] = 1.0;
        bad_zero_pivot[1][0] = 1.0;
        assert!(cholesky_psd(&bad_zero_pivot).is_none());
        let mut asymmetric = ZERO;
        asymmetric[0][1] = 1.0;
        assert!(cholesky_psd(&asymmetric).is_none());

        let invalid_omega = HModel {
            x: [0.0; N],
            p: negative,
        };
        assert!(ct_predict(&invalid_omega, 4.8).is_none());
        let mut invalid_cross = ZERO;
        invalid_cross[0][4] = 1.0;
        invalid_cross[4][0] = 1.0;
        let invalid_omega_cross = HModel {
            x: [0.0; N],
            p: invalid_cross,
        };
        assert!(ct_predict(&invalid_omega_cross, 4.8).is_none());
        let mut non_finite = ZERO;
        non_finite[2][2] = f64::INFINITY;
        assert!(cholesky_psd(&non_finite).is_none());
    }

    // REQ: OA-TRK-329
    #[test]
    fn cubature_prediction_is_deterministic_under_mirror_reflection() {
        let mut p = ZERO;
        p[0][0] = 25.0;
        p[1][1] = 36.0;
        p[2][2] = 4.0;
        p[3][3] = 9.0;
        p[4][4] = 0.04 * 0.04;
        let x = [10.0, 20.0, 230.0, 15.0, 0.05];
        let signs = [1.0, -1.0, 1.0, -1.0, -1.0];
        let mirrored_x = [10.0, -20.0, 230.0, -15.0, -0.05];
        let h = HFilter {
            m: [HModel { x, p }, HModel { x, p }],
            mu: [0.0, 1.0],
        };
        let mirrored = HFilter {
            m: [HModel { x: mirrored_x, p }, HModel { x: mirrored_x, p }],
            mu: [0.0, 1.0],
        };
        let a = ct_predict(&h.m[1], 4.8).expect("valid original covariance");
        let b = ct_predict(&mirrored.m[1], 4.8).expect("valid mirrored covariance");
        for i in 0..N {
            assert!((b.x[i] - signs[i] * a.x[i]).abs() <= 1.0e-12 * a.x[i].abs().max(1.0));
            for j in 0..N {
                assert!(
                    (b.p[i][j] - signs[i] * signs[j] * a.p[i][j]).abs()
                        <= 1.0e-12 * a.p[i][j].abs().max(1.0)
                );
            }
        }
    }
}

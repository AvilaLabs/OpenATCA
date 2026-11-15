//! The horizontal interacting-multiple-model filter: a nearly-constant-velocity model and a
//! coordinated-turn model with the turn rate as a state, in a local east-north plane. Core
//! (pure).
//!
//! Both models share one state `[east, north, east velocity, north velocity, turn rate]` so the
//! mixing step is a plain weighted sum. The straight model keeps the turn rate exactly zero
//! (its row and column of the transition matrix are zero). The turn model is an extended
//! Kalman filter: with `theta = turn rate * dt` the position advances by
//! `v (sin theta / w)` along and `(1 - cos theta) / w` across, and the velocity rotates by
//! `theta`; the Jacobian column of the turn rate is written out, with series for small `theta`
//! so the straight-flight limit is smooth.
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

#![allow(clippy::indexing_slicing, clippy::needless_range_loop)]

use crate::linalg::{
    add, ident, matvec, mul, outer, sandwich, scale, symmetrise, transpose, M5, V5, ZERO,
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

/// `(sin t / t, (1 - cos t) / t)` and the derivatives of `sin t / w` and `(1 - cos t) / w` with
/// respect to `w = t / dt`, divided by `dt^2`: `((a1, b1), (da, db))`; `s` and `c` are `sin t`
/// and `cos t`.
fn ct_terms(theta: f64, s: f64, c: f64) -> ((f64, f64), (f64, f64)) {
    if theta.abs() < 1.0e-4 {
        let t2 = theta * theta;
        (
            (1.0 - t2 / 6.0, theta / 2.0 - theta * t2 / 24.0),
            (-theta / 3.0, 0.5 - t2 / 8.0),
        )
    } else {
        (
            (s / theta, (1.0 - c) / theta),
            (
                (theta * c - s) / (theta * theta),
                (theta * s - (1.0 - c)) / (theta * theta),
            ),
        )
    }
}

/// Turn model prediction over `dt`.
fn ct_predict(m: &HModel, dt: f64) -> HModel {
    let mut p0 = m.p;
    // A turn can start at any time: the prior never claims more certainty than the floor.
    let w_var = p0.get(4).and_then(|r| r.get(4)).copied().unwrap_or(0.0);
    set(&mut p0, 4, 4, w_var.max(omega_var_floor()));
    let (x, y, vx, vy) = (g(&m.x, 0), g(&m.x, 1), g(&m.x, 2), g(&m.x, 3));
    let w = g(&m.x, 4).clamp(-OMEGA_MAX, OMEGA_MAX);
    let theta = w * dt;
    let (s, c) = sin_cos_near_zero(theta);
    let ((a1, b1), (da, db)) = ct_terms(theta, s, c);
    let (a, b) = (dt * a1, dt * b1);
    let (da, db) = (dt * dt * da, dt * dt * db);
    let xn = [
        x + vx * a - vy * b,
        y + vx * b + vy * a,
        vx * c - vy * s,
        vx * s + vy * c,
        w,
    ];
    let mut f = ident();
    set(&mut f, 0, 2, a);
    set(&mut f, 0, 3, -b);
    set(&mut f, 0, 4, vx * da - vy * db);
    set(&mut f, 1, 2, b);
    set(&mut f, 1, 3, a);
    set(&mut f, 1, 4, vx * db + vy * da);
    set(&mut f, 2, 2, c);
    set(&mut f, 2, 3, -s);
    set(&mut f, 2, 4, dt * (-vx * s - vy * c));
    set(&mut f, 3, 2, s);
    set(&mut f, 3, 3, c);
    set(&mut f, 3, 4, dt * (vx * c - vy * s));
    let mut p = sandwich(&f, &p0);
    add_accel_noise(&mut p, dt, H_Q_CT);
    add_to(&mut p, 4, 4, H_Q_OMEGA * dt);
    HModel {
        x: xn,
        p: symmetrise(&p),
    }
}

/// Mixes the two models for the next step and predicts each over `dt`. The returned filter's
/// `mu` is the predicted model probability.
pub(crate) fn h_predict(h: &HFilter, dt: f64, u: f64, mode: u8) -> HFilter {
    if mode != 0 {
        // Single straight-model filter (the comparison baseline): no mixing, no turn model.
        return HFilter {
            m: [cv_predict(&h.m[0], dt, cv_only_q(mode)), h.m[1]],
            mu: [1.0, 0.0],
        };
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
    HFilter {
        m: [cv_predict(&mixed[0], dt, H_Q_CV), ct_predict(&mixed[1], dt)],
        mu: c,
    }
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
}

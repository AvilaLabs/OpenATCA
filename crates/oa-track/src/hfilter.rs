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
use crate::params::{
    cv_only_q, CT_OMEGA_STD_FLOOR, H_MU0_CV, H_PRIOR_VEL_STD, H_Q_CT, H_Q_CV, H_Q_OMEGA,
    H_TAU_CT_S, H_TAU_CV_S, MU_FLOOR, OMEGA_MAX,
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
pub(crate) fn h_transition(dt: f64) -> [[f64; 2]; 2] {
    let s0 = (-dt / H_TAU_CV_S).exp();
    let s1 = (-dt / H_TAU_CT_S).exp();
    [[s0, 1.0 - s0], [1.0 - s1, s1]]
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
/// respect to `w = t / dt`, divided by `dt^2`: `((a1, b1), (da, db))`.
fn ct_terms(theta: f64) -> ((f64, f64), (f64, f64)) {
    if theta.abs() < 1.0e-4 {
        let t2 = theta * theta;
        (
            (1.0 - t2 / 6.0, theta / 2.0 - theta * t2 / 24.0),
            (-theta / 3.0, 0.5 - t2 / 8.0),
        )
    } else {
        let (s, c) = theta.sin_cos();
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
    let ((a1, b1), (da, db)) = ct_terms(theta);
    let (a, b) = (dt * a1, dt * b1);
    let (da, db) = (dt * dt * da, dt * dt * db);
    let (s, c) = theta.sin_cos();
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
pub(crate) fn h_predict(h: &HFilter, dt: f64, mode: u8) -> HFilter {
    if mode != 0 {
        // Single straight-model filter (the comparison baseline): no mixing, no turn model.
        return HFilter {
            m: [cv_predict(&h.m[0], dt, cv_only_q(mode)), h.m[1]],
            mu: [1.0, 0.0],
        };
    }
    let pi = h_transition(dt);
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
/// covariance `r`): updates `x` and `p` and returns the log likelihood (without the constant
/// shared by every model).
fn update_sel(x: &mut V5, p: &mut M5, i: [usize; 2], z: &[f64; 2], r: &[f64; 3]) -> f64 {
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
        return f64::NEG_INFINITY;
    }
    let (n00, n01, n11) = (s11 / det, -s01 / det, s00 / det);
    let d2 = y0 * (n00 * y0 + n01 * y1) + y1 * (n01 * y0 + n11 * y1);
    let ll = -0.5 * d2 - 0.5 * det.ln();
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
    ll
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
    let mut lw = [0.0f64; 2];
    for (j, m) in out.m.iter_mut().enumerate() {
        let mut ll = update_sel(&mut m.x, &mut m.p, [0, 1], z, r);
        if let Some((v, vr)) = vel {
            ll += update_sel(&mut m.x, &mut m.p, [2, 3], &v, &vr);
        }
        if let Some(w) = m.x.get_mut(4) {
            *w = w.clamp(-OMEGA_MAX, OMEGA_MAX);
        }
        if j == 0 {
            // The straight model keeps its turn rate at zero by construction.
            if let Some(w) = m.x.get_mut(4) {
                *w = 0.0;
            }
        }
        lw[j] = pred.mu.get(j).copied().unwrap_or(0.0).max(1.0e-300).ln() + ll;
    }
    let mx = lw[0].max(lw[1]);
    let mut mu = if mx.is_finite() {
        [(lw[0] - mx).exp(), (lw[1] - mx).exp()]
    } else {
        pred.mu
    };
    let s = mu[0] + mu[1];
    mu = [mu[0] / s, mu[1] / s];
    mu = [mu[0].max(MU_FLOOR), mu[1].max(MU_FLOOR)];
    let s = mu[0] + mu[1];
    out.mu = [mu[0] / s, mu[1] / s];
    out
}

/// The velocity rotation of a frame change: applies the 2 x 2 `r` to the velocity block of
/// every model state and covariance.
pub(crate) fn h_rotate_velocity(h: &mut HFilter, r: &[[f64; 2]; 2]) {
    let mut t = ident();
    set(&mut t, 2, 2, r[0][0]);
    set(&mut t, 2, 3, r[0][1]);
    set(&mut t, 3, 2, r[1][0]);
    set(&mut t, 3, 3, r[1][1]);
    for m in h.m.iter_mut() {
        m.x = matvec(&t, &m.x);
        m.p = symmetrise(&mul(&mul(&t, &m.p), &transpose(&t)));
    }
}

/// Shifts every model's position by `(dx, dy)` (the frame origin moved by `-(dx, dy)`).
pub(crate) fn h_shift(h: &mut HFilter, dx: f64, dy: f64) {
    for m in h.m.iter_mut() {
        if let Some(v) = m.x.get_mut(0) {
            *v += dx;
        }
        if let Some(v) = m.x.get_mut(1) {
            *v += dy;
        }
    }
}

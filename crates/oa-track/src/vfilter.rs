//! The vertical interacting-multiple-model filter: a level model and a climb/descent model on
//! barometric altitude (feet) and vertical rate (feet per second). Core (pure).
//!
//! The level model keeps the rate exactly zero and lets the altitude walk a little; the climb
//! model is a constant-rate model with white vertical acceleration. Altitude quantisation (25
//! ft for ADS-B, 100 ft for Mode C) enters as measurement noise of the matching uniform
//! variance `step^2 / 12`, plus a small base term, so a level aircraft crossing a quantisation
//! boundary is not read as a climb. As in the horizontal filter the Markov transition depends
//! on the time step. Fixed two-element arrays are indexed by the loop bound, the same exception
//! as `linalg.rs`.
#![allow(clippy::indexing_slicing, clippy::needless_range_loop)]

use crate::params::{
    MU_FLOOR, V_MU0_LEVEL, V_PRIOR_RATE_STD_FPS, V_Q_CLIMB, V_Q_LEVEL, V_TAU_CLIMB_S, V_TAU_LEVEL_S,
};

/// One model: `[altitude, rate]` and the covariance packed `[p00, p01, p11]`. Hot (A7).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct VModel {
    pub x: [f64; 2],
    pub p: [f64; 3],
}
const _: () = assert!(core::mem::size_of::<VModel>() == 40);

/// The two models and their probabilities `[level, climb or descent]`. Hot (A7).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct VFilter {
    pub m: [VModel; 2],
    pub mu: [f64; 2],
}
const _: () = assert!(core::mem::size_of::<VFilter>() == 96);

/// A filter at altitude `alt_ft` (variance `alt_var`), with a rate in feet per second and its
/// variance if known.
pub(crate) fn v_init(alt_ft: f64, alt_var: f64, rate: Option<(f64, f64)>) -> VFilter {
    let (r, rv) = rate.unwrap_or((0.0, V_PRIOR_RATE_STD_FPS * V_PRIOR_RATE_STD_FPS));
    VFilter {
        m: [
            VModel {
                x: [alt_ft, 0.0],
                p: [alt_var, 0.0, 0.0],
            },
            VModel {
                x: [alt_ft, r],
                p: [alt_var, 0.0, rv],
            },
        ],
        mu: [V_MU0_LEVEL, 1.0 - V_MU0_LEVEL],
    }
}

/// The model transition probabilities over `dt` seconds.
pub(crate) fn v_transition(dt: f64) -> [[f64; 2]; 2] {
    let s0 = (-dt / V_TAU_LEVEL_S).exp();
    let s1 = (-dt / V_TAU_CLIMB_S).exp();
    [[s0, 1.0 - s0], [1.0 - s1, s1]]
}

fn level_predict(m: &VModel, dt: f64) -> VModel {
    VModel {
        x: [m.x[0], 0.0],
        p: [m.p[0] + V_Q_LEVEL * dt, 0.0, 0.0],
    }
}

fn climb_predict(m: &VModel, dt: f64) -> VModel {
    let [p00, p01, p11] = m.p;
    let q = V_Q_CLIMB;
    VModel {
        x: [m.x[0] + dt * m.x[1], m.x[1]],
        p: [
            p00 + 2.0 * dt * p01 + dt * dt * p11 + q * dt * dt * dt / 3.0,
            p01 + dt * p11 + q * dt * dt / 2.0,
            p11 + q * dt,
        ],
    }
}

/// Mixes and predicts over `dt`; the returned `mu` is the predicted probability.
pub(crate) fn v_predict(v: &VFilter, dt: f64) -> VFilter {
    let pi = v_transition(dt);
    let mut mixed = v.m;
    let mut c = [0.0f64; 2];
    for j in 0..2 {
        let cj = pi[0][j] * v.mu[0] + pi[1][j] * v.mu[1];
        c[j] = cj;
        let w = if cj > 0.0 {
            [pi[0][j] * v.mu[0] / cj, pi[1][j] * v.mu[1] / cj]
        } else {
            [1.0, 0.0]
        };
        let mut x = [0.0; 2];
        for i in 0..2 {
            x[0] += w[i] * v.m[i].x[0];
            x[1] += w[i] * v.m[i].x[1];
        }
        let mut p = [0.0; 3];
        for i in 0..2 {
            let (d0, d1) = (v.m[i].x[0] - x[0], v.m[i].x[1] - x[1]);
            p[0] += w[i] * (v.m[i].p[0] + d0 * d0);
            p[1] += w[i] * (v.m[i].p[1] + d0 * d1);
            p[2] += w[i] * (v.m[i].p[2] + d1 * d1);
        }
        mixed[j] = VModel { x, p };
    }
    VFilter {
        m: [level_predict(&mixed[0], dt), climb_predict(&mixed[1], dt)],
        mu: c,
    }
}

/// Probability-weighted altitude and rate with their covariance `[p00, p01, p11]`.
pub(crate) fn v_combine(v: &VFilter) -> ([f64; 2], [f64; 3]) {
    let mut x = [0.0; 2];
    for i in 0..2 {
        x[0] += v.mu[i] * v.m[i].x[0];
        x[1] += v.mu[i] * v.m[i].x[1];
    }
    let mut p = [0.0; 3];
    for i in 0..2 {
        let (d0, d1) = (v.m[i].x[0] - x[0], v.m[i].x[1] - x[1]);
        p[0] += v.mu[i] * (v.m[i].p[0] + d0 * d0);
        p[1] += v.mu[i] * (v.m[i].p[1] + d0 * d1);
        p[2] += v.mu[i] * (v.m[i].p[2] + d1 * d1);
    }
    (x, p)
}

/// The altitude innovation `(z - predicted, innovation variance)` of the combined estimate.
pub(crate) fn v_alt_innovation(pred: &VFilter, z: f64, var: f64) -> (f64, f64) {
    let (x, p) = v_combine(pred);
    (z - x[0], p[0] + var)
}

/// Scalar update of state `i` (0 altitude, 1 rate) of one model; returns the log likelihood.
fn update_scalar(m: &mut VModel, i: usize, z: f64, r: f64) -> f64 {
    let (pii, p0i, p1i) = if i == 0 {
        (m.p[0], m.p[0], m.p[1])
    } else {
        (m.p[2], m.p[1], m.p[2])
    };
    let s = pii + r;
    if s.is_nan() || s <= 0.0 {
        return f64::NEG_INFINITY;
    }
    let y = z - m.x[i];
    let (k0, k1) = (p0i / s, p1i / s);
    m.x[0] += k0 * y;
    m.x[1] += k1 * y;
    m.p = [
        m.p[0] - p0i * p0i / s,
        m.p[1] - p0i * p1i / s,
        m.p[2] - p1i * p1i / s,
    ];
    -0.5 * y * y / s - 0.5 * s.ln()
}

/// Updates the predicted filter with an altitude (variance `var`, ft^2) and a rate (feet per
/// second, variance) if given; then the model probabilities.
pub(crate) fn v_update(
    pred: &VFilter,
    alt: Option<(f64, f64)>,
    rate: Option<(f64, f64)>,
) -> VFilter {
    let mut out = *pred;
    let mut lw = [0.0f64; 2];
    for j in 0..2 {
        let mut ll = 0.0;
        if let Some((z, r)) = alt {
            ll += update_scalar(&mut out.m[j], 0, z, r);
        }
        if let Some((z, r)) = rate {
            ll += update_scalar(&mut out.m[j], 1, z, r);
        }
        if j == 0 {
            out.m[0].x[1] = 0.0;
        }
        lw[j] = pred.mu[j].max(1.0e-300).ln() + ll;
    }
    let mx = lw[0].max(lw[1]);
    let mut mu = if mx.is_finite() {
        [(lw[0] - mx).exp(), (lw[1] - mx).exp()]
    } else {
        pred.mu
    };
    let s = mu[0] + mu[1];
    mu = [(mu[0] / s).max(MU_FLOOR), (mu[1] / s).max(MU_FLOOR)];
    let s = mu[0] + mu[1];
    out.mu = [mu[0] / s, mu[1] / s];
    out
}

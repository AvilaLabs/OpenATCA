//! The fixed-interval Rauch-Tung-Striebel smoother and its interpolation. Core (pure).
//!
//! Per axis the state is `[position, velocity, acceleration]` with a nearly-constant-acceleration
//! model: the acceleration is a random walk, i.e. white jerk of spectral density `q`
//! (m^2/s^5). East and north share one covariance because the model, the measurement noise
//! and the sample times are the same on both axes, so each report stores one packed 3 x 3
//! covariance and two 3-vectors, `[east, east velocity, east acceleration, north, ...]`.
//!
//! Interpolation at a time between two reports is the smoother's own: the forward estimate at
//! the earlier report is predicted to the query time and then smoothed against the smoothed
//! estimate at the later report, so the covariance between reports is not an approximation.

use crate::linalg::{
    add, add_v, inv, matvec, mul, pack, predict_cov, proc_noise, sub, sub_v, trans, transpose,
    unpack, M3, P6, V3,
};

/// Prior variance of the first position, m^2: diffuse against any report.
const PRIOR_POS_VAR: f64 = 1.0e6;
/// Prior variance of the first velocity, (m/s)^2 (250 m/s).
const PRIOR_VEL_VAR: f64 = 62_500.0;
/// Prior variance of the first acceleration, (m/s^2)^2 (30 m/s^2).
const PRIOR_ACC_VAR: f64 = 900.0;

/// `sqrt(2 ln 20)`: the 95 % radius of a circular normal, in sigmas.
pub const SIGMA95: f64 = 2.447_746_830_680_816;

/// The 95 % horizontal radius of a position variance, m. A variance that is NaN or infinite has
/// no radius (`None`): it must never read as zero uncertainty. A tiny negative one from rounding
/// is zero.
pub fn sigma95_h_m(var: f64) -> Option<f64> {
    var.is_finite().then(|| SIGMA95 * var.max(0.0).sqrt())
}

/// The smoother's output for one leg, one entry per report. A7.2: cold, per-leg scratch.
#[derive(Debug, Clone, Default)]
pub(crate) struct Smoothed {
    /// Forward (filtered) state.
    pub xf: Vec<[f64; 6]>,
    /// Forward covariance.
    pub pf: Vec<P6>,
    /// Smoothed state.
    pub xs: Vec<[f64; 6]>,
    /// Smoothed covariance.
    pub ps: Vec<P6>,
}

/// Borrowed columns of one leg: what interpolation needs. Plain data (A1); A7.2: transient view.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LegView<'a> {
    pub t_ns: &'a [i64],
    pub alt_ft: &'a [i32],
    pub vrate_fpm: &'a [i32],
    pub xf: &'a [[f64; 6]],
    pub pf: &'a [P6],
    pub xs: &'a [[f64; 6]],
    pub ps: &'a [P6],
}

fn split(x: &[f64; 6]) -> (V3, V3) {
    let [a, b, c, d, e, f] = *x;
    ([a, b, c], [d, e, f])
}

fn join(e: &V3, n: &V3) -> [f64; 6] {
    let [a, b, c] = *e;
    let [d, e, f] = *n;
    [a, b, c, d, e, f]
}

fn secs(dt_ns: i64) -> f64 {
    dt_ns as f64 / 1.0e9
}

/// One scalar-measurement Kalman update in Joseph form.
fn update(xp: &V3, pp: &M3, z: f64, r: f64) -> (V3, M3) {
    let [[p00, ..], [p10, ..], [p20, ..]] = *pp;
    let s = p00 + r;
    let k = [p00 / s, p10 / s, p20 / s];
    let [xp0, ..] = *xp;
    let innov = z - xp0;
    let x = add_v(xp, &k.map(|v| v * innov));
    let [k0, k1, k2] = k;
    let ikh: M3 = [[1.0 - k0, 0.0, 0.0], [-k1, 1.0, 0.0], [-k2, 0.0, 1.0]];
    let joseph = mul(&mul(&ikh, pp), &transpose(&ikh));
    let kkt: M3 = [
        [r * k0 * k0, r * k0 * k1, r * k0 * k2],
        [r * k1 * k0, r * k1 * k1, r * k1 * k2],
        [r * k2 * k0, r * k2 * k1, r * k2 * k2],
    ];
    (x, unpack(&pack(&add(&joseph, &kkt))))
}

/// Smooths one leg. `t_ns` strictly increasing, `ze`/`zn` east/north positions in metres,
/// `sigma` the 1-sigma noise per axis in metres, `q` the jerk spectral density. All slices
/// have the same length (the caller guarantees it; a shorter slice just ends the leg early).
pub(crate) fn smooth(t_ns: &[i64], ze: &[f64], zn: &[f64], sigma: &[f64], q: f64) -> Smoothed {
    let n = t_ns.len().min(ze.len()).min(zn.len()).min(sigma.len());
    let mut out = Smoothed::default();
    let mut xp_all: Vec<[f64; 6]> = Vec::with_capacity(n);
    let mut pp_all: Vec<P6> = Vec::with_capacity(n);
    let mut prev_t = 0i64;
    for k in 0..n {
        let (Some(&t), Some(&e), Some(&nn), Some(&sg)) =
            (t_ns.get(k), ze.get(k), zn.get(k), sigma.get(k))
        else {
            break;
        };
        let r = sg * sg;
        let (xpe, xpn, pp): (V3, V3, M3) = if k == 0 {
            (
                [e, 0.0, 0.0],
                [nn, 0.0, 0.0],
                [
                    [PRIOR_POS_VAR, 0.0, 0.0],
                    [0.0, PRIOR_VEL_VAR, 0.0],
                    [0.0, 0.0, PRIOR_ACC_VAR],
                ],
            )
        } else {
            let dt = secs(t - prev_t);
            let f = trans(dt);
            let (Some(xf), Some(pf)) = (out.xf.last(), out.pf.last()) else {
                break;
            };
            let (xe, xn) = split(xf);
            (
                matvec(&f, &xe),
                matvec(&f, &xn),
                predict_cov(&f, &unpack(pf), &proc_noise(dt, q)),
            )
        };
        xp_all.push(join(&xpe, &xpn));
        pp_all.push(pack(&pp));
        let (xe, p_new) = update(&xpe, &pp, e, r);
        let (xn, _) = update(&xpn, &pp, nn, r);
        out.xf.push(join(&xe, &xn));
        out.pf.push(pack(&p_new));
        prev_t = t;
    }
    // Backward pass.
    out.xs = out.xf.clone();
    out.ps = out.pf.clone();
    let m = out.xf.len();
    for k in (0..m.saturating_sub(1)).rev() {
        let (Some(&ta), Some(&tb)) = (t_ns.get(k), t_ns.get(k + 1)) else {
            continue;
        };
        let f = trans(secs(tb - ta));
        let (Some(xf), Some(pf), Some(xs1), Some(ps1), Some(xp1), Some(pp1)) = (
            out.xf.get(k),
            out.pf.get(k),
            out.xs.get(k + 1),
            out.ps.get(k + 1),
            xp_all.get(k + 1),
            pp_all.get(k + 1),
        ) else {
            continue;
        };
        let (xf, pf, xs1, ps1, xp1, pp1) = (*xf, *pf, *xs1, *ps1, *xp1, *pp1);
        let (xs, ps) = rts_step(&xf, &pf, &xs1, &ps1, &xp1, &unpack(&pp1), &f);
        if let Some(d) = out.xs.get_mut(k) {
            *d = xs;
        }
        if let Some(d) = out.ps.get_mut(k) {
            *d = ps;
        }
    }
    out
}

/// One smoothing step from the estimate at the later time back to the earlier one. `xp`/`pp`
/// are the later time's prediction from the earlier filtered estimate. If `pp` is singular the
/// earlier filtered estimate is kept.
fn rts_step(
    xf: &[f64; 6],
    pf: &P6,
    xs_next: &[f64; 6],
    ps_next: &P6,
    xp: &[f64; 6],
    pp: &M3,
    f: &M3,
) -> ([f64; 6], P6) {
    let Some(pp_inv) = inv(pp) else {
        return (*xf, *pf);
    };
    let pfm = unpack(pf);
    let c = mul(&mul(&pfm, &transpose(f)), &pp_inv);
    let (fe, fn_) = split(xf);
    let (se, sn) = split(xs_next);
    let (pe, pn) = split(xp);
    let xe = add_v(&fe, &matvec(&c, &sub_v(&se, &pe)));
    let xn = add_v(&fn_, &matvec(&c, &sub_v(&sn, &pn)));
    let dp = sub(&unpack(ps_next), pp);
    let ps = add(&pfm, &mul(&mul(&c, &dp), &transpose(&c)));
    (join(&xe, &xn), pack(&ps))
}

/// The smoothed state and covariance at `t_ns`, which lies in `[t[k], t[k+1]]` of the leg.
/// Returns `None` if `k` is out of range or `t_ns` is outside the interval.
pub(crate) fn interp(v: &LegView<'_>, k: usize, t_ns: i64, q: f64) -> Option<([f64; 6], P6)> {
    let (&ta, &tb) = (v.t_ns.get(k)?, v.t_ns.get(k + 1).or(v.t_ns.get(k))?);
    if t_ns < ta || t_ns > tb {
        return None;
    }
    if t_ns == ta {
        return Some((*v.xs.get(k)?, *v.ps.get(k)?));
    }
    if t_ns == tb {
        let k1 = if v.t_ns.get(k + 1).is_some() {
            k + 1
        } else {
            k
        };
        return Some((*v.xs.get(k1)?, *v.ps.get(k1)?));
    }
    let (tau, d) = (secs(t_ns - ta), secs(tb - t_ns));
    let xf = v.xf.get(k)?;
    let pf = unpack(v.pf.get(k)?);
    let f1 = trans(tau);
    let (xe, xn) = split(xf);
    let (xt_e, xt_n) = (matvec(&f1, &xe), matvec(&f1, &xn));
    let pt = predict_cov(&f1, &pf, &proc_noise(tau, q));
    let f2 = trans(d);
    let (xn_e, xn_n) = (matvec(&f2, &xt_e), matvec(&f2, &xt_n));
    let pn = predict_cov(&f2, &pt, &proc_noise(d, q));
    let xs1 = v.xs.get(k + 1)?;
    let ps1 = unpack(v.ps.get(k + 1)?);
    let Some(pn_inv) = inv(&pn) else {
        return Some((join(&xt_e, &xt_n), pack(&pt)));
    };
    let c = mul(&mul(&pt, &transpose(&f2)), &pn_inv);
    let (se, sn) = split(xs1);
    let xe = add_v(&xt_e, &matvec(&c, &sub_v(&se, &xn_e)));
    let xn = add_v(&xt_n, &matvec(&c, &sub_v(&sn, &xn_n)));
    let dp = sub(&ps1, &pn);
    let ps = add(&pt, &mul(&mul(&c, &dp), &transpose(&c)));
    Some((join(&xe, &xn), pack(&ps)))
}

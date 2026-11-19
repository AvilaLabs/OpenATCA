//! The fixed-interval Rauch-Tung-Striebel smoother and its interpolation. Core (pure, B7).
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

use super::linalg::{
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
    var.is_finite()
        .then(|| SIGMA95 * oa_ieee::sqrt(oa_math::max_f64(var, 0.0)))
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
            // Invariant: a leg's steps are in (0, leg_gap_ms] (the builder splits at longer ones;
            // leave-one-out drops non-adjacent reports, at most doubling a step).
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
        // Invariant: as in the forward pass, a step is at most leg_gap_ms.
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
    // A difference past the range of an i64 is no interval to interpolate in.
    let (tau, d) = (secs(t_ns.checked_sub(ta)?), secs(tb.checked_sub(t_ns)?));
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

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    // Reference values: the Kalman filter and Rauch-Tung-Striebel equations of the module
    // documentation (prior diag(1e6, 62500, 900), F(dt), Q(dt, q) of white jerk) evaluated
    // independently in double precision for a three-report leg: times 0, 2 and 5 s, east
    // 0, 10, 17 m, north 0, -3, -4 m, per-axis sigma 3, 2 and 4 m, q = 0.1.
    const T: [i64; 3] = [0, 2_000_000_000, 5_000_000_000];
    const ZE: [f64; 3] = [0.0, 10.0, 17.0];
    const ZN: [f64; 3] = [0.0, -3.0, -4.0];
    const SG: [f64; 3] = [3.0, 2.0, 4.0];
    const XF: [[f64; 6]; 3] = [
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [
            9.999842279477877,
            5.070722672320022,
            0.07097949230663987,
            -2.9999526838433628,
            -1.5212168016960064,
            -0.02129384769199196,
        ],
        [
            17.002731153964827,
            0.7370956151244927,
            -1.0651955224709098,
            -4.001171488044079,
            0.3650836052132238,
            0.46605159783548217,
        ],
    ];
    const PF: [[f64; 6]; 3] = [
        [8.999919000663795, 0.0, 0.0, 62500.0, 0.0, 900.0],
        [
            3.9999369117431343,
            2.0282890689122723,
            0.02839179692250582,
            890.6716861777313,
            887.4023093467652,
            887.4227449915741,
        ],
        [
            15.994877936660487,
            8.52672421456009,
            2.1308064159156856,
            8.310209787750864,
            2.956548722280786,
            1.2956001769971408,
        ],
    ];
    const XS: [[f64; 6]; 3] = [
        [
            0.0027408980420973172,
            6.062660386668204,
            -1.064903269348937,
            -0.0011299126512589464,
            -1.9649969630835127,
            0.46592556232962556,
        ],
        [
            9.99809902363962,
            3.9326245722509334,
            -1.0651187087706713,
            -2.999204940069637,
            -1.0330464772141434,
            0.46601864973639684,
        ],
        XF[2],
    ];
    const PS: [[f64; 6]; 3] = [
        [
            8.99569027370532,
            -6.29365837817107,
            1.797182101859386,
            7.54944688348769,
            -2.7619990396889844,
            1.26165876137361,
        ],
        [
            3.9978501405330826,
            0.665924299495122,
            -1.3315789856028333,
            1.241275765733576,
            -0.465168150283489,
            1.1154539784233748,
        ],
        PF[2],
    ];

    /// `rel` is the relative tolerance. The prior (variance 1e6 against reports of a few m^2)
    /// makes the backward step ill-conditioned: the cofactor inverse and a reference inverse
    /// differ by about 1e-9 in a state, 1e-6 in a velocity or acceleration variance and 2e-4 in a
    /// covariance between two reports. A wrong term in the equations is orders of magnitude more.
    fn close(got: &[f64; 6], want: &[f64; 6], what: &str, rel: f64) {
        for i in 0..6 {
            let tol = rel * want[i].abs().max(1.0);
            assert!(
                (got[i] - want[i]).abs() <= tol,
                "{what}[{i}]: {got:?} vs {want:?}"
            );
        }
    }

    fn run() -> Smoothed {
        smooth(&T, &ZE, &ZN, &SG, 0.1)
    }

    // REQ: OA-TRUTH-058
    #[test]
    fn the_smoother_gives_the_rts_state_and_full_covariance() {
        let sm = run();
        for k in 0..3 {
            close(&sm.xf[k], &XF[k], "xf", 1e-7);
            close(&sm.pf[k], &PF[k], "pf", 1e-7);
            close(&sm.xs[k], &XS[k], "xs", 1e-7);
            close(&sm.ps[k], &PS[k], "ps", 1e-3);
        }
    }

    fn view(sm: &Smoothed) -> LegView<'_> {
        LegView {
            t_ns: &T,
            alt_ft: &[0, 0, 0],
            vrate_fpm: &[0, 0, 0],
            xf: &sm.xf,
            pf: &sm.pf,
            xs: &sm.xs,
            ps: &sm.ps,
        }
    }

    // REQ: OA-TRUTH-058
    #[test]
    fn interpolation_between_reports_is_the_smoothed_state_and_covariance() {
        let sm = run();
        let v = view(&sm);
        let (x, p) = interp(&v, 0, 1_000_000_000, 0.1).unwrap();
        close(
            &x,
            &[
                5.532929779196803,
                4.997697608244749,
                -1.0650213662803771,
                -1.7331554399364373,
                -1.4990455151085724,
                0.4659768113845958,
            ],
            "x(1 s)",
            1e-7,
        );
        close(
            &p,
            &[
                3.3040729603962973,
                -0.47049487025651615,
                -0.3438351543550766,
                3.2494393707820564,
                -1.557940101901636,
                1.1685894478256387,
            ],
            "p(1 s)",
            1e-3,
        );
        let (x, p) = interp(&v, 1, 3_500_000_000, 0.1).unwrap();
        close(
            &x,
            &[
                14.698743848688288,
                2.334885298183951,
                -1.0651859207587038,
                -4.0244893116304885,
                -0.33399224709583564,
                0.4660474793232357,
            ],
            "x(3.5 s)",
            1e-7,
        );
        close(
            &p,
            &[
                5.534846254485274,
                0.7152306645921271,
                -0.8821793776573941,
                2.229108925026594,
                1.1415793969213155,
                1.1605823448156798,
            ],
            "p(3.5 s)",
            1e-3,
        );
    }

    // REQ: OA-TRUTH-058
    #[test]
    fn interpolation_at_a_report_time_is_that_reports_smoothed_estimate_and_outside_is_none() {
        let sm = run();
        let v = view(&sm);
        // At the later end of an interval: the later report's estimate, for the first and the
        // second interval.
        for (k, at) in [(0usize, 2_000_000_000i64), (1, 5_000_000_000)] {
            let (x, p) = interp(&v, k, at, 0.1).unwrap();
            assert_eq!(x, sm.xs[k + 1]);
            assert_eq!(p, sm.ps[k + 1]);
        }
        // At the earlier end: the earlier report's.
        let (x, p) = interp(&v, 1, 2_000_000_000, 0.1).unwrap();
        assert_eq!((x, p), (sm.xs[1], sm.ps[1]));
        // Outside [t[k], t[k+1]] on either side there is no answer.
        assert!(interp(&v, 1, 1_999_999_999, 0.1).is_none());
        assert!(interp(&v, 1, 5_000_000_001, 0.1).is_none());
        assert!(interp(&v, 0, -1, 0.1).is_none());
        assert!(interp(&v, 0, 2_000_000_001, 0.1).is_none());
    }
}

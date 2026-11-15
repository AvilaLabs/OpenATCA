//! Exact synthetic trajectories as an `oa-truth` [`Truth`]: smooth paths (straight, constant-rate
//! turns, climbs) written directly into the truth model so the radar simulator
//! (`oa-sensorsim`) and the scorer read the very same function. Core (pure).
//!
//! # Why this is exact truth
//!
//! The simulator answers "where is the aircraft at time t" with `oa_truth::truth_at`, and the
//! scorer asks the same function for the same aircraft at the track row's time. A tracker's
//! error is therefore measured against exactly the trajectory the plots were made from: the
//! truth's own error is zero by construction. The trajectory itself is the analytic path
//! (constant speed along a heading that turns at a constant rate, exact integrals) sampled
//! every [`KNOT_S`] seconds with its exact position, velocity and acceleration; `truth_at`
//! interpolates between the knots with its smoother's own model, which for these paths departs
//! from the analytic curve by under a centimetre where the acceleration is continuous and by up
//! to a few tenths of a metre in the one knot interval around the instant a turn starts or
//! stops (the acceleration steps there); both measured in the tests. That departure is not an
//! error of the study: the interpolated path *is* the truth.
//! The covariance written at each knot is a nominal [`KNOT_VAR_M2`] (1 cm one sigma), so no
//! sample is refused as too uncertain.
//!
//! Paths are planned in the azimuthal-equidistant plane about the radar (`oa_geo::aeqd`, the
//! frame `oa-truth` itself uses per leg: the leg origin is the radar), so a plan in metres is a
//! plan in metres from the radar; the speeds and distances of the plan are nominal to the
//! frame's 0.3 % scale error, which does not matter because truth is what `truth_at` returns.

use oa_adsb::NONE_I32;
use oa_truth::{Truth, TruthConfig, DEFAULT_CONFIG};

/// Spacing of the exact knots, seconds.
pub const KNOT_S: f64 = 1.0;
/// Nominal position variance written at each knot, m^2 (1 cm).
pub const KNOT_VAR_M2: f64 = 1.0e-4;
/// Limit: aircraft in one synthetic truth.
pub const MAX_TRAJ: usize = 4096;
/// Limit: knots per aircraft (a day at one per second is 86,400).
pub const MAX_KNOTS: usize = 90_000;

/// One segment of a path: for `dur_s` seconds a constant turn rate and vertical rate.
/// Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seg {
    pub dur_s: f64,
    /// Turn rate, degrees per second, clockwise positive (a heading increase).
    pub turn_dps: f64,
    /// Vertical rate, feet per minute.
    pub vrate_fpm: f64,
}

/// One aircraft's path. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, PartialEq)]
pub struct Traj {
    /// 24-bit address, the aircraft's key.
    pub addr: u32,
    /// Start time, seconds after the epoch.
    pub t0_s: f64,
    /// Start position in the radar plane, metres east and north.
    pub x0: f64,
    pub y0: f64,
    /// Start heading, degrees clockwise from north.
    pub heading_deg: f64,
    pub speed_mps: f64,
    pub alt_ft: f64,
    pub segs: Vec<Seg>,
}

/// The state of a path at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub e: f64,
    pub n: f64,
    pub ve: f64,
    pub vn: f64,
    pub ae: f64,
    pub an: f64,
    pub alt_ft: f64,
    pub vrate_fpm: f64,
    pub heading_deg: f64,
}

/// Total duration of a path, seconds.
pub fn duration_s(t: &Traj) -> f64 {
    t.segs.iter().map(|s| s.dur_s).sum()
}

/// The exact state of `t` at `dt` seconds after its start (`0 <= dt <= duration`).
pub fn state_at(t: &Traj, dt: f64) -> State {
    let (mut e, mut n, mut h) = (t.x0, t.y0, t.heading_deg.to_radians());
    let mut alt = t.alt_ft;
    let mut left = dt.max(0.0);
    let mut last = (0.0, 0.0);
    for s in &t.segs {
        let step = left.min(s.dur_s);
        let w = s.turn_dps.to_radians();
        let h1 = h + w * step;
        if w.abs() < 1.0e-12 {
            e += t.speed_mps * h.sin() * step;
            n += t.speed_mps * h.cos() * step;
        } else {
            e += t.speed_mps / w * (h.cos() - h1.cos());
            n += t.speed_mps / w * (h1.sin() - h.sin());
        }
        alt += s.vrate_fpm / 60.0 * step;
        h = h1;
        last = (w, s.vrate_fpm);
        left -= step;
        if left <= 0.0 {
            break;
        }
    }
    let (w, vr) = last;
    State {
        e,
        n,
        ve: t.speed_mps * h.sin(),
        vn: t.speed_mps * h.cos(),
        ae: t.speed_mps * w * h.cos(),
        an: -t.speed_mps * w * h.sin(),
        alt_ft: alt,
        vrate_fpm: vr,
        heading_deg: h.to_degrees().rem_euclid(360.0),
    }
}

/// Why a synthetic truth could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthError(pub String);

impl std::fmt::Display for SynthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SynthError {}

/// The truth of `trajs` in the plane about `(lat_deg, lon_deg)` with the clock starting at
/// `epoch_ns`. Refuses duplicate addresses, an empty or over-long path, a non-finite value and
/// more than [`MAX_TRAJ`] aircraft (E5).
pub fn build_synthetic_truth(
    lat_deg: f64,
    lon_deg: f64,
    epoch_ns: i64,
    trajs: &[Traj],
) -> Result<Truth, SynthError> {
    let bad = |w: &str| Err(SynthError(w.to_string()));
    if trajs.is_empty() || trajs.len() > MAX_TRAJ {
        return bad("aircraft count");
    }
    let mut order: Vec<usize> = (0..trajs.len()).collect();
    order.sort_by_key(|&i| trajs.get(i).map_or(0, |t| t.addr));
    let mut tr = Truth {
        cfg: TruthConfig {
            loo: false,
            ..DEFAULT_CONFIG
        },
        ..Truth::default()
    };
    let mut prev: Option<u32> = None;
    for &i in &order {
        let Some(t) = trajs.get(i) else { continue };
        if prev == Some(t.addr) {
            return bad("duplicate address");
        }
        prev = Some(t.addr);
        let fin = [t.t0_s, t.x0, t.y0, t.heading_deg, t.speed_mps, t.alt_ft];
        if !fin.iter().all(|v| v.is_finite()) || t.segs.is_empty() {
            return bad("path values");
        }
        if t.segs.iter().any(|s| {
            !(s.dur_s.is_finite()
                && s.dur_s > 0.0
                && s.turn_dps.is_finite()
                && s.vrate_fpm.is_finite())
        }) {
            return bad("segment values");
        }
        let dur = duration_s(t);
        let knots = (dur / KNOT_S).floor() as usize + 1;
        if !(2..=MAX_KNOTS).contains(&knots) {
            return bad("path duration");
        }
        tr.ac_addr.push(t.addr);
        tr.ac_first_leg.push(tr.leg_addr.len() as u32);
        tr.leg_addr.push(t.addr);
        tr.leg_first_pt.push(tr.pt_t_ns.len() as u32);
        tr.leg_lat0.push(lat_deg);
        tr.leg_lon0.push(lon_deg);
        for k in 0..knots {
            let dt = k as f64 * KNOT_S;
            let s = state_at(t, dt);
            let ns = epoch_ns + ((t.t0_s + dt) * 1.0e9).round() as i64;
            tr.pt_t_ns.push(ns);
            tr.pt_alt_ft.push(s.alt_ft.round() as i32);
            tr.pt_vrate_fpm.push(if s.vrate_fpm == 0.0 {
                0
            } else {
                s.vrate_fpm.round() as i32
            });
            let x = [s.e, s.ve, s.ae, s.n, s.vn, s.an];
            tr.pt_xf.push(x);
            tr.pt_xs.push(x);
            // Packed [p00, p01, p02, p11, p12, p22].
            let p = [KNOT_VAR_M2, 0.0, 0.0, 1.0e-6, 0.0, 1.0e-6];
            tr.pt_pf.push(p);
            tr.pt_ps.push(p);
        }
    }
    tr.ac_first_leg.push(tr.leg_addr.len() as u32);
    tr.leg_first_pt.push(tr.pt_t_ns.len() as u32);
    debug_assert!(tr.pt_alt_ft.iter().all(|&a| a != NONE_I32));
    Ok(tr)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use oa_truth::{truth_at, TruthQuery};

    const SITE: (f64, f64) = (41.976_940_27, -87.908_149_72);

    fn demo() -> Vec<Traj> {
        vec![
            Traj {
                addr: 0xA00002,
                t0_s: 0.0,
                x0: 20_000.0,
                y0: 10_000.0,
                heading_deg: 270.0,
                speed_mps: 130.0,
                alt_ft: 12_000.0,
                segs: vec![
                    Seg {
                        dur_s: 100.0,
                        turn_dps: 0.0,
                        vrate_fpm: 0.0,
                    },
                    Seg {
                        dur_s: 60.0,
                        turn_dps: 6.0,
                        vrate_fpm: 1_500.0,
                    },
                    Seg {
                        dur_s: 100.0,
                        turn_dps: 0.0,
                        vrate_fpm: 0.0,
                    },
                ],
            },
            Traj {
                addr: 0xA00001,
                t0_s: 5.0,
                x0: -30_000.0,
                y0: 40_000.0,
                heading_deg: 90.0,
                speed_mps: 200.0,
                alt_ft: 30_000.0,
                segs: vec![Seg {
                    dur_s: 200.0,
                    turn_dps: -3.0,
                    vrate_fpm: -1_000.0,
                }],
            },
        ]
    }

    // REQ: OA-TRK-055
    #[test]
    fn interpolation_stays_within_a_centimetre_of_the_analytic_path() {
        let epoch = 1_700_000_000_000_000_000i64;
        let trajs = demo();
        let truth = build_synthetic_truth(SITE.0, SITE.1, epoch, &trajs).unwrap();
        let frame = oa_geo::aeqd::frame_new(SITE.0, SITE.1);
        let (mut smooth, mut step): (f64, f64) = (0.0, 0.0);
        let mut checked = 0;
        for t in &trajs {
            // Instants where a segment starts: the acceleration steps there, and the knot
            // interval around it is interpolated through the step.
            let mut bounds = vec![];
            let mut acc = 0.0;
            for s in &t.segs {
                bounds.push(acc);
                acc += s.dur_s;
            }
            let dur = duration_s(t);
            let mut ms = ((t.t0_s + 0.01) * 1000.0) as i64;
            let end = ((t.t0_s + dur - 0.01) * 1000.0) as i64;
            while ms < end {
                let dt = ms as f64 / 1000.0 - t.t0_s;
                let want = state_at(t, dt);
                let q = truth_at(&truth, t.addr, epoch / 1_000_000 + ms);
                let TruthQuery::Ok(s) = q else {
                    panic!("no truth at {ms}: {q:?}");
                };
                let (px, py) = oa_geo::aeqd::project(&frame, s.lat_deg, s.lon_deg);
                let err = (px - want.e).hypot(py - want.n);
                if bounds.iter().any(|b| (dt - b).abs() < 1.0) {
                    step = step.max(err);
                } else {
                    smooth = smooth.max(err);
                }
                checked += 1;
                ms += 370;
            }
        }
        assert!(checked > 1000);
        assert!(
            smooth < 0.01,
            "away from a step the path is off by {smooth} m"
        );
        assert!(
            step < 0.5,
            "across an acceleration step the path is off by {step} m"
        );
    }

    // REQ: OA-TRK-055
    #[test]
    fn legs_and_addresses_are_ordered_and_bounded() {
        let truth = build_synthetic_truth(SITE.0, SITE.1, 0, &demo()).unwrap();
        assert_eq!(truth.ac_addr, vec![0xA00001, 0xA00002]);
        assert_eq!(truth.ac_first_leg, vec![0, 1, 2]);
        assert_eq!(truth.leg_first_pt.len(), 3);
        assert_eq!(
            oa_truth::leg_start_ns(&truth, oa_truth::LegIx(0)),
            Some(5_000_000_000)
        );
        // Outside the path there is no truth.
        assert_eq!(truth_at(&truth, 0xA00001, 3_000), TruthQuery::OutsideLegs);
        assert_eq!(
            truth_at(&truth, 0xA00009, 10_000),
            TruthQuery::UnknownAircraft
        );
    }

    // REQ: OA-TRK-056
    #[test]
    fn invalid_synthetic_input_is_refused() {
        let mut dup = demo();
        dup[1].addr = dup[0].addr;
        assert!(build_synthetic_truth(SITE.0, SITE.1, 0, &dup).is_err());
        assert!(build_synthetic_truth(SITE.0, SITE.1, 0, &[]).is_err());
        let mut nan = demo();
        nan[0].speed_mps = f64::NAN;
        assert!(build_synthetic_truth(SITE.0, SITE.1, 0, &nan).is_err());
        let mut noseg = demo();
        noseg[0].segs.clear();
        assert!(build_synthetic_truth(SITE.0, SITE.1, 0, &noseg).is_err());
        let mut zero = demo();
        zero[0].segs[0].dur_s = 0.0;
        assert!(build_synthetic_truth(SITE.0, SITE.1, 0, &zero).is_err());
    }

    #[test]
    fn a_standard_rate_turn_returns_to_its_start_after_a_full_circle() {
        let t = Traj {
            addr: 1,
            t0_s: 0.0,
            x0: 0.0,
            y0: 0.0,
            heading_deg: 0.0,
            speed_mps: 100.0,
            alt_ft: 0.0,
            segs: vec![Seg {
                dur_s: 120.0,
                turn_dps: 3.0,
                vrate_fpm: 0.0,
            }],
        };
        let s = state_at(&t, 120.0);
        assert!(s.e.hypot(s.n) < 1e-6, "{s:?}");
        // Centripetal acceleration v * omega.
        let s2 = state_at(&t, 30.0);
        assert!((s2.ae.hypot(s2.an) - 100.0 * 3.0_f64.to_radians()).abs() < 1e-9);
    }
}

//! One track's filter: horizontal and vertical interacting-multiple-model filters, the local
//! frame, and the time step. Core (pure).
//!
//! The horizontal state lives in a local east-north plane (the azimuthal equidistant frame of
//! `oa_geo::aeqd`) centred on the track's own last position and re-centred on every update, so
//! the geometry is accurate anywhere in the NAS without one projection's distortion. After an
//! update the frame origin moves to the filtered position (rounded to 1e-7 degree, the unit the
//! output uses), the positions are shifted to it and the velocity block is rotated by the change
//! of the frame's axes (the meridian convergence, about 0.03 degree per 4 km of east travel at
//! mid latitudes, and large near a pole). The whole state covariance, position and velocity
//! blocks and their cross terms, is carried through the differential of the change of frame
//! at the point. A measurement's covariance is in the target's own east-north axes and is
//! mapped into the frame's plane as `J R J^T` (J the differential of the projection at the
//! plot); the output covariances are mapped back to east-north at the output point. The filter state is stored as `f64` metres in that
//! local frame, an exception to A9 recorded here: fixed point would force a conversion at every
//! step of a calculation that is all products and sums; the positions that leave the module
//! (the output) are 1e-7 degree integers.
//!
//! The vertical filter keeps barometric altitude only; it is advanced to every update time
//! whether or not the update carries an altitude.

use oa_geo::aeqd::{
    frame_at_fix, frame_from_trig, frame_new, frame_trig, mat2_apply, mat2_congruence, project,
    try_frame_change, try_frame_from_true, try_frame_to_true, unproject, unproject_fix, Frame,
    Mat2,
};

use crate::hfilter::{
    dwell_decay, h_change_frame, h_combine, h_gate_d2, h_init, h_pos_nis, h_predict, h_update,
    HFilter,
};
use crate::params::{
    ALT_BASE_SIGMA_FT, ALT_GATE_SIGMAS, GATE_CHI2, NS_PER_S, RESYNC_AFTER, VRATE_SIGMA_FPS,
};
use crate::types::{
    to_e7, Measurement, OutRow, TrackerStats, M_ALT_GEOMETRIC, M_HAS_VEL, NONE_I32, O_HAS_ALT,
};
use crate::vfilter::{v_alt_innovation, v_combine, v_init, v_predict, v_update, VFilter};

/// One track's filter state. Hot (A7): stored per track and per stored update.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Filter {
    pub h: HFilter,
    pub v: VFilter,
    /// Time of the state, Unix ns.
    pub t_ns: i64,
    /// Origin of the local frame, 1e-7 degree.
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Sine and cosine of the origin's latitude, `[sin lat, cos lat]`, carried so the frame is
    /// rebuilt without a library call.
    pub trig: [f64; 2],
    /// Consecutive horizontal rejections.
    pub rej_h: u8,
    /// Consecutive altitude rejections.
    pub rej_v: u8,
    /// The vertical filter holds an altitude.
    pub has_alt: u8,
    /// Horizontal filter mode: 0 interacting pair, 1 to 3 single straight-model baselines.
    pub h_mode: u8,
}
// Budget: 496 (horizontal) + 96 (vertical) + 8 (time) + 8 (origin) + 16 (origin trig) + 4
// counters/flags = 628 packed; 632 with alignment. Stored twice per track: the current state and
// the ring anchor.
const _: () = assert!(core::mem::size_of::<Filter>() == 632);

/// What [`filter_apply`] did with a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Applied.
    Yes,
    /// The position innovation was too large: not applied.
    Rejected,
    /// Too many rejections in a row: the filter was re-initialised at this measurement.
    Resync,
}

fn deg(e7: i32) -> f64 {
    f64::from(e7) / 1.0e7
}

/// The frame about the filter's origin, from its carried sines and cosines.
pub(crate) fn filter_frame(f: &Filter) -> Frame {
    frame_from_trig(deg(f.lat_e7), deg(f.lon_e7), f.trig)
}

/// The measurement's position covariance in the plane at the plot's plane position: its
/// east-north covariance carried through the plane-from-true differential `from_true` there.
fn pos_cov(m: &Measurement, from_true: &Mat2) -> [f64; 3] {
    let [a, b, c] = m.pos_cov_m2;
    mat2_congruence(from_true, &[f64::from(a), f64::from(b), f64::from(c)])
}

/// The measurement's velocity and its covariance in the plane, through the plane-from-true
/// differential `from_true` at the plot's plane position.
fn vel_of(m: &Measurement, from_true: &Mat2) -> Option<([f64; 2], [f64; 3])> {
    if m.flags & M_HAS_VEL == 0 {
        return None;
    }
    let (vx, vy) = mat2_apply(
        from_true,
        (f64::from(m.vel_east_mps), f64::from(m.vel_north_mps)),
    );
    let [a, b, c] = m.vel_cov_m2s2;
    let cov = mat2_congruence(from_true, &[f64::from(a), f64::from(b), f64::from(c)]);
    Some(([vx, vy], cov))
}

/// Altitude variance: quantisation `step^2 / 12` plus the base term.
fn alt_var(m: &Measurement) -> f64 {
    let q = f64::from(m.alt_quant_ft);
    q * q / 12.0 + ALT_BASE_SIGMA_FT * ALT_BASE_SIGMA_FT
}

fn has_baro_alt(m: &Measurement) -> bool {
    m.alt_ft != NONE_I32 && m.flags & M_ALT_GEOMETRIC == 0
}

fn vertical_init(m: &Measurement) -> VFilter {
    let rate = (m.vrate_fpm != NONE_I32).then(|| {
        (
            f64::from(m.vrate_fpm) / 60.0,
            VRATE_SIGMA_FPS * VRATE_SIGMA_FPS,
        )
    });
    v_init(f64::from(m.alt_ft), alt_var(m), rate)
}

/// A filter started at `m`. The measurement is assumed valid (see `measurement_check`).
pub(crate) fn filter_init(m: &Measurement, mode: u8) -> Filter {
    let frame = frame_new(deg(m.lat_e7), deg(m.lon_e7));
    // The frame's origin is inside its chart, so the differential exists there.
    let from_true = try_frame_from_true(&frame, 0.0, 0.0).unwrap_or([[1.0, 0.0], [0.0, 1.0]]);
    let has_alt = has_baro_alt(m);
    let zero = VFilter {
        m: [crate::vfilter::VModel {
            x: [0.0; 2],
            p: [0.0; 3],
        }; 2],
        mu: [1.0, 0.0],
    };
    Filter {
        h: h_init(&pos_cov(m, &from_true), vel_of(m, &from_true), mode),
        v: if has_alt { vertical_init(m) } else { zero },
        t_ns: m.toa_ns,
        lat_e7: m.lat_e7,
        lon_e7: m.lon_e7,
        trig: frame_trig(&frame),
        rej_h: 0,
        rej_v: 0,
        has_alt: u8::from(has_alt),
        h_mode: mode,
    }
}

/// Applies `m` (at `m.toa_ns >= f.t_ns`) to `f`. Counters go to `stats`.
pub(crate) fn filter_apply(f: &mut Filter, m: &Measurement, stats: &mut TrackerStats) -> Applied {
    apply(f, m, stats, true)
}

/// Applies again a measurement that [`filter_apply`] applied to the same state (the ring's
/// replay): the same state results, bit for bit, without computing the position gate and the
/// innovation statistic, which only decide whether to apply and feed counters.
pub(crate) fn filter_replay(f: &mut Filter, m: &Measurement, stats: &mut TrackerStats) -> Applied {
    apply(f, m, stats, false)
}

/// Counts a measurement too far from the track to apply; after `RESYNC_AFTER` in a row the filter
/// restarts at it.
fn reject_jump(f: &mut Filter, m: &Measurement, stats: &mut TrackerStats) -> Applied {
    stats.jump_rejected += 1;
    f.rej_h = f.rej_h.saturating_add(1);
    if f.rej_h >= RESYNC_AFTER {
        *f = filter_init(m, f.h_mode);
        stats.resyncs += 1;
        return Applied::Resync;
    }
    Applied::Rejected
}

fn apply(f: &mut Filter, m: &Measurement, stats: &mut TrackerStats, gated: bool) -> Applied {
    let dt = (m.toa_ns - f.t_ns).max(0) as f64 / NS_PER_S as f64;
    let frame = filter_frame(f);
    let u = dwell_decay(dt);
    let pred = h_predict(&f.h, dt, u, f.h_mode);
    let (zx, zy) = project(&frame, deg(m.lat_e7), deg(m.lon_e7));
    // One differential serves the position covariance, the velocity and its covariance. A plot
    // at the frame's antipode, past the chart, is a jump: counted as one.
    let Some(from_true) = try_frame_from_true(&frame, zx, zy) else {
        return reject_jump(f, m, stats);
    };
    let r = pos_cov(m, &from_true);
    if gated {
        let nis = h_pos_nis(&pred, &[zx, zy], &r);
        let gate = if f.h_mode == 0 {
            h_gate_d2(&pred, &[zx, zy], &r)
        } else {
            nis
        };
        if gate.is_nan() || gate > GATE_CHI2 {
            return reject_jump(f, m, stats);
        }
        stats.nis_sum += nis;
        stats.nis_n += 1;
    }
    let mut h = h_update(&pred, &[zx, zy], &r, vel_of(m, &from_true), f.h_mode);
    // Re-centre on the filtered position.
    let (x, _) = h_combine(&h);
    let (px, py) = (
        x.first().copied().unwrap_or(0.0),
        x.get(1).copied().unwrap_or(0.0),
    );
    if !(px.is_finite() && py.is_finite() && x.iter().all(|v| v.is_finite())) {
        *f = filter_init(m, f.h_mode);
        stats.resyncs += 1;
        return Applied::Resync;
    }
    let Some(fix) = unproject_fix(&frame, px, py) else {
        *f = filter_init(m, f.h_mode);
        stats.resyncs += 1;
        return Applied::Resync;
    };
    let (la, lo) = (fix.lat_deg, fix.lon_deg);
    let (Some(lat_e7), Some(lon_e7)) = (to_e7(la), to_e7(lo)) else {
        *f = filter_init(m, f.h_mode);
        stats.resyncs += 1;
        return Applied::Resync;
    };
    let frame2 = frame_at_fix(&fix, deg(lat_e7), deg(lon_e7));
    let (sx, sy) = project(&frame2, la, lo);
    let Some(d) = try_frame_change(&frame, px, py, &frame2, sx, sy) else {
        *f = filter_init(m, f.h_mode);
        stats.resyncs += 1;
        return Applied::Resync;
    };
    h_change_frame(&mut h, [px, py], [sx, sy], &d);
    // Vertical: always advanced to the update time; updated when the report has an altitude.
    let mut v = f.v;
    let mut has_alt = f.has_alt;
    let mut rej_v = f.rej_v;
    if m.alt_ft != NONE_I32 && m.flags & M_ALT_GEOMETRIC != 0 {
        stats.alt_geometric_skipped += 1;
    }
    if has_alt != 0 {
        v = v_predict(&v, dt, u);
    }
    if has_baro_alt(m) {
        let var = alt_var(m);
        let rate = (m.vrate_fpm != NONE_I32).then(|| {
            (
                f64::from(m.vrate_fpm) / 60.0,
                VRATE_SIGMA_FPS * VRATE_SIGMA_FPS,
            )
        });
        if has_alt == 0 {
            v = vertical_init(m);
            has_alt = 1;
            rej_v = 0;
        } else {
            let (y, s) = v_alt_innovation(&v, f64::from(m.alt_ft), var);
            if y.abs() > ALT_GATE_SIGMAS * s.sqrt() {
                stats.alt_rejected += 1;
                rej_v = rej_v.saturating_add(1);
                if rej_v >= RESYNC_AFTER {
                    v = vertical_init(m);
                    rej_v = 0;
                }
            } else {
                v = v_update(&v, Some((f64::from(m.alt_ft), var)), rate);
                rej_v = 0;
            }
        }
    }
    f.h = h;
    f.v = v;
    f.t_ns = m.toa_ns;
    f.lat_e7 = lat_e7;
    f.lon_e7 = lon_e7;
    f.trig = frame_trig(&frame2);
    f.rej_h = 0;
    f.rej_v = rej_v;
    f.has_alt = has_alt;
    Applied::Yes
}

/// The track's state at `t_ns >= f.t_ns` as an output row (without address, kind and coast
/// flags): the filter predicted to that time, nothing stored. `None` if the predicted position
/// cannot be represented (past the projection's chart, or not a number): such a row has no
/// coordinate and is never given a zero one.
pub(crate) fn filter_output(f: &Filter, t_ns: i64) -> Option<OutRow> {
    let dt = (t_ns - f.t_ns).max(0) as f64 / NS_PER_S as f64;
    let frame = filter_frame(f);
    let (h, vf) = if dt > 0.0 {
        let u = dwell_decay(dt);
        (h_predict(&f.h, dt, u, f.h_mode), v_predict(&f.v, dt, u))
    } else {
        (f.h, f.v)
    };
    let (x, p) = h_combine(&h);
    let xg = |i: usize| x.get(i).copied().unwrap_or(0.0);
    let pg = |i: usize, j: usize| p.get(i).and_then(|r| r.get(j)).copied().unwrap_or(0.0);
    // A state past the chart has no position.
    let (la, lo) = unproject(&frame, xg(0), xg(1))?;
    let (lat_e7, lon_e7) = (to_e7(la)?, to_e7(lo)?);
    // The state is in the plane of the frame; the output is in east and north at the point.
    let jt = try_frame_to_true(&frame, xg(0), xg(1))?;
    let (ve, vn) = mat2_apply(&jt, (xg(2), xg(3)));
    let pc = mat2_congruence(&jt, &[pg(0, 0), pg(0, 1), pg(1, 1)]);
    let vc = mat2_congruence(&jt, &[pg(2, 2), pg(2, 3), pg(3, 3)]);
    let (xv, pv) = v_combine(&vf);
    let has_alt = f.has_alt != 0;
    Some(OutRow {
        t_ns,
        addr: 0,
        lat_e7,
        lon_e7,
        vel_east_mps: ve as f32,
        vel_north_mps: vn as f32,
        pos_cov_m2: [pc[0] as f32, pc[1] as f32, pc[2] as f32],
        vel_cov_m2s2: [vc[0] as f32, vc[1] as f32, vc[2] as f32],
        // The model's turn rate is counter-clockwise; the output is the heading rate (clockwise
        // positive, as a pilot reads it).
        turn_rate_rps: -xg(4) as f32,
        // An absent altitude is NaN, never 0 ft: a consumer that forgets the flag gets a value
        // that cannot be mistaken for sea level (and that propagates through any arithmetic).
        alt_ft: if has_alt { xv[0] as f32 } else { f32::NAN },
        alt_var_ft2: if has_alt { pv[0] as f32 } else { f32::NAN },
        vrate_fpm: if has_alt {
            (xv[1] * 60.0) as f32
        } else {
            f32::NAN
        },
        mu_h: [h.mu[0] as f32, h.mu[1] as f32],
        mu_v: [vf.mu[0] as f32, vf.mu[1] as f32],
        flags: if has_alt { O_HAS_ALT } else { 0 },
    })
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
    use crate::hfilter::h_pos_moments;
    use crate::types::NO_ADDR;
    use oa_geo::aeqd::Frame;

    const A: f64 = 6_378_137.0;
    const E2: f64 = 0.006_694_379_990_141_316;

    /// WGS-84 prime-vertical and meridian radii at a latitude, degrees.
    fn radii_at(lat_deg: f64) -> (f64, f64) {
        let s = lat_deg.to_radians().sin();
        let w2 = 1.0 - E2 * s * s;
        (A / w2.sqrt(), A * (1.0 - E2) / (w2 * w2.sqrt()))
    }

    /// The map from plane displacement to true east-north metres at `(x, y)` by central
    /// differences of `unproject` over 1 cm, independent of the closed form.
    fn num_to_true(f: &Frame, x: f64, y: f64) -> [[f64; 2]; 2] {
        let h = 0.01;
        let mut j = [[0.0; 2]; 2];
        for (c, d) in [(1.0, 0.0), (0.0, 1.0)].iter().enumerate() {
            let (la1, lo1) = unproject(f, x + h * d.0, y + h * d.1).unwrap();
            let (la0, lo0) = unproject(f, x - h * d.0, y - h * d.1).unwrap();
            let mut dlon = lo1 - lo0;
            if dlon > 180.0 {
                dlon -= 360.0;
            } else if dlon < -180.0 {
                dlon += 360.0;
            }
            let mid = (la0 + la1) / 2.0;
            let (n, m) = radii_at(mid);
            j[0][c] = dlon.to_radians() * n * mid.to_radians().cos() / (2.0 * h);
            j[1][c] = (la1 - la0).to_radians() * m / (2.0 * h);
        }
        j
    }

    fn inv(j: &[[f64; 2]; 2]) -> [[f64; 2]; 2] {
        let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
        [
            [j[1][1] / det, -j[0][1] / det],
            [-j[1][0] / det, j[0][0] / det],
        ]
    }

    /// `j s j^T` for `s = [a, b, c]`.
    fn cong(j: &[[f64; 2]; 2], s: &[f64; 3]) -> [f64; 3] {
        let m = [[s[0], s[1]], [s[1], s[2]]];
        let mut jm = [[0.0; 2]; 2];
        for i in 0..2 {
            for k in 0..2 {
                jm[i][k] = j[i][0] * m[0][k] + j[i][1] * m[1][k];
            }
        }
        let e = |i: usize, k: usize| jm[i][0] * j[k][0] + jm[i][1] * j[k][1];
        [e(0, 0), e(0, 1), e(1, 1)]
    }

    fn meas(lat: f64, lon: f64, t_s: i64, pos_cov: [f32; 3]) -> Measurement {
        Measurement {
            toa_ns: t_s * NS_PER_S,
            pos_cov_m2: pos_cov,
            vel_cov_m2s2: [0.0; 3],
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            lat_e7: to_e7(lat).unwrap(),
            lon_e7: to_e7(lon).unwrap(),
            alt_ft: NONE_I32,
            vrate_fpm: NONE_I32,
            addr: NO_ADDR,
            alt_quant_ft: 0,
            sensor: 0,
            ident: crate::types::NO_IDENT,
            flags: 0,
            category: 0,
        }
    }

    /// A track at `(lat, 0)` with the true velocity that takes it to `(lat, lon_t)` in `dt` s
    /// along its frame's straight line, with the given position and velocity covariances.
    fn moving_track(lat: f64, lon_t: f64, dt: f64, pc: [f32; 3], vc: [f32; 3]) -> Filter {
        let frame = frame_new(lat, 0.0);
        let (zx, zy) = project(&frame, lat, lon_t);
        let (n, m) = radii_at(lat);
        let mut m0 = meas(lat, 0.0, 0, pc);
        m0.flags = M_HAS_VEL;
        m0.vel_east_mps = (zx / dt * n / r_of(lat)) as f32;
        m0.vel_north_mps = (zy / dt * m / r_of(lat)) as f32;
        m0.vel_cov_m2s2 = vc;
        filter_init(&m0, 0)
    }

    /// The frame's sphere radius at a latitude (the Gaussian mean radius of curvature).
    fn r_of(lat_deg: f64) -> f64 {
        let (n, m) = radii_at(lat_deg);
        (n * m).sqrt()
    }

    // REQ: OA-TRK-260
    #[test]
    fn the_gate_uses_the_plot_covariance_in_the_frame_of_the_projection() {
        for (lat, lon_t) in [(89.99, 90.0), (89.99, 45.0), (-89.99, 90.0), (45.0, 0.05)] {
            let dt = 2.0;
            let f0 = moving_track(lat, lon_t, dt, [1.0, 0.0, 1.0], [1e-4, 0.0, 1e-4]);
            let frame = frame_new(lat, 0.0);
            let pred = h_predict(&f0.h, dt, dwell_decay(dt), 0);
            let (pp, _) = h_pos_moments(&pred);
            // 30 m along the target's own east, at the predicted position.
            let u = inv(&num_to_true(&frame, pp[0], pp[1]));
            let (la, lo) =
                unproject(&frame, pp[0] + 30.0 * u[0][0], pp[1] + 30.0 * u[1][0]).unwrap();
            let m = meas(la, lo, 2, [10000.0, 0.0, 1.0]);
            let (zx, zy) = project(&frame, deg(m.lat_e7), deg(m.lon_e7));
            let jz = inv(&num_to_true(&frame, zx, zy));
            let r = cong(&jz, &[10000.0, 0.0, 1.0]);
            let want = h_pos_nis(&pred, &[zx, zy], &r);
            let mut f = f0;
            let mut stats = TrackerStats::default();
            let got = filter_apply(&mut f, &m, &mut stats);
            assert_eq!(got, Applied::Yes, "{lat} {lon_t}: nis {want}");
            assert!(
                (stats.nis_sum - want).abs() < 2e-3 * want.max(1.0),
                "{lat} {lon_t}: {} vs {want}",
                stats.nis_sum
            );
        }
    }

    // REQ: OA-TRK-261
    #[test]
    fn a_known_ellipse_is_the_same_ellipse_after_recentring() {
        for (lat, lon_t) in [
            (89.99, 90.0),
            (89.99, 45.0),
            (-89.99, 90.0),
            (89.9, 90.0),
            (45.0, 0.05),
        ] {
            let dt = 2.0;
            let f0 = moving_track(lat, lon_t, dt, [10000.0, 0.0, 1.0], [4.0, 0.0, 0.25]);
            let frame = frame_new(lat, 0.0);
            let pred = h_predict(&f0.h, dt, dwell_decay(dt), 0);
            let (x, p) = h_combine(&pred);
            // The same physical ellipse in the target's own east-north axes.
            let jt = num_to_true(&frame, x[0], x[1]);
            let want_pos = cong(&jt, &[p[0][0], p[0][1], p[1][1]]);
            let want_vel = cong(&jt, &[p[2][2], p[2][3], p[3][3]]);
            let want_v = (
                jt[0][0] * x[2] + jt[0][1] * x[3],
                jt[1][0] * x[2] + jt[1][1] * x[3],
            );
            // A plot too coarse to move the estimate recentres the frame on it.
            let (la, lo) = unproject(&frame, x[0], x[1]).unwrap();
            let m = meas(la, lo, 2, [1.0e10, 0.0, 1.0e10]);
            let mut f = f0;
            let mut stats = TrackerStats::default();
            assert_eq!(filter_apply(&mut f, &m, &mut stats), Applied::Yes);
            assert!(f.lon_e7 != f0.lon_e7 || f.lat_e7 != f0.lat_e7);
            let o = filter_output(&f, 2 * NS_PER_S).unwrap();
            let rel = |a: &[f64; 3], b: [f32; 3]| {
                let scale = a[0].abs().max(a[2].abs());
                (0..3)
                    .map(|i| (a[i] - f64::from(b[i])).abs() / scale)
                    .fold(0.0, f64::max)
            };
            assert!(
                rel(&want_pos, o.pos_cov_m2) < 3e-3,
                "{lat} {lon_t}: pos {want_pos:?} vs {:?}",
                o.pos_cov_m2
            );
            assert!(
                rel(&want_vel, o.vel_cov_m2s2) < 3e-3,
                "{lat} {lon_t}: vel {want_vel:?} vs {:?}",
                o.vel_cov_m2s2
            );
            let sp = want_v.0.hypot(want_v.1);
            assert!(
                (want_v.0 - f64::from(o.vel_east_mps)).abs() < 3e-3 * sp
                    && (want_v.1 - f64::from(o.vel_north_mps)).abs() < 3e-3 * sp,
                "{lat} {lon_t}: v {want_v:?} vs {} {}",
                o.vel_east_mps,
                o.vel_north_mps
            );
        }
    }

    // REQ: OA-TRK-263
    #[test]
    fn a_new_track_returns_its_covariance_and_velocity_in_east_and_north() {
        for lat in [45.0, 41.9, 89.9999, -89.99, 89.99] {
            let mut m = meas(lat, 20.0, 0, [10000.0, 30.0, 1.0]);
            m.flags = M_HAS_VEL;
            m.vel_east_mps = 100.0;
            m.vel_north_mps = -40.0;
            m.vel_cov_m2s2 = [4.0, 0.5, 0.25];
            let f = filter_init(&m, 0);
            let o = filter_output(&f, 0).unwrap();
            for (a, b) in o.pos_cov_m2.iter().zip([10000.0, 30.0, 1.0]) {
                assert!(
                    (f64::from(*a) - b).abs() < 1e-4 * 10000.0,
                    "{lat}: {:?}",
                    o.pos_cov_m2
                );
            }
            for (a, b) in o.vel_cov_m2s2.iter().zip([4.0, 0.5, 0.25]) {
                assert!(
                    (f64::from(*a) - b).abs() < 1e-4 * 4.0,
                    "{lat}: {:?}",
                    o.vel_cov_m2s2
                );
            }
            assert!(
                (o.vel_east_mps - 100.0).abs() < 1e-3 && (o.vel_north_mps + 40.0).abs() < 1e-3,
                "{lat}: {} {}",
                o.vel_east_mps,
                o.vel_north_mps
            );
        }
    }

    // REQ: OA-TRK-295
    #[test]
    fn the_carried_sines_follow_the_origin_through_many_updates() {
        let (lat, lon) = (41.9, -88.3);
        let mut f = filter_init(&meas(lat, lon, 0, [100.0, 0.0, 100.0]), 0);
        let mut stats = TrackerStats::default();
        let mut worst = 0.0f64;
        for k in 1..3000i64 {
            // A track turning through a circle of 4 km, one fix a second.
            let a = 0.01 * k as f64;
            let m = meas(
                lat + 0.036 * a.sin(),
                lon + 0.05 * (1.0 - a.cos()),
                k,
                [100.0, 0.0, 100.0],
            );
            filter_apply(&mut f, &m, &mut stats);
            let (sa, ca) = oa_math::sin_cos(deg(f.lat_e7).to_radians());
            for (g, w) in f.trig.iter().zip([sa, ca]) {
                worst = worst.max((g - w).abs());
            }
        }
        assert!(stats.nis_n > 2500, "{stats:?}");
        assert!(worst < 1.0e-13, "{worst}");
        // The frame rebuilt from them is the frame of the origin.
        let (a, b) = (filter_frame(&f), frame_new(deg(f.lat_e7), deg(f.lon_e7)));
        let (pa, pb) = (project(&a, lat, lon), project(&b, lat, lon));
        assert!((pa.0 - pb.0).abs() < 1e-6 && (pa.1 - pb.1).abs() < 1e-6);
    }

    // REQ: OA-TRK-313
    #[test]
    fn a_plot_at_the_antipode_of_the_frame_is_a_counted_jump_and_never_a_nan_state() {
        let m0 = meas(0.0, 0.0, 0, [100.0, 0.0, 100.0]);
        let f0 = filter_init(&m0, 0);
        // The frame's antipode is past the chart's margin: no differential there.
        let far = meas(0.0, 180.0, 1, [100.0, 0.0, 100.0]);
        let mut f = f0;
        let mut stats = TrackerStats::default();
        assert_eq!(filter_apply(&mut f, &far, &mut stats), Applied::Rejected);
        assert_eq!((stats.jump_rejected, stats.resyncs), (1, 0));
        assert_eq!(
            (f.h, f.t_ns, f.rej_h),
            (f0.h, f0.t_ns, 1),
            "only the rejection count moved"
        );
        let o = filter_output(&f, NS_PER_S).unwrap();
        assert!(o.vel_east_mps.is_finite() && o.pos_cov_m2.iter().all(|v| v.is_finite()));
    }

    // REQ: OA-TRK-312
    #[test]
    fn a_coordinate_that_is_not_a_number_or_does_not_fit_is_refused() {
        assert_eq!(to_e7(41.9), Some(419_000_000));
        assert_eq!(to_e7(-180.0), Some(-1_800_000_000));
        for d in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            215.0,
            -215.0,
            1e300,
        ] {
            assert_eq!(to_e7(d), None, "{d}");
        }
    }
}

//! One track's filter: horizontal and vertical interacting-multiple-model filters, the local
//! frame, and the time step. Core (pure).
//!
//! The horizontal state lives in a local east-north plane (the azimuthal equidistant frame of
//! `oa_geo::aeqd`) centred on the track's own last position and re-centred on every update, so
//! the geometry is accurate anywhere in the NAS without one projection's distortion. After an
//! update the frame origin moves to the filtered position (rounded to 1e-7 degree, the unit the
//! output uses), the positions are shifted to it and the velocity block is rotated by the change
//! of the frame's axes (the meridian convergence, about 0.03 degree per 4 km of east travel at
//! mid latitudes) through the true-velocity mapping of the frame. The position covariance is not
//! rotated: the rotation is under 1e-3 rad. The filter state is stored as `f64` metres in that
//! local frame, an exception to A9 recorded here: fixed point would force a conversion at every
//! step of a calculation that is all products and sums; the positions that leave the module
//! (the output) are 1e-7 degree integers.
//!
//! The vertical filter keeps barometric altitude only; it is advanced to every update time
//! whether or not the update carries an altitude.

use oa_geo::aeqd::{frame_from_true_velocity, frame_new, frame_velocity, project, unproject};

use crate::hfilter::{
    h_combine, h_gate_d2, h_init, h_pos_nis, h_predict, h_rotate_velocity, h_shift, h_update,
    HFilter,
};
use crate::params::{
    ALT_BASE_SIGMA_FT, ALT_GATE_SIGMAS, GATE_CHI2, NS_PER_S, RESYNC_AFTER, VRATE_SIGMA_FPS,
};
use crate::types::{
    Measurement, OutRow, TrackerStats, M_ALT_GEOMETRIC, M_HAS_VEL, NONE_I32, O_HAS_ALT,
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
    /// Consecutive horizontal rejections.
    pub rej_h: u8,
    /// Consecutive altitude rejections.
    pub rej_v: u8,
    /// The vertical filter holds an altitude.
    pub has_alt: u8,
    /// Horizontal filter mode: 0 interacting pair, 1 to 3 single straight-model baselines.
    pub h_mode: u8,
}
// Budget: 496 (horizontal) + 96 (vertical) + 8 (time) + 8 (origin) + 4 counters/flags = 612
// packed; 616 with alignment. Stored once per track and per stored update (RING_DEPTH).
const _: () = assert!(core::mem::size_of::<Filter>() == 616);

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

fn to_e7(d: f64) -> i32 {
    // Saturating float to int conversion; latitudes and longitudes are far inside i32.
    (d * 1.0e7).round() as i32
}

fn pos_cov(m: &Measurement) -> [f64; 3] {
    let [a, b, c] = m.pos_cov_m2;
    [f64::from(a), f64::from(b), f64::from(c)]
}

fn vel_of(m: &Measurement, frame: &oa_geo::aeqd::Frame) -> Option<([f64; 2], [f64; 3])> {
    if m.flags & M_HAS_VEL == 0 {
        return None;
    }
    let (vx, vy) =
        frame_from_true_velocity(frame, f64::from(m.vel_east_mps), f64::from(m.vel_north_mps));
    let [a, b, c] = m.vel_cov_m2s2;
    Some(([vx, vy], [f64::from(a), f64::from(b), f64::from(c)]))
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
    let has_alt = has_baro_alt(m);
    let zero = VFilter {
        m: [crate::vfilter::VModel {
            x: [0.0; 2],
            p: [0.0; 3],
        }; 2],
        mu: [1.0, 0.0],
    };
    Filter {
        h: h_init(&pos_cov(m), vel_of(m, &frame), mode),
        v: if has_alt { vertical_init(m) } else { zero },
        t_ns: m.toa_ns,
        lat_e7: m.lat_e7,
        lon_e7: m.lon_e7,
        rej_h: 0,
        rej_v: 0,
        has_alt: u8::from(has_alt),
        h_mode: mode,
    }
}

/// Applies `m` (at `m.toa_ns >= f.t_ns`) to `f`. Counters go to `stats`.
pub(crate) fn filter_apply(f: &mut Filter, m: &Measurement, stats: &mut TrackerStats) -> Applied {
    let dt = (m.toa_ns - f.t_ns).max(0) as f64 / NS_PER_S as f64;
    let frame = frame_new(deg(f.lat_e7), deg(f.lon_e7));
    let pred = h_predict(&f.h, dt, f.h_mode);
    let (zx, zy) = project(&frame, deg(m.lat_e7), deg(m.lon_e7));
    let r = pos_cov(m);
    let nis = h_pos_nis(&pred, &[zx, zy], &r);
    let gate = if f.h_mode == 0 {
        h_gate_d2(&pred, &[zx, zy], &r)
    } else {
        nis
    };
    if gate.is_nan() || gate > GATE_CHI2 {
        stats.jump_rejected += 1;
        f.rej_h = f.rej_h.saturating_add(1);
        if f.rej_h >= RESYNC_AFTER {
            *f = filter_init(m, f.h_mode);
            stats.resyncs += 1;
            return Applied::Resync;
        }
        return Applied::Rejected;
    }
    stats.nis_sum += nis;
    stats.nis_n += 1;
    let mut h = h_update(&pred, &[zx, zy], &r, vel_of(m, &frame), f.h_mode);
    // Re-centre on the filtered position.
    let (x, _) = h_combine(&h);
    let (px, py) = (
        x.first().copied().unwrap_or(0.0),
        x.get(1).copied().unwrap_or(0.0),
    );
    if !(px.is_finite() && py.is_finite()) || !x.iter().all(|v| v.is_finite()) {
        *f = filter_init(m, f.h_mode);
        stats.resyncs += 1;
        return Applied::Resync;
    }
    let (la, lo) = unproject(&frame, px, py);
    let (lat_e7, lon_e7) = (to_e7(la), to_e7(lo));
    let frame2 = frame_new(deg(lat_e7), deg(lon_e7));
    let (sx, sy) = project(&frame2, la, lo);
    h_shift(&mut h, sx - px, sy - py);
    let img = |ex: f64, ey: f64| {
        let (te, tn) = frame_velocity(&frame, px, py, ex, ey);
        frame_from_true_velocity(&frame2, te, tn)
    };
    let (c0, c1) = (img(1.0, 0.0), img(0.0, 1.0));
    h_rotate_velocity(&mut h, &[[c0.0, c1.0], [c0.1, c1.1]]);
    // Vertical: always advanced to the update time; updated when the report has an altitude.
    let mut v = f.v;
    let mut has_alt = f.has_alt;
    let mut rej_v = f.rej_v;
    if m.alt_ft != NONE_I32 && m.flags & M_ALT_GEOMETRIC != 0 {
        stats.alt_geometric_skipped += 1;
    }
    if has_alt != 0 {
        v = v_predict(&v, dt);
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
    f.rej_h = 0;
    f.rej_v = rej_v;
    f.has_alt = has_alt;
    Applied::Yes
}

/// The track's state at `t_ns >= f.t_ns` as an output row (without address, kind and coast
/// flags): the filter predicted to that time, nothing stored.
pub(crate) fn filter_output(f: &Filter, t_ns: i64) -> OutRow {
    let dt = (t_ns - f.t_ns).max(0) as f64 / NS_PER_S as f64;
    let frame = frame_new(deg(f.lat_e7), deg(f.lon_e7));
    let (h, vf) = if dt > 0.0 {
        (h_predict(&f.h, dt, f.h_mode), v_predict(&f.v, dt))
    } else {
        (f.h, f.v)
    };
    let (x, p) = h_combine(&h);
    let xg = |i: usize| x.get(i).copied().unwrap_or(0.0);
    let pg = |i: usize, j: usize| p.get(i).and_then(|r| r.get(j)).copied().unwrap_or(0.0);
    let (la, lo) = unproject(&frame, xg(0), xg(1));
    let (ve, vn) = frame_velocity(&frame, xg(0), xg(1), xg(2), xg(3));
    let (xv, pv) = v_combine(&vf);
    let has_alt = f.has_alt != 0;
    OutRow {
        t_ns,
        addr: 0,
        lat_e7: to_e7(la),
        lon_e7: to_e7(lo),
        vel_east_mps: ve as f32,
        vel_north_mps: vn as f32,
        pos_cov_m2: [pg(0, 0) as f32, pg(0, 1) as f32, pg(1, 1) as f32],
        vel_cov_m2s2: [pg(2, 2) as f32, pg(2, 3) as f32, pg(3, 3) as f32],
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
    }
}

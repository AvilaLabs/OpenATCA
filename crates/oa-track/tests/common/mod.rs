//! Synthetic worlds with known truth for the tracker tests. Not a test file itself.
#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_geo::aeqd::{frame_new, project, try_frame_velocity, unproject, Frame};
use oa_track::{
    output_clear, output_len, output_new, tracker_flush, tracker_new, tracker_update, Measurement,
    TrackOutput, Tracker, TrackerConfig, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_IDENT,
};

pub const LAT0: f64 = 41.9;
pub const LON0: f64 = -88.3;

/// Deterministic normal noise (splitmix64 and Box-Muller).
pub struct Rng(pub u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    pub fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// The frame all truth is defined in.
pub fn frame0() -> Frame {
    frame_new(LAT0, LON0)
}

/// True state at one time, in the frame (metres, frame velocity).
#[derive(Clone, Copy, Debug)]
pub struct Truth {
    pub e: f64,
    pub n: f64,
    pub ve: f64,
    pub vn: f64,
    pub alt_ft: f64,
    pub vrate_fpm: f64,
}

/// NACp 9 (30 m 95 % bound) and NACv 2 (3 m/s) sigmas per axis.
pub const SIGMA_POS: f64 = 30.0 / 2.447_746_830_680_816;
pub const SIGMA_VEL: f64 = 3.0 / 2.447_746_830_680_816;

/// The measurement of a transponder whose velocity lags by the second of
/// [`Manoeuvre::at_lagged_velocity`].
pub fn with_lag(mut m: Measurement) -> Measurement {
    m.vel_lag_s = 1.0;
    m
}

/// A measurement of `tr` at `t_s` seconds with noise from `rng`.
#[allow(clippy::too_many_arguments)]
pub fn measure(
    f: &Frame,
    addr: u32,
    t_s: f64,
    tr: &Truth,
    sigma_pos: f64,
    sigma_vel: Option<f64>,
    with_alt: bool,
    rng: &mut Rng,
) -> Measurement {
    let e = tr.e + sigma_pos * rng.normal();
    let n = tr.n + sigma_pos * rng.normal();
    let (lat, lon) = unproject(f, e, n).unwrap();
    let pv = (sigma_pos * sigma_pos) as f32;
    let mut m = Measurement {
        toa_ns: (t_s * 1.0e9).round() as i64,
        pos_cov_m2: [pv, 0.0, pv],
        vel_cov_m2s2: [0.0; 3],
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        lat_e7: (lat * 1.0e7).round() as i32,
        lon_e7: (lon * 1.0e7).round() as i32,
        alt_ft: NONE_I32,
        vrate_fpm: NONE_I32,
        addr,
        alt_quant_ft: 0,
        sensor: 1,
        ident: u16::MAX,
        flags: 0,
        category: 0,
        vel_lag_s: 0.0,
    };
    if let Some(sv) = sigma_vel {
        let (te, tn) = try_frame_velocity(f, tr.e, tr.n, tr.ve, tr.vn).unwrap();
        m.vel_east_mps = (te + sv * rng.normal()) as f32;
        m.vel_north_mps = (tn + sv * rng.normal()) as f32;
        let vv = (sv * sv) as f32;
        m.vel_cov_m2s2 = [vv, 0.0, vv];
        m.flags |= M_HAS_VEL;
    }
    if with_alt {
        m.alt_ft = ((tr.alt_ft / 25.0).round() * 25.0) as i32;
        m.alt_quant_ft = 25;
        m.vrate_fpm = (tr.vrate_fpm + 64.0 * rng.normal()).round() as i32;
    }
    m
}

/// Straight, then a constant-rate turn, then straight: heading in radians clockwise from north.
pub struct Manoeuvre {
    pub speed: f64,
    pub heading0: f64,
    pub turn_start_s: f64,
    pub turn_end_s: f64,
    /// Turn rate, rad/s, positive clockwise.
    pub omega: f64,
    /// Speed change, m/s^2, applied throughout.
    pub accel: f64,
}

impl Manoeuvre {
    pub fn straight(speed: f64, heading_deg: f64) -> Manoeuvre {
        Manoeuvre {
            speed,
            heading0: heading_deg.to_radians(),
            turn_start_s: f64::MAX,
            turn_end_s: f64::MAX,
            omega: 0.0,
            accel: 0.0,
        }
    }
    pub fn turning(
        speed: f64,
        heading_deg: f64,
        start: f64,
        end: f64,
        rate_deg_s: f64,
    ) -> Manoeuvre {
        Manoeuvre {
            speed,
            heading0: heading_deg.to_radians(),
            turn_start_s: start,
            turn_end_s: end,
            omega: rate_deg_s.to_radians(),
            accel: 0.0,
        }
    }
    /// Truth at `t` with the velocity of 1 s earlier: what a transponder reports (the reported
    /// velocity lags the aircraft's). A literal, not `VEL_LAG_S`, so that a change of the constant
    /// does not move the world; the measurement is marked with [`with_lag`].
    pub fn at_lagged_velocity(&self, t: f64, e0: f64, n0: f64) -> Truth {
        let mut tr = self.at(t, e0, n0);
        let v = self.at(t - 1.0, e0, n0);
        tr.ve = v.ve;
        tr.vn = v.vn;
        tr
    }
    /// Truth at `t` by integrating in 10 ms steps (the exact path is not needed to better than
    /// a millimetre).
    pub fn at(&self, t: f64, e0: f64, n0: f64) -> Truth {
        let steps = (t / 0.01).ceil() as usize;
        let h = if steps == 0 { 0.0 } else { t / steps as f64 };
        let (mut e, mut n) = (e0, n0);
        let mut s = 0.0;
        let mut hd = self.heading0;
        let mut spd = self.speed;
        for _ in 0..steps {
            let tm = s + h / 2.0;
            let w = if tm >= self.turn_start_s && tm < self.turn_end_s {
                self.omega
            } else {
                0.0
            };
            let hm = hd + w * h / 2.0;
            let sm = spd + self.accel * h / 2.0;
            e += sm * hm.sin() * h;
            n += sm * hm.cos() * h;
            hd += w * h;
            spd += self.accel * h;
            s += h;
        }
        Truth {
            e,
            n,
            ve: spd * hd.sin(),
            vn: spd * hd.cos(),
            alt_ft: 0.0,
            vrate_fpm: 0.0,
        }
    }
}

pub fn big_config() -> TrackerConfig {
    TrackerConfig {
        max_tracks: 64,
        out_capacity: 1 << 20,
        log_capacity: 1024,
        ..DEFAULT_CONFIG
    }
}

/// Runs `ms` (in the order given) through a tracker and returns it with all rows.
pub fn run(cfg: &TrackerConfig, ms: &[Measurement], end_s: f64) -> (Tracker, TrackOutput) {
    let mut t = tracker_new(cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for m in ms {
        tracker_update(&mut t, m, &mut out);
    }
    tracker_flush(&mut t, (end_s * 1.0e9) as i64, &mut out);
    (t, out)
}

/// Position error (metres) of output row `i` against `truth`, measured in the truth frame.
pub fn pos_err(f: &Frame, o: &TrackOutput, i: usize, tr: &Truth) -> f64 {
    let (e, n) = project(
        f,
        f64::from(o.lat_e7[i]) / 1.0e7,
        f64::from(o.lon_e7[i]) / 1.0e7,
    );
    (e - tr.e).hypot(n - tr.n)
}

/// Speed error (m/s) and heading error (degrees) of row `i`.
pub fn vel_err(f: &Frame, o: &TrackOutput, i: usize, tr: &Truth) -> (f64, f64) {
    let (te, tn) = try_frame_velocity(f, tr.e, tr.n, tr.ve, tr.vn).unwrap();
    let (ve, vn) = (f64::from(o.vel_east_mps[i]), f64::from(o.vel_north_mps[i]));
    let ds = ve.hypot(vn) - te.hypot(tn);
    let mut dh = ve.atan2(vn) - te.atan2(tn);
    while dh > std::f64::consts::PI {
        dh -= 2.0 * std::f64::consts::PI;
    }
    while dh < -std::f64::consts::PI {
        dh += 2.0 * std::f64::consts::PI;
    }
    (ds, dh.to_degrees())
}

pub fn rms(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

pub fn clear(o: &mut TrackOutput) -> usize {
    let n = output_len(o);
    output_clear(o);
    n
}

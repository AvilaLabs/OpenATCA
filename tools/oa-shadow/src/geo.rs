//! Positions and separations. Plain functions of values; no I/O.

use crate::model::{EramSample, OaSample};
use crate::thresholds::Thresholds;

const EARTH_R_M: f64 = 6_371_008.8;

/// 1e-7 degree to degrees.
pub fn deg(e7: i32) -> f64 {
    f64::from(e7) * 1e-7
}

/// East and north metres from point 0 to point 1 (degrees), on the local tangent plane at their
/// mean latitude. Good to a few metres over the tens of kilometres the thresholds concern.
pub fn enu_m(lat0: f64, lon0: f64, lat1: f64, lon1: f64) -> (f64, f64) {
    let mid = 0.5 * (lat0 + lat1);
    let east = (lon1 - lon0).to_radians() * EARTH_R_M * mid.to_radians().cos();
    let north = (lat1 - lat0).to_radians() * EARTH_R_M;
    (east, north)
}

/// OpenATCA's state at one time: a row, or two rows interpolated.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OaPoint {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_ft: Option<f64>,
    pub vel_east_mps: f64,
    pub vel_north_mps: f64,
    pub coast: bool,
}

fn point_of(s: &OaSample) -> OaPoint {
    OaPoint {
        lat_deg: deg(s.lat_e7),
        lon_deg: deg(s.lon_e7),
        alt_ft: s.alt_ft.map(f64::from),
        vel_east_mps: f64::from(s.vel_east_mps),
        vel_north_mps: f64::from(s.vel_north_mps),
        coast: s.coast,
    }
}

/// OpenATCA at `t_ms`: the row at that time, or the two rows around it interpolated linearly
/// when they are at most `interp_gap_ms` apart; otherwise `None` (no sample then). A time with
/// a row on each side but a gap between them is not covered. `samples` is in time order.
pub fn oa_at(samples: &[OaSample], t_ms: i64, th: &Thresholds) -> Option<OaPoint> {
    let i = samples.partition_point(|s| s.t_ms < t_ms);
    let after = samples.get(i)?;
    if after.t_ms == t_ms {
        return Some(point_of(after));
    }
    let before = samples.get(i.checked_sub(1)?)?;
    let span = after.t_ms - before.t_ms;
    if span <= 0 || span > th.interp_gap_ms {
        return None;
    }
    let f = (t_ms - before.t_ms) as f64 / span as f64;
    let (a, b) = (point_of(before), point_of(after));
    let lerp = |x: f64, y: f64| x + (y - x) * f;
    Some(OaPoint {
        lat_deg: lerp(a.lat_deg, b.lat_deg),
        lon_deg: lerp(a.lon_deg, b.lon_deg),
        alt_ft: a.alt_ft.zip(b.alt_ft).map(|(x, y)| lerp(x, y)),
        vel_east_mps: lerp(a.vel_east_mps, b.vel_east_mps),
        vel_north_mps: lerp(a.vel_north_mps, b.vel_north_mps),
        coast: a.coast || b.coast,
    })
}

/// The OpenATCA row exactly at `t_ms`, if there is one.
pub fn oa_exact(samples: &[OaSample], t_ms: i64) -> Option<&OaSample> {
    let i = samples.partition_point(|s| s.t_ms < t_ms);
    samples.get(i).filter(|s| s.t_ms == t_ms)
}

/// How ERAM's position differs from OpenATCA's at ERAM's report time, metres.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Separation {
    /// Signed, positive to the right of OpenATCA's direction of travel.
    pub cross_m: f64,
    /// Signed, positive when ERAM is ahead of OpenATCA. Raw: the classification uses this.
    pub along_m: f64,
    /// The distance OpenATCA's aircraft covers in the ERAM stamp quantum (ground speed times
    /// 1 s). ERAM stamps whole seconds, so an along-track difference this small may be the
    /// stamp; events carry a flag for it, the classification does not use it.
    pub credit_m: f64,
    /// `hypot(cross, along)`.
    pub horiz_m: f64,
}

/// Cross-track and along-track separation of an ERAM position from OpenATCA's state, relative
/// to OpenATCA's direction of travel. When the aircraft is slower than `low_speed_mps` its
/// direction is undefined and the whole separation counts as cross-track.
pub fn separation(eram: &EramSample, oa: &OaPoint, th: &Thresholds) -> Separation {
    let (e, n) = enu_m(oa.lat_deg, oa.lon_deg, deg(eram.lat_e7), deg(eram.lon_e7));
    let speed = oa.vel_east_mps.hypot(oa.vel_north_mps);
    if speed.is_nan() || speed < th.low_speed_mps {
        let d = e.hypot(n);
        return Separation {
            cross_m: d,
            along_m: 0.0,
            credit_m: 0.0,
            horiz_m: d,
        };
    }
    let (ux, uy) = (oa.vel_east_mps / speed, oa.vel_north_mps / speed);
    let along = e * ux + n * uy;
    let cross = e * uy - n * ux;
    Separation {
        cross_m: cross,
        along_m: along,
        credit_m: speed * th.eram_time_quant_ms as f64 / 1000.0,
        horiz_m: cross.hypot(along),
    }
}

/// Which system was closer to the truth: a name for the adjudication column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closer {
    Eram,
    OpenAtca,
    Tie,
    /// No truth at that time.
    NoTruth,
}

impl Closer {
    pub fn name(self) -> &'static str {
        match self {
            Closer::Eram => "ERAM",
            Closer::OpenAtca => "OpenATCA",
            Closer::Tie => "tie",
            Closer::NoTruth => "",
        }
    }
}

/// Which of ERAM and OpenATCA is closer to the truth at the ERAM report time: both distances
/// are to the truth position (degrees) at that time, `None` when it has none. Used only after a
/// difference is classified.
pub fn adjudicate(
    eram: &EramSample,
    oa: &OaPoint,
    truth: Option<(f64, f64)>,
    th: &Thresholds,
) -> Closer {
    let Some(t) = truth else {
        return Closer::NoTruth;
    };
    let (ee, en) = enu_m(t.0, t.1, deg(eram.lat_e7), deg(eram.lon_e7));
    let (oe, on) = enu_m(t.0, t.1, oa.lat_deg, oa.lon_deg);
    let (d_eram, d_oa) = (ee.hypot(en), oe.hypot(on));
    if (d_eram - d_oa).abs() < th.tie_m {
        Closer::Tie
    } else if d_eram < d_oa {
        Closer::Eram
    } else {
        Closer::OpenAtca
    }
}

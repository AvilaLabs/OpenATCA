//! RAP against the winds and temperatures aircraft report over ADS-B (`oa-adsb` measurements):
//! readsb's `ws`/`wd` (derived from true airspeed, true heading, ground speed and track) and
//! `oat` (derived from Mach and true airspeed). Shell (B1, B7, C6): pure, but it works on
//! `oa-adsb` reports. The query is a closure, so the statistics are tested on synthetic hours and run against the archive by `oa-wind-check`.
//! What the readsb values mean and their limits are in the `oa-adsb` crate documentation.

use crate::shell::check::{obs_components, refusal_name, summarize, Summary};
use crate::shell::error::{LoadError, QueryError};
use crate::shell::sample::{Query, Sample};
use oa_adsb::{reports_len, Reports, F_GROUND, NONE_I16, NONE_I32, NONE_U16};

/// Limit: observations held for one comparison. At one observation per aircraft per
/// [`DEFAULT_MIN_GAP_S`] seconds a whole day over the contiguous US is well inside it.
pub const MAX_OBS: usize = 20_000_000;
/// Keep at most one observation per aircraft in this many seconds: consecutive readsb values
/// repeat or differ by noise, so denser rows are not independent.
pub const DEFAULT_MIN_GAP_S: f64 = 30.0;
/// Lower edges (pressure altitude, feet) of the altitude bands after the first.
pub const BAND_EDGES_FT: [f64; 3] = [10_000.0, 18_000.0, 30_000.0];
/// Names of the altitude bands, lowest first.
pub const BAND_NAMES: [&str; 4] = [
    "below 10000 ft",
    "10000 to 17999 ft",
    "18000 to 29999 ft",
    "30000 ft and above",
];
/// readsb does not compute a wind while the roll exceeds this many degrees or the track rate this
/// many degrees per second; a row that exceeds either is called turning here.
pub const TURN_ROLL_DEG: f64 = 10.0;
pub const TURN_RATE_DPS: f64 = 0.5;

/// One aircraft observation: where, when and what readsb reported there.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsbObs {
    pub lat: f64,
    pub lon: f64,
    /// Barometric (pressure) altitude, feet.
    pub alt_ft: f64,
    pub time_s: f64,
    /// readsb's wind: direction the wind blows from (degrees true) and speed (knots).
    pub wind: Option<(f64, f64)>,
    /// readsb's outer air temperature, degrees C.
    pub oat_c: Option<f64>,
    /// Roll (degrees) and track rate (degrees per second), when the row has them.
    pub roll_deg: Option<f64>,
    pub track_rate_dps: Option<f64>,
}

/// Collects the observations of `r` that have a wind or a temperature and a barometric altitude
/// (not on the ground), at most one per `min_gap_s` seconds per aircraft, appending to `out`
/// until it holds [`MAX_OBS`]. `r` must hold one aircraft's reports in time order (what
/// `oa_adsb::scan_archive` hands over per trace). Returns the rows it could not keep for the
/// size limit.
pub fn adsb_obs(r: &Reports, min_gap_s: f64, out: &mut Vec<AdsbObs>) -> usize {
    let mut dropped = 0;
    let mut last = f64::NEG_INFINITY;
    for i in 0..reports_len(r) {
        let (Some(&alt), Some(&fl), Some(&t), Some(&la), Some(&lo)) = (
            r.baro_alt_ft.get(i),
            r.flags.get(i),
            r.time_ns.get(i),
            r.lat_e7.get(i),
            r.lon_e7.get(i),
        ) else {
            continue;
        };
        if alt == NONE_I32 || fl & F_GROUND != 0 {
            continue;
        }
        let u = |c: &[u16]| c.get(i).copied().filter(|&v| v != NONE_U16);
        let s = |c: &[i16]| c.get(i).copied().filter(|&v| v != NONE_I16);
        let wind = match (u(&r.ws_kt), u(&r.wd_deg)) {
            (Some(ws), Some(wd)) => Some((f64::from(wd), f64::from(ws))),
            _ => None,
        };
        let oat_c = s(&r.oat_c).map(f64::from);
        if wind.is_none() && oat_c.is_none() {
            continue;
        }
        let time_s = t as f64 * 1e-9;
        if time_s < last + min_gap_s {
            continue;
        }
        if out.len() >= MAX_OBS {
            dropped += 1;
            continue;
        }
        last = time_s;
        out.push(AdsbObs {
            lat: f64::from(la) * 1e-7,
            lon: f64::from(lo) * 1e-7,
            alt_ft: f64::from(alt),
            time_s,
            wind,
            oat_c,
            roll_deg: s(&r.roll_cdeg).map(|v| f64::from(v) * 0.01),
            track_rate_dps: s(&r.track_rate_cdps).map(|v| f64::from(v) * 0.01),
        });
    }
    dropped
}

/// Whether the aircraft was turning at the observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// Roll or track rate known and both below readsb's gates.
    Straight,
    /// Roll or track rate over the gate.
    Turning,
    /// Neither is known.
    Unknown,
}

/// Names of [`Turn`] in index order.
pub const TURN_NAMES: [&str; 3] = ["straight", "turning", "turn unknown"];

/// Turning state of an observation: over a gate in either value is turning, otherwise known
/// straight if at least one value is present.
pub fn turn_state(o: &AdsbObs) -> Turn {
    let over = o.roll_deg.is_some_and(|v| v.abs() > TURN_ROLL_DEG)
        || o.track_rate_dps.is_some_and(|v| v.abs() > TURN_RATE_DPS);
    if over {
        Turn::Turning
    } else if o.roll_deg.is_some() || o.track_rate_dps.is_some() {
        Turn::Straight
    } else {
        Turn::Unknown
    }
}

/// Altitude band index (0 to 3) of a pressure altitude in feet.
pub fn band_of(alt_ft: f64) -> usize {
    BAND_EDGES_FT.iter().filter(|&&e| alt_ft >= e).count()
}

/// One compared observation (model minus observation).
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Compared {
    pub band: usize,
    pub turn: Turn,
    /// Wind: vector difference magnitude, signed speed difference, eastward and northward
    /// difference, all m/s; `None` when the observation has no wind.
    pub wind: Option<WindDiff>,
    /// Temperature difference, K (RAP T minus readsb `oat`); `None` without an `oat`.
    pub dt_k: Option<f64>,
}

/// A wind difference, m/s.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindDiff {
    pub vector: f64,
    pub speed: f64,
    pub du: f64,
    pub dv: f64,
}

/// Compared observations and what was refused.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AdsbCompare {
    /// Observations offered.
    pub offered: usize,
    /// Observations the model refused, by reason name; they are not in `rows`.
    pub refused: Vec<(&'static str, usize)>,
    pub rows: Vec<Compared>,
}

/// Celsius to kelvin.
const ZERO_C_K: f64 = 273.15;

/// Compares RAP, queried through `sample`, with every observation. A refused query is counted by
/// reason and the observation left out; a failed file load stops with its error. Observations
/// in time order keep the archive's cache from reloading.
pub fn compare_adsb(
    obs: &[AdsbObs],
    sample: &mut dyn FnMut(&Query) -> Result<Sample, QueryError>,
) -> Result<AdsbCompare, LoadError> {
    let mut out = AdsbCompare {
        offered: obs.len(),
        ..AdsbCompare::default()
    };
    for o in obs {
        let q = Query {
            lat: o.lat,
            lon: o.lon,
            alt_ft: o.alt_ft,
            time_s: o.time_s,
        };
        let s = match sample(&q) {
            Ok(s) => s,
            Err(QueryError::Refused(r)) => {
                let name = refusal_name(&r);
                match out.refused.iter_mut().find(|(n, _)| *n == name) {
                    Some(e) => e.1 += 1,
                    None => out.refused.push((name, 1)),
                }
                continue;
            }
            Err(QueryError::Load(e)) => return Err(e),
        };
        let wind = o.wind.map(|(dir, kt)| {
            let (ou, ov) = obs_components(dir, kt);
            WindDiff {
                vector: (s.u_ms - ou).hypot(s.v_ms - ov),
                speed: s.u_ms.hypot(s.v_ms) - ou.hypot(ov),
                du: s.u_ms - ou,
                dv: s.v_ms - ov,
            }
        });
        out.rows.push(Compared {
            band: band_of(o.alt_ft),
            turn: turn_state(o),
            wind,
            dt_k: o.oat_c.map(|c| s.temp_k - (c + ZERO_C_K)),
        });
    }
    Ok(out)
}

/// Statistics of one group of compared observations.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct GroupStats {
    pub name: String,
    /// Vector difference magnitude, m/s; `None` with no wind observation.
    pub wind: Option<Summary>,
    /// Mean (RAP minus readsb) speed, eastward and northward difference, m/s.
    pub speed_bias: f64,
    pub bias_u: f64,
    pub bias_v: f64,
    /// Absolute temperature difference, K; `None` with no temperature observation.
    pub temp_abs: Option<Summary>,
    /// Mean (RAP minus readsb) temperature difference, K.
    pub temp_bias: f64,
}

/// Statistics of the compared observations for which `keep` holds.
pub fn group_stats(c: &AdsbCompare, name: &str, keep: &dyn Fn(&Compared) -> bool) -> GroupStats {
    let (mut vec, mut tmp) = (Vec::new(), Vec::new());
    let (mut sb, mut bu, mut bv, mut tb) = (0.0, 0.0, 0.0, 0.0);
    for r in c.rows.iter().filter(|r| keep(r)) {
        if let Some(w) = r.wind {
            vec.push(w.vector);
            sb += w.speed;
            bu += w.du;
            bv += w.dv;
        }
        if let Some(d) = r.dt_k {
            tmp.push(d.abs());
            tb += d;
        }
    }
    let (nw, nt) = (vec.len().max(1) as f64, tmp.len().max(1) as f64);
    GroupStats {
        name: name.to_string(),
        wind: summarize(&mut vec),
        speed_bias: sb / nw,
        bias_u: bu / nw,
        bias_v: bv / nw,
        temp_abs: summarize(&mut tmp),
        temp_bias: tb / nt,
    }
}

/// All, each altitude band and each turning state, in that order.
pub fn all_groups(c: &AdsbCompare) -> Vec<GroupStats> {
    let mut v = vec![group_stats(c, "all", &|_| true)];
    for (k, name) in BAND_NAMES.iter().enumerate() {
        v.push(group_stats(c, name, &|r| r.band == k));
    }
    for (t, name) in [Turn::Straight, Turn::Turning, Turn::Unknown]
        .into_iter()
        .zip(TURN_NAMES)
    {
        v.push(group_stats(c, name, &|r| r.turn == t));
    }
    v
}

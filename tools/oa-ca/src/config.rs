//! The configuration file: plain text, one `key = value` per line, `#` starts a comment. Every
//! key is required (the core has no defaults and neither has the tool); an unknown or repeated
//! key, a value that does not parse or is not finite, and a missing key are errors that name it.
//! Units are the ones in the key: seconds, nautical miles, feet, metres, degrees.
//!
//! Volumes are lines `volume = ...`, tried in file order:
//!
//! ```text
//! volume = box <lat_min> <lat_max> <lon_min> <lon_max> <alt_lo_ft> <alt_hi_ft> <h_nm> <v_ft>
//! volume = polygon <alt_lo_ft> <alt_hi_ft> <h_nm> <v_ft> <lat>,<lon> <lat>,<lon> <lat>,<lon> ...
//! ```
//!
//! The default volume (a pair outside every volume above) is `default_h_nm` and `default_v_ft`
//! over the whole globe and every altitude.

use std::collections::BTreeMap;

use oa_ca::{Area, CaConfig, Volume, MAX_VOLUMES};
use oa_safety::geom::polygon_new;

const NM: f64 = 1852.0;
const S_NS: f64 = 1.0e9;

/// What the tool needs beyond the core's configuration.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub ca: CaConfig,
    /// A report of the net state older than this reads as unavailable, ns.
    pub net_stale_limit_ns: i64,
    /// A report of the net state may be dated at most this far after the clock, ns.
    pub net_future_horizon_ns: i64,
    pub alert_log_capacity: usize,
    pub event_log_capacity: usize,
}

const KEYS: [&str; 17] = [
    "look_ahead_s",
    "hysteresis_s",
    "default_h_nm",
    "default_v_ft",
    "max_tracks",
    "max_open",
    "max_candidates",
    "work_budget",
    "eval_budget",
    "max_speed_mps",
    "r95_cap_m",
    "grid_band_m",
    "net_stale_limit_s",
    "net_future_horizon_s",
    "alert_log_capacity",
    "event_log_capacity",
    "volume",
];

fn finite(key: &str, v: &str) -> Result<f64, String> {
    let x: f64 = v
        .parse()
        .map_err(|_| format!("{key}: `{v}` is not a number"))?;
    if x.is_finite() {
        Ok(x)
    } else {
        Err(format!("{key}: `{v}` is not finite"))
    }
}

fn whole(key: &str, v: &str) -> Result<u64, String> {
    v.parse::<u64>()
        .map_err(|_| format!("{key}: `{v}` is not a whole number"))
}

fn to_usize(key: &str, v: &str) -> Result<usize, String> {
    usize::try_from(whole(key, v)?).map_err(|_| format!("{key}: `{v}` is too large"))
}

fn seconds_ns(key: &str, v: &str) -> Result<i64, String> {
    let s = finite(key, v)?;
    let ns = s * S_NS;
    if !(0.0..9.0e18).contains(&ns) {
        return Err(format!("{key}: `{v}` is out of range"));
    }
    Ok(ns.round() as i64)
}

fn e7(key: &str, v: &str, limit_deg: f64) -> Result<i32, String> {
    let d = finite(key, v)?;
    if d.abs() > limit_deg {
        return Err(format!("{key}: `{v}` is off the globe"));
    }
    Ok((d * 1.0e7).round() as i32)
}

fn feet(key: &str, v: &str) -> Result<i32, String> {
    let x = finite(key, v)?;
    if x.abs() > 1.0e6 {
        return Err(format!("{key}: `{v}` is out of range"));
    }
    Ok(x.round() as i32)
}

fn volume(v: &str) -> Result<Volume, String> {
    let f: Vec<&str> = v.split_whitespace().collect();
    match f.as_slice() {
        ["box", la0, la1, lo0, lo1, z0, z1, h, z] => Ok(Volume {
            area: Area::Box {
                lat_min_e7: e7("volume lat_min", la0, 90.0)?,
                lat_max_e7: e7("volume lat_max", la1, 90.0)?,
                lon_min_e7: e7("volume lon_min", lo0, 180.0)?,
                lon_max_e7: e7("volume lon_max", lo1, 180.0)?,
            },
            alt_lo_ft: feet("volume alt_lo_ft", z0)?,
            alt_hi_ft: feet("volume alt_hi_ft", z1)?,
            h_m: finite("volume h_nm", h)? * NM,
            v_ft: finite("volume v_ft", z)?,
        }),
        ["polygon", z0, z1, h, z, points @ ..] if points.len() >= 3 => {
            let mut pts = Vec::new();
            for p in points {
                let (la, lo) = p
                    .split_once(',')
                    .ok_or_else(|| format!("volume: `{p}` is not lat,lon"))?;
                pts.push((e7("volume lat", la, 90.0)?, e7("volume lon", lo, 180.0)?));
            }
            let poly = polygon_new(&pts).map_err(|e| format!("volume polygon: {e}"))?;
            Ok(Volume {
                area: Area::Polygon(poly),
                alt_lo_ft: feet("volume alt_lo_ft", z0)?,
                alt_hi_ft: feet("volume alt_hi_ft", z1)?,
                h_m: finite("volume h_nm", h)? * NM,
                v_ft: finite("volume v_ft", z)?,
            })
        }
        _ => Err(format!(
            "volume: `{v}` is neither `box` with 8 values nor `polygon` with 4 values and 3 or more points"
        )),
    }
}

/// Parses the configuration text, and checks it with the core's own check.
pub fn parse_config(text: &str) -> Result<Settings, String> {
    let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
    let mut volumes: Vec<&str> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected `key = value`", n + 1))?;
        let (k, v) = (k.trim(), v.trim());
        if !KEYS.contains(&k) {
            return Err(format!("line {}: unknown key `{k}`", n + 1));
        }
        if k == "volume" {
            volumes.push(v);
        } else if kv.insert(k, v).is_some() {
            return Err(format!("line {}: `{k}` given twice", n + 1));
        }
    }
    let get = |k: &str| -> Result<&str, String> {
        kv.get(k)
            .copied()
            .ok_or_else(|| format!("the configuration has no `{k}`"))
    };
    if volumes.len() > MAX_VOLUMES {
        return Err(format!("more than {MAX_VOLUMES} volumes"));
    }
    let default_volume = Volume {
        area: Area::Box {
            lat_min_e7: -900_000_000,
            lat_max_e7: 900_000_000,
            lon_min_e7: -1_800_000_000,
            lon_max_e7: 1_800_000_000,
        },
        alt_lo_ft: -100_000,
        alt_hi_ft: 100_000,
        h_m: finite("default_h_nm", get("default_h_nm")?)? * NM,
        v_ft: finite("default_v_ft", get("default_v_ft")?)?,
    };
    let mut list = [default_volume; MAX_VOLUMES];
    for (slot, v) in list.iter_mut().zip(volumes.iter()) {
        *slot = volume(v)?;
    }
    let ca = CaConfig {
        max_tracks: to_usize("max_tracks", get("max_tracks")?)?,
        max_open: to_usize("max_open", get("max_open")?)?,
        max_candidates: to_usize("max_candidates", get("max_candidates")?)?,
        look_ahead_ns: seconds_ns("look_ahead_s", get("look_ahead_s")?)?,
        hysteresis_ns: seconds_ns("hysteresis_s", get("hysteresis_s")?)?,
        work_budget: whole("work_budget", get("work_budget")?)?,
        eval_budget: to_usize("eval_budget", get("eval_budget")?)?,
        max_speed_mps: finite("max_speed_mps", get("max_speed_mps")?)?,
        r95_cap_m: finite("r95_cap_m", get("r95_cap_m")?)?,
        grid_band_m: finite("grid_band_m", get("grid_band_m")?)?,
        default_volume,
        volumes: list,
        n_volumes: volumes.len(),
    };
    oa_ca::ca_check(&ca).map_err(|e| format!("configuration refused by conflict alert: {e}"))?;
    Ok(Settings {
        ca,
        net_stale_limit_ns: seconds_ns("net_stale_limit_s", get("net_stale_limit_s")?)?,
        net_future_horizon_ns: seconds_ns("net_future_horizon_s", get("net_future_horizon_s")?)?,
        alert_log_capacity: to_usize("alert_log_capacity", get("alert_log_capacity")?)?,
        event_log_capacity: to_usize("event_log_capacity", get("event_log_capacity")?)?,
    })
}

//! The configuration file: plain text, one `key = value` per line, `#` starts a comment. Every
//! single-valued key is required (the core has no defaults and neither has the tool); an unknown
//! or repeated key, a value that does not parse or is not finite, and a missing key are errors
//! that name it. Units are the ones in the key: seconds, nautical miles, feet, metres, degrees.
//!
//! Three keys may repeat:
//!
//! ```text
//! procedure = <floor_ft> <lat>,<lon> <lat>,<lon> <lat>,<lon> ...
//! cifp_airport = <ICAO identifier>
//! inhibit = <start_s> <end_s> <alt_lo_ft> <alt_hi_ft> <author> <reason> <lat>,<lon> <lat>,<lon> <lat>,<lon> ...
//! ```
//!
//! A procedure line is a procedure area of the core, with its published altitude. A `cifp_airport`
//! line asks for the procedure areas the CIFP gives at that airport (needs `--cifp`). An inhibit
//! line is an inhibition of the net made when the run starts: its times are seconds after the
//! first measurement of the clip, the author and the reason are one word each (`_` is a space).

use std::collections::BTreeMap;

use oa_msaw::{MsawConfig, ProcArea, MAX_PROCS};
use oa_safety::geom::{polygon_new, POLYGON_EMPTY};
use oa_safety::inhibit::InhibitConfig;
use oa_safety::net::NET_COUNT;

const S_NS: f64 = 1.0e9;
/// The most entries a log may be asked to hold (the tables are allocated whole at the start).
const MAX_LOG_CAPACITY: usize = 1 << 24;
/// The longest configuration text read, bytes (a real one is a few hundred lines).
const MAX_TEXT_BYTES: usize = 1 << 20;
/// The most inhibitions and CIFP airports a configuration may name.
const MAX_LINES: usize = 64;

/// One inhibition of the configuration.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct InhibitSpec {
    pub start_s: f64,
    pub end_s: f64,
    pub alt_lo_ft: i32,
    pub alt_hi_ft: i32,
    pub author: String,
    pub reason: String,
    pub vertices: Vec<(i32, i32)>,
}

/// What the tool needs beyond the core's configuration.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub msaw: MsawConfig,
    /// A report of the net state older than this reads as unavailable, ns.
    pub net_stale_limit_ns: i64,
    /// A report of the net state may be dated at most this far after the clock, ns.
    pub net_future_horizon_ns: i64,
    pub alert_log_capacity: usize,
    pub skip_log_capacity: usize,
    pub inhibit_cfg: InhibitConfig,
    pub inhibits: Vec<InhibitSpec>,
    pub cifp_airports: Vec<String>,
    /// The radius of the procedure area drawn round a runway threshold, nautical miles.
    pub cifp_radius_nm: f64,
}

const KEYS: [&str; 22] = [
    "look_ahead_s",
    "warning_s",
    "samples",
    "hysteresis_s",
    "terrain_clearance_ft",
    "obstacle_clearance_ft",
    "max_tracks",
    "max_open",
    "track_budget",
    "max_speed_mps",
    "r95_cap_m",
    "net_stale_limit_s",
    "net_future_horizon_s",
    "alert_log_capacity",
    "skip_log_capacity",
    "inhibit_max_duration_s",
    "inhibit_max_start_ahead_s",
    "inhibit_max_extent_deg",
    "cifp_radius_nm",
    "procedure",
    "cifp_airport",
    "inhibit",
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

/// A capacity of a log: at most [`MAX_LOG_CAPACITY`].
fn log_capacity(key: &str, v: &str) -> Result<usize, String> {
    let n = to_usize(key, v)?;
    if n > MAX_LOG_CAPACITY {
        return Err(format!("{key}: `{v}` is more than {MAX_LOG_CAPACITY}"));
    }
    Ok(n)
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

fn points(what: &str, p: &[&str]) -> Result<Vec<(i32, i32)>, String> {
    if p.len() < 3 || p.len() > oa_safety::geom::MAX_VERTICES {
        return Err(format!(
            "{what}: needs 3 to {} points",
            oa_safety::geom::MAX_VERTICES
        ));
    }
    let mut out = Vec::new();
    for q in p {
        let (la, lo) = q
            .split_once(',')
            .ok_or_else(|| format!("{what}: `{q}` is not lat,lon"))?;
        out.push((e7("lat", la, 90.0)?, e7("lon", lo, 180.0)?));
    }
    Ok(out)
}

fn procedure(v: &str) -> Result<ProcArea, String> {
    let f: Vec<&str> = v.split_whitespace().collect();
    let (floor, pts) = f
        .split_first()
        .ok_or_else(|| "procedure: empty".to_string())?;
    let verts = points("procedure", pts)?;
    Ok(ProcArea {
        area: polygon_new(&verts).map_err(|e| format!("procedure polygon: {e}"))?,
        floor_ft: feet("procedure floor_ft", floor)?,
    })
}

fn word(what: &str, w: &str) -> Result<String, String> {
    let s = w.replace('_', " ");
    if s.trim().is_empty() {
        return Err(format!("inhibit {what}: empty"));
    }
    Ok(s)
}

fn inhibit(v: &str) -> Result<InhibitSpec, String> {
    let f: Vec<&str> = v.split_whitespace().collect();
    let [s, e, lo, hi, author, reason, pts @ ..] = f.as_slice() else {
        return Err(format!(
            "inhibit: `{v}` needs start_s end_s alt_lo_ft alt_hi_ft author reason and 3 or more points"
        ));
    };
    let (start_s, end_s) = (finite("inhibit start_s", s)?, finite("inhibit end_s", e)?);
    if start_s < 0.0 || end_s <= start_s {
        return Err("inhibit: the end must be after the start, which is not negative".to_string());
    }
    let vertices = points("inhibit", pts)?;
    polygon_new(&vertices).map_err(|e| format!("inhibit polygon: {e}"))?;
    Ok(InhibitSpec {
        start_s,
        end_s,
        alt_lo_ft: feet("inhibit alt_lo_ft", lo)?,
        alt_hi_ft: feet("inhibit alt_hi_ft", hi)?,
        author: word("author", author)?,
        reason: word("reason", reason)?,
        vertices,
    })
}

/// Parses the configuration text, and checks it with the core's own check.
pub fn parse_config(text: &str) -> Result<Settings, String> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "the configuration is longer than {MAX_TEXT_BYTES} bytes"
        ));
    }
    let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
    let (mut procs, mut airports, mut inhibits): (Vec<&str>, Vec<&str>, Vec<&str>) =
        (Vec::new(), Vec::new(), Vec::new());
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
        match k {
            "procedure" => procs.push(v),
            "cifp_airport" => airports.push(v),
            "inhibit" => inhibits.push(v),
            _ => {
                if kv.insert(k, v).is_some() {
                    return Err(format!("line {}: `{k}` given twice", n + 1));
                }
            }
        }
    }
    let get = |k: &str| -> Result<&str, String> {
        kv.get(k)
            .copied()
            .ok_or_else(|| format!("the configuration has no `{k}`"))
    };
    if procs.len() > MAX_PROCS {
        return Err(format!("more than {MAX_PROCS} procedure areas"));
    }
    if airports.len() > MAX_LINES || inhibits.len() > MAX_LINES {
        return Err(format!(
            "more than {MAX_LINES} cifp_airport or inhibit lines"
        ));
    }
    let mut list = [ProcArea {
        area: POLYGON_EMPTY,
        floor_ft: 0,
    }; MAX_PROCS];
    for (slot, v) in list.iter_mut().zip(procs.iter()) {
        *slot = procedure(v)?;
    }
    let msaw = MsawConfig {
        max_tracks: to_usize("max_tracks", get("max_tracks")?)?,
        max_open: to_usize("max_open", get("max_open")?)?,
        track_budget: to_usize("track_budget", get("track_budget")?)?,
        look_ahead_s: finite("look_ahead_s", get("look_ahead_s")?)?,
        warning_s: finite("warning_s", get("warning_s")?)?,
        samples: to_usize("samples", get("samples")?)?,
        hysteresis_ns: seconds_ns("hysteresis_s", get("hysteresis_s")?)?,
        terrain_clearance_ft: feet("terrain_clearance_ft", get("terrain_clearance_ft")?)?,
        obstacle_clearance_ft: feet("obstacle_clearance_ft", get("obstacle_clearance_ft")?)?,
        max_speed_mps: finite("max_speed_mps", get("max_speed_mps")?)?,
        r95_cap_m: finite("r95_cap_m", get("r95_cap_m")?)?,
        procs: list,
        n_procs: procs.len(),
    };
    oa_msaw::msaw_check(&msaw).map_err(|e| format!("configuration refused by the MSAW: {e}"))?;
    let max_duration = seconds_ns("inhibit_max_duration_s", get("inhibit_max_duration_s")?)?;
    let inhibit_cfg = InhibitConfig {
        max_duration_ns: [Some(max_duration); NET_COUNT],
        max_start_ahead_ns: Some(seconds_ns(
            "inhibit_max_start_ahead_s",
            get("inhibit_max_start_ahead_s")?,
        )?),
        max_extent_e7: Some(e7(
            "inhibit_max_extent_deg",
            get("inhibit_max_extent_deg")?,
            180.0,
        )?),
    };
    let cifp_radius_nm = finite("cifp_radius_nm", get("cifp_radius_nm")?)?;
    if !(cifp_radius_nm > 0.0 && cifp_radius_nm <= 50.0) {
        return Err("cifp_radius_nm: must be above 0 and at most 50".to_string());
    }
    let mut specs = Vec::new();
    for v in &inhibits {
        specs.push(inhibit(v)?);
    }
    Ok(Settings {
        msaw,
        net_stale_limit_ns: seconds_ns("net_stale_limit_s", get("net_stale_limit_s")?)?,
        net_future_horizon_ns: seconds_ns("net_future_horizon_s", get("net_future_horizon_s")?)?,
        alert_log_capacity: log_capacity("alert_log_capacity", get("alert_log_capacity")?)?,
        skip_log_capacity: log_capacity("skip_log_capacity", get("skip_log_capacity")?)?,
        inhibit_cfg,
        inhibits: specs,
        cifp_airports: airports.iter().map(|s| (*s).to_string()).collect(),
        cifp_radius_nm,
    })
}

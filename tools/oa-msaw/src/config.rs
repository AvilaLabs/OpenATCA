//! The configuration file: plain text, one `key = value` per line, `#` starts a comment. Every
//! single-valued key is required (the core has no defaults and neither has the tool); an unknown
//! or repeated key, a value that does not parse or is not finite, and a missing key are errors
//! that name it. Units are the ones in the key: seconds, nautical miles, feet, metres, degrees.
//!
//! Four keys may repeat:
//!
//! ```text
//! corridor = <thr_lat> <thr_lon> <out_bearing_deg> <length_m> <near_half_m> <far_half_m> <thr_elev_ft> <faf_alt_ft>
//! cifp_airport = <ICAO identifier>
//! msaw_request = <addr:HEX24 | call:CALLSIGN> <start_s> <end_s>
//! inhibit = <start_s> <end_s> <alt_lo_ft> <alt_hi_ft> <author> <reason> <lat>,<lon> <lat>,<lon> <lat>,<lon> ...
//! ```
//!
//! A corridor line is a final-approach corridor of the core. A `cifp_airport` line asks for the
//! corridors the CIFP gives at that airport (needs `--cifp`). An inhibit
//! line is an inhibition of the net made when the run starts: its times are seconds after the
//! first measurement of the clip, the author and the reason are one word each (`_` is a space). A
//! `msaw_request` line is a pilot's request for MSAW service: a track with that address or callsign
//! in the span (seconds after the first measurement of the clip) is alerted whatever its flight
//! plan says.

use std::collections::BTreeMap;

use oa_msaw::{Corridor, MsawConfig, CK_APPROACH, MAX_CORRIDORS};
use oa_safety::geom::polygon_new;
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

/// Who a request for MSAW service names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requester {
    /// The 24-bit ICAO address.
    Address(u32),
    /// The callsign, upper case, at most eight characters.
    Callsign(String),
}

/// One `msaw_request` line: the track is alerted in the span, seconds after the first
/// measurement of the clip.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct RequestSpec {
    pub who: Requester,
    pub start_s: f64,
    pub end_s: f64,
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
    pub requests: Vec<RequestSpec>,
    /// A track below FL180 with no usable altimeter setting is given its pressure altitude lowered
    /// by this, feet (the conservative direction), and counted.
    pub no_qnh_allowance_ft: i32,
    /// The altimeter setting is taken from the nearest METAR station within this, nautical miles,
    /// whose observation is at most `metar_max_age_s` seconds old.
    pub metar_radius_nm: f64,
    pub metar_max_age_s: i64,
}

const KEYS: [&str; 29] = [
    "look_ahead_s",
    "warning_s",
    "samples",
    "hysteresis_s",
    "terrain_clearance_ft",
    "obstacle_clearance_ft",
    "approach_allowance_ft",
    "approach_min_clearance_ft",
    "departure_min_clearance_ft",
    "velocity_uncertainty_mps",
    "no_qnh_allowance_ft",
    "metar_radius_nm",
    "metar_max_age_s",
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
    "corridor",
    "cifp_airport",
    "msaw_request",
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

fn corridor(v: &str) -> Result<Corridor, String> {
    let f: Vec<&str> = v.split_whitespace().collect();
    let [la, lo, bearing, length, near, far, thr_elev, faf_alt] = f.as_slice() else {
        return Err(format!(
            "corridor: `{v}` needs thr_lat thr_lon out_bearing_deg length_m near_half_m far_half_m thr_elev_ft faf_alt_ft"
        ));
    };
    Ok(Corridor {
        kind: CK_APPROACH,
        thr_lat_e7: e7("corridor lat", la, 90.0)?,
        thr_lon_e7: e7("corridor lon", lo, 180.0)?,
        out_bearing_deg: finite("corridor out_bearing_deg", bearing)?,
        length_m: finite("corridor length_m", length)?,
        near_half_width_m: finite("corridor near_half_m", near)?,
        far_half_width_m: finite("corridor far_half_m", far)?,
        thr_elev_ft: feet("corridor thr_elev_ft", thr_elev)?,
        faf_alt_ft: feet("corridor faf_alt_ft", faf_alt)?,
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

fn request(v: &str) -> Result<RequestSpec, String> {
    let f: Vec<&str> = v.split_whitespace().collect();
    let [who, s, e] = f.as_slice() else {
        return Err(format!(
            "msaw_request: `{v}` needs addr:HEX or call:CALLSIGN, start_s and end_s"
        ));
    };
    let who = if let Some(h) = who.strip_prefix("addr:") {
        match u32::from_str_radix(h, 16) {
            Ok(a) if h.len() == 6 => Requester::Address(a),
            _ => return Err(format!("msaw_request: `{h}` is not a 24-bit hex address")),
        }
    } else if let Some(c) = who.strip_prefix("call:") {
        let c = c.to_ascii_uppercase();
        if c.is_empty() || c.len() > 8 || !c.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(format!("msaw_request: `{c}` is not a callsign"));
        }
        Requester::Callsign(c)
    } else {
        return Err(format!("msaw_request: `{who}` must start addr: or call:"));
    };
    let (start_s, end_s) = (
        finite("msaw_request start_s", s)?,
        finite("msaw_request end_s", e)?,
    );
    if start_s < 0.0 || end_s <= start_s {
        return Err(
            "msaw_request: the end must be after the start, which is not negative".to_string(),
        );
    }
    Ok(RequestSpec {
        who,
        start_s,
        end_s,
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
    let (mut procs, mut airports, mut inhibits, mut requests): (
        Vec<&str>,
        Vec<&str>,
        Vec<&str>,
        Vec<&str>,
    ) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
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
            "corridor" => procs.push(v),
            "cifp_airport" => airports.push(v),
            "inhibit" => inhibits.push(v),
            "msaw_request" => requests.push(v),
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
    if procs.len() > MAX_CORRIDORS {
        return Err(format!("more than {MAX_CORRIDORS} corridors"));
    }
    if airports.len() > MAX_LINES || inhibits.len() > MAX_LINES || requests.len() > MAX_LINES {
        return Err(format!(
            "more than {MAX_LINES} cifp_airport, msaw_request or inhibit lines"
        ));
    }
    let mut list = [Corridor {
        kind: CK_APPROACH,
        thr_lat_e7: 0,
        thr_lon_e7: 0,
        out_bearing_deg: 0.0,
        length_m: 1.0,
        near_half_width_m: 1.0,
        far_half_width_m: 1.0,
        thr_elev_ft: 0,
        faf_alt_ft: 0,
    }; MAX_CORRIDORS];
    for (slot, v) in list.iter_mut().zip(procs.iter()) {
        *slot = corridor(v)?;
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
        approach_allowance_ft: feet("approach_allowance_ft", get("approach_allowance_ft")?)?,
        approach_min_clearance_ft: feet(
            "approach_min_clearance_ft",
            get("approach_min_clearance_ft")?,
        )?,
        departure_min_clearance_ft: feet(
            "departure_min_clearance_ft",
            get("departure_min_clearance_ft")?,
        )?,
        velocity_uncertainty_mps: finite(
            "velocity_uncertainty_mps",
            get("velocity_uncertainty_mps")?,
        )?,
        corridors: list,
        n_corridors: procs.len(),
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
    let no_qnh_allowance_ft = feet("no_qnh_allowance_ft", get("no_qnh_allowance_ft")?)?;
    if no_qnh_allowance_ft < 0 {
        return Err("no_qnh_allowance_ft: must not be negative".to_string());
    }
    let metar_radius_nm = finite("metar_radius_nm", get("metar_radius_nm")?)?;
    if !(metar_radius_nm > 0.0 && metar_radius_nm <= 1000.0) {
        return Err("metar_radius_nm: must be above 0 and at most 1000".to_string());
    }
    let metar_max_age_s = seconds_ns("metar_max_age_s", get("metar_max_age_s")?)? / 1_000_000_000;
    if metar_max_age_s <= 0 {
        return Err("metar_max_age_s: must be above 0".to_string());
    }
    let mut specs = Vec::new();
    for v in &inhibits {
        specs.push(inhibit(v)?);
    }
    let mut reqs = Vec::new();
    for v in &requests {
        reqs.push(request(v)?);
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
        requests: reqs,
        no_qnh_allowance_ft,
        metar_radius_nm,
        metar_max_age_s,
    })
}

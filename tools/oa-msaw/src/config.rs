//! The configuration file: plain text, one `key = value` per line, `#` starts a comment. Every
//! single-valued key is required (the core has no defaults and neither has the tool); an unknown
//! or repeated key, a value that does not parse or is not finite, and a missing key are errors
//! that name it. Units are the ones in the key: seconds, nautical miles, feet, metres, degrees.
//!
//! Seven keys may repeat:
//!
//! ```text
//! corridor = <thr_lat> <thr_lon> <out_bearing_deg> <length_m> <near_half_m> <far_half_m> <thr_elev_ft> <faf_alt_ft>
//! cifp_airport = <ICAO identifier>
//! cifp_area_margin_nm = <nautical miles>   (optional)
//! gtm_clearance = box <lat_min> <lat_max> <lon_min> <lon_max> <ft>
//! gtm_clearance = file <path of lat,lon lines> <ft>
//! msaw_request = <addr:HEX24 | call:CALLSIGN> <start_s> <end_s>
//! msaw_aural_area = <area-id> <lat>,<lon> <lat>,<lon> <lat>,<lon> ...
//! inhibit = <start_s> <end_s> <alt_lo_ft> <alt_hi_ft> <author> <reason> <lat>,<lon> <lat>,<lon> <lat>,<lon> ...
//! ```
//!
//! `gtm_clearance_ft` is the clearance above terrain outside the corridors, the general terrain monitor's;
//! a `gtm_clearance` line sets another clearance in an area (the highest of those that hold a point
//! wins). The chart altitude is no alert floor.
//!
//! A corridor line is a final-approach corridor of the core. A `cifp_airport` line asks for the
//! corridors the CIFP gives at that airport (needs `--cifp`). `cifp_area_margin_nm` asks for the approach
//! capture boxes and runway strips of every airport with an instrument approach in the area of the clip
//! widened by that margin; the `cifp_airport` lines give theirs, with departure corridors, instead. An inhibit
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

/// The area of a `gtm_clearance` line.
#[derive(Debug, Clone, PartialEq)]
pub enum GtmShape {
    /// South, north, west, east, 1e-7 degree.
    Box(i32, i32, i32, i32),
    /// A file of `lat,lon` lines, read by the run.
    File(String),
}

/// One `gtm_clearance` line.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct GtmArea {
    pub shape: GtmShape,
    pub ft: i32,
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

/// One explicitly adapted geographic area where an MSAW episode requests an aural alarm.
/// The physical output remains the caller's responsibility and is reported unavailable when no
/// backend is connected.
// A7.2: cold one-shot row; id is <=24 bytes and the validated polygon is fixed-size.
#[derive(Debug, Clone, PartialEq)]
pub struct AuralArea {
    pub id: String,
    pub polygon: oa_safety::geom::Polygon,
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
    /// With `--cifp`: corridors for every runway end with an approach at every airport within the
    /// area of the clip widened by this many nautical miles (`cifp_airport` lines are overrides).
    pub cifp_area_margin_nm: Option<f64>,
    /// The address whose rows `--rows-of` asks for (set by the run, not the configuration).
    pub trace_addr: Option<u32>,
    pub requests: Vec<RequestSpec>,
    pub aural_areas: Vec<AuralArea>,
    /// The beacon codes of VFR traffic: 1200 and the `vfr_beacon_code` lines.
    pub vfr_codes: Vec<u16>,
    pub gtm_areas: Vec<GtmArea>,
    /// Half-width of an approach corridor, nautical miles (legacy's lateral monitor distance).
    pub approach_half_width_nm: f64,
    /// Radius of an airport terminal area around the reference point, nautical miles.
    pub airport_area_nm: f64,
    /// The area applies below this height above the airport, feet.
    pub airport_area_ceiling_ft_agl: i32,
    /// The clearance above terrain and obstacles in an airport terminal area, feet.
    pub airport_area_clearance_ft: i32,
    /// A track below FL180 with no usable altimeter setting is given its pressure altitude lowered
    /// by this, feet (the conservative direction), and counted.
    pub no_qnh_allowance_ft: i32,
    /// The altimeter setting is taken from the nearest METAR station within this, nautical miles,
    /// whose observation is at most `metar_max_age_s` seconds old.
    pub metar_radius_nm: f64,
    pub metar_max_age_s: i64,
}

const KEYS: [&str; 42] = [
    "look_ahead_s",
    "warning_s",
    "samples",
    "hysteresis_s",
    "gtm_clearance_ft",
    "obstacle_clearance_ft",
    "approach_half_width_nm",
    "approach_allowance_ft",
    "airport_area_nm",
    "airport_area_ceiling_ft_agl",
    "airport_area_clearance_ft",
    "approach_min_clearance_ft",
    "departure_min_clearance_ft",
    "velocity_uncertainty_mps",
    "max_altitude_age_s",
    "stale_hold_s",
    "altitude_loss_degrade_share",
    "terminal_lookahead_s",
    "capture_heading_deg",
    "cifp_area_margin_nm",
    "no_qnh_allowance_ft",
    "metar_radius_nm",
    "metar_max_age_s",
    "max_tracks",
    "max_open",
    "track_budget",
    "max_speed_mps",
    "wide_degrade_share",
    "net_stale_limit_s",
    "net_future_horizon_s",
    "alert_log_capacity",
    "skip_log_capacity",
    "inhibit_max_duration_s",
    "inhibit_max_start_ahead_s",
    "inhibit_max_extent_deg",
    "corridor",
    "cifp_airport",
    "gtm_clearance",
    "msaw_request",
    "msaw_aural_area",
    "vfr_beacon_code",
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

fn positive(key: &str, v: &str, max: f64) -> Result<f64, String> {
    let x = finite(key, v)?;
    if x <= 0.0 || x > max {
        return Err(format!("{key}: `{v}` is out of range (0 to {max})"));
    }
    Ok(x)
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
        land_from_m: 0.0,
        pave_half_m: 0.0,
        far_elev_ft: 0,
        path_angle_deg: 3.0,
        tch_ft: 50,
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

fn gtm_area(v: &str) -> Result<GtmArea, String> {
    let f: Vec<&str> = v.split_whitespace().collect();
    let (shape, ft) = match f.as_slice() {
        ["box", a, b, c, d, ft] => {
            let (la0, la1) = (e7("gtm box lat", a, 90.0)?, e7("gtm box lat", b, 90.0)?);
            let (lo0, lo1) = (e7("gtm box lon", c, 180.0)?, e7("gtm box lon", d, 180.0)?);
            if la0 >= la1 || lo0 >= lo1 {
                return Err("gtm_clearance: a box needs south < north and west < east".to_string());
            }
            (GtmShape::Box(la0, la1, lo0, lo1), *ft)
        }
        ["file", path, ft] => (GtmShape::File((*path).to_string()), *ft),
        _ => {
            return Err(format!(
            "gtm_clearance: `{v}` needs `box lat_min lat_max lon_min lon_max ft` or `file path ft`"
        ))
        }
    };
    let ft = feet("gtm_clearance ft", ft)?;
    if !(0..=oa_msaw::MAX_CLEARANCE_FT).contains(&ft) {
        return Err("gtm_clearance: the clearance is out of range".to_string());
    }
    Ok(GtmArea { shape, ft })
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

fn aural_area(v: &str) -> Result<AuralArea, String> {
    let fields: Vec<&str> = v.split_whitespace().collect();
    let [id, points @ ..] = fields.as_slice() else {
        return Err("msaw_aural_area needs an identifier and at least three vertices".into());
    };
    if id.is_empty()
        || id.len() > 24
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        || points.len() < 3
        || points.len() > oa_safety::geom::MAX_VERTICES
    {
        return Err("msaw_aural_area identifier or vertex count is outside its bound".into());
    }
    let vertices = points
        .iter()
        .map(|point| {
            let (lat, lon) = point
                .split_once(',')
                .ok_or("msaw_aural_area vertex needs lat,lon")?;
            let lat = e7("msaw_aural_area latitude", lat, 90.0)?;
            let lon = e7("msaw_aural_area longitude", lon, 180.0)?;
            Ok((lat, lon))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let polygon =
        polygon_new(&vertices).map_err(|error| format!("msaw_aural_area polygon: {error}"))?;
    Ok(AuralArea {
        id: (*id).to_string(),
        polygon,
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
    let mut vfr_codes: Vec<u16> = vec![1200];
    let (mut procs, mut airports, mut inhibits, mut requests, mut aural_areas, mut areas) = (
        Vec::<&str>::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
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
            "msaw_aural_area" => aural_areas.push(v),
            "vfr_beacon_code" => {
                let code: u16 = v
                    .parse()
                    .ok()
                    .filter(|c| {
                        (0..=7777).contains(c) && v.bytes().all(|b| (b'0'..=b'7').contains(&b))
                    })
                    .ok_or_else(|| format!("vfr_beacon_code: `{v}` is not a beacon code"))?;
                if !vfr_codes.contains(&code) {
                    vfr_codes.push(code);
                }
            }
            "gtm_clearance" => areas.push(v),
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
    if airports.len() > MAX_LINES
        || inhibits.len() > MAX_LINES
        || aural_areas.len() > MAX_LINES
        || requests.len() > MAX_LINES
        || areas.len() > MAX_LINES
    {
        return Err(format!(
            "more than {MAX_LINES} repeated corridor, airport, area, request or inhibit lines"
        ));
    }
    let list = procs
        .iter()
        .map(|v| corridor(v))
        .collect::<Result<Vec<Corridor>, String>>()?;
    let msaw = MsawConfig {
        max_tracks: to_usize("max_tracks", get("max_tracks")?)?,
        max_open: to_usize("max_open", get("max_open")?)?,
        track_budget: to_usize("track_budget", get("track_budget")?)?,
        look_ahead_s: finite("look_ahead_s", get("look_ahead_s")?)?,
        warning_s: finite("warning_s", get("warning_s")?)?,
        samples: to_usize("samples", get("samples")?)?,
        hysteresis_ns: seconds_ns("hysteresis_s", get("hysteresis_s")?)?,
        terrain_clearance_ft: feet("gtm_clearance_ft", get("gtm_clearance_ft")?)?,
        obstacle_clearance_ft: feet("obstacle_clearance_ft", get("obstacle_clearance_ft")?)?,
        max_speed_mps: finite("max_speed_mps", get("max_speed_mps")?)?,
        wide_degrade_share: finite("wide_degrade_share", get("wide_degrade_share")?)?,
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
        max_altitude_age_s: finite("max_altitude_age_s", get("max_altitude_age_s")?)?,
        stale_hold_s: finite("stale_hold_s", get("stale_hold_s")?)?,
        altitude_loss_degrade_share: finite(
            "altitude_loss_degrade_share",
            get("altitude_loss_degrade_share")?,
        )?,
        terminal_lookahead_s: finite("terminal_lookahead_s", get("terminal_lookahead_s")?)?,
        capture_heading_deg: finite("capture_heading_deg", get("capture_heading_deg")?)?,
        corridors: list.into_boxed_slice(),
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
        max_alert_suppression_ns: None,
        allow_military_group_suppression: false,
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
    let mut gtm_areas = Vec::new();
    for v in &areas {
        gtm_areas.push(gtm_area(v)?);
    }
    let mut reqs = Vec::new();
    for v in &requests {
        reqs.push(request(v)?);
    }
    let mut parsed_aural_areas = Vec::with_capacity(aural_areas.len());
    for value in &aural_areas {
        let area = aural_area(value)?;
        if parsed_aural_areas
            .iter()
            .any(|prior: &AuralArea| prior.id == area.id)
        {
            return Err(format!("msaw_aural_area `{}` is repeated", area.id));
        }
        parsed_aural_areas.push(area);
    }
    let apt_ft = feet(
        "airport_area_clearance_ft",
        get("airport_area_clearance_ft")?,
    )?;
    if !(0..=oa_msaw::MAX_CLEARANCE_FT).contains(&apt_ft) {
        return Err("airport_area_clearance_ft: out of range".to_string());
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
        trace_addr: None,
        cifp_area_margin_nm: match kv.get("cifp_area_margin_nm") {
            Some(v) => Some(positive("cifp_area_margin_nm", v, 100.0)?),
            None => None,
        },
        requests: reqs,
        aural_areas: parsed_aural_areas,
        vfr_codes,
        gtm_areas,
        approach_half_width_nm: positive(
            "approach_half_width_nm",
            get("approach_half_width_nm")?,
            5.0,
        )?,
        airport_area_nm: positive("airport_area_nm", get("airport_area_nm")?, 30.0)?,
        airport_area_ceiling_ft_agl: feet(
            "airport_area_ceiling_ft_agl",
            get("airport_area_ceiling_ft_agl")?,
        )?,
        airport_area_clearance_ft: feet(
            "airport_area_clearance_ft",
            get("airport_area_clearance_ft")?,
        )?,
        no_qnh_allowance_ft,
        metar_radius_nm,
        metar_max_age_s,
    })
}

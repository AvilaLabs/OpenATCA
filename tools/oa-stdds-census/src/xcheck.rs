//! Position cross-check of STDDS updates against an ADS-B recording of the same day.
//!
//! The question it serves: is a TAIS update with `track/adsb` false free of ADS-B? An address
//! cannot answer it (the address field reads zero on almost all such updates), so the update's
//! position is compared with every ADS-B aircraft, interpolated to the update's time.
//!
//! The reference. The ADS-B index is built with the same gating the tracker and the truth use,
//! `oa_truth::quality_check` with `DEFAULT_CONFIG` (source type adsb_icao, adsb_icao_nt or
//! adsb_other only, NACp, NIC, SIL, version, stale and range gates), in time order with the
//! per-aircraft carried state, not a parallel filter. This matters: a TIS-B report is a
//! rebroadcast of the FAA radar picture, so radar-only updates would match it at about zero
//! distance. Reports on the ground are dropped after the gates. [`ClipStats`] counts every
//! report by source type before any gating and what each gate removed.
//!
//! Time base: the STDDS time compared is `track/mrtTime` (the track's time stamp, UTC ms),
//! against the ADS-B report time of the clip (UTC, 10 ms resolution). Each ADS-B aircraft is
//! linearly interpolated between the two reports that bracket that time, if they are at most
//! [`Params::max_gap_ms`] apart; an aircraft with no such bracket is not a candidate. A segment
//! whose implied speed is above [`MAX_SEGMENT_MPS`] is left out and counted.
//! Altitude: the STDDS `reportedAltitude` (ASSUMED feet, pressure altitude) against the ADS-B
//! barometric altitude, interpolated the same way. If either side has none the altitude gate is
//! not applied to that pair (counted by the census as not compared).
//!
//! Exactness of the search at any latitude and across the antimeridian. Candidates are found
//! through a grid of 10 s time bins and 0.2 degree cells. Each segment (a pair of consecutive
//! reports of one aircraft) is stored in every cell its bounding box touches after the box is
//! widened by the largest gate, in latitude by the gate over the smallest metres per degree and
//! in longitude by the gate over the metres per degree at the box's highest latitude; a query
//! reads only the one cell of the point. Longitude cells wrap at 180 degrees, and interpolation
//! and the east difference take the short way round. Reports and updates above 85 degrees of
//! latitude are not supported: they are counted, not checked.

use crate::xcres::PLACEBO_S;
use oa_adsb::{reports_row, Reports, F_GROUND, NONE_I32, NONE_U16, NONE_U8, SOURCE_NAMES};
use oa_truth::{quality_check, QualityState, DEFAULT_CONFIG, REMOVE_KINDS};
use std::collections::{BTreeMap, HashMap};

/// Metres per nautical mile.
pub const NM_M: f64 = 1852.0;
/// Gates tried at once: horizontal, nautical miles. Index [`Params::primary`] is the one the
/// distributions are taken from; the others only change the counts.
pub const DEFAULT_GATES_NM: [f64; 3] = [0.5, 1.0, 2.0];
/// Vertical gate, feet (not varied in the sweep).
pub const DEFAULT_V_GATE_FT: f64 = 500.0;
/// Longest gap between two reports of one aircraft that is still interpolated, ms. A wider
/// bracket interpolates a manoeuvre away; the census prints how wide the matched brackets were.
pub const DEFAULT_MAX_GAP_MS: i64 = 5_000;
/// A segment faster than this is left out, metres per second (above any aircraft).
pub const MAX_SEGMENT_MPS: f64 = 600.0;
/// Coverage radius: an update counts as covered if any indexed aircraft is this near, metres.
pub const COVERAGE_M: f64 = 30.0 * NM_M;
/// Highest absolute latitude supported, degrees.
pub const MAX_LAT_DEG: f64 = 85.0;

const TBIN_MS: i64 = 10_000;
const CELL_DEG: f64 = 0.2;
const LON_CELLS: i64 = 1800;
const MAX_GATE_NM: f64 = 10.0;
const MAX_GAP_LIMIT_MS: i64 = 30_000;
/// Smallest metres per degree of latitude (at the equator), for widening boxes.
const MIN_M_PER_DEG_LAT: f64 = 110_500.0;
/// Metres per degree of longitude at the equator, a lower bound times the cosine of latitude.
const M_PER_DEG_LON_EQ: f64 = 111_319.0;
/// Ground speed from which an along-track difference is turned into seconds, m/s (about 100 kt).
pub const MIN_GS_FOR_TIME_MPS: f64 = 51.4;
const E7: f64 = 1e7;
const LON_FULL_E7: i64 = 3_600_000_000;

/// The gates and limits of a cross-check.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub gates_nm: [f64; 3],
    /// Index into `gates_nm` of the gate whose exactly-one matches feed the distributions.
    pub primary: usize,
    pub v_gate_ft: f64,
    pub max_gap_ms: i64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            gates_nm: DEFAULT_GATES_NM,
            primary: 1,
            v_gate_ft: DEFAULT_V_GATE_FT,
            max_gap_ms: DEFAULT_MAX_GAP_MS,
        }
    }
}

impl Params {
    /// The gates 0.5, `nm` and 2 NM in ascending order, with `nm` as the primary one.
    /// If `nm` is already one of them the list is the default one (no gate twice).
    pub fn with_primary_gate_nm(mut self, nm: f64) -> Params {
        let mut g = [0.5, nm, 2.0];
        if nm == 0.5 || nm == 2.0 || nm == 1.0 {
            g = [0.5, 1.0, 2.0];
        }
        g.sort_by(|a, b| a.total_cmp(b));
        self.primary = g.iter().position(|&x| x == nm).unwrap_or(1);
        self.gates_nm = g;
        self
    }

    /// The largest horizontal gate, metres.
    pub fn max_gate_m(&self) -> f64 {
        self.gates_nm.iter().fold(0.0, |a: f64, &g| a.max(g)) * NM_M
    }

    fn check(&self) -> Result<(), String> {
        if self.primary >= self.gates_nm.len() {
            return Err("primary gate index out of range".to_string());
        }
        if self
            .gates_nm
            .iter()
            .any(|&g| !(g > 0.0 && g <= MAX_GATE_NM))
        {
            return Err(format!(
                "every gate must be above 0 and at most {MAX_GATE_NM} NM"
            ));
        }
        if !(self.v_gate_ft > 0.0 && self.v_gate_ft.is_finite()) {
            return Err("the vertical gate must be above 0 ft".to_string());
        }
        if !(1..=MAX_GAP_LIMIT_MS).contains(&self.max_gap_ms) {
            return Err("the interpolation gap must be 1 ms to 30 s".to_string());
        }
        Ok(())
    }
}

/// A time-of-day window, seconds of the UTC day, `[from, to)`; applied to both sides.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub from_s: i64,
    pub to_s: i64,
}

impl Window {
    pub fn new(from_s: i64, to_s: i64) -> Result<Window, String> {
        if !(0 <= from_s && from_s < to_s && to_s <= 86_400) {
            return Err(
                "the window must satisfy 0 <= from < to <= 86400 (seconds of the UTC day)"
                    .to_string(),
            );
        }
        Ok(Window { from_s, to_s })
    }

    /// Whether the time of day of `t_ms` is in the window widened by `pad_ms` each side. The
    /// widened window may cross midnight (a window at the start or end of the day).
    pub fn contains_padded(&self, t_ms: i64, pad_ms: i64) -> bool {
        const DAY_MS: i64 = 86_400_000;
        let len = (self.to_s - self.from_s) * 1000 + 2 * pad_ms;
        len >= DAY_MS || (t_ms - (self.from_s * 1000 - pad_ms)).rem_euclid(DAY_MS) < len
    }

    pub fn contains(&self, t_ms: i64) -> bool {
        self.contains_padded(t_ms, 0)
    }
}

/// How far the ADS-B side of a windowed run is widened: the interpolation bracket, and the
/// placebo shift (a placebo lookup at the window's edge needs ADS-B up to [`PLACEBO_S`] away).
pub fn adsb_pad_ms(p: &Params) -> i64 {
    p.max_gap_ms + PLACEBO_S * 1000
}

/// One ADS-B report, as kept for interpolation.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pt {
    pub icao: u32,
    pub t_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Barometric altitude, feet; [`NONE_I32`] if absent.
    pub alt_ft: i32,
    /// Track, 0.1 degree; [`NONE_U16`] if absent.
    pub track_ddeg: u16,
    /// Ground speed, 0.1 kt; [`NONE_U16`] if absent.
    pub gs_dkt: u16,
}

/// What the gating did to the clip: counts by source type before any gate, and what each gate
/// removed.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipStats {
    /// Reports in the clip by source type ([`SOURCE_NAMES`] order, then "no source column").
    pub by_source: [u64; 15],
    pub reports: u64,
    /// Reports outside the time window (left out before the gates).
    pub outside_window: u64,
    /// Removed by each `oa_truth` rule ([`oa_truth::REMOVE_NAMES`]); the first eight apply here.
    pub removed: [u64; REMOVE_KINDS],
    /// Passed the gates but on the ground.
    pub ground: u64,
    /// Kept.
    pub kept: u64,
    /// Earliest and latest report time in the clip, UTC ms (all reports, before any filter).
    pub span_ms: Option<(i64, i64)>,
}

/// Names of [`ClipStats::by_source`].
pub fn source_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = SOURCE_NAMES.to_vec();
    v.push("no source column");
    v
}

/// One aircraft's interpolated state compared with an STDDS update.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cand {
    pub icao: u32,
    /// Horizontal distance, metres.
    pub dh_m: f64,
    /// STDDS minus ADS-B, feet; `None` if either altitude is missing.
    pub dv_ft: Option<f64>,
    /// STDDS minus ADS-B along the ADS-B track (positive: the STDDS position is ahead), metres.
    pub along_m: Option<f64>,
    /// Across the ADS-B track (positive: to its right), metres.
    pub cross_m: Option<f64>,
    /// ADS-B ground speed, metres per second.
    pub gs_mps: Option<f64>,
    /// Time between the two ADS-B reports that bracket the update, ms.
    pub gap_ms: i64,
}

impl Cand {
    /// Whether the candidate is inside the horizontal gate `h_m` and, when both altitudes exist,
    /// the vertical gate `v_ft` (both limits inclusive).
    pub fn passes(&self, h_m: f64, v_ft: f64) -> bool {
        self.dh_m <= h_m && self.dv_ft.is_none_or(|v| v.abs() <= v_ft)
    }
}

/// The ADS-B index.
pub struct Index {
    pts: Vec<Pt>,
    cells: HashMap<(i64, i32, i64), Vec<u32>>,
    pub params: Params,
    pub clip: ClipStats,
    /// Distinct aircraft and interpolable segments indexed.
    pub aircraft: u64,
    pub segments: u64,
    /// Segments left out: implied speed above [`MAX_SEGMENT_MPS`], or above [`MAX_LAT_DEG`].
    pub segments_fast: u64,
    pub segments_polar: u64,
    /// Neighbouring reports of one aircraft further apart than [`Params::max_gap_ms`] (no segment).
    pub segments_long: u64,
    bytes: u64,
}

/// Longitude difference `b - a` in 1e-7 degree, the short way round.
fn wrap_e7(d: i64) -> i64 {
    (d + LON_FULL_E7 / 2).rem_euclid(LON_FULL_E7) - LON_FULL_E7 / 2
}

fn lat_cell(lat_deg: f64) -> i32 {
    (lat_deg / CELL_DEG).floor() as i32
}

/// Longitude cell of an unwrapped longitude in degrees, wrapped into `0..LON_CELLS`.
fn lon_cell(lon_deg: f64) -> i64 {
    (((lon_deg + 180.0) / CELL_DEG).floor() as i64).rem_euclid(LON_CELLS)
}

/// Metres per radian of latitude and of longitude at latitude `lat_rad` (WGS-84).
fn radii(lat_rad: f64) -> (f64, f64) {
    const A: f64 = 6_378_137.0;
    const E2: f64 = 0.006_694_379_990_14;
    let s = lat_rad.sin();
    let w = (1.0 - E2 * s * s).sqrt();
    let n = A / w;
    let m = A * (1.0 - E2) / (w * w * w);
    (m, n * lat_rad.cos())
}

/// The candidate for the segment `a`-`b` at the state `s`, `de` and `dn` metres from the update.
fn make_cand(a: &Pt, b: &Pt, s: &SegAt, de: f64, dn: f64, alt_ft: Option<i32>) -> Cand {
    let lerp = |x: i32, y: i32| f64::from(x) + (f64::from(y) - f64::from(x)) * s.f;
    let alt_a = (a.alt_ft != NONE_I32 && b.alt_ft != NONE_I32).then(|| lerp(a.alt_ft, b.alt_ft));
    let dv_ft = alt_ft.zip(alt_a).map(|(v, x)| f64::from(v) - x);
    let track = [a.track_ddeg, b.track_ddeg]
        .into_iter()
        .find(|&t| t != NONE_U16)
        .map(|t| f64::from(t) / 10.0);
    let (along_m, cross_m) = match track {
        Some(deg) => {
            let (sn, cs) = deg.to_radians().sin_cos();
            (Some(de * sn + dn * cs), Some(de * cs - dn * sn))
        }
        None => (None, None),
    };
    let gs_mps = [a.gs_dkt, b.gs_dkt]
        .into_iter()
        .find(|&g| g != NONE_U16)
        .map(|g| f64::from(g) / 10.0 * NM_M / 3600.0);
    Cand {
        icao: a.icao,
        dh_m: de.hypot(dn),
        dv_ft,
        along_m,
        cross_m,
        gs_mps,
        gap_ms: b.t_ms - a.t_ms,
    }
}

/// The interpolated state of a segment at `t_ms`.
struct SegAt {
    lat_deg: f64,
    lon_e7: f64,
    f: f64,
}

fn seg_at(a: &Pt, b: &Pt, t_ms: i64) -> SegAt {
    let f = (t_ms - a.t_ms) as f64 / (b.t_ms - a.t_ms) as f64;
    let lat_e7 = f64::from(a.lat_e7) + (f64::from(b.lat_e7) - f64::from(a.lat_e7)) * f;
    let dlon = wrap_e7(i64::from(b.lon_e7) - i64::from(a.lon_e7)) as f64;
    SegAt {
        lat_deg: lat_e7 / E7,
        lon_e7: f64::from(a.lon_e7) + dlon * f,
        f,
    }
}

/// East and north differences, metres, of the point (`lat_e7`, `lon_e7`) minus the segment state,
/// the east difference the short way round.
fn diff_m(p_lat_e7: i32, p_lon_e7: i32, s: &SegAt) -> (f64, f64) {
    let (m_per_rad, n_cos) = radii(s.lat_deg.to_radians());
    let dlon_e7 = wrap_e7((f64::from(p_lon_e7) - s.lon_e7).round() as i64) as f64;
    let de = (dlon_e7 / E7).to_radians() * n_cos;
    let dn = (f64::from(p_lat_e7) / E7 - s.lat_deg).to_radians() * m_per_rad;
    (de, dn)
}

impl Index {
    /// Reports kept.
    pub fn points(&self) -> usize {
        self.pts.len()
    }

    /// Whether a point at this latitude can be searched.
    pub fn supports(&self, lat_e7: i32) -> bool {
        f64::from(lat_e7).abs() / E7 <= MAX_LAT_DEG
    }

    /// An estimate of the bytes the index holds.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Every aircraft that is interpolable at `t_ms` and within the largest gate of the point,
    /// one candidate per aircraft; altitude is compared but not gated here. `alt_ft` is the
    /// STDDS altitude, if it has one. The point must be [`supports`](Self::supports)ed.
    pub fn candidates(
        &self,
        t_ms: i64,
        lat_e7: i32,
        lon_e7: i32,
        alt_ft: Option<i32>,
    ) -> Vec<Cand> {
        let mut out: Vec<Cand> = Vec::new();
        let key = (
            t_ms.div_euclid(TBIN_MS),
            lat_cell(f64::from(lat_e7) / E7),
            lon_cell(f64::from(lon_e7) / E7),
        );
        let Some(list) = self.cells.get(&key) else {
            return out;
        };
        let max_h = self.params.max_gate_m();
        for &ix in list {
            let (Some(a), Some(b)) = (self.pts.get(ix as usize), self.pts.get(ix as usize + 1))
            else {
                continue;
            };
            if t_ms < a.t_ms || t_ms > b.t_ms || out.iter().any(|c| c.icao == a.icao) {
                continue;
            }
            let s = seg_at(a, b, t_ms);
            let (de, dn) = diff_m(lat_e7, lon_e7, &s);
            let dh = de.hypot(dn);
            if dh > max_h {
                continue;
            }
            out.push(make_cand(a, b, &s, de, dn, alt_ft));
        }
        out
    }

    /// The aircraft with address `icao`, interpolated to `t_ms`, compared with the point: no
    /// horizontal or vertical gate (the address, not the position, says which aircraft it is).
    /// `None` if the aircraft has no usable bracket at that time. The point must be
    /// [`supports`](Self::supports)ed.
    pub fn for_icao(
        &self,
        t_ms: i64,
        lat_e7: i32,
        lon_e7: i32,
        alt_ft: Option<i32>,
        icao: u32,
    ) -> Option<Cand> {
        let i = self
            .pts
            .partition_point(|p| (p.icao, p.t_ms) <= (icao, t_ms));
        let a = self.pts.get(i.checked_sub(1)?)?;
        let b = self.pts.get(i)?;
        if a.icao != icao || b.icao != icao || b.t_ms - a.t_ms > self.params.max_gap_ms {
            return None;
        }
        if f64::from(a.lat_e7.abs().max(b.lat_e7.abs())) / E7 > MAX_LAT_DEG {
            return None;
        }
        let from_a = SegAt {
            lat_deg: f64::from(a.lat_e7) / E7,
            lon_e7: f64::from(a.lon_e7),
            f: 0.0,
        };
        let (se, sn) = diff_m(b.lat_e7, b.lon_e7, &from_a);
        if se.hypot(sn) / ((b.t_ms - a.t_ms) as f64 / 1000.0) > MAX_SEGMENT_MPS {
            return None;
        }
        let s = seg_at(a, b, t_ms);
        let (de, dn) = diff_m(lat_e7, lon_e7, &s);
        Some(make_cand(a, b, &s, de, dn, alt_ft))
    }

    /// Whether any indexed aircraft, at any altitude, is within `radius_m` of the point at `t_ms`
    /// (coverage). Reads the cells around the point; a segment is stored in every cell of its
    /// own box, so the window of cells covering the radius finds any aircraft inside it.
    pub fn covered(&self, t_ms: i64, lat_e7: i32, lon_e7: i32, radius_m: f64) -> bool {
        let lat = f64::from(lat_e7) / E7;
        let lon = f64::from(lon_e7) / E7;
        let lat_x = radius_m / MIN_M_PER_DEG_LAT * 1.02;
        let lon_x =
            radius_m / (M_PER_DEG_LON_EQ * (lat.abs() + lat_x).min(89.9).to_radians().cos()) * 1.02;
        let tb = t_ms.div_euclid(TBIN_MS);
        let (c0, c1) = (lat_cell(lat - lat_x), lat_cell(lat + lat_x));
        let l0 = ((lon - lon_x + 180.0) / CELL_DEG).floor() as i64;
        let l1 = ((lon + lon_x + 180.0) / CELL_DEG).floor() as i64;
        let l1 = l1.min(l0 + LON_CELLS - 1);
        for la in c0..=c1 {
            for lo in l0..=l1 {
                let Some(list) = self.cells.get(&(tb, la, lo.rem_euclid(LON_CELLS))) else {
                    continue;
                };
                for &ix in list {
                    let (Some(a), Some(b)) =
                        (self.pts.get(ix as usize), self.pts.get(ix as usize + 1))
                    else {
                        continue;
                    };
                    if t_ms < a.t_ms || t_ms > b.t_ms {
                        continue;
                    }
                    let (de, dn) = diff_m(lat_e7, lon_e7, &seg_at(a, b, t_ms));
                    if de.hypot(dn) <= radius_m {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Builds the index from the clip's reports: the truth gates in time order, then the
/// ground and window filters.
pub fn build(r: &Reports, params: Params, window: Option<Window>) -> Result<Index, String> {
    params.check()?;
    let n = r.icao.len();
    let mut order: Vec<(i64, u32)> = Vec::with_capacity(n);
    for (i, &t) in r.time_ns.iter().enumerate() {
        order.push((
            t,
            u32::try_from(i).map_err(|_| "too many ADS-B reports".to_string())?,
        ));
    }
    order.sort_unstable();
    let mut stats = ClipStats::default();
    let mut quality: BTreeMap<u32, QualityState> = BTreeMap::new();
    let mut pts = Vec::new();
    for &(t_ns, i) in &order {
        let Some(row) = reports_row(r, i as usize) else {
            continue;
        };
        stats.reports += 1;
        let src = if row.source == NONE_U8 {
            14
        } else {
            usize::from(row.source).min(13)
        };
        if let Some(c) = stats.by_source.get_mut(src) {
            *c += 1;
        }
        let t_ms = t_ns.div_euclid(1_000_000);
        stats.span_ms = Some(
            stats
                .span_ms
                .map_or((t_ms, t_ms), |(a, b)| (a.min(t_ms), b.max(t_ms))),
        );
        if window.is_some_and(|w| !w.contains_padded(t_ms, adsb_pad_ms(&params))) {
            stats.outside_window += 1;
            continue;
        }
        let q = quality.entry(row.icao).or_default();
        if let Some(why) = quality_check(q, &row, t_ns, &DEFAULT_CONFIG).removal {
            if let Some(c) = stats.removed.get_mut(why) {
                *c += 1;
            }
            continue;
        }
        if row.flags & F_GROUND != 0 {
            stats.ground += 1;
            continue;
        }
        stats.kept += 1;
        pts.push(Pt {
            icao: row.icao,
            t_ms,
            lat_e7: row.lat_e7,
            lon_e7: row.lon_e7,
            alt_ft: row.baro_alt_ft,
            track_ddeg: row.track_ddeg,
            gs_dkt: row.gs_dkt,
        });
    }
    let mut idx = build_from_points(pts, params)?;
    idx.clip = stats;
    Ok(idx)
}

/// Builds the index from points (the tests' entry); no gating.
pub fn build_from_points(mut pts: Vec<Pt>, params: Params) -> Result<Index, String> {
    params.check()?;
    pts.sort_by_key(|p| (p.icao, p.t_ms));
    let mut cells: HashMap<(i64, i32, i64), Vec<u32>> = HashMap::new();
    let mut aircraft = 0u64;
    let mut last_icao: Option<u32> = None;
    for p in &pts {
        if last_icao != Some(p.icao) {
            last_icao = Some(p.icao);
            aircraft += 1;
        }
    }
    let (mut segments, mut segments_fast, mut segments_polar, mut segments_long) =
        (0u64, 0u64, 0u64, 0u64);
    let gate = params.max_gate_m();
    let lat_x = gate / MIN_M_PER_DEG_LAT * 1.02;
    for (i, w) in pts.windows(2).enumerate() {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
            continue;
        };
        if a.icao != b.icao || b.t_ms <= a.t_ms {
            continue;
        }
        if b.t_ms - a.t_ms > params.max_gap_ms {
            segments_long += 1;
            continue;
        }
        let (la, lb) = (f64::from(a.lat_e7) / E7, f64::from(b.lat_e7) / E7);
        if la.abs().max(lb.abs()) > MAX_LAT_DEG {
            segments_polar += 1;
            continue;
        }
        let from_a = SegAt {
            lat_deg: la,
            lon_e7: f64::from(a.lon_e7),
            f: 0.0,
        };
        let (de, dn) = diff_m(b.lat_e7, b.lon_e7, &from_a);
        let speed = de.hypot(dn) / ((b.t_ms - a.t_ms) as f64 / 1000.0);
        if speed > MAX_SEGMENT_MPS {
            segments_fast += 1;
            continue;
        }
        let Ok(ix) = u32::try_from(i) else {
            return Err("too many ADS-B reports".to_string());
        };
        let lat_hi = la.max(lb) + lat_x;
        let lat_lo = la.min(lb) - lat_x;
        let lon_a = f64::from(a.lon_e7) / E7;
        let lon_b = lon_a + wrap_e7(i64::from(b.lon_e7) - i64::from(a.lon_e7)) as f64 / E7;
        let cos = (la.abs().max(lb.abs()) + lat_x)
            .min(89.9)
            .to_radians()
            .cos();
        let lon_x = gate / (M_PER_DEG_LON_EQ * cos) * 1.02;
        let l0 = ((lon_a.min(lon_b) - lon_x + 180.0) / CELL_DEG).floor() as i64;
        let l1 = ((lon_a.max(lon_b) + lon_x + 180.0) / CELL_DEG).floor() as i64;
        let l1 = l1.min(l0 + LON_CELLS - 1);
        for tb in a.t_ms.div_euclid(TBIN_MS)..=b.t_ms.div_euclid(TBIN_MS) {
            for lc in lat_cell(lat_lo)..=lat_cell(lat_hi) {
                for lo in l0..=l1 {
                    cells
                        .entry((tb, lc, lo.rem_euclid(LON_CELLS)))
                        .or_default()
                        .push(ix);
                }
            }
        }
        segments += 1;
    }
    let entries: usize = cells.values().map(Vec::len).sum();
    let bytes = (pts.len() * std::mem::size_of::<Pt>()
        + cells.len() * 72
        + entries * std::mem::size_of::<u32>()) as u64;
    let span_ms = pts
        .iter()
        .map(|p| p.t_ms)
        .fold(None, |a: Option<(i64, i64)>, t| {
            Some(a.map_or((t, t), |(lo, hi)| (lo.min(t), hi.max(t))))
        });
    Ok(Index {
        pts,
        cells,
        params,
        clip: ClipStats {
            span_ms,
            ..ClipStats::default()
        },
        aircraft,
        segments,
        segments_fast,
        segments_polar,
        segments_long,
        bytes,
    })
}

/// Bytes [`build`] allocates for the sort order of each report of the clip (time and index),
/// whether or not the window keeps it.
pub const ORDER_BYTES_PER_REPORT: u64 = 16;
/// ASSUMED cell entries (`u32` each) and new grid cells (72 bytes each) per kept report: a
/// segment is stored in every cell its box touches, which for a gate of a few NM and a few
/// seconds is a handful. A clip of faster or sparser aircraft, or a larger gate, can exceed it.
pub const ASSUMED_ENTRIES_PER_REPORT: u64 = 8;
pub const ASSUMED_CELLS_PER_REPORT: u64 = 2;
/// Growth slack on a `Vec` or `HashMap` that is filled by pushes.
pub const GROWTH_SLACK: u64 = 2;

/// An estimate of the bytes [`build`] allocates for the reports of `r`, made from the record
/// counts before anything is allocated: the sort order for every report, and for each report
/// the window keeps (the padded window, if any) its point and its assumed share of the cell
/// entries and cell map, each with [`GROWTH_SLACK`]. It is an estimate under the stated
/// assumptions, not a bound. The clip's own columns, already loaded, are not counted.
pub fn estimate_build_bytes(r: &Reports, params: &Params, window: Option<Window>) -> u64 {
    let all = r.time_ns.len() as u64;
    let kept = r
        .time_ns
        .iter()
        .filter(|&&t| {
            window.is_none_or(|w| w.contains_padded(t.div_euclid(1_000_000), adsb_pad_ms(params)))
        })
        .count() as u64;
    let per_kept = GROWTH_SLACK
        * (std::mem::size_of::<Pt>() as u64
            + ASSUMED_ENTRIES_PER_REPORT * std::mem::size_of::<u32>() as u64
            + ASSUMED_CELLS_PER_REPORT * 72);
    all.saturating_mul(ORDER_BYTES_PER_REPORT)
        .saturating_add(kept.saturating_mul(per_kept))
}

/// The outcome of one gate: no candidate, exactly one, more than one.
pub const OUTCOME_NAMES: [&str; 3] = ["no candidate", "exactly one", "ambiguous"];

/// Index into [`OUTCOME_NAMES`] for `n` candidates inside a gate.
pub fn outcome(n: usize) -> usize {
    n.min(2)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
pub(crate) mod tests {
    use super::*;

    /// Metres per degree of latitude (WGS-84 meridian arc).
    pub fn m_per_deg_lat(lat_deg: f64) -> f64 {
        radii(lat_deg.to_radians()).0 * std::f64::consts::PI / 180.0
    }

    /// A report at (lat, lon) degrees.
    pub fn pt(icao: u32, t_ms: i64, lat: f64, lon: f64, alt: i32) -> Pt {
        Pt {
            icao,
            t_ms,
            lat_e7: (lat * 1e7).round() as i32,
            lon_e7: (lon * 1e7).round() as i32,
            alt_ft: alt,
            track_ddeg: 900, // due east
            gs_dkt: 4000,    // 400 kt
        }
    }

    /// A report with its own track (0.1 degree) and ground speed (0.1 kt); the longitude is
    /// wrapped into -180..180.
    pub fn pt_full(icao: u32, t_ms: i64, lat: f64, lon: f64, alt: i32, trk: u16, gs: u16) -> Pt {
        let lon = if lon >= 180.0 { lon - 360.0 } else { lon };
        Pt {
            track_ddeg: trk,
            gs_dkt: gs,
            ..pt(icao, t_ms, lat, lon, alt)
        }
    }

    pub fn deg_lon_per_m(lat_deg: f64) -> f64 {
        1.0 / (radii(lat_deg.to_radians()).1 * std::f64::consts::PI / 180.0)
    }

    pub fn deg_lat_per_m(lat_deg: f64) -> f64 {
        1.0 / m_per_deg_lat(lat_deg)
    }

    /// The default parameters with a 10 s bracket (the tests space reports 10 s apart).
    fn p10() -> Params {
        Params {
            max_gap_ms: 10_000,
            ..Params::default()
        }
    }

    fn idx(pts: Vec<Pt>) -> Index {
        build_from_points(pts, p10()).unwrap()
    }

    // REQ: OA-CEN-009
    #[test]
    fn interpolates_to_the_update_time_and_gives_the_offset() {
        // One aircraft flying east at 400 kt, reports 0 s and 10 s: at 4 s it is 0.4 of the way.
        let v = 400.0 * NM_M / 3600.0;
        let dlon = deg_lon_per_m(40.0);
        let x = idx(vec![
            pt(1, 0, 40.0, -75.0, 10_000),
            pt(1, 10_000, 40.0, -75.0 + 10.0 * v * dlon, 12_000),
        ]);
        // The update sits 300 m north and 100 m east of the interpolated point, 200 ft higher.
        let here_lon = -75.0 + (4.0 * v + 100.0) * dlon;
        let here_lat = 40.0 + 300.0 * deg_lat_per_m(40.0);
        let c = x.candidates(
            4_000,
            (here_lat * 1e7) as i32,
            (here_lon * 1e7) as i32,
            Some(10_999),
        );
        assert_eq!(c.len(), 1);
        let c = c[0];
        assert!((c.dh_m - 316.2).abs() < 1.0, "{}", c.dh_m);
        assert!((c.dv_ft.unwrap() - 199.0).abs() < 0.01);
        // Track is east: along is the east offset, cross to the right is minus the north offset.
        assert!((c.along_m.unwrap() - 100.0).abs() < 1.0);
        assert!((c.cross_m.unwrap() + 300.0).abs() < 1.0);
        assert!((c.gs_mps.unwrap() - v).abs() < 0.01);
    }

    // REQ: OA-CEN-009
    #[test]
    fn no_bracket_means_no_candidate() {
        let x = idx(vec![
            pt(1, 0, 40.0, -75.0, 10_000),
            pt(1, 11_000, 40.0, -75.0, 10_000), // gap over 10 s: no segment
            pt(2, 50_000, 40.0, -75.0, 10_000),
        ]);
        let at = |t| {
            x.candidates(t, 400_000_000, -750_000_000, Some(10_000))
                .len()
        };
        assert_eq!(at(5_000), 0);
        assert_eq!(at(50_000), 0);
        assert_eq!(x.segments, 0);
        assert_eq!(x.aircraft, 2);
    }

    // REQ: OA-CEN-009
    #[test]
    fn candidates_are_one_per_aircraft_at_a_report_time() {
        // The update time equals a report time: two segments touch it, one candidate.
        let x = idx(vec![
            pt(1, 0, 40.0, -75.0, 5000),
            pt(1, 5000, 40.0, -75.0, 5000),
            pt(1, 10_000, 40.0, -75.0, 5000),
        ]);
        assert_eq!(x.candidates(5000, 400_000_000, -750_000_000, None).len(), 1);
    }

    // REQ: OA-CEN-009
    #[test]
    fn gate_edges_are_inclusive_and_altitude_is_gated_only_when_both_exist() {
        let c = |dh_m, dv_ft| Cand {
            icao: 1,
            dh_m,
            dv_ft,
            along_m: None,
            cross_m: None,
            gs_mps: None,
            gap_ms: 0,
        };
        let h = NM_M;
        assert!(c(h, Some(500.0)).passes(h, 500.0));
        assert!(!c(h + 0.001, Some(0.0)).passes(h, 500.0));
        assert!(!c(0.0, Some(-500.001)).passes(h, 500.0));
        assert!(c(0.0, None).passes(h, 500.0));
    }

    // REQ: OA-CEN-009
    #[test]
    fn grid_finds_a_candidate_across_a_cell_and_time_bin_border() {
        // Aircraft just west of a 0.2 degree cell border, update just east of it, and the
        // update time just after a 10 s bin edge.
        let lon_border = -75.0; // -75 / 0.2 is a whole number of cells
        let d = 400.0 * deg_lon_per_m(40.0);
        let x = idx(vec![
            pt(1, 9_000, 40.0, lon_border - d, 8000),
            pt(1, 11_000, 40.0, lon_border - d, 8000),
        ]);
        let c = x.candidates(
            10_500,
            400_000_000,
            ((lon_border + d) * 1e7) as i32,
            Some(8000),
        );
        assert_eq!(c.len(), 1);
        assert!((c[0].dh_m - 800.0).abs() < 2.0);
    }

    // REQ: OA-CEN-009
    #[test]
    fn parameters_are_checked() {
        let bad = |p: Params| build_from_points(vec![], p).is_err();
        assert!(!bad(Params::default()));
        assert!(bad(Params {
            gates_nm: [0.5, 1.0, 11.0],
            ..Params::default()
        }));
        assert!(bad(Params {
            max_gap_ms: 40_000,
            ..Params::default()
        }));
        assert!(bad(Params {
            primary: 3,
            ..Params::default()
        }));
        assert!(bad(Params {
            v_gate_ft: 0.0,
            ..Params::default()
        }));
        assert_eq!(outcome(0), 0);
        assert_eq!(outcome(1), 1);
        assert_eq!(outcome(5), 2);
    }

    use oa_adsb::{reports_push, Row, ROW_EMPTY};
    use oa_truth::{REM_NACP, REM_SOURCE, REM_STALE};

    fn row(icao: u32, t_s: i64, lat: f64, lon: f64, source: u8) -> Row {
        Row {
            icao,
            time_ns: t_s * 1_000_000_000,
            lat_e7: (lat * 1e7).round() as i32,
            lon_e7: (lon * 1e7).round() as i32,
            baro_alt_ft: 10_000,
            gs_dkt: 4000,
            track_ddeg: 900,
            source,
            nac_p: 10,
            nic: 8,
            sil: 3,
            version: 2,
            ..ROW_EMPTY
        }
    }

    fn clip(rows: &[Row]) -> Reports {
        let mut r = Reports::default();
        for row in rows {
            reports_push(&mut r, row);
        }
        r
    }

    // REQ: OA-CEN-013
    #[test]
    fn a_tisb_report_on_a_radar_only_update_is_not_a_candidate() {
        // Aircraft 1 (adsb_icao) is 50 km north of the update. Aircraft 2 is a TIS-B report right
        // on the update; aircraft 3 is stale, 4 on the ground, 5 has NACp 3: none is kept.
        let (lat, lon) = (40.0, -75.0);
        let far = lat + 50_000.0 * deg_lat_per_m(40.0);
        let mut rows = Vec::new();
        for s in 0..=20 {
            rows.push(row(1, s, far, lon, 0));
            rows.push(row(2, s, lat, lon, 3));
            let mut st = row(3, s, lat, lon, 0);
            st.flags |= oa_adsb::F_STALE;
            rows.push(st);
            let mut gr = row(4, s, lat, lon, 0);
            gr.flags |= oa_adsb::F_GROUND;
            rows.push(gr);
            let mut lo = row(5, s, lat, lon, 0);
            lo.nac_p = 3;
            rows.push(lo);
        }
        let x = build(&clip(&rows), Params::default(), None).unwrap();
        let c = &x.clip;
        assert_eq!(c.reports, 105);
        assert_eq!(c.by_source[0], 84);
        assert_eq!(c.by_source[3], 21);
        assert_eq!(c.removed[REM_SOURCE], 21);
        assert_eq!(c.removed[REM_STALE], 21);
        assert_eq!(c.removed[REM_NACP], 21);
        assert_eq!(c.ground, 21);
        assert_eq!(c.kept, 21);
        assert_eq!(x.aircraft, 1);
        let at =
            |x: &Index| x.candidates(10_500, (lat * 1e7) as i32, (lon * 1e7) as i32, Some(10_000));
        assert!(at(&x).is_empty());
        // The same aircraft as an ADS-B source is a candidate: the filter, not the geometry,
        // is what removed it.
        let rows2: Vec<Row> = (0..=20).map(|s| row(2, s, lat, lon, 0)).collect();
        let y = build(&clip(&rows2), Params::default(), None).unwrap();
        assert_eq!(at(&y).len(), 1);
    }

    // REQ: OA-CEN-013
    #[test]
    fn the_time_window_pads_the_adsb_side() {
        let rows: Vec<Row> = (0..=200)
            .map(|s| row(1, 3_500 + s, 40.0, -75.0, 0))
            .collect();
        let w = Window::new(3_600, 3_610).unwrap();
        let p = Params::default();
        let x = build(&clip(&rows), p, Some(w)).unwrap();
        // The ADS-B side is kept from the window start minus the padding to the end plus it,
        // exclusive: the bracket (5 s) and the placebo shift (60 s), 3535 to 3675 s.
        assert_eq!(adsb_pad_ms(&p), 65_000);
        assert_eq!(x.clip.outside_window, 201 - 140);
        assert!(Window::new(5, 5).is_err() && Window::new(0, 86_401).is_err());
        assert!(w.contains(3_600_000) && !w.contains(3_610_000));
    }

    // REQ: OA-CEN-013
    #[test]
    fn the_padded_window_crosses_midnight() {
        let pad = 65_000;
        let day = 86_400_000;
        let early = Window::new(0, 100).unwrap();
        // 60 s before midnight of the previous day is inside the padded early window.
        assert!(early.contains_padded(day - 60_000, pad));
        assert!(!early.contains_padded(day - 66_000, pad));
        assert!(
            early.contains_padded(day + 164_999, pad) && !early.contains_padded(day + 165_000, pad)
        );
        let late = Window::new(86_300, 86_400).unwrap();
        assert!(
            late.contains_padded(day + 60_000, pad) && !late.contains_padded(day + 66_000, pad)
        );
        assert!(late.contains_padded(86_235_000, pad) && !late.contains_padded(86_234_999, pad));
        // A window of the whole day contains everything.
        assert!(Window::new(0, 86_400).unwrap().contains_padded(5, 0));
    }

    // REQ: OA-CEN-017
    #[test]
    fn a_placebo_lookup_at_the_window_edges_finds_its_shifted_adsb() {
        // An aircraft reporting every second across the window and the padding; the update at
        // the first and at the last second of the window, shifted by 60 s, has its aircraft.
        let rows: Vec<Row> = (0..=300)
            .map(|s| row(1, 3_500 + s, 40.0, -75.0, 0))
            .collect();
        let w = Window::new(3_600, 3_700).unwrap();
        let x = build(&clip(&rows), Params::default(), Some(w)).unwrap();
        for t_s in [3_600_i64, 3_699] {
            for shift in [60_i64, -60] {
                let c = x.candidates((t_s + shift) * 1000 + 500, 400_000_000, -750_000_000, None);
                assert_eq!(c.len(), 1, "{t_s} {shift}");
            }
        }
    }

    // REQ: OA-CEN-014
    #[test]
    fn the_search_is_exact_at_62_north_with_a_5_nm_gate() {
        // Gates 1, 2 and 5 NM; aircraft eastbound at 62 N. An update 4.8 NM ahead, 4.8 NM behind,
        // 4.8 NM north and 4.8 NM south of the aircraft is found (the old fixed grid missed the
        // longitude cases), and one at 5.2 NM is not.
        let p = Params {
            gates_nm: [1.0, 2.0, 5.0],
            primary: 2,
            ..Params::default()
        };
        let lat = 62.0;
        let x = build_from_points(
            (0..=20)
                .map(|s| {
                    pt(
                        1,
                        s * 1000,
                        lat,
                        -100.0 + s as f64 * 205.0 * deg_lon_per_m(lat),
                        9000,
                    )
                })
                .collect(),
            p,
        )
        .unwrap();
        let here = -100.0 + 10.5 * 205.0 * deg_lon_per_m(lat);
        for (de, dn, found) in [
            (4.8 * NM_M, 0.0, true),
            (-4.8 * NM_M, 0.0, true),
            (0.0, 4.8 * NM_M, true),
            (0.0, -4.8 * NM_M, true),
            (5.2 * NM_M, 0.0, false),
            (0.0, 5.2 * NM_M, false),
        ] {
            let lon = here + de * deg_lon_per_m(lat);
            let la = lat + dn * deg_lat_per_m(lat);
            let c = x.candidates(
                10_500,
                (la * 1e7).round() as i32,
                (lon * 1e7).round() as i32,
                None,
            );
            assert_eq!(c.len(), usize::from(found), "{de} {dn}");
            if found {
                assert!((c[0].dh_m - 4.8 * NM_M).abs() < 5.0, "{}", c[0].dh_m);
            }
        }
    }

    // REQ: OA-CEN-014
    #[test]
    fn the_search_crosses_the_antimeridian() {
        let lat = 40.0;
        let v = 205.0;
        // Eastbound across 180: at 10.5 s the aircraft is 100 m east of the line; its reports
        // before 9.5 s or so are west of it.
        let lon_ac = 180.0 + 100.0 * deg_lon_per_m(lat);
        let start = lon_ac - 10.5 * v * deg_lon_per_m(lat);
        let x = build_from_points(
            (0..=20)
                .map(|s| {
                    let lon = start + s as f64 * v * deg_lon_per_m(lat);
                    pt_full(1, s * 1000, lat, lon, 9000, 900, 4000)
                })
                .collect(),
            Params::default(),
        )
        .unwrap();
        // The update is 300 m ahead of it (east of the line), then 300 m behind (west of it).
        for (east, along) in [(300.0, 300.0), (-300.0, -300.0)] {
            let lon = lon_ac + east * deg_lon_per_m(lat);
            let lon = if lon >= 180.0 { lon - 360.0 } else { lon };
            let c = x.candidates(10_500, (lat * 1e7) as i32, (lon * 1e7).round() as i32, None);
            assert_eq!(c.len(), 1, "{east}");
            assert!((c[0].dh_m - 300.0).abs() < 2.0, "{}", c[0].dh_m);
            assert!((c[0].along_m.unwrap() - along).abs() < 2.0);
        }
        // Coverage works across the line as well: 20 NM east of the aircraft, within 30 NM.
        let lon = lon_ac + 20.0 * NM_M * deg_lon_per_m(lat);
        let lon = if lon >= 180.0 { lon - 360.0 } else { lon };
        assert!(x.covered(10_500, (lat * 1e7) as i32, (lon * 1e7) as i32, COVERAGE_M));
        let far = lon_ac + 40.0 * NM_M * deg_lon_per_m(lat);
        let far = if far >= 180.0 { far - 360.0 } else { far };
        assert!(!x.covered(10_500, (lat * 1e7) as i32, (far * 1e7) as i32, COVERAGE_M));
    }

    // REQ: OA-CEN-014
    #[test]
    fn polar_and_too_fast_segments_are_counted_not_indexed() {
        let fast_dlon = 7_000.0 * deg_lon_per_m(40.0);
        let x = build_from_points(
            vec![
                pt(1, 0, 40.0, -75.0, 9000),
                pt(1, 10_000, 40.0, -75.0 + fast_dlon, 9000), // 700 m/s
                pt(2, 0, 86.0, 10.0, 9000),
                pt(2, 5_000, 86.0, 10.0, 9000),
                pt(3, 0, 40.0, -75.0, 9000),
                pt(3, 10_000, 40.0, -75.0 + 5_000.0 * deg_lon_per_m(40.0), 9000), // 500 m/s
            ],
            p10(),
        )
        .unwrap();
        assert_eq!((x.segments, x.segments_fast, x.segments_polar), (1, 1, 1));
        assert!(!x.supports(860_000_000) && x.supports(850_000_000));
    }

    // REQ: OA-CEN-009
    #[test]
    fn along_and_cross_signs_for_northbound_and_southbound_tracks() {
        let lat = 40.0;
        let mk = |trk: u16| {
            build_from_points(
                vec![
                    pt_full(1, 0, lat, -75.0, 9000, trk, 4000),
                    pt_full(1, 10_000, lat, -75.0, 9000, trk, 4000),
                ],
                p10(),
            )
            .unwrap()
        };
        let at = |x: &Index, east: f64, north: f64| {
            let c = x.candidates(
                5_000,
                ((lat + north * deg_lat_per_m(lat)) * 1e7).round() as i32,
                ((-75.0 + east * deg_lon_per_m(lat)) * 1e7).round() as i32,
                None,
            );
            (c[0].along_m.unwrap(), c[0].cross_m.unwrap())
        };
        // North (0 degrees): 300 m north is ahead, 100 m east is to the right.
        let (a, c) = at(&mk(0), 100.0, 300.0);
        assert!(
            (a - 300.0).abs() < 1.0 && (c - 100.0).abs() < 1.0,
            "{a} {c}"
        );
        // South (180): 300 m south is ahead, 100 m west is to the right.
        let (a, c) = at(&mk(1800), -100.0, -300.0);
        assert!(
            (a - 300.0).abs() < 1.0 && (c - 100.0).abs() < 1.0,
            "{a} {c}"
        );
        // West (270): 300 m west is ahead, 100 m north is to the right.
        let (a, c) = at(&mk(2700), -300.0, 100.0);
        assert!(
            (a - 300.0).abs() < 1.0 && (c - 100.0).abs() < 1.0,
            "{a} {c}"
        );
        // Northeast (45): along is the projection on the diagonal.
        let (a, c) = at(&mk(450), 100.0, 0.0);
        assert!(
            (a - 70.71).abs() < 0.5 && (c - 70.71).abs() < 0.5,
            "{a} {c}"
        );
    }

    // REQ: OA-CEN-015
    #[test]
    fn coverage_is_separate_from_no_candidate() {
        let lat = 40.0;
        let x = build_from_points(
            (0..=20)
                .map(|s| pt(1, s * 1000, lat, -75.0, 9000))
                .collect(),
            Params::default(),
        )
        .unwrap();
        let near = |nm: f64| {
            let la = lat + nm * NM_M * deg_lat_per_m(lat);
            x.covered(10_500, (la * 1e7).round() as i32, -750_000_000, COVERAGE_M)
        };
        assert!(near(0.0) && near(29.5));
        assert!(!near(30.5));
        // Not covered when the time is outside every segment.
        assert!(!x.covered(500_000, (lat * 1e7) as i32, -750_000_000, COVERAGE_M));
    }

    // REQ: OA-CEN-020
    #[test]
    fn the_estimate_made_from_the_record_count_covers_the_index() {
        let rows: Vec<Row> = (0..=300)
            .flat_map(|s| {
                (1..=5)
                    .map(move |a| row(a, s, 40.0 + f64::from(a) * 0.3, -75.0 + s as f64 * 1e-3, 0))
            })
            .collect();
        let r = clip(&rows);
        let x = build(&r, Params::default(), None).unwrap();
        let est = estimate_build_bytes(&r, &Params::default(), None);
        assert!(est >= x.bytes(), "{est} {}", x.bytes());
        // A window leaves out the reports it cuts, so the estimate falls with it.
        let w = Window::new(100, 110).unwrap();
        assert!(estimate_build_bytes(&r, &Params::default(), Some(w)) < est);
    }

    // REQ: OA-CEN-009
    #[test]
    fn the_gate_list_is_sorted_with_the_given_gate_primary() {
        let g = |nm: f64| {
            let p = Params::default().with_primary_gate_nm(nm);
            (p.gates_nm, p.primary)
        };
        assert_eq!(g(3.0), ([0.5, 2.0, 3.0], 2));
        assert_eq!(g(1.0), ([0.5, 1.0, 2.0], 1));
        assert_eq!(g(0.25), ([0.25, 0.5, 2.0], 0));
        // A gate equal to a default one is not listed twice.
        assert_eq!(g(0.5), ([0.5, 1.0, 2.0], 0));
        assert_eq!(g(2.0), ([0.5, 1.0, 2.0], 2));
    }

    // REQ: OA-CEN-009
    #[test]
    fn an_aircraft_is_found_by_address_at_any_distance_and_a_long_bracket_is_not_used() {
        let x = idx(vec![
            pt(7, 0, 40.0, -75.0, 10_000),
            pt(7, 5_000, 40.0, -75.0, 10_000),
            pt(8, 0, 41.0, -75.0, 10_000),
            pt(8, 9_000, 41.0, -75.0, 10_000),
        ]);
        // 20 km north, far outside any gate.
        let lat = 40.0 + 20_000.0 * deg_lat_per_m(40.0);
        let c = x
            .for_icao(2_500, (lat * 1e7) as i32, -750_000_000, Some(10_000), 7)
            .unwrap();
        assert!((c.dh_m - 20_000.0).abs() < 30.0 && c.gap_ms == 5_000);
        assert!(x.for_icao(2_500, 0, 0, None, 99).is_none());
        // A default (5 s) index does not bracket over a 9 s gap; it counts it.
        let d = build_from_points(
            vec![pt(8, 0, 41.0, -75.0, 1), pt(8, 9_000, 41.0, -75.0, 1)],
            Params::default(),
        )
        .unwrap();
        assert_eq!((d.segments, d.segments_long), (0, 1));
        assert!(d
            .for_icao(4_000, 410_000_000, -750_000_000, None, 8)
            .is_none());
    }
}

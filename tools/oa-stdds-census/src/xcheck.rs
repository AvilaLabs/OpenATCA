//! Position cross-check of STDDS updates against an ADS-B recording of the same day.
//!
//! The question it serves: is a TAIS update with `track/adsb` false free of ADS-B? An address
//! cannot answer it (the address field reads zero on almost all such updates), so the update's
//! position is compared with every ADS-B aircraft, interpolated to the update's time.
//!
//! Time base: the STDDS time compared is `track/mrtTime` (the track's time stamp, UTC ms),
//! against the ADS-B report time of the clip (UTC, 10 ms resolution). Each ADS-B aircraft is
//! linearly interpolated between the two reports that bracket that time, if they are at most
//! [`Params::max_gap_ms`] apart; an aircraft with no such bracket is not a candidate.
//! Altitude: the STDDS `reportedAltitude` (ASSUMED feet, pressure altitude) against the ADS-B
//! barometric altitude, interpolated the same way. If either side has none the altitude gate is
//! not applied to that pair (counted by the census as not compared).
//! Reports on the ground and stale reports are left out of the index.
//!
//! Candidates are found through a grid of 10 s time bins and 0.2 degree cells holding the
//! segments (pairs of consecutive reports of one aircraft). A segment is stored in the cell of
//! its first report, and a query reads the 3 x 3 cells around the point, so the largest gate
//! plus the longest segment must stay below the cell: [`build`] refuses a gate above 5 NM or a
//! gap above 10 s (9.3 km + 3.4 km is below the 14 km a 0.2 degree cell is wide at 50 degrees
//! north). Latitudes above 60 degrees are not supported.

use oa_adsb::{reports_len, Reports, F_GROUND, F_STALE, NONE_I32, NONE_U16};
use std::collections::HashMap;

/// Metres per nautical mile.
pub const NM_M: f64 = 1852.0;
/// Gates tried at once: horizontal, nautical miles. Index [`Params::primary`] is the one the
/// distributions are taken from; the others only change the counts.
pub const DEFAULT_GATES_NM: [f64; 3] = [0.5, 1.0, 2.0];
/// Vertical gate, feet (not varied in the sweep).
pub const DEFAULT_V_GATE_FT: f64 = 500.0;
/// Longest gap between two reports of one aircraft that is still interpolated, ms.
pub const DEFAULT_MAX_GAP_MS: i64 = 10_000;

const TBIN_MS: i64 = 10_000;
const CELL_DEG: f64 = 0.2;
const MAX_GATE_NM: f64 = 5.0;
const MAX_GAP_LIMIT_MS: i64 = 10_000;
/// Ground speed from which an along-track difference is turned into seconds, m/s (about 100 kt).
pub const MIN_GS_FOR_TIME_MPS: f64 = 51.4;

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
            return Err("the interpolation gap must be 1 ms to 10 s".to_string());
        }
        Ok(())
    }
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
    cells: HashMap<(i64, i32, i32), Vec<u32>>,
    pub params: Params,
    /// Distinct aircraft and interpolable segments indexed.
    pub aircraft: u64,
    pub segments: u64,
}

fn cell_of(lat_e7: i32, lon_e7: i32) -> (i32, i32) {
    let c = |e7: i32| (f64::from(e7) / 1e7 / CELL_DEG).floor() as i32;
    (c(lat_e7), c(lon_e7))
}

/// Points from the clip's reports: ground and stale reports are dropped.
pub fn points_from_reports(r: &Reports) -> Vec<Pt> {
    let mut v = Vec::new();
    for i in 0..reports_len(r) {
        let (Some(&icao), Some(&ns), Some(&lat), Some(&lon), Some(&fl)) = (
            r.icao.get(i),
            r.time_ns.get(i),
            r.lat_e7.get(i),
            r.lon_e7.get(i),
            r.flags.get(i),
        ) else {
            continue;
        };
        if fl & (F_GROUND | F_STALE) != 0 {
            continue;
        }
        v.push(Pt {
            icao,
            t_ms: ns.div_euclid(1_000_000),
            lat_e7: lat,
            lon_e7: lon,
            alt_ft: r.baro_alt_ft.get(i).copied().unwrap_or(NONE_I32),
            track_ddeg: r.track_ddeg.get(i).copied().unwrap_or(NONE_U16),
            gs_dkt: r.gs_dkt.get(i).copied().unwrap_or(NONE_U16),
        });
    }
    v
}

/// Builds the index from the clip's reports.
pub fn build(r: &Reports, params: Params) -> Result<Index, String> {
    build_from_points(points_from_reports(r), params)
}

/// Builds the index from points (the tests' entry).
pub fn build_from_points(mut pts: Vec<Pt>, params: Params) -> Result<Index, String> {
    params.check()?;
    pts.sort_by_key(|p| (p.icao, p.t_ms));
    let mut cells: HashMap<(i64, i32, i32), Vec<u32>> = HashMap::new();
    let mut aircraft = 0u64;
    let mut segments = 0u64;
    let mut last_icao: Option<u32> = None;
    for p in &pts {
        if last_icao != Some(p.icao) {
            last_icao = Some(p.icao);
            aircraft += 1;
        }
    }
    for (i, w) in pts.windows(2).enumerate() {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
            continue;
        };
        if a.icao != b.icao || b.t_ms <= a.t_ms || b.t_ms - a.t_ms > params.max_gap_ms {
            continue;
        }
        let Ok(ix) = u32::try_from(i) else {
            return Err("too many ADS-B reports".to_string());
        };
        let (la, lo) = cell_of(a.lat_e7, a.lon_e7);
        for tb in a.t_ms.div_euclid(TBIN_MS)..=b.t_ms.div_euclid(TBIN_MS) {
            cells.entry((tb, la, lo)).or_default().push(ix);
        }
        segments += 1;
    }
    Ok(Index {
        pts,
        cells,
        params,
        aircraft,
        segments,
    })
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

impl Index {
    /// Reports kept.
    pub fn points(&self) -> usize {
        self.pts.len()
    }

    /// Every aircraft that is interpolable at `t_ms` and within the largest gate of the point,
    /// one candidate per aircraft; altitude is compared but not gated here. `alt_ft` is the
    /// STDDS altitude, if it has one.
    pub fn candidates(
        &self,
        t_ms: i64,
        lat_e7: i32,
        lon_e7: i32,
        alt_ft: Option<i32>,
    ) -> Vec<Cand> {
        let mut out: Vec<Cand> = Vec::new();
        let (cla, clo) = cell_of(lat_e7, lon_e7);
        let tb = t_ms.div_euclid(TBIN_MS);
        let max_h = self.params.max_gate_m();
        for la in cla - 1..=cla + 1 {
            for lo in clo - 1..=clo + 1 {
                let Some(list) = self.cells.get(&(tb, la, lo)) else {
                    continue;
                };
                for &ix in list {
                    let (Some(a), Some(b)) =
                        (self.pts.get(ix as usize), self.pts.get(ix as usize + 1))
                    else {
                        continue;
                    };
                    if t_ms < a.t_ms || t_ms > b.t_ms || out.iter().any(|c| c.icao == a.icao) {
                        continue;
                    }
                    let f = (t_ms - a.t_ms) as f64 / (b.t_ms - a.t_ms) as f64;
                    let lerp = |x: i32, y: i32| f64::from(x) + (f64::from(y) - f64::from(x)) * f;
                    let lat = lerp(a.lat_e7, b.lat_e7) / 1e7;
                    let lon = lerp(a.lon_e7, b.lon_e7) / 1e7;
                    let (m_per_rad, n_cos) = radii(lat.to_radians());
                    let de = (f64::from(lon_e7) / 1e7 - lon).to_radians() * n_cos;
                    let dn = (f64::from(lat_e7) / 1e7 - lat).to_radians() * m_per_rad;
                    let dh = de.hypot(dn);
                    if dh > max_h {
                        continue;
                    }
                    let alt_a = (a.alt_ft != NONE_I32 && b.alt_ft != NONE_I32)
                        .then(|| lerp(a.alt_ft, b.alt_ft));
                    let dv_ft = alt_ft.zip(alt_a).map(|(s, x)| f64::from(s) - x);
                    let track = [a.track_ddeg, b.track_ddeg]
                        .into_iter()
                        .find(|&t| t != NONE_U16)
                        .map(|t| f64::from(t) / 10.0);
                    let (along_m, cross_m) = match track {
                        Some(deg) => {
                            let (s, c) = deg.to_radians().sin_cos();
                            (Some(de * s + dn * c), Some(de * c - dn * s))
                        }
                        None => (None, None),
                    };
                    let gs_mps = [a.gs_dkt, b.gs_dkt]
                        .into_iter()
                        .find(|&g| g != NONE_U16)
                        .map(|g| f64::from(g) / 10.0 * NM_M / 3600.0);
                    out.push(Cand {
                        icao: a.icao,
                        dh_m: dh,
                        dv_ft,
                        along_m,
                        cross_m,
                        gs_mps,
                    });
                }
            }
        }
        out
    }
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

    pub fn deg_lon_per_m(lat_deg: f64) -> f64 {
        1.0 / (radii(lat_deg.to_radians()).1 * std::f64::consts::PI / 180.0)
    }

    pub fn deg_lat_per_m(lat_deg: f64) -> f64 {
        1.0 / m_per_deg_lat(lat_deg)
    }

    fn idx(pts: Vec<Pt>) -> Index {
        build_from_points(pts, Params::default()).unwrap()
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
            gates_nm: [0.5, 1.0, 6.0],
            ..Params::default()
        }));
        assert!(bad(Params {
            max_gap_ms: 20_000,
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
}

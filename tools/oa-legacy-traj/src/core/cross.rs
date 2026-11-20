//! When a truth path reached a point, and when an aircraft landed. Core (pure).
//!
//! **Crossing.** The truth path is a polyline through its samples, moving linearly in time
//! between two samples that truth answered consecutively; across a hole in the truth there is no
//! segment. "Reached a point" is the time of closest approach of the path to the point. The
//! distance is measured in a flat east-north frame centred on the point (the stretches that
//! matter are a few nautical miles; farther away the distance is only compared with the cut).
//! A stretch of the path within the cut distance is one passage, with its own closest approach,
//! so an aircraft that holds over a point passes it more than once.
//!
//! **Landing.** The landing time is the middle of the gap between the last airborne report and
//! the first ground report, where the last airborne report is within the runway radius of the
//! airport reference point, the aircraft then stays on the ground, and was low or slow.

use crate::core::types::{
    AirState, Approach, Config, Exclusion, Landing, LandingSet, Passage, PathPoint, Place, NO_ALT,
    NO_SPEED,
};

/// Metres per degree of latitude (a sphere of 6,371,008.8 m).
const M_PER_DEG: f64 = 111_194.926_644_558_74;

/// East and north metres of `(lat, lon)` from `origin` in the flat frame.
fn flat(origin: Place, lat_deg: f64, lon_deg: f64, cos_lat: f64) -> (f64, f64) {
    let mut dlon = lon_deg - origin.lon_deg;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    (
        dlon * cos_lat * M_PER_DEG,
        (lat_deg - origin.lat_deg) * M_PER_DEG,
    )
}

/// The distance in metres between two places, in the flat frame centred on the first.
pub fn flat_dist_m(a: Place, lat_deg: f64, lon_deg: f64) -> f64 {
    let cos_lat = oa_math::cos(a.lat_deg * (core::f64::consts::PI / 180.0));
    let (x, y) = flat(a, lat_deg, lon_deg, cos_lat);
    oa_ieee::sqrt(x * x + y * y)
}

/// One stretch of the path: the closest approach of a segment (or of a lone sample).
struct Item {
    /// Index of the first sample of the segment.
    first: usize,
    /// Index of the last sample of the segment.
    last: usize,
    dist_m: f64,
    t_ms: i64,
    /// The segment begins at the start of the path or after a hole.
    start_edge: bool,
    /// The segment ends at the end of the path or before a hole.
    end_edge: bool,
}

/// The closest approach of the path to `place`, and every passage within `cut_m`.
pub fn approach(path: &[PathPoint], place: Place, cut_m: f64) -> Approach {
    let cos_lat = oa_math::cos(place.lat_deg * (core::f64::consts::PI / 180.0));
    let n = path.len();
    let xy: Vec<(f64, f64)> = path
        .iter()
        .map(|p| flat(place, p.lat_deg, p.lon_deg, cos_lat))
        .collect();
    let joined = |i: usize| path.get(i).is_some_and(|p| !p.gap_before);
    let mut items: Vec<Item> = Vec::new();
    for i in 0..n {
        let (Some(&(x0, y0)), Some(p0)) = (xy.get(i), path.get(i)) else {
            continue;
        };
        let has_prev = i > 0 && joined(i);
        let has_next = i + 1 < n && joined(i + 1);
        if !has_next {
            if !has_prev {
                // A sample with no neighbour it is joined to: a segment of no length.
                items.push(Item {
                    first: i,
                    last: i,
                    dist_m: oa_ieee::sqrt(x0 * x0 + y0 * y0),
                    t_ms: p0.t_ms,
                    start_edge: true,
                    end_edge: true,
                });
            }
            continue;
        }
        let (Some(&(x1, y1)), Some(p1)) = (xy.get(i + 1), path.get(i + 1)) else {
            continue;
        };
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = dx * dx + dy * dy;
        let u = if len2 > 0.0 {
            oa_math::clamp_f64(-(x0 * dx + y0 * dy) / len2, 0.0, 1.0)
        } else {
            0.0
        };
        let (cx, cy) = (x0 + u * dx, y0 + u * dy);
        let span = (p1.t_ms - p0.t_ms) as f64;
        items.push(Item {
            first: i,
            last: i + 1,
            dist_m: oa_ieee::sqrt(cx * cx + cy * cy),
            t_ms: p0.t_ms + oa_ieee::round(u * span) as i64,
            start_edge: i == 0 || !joined(i),
            end_edge: i + 2 >= n || !joined(i + 2),
        });
    }
    let mut min_dist_m = f64::INFINITY;
    for it in &items {
        if it.dist_m < min_dist_m {
            min_dist_m = it.dist_m;
        }
    }
    let mut passages: Vec<Passage> = Vec::new();
    // The run of consecutive items within the cut being built: best distance and time, and
    // whether it touches an edge, and the last sample it reached.
    let mut run: Option<(f64, i64, bool, usize)> = None;
    for it in &items {
        let inside = it.dist_m <= cut_m;
        let continues = inside && run.is_some_and(|r| it.first <= r.3);
        if continues {
            if let Some(r) = run.as_mut() {
                if it.dist_m < r.0 {
                    r.0 = it.dist_m;
                    r.1 = it.t_ms;
                }
                r.2 = r.2 || it.end_edge;
                r.3 = it.last;
            }
            continue;
        }
        if let Some((d, t, edge, _)) = run.take() {
            passages.push(Passage {
                t_ms: t,
                dist_m: d,
                at_edge: edge,
            });
        }
        if inside {
            run = Some((it.dist_m, it.t_ms, it.start_edge || it.end_edge, it.last));
        }
    }
    if let Some((d, t, edge, _)) = run {
        passages.push(Passage {
            t_ms: t,
            dist_m: d,
            at_edge: edge,
        });
    }
    Approach {
        min_dist_m,
        passages,
    }
}

/// The time the aircraft reached the point for an estimate issued at `issue_ms`: the closest
/// approach of the first passage that is not before the issue; when every passage is before it,
/// the last one (the event happened before the message: the error is kept). The estimate never
/// chooses. A passage that is in progress at the issue is chosen by its closest approach like any
/// other.
pub fn choose_passage(a: &Approach, issue_ms: i64, cfg: &Config) -> Result<i64, Exclusion> {
    if a.min_dist_m.is_infinite() {
        return Err(Exclusion::NoTruthPath);
    }
    if a.min_dist_m > cfg.cut_m {
        return Err(Exclusion::BeyondCut);
    }
    let chosen = a
        .passages
        .iter()
        .find(|p| p.t_ms >= issue_ms)
        .or_else(|| a.passages.last());
    match chosen {
        None => Err(Exclusion::BeyondCut),
        Some(p) if p.at_edge => Err(Exclusion::PassageAtEdge),
        Some(p) => Ok(p.t_ms),
    }
}

/// Every change from airborne to ground in `states` (sorted by time) that holds up as a landing,
/// with whether the last airborne report is near `arp`. A change holds up when no airborne report
/// follows within the persistence time and the last airborne report is below the airport
/// elevation `elev_ft` plus the altitude limit, or below the speed limit. Changes near the
/// airport that do not are counted.
pub fn landings(states: &[AirState], arp: Place, elev_ft: Option<f64>, cfg: &Config) -> LandingSet {
    let mut out = LandingSet::default();
    for (i, w) in states.windows(2).enumerate() {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
            continue;
        };
        if a.ground || !b.ground {
            continue;
        }
        let at_airport = flat_dist_m(arp, a.lat_deg, a.lon_deg) <= cfg.runway_radius_m;
        let flies_on = states
            .iter()
            .skip(i + 2)
            .take_while(|s| s.t_ms <= b.t_ms + cfg.landing_persist_ms)
            .any(|s| !s.ground);
        if flies_on {
            if at_airport {
                out.refused_persist += 1;
            }
            continue;
        }
        let low = a.alt_ft != NO_ALT
            && elev_ft.is_some_and(|e| f64::from(a.alt_ft) < e + cfg.landing_alt_ft);
        let slow = a.gs_dkt != NO_SPEED && f64::from(a.gs_dkt) * 0.1 * KT_MS < cfg.landing_speed_ms;
        if !(low || slow) {
            if at_airport {
                out.refused_altitude += 1;
            }
            continue;
        }
        out.landings.push(Landing {
            t_ms: a.t_ms + (b.t_ms - a.t_ms) / 2,
            t_air_ms: a.t_ms,
            t_gnd_ms: b.t_ms,
            gap_ms: b.t_ms - a.t_ms,
            at_airport,
        });
    }
    out
}

/// Metres per second in a knot.
const KT_MS: f64 = 1852.0 / 3600.0;

/// The landing for an estimate issued at `issue_ms`: the first landing at the airport that is not
/// before the issue, else the last one before it (kept). When there is none at the airport: a
/// landing elsewhere is a diversion; refused changes near the airport are touch-and-go.
pub fn choose_landing(set: &LandingSet, issue_ms: i64, cfg: &Config) -> Result<Landing, Exclusion> {
    let here: Vec<&Landing> = set.landings.iter().filter(|x| x.at_airport).collect();
    let chosen = here
        .iter()
        .find(|x| x.t_ms >= issue_ms)
        .or_else(|| here.last());
    match chosen {
        Some(x) if x.gap_ms > cfg.landing_gap_max_ms => Err(Exclusion::LandingGap),
        Some(x) => Ok(**x),
        None if !set.landings.is_empty() => Err(Exclusion::Diversion),
        None if set.refused_persist + set.refused_altitude > 0 => Err(Exclusion::TouchAndGo),
        None => Err(Exclusion::NoLanding),
    }
}

/// The great-circle distance in metres between two places (haversine, sphere of 6,371,008.8 m).
pub fn great_circle_m(a: Place, b: Place) -> f64 {
    let rad = core::f64::consts::PI / 180.0;
    let (la, lb) = (a.lat_deg * rad, b.lat_deg * rad);
    let sin_dlat = oa_math::sin((lb - la) / 2.0);
    let sin_dlon = oa_math::sin((b.lon_deg - a.lon_deg) * rad / 2.0);
    let h = sin_dlat * sin_dlat + oa_math::cos(la) * oa_math::cos(lb) * sin_dlon * sin_dlon;
    2.0 * 6_371_008.8 * oa_math::asin(oa_math::min_f64(1.0, oa_ieee::sqrt(h)))
}

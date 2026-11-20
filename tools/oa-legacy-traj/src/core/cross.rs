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
//! **Landing.** The landing time is the last airborne report before the first ground report, where
//! that report is within the runway radius of the airport reference point.

use crate::core::types::{
    AirState, Approach, Config, Exclusion, Landing, Passage, PathPoint, Place,
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
/// approach of the first passage that is not before the issue. The estimate never chooses.
pub fn choose_passage(a: &Approach, issue_ms: i64, cfg: &Config) -> Result<i64, Exclusion> {
    if a.min_dist_m.is_infinite() {
        return Err(Exclusion::NoTruthPath);
    }
    if a.min_dist_m > cfg.cut_m {
        return Err(Exclusion::BeyondCut);
    }
    match a.passages.iter().find(|p| p.t_ms >= issue_ms) {
        None => Err(Exclusion::PassedBeforeIssue),
        Some(p) if p.at_edge => Err(Exclusion::PassageAtEdge),
        Some(p) => Ok(p.t_ms),
    }
}

/// Every change from airborne to ground in `states` (sorted by time), with whether the last
/// airborne report is near `arp`.
pub fn landings(states: &[AirState], arp: Place, cfg: &Config) -> Vec<Landing> {
    let mut out = Vec::new();
    for w in states.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
            continue;
        };
        if a.ground || !b.ground {
            continue;
        }
        out.push(Landing {
            t_ms: a.t_ms,
            gap_ms: b.t_ms - a.t_ms,
            at_airport: flat_dist_m(arp, a.lat_deg, a.lon_deg) <= cfg.runway_radius_m,
        });
    }
    out
}

/// The landing time for an estimate issued at `issue_ms`: the first landing at the airport that
/// is not before the issue.
pub fn choose_landing(l: &[Landing], issue_ms: i64, cfg: &Config) -> Result<i64, Exclusion> {
    let mut too_long = false;
    let mut before = false;
    for x in l.iter().filter(|x| x.at_airport) {
        if x.t_ms < issue_ms {
            before = true;
        } else if x.gap_ms > cfg.landing_gap_max_ms {
            too_long = true;
        } else {
            return Ok(x.t_ms);
        }
    }
    if too_long {
        Err(Exclusion::LandingGap)
    } else if before {
        Err(Exclusion::PassedBeforeIssue)
    } else {
        Err(Exclusion::NoLanding)
    }
}

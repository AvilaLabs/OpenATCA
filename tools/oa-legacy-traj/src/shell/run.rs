//! The run: recordings, clip, truth, the join, the scoring and the files. Shell (B3).

use std::collections::{BTreeMap, BTreeSet};

use oa_adsb::{reports_extend, reports_len, Reports, F_GROUND, NONE_I32};
use oa_bench::adsb_truth::{self, BuiltTruth};
use oa_bench::input::{load_adsb_counted, load_counted, msg_limit, read_capped_file, FeedInput};
use oa_bench::legacy::{
    name, norm_id, plan_candidates, plan_index, tbfm_kind, tfms_kind, Legacy, PlanIndex, NONE,
    NO_COORD, NO_TIME,
};
use oa_replay::FeedKind;

use crate::core::bins::score;
use crate::core::cross::{approach, choose_landing, choose_passage, landings};
use crate::core::join::{index_legs, join, legs_of, outcome_index, Leg, LegIndex, Outcome, Unit};
use crate::core::places::{classify, resolve, Resolution, CLASS_NAMES};
use crate::core::report::{
    closest_add, kind_add, kind_add_with, kind_exclude, render_counts_csv, render_csv, render_text,
    system_new, Extra, Section, SystemReport,
};
use crate::core::types::{
    AirState, Approach, Config, Exclusion, Landing, PathPoint, Place, Sample, NO_ADDR,
};
use crate::shell::args::Args;
use crate::shell::guard::guarded_opener;
use crate::shell::places::{
    airport_key, airport_places, apply_places, load_nasr, parse_places, place_of_e7, point_places,
    PlaceTable, PlacesFile,
};
use crate::shell::write::write_all;

const MS_PER_MIN: i64 = 60_000;
/// The longest places file read, bytes.
const MAX_PLACES_BYTES: u64 = 64 << 20;
/// The longest look-ahead scored, minutes: the end of the last bin.
const MAX_LOOK_AHEAD_MIN: i64 = 120;

/// Everything the scoring reads, besides the estimates.
// A7.2: cold struct, size guard waived
struct Ctx<'a> {
    cfg: Config,
    step_ms: i64,
    built: &'a BuiltTruth,
    ix: &'a LegIndex,
    table: &'a PlaceTable,
    states: &'a BTreeMap<u32, Vec<AirState>>,
    /// The TFMS actual arrival time by plan identifier, Unix ms (the latest issued).
    tfms_actual: &'a BTreeMap<[u8; 12], i64>,
}

/// The airport key of a code held as bytes.
fn key_of(code: &[u8]) -> String {
    airport_key(name(code))
}

/// At most this many reference places are taken from a path or a list of states.
const MAX_REFS: usize = 200;

/// Reference places for choosing among places of one name: every n-th of `v`, at most
/// [`MAX_REFS`].
fn thin(v: impl ExactSizeIterator<Item = Place>) -> Vec<Place> {
    let step = (v.len() / MAX_REFS).max(1);
    v.step_by(step).collect()
}

/// Resolves a point name against the table; the places of the route reference are asked for
/// only when the name has several. `Ok` carries whether one of several was taken.
fn point_result(
    table: &PlaceTable,
    nm: &str,
    refs: impl FnOnce() -> Vec<Place>,
) -> Result<(Place, bool), Exclusion> {
    let cands = point_places(table, nm);
    let r = if cands.len() > 1 { refs() } else { Vec::new() };
    match resolve(cands, &r) {
        Resolution::NotFound => Err(Exclusion::PlaceUnknown),
        Resolution::Unique(p) => Ok((p, false)),
        Resolution::Picked(p) => Ok((p, true)),
        Resolution::Refused => Err(Exclusion::PlaceAmbiguous),
    }
}

/// A name padded to 8 bytes and normalised, the callsign form the truth legs use.
fn callsign8(c: &[u8]) -> [u8; 8] {
    norm_id::<8>(c)
}

/// The core legs of the truth legs of the clip.
fn core_legs(built: &BuiltTruth) -> Vec<Leg> {
    built
        .legs
        .iter()
        .map(|l| Leg {
            addr: l.addr,
            start_ms: l.start_ms,
            end_ms: l.end_ms,
            callsign: l.callsign.map_or([0; 8], |c| callsign8(&c)),
        })
        .collect()
}

/// The truth path of the legs `leg_ixs` of `addr` within `[from_ms, to_ms]`, sampled every
/// `step_ms`. A time truth has no answer for leaves a hole: the next sample is marked as after
/// one, and so is the first sample of every leg.
fn sample_path(
    ctx: &Ctx<'_>,
    addr: u32,
    leg_ixs: &[u32],
    from_ms: i64,
    to_ms: i64,
) -> Vec<PathPoint> {
    let mut legs: Vec<&Leg> = leg_ixs
        .iter()
        .filter_map(|&i| ctx.ix.legs.get(i as usize))
        .collect();
    legs.sort_by_key(|l| (l.start_ms, l.end_ms));
    let mut out = Vec::new();
    for l in legs {
        let (lo, hi) = (l.start_ms.max(from_ms), l.end_ms.min(to_ms));
        let mut hole = true;
        let mut t = lo;
        while t <= hi {
            match adsb_truth::query(&ctx.built.truth, addr, t) {
                Ok(s) => {
                    out.push(PathPoint {
                        t_ms: t,
                        lat_deg: s.lat,
                        lon_deg: s.lon,
                        gap_before: hole,
                    });
                    hole = false;
                }
                Err(_) => hole = true,
            }
            t += ctx.step_ms;
        }
    }
    out
}

/// A key for a place, to keep one search per place of a flight.
fn place_key(p: Place) -> (i64, i64) {
    (
        (p.lat_deg * 1e6).round() as i64,
        (p.lon_deg * 1e6).round() as i64,
    )
}

/// The searches of one flight's path against its points, by place, so a point sent in every
/// message is searched once.
// A7.2: cold struct, size guard waived
#[derive(Default)]
struct Searches {
    by_place: BTreeMap<(i64, i64), Approach>,
}

/// The closest approach of `path` to `place`, searched once per place; a new search is counted in
/// the closest-approach buckets of `sys`.
fn approach_of<'a>(
    s: &'a mut Searches,
    sys: &mut SystemReport,
    path: &[PathPoint],
    place: Place,
    cfg: &Config,
) -> &'a Approach {
    s.by_place.entry(place_key(place)).or_insert_with(|| {
        let a = approach(path, place, cfg.cut_m);
        closest_add(sys, a.min_dist_m);
        a
    })
}

fn score_cross(
    a: &Approach,
    est_ms: i64,
    issue_ms: i64,
    cfg: &Config,
) -> Result<Sample, Exclusion> {
    let actual = choose_passage(a, issue_ms, cfg)?;
    score(est_ms, actual, issue_ms, cfg)
}

/// A flight of the TBFM or TFMS tables: its rows, and what it joined to.
// A7.2: cold struct, size guard waived
struct TableUnit {
    unit: Unit,
    outcome: Outcome,
    rows: Vec<u32>,
    /// The airport the flight lands at, as [`airport_key`], when the records name one.
    arrival: Option<String>,
    /// The plan identifier, to find the TFMS actual arrival of the flight.
    plan: [u8; 12],
}

/// The aircraft address the flights of `cands` carry: none, one, or conflicting.
fn addr_of(leg: &Legacy, cands: &[u32]) -> (u32, bool) {
    let set: BTreeSet<u32> = cands
        .iter()
        .filter_map(|&f| leg.flights.addr.get(f as usize).copied())
        .filter(|&a| a != NONE)
        .collect();
    let mut it = set.iter();
    match (it.next(), it.next()) {
        (None, _) => (NO_ADDR, false),
        (Some(&a), None) => (a, false),
        _ => (NO_ADDR, true),
    }
}

/// The one arrival airport of the flights of `cands`, when they name exactly one.
fn arrival_of(leg: &Legacy, cands: &[u32]) -> Option<String> {
    let set: BTreeSet<String> = cands
        .iter()
        .filter_map(|&f| leg.flights.arr.get(f as usize))
        .map(|a| key_of(a))
        .filter(|a| !a.is_empty())
        .collect();
    let mut it = set.iter();
    match (it.next(), it.next()) {
        (Some(a), None) => Some(a.clone()),
        _ => None,
    }
}

/// The interval of a flight: its first issue time up to its last plus the longest look-ahead;
/// `None` when no row has an issue time.
fn interval(issues: impl Iterator<Item = i64>) -> Option<(i64, i64)> {
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for t in issues.filter(|&t| t != NO_TIME) {
        lo = lo.min(t);
        hi = hi.max(t);
    }
    (lo <= hi).then_some((lo, hi + MAX_LOOK_AHEAD_MIN * MS_PER_MIN))
}

/// TBFM flights: one per plan identifier.
fn tbfm_units(
    leg: &Legacy,
    plans: &PlanIndex,
    ix: &LegIndex,
    tfms_arr: &BTreeMap<[u8; 12], String>,
) -> Vec<TableUnit> {
    let mut groups: BTreeMap<[u8; 12], Vec<u32>> = BTreeMap::new();
    for (i, p) in leg.tbfm.plan_id.iter().enumerate() {
        groups.entry(norm_id::<12>(p)).or_default().push(i as u32);
    }
    let mut out = Vec::new();
    for (key, rows) in groups {
        let cands = plan_candidates(plans, &key);
        let (addr, addr_conflict) = addr_of(leg, cands);
        let first = rows.first().copied().unwrap_or(0) as usize;
        let mut callsign = leg.tbfm.aid.get(first).map_or([0; 8], |a| callsign8(a));
        if callsign.iter().all(|&b| b == 0) {
            callsign = cands
                .first()
                .and_then(|&f| leg.flights.callsign.get(f as usize))
                .map_or([0; 8], |c| callsign8(c));
        }
        let (from_ms, to_ms) = interval(
            rows.iter()
                .filter_map(|&r| leg.tbfm.issue_ms.get(r as usize).copied()),
        )
        .unwrap_or((0, 0));
        let unit = Unit {
            addr,
            addr_conflict,
            callsign,
            from_ms,
            to_ms,
        };
        let outcome = join(ix, &unit);
        let arrival = arrival_of(leg, cands).or_else(|| tfms_arr.get(&key).cloned());
        out.push(TableUnit {
            unit,
            outcome,
            rows,
            arrival,
            plan: key,
        });
    }
    out
}

/// TFMS flights: one per plan identifier; only the arrival estimates are rows of a flight.
fn tfms_units(leg: &Legacy, plans: &PlanIndex, ix: &LegIndex) -> Vec<TableUnit> {
    let mut groups: BTreeMap<[u8; 16], Vec<u32>> = BTreeMap::new();
    for (i, (p, k)) in leg.tfms.plan_id.iter().zip(&leg.tfms.kind).enumerate() {
        if *k == tfms_kind::ETA {
            groups.entry(*p).or_default().push(i as u32);
        }
    }
    let mut out = Vec::new();
    for (plan, rows) in groups {
        let cands = plan_candidates(plans, &norm_id::<12>(&plan));
        let (addr, addr_conflict) = addr_of(leg, cands);
        let first = rows.first().copied().unwrap_or(0) as usize;
        let mut callsign = leg.tfms.acid.get(first).map_or([0; 8], |a| callsign8(a));
        if callsign.iter().all(|&b| b == 0) {
            callsign = cands
                .first()
                .and_then(|&f| leg.flights.callsign.get(f as usize))
                .map_or([0; 8], |c| callsign8(c));
        }
        let (from_ms, to_ms) = interval(
            rows.iter()
                .filter_map(|&r| leg.tfms.issue_ms.get(r as usize).copied()),
        )
        .unwrap_or((0, 0));
        let unit = Unit {
            addr,
            addr_conflict,
            callsign,
            from_ms,
            to_ms,
        };
        let outcome = join(ix, &unit);
        let arr: BTreeSet<String> = rows
            .iter()
            .filter_map(|&r| leg.tfms.arr.get(r as usize))
            .map(|a| key_of(a))
            .filter(|a| !a.is_empty())
            .collect();
        let mut it = arr.iter();
        let arrival = match (it.next(), it.next()) {
            (Some(a), None) => Some(a.clone()),
            _ => None,
        };
        out.push(TableUnit {
            unit,
            outcome,
            rows,
            arrival,
            plan: norm_id::<12>(&plan),
        });
    }
    out
}

fn joined_addr(o: Outcome) -> Option<u32> {
    match o {
        Outcome::ByAddress(a) | Outcome::ByCallsign(a) => Some(a),
        _ => None,
    }
}

fn by_callsign(o: Outcome) -> Option<bool> {
    match o {
        Outcome::ByAddress(_) => Some(false),
        Outcome::ByCallsign(_) => Some(true),
        _ => None,
    }
}

/// The ground-state reports of the aircraft in `want`, by address, in time order.
fn air_states(reports: &Reports, want: &BTreeSet<u32>) -> BTreeMap<u32, Vec<AirState>> {
    let mut out: BTreeMap<u32, Vec<AirState>> = BTreeMap::new();
    for i in 0..reports_len(reports) {
        let (Some(&a), Some(&ns), Some(&fl), Some(&lat), Some(&lon)) = (
            reports.icao.get(i),
            reports.time_ns.get(i),
            reports.flags.get(i),
            reports.lat_e7.get(i),
            reports.lon_e7.get(i),
        ) else {
            continue;
        };
        if !want.contains(&a) {
            continue;
        }
        let airborne = reports.baro_alt_ft.get(i).is_some_and(|&v| v != NONE_I32)
            || reports.geom_alt_ft.get(i).is_some_and(|&v| v != NONE_I32);
        let ground = fl & F_GROUND != 0;
        if !ground && !airborne {
            continue;
        }
        let p = place_of_e7(lat, lon);
        out.entry(a).or_default().push(AirState {
            t_ms: ns.div_euclid(1_000_000),
            lat_deg: p.lat_deg,
            lon_deg: p.lon_deg,
            ground,
        });
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| a.t_ms.cmp(&b.t_ms));
    }
    out
}

/// ERAM: the predicted time of each route point of each converted route, against when the
/// aircraft reached the point. The look-ahead runs from the message that carried the route.
fn eram(leg: &Legacy, ctx: &Ctx<'_>, sys: &mut SystemReport, flights_total: &mut (u64, u64)) {
    let mut by_flight: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (r, f) in leg.routes.flight.iter().enumerate() {
        by_flight.entry(*f).or_default().push(r as u32);
    }
    flights_total.0 = leg.flights.gufi.len() as u64;
    flights_total.1 = by_flight.len() as u64;
    for (&f, routes) in &by_flight {
        let ts = |r: u32| leg.routes.ts_ms.get(r as usize).copied().unwrap_or(NO_TIME);
        let n_points: u64 = routes
            .iter()
            .map(|&r| u64::from(leg.routes.len.get(r as usize).copied().unwrap_or(0)))
            .sum();
        let (from_ms, to_ms) = interval(routes.iter().map(|&r| ts(r))).unwrap_or((0, 0));
        let unit = Unit {
            addr: leg.flights.addr.get(f as usize).copied().unwrap_or(NONE),
            addr_conflict: false,
            callsign: leg
                .flights
                .callsign
                .get(f as usize)
                .map_or([0; 8], |c| callsign8(c)),
            from_ms,
            to_ms,
        };
        let outcome = join(ctx.ix, &unit);
        if let Some(c) = sys.outcomes.get_mut(outcome_index(outcome)) {
            *c += 1;
        }
        let Some(addr) = joined_addr(outcome) else {
            if let Some(k) = sys.kinds.first_mut() {
                kind_exclude(k, Exclusion::NotJoined, n_points);
            }
            continue;
        };
        let path = sample_path(ctx, addr, &legs_of_unit(ctx, addr, &unit), from_ms, to_ms);
        let mut searches = Searches::default();
        // Several places of one name: the one nearest the route's own coordinates, else nearest
        // the flown path.
        let route_refs = || {
            let mut v: Vec<Place> = Vec::new();
            for &r in routes {
                let (first, len) = (
                    leg.routes.first.get(r as usize).copied().unwrap_or(0) as usize,
                    leg.routes.len.get(r as usize).copied().unwrap_or(0) as usize,
                );
                for p in first..first + len {
                    if let (Some(&la), Some(&lo)) = (
                        leg.route_points.lat_e7.get(p),
                        leg.route_points.lon_e7.get(p),
                    ) {
                        if la != NO_COORD && lo != NO_COORD {
                            v.push(place_of_e7(la, lo));
                        }
                    }
                }
            }
            if v.is_empty() {
                v = thin(path.iter().map(|q| Place {
                    lat_deg: q.lat_deg,
                    lon_deg: q.lon_deg,
                }));
            } else {
                v = thin(v.into_iter());
            }
            v
        };
        let extra = Extra {
            by_callsign: by_callsign(outcome),
            ..Extra::default()
        };
        for &r in routes {
            let issue = ts(r);
            let (first, len) = (
                leg.routes.first.get(r as usize).copied().unwrap_or(0) as usize,
                leg.routes.len.get(r as usize).copied().unwrap_or(0) as usize,
            );
            for p in first..first + len {
                let est = leg.route_points.est_ms.get(p).copied().unwrap_or(NO_TIME);
                let (lat, lon) = (
                    leg.route_points.lat_e7.get(p).copied().unwrap_or(NO_COORD),
                    leg.route_points.lon_e7.get(p).copied().unwrap_or(NO_COORD),
                );
                let result = if issue == NO_TIME {
                    Err(Exclusion::NoIssueTime)
                } else if est == NO_TIME {
                    Err(Exclusion::NoEstimate)
                } else {
                    let mut picked = false;
                    let placed = if lat != NO_COORD && lon != NO_COORD {
                        Ok(place_of_e7(lat, lon))
                    } else {
                        let fix = leg.route_points.fix.get(p).copied().unwrap_or([0; 8]);
                        point_result(ctx.table, name(&fix), route_refs).map(|(pl, pk)| {
                            picked = pk;
                            pl
                        })
                    };
                    match placed {
                        Err(x) => Err(x),
                        Ok(_) if path.is_empty() => Err(Exclusion::NoTruthPath),
                        Ok(place) => {
                            if picked {
                                if let Some(k) = sys.kinds.first_mut() {
                                    k.resolved_ambiguous += 1;
                                }
                            }
                            let a = approach_of(&mut searches, sys, &path, place, &ctx.cfg);
                            score_cross(a, est, issue, &ctx.cfg)
                        }
                    }
                };
                if let Some(k) = sys.kinds.first_mut() {
                    kind_add_with(k, f, result, extra);
                }
            }
        }
    }
}

fn legs_of_unit(ctx: &Ctx<'_>, addr: u32, u: &Unit) -> Vec<u32> {
    legs_of(ctx.ix, addr, u)
}

/// TBFM: the meter-fix and runway estimates and the scheduled times, against the crossing of the
/// meter fix and the landing.
fn tbfm(leg: &Legacy, ctx: &Ctx<'_>, units: &[TableUnit], sys: &mut SystemReport) {
    let kinds = [
        tbfm_kind::ETA_MFX,
        tbfm_kind::STA_SFX,
        tbfm_kind::ETA_RWY,
        tbfm_kind::STA_RWY,
    ];
    for (uix, u) in units.iter().enumerate() {
        if let Some(c) = sys.outcomes.get_mut(outcome_index(u.outcome)) {
            *c += 1;
        }
        let addr = joined_addr(u.outcome);
        let mut path: Option<Vec<PathPoint>> = None;
        let mut searches = Searches::default();
        let mut lands: BTreeMap<String, AirportLandings> = BTreeMap::new();
        for &r in &u.rows {
            let ri = r as usize;
            let kind = leg.tbfm.kind.get(ri).copied().unwrap_or(0);
            let Some(slot) = kinds.iter().position(|&k| k == kind) else {
                continue;
            };
            let issue = leg.tbfm.issue_ms.get(ri).copied().unwrap_or(NO_TIME);
            let est = leg.tbfm.time_ms.get(ri).copied().unwrap_or(NO_TIME);
            let result = if addr.is_none() {
                Err(Exclusion::NotJoined)
            } else if issue == NO_TIME {
                Err(Exclusion::NoIssueTime)
            } else if est == NO_TIME {
                Err(Exclusion::NoEstimate)
            } else if let (Some(a), true) = (addr, slot < 2) {
                let fix = leg.tbfm.fix.get(ri).copied().unwrap_or([0; 8]);
                let nm = name(&fix);
                if point_places(ctx.table, nm).is_empty() {
                    Err(Exclusion::PlaceUnknown)
                } else {
                    let p = path.get_or_insert_with(|| {
                        sample_path(
                            ctx,
                            a,
                            &legs_of_unit(ctx, a, &u.unit),
                            u.unit.from_ms,
                            u.unit.to_ms,
                        )
                    });
                    if p.is_empty() {
                        Err(Exclusion::NoTruthPath)
                    } else {
                        let refs = || {
                            thin(p.iter().map(|q| Place {
                                lat_deg: q.lat_deg,
                                lon_deg: q.lon_deg,
                            }))
                        };
                        match point_result(ctx.table, nm, refs) {
                            Err(x) => Err(x),
                            Ok((place, picked)) => {
                                if picked {
                                    if let Some(k) = sys.kinds.get_mut(slot) {
                                        k.resolved_ambiguous += 1;
                                    }
                                }
                                let ap = approach_of(&mut searches, sys, p, place, &ctx.cfg);
                                score_cross(ap, est, issue, &ctx.cfg)
                                    .map(|sm| (sm, Extra::default()))
                            }
                        }
                    }
                }
            } else if let Some(a) = addr {
                let actual_arr = ctx.tfms_actual.get(&u.plan).copied();
                landing_result(
                    ctx,
                    a,
                    u.arrival.as_deref(),
                    &mut lands,
                    est,
                    issue,
                    actual_arr,
                )
                .map(|(sm, x, pk)| {
                    if pk {
                        if let Some(k) = sys.kinds.get_mut(slot) {
                            k.resolved_ambiguous += 1;
                        }
                    }
                    (sm, x)
                })
            } else {
                Err(Exclusion::NotJoined)
            };
            if let Some(k) = sys.kinds.get_mut(slot) {
                add_result(k, uix as u32, result, by_callsign(u.outcome));
            }
        }
    }
}

/// Adds a scored estimate, with the diagnostics it carries, or its exclusion.
fn add_result(
    k: &mut crate::core::report::KindTable,
    unit: u32,
    r: Result<(Sample, Extra), Exclusion>,
    cs: Option<bool>,
) {
    match r {
        Ok((sm, mut x)) => {
            x.by_callsign = cs;
            kind_add_with(k, unit, Ok(sm), x);
        }
        Err(e) => kind_add(k, unit, Err(e)),
    }
}

/// An airport of a flight: its place (and whether one of several was taken), and the landings of
/// the aircraft near it.
type AirportLandings = Result<(bool, Vec<Landing>), Exclusion>;

/// An estimate of a landing time against the landing of the aircraft at the airport. `actual_arr`
/// is the TFMS actual arrival time of the flight, if it has one (a diagnostic: the same estimate
/// is also scored against it). Returns the sample, its diagnostics, and whether the airport had
/// several places and one was taken.
fn landing_result(
    ctx: &Ctx<'_>,
    addr: u32,
    arrival: Option<&str>,
    cache: &mut BTreeMap<String, AirportLandings>,
    est: i64,
    issue: i64,
    actual_arr: Option<i64>,
) -> Result<(Sample, Extra, bool), Exclusion> {
    let Some(apt) = arrival else {
        return Err(Exclusion::AirportUnknown);
    };
    let entry = cache.entry(apt.to_string()).or_insert_with(|| {
        let states = ctx.states.get(&addr).map_or(&[][..], Vec::as_slice);
        let refs = thin(
            states
                .iter()
                .map(|s| Place {
                    lat_deg: s.lat_deg,
                    lon_deg: s.lon_deg,
                })
                .collect::<Vec<_>>()
                .into_iter(),
        );
        match resolve(airport_places(ctx.table, apt), &refs) {
            Resolution::NotFound => Err(Exclusion::AirportUnknown),
            Resolution::Refused => Err(Exclusion::PlaceAmbiguous),
            Resolution::Unique(arp) => Ok((false, landings(states, arp, &ctx.cfg))),
            Resolution::Picked(arp) => Ok((true, landings(states, arp, &ctx.cfg))),
        }
    });
    let (picked, l) = match entry {
        Ok((p, l)) => (*p, l),
        Err(e) => return Err(*e),
    };
    let landing = choose_landing(l, issue, &ctx.cfg)?;
    let sm = score(est, landing.t_ms, issue, &ctx.cfg)?;
    let alt = actual_arr.map(|t| {
        (
            (est - t) as f64 / 1000.0,
            (landing.t_ms - t) as f64 / 1000.0,
        )
    });
    let x = Extra {
        gap_s: Some(landing.gap_ms as f64 / 1000.0),
        alt,
        by_callsign: None,
    };
    Ok((sm, x, picked))
}

/// TFMS: the arrival estimates, against the landing at the arrival airport.
fn tfms(leg: &Legacy, ctx: &Ctx<'_>, units: &[TableUnit], sys: &mut SystemReport) {
    for (uix, u) in units.iter().enumerate() {
        if let Some(c) = sys.outcomes.get_mut(outcome_index(u.outcome)) {
            *c += 1;
        }
        let addr = joined_addr(u.outcome);
        let mut lands: BTreeMap<String, AirportLandings> = BTreeMap::new();
        let actual_arr = ctx.tfms_actual.get(&u.plan).copied();
        for &r in &u.rows {
            let ri = r as usize;
            let issue = leg.tfms.issue_ms.get(ri).copied().unwrap_or(NO_TIME);
            let est = leg.tfms.time_ms.get(ri).copied().unwrap_or(NO_TIME);
            let result = match addr {
                None => Err(Exclusion::NotJoined),
                Some(_) if issue == NO_TIME => Err(Exclusion::NoIssueTime),
                Some(_) if est == NO_TIME => Err(Exclusion::NoEstimate),
                Some(a) => landing_result(
                    ctx,
                    a,
                    u.arrival.as_deref(),
                    &mut lands,
                    est,
                    issue,
                    actual_arr,
                )
                .map(|(sm, x, pk)| {
                    if pk {
                        if let Some(k) = sys.kinds.first_mut() {
                            k.resolved_ambiguous += 1;
                        }
                    }
                    (sm, x)
                }),
            };
            if let Some(k) = sys.kinds.first_mut() {
                add_result(k, uix as u32, result, by_callsign(u.outcome));
            }
        }
    }
}

/// What the scoring reads. Cold container (A7.2).
// A7.2: cold struct, size guard waived
pub struct Inputs<'a> {
    pub legacy: &'a Legacy,
    pub built: &'a BuiltTruth,
    pub cfg: Config,
    /// The step of the truth path sampling, ms.
    pub step_ms: i64,
    /// Where the named points and airports are.
    pub table: PlaceTable,
}

/// What the scoring produced. Cold container (A7.2).
// A7.2: cold struct, size guard waived
pub struct Scored {
    /// ERAM, TBFM and TFMS, in that order.
    pub systems: [SystemReport; 3],
    /// SFDPS flights in all, and the ones with an expanded route.
    pub sfdps_flights: (u64, u64),
    /// TFMS departure estimate rows, which are not scored.
    pub etd_rows: u64,
    /// The match rates of the names against the table.
    pub sections: Vec<Section>,
}

/// How many of the most frequent unresolved names are listed.
const TOP_UNRESOLVED: usize = 20;

/// The match of the names in `occ` (name and how many times it occurs) against the table.
fn match_section(
    title: &str,
    what: &str,
    occ: &BTreeMap<String, u64>,
    class_of: impl Fn(&str) -> usize,
) -> Section {
    let mut names = [0u64; 4];
    let mut times = [0u64; 4];
    let mut bad: Vec<(u64, &String, usize)> = Vec::new();
    for (nm, &n) in occ {
        let c = class_of(nm);
        if let (Some(a), Some(b)) = (names.get_mut(c), times.get_mut(c)) {
            *a += 1;
            *b += n;
        }
        if c == 0 || c == 3 {
            bad.push((n, nm, c));
        }
    }
    let (tn, tt): (u64, u64) = (names.iter().sum(), times.iter().sum());
    let pct = |a: u64, b: u64| {
        if b == 0 {
            "n/a".to_string()
        } else {
            format!("{:.1}%", 100.0 * a as f64 / b as f64)
        }
    };
    let mut lines = vec![format!("{what}: {tn} distinct, {tt} occurrences")];
    for (k, c) in CLASS_NAMES.iter().enumerate() {
        let (a, b) = (
            names.get(k).copied().unwrap_or(0),
            times.get(k).copied().unwrap_or(0),
        );
        lines.push(format!(
            "  {c}: {a} names ({}), {b} occurrences ({})",
            pct(a, tn),
            pct(b, tt)
        ));
    }
    let resolved = |v: &[u64; 4]| v.get(1).copied().unwrap_or(0) + v.get(2).copied().unwrap_or(0);
    lines.push(format!(
        "  resolvable: {} names ({}), {} occurrences ({})",
        resolved(&names),
        pct(resolved(&names), tn),
        resolved(&times),
        pct(resolved(&times), tt)
    ));
    bad.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    lines.push(format!(
        "  the {TOP_UNRESOLVED} most frequent names not resolvable (occurrences, name, class):"
    ));
    for (n, nm, c) in bad.iter().take(TOP_UNRESOLVED) {
        lines.push(format!(
            "    {n} {nm} ({})",
            CLASS_NAMES.get(*c).copied().unwrap_or("?")
        ));
    }
    Section {
        title: title.to_string(),
        lines,
    }
}

/// The match rates of the three name sets of the recordings against the table.
fn match_sections(leg: &Legacy, table: &PlaceTable) -> Vec<Section> {
    let mut eram: BTreeMap<String, u64> = BTreeMap::new();
    let (mut with_coord, mut total) = (0u64, 0u64);
    for (i, fx) in leg.route_points.fix.iter().enumerate() {
        total += 1;
        let has = leg
            .route_points
            .lat_e7
            .get(i)
            .is_some_and(|&v| v != NO_COORD)
            && leg
                .route_points
                .lon_e7
                .get(i)
                .is_some_and(|&v| v != NO_COORD);
        if has {
            with_coord += 1;
        } else if !name(fx).is_empty() {
            *eram.entry(name(fx).to_ascii_uppercase()).or_default() += 1;
        }
    }
    let mut tb: BTreeMap<String, u64> = BTreeMap::new();
    for (i, k) in leg.tbfm.kind.iter().enumerate() {
        if *k == tbfm_kind::ETA_MFX || *k == tbfm_kind::STA_SFX {
            let nm = leg.tbfm.fix.get(i).map_or("", |f| name(f));
            if !nm.is_empty() {
                *tb.entry(nm.to_ascii_uppercase()).or_default() += 1;
            }
        }
    }
    let mut tf: BTreeMap<String, u64> = BTreeMap::new();
    for (i, k) in leg.tfms.kind.iter().enumerate() {
        if *k == tfms_kind::ETA {
            let c = leg.tfms.arr.get(i).map_or_else(String::new, |a| key_of(a));
            if !c.is_empty() {
                *tf.entry(c).or_default() += 1;
            }
        }
    }
    let mut out = vec![match_section(
        "match rates: ERAM route-point names (points without coordinates in the message)",
        "name-only route points",
        &eram,
        |n| classify(point_places(table, n)),
    )];
    out.push(Section {
        title: "ERAM route points by coordinates".to_string(),
        lines: vec![format!(
            "{total} route points, {with_coord} carry coordinates in the message, {} are name only",
            total - with_coord
        )],
    });
    out.push(match_section(
        "match rates: TBFM meter-fix and scheduling-fix names (eta_mfx, sta_sfx rows)",
        "fix names",
        &tb,
        |n| classify(point_places(table, n)),
    ));
    out.push(match_section(
        "match rates: TFMS arrival codes (arrival estimate rows)",
        "arrival codes",
        &tf,
        |n| classify(airport_places(table, n)),
    ));
    out
}

/// Scores the three systems. `reports` is the ADS-B clip the truth was built from; it is taken
/// and let go once the ground-state reports are read from it.
pub fn score_all(inp: &Inputs<'_>, reports: Reports) -> Scored {
    let leg = inp.legacy;
    let ix = index_legs(&core_legs(inp.built));
    let plans = plan_index(leg);
    let mut tfms_arr: BTreeMap<[u8; 12], String> = BTreeMap::new();
    for (p, a) in leg.tfms.plan_id.iter().zip(&leg.tfms.arr) {
        let k = key_of(a);
        if !k.is_empty() {
            tfms_arr.entry(norm_id::<12>(p)).or_insert(k);
        }
    }
    let mut tfms_actual: BTreeMap<[u8; 12], (i64, i64)> = BTreeMap::new();
    for (i, p) in leg.tfms_actual.plan_id.iter().enumerate() {
        let (t, iss) = (
            leg.tfms_actual.time_ms.get(i).copied().unwrap_or(NO_TIME),
            leg.tfms_actual.issue_ms.get(i).copied().unwrap_or(NO_TIME),
        );
        let e = tfms_actual.entry(norm_id::<12>(p)).or_insert((t, iss));
        if iss > e.1 {
            *e = (t, iss);
        }
    }
    let tfms_actual: BTreeMap<[u8; 12], i64> =
        tfms_actual.into_iter().map(|(k, (t, _))| (k, t)).collect();
    let tb_units = tbfm_units(leg, &plans, &ix, &tfms_arr);
    let tf_units = tfms_units(leg, &plans, &ix);
    let want: BTreeSet<u32> = tb_units
        .iter()
        .chain(tf_units.iter())
        .filter_map(|u| joined_addr(u.outcome))
        .collect();
    let states = air_states(&reports, &want);
    drop(reports);
    let ctx = Ctx {
        cfg: inp.cfg,
        step_ms: inp.step_ms,
        built: inp.built,
        ix: &ix,
        table: &inp.table,
        states: &states,
        tfms_actual: &tfms_actual,
    };
    let mut e = system_new("ERAM", &["route point time"]);
    let mut counts = (0u64, 0u64);
    eram(leg, &ctx, &mut e, &mut counts);
    let mut b = system_new("TBFM", &["eta_mfx", "sta_sfx", "eta_rwy", "sta_rwy"]);
    tbfm(leg, &ctx, &tb_units, &mut b);
    let mut t = system_new("TFMS", &["eta"]);
    tfms(leg, &ctx, &tf_units, &mut t);
    Scored {
        systems: [e, b, t],
        sfdps_flights: counts,
        etd_rows: leg
            .tfms
            .kind
            .iter()
            .filter(|&&k| k == tfms_kind::ETD)
            .count() as u64,
        sections: match_sections(leg, &inp.table),
    }
}

/// The run: reads the recordings and the clips, writes the report and the CSV files under
/// `args.out`, and returns the report text.
pub fn run_counted(args: &Args, sealed: &mut u64) -> Result<String, String> {
    let cfg = Config {
        cut_m: args.cut_nm * 1852.0,
        runway_radius_m: args.runway_nm * 1852.0,
        landing_gap_max_ms: args.landing_gap_s * 1000,
        max_look_ahead_ms: MAX_LOOK_AHEAD_MIN * MS_PER_MIN,
    };
    let mut table = PlaceTable::default();
    let mut nasr_line = "none given: no point or airport has a place".to_string();
    if let Some(z) = &args.nasr {
        let (t, sha) = load_nasr(z)?;
        nasr_line = format!(
            "{}: {} point names, {} airport codes, {} rows rejected by the parser, sha256 {sha}",
            z.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            t.points.len(),
            t.airports.len(),
            t.rejected
        );
        table = t;
    }
    let places = match &args.places {
        Some(p) => {
            let bytes = read_capped_file(p, MAX_PLACES_BYTES).map_err(|e| e.to_string())?;
            let text =
                String::from_utf8(bytes).map_err(|_| "places file is not UTF-8".to_string())?;
            parse_places(&text)?
        }
        None => PlacesFile::default(),
    };
    apply_places(&mut table, &places);
    let mut header: Vec<(String, String)> = Vec::new();
    header.push(("nasr".to_string(), nasr_line));
    let mut reports = Reports::default();
    let mut clip_sealed = 0u64;
    for p in &args.adsb {
        let mut s = 0u64;
        let clip = load_adsb_counted(p, &guarded_opener(), &mut s);
        clip_sealed += s;
        *sealed = clip_sealed;
        let clip = clip.map_err(|e| e.to_string())?;
        header.push((
            "adsb clip".to_string(),
            format!(
                "{}: {} reports, {} bad frames, {} dropped records, sha256 {}",
                p.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                reports_len(&clip.reports),
                clip.bad_frames,
                clip.dropped_records,
                clip.hash.sha256
            ),
        ));
        if reports_len(&reports) == 0 {
            reports = clip.reports;
        } else {
            reports_extend(&mut reports, &clip.reports);
        }
        eprintln!(
            "oa-legacy-traj: clip read, {} reports so far",
            reports_len(&reports)
        );
    }
    let mut tcfg = adsb_truth::config_with(None, None);
    // The leave-one-out accuracy measurement is the truth's own check and is not needed here.
    tcfg.loo = false;
    let built = adsb_truth::build(&reports, &tcfg).map_err(|e| format!("truth: {e}"))?;
    eprintln!("oa-legacy-traj: truth built, {} legs", built.legs.len());
    header.push((
        "truth".to_string(),
        format!("{} legs, digest {}", built.legs.len(), built.digest),
    ));
    let feeds = [
        FeedInput {
            feed: FeedKind::Sfdps,
            dir: args.sfdps.clone(),
        },
        FeedInput {
            feed: FeedKind::Tbfm,
            dir: args.tbfm.clone(),
        },
        FeedInput {
            feed: FeedKind::Tfms,
            dir: args.tfms.clone(),
        },
    ];
    let mut feeds_sealed = 0u64;
    let loaded = load_counted(&feeds, &guarded_opener(), msg_limit(), &mut feeds_sealed);
    *sealed += feeds_sealed;
    let mut loaded = loaded.map_err(|e| e.to_string())?;
    eprintln!("oa-legacy-traj: recordings read");
    for h in &loaded.inputs {
        header.push((
            "input".to_string(),
            format!(
                "{}: {} files, {} bytes, sha256 {}",
                h.label, h.files, h.bytes, h.sha256
            ),
        ));
    }
    header.push(("replay state hash".to_string(), loaded.replay_hash.clone()));
    let l = &mut loaded.legacy;
    header.push((
        "legacy rows".to_string(),
        format!(
            "{} SFDPS flights, {} expanded routes with {} points, {} TBFM values, {} TFMS values",
            l.flights.gufi.len(),
            l.routes.flight.len(),
            l.route_points.fix.len(),
            l.tbfm.kind.len(),
            l.tfms.kind.len()
        ),
    ));
    // Not read here: let go of the track samples, handoffs and beacon changes.
    l.th = Default::default();
    l.handoffs = Default::default();
    l.beacons = Default::default();
    header.push((
        "places file".to_string(),
        format!(
            "{} named points, {} airports",
            places.fixes.len(),
            places.airports.len()
        ),
    ));
    let scored = score_all(
        &Inputs {
            legacy: &loaded.legacy,
            built: &built,
            cfg,
            step_ms: args.step_s * 1000,
            table,
        },
        reports,
    );
    header.push((
        "SFDPS flights".to_string(),
        format!(
            "{} in all, {} with an expanded route (the flights ERAM is scored on)",
            scored.sfdps_flights.0, scored.sfdps_flights.1
        ),
    ));
    header.push((
        "TFMS departure estimates".to_string(),
        format!("{} rows, not scored (the task is arrival)", scored.etd_rows),
    ));
    header.push((
        "sealed-window rows dropped".to_string(),
        (*sealed).to_string(),
    ));
    let text = render_text(&header, &cfg, &scored.sections, &scored.systems);
    let mut files = vec![("report.txt".to_string(), text.clone())];
    for s in &scored.systems {
        let n = s.name.to_ascii_lowercase();
        files.push((format!("{n}.csv"), render_csv(s)));
        files.push((format!("{n}-counts.csv"), render_counts_csv(s)));
    }
    write_all(&args.out, &files)?;
    Ok(text)
}

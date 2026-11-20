//! The run: recordings, clip, truth, the join, the scoring and the files. Shell (B3).

use std::collections::{BTreeMap, BTreeSet};

use oa_adsb::{reports_extend, reports_len, Reports, F_GROUND, NONE_I32};
use oa_bench::adsb_truth::{self, BuiltTruth};
use oa_bench::input::{load_adsb_counted, load_counted, msg_limit, FeedInput};
use oa_bench::legacy::{
    name, norm_id, plan_candidates, plan_index, tbfm_kind, tfms_kind, Legacy, PlanIndex, NONE,
    NO_COORD, NO_TIME,
};
use oa_replay::FeedKind;

use crate::core::bins::score;
use crate::core::cross::{approach, choose_landing, choose_passage, landings};
use crate::core::join::{index_legs, join, legs_of, outcome_index, Leg, LegIndex, Outcome, Unit};
use crate::core::report::{
    closest_add, kind_add, kind_exclude, render_counts_csv, render_csv, render_text, system_new,
    SystemReport,
};
use crate::core::types::{
    AirState, Approach, Config, Exclusion, Landing, PathPoint, Place, Sample, NO_ADDR,
};
use crate::shell::args::Args;
use crate::shell::guard::guarded_opener;
use crate::shell::places::{
    book_airport, book_counts, derive_airports, find, place_book, place_of_e7, Found, PlaceBook,
};
use crate::shell::write::write_all;

const MS_PER_MIN: i64 = 60_000;
/// The longest look-ahead scored, minutes: the end of the last bin.
const MAX_LOOK_AHEAD_MIN: i64 = 120;

/// Everything the scoring reads, besides the estimates.
// A7.2: cold struct, size guard waived
struct Ctx<'a> {
    cfg: Config,
    step_ms: i64,
    built: &'a BuiltTruth,
    ix: &'a LegIndex,
    book: &'a PlaceBook,
    states: &'a BTreeMap<u32, Vec<AirState>>,
    arps: &'a BTreeMap<String, Place>,
}

/// The airport code as the tables compare it: upper case, with the K of a four-letter contiguous
/// US code dropped, so `KORD` and `ORD` are one airport.
pub fn airport_key(code: &[u8]) -> String {
    let s: String = name(code).trim().to_ascii_uppercase();
    match s.strip_prefix('K') {
        Some(rest) if s.len() == 4 => rest.to_string(),
        _ => s,
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
        .map(|a| airport_key(a))
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
            .map(|a| airport_key(a))
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
                    match point_place(leg, ctx, p, lat, lon) {
                        Err(x) => Err(x),
                        Ok(place) if path.is_empty() => {
                            let _ = place;
                            Err(Exclusion::NoTruthPath)
                        }
                        Ok(place) => {
                            let a = approach_of(&mut searches, sys, &path, place, &ctx.cfg);
                            score_cross(a, est, issue, &ctx.cfg)
                        }
                    }
                };
                if let Some(k) = sys.kinds.first_mut() {
                    kind_add(k, f, result);
                }
            }
        }
    }
}

fn legs_of_unit(ctx: &Ctx<'_>, addr: u32, u: &Unit) -> Vec<u32> {
    legs_of(ctx.ix, addr, u)
}

/// The place of route point `p`: its own coordinates, else the place the name has in the book.
fn point_place(
    leg: &Legacy,
    ctx: &Ctx<'_>,
    p: usize,
    lat: i32,
    lon: i32,
) -> Result<Place, Exclusion> {
    if lat != NO_COORD && lon != NO_COORD {
        return Ok(place_of_e7(lat, lon));
    }
    let fix = leg.route_points.fix.get(p).copied().unwrap_or([0; 8]);
    match find(ctx.book, &fix) {
        Found::At(pl) => Ok(pl),
        Found::Unknown => Err(Exclusion::PlaceUnknown),
        Found::Ambiguous => Err(Exclusion::PlaceAmbiguous),
    }
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
        let mut lands: BTreeMap<String, Vec<Landing>> = BTreeMap::new();
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
                match find(ctx.book, &fix) {
                    Found::Unknown => Err(Exclusion::PlaceUnknown),
                    Found::Ambiguous => Err(Exclusion::PlaceAmbiguous),
                    Found::At(place) => {
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
                            let ap = approach_of(&mut searches, sys, p, place, &ctx.cfg);
                            score_cross(ap, est, issue, &ctx.cfg)
                        }
                    }
                }
            } else if let Some(a) = addr {
                landing_result(ctx, a, u.arrival.as_deref(), &mut lands, est, issue)
            } else {
                Err(Exclusion::NotJoined)
            };
            if let Some(k) = sys.kinds.get_mut(slot) {
                kind_add(k, uix as u32, result);
            }
        }
    }
}

/// An estimate of a landing time against the landing of the aircraft at the airport.
fn landing_result(
    ctx: &Ctx<'_>,
    addr: u32,
    arrival: Option<&str>,
    cache: &mut BTreeMap<String, Vec<Landing>>,
    est: i64,
    issue: i64,
) -> Result<Sample, Exclusion> {
    let Some(apt) = arrival else {
        return Err(Exclusion::AirportUnknown);
    };
    let Some(&arp) = ctx.arps.get(apt) else {
        return Err(Exclusion::AirportUnknown);
    };
    let l = cache.entry(apt.to_string()).or_insert_with(|| {
        ctx.states
            .get(&addr)
            .map_or_else(Vec::new, |s| landings(s, arp, &ctx.cfg))
    });
    let actual = choose_landing(l, issue, &ctx.cfg)?;
    score(est, actual, issue, &ctx.cfg)
}

/// TFMS: the arrival estimates, against the landing at the arrival airport.
fn tfms(leg: &Legacy, ctx: &Ctx<'_>, units: &[TableUnit], sys: &mut SystemReport) {
    for (uix, u) in units.iter().enumerate() {
        if let Some(c) = sys.outcomes.get_mut(outcome_index(u.outcome)) {
            *c += 1;
        }
        let addr = joined_addr(u.outcome);
        let mut lands: BTreeMap<String, Vec<Landing>> = BTreeMap::new();
        for &r in &u.rows {
            let ri = r as usize;
            let issue = leg.tfms.issue_ms.get(ri).copied().unwrap_or(NO_TIME);
            let est = leg.tfms.time_ms.get(ri).copied().unwrap_or(NO_TIME);
            let result = match addr {
                None => Err(Exclusion::NotJoined),
                Some(_) if issue == NO_TIME => Err(Exclusion::NoIssueTime),
                Some(_) if est == NO_TIME => Err(Exclusion::NoEstimate),
                Some(a) => landing_result(ctx, a, u.arrival.as_deref(), &mut lands, est, issue),
            };
            if let Some(k) = sys.kinds.first_mut() {
                kind_add(k, uix as u32, result);
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
    /// The airport reference points by [`airport_key`], or `None` to derive them from the
    /// recordings.
    pub arps: Option<BTreeMap<String, Place>>,
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
    /// Names with one place in the book, names with several.
    pub places: (u64, u64),
    /// Airport reference points used.
    pub arps: usize,
}

/// Scores the three systems. `reports` is the ADS-B clip the truth was built from; it is taken
/// and let go once the ground-state reports are read from it.
pub fn score_all(inp: &Inputs<'_>, reports: Reports) -> Scored {
    let leg = inp.legacy;
    let ix = index_legs(&core_legs(inp.built));
    let book = place_book(leg);
    let plans = plan_index(leg);
    let mut tfms_arr: BTreeMap<[u8; 12], String> = BTreeMap::new();
    for (p, a) in leg.tfms.plan_id.iter().zip(&leg.tfms.arr) {
        let k = airport_key(a);
        if !k.is_empty() {
            tfms_arr.entry(norm_id::<12>(p)).or_insert(k);
        }
    }
    let tb_units = tbfm_units(leg, &plans, &ix, &tfms_arr);
    let tf_units = tfms_units(leg, &plans, &ix);
    let want: BTreeSet<u32> = tb_units
        .iter()
        .chain(tf_units.iter())
        .filter_map(|u| joined_addr(u.outcome))
        .collect();
    let states = air_states(&reports, &want);
    drop(reports);
    let arps = match &inp.arps {
        Some(a) => a.clone(),
        None => {
            let flights = arrival_flights(&tb_units, &tf_units);
            let mut a = derive_airports(&flights, &states);
            for (apt, ..) in &flights {
                if let Some(p) = book_airport(&book, apt) {
                    a.insert(apt.clone(), p);
                }
            }
            a
        }
    };
    let ctx = Ctx {
        cfg: inp.cfg,
        step_ms: inp.step_ms,
        built: inp.built,
        ix: &ix,
        book: &book,
        states: &states,
        arps: &arps,
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
        places: book_counts(&book),
        arps: arps.len(),
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
    *sealed = feeds_sealed;
    let mut loaded = loaded.map_err(|e| e.to_string())?;
    let mut header: Vec<(String, String)> = Vec::new();
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
    let mut reports = Reports::default();
    let mut clip_sealed = 0u64;
    for p in &args.adsb {
        let mut s = 0u64;
        let clip = load_adsb_counted(p, &guarded_opener(), &mut s);
        clip_sealed += s;
        *sealed = feeds_sealed + clip_sealed;
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
        reports_extend(&mut reports, &clip.reports);
    }
    let mut tcfg = adsb_truth::config_with(None, None);
    // The leave-one-out accuracy measurement is the truth's own check and is not needed here.
    tcfg.loo = false;
    let built = adsb_truth::build(&reports, &tcfg).map_err(|e| format!("truth: {e}"))?;
    header.push((
        "truth".to_string(),
        format!("{} legs, digest {}", built.legs.len(), built.digest),
    ));
    let scored = score_all(
        &Inputs {
            legacy: &loaded.legacy,
            built: &built,
            cfg,
            step_ms: args.step_s * 1000,
            arps: None,
        },
        reports,
    );
    header.push((
        "named points".to_string(),
        format!(
            "{} names with one place from route coordinates, {} with several",
            scored.places.0, scored.places.1
        ),
    ));
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
        "airport reference points".to_string(),
        format!("{} derived from the recordings", scored.arps),
    ));
    header.push((
        "sealed-window rows dropped".to_string(),
        (*sealed).to_string(),
    ));
    let text = render_text(&header, &cfg, &scored.systems);
    let mut files = vec![("report.txt".to_string(), text.clone())];
    for s in &scored.systems {
        let n = s.name.to_ascii_lowercase();
        files.push((format!("{n}.csv"), render_csv(s)));
        files.push((format!("{n}-counts.csv"), render_counts_csv(s)));
    }
    write_all(&args.out, &files)?;
    Ok(text)
}

/// The arrival airport of every joined flight with its aircraft and interval, for deriving
/// reference points.
fn arrival_flights(a: &[TableUnit], b: &[TableUnit]) -> Vec<(String, u32, i64, i64)> {
    a.iter()
        .chain(b.iter())
        .filter_map(|u| {
            Some((
                u.arrival.clone()?,
                joined_addr(u.outcome)?,
                u.unit.from_ms,
                u.unit.to_ms,
            ))
        })
        .collect()
}

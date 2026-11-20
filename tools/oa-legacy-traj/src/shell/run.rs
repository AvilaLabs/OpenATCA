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

use crate::core::bins::{lead_bin, lead_in_range, score};
use crate::core::cross::{
    airborne_span, approach, choose_landing, choose_passage, great_circle_m, landings,
    near_box_edge, pick_passage, unsplit_choice_ms,
};
use crate::core::join::{
    all_legs_of, bind_leg, index_legs, join, leg_in_window, legs_of, outcome_index, Binding, Known,
    Leg, LegFacts, LegIndex, Miss, Outcome, Refused, Unit,
};
use crate::core::places::{classify, resolve, Resolution, CLASS_NAMES};
use crate::core::report::{
    abeam_add, airport_offer, airport_scored, closest_add, count, group_of, kind_record,
    render_counts_csv, render_csv, render_text, scored_flights, sens_add, system_new, Extra,
    KindTable, Section, SystemReport, GROUPS, GROUP_NAMES,
};
use crate::core::types::{
    exclusion_index, AirState, Approach, Config, Exclusion, LandingSet, PathPoint, Place, Sample,
    EXCLUSION_COUNT, EXCLUSION_NAMES, NO_ADDR, NO_ALT, NO_SPEED,
};
use crate::shell::args::Args;
use crate::shell::guard::guarded_opener;
use crate::shell::places::{
    airport_elev_ft, airport_key, airport_places, apply_places, load_nasr, parse_places,
    place_of_e7, point_places, PlaceTable, PlacesFile,
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
    /// The crossing cuts searched, metres: the sensitivity cuts and the headline cut.
    cuts_m: Vec<f64>,
    /// The index of the headline cut in `cuts_m`.
    primary: usize,
    step_ms: i64,
    built: &'a BuiltTruth,
    ix: &'a LegIndex,
    table: &'a PlaceTable,
    states: &'a BTreeMap<u32, Vec<AirState>>,
    /// The TFMS actual arrival time by plan identifier, Unix ms (the latest issued).
    tfms_actual: &'a BTreeMap<[u8; 12], i64>,
    /// Where each truth leg took off and landed.
    leg_ends: &'a [Option<(Place, Place)>],
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

/// The longest stretch of the truth path that is taken as continuous, ms: samples more than this
/// apart are a hole. (The truth answers only within 10 s of a real report on both sides, so with
/// the default 5 s step no two consecutive answered samples are this far apart; the rule only
/// matters for a larger `--step-s`.)
const MAX_SEGMENT_MS: i64 = 30_000;

/// The truth path of leg `leg` of `addr`, sampled every `step_ms`, and the number of segments
/// longer than [`MAX_SEGMENT_MS`] made holes. A time truth has no answer for leaves a hole: the
/// next sample is marked as after one, and so is the first sample.
fn sample_path(ctx: &Ctx<'_>, addr: u32, leg: u32) -> (Vec<PathPoint>, u64) {
    let Some(l) = ctx.ix.legs.get(leg as usize) else {
        return (Vec::new(), 0);
    };
    let mut out: Vec<PathPoint> = Vec::new();
    let mut forced = 0u64;
    let mut hole = true;
    let mut t = l.start_ms;
    while t <= l.end_ms {
        match adsb_truth::query(&ctx.built.truth, addr, t) {
            Ok(s) => {
                if !hole && out.last().is_some_and(|p| t - p.t_ms > MAX_SEGMENT_MS) {
                    hole = true;
                    forced += 1;
                }
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
    (out, forced)
}

/// A key for a place, to keep one search per place of a flight.
fn place_key(p: Place) -> (i64, i64) {
    (
        (p.lat_deg * 1e6).round() as i64,
        (p.lon_deg * 1e6).round() as i64,
    )
}

/// A flight of the TBFM or TFMS tables: its rows, and what it joined to.
// A7.2: cold struct, size guard waived
struct TableUnit {
    unit: Unit,
    outcome: Outcome,
    rows: Vec<u32>,
    /// The airport the flight lands at, as [`airport_key`], when the records name one.
    arrival: Option<String>,
    /// The airport the flight departs from, as [`airport_key`], when the records name one.
    dep: Option<String>,
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

/// The one arrival (`arrival`) or departure airport of the flights of `cands`, when they name
/// exactly one.
fn airport_of(leg: &Legacy, cands: &[u32], arrival: bool) -> Option<String> {
    let set: BTreeSet<String> = cands
        .iter()
        .filter_map(|&f| {
            if arrival {
                leg.flights.arr.get(f as usize)
            } else {
                leg.flights.dep.get(f as usize)
            }
        })
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
    tfms_dep: &BTreeMap<[u8; 12], String>,
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
        let arrival = airport_of(leg, cands, true).or_else(|| tfms_arr.get(&key).cloned());
        let dep = airport_of(leg, cands, false).or_else(|| tfms_dep.get(&key).cloned());
        out.push(TableUnit {
            unit,
            outcome,
            rows,
            arrival,
            dep,
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
        let dep: BTreeSet<String> = rows
            .iter()
            .filter_map(|&r| leg.tfms.dep.get(r as usize))
            .map(|a| key_of(a))
            .filter(|a| !a.is_empty())
            .collect();
        let mut it = dep.iter();
        let dep = match (it.next(), it.next()) {
            (Some(a), None) => Some(a.clone()),
            _ => None,
        };
        out.push(TableUnit {
            unit,
            outcome,
            rows,
            arrival,
            dep,
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
        let alt = [reports.geom_alt_ft.get(i), reports.baro_alt_ft.get(i)]
            .into_iter()
            .flatten()
            .copied()
            .find(|&v| v != NONE_I32)
            .unwrap_or(NO_ALT);
        let gs = reports.gs_dkt.get(i).copied().unwrap_or(NO_SPEED);
        let p = place_of_e7(lat, lon);
        out.entry(a).or_default().push(AirState {
            t_ms: ns.div_euclid(1_000_000),
            lat_deg: p.lat_deg,
            lon_deg: p.lon_deg,
            ground,
            alt_ft: alt,
            gs_dkt: gs,
        });
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| a.t_ms.cmp(&b.t_ms));
    }
    out
}

/// The truth legs' first and last places, by leg index: where each took off and landed.
fn leg_ends(built: &BuiltTruth) -> Vec<Option<(Place, Place)>> {
    built
        .legs
        .iter()
        .map(|l| {
            let at = |t| {
                adsb_truth::query(&built.truth, l.addr, t)
                    .ok()
                    .map(|s| Place {
                        lat_deg: s.lat,
                        lon_deg: s.lon,
                    })
            };
            Some((at(l.start_ms)?, at(l.end_ms)?))
        })
        .collect()
}

/// Whether `p` is within the leg-airport distance of a place of the airport `code`; `None` when
/// the table has no place for the airport.
fn near_airport(ctx: &Ctx<'_>, code: &str, p: Place) -> Option<bool> {
    let c = airport_places(ctx.table, code);
    (!c.is_empty()).then(|| {
        c.iter()
            .any(|a| great_circle_m(*a, p) <= ctx.cfg.leg_airport_m)
    })
}

/// The leg took off at the flight's departure airport (true when the airport is not known): the
/// rule of the earlier rounds, kept only to say what became of the rows it bound.
fn takes_off_ok_old(ctx: &Ctx<'_>, leg: u32, dep: Option<&str>) -> bool {
    let Some(d) = dep else { return true };
    let Some(Some((start, _))) = ctx.leg_ends.get(leg as usize) else {
        return false;
    };
    near_airport(ctx, d, *start).unwrap_or(true)
}

/// Whether the airport has a place in the table.
fn airport_known(ctx: &Ctx<'_>, code: Option<&str>) -> bool {
    code.is_some_and(|c| !airport_places(ctx.table, c).is_empty())
}

/// The reports of the aircraft within the leg, in time order.
fn leg_reports<'b>(ctx: &Ctx<'b>, addr: u32, leg: u32) -> &'b [AirState] {
    let (Some(l), Some(all)) = (ctx.ix.legs.get(leg as usize), ctx.states.get(&addr)) else {
        return &[];
    };
    let lo = all.partition_point(|s| s.t_ms < l.start_ms);
    let hi = all.partition_point(|s| s.t_ms <= l.end_ms);
    all.get(lo..hi).unwrap_or(&[])
}

/// What the callsign join of a flight is refused for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// No leg of the aircraft took off from the departure airport or landed at the arrival one.
    Airport,
    /// No leg of the aircraft starts within the window after the flight's first issue.
    Window,
}

const REFUSAL_KEYS: [&str; 2] = [
    "callsign joins refused: no leg took off at the departure airport or landed at the arrival airport",
    "callsign joins refused: no leg starts within the window after the first issue",
];

/// A callsign join also needs a leg that starts within the window after the first issue (or is in
/// progress at it) and whose take-off airport is the departure airport or landing airport the
/// arrival airport, when those are known.
fn callsign_check(
    ctx: &Ctx<'_>,
    addr: u32,
    unit: &Unit,
    dep: Option<&str>,
    arr: Option<&str>,
) -> Result<(), Refusal> {
    let legs: Vec<u32> = legs_of(ctx.ix, addr, unit)
        .into_iter()
        .filter(|&i| {
            ctx.ix
                .legs
                .get(i as usize)
                .is_some_and(|l| leg_in_window(l, unit.from_ms, ctx.cfg.leg_window_ms))
        })
        .collect();
    if legs.is_empty() {
        return Err(Refusal::Window);
    }
    fn known<'x>(ctx: &Ctx<'_>, a: Option<&'x str>) -> Option<&'x str> {
        a.filter(|c| !airport_places(ctx.table, c).is_empty())
    }
    let (d, r) = (known(ctx, dep), known(ctx, arr));
    if d.is_none() && r.is_none() {
        return Ok(());
    }
    let ok = legs.iter().any(|&i| {
        let Some(Some((start, end))) = ctx.leg_ends.get(i as usize) else {
            return false;
        };
        d.is_some_and(|c| near_airport(ctx, c, *start) == Some(true))
            || r.is_some_and(|c| near_airport(ctx, c, *end) == Some(true))
    });
    if ok {
        Ok(())
    } else {
        Err(Refusal::Airport)
    }
}

/// Applies the extra callsign conditions to a join outcome, counting a refusal; a refused join
/// has no truth.
fn refine(
    ctx: &Ctx<'_>,
    sys: &mut SystemReport,
    o: Outcome,
    unit: &Unit,
    dep: Option<&str>,
    arr: Option<&str>,
) -> Outcome {
    if let Outcome::ByCallsign(a) = o {
        if let Err(r) = callsign_check(ctx, a, unit, dep, arr) {
            count(sys, REFUSAL_KEYS.get(r as usize).copied().unwrap_or("?"), 1);
            return Outcome::NoTruth;
        }
    }
    o
}

/// The airborne-at-issue test of the earlier rounds (an airborne report within this before the
/// issue time, ms), kept only to say what became of the rows it classed.
const OLD_AIRBORNE_MS: i64 = 60_000;

/// One joined flight's scoring state: the leg each message binds to, and the truth paths,
/// crossing searches and landings of those legs, each computed once.
// A7.2: cold struct, size guard waived
struct Flight<'a, 'b> {
    ctx: &'a Ctx<'b>,
    addr: u32,
    /// The airport the flight lands at, as [`airport_key`], when the records name one.
    arr: Option<String>,
    cands: Vec<u32>,
    /// What is known of each of `cands` for this flight.
    facts: Vec<LegFacts>,
    known: Known,
    /// Whether each of `cands` passes the take-off test of the earlier rounds.
    old_ok: Vec<bool>,
    /// By message time: the binding, and the leg the rule without the shuttle test bound.
    binds: BTreeMap<i64, (Binding, Result<u32, Miss>)>,
    /// Segments made holes for being over 30 s long, over this flight's paths.
    long_gaps: u64,
    paths: BTreeMap<u32, Vec<PathPoint>>,
    searches: BTreeMap<(u32, (i64, i64)), Approach>,
    lands: BTreeMap<(u32, String), AirportLandings>,
    /// Whether the leg has any ground report.
    ground_cov: BTreeMap<u32, bool>,
}

impl<'a, 'b> Flight<'a, 'b> {
    fn new(
        ctx: &'a Ctx<'b>,
        addr: u32,
        (dep, arr): (Option<String>, Option<String>),
        callsign: [u8; 8],
    ) -> Self {
        let cands = all_legs_of(ctx.ix, addr);
        let known = Known {
            dep: airport_known(ctx, dep.as_deref()),
            arr: airport_known(ctx, arr.as_deref()),
        };
        let has_cs = callsign.iter().any(|&b| b != 0);
        let facts = cands
            .iter()
            .map(|&i| {
                let ends = ctx.leg_ends.get(i as usize).copied().flatten();
                let near = |code: &Option<String>, k: bool, p: Option<Place>| {
                    k && match (code.as_deref(), p) {
                        (Some(c), Some(p)) => near_airport(ctx, c, p) == Some(true),
                        _ => false,
                    }
                };
                let span = airborne_span(leg_reports(ctx, addr, i), &ctx.cfg);
                let at_edge = |p: Option<Place>| {
                    p.is_some_and(|p| near_box_edge(p, ctx.cfg.box_deg, ctx.cfg.edge_margin_m))
                };
                LegFacts {
                    dep_ok: near(&dep, known.dep, ends.map(|e| e.0)),
                    arr_ok: near(&arr, known.arr, ends.map(|e| e.1)),
                    dep_edge: at_edge(ends.map(|e| e.0)),
                    arr_edge: at_edge(ends.map(|e| e.1)),
                    cs_ok: has_cs
                        && ctx
                            .ix
                            .legs
                            .get(i as usize)
                            .is_some_and(|l| l.callsign == callsign),
                    air_from_ms: span.map_or(i64::MAX, |s| s.0),
                    air_to_ms: span.map_or(i64::MIN, |s| s.1),
                }
            })
            .collect();
        let old_ok = cands
            .iter()
            .map(|&i| takes_off_ok_old(ctx, i, dep.as_deref()))
            .collect();
        Flight {
            ctx,
            addr,
            arr,
            cands,
            facts,
            known,
            old_ok,
            binds: BTreeMap::new(),
            long_gaps: 0,
            paths: BTreeMap::new(),
            searches: BTreeMap::new(),
            lands: BTreeMap::new(),
            ground_cov: BTreeMap::new(),
        }
    }

    /// The one leg a message issued at `issue` binds to, or why none; refusals of a leg in
    /// progress are counted once per message time, and every row whose leg the shuttle test
    /// (both known airports must match) changed is counted.
    fn leg_for(&mut self, sys: &mut SystemReport, issue: i64) -> Result<u32, Exclusion> {
        let (b, loose) = match self.binds.get(&issue) {
            Some(b) => *b,
            None => {
                let ctx = self.ctx;
                let bind = |strict| {
                    bind_leg(
                        ctx.ix,
                        &self.cands,
                        &self.facts,
                        self.known,
                        (issue, ctx.cfg.leg_window_ms),
                        strict,
                    )
                };
                let (b, loose) = (bind(true), bind(false));
                match b.refused {
                    Some(Refused::NotOwn) => count(sys, REFUSED_KEYS[0], 1),
                    Some(Refused::Landed) => count(sys, REFUSED_KEYS[1], 1),
                    None => {}
                }
                self.binds.insert(issue, (b, loose.leg));
                (b, loose.leg)
            }
        };
        if b.leg != loose {
            count(sys, SHUTTLE_KEY, 1);
        }
        b.leg.map_err(|m| match m {
            Miss::NoLeg => Exclusion::NoLeg,
            Miss::NotOwn => Exclusion::LegNotOwn,
            Miss::TooFar => Exclusion::DepartureTooFar,
        })
    }

    /// The leg the rule of the earlier rounds bound a message to: the one in progress, else the
    /// first starting after it within the window whose take-off airport was the departure airport.
    fn old_leg(&self, issue: i64) -> Option<u32> {
        let ctx = self.ctx;
        let get = |i: u32| ctx.ix.legs.get(i as usize);
        self.cands
            .iter()
            .copied()
            .find(|&i| get(i).is_some_and(|l| l.start_ms <= issue && l.end_ms >= issue))
            .or_else(|| {
                self.cands
                    .iter()
                    .zip(&self.old_ok)
                    .find(|(&i, &ok)| {
                        ok && get(i).is_some_and(|l| {
                            l.start_ms > issue && l.start_ms - issue <= ctx.cfg.leg_window_ms
                        })
                    })
                    .map(|(&i, _)| i)
            })
    }

    /// The airborne test of the earlier rounds.
    fn old_airborne(&self, leg: u32, issue: i64) -> bool {
        let st = self.leg_states(leg);
        let hi = st.partition_point(|s| s.t_ms <= issue);
        st.get(..hi)
            .unwrap_or(&[])
            .iter()
            .rev()
            .take_while(|s| s.t_ms >= issue - OLD_AIRBORNE_MS)
            .any(|s| !s.ground)
    }

    fn path(&mut self, leg: u32) -> &Vec<PathPoint> {
        let (ctx, addr) = (self.ctx, self.addr);
        if !self.paths.contains_key(&leg) {
            let (p, forced) = sample_path(ctx, addr, leg);
            self.long_gaps += forced;
            self.paths.insert(leg, p);
        }
        self.paths.entry(leg).or_default()
    }

    /// The reports of the aircraft within the leg, in time order.
    fn leg_states(&self, leg: u32) -> &'b [AirState] {
        leg_reports(self.ctx, self.addr, leg)
    }

    /// The aircraft is airborne on the leg at the issue time: the issue is between the leg's first
    /// airborne report and its landing (or last report).
    fn airborne_at(&self, leg: u32, issue: i64) -> bool {
        self.cands
            .iter()
            .position(|&c| c == leg)
            .and_then(|k| self.facts.get(k))
            .is_some_and(|f| f.airborne_at(issue))
    }

    /// The bound leg ended at the clip box edge, not inside it.
    fn end_at_edge(&self, leg: u32) -> bool {
        let ctx = self.ctx;
        matches!(ctx.leg_ends.get(leg as usize), Some(Some((_, end)))
            if near_box_edge(*end, ctx.cfg.box_deg, ctx.cfg.edge_margin_m))
    }

    /// The crossing of `place` on `leg` for an estimate, once for every cut. The closest
    /// approaches are counted when `sys` is given.
    fn cross(
        &mut self,
        mut sys: Option<&mut SystemReport>,
        leg: u32,
        place: Place,
        est: i64,
        issue: i64,
    ) -> Vec<Result<Sample, Exclusion>> {
        let ctx = self.ctx;
        let key = (leg, place_key(place));
        if !self.searches.contains_key(&key) {
            // One search at the largest cut: the passage is chosen once, and each cut scores it.
            let widest = ctx.cuts_m.last().copied().unwrap_or(ctx.cfg.cut_m);
            let a = approach(self.path(leg), place, widest);
            if let Some(sys) = sys.as_deref_mut() {
                closest_add(sys, a.min_dist_m);
            }
            self.searches.insert(key, a);
        }
        let Some(a) = self.searches.get(&key) else {
            return Vec::new();
        };
        if let Some(sys) = sys {
            if a.passages.len() > 1 {
                count(sys, MULTI_KEY, 1);
                if pick_passage(a, issue).map(|p| p.t_ms) != unsplit_choice_ms(a, issue) {
                    count(sys, CHANGED_KEY, 1);
                }
            }
        }
        ctx.cuts_m
            .iter()
            .map(|&c| {
                let cfg = Config {
                    cut_m: c,
                    ..ctx.cfg
                };
                let p = choose_passage(a, issue, &cfg)?;
                let mut sm = score(est, p.t_ms, issue, &cfg)?;
                sm.dist_m = p.dist_m;
                Ok(sm)
            })
            .collect()
    }

    /// An estimate of a landing time against the touchdown on `leg` at the airport `apt`.
    fn landing(
        &mut self,
        sys: &mut SystemReport,
        leg: u32,
        apt: Option<&str>,
        est: i64,
        issue: i64,
        actual_arr: Option<i64>,
    ) -> Result<(Sample, Extra, bool), Exclusion> {
        let ctx = self.ctx;
        lead_in_range(est, issue, &ctx.cfg)?;
        let Some(apt) = apt else {
            return Err(Exclusion::AirportUnknown);
        };
        let states = self.leg_states(leg);
        let covered = *self
            .ground_cov
            .entry(leg)
            .or_insert_with(|| states.iter().any(|s| s.ground));
        if !covered {
            return Err(Exclusion::NoGroundCoverage);
        }
        let key = (leg, apt.to_string());
        if !self.lands.contains_key(&key) {
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
            let r = match resolve(airport_places(ctx.table, apt), &refs) {
                Resolution::NotFound => Err(Exclusion::AirportUnknown),
                Resolution::Refused => Err(Exclusion::PlaceAmbiguous),
                Resolution::Unique(arp) | Resolution::Picked(arp) => {
                    let picked = matches!(
                        resolve(airport_places(ctx.table, apt), &refs),
                        Resolution::Picked(_)
                    );
                    let set = landings(states, arp, airport_elev_ft(ctx.table, apt), &ctx.cfg);
                    count(
                        sys,
                        "changes near the airport refused: ground not held (touch and go, blip)",
                        set.refused_persist,
                    );
                    count(
                        sys,
                        "changes near the airport refused: too high and too fast",
                        set.refused_altitude,
                    );
                    Ok((picked, set))
                }
            };
            self.lands.insert(key.clone(), r);
        }
        let (picked, set) = match self.lands.get(&key) {
            Some(Ok((p, s))) => (*p, s),
            Some(Err(e)) => return Err(*e),
            None => return Err(Exclusion::AirportUnknown),
        };
        let landing = choose_landing(set, issue, &ctx.cfg)?;
        let sm = score(est, landing.t_ms, issue, &ctx.cfg)?;
        let x = Extra {
            gap_s: Some(landing.gap_ms as f64 / 1000.0),
            alt: actual_arr.map(|t| {
                (
                    (est - t) as f64 / 1000.0,
                    (landing.t_ms - t) as f64 / 1000.0,
                )
            }),
            td: Some((
                (est - landing.t_air_ms) as f64 / 1000.0,
                (est - landing.t_gnd_ms) as f64 / 1000.0,
            )),
            issue_ms: issue,
            ..Extra::default()
        };
        Ok((sm, x, picked))
    }
}

/// The arrival airport shown for flights whose records name none.
const NO_AIRPORT: &str = "(none named)";

/// Truth path segments longer than 30 s made holes (zero with a step of 30 s or less: the truth
/// answers only within 10 s of a report on both sides).
const LONG_GAP_KEY: &str = "truth path segments over 30 s made holes";

/// Rows dropped as exact duplicates (same flight, point, issue time and estimate) before scoring.
const DUPLICATE_KEY: &str =
    "exact duplicate rows dropped before scoring (same flight, point, issue time and estimate)";

/// Rows (message, point) whose truth path has more than one passage within the widest cut, and of
/// them those whose chosen passage differs from the one the runs-not-split rule chose.
const MULTI_KEY: &str = "rows (message, point) with more than one passage within the widest cut";
const CHANGED_KEY: &str =
    "  of them, rows whose chosen passage differs from the earlier rule (runs not split, closest)";

/// Rows whose bound leg differs from the one the rule without the shuttle test bound.
const SHUTTLE_KEY: &str = "shuttle rule (departure and arrival airports both must match an in-progress leg): rows whose bound leg changed";

/// The counts of the in-progress legs a message was not bound to.
const REFUSED_KEYS: [&str; 2] = [
    "leg binding: in-progress leg refused, airborne but not the flight's own (neither airport matches, or the callsign differs when no airport is known), messages",
    "leg binding: in-progress leg refused, already landed before the issue (taxi-in), messages",
];

/// Records the crossing results of one estimate: the headline cut into its class, and, for a
/// headline row, every cut into the sensitivity tables, and the abeam passages.
fn record_cross(
    sys: &mut SystemReport,
    kind: usize,
    fl: &Flight<'_, '_>,
    (unit_id, cs): (u32, bool),
    (leg, issue, est): (u32, i64, i64),
    results: Vec<Result<Sample, Exclusion>>,
    flown: bool,
) {
    let g = group_of(cs, fl.airborne_at(leg, issue));
    let head = results
        .get(fl.ctx.primary)
        .copied()
        .unwrap_or(Err(Exclusion::NoTruthPath));
    let mut nested_broken = false;
    if g == 0 {
        for pair in results.windows(2) {
            if let (Some(Ok(_)), Some(Err(_))) = (pair.first(), pair.get(1)) {
                nested_broken = true;
            }
        }
    }
    let Some(k) = sys.kinds.get_mut(kind) else {
        return;
    };
    kind_record(
        k,
        unit_id,
        g,
        head,
        Extra {
            issue_ms: issue,
            lead_bin: lead_bin(est, issue),
            flown,
            ..Extra::default()
        },
    );
    if g == 0 {
        for (r, c) in results.iter().zip(&fl.ctx.cuts_m) {
            sens_add(k, &format!("crossing cut {:.1} NM", c / 1852.0), r);
        }
        let head_cut = fl.ctx.cuts_m.get(fl.ctx.primary).copied().unwrap_or(0.0);
        if let Some(Ok(s)) = results.last() {
            if s.dist_m > head_cut {
                abeam_add(k, s);
            }
        }
    }
    if nested_broken {
        count(sys, NESTED_KEY, 1);
    }
}

/// Headline rows scored at one crossing cut but not at the next larger one (there are none when
/// the cuts nest).
const NESTED_KEY: &str =
    "headline rows scored at a smaller crossing cut but not at a larger one (cuts should nest)";

/// Records one excluded estimate (the class does not matter), with the bin of its stated lead.
fn record_excluded(sys: &mut SystemReport, kind: usize, x: Exclusion, lead: Option<usize>) {
    if let Some(k) = sys.kinds.get_mut(kind) {
        kind_record(
            k,
            0,
            0,
            Err(x),
            Extra {
                lead_bin: lead,
                ..Extra::default()
            },
        );
    }
}

/// Records a landing estimate.
fn record_landing(
    sys: &mut SystemReport,
    kind: usize,
    fl: &Flight<'_, '_>,
    (unit_id, cs): (u32, bool),
    (leg, issue, est): (u32, i64, i64),
    r: Result<(Sample, Extra, bool), Exclusion>,
) {
    let g = group_of(cs, fl.airborne_at(leg, issue));
    let Some(k) = sys.kinds.get_mut(kind) else {
        return;
    };
    match r {
        Ok((sm, x, picked)) => {
            airport_scored(k, fl.arr.as_deref().unwrap_or(NO_AIRPORT));
            if picked {
                k.resolved_ambiguous += 1;
            }
            kind_record(k, unit_id, g, Ok(sm), Extra { flown: picked, ..x });
        }
        Err(e) => kind_record(
            k,
            unit_id,
            g,
            Err(e),
            Extra {
                lead_bin: lead_bin(est, issue),
                end_at_edge: fl.end_at_edge(leg),
                ..Extra::default()
            },
        ),
    }
}

/// The joined flights whose interval touched more than one truth leg, counted.
fn count_multi(ctx: &Ctx<'_>, sys: &mut SystemReport, addr: u32, unit: &Unit) {
    if legs_of(ctx.ix, addr, unit).len() > 1 {
        count(
            sys,
            "joined flights whose interval touched more than one leg",
            1,
        );
    }
}

/// The coordinates of the route points of `routes`, thinned: the geometry the flight filed.
fn route_geometry(leg: &Legacy, routes: &[u32]) -> Vec<Place> {
    let mut v: Vec<Place> = Vec::new();
    for &r in routes {
        let (first, len) = (
            leg.routes.first.get(r as usize).copied().unwrap_or(0) as usize,
            leg.routes.len.get(r as usize).copied().unwrap_or(0) as usize,
        );
        for q in first..first + len {
            if let (Some(&la), Some(&lo)) = (
                leg.route_points.lat_e7.get(q),
                leg.route_points.lon_e7.get(q),
            ) {
                if la != NO_COORD && lo != NO_COORD {
                    v.push(place_of_e7(la, lo));
                }
            }
        }
    }
    thin(v.into_iter())
}

/// How many route points with coordinates either side of a point are its neighbours.
const NEIGHBOURS_EACH_SIDE: usize = 2;

/// The nearest points with coordinates before and after point `p` of its route (`first` and `len`
/// are the route's), up to [`NEIGHBOURS_EACH_SIDE`] each side.
fn route_neighbours(leg: &Legacy, (first, len): (usize, usize), p: usize) -> Vec<Place> {
    let at = |q: usize| {
        let (la, lo) = (
            leg.route_points.lat_e7.get(q).copied()?,
            leg.route_points.lon_e7.get(q).copied()?,
        );
        (la != NO_COORD && lo != NO_COORD).then(|| place_of_e7(la, lo))
    };
    let before = (first..p).rev().filter_map(at).take(NEIGHBOURS_EACH_SIDE);
    let after = (p + 1..first + len)
        .filter_map(at)
        .take(NEIGHBOURS_EACH_SIDE);
    before.chain(after).collect()
}

/// What a name with several places was resolved against.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefSource {
    Neighbours,
    RouteGeometry,
    FlownPath,
}

const REF_KEYS: [&str; 3] = [
    "names with several places resolved against the filed route's neighbouring points",
    "names with several places resolved against the filed route's other points (no neighbour has coordinates)",
    "names with several places resolved against the flown path (the flight's routes carry no coordinates)",
];

/// The tally of a kind: its exclusions and the rows scored per class.
fn tally(k: &KindTable) -> ([u64; EXCLUSION_COUNT], [u64; GROUPS]) {
    let mut g = [0u64; GROUPS];
    for (c, grp) in g.iter_mut().zip(&k.groups) {
        *c = grp.errors.iter().map(|e| e.len() as u64).sum();
    }
    (k.exclusions, g)
}

/// What became of one row, from the tallies before and after it: the reason it was excluded or
/// the class it was scored in (and, for the headline, whether on the leg it was bound to before).
fn fate(
    before: &([u64; EXCLUSION_COUNT], [u64; GROUPS]),
    after: &([u64; EXCLUSION_COUNT], [u64; GROUPS]),
    same_leg: bool,
) -> String {
    for (k, name) in EXCLUSION_NAMES.iter().enumerate() {
        if after.0.get(k) > before.0.get(k) {
            return format!("excluded: {name}");
        }
    }
    for (g, title) in GROUP_NAMES.iter().enumerate() {
        if after.1.get(g) > before.1.get(g) {
            return if g == 0 {
                format!(
                    "scored in the headline, {}",
                    if same_leg {
                        "on the same leg"
                    } else {
                        "on another leg"
                    }
                )
            } else {
                format!("scored, but in the class: {title}")
            };
        }
    }
    "not counted".to_string()
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
        let span = |r: u32| {
            (
                leg.routes.first.get(r as usize).copied().unwrap_or(0) as usize,
                leg.routes.len.get(r as usize).copied().unwrap_or(0) as usize,
            )
        };
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
        let dep = airport_of(leg, &[f], false);
        let arr = airport_of(leg, &[f], true);
        let outcome = refine(
            ctx,
            sys,
            join(ctx.ix, &unit),
            &unit,
            dep.as_deref(),
            arr.as_deref(),
        );
        if let Some(c) = sys.outcomes.get_mut(outcome_index(outcome)) {
            *c += 1;
        }
        let est_of = |p: usize| leg.route_points.est_ms.get(p).copied().unwrap_or(NO_TIME);
        let Some(addr) = joined_addr(outcome) else {
            for &r in routes {
                let (first, len) = span(r);
                for p in first..first + len {
                    record_excluded(sys, 0, Exclusion::NotJoined, lead_bin(est_of(p), ts(r)));
                }
            }
            continue;
        };
        count_multi(ctx, sys, addr, &unit);
        let cs = by_callsign(outcome) == Some(true);
        let mut fl = Flight::new(ctx, addr, (dep, arr), unit.callsign);
        let mut seen: BTreeSet<(i64, i64, i32, i32, [u8; 8])> = BTreeSet::new();
        for &r in routes {
            let issue = ts(r);
            let (first, len) = span(r);
            for p in first..first + len {
                let est = est_of(p);
                let lb = lead_bin(est, issue);
                let (lat, lon) = (
                    leg.route_points.lat_e7.get(p).copied().unwrap_or(NO_COORD),
                    leg.route_points.lon_e7.get(p).copied().unwrap_or(NO_COORD),
                );
                let fix = leg.route_points.fix.get(p).copied().unwrap_or([0; 8]);
                if !seen.insert((issue, est, lat, lon, fix)) {
                    count(sys, DUPLICATE_KEY, 1);
                    continue;
                }
                // What the earlier rounds' binding scored in the headline at 60 to 120 minutes,
                // and what became of the row now.
                let old = if lb == Some(OLD_FATE_BIN) && !cs {
                    old_headline(&mut fl, leg, routes, (lat, lon, &fix), (issue, est))
                } else {
                    None
                };
                let before = old.and_then(|_| sys.kinds.first().map(tally));
                'row: {
                    if issue == NO_TIME {
                        record_excluded(sys, 0, Exclusion::NoIssueTime, None);
                        break 'row;
                    }
                    if est == NO_TIME {
                        record_excluded(sys, 0, Exclusion::NoEstimate, None);
                        break 'row;
                    }
                    let lg = match fl.leg_for(sys, issue) {
                        Ok(l) => l,
                        Err(x) => {
                            record_excluded(sys, 0, x, lb);
                            break 'row;
                        }
                    };
                    if fl.path(lg).is_empty() {
                        record_excluded(sys, 0, Exclusion::NoTruthPath, lb);
                        break 'row;
                    }
                    // Several places of one name: the one nearest the route's neighbouring
                    // points, else nearest the route's other points, else nearest the flown path.
                    let mut picked = false;
                    let mut source = RefSource::Neighbours;
                    let placed = if lat != NO_COORD && lon != NO_COORD {
                        Ok(place_of_e7(lat, lon))
                    } else {
                        let paths = &fl.paths;
                        let refs =
                            || {
                                let nb = route_neighbours(leg, (first, len), p);
                                if !nb.is_empty() {
                                    return nb;
                                }
                                let all = route_geometry(leg, routes);
                                if !all.is_empty() {
                                    source = RefSource::RouteGeometry;
                                    return all;
                                }
                                source = RefSource::FlownPath;
                                thin(paths.get(&lg).map_or(&[][..], Vec::as_slice).iter().map(
                                    |q| Place {
                                        lat_deg: q.lat_deg,
                                        lon_deg: q.lon_deg,
                                    },
                                ))
                            };
                        point_result(ctx.table, name(&fix), refs).map(|(pl, pk)| {
                            picked = pk;
                            pl
                        })
                    };
                    match placed {
                        Err(x) => record_excluded(sys, 0, x, lb),
                        Ok(place) => {
                            if picked {
                                if let Some(k) = sys.kinds.first_mut() {
                                    k.resolved_ambiguous += 1;
                                }
                                count(
                                    sys,
                                    REF_KEYS.get(source as usize).copied().unwrap_or("?"),
                                    1,
                                );
                            }
                            let res = fl.cross(Some(&mut *sys), lg, place, est, issue);
                            record_cross(
                                sys,
                                0,
                                &fl,
                                (f, cs),
                                (lg, issue, est),
                                res,
                                picked && source == RefSource::FlownPath,
                            );
                        }
                    }
                }
                if let (Some(b), Some(ol)) = (before, old) {
                    if let Some(k) = sys.kinds.first() {
                        let new_leg = fl.binds.get(&issue).and_then(|b| b.0.leg.ok());
                        let label = fate(&b, &tally(k), new_leg == Some(ol));
                        count(sys, OLD_FATE_ALL, 1);
                        count(sys, &format!("{OLD_FATE_PREFIX}{label}"), 1);
                    }
                }
            }
        }
        count(sys, LONG_GAP_KEY, fl.long_gaps);
    }
}

/// The bin whose headline rows the earlier binding's fate is reported for (60 to 120 minutes).
const OLD_FATE_BIN: usize = 5;
const OLD_FATE_ALL: &str = "ERAM headline rows at 60 to 120 min under the earlier binding (in progress or next leg, no ownership test)";
const OLD_FATE_PREFIX: &str = "  of them, now: ";

/// The leg the earlier rounds' binding scored an address-joined ERAM row at 60 to 120 minutes of
/// stated lead in the headline on (an airborne report within a minute before the issue, and a
/// crossing within the cut), when it did.
fn old_headline(
    fl: &mut Flight<'_, '_>,
    leg: &Legacy,
    routes: &[u32],
    (lat, lon, fix): (i32, i32, &[u8; 8]),
    (issue, est): (i64, i64),
) -> Option<u32> {
    if issue == NO_TIME || est == NO_TIME {
        return None;
    }
    let ol = fl.old_leg(issue)?;
    if !fl.old_airborne(ol, issue) || fl.path(ol).is_empty() {
        return None;
    }
    let ctx = fl.ctx;
    let place = if lat != NO_COORD && lon != NO_COORD {
        place_of_e7(lat, lon)
    } else {
        let paths = &fl.paths;
        let refs = || {
            let all = route_geometry(leg, routes);
            if all.is_empty() {
                thin(
                    paths
                        .get(&ol)
                        .map_or(&[][..], Vec::as_slice)
                        .iter()
                        .map(|q| Place {
                            lat_deg: q.lat_deg,
                            lon_deg: q.lon_deg,
                        }),
                )
            } else {
                all
            }
        };
        point_result(ctx.table, name(fix), refs).ok()?.0
    };
    let res = fl.cross(None, ol, place, est, issue);
    match res.get(ctx.primary) {
        Some(Ok(s)) if s.bin == OLD_FATE_BIN => Some(ol),
        _ => None,
    }
}

/// TBFM: the meter-fix and runway estimates and the scheduled times, against the crossing of the
/// meter fix and the touchdown.
fn tbfm(leg: &Legacy, ctx: &Ctx<'_>, units: &[TableUnit], sys: &mut SystemReport) {
    let kinds = [
        tbfm_kind::ETA_MFX,
        tbfm_kind::STA_SFX,
        tbfm_kind::ETA_RWY,
        tbfm_kind::STA_RWY,
    ];
    for (uix, u) in units.iter().enumerate() {
        let outcome = refine(
            ctx,
            sys,
            u.outcome,
            &u.unit,
            u.dep.as_deref(),
            u.arrival.as_deref(),
        );
        if let Some(c) = sys.outcomes.get_mut(outcome_index(outcome)) {
            *c += 1;
        }
        let addr = joined_addr(outcome);
        if let Some(a) = addr {
            count_multi(ctx, sys, a, &u.unit);
        }
        let cs = by_callsign(outcome) == Some(true);
        let mut fl =
            addr.map(|a| Flight::new(ctx, a, (u.dep.clone(), u.arrival.clone()), u.unit.callsign));
        let actual_arr = ctx.tfms_actual.get(&u.plan).copied();
        let mut seen: BTreeSet<(u8, i64, i64, [u8; 8])> = BTreeSet::new();
        for &r in &u.rows {
            let ri = r as usize;
            let kind = leg.tbfm.kind.get(ri).copied().unwrap_or(0);
            let Some(slot) = kinds.iter().position(|&k| k == kind) else {
                continue;
            };
            let issue = leg.tbfm.issue_ms.get(ri).copied().unwrap_or(NO_TIME);
            let est = leg.tbfm.time_ms.get(ri).copied().unwrap_or(NO_TIME);
            let fix = leg.tbfm.fix.get(ri).copied().unwrap_or([0; 8]);
            if !seen.insert((kind, issue, est, fix)) {
                count(sys, DUPLICATE_KEY, 1);
                continue;
            }
            let lb = lead_bin(est, issue);
            if slot >= 2 {
                if let Some(k) = sys.kinds.get_mut(slot) {
                    airport_offer(k, u.arrival.as_deref().unwrap_or(NO_AIRPORT));
                }
            }
            let Some(fl) = fl.as_mut() else {
                record_excluded(sys, slot, Exclusion::NotJoined, lb);
                continue;
            };
            if issue == NO_TIME {
                record_excluded(sys, slot, Exclusion::NoIssueTime, None);
                continue;
            }
            if est == NO_TIME {
                record_excluded(sys, slot, Exclusion::NoEstimate, None);
                continue;
            }
            let nm = name(&fix);
            if slot < 2 && point_places(ctx.table, nm).is_empty() {
                record_excluded(sys, slot, Exclusion::PlaceUnknown, lb);
                continue;
            }
            let lg = match fl.leg_for(sys, issue) {
                Ok(l) => l,
                Err(x) => {
                    record_excluded(sys, slot, x, lb);
                    continue;
                }
            };
            if slot < 2 {
                if fl.path(lg).is_empty() {
                    record_excluded(sys, slot, Exclusion::NoTruthPath, lb);
                    continue;
                }
                let paths = &fl.paths;
                let refs = || {
                    thin(
                        paths
                            .get(&lg)
                            .map_or(&[][..], Vec::as_slice)
                            .iter()
                            .map(|q| Place {
                                lat_deg: q.lat_deg,
                                lon_deg: q.lon_deg,
                            }),
                    )
                };
                match point_result(ctx.table, nm, refs) {
                    Err(x) => record_excluded(sys, slot, x, lb),
                    Ok((place, picked)) => {
                        if picked {
                            if let Some(k) = sys.kinds.get_mut(slot) {
                                k.resolved_ambiguous += 1;
                            }
                        }
                        let res = fl.cross(Some(&mut *sys), lg, place, est, issue);
                        record_cross(
                            sys,
                            slot,
                            fl,
                            (uix as u32, cs),
                            (lg, issue, est),
                            res,
                            picked,
                        );
                    }
                }
            } else {
                let res = fl.landing(sys, lg, u.arrival.as_deref(), est, issue, actual_arr);
                record_landing(sys, slot, fl, (uix as u32, cs), (lg, issue, est), res);
            }
        }
        if let Some(f) = &fl {
            count(sys, LONG_GAP_KEY, f.long_gaps);
        }
    }
}

/// An airport's landings on a leg: whether one of several places was taken, and the set; or why
/// not.
type AirportLandings = Result<(bool, LandingSet), Exclusion>;

/// TFMS: the arrival estimates, against the touchdown at the arrival airport.
fn tfms(leg: &Legacy, ctx: &Ctx<'_>, units: &[TableUnit], sys: &mut SystemReport) {
    for (uix, u) in units.iter().enumerate() {
        let outcome = refine(
            ctx,
            sys,
            u.outcome,
            &u.unit,
            u.dep.as_deref(),
            u.arrival.as_deref(),
        );
        if let Some(c) = sys.outcomes.get_mut(outcome_index(outcome)) {
            *c += 1;
        }
        let addr = joined_addr(outcome);
        if let Some(a) = addr {
            count_multi(ctx, sys, a, &u.unit);
        }
        let cs = by_callsign(outcome) == Some(true);
        let mut fl =
            addr.map(|a| Flight::new(ctx, a, (u.dep.clone(), u.arrival.clone()), u.unit.callsign));
        let actual_arr = ctx.tfms_actual.get(&u.plan).copied();
        let mut seen: BTreeSet<(i64, i64)> = BTreeSet::new();
        for &r in &u.rows {
            let ri = r as usize;
            let issue = leg.tfms.issue_ms.get(ri).copied().unwrap_or(NO_TIME);
            let est = leg.tfms.time_ms.get(ri).copied().unwrap_or(NO_TIME);
            if !seen.insert((issue, est)) {
                count(sys, DUPLICATE_KEY, 1);
                continue;
            }
            let lb = lead_bin(est, issue);
            if let Some(k) = sys.kinds.first_mut() {
                airport_offer(k, u.arrival.as_deref().unwrap_or(NO_AIRPORT));
            }
            let Some(fl) = fl.as_mut() else {
                record_excluded(sys, 0, Exclusion::NotJoined, lb);
                continue;
            };
            if issue == NO_TIME {
                record_excluded(sys, 0, Exclusion::NoIssueTime, None);
                continue;
            }
            if est == NO_TIME {
                record_excluded(sys, 0, Exclusion::NoEstimate, None);
                continue;
            }
            let lg = match fl.leg_for(sys, issue) {
                Ok(l) => l,
                Err(x) => {
                    record_excluded(sys, 0, x, lb);
                    continue;
                }
            };
            let res = fl.landing(sys, lg, u.arrival.as_deref(), est, issue, actual_arr);
            record_landing(sys, 0, fl, (uix as u32, cs), (lg, issue, est), res);
        }
        if let Some(f) = &fl {
            count(sys, LONG_GAP_KEY, f.long_gaps);
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
    /// Flights in the TBFM and TFMS recordings (SFDPS is `sfdps_flights.0`).
    pub recording_flights: [u64; 2],
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
    let mut tfms_dep: BTreeMap<[u8; 12], String> = BTreeMap::new();
    for (p, a) in leg.tfms.plan_id.iter().zip(&leg.tfms.dep) {
        let k = key_of(a);
        if !k.is_empty() {
            tfms_dep.entry(norm_id::<12>(p)).or_insert(k);
        }
    }
    let tb_units = tbfm_units(leg, &plans, &ix, &tfms_arr, &tfms_dep);
    let tf_units = tfms_units(leg, &plans, &ix);
    let want: BTreeSet<u32> = ix.by_addr.keys().copied().collect();
    let states = air_states(&reports, &want);
    drop(reports);
    let ends = leg_ends(inp.built);
    let mut cuts_m: Vec<f64> = [0.5, 1.0, 2.0, 5.0].iter().map(|c| c * 1852.0).collect();
    if !cuts_m.iter().any(|c| (c - inp.cfg.cut_m).abs() < 1e-6) {
        cuts_m.push(inp.cfg.cut_m);
    }
    cuts_m.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let primary = cuts_m
        .iter()
        .position(|c| (c - inp.cfg.cut_m).abs() < 1e-6)
        .unwrap_or(0);
    let ctx = Ctx {
        cfg: inp.cfg,
        cuts_m,
        primary,
        step_ms: inp.step_ms,
        built: inp.built,
        ix: &ix,
        table: &inp.table,
        states: &states,
        tfms_actual: &tfms_actual,
        leg_ends: &ends,
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
        recording_flights: [tb_units.len() as u64, tf_units.len() as u64],
    }
}

/// The population of a system for the header: its flights in the recording, how many joined to an
/// ADS-B aircraft, how many have a scored estimate, and the estimates whose bound leg has no
/// ground report.
pub fn population_line(sys: &SystemReport, total: u64) -> String {
    let joined =
        sys.outcomes.first().copied().unwrap_or(0) + sys.outcomes.get(1).copied().unwrap_or(0);
    let all: u64 = sys.outcomes.iter().sum();
    let by_cs = sys.outcomes.get(1).copied().unwrap_or(0);
    let no_cov: u64 = sys
        .kinds
        .iter()
        .map(|k| {
            k.exclusions
                .get(exclusion_index(Exclusion::NoGroundCoverage))
                .copied()
                .unwrap_or(0)
        })
        .sum();
    format!(
        "{total} flights in the recording ({all} counted by join), {joined} joined to an ADS-B aircraft in the ZAU clip ({:.1}%; {by_cs} of them by callsign), {} with a scored estimate, {no_cov} estimates excluded because the bound leg has no ground report",
        if all == 0 {
            0.0
        } else {
            100.0 * joined as f64 / all as f64
        },
        scored_flights(sys)
    )
}

/// The run: reads the recordings and the clips, writes the report and the CSV files under
/// `args.out`, and returns the report text.
pub fn run_counted(args: &Args, sealed: &mut u64) -> Result<String, String> {
    let cfg = Config::new(
        args.cut_nm * 1852.0,
        args.runway_nm * 1852.0,
        args.landing_gap_s * 1000,
        MAX_LOOK_AHEAD_MIN * MS_PER_MIN,
    );
    let mut table = PlaceTable::default();
    let mut nasr_line = "none given: no point or airport has a place".to_string();
    if let Some(z) = &args.nasr {
        let (t, sha) = load_nasr(z)?;
        nasr_line = format!(
            "{}: {} point names, {} airport codes, {} airport keys shared by two airports, {} rows rejected by the parser, sha256 {sha}",
            z.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            t.points.len(),
            t.airports.len(),
            t.collisions.len(),
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
    let names = ["ERAM", "TBFM", "TFMS"];
    let totals = [
        scored.sfdps_flights.1,
        scored.recording_flights[0],
        scored.recording_flights[1],
    ];
    for ((sys, name), total) in scored.systems.iter().zip(names).zip(totals) {
        header.push((format!("population {name}"), population_line(sys, total)));
    }
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

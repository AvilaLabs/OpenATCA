//! Metrics F1 to F3, R1 and H1: time estimates, routes and handoffs against truth. Core (pure).
//!
//! **Truth crossing of a fix** (M2_DESIGN section 3): the time of closest approach of the truth
//! path to the fix, found on a coarse grid then refined, accepted only when the refined distance
//! is within the fix radius (2 NM) and the closest point is not the first or last point of a
//! truth leg (an aircraft that appears or disappears near a fix did not pass it). A flight that
//! never passes within the radius is excluded and counted. A leg can pass a fix more than once
//! (a hold): each contiguous stretch inside the radius is one passage, with its own closest
//! approach. An estimate is scored against the first passage that ends after its issue time (the
//! passage it was issued in, when it was issued inside one); its estimated time never chooses.
//!
//! **One occurrence per flight.** An aircraft can fly several trips in a recording, crossing the
//! same fix and using the same airport more than once. An estimate belongs to one flight plan,
//! so its truth is taken from the one truth leg of that flight: the leg that overlaps the flight's
//! active interval (first to last ERAM sample of the plan) the most, or, when the plan has no ERAM
//! sample, the interval in which the estimate can be scored (the issue time and the 60 minutes
//! after it). Events are never searched across all legs of the address, and the caches are keyed
//! by the leg.
//!
//! **Ground evidence.** A landing or takeoff needs the aircraft near a runway threshold with a
//! barometric altitude near the field elevation. A leg that ends or starts near a threshold with
//! no barometric altitude is not a landing or takeoff: it is excluded and counted.
//!
//! **Look-ahead** is the truth event time minus the time the estimate was issued, in 5-minute
//! bins to 60 minutes; the error is the estimate minus the truth time, signed seconds.

use std::collections::{BTreeMap, HashMap};

use crate::assoc::{Assoc, TruthIndex};
use crate::geo::{dist_m, enu, NM_M};
use crate::legacy::{
    hev, name, norm_id, plan_candidates, plan_overflowed, tbfm_kind, tfms_kind, Legacy, PlanIndex,
    NONE, NO_COORD, NO_TIME,
};
use crate::pair::PairTable;
use crate::stats::{exclude, group_rows, new_result, Exclusions, MetricResult, MAX_CASES};
use crate::truth::{AirportRef, Env, Truth, TruthSample, STANDARD_INHG};

/// Look-ahead bin names, 5 minutes each to 60.
pub const LOOKAHEAD_NAMES: [&str; 12] = [
    "0 to 5 min",
    "5 to 10 min",
    "10 to 15 min",
    "15 to 20 min",
    "20 to 25 min",
    "25 to 30 min",
    "30 to 35 min",
    "35 to 40 min",
    "40 to 45 min",
    "45 to 50 min",
    "50 to 55 min",
    "55 to 60 min",
];

/// Event metric settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventConfig {
    /// A truth path must pass within this of a meter fix, metres.
    pub fix_radius_m: f64,
    /// Coarse search step along a truth leg, ms.
    pub coarse_ms: i64,
    /// Refinement step, ms.
    pub fine_ms: i64,
    /// Width of a look-ahead bin, ms.
    pub bin_ms: i64,
    /// Landing or takeoff must be within this of a runway threshold, metres (1 NM).
    pub runway_radius_m: f64,
    /// ... and within this above the field elevation, feet.
    pub ground_margin_ft: i32,
    /// Handoff: how far either side of the event time to look for the boundary crossing, ms.
    pub handoff_window_ms: i64,
    /// Handoff: search step, ms.
    pub handoff_step_ms: i64,
    /// Route conformance: distance beyond which time counts as off-route, metres (2 NM).
    pub route_limit_m: f64,
    /// Route conformance: longest time one sample stands for, ms.
    pub route_max_dt_ms: i64,
}

impl Default for EventConfig {
    fn default() -> EventConfig {
        EventConfig {
            fix_radius_m: 2.0 * NM_M,
            coarse_ms: 5_000,
            fine_ms: 250,
            bin_ms: 300_000,
            runway_radius_m: NM_M,
            ground_margin_ft: 200,
            handoff_window_ms: 900_000,
            handoff_step_ms: 10_000,
            route_limit_m: 2.0 * NM_M,
            route_max_dt_ms: 60_000,
        }
    }
}

/// What the time-estimate metrics need besides the legacy tables (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
pub struct EventCtx<'a> {
    pub assoc: &'a Assoc,
    pub truth: &'a Truth<'a>,
    pub tix: &'a TruthIndex,
    pub env: &'a Env<'a>,
    pub cfg: &'a EventConfig,
}

/// Active interval of every flight with ERAM samples: first and last position time, Unix ms.
fn flight_windows(leg: &Legacy) -> HashMap<u32, (i64, i64)> {
    let mut w: HashMap<u32, (i64, i64)> = HashMap::new();
    for (&f, &t) in leg.th.flight.iter().zip(&leg.th.pos_ms) {
        let e = w.entry(f).or_insert((t, t));
        e.0 = e.0.min(t);
        e.1 = e.1.max(t);
    }
    w
}

/// The interval an estimate of flight `f` can belong to: the flight's active interval, or when
/// it has no ERAM sample the span in which the estimate could be scored (the issue time and the
/// look-ahead range after it; for no issue time, the range before the estimated time).
fn estimate_window(
    windows: &HashMap<u32, (i64, i64)>,
    f: u32,
    issue_ms: i64,
    time_ms: i64,
    cfg: &EventConfig,
) -> (i64, i64) {
    if let Some(&w) = windows.get(&f) {
        return w;
    }
    let span = cfg.bin_ms.saturating_mul(LOOKAHEAD_NAMES.len() as i64);
    if issue_ms == NO_TIME {
        (time_ms.saturating_sub(span), time_ms)
    } else {
        (issue_ms, issue_ms.saturating_add(span))
    }
}

const NOT_MATCHED: &str = "not matched to an SFDPS flight";
const NONE_ACTIVE: &str = "plan identifier used by several flights, none active at the issue time";
const SEVERAL_ACTIVE: &str =
    "plan identifier used by several flights, more than one fits the estimate";

const LONE_OTHER: &str = "plan identifier's one flight has another aircraft identification";
const OVERFLOWED: &str = "plan identifier used by more flights than the index keeps";

/// The flight an estimate belongs to. `est` is its issue time, its estimated time and the aircraft
/// identification it carries (all zero when it has none).
///
/// * An identifier that more flights used than the index keeps names none: the estimate cannot be
///   told from the flights left out.
/// * One flight: it, unless both it and the estimate carry an identification and the two differ
///   (the identifier was reused by a flight outside the recording). No time check, so an estimate
///   issued before the flight's first ERAM sample still joins.
/// * Several flights: the one whose callsign equals the estimate's identification, when exactly
///   one does (even if the issue time is outside its interval); otherwise the one whose active
///   interval ([`estimate_window`], the interval that picks the truth leg, with its 60-minute
///   rule for a flight with no ERAM sample) contains the issue time among those whose callsign
///   does not contradict the identification. A flight with no callsign is not contradicted. None
///   or more than one is `Err` with the reason to count.
///
/// An estimate with no issue time is placed by its estimated time.
fn resolve_plan(
    leg: &Legacy,
    plan: &PlanIndex,
    windows: &HashMap<u32, (i64, i64)>,
    key: &[u8; 12],
    est: (i64, i64, [u8; 8]),
    cfg: &EventConfig,
) -> Result<u32, &'static str> {
    let (issue, time, aid) = est;
    let aid = norm_id::<8>(&aid);
    if plan_overflowed(plan, key) {
        return Err(OVERFLOWED);
    }
    let cands = plan_candidates(plan, key);
    let has_aid = aid.iter().any(|&b| b != 0);
    let call_of = |f: u32| {
        norm_id::<8>(
            leg.flights
                .callsign
                .get(f as usize)
                .map_or(&[][..], |c| &c[..]),
        )
    };
    let has_call = |f: u32| call_of(f).iter().any(|&b| b != 0);
    let (&only, rest) = match cands.split_first() {
        None => return Err(NOT_MATCHED),
        Some(x) => x,
    };
    if rest.is_empty() {
        if has_aid && has_call(only) && call_of(only) != aid {
            return Err(LONE_OTHER);
        }
        return Ok(only);
    }
    if has_aid {
        let mut named = cands.iter().copied().filter(|&f| call_of(f) == aid);
        if let (Some(f), None) = (named.next(), named.next()) {
            return Ok(f);
        }
    }
    let probe = if issue == NO_TIME { time } else { issue };
    let mut fits = cands.iter().copied().filter(|&f| {
        let (lo, hi) = estimate_window(windows, f, issue, time, cfg);
        let call_ok = !has_aid || !has_call(f) || call_of(f) == aid;
        probe >= lo && probe <= hi && call_ok
    });
    match (fits.next(), fits.next()) {
        (Some(f), None) => Ok(f),
        (None, _) => Err(NONE_ACTIVE),
        _ => Err(SEVERAL_ACTIVE),
    }
}

/// The truth leg of aircraft `addr` that overlaps `window` the most (the earlier leg on a tie),
/// as an index into [`Truth::legs`]; `None` when no leg of the aircraft overlaps it.
pub fn occurrence_leg(
    truth: &Truth<'_>,
    tix: &TruthIndex,
    addr: u32,
    window: (i64, i64),
) -> Option<u32> {
    let mut best: Option<(i64, u32)> = None;
    for &i in tix.by_addr.get(&addr)? {
        let Some(l) = truth.legs.get(i as usize) else {
            continue;
        };
        let overlap = l.end_ms.min(window.1) - l.start_ms.max(window.0);
        if overlap >= 0 && best.is_none_or(|b| overlap > b.0) {
            best = Some((overlap, i));
        }
    }
    best.map(|b| b.1)
}

/// One passage of a fix in a truth leg: a contiguous stretch of the leg inside the fix radius.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Passage {
    /// The last time of the stretch inside the radius, Unix ms.
    pub end_ms: i64,
    /// The closest approach within the stretch, Unix ms; `None` when it is the first or last
    /// point of the leg (an aircraft that appears or disappears near a fix did not pass it).
    pub cross_ms: Option<i64>,
}

/// Every passage of a fix by the aircraft of truth leg `leg_ix`, in time order: each contiguous
/// stretch of the leg within the fix radius, found on the coarse grid and refined, with its
/// closest approach (none when the stretch begins or ends at a hole in the truth). Empty when the aircraft never came within the radius.
pub fn fix_passages(
    truth: &Truth<'_>,
    leg_ix: u32,
    lat: f64,
    lon: f64,
    cfg: &EventConfig,
) -> Vec<Passage> {
    let Some(l) = truth.legs.get(leg_ix as usize) else {
        return Vec::new();
    };
    let (addr, ls, le) = (l.addr, l.start_ms, l.end_ms);
    let (coarse, fine) = (cfg.coarse_ms.max(1), cfg.fine_ms.max(1));
    let dist = |t: i64| {
        (truth.query)(addr, t)
            .ok()
            .map(|s| dist_m(lat, lon, s.lat, s.lon))
    };
    // Windows around the coarse samples that are near the fix, merged when they touch.
    let mut windows: Vec<(i64, i64)> = Vec::new();
    let mut t = ls;
    while t <= le {
        if dist(t).is_some_and(|d| d <= cfg.fix_radius_m + 2_000.0) {
            let w = ((t - coarse).max(ls), (t + coarse).min(le));
            match windows.last_mut() {
                Some(last) if w.0 <= last.1 => last.1 = last.1.max(w.1),
                _ => windows.push(w),
            }
        }
        t += coarse;
    }
    let mut out = Vec::new();
    for (lo, hi) in windows {
        // (closest distance, its time, last time inside, began after a hole in the truth) of the
        // stretch being walked.
        let mut cur: Option<(f64, i64, i64, bool)> = None;
        let mut after_hole = false;
        let mut t = lo;
        while t <= hi + fine {
            let raw = if t <= hi { dist(t) } else { None };
            let hole = t <= hi && raw.is_none();
            match (raw.filter(|&d| d <= cfg.fix_radius_m), cur) {
                (Some(d), Some((bd, bt, _, h))) => {
                    cur = Some(if d < bd { (d, t, t, h) } else { (bd, bt, t, h) });
                }
                (Some(d), None) => cur = Some((d, t, t, after_hole)),
                (None, Some((_, bt, end, began_after_hole))) => {
                    // A stretch that begins or ends at a hole in the truth has no known closest
                    // approach: the aircraft may have been nearer inside the hole.
                    let inner = bt - ls >= coarse && le - bt >= coarse;
                    out.push(Passage {
                        end_ms: end,
                        cross_ms: (inner && !began_after_hole && !hole).then_some(bt),
                    });
                    cur = None;
                }
                (None, None) => {}
            }
            after_hole = hole;
            t += fine;
        }
    }
    out
}

/// What a fix identifier resolved to.
enum Placed {
    At(f64, f64),
    /// Not in the model.
    Missing,
    /// Names more than one place and no context chose one.
    Ambiguous,
}

/// The place of a meter fix. An identifier with several places is the one inside the zone when
/// exactly one is; otherwise it is ambiguous (the feed gives no other context).
fn place_meter_fix(env: &Env<'_>, name: &str) -> Placed {
    if let Some((la, lo)) = (env.fix)(name) {
        return Placed::At(la, lo);
    }
    let all = (env.fix_all)(name);
    match all.as_slice() {
        [] => Placed::Missing,
        [(la, lo)] => Placed::At(*la, *lo),
        many => {
            let mut inside = many.iter().filter(|p| (env.in_zone)(p.0, p.1));
            match (inside.next(), inside.next()) {
                (Some(&(la, lo)), None) => Placed::At(la, lo),
                _ => Placed::Ambiguous,
            }
        }
    }
}

const NO_LEG: &str = "no truth leg of the aircraft in the flight's active interval";

fn lookahead_bin(cfg: &EventConfig, ahead_ms: i64) -> Option<u8> {
    if ahead_ms < 0 {
        return None;
    }
    let b = ahead_ms / cfg.bin_ms.max(1);
    (b < LOOKAHEAD_NAMES.len() as i64).then_some(b as u8)
}

fn bin_result(id: &'static str, title: &'static str, vals: &[f64], bins: &[u8]) -> MetricResult {
    let mut r = new_result(id, title, "s");
    r.rows = group_rows(vals, &[("lookahead", bins, &LOOKAHEAD_NAMES)]);
    r
}

/// F1 (meter-fix ETA) and F2 (scheduled time) of TBFM against the truth crossing of the meter
/// fix. F2 uses `sta_sfx` against the crossing of the section's meter fix: the feed has no
/// `sta_mfx`, and whether the scheduling fix is the meter fix is unconfirmed
/// (docs/TBFM_SURVEY.md), so F2 carries that note.
pub fn f1_f2_tbfm(
    leg: &Legacy,
    plan: &PlanIndex,
    ctx: &EventCtx<'_>,
    ex: &mut Exclusions,
) -> (MetricResult, MetricResult) {
    let EventCtx {
        assoc,
        truth,
        tix,
        env,
        cfg,
    } = *ctx;
    let (mut v1, mut b1, mut v2, mut b2) = (vec![], vec![], vec![], vec![]);
    let mut cache: BTreeMap<(u32, [u8; 8], u32), Option<Passage>> = BTreeMap::new();
    let windows = flight_windows(leg);
    let t = &leg.tbfm;
    for i in 0..t.kind.len() {
        let kind = t.kind.get(i).copied().unwrap_or(0);
        let scope = match kind {
            tbfm_kind::ETA_MFX => "F1",
            tbfm_kind::STA_SFX => "F2",
            _ => continue,
        };
        let issue = t.issue_ms.get(i).copied().unwrap_or(NO_TIME);
        let key = t.plan_id.get(i).copied().unwrap_or([0; 12]);
        let aid = t.aid.get(i).copied().unwrap_or([0; 8]);
        let est = (issue, t.time_ms.get(i).copied().unwrap_or(NO_TIME), aid);
        let f = match resolve_plan(leg, plan, &windows, &key, est, cfg) {
            Ok(f) => f,
            Err(why) => {
                exclude(ex, scope, why);
                continue;
            }
        };
        let addr = assoc.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE {
            exclude(ex, scope, "flight not associated with truth");
            continue;
        }
        let fix = t.fix.get(i).copied().unwrap_or([0; 8]);
        let (lat, lon) = match place_meter_fix(env, name(&fix)) {
            Placed::At(la, lo) => (la, lo),
            Placed::Missing => {
                exclude(ex, scope, "meter fix not in the model");
                continue;
            }
            Placed::Ambiguous => {
                exclude(ex, scope, "meter fix names more than one place");
                continue;
            }
        };
        let window = estimate_window(&windows, f, issue, est.1, cfg);
        let Some(li) = occurrence_leg(truth, tix, addr, window) else {
            exclude(ex, scope, NO_LEG);
            continue;
        };
        // The first passage that ends after the issue time (the one the issue time falls in, when
        // it falls inside one); the estimated time never chooses it.
        let mut k = 0u32;
        let passage = loop {
            let p = *cache.entry((li, fix, k)).or_insert_with(|| {
                fix_passages(truth, li, lat, lon, cfg)
                    .get(k as usize)
                    .copied()
            });
            match p {
                Some(p) if issue != NO_TIME && p.end_ms <= issue => k += 1,
                other => break other,
            }
        };
        let Some(passage) = passage else {
            exclude(
                ex,
                scope,
                if k == 0 {
                    "truth never within 2 NM of the meter fix"
                } else {
                    "issued after the crossing or more than 60 min before it"
                },
            );
            continue;
        };
        let Some(cross) = passage.cross_ms else {
            exclude(ex, scope, "truth never within 2 NM of the meter fix");
            continue;
        };
        if issue == NO_TIME {
            exclude(ex, scope, "no issue time");
            continue;
        }
        let Some(bin) = lookahead_bin(cfg, cross - issue) else {
            exclude(
                ex,
                scope,
                "issued after the crossing or more than 60 min before it",
            );
            continue;
        };
        let err = (t.time_ms.get(i).copied().unwrap_or(0) - cross) as f64 / 1000.0;
        if kind == tbfm_kind::ETA_MFX {
            v1.push(err);
            b1.push(bin);
        } else {
            v2.push(err);
            b2.push(bin);
        }
    }
    let r1 = bin_result(
        "F1",
        "TBFM meter-fix ETA error (estimate minus truth crossing)",
        &v1,
        &b1,
    );
    let mut r2 = bin_result(
        "F2",
        "TBFM scheduled time error (sta_sfx minus truth crossing)",
        &v2,
        &b2,
    );
    r2.notes.push(
        "the feed has no STA at the meter fix; sta_sfx is compared with the crossing of the section's meter fix (unconfirmed, docs/TBFM_SURVEY.md)".into(),
    );
    (r1, r2)
}

fn near_threshold(ap: &AirportRef, lat: f64, lon: f64, radius: f64) -> bool {
    ap.thresholds
        .iter()
        .any(|&(a, b)| dist_m(a, b, lat, lon) <= radius)
}

/// What the truth says about being on the ground at an airport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ground {
    Yes,
    No,
    /// Near a threshold but no barometric altitude to say whether it is low.
    AltitudeUnknown,
    /// Near a threshold with a pressure altitude but no altimeter setting for the field within
    /// 60 minutes to bring it to the field elevation's reference (feet above mean sea level).
    NoAltimeter,
}

/// A truth landing or takeoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundEvent {
    /// The event, Unix ms.
    At(i64),
    /// The leg ends (starts) near a threshold, but with no barometric altitude: nothing shows the
    /// aircraft was on the ground.
    AltitudeUnknown,
    /// The leg ends (starts) near a threshold with a pressure altitude, but the field has no
    /// altimeter setting within 60 minutes: the altitude cannot be compared with the field
    /// elevation.
    NoAltimeter,
    /// No such event in the leg.
    NotFound,
}

/// Whether the truth sample at `t_ms` is on the ground at the airport. The sample's altitude is
/// pressure altitude; the field elevation is feet above mean sea level, so the altitude is first
/// brought to feet above mean sea level with the field's altimeter setting nearest in time:
/// `pressure_alt + (inHg - 29.92) * 1000`. Without a setting the two are never compared.
fn on_ground_at_airport(s: &TruthSample, t_ms: i64, ap: &AirportRef, cfg: &EventConfig) -> Ground {
    if !near_threshold(ap, s.lat, s.lon, cfg.runway_radius_m) {
        return Ground::No;
    }
    let Some(a) = s.baro_alt_ft else {
        return Ground::AltitudeUnknown;
    };
    let Some(inhg) = ap.altimeter_near(t_ms) else {
        return Ground::NoAltimeter;
    };
    let msl_ft = f64::from(a) + (inhg - STANDARD_INHG) * 1000.0;
    if msl_ft <= f64::from(ap.elev_ft) + f64::from(cfg.ground_margin_ft) {
        Ground::Yes
    } else {
        Ground::No
    }
}

fn ground_event(g: Ground, t: i64) -> GroundEvent {
    match g {
        Ground::Yes => GroundEvent::At(t),
        Ground::AltitudeUnknown => GroundEvent::AltitudeUnknown,
        Ground::NoAltimeter => GroundEvent::NoAltimeter,
        Ground::No => GroundEvent::NotFound,
    }
}

/// Truth landing time at `ap` in truth leg `leg_ix`: the last truth point of the leg when it
/// ends on the ground within the radius of a threshold; looks back up to 30 s from the leg end
/// for the last bracketed time.
pub fn landing_time(
    truth: &Truth<'_>,
    leg_ix: u32,
    ap: &AirportRef,
    cfg: &EventConfig,
) -> GroundEvent {
    let Some(l) = truth.legs.get(leg_ix as usize) else {
        return GroundEvent::NotFound;
    };
    for back in 0..=30 {
        let t = l.end_ms - back * 1000;
        if t < l.start_ms {
            break;
        }
        if let Ok(s) = (truth.query)(l.addr, t) {
            return ground_event(on_ground_at_airport(&s, t, ap, cfg), t);
        }
    }
    GroundEvent::NotFound
}

/// Truth takeoff time at `ap` in truth leg `leg_ix`: the first truth point of the leg when it
/// begins on the ground within the radius of a threshold; looks forward up to 30 s from the leg
/// start.
pub fn takeoff_time(
    truth: &Truth<'_>,
    leg_ix: u32,
    ap: &AirportRef,
    cfg: &EventConfig,
) -> GroundEvent {
    let Some(l) = truth.legs.get(leg_ix as usize) else {
        return GroundEvent::NotFound;
    };
    for fwd in 0..=30 {
        let t = l.start_ms + fwd * 1000;
        if t > l.end_ms {
            break;
        }
        if let Ok(s) = (truth.query)(l.addr, t) {
            return ground_event(on_ground_at_airport(&s, t, ap, cfg), t);
        }
    }
    GroundEvent::NotFound
}

/// F3: TFMS estimated arrival time against the truth landing, and estimated departure time
/// against the truth takeoff, by look-ahead. Matching a TFMS entry to an SFDPS flight is by the
/// shared flight plan identifier (`gufi`).
pub fn f3_tfms(
    leg: &Legacy,
    plan: &PlanIndex,
    ctx: &EventCtx<'_>,
    ex: &mut Exclusions,
) -> (MetricResult, MetricResult) {
    let EventCtx {
        assoc,
        truth,
        tix,
        env,
        cfg,
    } = *ctx;
    let (mut va, mut ba, mut vd, mut bd) = (vec![], vec![], vec![], vec![]);
    let mut cache: BTreeMap<(u32, [u8; 8], u8), GroundEvent> = BTreeMap::new();
    let windows = flight_windows(leg);
    let t = &leg.tfms;
    for i in 0..t.kind.len() {
        let kind = t.kind.get(i).copied().unwrap_or(0);
        let (scope, arr) = if kind == tfms_kind::ETA {
            ("F3-arrival", true)
        } else {
            ("F3-departure", false)
        };
        let key: [u8; 12] = t.plan_id.get(i).map_or([0; 12], |p| norm_id(p));
        let issue = t.issue_ms.get(i).copied().unwrap_or(NO_TIME);
        let acid = t.acid.get(i).copied().unwrap_or([0; 8]);
        let est = (issue, t.time_ms.get(i).copied().unwrap_or(NO_TIME), acid);
        let f = match resolve_plan(leg, plan, &windows, &key, est, cfg) {
            Ok(f) => f,
            Err(why) => {
                exclude(ex, scope, why);
                continue;
            }
        };
        let addr = assoc.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE {
            exclude(ex, scope, "flight not associated with truth");
            continue;
        }
        let apt_raw = if arr { t.arr.get(i) } else { t.dep.get(i) };
        let apt: [u8; 8] = apt_raw.copied().unwrap_or([0; 8]);
        let Some(ap) = (env.airport)(name(&apt)) else {
            exclude(ex, scope, "airport not in the model");
            continue;
        };
        let window = estimate_window(&windows, f, issue, est.1, cfg);
        let Some(li) = occurrence_leg(truth, tix, addr, window) else {
            exclude(ex, scope, NO_LEG);
            continue;
        };
        let event = *cache.entry((li, apt, u8::from(arr))).or_insert_with(|| {
            if arr {
                landing_time(truth, li, &ap, cfg)
            } else {
                takeoff_time(truth, li, &ap, cfg)
            }
        });
        let truth_t = match event {
            GroundEvent::At(t) => t,
            GroundEvent::AltitudeUnknown => {
                exclude(
                    ex,
                    scope,
                    if arr {
                        "truth altitude unknown at the landing point, ground not confirmed"
                    } else {
                        "truth altitude unknown at the takeoff point, ground not confirmed"
                    },
                );
                continue;
            }
            GroundEvent::NoAltimeter => {
                exclude(ex, scope, "no altimeter setting for the field");
                continue;
            }
            GroundEvent::NotFound => {
                exclude(
                    ex,
                    scope,
                    if arr {
                        "no truth landing at the destination"
                    } else {
                        "no truth takeoff at the origin"
                    },
                );
                continue;
            }
        };
        if issue == NO_TIME {
            exclude(ex, scope, "no issue time");
            continue;
        }
        let Some(bin) = lookahead_bin(cfg, truth_t - issue) else {
            exclude(
                ex,
                scope,
                "issued after the event or more than 60 min before it",
            );
            continue;
        };
        let err = (t.time_ms.get(i).copied().unwrap_or(0) - truth_t) as f64 / 1000.0;
        if arr {
            va.push(err);
            ba.push(bin);
        } else {
            vd.push(err);
            bd.push(bin);
        }
    }
    let mut ra = bin_result(
        "F3-arrival",
        "TFMS arrival estimate error (estimate minus truth landing)",
        &va,
        &ba,
    );
    ra.notes.push(
        "truth ends at the last bracketed ADS-B report, which can be up to 10 s before touchdown"
            .into(),
    );
    let rd = bin_result(
        "F3-departure",
        "TFMS departure estimate error (estimate minus truth takeoff)",
        &vd,
        &bd,
    );
    (ra, rd)
}

/// One point of an expanded route as far as it could be placed.
#[derive(Clone, Debug)]
enum Slot {
    At(f64, f64),
    /// No coordinates and not in the model.
    Missing,
    /// The identifier names several places and the route has not chosen yet.
    Many(Vec<(f64, f64)>),
}

impl Slot {
    fn place(&self) -> Option<(f64, f64)> {
        match self {
            Slot::At(la, lo) => Some((*la, *lo)),
            _ => None,
        }
    }
}

/// A route as a polyline in degrees, or why it is not one. A route is never joined across a
/// point that could not be placed: that would invent a direct leg the aircraft was not cleared to
/// fly.
#[derive(Clone, Debug)]
enum RouteLine {
    Complete(Vec<(f64, f64)>),
    /// A point between two others has no place.
    Unresolved,
    /// A point names several places and the neighbouring points do not choose between them.
    Ambiguous,
}

/// The place among `cands` that is clearly nearest to `anchor` (the second nearest at least 1.5
/// times as far), else `None`.
fn nearest_clearly(cands: &[(f64, f64)], anchor: (f64, f64)) -> Option<(f64, f64)> {
    let mut d: Vec<(f64, (f64, f64))> = cands
        .iter()
        .map(|&c| (dist_m(anchor.0, anchor.1, c.0, c.1), c))
        .collect();
    d.sort_by(|a, b| a.0.total_cmp(&b.0));
    match (d.first(), d.get(1)) {
        (Some(&(d0, c)), Some(&(d1, _))) if d1 >= 1.5 * d0 => Some(c),
        _ => None,
    }
}

/// Places the points of a route. A repeated identifier is resolved with the route as context: the
/// place clearly nearest to the previous placed point (the next one for the first point).
fn polyline(leg: &Legacy, route: usize, env: &Env<'_>) -> RouteLine {
    let first = leg.routes.first.get(route).copied().unwrap_or(0) as usize;
    let len = leg.routes.len.get(route).copied().unwrap_or(0) as usize;
    let mut slots: Vec<Slot> = Vec::new();
    for k in first..first + len {
        let (lat, lon) = (
            leg.route_points.lat_e7.get(k).copied().unwrap_or(NO_COORD),
            leg.route_points.lon_e7.get(k).copied().unwrap_or(NO_COORD),
        );
        if lat != NO_COORD && lon != NO_COORD {
            slots.push(Slot::At(f64::from(lat) * 1e-7, f64::from(lon) * 1e-7));
            continue;
        }
        let Some(fix) = leg.route_points.fix.get(k) else {
            slots.push(Slot::Missing);
            continue;
        };
        let nm = name(fix);
        slots.push(if let Some((la, lo)) = (env.fix)(nm) {
            Slot::At(la, lo)
        } else {
            match (env.fix_all)(nm).as_slice() {
                [] => Slot::Missing,
                [(la, lo)] => Slot::At(*la, *lo),
                many => Slot::Many(many.to_vec()),
            }
        });
    }
    // Points at either end that have no place (an airport is not a fix) shorten the route.
    while matches!(slots.last(), Some(Slot::Missing)) {
        slots.pop();
    }
    let lead = slots
        .iter()
        .take_while(|s| matches!(s, Slot::Missing))
        .count();
    slots.drain(..lead);
    let mut out: Vec<Slot> = Vec::with_capacity(slots.len());
    for (i, sl) in slots.iter().enumerate() {
        out.push(match sl {
            Slot::Many(c) => {
                let anchor = out
                    .iter()
                    .rev()
                    .find_map(Slot::place)
                    .or_else(|| slots.iter().skip(i + 1).find_map(Slot::place));
                match anchor.and_then(|a| nearest_clearly(c, a)) {
                    Some((la, lo)) => Slot::At(la, lo),
                    None => Slot::Many(c.clone()),
                }
            }
            other => other.clone(),
        });
    }
    if out.iter().any(|s| matches!(s, Slot::Missing)) {
        RouteLine::Unresolved
    } else if out.iter().any(|s| matches!(s, Slot::Many(_))) {
        RouteLine::Ambiguous
    } else {
        RouteLine::Complete(out.iter().filter_map(Slot::place).collect())
    }
}

/// Distance in metres from a point to a polyline (the nearest segment; one point is a point).
pub fn dist_to_polyline(lat: f64, lon: f64, line: &[(f64, f64)]) -> f64 {
    let mut best = f64::MAX;
    if let [only] = line {
        return dist_m(lat, lon, only.0, only.1);
    }
    for w in line.windows(2) {
        let (Some(&(la, lo)), Some(&(lb, ob))) = (w.first(), w.get(1)) else {
            continue;
        };
        let (bx, by) = enu(la, lo, lb, ob);
        let (px, py) = enu(la, lo, lat, lon);
        let l2 = bx * bx + by * by;
        let u = if l2 > 0.0 {
            ((px * bx + py * by) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = (px - u * bx).hypot(py - u * by);
        best = best.min(d);
    }
    best
}

/// R1: route conformance. Over the paired samples of each flight that has an expanded route (the
/// latest one issued at or before the sample), the truth distance from the route, metres, and the
/// share of flight time beyond the limit. Each flight with any time beyond it is listed.
pub fn r1_route_conformance(
    leg: &Legacy,
    p: &PairTable,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &EventConfig,
    ex: &mut Exclusions,
) -> MetricResult {
    let mut routes_of: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for r in 0..leg.routes.flight.len() {
        if let Some(&f) = leg.routes.flight.get(r) {
            routes_of.entry(f).or_default().push(r as u32);
        }
    }
    for v in routes_of.values_mut() {
        v.sort_by_key(|&r| (leg.routes.ts_ms.get(r as usize).copied().unwrap_or(0), r));
    }
    let mut lines: BTreeMap<u32, RouteLine> = BTreeMap::new();
    let mut dists = Vec::new();
    let (mut t_all, mut t_off) = (0i64, 0i64);
    let (mut flights, mut off_flights) = (0u64, 0u64);
    let mut max_share = 0.0f64;
    let mut cases: Vec<(i64, String)> = Vec::new();
    let n = p.t_ms.len();
    let mut i = 0usize;
    while i < n {
        let f = p.flight.get(i).copied().unwrap_or(0);
        let mut j = i;
        while j < n && p.flight.get(j).copied() == Some(f) {
            j += 1;
        }
        let Some(rs) = routes_of.get(&f) else {
            exclude(ex, "R1", "flight has no expanded route");
            i = j;
            continue;
        };
        let (mut f_all, mut f_off, mut f_max, mut first_off) = (0i64, 0i64, 0.0f64, None);
        let mut any = false;
        for k in i..j {
            let t = p.t_ms.get(k).copied().unwrap_or(0);
            let r = rs
                .iter()
                .rev()
                .find(|&&r| leg.routes.ts_ms.get(r as usize).is_some_and(|&ts| ts <= t));
            let Some(&r) = r else {
                exclude(ex, "R1", "no route issued yet at the sample");
                continue;
            };
            let line = lines
                .entry(r)
                .or_insert_with(|| polyline(leg, r as usize, env));
            let line = match &*line {
                RouteLine::Complete(l) if l.len() >= 2 => l,
                RouteLine::Complete(_) => {
                    exclude(ex, "R1", "route has fewer than two resolved points");
                    continue;
                }
                RouteLine::Unresolved => {
                    exclude(ex, "R1", "route has an unresolved intermediate point");
                    continue;
                }
                RouteLine::Ambiguous => {
                    exclude(ex, "R1", "route has an ambiguous fix identifier");
                    continue;
                }
            };
            let own = p.addr.get(k).copied().unwrap_or(0);
            let Ok(s) = (truth.query)(own, t) else {
                continue;
            };
            let d = dist_to_polyline(s.lat, s.lon, line);
            let dt = if k + 1 < j {
                (p.t_ms.get(k + 1).copied().unwrap_or(t) - t).clamp(0, cfg.route_max_dt_ms)
            } else {
                0
            };
            any = true;
            dists.push(d);
            f_all += dt;
            if d > cfg.route_limit_m {
                f_off += dt;
                f_max = f_max.max(d);
                first_off.get_or_insert(t);
            }
        }
        if any {
            flights += 1;
            t_all += f_all;
            t_off += f_off;
            if first_off.is_some() {
                off_flights += 1;
                let cs = leg.flights.callsign.get(f as usize).map_or("", |c| name(c));
                let share = if f_all > 0 {
                    f_off as f64 / f_all as f64
                } else {
                    0.0
                };
                max_share = max_share.max(share);
                cases.push((
                    -(f_max * 1000.0) as i64,
                    format!(
                        "flight {cs}: {:.1}% of time beyond limit, up to {:.2} NM, first at {} ms",
                        share * 100.0,
                        f_max / NM_M,
                        first_off.unwrap_or(0)
                    ),
                ));
            }
        }
        i = j;
    }
    let mut r = new_result(
        "R1",
        "route conformance (truth distance from ERAM expanded route)",
        "m",
    );
    r.rows = group_rows(&dists, &[]);
    r.scalars.push(("flights_evaluated".into(), flights as f64));
    r.scalars
        .push(("flights_with_time_beyond_limit".into(), off_flights as f64));
    r.scalars.push((
        "share_of_time_beyond_limit".into(),
        if t_all > 0 {
            t_off as f64 / t_all as f64
        } else {
            f64::NAN
        },
    ));
    r.scalars.push((
        "max_flight_share".into(),
        if flights > 0 { max_share } else { f64::NAN },
    ));
    cases.sort();
    r.scalars.push((
        "cases_not_listed".into(),
        cases.len().saturating_sub(MAX_CASES) as f64,
    ));
    r.cases = cases.into_iter().take(MAX_CASES).map(|c| c.1).collect();
    r.notes.push(format!(
        "limit {:.0} m; each sample stands for the time to the next, at most {} s",
        cfg.route_limit_m,
        cfg.route_max_dt_ms / 1000
    ));
    r
}

/// Why no crossing time is given for a handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoCrossing {
    /// The units were never seen to change from the transferring to the receiving one.
    NotSeen,
    /// They were, but the truth is not continuous across the change: the time it happened is not
    /// known (no truth between the two sides for longer than [`oa_truth::LEG_GAP_MS`], or no unit
    /// at a probe inside the interval).
    TruthGap,
    /// A truth position in the window lies in the areas of more than one unit: the unit there
    /// is not known and no name order decides it.
    Ambiguous,
}

/// The truth crossing of the boundary from unit `from` to unit `to` nearest the time `around`,
/// Unix ms. Only where the truth is continuous across it: the last sample over `from` and the
/// first over `to` must lie no further apart than the truth's own continuity gap, and the unit is
/// known at every probe of the refinement; otherwise the time is unknown and nothing is
/// interpolated across the gap.
fn boundary_crossing(
    truth: &Truth<'_>,
    env: &Env<'_>,
    addr: u32,
    from: [u8; 8],
    to: [u8; 8],
    around: i64,
    cfg: &EventConfig,
) -> Result<i64, NoCrossing> {
    // An ambiguous probe (inside several units' areas) is unknown for the search, and the
    // handoff is excluded as ambiguous whatever else the window holds.
    let ambiguous = std::cell::Cell::new(false);
    let unit = |t: i64| -> Option<[u8; 8]> {
        let s = (truth.query)(addr, t).ok()?;
        let u = (env.unit_at)(s.lat, s.lon, s.baro_alt_ft);
        if u.is_none() && (env.unit_ambiguous)(s.lat, s.lon) {
            ambiguous.set(true);
        }
        u
    };
    let step = cfg.handoff_step_ms.max(1);
    let mut t = around - cfg.handoff_window_ms;
    let mut prev: Option<(i64, [u8; 8])> = None;
    let mut best: Option<i64> = None;
    let mut seen_gap = false;
    while t <= around + cfg.handoff_window_ms {
        if let Some(u) = unit(t) {
            if let Some((pt, pu)) = prev {
                if pu == from && u == to {
                    match refine_crossing(&unit, from, to, pt, t) {
                        Some(c) if t - pt <= oa_truth::LEG_GAP_MS => {
                            if best.is_none_or(|b| (c - around).abs() < (b - around).abs()) {
                                best = Some(c);
                            }
                        }
                        _ => seen_gap = true,
                    }
                }
            }
            prev = Some((t, u));
        }
        t += step;
    }
    match (best, seen_gap) {
        _ if ambiguous.get() => Err(NoCrossing::Ambiguous),
        (Some(c), _) => Ok(c),
        (None, true) => Err(NoCrossing::TruthGap),
        (None, false) => Err(NoCrossing::NotSeen),
    }
}

/// The first time within `(lo, hi]` at which the unit is `to` rather than `from`, found by
/// bisection to 1 s; `None` when a probe has neither unit (the truth or the model says nothing
/// there, or a third unit is there).
fn refine_crossing(
    unit: &dyn Fn(i64) -> Option<[u8; 8]>,
    from: [u8; 8],
    to: [u8; 8],
    mut lo: i64,
    mut hi: i64,
) -> Option<i64> {
    while hi - lo > 1000 {
        let mid = lo + (hi - lo) / 2;
        let u = unit(mid)?;
        if u == from {
            lo = mid;
        } else if u == to {
            hi = mid;
        } else {
            return None;
        }
    }
    Some(hi)
}

/// H1: handoff initiation and acceptance time against the truth crossing of the boundary between
/// the transferring and receiving unit, seconds, signed (event minus crossing).
pub fn h1_handoffs(
    leg: &Legacy,
    assoc: &Assoc,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &EventConfig,
    ex: &mut Exclusions,
) -> MetricResult {
    let (mut vals, mut kinds) = (vec![], vec![]);
    let h = &leg.handoffs;
    for i in 0..h.flight.len() {
        let ev = h.event.get(i).copied().unwrap_or(0);
        let code = match ev {
            hev::INITIATION => 0u8,
            hev::ACCEPTANCE => 1u8,
            _ => continue,
        };
        let f = h.flight.get(i).copied().unwrap_or(0);
        let addr = assoc.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE {
            exclude(ex, "H1", "flight not associated with truth");
            continue;
        }
        let (from, to) = (
            h.from.get(i).copied().unwrap_or([0; 8]),
            h.to.get(i).copied().unwrap_or([0; 8]),
        );
        if from.iter().all(|&b| b == 0) || to.iter().all(|&b| b == 0) {
            exclude(ex, "H1", "handoff lacks a transferring or receiving unit");
            continue;
        }
        let ts = h.ts_ms.get(i).copied().unwrap_or(NO_TIME);
        if ts == NO_TIME {
            exclude(ex, "H1", "no message time");
            continue;
        }
        match boundary_crossing(truth, env, addr, from, to, ts, cfg) {
            Ok(c) => {
                vals.push((ts - c) as f64 / 1000.0);
                kinds.push(code);
            }
            Err(NoCrossing::TruthGap) => {
                exclude(ex, "H1", "truth missing across the boundary crossing")
            }
            Err(NoCrossing::NotSeen) => {
                exclude(ex, "H1", "no boundary crossing in truth near the event")
            }
            Err(NoCrossing::Ambiguous) => exclude(
                ex,
                "H1",
                "truth position inside more than one ARTCC area, unit ambiguous",
            ),
        }
    }
    let mut r = new_result(
        "H1",
        "handoff time error (event minus truth boundary crossing)",
        "s",
    );
    r.rows = group_rows(&vals, &[("event", &kinds, &["initiation", "acceptance"])]);
    r.notes.push(
        "units are compared exactly as ERAM names them (unit and sector); the boundary function is supplied by the airspace model".into(),
    );
    r
}

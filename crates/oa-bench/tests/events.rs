//! Event metrics (F1 to F3, R1) on made-up worlds: routes with a point that cannot be placed,
//! identifiers that name two places, one aircraft flying two trips, and truth with no barometric
//! altitude. Every identifier and position is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;

use oa_bench::assoc::{index_truth, Assoc};
use oa_bench::geo::from_enu;
use oa_bench::legacy::{
    flight_ix, new_legacy, pad, plan_index, push_route, push_tbfm, push_tfms, push_th, tbfm_kind,
    tfms_kind, Legacy, RoutePointIn, TfmsIn, NO_COORD, NO_TIME,
};
use oa_bench::metrics_event::{f1_f2_tbfm, f3_tfms, r1_route_conformance, EventConfig, EventCtx};
use oa_bench::pair::PairTable;
use oa_bench::stats::{Exclusions, MetricResult};
use oa_bench::synth::{env, LAT0, LON0, T0_MS};
use oa_bench::truth::{empty_env, Env, NoTruth, Truth, TruthLeg, TruthSample};

const ADDR: u32 = 0xA1_0001;

/// One truth leg: first and last time (Unix ms) and the path, east and north metres from the
/// world origin, flown at a constant speed.
type LegPath = (i64, i64, Vec<(f64, f64)>);

fn at_s(pts: &[(f64, f64)], s: f64) -> (f64, f64) {
    let total: f64 = pts.windows(2).map(|w| seg_len(w[0], w[1])).sum();
    let mut left = s.clamp(0.0, 1.0) * total;
    for w in pts.windows(2) {
        let l = seg_len(w[0], w[1]);
        if left <= l || std::ptr::eq(&w[1], pts.last().unwrap()) {
            let f = if l > 0.0 { (left / l).min(1.0) } else { 0.0 };
            return (
                w[0].0 + f * (w[1].0 - w[0].0),
                w[0].1 + f * (w[1].1 - w[0].1),
            );
        }
        left -= l;
    }
    pts[0]
}

fn seg_len(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

fn query(
    legs: &[LegPath],
    alt: Option<i32>,
) -> impl Fn(u32, i64) -> Result<TruthSample, NoTruth> + '_ {
    move |a, t| {
        if a != ADDR {
            return Err(NoTruth::UnknownAircraft);
        }
        for (i, (s, e, pts)) in legs.iter().enumerate() {
            if t >= *s && t <= *e {
                let (x, y) = at_s(pts, (t - s) as f64 / (e - s) as f64);
                let (lat, lon) = from_enu(LAT0, LON0, x, y);
                return Ok(TruthSample {
                    lat,
                    lon,
                    baro_alt_ft: alt,
                    vel_east_mps: 100.0,
                    vel_north_mps: 0.0,
                    vert_rate_fpm: Some(0),
                    sigma95_h_m: 5.0,
                    leg: i as u32,
                });
            }
        }
        Err(NoTruth::OutsideLegs)
    }
}

fn truth_legs(legs: &[LegPath]) -> Vec<TruthLeg> {
    legs.iter()
        .map(|l| TruthLeg {
            addr: ADDR,
            start_ms: l.0,
            end_ms: l.1,
            callsign: None,
        })
        .collect()
}

fn e7(deg: f64) -> i32 {
    (deg * 1e7).round() as i32
}

fn rp(name: &[u8], xy: Option<(f64, f64)>) -> RoutePointIn {
    let (lat, lon) = xy.map_or((NO_COORD, NO_COORD), |(x, y)| {
        let (la, lo) = from_enu(LAT0, LON0, x, y);
        (e7(la), e7(lo))
    });
    RoutePointIn {
        fix: pad(name),
        est_ms: NO_TIME,
        lat_e7: lat,
        lon_e7: lon,
    }
}

fn scalar(m: &MetricResult, k: &str) -> f64 {
    m.scalars.iter().find(|(n, _)| n == k).unwrap().1
}

fn ex_count(ex: &Exclusions, scope: &str, reason: &str) -> u64 {
    ex.iter()
        .filter(|((s, r), _)| *s == scope && *r == reason)
        .map(|(_, n)| *n)
        .sum()
}

/// R1 over a flight that flies `path` with the expanded route `points`.
fn r1(points: &[RoutePointIn], path: &[(f64, f64)], env: &Env<'_>) -> (MetricResult, Exclusions) {
    let legs: Vec<LegPath> = vec![(T0_MS, T0_MS + 1_000_000, path.to_vec())];
    let q = query(&legs, Some(30_000));
    let tl = truth_legs(&legs);
    let truth = Truth {
        query: &q,
        legs: &tl,
    };
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    push_route(&mut leg, f, T0_MS - 1000, points);
    let mut p = PairTable::default();
    for k in 0..100i64 {
        p.t_ms.push(T0_MS + k * 10_000);
        p.flight.push(f);
        p.addr.push(ADDR);
    }
    let mut ex = Exclusions::new();
    let r = r1_route_conformance(&leg, &p, &truth, env, &EventConfig::default(), &mut ex);
    (r, ex)
}

const A: (f64, f64) = (-50_000.0, 0.0);
const B: (f64, f64) = (0.0, 40_000.0);
const C: (f64, f64) = (50_000.0, 0.0);

// REQ: OA-BENCH-050
#[test]
fn a_route_is_never_joined_across_a_point_that_cannot_be_placed() {
    // A to C by way of B, but B is not in the model: the dogleg is not the straight line A-C.
    let pts = [rp(b"AAA", Some(A)), rp(b"BBB", None), rp(b"CCC", Some(C))];
    let (r, ex) = r1(&pts, &[A, B, C], &empty_env());
    assert_eq!(scalar(&r, "flights_evaluated"), 0.0);
    assert!(ex_count(&ex, "R1", "route has an unresolved intermediate point") > 0);
    // Truth cutting straight across is not called conforming either.
    let (r, _) = r1(&pts, &[A, C], &empty_env());
    assert_eq!(scalar(&r, "flights_evaluated"), 0.0);
}

// REQ: OA-BENCH-050
#[test]
fn unplaced_end_points_of_a_route_are_trimmed_not_joined() {
    // Airports are not fixes: an unplaced first and last point only shorten the route.
    let pts = [
        rp(b"KORD", None),
        rp(b"AAA", Some(A)),
        rp(b"CCC", Some(C)),
        rp(b"KMDW", None),
    ];
    let (r, _) = r1(&pts, &[A, C], &empty_env());
    assert_eq!(scalar(&r, "flights_evaluated"), 1.0);
    assert_eq!(scalar(&r, "flights_with_time_beyond_limit"), 0.0);
}

fn near_and_far(name: &str) -> Vec<(f64, f64)> {
    if name == "DUPX" {
        vec![
            from_enu(LAT0, LON0, B.0, B.1),
            from_enu(LAT0, LON0, 0.0, -300_000.0),
        ]
    } else if name == "TWIN" {
        vec![
            from_enu(LAT0, LON0, 0.0, 40_000.0),
            from_enu(LAT0, LON0, 0.0, -40_000.0),
        ]
    } else {
        Vec::new()
    }
}

// REQ: OA-BENCH-051
#[test]
fn a_repeated_identifier_is_resolved_by_the_neighbouring_points_of_the_route() {
    let fix = |_: &str| None;
    let fix_all = |n: &str| near_and_far(n);
    let e = Env {
        fix: &fix,
        fix_all: &fix_all,
        ..empty_env()
    };
    let pts = [rp(b"AAA", Some(A)), rp(b"DUPX", None), rp(b"CCC", Some(C))];
    let (r, _) = r1(&pts, &[A, B, C], &e);
    assert_eq!(scalar(&r, "flights_evaluated"), 1.0);
    assert_eq!(scalar(&r, "flights_with_time_beyond_limit"), 0.0);
}

// REQ: OA-BENCH-051
#[test]
fn a_repeated_identifier_the_route_cannot_separate_excludes_the_route() {
    let fix = |_: &str| None;
    let fix_all = |n: &str| near_and_far(n);
    let e = Env {
        fix: &fix,
        fix_all: &fix_all,
        ..empty_env()
    };
    // Both places are as far from the previous point: no context decides.
    let pts = [rp(b"AAA", Some(A)), rp(b"TWIN", None), rp(b"CCC", Some(C))];
    let (r, ex) = r1(&pts, &[A, B, C], &e);
    assert_eq!(scalar(&r, "flights_evaluated"), 0.0);
    assert!(ex_count(&ex, "R1", "route has an ambiguous fix identifier") > 0);
}

/// Truth: one aircraft flying the same path from the origin airport through the meter fix to the
/// destination airport twice, the second time 5,000 s later.
fn two_trips() -> Vec<LegPath> {
    let path = vec![(-50_000.0, -50_000.0), (50_000.0, 50_000.0)];
    vec![
        (T0_MS, T0_MS + 1_000_000, path.clone()),
        (T0_MS + 5_000_000, T0_MS + 6_000_000, path),
    ]
}

struct Trips {
    leg: Legacy,
    assoc: Assoc,
}

/// Two SFDPS flights (plans `PLAN0`, `PLAN1`) of the one aircraft, each with perfect estimates:
/// meter fix `MFXA` crossing at the middle of the trip, takeoff at its start, landing at its end.
fn trips(with_samples: bool) -> Trips {
    let mut leg = new_legacy();
    let mut assoc = Assoc::default();
    for (k, plan) in [b"PLAN0", b"PLAN1"].into_iter().enumerate() {
        let f = flight_ix(&mut leg, k as u128 + 1).unwrap();
        leg.flights.plan_id[f as usize] = pad(plan);
        assoc.addr.push(ADDR);
        let t0 = T0_MS + k as i64 * 5_000_000;
        if with_samples {
            for s in 0..10i64 {
                push_th(
                    &mut leg,
                    f,
                    t0 + s * 100_000,
                    NO_TIME,
                    0,
                    0,
                    30_000,
                    None,
                    false,
                );
            }
        }
        push_tbfm(
            &mut leg,
            pad(plan),
            pad(b"AID"),
            pad(b"MFXA"),
            tbfm_kind::ETA_MFX,
            t0 + 500_000,
            t0 + 200_000,
        );
        for (kind, time, issue) in [
            (tfms_kind::ETA, t0 + 1_000_000, t0 + 500_000),
            (tfms_kind::ETD, t0, t0 - 300_000),
        ] {
            push_tfms(
                &mut leg,
                &TfmsIn {
                    plan_id: pad(plan),
                    acid: pad(b"AID"),
                    dep: pad(b"KDEP"),
                    arr: pad(b"KLND"),
                    kind,
                    time_ms: time,
                    issue_ms: issue,
                },
            );
        }
    }
    Trips { leg, assoc }
}

fn score(
    t: &Trips,
    legs: &[LegPath],
    alt: Option<i32>,
    env: &Env<'_>,
) -> (Vec<MetricResult>, Exclusions) {
    let q = query(legs, alt);
    let tl = truth_legs(legs);
    let truth = Truth {
        query: &q,
        legs: &tl,
    };
    let tix = index_truth(&tl);
    let cfg = EventConfig::default();
    let ctx = EventCtx {
        assoc: &t.assoc,
        truth: &truth,
        tix: &tix,
        env,
        cfg: &cfg,
    };
    let plan: HashMap<[u8; 12], u32> = plan_index(&t.leg);
    let mut ex = Exclusions::new();
    let (f1, _) = f1_f2_tbfm(&t.leg, &plan, &ctx, &mut ex);
    let (fa, fd) = f3_tfms(&t.leg, &plan, &ctx, &mut ex);
    (vec![f1, fa, fd], ex)
}

fn total_n(m: &MetricResult) -> u64 {
    m.rows
        .iter()
        .find(|r| r.label == "all")
        .map_or(0, |r| r.stats.n)
}

fn worst(m: &MetricResult) -> f64 {
    m.rows
        .iter()
        .find(|r| r.label == "all")
        .map_or(f64::INFINITY, |r| r.stats.max)
}

// REQ: OA-BENCH-052
#[test]
fn repeated_trips_of_one_aircraft_each_score_against_their_own_occurrence() {
    for with_samples in [true, false] {
        let t = trips(with_samples);
        let (m, ex) = score(&t, &two_trips(), Some(500), &env());
        for r in &m {
            assert_eq!(total_n(r), 2, "{} samples {with_samples}: {ex:?}", r.id);
            assert!(worst(r) < 0.5, "{} error {}", r.id, worst(r));
        }
    }
}

// REQ: OA-BENCH-053
#[test]
fn an_unknown_barometric_altitude_is_not_evidence_of_being_on_the_ground() {
    // The truth has no barometric altitude anywhere: the legs end over the threshold, but
    // nothing says the aircraft was on the ground.
    let t = trips(true);
    let (m, ex) = score(&t, &two_trips(), None, &env());
    assert_eq!(total_n(&m[1]), 0, "arrival scored");
    assert_eq!(total_n(&m[2]), 0, "departure scored");
    assert_eq!(
        ex_count(
            &ex,
            "F3-arrival",
            "truth altitude unknown at the landing point, ground not confirmed"
        ),
        2
    );
    assert_eq!(
        ex_count(
            &ex,
            "F3-departure",
            "truth altitude unknown at the takeoff point, ground not confirmed"
        ),
        2
    );
}

/// An environment whose `MFXA` names two places: the real one (the origin, inside the zone) and
/// another 300 km away.
fn two_places(second: (f64, f64)) -> Vec<(f64, f64)> {
    vec![
        from_enu(LAT0, LON0, 0.0, 0.0),
        from_enu(LAT0, LON0, second.0, second.1),
    ]
}

// REQ: OA-BENCH-051
#[test]
fn a_meter_fix_named_twice_is_the_one_inside_the_zone_or_excluded() {
    let t = trips(true);
    let base = env();
    let fix = |_: &str| None;
    let far = |n: &str| {
        if n == "MFXA" {
            two_places((300_000.0, 0.0))
        } else {
            Vec::new()
        }
    };
    let e = Env {
        fix: &fix,
        fix_all: &far,
        ..base
    };
    let (m, _) = score(&t, &two_trips(), Some(500), &e);
    assert_eq!(total_n(&m[0]), 2, "the in-zone place is the meter fix");
    let both = |n: &str| {
        if n == "MFXA" {
            two_places((20_000.0, 0.0))
        } else {
            Vec::new()
        }
    };
    let e = Env {
        fix: &fix,
        fix_all: &both,
        ..base
    };
    let (m, ex) = score(&t, &two_trips(), Some(500), &e);
    assert_eq!(total_n(&m[0]), 0);
    assert_eq!(
        ex_count(&ex, "F1", "meter fix names more than one place"),
        2
    );
}

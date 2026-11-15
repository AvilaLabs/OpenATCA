//! Scoring that must not invent a reference or a perfect value: handoff crossings across missing
//! truth, statistics of extreme finite values, a passage across a hole in the truth, and a
//! squared-millisecond sum that overflows. Every identifier and position is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::Assoc;
use oa_bench::chunked::MsHist;
use oa_bench::geo::from_enu;
use oa_bench::legacy::{flight_ix, hev, new_legacy, pad, push_handoff};
use oa_bench::metrics_event::{fix_passages, h1_handoffs, EventConfig};
use oa_bench::stats::{stats_of, Exclusions};
use oa_bench::synth::{LAT0, LON0};
use oa_bench::truth::{empty_env, NoTruth, Truth, TruthLeg, TruthSample};

const ADDR: u32 = 0xA1_0001;
const TS: i64 = 1_000_000_000;

fn sample(lat: f64, lon: f64) -> TruthSample {
    TruthSample {
        lat,
        lon,
        baro_alt_ft: Some(30_000),
        vel_east_mps: 100.0,
        vel_north_mps: 0.0,
        vert_rate_fpm: Some(0),
        sigma95_h_m: 5.0,
        leg: 0,
    }
}

fn ex_count(ex: &Exclusions, scope: &str, reason: &str) -> u64 {
    ex.iter()
        .filter(|((s, r), _)| *s == scope && *r == reason)
        .map(|(_, n)| *n)
        .sum()
}

/// H1 for one acceptance at `TS` from unit A to unit B. The aircraft is over A (latitude 5) until
/// `cross_ms`, over B (latitude 25) after; no truth in `hole` (start, end inclusive).
fn h1(cross_ms: i64, hole: Option<(i64, i64)>) -> (oa_bench::stats::MetricResult, Exclusions) {
    let q = move |a: u32, t: i64| -> Result<TruthSample, NoTruth> {
        if a != ADDR {
            return Err(NoTruth::UnknownAircraft);
        }
        if hole.is_some_and(|(s, e)| t >= s && t <= e) {
            return Err(NoTruth::NoNearbyReport);
        }
        Ok(sample(if t < cross_ms { 5.0 } else { 25.0 }, -88.0))
    };
    let legs = [TruthLeg {
        addr: ADDR,
        start_ms: 0,
        end_ms: 2 * TS,
        callsign: None,
    }];
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let unit = |lat: f64, _lon: f64, _alt: Option<i32>| -> Option<[u8; 8]> {
        if lat < 10.0 {
            Some(pad::<8>(b"AAA"))
        } else if lat > 20.0 {
            Some(pad::<8>(b"BBB"))
        } else {
            None
        }
    };
    let mut env = empty_env();
    env.unit_at = &unit;
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    push_handoff(
        &mut leg,
        f,
        TS,
        hev::ACCEPTANCE,
        pad::<8>(b"AAA"),
        pad::<8>(b"BBB"),
    );
    let assoc = Assoc {
        addr: vec![ADDR],
        ..Assoc::default()
    };
    let mut ex = Exclusions::new();
    let r = h1_handoffs(&leg, &assoc, &truth, &env, &EventConfig::default(), &mut ex);
    (r, ex)
}

// REQ: OA-BENCH-100
#[test]
fn a_handoff_crossing_across_a_long_truth_gap_is_excluded_and_counted() {
    // Over A until 5 minutes before the event, nothing for 5 minutes, then over B.
    let (r, ex) = h1(TS - 20_000, Some((TS - 300_000, TS - 30_000)));
    assert_eq!(r.rows[0].stats.n, 0, "{:?}", r.rows[0].stats);
    assert_eq!(
        ex_count(&ex, "H1", "truth missing across the boundary crossing"),
        1,
        "{ex:?}"
    );
}

// REQ: OA-BENCH-100
#[test]
fn a_handoff_crossing_inside_continuous_truth_is_still_scored() {
    let (r, ex) = h1(TS - 20_000, None);
    assert_eq!(r.rows[0].stats.n, 1, "{ex:?}");
    assert!(
        (r.rows[0].stats.mean - 20.0).abs() <= 1.0,
        "{:?}",
        r.rows[0].stats
    );
    assert!(ex.is_empty(), "{ex:?}");
}

// REQ: OA-BENCH-100
#[test]
fn a_crossing_with_a_short_truth_gap_that_hides_the_crossing_is_excluded() {
    // A 60 s hole is inside the continuity gap, but the crossing is inside it: unknown probes.
    let (r, ex) = h1(TS - 20_000, Some((TS - 60_000, TS - 5_000)));
    assert_eq!(r.rows[0].stats.n, 0);
    assert_eq!(
        ex_count(&ex, "H1", "truth missing across the boundary crossing"),
        1,
        "{ex:?}"
    );
}

// REQ: OA-BENCH-101
#[test]
fn rms_of_values_whose_squares_overflow_is_not_zero() {
    let s = stats_of(&[1e200]);
    assert_eq!(s.n, 1);
    assert_eq!(s.rms, 1e200, "{s:?}");
    let s = stats_of(&[f64::MAX, f64::MAX, -f64::MAX]);
    assert!((s.rms - f64::MAX).abs() <= f64::MAX * 1e-15, "{s:?}");
    assert_eq!(s.max, f64::MAX);
    assert!((s.mean - f64::MAX / 3.0).abs() <= f64::MAX * 1e-15, "{s:?}");
    let s = stats_of(&[f64::MAX, f64::MAX]);
    assert_eq!(s.mean, f64::MAX, "{s:?}");
}

// REQ: OA-BENCH-101
#[test]
fn rms_of_subnormal_values_is_not_zero() {
    let tiny = f64::from_bits(1);
    let s = stats_of(&[tiny, -tiny, 3.0 * tiny, -3.0 * tiny]);
    // sqrt((1 + 1 + 9 + 9) / 4) = sqrt(5) units of the smallest subnormal.
    // The result is a whole number of the smallest subnormal: 2.24 rounds to 2.
    assert!(s.rms >= 2.0 * tiny && s.rms <= 3.0 * tiny, "{s:?}");
    assert_eq!(s.max, 3.0 * tiny);
}

// REQ: OA-BENCH-101
#[test]
fn one_huge_value_among_ordinary_ones_gives_its_rms() {
    let s = stats_of(&[1.0, 2.0, -3.0, 1e200]);
    let want = 1e200 / 2.0; // sqrt(1e400 / 4)
    assert!((s.rms - want).abs() <= want * 1e-12, "{s:?}");
    let s = stats_of(&[1e-200, 1e-200]);
    assert!((s.rms - 1e-200).abs() <= 1e-212, "{s:?}");
}

// REQ: OA-BENCH-103
#[test]
fn a_squared_millisecond_sum_that_overflows_is_not_a_perfect_rms() {
    let mut h = MsHist::default();
    for _ in 0..8 {
        h.add(i64::MAX);
    }
    let s = h.stats();
    assert_eq!(s.n, 8);
    assert!(s.rms.is_nan() || s.rms > 9.0e15, "{s:?}");
}

// REQ: OA-BENCH-103
#[test]
fn a_fix_passage_is_not_placed_inside_a_hole_in_the_truth() {
    // East at 100 m/s along the equator-ish line; the fix is 50 km along. No truth for 40 s
    // around it, so the closest approach is unknown: no passage time may be given.
    let hole = (480_000i64, 520_000i64);
    let q = move |_: u32, t: i64| -> Result<TruthSample, NoTruth> {
        if t >= hole.0 && t <= hole.1 {
            return Err(NoTruth::NoNearbyReport);
        }
        let (lat, lon) = from_enu(LAT0, LON0, t as f64 / 1000.0 * 100.0, 0.0);
        Ok(sample(lat, lon))
    };
    let legs = [TruthLeg {
        addr: ADDR,
        start_ms: 0,
        end_ms: 1_000_000,
        callsign: None,
    }];
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let (flat, flon) = from_enu(LAT0, LON0, 50_000.0, 0.0);
    let p = fix_passages(&truth, 0, flat, flon, &EventConfig::default());
    assert!(!p.is_empty());
    assert!(p.iter().all(|x| x.cross_ms.is_none()), "{p:?}");
}

// REQ: OA-BENCH-111
#[test]
fn a_handoff_whose_truth_passes_through_an_ambiguous_area_is_excluded_and_counted() {
    // Over A (latitude 5) until 20 s before the event, in the overlap of A and B (latitude 15)
    // for 10 s, then over B. Name order would put the overlap in A and score a crossing 10 s
    // late; here no unit is known there and the handoff is excluded.
    let q = |a: u32, t: i64| -> Result<TruthSample, NoTruth> {
        if a != ADDR {
            return Err(NoTruth::UnknownAircraft);
        }
        let lat = if t < TS - 20_000 {
            5.0
        } else if t < TS - 10_000 {
            15.0
        } else {
            25.0
        };
        Ok(sample(lat, -88.0))
    };
    let legs = [TruthLeg {
        addr: ADDR,
        start_ms: 0,
        end_ms: 2 * TS,
        callsign: None,
    }];
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let unit = |lat: f64, _lon: f64, _alt: Option<i32>| -> Option<[u8; 8]> {
        if lat < 10.0 {
            Some(pad::<8>(b"AAA"))
        } else if lat > 20.0 {
            Some(pad::<8>(b"BBB"))
        } else {
            None
        }
    };
    let ambiguous = |lat: f64, _lon: f64| (10.0..=20.0).contains(&lat);
    let mut env = empty_env();
    env.unit_at = &unit;
    env.unit_ambiguous = &ambiguous;
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    push_handoff(
        &mut leg,
        f,
        TS,
        hev::ACCEPTANCE,
        pad::<8>(b"AAA"),
        pad::<8>(b"BBB"),
    );
    let assoc = Assoc {
        addr: vec![ADDR],
        ..Assoc::default()
    };
    let mut ex = Exclusions::new();
    let r = h1_handoffs(&leg, &assoc, &truth, &env, &EventConfig::default(), &mut ex);
    assert_eq!(r.rows[0].stats.n, 0);
    assert_eq!(
        ex_count(
            &ex,
            "H1",
            "truth position inside more than one ARTCC area, unit ambiguous"
        ),
        1,
        "{ex:?}"
    );
}

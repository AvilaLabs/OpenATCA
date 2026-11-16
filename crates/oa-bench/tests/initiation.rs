//! T8 initiation delay: the first associated position of the trip against the zone entry. Every
//! identifier and position is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::Assoc;
use oa_bench::legacy::{flight_ix, new_legacy, push_th};
use oa_bench::metrics_flow::{t7_t8_presence, PresenceConfig};
use oa_bench::stats::MetricResult;
use oa_bench::thindex::build_th_index;
use oa_bench::truth::{empty_env, NoTruth, Truth, TruthLeg, TruthSample};

const ADDR: u32 = 0xA1_0002;
const ENTRY_MS: i64 = 1_000_000;

fn sample(lat: f64) -> TruthSample {
    TruthSample {
        lat,
        lon: -88.0,
        baro_alt_ft: Some(30_000),
        vel_east_mps: 100.0,
        vel_north_mps: 0.0,
        vert_rate_fpm: Some(0),
        sigma95_h_m: 5.0,
        leg: 0,
    }
}

fn scalar(r: &MetricResult, name: &str) -> f64 {
    r.scalars.iter().find(|s| s.0 == name).unwrap().1
}

/// T8 for one truth leg from 0 s that is outside the zone (latitude 5) until `ENTRY_MS` and
/// inside it (latitude 25) after, with ERAM positions at `pos_ms`.
fn t8(pos_ms: &[i64]) -> MetricResult {
    let q = |a: u32, t: i64| -> Result<TruthSample, NoTruth> {
        if a != ADDR {
            return Err(NoTruth::UnknownAircraft);
        }
        Ok(sample(if t < ENTRY_MS { 5.0 } else { 25.0 }))
    };
    let legs = [TruthLeg {
        addr: ADDR,
        start_ms: 0,
        end_ms: 2_000_000,
        callsign: None,
        start: Default::default(),
    }];
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let in_zone = |lat: f64, _lon: f64| lat > 20.0;
    let mut env = empty_env();
    env.in_zone = &in_zone;
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    for &t in pos_ms {
        push_th(
            &mut leg,
            f,
            t,
            t,
            250_000_000,
            -880_000_000,
            300,
            None,
            false,
        );
    }
    let ix = build_th_index(&leg.th, leg.flights.gufi.len());
    let assoc = Assoc {
        addr: vec![ADDR],
        ..Assoc::default()
    };
    t7_t8_presence(&leg, &ix, &assoc, &truth, &env, &PresenceConfig::default()).1
}

// REQ: OA-BENCH-117
#[test]
fn a_track_established_long_before_entry_is_counted_not_scored_as_a_late_start() {
    // Positions at 0 s and 1060 s, entry at 1000 s: tracked since 0 s.
    let r = t8(&[0, 1_060_000]);
    assert_eq!(
        r.rows[0].stats.n, 0,
        "scored as a delay: {:?}",
        r.rows[0].stats
    );
    assert_eq!(scalar(&r, "established_before_entry"), 1.0);
}

// REQ: OA-BENCH-117
#[test]
fn a_pop_up_after_entry_gives_its_delay() {
    let r = t8(&[ENTRY_MS + 20_000, ENTRY_MS + 80_000]);
    assert_eq!(r.rows[0].stats.n, 1);
    assert_eq!(r.rows[0].stats.mean, 20.0);
    assert_eq!(scalar(&r, "established_before_entry"), 0.0);
}

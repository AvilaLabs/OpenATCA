//! A tracker's output scored through the unchanged metric code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_bench::legacy::NO_ALT;
use oa_bench::run::{find, run_benchmark, RunConfig};
use oa_bench::synth::{build_world, env, truth_at, Injected, T0_MS};
use oa_bench::tracker_input::{
    legacy_from_track_samples, legacy_from_track_samples_by_leg, SamplesError, TrackSamples,
};
use oa_bench::truth::Truth;

struct Cols {
    addr: Vec<u32>,
    t_ns: Vec<i64>,
    lat: Vec<i32>,
    lon: Vec<i32>,
    alt: Vec<i32>,
    ve: Vec<f32>,
    vn: Vec<f32>,
    coast: Vec<bool>,
}

fn view(c: &Cols) -> TrackSamples<'_> {
    TrackSamples {
        addr: &c.addr,
        t_ns: &c.t_ns,
        lat_e7: &c.lat,
        lon_e7: &c.lon,
        alt_ft: &c.alt,
        vel_east_mps: &c.ve,
        vel_north_mps: &c.vn,
        coast: &c.coast,
    }
}

/// The synthetic world's truth sampled at 1 s as a "tracker output", with a constant error:
/// `north_m` metres north, `speed_mps` metres per second too fast, altitude `alt_err_ft` too high.
fn output_with_error(
    north_m: f64,
    speed_mps: f64,
    alt_err_ft: i32,
) -> (Cols, oa_bench::synth::World) {
    let w = build_world(&Injected::default());
    let mut c = Cols {
        addr: vec![],
        t_ns: vec![],
        lat: vec![],
        lon: vec![],
        alt: vec![],
        ve: vec![],
        vn: vec![],
        coast: vec![],
    };
    for l in &w.legs {
        let mut t = l.start_ms + 2000;
        while t <= l.end_ms - 2000 {
            if let Ok(s) = truth_at(&w.aircraft, l.addr, t) {
                let v = s.vel_east_mps.hypot(s.vel_north_mps).max(1e-9);
                c.addr.push(l.addr);
                c.t_ns.push(t * 1_000_000);
                c.lat
                    .push(((s.lat + north_m / 111_195.0) * 1e7).round() as i32);
                c.lon.push((s.lon * 1e7).round() as i32);
                c.alt.push(s.baro_alt_ft.map_or(NO_ALT, |a| a + alt_err_ft));
                c.ve.push((s.vel_east_mps * (1.0 + speed_mps / v)) as f32);
                c.vn.push((s.vel_north_mps * (1.0 + speed_mps / v)) as f32);
                c.coast.push(false);
            }
            t += 1000;
        }
    }
    (c, w)
}

fn rms_of(out: &oa_bench::run::RunOutput, id: &str) -> f64 {
    find(out, id).unwrap().rows[0].stats.rms
}

// REQ: OA-TRK-021
#[test]
fn a_tracker_output_is_scored_by_the_unchanged_metrics() {
    let (c, w) = output_with_error(100.0, 2.0, 50);
    let leg = legacy_from_track_samples(&view(&c)).unwrap();
    // One flight per address, keyed by it, with every row kept.
    assert_eq!(leg.flights.gufi.len(), w.aircraft.len());
    assert_eq!(leg.th.flight.len(), c.addr.len());
    assert!(leg
        .flights
        .addr
        .iter()
        .all(|&a| a != oa_bench::legacy::NONE));
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let out = run_benchmark(&leg, &truth, &env(), &RunConfig::default());
    let (cross, along) = (rms_of(&out, "T1"), rms_of(&out, "T2-raw"));
    println!(
        "T1 rms {cross:.2} m, T2 rms {along:.2} m, T3 speed rms {:.2} kt, T4 rms {:.1} ft",
        rms_of(&out, "T3-speed"),
        rms_of(&out, "T4")
    );
    // A 100 m error in a known direction splits into cross and along; its length is recovered.
    assert!(((cross * cross + along * along).sqrt() - 100.0).abs() < 2.0);
    // T3 speed: 2 m/s too fast is 3.89 kt; T4: 50 ft.
    assert!((rms_of(&out, "T3-speed") - 2.0 / 0.514_444_444_444).abs() < 0.2);
    assert!((rms_of(&out, "T4") - 50.0).abs() < 1.0);
    // An exact copy of truth scores zero.
    let (c0, _) = output_with_error(0.0, 0.0, 0);
    let leg0 = legacy_from_track_samples(&view(&c0)).unwrap();
    let out0 = run_benchmark(&leg0, &truth, &env(), &RunConfig::default());
    assert!(rms_of(&out0, "T1") < 0.5 && rms_of(&out0, "T2-raw") < 0.5);
}

// REQ: OA-TRK-022
#[test]
fn track_sample_columns_of_different_lengths_are_refused() {
    let (mut c, _) = output_with_error(0.0, 0.0, 0);
    c.coast.pop();
    assert_eq!(
        legacy_from_track_samples(&view(&c)).err(),
        Some(SamplesError::ColumnLength)
    );
    let _ = T0_MS;
}

fn one_address_two_trips() -> Cols {
    let mut c = Cols {
        addr: vec![],
        t_ns: vec![],
        lat: vec![],
        lon: vec![],
        alt: vec![],
        ve: vec![],
        vn: vec![],
        coast: vec![],
    };
    // Ten rows one second apart, then ten more starting 300 s after the last of the first.
    for base_s in [0i64, 309] {
        for k in 0..10i64 {
            c.addr.push(0xA0_0001);
            c.t_ns.push((base_s + k) * 1_000_000_000);
            c.lat.push(410_000_000);
            c.lon.push(-880_000_000);
            c.alt.push(NO_ALT);
            c.ve.push(f32::NAN);
            c.vn.push(f32::NAN);
            c.coast.push(false);
        }
    }
    c
}

fn t5_all_n(leg: &oa_bench::legacy::Legacy) -> u64 {
    let ix = oa_bench::thindex::build_th_index(&leg.th, leg.flights.gufi.len());
    let mut h = oa_bench::chunked::T5Hists::default();
    oa_bench::chunked::t5_add(&mut h, leg, &ix);
    h.all.stats().n
}

// REQ: OA-BENCH-118
#[test]
fn the_publication_interval_is_never_taken_across_two_trips_of_one_aircraft() {
    let c = one_address_two_trips();
    let legs = [
        oa_bench::truth::TruthLeg {
            addr: 0xA0_0001,
            start_ms: 0,
            end_ms: 9_000,
            callsign: None,
            start: Default::default(),
        },
        oa_bench::truth::TruthLeg {
            addr: 0xA0_0001,
            start_ms: 309_000,
            end_ms: 318_000,
            callsign: None,
            start: Default::default(),
        },
    ];
    let by_leg = legacy_from_track_samples_by_leg(&view(&c), &legs).unwrap();
    assert_eq!(by_leg.flights.gufi.len(), 2);
    assert_eq!(by_leg.th.flight.len(), 20);
    // 9 steps in each trip; the 300 s step between them is not a publication interval.
    assert_eq!(t5_all_n(&by_leg), 18);
    // Pooled by address, as `legacy_from_track_samples` does, the 300 s step is in the distribution.
    let pooled = legacy_from_track_samples(&view(&c)).unwrap();
    assert_eq!(pooled.flights.gufi.len(), 1);
    assert_eq!(t5_all_n(&pooled), 19);
    // An address no leg names is one flight, and the legs of other addresses do not split it.
    let other = [oa_bench::truth::TruthLeg {
        addr: 0xA0_0002,
        start_ms: 100_000,
        end_ms: 200_000,
        callsign: None,
        start: Default::default(),
    }];
    let un = legacy_from_track_samples_by_leg(&view(&c), &other).unwrap();
    assert_eq!(un.flights.gufi.len(), 1);
}

// REQ: OA-BENCH-120
#[test]
fn scoring_track_samples_does_not_count_the_gap_between_two_trips_as_a_publication_interval() {
    use oa_bench::run::score_track_samples;
    use oa_bench::truth::{empty_env, NoTruth, TruthLeg};
    let c = one_address_two_trips();
    let legs = [
        TruthLeg {
            addr: 0xA0_0001,
            start_ms: 0,
            end_ms: 9_000,
            callsign: None,
            start: Default::default(),
        },
        TruthLeg {
            addr: 0xA0_0001,
            start_ms: 309_000,
            end_ms: 318_000,
            callsign: None,
            start: Default::default(),
        },
    ];
    let q = |_: u32, _: i64| Err(NoTruth::OutsideLegs);
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let out = score_track_samples(&view(&c), &truth, &empty_env(), &RunConfig::default()).unwrap();
    let t5 = find(&out, "T5").unwrap();
    let all = t5.rows.iter().find(|r| r.label == "all").unwrap();
    // 9 one-second steps in each trip; the 309 s step between them is not an interval.
    assert_eq!(all.stats.n, 18, "T5 intervals: {}", all.stats.n);
    assert!(all.stats.p99 < 2.0, "T5 p99 {}", all.stats.p99);
}

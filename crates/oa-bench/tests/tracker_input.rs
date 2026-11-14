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
use oa_bench::tracker_input::{legacy_from_track_samples, SamplesError, TrackSamples};
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

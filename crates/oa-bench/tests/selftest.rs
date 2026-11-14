//! The synthetic self-test (M2_DESIGN section 4): every metric must recover its injected value
//! within a stated tolerance, the offset-drift guard must withhold, and the swap must be found.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::{associate, astatus, index_truth, AssocConfig};
use oa_bench::legacy::{new_legacy, push_th, set_addr, NONE, NO_TIME};
use oa_bench::pair::{build_pairs, PairConfig};
use oa_bench::run::{find, run_benchmark, RunConfig};
use oa_bench::stats::{method, MetricResult, Stats};
use oa_bench::synth::{
    build_world, check_ok, env, run_world, self_test, self_test_config, truth_at, Injected, T0_MS,
};
use oa_bench::thindex::build_th_index;
use oa_bench::truth::{NoTruth, Truth, TruthLeg, TruthSample};

fn row<'a>(m: &'a MetricResult, label: &str) -> &'a Stats {
    &m.rows
        .iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| panic!("{} has no row {label}", m.id))
        .stats
}

fn scalar(m: &MetricResult, k: &str) -> f64 {
    m.scalars.iter().find(|(n, _)| n == k).unwrap().1
}

// REQ: OA-BENCH-025
#[test]
fn every_injected_value_is_recovered_within_its_tolerance() {
    let checks = self_test();
    assert!(checks.len() >= 24, "{} checks", checks.len());
    for c in &checks {
        assert!(
            check_ok(c),
            "{} {}: injected {} recovered {} tolerance {}",
            c.metric,
            c.what,
            c.injected,
            c.recovered,
            c.tolerance
        );
    }
}

/// The world without the 20 km error of the callsign-only aircraft that fails the 3 NM check:
/// that aircraft stays in the headline numbers (association must not depend on ERAM's error), so
/// the recovery of the other injected values needs the world without its error.
fn clean() -> Injected {
    Injected {
        gate_failed_offset_m: 0.0,
        ..Injected::default()
    }
}

fn baseline() -> oa_bench::run::RunOutput {
    run_world(&clean(), &self_test_config()).1
}

/// The full world, with the gate-failed aircraft's 20 km error.
fn full_baseline() -> oa_bench::run::RunOutput {
    run_world(&Injected::default(), &self_test_config()).1
}

// REQ: OA-BENCH-009
#[test]
fn cross_track_and_offset_corrected_along_track_recover_the_injection() {
    let inj = Injected::default();
    let out = baseline();
    let t1 = find(&out, "T1").unwrap();
    let nc = row(t1, "coast=not coasting");
    assert!((nc.mean - inj.cross_bias_m).abs() < 4.0, "{}", nc.mean);
    assert!((nc.rms - 40.31).abs() < 4.0, "{}", nc.rms);
    assert!((row(t1, "coast=coasting").mean - 435.0).abs() < 15.0);
    // Along-track: raw error is about tau * speed, the fitted offset is tau, corrected is noise.
    let raw = find(&out, "T2-raw").unwrap();
    assert!((row(raw, "phase=cruise").mean - 1.5 * 200.0).abs() < 15.0);
    let t2 = find(&out, "T2-offset").unwrap();
    assert!(t2.withheld.is_none());
    assert!((scalar(t2, "fitted_offset_s") - 1.5).abs() < 0.05);
    assert!(row(t2, "all").mean.abs() < 5.0);
    assert!((row(t2, "all").rms - inj.noise_along_m).abs() < 5.0);
    assert!(scalar(t2, "bins_tested") >= 2.0);
}

// REQ: OA-BENCH-010
#[test]
fn a_drifting_offset_withholds_the_corrected_along_track_numbers_with_the_reason() {
    let inj = Injected {
        tau_ramp: 0.004,
        ..clean()
    };
    let (_, out) = run_world(&inj, &self_test_config());
    let t2 = find(&out, "T2-offset").unwrap();
    let why = t2.withheld.as_ref().expect("drift must withhold");
    assert!(why.contains("drifts"), "{why}");
    assert!(t2.rows.is_empty());
    // The fitted offset is still published, and the raw numbers stay.
    assert!(scalar(t2, "fitted_offset_s") > 1.5);
    assert!(scalar(t2, "max_bin_dev_s") > 1.0);
    assert!(!find(&out, "T2-raw").unwrap().rows.is_empty());
    // A steady offset of any size is not drift: the offset is recovered, nothing withheld.
    let big = Injected {
        tau_s: 4.0,
        ..clean()
    };
    let (_, out) = run_world(&big, &self_test_config());
    let t2 = find(&out, "T2-offset").unwrap();
    assert!(t2.withheld.is_none());
    assert!((scalar(t2, "fitted_offset_s") - 4.0).abs() < 0.1);
}

// REQ: OA-BENCH-011
#[test]
fn speed_heading_and_altitude_errors_recover_the_injected_biases() {
    let inj = Injected::default();
    let out = baseline();
    assert!((row(find(&out, "T3-speed").unwrap(), "all").mean - inj.speed_bias_kt).abs() < 0.3);
    assert!((row(find(&out, "T3-heading").unwrap(), "all").mean - inj.hdg_bias_deg).abs() < 0.1);
    let t4 = find(&out, "T4").unwrap();
    assert!((row(t4, "all").mean - inj.alt_bias_ft).abs() < 8.0);
    // Cruise altitudes are multiples of 100 ft: the error is exactly the bias.
    assert!((row(t4, "phase=cruise").mean - inj.alt_bias_ft).abs() < 1e-6);
}

// REQ: OA-BENCH-012
#[test]
fn publication_interval_and_staleness_are_measured_from_the_stream() {
    let inj = Injected::default();
    let out = baseline();
    let t5 = find(&out, "T5").unwrap();
    assert_eq!(row(t5, "all").median, inj.interval_s);
    // The dropout of the first aircraft shows as one 156 s interval.
    assert_eq!(row(t5, "all").max, 156.0);
    let t6 = find(&out, "T6").unwrap();
    assert_eq!(row(t6, "all").mean, inj.stale_s);
    // The flight with no receive time is counted, not measured.
    assert_eq!(scalar(t6, "samples_without_receive_time"), 5.0);
}

// REQ: OA-BENCH-013
#[test]
fn presence_and_initiation_delay_match_the_construction() {
    let inj = Injected::default();
    let (w, out) = run_world(&inj, &self_test_config());
    let t7 = find(&out, "T7").unwrap();
    // Brute-force the expected presence from the legacy stream and the paths: every 5 s grid
    // point of a truth leg inside the zone, present when a sample of that aircraft lies within
    // 30 s. Associated aircraft only (the others sit outside the zone).
    let e = env();
    let (mut n_in, mut n_pres) = (0u64, 0u64);
    for l in &w.legs {
        let mut times: Vec<i64> = Vec::new();
        for i in 0..w.legacy.th.flight.len() {
            let f = w.legacy.th.flight[i] as usize;
            if out.assoc.addr[f] == l.addr {
                times.push(w.legacy.th.pos_ms[i]);
            }
        }
        let mut t = l.start_ms;
        while t <= l.end_ms {
            let s = truth_at(&w.aircraft, l.addr, t).unwrap();
            if (e.in_zone)(s.lat, s.lon) {
                n_in += 1;
                if times.iter().any(|&x| (x - t).abs() <= 30_000) {
                    n_pres += 1;
                }
            }
            t += 5_000;
        }
    }
    assert_eq!(scalar(t7, "truth_grid_points_in_zone"), n_in as f64);
    assert_eq!(scalar(t7, "with_eram_position"), n_pres as f64);
    assert!(
        n_pres < n_in,
        "the dropout and the start delay must cost presence"
    );
    let t8 = find(&out, "T8").unwrap();
    assert_eq!(row(t8, "all").n, 3);
    assert!((row(t8, "all").mean - inj.init_delay_s).abs() <= 5.1);
    assert_eq!(scalar(t8, "legs_never_in_zone"), 3.0);
    assert_eq!(scalar(t8, "legs_starting_inside_zone_not_a_takeoff"), 4.0);
}

// REQ: OA-BENCH-014
#[test]
fn the_deliberate_track_swap_is_found_and_no_swap_is_not_invented() {
    let (_, none) = run_world(&Injected::default(), &self_test_config());
    assert_eq!(
        scalar(find(&none, "T9").unwrap(), "association_errors"),
        0.0
    );
    let inj = Injected {
        swap: true,
        ..Injected::default()
    };
    let (_, out) = run_world(&inj, &self_test_config());
    let t9 = find(&out, "T9").unwrap();
    assert_eq!(scalar(t9, "association_errors"), 1.0, "{:?}", t9.cases);
    assert_eq!(t9.cases.len(), 1);
    // The flight that follows another aircraft is SYN006 (A00006), and it follows A00007.
    assert!(t9.cases[0].contains("SYN006"), "{}", t9.cases[0]);
    assert!(
        t9.cases[0].contains("own A00006 follows A00007"),
        "{}",
        t9.cases[0]
    );
}

// REQ: OA-BENCH-015
#[test]
fn tbfm_meter_fix_and_schedule_errors_by_lookahead() {
    let inj = Injected::default();
    let out = baseline();
    let f1 = find(&out, "F1").unwrap();
    for (bin, e) in [
        ("0 to 5 min", 10.0),
        ("5 to 10 min", 30.0),
        ("25 to 30 min", 60.0),
        ("50 to 55 min", 120.0),
    ] {
        let r = row(f1, &format!("lookahead={bin}"));
        assert_eq!(r.n, 5);
        assert!((r.mean - e).abs() < 0.5, "{bin}: {}", r.mean);
    }
    let f2 = find(&out, "F2").unwrap();
    assert!((row(f2, "lookahead=0 to 5 min").mean - inj.tbfm_sta_err_s).abs() < 0.5);
    assert!(f2.notes.iter().any(|n| n.contains("sta_sfx")));
    // The fix the aircraft never nears, and the estimate issued after the crossing, are counted.
    let ex = &out.exclusions;
    assert_eq!(ex[&("F1", "truth never within 2 NM of the meter fix")], 1);
    assert_eq!(
        ex[&(
            "F1",
            "issued after the crossing or more than 60 min before it"
        )],
        1
    );
}

// REQ: OA-BENCH-016
#[test]
fn tfms_arrival_and_departure_errors_by_lookahead() {
    let inj = Injected::default();
    let out = baseline();
    let a = find(&out, "F3-arrival").unwrap();
    assert!((row(a, "lookahead=0 to 5 min").mean - inj.tfms_eta_err_s[0]).abs() < 1.0);
    assert!((row(a, "lookahead=10 to 15 min").mean - inj.tfms_eta_err_s[1]).abs() < 1.0);
    assert_eq!(row(a, "lookahead=0 to 5 min").n, 5);
    let d = find(&out, "F3-departure").unwrap();
    assert!((row(d, "lookahead=5 to 10 min").mean - inj.tfms_etd_err_s).abs() < 1.0);
}

// REQ: OA-BENCH-017
#[test]
fn route_conformance_lists_the_flight_that_leaves_its_route() {
    let out = baseline();
    let r1 = find(&out, "R1").unwrap();
    assert_eq!(scalar(r1, "flights_evaluated"), 2.0);
    assert_eq!(scalar(r1, "flights_with_time_beyond_limit"), 1.0);
    assert!((scalar(r1, "max_flight_share") - 0.662).abs() < 0.01);
    assert_eq!(r1.cases.len(), 1);
    assert!(r1.cases[0].contains("SYN003"), "{}", r1.cases[0]);
    // The rest of the flights have no expanded route and are counted.
    assert!(out.exclusions[&("R1", "flight has no expanded route")] >= 4);
}

// REQ: OA-BENCH-018
#[test]
fn handoff_times_are_compared_with_the_truth_boundary_crossing() {
    let inj = Injected::default();
    let out = baseline();
    let h1 = find(&out, "H1").unwrap();
    assert_eq!(row(h1, "event=initiation").n, 1);
    assert!((row(h1, "event=initiation").mean - inj.handoff_init_s).abs() < 1.5);
    assert!((row(h1, "event=acceptance").mean - inj.handoff_accept_s).abs() < 1.5);
}

fn small_world_pairs(
    legs: &[TruthLeg],
    q: &dyn Fn(u32, i64) -> Result<TruthSample, NoTruth>,
    samples: &[(f64, f64, i32)],
    callsign: &[u8],
    addr: Option<u32>,
) -> (
    oa_bench::assoc::Assoc,
    oa_bench::pair::PairTable,
    oa_bench::stats::Exclusions,
) {
    let mut leg = new_legacy();
    let f = oa_bench::legacy::flight_ix(&mut leg, 1).unwrap();
    leg.flights.callsign[f as usize][..callsign.len()].copy_from_slice(callsign);
    if let Some(a) = addr {
        set_addr(&mut leg, f, a);
    }
    for (i, &(lat, lon, alt)) in samples.iter().enumerate() {
        push_th(
            &mut leg,
            f,
            T0_MS + 12_000 * i as i64,
            NO_TIME,
            (lat * 1e7) as i32,
            (lon * 1e7) as i32,
            alt,
            None,
            false,
        );
    }
    let truth = Truth { query: q, legs };
    let ix = build_th_index(&leg.th, 1);
    let tix = index_truth(legs);
    let a = associate(&leg, &ix, &truth, &tix, &AssocConfig::default());
    let (p, ex) = build_pairs(&leg, &ix, &a, &truth, &PairConfig::default());
    (a, p, ex)
}

fn level_truth(t_ms: i64) -> TruthSample {
    let s = (t_ms - T0_MS) as f64 / 1000.0;
    TruthSample {
        lat: 41.0,
        lon: -88.0 + 200.0 * s / 82_000.0,
        baro_alt_ft: Some(30_000),
        vel_east_mps: 200.0,
        vel_north_mps: 0.0,
        vert_rate_fpm: Some(0),
        sigma95_h_m: 5.0,
        leg: 0,
    }
}

// REQ: OA-BENCH-008
#[test]
fn the_callsign_gate_needs_80_percent_within_3_nm_and_1000_ft() {
    let legs = [TruthLeg {
        addr: 0xABC001,
        start_ms: T0_MS,
        end_ms: T0_MS + 1_000_000,
        callsign: Some(*b"GATE01\0\0"),
    }];
    let q = |a: u32, t: i64| {
        if a == 0xABC001 {
            Ok(level_truth(t))
        } else {
            Err(NoTruth::UnknownAircraft)
        }
    };
    // One degree of latitude is 60 NM; 0.02 deg is 1.2 NM, 0.1 deg is 6 NM.
    let mk = |bad_h: usize, bad_v: usize| -> Vec<(f64, f64, i32)> {
        (0..10)
            .map(|i| {
                let t = level_truth(T0_MS + 12_000 * i as i64);
                let lat = if i < bad_h { t.lat + 0.1 } else { t.lat + 0.02 };
                let alt = if (bad_h..bad_h + bad_v).contains(&i) {
                    31_500
                } else {
                    30_000
                };
                (lat, t.lon, alt)
            })
            .collect()
    };
    // 8 of 10 pass the gate: associated by callsign.
    let (a, p, _) = small_world_pairs(&legs, &q, &mk(2, 0), b"GATE01", None);
    assert_eq!(a.status[0], astatus::CALLSIGN);
    assert_eq!(a.addr[0], 0xABC001);
    assert_eq!(a.method[0], method::CALLSIGN);
    assert!(p.method.iter().all(|&m| m == method::CALLSIGN));
    // 7 of 10 within 3 NM: not associated.
    let (a, p, ex) = small_world_pairs(&legs, &q, &mk(3, 0), b"GATE01", None);
    // The selection does not depend on ERAM's error: the flight is kept under its own method
    // and its errors are scored (it used to be dropped here, flattering ERAM).
    assert_eq!(a.status[0], astatus::GATE_FAILED);
    assert_eq!(a.addr[0], 0xABC001);
    assert_eq!(a.method[0], method::CALLSIGN_GATE_FAILED);
    assert_eq!(p.t_ms.len(), 10);
    assert!(p.method.iter().all(|&m| m == method::CALLSIGN_GATE_FAILED));
    assert!(!ex.keys().any(|(scope, _)| *scope == "pairing"));
    // One sample 6 NM out and one 1,500 ft off: 8 of 10 pass. A second altitude failure: 7 of 10.
    assert_eq!(
        small_world_pairs(&legs, &q, &mk(1, 1), b"GATE01", None)
            .0
            .status[0],
        astatus::CALLSIGN
    );
    assert_eq!(
        small_world_pairs(&legs, &q, &mk(1, 2), b"GATE01", None)
            .0
            .status[0],
        astatus::GATE_FAILED
    );
    // The address wins without the 3 NM check (a contradicting callsign would flag it instead, see
    // the fairness tests), and is stored as the method.
    let (a, p, _) = small_world_pairs(&legs, &q, &mk(3, 0), b"GATE01", Some(0xABC001));
    assert_eq!(a.status[0], astatus::ADDRESS);
    assert!(p.method.iter().all(|&m| m == method::ADDRESS));
}

// REQ: OA-BENCH-007
#[test]
fn association_counts_address_callsign_ambiguous_and_unknown_cases() {
    let out = full_baseline();
    let c = &out.assoc.counts;
    assert_eq!(c[astatus::ADDRESS as usize], 5);
    assert_eq!(c[astatus::CALLSIGN as usize], 2);
    assert_eq!(c[astatus::AMBIGUOUS as usize], 1);
    assert_eq!(c[astatus::GATE_FAILED as usize], 1);
    assert_eq!(c[astatus::ADDRESS_NO_TRUTH as usize], 1);
    assert_eq!(c.iter().sum::<u64>(), 10);
    // Ambiguous and unassociated flights contribute no pairs, and their samples are excluded.
    let m = find(&out, "ASSOC").unwrap();
    assert_eq!(scalar(m, "ambiguous (two aircraft pass the gate)"), 1.0);
    assert!(out.exclusions[&("pairing", "ambiguous (two aircraft pass the gate)")] > 0);
    // Every pair is tagged with its method; the splits carry it.
    let t1 = find(&out, "T1").unwrap();
    assert!(row(t1, "method=address").n > 0);
    assert!(row(t1, "method=callsign").n > 0);
    // The gate-failed aircraft is kept: its samples are in "all" under their own method.
    let gf = row(t1, "method=callsign (failed 3 NM / 1000 ft check)").n;
    assert!(gf > 0);
    assert_eq!(
        row(t1, "method=address").n + row(t1, "method=callsign").n + gf,
        row(t1, "all").n
    );
    // Each such flight is listed, with its error shown in the separate split.
    assert!(m
        .cases
        .iter()
        .any(|c| c.contains("GATEFL") && c.contains("kept")));
}

// REQ: OA-BENCH-003
#[test]
fn truth_that_is_missing_is_counted_by_reason_and_never_filled_in() {
    let legs = [TruthLeg {
        addr: 0xABC001,
        start_ms: T0_MS,
        end_ms: T0_MS + 1_000_000,
        callsign: None,
    }];
    let q = |_: u32, t: i64| match (t - T0_MS) / 12_000 {
        0 => Err(NoTruth::NoNearbyReport),
        1 => Err(NoTruth::TooUncertain),
        2 => Err(NoTruth::OutsideLegs),
        3 => Err(NoTruth::UnknownAircraft),
        4 => Ok(TruthSample {
            sigma95_h_m: 80.0,
            ..level_truth(t)
        }),
        _ => Ok(level_truth(t)),
    };
    let samples: Vec<(f64, f64, i32)> = (0..8)
        .map(|i| {
            let t = level_truth(T0_MS + 12_000 * i);
            (t.lat, t.lon, 30_000)
        })
        .collect();
    let (_, p, ex) = small_world_pairs(&legs, &q, &samples, b"X", Some(0xABC001));
    assert_eq!(p.t_ms.len(), 3);
    assert_eq!(ex[&("pairing", "no nearby report")], 1);
    assert_eq!(ex[&("pairing", "outside legs")], 1);
    assert_eq!(ex[&("pairing", "unknown aircraft")], 1);
    // One from the truth function, one from the 50 m gate applied here.
    assert_eq!(ex[&("pairing", "truth too uncertain")], 2);
}

// REQ: OA-BENCH-026
#[test]
fn the_benchmark_runs_the_same_on_a_truth_built_from_a_plain_table() {
    // The truth interface is a plain query and a leg list: a lookup table wired the same way
    // gives the same answers as the closed-form closure.
    let w = build_world(&Injected::default());
    let table: Vec<(u32, i64, TruthSample)> = w
        .legs
        .iter()
        .flat_map(|l| {
            (0..)
                .map(|k| l.start_ms + 1000 * k)
                .take_while(|&t| t <= l.end_ms + 1000)
                .map(|t| (l.addr, t, truth_at(&w.aircraft, l.addr, t)))
                .filter_map(|(a, t, r)| r.ok().map(|s| (a, t, s)))
                .collect::<Vec<_>>()
        })
        .collect();
    let q = |a: u32, t: i64| {
        let k = (t as f64 / 1000.0).round() as i64 * 1000;
        if (t - k).abs() > 0 {
            // Off the 1 s grid: use the closed form (the table is a 1 s sampling).
            return truth_at(&w.aircraft, a, t);
        }
        table
            .iter()
            .find(|(x, tt, _)| *x == a && *tt == t)
            .map(|(_, _, s)| *s)
            .ok_or(NoTruth::OutsideLegs)
    };
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let a = run_benchmark(
        &w.legacy,
        &truth,
        &env(),
        &RunConfig {
            ..self_test_config()
        },
    );
    let b = baseline();
    assert_eq!(a.pairs, b.pairs);
    assert_eq!(
        row(find(&a, "T1").unwrap(), "all").n,
        row(find(&b, "T1").unwrap(), "all").n
    );
}

// REQ: OA-BENCH-027
#[test]
fn the_address_hint_associates_when_no_aircraft_address_is_given_and_is_its_own_method() {
    let legs = [TruthLeg {
        addr: 0xABC001,
        start_ms: T0_MS,
        end_ms: T0_MS + 1_000_000,
        callsign: Some(*b"HINT01\0\0"),
    }];
    let q = |a: u32, t: i64| {
        if a == 0xABC001 {
            Ok(level_truth(t))
        } else {
            Err(NoTruth::UnknownAircraft)
        }
    };
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let tix = index_truth(&legs);
    let mut leg = new_legacy();
    let mut mk = |gufi: u128, hint: u32, addr: u32| {
        let f = oa_bench::legacy::flight_ix(&mut leg, gufi).unwrap();
        leg.flights.addr_hint[f as usize] = hint;
        leg.flights.addr[f as usize] = addr;
        leg.flights.callsign[f as usize][..6].copy_from_slice(b"HINT01");
        for i in 0..6 {
            let t = level_truth(T0_MS + 12_000 * i);
            push_th(
                &mut leg,
                f,
                T0_MS + 12_000 * i,
                NO_TIME,
                (t.lat * 1e7) as i32,
                (t.lon * 1e7) as i32,
                30_000,
                None,
                false,
            );
        }
    };
    mk(1, 0xABC001, NONE); // hint only
    mk(2, 0xABC001, 0xABC001); // both, equal
    mk(3, 0xDEAD00, NONE); // hint with no truth: not retried by callsign
    mk(4, NONE, NONE); // callsign only
    let ix = build_th_index(&leg.th, 4);
    let a = associate(&leg, &ix, &truth, &tix, &AssocConfig::default());
    assert_eq!(a.status[0], astatus::ADDRESS_HINT);
    assert_eq!(a.method[0], method::ADDRESS_HINT);
    assert_eq!(a.status[1], astatus::ADDRESS);
    assert_eq!(a.status[2], astatus::ADDRESS_NO_TRUTH);
    assert_eq!(a.addr[2], NONE);
    assert_eq!(a.status[3], astatus::CALLSIGN);
    let (p, _) = build_pairs(&leg, &ix, &a, &truth, &PairConfig::default());
    assert_eq!(
        p.method
            .iter()
            .filter(|&&m| m == method::ADDRESS_HINT)
            .count(),
        6
    );
}

// REQ: OA-BENCH-019
#[test]
fn nothing_that_cannot_be_scored_is_dropped_silently() {
    let out = full_baseline();
    let ex = &out.exclusions;
    // Samples of flights that are not associated, by outcome.
    assert_eq!(ex[&("pairing", "address has no truth")], 5);
    // The callsign flight that fails the 3 NM check is scored, not excluded.
    assert!(!ex.keys().any(|(_, r)| r.contains("failed the 3 NM")));
    // TBFM and TFMS rows that cannot be scored, by reason.
    assert_eq!(ex[&("F1", "truth never within 2 NM of the meter fix")], 1);
    assert!(ex.contains_key(&(
        "F1",
        "issued after the crossing or more than 60 min before it"
    )));
    assert!(ex.contains_key(&("R1", "flight has no expanded route")));
    // The count of every ERAM sample is accounted for: paired plus excluded equals offered.
    let leg = run_world(&Injected::default(), &self_test_config())
        .0
        .legacy;
    let excluded: u64 = ex
        .iter()
        .filter(|((scope, _), _)| *scope == "pairing")
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(out.pairs as u64 + excluded, leg.th.flight.len() as u64);
}

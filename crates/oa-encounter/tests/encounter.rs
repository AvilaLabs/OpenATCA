//! The close-encounter record: geometry at the class edges, hysteresis, closest approach, the
//! ground and altitude rules, the work budget, determinism and the grid against brute force.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_encounter::*;
use oa_geo::aeqd::{frame_new, project, unproject};

const NM: f64 = 1852.0;
const FT: f64 = 0.3048;
const S: i64 = 1_000_000_000;

fn cfg() -> EncounterConfig {
    let mut classes = [ClassLimit {
        h_m: 1.0,
        v_ft: 1.0,
    }; MAX_CLASSES];
    classes[0] = ClassLimit {
        h_m: 1500.0 * FT,
        v_ft: 200.0,
    };
    classes[1] = ClassLimit {
        h_m: NM,
        v_ft: 400.0,
    };
    classes[2] = ClassLimit {
        h_m: 3.0 * NM,
        v_ft: 1000.0,
    };
    classes[3] = ClassLimit {
        h_m: 5.0 * NM,
        v_ft: 1000.0,
    };
    EncounterConfig {
        max_tracks: 512,
        max_open: 512,
        max_candidates: 4096,
        volume_h_m: 5.0 * NM,
        volume_v_ft: 1000.0,
        classes,
        n_classes: 4,
        hysteresis_ns: 10 * S,
        work_budget: 1_000_000,
        eval_budget: 4096,
    }
}

fn obs(addr: u32, lat: f64, lon: f64, alt: f32) -> Obs {
    Obs {
        lat_deg: lat,
        lon_deg: lon,
        addr,
        alt_ft: alt,
        vrate_fpm: 0.0,
        flags: 0,
        ground: AIRBORNE,
    }
}

/// The point `x` metres east and `y` north of (lat, lon) in the frame the record measures in.
fn offset(lat: f64, lon: f64, x: f64, y: f64) -> (f64, f64) {
    unproject(&frame_new(lat, lon), x, y).unwrap()
}

/// Two aircraft `d` metres apart (north), A at (40, -90), at altitudes `a` and `b`.
fn pair(d: f64, a: f32, b: f32) -> [Obs; 2] {
    let (lat, lon) = offset(40.0, -90.0, 0.0, d);
    [obs(1, 40.0, -90.0, a), obs(2, lat, lon, b)]
}

fn run(st: &mut EncounterState, log: &mut EncounterLog, t_s: i64, rows: &[Obs]) -> CycleStats {
    encounter_cycle(st, t_s * S, rows, log)
}

fn fresh() -> (EncounterState, EncounterLog) {
    (encounter_new(&cfg()).unwrap(), encounter_log_new(1024))
}

// REQ: OA-ENC-001
#[test]
fn classes_have_strict_edges() {
    let c = cfg();
    let edges = [
        (0usize, 1500.0 * FT, 200.0),
        (1, NM, 400.0),
        (2, 3.0 * NM, 1000.0),
        (3, 5.0 * NM, 1000.0),
    ];
    for (k, h, v) in edges {
        // One unit inside both limits is this class; at either limit it is not.
        assert_eq!(
            pair_class(&c, h - 1.0, v - 1.0),
            Some(k as u8),
            "inside {k}"
        );
        let at_h = pair_class(&c, h, v - 1.0);
        let at_v = pair_class(&c, h - 1.0, v);
        let want_h = if k < 3 { Some(k as u8 + 1) } else { None };
        assert_eq!(at_h, want_h, "horizontal edge {k}");
        let want_v = match k {
            0 => Some(1),
            1 => Some(2),
            _ => None,
        };
        assert_eq!(at_v, want_v, "vertical edge {k}");
        if k == 3 {
            assert_eq!(pair_class(&c, h + 1.0, v + 1.0), None, "outside {k}");
        }
    }
    // The record volume has the same edges; an unknown vertical separation is inside by distance.
    assert!(inside_volume(&c, 5.0 * NM - 1.0, 999.0));
    assert!(!inside_volume(&c, 5.0 * NM, 0.0));
    assert!(!inside_volume(&c, 100.0, 1000.0));
    assert!(inside_volume(&c, 100.0, f64::NAN));
    assert!(!inside_volume(&c, 5.0 * NM, f64::NAN));
    // Through a cycle: a metre inside the record radius opens, a metre outside does not.
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(5.0 * NM - 1.0, 5000.0, 5000.0));
    assert_eq!(encounter_open_len(&st), 1);
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(5.0 * NM + 1.0, 5000.0, 5000.0));
    assert_eq!(encounter_open_len(&st), 0);
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(2000.0, 5000.0, 5999.0));
    assert_eq!(encounter_open_len(&st), 1);
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(2000.0, 5000.0, 6001.0));
    assert_eq!(encounter_open_len(&st), 0);
    // The class of a finished encounter is the tightest tick: 1 NM and 399 ft is C2.
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(NM - 1.0, 5000.0, 5399.0));
    encounter_finish(&mut st, &mut log);
    assert_eq!(log.items[0].class, 1);
}

// REQ: OA-ENC-002
#[test]
fn an_encounter_ends_after_the_hysteresis_and_not_before() {
    let (mut st, mut log) = fresh();
    for t in 0..3 {
        run(&mut st, &mut log, t, &pair(3000.0, 5000.0, 5000.0));
    }
    // Out for 9 seconds (ticks 3 to 11): last inside was tick 2, still open at tick 11.
    for t in 3..12 {
        run(&mut st, &mut log, t, &pair(30_000.0, 5000.0, 5000.0));
    }
    assert!(log.items.is_empty());
    assert_eq!(encounter_open_len(&st), 1);
    // Tick 12 is 10 s after the last tick inside: it ends there.
    let s = run(&mut st, &mut log, 12, &pair(30_000.0, 5000.0, 5000.0));
    assert_eq!(s.closed, 1);
    assert_eq!(log.items.len(), 1);
    let e = log.items[0];
    assert_eq!((e.start_ns, e.end_ns), (0, 2 * S));
    assert_eq!(e.ticks_inside, 3);
    // Re-entry within the hysteresis continues the same encounter.
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(3000.0, 5000.0, 5000.0));
    for t in 1..9 {
        run(&mut st, &mut log, t, &pair(30_000.0, 5000.0, 5000.0));
    }
    run(&mut st, &mut log, 9, &pair(3000.0, 5000.0, 5000.0));
    assert!(log.items.is_empty());
    encounter_finish(&mut st, &mut log);
    assert_eq!(log.items.len(), 1);
    assert_eq!(log.items[0].ticks_inside, 2);
    // Re-entry after it closed is a second encounter.
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(3000.0, 5000.0, 5000.0));
    for t in 1..11 {
        run(&mut st, &mut log, t, &pair(30_000.0, 5000.0, 5000.0));
    }
    run(&mut st, &mut log, 11, &pair(3000.0, 5000.0, 5000.0));
    encounter_finish(&mut st, &mut log);
    assert_eq!(log.items.len(), 2);
    assert_eq!(log.items[1].start_ns, 11 * S);
}

// REQ: OA-ENC-003
#[test]
fn the_closest_approach_is_recorded_with_both_tracks_flags() {
    let (mut st, mut log) = fresh();
    let seps = [6000.0, 4000.0, 2500.0, 2600.0, 5000.0];
    for (t, d) in seps.iter().enumerate() {
        let mut r = pair(*d, 5000.0, 5000.0 + 100.0 * t as f32);
        r[0].flags = 3;
        r[1].flags = if t == 2 { 1 | 32 } else { 0 };
        r[1].ground = if t == 2 { ON_GROUND } else { AIRBORNE };
        run(&mut st, &mut log, t as i64, &r);
    }
    encounter_finish(&mut st, &mut log);
    let e = log.items[0];
    assert_eq!(e.closest_ns, 2 * S);
    assert!((f64::from(e.min_h_m) - 2500.0).abs() < 0.5, "{}", e.min_h_m);
    assert_eq!(e.v_at_closest_ft, 200.0);
    assert_eq!(e.alt_ft, 5100.0);
    assert_eq!((e.flags_a, e.flags_b), (3, 33));
    assert_eq!(e.ground_either, 1);
    assert_eq!(e.ticks_inside, 5);
    assert_eq!((e.start_ns, e.end_ns), (0, 4 * S));
    // The scaled separation is the smallest over the ticks with a known altitude.
    let scaled = |d: f64, v: f64| (d * d + (v / 1000.0 * 5.0 * NM).powi(2)).sqrt();
    let want = (0..5)
        .map(|t| scaled(seps[t], 100.0 * t as f64))
        .fold(f64::INFINITY, f64::min);
    assert!((f64::from(e.min_scaled_m) - want).abs() < 1.0);
    // Aircraft are named lower address first, and the midpoint lies between them.
    assert_eq!((e.addr_a, e.addr_b), (1, 2));
    assert!(e.lat_e7 > 400_000_000 && e.lat_e7 < 400_300_000);
}

// REQ: OA-ENC-004
#[test]
fn two_aircraft_on_the_ground_are_counted_and_not_recorded() {
    let (mut st, mut log) = fresh();
    let mut r = pair(300.0, 100.0, 100.0);
    r[0].ground = ON_GROUND;
    r[1].ground = ON_GROUND;
    let s = run(&mut st, &mut log, 0, &r);
    assert_eq!((s.ground_excluded, s.in_volume), (1, 0));
    assert_eq!(encounter_open_len(&st), 0);
    // One on the ground is an encounter, flagged.
    r[1].ground = AIRBORNE;
    let s = run(&mut st, &mut log, 1, &r);
    assert_eq!((s.ground_excluded, s.in_volume), (0, 1));
    encounter_finish(&mut st, &mut log);
    assert_eq!(log.items[0].ground_either, 1);
    // Unknown ground state is not on the ground.
    let (mut st, mut log) = fresh();
    r[0].ground = GROUND_UNKNOWN;
    r[1].ground = GROUND_UNKNOWN;
    let s = run(&mut st, &mut log, 0, &r);
    assert_eq!(s.in_volume, 1);
    assert_eq!(encounter_totals(&st).ground_excluded, 0);
    // Pairs outside the volume are not counted as excluded.
    let (mut st, mut log) = fresh();
    let mut far = pair(20_000.0, 100.0, 100.0);
    far[0].ground = ON_GROUND;
    far[1].ground = ON_GROUND;
    assert_eq!(run(&mut st, &mut log, 0, &far).ground_excluded, 0);
}

// REQ: OA-ENC-005
#[test]
fn an_unknown_altitude_is_its_own_class_by_horizontal_distance() {
    let (mut st, mut log) = fresh();
    let s = run(&mut st, &mut log, 0, &pair(1000.0, f32::NAN, 5000.0));
    assert_eq!(s.in_volume, 1);
    run(&mut st, &mut log, 1, &pair(900.0, 5000.0, f32::NAN));
    encounter_finish(&mut st, &mut log);
    let e = log.items[0];
    assert_eq!(e.class, CLASS_ALT_UNKNOWN);
    assert_eq!(e.ticks_alt_unknown, 2);
    assert!(e.v_at_closest_ft.is_nan() && e.alt_ft.is_nan() && e.min_scaled_m.is_nan());
    assert!((f64::from(e.min_h_m) - 900.0).abs() < 0.5);
    // Beyond the horizontal radius an unknown altitude is still outside.
    let (mut st, mut log) = fresh();
    run(
        &mut st,
        &mut log,
        0,
        &pair(5.0 * NM + 1.0, f32::NAN, 5000.0),
    );
    assert_eq!(encounter_open_len(&st), 0);
    // A known tick later gives the encounter a real class, the closest approach stays the
    // closest by distance, with its vertical unknown.
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(300.0, f32::NAN, 5000.0));
    run(&mut st, &mut log, 1, &pair(4000.0, 5000.0, 5100.0));
    encounter_finish(&mut st, &mut log);
    let e = log.items[0];
    assert_eq!(e.class, 2);
    assert_eq!(e.closest_ns, 0);
    assert!(e.v_at_closest_ft.is_nan());
    assert_eq!((e.ticks_inside, e.ticks_alt_unknown), (2, 1));
}

// REQ: OA-ENC-006
#[test]
fn over_budget_the_closest_pairs_are_evaluated_and_the_rest_counted() {
    let mut c = cfg();
    c.eval_budget = 2;
    let mut st = encounter_new(&c).unwrap();
    let mut log = encounter_log_new(64);
    // Four pairs a degree of latitude apart, separations 3000, 1000, 2000 and 4000 m.
    let seps = [3000.0, 1000.0, 2000.0, 4000.0];
    let mut rows = Vec::new();
    for (k, d) in seps.iter().enumerate() {
        let lat = 30.0 + k as f64;
        let (lat2, lon2) = offset(lat, -90.0, 0.0, *d);
        rows.push(obs(10 * k as u32 + 1, lat, -90.0, 5000.0));
        rows.push(obs(10 * k as u32 + 2, lat2, lon2, 5000.0));
    }
    let s = encounter_cycle(&mut st, 0, &rows, &mut log);
    assert_eq!((s.candidates, s.evaluated, s.not_evaluated), (4, 2, 2));
    encounter_finish(&mut st, &mut log);
    let mut got: Vec<u32> = log.items.iter().map(|e| e.addr_a).collect();
    got.sort_unstable();
    assert_eq!(got, vec![11, 21]);
    assert_eq!(encounter_totals(&st).not_evaluated, 2);
    // An open encounter whose pair is over budget is kept alive and marked, not ended.
    let mut c = cfg();
    c.eval_budget = 1;
    let mut st = encounter_new(&c).unwrap();
    let mut log = encounter_log_new(64);
    let mk = |near: f64| {
        let (la, lo) = offset(31.0, -90.0, 0.0, near);
        let (lb, lb_lon) = offset(30.0, -90.0, 0.0, 3000.0);
        vec![
            obs(1, 30.0, -90.0, 5000.0),
            obs(2, lb, lb_lon, 5000.0),
            obs(11, 31.0, -90.0, 5000.0),
            obs(12, la, lo, 5000.0),
        ]
    };
    encounter_cycle(&mut st, 0, &mk(30_000.0), &mut log);
    assert_eq!(encounter_open_len(&st), 1);
    let s = encounter_cycle(&mut st, S, &mk(1000.0), &mut log);
    assert_eq!((s.evaluated, s.not_evaluated), (1, 1));
    encounter_finish(&mut st, &mut log);
    let e = log.items.iter().find(|e| e.addr_a == 1).unwrap();
    assert_eq!((e.ticks_unevaluated, e.end_ns), (1, S));
    // A grid budget that runs out is counted in tracks, and candidates over the table too.
    let mut c = cfg();
    c.work_budget = 3;
    let mut st = encounter_new(&c).unwrap();
    let s = encounter_cycle(&mut st, 0, &mk(1000.0), &mut log);
    assert!(s.tracks_unscanned > 0 && s.grid_work == 3);
    let mut c = cfg();
    c.max_candidates = 1;
    let mut st = encounter_new(&c).unwrap();
    let s = encounter_cycle(&mut st, 0, &mk(1000.0), &mut log);
    assert_eq!(
        (s.candidates, s.candidates_dropped, s.not_evaluated),
        (2, 1, 1)
    );
    assert_eq!(s.evaluated + s.not_evaluated, s.candidates);
}

// REQ: OA-ENC-007
#[test]
fn the_record_does_not_depend_on_row_order_and_repeats_exactly() {
    let mut rows = Vec::new();
    let mut x = 12345u64;
    let mut rnd = || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) as f64 / (1u64 << 31) as f64
    };
    for a in 0..120u32 {
        rows.push(obs(
            1000 + a,
            40.0 + 0.2 * rnd(),
            -90.0 + 0.2 * rnd(),
            (4000.0 + 1500.0 * rnd()) as f32,
        ));
    }
    let go = |rows: &[Obs]| {
        let (mut st, mut log) = fresh();
        let mut all = Vec::new();
        for t in 0..4 {
            let mut r = rows.to_vec();
            if t % 2 == 1 {
                r.reverse();
            }
            all.push(run(&mut st, &mut log, t, &r));
        }
        encounter_finish(&mut st, &mut log);
        (all, log.items.clone())
    };
    let a = go(&rows);
    let b = go(&rows);
    assert_eq!(a, b);
    assert!(!a.1.is_empty());
    let mut rev = rows.clone();
    rev.reverse();
    // The same rows in another order give the same encounters.
    let c = go(&rev);
    assert_eq!(a.1, c.1);
    // Two rows with one address are both left out and counted; a bad position too.
    let (mut st, mut log) = fresh();
    let mut r = pair(2000.0, 5000.0, 5000.0).to_vec();
    r.push(obs(2, 40.0, -90.001, 5000.0));
    r.push(obs(9, f64::NAN, 0.0, 0.0));
    r.push(obs(10, 91.0, 0.0, 0.0));
    let s = run(&mut st, &mut log, 0, &r);
    assert_eq!((s.rows_duplicate, s.rows_bad, s.tracks), (2, 2, 1));
    assert_eq!(s.in_volume, 0);
}

// REQ: OA-ENC-008
#[test]
fn the_grid_finds_every_pair_the_exact_test_finds() {
    let mut x = 987654321u64;
    let mut rnd = || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) as f64 / (1u64 << 31) as f64
    };
    // Places across latitudes, one across the antimeridian, and a band of clutter.
    for (lat0, lon0, span) in [
        (0.0, 10.0, 0.4),
        (45.0, 179.9, 0.5),
        (-60.0, -179.95, 0.6),
        (75.0, 20.0, 1.0),
        (40.0, -90.0, 0.25),
    ] {
        let n = 250;
        let mut rows = Vec::new();
        for a in 0..n {
            let lat = lat0 + span * rnd();
            let mut lon = lon0 + span * rnd();
            if lon > 180.0 {
                lon -= 360.0;
            }
            rows.push(obs(a + 1, lat, lon, 3000.0));
        }
        let mut c = cfg();
        c.max_candidates = 1 << 16;
        c.max_open = 1 << 16;
        c.eval_budget = 1 << 16;
        let mut st = encounter_new(&c).unwrap();
        let mut log = encounter_log_new(1 << 16);
        let s = encounter_cycle(&mut st, 0, &rows, &mut log);
        assert_eq!((s.tracks_unscanned, s.not_evaluated), (0, 0));
        encounter_finish(&mut st, &mut log);
        let mut got: Vec<(u32, u32)> = log.items.iter().map(|e| (e.addr_a, e.addr_b)).collect();
        got.sort_unstable();
        let mut want = Vec::new();
        for i in 0..rows.len() {
            for j in (i + 1)..rows.len() {
                let f = frame_new(rows[i].lat_deg, rows[i].lon_deg);
                let (px, py) = project(&f, rows[j].lat_deg, rows[j].lon_deg);
                if px.hypot(py) < c.volume_h_m {
                    want.push((rows[i].addr, rows[j].addr));
                }
            }
        }
        want.sort_unstable();
        assert!(!want.is_empty(), "{lat0}");
        assert_eq!(got, want, "at {lat0} {lon0}");
    }
}

// REQ: OA-ENC-010
#[test]
fn a_cycle_not_after_the_last_is_refused_and_a_bad_configuration_is_too() {
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 5, &pair(1000.0, 1.0, 1.0));
    let s = run(&mut st, &mut log, 5, &pair(1000.0, 1.0, 1.0));
    assert_eq!((s.out_of_order, s.cycles), (1, 0));
    let s = run(&mut st, &mut log, 4, &[]);
    assert_eq!(s.out_of_order, 1);
    assert_eq!(encounter_totals(&st).out_of_order, 2);
    assert_eq!(encounter_open_len(&st), 1);
    for bad in [
        EncounterConfig {
            max_tracks: 0,
            ..cfg()
        },
        EncounterConfig {
            volume_h_m: 10.0,
            ..cfg()
        },
        EncounterConfig {
            volume_v_ft: f64::NAN,
            ..cfg()
        },
        EncounterConfig {
            n_classes: 9,
            ..cfg()
        },
        EncounterConfig {
            hysteresis_ns: -1,
            ..cfg()
        },
        EncounterConfig {
            eval_budget: 0,
            ..cfg()
        },
        EncounterConfig {
            work_budget: 0,
            ..cfg()
        },
    ] {
        assert!(encounter_check(&bad).is_err());
        assert!(encounter_new(&bad).is_err());
    }
    assert!(encounter_check(&cfg()).is_ok());
    // A full log counts what it loses.
    let mut st = encounter_new(&cfg()).unwrap();
    let mut log = encounter_log_new(0);
    run(&mut st, &mut log, 0, &pair(1000.0, 1.0, 1.0));
    encounter_finish(&mut st, &mut log);
    assert_eq!((log.items.len(), log.overflow), (0, 1));
}

// REQ: OA-ENC-013
#[test]
fn the_true_minimum_is_interpolated_between_ticks_and_the_sampled_one_kept() {
    let (mut st, mut log) = fresh();
    // B passes A at 100 m abeam, 600 m per tick: no tick is nearer than 316 m.
    for (t, x) in [-900.0, -300.0, 300.0, 900.0].into_iter().enumerate() {
        let (lat, lon) = offset(40.0, -90.0, x, 100.0);
        let mut b = obs(2, lat, lon, 5300.0);
        b.vrate_fpm = if t == 2 { -700.0 } else { 100.0 };
        let mut a = obs(1, 40.0, -90.0, 5000.0);
        a.vrate_fpm = 50.0;
        run(&mut st, &mut log, t as i64, &[a, b]);
    }
    encounter_finish(&mut st, &mut log);
    let e = log.items[0];
    assert!((f64::from(e.min_h_m) - 316.2).abs() < 0.5, "{}", e.min_h_m);
    assert!(e.closest_ns == S || e.closest_ns == 2 * S);
    assert!(
        (f64::from(e.min_h_interp_m) - 100.0).abs() < 0.5,
        "{}",
        e.min_h_interp_m
    );
    assert!(
        (e.interp_ns - 3 * S / 2).abs() < 2_000_000,
        "{}",
        e.interp_ns
    );
    assert!(e.min_h_interp_m <= e.min_h_m);
    // Vertical and rates: the separation is 300 ft throughout; the largest rate is 700 fpm.
    assert_eq!(e.min_v_ft, 300.0);
    assert_eq!(e.max_abs_vrate_fpm, 700.0);
    assert!(e.vrate_a_fpm == 50.0 && (e.vrate_b_fpm == 100.0 || e.vrate_b_fpm == -700.0));
    // A tick outside the volume between two inside ones: the straight line is not trusted
    // across it.
    let (mut st, mut log) = fresh();
    for (t, x) in [(0i64, -900.0), (1, 30_000.0), (2, 900.0)] {
        let (lat, lon) = offset(40.0, -90.0, x, 100.0);
        run(
            &mut st,
            &mut log,
            t,
            &[obs(1, 40.0, -90.0, 5000.0), obs(2, lat, lon, 5000.0)],
        );
    }
    encounter_finish(&mut st, &mut log);
    assert!(
        log.items[0].min_h_interp_m > 800.0,
        "{}",
        log.items[0].min_h_interp_m
    );
}

// REQ: OA-ENC-019
#[test]
fn a_shortfall_never_ends_or_splits_an_encounter() {
    // Pair X (addresses 21, 22) is open; at the middle tick a pair with lower addresses fills the
    // one-place candidate table and X is dropped from it.
    let mut c = cfg();
    c.max_candidates = 1;
    let mk = |near: f64| {
        let (la, lo) = offset(31.0, -90.0, 0.0, near);
        let (lb, lb_lon) = offset(30.0, -90.0, 0.0, 3000.0);
        vec![
            obs(1, 31.0, -90.0, 5000.0),
            obs(2, la, lo, 5000.0),
            obs(21, 30.0, -90.0, 5000.0),
            obs(22, lb, lb_lon, 5000.0),
        ]
    };
    let mut st = encounter_new(&c).unwrap();
    let mut log = encounter_log_new(64);
    encounter_cycle(&mut st, 0, &mk(30_000.0), &mut log);
    let ticks: Vec<(i64, f64)> = vec![(1, 1000.0), (2, 1000.0), (12, 30_000.0)];
    let mut dropped = 0;
    for (t, near) in ticks {
        // Long gaps: the mid ticks are within the hysteresis of the first.
        let s = encounter_cycle(&mut st, t * S, &mk(near), &mut log);
        dropped += s.candidates_dropped;
        assert_eq!(s.evaluated + s.not_evaluated, s.candidates);
    }
    assert!(dropped >= 1);
    encounter_finish(&mut st, &mut log);
    let x: Vec<&Encounter> = log.items.iter().filter(|e| e.addr_a == 21).collect();
    assert_eq!(x.len(), 1);
    assert!(x[0].ticks_unevaluated >= 1);
    assert_eq!(x[0].start_ns, 0);
    // A cycle that loses a row to a duplicate address in the middle of an encounter.
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(2000.0, 5000.0, 5000.0));
    let mut r = pair(2000.0, 5000.0, 5000.0).to_vec();
    r.push(obs(2, 40.0, -90.001, 5000.0));
    let s = run(&mut st, &mut log, 11, &r);
    assert_eq!((s.rows_duplicate, s.closed), (2, 0));
    run(&mut st, &mut log, 12, &pair(2000.0, 5000.0, 5000.0));
    encounter_finish(&mut st, &mut log);
    assert_eq!(log.items.len(), 1);
    assert_eq!(
        (log.items[0].ticks_unevaluated, log.items[0].ticks_inside),
        (1, 2)
    );
    // Without the shortfall the same gap ends it.
    let (mut st, mut log) = fresh();
    run(&mut st, &mut log, 0, &pair(2000.0, 5000.0, 5000.0));
    run(&mut st, &mut log, 11, &[]);
    assert_eq!(log.items.len(), 1);
}

// REQ: OA-ENC-020
#[test]
fn pairs_a_volume_apart_in_altitude_are_skipped_and_counted_and_the_record_is_unchanged() {
    // Known altitudes 1,000 ft apart (strict edge): skipped; 999 ft: candidate; unknown: candidate.
    for (a, b, skipped, cand) in [
        (5000.0f32, 6000.0f32, 1u64, 0u64),
        (5000.0, 5999.0, 0, 1),
        (f32::NAN, 9000.0, 0, 1),
    ] {
        let (mut st, mut log) = fresh();
        let s = run(&mut st, &mut log, 0, &pair(2000.0, a, b));
        assert_eq!(
            (s.pairs_skipped_altitude, s.candidates),
            (skipped, cand),
            "{a} {b}"
        );
        assert_eq!(s.evaluated + s.not_evaluated, s.candidates);
        assert_eq!(encounter_open_len(&st), usize::from(cand == 1));
    }
}

// REQ: OA-ENC-021
#[test]
fn rows_above_the_latitude_limit_are_counted_and_the_limit_is_stated() {
    let st = encounter_new(&cfg()).unwrap();
    let lim = encounter_lat_limit_deg(&st);
    assert!((lim - 85.8).abs() < 0.1, "{lim}");
    // A smaller radius allows a higher latitude.
    let mut c = cfg();
    c.volume_h_m = 2000.0;
    assert!(encounter_lat_limit_deg(&encounter_new(&c).unwrap()) > lim);
    let (mut st, mut log) = fresh();
    let rows = [
        obs(1, 86.5, 10.0, 1.0),
        obs(2, 85.0, 10.0, 1.0),
        obs(3, -87.0, 10.0, 1.0),
    ];
    let s = run(&mut st, &mut log, 0, &rows);
    assert_eq!((s.rows_high_lat, s.tracks), (2, 3));
    assert_eq!(encounter_totals(&st).rows_high_lat, 2);
}

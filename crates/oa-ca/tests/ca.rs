//! Conflict alert: the solvers, the effective minima, the unknown-altitude rules, the volumes,
//! the grid against brute force, the budgets and the published state, the alert lifecycle,
//! determinism. Every configuration value below is a SYNTHETIC TEST VALUE: none is an ERAM,
//! STARS or 7110.65 number.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_ca::solve::*;
use oa_ca::*;
use oa_geo::aeqd::{frame_new, unproject};
use oa_safety::net::{net_status, state_table_new, NetId, NetReport, NetState, Reason, StateTable};

const S: i64 = 1_000_000_000;
const BASE: (f64, f64) = (41.5, -88.0);

/// Synthetic test volume.
fn vol(h_m: f64, v_ft: f64) -> Volume {
    Volume {
        area: Area::Box {
            lat_min_e7: -1,
            lat_max_e7: 1,
            lon_min_e7: -1,
            lon_max_e7: 1,
        },
        alt_lo_ft: -100_000,
        alt_hi_ft: 100_000,
        h_m,
        v_ft,
    }
}

/// Synthetic test configuration: look-ahead 120 s, default volume 5,000 m and 300 ft.
fn cfg() -> CaConfig {
    CaConfig {
        max_tracks: 512,
        max_open: 512,
        max_candidates: 4096,
        look_ahead_ns: 120 * S,
        hysteresis_ns: 5 * S,
        work_budget: 1_000_000,
        eval_budget: 10_000,
        max_speed_mps: 700.0,
        r95_cap_m: 20_000.0,
        grid_band_m: 10_000.0,
        default_volume: vol(5000.0, 300.0),
        volumes: [vol(1.0, 1.0); MAX_VOLUMES],
        n_volumes: 0,
    }
}

fn place(x: f64, y: f64) -> (f64, f64) {
    unproject(&frame_new(BASE.0, BASE.1), x, y).unwrap()
}

/// A track at `(x, y)` metres from the base point, moving `(ve, vn)` m/s, at `alt` ft level.
fn trk(addr: u32, x: f64, y: f64, ve: f32, vn: f32, alt: f32) -> CaTrack {
    let (lat, lon) = place(x, y);
    CaTrack {
        lat_deg: lat,
        lon_deg: lon,
        addr,
        vel_east_mps: ve,
        vel_north_mps: vn,
        r95_m: 0.0,
        alt_ft: alt,
        vrate_fpm: if alt.is_nan() { f32::NAN } else { 0.0 },
        flags: 0,
        ground: AIRBORNE,
    }
}

struct Rig {
    st: CaState,
    alerts: AlertLog,
    events: EventLog,
    net: StateTable,
}

fn rig_with(c: &CaConfig) -> Rig {
    Rig {
        st: ca_new(c).unwrap(),
        alerts: alert_log_new(256),
        events: event_log_new(256),
        net: state_table_new(10 * S, S).unwrap(),
    }
}

fn rig() -> Rig {
    rig_with(&cfg())
}

fn step(r: &mut Rig, t_s: i64, rows: &[CaTrack]) -> CycleOut {
    ca_cycle(
        &mut r.st,
        t_s * S,
        rows,
        &mut r.alerts,
        &mut r.events,
        &mut r.net,
    )
}

/// Head-on pair on one line, 40 km apart, closing at 400 m/s, same altitude.
fn head_on(alt_b: f32) -> [CaTrack; 2] {
    [
        trk(1, -20_000.0, 0.0, 200.0, 0.0, 10_000.0),
        trk(2, 20_000.0, 0.0, -200.0, 0.0, alt_b),
    ]
}

// REQ: OA-CA-001
#[test]
fn the_horizontal_interval_is_the_solved_quadratic_with_strict_edges() {
    // Head on: 40 km, closing 400 m/s, limit 5 km: inside from (40000 - 5000) / 400 = 87.5 s
    // until it has passed through, clipped to the look-ahead.
    let s = horizontal_span(-40_000.0, 0.0, 400.0, 0.0, 5000.0, 120.0).unwrap();
    assert!((s.lo - 87.5).abs() < 1e-9 && (s.hi - 112.5).abs() < 1e-9 && !s.now);
    // Clipped at the look-ahead.
    let s = horizontal_span(-40_000.0, 0.0, 400.0, 0.0, 5000.0, 100.0).unwrap();
    assert!((s.hi - 100.0).abs() < 1e-12);
    // Not reached in the look-ahead.
    assert!(horizontal_span(-40_000.0, 0.0, 400.0, 0.0, 5000.0, 80.0).is_none());
    // Moving away.
    assert!(horizontal_span(40_000.0, 0.0, 400.0, 0.0, 5000.0, 120.0).is_none());
    // A pass at exactly the limit (tangent) is not inside; a metre inside is.
    assert!(horizontal_span(-40_000.0, 5000.0, 400.0, 0.0, 5000.0, 120.0).is_none());
    let s = horizontal_span(-40_000.0, 4999.0, 400.0, 0.0, 5000.0, 120.0).unwrap();
    assert!(s.lo < s.hi && !s.now);
    // Inside now: current, from zero.
    let s = horizontal_span(1000.0, 0.0, 100.0, 0.0, 5000.0, 120.0).unwrap();
    assert!(s.now && s.lo == 0.0);
    // Exactly at the limit now is not inside now.
    let s = horizontal_span(5000.0, 0.0, -100.0, 0.0, 5000.0, 120.0).unwrap();
    assert!(!s.now);
    // Zero relative velocity: inside for the whole look-ahead or never.
    let s = horizontal_span(1000.0, 0.0, 0.0, 0.0, 5000.0, 120.0).unwrap();
    assert!(s.now && s.lo == 0.0 && s.hi == 120.0);
    assert!(horizontal_span(5000.0, 0.0, 0.0, 0.0, 5000.0, 120.0).is_none());
    assert!(horizontal_span(9000.0, 0.0, 0.0, 0.0, 5000.0, 120.0).is_none());
    // Below the stationary threshold counts as zero.
    assert!(horizontal_span(9000.0, 0.0, 4.0e-5, 0.0, 5000.0, 120.0).is_none());
    // Miss distance over the look-ahead: zero for a head-on, the offset for a parallel pass, the
    // end of the look-ahead when the closest approach is later.
    assert!(horizontal_miss(-40_000.0, 0.0, 400.0, 0.0, 120.0) < 1e-9);
    assert!((horizontal_miss(-40_000.0, 700.0, 400.0, 0.0, 120.0) - 700.0).abs() < 1e-9);
    assert!((horizontal_miss(-80_000.0, 0.0, 400.0, 0.0, 120.0) - 32_000.0).abs() < 1e-9);
}

// REQ: OA-CA-002
#[test]
fn the_vertical_interval_is_linear_with_strict_edges_and_a_level_case() {
    // Level 1,000 ft apart with a 300 ft limit: never; 299 ft apart: always.
    assert!(vertical_span(1000.0, 0.0, 300.0, 120.0).is_none());
    assert!(vertical_span(300.0, 0.0, 300.0, 120.0).is_none());
    let s = vertical_span(299.0, 0.0, 300.0, 120.0).unwrap();
    assert!(s.now && s.lo == 0.0 && s.hi == 120.0);
    // Closing at 10 ft/s from 1,000 ft: inside the 300 ft limit from 70 s to 130 s.
    let s = vertical_span(-1000.0, 10.0, 300.0, 120.0).unwrap();
    assert!((s.lo - 70.0).abs() < 1e-9 && (s.hi - 120.0).abs() < 1e-9 && !s.now);
    // The same with the signs reversed.
    let s = vertical_span(1000.0, -10.0, 300.0, 120.0).unwrap();
    assert!((s.lo - 70.0).abs() < 1e-9);
    // Too slow to arrive in the look-ahead; moving apart.
    assert!(vertical_span(-1000.0, 5.0, 300.0, 120.0).is_none());
    assert!(vertical_span(-1000.0, -10.0, 300.0, 120.0).is_none());
    // Vertical miss: crossing gives zero, level gives the offset, a pass that has not reached it
    // gives the end value.
    assert_eq!(vertical_miss(-1000.0, 10.0, 120.0), 0.0);
    assert_eq!(vertical_miss(500.0, 0.0, 120.0), 500.0);
    assert!((vertical_miss(-2000.0, 10.0, 120.0) - 800.0).abs() < 1e-9);
}

// REQ: OA-CA-003
#[test]
fn a_prediction_reports_time_to_loss_the_misses_and_whether_the_loss_is_current() {
    let v = Vertical {
        dz_ft: 0.0,
        dvz_fps: 0.0,
        v_eff_ft: 300.0,
    };
    let p = predict(-40_000.0, 0.0, 400.0, 0.0, 5000.0, Some(&v), 120.0).unwrap();
    assert!((p.ttl_s - 87.5).abs() < 1e-9 && !p.now);
    assert!(p.h_miss_m < 1e-9 && p.v_miss_ft == 0.0);
    // Vertical convergence delays the start: the horizontal loss starts at 87.5 s but the
    // vertical one at 100 s (dz -1300 ft closing 10 ft/s to 300 ft).
    let v = Vertical {
        dz_ft: -1300.0,
        dvz_fps: 10.0,
        v_eff_ft: 300.0,
    };
    let p = predict(-40_000.0, 0.0, 400.0, 0.0, 5000.0, Some(&v), 120.0).unwrap();
    assert!((p.ttl_s - 100.0).abs() < 1e-9);
    // Horizontal and vertical losses that never overlap: no conflict.
    let v = Vertical {
        dz_ft: -2000.0,
        dvz_fps: 10.0,
        v_eff_ft: 300.0,
    };
    assert!(predict(-40_000.0, 0.0, 400.0, 0.0, 5000.0, Some(&v), 120.0).is_none());
    // Current loss: inside both now.
    let v = Vertical {
        dz_ft: 100.0,
        dvz_fps: 0.0,
        v_eff_ft: 300.0,
    };
    let p = predict(1000.0, 0.0, 10.0, 0.0, 5000.0, Some(&v), 120.0).unwrap();
    assert!(p.now && p.ttl_s == 0.0);
    // Through the cycle: the alert carries start, peak and the misses.
    let mut r = rig();
    step(&mut r, 0, &head_on(10_000.0));
    assert_eq!(ca_open_len(&r.st), 1);
    let a = ca_open(&r.st)[0];
    assert_eq!((a.addr_a, a.addr_b), (1, 2));
    assert!((a.ttl_first_s - 87.5).abs() < 0.5 && (a.ttl_peak_s - 87.5).abs() < 0.5);
    assert!(a.min_h_miss_m < 1.0 && a.min_v_miss_ft == 0.0);
    assert!((a.h_now_peak_m - 40_000.0).abs() < 10.0 && a.flags == 0);
    assert_eq!((a.ticks_conflict, a.ticks_current), (1, 0));
}

// REQ: OA-CA-004
#[test]
fn a_poor_track_widens_the_horizontal_minimum_by_its_95_percent_radius() {
    // Offset 7 km: a pass outside the 5 km minimum, so no alert for healthy tracks.
    let pass = |r95: f32, flags: u8| {
        let mut a = trk(1, -20_000.0, 7000.0, 200.0, 0.0, 10_000.0);
        let mut b = trk(2, 20_000.0, 0.0, -200.0, 0.0, 10_000.0);
        a.r95_m = r95;
        b.r95_m = r95;
        a.flags = flags;
        let mut r = rig();
        step(&mut r, 0, &[a, b]);
        ca_open_len(&r.st)
    };
    assert_eq!(pass(0.0, 0), 0);
    assert_eq!(pass(500.0, 0), 0, "5 km + 1 km is under the 7 km offset");
    // The effective minimum is 5,000 + 2 x 1,100 = 7,200 m, more than the 7,000 m offset.
    assert_eq!(pass(1100.0, 0), 1);
    // An uncertain radius cannot hide a pair: a row with none usable takes the cap (counted).
    let mut a = trk(1, -20_000.0, 7000.0, 200.0, 0.0, 10_000.0);
    let b = trk(2, 20_000.0, 0.0, -200.0, 0.0, 10_000.0);
    a.r95_m = f32::NAN;
    let mut r = rig();
    let out = step(&mut r, 0, &[a, b]);
    assert_eq!(out.stats.r95_clamped, 1);
    assert_eq!(ca_open_len(&r.st), 1);
}

// REQ: OA-CA-005
#[test]
fn a_coasting_or_uncertain_track_widens_the_vertical_minimum_and_is_flagged() {
    // 350 ft apart against a 300 ft limit: no alert; with one coasting track the limit is 400 ft.
    let run = |fa: u8, fb: u8| {
        let mut p = head_on(10_350.0);
        p[0].flags = fa;
        p[1].flags = fb;
        let mut r = rig();
        step(&mut r, 0, &p);
        ca_finish(&mut r.st, &mut r.alerts);
        r.alerts.items.clone()
    };
    assert!(run(0, 0).is_empty());
    let a = run(F_COAST, 0);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].flags & AF_COAST, AF_COAST);
    assert_eq!(a[0].v_eff_ft, 400.0);
    let a = run(0, F_UNCERTAIN);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].flags & AF_UNCERTAIN, AF_UNCERTAIN);
    // Two poor tracks: 300 + 2 x 100.
    let a = run(F_COAST, F_UNCERTAIN);
    assert_eq!(a[0].v_eff_ft, 500.0);
    assert_eq!(COAST_V_ALLOWANCE_FT, 100.0);
    // Radar-only and unconfirmed identity ride along as flags and change nothing else.
    let a = run(F_COAST | F_RADAR_ONLY, F_IDENTITY_UNCONFIRMED | F_UNCERTAIN);
    assert_eq!(
        a[0].flags & (AF_RADAR_ONLY | AF_IDENTITY_UNCONFIRMED),
        AF_RADAR_ONLY | AF_IDENTITY_UNCONFIRMED
    );
}

// REQ: OA-CA-006
#[test]
fn an_unknown_altitude_never_hides_a_horizontal_conflict_and_carries_alt_unknown() {
    for unknown in [(true, false), (false, true), (true, true)] {
        let mut p = head_on(30_000.0);
        // 20,000 ft apart: a known pair is no conflict at all.
        let mut known = rig();
        step(&mut known, 0, &p);
        assert_eq!(ca_open_len(&known.st), 0);
        if unknown.0 {
            p[0].alt_ft = f32::NAN;
            p[0].vrate_fpm = f32::NAN;
        }
        if unknown.1 {
            p[1].alt_ft = f32::NAN;
            p[1].vrate_fpm = f32::NAN;
        }
        let mut r = rig();
        let out = step(&mut r, 0, &p);
        assert_eq!(ca_open_len(&r.st), 1, "{unknown:?}");
        assert_eq!(out.stats.alt_unknown_pairs, 1);
        ca_finish(&mut r.st, &mut r.alerts);
        let a = r.alerts.items[0];
        assert_eq!(a.flags & AF_ALT_UNKNOWN, AF_ALT_UNKNOWN);
        assert!(a.min_v_miss_ft.is_nan() && a.v_eff_ft.is_nan() && a.alt_ft.is_nan());
        assert!((a.ttl_peak_s - 87.5).abs() < 0.5, "{}", a.ttl_peak_s);
    }
    // A pair that is horizontally clear stays clear without an altitude.
    let mut p = [
        trk(1, -20_000.0, 30_000.0, 200.0, 0.0, f32::NAN),
        trk(2, 20_000.0, 0.0, -200.0, 0.0, 10_000.0),
    ];
    p[0].vrate_fpm = f32::NAN;
    let mut r = rig();
    step(&mut r, 0, &p);
    assert_eq!(ca_open_len(&r.st), 0);
}

// REQ: OA-CA-007
#[test]
fn a_track_that_loses_its_altitude_raises_one_event_per_loss_and_is_still_tested() {
    let mut r = rig();
    let with = |alt: f32| {
        let mut t = trk(7, 0.0, 0.0, 100.0, 0.0, alt);
        if alt.is_nan() {
            t.vrate_fpm = f32::NAN;
        }
        [t]
    };
    let mut raised = Vec::new();
    for (t, alt) in [
        (0, f32::NAN),
        (1, 9000.0),
        (2, f32::NAN),
        (3, f32::NAN),
        (4, 9100.0),
        (5, f32::NAN),
    ] {
        let out = step(&mut r, t, &with(alt));
        raised.push(out.stats.alt_lost);
    }
    // A track that never had an altitude raises nothing; each loss raises one event, once.
    assert_eq!(raised, vec![0, 0, 1, 0, 0, 1]);
    assert_eq!(r.events.items.len(), 2);
    assert_eq!(r.events.items[0].addr, 7);
    assert_eq!(r.events.items[0].last_alt_ft, 9000.0);
    assert_eq!(r.events.items[1].last_alt_ft, 9100.0);
    assert_eq!(r.events.items[1].t_ns, 5 * S);
    assert_eq!(ca_totals(&r.st).alt_lost, 2);
    // The event log is bounded and counts what it loses.
    let mut r = rig();
    r.events = event_log_new(1);
    for (t, alt) in [(0, 9000.0), (1, f32::NAN), (2, 9000.0), (3, f32::NAN)] {
        let out = step(&mut r, t, &with(alt));
        if t == 3 {
            assert_eq!(out.stats.alt_lost_dropped, 1);
            assert!(matches!(
                out.report,
                NetReport::Degraded {
                    reason: Reason::BudgetExceeded,
                    count: 1,
                    ..
                }
            ));
        }
    }
    assert_eq!((r.events.items.len(), r.events.overflow), (1, 1));
    // A pair with the lost altitude takes part in the horizontal test at once.
    let mut r = rig();
    let mut p = head_on(30_000.0);
    step(&mut r, 0, &p);
    assert_eq!(ca_open_len(&r.st), 0);
    p[1].alt_ft = f32::NAN;
    p[1].vrate_fpm = f32::NAN;
    let out = step(&mut r, 1, &p);
    assert_eq!((out.stats.alt_lost, ca_open_len(&r.st)), (1, 1));
}

fn area_box() -> Area {
    Area::Box {
        lat_min_e7: 410_000_000,
        lat_max_e7: 420_000_000,
        lon_min_e7: -885_000_000,
        lon_max_e7: -875_000_000,
    }
}

// REQ: OA-CA-008
#[test]
fn the_volume_of_a_pair_is_the_one_holding_its_midpoint_and_altitude() {
    let mut c = cfg();
    // Synthetic: a low box with a small minimum, the same box above it with a larger one.
    c.volumes[0] = Volume {
        area: area_box(),
        alt_lo_ft: 0,
        alt_hi_ft: 18_000,
        h_m: 2000.0,
        v_ft: 200.0,
    };
    c.volumes[1] = Volume {
        area: area_box(),
        alt_lo_ft: 18_001,
        alt_hi_ft: 60_000,
        h_m: 6000.0,
        v_ft: 500.0,
    };
    c.n_volumes = 2;
    let (lat, lon) = (415_000_000, -880_000_000);
    assert_eq!(volume_minima(&c, lat, lon, Some(10_000.0)), (2000.0, 200.0));
    assert_eq!(volume_minima(&c, lat, lon, Some(18_000.0)), (2000.0, 200.0));
    assert_eq!(volume_minima(&c, lat, lon, Some(18_001.0)), (6000.0, 500.0));
    // Above every band, or outside every area: the default volume.
    assert_eq!(volume_minima(&c, lat, lon, Some(70_000.0)), (5000.0, 300.0));
    assert_eq!(
        volume_minima(&c, 300_000_000, lon, Some(10_000.0)),
        (5000.0, 300.0)
    );
    // An unknown altitude takes the larger minima of the volumes holding the point.
    assert_eq!(volume_minima(&c, lat, lon, None), (6000.0, 500.0));
    assert_eq!(volume_minima(&c, 300_000_000, lon, None), (5000.0, 300.0));
    assert_eq!(volume_maxima(&c), (6000.0, 500.0));
    // Through a cycle: a pass 5,500 m abeam is clear in the low volume and in none of the others.
    let pass = |c: &CaConfig, alt: f32| {
        let mut p = [
            trk(1, -20_000.0, 5500.0, 200.0, 0.0, alt),
            trk(2, 20_000.0, 0.0, -200.0, 0.0, alt),
        ];
        if alt.is_nan() {
            p[0].vrate_fpm = f32::NAN;
            p[1].vrate_fpm = f32::NAN;
        }
        let mut r = rig_with(c);
        step(&mut r, 0, &p);
        ca_open_len(&r.st)
    };
    assert_eq!(pass(&c, 10_000.0), 0);
    assert_eq!(pass(&c, 20_000.0), 1);
    assert_eq!(pass(&c, 36_000.0), 1);
    assert_eq!(pass(&c, f32::NAN), 1);
    // A polygon of oa-safety is an area too (its edge is outside).
    let sq = oa_safety::geom::polygon_new(&[
        (410_000_000, -885_000_000),
        (420_000_000, -885_000_000),
        (420_000_000, -875_000_000),
        (410_000_000, -875_000_000),
    ])
    .unwrap();
    c.volumes[0].area = Area::Polygon(sq);
    assert_eq!(volume_minima(&c, lat, lon, Some(10_000.0)), (2000.0, 200.0));
    assert_eq!(
        volume_minima(&c, 420_000_000, lon, Some(10_000.0)),
        (5000.0, 300.0)
    );
}

/// A deterministic scatter of `n` tracks over `span` metres with speeds up to 250 m/s.
fn scatter(n: u32, span: f64, seed: u64) -> Vec<CaTrack> {
    let mut s = seed;
    let mut next = move || {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (s >> 33) as f64 / (1u64 << 31) as f64
    };
    (0..n)
        .map(|k| {
            let (x, y) = ((next() - 0.5) * span, (next() - 0.5) * span);
            let (ve, vn) = ((next() - 0.5) * 500.0, (next() - 0.5) * 500.0);
            let mut t = trk(100 + k, x, y, ve as f32, vn as f32, 10_000.0);
            t.r95_m = (next() * 300.0) as f32;
            t
        })
        .collect()
}

// REQ: OA-CA-009
#[test]
fn the_grid_finds_every_pair_a_brute_force_test_finds_and_skips_vertical_pairs_counted() {
    let rows = scatter(150, 200_000.0, 7);
    let finest = cfg();
    let mut coarse = cfg();
    coarse.grid_band_m = 1.0e6;
    let mut a = rig_with(&finest);
    let mut b = rig_with(&coarse);
    let oa = step(&mut a, 0, &rows);
    let ob = step(&mut b, 0, &rows);
    assert_eq!(oa.stats.candidates, ob.stats.candidates);
    assert_eq!(oa.stats.in_conflict, ob.stats.in_conflict);
    assert_eq!(ca_open(&a.st), ca_open(&b.st));
    // The oracle: every pair whose exact separation is inside its radius is a candidate.
    let mut inside = 0u64;
    for (i, p) in rows.iter().enumerate() {
        for q in &rows[i + 1..] {
            let f = frame_new(p.lat_deg, p.lon_deg);
            let (x, y) = oa_geo::aeqd::project(&f, q.lat_deg, q.lon_deg);
            let sp = |t: &CaTrack| f64::from(t.vel_east_mps).hypot(f64::from(t.vel_north_mps));
            let radius = 120.0 * (sp(p) + sp(q)) + 5000.0 + f64::from(p.r95_m) + f64::from(q.r95_m);
            if x.hypot(y) <= radius {
                inside += 1;
            }
        }
    }
    assert!(inside > 100 && oa.stats.candidates >= inside, "{inside}");
    assert!(oa.stats.in_conflict > 0);
    // Pairs a volume apart in altitude, both rates known and level, are skipped and counted; the
    // alerts are those of the same traffic with the skip not available (rates unknown).
    let mut high = rows.clone();
    for (k, t) in high.iter_mut().enumerate() {
        t.alt_ft = if k % 2 == 0 { 10_000.0 } else { 14_000.0 };
    }
    let mut r = rig();
    let out = step(&mut r, 0, &high);
    assert!(out.stats.pairs_skipped_vertical > 0);
    assert_eq!(out.stats.pairs_skipped_vertical + out.stats.candidates, {
        let mut r2 = rig();
        // Rates unknown on every track: no pair is skipped (taken as level, flagged).
        for t in &mut high {
            t.vrate_fpm = f32::NAN;
        }
        step(&mut r2, 0, &high).stats.candidates
    });
    // A pair closing vertically within the look-ahead is not skipped.
    let mut p = head_on(14_000.0);
    p[0].vrate_fpm = 2000.0;
    let mut r = rig();
    let out = step(&mut r, 0, &p);
    assert_eq!(
        (out.stats.pairs_skipped_vertical, out.stats.candidates),
        (0, 1)
    );
}

/// A stationary pair `d` metres apart at `(x, y)`: inside now for ever, same altitude.
fn still(a: u32, x: f64, y: f64, d: f64) -> [CaTrack; 2] {
    [
        trk(a, x, y, 0.0, 0.0, 10_000.0),
        trk(a + 1, x + d, y, 0.0, 0.0, 10_000.0),
    ]
}

// REQ: OA-CA-010
#[test]
fn over_budget_the_closest_pairs_are_evaluated_the_rest_counted_and_open_alerts_kept() {
    let mut c = cfg();
    c.eval_budget = 1;
    c.hysteresis_ns = 3 * S;
    let mut r = rig_with(&c);
    let far = still(3, 100_000.0, 0.0, 2000.0);
    let near = still(1, 0.0, 0.0, 500.0);
    // Alone, the far pair is evaluated and opens its alert.
    let out = step(&mut r, 0, &far);
    assert_eq!((out.stats.evaluated, out.stats.not_evaluated), (1, 0));
    assert_eq!(out.report, NetReport::Running);
    // With the near pair added, only the near pair (the smaller separation) is evaluated; the
    // far pair is counted and its alert is kept alive across more than the hysteresis.
    let both = [near[0], near[1], far[0], far[1]];
    for t in 1..=10 {
        let out = step(&mut r, t, &both);
        assert_eq!(
            (out.stats.evaluated, out.stats.not_evaluated),
            (1, 1),
            "{t}"
        );
        assert!(matches!(
            out.report,
            NetReport::Degraded {
                reason: Reason::BudgetExceeded,
                count: 1,
                ..
            }
        ));
    }
    let open = ca_open(&r.st);
    assert_eq!(open.len(), 2);
    let far_alert = open.iter().find(|a| a.addr_a == 3).unwrap();
    assert_eq!(
        (far_alert.ticks_unevaluated, far_alert.ticks_conflict),
        (10, 1)
    );
    assert_eq!(far_alert.end_ns, 10 * S);
    assert_eq!(ca_totals(&r.st).not_evaluated, 10);
    // The same priority holds with the pairs in the other order of addresses (ties by address).
    let mut r = rig_with(&c);
    let first = still(5, 0.0, 0.0, 1000.0);
    let second = still(1, 100_000.0, 0.0, 1000.0);
    step(&mut r, 0, &[first[0], first[1], second[0], second[1]]);
    assert_eq!(ca_open(&r.st)[0].addr_a, 1);
    // A grid work budget that runs out counts the tracks it did not reach and degrades the net.
    let mut c = cfg();
    c.work_budget = 3;
    let mut r = rig_with(&c);
    let out = step(&mut r, 0, &[near[0], near[1], far[0], far[1]]);
    assert!(out.stats.tracks_unscanned > 0);
    assert!(matches!(
        out.report,
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            ..
        }
    ));
    // A candidate table that fills drops, counts and degrades.
    let mut c = cfg();
    c.max_candidates = 1;
    let mut r = rig_with(&c);
    let out = step(&mut r, 0, &[near[0], near[1], far[0], far[1]]);
    assert_eq!(out.stats.candidates_dropped, 1);
    assert_eq!(
        out.stats.candidates,
        out.stats.evaluated + out.stats.not_evaluated
    );
    // Rows past the track capacity are counted and degrade it.
    let mut c = cfg();
    c.max_tracks = 3;
    let mut r = rig_with(&c);
    let out = step(&mut r, 0, &[near[0], near[1], far[0], far[1]]);
    assert_eq!(out.stats.rows_over_capacity, 1);
    assert!(matches!(out.report, NetReport::Degraded { .. }));
    // A full open table drops and counts.
    let mut c = cfg();
    c.max_open = 1;
    let mut r = rig_with(&c);
    let out = step(&mut r, 0, &[near[0], near[1], far[0], far[1]]);
    assert_eq!((out.stats.opened, out.stats.open_dropped), (1, 1));
    assert!(matches!(out.report, NetReport::Degraded { .. }));
}

// REQ: OA-CA-011
#[test]
fn every_cycle_publishes_the_net_state_and_a_refused_cycle_is_unavailable() {
    let mut r = rig();
    let status = |r: &Rig, t: i64| {
        net_status(
            &r.net,
            &oa_safety::inhibit::inhibits_new(),
            NetId::ConflictAlert,
            t * S,
        )
        .state
    };
    // Never reported: unavailable.
    assert!(matches!(
        status(&r, 0),
        NetState::Unavailable {
            reason: Reason::NeverReported
        }
    ));
    let rows = head_on(10_000.0);
    let out = step(&mut r, 10, &rows);
    assert_eq!(out.report, NetReport::Running);
    assert_eq!(status(&r, 10), NetState::Running);
    // A report older than the table's staleness limit reads as unavailable, not running.
    assert!(matches!(
        status(&r, 30),
        NetState::Unavailable {
            reason: Reason::StateStale
        }
    ));
    // A cycle not after the last is refused, counted, and published as unavailable.
    let out = step(&mut r, 10, &rows);
    assert_eq!(out.stats.out_of_order, 1);
    assert_eq!(out.stats.cycles, 0);
    assert!(matches!(
        status(&r, 10),
        NetState::Unavailable {
            reason: Reason::SurveillanceLost
        }
    ));
    let out = step(&mut r, 9, &rows);
    assert_eq!(out.stats.out_of_order, 1);
    assert_eq!(
        out.stats.net_refused, 1,
        "a second report that is not worse is refused"
    );
    // The next good cycle runs again.
    let out = step(&mut r, 11, &rows);
    assert_eq!(out.report, NetReport::Running);
    assert_eq!(status(&r, 11), NetState::Running);
    // Degraded states carry the count and read as degraded.
    let mut c = cfg();
    c.eval_budget = 1;
    let mut r = rig_with(&c);
    let two = [still(1, 0.0, 0.0, 500.0), still(3, 100_000.0, 0.0, 700.0)].concat();
    step(&mut r, 1, &two);
    assert!(matches!(
        status(&r, 1),
        NetState::Degraded {
            reason: Reason::BudgetExceeded,
            count: 1,
            ..
        }
    ));
    // Rows left out (a bad position, a duplicate address) degrade it as input missing.
    let mut bad = two.clone();
    bad[1].lat_deg = f64::NAN;
    let mut r = rig();
    let out = step(&mut r, 1, &bad);
    assert_eq!(out.stats.rows_bad, 1);
    assert!(matches!(
        out.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 1,
            ..
        }
    ));
    // A refused configuration is published as unavailable too.
    let mut net = state_table_new(10 * S, S).unwrap();
    assert!(ca_publish_refused(&mut net, 5 * S));
    assert!(matches!(
        net_status(
            &net,
            &oa_safety::inhibit::inhibits_new(),
            NetId::ConflictAlert,
            5 * S
        )
        .state,
        NetState::Unavailable {
            reason: Reason::InputMissing
        }
    ));
}

// REQ: OA-CA-012
#[test]
fn one_conflict_is_one_alert_with_a_start_a_peak_and_an_end_after_the_hysteresis() {
    let mut r = rig();
    // A pair closing from 40 km: the time to loss falls each second and the peak is the last.
    let closing = |t: i64| {
        let d = t as f64 * 400.0;
        [
            trk(1, -20_000.0 + d / 2.0, 0.0, 200.0, 0.0, 10_000.0),
            trk(2, 20_000.0 - d / 2.0, 0.0, -200.0, 0.0, 10_000.0),
        ]
    };
    for t in 0..4 {
        step(&mut r, t, &closing(t));
    }
    assert_eq!(ca_open_len(&r.st), 1);
    let a = ca_open(&r.st)[0];
    assert_eq!((a.start_ns, a.end_ns, a.peak_ns), (0, 3 * S, 3 * S));
    assert!(a.ttl_first_s > a.ttl_peak_s + 2.0);
    assert_eq!(a.ticks_conflict, 4);
    // The pair leaves (no rows): the alert stays open for the hysteresis, then ends at its last
    // conflict cycle, and is logged once.
    for t in 4..8 {
        step(&mut r, t, &[]);
        assert_eq!(r.alerts.items.len(), 0, "{t}");
    }
    let out = step(&mut r, 8, &[]);
    assert_eq!(out.stats.closed, 1);
    assert_eq!(ca_open_len(&r.st), 0);
    assert_eq!(r.alerts.items.len(), 1);
    assert_eq!(r.alerts.items[0].end_ns, 3 * S);
    // A conflict that comes back inside the hysteresis is the same alert; after it, a new one.
    let mut r = rig();
    let pair = still(1, 0.0, 0.0, 500.0);
    for (t, present) in [
        (0, true),
        (1, true),
        (2, false),
        (3, false),
        (4, true),
        (5, true),
    ] {
        step(&mut r, t, if present { &pair } else { &[] });
    }
    assert_eq!(ca_open_len(&r.st), 1);
    assert_eq!(ca_open(&r.st)[0].ticks_conflict, 4);
    for t in 6..20 {
        step(&mut r, t, if t == 14 { &pair } else { &[] });
    }
    ca_finish(&mut r.st, &mut r.alerts);
    assert_eq!(r.alerts.items.len(), 2);
    assert_eq!(
        (r.alerts.items[0].start_ns, r.alerts.items[0].end_ns),
        (0, 5 * S)
    );
    assert_eq!(r.alerts.items[1].start_ns, 14 * S);
    // A current loss counts its current cycles.
    assert_eq!(r.alerts.items[0].ticks_current, 4);
}

// REQ: OA-CA-013
#[test]
fn a_configuration_out_of_range_is_refused_naming_the_value() {
    let bad = |f: &dyn Fn(&mut CaConfig)| {
        let mut c = cfg();
        f(&mut c);
        ca_check(&c).err().unwrap()
    };
    assert!(ca_check(&cfg()).is_ok());
    assert_eq!(bad(&|c| c.max_tracks = 0), "max_tracks");
    assert_eq!(bad(&|c| c.max_open = 0), "max_open");
    assert_eq!(bad(&|c| c.max_candidates = 0), "max_candidates");
    assert_eq!(bad(&|c| c.look_ahead_ns = 0), "look_ahead_ns");
    assert_eq!(bad(&|c| c.look_ahead_ns = 20_000 * S), "look_ahead_ns");
    assert_eq!(bad(&|c| c.hysteresis_ns = 0), "hysteresis_ns");
    assert_eq!(bad(&|c| c.work_budget = 0), "work_budget");
    assert_eq!(bad(&|c| c.eval_budget = 0), "eval_budget");
    assert_eq!(bad(&|c| c.max_speed_mps = 0.0), "max_speed_mps");
    assert_eq!(bad(&|c| c.max_speed_mps = f64::NAN), "max_speed_mps");
    assert_eq!(bad(&|c| c.r95_cap_m = -1.0), "r95_cap_m");
    assert_eq!(bad(&|c| c.grid_band_m = 10.0), "grid_band_m");
    assert_eq!(bad(&|c| c.n_volumes = MAX_VOLUMES + 1), "n_volumes");
    assert_eq!(bad(&|c| c.default_volume.h_m = 0.0), "volume h_m");
    assert_eq!(
        bad(&|c| c.default_volume.v_ft = f64::INFINITY),
        "volume v_ft"
    );
    assert_eq!(
        bad(&|c| c.default_volume.alt_lo_ft = 200_000),
        "volume altitude band reversed"
    );
    assert_eq!(
        bad(&|c| {
            c.volumes[0].area = Area::Box {
                lat_min_e7: 10,
                lat_max_e7: 5,
                lon_min_e7: 0,
                lon_max_e7: 1,
            };
            c.n_volumes = 1;
        }),
        "volume box empty or reversed"
    );
    // A look-ahead and speed whose reach is beyond what the solver is proved for.
    assert_eq!(
        bad(&|c| {
            c.look_ahead_ns = 5000 * S;
            c.max_speed_mps = 900.0;
        }),
        "look-ahead reach beyond the solver bound"
    );
    assert!(ca_new(&{
        let mut c = cfg();
        c.eval_budget = 0;
        c
    })
    .is_err());
}

// REQ: OA-CA-014
#[test]
fn a_full_alert_log_counts_what_it_loses_and_degrades_the_net() {
    let mut r = rig();
    r.alerts = alert_log_new(1);
    let rows = [
        still(1, 0.0, 0.0, 500.0),
        still(3, 100_000.0, 0.0, 500.0),
        still(5, 200_000.0, 0.0, 500.0),
    ]
    .concat();
    step(&mut r, 0, &rows);
    assert_eq!(ca_open_len(&r.st), 3);
    let mut dropped = 0;
    for t in 1..=6 {
        let out = step(&mut r, t, &[]);
        dropped += out.stats.alerts_dropped;
        if out.stats.closed > 0 {
            assert!(matches!(
                out.report,
                NetReport::Degraded {
                    reason: Reason::BudgetExceeded,
                    count: 2,
                    ..
                }
            ));
        }
    }
    assert_eq!(
        (r.alerts.items.len(), r.alerts.overflow, dropped),
        (1, 2, 2)
    );
    // The consumer clears the log, keeping the allocation and the count.
    alert_log_clear(&mut r.alerts);
    assert_eq!((r.alerts.items.len(), r.alerts.overflow), (0, 2));
}

// REQ: OA-CA-015
#[test]
fn the_alerts_do_not_depend_on_row_order_and_bad_or_duplicate_rows_are_left_out_and_counted() {
    let rows = scatter(120, 150_000.0, 11);
    let mut shuffled = rows.clone();
    shuffled.reverse();
    shuffled.rotate_left(37);
    let mut a = rig();
    let mut b = rig();
    for t in 0..3 {
        let oa = step(&mut a, t, &rows);
        let ob = step(&mut b, t, &shuffled);
        assert_eq!(oa, ob);
    }
    assert_eq!(ca_open(&a.st), ca_open(&b.st));
    assert!(ca_open_len(&a.st) > 0);
    assert_eq!(ca_totals(&a.st), ca_totals(&b.st));
    // Two rows with one address are both left out (which is right is not known) and counted.
    let mut r = rig();
    let mut p = head_on(10_000.0).to_vec();
    p.push(p[0]);
    let out = step(&mut r, 0, &p);
    assert_eq!((out.stats.rows_duplicate, out.stats.tracks), (2, 1));
    assert_eq!(ca_open_len(&r.st), 0);
    // A row with a speed over the configured bound, or a non-finite value, is refused as bad.
    let mut r = rig();
    let mut p = head_on(10_000.0);
    p[0].vel_east_mps = 800.0;
    let out = step(&mut r, 0, &p);
    assert_eq!(out.stats.rows_bad, 1);
    let mut p = head_on(10_000.0);
    p[1].alt_ft = f32::INFINITY;
    p[0].vrate_fpm = 1.0e9;
    let out = step(&mut r, 1, &p);
    assert_eq!(out.stats.rows_bad, 2);
}

// REQ: OA-CA-016
#[test]
fn two_aircraft_on_the_ground_are_counted_apart_and_one_on_the_ground_is_flagged() {
    let mut r = rig();
    let mut p = still(1, 0.0, 0.0, 500.0);
    p[0].ground = ON_GROUND;
    p[1].ground = ON_GROUND;
    let out = step(&mut r, 0, &p);
    assert_eq!((out.stats.ground_excluded, ca_open_len(&r.st)), (1, 0));
    assert_eq!(out.report, NetReport::Running);
    // One on the ground with no altitude, one airborne: tested horizontally and flagged.
    let mut r = rig();
    let mut p = still(1, 0.0, 0.0, 500.0);
    p[0].ground = ON_GROUND;
    p[0].alt_ft = f32::NAN;
    p[0].vrate_fpm = f32::NAN;
    step(&mut r, 0, &p);
    ca_finish(&mut r.st, &mut r.alerts);
    let a = r.alerts.items[0];
    assert_eq!(
        a.flags & (AF_ONE_GROUND | AF_ALT_UNKNOWN),
        AF_ONE_GROUND | AF_ALT_UNKNOWN
    );
    assert_eq!(a.ground_either, 1);
}

// REQ: OA-CA-017
#[test]
fn an_unknown_vertical_rate_is_taken_as_level_and_flagged() {
    let mut p = head_on(10_100.0);
    p[1].vrate_fpm = f32::NAN;
    let mut r = rig();
    step(&mut r, 0, &p);
    ca_finish(&mut r.st, &mut r.alerts);
    let a = r.alerts.items[0];
    assert_eq!(a.flags & AF_RATE_UNKNOWN, AF_RATE_UNKNOWN);
    assert_eq!(a.flags & AF_ALT_UNKNOWN, 0);
    assert_eq!(a.min_v_miss_ft, 100.0);
}

// REQ: OA-CA-027
#[test]
fn the_conflict_decision_agrees_with_a_brute_force_scan_of_sampled_times_within_a_metre() {
    // The tested counterpart of the proofs of OA-CA-020: pseudo-random pairs over the stated
    // ranges, the decision against a scan of the look-ahead every 1/8 s, with a 1 m tolerance
    // in distance (and 1 ft vertically) and the scan's own resolution.
    const TOL_M: f64 = 1.0;
    const TOL_FT: f64 = 1.0;
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    let mut u = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut hits, mut clear) = (0u32, 0u32);
    for _ in 0..40_000 {
        let t_max = 30.0 + u() * 270.0;
        let h = 500.0 + u() * 20_000.0;
        let (rx, ry) = ((u() - 0.5) * 120_000.0, (u() - 0.5) * 120_000.0);
        let sp = u() * 700.0;
        let ang = u() * std::f64::consts::TAU;
        let (vx, vy) = (sp * ang.cos(), sp * ang.sin());
        let (dz, dvz, v) = (
            (u() - 0.5) * 6000.0,
            (u() - 0.5) * 60.0,
            100.0 + u() * 1500.0,
        );
        // Brute force: the first sampled time with both separations inside the limits less the
        // tolerance (a certain loss) and the last sampled time inside them plus the tolerance.
        let (mut certain, mut possible) = (false, false);
        let steps = (t_max * 8.0) as usize;
        for k in 0..=steps {
            let t = (k as f64 / 8.0).min(t_max);
            let d = ((rx + vx * t).hypot(ry + vy * t), (dz + dvz * t).abs());
            certain |= d.0 < h - TOL_M && d.1 < v - TOL_FT;
            possible |= d.0 < h + TOL_M && d.1 < v + TOL_FT;
        }
        let z = Vertical {
            dz_ft: dz,
            dvz_fps: dvz,
            v_eff_ft: v,
        };
        let got = conflict_span(rx, ry, vx, vy, h, Some(&z), t_max).is_some();
        // A certain loss is a conflict; no conflict means no sampled time was even possibly
        // inside. (A conflict whose overlap falls between two samples, shorter than 1/8 s of the
        // pass at speed, may be neither: the scan's resolution, not the decision's error.)
        if certain {
            assert!(
                got,
                "missed: {rx} {ry} {vx} {vy} {h} {dz} {dvz} {v} {t_max}"
            );
            hits += 1;
        }
        if !possible && !got {
            clear += 1;
        }
        if got && !possible {
            // The overlap between two samples: it must be shorter than one scan step at the
            // closing speed and the vertical rate allow, so the 1 m tolerance is not the margin.
            let s = conflict_span(rx, ry, vx, vy, h, Some(&z), t_max).unwrap();
            assert!(
                s.hi - s.lo <= 0.125 + 1e-9 || s.now,
                "invented: {rx} {ry} {vx} {vy} {h} {dz} {dvz} {v} {t_max} {s:?}"
            );
        }
    }
    assert!(hits > 500 && clear > 500, "{hits} {clear}");
}

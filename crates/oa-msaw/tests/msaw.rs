//! Tests of the minimum safe altitude warning. Every value here is made up for the test (no FAA
//! or facility value). The Guam scenario is SYNTHETIC: it models the kind of approach flown by
//! Korean Air 801 in 1997 (an early descent onto rising terrain), not its recorded track.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_msaw::calc::{
    alt_floor_ft, grade_of, lowest_alt_ft, predicted_alt_ft, proc_floor_at, sample_points,
    sample_span_s, sample_time_s, violation_time_s, PathStart,
};
use oa_msaw::*;
use oa_safety::geom::{polygon_new, POLYGON_EMPTY};
use oa_safety::inhibit::{
    inhibit_create, inhibits_new, inhibits_slice, suppressed_log, text_of, InhibitConfig,
    InhibitRequest, InhibitTable,
};
use oa_safety::net::{net_status, state_table_new, NetId, NetReport, NetState, Reason, StateTable};

const S: i64 = 1_000_000_000;

fn cfg() -> MsawConfig {
    MsawConfig {
        max_tracks: 64,
        max_open: 8,
        track_budget: 32,
        look_ahead_s: 60.0,
        warning_s: 20.0,
        samples: 13,
        hysteresis_ns: 5 * S,
        terrain_clearance_ft: 500,
        obstacle_clearance_ft: 500,
        max_speed_mps: 400.0,
        r95_cap_m: 2000.0,
        procs: [ProcArea {
            area: POLYGON_EMPTY,
            floor_ft: 0,
        }; MAX_PROCS],
        n_procs: 0,
    }
}

struct Rig {
    st: MsawState,
    alerts: AlertLog,
    skips: SkipLog,
    net: StateTable,
    inh: Box<InhibitTable>,
}

fn rig(c: &MsawConfig) -> Rig {
    Rig {
        st: msaw_new(c).unwrap(),
        alerts: alert_log_new(1000).unwrap(),
        skips: skip_log_new(1000).unwrap(),
        net: state_table_new(10 * S, 2 * S).unwrap(),
        inh: Box::new(inhibits_new()),
    }
}

fn tr(addr: u32, lat: f64, lon: f64, ve: f32, vn: f32, alt: f32, vr: f32) -> MsawTrack {
    MsawTrack {
        lat_deg: lat,
        lon_deg: lon,
        addr,
        vel_east_mps: ve,
        vel_north_mps: vn,
        r95_m: 50.0,
        alt_ft: alt,
        vrate_fpm: vr,
        flags: 0,
        ground: AIRBORNE,
    }
}

/// One cycle with floors from `world` (terrain, obstacle, chart in feet at a point).
fn cycle(
    r: &mut Rig,
    t: i64,
    rows: &[MsawTrack],
    world: &dyn Fn(i32, i32) -> FloorSample,
) -> CycleOut {
    msaw_plan(&mut r.st, t, rows, &mut r.skips);
    let floors: Vec<FloorSample> = msaw_points(&r.st)
        .iter()
        .map(|p| world(p.lat_e7, p.lon_e7))
        .collect();
    msaw_evaluate(
        &mut r.st,
        &floors,
        &mut r.alerts,
        &mut r.skips,
        &mut r.net,
        &mut r.inh,
    )
}

fn flat(terrain: i32, mva: i32) -> impl Fn(i32, i32) -> FloorSample {
    move |_, _| FloorSample {
        terrain_ft: terrain,
        obstacle_ft: FLOOR_NONE,
        mva_ft: mva,
    }
}

fn state(r: &Rig, t: i64) -> NetState {
    net_status(&r.net, &r.inh, NetId::Msaw, t).state
}

type Edit = fn(&mut MsawConfig);

// REQ: OA-MSAW-001
#[test]
fn a_configuration_out_of_range_is_refused_naming_the_key() {
    assert!(msaw_check(&cfg()).is_ok());
    let cases: [(&str, Edit); 11] = [
        ("max_tracks", |c| c.max_tracks = 0),
        ("max_tracks", |c| c.max_tracks = MAX_TRACKS_LIMIT + 1),
        ("max_open", |c| c.max_open = MAX_OPEN_LIMIT + 1),
        ("track_budget", |c| c.track_budget = 0),
        ("look_ahead_s", |c| c.look_ahead_s = MAX_LOOK_AHEAD_S + 1.0),
        ("look_ahead_s", |c| c.look_ahead_s = f64::NAN),
        ("warning_s", |c| c.warning_s = 61.0),
        ("samples", |c| c.samples = 1),
        ("samples", |c| c.samples = MAX_SAMPLES + 1),
        ("hysteresis_ns", |c| c.hysteresis_ns = 0),
        ("terrain_clearance_ft", |c| c.terrain_clearance_ft = -1),
    ];
    for (key, f) in cases {
        let mut c = cfg();
        f(&mut c);
        let e = msaw_check(&c).unwrap_err();
        assert!(e.starts_with(key), "{key}: {e}");
        assert!(msaw_new(&c).is_err());
    }
    let mut c = cfg();
    c.max_speed_mps = 1000.0;
    c.look_ahead_s = 600.0;
    c.samples = 2;
    assert!(msaw_check(&c).unwrap_err().contains("query radius"));
    let mut c = cfg();
    c.n_procs = 1;
    assert_eq!(msaw_check(&c).unwrap_err(), "procedure area vertex count");
}

// REQ: OA-MSAW-002
#[test]
fn aircraft_on_the_ground_are_excluded_and_counted() {
    let mut r = rig(&cfg());
    let mut g = tr(1, 40.0, -88.0, 0.0, 0.0, 600.0, 0.0);
    g.ground = ON_GROUND;
    let a = tr(2, 40.0, -88.0, 0.0, 100.0, 20_000.0, 0.0);
    let o = cycle(&mut r, S, &[g, a], &flat(500, 2000));
    assert_eq!(o.stats.ground_excluded, 1);
    assert_eq!(o.stats.evaluated, 1);
    assert_eq!(msaw_planned(&r.st).len(), 1);
    assert_eq!(o.stats.violations, 0);
    assert!(r.skips.items.is_empty());
    assert_eq!(o.report, NetReport::Running);
}

// REQ: OA-MSAW-003
#[test]
fn a_track_with_no_altitude_is_counted_listed_and_the_net_is_degraded() {
    let mut r = rig(&cfg());
    let a = tr(7, 40.0, -88.0, 0.0, 100.0, f32::NAN, f32::NAN);
    let b = tr(8, 40.1, -88.0, 0.0, 100.0, 20_000.0, 0.0);
    let o = cycle(&mut r, S, &[a, b], &flat(500, 2000));
    assert_eq!(o.stats.alt_missing, 1);
    assert_eq!(r.skips.items.len(), 1);
    let s = r.skips.items[0];
    assert_eq!((s.addr, s.why, s.t_ns), (7, SkipWhy::AltitudeMissing, S));
    assert_eq!(
        o.report,
        NetReport::Degraded {
            reason: Reason::AltitudeMissing,
            area: oa_safety::net::Area::Whole,
            count: 1
        }
    );
    assert!(matches!(state(&r, S), NetState::Degraded { count: 1, .. }));
    // The other track was still evaluated.
    assert_eq!(o.stats.evaluated, 1);
}

// REQ: OA-MSAW-004
#[test]
fn no_terrain_or_chart_coverage_is_counted_and_never_treated_as_safe() {
    let mut r = rig(&cfg());
    // A low track, with no terrain coverage at all: no floor, no alert, and said so.
    let a = tr(1, 40.0, -88.0, 0.0, 100.0, 100.0, 0.0);
    let none_t = |_: i32, _: i32| FloorSample {
        terrain_ft: FLOOR_NONE,
        obstacle_ft: FLOOR_NONE,
        mva_ft: 2000,
    };
    let o = cycle(&mut r, S, &[a], &none_t);
    assert_eq!(o.stats.no_coverage, 1);
    assert_eq!(o.stats.violations, 0);
    assert_eq!(r.skips.items[0].why, SkipWhy::NoCoverage);
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 1,
            ..
        }
    ));
    // No chart coverage is the same.
    let none_m = |_: i32, _: i32| FloorSample {
        terrain_ft: 0,
        obstacle_ft: FLOOR_NONE,
        mva_ft: FLOOR_NONE,
    };
    let o = cycle(&mut r, 2 * S, &[a], &none_m);
    assert_eq!(o.stats.no_coverage, 1);
    // Coverage that stops partway: the covered points are still judged, and the track is listed.
    let half = |lat: i32, _: i32| {
        if lat < 400_005_000 {
            FloorSample {
                terrain_ft: 0,
                obstacle_ft: FLOOR_NONE,
                mva_ft: 2000,
            }
        } else {
            FloorSample {
                terrain_ft: FLOOR_NONE,
                obstacle_ft: FLOOR_NONE,
                mva_ft: FLOOR_NONE,
            }
        }
    };
    let o = cycle(&mut r, 3 * S, &[a], &half);
    assert_eq!(o.stats.no_coverage, 1);
    assert_eq!(
        o.stats.violations, 1,
        "the covered part is below the chart floor"
    );
    assert_ne!(r.alerts.items.len() + msaw_open_len(&r.st), 0);
    let open = msaw_open(&r.st);
    assert_ne!(open[0].flags & AF_PARTIAL_COVERAGE, 0);
}

// REQ: OA-MSAW-005
#[test]
fn over_budget_the_lowest_altitudes_are_evaluated_first_and_the_rest_counted() {
    let mut c = cfg();
    c.track_budget = 2;
    let mut r = rig(&c);
    let rows: Vec<MsawTrack> = (0..5u32)
        .map(|i| {
            tr(
                10 + i,
                40.0 + f64::from(i) * 0.5,
                -88.0,
                0.0,
                50.0,
                8000.0 - 1000.0 * i as f32,
                0.0,
            )
        })
        .collect();
    let o = cycle(&mut r, S, &rows, &flat(0, 1000));
    assert_eq!(o.stats.evaluated, 2);
    assert_eq!(o.stats.over_budget, 3);
    // The two lowest, in address order.
    let kept: Vec<u32> = msaw_planned(&r.st).iter().map(|t| t.addr).collect();
    assert_eq!(kept, vec![13, 14]);
    let mut listed: Vec<u32> = r
        .skips
        .items
        .iter()
        .filter(|s| s.why == SkipWhy::OverBudget)
        .map(|s| s.addr)
        .collect();
    listed.sort_unstable();
    assert_eq!(listed, vec![10, 11, 12]);
    assert_eq!(
        o.report,
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            area: oa_safety::net::Area::Whole,
            count: 3
        }
    );
    // Rows past the track capacity are counted too.
    let mut c = cfg();
    c.max_tracks = 2;
    let mut r = rig(&c);
    let o = cycle(&mut r, S, &rows, &flat(0, 1000));
    assert_eq!(o.stats.rows_over_capacity, 3);
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            count: 3,
            ..
        }
    ));
}

/// The synthetic approach: heading north at 100 m/s from latitude 13.40 over a ridge that rises
/// 3,000 ft per 0.1 degree, descending early at 700 ft/min from 2,000 ft.
fn ridge(lat_e7: i32, _lon: i32) -> FloorSample {
    let lat = f64::from(lat_e7) / 1e7;
    let t = ((lat - 13.40) / 0.10 * 3000.0).max(0.0);
    FloorSample {
        terrain_ft: t.ceil() as i32,
        obstacle_ft: FLOOR_NONE,
        mva_ft: 100,
    }
}

fn approach(c: i64) -> MsawTrack {
    let s = c as f64;
    tr(
        801,
        13.40 + 0.0009 * s,
        144.80,
        0.0,
        100.0,
        (2000.0 - 700.0 / 60.0 * s) as f32,
        -700.0,
    )
}

// REQ: OA-MSAW-006
#[test]
fn guam_synthetic_an_early_descent_onto_rising_terrain_alerts_caution_then_warning() {
    let mut r = rig(&cfg());
    for c in 0..40 {
        cycle(&mut r, c * S, &[approach(c)], &ridge);
    }
    msaw_finish(&mut r.st, &mut r.alerts);
    assert_eq!(r.alerts.items.len(), 1);
    let a = r.alerts.items[0];
    assert_eq!(a.addr, 801);
    let caution = a.caution_ns.unwrap();
    let warning = a.warning_ns.unwrap();
    assert_eq!(
        caution, 0,
        "caution at the first cycle: the violation is 38 s ahead"
    );
    assert!(warning > caution && warning <= 25 * S, "warning {warning}");
    assert_eq!(a.peak, Grade::Warning);
    assert_eq!(a.worst_kind, FloorKind::Terrain);
    assert!(a.worst_margin_ft < 0.0);
}

fn inhibit_area(end_ns: i64) -> Box<InhibitTable> {
    let mut t = Box::new(inhibits_new());
    let cfg = InhibitConfig {
        max_duration_ns: [Some(3600 * S); 4],
        max_start_ahead_ns: Some(3600 * S),
        max_extent_e7: Some(20_000_000),
    };
    let area = [
        (132_000_000, 1_446_000_000),
        (132_000_000, 1_450_000_000),
        (136_000_000, 1_450_000_000),
        (136_000_000, 1_446_000_000),
    ];
    // The ridge is in the area's band; the inhibition is the Guam one: on purpose, for nuisance.
    let r = InhibitRequest {
        net: NetId::Msaw,
        vertices: &area,
        alt_low_ft: -1000,
        alt_high_ft: 20_000,
        start_ns: 0,
        end_ns,
        reason: "NUISANCE ALERTS",
        author: "SUPERVISOR 1",
    };
    inhibit_create(&mut t, &cfg, &r, 0).unwrap();
    t
}

// REQ: OA-MSAW-007
#[test]
fn guam_synthetic_with_an_inhibition_there_is_no_alert_and_the_inhibition_is_visible() {
    let mut r = rig(&cfg());
    r.inh = inhibit_area(30 * S);
    for c in 0..25 {
        cycle(&mut r, c * S, &[approach(c)], &ridge);
    }
    assert!(
        r.alerts.items.is_empty() && msaw_open(&r.st).is_empty(),
        "hidden"
    );
    let t = msaw_totals(&r.st);
    assert!(t.suppressed >= 20, "{}", t.suppressed);
    assert_eq!(t.opened, 0);
    // The net says it is inhibited, and the inhibition is in the output with its end time.
    let s = net_status(&r.net, &r.inh, NetId::Msaw, 24 * S);
    assert_eq!(s.state, NetState::Inhibited);
    assert_eq!(s.inhibitions, 1);
    let i = inhibits_slice(&r.inh);
    assert_eq!(i.len(), 1);
    assert_eq!(i[0].end_ns, 30 * S);
    assert_eq!(text_of(&i[0].reason), "NUISANCE ALERTS");
    // Every hidden violation is recorded against the inhibition's id.
    let log = suppressed_log(&r.inh);
    assert_eq!(log.len() as u64, t.suppressed);
    assert!(log
        .iter()
        .all(|e| e.inhibition_id == i[0].id && e.net == NetId::Msaw));
    // It expires: from its end time on, the same violation alerts.
    let o = cycle(&mut r, 30 * S, &[approach(30)], &ridge);
    assert_eq!(o.stats.suppressed, 0);
    assert_eq!(o.stats.opened, 1);
    assert_eq!(msaw_open(&r.st)[0].peak, Grade::Warning);
}

// REQ: OA-MSAW-007
#[test]
fn an_inhibition_of_another_net_or_another_band_hides_nothing() {
    let mut r = rig(&cfg());
    // The band is above the aircraft: no suppression.
    let mut t = Box::new(inhibits_new());
    let cfgi = InhibitConfig {
        max_duration_ns: [Some(3600 * S); 4],
        max_start_ahead_ns: Some(3600 * S),
        max_extent_e7: Some(20_000_000),
    };
    let area = [
        (132_000_000, 1_446_000_000),
        (132_000_000, 1_450_000_000),
        (136_000_000, 1_450_000_000),
        (136_000_000, 1_446_000_000),
    ];
    for (net, lo, hi) in [
        (NetId::ConflictAlert, -1000, 20_000),
        (NetId::Msaw, 10_000, 20_000),
    ] {
        let q = InhibitRequest {
            net,
            vertices: &area,
            alt_low_ft: lo,
            alt_high_ft: hi,
            start_ns: 0,
            end_ns: 100 * S,
            reason: "X",
            author: "Y",
        };
        inhibit_create(&mut t, &cfgi, &q, 0).unwrap();
    }
    r.inh = t;
    let o = cycle(&mut r, S, &[approach(1)], &ridge);
    assert_eq!(o.stats.suppressed, 0);
    assert_eq!(o.stats.opened, 1);
}

// REQ: OA-MSAW-008
#[test]
fn the_floor_is_the_higher_of_terrain_obstacle_and_chart_with_clearances() {
    let c = cfg();
    let s = |t, o, m| FloorSample {
        terrain_ft: t,
        obstacle_ft: o,
        mva_ft: m,
    };
    assert_eq!(
        floor_of(&s(1000, FLOOR_NONE, 1200), None, &c),
        Some((1500, FloorKind::Terrain))
    );
    assert_eq!(
        floor_of(&s(1000, 1200, 1200), None, &c),
        Some((1700, FloorKind::Obstacle))
    );
    assert_eq!(
        floor_of(&s(1000, 1200, 3000), None, &c),
        Some((3000, FloorKind::Mva))
    );
    // A tie goes to the terrain, then the obstacle.
    assert_eq!(
        floor_of(&s(1000, 1000, 1500), None, &c),
        Some((1500, FloorKind::Terrain))
    );
    // No coverage: no floor.
    assert_eq!(floor_of(&s(FLOOR_NONE, 0, 0), None, &c), None);
    assert_eq!(floor_of(&s(0, 0, FLOOR_NONE), None, &c), None);
    // No obstacle is not no coverage.
    assert!(floor_of(&s(0, FLOOR_NONE, 0), None, &c).is_some());
}

// REQ: OA-MSAW-009
#[test]
fn a_procedure_area_lowers_the_floor_in_that_area_only() {
    let mut c = cfg();
    let sq =
        |a: i32, b: i32, c2: i32, d: i32| polygon_new(&[(a, c2), (a, d), (b, d), (b, c2)]).unwrap();
    c.procs[0] = ProcArea {
        area: sq(400_000_000, 401_000_000, -880_000_000, -879_000_000),
        floor_ft: 1200,
    };
    c.procs[1] = ProcArea {
        area: sq(400_000_000, 401_000_000, -880_000_000, -879_000_000),
        floor_ft: 1500,
    };
    c.n_procs = 2;
    assert!(msaw_check(&c).is_ok());
    // Inside, the highest procedure altitude of the areas that contain the point; outside, none.
    assert_eq!(proc_floor_at(&c, 400_500_000, -879_500_000), Some(1500));
    assert_eq!(proc_floor_at(&c, 410_500_000, -879_500_000), None);
    let s = FloorSample {
        terrain_ft: 600,
        obstacle_ft: FLOOR_NONE,
        mva_ft: 3000,
    };
    // Outside: the normal floor. Inside: the published altitude, when lower.
    assert_eq!(floor_of(&s, None, &c), Some((3000, FloorKind::Mva)));
    assert_eq!(
        floor_of(&s, Some(1500), &c),
        Some((1500, FloorKind::Procedure))
    );
    // A procedure altitude above the normal floor never raises it.
    assert_eq!(floor_of(&s, Some(4000), &c), Some((3000, FloorKind::Mva)));
    // No coverage is no floor, inside a procedure area as well.
    let n = FloorSample {
        terrain_ft: FLOOR_NONE,
        ..s
    };
    assert_eq!(floor_of(&n, Some(1500), &c), None);
    // Through a cycle: a landing aircraft at 1,600 ft inside the area is clear; outside it alerts.
    let mut rg = rig(&c);
    let inside = tr(
        1,
        400_500_000.0 / 1e7,
        -879_500_000.0 / 1e7 + 0.0,
        0.0,
        0.0,
        1600.0,
        0.0,
    );
    let world = |_: i32, _: i32| FloorSample {
        terrain_ft: 600,
        obstacle_ft: FLOOR_NONE,
        mva_ft: 3000,
    };
    let o = cycle(&mut rg, S, &[inside], &world);
    assert_eq!(o.stats.violations, 0);
    let outside = tr(2, 41.0, -87.0, 0.0, 0.0, 1600.0, 0.0);
    let o = cycle(&mut rg, 2 * S, &[outside], &world);
    assert_eq!(o.stats.violations, 1);
}

// REQ: OA-MSAW-010
#[test]
fn the_grade_follows_the_time_to_the_violation() {
    assert_eq!(grade_of(0.0, 20.0), Grade::Warning);
    assert_eq!(grade_of(20.0, 20.0), Grade::Warning);
    assert_eq!(grade_of(20.1, 20.0), Grade::Caution);
    // Already below the floor: a warning at once.
    let mut r = rig(&cfg());
    let o = cycle(
        &mut r,
        S,
        &[tr(1, 40.0, -88.0, 0.0, 0.0, 1000.0, 0.0)],
        &flat(2000, 100),
    );
    assert_eq!(o.stats.violations, 1);
    let a = msaw_open(&r.st)[0];
    assert_eq!((a.peak, a.ttv_min_s), (Grade::Warning, 0.0));
    // A descent that reaches the floor in about 45 s is a caution at first sight.
    let mut r = rig(&cfg());
    cycle(
        &mut r,
        S,
        &[tr(1, 40.0, -88.0, 0.0, 0.0, 3000.0, -2000.0)],
        &flat(1000, 100),
    );
    let a = msaw_open(&r.st)[0];
    assert_eq!(a.peak, Grade::Caution);
    assert!(a.warning_ns.is_none() && a.caution_ns == Some(S));
    assert!((44.0..=46.0).contains(&a.ttv_min_s), "{}", a.ttv_min_s);
    // Beyond the look-ahead: nothing.
    let mut r = rig(&cfg());
    let o = cycle(
        &mut r,
        S,
        &[tr(1, 40.0, -88.0, 0.0, 0.0, 3000.0, -300.0)],
        &flat(1000, 100),
    );
    assert_eq!(o.stats.violations, 0);
}

// REQ: OA-MSAW-011
#[test]
fn one_alert_per_track_ends_after_the_hysteresis() {
    let mut r = rig(&cfg());
    let low = tr(1, 40.0, -88.0, 0.0, 0.0, 1000.0, 0.0);
    let high = tr(1, 40.0, -88.0, 0.0, 0.0, 9000.0, 0.0);
    for c in 1..=3 {
        cycle(&mut r, c * S, &[low], &flat(2000, 100));
    }
    assert_eq!(msaw_open_len(&r.st), 1);
    assert_eq!(msaw_open(&r.st)[0].cycles, 3);
    // Clear for four seconds: still open; for more than five: ended.
    cycle(&mut r, 7 * S, &[high], &flat(2000, 100));
    assert_eq!(msaw_open_len(&r.st), 1);
    assert!(r.alerts.items.is_empty());
    let o = cycle(&mut r, 9 * S, &[high], &flat(2000, 100));
    assert_eq!(o.stats.closed, 1);
    assert_eq!(r.alerts.items.len(), 1);
    assert_eq!(
        (r.alerts.items[0].start_ns, r.alerts.items[0].end_ns),
        (S, 3 * S)
    );
    assert_eq!(msaw_open_len(&r.st), 0);
    // The end of the run ends what is open.
    cycle(&mut r, 20 * S, &[low], &flat(2000, 100));
    assert_eq!(msaw_finish(&mut r.st, &mut r.alerts), 0);
    assert_eq!(r.alerts.items.len(), 2);
}

// REQ: OA-MSAW-012
#[test]
fn a_full_open_table_logs_the_alert_at_once_and_counts_it() {
    let mut c = cfg();
    c.max_open = 1;
    let mut r = rig(&c);
    let rows = [
        tr(1, 40.0, -88.0, 0.0, 0.0, 1000.0, 0.0),
        tr(2, 40.5, -88.0, 0.0, 0.0, 1000.0, 0.0),
    ];
    let o = cycle(&mut r, S, &rows, &flat(2000, 100));
    assert_eq!((o.stats.opened, o.stats.open_full), (1, 1));
    assert_eq!(r.alerts.items.len(), 1, "the second alert is not lost");
    assert_eq!(r.alerts.items[0].addr, 2);
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            ..
        }
    ));
}

// REQ: OA-MSAW-013
#[test]
fn a_full_log_counts_what_it_drops_and_the_net_reads_degraded() {
    let mut r = rig(&cfg());
    r.alerts = alert_log_new(0).unwrap();
    r.skips = skip_log_new(0).unwrap();
    let rows = [tr(1, 40.0, -88.0, 0.0, 0.0, f32::NAN, 0.0)];
    let o = cycle(&mut r, S, &rows, &flat(0, 0));
    assert_eq!(o.stats.skips_dropped, 1);
    assert_eq!(r.skips.overflow, 1);
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            count: 2,
            ..
        }
    ));
    let low = [tr(1, 40.0, -88.0, 0.0, 0.0, 100.0, 0.0)];
    cycle(&mut r, 2 * S, &low, &flat(2000, 100));
    let dropped = msaw_finish(&mut r.st, &mut r.alerts);
    assert_eq!(dropped, 1);
    assert_eq!(r.alerts.overflow, 1);
    assert_eq!(msaw_totals(&r.st).alerts_dropped, 1);
}

// REQ: OA-MSAW-014
#[test]
fn a_cycle_out_of_order_or_with_the_wrong_floors_publishes_unavailable() {
    let mut r = rig(&cfg());
    cycle(&mut r, 5 * S, &[], &flat(0, 0));
    assert_eq!(state(&r, 5 * S), NetState::Running);
    let o = cycle(&mut r, 5 * S, &[], &flat(0, 0));
    assert_eq!(o.stats.out_of_order, 1);
    assert_eq!(
        o.report,
        NetReport::Unavailable {
            reason: Reason::SurveillanceLost
        }
    );
    let rows = [tr(1, 40.0, -88.0, 0.0, 0.0, 9000.0, 0.0)];
    msaw_plan(&mut r.st, 6 * S, &rows, &mut r.skips);
    let o = msaw_evaluate(
        &mut r.st,
        &[],
        &mut r.alerts,
        &mut r.skips,
        &mut r.net,
        &mut r.inh,
    );
    assert_eq!(o.stats.floor_mismatch, 1);
    assert_eq!(
        o.report,
        NetReport::Unavailable {
            reason: Reason::InputMissing
        }
    );
    assert!(matches!(state(&r, 6 * S), NetState::Unavailable { .. }));
    // A net that has not reported for longer than the limit reads unavailable on the table.
    assert!(matches!(
        state(&r, 100 * S),
        NetState::Unavailable {
            reason: Reason::StateStale
        }
    ));
    let mut net = state_table_new(S, S).unwrap();
    assert!(msaw_publish_refused(&mut net, 3 * S));
}

// REQ: OA-MSAW-015
#[test]
fn the_sample_points_follow_the_straight_path_with_a_radius_that_covers_the_gaps() {
    let c = cfg();
    let start = PathStart {
        lat_deg: 40.0,
        lon_deg: -88.0,
        ve: 100.0,
        vn: 0.0,
        r95_m: 50.0,
    };
    let mut out = [SamplePoint {
        lat_e7: 0,
        lon_e7: 0,
        radius_m: 0.0,
    }; 13];
    assert_eq!(sample_points(&start, &c, &mut out), Some(13));
    assert_eq!((out[0].lat_e7, out[0].lon_e7), (400_000_000, -880_000_000));
    // 100 m/s east for 60 s at 40 N: about 6 km, 0.0704 degrees of longitude.
    let dlon = f64::from(out[12].lon_e7 + 880_000_000) / 1e7;
    assert!((dlon - 0.0704).abs() < 0.001, "{dlon}");
    assert_eq!(out[12].lat_e7, 400_000_000);
    // Radius: 50 m plus half the 5 s step at 100 m/s.
    assert!((f64::from(out[3].radius_m) - 300.0).abs() < 0.01);
    // Times and spans.
    assert_eq!(sample_time_s(12, 13, 60.0), 60.0);
    assert_eq!(sample_span_s(0, 13, 60.0), (0.0, 2.5));
    assert_eq!(sample_span_s(12, 13, 60.0), (57.5, 60.0));
    // A short output slice is filled to its length; a bad start is refused.
    let mut small = [out[0]; 3];
    assert_eq!(sample_points(&start, &c, &mut small), Some(3));
    let bad = PathStart {
        lat_deg: 89.0,
        ..start
    };
    assert_eq!(sample_points(&bad, &c, &mut out), None);
    let bad = PathStart {
        ve: f64::NAN,
        ..start
    };
    assert_eq!(sample_points(&bad, &c, &mut out), None);
}

// REQ: OA-MSAW-016, OA-MSAW-020, OA-MSAW-022
#[test]
fn a_descent_between_two_points_is_judged_at_the_lowest_altitude_of_the_span() {
    // 600 ft/min is 10 ft/s: over a 5 s step the aircraft drops 50 ft between sample times.
    let low = lowest_alt_ft(1000.0, -600.0, 2.5, 7.5);
    assert_eq!(low, 925.0);
    assert_eq!(lowest_alt_ft(1000.0, 600.0, 2.5, 7.5), 1025.0);
    assert_eq!(predicted_alt_ft(1000.0, -600.0, 0.0), 1000.0);
    assert_eq!(alt_floor_ft(924.9), 924);
    assert_eq!(alt_floor_ft(-0.5), -1);
    assert_eq!(alt_floor_ft(f64::NAN), i32::MIN);
    // The violation time is where the line crosses the floor, inside the span.
    let t = violation_time_s(1000.0, -600.0, 960, 2.5, 7.5);
    assert!((t - 4.0).abs() < 1e-9, "{t}");
    assert_eq!(violation_time_s(900.0, -600.0, 960, 2.5, 7.5), 2.5);
    assert_eq!(violation_time_s(900.0, 600.0, 960, 2.5, 7.5), 2.5);
    // A rate that rounds to nothing (a denormal, which a proof found gave 0/0) gives the earliest time.
    assert_eq!(violation_time_s(960.0, -1.0e-320, 960, 2.5, 7.5), 2.5);
    // A floor that a point's span just dips under is caught: 1,030 ft at 8 s sample, floor 1,000.
    let mut r = rig(&cfg());
    let a = tr(1, 40.0, -88.0, 0.0, 0.0, 1060.0, -600.0);
    let o = cycle(&mut r, S, &[a], &flat(500, 100));
    assert_eq!(o.stats.violations, 1);
}

// REQ: OA-MSAW-017
#[test]
fn the_report_gives_the_reason_of_highest_priority_and_the_whole_count() {
    let s = CycleStats::default();
    assert_eq!(net_report_of(&s), NetReport::Running);
    let alt = CycleStats {
        alt_missing: 2,
        ..s
    };
    assert!(matches!(
        net_report_of(&alt),
        NetReport::Degraded {
            reason: Reason::AltitudeMissing,
            count: 2,
            ..
        }
    ));
    let input = CycleStats {
        no_coverage: 1,
        rows_bad: 1,
        rows_lost_upstream: 1,
        ..alt
    };
    assert!(matches!(
        net_report_of(&input),
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 5,
            ..
        }
    ));
    let work = CycleStats {
        over_budget: 4,
        ..input
    };
    assert!(matches!(
        net_report_of(&work),
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            count: 9,
            ..
        }
    ));
    // Rows told lost upstream degrade the next cycle once.
    let mut r = rig(&cfg());
    msaw_note_lost_input(&mut r.st, 3);
    let o = cycle(&mut r, S, &[], &flat(0, 0));
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 3,
            ..
        }
    ));
    let o = cycle(&mut r, 2 * S, &[], &flat(0, 0));
    assert_eq!(o.report, NetReport::Running);
}

// REQ: OA-MSAW-018
#[test]
fn an_unknown_vertical_rate_predicts_level_and_is_flagged() {
    let mut r = rig(&cfg());
    let a = tr(1, 40.0, -88.0, 0.0, 0.0, 1000.0, f32::NAN);
    let o = cycle(&mut r, S, &[a], &flat(2000, 100));
    assert_eq!(o.stats.rate_unknown, 1);
    assert_eq!(o.stats.violations, 1);
    assert_ne!(msaw_open(&r.st)[0].flags & AF_RATE_UNKNOWN, 0);
    // Level above the floor: nothing.
    let b = tr(2, 41.0, -88.0, 0.0, 0.0, 5000.0, f32::NAN);
    let o = cycle(&mut r, 2 * S, &[b], &flat(2000, 100));
    assert_eq!(o.stats.violations, 0);
}

// REQ: OA-MSAW-019
#[test]
fn a_bad_row_is_counted_and_listed_and_a_poor_radius_is_held_to_the_cap() {
    let mut r = rig(&cfg());
    let mut rows = vec![
        tr(1, f64::NAN, -88.0, 0.0, 0.0, 5000.0, 0.0),
        tr(2, 40.0, -88.0, 900.0, 0.0, 5000.0, 0.0),
        tr(3, 40.0, -88.0, 0.0, 0.0, 5.0e5, 0.0),
        tr(4, 40.0, -88.0, 0.0, 0.0, 5000.0, 9.0e4),
        tr(5, 88.0, -88.0, 0.0, 0.0, 5000.0, 0.0),
    ];
    let mut ok = tr(6, 40.0, -88.0, 0.0, 0.0, 5000.0, 0.0);
    ok.r95_m = f32::NAN;
    rows.push(ok);
    let mut big = tr(7, 40.1, -88.0, 0.0, 0.0, 5000.0, 0.0);
    big.r95_m = 1.0e6;
    rows.push(big);
    let o = cycle(&mut r, S, &rows, &flat(0, 100));
    assert_eq!(o.stats.rows_bad, 5);
    assert_eq!(
        r.skips
            .items
            .iter()
            .filter(|s| s.why == SkipWhy::BadRow)
            .count(),
        5
    );
    assert_eq!(o.stats.r95_held, 2);
    assert!(msaw_points(&r.st)
        .iter()
        .all(|p| f64::from(p.radius_m) <= 2000.0 + 400.0 * 2.5));
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 5,
            ..
        }
    ));
}

// REQ: OA-MSAW-023
#[test]
fn a_capacity_above_its_bound_is_refused_naming_the_key() {
    let mut c = cfg();
    c.max_tracks = MAX_TRACKS_LIMIT;
    c.samples = MAX_SAMPLES;
    assert_eq!(msaw_check(&c).unwrap_err(), "max_tracks times samples");
    assert!(alert_log_new(usize::MAX)
        .unwrap_err()
        .contains("alert_log_capacity"));
    assert!(skip_log_new(usize::MAX)
        .unwrap_err()
        .contains("skip_log_capacity"));
    // A capacity within the bounds is allocated whole at the start and never grows after.
    let c = cfg();
    let st = msaw_new(&c).unwrap();
    assert_eq!(msaw_open_len(&st), 0);
    assert_eq!(MAX_SAMPLES, 32);
}

// REQ: OA-MSAW-024
#[test]
fn the_same_cycles_give_the_same_alerts() {
    let run = || {
        let mut r = rig(&cfg());
        for c in 0..40 {
            cycle(
                &mut r,
                c * S,
                &[approach(c), tr(9, 40.0, -88.0, 0.0, 0.0, f32::NAN, 0.0)],
                &ridge,
            );
        }
        msaw_finish(&mut r.st, &mut r.alerts);
        (
            r.alerts.items.clone(),
            r.skips.items.len(),
            msaw_totals(&r.st),
        )
    };
    assert_eq!(run(), run());
}

// REQ: OA-MSAW-021
#[test]
fn a_higher_floor_never_removes_an_alert_over_a_sweep() {
    let c = cfg();
    for terrain in [-500, 0, 800, 5000] {
        for mva in [0, 1500, 4000] {
            for obstacle in [FLOOR_NONE, 100, 3000] {
                for bump in [0, 1, 700] {
                    let a = FloorSample {
                        terrain_ft: terrain,
                        obstacle_ft: obstacle,
                        mva_ft: mva,
                    };
                    let b = FloorSample {
                        terrain_ft: terrain + bump,
                        obstacle_ft: if obstacle == FLOOR_NONE {
                            FLOOR_NONE
                        } else {
                            obstacle + bump
                        },
                        mva_ft: mva + bump,
                    };
                    let (fa, _) = floor_of(&a, None, &c).unwrap();
                    let (fb, _) = floor_of(&b, None, &c).unwrap();
                    assert!(fa <= fb);
                    for alt in [-100, 0, 1000, 2000, 6000] {
                        if violates(alt, fa) {
                            assert!(violates(alt, fb));
                        }
                    }
                }
            }
        }
    }
}

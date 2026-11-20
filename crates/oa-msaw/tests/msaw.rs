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
    alt_floor_ft, corridor_hit_ft, corridor_hit_linear, grade_of, lowest_alt_ft, predicted_alt_ft,
    sample_points, sample_span_s, sample_time_s, violation_time_s, PathStart,
};
use oa_msaw::*;
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
        approach_allowance_ft: 300,
        approach_min_clearance_ft: 100,
        departure_min_clearance_ft: 100,
        velocity_uncertainty_mps: 5.0,
        max_altitude_age_s: 12.0,
        terminal_lookahead_s: 15.0,
        corridors: [blank_corridor(); MAX_CORRIDORS],
        n_corridors: 0,
    }
}

fn blank_corridor() -> Corridor {
    Corridor {
        kind: CK_APPROACH,
        thr_lat_e7: 0,
        thr_lon_e7: 0,
        out_bearing_deg: 0.0,
        length_m: 1.0,
        near_half_width_m: 1.0,
        far_half_width_m: 1.0,
        thr_elev_ft: 0,
        faf_alt_ft: 0,
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
        alt_age_s: 0.0,
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
        obstacle_covered: true,
        mva_ft: mva,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
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
    c.n_corridors = 1;
    c.corridors[0].length_m = 0.0;
    assert_eq!(msaw_check(&c).unwrap_err(), "corridor geometry");
    c.corridors[0] = north_corridor();
    assert!(msaw_check(&c).is_ok());
    c.corridors[0].faf_alt_ft = MAX_CLEARANCE_FT + 1;
    assert_eq!(msaw_check(&c).unwrap_err(), "corridor elevation");
    c.n_corridors = MAX_CORRIDORS + 1;
    assert_eq!(msaw_check(&c).unwrap_err(), "n_corridors");
    let mut c = cfg();
    c.velocity_uncertainty_mps = -1.0;
    assert_eq!(msaw_check(&c).unwrap_err(), "velocity_uncertainty_mps");
    let mut c = cfg();
    c.approach_min_clearance_ft = -1;
    assert_eq!(msaw_check(&c).unwrap_err(), "approach_min_clearance_ft");
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
        obstacle_covered: true,
        mva_ft: 2000,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
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
    // No chart coverage is not a shortfall: the chart is not a floor.
    let none_m = |_: i32, _: i32| FloorSample {
        terrain_ft: 0,
        obstacle_ft: FLOOR_NONE,
        obstacle_covered: true,
        mva_ft: FLOOR_NONE,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
    };
    let o = cycle(&mut r, 2 * S, &[a], &none_m);
    assert_eq!(o.stats.no_coverage, 0);
    assert_eq!(o.report, NetReport::Running);
    // Coverage that stops partway: the covered points are still judged, and the track is listed.
    let half = |lat: i32, _: i32| {
        if lat < 400_005_000 {
            FloorSample {
                terrain_ft: 0,
                obstacle_ft: FLOOR_NONE,
                obstacle_covered: true,
                mva_ft: 2000,
                gtm_clearance_ft: FLOOR_NONE,
                apt_clearance_ft: FLOOR_NONE,
                apt_ceiling_ft: FLOOR_NONE,
            }
        } else {
            FloorSample {
                terrain_ft: FLOOR_NONE,
                obstacle_ft: FLOOR_NONE,
                obstacle_covered: true,
                mva_ft: FLOOR_NONE,
                gtm_clearance_ft: FLOOR_NONE,
                apt_clearance_ft: FLOOR_NONE,
                apt_ceiling_ft: FLOOR_NONE,
            }
        }
    };
    let o = cycle(&mut r, 3 * S, &[a], &half);
    assert_eq!(o.stats.no_coverage, 1);
    assert_eq!(
        o.stats.violations, 1,
        "the covered part is below the terrain floor"
    );
    assert_ne!(r.alerts.items.len() + msaw_open_len(&r.st), 0);
    let open = msaw_open(&r.st);
    assert_ne!(open[0].flags & AF_PARTIAL_COVERAGE, 0);
}

// REQ: OA-MSAW-005
#[test]
fn over_budget_the_lowest_predicted_altitudes_are_evaluated_first_and_the_rest_counted() {
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
    // The order is by the altitude predicted at the end of the look-ahead: a higher aircraft
    // descending fast (5,000 ft at -6,000 fpm is at -1,000 ft in a minute) comes before a lower
    // one that is level (3,000 ft), and an equal prediction goes by address.
    let mut c = cfg();
    c.track_budget = 2;
    let mut r = rig(&c);
    let rows = [
        tr(30, 40.0, -88.0, 0.0, 50.0, 3000.0, 0.0),
        tr(31, 40.5, -88.0, 0.0, 50.0, 5000.0, -6000.0),
        tr(32, 41.0, -88.0, 0.0, 50.0, 4000.0, 0.0),
        tr(33, 41.5, -88.0, 0.0, 50.0, 3000.0, 0.0),
    ];
    cycle(&mut r, S, &rows, &flat(0, 1000));
    let kept: Vec<u32> = msaw_planned(&r.st).iter().map(|t| t.addr).collect();
    assert_eq!(
        kept,
        vec![30, 31],
        "the fast descent and the lowest level one; ties by address"
    );
    // Rows past the track capacity are counted too.
    let rows: Vec<MsawTrack> = (0..5u32)
        .map(|i| {
            tr(
                10 + i,
                40.0 + f64::from(i) * 0.5,
                -88.0,
                0.0,
                50.0,
                8000.0,
                0.0,
            )
        })
        .collect();
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
        obstacle_covered: true,
        mva_ft: 100,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
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
fn the_floor_is_the_higher_of_terrain_and_obstacle_with_clearances_and_the_chart_is_none() {
    let c = cfg();
    let s = |t, o, m| FloorSample {
        terrain_ft: t,
        obstacle_ft: o,
        obstacle_covered: true,
        mva_ft: m,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
    };
    assert_eq!(
        floor_of(&s(1000, FLOOR_NONE, 1200), None, &c),
        Some((1500, FloorKind::Terrain))
    );
    assert_eq!(
        floor_of(&s(1000, 1200, 1200), None, &c),
        Some((1700, FloorKind::Obstacle))
    );
    // The chart altitude is no floor, however high.
    assert_eq!(
        floor_of(&s(1000, 1200, 3000), None, &c),
        Some((1700, FloorKind::Obstacle))
    );
    // A tie goes to the terrain, then the obstacle.
    assert_eq!(
        floor_of(&s(1000, 1000, 1500), None, &c),
        Some((1500, FloorKind::Terrain))
    );
    // No coverage: no floor.
    assert_eq!(floor_of(&s(FLOOR_NONE, 0, 0), None, &c), None);
    assert!(floor_of(&s(0, 0, FLOOR_NONE), None, &c).is_some());
    // No obstacle is not no coverage.
    assert!(floor_of(&s(0, FLOOR_NONE, 0), None, &c).is_some());
}

/// A made-up corridor for a runway end at 40.0 N, 88.0 W landed north: it runs south from the
/// threshold 5 NM (9,260 m), 556 m either side at the threshold widening to 1,852 m; the
/// threshold is 600 ft and the fix is published at 2,250 ft (the 3 degree path at that distance
/// is about 2,245 ft).
fn north_corridor() -> Corridor {
    Corridor {
        kind: CK_APPROACH,
        thr_lat_e7: 400_000_000,
        thr_lon_e7: -880_000_000,
        out_bearing_deg: 180.0,
        length_m: 9260.0,
        near_half_width_m: 556.0,
        far_half_width_m: 1852.0,
        thr_elev_ft: 600,
        faf_alt_ft: 2250,
    }
}

fn corridor_cfg() -> MsawConfig {
    let mut c = cfg();
    c.corridors[0] = north_corridor();
    c.n_corridors = 1;
    c
}

/// Degrees of latitude south of the threshold for `m` metres.
fn south_m(m: f64) -> f64 {
    40.0 - m / 111_194.926_644_558_74
}

/// What a corridor with a surface of `ft` says: the approach clearance of `cfg()`.
fn hit(ft: i32) -> CorridorHit {
    CorridorHit {
        surface_ft: Some(ft),
        clearance_ft: 100,
    }
}

fn surf(ft: i32) -> Option<CorridorHit> {
    Some(hit(ft))
}

// REQ: OA-MSAW-009
#[test]
fn a_corridor_gives_its_floor_only_where_the_whole_query_circle_is_inside() {
    let c = corridor_cfg();
    let k = north_corridor();
    let at = |m: f64, cross_m: f64, r: f64| {
        let lat = (south_m(m) * 1e7).round() as i32;
        let lon = ((-88.0 + cross_m / (111_194.926_644_558_74 * 40.0f64.to_radians().cos())) * 1e7)
            .round() as i32;
        corridor_hit_ft(&k, lat, lon, r, &c).map(|h| h.surface_ft.unwrap())
    };
    // The surface runs from 600 ft at the threshold to 1,950 ft (2,250 less the allowance) at
    // the far end, taken at the far side of the circle, rounded up.
    assert_eq!(at(4630.0, 0.0, 100.0), Some(600 + 1350 * 4730 / 9260 + 1));
    // The whole circle inside: on the centreline at 2 NM with a 500 m circle.
    assert!(at(3704.0, 0.0, 500.0).is_some());
    // Straddling the threshold end, the far end or a side: none.
    assert_eq!(at(400.0, 0.0, 500.0), None);
    assert_eq!(at(9000.0, 0.0, 500.0), None);
    assert_eq!(at(3704.0, 700.0, 500.0), None);
    // Just outside the side (half-width 556 + 0.14 * 1,000 m at 1 km is 696 m): none; just
    // inside with room for the circle: some.
    assert_eq!(at(1000.0, 800.0, 10.0), None);
    assert!(at(1000.0, 600.0, 10.0).is_some());
    // The other side of the line, and the runway side of the threshold, are not in it.
    assert_eq!(at(-2000.0, 0.0, 10.0), None);
    assert_eq!(at(3704.0, -1200.0, 10.0), None);
    assert!(at(3704.0, -800.0, 10.0).is_some());
    // The highest surface of two corridors that hold the circle.
    let mut two = c;
    two.corridors[1] = Corridor {
        faf_alt_ft: 3250,
        ..north_corridor()
    };
    two.n_corridors = 2;
    let p = SamplePoint {
        lat_e7: (south_m(4630.0) * 1e7).round() as i32,
        lon_e7: -880_000_000,
        radius_m: 100.0,
    };
    let one = corridor_hit_linear(&c, &p).unwrap().surface_ft.unwrap();
    let both = corridor_hit_linear(&two, &p).unwrap().surface_ft.unwrap();
    assert!(both > one);
    assert_eq!(
        corridor_hit_linear(&cfg(), &p),
        None,
        "no corridor configured"
    );
    // The floor inside: the higher of the surface and terrain or obstacle plus the approach
    // clearance, and no chart floor; outside: the normal floor with the chart.
    let s = FloorSample {
        terrain_ft: 650,
        obstacle_ft: 1000,
        obstacle_covered: true,
        mva_ft: 3000,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
    };
    assert_eq!(floor_of(&s, None, &c), Some((1500, FloorKind::Obstacle)));
    assert_eq!(
        floor_of(&s, surf(900), &c),
        Some((1100, FloorKind::Obstacle))
    );
    assert_eq!(
        floor_of(&s, surf(1500), &c),
        Some((1500, FloorKind::Procedure))
    );
    let low = FloorSample {
        terrain_ft: 1700,
        obstacle_ft: FLOOR_NONE,
        ..s
    };
    assert_eq!(
        floor_of(&low, surf(1500), &c),
        Some((1800, FloorKind::Terrain))
    );
    // Terrain and the obstacle data are needed in a corridor; the chart is not.
    let nochart = FloorSample {
        mva_ft: FLOOR_NONE,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
        ..s
    };
    assert!(floor_of(&nochart, surf(900), &c).is_some());
    assert!(floor_of(&nochart, None, &c).is_some());
    let noterrain = FloorSample {
        terrain_ft: FLOOR_NONE,
        ..s
    };
    assert_eq!(floor_of(&noterrain, surf(900), &c), None);
    let noobst = FloorSample {
        obstacle_covered: false,
        ..s
    };
    assert_eq!(floor_of(&noobst, surf(900), &c), None);
    assert_eq!(floor_of(&noobst, None, &c), None);
}

/// A flat world at `terrain_ft` with the chart at 100 ft, except a ridge of `ridge_ft` 3 NM
/// south of the threshold (a kilometre wide, whatever the longitude).
fn ridge_world(terrain_ft: i32, ridge_ft: i32) -> impl Fn(i32, i32) -> FloorSample {
    move |lat_e7, _| {
        let m_south = (40.0 - f64::from(lat_e7) / 1e7) * 111_194.926_644_558_74;
        FloorSample {
            terrain_ft: if (m_south - 5556.0).abs() < 600.0 {
                ridge_ft
            } else {
                terrain_ft
            },
            obstacle_ft: FLOOR_NONE,
            obstacle_covered: true,
            mva_ft: 100,
            gtm_clearance_ft: FLOOR_NONE,
            apt_clearance_ft: FLOOR_NONE,
            apt_ceiling_ft: FLOOR_NONE,
        }
    }
}

/// An aircraft heading north at 70 m/s, `m` metres south of the threshold at `c` seconds into
/// the cycles, with the altitude of the 3 degree path (650 ft at the threshold) plus `above` ft.
fn final_track(c: i64, start_m: f64, above: f64, vr: f32) -> MsawTrack {
    let d = start_m - 70.0 * c as f64;
    let alt = 650.0 + 0.1728 * d + above;
    tr(77, south_m(d), -88.0, 0.0, 70.0, alt as f32, vr)
}

// REQ: OA-MSAW-042
#[test]
fn a_normal_three_degree_approach_in_its_corridor_does_not_alert() {
    let mut r = rig(&corridor_cfg());
    for c in 0..=30 {
        let o = cycle(
            &mut r,
            c * S,
            &[final_track(c, 8000.0, 0.0, -722.0)],
            &ridge_world(600, 600),
        );
        assert_eq!(o.stats.violations, 0, "cycle {c}");
        assert_eq!(o.report, NetReport::Running);
    }
}

// REQ: OA-MSAW-042
#[test]
fn guam_synthetic_an_early_descent_in_the_corridor_over_a_ridge_alerts() {
    // The ridge is 700 ft, 3 NM from the threshold; the aircraft has descended early to 650 ft
    // inside the corridor and flies level into it. The surface (about 1,400 ft there) is above
    // it, and so would terrain plus the approach clearance be (800 ft).
    let mut r = rig(&corridor_cfg());
    let a = |c: i64| {
        let d = 9000.0 - 70.0 * c as f64;
        tr(801, south_m(d), -88.0, 0.0, 70.0, 650.0, 0.0)
    };
    for c in 0..=20 {
        cycle(&mut r, c * S, &[a(c)], &ridge_world(600, 700));
    }
    msaw_finish(&mut r.st, &mut r.alerts);
    assert_eq!(r.alerts.items.len(), 1);
    assert_eq!(r.alerts.items[0].worst_kind, FloorKind::Procedure);
    // The same with a surface far below the ridge (the fix published at 0 ft): the terrain
    // clearance alone alerts, in the corridor.
    let mut c = corridor_cfg();
    c.corridors[0].faf_alt_ft = 0;
    c.corridors[0].thr_elev_ft = 0;
    let mut r = rig(&c);
    for k in 0..=40 {
        cycle(&mut r, k * S, &[a(k)], &ridge_world(600, 700));
    }
    msaw_finish(&mut r.st, &mut r.alerts);
    assert_eq!(r.alerts.items.len(), 1);
    assert_eq!(r.alerts.items[0].worst_kind, FloorKind::Terrain);
    assert_eq!(r.alerts.items[0].worst_floor_ft, 800);
    // The same aircraft on the path (50 ft above it) does not alert over a 600 ft ridge.
    let mut r = rig(&corridor_cfg());
    for k in 0..=20 {
        let o = cycle(
            &mut r,
            k * S,
            &[final_track(k, 9000.0, 50.0, -722.0)],
            &ridge_world(600, 600),
        );
        assert_eq!(o.stats.violations, 0, "cycle {k}");
    }
}

// REQ: OA-MSAW-042
#[test]
fn outside_the_corridor_or_straddling_its_edge_the_normal_floor_applies() {
    // A stationary aircraft at 400 ft, 1,200 m south of the threshold: inside the corridor the
    // floor there is the surface (about 250 ft), clear; 1,500 m east of the line it is outside,
    // and the normal floor (terrain 0 plus 500 ft) alerts.
    let mut k = corridor_cfg();
    k.corridors[0].thr_elev_ft = 0;
    k.corridors[0].faf_alt_ft = 1800;
    let world = flat(0, 100);
    let still = |cross_m: f64| {
        let lon = -88.0 + cross_m / (111_194.926_644_558_74 * 40.0f64.to_radians().cos());
        tr(5, south_m(1200.0), lon, 0.0, 0.0, 400.0, 0.0)
    };
    let mut r = rig(&k);
    let o = cycle(&mut r, S, &[still(0.0)], &world);
    assert_eq!(o.stats.violations, 0);
    let mut r = rig(&k);
    let o = cycle(&mut r, S, &[still(1500.0)], &world);
    assert_eq!(o.stats.violations, 1);
    assert_eq!(msaw_open(&r.st)[0].worst_kind, FloorKind::Terrain);
    // 500 m east: the centre is inside, but the circle (50 m growing 5 m per second) leaves the
    // side after 34 s, and from there the normal floor applies: beyond the 15 s terminal
    // look-ahead, so only a corridor look-ahead of 60 s sees it.
    let mut r = rig(&k);
    let o = cycle(&mut r, S, &[still(500.0)], &world);
    assert_eq!(o.stats.violations, 0);
    k.terminal_lookahead_s = 60.0;
    let mut r = rig(&k);
    let o = cycle(&mut r, S, &[still(500.0)], &world);
    assert_eq!(o.stats.violations, 1);
    assert_eq!(msaw_open(&r.st)[0].worst_floor_ft, 500);
    assert_eq!(msaw_open(&r.st)[0].worst_kind, FloorKind::Terrain);
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

// REQ: OA-MSAW-015, OA-MSAW-046
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
    // Radius: 50 m, plus 5 m/s of velocity uncertainty times the prediction time, plus half the
    // 5 s step at 100 m/s (250 m).
    assert!((f64::from(out[0].radius_m) - 300.0).abs() < 0.01);
    assert!((f64::from(out[3].radius_m) - 375.0).abs() < 0.01);
    assert!((f64::from(out[12].radius_m) - 600.0).abs() < 0.01);
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
                        obstacle_covered: true,
                        mva_ft: mva,
                        gtm_clearance_ft: FLOOR_NONE,
                        apt_clearance_ft: FLOOR_NONE,
                        apt_ceiling_ft: FLOOR_NONE,
                    };
                    let b = FloorSample {
                        terrain_ft: terrain + bump,
                        obstacle_ft: if obstacle == FLOOR_NONE {
                            FLOOR_NONE
                        } else {
                            obstacle + bump
                        },
                        obstacle_covered: true,
                        mva_ft: mva + bump,
                        gtm_clearance_ft: FLOOR_NONE,
                        apt_clearance_ft: FLOOR_NONE,
                        apt_ceiling_ft: FLOOR_NONE,
                    };
                    for surface in [None, Some(0), Some(1500), Some(9000)] {
                        let (fa, _) = floor_of(&a, surface.map(hit), &c).unwrap();
                        let (fb, _) = floor_of(&b, surface.map(hit), &c).unwrap();
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
}

// REQ: OA-MSAW-044
#[test]
fn no_obstacle_data_is_no_coverage_and_the_net_reads_degraded() {
    let mut r = rig(&cfg());
    let a = tr(1, 40.0, -88.0, 0.0, 100.0, 100.0, 0.0);
    let no_dof = |_: i32, _: i32| FloorSample {
        terrain_ft: 0,
        obstacle_ft: FLOOR_NONE,
        obstacle_covered: false,
        mva_ft: 2000,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
    };
    let o = cycle(&mut r, S, &[a], &no_dof);
    assert_eq!(o.stats.no_coverage, 1);
    assert_eq!(
        o.stats.violations, 0,
        "nothing is judged against a floor that is not known"
    );
    assert_eq!(r.skips.items[0].why, SkipWhy::NoCoverage);
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 1,
            ..
        }
    ));
}

// REQ: OA-MSAW-045
#[test]
fn a_track_flagged_with_no_altimeter_setting_is_counted_listed_and_evaluated() {
    let mut r = rig(&cfg());
    let mut a = tr(1, 40.0, -88.0, 0.0, 100.0, 100.0, 0.0);
    a.flags |= F_QNH_MISSING;
    let o = cycle(&mut r, S, &[a], &flat(0, 2000));
    assert_eq!(o.stats.qnh_missing, 1);
    assert_eq!(o.stats.evaluated, 1);
    assert_eq!(
        o.stats.violations, 1,
        "evaluated, and below the chart floor"
    );
    assert!(r
        .skips
        .items
        .iter()
        .any(|s| s.why == SkipWhy::QnhMissing && s.addr == 1));
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 1,
            ..
        }
    ));
    assert_ne!(msaw_open(&r.st)[0].flags & AF_QNH_MISSING, 0);
    // A track with a setting is not counted.
    let mut r = rig(&cfg());
    let b = tr(2, 40.0, -88.0, 0.0, 100.0, 5000.0, 0.0);
    let o = cycle(&mut r, S, &[b], &flat(0, 2000));
    assert_eq!(o.stats.qnh_missing, 0);
    assert_eq!(o.report, NetReport::Running);
}

/// A made-up departure corridor from the departure end at 40.0 N, 88.0 W heading north: 5 NM,
/// 556 m either side at the end widening to 1,852 m.
fn departure_corridor() -> Corridor {
    Corridor {
        kind: CK_DEPARTURE,
        thr_lat_e7: 400_000_000,
        thr_lon_e7: -880_000_000,
        out_bearing_deg: 0.0,
        length_m: 9260.0,
        near_half_width_m: 556.0,
        far_half_width_m: 1852.0,
        thr_elev_ft: 600,
        faf_alt_ft: 0,
    }
}

// REQ: OA-MSAW-053
#[test]
fn a_departure_climbing_normally_does_not_alert_and_one_levelling_off_below_its_floor_does() {
    let mut c = cfg();
    c.corridors[0] = departure_corridor();
    c.n_corridors = 1;
    c.departure_min_clearance_ft = 100;
    // Terrain 1,000 ft everywhere, the chart at 6,000 ft: inside the corridor only terrain plus
    // the departure clearance (1,100 ft) is the floor, the chart does not apply.
    let world = flat(1000, 6000);
    let dep = |k: i64, alt: f32, vr: f32| {
        let along = 600.0 + 70.0 * k as f64;
        tr(
            9,
            40.0 + along / 111_194.926_644_558_74,
            -88.0,
            0.0,
            70.0,
            alt,
            vr,
        )
    };
    let mut r = rig(&c);
    for k in 0..=30 {
        let o = cycle(
            &mut r,
            k * S,
            &[dep(k, 1200.0 + 25.0 * k as f32, 1500.0)],
            &world,
        );
        assert_eq!(o.stats.violations, 0, "cycle {k}");
    }
    // Levelling off at 1,050 ft, 50 ft below terrain plus the clearance.
    let mut r = rig(&c);
    for k in 0..=30 {
        cycle(&mut r, k * S, &[dep(k, 1050.0, 0.0)], &world);
    }
    msaw_finish(&mut r.st, &mut r.alerts);
    assert_eq!(r.alerts.items.len(), 1);
    assert_eq!(r.alerts.items[0].worst_kind, FloorKind::Terrain);
    assert_eq!(r.alerts.items[0].worst_floor_ft, 1100);
    // The same flight outside the corridor (3 km east of the line) is held to terrain plus the
    // general clearance (500 ft), not the corridor's 100 ft.
    let mut r = rig(&c);
    for k in 0..=30 {
        let mut t = dep(k, 1200.0 + 25.0 * k as f32, 1500.0);
        t.lon_deg = -88.0 + 3000.0 / (111_194.926_644_558_74 * 40.0f64.to_radians().cos());
        cycle(&mut r, k * S, &[t], &world);
    }
    msaw_finish(&mut r.st, &mut r.alerts);
    assert_eq!(r.alerts.items.len(), 1);
    assert_eq!(r.alerts.items[0].worst_kind, FloorKind::Terrain);
    assert_eq!(r.alerts.items[0].worst_floor_ft, 1500);
}

// REQ: OA-MSAW-053
#[test]
fn a_runway_corridor_holds_the_threshold_elevation_and_the_approach_clearance_and_no_chart() {
    let mut c = cfg();
    let mut k = departure_corridor();
    k.kind = CK_RUNWAY;
    k.length_m = 2500.0;
    c.corridors[0] = k;
    c.n_corridors = 1;
    let at = |along_m: f64| {
        let p = SamplePoint {
            lat_e7: (((40.0 + along_m / 111_194.926_644_558_74) * 1e7).round()) as i32,
            lon_e7: -880_000_000,
            radius_m: 100.0,
        };
        corridor_hit_linear(&c, &p)
    };
    assert_eq!(
        at(1000.0),
        Some(CorridorHit {
            surface_ft: Some(600),
            clearance_ft: 100
        })
    );
    assert_eq!(at(2450.0), None, "the circle reaches past the far end");
    assert_eq!(at(50.0), None, "the circle reaches past the near end");
}

// REQ: OA-MSAW-054
#[test]
fn the_corridor_index_gives_the_answer_of_looking_at_every_corridor() {
    let mut state = 12345u64;
    let mut rnd = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 11) as f64) / ((1u64 << 53) as f64)
    };
    let mut c = cfg();
    for k in 0..400 {
        c.corridors[k] = Corridor {
            kind: [CK_APPROACH, CK_RUNWAY, CK_DEPARTURE][k % 3],
            thr_lat_e7: 400_000_000 + (rnd() * 1.0e7) as i32 - 5_000_000,
            thr_lon_e7: -880_000_000 + (rnd() * 1.0e7) as i32 - 5_000_000,
            out_bearing_deg: rnd() * 360.0,
            length_m: 1000.0 + rnd() * 20_000.0,
            near_half_width_m: 100.0 + rnd() * 600.0,
            far_half_width_m: 100.0 + rnd() * 2000.0,
            thr_elev_ft: (rnd() * 3000.0) as i32,
            faf_alt_ft: (rnd() * 6000.0) as i32,
        };
    }
    c.n_corridors = 400;
    let ix = corridor_index_new(&c).unwrap();
    assert!(ix.len() >= 400);
    let (mut inside, mut outside) = (0u32, 0u32);
    for _ in 0..20_000 {
        let p = SamplePoint {
            lat_e7: 400_000_000 + (rnd() * 1.0e7) as i32 - 5_000_000,
            lon_e7: -880_000_000 + (rnd() * 1.0e7) as i32 - 5_000_000,
            radius_m: (rnd() * 700.0) as f32,
        };
        let (fast, slow) = (corridor_hit_at(&c, &ix, &p), corridor_hit_linear(&c, &p));
        assert_eq!(fast, slow, "{p:?}");
        if slow.is_some() {
            inside += 1;
        } else {
            outside += 1;
        }
    }
    assert!(
        inside > 500 && outside > 500,
        "{inside} inside, {outside} outside"
    );
}

// REQ: OA-MSAW-057
#[test]
fn below_the_chart_but_above_the_alert_floor_is_listed_and_counted_and_never_an_alert() {
    // An aircraft level at 2,600 ft over 1,100 ft terrain (1,500 ft above it) with the chart at
    // 3,000 ft: 400 ft below the MVA, far above terrain plus 500 ft.
    let mut r = rig(&cfg());
    let a = tr(7, 40.0, -88.0, 0.0, 80.0, 2600.0, 0.0);
    let o = cycle(&mut r, S, &[a], &flat(1100, 3000));
    assert_eq!(o.stats.violations, 0);
    assert_eq!(o.stats.below_mva, 1);
    assert_eq!(r.skips.items.len(), 1);
    assert_eq!(r.skips.items[0].why, SkipWhy::BelowMva);
    assert_eq!(o.report, NetReport::Running, "not a shortfall of the net");
    assert_eq!(msaw_open_len(&r.st), 0);
    // Above the chart: nothing. Below terrain plus the clearance: an alert, and no below-MVA row.
    let mut r = rig(&cfg());
    let o = cycle(
        &mut r,
        S,
        &[tr(7, 40.0, -88.0, 0.0, 80.0, 3100.0, 0.0)],
        &flat(1100, 3000),
    );
    assert_eq!((o.stats.below_mva, o.stats.violations), (0, 0));
    let o = cycle(
        &mut r,
        2 * S,
        &[tr(7, 40.0, -88.0, 0.0, 80.0, 1500.0, 0.0)],
        &flat(1100, 3000),
    );
    assert_eq!((o.stats.below_mva, o.stats.violations), (0, 1));
    // No chart at all: no row and the net still runs.
    let mut r = rig(&cfg());
    let o = cycle(
        &mut r,
        S,
        &[tr(7, 40.0, -88.0, 0.0, 80.0, 2600.0, 0.0)],
        &flat(1100, FLOOR_NONE),
    );
    assert_eq!((o.stats.below_mva, o.stats.no_coverage), (0, 0));
    assert_eq!(o.report, NetReport::Running);
}

// REQ: OA-MSAW-058
#[test]
fn an_area_clearance_replaces_the_general_one_outside_the_corridors() {
    let c = cfg();
    let s = FloorSample {
        terrain_ft: 1000,
        obstacle_ft: FLOOR_NONE,
        obstacle_covered: true,
        mva_ft: FLOOR_NONE,
        gtm_clearance_ft: 200,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
    };
    assert_eq!(floor_of(&s, None, &c), Some((1200, FloorKind::Terrain)));
    let general = FloorSample {
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
        ..s
    };
    assert_eq!(
        floor_of(&general, None, &c),
        Some((1500, FloorKind::Terrain))
    );
    // In a corridor the corridor's clearance holds, not the area's.
    assert_eq!(floor_of(&s, surf(0), &c), Some((1100, FloorKind::Terrain)));
}

/// The capture box of the A2.08 standard: 1 NM either side of the centreline all along.
fn box_cfg() -> MsawConfig {
    let mut c = corridor_cfg();
    c.corridors[0].near_half_width_m = 1852.0;
    c.corridors[0].far_half_width_m = 1852.0;
    c
}

/// Metres east of the centreline in degrees of longitude at 40 N.
fn east_deg(m: f64) -> f64 {
    -88.0 + m / (111_194.926_644_558_74 * 40.0f64.to_radians().cos())
}

// REQ: OA-MSAW-060
#[test]
fn an_approach_inside_the_capture_box_is_judged_by_the_surface_and_outside_it_by_the_general_floor()
{
    let world = ridge_world(600, 600);
    // 400 ft below the path, 8,000 m out: under the surface, above terrain plus 500 ft.
    let mut t = final_track(0, 8000.0, -400.0, 0.0);
    t.lon_deg = east_deg(0.8 * 1852.0);
    let mut r = rig(&box_cfg());
    let o = cycle(&mut r, S, &[t], &world);
    assert_eq!(o.stats.violations, 1);
    assert_eq!(msaw_open(&r.st)[0].worst_kind, FloorKind::Procedure);
    // 1.2 NM off the centreline is outside the box: the general rules, no alert.
    t.lon_deg = east_deg(1.2 * 1852.0);
    let mut r = rig(&box_cfg());
    let o = cycle(&mut r, S, &[t], &world);
    assert_eq!(o.stats.violations, 0);
}

fn airport_world(terrain: i32, tower_top: i32) -> impl Fn(i32, i32) -> FloorSample {
    move |_, _| FloorSample {
        terrain_ft: terrain,
        obstacle_ft: tower_top,
        obstacle_covered: true,
        mva_ft: FLOOR_NONE,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: 200,
        apt_ceiling_ft: terrain + 3000,
    }
}

// REQ: OA-MSAW-059
#[test]
fn a_visual_base_leg_in_an_airport_area_alerts_only_for_a_tower_and_the_floor_is_never_an_inhibit()
{
    // 900 ft AGL over flat terrain, descending 500 ft/min: 1,000 ft in a minute, above terrain
    // plus 200 ft (800) and below terrain plus the general 500 ft (1,100).
    let base = tr(5, 40.0, -88.0, 70.0, 0.0, 1500.0, -500.0);
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[base], &airport_world(600, FLOOR_NONE));
    assert_eq!(o.stats.violations, 0);
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[base], &flat(600, FLOOR_NONE));
    assert_eq!(o.stats.violations, 1, "the general floor would alert");
    // 150 ft above a 1,000 ft tower (top 1,600 ft) within the radius: the obstacle keeps its
    // 500 ft clearance and the track alerts.
    let level = tr(5, 40.0, -88.0, 70.0, 0.0, 1750.0, 0.0);
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[level], &airport_world(600, 1600));
    assert_eq!(o.stats.violations, 1);
    assert_eq!(msaw_open(&r.st)[0].worst_kind, FloorKind::Obstacle);
    // Above the area's ceiling the general clearance holds again.
    let high = tr(5, 40.0, -88.0, 70.0, 0.0, 4000.0, -3500.0);
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[high], &airport_world(600, FLOOR_NONE));
    assert_eq!(o.stats.violations, 1);
    assert_eq!(msaw_open(&r.st)[0].worst_floor_ft, 800);
}

// REQ: OA-MSAW-064
#[test]
fn inside_an_airport_area_the_prediction_is_15_s_and_outside_it_is_the_general_look_ahead() {
    // 2,000 ft, 1,200 ft above the area floor (terrain 600 + 200), descending 1,500 ft/min: 1,625 ft
    // in 15 s, 500 ft in 60 s.
    let arrival = tr(9, 40.0, -88.0, 70.0, 0.0, 2000.0, -1500.0);
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[arrival], &airport_world(600, FLOOR_NONE));
    assert_eq!(o.stats.violations, 0);
    // The same geometry outside any terminal area: the general floor (1,100 ft) and look-ahead.
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[arrival], &flat(600, FLOOR_NONE));
    assert_eq!(o.stats.violations, 1);
    // Below the area's floor now: an alert at once, with a time to violation of zero.
    let low = tr(9, 40.0, -88.0, 70.0, 0.0, 700.0, 0.0);
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[low], &airport_world(600, FLOOR_NONE));
    assert_eq!(o.stats.violations, 1);
    assert_eq!(msaw_open(&r.st)[0].ttv_min_s, 0.0);
    // Above the area's ceiling the general look-ahead holds again.
    let high = tr(9, 40.0, -88.0, 70.0, 0.0, 4600.0, -4000.0);
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[high], &airport_world(600, FLOOR_NONE));
    assert_eq!(o.stats.violations, 1);
}

// REQ: OA-MSAW-064
#[test]
fn the_guam_descent_in_a_terminal_area_alerts_later_by_the_look_ahead_difference() {
    let first = |terminal: bool| {
        let mut r = rig(&cfg());
        for c in 0..60 {
            let world = |la: i32, lo: i32| {
                let mut s = ridge(la, lo);
                if terminal {
                    s.apt_clearance_ft = 500;
                    s.apt_ceiling_ft = 100_000;
                }
                s
            };
            cycle(&mut r, c * S, &[approach(c)], &world);
        }
        msaw_finish(&mut r.st, &mut r.alerts);
        assert_eq!(r.alerts.items.len(), 1, "the alert is not lost");
        r.alerts.items[0].start_ns
    };
    let (general, terminal) = (first(false), first(true));
    assert_eq!(general, 0);
    // The violation is 38 s ahead at the first cycle: seen from 15 s ahead it is 23 s later, 24 s
    // on the 5 s sample grid.
    assert_eq!(terminal, 24 * S, "{terminal}");
}

// REQ: OA-MSAW-065
#[test]
fn an_altitude_older_than_the_limit_is_not_monitored_is_counted_listed_and_degrades_the_net() {
    let mut old = tr(9, 40.0, -88.0, 70.0, 0.0, 700.0, 0.0);
    old.alt_age_s = 20.0;
    let mut r = rig(&cfg());
    let o = cycle(&mut r, S, &[old], &airport_world(600, FLOOR_NONE));
    assert_eq!(
        (o.stats.alt_stale, o.stats.evaluated, o.stats.violations),
        (1, 0, 0)
    );
    assert_eq!(r.skips.items[0].why, SkipWhy::AltitudeStale);
    assert!(matches!(
        o.report,
        NetReport::Degraded {
            reason: Reason::AltitudeMissing,
            count: 1,
            ..
        }
    ));
    // 11 s old is judged; an unknown age is taken as fresh.
    old.alt_age_s = 11.0;
    let o = cycle(&mut r, 2 * S, &[old], &airport_world(600, FLOOR_NONE));
    assert_eq!((o.stats.alt_stale, o.stats.violations), (0, 1));
    old.alt_age_s = f32::NAN;
    let o = cycle(&mut r, 3 * S, &[old], &airport_world(600, FLOOR_NONE));
    assert_eq!(o.stats.alt_stale, 0);
}

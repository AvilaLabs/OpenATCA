//! Small offline qualification slice. All rows, limits and truth intervals are synthetic.
//! Expected intervals and alert desire are declared before the core is run; these are not
//! recorded-traffic distributions or evidence of operational readiness.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_ca::*;
use oa_geo::aeqd::{frame_new, unproject};
use oa_safety::inhibit::inhibits_new;
use oa_safety::net::{net_status, state_table_new, NetId, NetReport, NetState, Reason};

const SECOND: i64 = 1_000_000_000;
const BASE: (f64, f64) = (41.5, -88.0);
const H_LIMIT_M: f64 = 5_000.0;
const V_LIMIT_FT: f64 = 300.0;

#[derive(Clone, Copy)]
struct SyntheticTruth {
    name: &'static str,
    /// Open interval of the independently specified, actual volume incursion, seconds.
    expected_interval_s: Option<(f64, f64)>,
    /// Whether the declared truth calls for an alert.
    alert_desired: bool,
}

// Declared before producing any alert output. The hit's relative position is -40,000 + 400t m,
// so |x| < 5,000 m gives (87.5, 112.5) s; its vertical separation is 0 ft. The nuisance pair
// follows the same horizontal path at a constant 1,000 ft vertical separation, outside the
// synthetic vertical limit throughout. These are analytic synthetic references, not a baseline
// claimed to represent any deployed system.
const TRUTH: [SyntheticTruth; 2] = [
    SyntheticTruth {
        name: "head_on_loss",
        expected_interval_s: Some((87.5, 112.5)),
        alert_desired: true,
    },
    SyntheticTruth {
        name: "vertically_separated_pass",
        expected_interval_s: None,
        alert_desired: false,
    },
];

fn volume() -> Volume {
    Volume {
        area: Area::Box {
            lat_min_e7: -1,
            lat_max_e7: 1,
            lon_min_e7: -1,
            lon_max_e7: 1,
        },
        alt_lo_ft: -100_000,
        alt_hi_ft: 100_000,
        h_m: H_LIMIT_M,
        v_ft: V_LIMIT_FT,
    }
}

fn config() -> CaConfig {
    CaConfig {
        max_tracks: 16,
        max_open: 16,
        max_candidates: 64,
        look_ahead_ns: 120 * SECOND,
        hysteresis_ns: 5 * SECOND,
        work_budget: 100_000,
        eval_budget: 64,
        max_speed_mps: 700.0,
        r95_cap_m: 20_000.0,
        grid_band_m: 10_000.0,
        default_volume: volume(),
        volumes: [volume(); MAX_VOLUMES],
        n_volumes: 0,
    }
}

fn track(addr: u32, east_m: f64, east_mps: f32, altitude_ft: f32) -> CaTrack {
    track_xy(addr, east_m, 0.0, east_mps, 0.0, altitude_ft)
}

fn track_xy(
    addr: u32,
    east_m: f64,
    north_m: f64,
    east_mps: f32,
    north_mps: f32,
    altitude_ft: f32,
) -> CaTrack {
    let (lat_deg, lon_deg) = unproject(&frame_new(BASE.0, BASE.1), east_m, north_m).unwrap();
    CaTrack {
        lat_deg,
        lon_deg,
        addr,
        vel_east_mps: east_mps,
        vel_north_mps: north_mps,
        r95_m: 0.0,
        alt_ft: altitude_ft,
        vrate_fpm: 0.0,
        flags: 0,
        ground: AIRBORNE,
    }
}

fn pair(altitude_b_ft: f32) -> [CaTrack; 2] {
    [
        track(1, -20_000.0, 200.0, 10_000.0),
        track(2, 20_000.0, -200.0, altitude_b_ft),
    ]
}

fn rig() -> (CaState, AlertLog, EventLog, oa_safety::net::StateTable) {
    (
        ca_new(&config()).unwrap(),
        alert_log_new(16).unwrap(),
        event_log_new(16).unwrap(),
        state_table_new(600 * SECOND, SECOND).unwrap(),
    )
}

fn alerted(rows: &[CaTrack]) -> bool {
    let (mut state, mut alerts, mut events, mut net) = rig();
    ca_cycle(&mut state, 0, rows, &mut alerts, &mut events, &mut net);
    ca_open_len(&state) > 0
}

// REQ: OA-CA-004
#[test]
fn finite_over_cap_radius_can_qualify_uncapped_but_is_absent_from_capped_candidates() {
    // Declare both geometries before asking CA for output. The equal-altitude stationary pair is
    // inside the horizontal limit if its supplied 25 km radii are retained. The spatial grid uses
    // radii held to the configured 20 km cap. A conservative 1.10 bracket exceeds the current
    // grid's 1.02 cheap margin, so this fixture remains separated from the capped candidate reach.
    let cfg = config();
    let uncapped_radius_m = 25_000.0_f32;
    let expected_distance_m = 50_000.0;
    let uncapped_conflict_limit_m = cfg.default_volume.h_m + 2.0 * f64::from(uncapped_radius_m);
    let capped_search_upper_m = (cfg.default_volume.h_m + 2.0 * cfg.r95_cap_m) * 1.10;
    let mut rows = [
        track_xy(501, -expected_distance_m / 2.0, 0.0, 0.0, 0.0, 10_000.0),
        track_xy(502, expected_distance_m / 2.0, 0.0, 0.0, 0.0, 10_000.0),
    ];
    rows[0].r95_m = uncapped_radius_m;
    rows[1].r95_m = uncapped_radius_m;
    let ellipsoid = oa_geo::wgs84();
    let distance_m = ellipsoid
        .inverse(
            oa_geo::LatLon::new(rows[0].lat_deg, rows[0].lon_deg).unwrap(),
            oa_geo::LatLon::new(rows[1].lat_deg, rows[1].lon_deg).unwrap(),
        )
        .unwrap()
        .distance_m;
    assert!(
        distance_m > capped_search_upper_m,
        "{distance_m} > {capped_search_upper_m}"
    );
    assert!(
        distance_m < uncapped_conflict_limit_m,
        "{distance_m} < {uncapped_conflict_limit_m}"
    );

    let mut clipped = rig();
    let out = ca_cycle(
        &mut clipped.0,
        0,
        &rows,
        &mut clipped.1,
        &mut clipped.2,
        &mut clipped.3,
    );
    assert_eq!(out.stats.r95_clamped, 2);
    assert_eq!(out.stats.tracks, 2);
    assert_eq!(out.stats.candidates, 0);
    assert_eq!(out.stats.evaluated, 0);
    assert_eq!(out.stats.not_evaluated, 0);
    assert_eq!(out.stats.tracks_unscanned, 0);
    assert_eq!((out.work_short, out.input_short), (0, 2));
    assert!(matches!(
        out.report,
        NetReport::Degraded {
            reason: Reason::RadiusCapped,
            count: 2,
            ..
        }
    ));
    assert_eq!(ca_open_len(&clipped.0), 0);

    // The in-cap head-on control uses the measured geodesic separation in its timing oracle.
    // It closes at 400 m/s and enters the synthetic 5 km horizontal limit near 87.5 s.
    let control_rows = [
        track_xy(601, -20_000.0, 0.0, 200.0, 0.0, 10_000.0),
        track_xy(602, 20_000.0, 0.0, -200.0, 0.0, 10_000.0),
    ];
    let control_distance = ellipsoid
        .inverse(
            oa_geo::LatLon::new(control_rows[0].lat_deg, control_rows[0].lon_deg).unwrap(),
            oa_geo::LatLon::new(control_rows[1].lat_deg, control_rows[1].lon_deg).unwrap(),
        )
        .unwrap()
        .distance_m;
    let control_entry_s = (control_distance - cfg.default_volume.h_m) / 400.0;
    let mut control = rig();
    let out = ca_cycle(
        &mut control.0,
        0,
        &control_rows,
        &mut control.1,
        &mut control.2,
        &mut control.3,
    );
    assert_eq!(out.stats.r95_clamped, 0);
    assert_eq!(out.stats.candidates, 1);
    assert_eq!(out.stats.evaluated, 1);
    assert_eq!((out.work_short, out.input_short), (0, 0));
    assert_eq!(out.report, NetReport::Running);
    let warning = ca_open(&control.0)
        .first()
        .expect("healthy in-cap control is alerted");
    assert!(
        (f64::from(warning.ttl_first_s) - control_entry_s).abs() < 0.5,
        "CA entry {} s, geodesic oracle {control_entry_s} s",
        warning.ttl_first_s
    );
}

// REQ: OA-CA-002, OA-CA-003, OA-CA-012
#[test]
fn synthetic_truth_qualification_counts_misses_nuisance_and_lead_time() {
    // Output is evaluated against the declarations above, with rates defined per declared case:
    // miss rate = missed desired-alert cases / desired-alert cases; nuisance rate = alerts on
    // no-alert cases / no-alert cases. Lead time = true interval entry minus first alert time.
    let hit_rows = pair(10_000.0);
    let nuisance_rows = pair(11_000.0);
    let hit = alerted(&hit_rows);
    let nuisance = alerted(&nuisance_rows);
    let misses = usize::from(TRUTH[0].alert_desired && !hit);
    let desired_denominator = TRUTH.iter().filter(|truth| truth.alert_desired).count();
    let nuisance_alerts = usize::from(!TRUTH[1].alert_desired && nuisance);
    let nuisance_denominator = TRUTH.iter().filter(|truth| !truth.alert_desired).count();
    assert_eq!((misses, desired_denominator), (0, 1));
    assert_eq!((nuisance_alerts, nuisance_denominator), (0, 1));

    let (mut state, mut alerts, mut events, mut net) = rig();
    let out = ca_cycle(&mut state, 0, &hit_rows, &mut alerts, &mut events, &mut net);
    assert_eq!(out.report, NetReport::Running);
    assert_eq!(ca_open_len(&state), 1);
    let (entry_s, exit_s) = TRUTH[0].expected_interval_s.unwrap();
    let first = ca_open(&state)[0];
    let alert_time_s = first.start_ns as f64 / SECOND as f64;
    let lead_time_s = entry_s - alert_time_s;
    assert!((lead_time_s - 87.5).abs() < 1e-6);
    assert!(
        (f64::from(first.ttl_first_s) - lead_time_s).abs() < 0.5,
        "local projection predicted {} s against {lead_time_s} s analytic lead time",
        first.ttl_first_s
    );
    assert!(exit_s > entry_s);
    assert_eq!(TRUTH[0].name, "head_on_loss");
    assert_eq!(TRUTH[1].name, "vertically_separated_pass");
}

// REQ: OA-CA-006, OA-CA-011, OA-CA-012
#[test]
fn missing_input_and_unknown_altitude_degrade_without_clearing_an_open_warning() {
    let (mut state, mut alerts, mut events, mut net) = rig();
    let known = pair(10_000.0);
    ca_cycle(&mut state, 0, &known, &mut alerts, &mut events, &mut net);
    assert_eq!(ca_open_len(&state), 1);

    let mut unknown = known;
    unknown[0].alt_ft = f32::NAN;
    unknown[0].vrate_fpm = f32::NAN;
    ca_note_lost_input(&mut state, 2);
    let out = ca_cycle(
        &mut state,
        SECOND,
        &unknown,
        &mut alerts,
        &mut events,
        &mut net,
    );
    assert_eq!(out.stats.alt_unknown_pairs, 1);
    assert!(matches!(
        out.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 2,
            ..
        }
    ));
    assert_eq!(ca_open_len(&state), 1, "the known warning survives the gap");
    let active = ca_open(&state)[0];
    assert_ne!(active.flags & AF_ALT_UNKNOWN, 0);
    assert_eq!(
        active.ticks_unevaluated, 0,
        "this pair was still evaluated horizontally"
    );
    let status = net_status(&net, &inhibits_new(), NetId::ConflictAlert, SECOND);
    assert!(matches!(status.state, NetState::Degraded { count: 2, .. }));
}

#[test]
fn synthetic_per_second_cases_score_crossings_and_safe_passes() {
    // Truth is declared analytically from the generated trajectories before the CA cycles run.
    // Each case gets its own state so pairs cannot interact. The open interval is |x| < 5 km.
    let cases = [
        (
            "fast_head_on",
            20_000.0,
            200.0,
            10_000.0,
            Some((87.5, 112.5)),
        ),
        (
            "slow_head_on",
            15_000.0,
            150.0,
            10_000.0,
            Some((83.333_333, 116.666_667)),
        ),
        ("separated_pass", 20_000.0, 200.0, 11_000.0, None),
    ];
    let mut misses = 0;
    let mut desired_cases = 0;
    let mut nuisance_alerts = 0;
    let mut no_alert_cases = 0;
    let mut leads = Vec::new();

    for (case_ix, (name, half_gap, speed, altitude_b, interval)) in cases.into_iter().enumerate() {
        let mut rows = [
            track(100 + case_ix as u32 * 2, -half_gap, speed as f32, 10_000.0),
            track(
                101 + case_ix as u32 * 2,
                half_gap,
                -(speed as f32),
                altitude_b,
            ),
        ];
        let (mut state, mut alerts, mut events, mut net) = rig();
        let mut first_alert_s = None;
        let mut observed_alert = false;
        for tick in 0..=120 {
            let travel = f64::from(tick) * speed;
            rows[0] = track(rows[0].addr, -half_gap + travel, speed as f32, 10_000.0);
            rows[1] = track(rows[1].addr, half_gap - travel, -(speed as f32), altitude_b);
            let out = ca_cycle(
                &mut state,
                i64::from(tick) * SECOND,
                &rows,
                &mut alerts,
                &mut events,
                &mut net,
            );
            assert_eq!(out.stats.out_of_order, 0, "{name} tick {tick}");
            let open = ca_open(&state)
                .iter()
                .any(|alert| alert.addr_a == rows[0].addr && alert.addr_b == rows[1].addr);
            if open {
                observed_alert = true;
                first_alert_s.get_or_insert(tick as f64);
            }
        }

        if let Some((entry_s, exit_s)) = interval {
            desired_cases += 1;
            misses += usize::from(!observed_alert || first_alert_s.is_none());
            if let Some(alert_s) = first_alert_s {
                leads.push(entry_s - alert_s);
                assert!(
                    alert_s < entry_s,
                    "{name} warning must precede the loss interval"
                );
            }
            assert!(exit_s > entry_s);
        } else {
            no_alert_cases += 1;
            nuisance_alerts += usize::from(observed_alert);
        }
    }

    assert_eq!((misses, desired_cases), (0, 2));
    assert_eq!((nuisance_alerts, no_alert_cases), (0, 1));
    assert_eq!(leads.len(), 2);
    assert!((leads[0] - 87.5).abs() < 1e-6);
    assert!((leads[1] - 83.333_333).abs() < 1e-6);
}

#[derive(Clone, Copy)]
struct GridCase {
    name: &'static str,
    rel_start_m: (f64, f64),
    rel_velocity_mps: (f64, f64),
    vertical_separation_ft: f32,
    /// Continuous interval of true entry into the declared 5 km / 300 ft volume.
    /// Computed from the declared straight trajectories, before running any CA cycle.
    truth_interval_s: Option<(f64, f64)>,
    degraded_at_s: Option<i64>,
}

// Independent synthetic truth: for horizontal relative position p and velocity v, the closest
// time is -p·v/|v|² and the 5 km interval is that time ± sqrt((5000²-d_min²)/|v|²).
// Vertical separation is constant. The lateral control has a 1.5 km margin outside the limit;
// the vertical control has a 500 ft margin outside its limit. These are curated geometry cases,
// not recorded-traffic samples or an operational-performance claim.
const GRID: [GridCase; 5] = [
    GridCase {
        name: "oblique_crossing_200mps",
        rel_start_m: (-20_000.0, -10_000.0),
        rel_velocity_mps: (200.0, 100.0),
        vertical_separation_ft: 0.0,
        truth_interval_s: Some((77.639_320_225, 122.360_679_775)),
        degraded_at_s: None,
    },
    GridCase {
        name: "diagonal_crossing_282mps",
        rel_start_m: (-20_000.0, -20_000.0),
        rel_velocity_mps: (200.0, 200.0),
        vertical_separation_ft: 0.0,
        truth_interval_s: Some((82.322_330_47, 117.677_669_53)),
        degraded_at_s: None,
    },
    GridCase {
        name: "same_track_overtake_50mps",
        rel_start_m: (-8_000.0, 0.0),
        rel_velocity_mps: (50.0, 0.0),
        vertical_separation_ft: 0.0,
        truth_interval_s: Some((60.0, 260.0)),
        degraded_at_s: Some(80),
    },
    GridCase {
        name: "lateral_clearance_1500m",
        rel_start_m: (-20_000.0, 6_500.0),
        rel_velocity_mps: (200.0, 0.0),
        vertical_separation_ft: 0.0,
        truth_interval_s: None,
        degraded_at_s: None,
    },
    GridCase {
        name: "vertical_clearance_500ft",
        rel_start_m: (-20_000.0, 0.0),
        rel_velocity_mps: (200.0, 0.0),
        vertical_separation_ft: 800.0,
        truth_interval_s: None,
        degraded_at_s: None,
    },
];

#[test]
fn synthetic_maneuver_grid_scores_sampled_alerts_and_coverage_separately() {
    let mut desired_denominator = 0;
    let mut misses = 0;
    let mut early = 0;
    let mut late = 0;
    let mut no_alert_denominator = 0;
    let mut nuisance_cases = 0;
    let mut lead_seconds = Vec::new();
    let mut degraded_cycles = 0;
    let mut expected_cycles = 0;

    for (case_ix, case) in GRID.into_iter().enumerate() {
        let wants_alert = case.truth_interval_s.is_some();
        let velocity_sq = case.rel_velocity_mps.0.powi(2) + case.rel_velocity_mps.1.powi(2);
        let closest_s = -(case.rel_start_m.0 * case.rel_velocity_mps.0
            + case.rel_start_m.1 * case.rel_velocity_mps.1)
            / velocity_sq;
        let closest_e = case.rel_start_m.0 + case.rel_velocity_mps.0 * closest_s;
        let closest_n = case.rel_start_m.1 + case.rel_velocity_mps.1 * closest_s;
        let closest_distance = closest_e.hypot(closest_n);
        if wants_alert {
            assert!(
                f64::from(case.vertical_separation_ft) < V_LIMIT_FT,
                "{}",
                case.name
            );
        } else {
            assert!(
                closest_distance >= H_LIMIT_M + 1_500.0
                    || f64::from(case.vertical_separation_ft) >= V_LIMIT_FT + 500.0,
                "{} must have its declared safe margin",
                case.name
            );
        }
        let addr_a = 1000 + case_ix as u32 * 2;
        let addr_b = addr_a + 1;
        let (mut state, mut alerts, mut events, mut net) = rig();
        let mut first_alert_s = None;
        let mut degraded_at_alert = false;
        for tick in 0..=120_i64 {
            let t = tick as f64;
            let rel_e = case.rel_start_m.0 + case.rel_velocity_mps.0 * t;
            let rel_n = case.rel_start_m.1 + case.rel_velocity_mps.1 * t;
            let sampled_truth = rel_e * rel_e + rel_n * rel_n < H_LIMIT_M * H_LIMIT_M
                && f64::from(case.vertical_separation_ft) < V_LIMIT_FT;
            let declared_truth = case
                .truth_interval_s
                .is_some_and(|(entry, exit)| t > entry && t < exit);
            assert_eq!(sampled_truth, declared_truth, "{} tick {tick}", case.name);
            let mut rows = [
                track_xy(
                    addr_a,
                    (-case.rel_start_m.0 - case.rel_velocity_mps.0 * t) / 2.0,
                    (-case.rel_start_m.1 - case.rel_velocity_mps.1 * t) / 2.0,
                    (-case.rel_velocity_mps.0 / 2.0) as f32,
                    (-case.rel_velocity_mps.1 / 2.0) as f32,
                    10_000.0,
                ),
                track_xy(
                    addr_b,
                    (case.rel_start_m.0 + case.rel_velocity_mps.0 * t) / 2.0,
                    (case.rel_start_m.1 + case.rel_velocity_mps.1 * t) / 2.0,
                    (case.rel_velocity_mps.0 / 2.0) as f32,
                    (case.rel_velocity_mps.1 / 2.0) as f32,
                    10_000.0 + case.vertical_separation_ft,
                ),
            ];
            if case.degraded_at_s == Some(tick) {
                // A synthetic upstream loss and unknown observed altitude are visible as a
                // degraded cycle; they do not erase the already-open horizontal warning.
                ca_note_lost_input(&mut state, 1);
                rows[1].alt_ft = f32::NAN;
                rows[1].vrate_fpm = f32::NAN;
            }
            let out = ca_cycle(
                &mut state,
                tick * SECOND,
                &rows,
                &mut alerts,
                &mut events,
                &mut net,
            );
            expected_cycles += 1;
            if !matches!(out.report, NetReport::Running) {
                degraded_cycles += 1;
            }
            if case.degraded_at_s == Some(tick) {
                assert!(matches!(
                    out.report,
                    NetReport::Degraded {
                        reason: Reason::InputMissing,
                        count: 1,
                        ..
                    }
                ));
            }
            assert_eq!(out.stats.out_of_order, 0, "{} tick {tick}", case.name);
            let active = ca_open(&state)
                .iter()
                .find(|a| a.addr_a == addr_a && a.addr_b == addr_b);
            if let Some(alert) = active {
                first_alert_s.get_or_insert(t);
                if case.degraded_at_s == Some(tick) {
                    degraded_at_alert = alert.flags & AF_ALT_UNKNOWN != 0;
                    assert_eq!(alert.ticks_unevaluated, 0, "{}", case.name);
                }
            }
        }

        if let Some((entry_s, exit_s)) = case.truth_interval_s {
            desired_denominator += 1;
            assert!(exit_s > entry_s, "{} truth window", case.name);
            match first_alert_s {
                None => misses += 1,
                Some(t) if t < entry_s => {
                    early += 1;
                    lead_seconds.push(entry_s - t);
                }
                Some(_) => late += 1,
            }
            if case.degraded_at_s.is_some() {
                assert!(degraded_at_alert, "{} lost-warning state", case.name);
            }
        } else {
            no_alert_denominator += 1;
            nuisance_cases += usize::from(first_alert_s.is_some());
        }
    }

    // Case denominators are declared cases, independent of cycle count and detector output.
    assert_eq!((misses, early, late, desired_denominator), (0, 3, 0, 3));
    assert_eq!((nuisance_cases, no_alert_denominator), (0, 2));
    assert_eq!(lead_seconds.len(), 3);
    assert!((lead_seconds[0] - 77.639_320_225).abs() < 1e-6);
    assert!((lead_seconds[1] - 82.322_330_47).abs() < 1e-6);
    assert!((lead_seconds[2] - 60.0).abs() < 1e-6);
    // Availability has a cycle denominator separate from the three desired-alert cases.
    assert_eq!((degraded_cycles, expected_cycles), (1, 5 * 121));
}

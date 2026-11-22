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
    let (lat_deg, lon_deg) = unproject(&frame_new(BASE.0, BASE.1), east_m, 0.0).unwrap();
    CaTrack {
        lat_deg,
        lon_deg,
        addr,
        vel_east_mps: east_mps,
        vel_north_mps: 0.0,
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

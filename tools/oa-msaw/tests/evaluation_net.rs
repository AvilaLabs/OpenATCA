//! The evaluator must accept real net records and respect the information they actually retain.
//! All motion, floors and configuration values here are synthetic development fixtures.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use oa_msaw::*;
use oa_msaw_run::evaluation::{evaluate, EvaluationError, HazardWindow};
use oa_safety::inhibit::inhibits_new;
use oa_safety::net::state_table_new;

const S: i64 = 1_000_000_000;

fn config() -> MsawConfig {
    MsawConfig {
        max_tracks: 4,
        max_open: 4,
        track_budget: 4,
        look_ahead_s: 60.0,
        warning_s: 20.0,
        samples: 13,
        hysteresis_ns: 5 * S,
        terrain_clearance_ft: 500,
        obstacle_clearance_ft: 500,
        max_speed_mps: 400.0,
        wide_degrade_share: 0.5,
        approach_allowance_ft: 300,
        approach_min_clearance_ft: 100,
        departure_min_clearance_ft: 100,
        velocity_uncertainty_mps: 5.0,
        max_altitude_age_s: 12.0,
        stale_hold_s: 30.0,
        altitude_loss_degrade_share: 0.5,
        terminal_lookahead_s: 15.0,
        capture_heading_deg: 30.0,
        corridors: Box::default(),
    }
}

fn net_records(rows: &[(i64, f32, f32)]) -> Vec<Alert> {
    let mut state = msaw_new(&config()).unwrap();
    let mut alerts = alert_log_new(8).unwrap();
    let mut skips = skip_log_new(16).unwrap();
    let mut status = state_table_new(10 * S, 2 * S).unwrap();
    let mut inhibitions = Box::new(inhibits_new());
    for &(seconds, altitude_ft, rate_fpm) in rows {
        let track = MsawTrack {
            addr: 7,
            lat_deg: 0.0,
            lon_deg: 0.0,
            vel_east_mps: 0.0,
            vel_north_mps: 100.0,
            r95_m: 50.0,
            alt_ft: altitude_ft,
            vrate_fpm: rate_fpm,
            flags: 0,
            ground: AIRBORNE,
            alt_age_s: 0.0,
            alt_change_10s_ft: 0.0,
            ground_ft: f32::NAN,
        };
        msaw_plan(&mut state, seconds * S, &[track], &mut skips);
        let floors = vec![
            FloorSample {
                terrain_ft: 0,
                obstacle_ft: FLOOR_NONE,
                obstacle_covered: true,
                mva_ft: FLOOR_NONE,
                gtm_clearance_ft: FLOOR_NONE,
                apt_clearance_ft: FLOOR_NONE,
                apt_ceiling_ft: FLOOR_NONE,
            };
            msaw_points(&state).len()
        ];
        msaw_evaluate(
            &mut state,
            &floors,
            &mut alerts,
            &mut skips,
            &mut status,
            &mut inhibitions,
        );
    }
    assert_eq!(msaw_finish(&mut state, &mut alerts), 0);
    assert_eq!(alerts.overflow, 0);
    assert_eq!(skips.overflow, 0);
    alerts.items
}

fn encounter() -> HazardWindow {
    HazardWindow {
        addr: 7,
        start_ns: 100 * S,
        warning_due_ns: 110 * S,
        end_ns: 120 * S,
    }
}

// REQ: OA-MSAW-200, OA-MSAW-203
#[test]
fn real_net_warning_and_terminal_altitude_loss_are_scored_separately() {
    let records = net_records(&[(105, 400.0, 0.0), (115, f32::NAN, 0.0)]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].warning_ns, Some(105 * S));
    assert_eq!(records[0].end_ns, 105 * S);
    assert_eq!(records[0].protect_ns, 115 * S);
    let result = evaluate(Some(&[encounter()]), Some(&records)).unwrap();
    assert_eq!(result.timely, Some(1));
    assert_eq!(result.missed, Some(0));
    assert_eq!(result.loss_marked_alert_records, Some(1));
    assert_eq!(result.loss_observations_during_hazard, Some(1));
}

// REQ: OA-MSAW-201, OA-MSAW-205
#[test]
fn a_retained_first_warning_does_not_prove_warning_after_protection_returns() {
    let records = net_records(&[(80, 400.0, 0.0), (95, f32::NAN, 0.0), (105, 800.0, -600.0)]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].warning_ns, Some(80 * S));
    assert_eq!(records[0].end_ns, 105 * S);
    assert_eq!(records[0].end_reason, END_CLEAR);
    assert_eq!(records[0].protect_ns, 0);
    assert_eq!(
        evaluate(Some(&[encounter()]), Some(&records)),
        Err(EvaluationError::AmbiguousPreWindowWarning)
    );
}

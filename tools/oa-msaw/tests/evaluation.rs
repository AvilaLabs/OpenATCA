//! Independent synthetic hazard windows are the scoring oracle; no window is inferred from alerts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_msaw::{
    Alert, FloorKind, Grade, END_CLEAR, END_LOST_ALTITUDE_MISSING, END_LOST_ALTITUDE_STALE,
};
use oa_msaw_run::evaluation::{evaluate, EvaluationError, HazardWindow, MAX_EVALUATION_ROWS};

fn hazard(addr: u32, start_ns: i64, due_ns: i64, end_ns: i64) -> HazardWindow {
    HazardWindow {
        addr,
        start_ns,
        warning_due_ns: due_ns,
        end_ns,
    }
}

fn alert(addr: u32, start_ns: i64, end_ns: i64, warning_ns: Option<i64>) -> Alert {
    Alert {
        addr,
        start_ns,
        end_ns,
        caution_ns: Some(start_ns),
        warning_ns,
        peak: if warning_ns.is_some() {
            Grade::Warning
        } else {
            Grade::Caution
        },
        lat_e7: 0,
        lon_e7: 0,
        alt_ft: 0.0,
        vrate_fpm: 0.0,
        speed_mps: 0.0,
        worst_margin_ft: -1.0,
        worst_floor_ft: 0,
        worst_kind: FloorKind::Terrain,
        worst_lat_e7: 0,
        worst_lon_e7: 0,
        worst_radius_m: 0.0,
        worst_k: 0,
        worst_ns: start_ns,
        worst_flags: 0,
        r95_m: 0.0,
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        ttv_min_s: 0.0,
        track_deg: -1,
        worst_kinds: 0,
        cycles: 1,
        flags: 0,
        end_reason: END_CLEAR,
        protect_ns: 0,
    }
}

// REQ: OA-MSAW-200, OA-MSAW-201
#[test]
fn independent_windows_score_warning_deadline_and_end_boundaries_with_conservation() {
    let truth = [
        hazard(10, 100, 110, 120),
        hazard(20, 200, 210, 220),
        hazard(30, 300, 310, 320),
        hazard(40, 400, 410, 420),
    ];
    let emitted = [
        alert(10, 110, 115, Some(110)), // At the deadline: timely.
        alert(20, 200, 215, Some(211)), // One tick after deadline: late.
        alert(30, 300, 320, None),      // Caution without warning: missed.
        alert(40, 390, 415, Some(400)), // Warning onset at encounter start: timely.
    ];
    let scored = evaluate(Some(&truth), Some(&emitted)).unwrap();
    assert_eq!(scored.encounters, Some(4));
    assert_eq!(scored.timely, Some(2));
    assert_eq!(scored.late, Some(1));
    assert_eq!(scored.missed, Some(1));
    assert_eq!(
        scored.timely.unwrap() + scored.late.unwrap() + scored.missed.unwrap(),
        scored.encounters.unwrap()
    );
    assert_eq!(evaluate(Some(&truth), Some(&emitted)).unwrap(), scored);
    let mut reversed_truth = truth;
    reversed_truth.reverse();
    let mut reversed_emitted = emitted;
    reversed_emitted.reverse();
    assert_eq!(
        evaluate(Some(&reversed_truth), Some(&reversed_emitted)).unwrap(),
        scored
    );
}

// REQ: OA-MSAW-202, OA-MSAW-203
#[test]
fn full_address_matching_nuisance_and_protection_loss_are_separate_counts() {
    let truth = [hazard(0x00ab_cdef, 100, 110, 120)];
    let mut lost = alert(0x00ab_cdef, 105, 108, Some(105));
    lost.end_reason = END_LOST_ALTITUDE_MISSING;
    lost.protect_ns = 115;
    let emitted = [
        lost,
        alert(0x00ab_cdee, 100, 120, Some(100)), // Similar address, distinct identity.
        alert(0x00ab_cdef, 130, 140, None),      // Same address, outside truth.
    ];
    let scored = evaluate(Some(&truth), Some(&emitted)).unwrap();
    assert_eq!(scored.timely, Some(1));
    assert_eq!(scored.alert_episodes, Some(3));
    assert_eq!(scored.nuisance_episodes, Some(2));
    assert_eq!(scored.loss_marked_alert_records, Some(1));
    assert_eq!(scored.loss_observations_during_hazard, Some(1));
}

// REQ: OA-MSAW-201, OA-MSAW-203
#[test]
fn warning_credit_stops_at_last_violation_and_loss_is_a_point_observation() {
    let truth = [hazard(7, 100, 110, 120)];
    let mut warning_ended_early = alert(7, 80, 90, Some(80));
    warning_ended_early.end_reason = END_LOST_ALTITUDE_MISSING;
    warning_ended_early.protect_ns = 115;
    let scored = evaluate(Some(&truth), Some(&[warning_ended_early])).unwrap();
    assert_eq!(scored.timely, Some(0));
    assert_eq!(scored.late, Some(0));
    assert_eq!(scored.missed, Some(1));
    assert_eq!(scored.loss_marked_alert_records, Some(1));
    assert_eq!(scored.loss_observations_during_hazard, Some(1));

    let mut episode_overlaps_but_loss_is_later = alert(7, 105, 110, None);
    episode_overlaps_but_loss_is_later.end_reason = END_LOST_ALTITUDE_STALE;
    episode_overlaps_but_loss_is_later.protect_ns = 130;
    let scored = evaluate(Some(&truth), Some(&[episode_overlaps_but_loss_is_later])).unwrap();
    assert_eq!(scored.nuisance_episodes, Some(0));
    assert_eq!(scored.loss_marked_alert_records, Some(1));
    assert_eq!(scored.loss_observations_during_hazard, Some(0));
}

// REQ: OA-MSAW-201
#[test]
fn a_pre_window_warning_spanning_the_encounter_is_ambiguous_not_proven_active() {
    let truth = [hazard(8, 100, 110, 120)];
    // The compressed alert records that warning occurred at 80 and that the episode continued
    // through 115, but does not say whether its grade stayed WARNING or returned to CAUTION.
    let prior_warning_then_unknown_grade = alert(8, 80, 115, Some(80));
    assert_eq!(
        evaluate(Some(&truth), Some(&[prior_warning_then_unknown_grade])),
        Err(EvaluationError::AmbiguousPreWindowWarning)
    );
}

// REQ: OA-MSAW-204
#[test]
fn missing_ground_truth_and_alert_evidence_are_not_reported_as_zeroes() {
    let no_truth = evaluate(None, Some(&[])).unwrap();
    assert_eq!(no_truth.encounters, None);
    assert_eq!(no_truth.timely, None);
    assert_eq!(no_truth.late, None);
    assert_eq!(no_truth.missed, None);
    assert_eq!(no_truth.nuisance_episodes, None);
    assert_eq!(no_truth.loss_marked_alert_records, Some(0));

    let empty_truth = evaluate(Some(&[]), Some(&[])).unwrap();
    assert_eq!(empty_truth.encounters, Some(0));
    assert_eq!(empty_truth.timely, Some(0));
    assert_eq!(empty_truth.late, Some(0));
    assert_eq!(empty_truth.missed, Some(0));
    assert_eq!(empty_truth.nuisance_episodes, Some(0));

    let missing_alerts = evaluate(Some(&[hazard(1, 0, 1, 2)]), None).unwrap();
    assert_eq!(missing_alerts.encounters, Some(1));
    assert_eq!(missing_alerts.timely, None);
    assert_eq!(missing_alerts.missed, None);
    assert_eq!(missing_alerts.loss_marked_alert_records, None);
}

// REQ: OA-MSAW-205
#[test]
fn invalid_windows_alert_transitions_and_protection_times_are_refused() {
    assert_eq!(
        evaluate(Some(&[hazard(1, 10, 9, 20)]), Some(&[])),
        Err(EvaluationError::InvalidHazardWindow)
    );
    let malformed_and_valid = [hazard(2, 10, 9, 20), hazard(1, 0, 1, 2)];
    assert_eq!(
        evaluate(Some(&malformed_and_valid), Some(&[])),
        Err(EvaluationError::InvalidHazardWindow)
    );
    assert_eq!(
        evaluate(
            Some(&[malformed_and_valid[1], malformed_and_valid[0]]),
            Some(&[])
        ),
        Err(EvaluationError::InvalidHazardWindow)
    );
    assert_eq!(
        evaluate(
            Some(&[hazard(1, 0, 10, 20), hazard(1, 20, 30, 40)]),
            Some(&[])
        ),
        Err(EvaluationError::OverlappingHazardWindows)
    );
    let duplicate = [hazard(1, 0, 10, 20), hazard(1, 0, 10, 20)];
    assert_eq!(
        evaluate(Some(&duplicate), Some(&[])),
        Err(EvaluationError::OverlappingHazardWindows)
    );
    assert_eq!(
        evaluate(Some(&[duplicate[1], duplicate[0]]), Some(&[])),
        Err(EvaluationError::OverlappingHazardWindows)
    );
    assert_eq!(
        evaluate(Some(&[]), Some(&[alert(1, 10, 9, None)])),
        Err(EvaluationError::InvalidAlertInterval)
    );
    let mut bad_transition = alert(1, 10, 20, Some(21));
    bad_transition.caution_ns = Some(9);
    assert_eq!(
        evaluate(Some(&[]), Some(&[bad_transition])),
        Err(EvaluationError::InvalidAlertTransition)
    );
    let mut bad_grade = alert(1, 10, 20, None);
    bad_grade.peak = Grade::Warning;
    assert_eq!(
        evaluate(Some(&[]), Some(&[bad_grade])),
        Err(EvaluationError::InvalidAlertTransition)
    );
    let mut bad_protection = alert(1, 10, 20, None);
    bad_protection.end_reason = END_LOST_ALTITUDE_MISSING;
    bad_protection.protect_ns = 19;
    assert_eq!(
        evaluate(Some(&[]), Some(&[bad_protection])),
        Err(EvaluationError::InvalidProtectionTime)
    );
    let mut unknown_end = alert(1, 10, 20, None);
    unknown_end.end_reason = 9;
    assert_eq!(
        evaluate(Some(&[]), Some(&[unknown_end])),
        Err(EvaluationError::InvalidAlertState)
    );
}

// REQ: OA-MSAW-205
#[test]
fn runtime_impossible_episode_states_are_refused() {
    let mut late_caution = alert(1, 10, 20, None);
    late_caution.caution_ns = Some(11);
    assert_eq!(
        evaluate(Some(&[]), Some(&[late_caution])),
        Err(EvaluationError::InvalidAlertTransition)
    );
    let mut simultaneous_loss = alert(1, 10, 20, None);
    simultaneous_loss.end_reason = END_LOST_ALTITUDE_MISSING;
    simultaneous_loss.protect_ns = 20;
    assert_eq!(
        evaluate(Some(&[]), Some(&[simultaneous_loss])),
        Err(EvaluationError::InvalidProtectionTime)
    );
    let mut no_cycle = alert(1, 10, 20, None);
    no_cycle.cycles = 0;
    assert_eq!(
        evaluate(Some(&[]), Some(&[no_cycle])),
        Err(EvaluationError::InvalidAlertState)
    );
}

// REQ: OA-MSAW-205
#[test]
fn public_evaluator_accepts_exact_pair_budget_and_row_limit_but_refuses_more() {
    // 3 * 125 * 26,646 + 125 * 124 / 2 = 10,000,000, the whole configured budget.
    let hazards: Vec<_> = (1..=125).map(|addr| hazard(addr, 10, 15, 20)).collect();
    let mut records = vec![alert(1000, 10, 20, None); 26_646];
    let result = evaluate(Some(&hazards), Some(&records)).unwrap();
    assert_eq!(result.missed, Some(125));
    assert_eq!(result.nuisance_episodes, Some(26_646));
    records.push(alert(1000, 10, 20, None));
    assert_eq!(
        evaluate(Some(&hazards), Some(&records)),
        Err(EvaluationError::TooManyComparisons)
    );
    records.resize(MAX_EVALUATION_ROWS, alert(1000, 10, 20, None));
    assert_eq!(
        evaluate(None, Some(&records)).unwrap().alert_episodes,
        Some(MAX_EVALUATION_ROWS)
    );
    records.push(alert(1000, 10, 20, None));
    assert_eq!(
        evaluate(None, Some(&records)),
        Err(EvaluationError::TooManyRows)
    );
}

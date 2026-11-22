//! Synthetic matcher fixtures: labels are declared independently from warning episodes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use oa_encounter_run::qualification::{
    score, AlertEpisode, Case, CaseResult, Outcome, ScoreError, Subject, MAX_CASES, MAX_WARNINGS,
};
use oa_safety::net::{Area, NetReport, Reason};

fn desired(subject: Subject, report: NetReport) -> Case {
    Case {
        subject,
        window_start_ns: 40,
        window_end_ns: 120,
        violation: Some((100, 110)),
        alert_desired: true,
        warning_from_ns: 50,
        warning_deadline_ns: 80,
        evaluation: report,
    }
}

fn control(subject: Subject, report: NetReport) -> Case {
    Case {
        subject,
        window_start_ns: 40,
        window_end_ns: 120,
        violation: None,
        alert_desired: false,
        warning_from_ns: 40,
        warning_deadline_ns: 120,
        evaluation: report,
    }
}

#[test]
fn matching_is_subject_specific_deduplicated_and_keeps_gaps_in_denominators() {
    let pair_hit = Subject { a: 1, b: 2 };
    let pair_control = Subject { a: 3, b: 4 };
    let pair_unknown = Subject { a: 5, b: 6 };
    let pair_wrong_target = Subject { a: 8, b: 9 };
    let track_unknown = Subject { a: 7, b: 0 };
    let mut second_event_same_subject = desired(pair_hit, NetReport::Running);
    second_event_same_subject.window_end_ns = 220;
    second_event_same_subject.violation = Some((200, 210));
    let cases = [
        desired(pair_hit, NetReport::Running),
        second_event_same_subject,
        control(pair_control, NetReport::Running),
        desired(
            pair_unknown,
            NetReport::Degraded {
                reason: Reason::InputMissing,
                area: Area::Whole,
                count: 2,
            },
        ),
        control(
            track_unknown,
            NetReport::Unavailable {
                reason: Reason::SurveillanceLost,
            },
        ),
    ];
    let warnings = [
        AlertEpisode {
            subject: pair_hit,
            start_ns: 60,
            end_ns: 70,
            warning_ns: Some(60),
        },
        // Duplicate record for one onset must not yield another hit or lead sample.
        AlertEpisode {
            subject: pair_hit,
            start_ns: 60,
            end_ns: 75,
            warning_ns: Some(60),
        },
        // A warning for the wrong target cannot satisfy the desired event.
        AlertEpisode {
            subject: pair_wrong_target,
            start_ns: 60,
            end_ns: 70,
            warning_ns: Some(60),
        },
        // The warning exists, but the matching label is unevaluable and must remain unknown.
        AlertEpisode {
            subject: pair_unknown,
            start_ns: 60,
            end_ns: 70,
            warning_ns: Some(60),
        },
    ];
    let mut results = [CaseResult {
        outcome: Outcome::Quiet,
        warning_index: None,
        lead_ns: None,
    }; 5];

    let totals = score(&cases, &warnings, &mut results).unwrap();
    assert_eq!(
        (
            totals.desired_cases,
            totals.desired_evaluable,
            totals.hits,
            totals.misses
        ),
        (3, 2, 1, 1)
    );
    assert_eq!(totals.desired_unavailable, 1);
    assert_eq!(
        (
            totals.no_alert_cases,
            totals.controls_evaluable,
            totals.nuisance_controls,
            totals.quiet_controls,
            totals.controls_unavailable,
        ),
        (2, 1, 0, 1, 1)
    );
    assert_eq!(results[0].outcome, Outcome::Hit);
    assert_eq!(results[0].warning_index, Some(0));
    assert_eq!(results[0].lead_ns, Some(40));
    assert_eq!(results[1].outcome, Outcome::Miss);
    assert_eq!(results[2].outcome, Outcome::Quiet);
    assert_eq!(
        results[3].outcome,
        Outcome::Unavailable {
            reason: Reason::InputMissing,
            shortfall: 2,
        }
    );
    assert_eq!(
        results[4].outcome,
        Outcome::Unavailable {
            reason: Reason::SurveillanceLost,
            shortfall: 0,
        }
    );
}

#[test]
fn misses_nuisance_controls_and_capacity_failures_are_explicit() {
    let pair_missed = Subject { a: 11, b: 12 };
    let track_nuisance = Subject { a: 13, b: 0 };
    let cases = [
        desired(pair_missed, NetReport::Running),
        control(track_nuisance, NetReport::Running),
    ];
    let warnings = [AlertEpisode {
        subject: track_nuisance,
        start_ns: 50,
        end_ns: 90,
        warning_ns: Some(50),
    }];
    let mut results = [CaseResult {
        outcome: Outcome::Quiet,
        warning_index: None,
        lead_ns: None,
    }; 2];
    let totals = score(&cases, &warnings, &mut results).unwrap();
    assert_eq!(
        (totals.desired_cases, totals.hits, totals.misses),
        (1, 0, 1)
    );
    assert_eq!(
        (
            totals.no_alert_cases,
            totals.nuisance_controls,
            totals.quiet_controls
        ),
        (1, 1, 0)
    );
    assert_eq!(results[0].outcome, Outcome::Miss);
    assert_eq!(results[1].outcome, Outcome::Nuisance);

    let too_many_cases = vec![control(track_nuisance, NetReport::Running); MAX_CASES + 1];
    let mut too_many_results = vec![results[0]; MAX_CASES + 1];
    assert!(score(&too_many_cases, &[], &mut too_many_results).is_err());
    let many_warnings = vec![warnings[0]; MAX_WARNINGS + 1];
    assert!(score(&[], &many_warnings, &mut []).is_err());
    assert!(score(&cases, &warnings, &mut []).is_err());
}

#[test]
fn unrepresentable_warning_lead_is_refused_before_any_result_changes() {
    let cases = [
        control(Subject { a: 1, b: 0 }, NetReport::Running),
        Case {
            window_start_ns: i64::MIN,
            warning_from_ns: i64::MIN,
            warning_deadline_ns: -1,
            violation: Some((0, 110)),
            ..desired(Subject { a: 2, b: 0 }, NetReport::Running)
        },
    ];
    let initial = [CaseResult {
        outcome: Outcome::Miss,
        warning_index: None,
        lead_ns: None,
    }; 2];
    let mut results = initial;
    assert_eq!(
        score(&cases, &[], &mut results),
        Err(ScoreError::InvalidInterval)
    );
    assert_eq!(results, initial);
}

//! Deterministic synthetic mutation fixtures for the independent monitor.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use oa_safety::monitor::{
    check_pairs, MonitorError, MonitorFinding, MonitorLimits, MonitorPair, MonitorStatus,
    COMMON_LOCAL_FRAME, HAS_CLAIM, HAS_REFERENCE, MAX_PAIRS, VIOLATION_ALTITUDE, VIOLATION_EAST,
};

const SECOND: i64 = 1_000_000_000;
const NOW: i64 = 100 * SECOND;
const PRESENT: u8 = HAS_REFERENCE | HAS_CLAIM | COMMON_LOCAL_FRAME;
const LIMITS: MonitorLimits = MonitorLimits {
    max_age_ns: 2 * SECOND as u64,
    max_pair_skew_ns: SECOND as u64,
    max_horizontal_rate_mm_s: 1_000,
    max_vertical_rate_ft_s: 10,
    horizontal_tolerance_mm: 50,
    vertical_tolerance_ft: 2,
};

fn pair(id: u32) -> MonitorPair {
    MonitorPair {
        reference_id: id,
        claim_id: id,
        reference_time_ns: NOW,
        claim_time_ns: NOW,
        reference_east_mm: 0,
        reference_north_mm: 0,
        claim_east_mm: 0,
        claim_north_mm: 0,
        reference_alt_ft: 1_000,
        claim_alt_ft: 1_000,
        presence: PRESENT,
    }
}

fn empty_finding() -> MonitorFinding {
    MonitorFinding {
        reference_id: 0,
        claim_id: 0,
        status: MonitorStatus::InvalidEvidence,
        violations: 0,
    }
}

// REQ: OA-SAF-020
#[test]
fn synthetic_mutations_separate_detected_missed_and_unavailable_evidence() {
    let mut inputs = [
        pair(1),
        pair(2),
        pair(3),
        pair(4),
        pair(5),
        pair(6),
        pair(7),
        pair(8),
        pair(9),
        pair(10),
        pair(11),
        pair(12),
        pair(14),
        pair(15),
        pair(16),
        pair(17),
        pair(18),
        pair(19),
        pair(20),
        pair(22),
        pair(23),
    ];
    // Three declared output mutations: two exceed the supplied envelope; the 200 mm shift is
    // plausible within the one-second motion allowance and is deliberately expected to be missed.
    inputs[0].reference_time_ns = NOW - SECOND;
    inputs[0].claim_east_mm = 100;
    inputs[0].claim_alt_ft = 1_005;
    inputs[1].claim_east_mm = 5_000;
    inputs[2].claim_alt_ft = 1_050;
    inputs[3].reference_time_ns = NOW - SECOND;
    inputs[3].claim_east_mm = 200;

    // Evidence and identity controls. The keys are supplied externally; no pairing is inferred.
    inputs[4].presence &= !COMMON_LOCAL_FRAME;
    inputs[5].reference_time_ns = NOW - 10 * SECOND;
    inputs[6].presence = HAS_CLAIM | COMMON_LOCAL_FRAME;
    inputs[7].presence = HAS_REFERENCE | COMMON_LOCAL_FRAME;
    inputs[8].claim_id = 90;
    inputs[9].claim_time_ns = i64::MAX;
    inputs[10].reference_id = 11;
    inputs[11].reference_id = 11;
    inputs[12].presence |= 0x80;
    inputs[13].presence = COMMON_LOCAL_FRAME;
    inputs[14].claim_time_ns = NOW - 10 * SECOND;
    inputs[15].reference_time_ns = NOW - 10 * SECOND;
    inputs[15].claim_time_ns = NOW - 10 * SECOND;
    inputs[16].reference_time_ns = NOW - 2 * SECOND;
    inputs[17].reference_time_ns = i64::MAX;
    inputs[18].claim_id = 21;
    inputs[19].claim_id = 21;
    // The same aircraft at a different source and claim time is not duplicate evidence.
    inputs[20].reference_id = 1;
    inputs[20].claim_id = 1;
    inputs[20].reference_time_ns = NOW - 2 * SECOND;
    inputs[20].claim_time_ns = NOW - SECOND;

    let mut findings = [empty_finding(); 21];
    let report = check_pairs(NOW, &inputs, LIMITS, &mut findings).unwrap();
    assert_eq!(
        (report.pairs, report.healthy, report.contradicted),
        (21, 3, 2)
    );
    assert_eq!(report.not_comparable, 16);
    let expected = [
        MonitorStatus::Healthy,
        MonitorStatus::Contradiction,
        MonitorStatus::Contradiction,
        MonitorStatus::Healthy,
        MonitorStatus::UnsupportedFrame,
        MonitorStatus::StaleReference,
        MonitorStatus::MissingReference,
        MonitorStatus::MissingClaim,
        MonitorStatus::IdentityMismatch,
        MonitorStatus::FutureTimestamp,
        MonitorStatus::DuplicateEvidence,
        MonitorStatus::DuplicateEvidence,
        MonitorStatus::InvalidEvidence,
        MonitorStatus::MissingBoth,
        MonitorStatus::StaleClaim,
        MonitorStatus::StaleBoth,
        MonitorStatus::TimeSkew,
        MonitorStatus::FutureTimestamp,
        MonitorStatus::DuplicateEvidence,
        MonitorStatus::DuplicateEvidence,
        MonitorStatus::Healthy,
    ];
    for (finding, status) in findings.iter().zip(expected) {
        assert_eq!(finding.status, status);
    }
    assert_eq!(findings[1].violations, VIOLATION_EAST);
    assert_eq!(findings[2].violations, VIOLATION_ALTITUDE);

    // Mutation denominator is the three declared changed outputs, not all controls.
    let caught = usize::from(findings[1].status == MonitorStatus::Contradiction)
        + usize::from(findings[2].status == MonitorStatus::Contradiction);
    let missed = usize::from(findings[3].status == MonitorStatus::Healthy);
    assert_eq!((caught, missed, caught + missed), (2, 1, 3));
}

// REQ: OA-SAF-020
#[test]
fn caller_owned_bounds_fail_before_any_finding_is_written() {
    let inputs = [pair(1), pair(2)];
    let sentinel = MonitorFinding {
        reference_id: 99,
        claim_id: 99,
        status: MonitorStatus::StaleBoth,
        violations: 3,
    };
    let mut too_short = [sentinel];
    assert_eq!(
        check_pairs(NOW, &inputs, LIMITS, &mut too_short),
        Err(MonitorError::FindingsTooShort)
    );
    assert_eq!(too_short[0], sentinel);

    let too_many = [pair(1); MAX_PAIRS + 1];
    let mut output = [empty_finding(); MAX_PAIRS + 1];
    assert_eq!(
        check_pairs(NOW, &too_many, LIMITS, &mut output),
        Err(MonitorError::TooManyPairs)
    );
}

// REQ: OA-SAF-020
#[test]
fn unrepresentable_motion_envelope_is_not_reported_healthy() {
    let mut input = [pair(20)];
    input[0].reference_time_ns = i64::MIN;
    input[0].claim_time_ns = i64::MAX - 1;
    let limits = MonitorLimits {
        max_age_ns: u64::MAX,
        max_pair_skew_ns: u64::MAX,
        max_horizontal_rate_mm_s: u32::MAX,
        max_vertical_rate_ft_s: u32::MAX,
        horizontal_tolerance_mm: 0,
        vertical_tolerance_ft: 0,
    };
    let mut findings = [empty_finding(); 1];
    let report = check_pairs(i64::MAX, &input, limits, &mut findings).unwrap();
    assert_eq!(report.not_comparable, 1);
    assert_eq!(findings[0].status, MonitorStatus::UnsupportedRange);
}

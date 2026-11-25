#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_migration_shadow::{
    blocks, classify, compare, family_blocks, family_index, report_blocks, validate_conservation,
    BusinessKey, Classification, CompareError, Comparison, Coverage, EventState, EvidenceHeader,
    Family, FieldId, FieldIssue, FieldIssueKind, FieldState, FieldValue, Judgment, KeyState,
    Observation, Outcome, ReviewDecision, ReviewRole, SourceIdentity, ValueKind, FAMILY_COUNT,
    MAX_COMPARISON_FIELD_ISSUES,
};

fn source(stream: u32, offset: u64) -> SourceIdentity {
    SourceIdentity {
        stream,
        decoded_offset: offset,
        row: 0,
        operation_index: 0,
    }
}

fn key(family: Family, ordinal: u64, entity_byte: u8) -> BusinessKey {
    let mut entity_key = [0; 32];
    entity_key[0] = entity_byte;
    BusinessKey {
        entity_key,
        family,
        ordinal,
    }
}

fn header(coverage: [Coverage; FAMILY_COUNT], epoch_byte: u8) -> EvidenceHeader {
    let mut epoch_sha256 = [0; 32];
    epoch_sha256[0] = epoch_byte;
    EvidenceHeader {
        function_id: 4,
        scope_id: 26,
        window_from_ns: 0,
        window_to_ns: 10,
        source_time_basis: 1,
        observed_time_basis: 2,
        epoch_sha256,
        coverage,
    }
}

fn family_coverage(family: Family, status: Coverage) -> [Coverage; FAMILY_COUNT] {
    let mut coverage = [Coverage::CompleteEmpty; FAMILY_COUNT];
    coverage[family_index(family)] = status;
    coverage
}

fn event<'a>(
    family: Family,
    ordinal: u64,
    entity: u8,
    source: SourceIdentity,
    time: Option<i64>,
    observed: Option<i64>,
    fields: &'a [FieldValue],
) -> Observation<'a> {
    Observation {
        source,
        key_state: KeyState::Valid,
        key: key(family, ordinal, entity),
        source_time_ns: time,
        observed_time_ns: observed,
        event_state: EventState::Observed,
        fields,
    }
}

fn integer_field(id: u16, value: i64) -> FieldValue {
    FieldValue {
        id,
        state: FieldState::Present,
        kind: ValueKind::Integer,
        integer: value,
        bytes: [0; 32],
    }
}

fn complete_fields(family: Family, changed_id: Option<u16>, changed_value: i64) -> Vec<FieldValue> {
    oa_migration_shadow::required_field_ids(family)
        .iter()
        .map(|id| {
            integer_field(
                *id,
                if Some(*id) == changed_id {
                    changed_value
                } else {
                    7
                },
            )
        })
        .collect()
}

fn run<'a>(
    left_header: &EvidenceHeader,
    left: &[Observation<'a>],
    right_header: &EvidenceHeader,
    right: &[Observation<'a>],
) -> (oa_migration_shadow::ComparisonReport, Vec<Comparison>) {
    let mut output = vec![
        Comparison {
            key: None,
            family: Family::Handoff,
            legacy: None,
            candidate: None,
            legacy_count: 0,
            candidate_count: 0,
            outcome: Outcome::Unobservable,
            field_differences: 0,
            field_unobservable: 0,
            field_issue_count: 0,
            field_issues: [FieldIssue {
                id: 0,
                kind: FieldIssueKind::Unobservable
            }; MAX_COMPARISON_FIELD_ISSUES],
            source_time_delta_ns: None,
            observed_time_delta_ns: None,
        };
        left.len() + right.len()
    ];
    let report = compare(left_header, left, right_header, right, &mut output).unwrap();
    validate_conservation(&report).unwrap();
    output.truncate(report.comparison_rows);
    (report, output)
}

// REQ: OA-MIG-401
#[test]
fn exact_handoff_pair_keeps_event_and_observed_clocks_separate() {
    let lfields = complete_fields(Family::Handoff, None, 0);
    let rfields = complete_fields(Family::Handoff, None, 0);
    let left = [event(
        Family::Handoff,
        1,
        3,
        source(1, 10),
        Some(4),
        Some(5),
        &lfields,
    )];
    let right = [event(
        Family::Handoff,
        1,
        3,
        source(2, 20),
        Some(4),
        Some(5),
        &rfields,
    )];
    let lh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &right);

    assert_eq!(rows[0].outcome, Outcome::Equivalent);
    assert_eq!(rows[0].source_time_delta_ns, Some(0));
    assert_eq!(rows[0].observed_time_delta_ns, Some(0));
    assert_eq!(
        report.families[family_index(Family::Handoff)].exact_pairs,
        1
    );
    assert_eq!(
        report.families[family_index(Family::Handoff)].observed_time_differences,
        0
    );
    assert!(!family_blocks(
        &report,
        Family::Handoff,
        Classification::Equivalent
    ));
    assert!(report_blocks(&report, Classification::Equivalent));
}

// REQ: OA-MIG-401
#[test]
fn missing_handoff_projection_is_unobservable_even_when_both_sides_omit_it() {
    let left_empty = [event(
        Family::Handoff,
        1,
        1,
        source(1, 1),
        Some(2),
        Some(3),
        &[],
    )];
    let right_empty = [event(
        Family::Handoff,
        1,
        1,
        source(2, 1),
        Some(2),
        Some(3),
        &[],
    )];
    let lh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        2,
    );
    let (_, rows) = run(&lh, &left_empty, &rh, &right_empty);
    assert_eq!(rows[0].outcome, Outcome::Unobservable);
    assert!(
        rows[0].field_issues[..usize::from(rows[0].field_issue_count)]
            .iter()
            .any(|issue| issue.id == FieldId::HandoffReceivingUnit as u16
                && issue.kind == FieldIssueKind::MissingRequired)
    );

    let mut left_fields = complete_fields(Family::Handoff, None, 0);
    let mut right_fields = complete_fields(Family::Handoff, None, 0);
    left_fields.retain(|field| field.id != FieldId::HandoffReceivingUnit as u16);
    right_fields.retain(|field| field.id != FieldId::HandoffReceivingUnit as u16);
    let left_missing_destination = [event(
        Family::Handoff,
        1,
        2,
        source(1, 2),
        Some(4),
        Some(5),
        &left_fields,
    )];
    let right_missing_destination = [event(
        Family::Handoff,
        1,
        2,
        source(2, 2),
        Some(4),
        Some(5),
        &right_fields,
    )];
    let (_, rows) = run(
        &lh,
        &left_missing_destination,
        &rh,
        &right_missing_destination,
    );
    assert_eq!(rows[0].outcome, Outcome::Unobservable);
    assert!(
        rows[0].field_issues[..usize::from(rows[0].field_issue_count)]
            .iter()
            .any(|issue| issue.id == FieldId::HandoffReceivingUnit as u16
                && issue.kind == FieldIssueKind::MissingRequired)
    );
}

// REQ: OA-MIG-401
#[test]
fn observed_time_difference_is_not_hidden_by_matching_event_time() {
    let fields = complete_fields(Family::Handoff, None, 0);
    let left = [event(
        Family::Handoff,
        1,
        3,
        source(1, 10),
        Some(4),
        Some(5),
        &fields,
    )];
    let right = [event(
        Family::Handoff,
        1,
        3,
        source(2, 20),
        Some(4),
        Some(8),
        &fields,
    )];
    let lh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &right);
    assert_eq!(rows[0].source_time_delta_ns, Some(0));
    assert_eq!(rows[0].observed_time_delta_ns, Some(3));
    assert_eq!(rows[0].outcome, Outcome::Different);
    assert_eq!(
        report.families[family_index(Family::Handoff)].observed_time_differences,
        1
    );
}

// REQ: OA-MIG-401, OA-MIG-402
#[test]
fn disagreement_is_unresolved_until_scoped_review_and_evidence() {
    let lfields = complete_fields(Family::PointOut, Some(FieldId::PointOutState as u16), 7);
    let rfields = complete_fields(Family::PointOut, Some(FieldId::PointOutState as u16), 8);
    let left = [event(
        Family::PointOut,
        1,
        3,
        source(1, 10),
        Some(4),
        Some(5),
        &lfields,
    )];
    let right = [event(
        Family::PointOut,
        1,
        3,
        source(2, 20),
        Some(4),
        Some(5),
        &rfields,
    )];
    let lh = header(
        family_coverage(Family::PointOut, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::PointOut, Coverage::CompleteWithEvents),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &right);
    let diff = &rows[0];
    assert_eq!(diff.outcome, Outcome::Different);
    assert_eq!(diff.field_issue_count, 1);
    assert_eq!(diff.field_issues[0].id, FieldId::PointOutState as u16);
    assert_eq!(diff.field_issues[0].kind, FieldIssueKind::Different);
    assert_eq!(classify(&report, diff, None), Classification::Unresolved);
    assert!(blocks(Classification::Unresolved));

    let mut evidence_sha256 = [0; 32];
    evidence_sha256[0] = 0xa5;
    let review = ReviewDecision {
        policy_version: report.policy_version,
        function_id: report.function_id,
        scope_id: report.scope_id,
        window_from_ns: report.window_from_ns,
        window_to_ns: report.window_to_ns,
        legacy_epoch_sha256: report.legacy_epoch_sha256,
        candidate_epoch_sha256: report.candidate_epoch_sha256,
        key: diff.key,
        family: diff.family,
        legacy: diff.legacy,
        candidate: diff.candidate,
        reviewer_role: ReviewRole::IndependentAssessor,
        evidence_sha256,
        judgment: Judgment::Better,
    };
    assert_eq!(
        classify(&report, diff, Some(&review)),
        Classification::BetterReviewed
    );
    assert!(!family_blocks(
        &report,
        Family::PointOut,
        Classification::BetterReviewed
    ));

    let mut wrong_pair = review;
    wrong_pair.legacy = Some(source(99, 99));
    assert_eq!(
        classify(&report, diff, Some(&wrong_pair)),
        Classification::Unresolved
    );
    let mut no_evidence = review;
    no_evidence.evidence_sha256 = [0; 32];
    assert_eq!(
        classify(&report, diff, Some(&no_evidence)),
        Classification::Unresolved
    );
    let mut wrong_epoch = review;
    wrong_epoch.legacy_epoch_sha256[1] = 1;
    assert_eq!(
        classify(&report, diff, Some(&wrong_epoch)),
        Classification::Unresolved
    );
}

// REQ: OA-MIG-401, OA-MIG-402
#[test]
fn missing_legacy_alert_coverage_is_unobservable_not_agreement() {
    let candidate_coverage = family_coverage(Family::Alert, Coverage::CompleteEmpty);
    let legacy_coverage = family_coverage(Family::Alert, Coverage::Missing);
    let lh = header(legacy_coverage, 1);
    let rh = header(candidate_coverage, 2);
    let (report, rows) = run(&lh, &[], &rh, &[]);
    assert!(rows.is_empty());
    assert_ne!(
        report.coverage_incomplete_families & (1 << family_index(Family::Alert)),
        0
    );
    assert!(family_blocks(
        &report,
        Family::Alert,
        Classification::Equivalent
    ));
}

// REQ: OA-MIG-401, OA-MIG-402
#[test]
fn complete_empty_family_is_not_mistaken_for_a_matched_action() {
    let coverage = family_coverage(Family::Alert, Coverage::CompleteEmpty);
    let lh = header(coverage, 1);
    let rh = header(coverage, 2);
    let (report, rows) = run(&lh, &[], &rh, &[]);
    assert!(rows.is_empty());
    assert_ne!(
        report.no_event_families & (1 << family_index(Family::Alert)),
        0
    );
    assert!(family_blocks(
        &report,
        Family::Alert,
        Classification::Equivalent
    ));
}

// REQ: OA-MIG-401
#[test]
fn duplicate_business_keys_are_never_paired_first_wins() {
    let fields_a = [integer_field(1, 1)];
    let fields_b = [integer_field(1, 2)];
    let left = [
        event(
            Family::SectorAssignment,
            1,
            3,
            source(1, 10),
            Some(4),
            Some(5),
            &fields_a,
        ),
        event(
            Family::SectorAssignment,
            1,
            3,
            source(1, 11),
            Some(4),
            Some(6),
            &fields_b,
        ),
    ];
    let right = [event(
        Family::SectorAssignment,
        1,
        3,
        source(2, 20),
        Some(4),
        Some(5),
        &fields_a,
    )];
    let lh = header(
        family_coverage(Family::SectorAssignment, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::SectorAssignment, Coverage::CompleteWithEvents),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &right);
    assert_eq!(rows.len(), 3);
    assert!(rows
        .iter()
        .all(|row| row.outcome == Outcome::DuplicateAmbiguous));
    assert!(rows.iter().any(|row| row.legacy == Some(source(1, 10))));
    assert!(rows.iter().any(|row| row.legacy == Some(source(1, 11))));
    assert!(rows.iter().any(|row| row.candidate == Some(source(2, 20))));
    assert!(rows
        .iter()
        .all(|row| row.legacy_count == 2 && row.candidate_count == 1));
    assert_eq!(
        report.families[family_index(Family::SectorAssignment)].duplicate_groups,
        1
    );
}

// REQ: OA-MIG-401
#[test]
fn half_open_window_and_missing_time_are_explicit() {
    let fields = [integer_field(1, 42)];
    let left = [
        event(
            Family::FlightAmendment,
            1,
            1,
            source(1, 1),
            Some(0),
            Some(1),
            &fields,
        ),
        event(
            Family::FlightAmendment,
            2,
            1,
            source(1, 2),
            Some(10),
            Some(11),
            &fields,
        ),
        event(
            Family::FlightAmendment,
            3,
            1,
            source(1, 3),
            None,
            None,
            &fields,
        ),
    ];
    let right = [event(
        Family::FlightAmendment,
        1,
        1,
        source(2, 1),
        Some(0),
        Some(2),
        &fields,
    )];
    let lh = header(
        family_coverage(Family::FlightAmendment, Coverage::Partial),
        1,
    );
    let rh = header(
        family_coverage(Family::FlightAmendment, Coverage::CompleteWithEvents),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &right);
    let counts = report.families[family_index(Family::FlightAmendment)];
    assert_eq!(counts.legacy_records, 3);
    assert_eq!(counts.legacy_eligible, 1);
    assert_eq!(counts.legacy_outside_window, 1);
    assert_eq!(counts.legacy_missing_time, 1);
    assert_eq!(counts.legacy_observed_time_missing, 1);
    assert!(rows.iter().any(
        |row| row.key.is_some_and(|k| k.ordinal == 2) && row.outcome == Outcome::OutsideWindow
    ));
    assert!(
        rows.iter()
            .any(|row| row.key.is_some_and(|k| k.ordinal == 3)
                && row.outcome == Outcome::Unobservable)
    );
}

// REQ: OA-MIG-401
#[test]
fn missing_identity_remains_a_separate_denominator_outcome() {
    let fields = [integer_field(1, 42)];
    let mut unkeyed = event(
        Family::FlightAmendment,
        0,
        0,
        source(1, 1),
        Some(2),
        Some(3),
        &fields,
    );
    unkeyed.key_state = KeyState::Missing;
    let mut invalid = unkeyed;
    invalid.source = source(1, 2);
    invalid.key_state = KeyState::Invalid;
    let mut ambiguous = unkeyed;
    ambiguous.source = source(1, 3);
    ambiguous.key_state = KeyState::Ambiguous;
    let left = [unkeyed, invalid, ambiguous];
    let lh = header(
        family_coverage(Family::FlightAmendment, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::FlightAmendment, Coverage::CompleteEmpty),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &[]);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|row| row.outcome == Outcome::MissingKey));
    assert!(rows.iter().any(|row| row.outcome == Outcome::InvalidKey));
    assert!(rows.iter().any(|row| row.outcome == Outcome::AmbiguousKey));
    let counts = report.families[family_index(Family::FlightAmendment)];
    assert_eq!(counts.legacy_missing_key, 1);
    assert_eq!(counts.legacy_invalid_key, 1);
    assert_eq!(counts.legacy_ambiguous_key, 1);
    assert_eq!(counts.legacy_fields_present, 3);
    assert_eq!(counts.legacy_eligible, 0);
}

// REQ: OA-MIG-401, OA-MIG-402
#[test]
fn out_of_order_source_times_are_counted_and_block_the_family() {
    let fields = complete_fields(Family::Handoff, None, 0);
    let left = [
        event(
            Family::Handoff,
            1,
            1,
            source(1, 1),
            Some(5),
            Some(6),
            &fields,
        ),
        event(
            Family::Handoff,
            2,
            1,
            source(1, 2),
            Some(4),
            Some(7),
            &fields,
        ),
    ];
    let right = [
        event(
            Family::Handoff,
            1,
            1,
            source(2, 1),
            Some(5),
            Some(6),
            &fields,
        ),
        event(
            Family::Handoff,
            2,
            1,
            source(2, 2),
            Some(4),
            Some(7),
            &fields,
        ),
    ];
    let lh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &right);
    assert_eq!(
        report.families[family_index(Family::Handoff)].legacy_out_of_order,
        1
    );
    assert_eq!(
        report.families[family_index(Family::Handoff)].candidate_out_of_order,
        1
    );
    assert_ne!(
        report.out_of_order_side_families & (1 << family_index(Family::Handoff)),
        0
    );
    assert!(rows.iter().all(|row| row.outcome == Outcome::Equivalent));
    assert!(family_blocks(
        &report,
        Family::Handoff,
        Classification::Equivalent
    ));
}

// REQ: OA-MIG-401
#[test]
fn out_of_window_rows_do_not_reset_in_window_order_checks() {
    let fields = complete_fields(Family::Handoff, None, 0);
    let left = [
        event(
            Family::Handoff,
            1,
            1,
            source(1, 1),
            Some(200),
            Some(201),
            &fields,
        ),
        event(
            Family::Handoff,
            2,
            1,
            source(1, 2),
            Some(50),
            Some(51),
            &fields,
        ),
        event(
            Family::Handoff,
            3,
            1,
            source(1, 3),
            Some(150),
            Some(151),
            &fields,
        ),
    ];
    let right = [
        event(
            Family::Handoff,
            1,
            1,
            source(2, 1),
            Some(200),
            Some(201),
            &fields,
        ),
        event(
            Family::Handoff,
            2,
            1,
            source(2, 2),
            Some(50),
            Some(51),
            &fields,
        ),
        event(
            Family::Handoff,
            3,
            1,
            source(2, 3),
            Some(150),
            Some(151),
            &fields,
        ),
    ];
    let mut lh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        1,
    );
    let mut rh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        2,
    );
    lh.window_from_ns = 100;
    rh.window_from_ns = 100;
    lh.window_to_ns = 300;
    rh.window_to_ns = 300;
    let (report, rows) = run(&lh, &left, &rh, &right);
    let counts = report.families[family_index(Family::Handoff)];
    assert_eq!(counts.legacy_out_of_order, 1);
    assert_eq!(counts.candidate_out_of_order, 1);
    assert_eq!(counts.outside_only_groups, 1);
    assert_eq!(rows.len(), 3);
}

// REQ: OA-MIG-401
#[test]
fn incomplete_fields_do_not_become_false_differences_or_matches() {
    let left_fields = [FieldValue {
        id: 1,
        state: FieldState::Unknown,
        kind: ValueKind::None,
        integer: 0,
        bytes: [0; 32],
    }];
    let right_fields = [integer_field(1, 9)];
    let left = [event(
        Family::Alert,
        1,
        1,
        source(1, 1),
        Some(2),
        Some(3),
        &left_fields,
    )];
    let right = [event(
        Family::Alert,
        1,
        1,
        source(2, 1),
        Some(2),
        Some(3),
        &right_fields,
    )];
    let lh = header(
        family_coverage(Family::Alert, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(
        family_coverage(Family::Alert, Coverage::CompleteWithEvents),
        2,
    );
    let (report, rows) = run(&lh, &left, &rh, &right);
    assert_eq!(rows[0].outcome, Outcome::Unobservable);
    let counts = report.families[family_index(Family::Alert)];
    assert_eq!(counts.field_unobservable, 6);
    assert_eq!(counts.legacy_fields_unknown, 1);
    assert_eq!(counts.candidate_fields_present, 1);
    assert_eq!(rows[0].field_issue_count, 6);
    assert_eq!(rows[0].field_issues[0].id, 1);
    assert_eq!(rows[0].field_issues[0].kind, FieldIssueKind::Unobservable);
}

// REQ: OA-MIG-401
#[test]
fn comparison_refuses_scope_window_order_and_output_capacity_before_writing() {
    let fields = [integer_field(1, 1)];
    let left = [
        event(
            Family::Handoff,
            2,
            1,
            source(1, 1),
            Some(4),
            Some(5),
            &fields,
        ),
        event(
            Family::Handoff,
            1,
            1,
            source(1, 2),
            Some(5),
            Some(6),
            &fields,
        ),
    ];
    let right = [];
    let lh = header(
        family_coverage(Family::Handoff, Coverage::CompleteWithEvents),
        1,
    );
    let rh = header(family_coverage(Family::Handoff, Coverage::CompleteEmpty), 2);
    let mut output = [Comparison {
        key: None,
        family: Family::Alert,
        legacy: None,
        candidate: None,
        legacy_count: 99,
        candidate_count: 99,
        outcome: Outcome::Unobservable,
        field_differences: 0,
        field_unobservable: 0,
        field_issue_count: 0,
        field_issues: [FieldIssue {
            id: 0,
            kind: FieldIssueKind::Unobservable,
        }; MAX_COMPARISON_FIELD_ISSUES],
        source_time_delta_ns: None,
        observed_time_delta_ns: None,
    }; 2];
    assert_eq!(
        compare(&lh, &left, &rh, &right, &mut output),
        Err(CompareError::KeyOrder)
    );
    assert_eq!(output[0].legacy_count, 99);

    let sorted = [left[1], left[0]];
    assert_eq!(
        compare(&lh, &sorted, &rh, &right, &mut []),
        Err(CompareError::OutputCap)
    );

    let mut another_function = rh;
    another_function.function_id = lh.function_id + 1;
    let mut adequate_output = [output[0]; 2];
    assert_eq!(
        compare(
            &lh,
            &sorted,
            &another_function,
            &right,
            &mut adequate_output
        ),
        Err(CompareError::FunctionMismatch)
    );
}

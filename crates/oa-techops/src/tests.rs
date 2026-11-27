#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

// H2: tests use assertion panic paths intentionally so fault-matrix failures identify the violated contract.
use crate::*;

// REQ: OA-TECHOPS-001, OA-TECHOPS-002
#[test]
fn adaptation_identity_comparison_keeps_unavailable_and_mismatched_fields_distinct() {
    let matched = compare_adaptation_identity(
        [1; 32],
        Some([2; 32]),
        Some([1; 32]),
        Some([2; 32]),
        Some([2; 32]),
    );
    assert_eq!(matched.state, AdaptationIdentityState::Match);
    assert_eq!(matched.issued_manifest_matches, Some(true));
    assert_eq!(matched.loaded_matches_issued, Some(true));
    assert_eq!(matched.disk_matches_issued, Some(true));

    let unavailable =
        compare_adaptation_identity([1; 32], Some([2; 32]), Some([1; 32]), Some([2; 32]), None);
    assert_eq!(unavailable.state, AdaptationIdentityState::Incomplete);
    assert_eq!(unavailable.disk_matches_issued, None);
    assert_eq!(unavailable.loaded_matches_disk, None);

    let changed_disk = compare_adaptation_identity(
        [1; 32],
        Some([2; 32]),
        Some([3; 32]),
        Some([2; 32]),
        Some([4; 32]),
    );
    assert_eq!(changed_disk.state, AdaptationIdentityState::Mismatch);
    assert_eq!(changed_disk.issued_manifest_matches, Some(false));
    assert_eq!(changed_disk.disk_matches_issued, Some(false));
    assert_eq!(changed_disk.loaded_matches_disk, Some(false));

    let unavailable_and_mismatch =
        compare_adaptation_identity([1; 32], Some([2; 32]), None, Some([3; 32]), None);
    assert_eq!(
        unavailable_and_mismatch.state,
        AdaptationIdentityState::IncompleteAndMismatch
    );
}

fn inventory_query(
    previous_epoch: Option<ServiceEpoch>,
    freshness_limit_ns: i64,
) -> InventoryQuery {
    InventoryQuery {
        previous_epoch,
        epoch: ServiceEpoch(7),
        clock_domain_id: ClockDomainId(9),
        release_manifest_sha256: [4; 32],
        query_time_ns: 1_000,
        freshness_limit_ns,
    }
}

fn expected(id: u32, first: u64, next: u64) -> ExpectedProcessor {
    ExpectedProcessor {
        processor_id: ProcessorId(id),
        source_id: SourceId(id + 10),
        firmware_sha256: [1; 32],
        running_image_sha256: [2; 32],
        adaptation_sha256: [3; 32],
        sequence_first: first,
        sequence_next: next,
    }
}

fn report(id: u32, sequence: u64, time: i64) -> ProcessorReport {
    ProcessorReport {
        processor_id: ProcessorId(id),
        source_id: SourceId(id + 10),
        epoch: ServiceEpoch(7),
        clock_domain_id: ClockDomainId(9),
        sequence,
        sample_time_ns: time,
        release_manifest_sha256: [4; 32],
        firmware_sha256: [1; 32],
        running_image_sha256: [2; 32],
        loaded_adaptation_sha256: [3; 32],
        disk_adaptation_sha256: [3; 32],
        telemetry_loss_events: 0,
        adaptation_components_complete: true,
    }
}

// REQ: OA-TECHOPS-003
#[test]
fn issued_adaptation_companion_gap_sets_incomplete_aggregate_flag() {
    let expected = [expected(11, 1, 2)];
    let mut row = report(11, 1, 1_000);
    row.adaptation_components_complete = false;
    let result =
        assess_inventory(&expected, &[row], inventory_query(None, 1_000)).expect("bounded query");
    assert_ne!(
        result.flags & crate::FLAG_ADAPTATION_COMPONENT_INCOMPLETE,
        0
    );
    assert_ne!(
        result.processors[0].flags & crate::FLAG_ADAPTATION_COMPONENT_INCOMPLETE,
        0
    );
    assert_eq!(result.state, AssessmentState::Incomplete);
}

fn metric_contract(first: u64, next: u64, maximum: i64) -> MetricWindowContract {
    MetricWindowContract {
        profile_id: ProfileId(4),
        metric_id: MetricId(2),
        processor_id: ProcessorId(11),
        previous_epoch: Some(ServiceEpoch(6)),
        epoch: ServiceEpoch(7),
        clock_domain_id: ClockDomainId(9),
        window_start_ns: 100,
        window_end_ns: 1_000,
        sequence_first: first,
        sequence_next: next,
        maximum_allowed: maximum,
    }
}

fn metric_sample(sequence: u64, time: i64, value: i64) -> PerformanceSample {
    PerformanceSample {
        profile_id: ProfileId(4),
        metric_id: MetricId(2),
        processor_id: ProcessorId(11),
        epoch: ServiceEpoch(7),
        clock_domain_id: ClockDomainId(9),
        sequence,
        sample_time_ns: time,
        value,
        telemetry_loss_events: 0,
    }
}

// REQ: OA-TECHOPS-001, OA-TECHOPS-002
#[test]
fn inventory_counts_missing_processor_and_head_interior_tail_sequences() {
    let inventory = [expected(1, 10, 14), expected(2, 0, 2)];
    let reports = [report(1, 10, 980), report(1, 12, 990)];
    let result = assess_inventory(
        &inventory,
        &reports,
        inventory_query(Some(ServiceEpoch(6)), 100),
    )
    .expect("bounded inventory");
    assert_eq!(result.expected_sequences, 6);
    assert_eq!(result.unique_sequences, 2);
    assert_eq!(result.missing_sequences, 4);
    assert_eq!(result.missing_processors, 1);
    assert_eq!(result.processors[0].missing_sequences, 2);
    assert_eq!(result.processors[1].missing_sequences, 2);
    assert_eq!(result.state, AssessmentState::Incomplete);
    assert!(result.provenance_unverified);
}

// REQ: OA-TECHOPS-001, OA-TECHOPS-003
#[test]
fn inventory_retains_unexpected_duplicate_order_stale_future_loss_and_identity_findings() {
    let inventory = [expected(1, 1, 4)];
    let a = report(1, 1, 900);
    let mut b = report(1, 3, 1_001);
    b.source_id = SourceId(99);
    b.epoch = ServiceEpoch(8);
    b.clock_domain_id = ClockDomainId(8);
    b.release_manifest_sha256 = [5; 32];
    b.firmware_sha256 = [6; 32];
    b.running_image_sha256 = [7; 32];
    b.loaded_adaptation_sha256 = [8; 32];
    b.disk_adaptation_sha256 = [9; 32];
    b.telemetry_loss_events = 4;
    let c = report(1, 2, 950);
    let d = report(1, 2, 950);
    let unexpected = report(99, 1, 980);
    let result = assess_inventory(
        &inventory,
        &[a, b, c, d, unexpected],
        inventory_query(Some(ServiceEpoch(6)), 50),
    )
    .expect("bounded inventory");
    assert_eq!(result.unexpected_reports, 1);
    assert_eq!(result.duplicate_sequences, 1);
    assert_eq!(result.out_of_order_sequences, 1);
    assert_eq!(result.out_of_order_times, 2);
    assert_eq!(result.stale_rows, 1);
    assert_eq!(result.future_rows, 1);
    assert_eq!(result.telemetry_loss_events, 4);
    assert_eq!(result.source_mismatches, 1);
    assert_eq!(result.epoch_mismatches, 1);
    assert_eq!(result.clock_domain_mismatches, 1);
    assert_eq!(result.release_manifest_mismatches, 1);
    assert_eq!(result.firmware_mismatches, 1);
    assert_eq!(result.running_image_mismatches, 1);
    assert_eq!(result.loaded_adaptation_mismatches, 1);
    assert_eq!(result.disk_adaptation_mismatches, 1);
    assert_eq!(result.state, AssessmentState::IncompleteAndMismatch);
}

// REQ: OA-TECHOPS-001
#[test]
fn matching_inventory_claims_never_become_pass() {
    let inventory = [expected(1, 10, 12)];
    let result = assess_inventory(
        &inventory,
        &[report(1, 10, 950), report(1, 11, 980)],
        inventory_query(Some(ServiceEpoch(6)), 100),
    )
    .expect("bounded inventory");
    assert_eq!(result.state, AssessmentState::ClaimsMatchUnverified);
    assert!(result.provenance_unverified);
    assert_eq!(result.unique_sequences, result.expected_sequences);
}

// REQ: OA-ADAPT-003, OA-TECHOPS-001, OA-TECHOPS-002
#[test]
fn issued_adaptation_pin_is_checked_for_each_processor_at_startup_and_on_demand() {
    let mut expected_a = expected(1, 1, 2);
    let mut expected_b = expected(2, 1, 2);
    expected_a.adaptation_sha256 = [8; 32];
    expected_b.adaptation_sha256 = [8; 32];
    let inventory = [expected_a, expected_b];
    let mut query = inventory_query(Some(ServiceEpoch(6)), 100);
    query.release_manifest_sha256 = [8; 32];

    // Startup: every processor reports the same independently pinned issued manifest and digest.
    let mut startup_a = report(1, 1, 980);
    let mut startup_b = report(2, 1, 980);
    for row in [&mut startup_a, &mut startup_b] {
        row.release_manifest_sha256 = [8; 32];
        row.loaded_adaptation_sha256 = [8; 32];
        row.disk_adaptation_sha256 = [8; 32];
    }
    let startup = assess_inventory(&inventory, &[startup_a, startup_b], query)
        .expect("bounded startup inventory");
    assert_eq!(startup.state, AssessmentState::ClaimsMatchUnverified);
    assert_eq!(startup.release_manifest_mismatches, 0);
    assert_eq!(startup.loaded_adaptation_mismatches, 0);
    assert_eq!(startup.disk_adaptation_mismatches, 0);

    let mut on_demand_a = report(1, 1, 980);
    let mut on_demand_b = report(2, 1, 980);
    for row in [&mut on_demand_a, &mut on_demand_b] {
        row.release_manifest_sha256 = [8; 32];
        row.loaded_adaptation_sha256 = [8; 32];
        row.disk_adaptation_sha256 = [8; 32];
    }
    on_demand_b.disk_adaptation_sha256 = [10; 32];
    let on_demand = assess_inventory(&inventory, &[on_demand_a, on_demand_b], query)
        .expect("bounded on-demand inventory");
    assert_eq!(on_demand.expected_processor_count, 2);
    assert_eq!(on_demand.disk_adaptation_mismatches, 1);
    assert_eq!(on_demand.state, AssessmentState::Mismatch);
    assert_eq!(on_demand.processors[0].disk_adaptation_mismatches, 0);
    assert_eq!(on_demand.processors[1].disk_adaptation_mismatches, 1);
    assert!(on_demand.provenance_unverified);
}

// REQ: OA-TECHOPS-002
#[test]
fn invalid_monotonic_timestamp_is_counted_and_not_called_future_or_fresh() {
    let inventory = [expected(1, 10, 11)];
    let result = assess_inventory(
        &inventory,
        &[report(1, 10, -1)],
        inventory_query(Some(ServiceEpoch(6)), 100),
    )
    .expect("bounded inventory");
    assert_eq!(result.invalid_time_rows, 1);
    assert_eq!(result.future_rows, 0);
    assert_eq!(result.stale_rows, 0);
    assert_ne!(result.flags & FLAG_INVALID_TIME, 0);
    assert_eq!(result.state, AssessmentState::Incomplete);
}

// REQ: OA-TECHOPS-001, OA-TECHOPS-003
#[test]
fn epoch_history_unknown_and_regression_are_never_hidden() {
    let inventory = [expected(1, 10, 11)];
    let reports = [report(1, 10, 980)];
    let unknown = assess_inventory(&inventory, &reports, inventory_query(None, 100))
        .expect("bounded inventory");
    assert!(!unknown.epoch_order_checked);
    assert_eq!(unknown.state, AssessmentState::Incomplete);
    let regressed = assess_inventory(
        &inventory,
        &reports,
        inventory_query(Some(ServiceEpoch(8)), 100),
    )
    .expect("bounded inventory");
    assert!(regressed.epoch_order_checked);
    assert!(regressed.epoch_regressed);
    assert_eq!(regressed.state, AssessmentState::Mismatch);
}

// REQ: OA-TECHOPS-001, OA-TECHOPS-004, OA-TECHOPS-005
#[test]
fn inventory_refuses_duplicate_expected_ids_and_over_capacity_before_scan() {
    let duplicate = [expected(1, 1, 2), expected(1, 2, 3)];
    assert_eq!(
        assess_inventory(&duplicate, &[], inventory_query(Some(ServiceEpoch(6)), 100),),
        Err(StatusError::DuplicateExpectedProcessor)
    );
    let over = [expected(1, 0, 1); MAX_EXPECTED_PROCESSORS + 1];
    assert_eq!(
        assess_inventory(&over, &[], inventory_query(Some(ServiceEpoch(6)), 100),),
        Err(StatusError::ExpectedInventoryOverCapacity)
    );
    let invalid_span = [expected(1, 10, 10)];
    assert_eq!(
        assess_inventory(
            &invalid_span,
            &[],
            inventory_query(Some(ServiceEpoch(6)), 100),
        ),
        Err(StatusError::InvalidSequenceWindow)
    );
    let over_span = [expected(1, 0, MAX_SEQUENCE_SLOTS + 1)];
    assert_eq!(
        assess_inventory(&over_span, &[], inventory_query(Some(ServiceEpoch(6)), 100),),
        Err(StatusError::InvalidSequenceWindow)
    );
    let one = [expected(1, 1, 2)];
    let over_reports = std::vec![report(1, 1, 950); MAX_INVENTORY_REPORTS + 1];
    assert_eq!(
        assess_inventory(
            &one,
            &over_reports,
            inventory_query(Some(ServiceEpoch(6)), 100),
        ),
        Err(StatusError::ReportRowsOverCapacity)
    );
}

// REQ: OA-TECHOPS-002, OA-TECHOPS-003
#[test]
fn complete_metric_window_reports_denominator_and_unverified_limit_failure() {
    let contract = metric_contract(1, 3, 100);
    let samples = [metric_sample(1, 800, 20), metric_sample(2, 900, 101)];
    let result = assess_metric_window(&contract, &samples, 1_000, 500)
        .expect("closed bounded metric window");
    assert_eq!(result.expected_samples, 2);
    assert_eq!(result.reported_rows, 2);
    assert_eq!(result.unique_sequences, 2);
    assert_eq!(result.usable_samples, 2);
    assert_eq!(result.observed_maximum, 101);
    assert!(result.maximum_exceeded);
    assert_eq!(
        result.state,
        AssessmentState::ClaimedLimitExceededUnverified
    );
    assert!(result.provenance_unverified);
}

// REQ: OA-TECHOPS-002, OA-TECHOPS-004
#[test]
fn unexpected_processor_row_cannot_fill_an_expected_metric_sequence_slot() {
    let contract = metric_contract(1, 4, 100);
    let mut unexpected = metric_sample(2, 850, 30);
    unexpected.processor_id = ProcessorId(99);
    let result = assess_metric_window(
        &contract,
        &[
            metric_sample(1, 800, 20),
            metric_sample(3, 900, 40),
            unexpected,
        ],
        1_000,
        500,
    )
    .expect("bounded metric window");
    assert_eq!(result.expected_samples, 3);
    assert_eq!(result.unique_sequences, 2);
    assert_eq!(result.missing_samples, 1);
    assert_eq!(result.unexpected_samples, 1);
    assert_ne!(result.flags & FLAG_UNEXPECTED_PROCESSOR, 0);
    assert_eq!(result.state, AssessmentState::Incomplete);
}

// REQ: OA-TECHOPS-002, OA-TECHOPS-003
#[test]
fn metric_window_keeps_fault_rows_and_never_calls_partial_coverage_pass() {
    let contract = metric_contract(1, 5, 100);
    let mut stale = metric_sample(1, 800, 20);
    stale.telemetry_loss_events = 2;
    let future = metric_sample(2, 1_001, 30);
    let mut mismatch = metric_sample(3, 900, 150);
    mismatch.profile_id = ProfileId(5);
    mismatch.epoch = ServiceEpoch(8);
    let duplicate = metric_sample(3, 900, 40);
    let result = assess_metric_window(&contract, &[stale, future, mismatch, duplicate], 1_000, 100)
        .expect("bounded metric window");
    assert_eq!(result.expected_samples, 4);
    assert_eq!(result.unique_sequences, 3);
    assert_eq!(result.missing_samples, 1);
    assert_eq!(result.duplicate_samples, 1);
    assert_eq!(result.stale_samples, 1);
    assert_eq!(result.future_samples, 1);
    assert_eq!(result.profile_mismatches, 1);
    assert_eq!(result.epoch_mismatches, 1);
    assert_eq!(result.telemetry_loss_events, 2);
    assert_eq!(result.state, AssessmentState::IncompleteAndMismatch);
    assert!(result.provenance_unverified);
}

// REQ: OA-TECHOPS-002, OA-TECHOPS-004, OA-TECHOPS-005
#[test]
fn metric_contract_refuses_open_invalid_and_oversized_windows() {
    let samples = [];
    let mut contract = metric_contract(1, 2, 100);
    contract.window_end_ns = 1_001;
    assert_eq!(
        assess_metric_window(&contract, &samples, 1_000, 100),
        Err(StatusError::InvalidPerformanceWindow)
    );
    contract = metric_contract(4, 3, 100);
    assert_eq!(
        assess_metric_window(&contract, &samples, 1_000, 100),
        Err(StatusError::InvalidSequenceWindow)
    );
    contract = metric_contract(0, MAX_SEQUENCE_SLOTS + 1, 100);
    assert_eq!(
        assess_metric_window(&contract, &samples, 1_000, 100),
        Err(StatusError::InvalidSequenceWindow)
    );
    contract = metric_contract(1, 2, 100);
    let over_samples = std::vec![metric_sample(1, 200, 1); MAX_PERFORMANCE_SAMPLES + 1];
    assert_eq!(
        assess_metric_window(&contract, &over_samples, 1_000, 100),
        Err(StatusError::PerformanceSamplesOverCapacity)
    );
}

// REQ: OA-CERT-009
#[test]
fn fixed_target_map_retains_packet_coordinates_and_explicit_missing_values() {
    let mut target_id = [0; 32];
    target_id[..6].copy_from_slice(b"TGT-01");
    let missing = PverTargetMapRow {
        target_id,
        target_id_len: 6,
        state: PverTargetMapState::MissingOutput,
        output_time_ns: None,
        target_lat_e7: 400_000_000,
        target_lon_e7: -750_000_000,
        expected_lat_e7: None,
        expected_lon_e7: None,
        observed_lat_e7: None,
        observed_lon_e7: None,
        expected_east_um_s: 42_000_000,
        expected_north_um_s: 12_000_000,
        observed_east_um_s: None,
        observed_north_um_s: None,
        east_position_residual_um: None,
        north_position_residual_um: None,
        east_velocity_residual_um_s: None,
        north_velocity_residual_um_s: None,
        position_error_um: None,
        position_tolerance_um: 1_500_000_000,
        velocity_error_um_s: None,
        velocity_tolerance_um_s: 35_000_000,
    };
    assert_eq!(missing.state, PverTargetMapState::MissingOutput);
    assert_eq!(
        (missing.target_lat_e7, missing.target_lon_e7),
        (400_000_000, -750_000_000)
    );
    assert_eq!(missing.observed_lat_e7, None);
    assert_eq!(missing.position_error_um, None);

    let emitted = PverTargetMapRow {
        output_time_ns: Some(1_800_000_000_000_000_000),
        expected_lat_e7: Some(400_000_000),
        expected_lon_e7: Some(-750_000_000),
        observed_lat_e7: Some(400_000_001),
        observed_lon_e7: Some(-749_999_999),
        observed_east_um_s: Some(42_000_000),
        observed_north_um_s: Some(12_000_000),
        east_position_residual_um: Some(10_000),
        north_position_residual_um: Some(10_000),
        east_velocity_residual_um_s: Some(0),
        north_velocity_residual_um_s: Some(0),
        position_error_um: Some(14_142),
        velocity_error_um_s: Some(0),
        state: PverTargetMapState::Passed,
        ..missing
    };
    assert_eq!(emitted.state, PverTargetMapState::Passed);
    assert_eq!(emitted.observed_lat_e7, Some(400_000_001));
    assert_eq!(emitted.east_velocity_residual_um_s, Some(0));
}

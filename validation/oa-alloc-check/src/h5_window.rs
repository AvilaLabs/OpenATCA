#![forbid(unsafe_code)]
//! C5 allocation fixture for the H5 selector replay profile.

use crate::allocator::{self, Counts};
use oa_migration::{
    self as m1, AuthorityApproval, AuthorityRole, Comparison, EvidenceQuality, MetricCriterion,
    MetricEvidence, RollbackRule, RollbackTriggerPolicy, Stage, SwapUnitKey as M1Key,
    TransitionInput, TransitionPolicy,
};
use oa_migration_switch::{
    authorize_window, capacity_diff_budget_bytes, capacity_storage_budget_bytes, ingest_view,
    new_promotion_intent, new_selector_with_capacity, observe_delivery, prepare_candidate_into,
    rollback, select_candidate, AlertState, BusinessKey, CapacityProfile, Diff, EndpointClass,
    EntityFamily, EvidenceKind, FamilyCoverage, FieldStatus, FlightState, HandoffState, Health,
    Output, PromotionIntentRequest, RollbackKind, RollbackReceipt, Side, SnapshotView,
    SourceIdentity, SwapUnitKey, TrackState, UnitPolicy, REPLAY_COORDINATION_RECORDS,
    REPLAY_MAX_DIFFS, REPLAY_ROWS_PER_FAMILY,
};
use std::vec::Vec;

const START_NS: i64 = 1_000_000_000;
const CYCLES: u64 = 100;

struct RowFamilies<'a> {
    tracks: &'a [Option<TrackState>],
    flights: &'a [Option<FlightState>],
    handoffs: &'a [Option<HandoffState>],
    alerts: &'a [Option<AlertState>],
}

pub(super) struct H5Evidence {
    pub counts: Counts,
    pub rows_per_family: usize,
    pub cycles_completed: u64,
    pub trace_records: u16,
    pub coordination_records: u16,
    pub storage_budget_bytes: usize,
    pub diff_budget_bytes: usize,
    pub injected: bool,
}

fn policy() -> UnitPolicy {
    UnitPolicy {
        policy_version: 1,
        policy_digest: [3; 32],
        source_epoch_digest: [4; 32],
        update_interval_ns: 4_800_000_000,
        max_decision_delay_ns: 1_000_000_000,
        max_snapshot_age_ns: 100,
        evidence_set_handle: [10; 32],
        approval_set_handle: [11; 32],
        state_schema_digest: [12; 32],
        consumer_id: 1,
        endpoint_class: EndpointClass::Terminal,
        promotion_role_mask: 1,
        controller_rollback_role_mask: 1,
        health_rollback_role_mask: 1,
        track_field_mask: 1,
        flight_field_mask: 1,
        handoff_field_mask: 1,
        alert_field_mask: 1,
    }
}

fn unit() -> SwapUnitKey {
    SwapUnitKey {
        function_id: 1,
        scope_id: 2,
        window_start_ns: START_NS,
        window_end_ns: START_NS + 10_000,
    }
}

fn make_rows(stream: u32) -> Result<Vec<Option<TrackState>>, &'static str> {
    let mut rows = Vec::new();
    rows.try_reserve_exact(REPLAY_ROWS_PER_FAMILY)
        .map_err(|_| "H5 track fixture allocation failed")?;
    for ordinal in
        1..=u64::try_from(REPLAY_ROWS_PER_FAMILY).map_err(|_| "H5 track count conversion failed")?
    {
        rows.push(Some(TrackState {
            key: BusinessKey {
                entity_key: [9; 32],
                key_status: FieldStatus::Present,
                family: EntityFamily::Track,
                ordinal,
            },
            source_identity: SourceIdentity {
                stream,
                decoded_offset: ordinal,
                row: u32::try_from(ordinal).map_err(|_| "H5 track row conversion failed")?,
                operation_index: 0,
            },
            time_basis_id: 1,
            source_time_ns: START_NS + 5,
            observed_time_ns: START_NS + 5,
            status: FieldStatus::Present,
            lat_e7: 1,
            lon_e7: 2,
            altitude_ft: 3,
            speed_dkt: 4,
            track_ddeg: 5,
            flags: 0,
            field_mask: 1,
            projection_digest: [13; 32],
        }));
    }
    Ok(rows)
}

fn make_flight_rows(stream: u32) -> Result<Vec<Option<FlightState>>, &'static str> {
    let mut rows = Vec::new();
    rows.try_reserve_exact(REPLAY_ROWS_PER_FAMILY)
        .map_err(|_| "H5 flight fixture allocation failed")?;
    for ordinal in 1..=u64::try_from(REPLAY_ROWS_PER_FAMILY)
        .map_err(|_| "H5 flight count conversion failed")?
    {
        rows.push(Some(FlightState {
            key: BusinessKey {
                entity_key: [9; 32],
                key_status: FieldStatus::Present,
                family: EntityFamily::Flight,
                ordinal,
            },
            source_identity: SourceIdentity {
                stream,
                decoded_offset: ordinal,
                row: u32::try_from(ordinal).map_err(|_| "H5 flight row conversion failed")?,
                operation_index: 0,
            },
            time_basis_id: 1,
            source_time_ns: START_NS + 5,
            observed_time_ns: START_NS + 5,
            status: FieldStatus::Present,
            flight_plan_digest: [14; 32],
            flags: 0,
            field_mask: 1,
            projection_digest: [15; 32],
        }));
    }
    Ok(rows)
}

fn make_handoff_rows(stream: u32) -> Result<Vec<Option<HandoffState>>, &'static str> {
    let mut rows = Vec::new();
    rows.try_reserve_exact(REPLAY_ROWS_PER_FAMILY)
        .map_err(|_| "H5 handoff fixture allocation failed")?;
    for ordinal in 1..=u64::try_from(REPLAY_ROWS_PER_FAMILY)
        .map_err(|_| "H5 handoff count conversion failed")?
    {
        rows.push(Some(HandoffState {
            key: BusinessKey {
                entity_key: [9; 32],
                key_status: FieldStatus::Present,
                family: EntityFamily::Handoff,
                ordinal,
            },
            source_identity: SourceIdentity {
                stream,
                decoded_offset: ordinal,
                row: u32::try_from(ordinal).map_err(|_| "H5 handoff row conversion failed")?,
                operation_index: 0,
            },
            time_basis_id: 1,
            source_time_ns: START_NS + 5,
            observed_time_ns: START_NS + 5,
            status: FieldStatus::Present,
            lifecycle: 2,
            owner: 7,
            recipient: 8,
            flags: 0,
            field_mask: 1,
            projection_digest: [16; 32],
        }));
    }
    Ok(rows)
}

fn make_alert_rows(stream: u32) -> Result<Vec<Option<AlertState>>, &'static str> {
    let mut rows = Vec::new();
    rows.try_reserve_exact(REPLAY_ROWS_PER_FAMILY)
        .map_err(|_| "H5 alert fixture allocation failed")?;
    for ordinal in
        1..=u64::try_from(REPLAY_ROWS_PER_FAMILY).map_err(|_| "H5 alert count conversion failed")?
    {
        rows.push(Some(AlertState {
            key: BusinessKey {
                entity_key: [9; 32],
                key_status: FieldStatus::Present,
                family: EntityFamily::Alert,
                ordinal,
            },
            source_identity: SourceIdentity {
                stream,
                decoded_offset: ordinal,
                row: u32::try_from(ordinal).map_err(|_| "H5 alert row conversion failed")?,
                operation_index: 0,
            },
            time_basis_id: 1,
            source_time_ns: START_NS + 5,
            observed_time_ns: START_NS + 5,
            status: FieldStatus::Present,
            active: 1,
            acknowledged: 1,
            suppressed: 0,
            kind: 4,
            field_mask: 1,
            projection_digest: [17; 32],
        }));
    }
    Ok(rows)
}

fn snapshot_view<'a>(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    side: Side,
    rows: &'a RowFamilies<'a>,
    sequence: u64,
    now_ns: i64,
) -> SnapshotView<'a> {
    SnapshotView {
        unit,
        policy,
        evidence: EvidenceKind::Synthetic,
        side,
        source_epoch: if side == Side::Legacy { 11 } else { 12 },
        state_version: sequence,
        sequence,
        source_time_ns: now_ns,
        observed_time_ns: now_ns,
        health: Health::Healthy,
        track_coverage: FamilyCoverage::CompleteWithEvents,
        flight_coverage: FamilyCoverage::CompleteWithEvents,
        handoff_coverage: FamilyCoverage::CompleteWithEvents,
        alert_coverage: FamilyCoverage::CompleteWithEvents,
        tracks: rows.tracks,
        flights: rows.flights,
        handoffs: rows.handoffs,
        alerts: rows.alerts,
    }
}

fn authority(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evaluated_ns: i64,
) -> Result<m1::TransitionResult, &'static str> {
    let key = M1Key {
        function_id: unit.function_id,
        scope_id: unit.scope_id,
        window_start_ns: unit.window_start_ns,
        window_end_ns: unit.window_end_ns,
    };
    let authority_unit = m1::new_unit(
        key,
        policy.policy_version,
        policy.policy_digest,
        policy.source_epoch_digest,
        unit.window_start_ns,
    )
    .map_err(|_| "H5 M1 unit construction failed")?;
    let criteria = [MetricCriterion {
        metric_id: 4,
        comparison: Comparison::AtMost,
        threshold: 3,
        minimum_samples: 1,
        minimum_denominator: 1,
        minimum_exposure_ns: 1,
    }; m1::MAX_CRITERIA];
    let dependencies = [m1::DependencyRequirement {
        key,
        minimum_stage: Stage::S0Shadow,
        policy_version: policy.policy_version,
        policy_digest: policy.policy_digest,
        source_epoch_digest: policy.source_epoch_digest,
    }; m1::MAX_DEPENDENCIES];
    let rollback_policy = RollbackTriggerPolicy {
        blocking_difference: RollbackRule {
            enabled: true,
            maximum_response_ns: 1_000_000_000,
            required_role_mask: m1::ROLE_SUPERVISOR,
        },
        controller_request: RollbackRule {
            enabled: true,
            maximum_response_ns: 1_000_000_000,
            required_role_mask: m1::ROLE_CONTROLLER_ON_POSITION,
        },
        health_fault: RollbackRule {
            enabled: true,
            maximum_response_ns: 1_000_000_000,
            required_role_mask: m1::ROLE_TECH_OPS,
        },
    };
    let make_policy = |from, to| TransitionPolicy {
        key,
        from,
        to,
        policy_version: policy.policy_version,
        policy_digest: policy.policy_digest,
        source_epoch_digest: policy.source_epoch_digest,
        maximum_evidence_age_ns: 10_000_000_000,
        required_role_mask: m1::ROLE_SUPERVISOR | m1::ROLE_ASSESSOR,
        criterion_count: 1,
        criteria,
        dependency_count: 0,
        dependencies,
        rollback_policy,
    };
    let approvals = |from, to, at_ns| {
        [
            AuthorityApproval {
                key,
                from,
                to,
                role: AuthorityRole::FacilitySupervisor,
                approved: true,
                principal_id: 8,
                approved_at_ns: at_ns,
                policy_version: policy.policy_version,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                authority_record_handle: [21; 32],
            },
            AuthorityApproval {
                key,
                from,
                to,
                role: AuthorityRole::IndependentAssessor,
                approved: true,
                principal_id: 9,
                approved_at_ns: at_ns,
                policy_version: policy.policy_version,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                authority_record_handle: [22; 32],
            },
        ]
    };
    let metric = |stage, from_ns, end_ns| -> Result<[MetricEvidence; 1], &'static str> {
        let exposure_ns = u64::try_from(end_ns - from_ns).map_err(|_| "H5 M1 exposure invalid")?;
        Ok([MetricEvidence {
            key,
            measured_stage: stage,
            metric_id: 4,
            quality: EvidenceQuality::Complete,
            policy_version: policy.policy_version,
            samples: 5,
            denominator: 8,
            observed: 2,
            exposure_ns,
            observation_start_ns: from_ns,
            observation_end_ns: end_ns,
            observed_at_ns: end_ns,
            policy_digest: policy.policy_digest,
            source_epoch_digest: policy.source_epoch_digest,
            artifact_handle: [20; 32],
            evidence_set_handle: policy.evidence_set_handle,
            observation_origin_key: key,
            origin_stage_activation_ns: 0,
            origin_receipt_handle: [0; 32],
        }])
    };
    let first_at = unit.window_start_ns + 5;
    let first_policy = make_policy(Stage::S0Shadow, Stage::S1Advisory);
    let first_metric = metric(Stage::S0Shadow, unit.window_start_ns, first_at - 1)?;
    let first_approvals = approvals(Stage::S0Shadow, Stage::S1Advisory, first_at);
    let mut audit = m1::empty_audit_log();
    let first = m1::evaluate_transition(
        &authority_unit,
        TransitionInput {
            policy: &first_policy,
            metrics: &first_metric,
            dependencies: &[],
            approvals: &first_approvals,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            now_ns: first_at,
        },
        &mut audit,
    );
    if m1::transition_event(&first).refusal != m1::Refusal::None {
        return Err("H5 first M1 transition refused");
    }
    let second_policy = make_policy(Stage::S1Advisory, Stage::S2Window);
    let second_metric = metric(Stage::S1Advisory, first_at, evaluated_ns - 1)?;
    let second_approvals = approvals(Stage::S1Advisory, Stage::S2Window, evaluated_ns);
    let second = m1::evaluate_transition(
        &m1::transition_unit(&first),
        TransitionInput {
            policy: &second_policy,
            metrics: &second_metric,
            dependencies: &[],
            approvals: &second_approvals,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            now_ns: evaluated_ns,
        },
        &mut audit,
    );
    if m1::transition_event(&second).refusal != m1::Refusal::None {
        return Err("H5 second M1 transition refused");
    }
    Ok(second)
}

pub(super) fn h5_mirror_window(inject: bool) -> Result<H5Evidence, &'static str> {
    let unit = unit();
    let policy = policy();
    let legacy_rows = make_rows(1)?;
    let candidate_rows = make_rows(2)?;
    let legacy_flights = make_flight_rows(1)?;
    let candidate_flights = make_flight_rows(2)?;
    let legacy_handoffs = make_handoff_rows(1)?;
    let candidate_handoffs = make_handoff_rows(2)?;
    let legacy_alerts = make_alert_rows(1)?;
    let candidate_alerts = make_alert_rows(2)?;
    let legacy_families = RowFamilies {
        tracks: &legacy_rows,
        flights: &legacy_flights,
        handoffs: &legacy_handoffs,
        alerts: &legacy_alerts,
    };
    let candidate_families = RowFamilies {
        tracks: &candidate_rows,
        flights: &candidate_flights,
        handoffs: &candidate_handoffs,
        alerts: &candidate_alerts,
    };
    let mut selector = new_selector_with_capacity(
        unit,
        policy,
        EvidenceKind::Synthetic,
        START_NS,
        &[
            1001, 1002, 1003, 1004, 1005, 1006, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014,
            1015, 1016,
        ],
        CapacityProfile::REPLAY_4096,
    )
    .map_err(|_| "H5 replay selector construction failed")?;
    let mut diffs: Vec<Option<Diff>> = Vec::new();
    diffs
        .try_reserve_exact(REPLAY_MAX_DIFFS)
        .map_err(|_| "H5 diff allocation failed")?;
    diffs.resize(REPLAY_MAX_DIFFS, None);
    let mut now = START_NS + 10;
    ingest_view(
        &mut selector,
        snapshot_view(unit, policy, Side::Legacy, &legacy_families, 1, now),
        now,
        policy.max_snapshot_age_ns,
    )
    .map_err(|_| "H5 initial legacy ingest refused")?;
    ingest_view(
        &mut selector,
        snapshot_view(unit, policy, Side::Candidate, &candidate_families, 1, now),
        now,
        policy.max_snapshot_age_ns,
    )
    .map_err(|_| "H5 initial candidate ingest refused")?;
    let m1_result = authority(unit, policy, START_NS + 10)?;
    let authorization = authorize_window(&selector, &m1_result)
        .map_err(|_| "H5 M1 window authorization refused")?;
    let storage_budget_bytes = capacity_storage_budget_bytes(CapacityProfile::REPLAY_4096)
        .map_err(|_| "H5 storage budget overflow")?;
    let diff_budget_bytes = capacity_diff_budget_bytes(CapacityProfile::REPLAY_4096)
        .map_err(|_| "H5 diff budget overflow")?;
    if !allocator::start() {
        return Err("H5 allocation measurement failed to arm");
    }
    let mut cycles_completed = 0u64;
    let mut failed = false;
    let mut cycle = 0u64;
    while cycle < CYCLES {
        if cycle > 0 {
            now += 2;
            let sequence = cycle + 1;
            if ingest_view(
                &mut selector,
                snapshot_view(unit, policy, Side::Legacy, &legacy_families, sequence, now),
                now,
                policy.max_snapshot_age_ns,
            )
            .is_err()
                || ingest_view(
                    &mut selector,
                    snapshot_view(
                        unit,
                        policy,
                        Side::Candidate,
                        &candidate_families,
                        sequence,
                        now,
                    ),
                    now,
                    policy.max_snapshot_age_ns,
                )
                .is_err()
            {
                failed = true;
                break;
            }
        }
        if inject && cycle == 0 {
            if !allocator::inject_core_allocation() {
                failed = true;
                break;
            }
        }
        let (_, readiness) =
            match prepare_candidate_into(&selector, now, policy.max_snapshot_age_ns, &mut diffs) {
                Ok(result) => result,
                Err(_) => {
                    failed = true;
                    break;
                }
            };
        let authority_sequence = cycle * 2 + 1;
        let event_byte = match u8::try_from(authority_sequence) {
            Ok(value) if value != 0 => value,
            _ => {
                failed = true;
                break;
            }
        };
        let intent = match new_promotion_intent(
            &selector,
            &readiness,
            &authorization,
            PromotionIntentRequest {
                sequence: authority_sequence,
                evaluated_at_ns: now,
                principal_role_id: 1,
                principal_id: 8,
                event_key: [event_byte; 32],
            },
        ) {
            Ok(intent) => intent,
            Err(_) => {
                failed = true;
                break;
            }
        };
        if select_candidate(
            &mut selector,
            &readiness,
            &authorization,
            &intent,
            now,
            policy.max_snapshot_age_ns,
        )
        .is_err()
        {
            failed = true;
            break;
        }
        if observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Candidate,
            authority_sequence,
            now,
        )
        .is_err()
        {
            failed = true;
            break;
        }
        let rollback_sequence = authority_sequence + 1;
        let rollback_at = now + 1;
        let mut event_id = [7; 32];
        event_id[..8].copy_from_slice(&rollback_sequence.to_le_bytes());
        let receipt = RollbackReceipt {
            unit,
            kind: RollbackKind::ControllerRequest,
            principal_id: [8; 32],
            principal_role_id: 1,
            policy_version: policy.policy_version,
            policy_digest: policy.policy_digest,
            source_epoch_digest: policy.source_epoch_digest,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            sequence: rollback_sequence,
            legacy_epoch: 11,
            candidate_epoch: 12,
            legacy_state_version: cycle + 1,
            candidate_state_version: cycle + 1,
            legacy_sequence: cycle + 1,
            candidate_sequence: cycle + 1,
            legacy_health: Health::Healthy,
            legacy_source_available: true,
            legacy_health_observed_ns: rollback_at,
            event_id,
            observed_ns: rollback_at,
        };
        if rollback(
            &mut selector,
            receipt,
            rollback_at,
            policy.max_snapshot_age_ns,
        )
        .is_err()
            || observe_delivery(
                &mut selector,
                policy.consumer_id,
                Output::Legacy,
                rollback_sequence,
                rollback_at,
            )
            .is_err()
        {
            failed = true;
            break;
        }
        cycles_completed += 1;
        cycle += 1;
    }
    let injection_observed = !inject || allocator::inject_core_allocation();
    let counts = allocator::stop().ok_or("H5 allocation measurement failed to disarm")?;
    let (trace_records, trace_count) = oa_migration_switch::trace(&selector);
    let (notices, coordination_count) = oa_migration_switch::coordination_records(&selector);
    if failed
        || cycles_completed != CYCLES
        || trace_count != 200
        || coordination_count
            != u16::try_from(REPLAY_COORDINATION_RECORDS)
                .map_err(|_| "H5 receipt count conversion failed")?
        || trace_records
            .iter()
            .take(usize::from(trace_count))
            .flatten()
            .count()
            != 200
        || notices.len() != REPLAY_COORDINATION_RECORDS
        || counts.overflow
        || !injection_observed
        || (inject && counts.alloc == 0)
        || (!inject && (counts.alloc != 0 || counts.zeroed != 0 || counts.realloc != 0))
    {
        return Err("H5 replay profile did not preserve receipt capacity or allocation bound");
    }
    Ok(H5Evidence {
        counts,
        rows_per_family: REPLAY_ROWS_PER_FAMILY,
        cycles_completed,
        trace_records: trace_count,
        coordination_records: coordination_count,
        storage_budget_bytes,
        diff_budget_bytes,
        injected: true,
    })
}

#[cfg(test)]
#[test]
fn h5_replay_profile_has_zero_runtime_allocations() {
    let evidence = h5_mirror_window(false);
    assert!(evidence.is_ok());
    if let Ok(evidence) = evidence {
        assert_eq!(evidence.rows_per_family, REPLAY_ROWS_PER_FAMILY);
        assert_eq!(evidence.cycles_completed, CYCLES);
        assert_eq!(evidence.trace_records, 200);
        assert_eq!(
            evidence.coordination_records,
            REPLAY_COORDINATION_RECORDS as u16
        );
        assert_eq!(evidence.counts.alloc, 0);
        assert_eq!(evidence.counts.zeroed, 0);
        assert_eq!(evidence.counts.realloc, 0);
        assert!(!evidence.counts.overflow);
    }
}

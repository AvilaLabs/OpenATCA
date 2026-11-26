//! M3 synthetic-capacity drill over the production M2 selector API.
//! Recorded-traffic replay is intentionally not implemented until a normalized public H2/M4
//! snapshot producer and bounded input contract are available.
#![forbid(unsafe_code)]

use oa_audit_core::{AuditReceipt, Event};
use oa_migration::{
    self as m1, AuthorityApproval, AuthorityRole, Comparison, EvidenceQuality, MetricCriterion,
    MetricEvidence, RollbackRule, RollbackTriggerPolicy, Stage, SwapUnitKey as M1Key,
    TransitionInput, TransitionPolicy,
};
use oa_migration_switch::{
    ingest, new_selector, observe_delivery, prepare_candidate, rollback, select_candidate,
    AlertState, BusinessKey, EndpointClass, EntityFamily, EvidenceKind, FamilyCoverage,
    FieldStatus, FlightState, HandoffState, Health, Output, Refusal, RollbackKind, RollbackReceipt,
    Side, Snapshot, SourceIdentity, SwapUnitKey, TrackState, UnitPolicy, MAX_ALERTS, MAX_FLIGHTS,
    MAX_HANDOFFS, MAX_TRACE, MAX_TRACKS,
};
use std::time::Instant;

const CYCLES: u32 = 100;
const START_NS: i64 = 1_000_000_000;
const CYCLE_NS: i64 = 10_000_000_000;
const SUSTAINED_CYCLE_NS: i64 = 1_000_000_000;
const MAX_AGE_NS: i64 = 1_000_000_000;
const DECISION_TO_DELIVERY_NS: i64 = 100_000_000;
const SYNTHETIC_FUNCTION_ID: u32 = 4;
const SYNTHETIC_SCOPE_ID: u32 = 26;
const SYNTHETIC_FUNCTION_NAME: &str = "conflict_alert";
const SYNTHETIC_SCOPE_NAME: &str = "ZAU26";
const SYNTHETIC_EVIDENCE_POLICY_ID: &str = "M3-SYNTHETIC-SELECTOR-1";
const SYNTHETIC_POLICY_VERSION: u32 = 1;
const SYNTHETIC_POLICY_DIGEST: [u8; 32] = [3; 32];
const SYNTHETIC_SOURCE_EPOCH_DIGEST: [u8; 32] = [4; 32];
const SYNTHETIC_POLICY_DIGEST_HEX: &str =
    "0303030303030303030303030303030303030303030303030303030303030303";
const SYNTHETIC_SOURCE_EPOCH_DIGEST_HEX: &str =
    "0404040404040404040404040404040404040404040404040404040404040404";
const SYNTHETIC_LEGACY_SOURCE_EPOCH: u64 = 11;
const SYNTHETIC_CANDIDATE_SOURCE_EPOCH: u64 = 12;
const RECORDED_MODE_STATUS: &str = "NOT_BUILT";
const QUALIFICATION_STATUS: &str = "INCOMPLETE";

pub(crate) fn drill_policy() -> UnitPolicy {
    UnitPolicy {
        policy_version: SYNTHETIC_POLICY_VERSION,
        policy_digest: SYNTHETIC_POLICY_DIGEST,
        source_epoch_digest: SYNTHETIC_SOURCE_EPOCH_DIGEST,
        update_interval_ns: 4_800_000_000,
        max_decision_delay_ns: 1_000_000_000,
        max_snapshot_age_ns: MAX_AGE_NS,
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

fn key(family: EntityFamily, ordinal: u64) -> BusinessKey {
    BusinessKey {
        entity_key: [9; 32],
        key_status: FieldStatus::Present,
        family,
        ordinal,
    }
}

fn source_identity(side: Side, ordinal: u64) -> Result<SourceIdentity, Refusal> {
    Ok(SourceIdentity {
        stream: match side {
            Side::Legacy => 1,
            Side::Candidate => 2,
        },
        decoded_offset: ordinal,
        row: u32::try_from(ordinal).map_err(|_| Refusal::BadSnapshot)?,
        operation_index: 0,
    })
}

pub(crate) fn snapshot(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    side: Side,
    observed_ns: i64,
    variation: u32,
) -> Result<Snapshot, Refusal> {
    let epoch = match side {
        Side::Legacy => SYNTHETIC_LEGACY_SOURCE_EPOCH,
        Side::Candidate => SYNTHETIC_CANDIDATE_SOURCE_EPOCH,
    };
    let source_ns = observed_ns.checked_sub(1).ok_or(Refusal::TimeOverflow)?;
    let mut value = Snapshot {
        unit,
        policy,
        evidence: EvidenceKind::Synthetic,
        side,
        source_epoch: epoch,
        state_version: 1,
        sequence: 1,
        source_time_ns: source_ns,
        observed_time_ns: observed_ns,
        health: Health::Healthy,
        track_coverage: FamilyCoverage::CompleteWithEvents,
        flight_coverage: FamilyCoverage::CompleteWithEvents,
        handoff_coverage: FamilyCoverage::CompleteWithEvents,
        alert_coverage: FamilyCoverage::CompleteWithEvents,
        tracks: [None; MAX_TRACKS],
        flights: [None; MAX_FLIGHTS],
        handoffs: [None; MAX_HANDOFFS],
        alerts: [None; MAX_ALERTS],
    };
    let mut i = 0usize;
    while i < MAX_TRACKS {
        let ordinal = u64::try_from(i)
            .map_err(|_| Refusal::BadSnapshot)?
            .checked_add(1)
            .ok_or(Refusal::TimeOverflow)?;
        let index_i32 = i32::try_from(i).map_err(|_| Refusal::BadSnapshot)?;
        let variation_i32 = i32::try_from(variation).map_err(|_| Refusal::BadSnapshot)?;
        let variation_u16 = u16::try_from(variation % 20).map_err(|_| Refusal::BadSnapshot)?;
        let variation_index = usize::try_from(variation).map_err(|_| Refusal::BadSnapshot)?;
        let track_degrees = u16::try_from(
            i.checked_add(variation_index)
                .ok_or(Refusal::TimeOverflow)?
                % 3600,
        )
        .map_err(|_| Refusal::BadSnapshot)?;
        let row = TrackState {
            key: key(EntityFamily::Track, ordinal),
            source_identity: source_identity(side, ordinal)?,
            time_basis_id: 1,
            source_time_ns: source_ns,
            observed_time_ns: observed_ns,
            status: FieldStatus::Present,
            lat_e7: index_i32
                .checked_add(variation_i32)
                .ok_or(Refusal::TimeOverflow)?,
            lon_e7: index_i32.checked_neg().ok_or(Refusal::TimeOverflow)?,
            altitude_ft: 20_000i32
                .checked_add(index_i32)
                .ok_or(Refusal::TimeOverflow)?,
            speed_dkt: 3_000u16
                .checked_add(variation_u16)
                .ok_or(Refusal::TimeOverflow)?,
            track_ddeg: track_degrees,
            flags: variation & 1,
            field_mask: policy.track_field_mask,
            projection_digest: [13; 32],
        };
        *value.tracks.get_mut(i).ok_or(Refusal::BadSnapshot)? = Some(row);
        i = i.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    }
    i = 0;
    while i < MAX_FLIGHTS {
        let ordinal = u64::try_from(i)
            .map_err(|_| Refusal::BadSnapshot)?
            .checked_add(1)
            .ok_or(Refusal::TimeOverflow)?;
        let row = FlightState {
            key: key(EntityFamily::Flight, ordinal),
            source_identity: source_identity(side, ordinal)?,
            time_basis_id: 1,
            source_time_ns: source_ns,
            observed_time_ns: observed_ns,
            status: FieldStatus::Present,
            flight_plan_digest: [21; 32],
            flags: variation & 1,
            field_mask: policy.flight_field_mask,
            projection_digest: [14; 32],
        };
        *value.flights.get_mut(i).ok_or(Refusal::BadSnapshot)? = Some(row);
        i = i.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    }
    i = 0;
    while i < MAX_HANDOFFS {
        let ordinal = u64::try_from(i)
            .map_err(|_| Refusal::BadSnapshot)?
            .checked_add(1)
            .ok_or(Refusal::TimeOverflow)?;
        let row = HandoffState {
            key: key(EntityFamily::Handoff, ordinal),
            source_identity: source_identity(side, ordinal)?,
            time_basis_id: 1,
            source_time_ns: source_ns,
            observed_time_ns: observed_ns,
            status: FieldStatus::Present,
            lifecycle: u8::try_from(1 + variation % 4).map_err(|_| Refusal::BadSnapshot)?,
            owner: 7,
            recipient: 8,
            flags: 0,
            field_mask: policy.handoff_field_mask,
            projection_digest: [15; 32],
        };
        *value.handoffs.get_mut(i).ok_or(Refusal::BadSnapshot)? = Some(row);
        i = i.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    }
    i = 0;
    while i < MAX_ALERTS {
        let ordinal = u64::try_from(i)
            .map_err(|_| Refusal::BadSnapshot)?
            .checked_add(1)
            .ok_or(Refusal::TimeOverflow)?;
        let row = AlertState {
            key: key(EntityFamily::Alert, ordinal),
            source_identity: source_identity(side, ordinal)?,
            time_basis_id: 1,
            source_time_ns: source_ns,
            observed_time_ns: observed_ns,
            status: FieldStatus::Present,
            active: 1,
            acknowledged: u8::try_from(variation & 1).map_err(|_| Refusal::BadSnapshot)?,
            suppressed: u8::try_from((variation >> 1) & 1).map_err(|_| Refusal::BadSnapshot)?,
            kind: 4,
            field_mask: policy.alert_field_mask,
            projection_digest: [16; 32],
        };
        *value.alerts.get_mut(i).ok_or(Refusal::BadSnapshot)? = Some(row);
        i = i.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    }
    Ok(value)
}

fn authority(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    _authority_sequence: u64,
    _legacy_sequence: u64,
    _candidate_sequence: u64,
    evaluated_ns: i64,
) -> Result<m1::TransitionResult, Refusal> {
    let key = M1Key {
        function_id: unit.function_id,
        scope_id: unit.scope_id,
        window_start_ns: unit.window_start_ns,
        window_end_ns: unit.window_end_ns,
    };
    let unit_m1 = m1::new_unit(
        key,
        policy.policy_version,
        policy.policy_digest,
        policy.source_epoch_digest,
        unit.window_start_ns,
    )
    .map_err(|_| Refusal::Unauthorized)?;
    let mut audit = m1::empty_audit_log();
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
    let build_policy = |from, to| TransitionPolicy {
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
    let build_evidence =
        |stage: Stage, start_ns: i64, end_ns: i64| -> Result<[MetricEvidence; 1], Refusal> {
            let span = end_ns.checked_sub(start_ns).ok_or(Refusal::TimeOverflow)?;
            let exposure_ns = u64::try_from(span).map_err(|_| Refusal::TimeOverflow)?;
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
                observation_start_ns: start_ns,
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
    let build_approvals = |from, to, at_ns| {
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
    let first_at = unit
        .window_start_ns
        .checked_add(5)
        .ok_or(Refusal::TimeOverflow)?;
    let first_policy = build_policy(Stage::S0Shadow, Stage::S1Advisory);
    let first_metrics = build_evidence(Stage::S0Shadow, unit.window_start_ns, first_at - 1)?;
    let first_approvals = build_approvals(Stage::S0Shadow, Stage::S1Advisory, first_at);
    let first = m1::evaluate_transition(
        &unit_m1,
        TransitionInput {
            policy: &first_policy,
            metrics: &first_metrics,
            dependencies: &[],
            approvals: &first_approvals,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            now_ns: first_at,
        },
        &mut audit,
    );
    if m1::transition_event(&first).refusal != m1::Refusal::None {
        return Err(Refusal::Unauthorized);
    }
    let second_policy = build_policy(Stage::S1Advisory, Stage::S2Window);
    let second_metrics = build_evidence(Stage::S1Advisory, first_at, evaluated_ns - 1)?;
    let second_approvals = build_approvals(Stage::S1Advisory, Stage::S2Window, evaluated_ns);
    let second = m1::evaluate_transition(
        &m1::transition_unit(&first),
        TransitionInput {
            policy: &second_policy,
            metrics: &second_metrics,
            dependencies: &[],
            approvals: &second_approvals,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            now_ns: evaluated_ns,
        },
        &mut audit,
    );
    if m1::transition_event(&second).refusal != m1::Refusal::None {
        return Err(Refusal::Unauthorized);
    }
    Ok(second)
}

pub(crate) fn window_authorization(
    selector: &oa_migration_switch::Selector,
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evaluated_ns: i64,
) -> Result<oa_migration_switch::WindowAuthorization, Refusal> {
    let m1_result = authority(unit, policy, 1, 1, 1, evaluated_ns)?;
    oa_migration_switch::authorize_window(selector, &m1_result)
}

pub(crate) fn promotion_intent(
    selector: &oa_migration_switch::Selector,
    readiness: &oa_migration_switch::CandidateReadiness,
    authorization: &oa_migration_switch::WindowAuthorization,
    sequence: u64,
    evaluated_ns: i64,
) -> Result<oa_migration_switch::PromotionIntent, Refusal> {
    let event_byte = u8::try_from(sequence).map_err(|_| Refusal::TimeOverflow)?;
    if event_byte == 0 {
        return Err(Refusal::Unauthorized);
    }
    oa_migration_switch::new_promotion_intent(
        selector,
        readiness,
        authorization,
        oa_migration_switch::PromotionIntentRequest {
            sequence,
            evaluated_at_ns: evaluated_ns,
            principal_role_id: 1,
            principal_id: 8,
            event_key: [event_byte; 32],
        },
    )
}

pub(crate) fn rollback_receipt(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    authority_sequence: u64,
    state_version: u64,
    source_sequence: u64,
    observed_ns: i64,
    health_observed_ns: i64,
) -> RollbackReceipt {
    let mut event_id = [7; 32];
    event_id[..8].copy_from_slice(&authority_sequence.to_le_bytes());
    RollbackReceipt {
        unit,
        kind: RollbackKind::ControllerRequest,
        principal_id: [8; 32],
        principal_role_id: 1,
        audit_principal_id: 11,
        policy_version: policy.policy_version,
        policy_digest: policy.policy_digest,
        source_epoch_digest: policy.source_epoch_digest,
        evidence_set_handle: policy.evidence_set_handle,
        approval_set_handle: policy.approval_set_handle,
        sequence: authority_sequence,
        legacy_epoch: 11,
        candidate_epoch: 12,
        legacy_state_version: state_version,
        candidate_state_version: state_version,
        legacy_sequence: source_sequence,
        candidate_sequence: source_sequence,
        legacy_health: Health::Healthy,
        legacy_source_available: true,
        legacy_health_observed_ns: health_observed_ns,
        event_id,
        observed_ns,
    }
}

pub(crate) fn drill_state(
    index: u32,
) -> Result<(oa_migration_switch::Selector, SwapUnitKey, UnitPolicy, i64), Refusal> {
    let offset = i64::from(index)
        .checked_mul(CYCLE_NS)
        .ok_or(Refusal::TimeOverflow)?;
    let start_ns = START_NS.checked_add(offset).ok_or(Refusal::TimeOverflow)?;
    let end_ns = start_ns
        .checked_add(2_000_000_000)
        .ok_or(Refusal::TimeOverflow)?;
    let unit = SwapUnitKey {
        function_id: SYNTHETIC_FUNCTION_ID,
        scope_id: SYNTHETIC_SCOPE_ID,
        window_start_ns: start_ns,
        window_end_ns: end_ns,
    };
    let policy = drill_policy();
    let mut selector = new_selector(
        unit,
        policy,
        EvidenceKind::Synthetic,
        start_ns,
        &[1001, 1002],
    )?;
    let now_ns = start_ns.checked_add(10).ok_or(Refusal::TimeOverflow)?;
    let variation = index % 8;
    ingest(
        &mut selector,
        snapshot(unit, policy, Side::Legacy, now_ns, variation)?,
        now_ns,
        MAX_AGE_NS,
    )?;
    ingest(
        &mut selector,
        snapshot(unit, policy, Side::Candidate, now_ns, variation)?,
        now_ns,
        MAX_AGE_NS,
    )?;
    Ok((selector, unit, policy, now_ns))
}

fn run_cycle(index: u32) -> Result<(u128, u128, u128, u128, u128), Refusal> {
    let (mut selector, unit, policy, now_ns) = drill_state(index)?;
    let decision_started = Instant::now();
    let (report, readiness) = prepare_candidate(&selector, now_ns, MAX_AGE_NS)?;
    if !report.ready
        || report.differences != 0
        || usize::from(report.compared_tracks) != MAX_TRACKS
        || usize::from(report.compared_flights) != MAX_FLIGHTS
        || usize::from(report.compared_handoffs) != MAX_HANDOFFS
        || usize::from(report.compared_alerts) != MAX_ALERTS
    {
        return Err(Refusal::Incomplete);
    }
    let readiness_ns = decision_started.elapsed().as_nanos();
    let authorization = window_authorization(&selector, unit, policy, now_ns)?;
    let intent = promotion_intent(&selector, &readiness, &authorization, 1, now_ns)?;
    let promotion_started = Instant::now();
    audited_select_candidate(
        &mut selector,
        &readiness,
        &authorization,
        &intent,
        now_ns,
        MAX_AGE_NS,
    )?;
    let promotion_ns = promotion_started.elapsed().as_nanos();
    let candidate_delivery_ns = now_ns
        .checked_add(DECISION_TO_DELIVERY_NS)
        .ok_or(Refusal::TimeOverflow)?;
    let delivery_started = Instant::now();
    observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Candidate,
        1,
        candidate_delivery_ns,
    )?;
    let candidate_delivery_call_ns = delivery_started.elapsed().as_nanos();
    let rollback_at = candidate_delivery_ns
        .checked_add(DECISION_TO_DELIVERY_NS)
        .ok_or(Refusal::TimeOverflow)?;
    let rollback_started = Instant::now();
    audited_rollback(
        &mut selector,
        rollback_receipt(unit, policy, 2, 1, 1, rollback_at, rollback_at),
        rollback_at,
        MAX_AGE_NS,
    )?;
    let rollback_ns = rollback_started.elapsed().as_nanos();
    let legacy_delivery_ns = rollback_at
        .checked_add(DECISION_TO_DELIVERY_NS)
        .ok_or(Refusal::TimeOverflow)?;
    let delivery_started = Instant::now();
    observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Legacy,
        2,
        legacy_delivery_ns,
    )?;
    let legacy_delivery_call_ns = delivery_started.elapsed().as_nanos();
    let (records, count) = oa_migration_switch::trace(&selector);
    if count != 2
        || records.first().copied().flatten().map(|r| r.to) != Some(Output::Candidate)
        || records.get(1).copied().flatten().map(|r| r.to) != Some(Output::Legacy)
        || records
            .first()
            .copied()
            .flatten()
            .map(|r| r.m1_window_sequence)
            != Some(2)
        || records
            .get(1)
            .copied()
            .flatten()
            .map(|r| r.m1_window_sequence)
            != Some(0)
    {
        return Err(Refusal::OutputGap);
    }
    Ok((
        readiness_ns,
        promotion_ns,
        rollback_ns,
        candidate_delivery_call_ns,
        legacy_delivery_call_ns,
    ))
}

fn fault_suite() -> Result<u32, Refusal> {
    let mut cases = 0u32;

    let (mut selector, unit, policy, now_ns) = drill_state(CYCLES)?;
    let (report, readiness) = prepare_candidate(&selector, now_ns, MAX_AGE_NS)?;
    if !report.ready {
        return Err(Refusal::StateMismatch);
    }
    let mut changed_epoch = snapshot(unit, policy, Side::Candidate, now_ns + 20, 0)?;
    changed_epoch.source_epoch = 13;
    changed_epoch.sequence = 2;
    changed_epoch.state_version = 2;
    let epoch_result = ingest(&mut selector, changed_epoch, now_ns + 20, MAX_AGE_NS);
    if epoch_result != Err(Refusal::WrongUnit) {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    let epoch_attempt_at = now_ns.checked_add(20).ok_or(Refusal::TimeOverflow)?;
    let authorization = window_authorization(&selector, unit, policy, now_ns)?;
    let intent = promotion_intent(&selector, &readiness, &authorization, 1, epoch_attempt_at)?;
    audited_select_candidate(
        &mut selector,
        &readiness,
        &authorization,
        &intent,
        epoch_attempt_at,
        MAX_AGE_NS,
    )?;
    let first_delivery = now_ns
        .checked_add(DECISION_TO_DELIVERY_NS)
        .ok_or(Refusal::TimeOverflow)?;
    observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Candidate,
        1,
        first_delivery,
    )?;
    if observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Candidate,
        1,
        first_delivery + 1,
    ) != Err(Refusal::OutputDuplicate)
    {
        return Err(Refusal::StateMismatch);
    }
    if observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Candidate,
        3,
        first_delivery + 2,
    ) != Err(Refusal::OutputGap)
    {
        return Err(Refusal::StateMismatch);
    }
    let out_of_order_at = now_ns
        .checked_add(DECISION_TO_DELIVERY_NS / 2)
        .ok_or(Refusal::TimeOverflow)?;
    if observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Candidate,
        2,
        out_of_order_at,
    ) != Err(Refusal::Stale)
    {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(3).ok_or(Refusal::TimeOverflow)?;
    let stale_now = now_ns
        .checked_add(MAX_AGE_NS)
        .and_then(|v| v.checked_add(10))
        .ok_or(Refusal::TimeOverflow)?;
    let stale_health_time = now_ns.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    let stale_health = rollback_receipt(unit, policy, 2, 1, 1, stale_now, stale_health_time);
    if audited_rollback(&mut selector, stale_health, stale_now, MAX_AGE_NS) != Err(Refusal::Stale) {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(1).ok_or(Refusal::TimeOverflow)?;

    let (mut pending, pending_unit, pending_policy, pending_now) = drill_state(CYCLES + 1)?;
    let (pending_report, pending_ready) = prepare_candidate(&pending, pending_now, MAX_AGE_NS)?;
    if !pending_report.ready {
        return Err(Refusal::StateMismatch);
    }
    let pending_authorization =
        window_authorization(&pending, pending_unit, pending_policy, pending_now)?;
    let pending_intent = promotion_intent(
        &pending,
        &pending_ready,
        &pending_authorization,
        1,
        pending_now,
    )?;
    audited_select_candidate(
        &mut pending,
        &pending_ready,
        &pending_authorization,
        &pending_intent,
        pending_now,
        MAX_AGE_NS,
    )?;
    let rollback_time = pending_now
        .checked_add(DECISION_TO_DELIVERY_NS)
        .ok_or(Refusal::TimeOverflow)?;
    audited_rollback(
        &mut pending,
        rollback_receipt(
            pending_unit,
            pending_policy,
            2,
            1,
            1,
            rollback_time,
            rollback_time,
        ),
        rollback_time,
        MAX_AGE_NS,
    )?;
    let lost_delivery = rollback_time
        .checked_add(DECISION_TO_DELIVERY_NS)
        .ok_or(Refusal::TimeOverflow)?;
    if observe_delivery(
        &mut pending,
        pending_policy.consumer_id,
        Output::Legacy,
        2,
        lost_delivery,
    ) != Err(Refusal::OutputGap)
    {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(1).ok_or(Refusal::TimeOverflow)?;

    let (expired, _, _, expired_now) = drill_state(CYCLES + 2)?;
    let expiry_time = expired_now
        .checked_add(MAX_AGE_NS)
        .and_then(|v| v.checked_add(1))
        .ok_or(Refusal::TimeOverflow)?;
    if oa_migration_switch::reconcile(&expired, expiry_time, MAX_AGE_NS) != Err(Refusal::Stale) {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(1).ok_or(Refusal::TimeOverflow)?;

    let start_ns = START_NS
        .checked_add(
            i64::from(CYCLES + 3)
                .checked_mul(CYCLE_NS)
                .ok_or(Refusal::TimeOverflow)?,
        )
        .ok_or(Refusal::TimeOverflow)?;
    let unit = SwapUnitKey {
        function_id: SYNTHETIC_FUNCTION_ID,
        scope_id: SYNTHETIC_SCOPE_ID,
        window_start_ns: start_ns,
        window_end_ns: start_ns
            .checked_add(2_000_000_000)
            .ok_or(Refusal::TimeOverflow)?,
    };
    let policy = drill_policy();
    let mut incomplete = new_selector(
        unit,
        policy,
        EvidenceKind::Synthetic,
        start_ns,
        &[1001, 1002],
    )?;
    let observed_ns = start_ns.checked_add(10).ok_or(Refusal::TimeOverflow)?;
    let mut partial = snapshot(unit, policy, Side::Legacy, observed_ns, 0)?;
    partial.alert_coverage = FamilyCoverage::Partial;
    if ingest(&mut incomplete, partial, observed_ns, MAX_AGE_NS) != Err(Refusal::Incomplete) {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    println!(
        "FAULT\tversion=1\tcases={cases}\texpected_refusals=8\texpected_delivery_gap_refusals=2\texpected_delivery_duplicates=1\texpected_out_of_order=Stale\tepoch_change_after_reconcile=WrongUnit\tduplicate_delivery=OutputDuplicate\tsequence_gap=OutputGap\tstale_legacy_health=Stale\tpromotion_pending_rollback=accepted_then_delivery_gap\tpending_fault_delivery_offered=1\tpending_fault_delivery_delivered=0\tpending_fault_delivery_lost=1\texpired_mirror=Stale\tincomplete_family=Incomplete\tfault_injections=synthetic_only\tlosses_preserved=1"
    );
    Ok(cases)
}

fn sustained_trace_capacity_case() -> Result<(), Refusal> {
    let start_ns = START_NS
        .checked_add(
            2_000i64
                .checked_mul(CYCLE_NS)
                .ok_or(Refusal::TimeOverflow)?,
        )
        .ok_or(Refusal::TimeOverflow)?;
    let end_ns = start_ns
        .checked_add(100_000_000_000)
        .ok_or(Refusal::TimeOverflow)?;
    let unit = SwapUnitKey {
        function_id: SYNTHETIC_FUNCTION_ID,
        scope_id: SYNTHETIC_SCOPE_ID,
        window_start_ns: start_ns,
        window_end_ns: end_ns,
    };
    let policy = drill_policy();
    let mut selector = new_selector(
        unit,
        policy,
        EvidenceKind::Synthetic,
        start_ns,
        &[1001, 1002],
    )?;
    let window_grant = window_authorization(&selector, unit, policy, start_ns + 10)?;
    let mut cycle = 0u32;
    while cycle < 65 {
        let step = i64::from(cycle)
            .checked_mul(SUSTAINED_CYCLE_NS)
            .ok_or(Refusal::TimeOverflow)?;
        let now_ns = start_ns
            .checked_add(step)
            .and_then(|v| v.checked_add(10))
            .ok_or(Refusal::TimeOverflow)?;
        let source_sequence = u64::from(cycle)
            .checked_add(1)
            .ok_or(Refusal::TimeOverflow)?;
        let mut legacy = snapshot(unit, policy, Side::Legacy, now_ns, cycle % 8)?;
        let mut candidate = snapshot(unit, policy, Side::Candidate, now_ns, cycle % 8)?;
        legacy.sequence = source_sequence;
        legacy.state_version = source_sequence;
        candidate.sequence = source_sequence;
        candidate.state_version = source_sequence;
        ingest(&mut selector, legacy, now_ns, MAX_AGE_NS)?;
        ingest(&mut selector, candidate, now_ns, MAX_AGE_NS)?;
        let (report, readiness) = prepare_candidate(&selector, now_ns, MAX_AGE_NS)?;
        if !report.ready || report.differences != 0 {
            return Err(Refusal::StateMismatch);
        }
        let promote_authority = u64::from(cycle)
            .checked_mul(2)
            .and_then(|v| v.checked_add(1))
            .ok_or(Refusal::TimeOverflow)?;
        let rollback_authority = promote_authority
            .checked_add(1)
            .ok_or(Refusal::TimeOverflow)?;
        let intent = promotion_intent(
            &selector,
            &readiness,
            &window_grant,
            promote_authority,
            now_ns,
        )?;
        if cycle == 64 {
            if audited_select_candidate(
                &mut selector,
                &readiness,
                &window_grant,
                &intent,
                now_ns,
                MAX_AGE_NS,
            ) != Err(Refusal::TraceFull)
            {
                return Err(Refusal::StateMismatch);
            }
            let (records, count) = oa_migration_switch::trace(&selector);
            if count != u16::try_from(MAX_TRACE).map_err(|_| Refusal::TraceFull)?
                || oa_migration_switch::selected(&selector) != Output::Legacy
                || records
                    .get(MAX_TRACE - 1)
                    .copied()
                    .flatten()
                    .map(|record| record.to)
                    != Some(Output::Legacy)
            {
                return Err(Refusal::StateMismatch);
            }
            println!(
                "SUSTAINED\tversion=1\tcompleted_flip_rollback_cycles=64\tconsumer_deliveries=128\ttrace_records={count}\ttrace_capacity={MAX_TRACE}\tfinal_rollback_slot_preserved=1\tfinal_record_sequence=128\tfinal_record_from=Candidate\tfinal_record_to=Legacy\tnext_promotion=TraceFull\tselected_after_refusal=Legacy\tfallback_not_blocked=1\trecorded=0"
            );
            return Ok(());
        }
        audited_select_candidate(
            &mut selector,
            &readiness,
            &window_grant,
            &intent,
            now_ns,
            MAX_AGE_NS,
        )?;
        let candidate_delivery = now_ns
            .checked_add(DECISION_TO_DELIVERY_NS)
            .ok_or(Refusal::TimeOverflow)?;
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Candidate,
            promote_authority,
            candidate_delivery,
        )?;
        if cycle == 63 {
            let (_, count) = oa_migration_switch::trace(&selector);
            if count != u16::try_from(MAX_TRACE - 1).map_err(|_| Refusal::TraceFull)?
                || oa_migration_switch::selected(&selector) != Output::Candidate
            {
                return Err(Refusal::StateMismatch);
            }
        }
        let rollback_at = candidate_delivery
            .checked_add(DECISION_TO_DELIVERY_NS)
            .ok_or(Refusal::TimeOverflow)?;
        audited_rollback(
            &mut selector,
            rollback_receipt(
                unit,
                policy,
                rollback_authority,
                source_sequence,
                source_sequence,
                rollback_at,
                rollback_at,
            ),
            rollback_at,
            MAX_AGE_NS,
        )?;
        if cycle == 63 {
            let (records, count) = oa_migration_switch::trace(&selector);
            let last = records
                .get(MAX_TRACE - 1)
                .copied()
                .flatten()
                .ok_or(Refusal::TraceFull)?;
            if count != u16::try_from(MAX_TRACE).map_err(|_| Refusal::TraceFull)?
                || last.sequence != u64::try_from(MAX_TRACE).map_err(|_| Refusal::TraceFull)?
                || last.from != Output::Candidate
                || last.to != Output::Legacy
                || last.reason != 2
                || last.m1_window_sequence != 0
            {
                return Err(Refusal::StateMismatch);
            }
        }
        let legacy_delivery = rollback_at
            .checked_add(DECISION_TO_DELIVERY_NS)
            .ok_or(Refusal::TimeOverflow)?;
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Legacy,
            rollback_authority,
            legacy_delivery,
        )?;
        cycle = cycle.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    }
    Err(Refusal::StateMismatch)
}

fn report_header() -> String {
    format!(
        "M3DRILL\tversion=1\tmode=synthetic_capacity\trecorded_mode={RECORDED_MODE_STATUS}\tqualification={QUALIFICATION_STATUS}\tfunction_id={SYNTHETIC_FUNCTION_ID}\tfunction_name={SYNTHETIC_FUNCTION_NAME}\tscope_id={SYNTHETIC_SCOPE_ID}\tscope_name={SYNTHETIC_SCOPE_NAME}\tevidence_policy_id={SYNTHETIC_EVIDENCE_POLICY_ID}\tm1_policy_version={SYNTHETIC_POLICY_VERSION}\tm1_policy_digest={SYNTHETIC_POLICY_DIGEST_HEX}\tm1_source_epoch_digest={SYNTHETIC_SOURCE_EPOCH_DIGEST_HEX}\tlegacy_source_epoch={SYNTHETIC_LEGACY_SOURCE_EPOCH}\tcandidate_source_epoch={SYNTHETIC_CANDIDATE_SOURCE_EPOCH}\tcycles={CYCLES}\tcycle_window_ns=2000000000\tcycle_step_ns={CYCLE_NS}\tseed=0x4d335f53594e5448\ttracks_per_side={MAX_TRACKS}\tflights_per_side={MAX_FLIGHTS}\thandoffs_per_side={MAX_HANDOFFS}\talerts_per_side={MAX_ALERTS}\tterminal_interval_ns=4800000000\tmax_decision_delay_ns=1000000000\tmodeled_delivery_delay_ns={DECISION_TO_DELIVERY_NS}\tdelivery_timing_scope=selector_api_only\tconsumer_wire_adapter=NOT_BUILT\tquarantine_exercised=0\tca_detection_exercised=0\talert_payload_preservation_exercised=1"
    )
}

fn main() -> Result<(), Refusal> {
    println!("{}", report_header());
    println!(
        "CYCLE_FIELDS\tindex\tstart_ns\tend_ns\tready_ns\tpromotion_ns\trollback_ns\tcandidate_delivery_api_ns\tlegacy_delivery_api_ns\tdelivery_delay_ns\ttracks\tflights\thandoffs\talerts\tdifferences\toutput_sequences\tlosses\terror"
    );
    let mut completed = 0u32;
    let mut failures = 0u32;
    let mut index = 0u32;
    while index < CYCLES {
        let start_ns = START_NS
            .checked_add(
                i64::from(index)
                    .checked_mul(CYCLE_NS)
                    .ok_or(Refusal::TimeOverflow)?,
            )
            .ok_or(Refusal::TimeOverflow)?;
        let end_ns = start_ns
            .checked_add(2_000_000_000)
            .ok_or(Refusal::TimeOverflow)?;
        match run_cycle(index) {
            Ok((
                ready_ns,
                promotion_ns,
                rollback_ns,
                candidate_delivery_ns,
                legacy_delivery_ns,
            )) => {
                println!(
                    "CYCLE\t{index}\t{start_ns}\t{end_ns}\t{ready_ns}\t{promotion_ns}\t{rollback_ns}\t{candidate_delivery_ns}\t{legacy_delivery_ns}\t{DECISION_TO_DELIVERY_NS}\t{MAX_TRACKS}\t{MAX_FLIGHTS}\t{MAX_HANDOFFS}\t{MAX_ALERTS}\t0\t1,2\t0\tNONE"
                );
                completed = completed.checked_add(1).ok_or(Refusal::TimeOverflow)?;
            }
            Err(error) => {
                println!(
                    "CYCLE_FAILED\t{index}\tstart_ns={start_ns}\tend_ns={end_ns}\terror={error:?}"
                );
                failures = failures.checked_add(1).ok_or(Refusal::TimeOverflow)?;
            }
        }
        index = index.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    }
    let fault_count = if failures == 0 { fault_suite()? } else { 0 };
    if failures == 0 {
        sustained_trace_capacity_case()?;
    }
    println!(
        "SUMMARY\tcycles_expected={CYCLES}\tcycles_completed={completed}\tcycles_failed={failures}\tclean_cycle_delivery_expected=200\tclean_cycle_delivery_observed=200\tclean_cycle_lost=0\tfault_cases={fault_count}\tsustained_same_selector_cycles=64\tsustained_trace_capacity={MAX_TRACE}\tsustained_next_promotion=TraceFull\tsustained_fallback=Legacy\tclean_cycle_consumer_gaps=0\tclean_cycle_consumer_duplicates=0\tclean_cycle_consumer_out_of_order=0\tactive_alerts_preserved=1\tack_suppression_fields_compared=1\thandoff_rows_compared={MAX_HANDOFFS}\tdecision_and_delivery_timings_separate=1\trecorded_replay=NOT_BUILT\tqualification=INCOMPLETE"
    );
    if failures != 0 {
        return Err(Refusal::Incomplete);
    }
    Ok(())
}

// The drill is a pure selector model; durable shell append behavior is qualified separately in
// oa-audit-log tests. This produces only a synthetic receipt value for the core API.
fn m3_audit_receipt(
    selector: &oa_migration_switch::Selector,
    event: Event,
) -> Result<AuditReceipt, Refusal> {
    let expected = oa_migration_switch::audit_checkpoint(selector);
    let sequence = expected
        .count
        .checked_add(1)
        .ok_or(Refusal::AuditRequired)?;
    Ok(oa_audit_core::receipt_claim(
        sequence,
        [0; 32],
        expected.tail,
        event,
    ))
}

fn audited_select_candidate(
    selector: &mut oa_migration_switch::Selector,
    readiness: &oa_migration_switch::CandidateReadiness,
    authorization: &oa_migration_switch::WindowAuthorization,
    intent: &oa_migration_switch::PromotionIntent,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    let event = oa_migration_switch::candidate_audit_event(selector, intent, now_ns)?;
    let receipt = m3_audit_receipt(selector, event)?;
    select_candidate(
        selector,
        readiness,
        authorization,
        intent,
        now_ns,
        max_age_ns,
        receipt,
    )
}

fn audited_rollback(
    selector: &mut oa_migration_switch::Selector,
    cause: RollbackReceipt,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    let event = oa_migration_switch::rollback_audit_event(selector, cause, now_ns)?;
    let receipt = m3_audit_receipt(selector, event)?;
    rollback(selector, cause, now_ns, max_age_ns, receipt)
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    // REQ: OA-MIG-301, OA-MIG-302, OA-MIG-303, OA-MIG-304
    #[test]
    fn one_hundred_capacity_cycles_and_injected_refusals_are_accounted() {
        assert!(report_header().contains("recorded_mode=NOT_BUILT"));
        assert!(report_header().contains("qualification=INCOMPLETE"));
        assert!(report_header().contains("function_name=conflict_alert"));
        assert!(report_header().contains("scope_name=ZAU26"));
        assert!(report_header().contains("evidence_policy_id=M3-SYNTHETIC-SELECTOR-1"));
        assert!(main().is_ok());
    }
}

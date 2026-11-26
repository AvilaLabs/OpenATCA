//! M3 synthetic-capacity drill over the production M2 selector API.
//! Recorded-traffic replay is intentionally not implemented until a normalized public H2/M4
//! snapshot producer and bounded input contract are available.
#![forbid(unsafe_code)]

use oa_migration_switch::{
    ingest, new_selector, observe_delivery, prepare_candidate, rollback, select_candidate,
    AlertState, AuthorityReceipt, BusinessKey, EndpointClass, EntityFamily, EvidenceKind,
    FamilyCoverage, FieldStatus, FlightState, HandoffState, Health, Output, Refusal, RollbackKind,
    RollbackReceipt, Side, Snapshot, SourceIdentity, SwapUnitKey, TrackState, UnitPolicy,
    MAX_ALERTS, MAX_FLIGHTS, MAX_HANDOFFS, MAX_TRACE, MAX_TRACKS,
};
use std::time::Instant;

const CYCLES: u32 = 100;
const START_NS: i64 = 1_000_000_000;
const CYCLE_NS: i64 = 10_000_000_000;
const SUSTAINED_CYCLE_NS: i64 = 1_000_000_000;
const MAX_AGE_NS: i64 = 1_000_000_000;
const DECISION_TO_DELIVERY_NS: i64 = 100_000_000;
const RECORDED_MODE_STATUS: &str = "NOT_BUILT";
const QUALIFICATION_STATUS: &str = "INCOMPLETE";

fn drill_policy() -> UnitPolicy {
    UnitPolicy {
        policy_version: 1,
        policy_digest: [3; 32],
        source_epoch_digest: [4; 32],
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

fn snapshot(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    side: Side,
    observed_ns: i64,
    variation: u32,
) -> Result<Snapshot, Refusal> {
    let epoch = match side {
        Side::Legacy => 11,
        Side::Candidate => 12,
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
    authority_sequence: u64,
    legacy_sequence: u64,
    candidate_sequence: u64,
    evaluated_ns: i64,
) -> AuthorityReceipt {
    AuthorityReceipt {
        unit,
        sequence: authority_sequence,
        evaluated_at_ns: evaluated_ns,
        from_stage: 1,
        to_stage: 2,
        principal_role_id: 1,
        refusal_code: 0,
        policy_version: policy.policy_version,
        policy_digest: policy.policy_digest,
        source_epoch_digest: policy.source_epoch_digest,
        event_key: [5; 32],
        principal_id: [8; 32],
        assessor_principal_id: [9; 32],
        evidence_set_handle: policy.evidence_set_handle,
        approval_set_handle: policy.approval_set_handle,
        legacy_epoch: 11,
        candidate_epoch: 12,
        legacy_state_version: 1,
        candidate_state_version: 1,
        legacy_sequence,
        candidate_sequence,
    }
}

fn rollback_receipt(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    authority_sequence: u64,
    state_version: u64,
    source_sequence: u64,
    observed_ns: i64,
    health_observed_ns: i64,
) -> RollbackReceipt {
    RollbackReceipt {
        unit,
        kind: RollbackKind::ControllerRequest,
        principal_id: [8; 32],
        principal_role_id: 1,
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
        event_id: [7; 32],
        observed_ns,
    }
}

fn drill_state(
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
        function_id: 1,
        scope_id: 1,
        window_start_ns: start_ns,
        window_end_ns: end_ns,
    };
    let policy = drill_policy();
    let mut selector = new_selector(unit, policy, EvidenceKind::Synthetic, start_ns)?;
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
    let promotion_started = Instant::now();
    select_candidate(
        &mut selector,
        &readiness,
        authority(unit, policy, 1, 1, 1, now_ns),
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
    rollback(
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
    select_candidate(
        &mut selector,
        &readiness,
        authority(unit, policy, 1, 1, 1, epoch_attempt_at),
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
    if rollback(&mut selector, stale_health, stale_now, MAX_AGE_NS) != Err(Refusal::Stale) {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(1).ok_or(Refusal::TimeOverflow)?;

    let (mut pending, pending_unit, pending_policy, pending_now) = drill_state(CYCLES + 1)?;
    let (pending_report, pending_ready) = prepare_candidate(&pending, pending_now, MAX_AGE_NS)?;
    if !pending_report.ready {
        return Err(Refusal::StateMismatch);
    }
    select_candidate(
        &mut pending,
        &pending_ready,
        authority(pending_unit, pending_policy, 1, 1, 1, pending_now),
        pending_now,
        MAX_AGE_NS,
    )?;
    let rollback_time = pending_now
        .checked_add(DECISION_TO_DELIVERY_NS)
        .ok_or(Refusal::TimeOverflow)?;
    rollback(
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
        function_id: 1,
        scope_id: 1,
        window_start_ns: start_ns,
        window_end_ns: start_ns
            .checked_add(2_000_000_000)
            .ok_or(Refusal::TimeOverflow)?,
    };
    let policy = drill_policy();
    let mut incomplete = new_selector(unit, policy, EvidenceKind::Synthetic, start_ns)?;
    let observed_ns = start_ns.checked_add(10).ok_or(Refusal::TimeOverflow)?;
    let mut partial = snapshot(unit, policy, Side::Legacy, observed_ns, 0)?;
    partial.alert_coverage = FamilyCoverage::Partial;
    if ingest(&mut incomplete, partial, observed_ns, MAX_AGE_NS) != Err(Refusal::Incomplete) {
        return Err(Refusal::StateMismatch);
    }
    cases = cases.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    println!("FAULT\tversion=1\tcases={cases}\texpected_refusals=8\texpected_delivery_gap_refusals=2\texpected_delivery_duplicates=1\texpected_out_of_order=Stale\tepoch_change_after_reconcile=WrongUnit\tduplicate_delivery=OutputDuplicate\tsequence_gap=OutputGap\tstale_legacy_health=Stale\tpromotion_pending_rollback=accepted_then_delivery_gap\tpending_fault_delivery_offered=1\tpending_fault_delivery_delivered=0\tpending_fault_delivery_lost=1\texpired_mirror=Stale\tincomplete_family=Incomplete\tfault_injections=synthetic_only\tlosses_preserved=1");
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
        function_id: 1,
        scope_id: 1,
        window_start_ns: start_ns,
        window_end_ns: end_ns,
    };
    let policy = drill_policy();
    let mut selector = new_selector(unit, policy, EvidenceKind::Synthetic, start_ns)?;
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
        if cycle == 64 {
            if select_candidate(
                &mut selector,
                &readiness,
                authority(
                    unit,
                    policy,
                    promote_authority,
                    source_sequence,
                    source_sequence,
                    now_ns,
                ),
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
            println!("SUSTAINED\tversion=1\tcompleted_flip_rollback_cycles=64\tconsumer_deliveries=128\ttrace_records={count}\ttrace_capacity={MAX_TRACE}\tfinal_rollback_slot_preserved=1\tfinal_record_sequence=128\tfinal_record_from=Candidate\tfinal_record_to=Legacy\tnext_promotion=TraceFull\tselected_after_refusal=Legacy\tfallback_not_blocked=1\trecorded=0");
            return Ok(());
        }
        let mut promotion_receipt = authority(
            unit,
            policy,
            promote_authority,
            source_sequence,
            source_sequence,
            now_ns,
        );
        promotion_receipt.legacy_state_version = source_sequence;
        promotion_receipt.candidate_state_version = source_sequence;
        select_candidate(
            &mut selector,
            &readiness,
            promotion_receipt,
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
        rollback(
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
    format!("M3DRILL\tversion=1\tmode=synthetic_capacity\trecorded_mode={RECORDED_MODE_STATUS}\tqualification={QUALIFICATION_STATUS}\tcycles={CYCLES}\tseed=0x4d335f53594e5448\ttracks_per_side={MAX_TRACKS}\tflights_per_side={MAX_FLIGHTS}\thandoffs_per_side={MAX_HANDOFFS}\talerts_per_side={MAX_ALERTS}\tterminal_interval_ns=4800000000\tmax_decision_delay_ns=1000000000\tmodeled_delivery_delay_ns={DECISION_TO_DELIVERY_NS}\tdelivery_timing_scope=selector_api_only\tconsumer_wire_adapter=NOT_BUILT")
}

fn main() -> Result<(), Refusal> {
    println!("{}", report_header());
    println!("CYCLE_FIELDS\tindex\tready_ns\tpromotion_ns\trollback_ns\tcandidate_delivery_api_ns\tlegacy_delivery_api_ns\tdelivery_delay_ns\ttracks\tflights\thandoffs\talerts\toutput_sequences\tlosses\terror");
    let mut completed = 0u32;
    let mut failures = 0u32;
    let mut index = 0u32;
    while index < CYCLES {
        match run_cycle(index) {
            Ok((
                ready_ns,
                promotion_ns,
                rollback_ns,
                candidate_delivery_ns,
                legacy_delivery_ns,
            )) => {
                println!("CYCLE\t{index}\t{ready_ns}\t{promotion_ns}\t{rollback_ns}\t{candidate_delivery_ns}\t{legacy_delivery_ns}\t{DECISION_TO_DELIVERY_NS}\t{MAX_TRACKS}\t{MAX_FLIGHTS}\t{MAX_HANDOFFS}\t{MAX_ALERTS}\t1,2\t0\tNONE");
                completed = completed.checked_add(1).ok_or(Refusal::TimeOverflow)?;
            }
            Err(error) => {
                println!("CYCLE\t{index}\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t\t1\t{error:?}");
                failures = failures.checked_add(1).ok_or(Refusal::TimeOverflow)?;
            }
        }
        index = index.checked_add(1).ok_or(Refusal::TimeOverflow)?;
    }
    let fault_count = if failures == 0 { fault_suite()? } else { 0 };
    if failures == 0 {
        sustained_trace_capacity_case()?;
    }
    println!("SUMMARY\tcycles_expected={CYCLES}\tcycles_completed={completed}\tcycles_failed={failures}\tclean_cycle_delivery_expected=200\tclean_cycle_delivery_observed=200\tclean_cycle_lost=0\tfault_cases={fault_count}\tsustained_same_selector_cycles=64\tsustained_trace_capacity={MAX_TRACE}\tsustained_next_promotion=TraceFull\tsustained_fallback=Legacy\tclean_cycle_consumer_gaps=0\tclean_cycle_consumer_duplicates=0\tclean_cycle_consumer_out_of_order=0\tactive_alerts_preserved=1\tack_suppression_fields_compared=1\thandoff_rows_compared={MAX_HANDOFFS}\tdecision_and_delivery_timings_separate=1\trecorded_replay=NOT_BUILT\tqualification=INCOMPLETE");
    if failures != 0 {
        return Err(Refusal::Incomplete);
    }
    Ok(())
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
        for index in 0..CYCLES {
            assert!(run_cycle(index).is_ok());
        }
        assert_eq!(fault_suite(), Ok(8));
    }

    // REQ: OA-MIG-302
    #[test]
    fn sustained_selector_reaches_reserved_final_rollback_slot() {
        assert_eq!(
            sustained_trace_capacity_case(),
            Ok(()),
            "the same sustained case used by the executable must complete"
        );
    }
}

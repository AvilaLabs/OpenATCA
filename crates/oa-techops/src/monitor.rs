//! Bounded status/alarm view and control operations over caller-supplied values.
//!
//! This is a model-only core. It performs no I/O, reads no clock, authenticates no principal,
//! and invokes no shell callback. Adapter values remain self-reported and unauthenticated.
//! Display integration is outside this crate; Remote Maintenance Monitoring (RMM) remains UNMET.
//! Timestamps are nonnegative nanoseconds in caller-named monotonic domains; domains are never
//! converted or compared across identifiers.

use crate::{
    AssessmentState, ClockDomainId, InventoryAssessment, ProcessorAssessment,
    FLAG_CLOCK_DOMAIN_MISMATCH, FLAG_FUTURE, FLAG_INVALID_TIME, FLAG_STALE,
};

pub const MAX_MONITOR_RESOURCES: usize = 256;
pub const MAX_ALARMS: usize = 256;
pub const MAX_TRACKS_PER_SCAN: u32 = 1_000_000;
pub const MIN_LATENCY_LIMIT_NS: u64 = 1_000_000;
pub const MAX_LATENCY_LIMIT_NS: u64 = 60_000_000_000;
pub const REASON_STALE_OR_INCOMPARABLE_CLOCK: u16 = 1;
pub const REASON_INVENTORY_NO_REPORT: u16 = 2;
pub const REASON_INVENTORY_INCOMPLETE: u16 = 3;
pub const REASON_INVENTORY_MISMATCH_OR_LOSS: u16 = 4;
pub const REASON_INVENTORY_CLAIM_UNVERIFIED: u16 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ResourceKind {
    Service = 1,
    Processor = 2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ResourceState {
    Up = 1,
    Degraded = 2,
    Down = 3,
    Unknown = 4,
}

/// Full identity for one row. Numeric IDs are scoped by resource kind and parent service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ResourceKey {
    pub resource_id: u32,
    pub parent_service_id: u32,
    pub kind: ResourceKind,
}
const _: () = assert!(core::mem::size_of::<ResourceKey>() == 12);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AlarmSeverity {
    Informational = 1,
    Warning = 2,
    Critical = 3,
}

/// Plain authorization category supplied by the future adapter after its own registry check.
/// This core has no credentials, registry lookup, callbacks, or authentication mechanism.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MonitorRole {
    Monitor = 1,
    Operate = 2,
    Configure = 3,
}

/// Plain action intent for later adapter-side, append-only audit wiring. It is not an audit log,
/// authentication proof, or durable receipt; callers must persist/attribute it outside this core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MonitorAction {
    View = 1,
    Acknowledge = 2,
    ConfigureThresholds = 3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MonitorEventIntent {
    pub action: MonitorAction,
    pub role: MonitorRole,
    pub actor_id: u32,
    pub clock_domain: ClockDomainId,
    pub at_ns: i64,
    pub target_id: u64,
    pub reason_code: u16,
}
const _: () = assert!(core::mem::size_of::<MonitorEventIntent>() == 40);

pub const fn event_intent(
    action: MonitorAction,
    role: MonitorRole,
    actor_id: u32,
    clock_domain: ClockDomainId,
    at_ns: i64,
    target_id: u64,
    reason_code: u16,
) -> Result<MonitorEventIntent, MonitorError> {
    if actor_id == 0 || at_ns < 0 {
        return Err(MonitorError::InvalidTime);
    }
    let allowed = match action {
        MonitorAction::View => true,
        MonitorAction::Acknowledge => {
            matches!(role, MonitorRole::Operate | MonitorRole::Configure)
        }
        MonitorAction::ConfigureThresholds => matches!(role, MonitorRole::Configure),
    };
    if !allowed {
        return Err(MonitorError::WrongRole);
    }
    Ok(MonitorEventIntent {
        action,
        role,
        actor_id,
        clock_domain,
        at_ns,
        target_id,
        reason_code,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ResourceInput {
    pub resource_id: u32,
    pub parent_service_id: u32,
    pub kind: ResourceKind,
    pub reported_state: ResourceState,
    pub reason_code: u16,
    pub clock_domain: ClockDomainId,
    pub observed_at_ns: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ResourceOutput {
    pub resource_id: u32,
    pub parent_service_id: u32,
    pub kind: ResourceKind,
    pub state: ResourceState,
    pub reason_code: u16,
    pub age_ns: i64,
}
const _: () = assert!(core::mem::size_of::<ResourceInput>() == 24);
const _: () = assert!(core::mem::size_of::<ResourceOutput>() == 24);

/// Self-reported timing summary and observed tracks in one scan. Values are never synthesized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct LatencyClaim {
    pub clock_domain: ClockDomainId,
    pub sampled_at_ns: i64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    pub tracks_per_scan: u32,
}
const _: () = assert!(core::mem::size_of::<LatencyClaim>() == 56);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ThresholdConfig {
    pub p99_limit_ns: u64,
    pub tracks_per_scan_limit: u32,
}
const _: () = assert!(core::mem::size_of::<ThresholdConfig>() == 16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MonitorError {
    WrongRole,
    EmptyResources,
    ResourcesOverCapacity,
    InvalidTime,
    InvalidFreshness,
    InvalidThreshold,
    InvalidAssessment,
    DuplicateResourceIdentity,
    InvalidLatencyOrder,
    LatencyOverThreshold,
    TracksOverThreshold,
    InvalidAlarm,
    ClockDomainMismatch,
    AlarmCapacity,
    AlarmNotFound,
    AlreadyAcknowledged,
    CounterOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MonitorView {
    pub resource_count: u16,
    pub unknown_count: u16,
    pub degraded_count: u16,
}
const _: () = assert!(core::mem::size_of::<MonitorView>() == 6);

/// View a bounded snapshot. A stale or incomparable clock is `Unknown`, never `Down`. Duplicate
/// `(kind, parent_service_id, resource_id)` keys refuse before output is modified; the nested
/// uniqueness check performs at most 32,640 comparisons at the 256-row cap.
pub fn view_monitor(
    role: MonitorRole,
    inputs: &[ResourceInput],
    outputs: &mut [ResourceOutput],
    query_domain: ClockDomainId,
    query_time_ns: i64,
    freshness_ns: i64,
) -> Result<MonitorView, MonitorError> {
    if !matches!(
        role,
        MonitorRole::Monitor | MonitorRole::Operate | MonitorRole::Configure
    ) {
        return Err(MonitorError::WrongRole);
    }
    if inputs.is_empty() {
        return Err(MonitorError::EmptyResources);
    }
    if inputs.len() > MAX_MONITOR_RESOURCES || inputs.len() > outputs.len() {
        return Err(MonitorError::ResourcesOverCapacity);
    }
    if query_time_ns < 0 {
        return Err(MonitorError::InvalidTime);
    }
    if freshness_ns <= 0 {
        return Err(MonitorError::InvalidFreshness);
    }
    // Reject repeated full identities before touching the caller's output rows.
    for index in 0..inputs.len() {
        let Some(input) = inputs.get(index) else {
            return Err(MonitorError::ResourcesOverCapacity);
        };
        for previous_index in 0..index {
            let Some(previous) = inputs.get(previous_index) else {
                return Err(MonitorError::ResourcesOverCapacity);
            };
            if input.resource_id == previous.resource_id
                && input.parent_service_id == previous.parent_service_id
                && input.kind == previous.kind
            {
                return Err(MonitorError::DuplicateResourceIdentity);
            }
        }
    }
    let mut unknown = 0_u16;
    let mut degraded = 0_u16;
    for (input, output) in inputs.iter().zip(outputs.iter_mut()) {
        let age = if input.clock_domain == query_domain
            && input.observed_at_ns >= 0
            && input.observed_at_ns <= query_time_ns
        {
            query_time_ns
                .checked_sub(input.observed_at_ns)
                .ok_or(MonitorError::InvalidTime)?
        } else {
            -1
        };
        let fresh = age >= 0 && age <= freshness_ns;
        let state = if fresh {
            input.reported_state
        } else {
            ResourceState::Unknown
        };
        if state == ResourceState::Unknown {
            unknown = unknown
                .checked_add(1)
                .ok_or(MonitorError::CounterOverflow)?;
        }
        if state == ResourceState::Degraded {
            degraded = degraded
                .checked_add(1)
                .ok_or(MonitorError::CounterOverflow)?;
        }
        *output = ResourceOutput {
            resource_id: input.resource_id,
            parent_service_id: input.parent_service_id,
            kind: input.kind,
            state,
            reason_code: if fresh
                || (input.reported_state == ResourceState::Unknown && input.reason_code != 0)
            {
                input.reason_code
            } else {
                REASON_STALE_OR_INCOMPARABLE_CLOCK
            },
            age_ns: age,
        };
    }
    Ok(MonitorView {
        resource_count: inputs.len() as u16,
        unknown_count: unknown,
        degraded_count: degraded,
    })
}

/// Convert a real inventory assessment to model inputs. The service row uses the supplied
/// assessment time; processor rows preserve the newest valid sample time retained by the
/// assessment. A missing or incomparable processor time remains negative so `view_monitor`
/// produces `Unknown`; no source timestamp or clock domain is invented.
pub fn inventory_resource_inputs(
    assessment: &InventoryAssessment,
    service_id: u32,
    output: &mut [ResourceInput],
) -> Result<usize, MonitorError> {
    let processor_count = usize::try_from(assessment.processor_result_count)
        .map_err(|_| MonitorError::ResourcesOverCapacity)?;
    if processor_count != assessment.expected_processor_count as usize {
        return Err(MonitorError::InvalidAssessment);
    }
    let row_count = processor_count
        .checked_add(1)
        .ok_or(MonitorError::ResourcesOverCapacity)?;
    if service_id == 0 || assessment.query_time_ns < 0 {
        return Err(MonitorError::InvalidTime);
    }
    if processor_count > crate::MAX_EXPECTED_PROCESSORS
        || row_count > MAX_MONITOR_RESOURCES
        || row_count > output.len()
    {
        return Err(MonitorError::ResourcesOverCapacity);
    }
    let service_flags = assessment.flags;
    let service_state = classify_inventory(
        assessment.report_rows,
        service_flags,
        assessment.state,
        assessment.telemetry_loss_events,
    );
    let Some(service_slot) = output.get_mut(0) else {
        return Err(MonitorError::ResourcesOverCapacity);
    };
    *service_slot = ResourceInput {
        resource_id: service_id,
        parent_service_id: service_id,
        kind: ResourceKind::Service,
        reported_state: service_state.0,
        reason_code: service_state.1,
        clock_domain: assessment.clock_domain_id,
        observed_at_ns: assessment.query_time_ns,
    };
    for index in 0..processor_count {
        let Some(row) = assessment.processors.get(index).copied() else {
            return Err(MonitorError::ResourcesOverCapacity);
        };
        let classified = classify_processor(&row);
        let Some(slot) = output.get_mut(index + 1) else {
            return Err(MonitorError::ResourcesOverCapacity);
        };
        *slot = ResourceInput {
            resource_id: row.processor_id.0,
            parent_service_id: service_id,
            kind: ResourceKind::Processor,
            reported_state: classified.0,
            reason_code: classified.1,
            clock_domain: assessment.clock_domain_id,
            observed_at_ns: if row.clock_domain_mismatches > 0 || row.newest_sample_time_ns < 0 {
                -1
            } else {
                row.newest_sample_time_ns
            },
        };
    }
    Ok(row_count)
}

fn classify_processor(row: &ProcessorAssessment) -> (ResourceState, u16) {
    if row.report_rows == 0 {
        return (ResourceState::Unknown, REASON_INVENTORY_NO_REPORT);
    }
    if row.flags & (FLAG_STALE | FLAG_FUTURE | FLAG_INVALID_TIME | FLAG_CLOCK_DOMAIN_MISMATCH) != 0
    {
        return (ResourceState::Unknown, REASON_STALE_OR_INCOMPARABLE_CLOCK);
    }
    if row.flags != 0 || row.telemetry_loss_events > 0 {
        return (ResourceState::Degraded, REASON_INVENTORY_MISMATCH_OR_LOSS);
    }
    (ResourceState::Up, REASON_INVENTORY_CLAIM_UNVERIFIED)
}

fn classify_inventory(
    report_rows: u32,
    flags: u32,
    state: AssessmentState,
    loss_events: u64,
) -> (ResourceState, u16) {
    if report_rows == 0 {
        return (ResourceState::Unknown, REASON_INVENTORY_NO_REPORT);
    }
    if flags & (FLAG_STALE | FLAG_FUTURE | FLAG_INVALID_TIME | FLAG_CLOCK_DOMAIN_MISMATCH) != 0 {
        return (ResourceState::Unknown, REASON_STALE_OR_INCOMPARABLE_CLOCK);
    }
    match state {
        AssessmentState::ClaimsMatchUnverified if flags == 0 && loss_events == 0 => {
            (ResourceState::Up, REASON_INVENTORY_CLAIM_UNVERIFIED)
        }
        AssessmentState::Incomplete => (ResourceState::Degraded, REASON_INVENTORY_INCOMPLETE),
        _ => (ResourceState::Degraded, REASON_INVENTORY_MISMATCH_OR_LOSS),
    }
}

pub fn configure_thresholds(
    role: MonitorRole,
    config: ThresholdConfig,
) -> Result<(), MonitorError> {
    if role != MonitorRole::Configure {
        return Err(MonitorError::WrongRole);
    }
    if !(MIN_LATENCY_LIMIT_NS..=MAX_LATENCY_LIMIT_NS).contains(&config.p99_limit_ns)
        || config.tracks_per_scan_limit == 0
        || config.tracks_per_scan_limit > MAX_TRACKS_PER_SCAN
    {
        return Err(MonitorError::InvalidThreshold);
    }
    Ok(())
}

/// Validate a fresh supplied B3 metric claim against its configured limits.
pub fn validate_latency_claim(
    claim: LatencyClaim,
    domain: ClockDomainId,
    now_ns: i64,
    freshness_ns: i64,
    limits: ThresholdConfig,
) -> Result<(), MonitorError> {
    configure_thresholds(MonitorRole::Configure, limits)?;
    if now_ns < 0 || claim.sampled_at_ns < 0 || claim.sampled_at_ns > now_ns || freshness_ns <= 0 {
        return Err(MonitorError::InvalidTime);
    }
    if claim.clock_domain != domain || now_ns - claim.sampled_at_ns > freshness_ns {
        return Err(MonitorError::InvalidFreshness);
    }
    if claim.p50_ns > claim.p95_ns || claim.p95_ns > claim.p99_ns || claim.p99_ns > claim.max_ns {
        return Err(MonitorError::InvalidLatencyOrder);
    }
    if claim.p99_ns > limits.p99_limit_ns {
        return Err(MonitorError::LatencyOverThreshold);
    }
    if claim.tracks_per_scan > limits.tracks_per_scan_limit {
        return Err(MonitorError::TracksOverThreshold);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AlarmState {
    Empty = 0,
    Active = 1,
    Cleared = 2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Alarm {
    pub instance_id: u64,
    pub resource: ResourceKey,
    pub reason_code: u16,
    pub severity: AlarmSeverity,
    pub state: AlarmState,
    pub clock_domain: ClockDomainId,
    pub raised_at_ns: i64,
    pub cleared_at_ns: i64,
    pub acknowledged_at_ns: i64,
}
const _: () = assert!(core::mem::size_of::<Alarm>() == 56);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AlarmTime {
    pub clock_domain: ClockDomainId,
    pub at_ns: i64,
}
const _: () = assert!(core::mem::size_of::<AlarmTime>() == 16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AlarmTransition {
    Raised,
    Unchanged,
    Cleared,
}

/// Apply an explicit alarm-state observation. All timestamps for an instance use its fixed
/// caller-supplied monotonic clock domain; a persistent instance cannot be raised twice.
pub fn record_alarm_transition(
    alarms: &mut [Alarm],
    instance_id: u64,
    resource: ResourceKey,
    reason_code: u16,
    severity: AlarmSeverity,
    active: bool,
    time: AlarmTime,
) -> Result<AlarmTransition, MonitorError> {
    let AlarmTime {
        clock_domain,
        at_ns,
    } = time;
    if alarms.len() > MAX_ALARMS {
        return Err(MonitorError::AlarmCapacity);
    }
    if instance_id == 0 || resource.resource_id == 0 || resource.parent_service_id == 0 || at_ns < 0
    {
        return Err(MonitorError::InvalidAlarm);
    }
    if let Some(alarm) = alarms
        .iter_mut()
        .find(|a| a.instance_id == instance_id && a.state != AlarmState::Empty)
    {
        if alarm.resource != resource || alarm.reason_code != reason_code {
            return Err(MonitorError::InvalidAlarm);
        }
        if alarm.clock_domain != clock_domain {
            return Err(MonitorError::ClockDomainMismatch);
        }
        if alarm.state == AlarmState::Cleared {
            if at_ns < alarm.cleared_at_ns {
                return Err(MonitorError::InvalidTime);
            }
            return Ok(AlarmTransition::Unchanged);
        }
        if active {
            if at_ns < alarm.raised_at_ns
                || (alarm.acknowledged_at_ns >= 0 && at_ns < alarm.acknowledged_at_ns)
            {
                return Err(MonitorError::InvalidTime);
            }
            return Ok(AlarmTransition::Unchanged);
        }
        if at_ns < alarm.raised_at_ns
            || (alarm.acknowledged_at_ns >= 0 && at_ns < alarm.acknowledged_at_ns)
        {
            return Err(MonitorError::InvalidTime);
        }
        alarm.state = AlarmState::Cleared;
        alarm.cleared_at_ns = at_ns;
        return Ok(AlarmTransition::Cleared);
    }
    if !active {
        return Ok(AlarmTransition::Unchanged);
    }
    let Some(slot) = alarms.iter_mut().find(|a| a.state == AlarmState::Empty) else {
        return Err(MonitorError::AlarmCapacity);
    };
    *slot = Alarm {
        instance_id,
        resource,
        reason_code,
        severity,
        state: AlarmState::Active,
        clock_domain,
        raised_at_ns: at_ns,
        cleared_at_ns: -1,
        acknowledged_at_ns: -1,
    };
    Ok(AlarmTransition::Raised)
}

/// Acknowledge one active instance in the same explicit time domain used when it was raised.
pub fn ack_alarm(
    role: MonitorRole,
    alarms: &mut [Alarm],
    instance_id: u64,
    clock_domain: ClockDomainId,
    at_ns: i64,
) -> Result<(), MonitorError> {
    if !matches!(role, MonitorRole::Operate | MonitorRole::Configure) {
        return Err(MonitorError::WrongRole);
    }
    if at_ns < 0 || alarms.len() > MAX_ALARMS {
        return Err(MonitorError::InvalidAlarm);
    }
    let Some(alarm) = alarms
        .iter_mut()
        .find(|a| a.instance_id == instance_id && a.state == AlarmState::Active)
    else {
        return Err(MonitorError::AlarmNotFound);
    };
    if alarm.clock_domain != clock_domain {
        return Err(MonitorError::ClockDomainMismatch);
    }
    if alarm.acknowledged_at_ns >= 0 {
        return Err(MonitorError::AlreadyAcknowledged);
    }
    if at_ns < alarm.raised_at_ns {
        return Err(MonitorError::InvalidTime);
    }
    alarm.acknowledged_at_ns = at_ns;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const EMPTY_ALARM: Alarm = Alarm {
        instance_id: 0,
        resource: ResourceKey {
            resource_id: 0,
            parent_service_id: 0,
            kind: ResourceKind::Service,
        },
        reason_code: 0,
        severity: AlarmSeverity::Informational,
        state: AlarmState::Empty,
        clock_domain: ClockDomainId(0),
        raised_at_ns: 0,
        cleared_at_ns: 0,
        acknowledged_at_ns: 0,
    };
    const TEST_SERVICE_RESOURCE: ResourceKey = ResourceKey {
        resource_id: 1,
        parent_service_id: 1,
        kind: ResourceKind::Service,
    };
    // REQ: OA-L23-001
    #[test]
    fn monitor_states_and_stale_clock_become_unknown() {
        let input = [
            ResourceInput {
                resource_id: 1,
                parent_service_id: 10,
                kind: ResourceKind::Service,
                reported_state: ResourceState::Up,
                reason_code: 0,
                clock_domain: ClockDomainId(7),
                observed_at_ns: 200,
            },
            ResourceInput {
                resource_id: 2,
                parent_service_id: 10,
                kind: ResourceKind::Processor,
                reported_state: ResourceState::Degraded,
                reason_code: 9,
                clock_domain: ClockDomainId(7),
                observed_at_ns: 150,
            },
            ResourceInput {
                resource_id: 3,
                parent_service_id: 10,
                kind: ResourceKind::Processor,
                reported_state: ResourceState::Down,
                reason_code: 4,
                clock_domain: ClockDomainId(8),
                observed_at_ns: 100,
            },
            ResourceInput {
                resource_id: 4,
                parent_service_id: 10,
                kind: ResourceKind::Processor,
                reported_state: ResourceState::Down,
                reason_code: 11,
                clock_domain: ClockDomainId(7),
                observed_at_ns: 100,
            },
            ResourceInput {
                resource_id: 5,
                parent_service_id: 10,
                kind: ResourceKind::Processor,
                reported_state: ResourceState::Down,
                reason_code: 12,
                clock_domain: ClockDomainId(7),
                observed_at_ns: 200,
            },
        ];
        let mut out = [ResourceOutput {
            resource_id: 0,
            parent_service_id: 0,
            kind: ResourceKind::Service,
            state: ResourceState::Unknown,
            reason_code: 0,
            age_ns: 0,
        }; 5];
        let summary = view_monitor(
            MonitorRole::Monitor,
            &input,
            &mut out,
            ClockDomainId(7),
            200,
            50,
        )
        .unwrap();
        assert_eq!((summary.unknown_count, summary.degraded_count), (2, 1));
        assert_eq!(out[2].state, ResourceState::Unknown);
        assert_eq!(out[2].reason_code, REASON_STALE_OR_INCOMPARABLE_CLOCK);
        assert_eq!(out[3].state, ResourceState::Unknown);
        assert_eq!(out[4].state, ResourceState::Down);
        let mut repeated_out = out;
        let repeated = view_monitor(
            MonitorRole::Monitor,
            &input,
            &mut repeated_out,
            ClockDomainId(7),
            200,
            50,
        )
        .unwrap();
        assert_eq!(repeated, summary);
        assert_eq!(repeated_out, out);

        let expected = [
            crate::ExpectedProcessor {
                processor_id: crate::ProcessorId(11),
                source_id: crate::SourceId(21),
                firmware_sha256: [1; 32],
                running_image_sha256: [2; 32],
                adaptation_sha256: [3; 32],
                sequence_first: 1,
                sequence_next: 2,
            },
            crate::ExpectedProcessor {
                processor_id: crate::ProcessorId(12),
                source_id: crate::SourceId(22),
                firmware_sha256: [1; 32],
                running_image_sha256: [2; 32],
                adaptation_sha256: [3; 32],
                sequence_first: 1,
                sequence_next: 2,
            },
            crate::ExpectedProcessor {
                processor_id: crate::ProcessorId(13),
                source_id: crate::SourceId(23),
                firmware_sha256: [1; 32],
                running_image_sha256: [2; 32],
                adaptation_sha256: [3; 32],
                sequence_first: 1,
                sequence_next: 2,
            },
            crate::ExpectedProcessor {
                processor_id: crate::ProcessorId(14),
                source_id: crate::SourceId(24),
                firmware_sha256: [1; 32],
                running_image_sha256: [2; 32],
                adaptation_sha256: [3; 32],
                sequence_first: 1,
                sequence_next: 2,
            },
        ];
        let report = |id: u32, source: u32, loss: u32, sample_time: i64| crate::ProcessorReport {
            processor_id: crate::ProcessorId(id),
            source_id: crate::SourceId(source),
            epoch: crate::ServiceEpoch(2),
            clock_domain_id: ClockDomainId(9),
            sequence: 1,
            sample_time_ns: sample_time,
            release_manifest_sha256: [4; 32],
            firmware_sha256: [1; 32],
            running_image_sha256: [2; 32],
            loaded_adaptation_sha256: [3; 32],
            disk_adaptation_sha256: [3; 32],
            telemetry_loss_events: loss,
        };
        let reports = [
            report(11, 21, 0, 950),
            report(12, 22, 1, 950),
            report(13, 23, 0, 800),
        ];
        let assessment = crate::assess_inventory(
            &expected,
            &reports,
            crate::InventoryQuery {
                previous_epoch: Some(crate::ServiceEpoch(1)),
                epoch: crate::ServiceEpoch(2),
                clock_domain_id: ClockDomainId(9),
                release_manifest_sha256: [4; 32],
                query_time_ns: 1_000,
                freshness_limit_ns: 100,
            },
        )
        .unwrap();
        let mut inventory_rows = [input[0]; 5];
        assert_eq!(
            inventory_resource_inputs(&assessment, 77, &mut inventory_rows),
            Ok(5)
        );
        assert_eq!(inventory_rows[1].observed_at_ns, 950);
        assert_eq!(inventory_rows[3].observed_at_ns, 800);
        let mut inventory_view = [out[0]; 5];
        let summary = view_monitor(
            MonitorRole::Monitor,
            &inventory_rows,
            &mut inventory_view,
            ClockDomainId(9),
            1_000,
            100,
        )
        .unwrap();
        assert_eq!(summary.unknown_count, 3); // service, stale processor, and absent processor
        assert_eq!(summary.degraded_count, 1);
        assert_eq!(inventory_view[1].state, ResourceState::Up);
        assert_eq!(inventory_view[2].state, ResourceState::Degraded);
        assert_eq!(inventory_view[3].state, ResourceState::Unknown);
        assert_eq!(
            inventory_view[3].reason_code,
            REASON_STALE_OR_INCOMPARABLE_CLOCK
        );
        assert_eq!(inventory_view[4].state, ResourceState::Unknown);
        assert_eq!(inventory_view[4].reason_code, REASON_INVENTORY_NO_REPORT);

        let one = crate::ExpectedProcessor {
            processor_id: crate::ProcessorId(1),
            source_id: crate::SourceId(2),
            firmware_sha256: [1; 32],
            running_image_sha256: [2; 32],
            adaptation_sha256: [3; 32],
            sequence_first: 0,
            sequence_next: 1,
        };
        let one_report = crate::ProcessorReport {
            processor_id: crate::ProcessorId(1),
            source_id: crate::SourceId(2),
            epoch: crate::ServiceEpoch(2),
            clock_domain_id: ClockDomainId(9),
            sequence: 0,
            sample_time_ns: 100,
            release_manifest_sha256: [4; 32],
            firmware_sha256: [1; 32],
            running_image_sha256: [2; 32],
            loaded_adaptation_sha256: [3; 32],
            disk_adaptation_sha256: [3; 32],
            telemetry_loss_events: 0,
        };
        let same_id_assessment = crate::assess_inventory(
            &[one],
            &[one_report],
            crate::InventoryQuery {
                previous_epoch: Some(crate::ServiceEpoch(1)),
                epoch: crate::ServiceEpoch(2),
                clock_domain_id: ClockDomainId(9),
                release_manifest_sha256: [4; 32],
                query_time_ns: 100,
                freshness_limit_ns: 20,
            },
        )
        .unwrap();
        let mut same_id_rows = [input[0]; 2];
        assert_eq!(
            inventory_resource_inputs(&same_id_assessment, 1, &mut same_id_rows),
            Ok(2)
        );
        assert_eq!(same_id_rows[0].resource_id, same_id_rows[1].resource_id);
        assert_eq!(
            same_id_rows[0].parent_service_id,
            same_id_rows[1].parent_service_id
        );
        assert_ne!(same_id_rows[0].kind, same_id_rows[1].kind);
        let mut same_id_view = [out[0]; 2];
        assert_eq!(
            view_monitor(
                MonitorRole::Monitor,
                &same_id_rows,
                &mut same_id_view,
                ClockDomainId(9),
                100,
                20,
            ),
            Ok(MonitorView {
                resource_count: 2,
                unknown_count: 0,
                degraded_count: 0,
            })
        );
        let duplicate_rows = [same_id_rows[0]; 2];
        let mut duplicate_output = same_id_view;
        assert_eq!(
            view_monitor(
                MonitorRole::Monitor,
                &duplicate_rows,
                &mut duplicate_output,
                ClockDomainId(9),
                100,
                20,
            ),
            Err(MonitorError::DuplicateResourceIdentity)
        );
        assert_eq!(duplicate_output, same_id_view);
    }

    // REQ: OA-L23-002
    #[test]
    fn alarm_raise_ack_clear_is_idempotent_for_one_instance() {
        let mut alarms = [EMPTY_ALARM; 2];
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 10
                },
            )
            .unwrap(),
            AlarmTransition::Raised
        );
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 20
                },
            )
            .unwrap(),
            AlarmTransition::Unchanged
        );
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(5),
                    at_ns: 20
                },
            ),
            Err(MonitorError::ClockDomainMismatch)
        );
        assert_eq!(
            ack_alarm(MonitorRole::Operate, &mut alarms, 5, ClockDomainId(5), 21),
            Err(MonitorError::ClockDomainMismatch)
        );
        assert_eq!(
            ack_alarm(MonitorRole::Operate, &mut alarms, 5, ClockDomainId(4), 21),
            Ok(())
        );
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 20
                },
            ),
            Err(MonitorError::InvalidTime)
        );
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                false,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 15
                },
            ),
            Err(MonitorError::InvalidTime)
        );
        assert_eq!(alarms[0].state, AlarmState::Active);
        assert_eq!(alarms[0].cleared_at_ns, -1);
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                false,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 30
                },
            )
            .unwrap(),
            AlarmTransition::Cleared
        );
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                false,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 25
                },
            ),
            Err(MonitorError::InvalidTime)
        );
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                5,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 40
                },
            )
            .unwrap(),
            AlarmTransition::Unchanged
        );
        assert_eq!(alarms[0].raised_at_ns, 10);
        assert_eq!(alarms[0].acknowledged_at_ns, 21);
        assert_eq!(alarms[0].cleared_at_ns, 30);

        let mut keyed_alarms = [EMPTY_ALARM; 2];
        let service_key = ResourceKey {
            resource_id: 1,
            parent_service_id: 1,
            kind: ResourceKind::Service,
        };
        let processor_key = ResourceKey {
            resource_id: 1,
            parent_service_id: 1,
            kind: ResourceKind::Processor,
        };
        assert_eq!(
            record_alarm_transition(
                &mut keyed_alarms,
                31,
                service_key,
                7,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 50,
                },
            ),
            Ok(AlarmTransition::Raised)
        );
        assert_eq!(
            record_alarm_transition(
                &mut keyed_alarms,
                32,
                processor_key,
                7,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 50,
                },
            ),
            Ok(AlarmTransition::Raised)
        );
        assert_ne!(keyed_alarms[0].resource, keyed_alarms[1].resource);
        assert_eq!(
            record_alarm_transition(
                &mut keyed_alarms,
                31,
                processor_key,
                7,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(4),
                    at_ns: 60,
                },
            ),
            Err(MonitorError::InvalidAlarm)
        );
    }

    // REQ: OA-L23-003
    #[test]
    fn wrong_role_and_out_of_range_thresholds_refuse() {
        assert_eq!(
            configure_thresholds(
                MonitorRole::Operate,
                ThresholdConfig {
                    p99_limit_ns: 100_000_000,
                    tracks_per_scan_limit: 100
                }
            ),
            Err(MonitorError::WrongRole)
        );
        assert_eq!(
            configure_thresholds(
                MonitorRole::Configure,
                ThresholdConfig {
                    p99_limit_ns: 1,
                    tracks_per_scan_limit: 100
                }
            ),
            Err(MonitorError::InvalidThreshold)
        );
        let mut alarms = [EMPTY_ALARM; 1];
        assert_eq!(
            ack_alarm(MonitorRole::Monitor, &mut alarms, 1, ClockDomainId(1), 0),
            Err(MonitorError::WrongRole)
        );
        assert_eq!(
            event_intent(
                MonitorAction::ConfigureThresholds,
                MonitorRole::Operate,
                1,
                ClockDomainId(1),
                0,
                2,
                0
            ),
            Err(MonitorError::WrongRole)
        );
        assert_eq!(
            event_intent(
                MonitorAction::Acknowledge,
                MonitorRole::Operate,
                1,
                ClockDomainId(1),
                0,
                2,
                0
            )
            .unwrap()
            .target_id,
            2
        );
        assert_eq!(
            configure_thresholds(
                MonitorRole::Configure,
                ThresholdConfig {
                    p99_limit_ns: MIN_LATENCY_LIMIT_NS,
                    tracks_per_scan_limit: 1,
                }
            ),
            Ok(())
        );
        assert_eq!(
            configure_thresholds(
                MonitorRole::Configure,
                ThresholdConfig {
                    p99_limit_ns: MAX_LATENCY_LIMIT_NS,
                    tracks_per_scan_limit: MAX_TRACKS_PER_SCAN,
                }
            ),
            Ok(())
        );
    }

    // REQ: OA-L23-004
    #[test]
    fn latency_claim_is_fresh_ordered_and_limited() {
        let limits = ThresholdConfig {
            p99_limit_ns: 100_000_000,
            tracks_per_scan_limit: 4096,
        };
        let good = LatencyClaim {
            clock_domain: ClockDomainId(1),
            sampled_at_ns: 100,
            p50_ns: 1,
            p95_ns: 2,
            p99_ns: 3,
            max_ns: 4,
            tracks_per_scan: 200,
        };
        assert_eq!(
            validate_latency_claim(good, ClockDomainId(1), 150, 100, limits),
            Ok(())
        );
        assert_eq!(
            validate_latency_claim(good, ClockDomainId(1), 201, 100, limits),
            Err(MonitorError::InvalidFreshness)
        );
        let excessive = LatencyClaim {
            p99_ns: 100_000_001,
            max_ns: 100_000_001,
            ..good
        };
        assert_eq!(
            validate_latency_claim(excessive, ClockDomainId(1), 150, 100, limits),
            Err(MonitorError::LatencyOverThreshold)
        );
        let too_many = LatencyClaim {
            tracks_per_scan: 4097,
            ..good
        };
        assert_eq!(
            validate_latency_claim(too_many, ClockDomainId(1), 150, 100, limits),
            Err(MonitorError::TracksOverThreshold)
        );
    }

    // REQ: OA-L23-005
    #[test]
    fn resource_and_alarm_capacity_limits_are_enforced() {
        let empty = [];
        let mut out = [];
        assert_eq!(
            view_monitor(
                MonitorRole::Configure,
                &empty,
                &mut out,
                ClockDomainId(1),
                0,
                1
            ),
            Err(MonitorError::EmptyResources)
        );
        let mut alarms = [EMPTY_ALARM; 1];
        record_alarm_transition(
            &mut alarms,
            1,
            TEST_SERVICE_RESOURCE,
            1,
            AlarmSeverity::Critical,
            true,
            AlarmTime {
                clock_domain: ClockDomainId(1),
                at_ns: 0,
            },
        )
        .unwrap();
        assert_eq!(
            record_alarm_transition(
                &mut alarms,
                2,
                TEST_SERVICE_RESOURCE,
                2,
                AlarmSeverity::Critical,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(1),
                    at_ns: 0,
                }
            ),
            Err(MonitorError::AlarmCapacity)
        );
        let sample = ResourceInput {
            resource_id: 1,
            parent_service_id: 1,
            kind: ResourceKind::Processor,
            reported_state: ResourceState::Up,
            reason_code: 0,
            clock_domain: ClockDomainId(1),
            observed_at_ns: 0,
        };
        let too_many = [sample; MAX_MONITOR_RESOURCES + 1];
        let mut outputs = [ResourceOutput {
            resource_id: 0,
            parent_service_id: 0,
            kind: ResourceKind::Service,
            state: ResourceState::Unknown,
            reason_code: 0,
            age_ns: 0,
        }; MAX_MONITOR_RESOURCES + 1];
        assert_eq!(
            view_monitor(
                MonitorRole::Monitor,
                &too_many,
                &mut outputs,
                ClockDomainId(1),
                0,
                1
            ),
            Err(MonitorError::ResourcesOverCapacity)
        );
        let mut oversized_alarm_table = [EMPTY_ALARM; MAX_ALARMS + 1];
        assert_eq!(
            record_alarm_transition(
                &mut oversized_alarm_table,
                1,
                TEST_SERVICE_RESOURCE,
                1,
                AlarmSeverity::Warning,
                true,
                AlarmTime {
                    clock_domain: ClockDomainId(1),
                    at_ns: 0,
                }
            ),
            Err(MonitorError::AlarmCapacity)
        );
    }
}

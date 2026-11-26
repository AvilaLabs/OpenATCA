#![forbid(unsafe_code)]
#![no_std]

//! Recorded/synthetic model of output selection and state reconciliation.
//! It does not connect to or certify the health of an operational system.

// User approved the typed M1-to-M2 authority edge on 2026-11-26 (private F1 decision).

#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-migration-switch core supports 64-bit targets only");

pub const MAX_TRACKS: usize = 64;
pub const MAX_FLIGHTS: usize = 64;
pub const MAX_HANDOFFS: usize = 64;
pub const MAX_ALERTS: usize = 64;
pub const MAX_TRACE: usize = 128;
pub const MAX_DIFFS: usize = (MAX_TRACKS + MAX_FLIGHTS + MAX_HANDOFFS + MAX_ALERTS) * 2;

// M1's accepted S1->S2 transition requires FacilitySupervisor and IndependentAssessor.
// M2's role mask encodes the supervisor role as ID 1. These are declared IDs, not
// authentication claims; M1 and M2 both consume caller-supplied records.
const M1_PROMOTION_SUPERVISOR_ROLE_ID: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct SwapUnitKey {
    pub function_id: u32,
    pub scope_id: u32,
    pub window_start_ns: i64,
    pub window_end_ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct UnitPolicy {
    pub policy_version: u32,
    pub policy_digest: [u8; 32],
    pub source_epoch_digest: [u8; 32],
    pub update_interval_ns: i64,
    pub max_decision_delay_ns: i64,
    pub max_snapshot_age_ns: i64,
    pub evidence_set_handle: [u8; 32],
    pub approval_set_handle: [u8; 32],
    pub state_schema_digest: [u8; 32],
    pub consumer_id: u32,
    pub endpoint_class: EndpointClass,
    pub promotion_role_mask: u64,
    pub controller_rollback_role_mask: u64,
    pub health_rollback_role_mask: u64,
    pub track_field_mask: u64,
    pub flight_field_mask: u64,
    pub handoff_field_mask: u64,
    pub alert_field_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EndpointClass {
    Terminal,
    EnrouteLongRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RollbackKind {
    ControllerRequest,
    HealthFault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct RollbackReceipt {
    pub unit: SwapUnitKey,
    pub kind: RollbackKind,
    pub principal_id: [u8; 32],
    pub principal_role_id: u8,
    pub policy_version: u32,
    pub policy_digest: [u8; 32],
    pub source_epoch_digest: [u8; 32],
    pub evidence_set_handle: [u8; 32],
    pub approval_set_handle: [u8; 32],
    pub sequence: u64,
    pub legacy_epoch: u64,
    pub candidate_epoch: u64,
    pub legacy_state_version: u64,
    pub candidate_state_version: u64,
    pub legacy_sequence: u64,
    pub candidate_sequence: u64,
    /// Fresh health observation supplied by the adapter; this core does not verify a live source.
    pub legacy_health: Health,
    pub legacy_source_available: bool,
    pub legacy_health_observed_ns: i64,
    pub event_id: [u8; 32],
    pub observed_ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EvidenceKind {
    Recorded,
    Synthetic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Health {
    Healthy,
    Degraded,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Side {
    Legacy,
    Candidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Output {
    Legacy,
    Candidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum FieldStatus {
    Present,
    Absent,
    Unknown,
    Unsupported,
    Missing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FamilyCoverage {
    CompleteWithEvents,
    CompleteEmpty,
    Partial,
    Missing,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct SourceIdentity {
    pub stream: u32,
    pub decoded_offset: u64,
    pub row: u32,
    pub operation_index: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum EntityFamily {
    Handoff = 1,
    PointOut = 2,
    SectorAssignment = 3,
    FlightAmendment = 4,
    Alert = 5,
    Track = 6,
    Flight = 7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(C)]
pub struct BusinessKey {
    pub entity_key: [u8; 32],
    pub key_status: FieldStatus,
    pub family: EntityFamily,
    pub ordinal: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct TrackState {
    pub key: BusinessKey,
    pub source_identity: SourceIdentity,
    pub time_basis_id: u32,
    pub source_time_ns: i64,
    pub observed_time_ns: i64,
    pub status: FieldStatus,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub altitude_ft: i32,
    pub speed_dkt: u16,
    pub track_ddeg: u16,
    pub flags: u32,
    pub field_mask: u64,
    pub projection_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct FlightState {
    pub key: BusinessKey,
    pub source_identity: SourceIdentity,
    pub time_basis_id: u32,
    pub source_time_ns: i64,
    pub observed_time_ns: i64,
    pub status: FieldStatus,
    pub flight_plan_digest: [u8; 32],
    pub flags: u32,
    pub field_mask: u64,
    pub projection_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct HandoffState {
    pub key: BusinessKey,
    pub source_identity: SourceIdentity,
    pub time_basis_id: u32,
    pub source_time_ns: i64,
    pub observed_time_ns: i64,
    pub status: FieldStatus,
    pub lifecycle: u8,
    pub owner: u32,
    pub recipient: u32,
    pub flags: u32,
    pub field_mask: u64,
    pub projection_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AlertState {
    pub key: BusinessKey,
    pub source_identity: SourceIdentity,
    pub time_basis_id: u32,
    pub source_time_ns: i64,
    pub observed_time_ns: i64,
    pub status: FieldStatus,
    pub active: u8,
    pub acknowledged: u8,
    pub suppressed: u8,
    pub kind: u32,
    pub field_mask: u64,
    pub projection_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Snapshot {
    pub unit: SwapUnitKey,
    pub policy: UnitPolicy,
    pub evidence: EvidenceKind,
    pub side: Side,
    pub source_epoch: u64,
    pub state_version: u64,
    pub sequence: u64,
    pub source_time_ns: i64,
    pub observed_time_ns: i64,
    pub health: Health,
    pub track_coverage: FamilyCoverage,
    pub flight_coverage: FamilyCoverage,
    pub handoff_coverage: FamilyCoverage,
    pub alert_coverage: FamilyCoverage,
    pub tracks: [Option<TrackState>; MAX_TRACKS],
    pub flights: [Option<FlightState>; MAX_FLIGHTS],
    pub handoffs: [Option<HandoffState>; MAX_HANDOFFS],
    pub alerts: [Option<AlertState>; MAX_ALERTS],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Refusal {
    InvalidUnit,
    WrongUnit,
    WrongSide,
    WrongEvidence,
    BadSnapshot,
    Stale,
    SequenceGap,
    Duplicate,
    Unhealthy,
    Incomplete,
    StateMismatch,
    DifferenceOverflow,
    TokenStale,
    Unauthorized,
    OutputGap,
    OutputDuplicate,
    TraceFull,
    ContinuityExceeded,
    TimeOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DiffKind {
    Track,
    Flight,
    Handoff,
    Alert,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
/// One reported difference in the declared consumer projection. For a matched key, bits in
/// `field_mask` identify differing normalized fields; `u32::MAX` means the key exists on only one
/// side. Track bits are status/time/basis/position/altitude/speed/track/flags/coverage/digest;
/// flight, handoff and alert fields use their corresponding normalized record order.
pub struct Diff {
    pub kind: DiffKind,
    pub key: BusinessKey,
    pub legacy_status: FieldStatus,
    pub candidate_status: FieldStatus,
    pub field_mask: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct ReconcileReport {
    pub compared_tracks: u16,
    pub compared_flights: u16,
    pub compared_handoffs: u16,
    pub compared_alerts: u16,
    pub candidate_tracks: u16,
    pub candidate_flights: u16,
    pub candidate_handoffs: u16,
    pub candidate_alerts: u16,
    pub differences: u16,
    pub diffs: [Option<Diff>; MAX_DIFFS],
    pub as_of_ns: i64,
    pub ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
struct ReconcileToken {
    unit: SwapUnitKey,
    policy: UnitPolicy,
    legacy_epoch: u64,
    candidate_epoch: u64,
    legacy_version: u64,
    candidate_version: u64,
    legacy_sequence: u64,
    candidate_sequence: u64,
    as_of_ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct CandidateReadiness {
    token: ReconcileToken,
}

/// Opaque accepted M1 lifecycle grant for this exact window and policy. It can authorize
/// multiple distinct M2 switch intents in the window; it is not a per-switch intent or an
/// authentication credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowAuthorization {
    unit: SwapUnitKey,
    m1_sequence: u32,
    evaluated_at_ns: i64,
    policy_version: u32,
    policy_digest: [u8; 32],
    source_epoch_digest: [u8; 32],
    evidence_set_handle: [u8; 32],
    approval_set_handle: [u8; 32],
    principal_id: u32,
    assessor_principal_id: u32,
}

/// Opaque M2 per-switch intent bound to one exact current reconciliation generation.
/// Its identifiers are declared input records, not authenticated principals.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromotionIntent {
    unit: SwapUnitKey,
    sequence: u64,
    evaluated_at_ns: i64,
    principal_role_id: u8,
    principal_id: u32,
    event_key: [u8; 32],
    readiness: ReconcileToken,
    window_authorization: WindowAuthorization,
    m1_sequence: u32,
    policy_version: u32,
    policy_digest: [u8; 32],
    source_epoch_digest: [u8; 32],
    evidence_set_handle: [u8; 32],
    approval_set_handle: [u8; 32],
}

/// Caller-declared M2 intent fields. The selector validates these before producing an opaque
/// `PromotionIntent`; the values are not authenticated authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PromotionIntentRequest {
    pub sequence: u64,
    pub evaluated_at_ns: i64,
    pub principal_role_id: u8,
    pub principal_id: u32,
    pub event_key: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct TraceRecord {
    pub sequence: u64,
    /// M2 per-switch intent sequence; rollback actions use their rollback receipt sequence.
    pub authority_sequence: u64,
    /// M2 intent or rollback event key. It is not an authenticated identifier.
    pub event_key: [u8; 32],
    pub at_ns: i64,
    /// Reused M1 window grant sequence for promotions; zero for rollback records.
    pub m1_window_sequence: u32,
    pub from: Output,
    pub to: Output,
    pub reason: u8,
}

#[derive(Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Selector {
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evidence: EvidenceKind,
    legacy: Option<Snapshot>,
    candidate: Option<Snapshot>,
    selected: Output,
    output_sequence: u64,
    delivered_sequence: u64,
    last_authority_sequence: u64,
    last_output_time_ns: i64,
    last_delivery_time_ns: i64,
    pending_delivery: bool,
    trace_len: u16,
    trace: [Option<TraceRecord>; MAX_TRACE],
}

const _: () = assert!(core::mem::size_of::<SwapUnitKey>() == 24);
const _: () = assert!(core::mem::size_of::<UnitPolicy>() == 256);
const _: () = assert!(core::mem::size_of::<RollbackReceipt>() == 304);
const _: () = assert!(core::mem::size_of::<SourceIdentity>() == 24);
const _: () = assert!(core::mem::size_of::<BusinessKey>() == 48);
const _: () = assert!(core::mem::size_of::<TrackState>() == 160);
const _: () = assert!(core::mem::size_of::<FlightState>() == 176);
const _: () = assert!(core::mem::size_of::<HandoffState>() == 152);
const _: () = assert!(core::mem::size_of::<AlertState>() == 144);
const _: () = assert!(core::mem::size_of::<Snapshot>() == 40_784);
const _: () = assert!(core::mem::size_of::<Diff>() == 64);
const _: () = assert!(core::mem::size_of::<ReconcileReport>() == 32_808);
const _: () = assert!(core::mem::size_of::<CandidateReadiness>() == 336);
const _: () = assert!(core::mem::size_of::<TraceRecord>() == 64);
const _: () = assert!(core::mem::size_of::<PromotionIntentRequest>() == 56);
const _: () = assert!(core::mem::size_of::<Selector>() == 90_104);

pub const fn valid_unit(unit: SwapUnitKey) -> bool {
    unit.function_id != 0 && unit.scope_id != 0 && unit.window_start_ns < unit.window_end_ns
}

const fn in_window(unit: SwapUnitKey, at_ns: i64) -> bool {
    at_ns >= unit.window_start_ns && at_ns < unit.window_end_ns
}

fn role_allowed(mask: u64, role_id: u8) -> bool {
    let Some(index) = role_id.checked_sub(1) else {
        return false;
    };
    let Some(bit) = 1u64.checked_shl(u32::from(index)) else {
        return false;
    };
    mask & bit != 0
}

pub fn new_selector(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evidence: EvidenceKind,
    start_ns: i64,
) -> Result<Selector, Refusal> {
    if !valid_unit(unit)
        || start_ns < unit.window_start_ns
        || !in_window(unit, start_ns)
        || policy.policy_version == 0
        || policy.policy_digest == [0; 32]
        || policy.source_epoch_digest == [0; 32]
        || policy.evidence_set_handle == [0; 32]
        || policy.approval_set_handle == [0; 32]
        || policy.state_schema_digest == [0; 32]
        || policy.consumer_id == 0
        || policy.promotion_role_mask == 0
        || policy.controller_rollback_role_mask == 0
        || policy.health_rollback_role_mask == 0
        || policy.track_field_mask == 0
        || policy.flight_field_mask == 0
        || policy.handoff_field_mask == 0
        || policy.alert_field_mask == 0
        || match policy.endpoint_class {
            EndpointClass::Terminal => policy.update_interval_ns != 4_800_000_000,
            EndpointClass::EnrouteLongRange => policy.update_interval_ns != 12_000_000_000,
        }
        || policy.max_decision_delay_ns <= 0
        || policy.max_decision_delay_ns > 1_000_000_000
        || policy.max_snapshot_age_ns <= 0
        || policy.max_snapshot_age_ns > 12_000_000_000
    {
        return Err(Refusal::InvalidUnit);
    }
    Ok(Selector {
        unit,
        policy,
        evidence,
        legacy: None,
        candidate: None,
        selected: Output::Legacy,
        output_sequence: 0,
        delivered_sequence: 0,
        last_authority_sequence: 0,
        last_output_time_ns: start_ns,
        last_delivery_time_ns: start_ns,
        pending_delivery: false,
        trace_len: 0,
        trace: [None; MAX_TRACE],
    })
}

fn array_count<T: Copy, const N: usize>(items: &[Option<T>; N]) -> Result<usize, Refusal> {
    let mut count = 0usize;
    let mut ended = false;
    for item in items {
        if item.is_none() {
            ended = true;
        } else if ended {
            return Err(Refusal::BadSnapshot);
        } else {
            count = count.checked_add(1).ok_or(Refusal::BadSnapshot)?;
        }
    }
    Ok(count)
}

#[derive(Clone, Copy)]
struct RowFacts {
    key: BusinessKey,
    source_identity: SourceIdentity,
    time_basis_id: u32,
    source_time_ns: i64,
    observed_time_ns: i64,
    status: FieldStatus,
    field_mask: u64,
    projection_digest: [u8; 32],
}

fn valid_rows<T: Copy, const N: usize>(
    items: &[Option<T>; N],
    facts: impl Fn(T) -> RowFacts,
    required_mask: u64,
    observed_ns: i64,
    family: EntityFamily,
) -> Result<(), Refusal> {
    let count = array_count(items)?;
    let mut i = 0usize;
    while i < count {
        let row = items
            .get(i)
            .copied()
            .flatten()
            .ok_or(Refusal::BadSnapshot)?;
        let row_facts = facts(row);
        if row_facts.key.key_status != FieldStatus::Present
            || row_facts.key.family != family
            || row_facts.source_identity.stream == 0
            || row_facts.time_basis_id == 0
            || row_facts.source_time_ns > row_facts.observed_time_ns
            || row_facts.observed_time_ns > observed_ns
            || row_facts.status != FieldStatus::Present
            || row_facts.field_mask != required_mask
            || row_facts.projection_digest == [0; 32]
        {
            return Err(Refusal::Incomplete);
        }
        if i > 0
            && facts(
                items
                    .get(i - 1)
                    .copied()
                    .flatten()
                    .ok_or(Refusal::BadSnapshot)?,
            )
            .key >= row_facts.key
        {
            return Err(Refusal::BadSnapshot);
        }
        i = i.checked_add(1).ok_or(Refusal::BadSnapshot)?;
    }
    Ok(())
}

fn validate_snapshot(
    s: &Snapshot,
    expected_unit: SwapUnitKey,
    policy: UnitPolicy,
    evidence: EvidenceKind,
    side: Side,
    now_ns: i64,
) -> Result<(), Refusal> {
    if !in_window(expected_unit, now_ns) {
        return Err(Refusal::Stale);
    }
    if s.unit != expected_unit {
        return Err(Refusal::WrongUnit);
    }
    if s.policy != policy {
        return Err(Refusal::WrongUnit);
    }
    if s.evidence != evidence {
        return Err(Refusal::WrongEvidence);
    }
    if s.side != side {
        return Err(Refusal::WrongSide);
    }
    if s.source_epoch == 0
        || s.state_version == 0
        || s.sequence == 0
        || s.observed_time_ns > now_ns
        || s.source_time_ns > s.observed_time_ns
    {
        return Err(Refusal::BadSnapshot);
    }
    if !in_window(expected_unit, s.observed_time_ns) {
        return Err(Refusal::BadSnapshot);
    }
    if s.health != Health::Healthy {
        return Err(Refusal::Unhealthy);
    }
    valid_rows(
        &s.tracks,
        |r| RowFacts {
            key: r.key,
            source_identity: r.source_identity,
            time_basis_id: r.time_basis_id,
            source_time_ns: r.source_time_ns,
            observed_time_ns: r.observed_time_ns,
            status: r.status,
            field_mask: r.field_mask,
            projection_digest: r.projection_digest,
        },
        policy.track_field_mask,
        s.observed_time_ns,
        EntityFamily::Track,
    )?;
    valid_rows(
        &s.flights,
        |r| RowFacts {
            key: r.key,
            source_identity: r.source_identity,
            time_basis_id: r.time_basis_id,
            source_time_ns: r.source_time_ns,
            observed_time_ns: r.observed_time_ns,
            status: r.status,
            field_mask: r.field_mask,
            projection_digest: r.projection_digest,
        },
        policy.flight_field_mask,
        s.observed_time_ns,
        EntityFamily::Flight,
    )?;
    valid_rows(
        &s.handoffs,
        |r| RowFacts {
            key: r.key,
            source_identity: r.source_identity,
            time_basis_id: r.time_basis_id,
            source_time_ns: r.source_time_ns,
            observed_time_ns: r.observed_time_ns,
            status: r.status,
            field_mask: r.field_mask,
            projection_digest: r.projection_digest,
        },
        policy.handoff_field_mask,
        s.observed_time_ns,
        EntityFamily::Handoff,
    )?;
    valid_rows(
        &s.alerts,
        |r| RowFacts {
            key: r.key,
            source_identity: r.source_identity,
            time_basis_id: r.time_basis_id,
            source_time_ns: r.source_time_ns,
            observed_time_ns: r.observed_time_ns,
            status: r.status,
            field_mask: r.field_mask,
            projection_digest: r.projection_digest,
        },
        policy.alert_field_mask,
        s.observed_time_ns,
        EntityFamily::Alert,
    )?;
    let coverages = [
        (s.track_coverage, array_count(&s.tracks)?),
        (s.flight_coverage, array_count(&s.flights)?),
        (s.handoff_coverage, array_count(&s.handoffs)?),
        (s.alert_coverage, array_count(&s.alerts)?),
    ];
    for (coverage, count) in coverages {
        match (coverage, count) {
            (FamilyCoverage::CompleteWithEvents, 1..) | (FamilyCoverage::CompleteEmpty, 0) => (),
            _ => return Err(Refusal::Incomplete),
        }
    }
    for row in s.alerts.iter().flatten() {
        if row.active > 1 || row.acknowledged > 1 || row.suppressed > 1 {
            return Err(Refusal::BadSnapshot);
        }
    }
    Ok(())
}

pub fn ingest(
    selector: &mut Selector,
    snapshot: Snapshot,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    validate_snapshot(
        &snapshot,
        selector.unit,
        selector.policy,
        selector.evidence,
        snapshot.side,
        now_ns,
    )?;
    if max_age_ns != selector.policy.max_snapshot_age_ns {
        return Err(Refusal::BadSnapshot);
    }
    if now_ns
        .checked_sub(snapshot.observed_time_ns)
        .ok_or(Refusal::TimeOverflow)?
        > max_age_ns
    {
        return Err(Refusal::Stale);
    }
    let slot = match snapshot.side {
        Side::Legacy => &mut selector.legacy,
        Side::Candidate => &mut selector.candidate,
    };
    if let Some(previous) = slot {
        if snapshot.source_epoch != previous.source_epoch {
            return Err(Refusal::WrongUnit);
        }
        if snapshot.sequence <= previous.sequence
            || snapshot.state_version <= previous.state_version
        {
            return Err(Refusal::Duplicate);
        }
        if snapshot.sequence
            != previous
                .sequence
                .checked_add(1)
                .ok_or(Refusal::TimeOverflow)?
        {
            return Err(Refusal::SequenceGap);
        }
        if snapshot.observed_time_ns < previous.observed_time_ns {
            return Err(Refusal::Stale);
        }
    } else if snapshot.sequence != 1 || snapshot.state_version != 1 {
        return Err(Refusal::SequenceGap);
    }
    *slot = Some(snapshot);
    Ok(())
}

fn diff_rows<T: Copy, const N: usize>(
    a: &[Option<T>; N],
    b: &[Option<T>; N],
    kind: DiffKind,
    key: impl Fn(T) -> BusinessKey,
    status: impl Fn(T) -> FieldStatus,
    mask: impl Fn(T, T) -> u32,
    report: &mut ReconcileReport,
) -> Result<(), Refusal> {
    let mut i = 0usize;
    let mut j = 0usize;
    loop {
        let left = a.get(i).copied().flatten();
        let right = b.get(j).copied().flatten();
        match (left, right) {
            (None, None) => return Ok(()),
            (Some(x), Some(y)) => {
                let xk = key(x);
                let yk = key(y);
                if xk == yk {
                    let fields = mask(x, y);
                    if fields != 0 {
                        let n = usize::from(report.differences);
                        *report.diffs.get_mut(n).ok_or(Refusal::DifferenceOverflow)? = Some(Diff {
                            kind,
                            key: xk,
                            legacy_status: status(x),
                            candidate_status: status(y),
                            field_mask: fields,
                        });
                        report.differences = report
                            .differences
                            .checked_add(1)
                            .ok_or(Refusal::BadSnapshot)?;
                    }
                    i = i.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                    j = j.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                } else if xk < yk {
                    let n = usize::from(report.differences);
                    *report.diffs.get_mut(n).ok_or(Refusal::DifferenceOverflow)? = Some(Diff {
                        kind,
                        key: xk,
                        legacy_status: status(x),
                        candidate_status: FieldStatus::Absent,
                        field_mask: u32::MAX,
                    });
                    report.differences = report
                        .differences
                        .checked_add(1)
                        .ok_or(Refusal::BadSnapshot)?;
                    i = i.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                } else {
                    let n = usize::from(report.differences);
                    *report.diffs.get_mut(n).ok_or(Refusal::DifferenceOverflow)? = Some(Diff {
                        kind,
                        key: yk,
                        legacy_status: FieldStatus::Absent,
                        candidate_status: status(y),
                        field_mask: u32::MAX,
                    });
                    report.differences = report
                        .differences
                        .checked_add(1)
                        .ok_or(Refusal::BadSnapshot)?;
                    j = j.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                }
            }
            (Some(x), None) => {
                let n = usize::from(report.differences);
                *report.diffs.get_mut(n).ok_or(Refusal::DifferenceOverflow)? = Some(Diff {
                    kind,
                    key: key(x),
                    legacy_status: status(x),
                    candidate_status: FieldStatus::Absent,
                    field_mask: u32::MAX,
                });
                report.differences = report
                    .differences
                    .checked_add(1)
                    .ok_or(Refusal::BadSnapshot)?;
                i = i.checked_add(1).ok_or(Refusal::BadSnapshot)?;
            }
            (None, Some(y)) => {
                let n = usize::from(report.differences);
                *report.diffs.get_mut(n).ok_or(Refusal::DifferenceOverflow)? = Some(Diff {
                    kind,
                    key: key(y),
                    legacy_status: FieldStatus::Absent,
                    candidate_status: status(y),
                    field_mask: u32::MAX,
                });
                report.differences = report
                    .differences
                    .checked_add(1)
                    .ok_or(Refusal::BadSnapshot)?;
                j = j.checked_add(1).ok_or(Refusal::BadSnapshot)?;
            }
        }
    }
}

const fn mask_bit(value: bool) -> u32 {
    if value {
        1
    } else {
        0
    }
}

fn reconcile_inner(
    selector: &Selector,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(ReconcileReport, Option<ReconcileToken>), Refusal> {
    let l = selector.legacy.as_ref().ok_or(Refusal::Incomplete)?;
    let c = selector.candidate.as_ref().ok_or(Refusal::Incomplete)?;
    validate_snapshot(
        l,
        selector.unit,
        selector.policy,
        selector.evidence,
        Side::Legacy,
        now_ns,
    )?;
    validate_snapshot(
        c,
        selector.unit,
        selector.policy,
        selector.evidence,
        Side::Candidate,
        now_ns,
    )?;
    if max_age_ns != selector.policy.max_snapshot_age_ns {
        return Err(Refusal::BadSnapshot);
    }
    if now_ns
        .checked_sub(l.observed_time_ns)
        .ok_or(Refusal::TimeOverflow)?
        > max_age_ns
        || now_ns
            .checked_sub(c.observed_time_ns)
            .ok_or(Refusal::TimeOverflow)?
            > max_age_ns
    {
        return Err(Refusal::Stale);
    }
    if l.source_epoch == c.source_epoch {
        return Err(Refusal::WrongUnit);
    }
    let mut report = ReconcileReport {
        compared_tracks: u16::try_from(array_count(&l.tracks)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        compared_flights: u16::try_from(array_count(&l.flights)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        compared_handoffs: u16::try_from(array_count(&l.handoffs)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        compared_alerts: u16::try_from(array_count(&l.alerts)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        candidate_tracks: u16::try_from(array_count(&c.tracks)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        candidate_flights: u16::try_from(array_count(&c.flights)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        candidate_handoffs: u16::try_from(array_count(&c.handoffs)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        candidate_alerts: u16::try_from(array_count(&c.alerts)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        differences: 0,
        diffs: [None; MAX_DIFFS],
        as_of_ns: core::cmp::min(l.observed_time_ns, c.observed_time_ns),
        ready: false,
    };
    diff_rows(
        &l.tracks,
        &c.tracks,
        DiffKind::Track,
        |r| r.key,
        |r| r.status,
        |a, b| {
            mask_bit(a.status != b.status)
                | (mask_bit(a.source_time_ns != b.source_time_ns) << 1)
                | (mask_bit(a.time_basis_id != b.time_basis_id) << 2)
                | (mask_bit(a.lat_e7 != b.lat_e7) << 3)
                | (mask_bit(a.lon_e7 != b.lon_e7) << 4)
                | (mask_bit(a.altitude_ft != b.altitude_ft) << 5)
                | (mask_bit(a.speed_dkt != b.speed_dkt) << 6)
                | (mask_bit(a.track_ddeg != b.track_ddeg) << 7)
                | (mask_bit(a.flags != b.flags) << 8)
                | (mask_bit(a.field_mask != b.field_mask) << 9)
                | (mask_bit(a.projection_digest != b.projection_digest) << 10)
        },
        &mut report,
    )?;
    diff_rows(
        &l.flights,
        &c.flights,
        DiffKind::Flight,
        |r| r.key,
        |r| r.status,
        |a, b| {
            mask_bit(a.status != b.status)
                | (mask_bit(a.source_time_ns != b.source_time_ns) << 1)
                | (mask_bit(a.time_basis_id != b.time_basis_id) << 2)
                | (mask_bit(a.flight_plan_digest != b.flight_plan_digest) << 3)
                | (mask_bit(a.flags != b.flags) << 4)
                | (mask_bit(a.field_mask != b.field_mask) << 5)
                | (mask_bit(a.projection_digest != b.projection_digest) << 6)
        },
        &mut report,
    )?;
    diff_rows(
        &l.handoffs,
        &c.handoffs,
        DiffKind::Handoff,
        |r| r.key,
        |r| r.status,
        |a, b| {
            mask_bit(a.status != b.status)
                | (mask_bit(a.source_time_ns != b.source_time_ns) << 1)
                | (mask_bit(a.time_basis_id != b.time_basis_id) << 2)
                | (mask_bit(a.lifecycle != b.lifecycle) << 3)
                | (mask_bit(a.owner != b.owner) << 4)
                | (mask_bit(a.recipient != b.recipient) << 5)
                | (mask_bit(a.flags != b.flags) << 6)
                | (mask_bit(a.field_mask != b.field_mask) << 7)
                | (mask_bit(a.projection_digest != b.projection_digest) << 8)
        },
        &mut report,
    )?;
    diff_rows(
        &l.alerts,
        &c.alerts,
        DiffKind::Alert,
        |r| r.key,
        |r| r.status,
        |a, b| {
            mask_bit(a.status != b.status)
                | (mask_bit(a.source_time_ns != b.source_time_ns) << 1)
                | (mask_bit(a.time_basis_id != b.time_basis_id) << 2)
                | (mask_bit(a.active != b.active) << 3)
                | (mask_bit(a.acknowledged != b.acknowledged) << 4)
                | (mask_bit(a.suppressed != b.suppressed) << 5)
                | (mask_bit(a.kind != b.kind) << 6)
                | (mask_bit(a.field_mask != b.field_mask) << 7)
                | (mask_bit(a.projection_digest != b.projection_digest) << 8)
        },
        &mut report,
    )?;
    report.ready = report.differences == 0;
    if !report.ready {
        return Ok((report, None));
    }
    let token = ReconcileToken {
        unit: selector.unit,
        policy: selector.policy,
        legacy_epoch: l.source_epoch,
        candidate_epoch: c.source_epoch,
        legacy_version: l.state_version,
        candidate_version: c.state_version,
        legacy_sequence: l.sequence,
        candidate_sequence: c.sequence,
        as_of_ns: report.as_of_ns,
    };
    Ok((report, Some(token)))
}

pub fn reconcile(
    selector: &Selector,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<ReconcileReport, Refusal> {
    reconcile_inner(selector, now_ns, max_age_ns).map(|(report, _)| report)
}

fn token_matches(selector: &Selector, token: ReconcileToken) -> bool {
    matches!((selector.legacy.as_ref(), selector.candidate.as_ref()), (Some(l), Some(c)) if token.unit == selector.unit && token.policy == selector.policy && token.legacy_epoch == l.source_epoch && token.candidate_epoch == c.source_epoch && token.legacy_version == l.state_version && token.candidate_version == c.state_version && token.legacy_sequence == l.sequence && token.candidate_sequence == c.sequence)
}

pub fn prepare_candidate(
    selector: &Selector,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(ReconcileReport, CandidateReadiness), Refusal> {
    let (report, token) = reconcile_inner(selector, now_ns, max_age_ns)?;
    let token = token.ok_or(Refusal::StateMismatch)?;
    Ok((report, CandidateReadiness { token }))
}

/// Converts an opaque accepted M1 S1 Advisory → S2 Window transition into a reusable
/// authorization for M2 intents within the exact same half-open unit window.
/// M1 principal IDs and external handles are declared input records, not authentication.
///
/// M1 result fields are private, so a caller cannot manufacture a successful result:
/// ```compile_fail
/// use oa_migration::{AuditEvent, Refusal, Stage, SwapUnitKey, TransitionResult};
/// let key = SwapUnitKey { function_id: 1, scope_id: 1, window_start_ns: 1, window_end_ns: 10 };
/// let unit = oa_migration::new_unit(key, 1, [1; 32], [2; 32], 1).unwrap();
/// let event = AuditEvent {
///     key, sequence: 1, evaluated_at_ns: 2, from: Stage::S1Advisory,
///     to: Stage::S2Window, refusal: Refusal::None, policy_version: 1,
///     principal_id: 3, assessor_principal_id: 4, policy_digest: [1; 32],
///     source_epoch_digest: [2; 32], evidence_set_handle: [5; 32], approval_set_handle: [6; 32],
/// };
/// let _forged = TransitionResult { unit, event };
/// ```
pub fn authorize_window(
    selector: &Selector,
    result: &oa_migration::TransitionResult,
) -> Result<WindowAuthorization, Refusal> {
    let unit = oa_migration::transition_unit(result);
    let event = oa_migration::transition_event(result);
    let m1_key = oa_migration::swap_unit_key(&unit);
    let key_matches = m1_key.function_id == selector.unit.function_id
        && m1_key.scope_id == selector.unit.scope_id
        && m1_key.window_start_ns == selector.unit.window_start_ns
        && m1_key.window_end_ns == selector.unit.window_end_ns;
    if event.refusal != oa_migration::Refusal::None
        || event.from != oa_migration::Stage::S1Advisory
        || event.to != oa_migration::Stage::S2Window
        || oa_migration::swap_unit_stage(&unit) != oa_migration::Stage::S2Window
        || event.key != m1_key
        || !key_matches
        || event.sequence == 0
        || event.sequence != oa_migration::swap_unit_audit_sequence(&unit)
        || event.policy_version != selector.policy.policy_version
        || event.policy_version != oa_migration::swap_unit_policy_version(&unit)
        || event.policy_digest != selector.policy.policy_digest
        || event.policy_digest != oa_migration::swap_unit_policy_digest(&unit)
        || event.source_epoch_digest != selector.policy.source_epoch_digest
        || event.source_epoch_digest != oa_migration::swap_unit_source_epoch_digest(&unit)
        || event.principal_id == 0
        || event.assessor_principal_id == 0
        || event.principal_id == event.assessor_principal_id
        || event.evidence_set_handle != selector.policy.evidence_set_handle
        || event.approval_set_handle != selector.policy.approval_set_handle
        || !role_allowed(
            selector.policy.promotion_role_mask,
            M1_PROMOTION_SUPERVISOR_ROLE_ID,
        )
        || !in_window(selector.unit, event.evaluated_at_ns)
    {
        return Err(Refusal::Unauthorized);
    }
    Ok(WindowAuthorization {
        unit: selector.unit,
        m1_sequence: event.sequence,
        evaluated_at_ns: event.evaluated_at_ns,
        policy_version: event.policy_version,
        policy_digest: event.policy_digest,
        source_epoch_digest: event.source_epoch_digest,
        evidence_set_handle: event.evidence_set_handle,
        approval_set_handle: event.approval_set_handle,
        principal_id: event.principal_id,
        assessor_principal_id: event.assessor_principal_id,
    })
}

/// Creates an opaque M2 intent from current readiness plus caller-declared intent fields.
/// This is a modeled intent record, not authenticated authority; selection rechecks all bindings.
pub fn new_promotion_intent(
    selector: &Selector,
    readiness: &CandidateReadiness,
    authorization: &WindowAuthorization,
    request: PromotionIntentRequest,
) -> Result<PromotionIntent, Refusal> {
    let PromotionIntentRequest {
        sequence,
        evaluated_at_ns,
        principal_role_id,
        principal_id,
        event_key,
    } = request;
    if !window_authorization_matches(selector, authorization)
        || !token_matches(selector, readiness.token)
        || sequence == 0
        || sequence <= selector.last_authority_sequence
        || principal_id != authorization.principal_id
        || principal_role_id != M1_PROMOTION_SUPERVISOR_ROLE_ID
        || event_key == [0; 32]
        || intent_event_key_used(selector, event_key)
        || !role_allowed(selector.policy.promotion_role_mask, principal_role_id)
        || !in_window(selector.unit, evaluated_at_ns)
        || evaluated_at_ns < authorization.evaluated_at_ns
    {
        return Err(Refusal::Unauthorized);
    }
    Ok(PromotionIntent {
        unit: selector.unit,
        sequence,
        evaluated_at_ns,
        principal_role_id,
        principal_id,
        event_key,
        readiness: readiness.token,
        window_authorization: authorization.clone(),
        m1_sequence: authorization.m1_sequence,
        policy_version: selector.policy.policy_version,
        policy_digest: selector.policy.policy_digest,
        source_epoch_digest: selector.policy.source_epoch_digest,
        evidence_set_handle: selector.policy.evidence_set_handle,
        approval_set_handle: selector.policy.approval_set_handle,
    })
}

fn window_authorization_matches(selector: &Selector, authorization: &WindowAuthorization) -> bool {
    authorization.unit == selector.unit
        && authorization.m1_sequence > 0
        && authorization.policy_version == selector.policy.policy_version
        && authorization.policy_digest == selector.policy.policy_digest
        && authorization.source_epoch_digest == selector.policy.source_epoch_digest
        && authorization.evidence_set_handle == selector.policy.evidence_set_handle
        && authorization.approval_set_handle == selector.policy.approval_set_handle
        && authorization.principal_id != 0
        && authorization.assessor_principal_id != 0
        && authorization.principal_id != authorization.assessor_principal_id
        && in_window(selector.unit, authorization.evaluated_at_ns)
}

fn intent_event_key_used(selector: &Selector, event_key: [u8; 32]) -> bool {
    for record in selector
        .trace
        .iter()
        .take(usize::from(selector.trace_len))
        .flatten()
    {
        if record.reason == 1 && record.event_key == event_key {
            return true;
        }
    }
    false
}

fn rollback_event_key_used(selector: &Selector, event_key: [u8; 32]) -> bool {
    for record in selector
        .trace
        .iter()
        .take(usize::from(selector.trace_len))
        .flatten()
    {
        if matches!(record.reason, 2 | 3) && record.event_key == event_key {
            return true;
        }
    }
    false
}

/// Selects the candidate for one fresh M2 intent under an accepted M1 window grant.
/// M1 authorization can be reused within its window; each M2 intent has its own replay-checked
/// sequence and readiness generation.
pub fn select_candidate(
    selector: &mut Selector,
    readiness: &CandidateReadiness,
    authorization: &WindowAuthorization,
    intent: &PromotionIntent,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    if selector.selected != Output::Legacy {
        return Err(Refusal::StateMismatch);
    }
    // Keep one trace slot available for an immediate rollback after promotion.
    if usize::from(selector.trace_len) >= MAX_TRACE - 1 {
        return Err(Refusal::TraceFull);
    }
    if max_age_ns != selector.policy.max_snapshot_age_ns {
        return Err(Refusal::BadSnapshot);
    }
    if !window_authorization_matches(selector, authorization)
        || !in_window(selector.unit, now_ns)
        || authorization.evaluated_at_ns > now_ns
        || intent.unit != selector.unit
        || intent.sequence == 0
        || intent.sequence <= selector.last_authority_sequence
        || intent.principal_id == 0
        || intent.principal_id != authorization.principal_id
        || intent.principal_role_id != M1_PROMOTION_SUPERVISOR_ROLE_ID
        || intent.event_key == [0; 32]
        || intent_event_key_used(selector, intent.event_key)
        || !role_allowed(
            selector.policy.promotion_role_mask,
            intent.principal_role_id,
        )
        || intent.readiness != readiness.token
        || intent.window_authorization != *authorization
        || intent.m1_sequence != authorization.m1_sequence
        || intent.policy_version != selector.policy.policy_version
        || intent.policy_digest != selector.policy.policy_digest
        || intent.source_epoch_digest != selector.policy.source_epoch_digest
        || intent.evidence_set_handle != selector.policy.evidence_set_handle
        || intent.approval_set_handle != selector.policy.approval_set_handle
        || !in_window(selector.unit, intent.evaluated_at_ns)
        || intent.evaluated_at_ns < authorization.evaluated_at_ns
        || intent.evaluated_at_ns > now_ns
    {
        return Err(Refusal::Unauthorized);
    }
    if now_ns
        .checked_sub(intent.evaluated_at_ns)
        .ok_or(Refusal::TimeOverflow)?
        > selector.policy.max_decision_delay_ns
    {
        return Err(Refusal::ContinuityExceeded);
    }
    if !token_matches(selector, readiness.token) {
        return Err(Refusal::TokenStale);
    }
    let token_age = now_ns
        .checked_sub(readiness.token.as_of_ns)
        .ok_or(Refusal::TimeOverflow)?;
    if token_age < 0 || token_age > max_age_ns {
        return Err(Refusal::Stale);
    }
    let _ = selector.candidate.as_ref().ok_or(Refusal::Incomplete)?;
    record_switch(
        selector,
        SwitchRecord {
            to: Output::Candidate,
            at_ns: now_ns,
            reason: 1,
            allow_pending_delivery: false,
            authority_sequence: intent.sequence,
            m1_window_sequence: intent.m1_sequence,
            event_key: intent.event_key,
        },
    )?;
    selector.last_authority_sequence = intent.sequence;
    Ok(())
}

pub fn rollback(
    selector: &mut Selector,
    cause: RollbackReceipt,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    if selector.selected != Output::Candidate {
        return Err(Refusal::StateMismatch);
    }
    if max_age_ns != selector.policy.max_snapshot_age_ns {
        return Err(Refusal::BadSnapshot);
    }
    if cause.event_id == [0; 32]
        || rollback_event_key_used(selector, cause.event_id)
        || cause.unit != selector.unit
        || cause.principal_id == [0; 32]
        || cause.principal_role_id == 0
        || cause.policy_version != selector.policy.policy_version
        || cause.policy_digest != selector.policy.policy_digest
        || cause.source_epoch_digest != selector.policy.source_epoch_digest
        || cause.evidence_set_handle != selector.policy.evidence_set_handle
        || cause.approval_set_handle != selector.policy.approval_set_handle
        || cause.sequence <= selector.last_authority_sequence
        || cause.observed_ns > now_ns
        || cause.observed_ns < selector.last_output_time_ns
        || !in_window(selector.unit, cause.observed_ns)
        || !in_window(selector.unit, now_ns)
    {
        return Err(Refusal::Unauthorized);
    }
    let rollback_role_mask = match cause.kind {
        RollbackKind::ControllerRequest => selector.policy.controller_rollback_role_mask,
        RollbackKind::HealthFault => selector.policy.health_rollback_role_mask,
    };
    if !role_allowed(rollback_role_mask, cause.principal_role_id) {
        return Err(Refusal::Unauthorized);
    }
    if now_ns
        .checked_sub(cause.observed_ns)
        .ok_or(Refusal::TimeOverflow)?
        > selector.policy.max_decision_delay_ns
    {
        return Err(Refusal::ContinuityExceeded);
    }
    let l = selector.legacy.as_ref().ok_or(Refusal::Incomplete)?;
    let c = selector.candidate.as_ref().ok_or(Refusal::Incomplete)?;
    if cause.legacy_epoch != l.source_epoch
        || cause.candidate_epoch != c.source_epoch
        || cause.legacy_state_version != l.state_version
        || cause.candidate_state_version != c.state_version
        || cause.legacy_sequence != l.sequence
        || cause.candidate_sequence != c.sequence
    {
        return Err(Refusal::TokenStale);
    }
    if !cause.legacy_source_available
        || cause.legacy_health != Health::Healthy
        || !in_window(selector.unit, cause.legacy_health_observed_ns)
        || cause.legacy_health_observed_ns < l.observed_time_ns
        || cause.legacy_health_observed_ns > now_ns
    {
        return Err(Refusal::Unhealthy);
    }
    if now_ns
        .checked_sub(cause.legacy_health_observed_ns)
        .ok_or(Refusal::TimeOverflow)?
        > max_age_ns
    {
        return Err(Refusal::Stale);
    }
    let legacy_age = now_ns
        .checked_sub(l.observed_time_ns)
        .ok_or(Refusal::TimeOverflow)?;
    if legacy_age < 0 || legacy_age > max_age_ns {
        return Err(Refusal::Stale);
    }
    let reason = match cause.kind {
        RollbackKind::ControllerRequest => 2,
        RollbackKind::HealthFault => 3,
    };
    record_switch(
        selector,
        SwitchRecord {
            to: Output::Legacy,
            at_ns: now_ns,
            reason,
            allow_pending_delivery: true,
            authority_sequence: cause.sequence,
            m1_window_sequence: 0,
            event_key: cause.event_id,
        },
    )?;
    selector.last_authority_sequence = cause.sequence;
    Ok(())
}

struct SwitchRecord {
    to: Output,
    at_ns: i64,
    reason: u8,
    allow_pending_delivery: bool,
    authority_sequence: u64,
    m1_window_sequence: u32,
    event_key: [u8; 32],
}

fn record_switch(selector: &mut Selector, switch: SwitchRecord) -> Result<(), Refusal> {
    let SwitchRecord {
        to,
        at_ns,
        reason,
        allow_pending_delivery,
        authority_sequence,
        m1_window_sequence,
        event_key,
    } = switch;
    if selector.pending_delivery && !allow_pending_delivery {
        return Err(Refusal::ContinuityExceeded);
    }
    let sequence = selector
        .output_sequence
        .checked_add(1)
        .ok_or(Refusal::TimeOverflow)?;
    if at_ns < selector.last_output_time_ns
        || at_ns < selector.unit.window_start_ns
        || !in_window(selector.unit, at_ns)
    {
        return Err(Refusal::Stale);
    }
    let i = usize::from(selector.trace_len);
    if i >= MAX_TRACE {
        return Err(Refusal::TraceFull);
    }
    *selector.trace.get_mut(i).ok_or(Refusal::TraceFull)? = Some(TraceRecord {
        sequence,
        authority_sequence,
        m1_window_sequence,
        event_key,
        at_ns,
        from: selector.selected,
        to,
        reason,
    });
    selector.trace_len = selector
        .trace_len
        .checked_add(1)
        .ok_or(Refusal::TraceFull)?;
    selector.output_sequence = sequence;
    selector.last_output_time_ns = at_ns;
    selector.pending_delivery = true;
    selector.selected = to;
    Ok(())
}

pub fn selected(selector: &Selector) -> Output {
    selector.selected
}
pub fn trace(selector: &Selector) -> (&[Option<TraceRecord>; MAX_TRACE], u16) {
    (&selector.trace, selector.trace_len)
}

/// Records a delivered consumer sequence observation. This is evidence supplied by a drill/feed
/// adapter; it is not proof that a live downstream consumer received the update.
pub fn observe_delivery(
    selector: &mut Selector,
    consumer_id: u32,
    output: Output,
    sequence: u64,
    at_ns: i64,
) -> Result<(), Refusal> {
    if consumer_id != selector.policy.consumer_id {
        return Err(Refusal::WrongUnit);
    }
    if output != selector.selected {
        return Err(Refusal::StateMismatch);
    }
    if sequence <= selector.delivered_sequence {
        return Err(Refusal::OutputDuplicate);
    }
    if sequence
        != selector
            .delivered_sequence
            .checked_add(1)
            .ok_or(Refusal::TimeOverflow)?
    {
        return Err(Refusal::OutputGap);
    }
    if at_ns < selector.last_delivery_time_ns || at_ns < selector.last_output_time_ns {
        return Err(Refusal::Stale);
    }
    if !in_window(selector.unit, at_ns) {
        return Err(Refusal::Stale);
    }
    if selector.pending_delivery {
        let delay = at_ns
            .checked_sub(selector.last_output_time_ns)
            .ok_or(Refusal::TimeOverflow)?;
        if delay < 0
            || delay > selector.policy.update_interval_ns
            || delay > selector.policy.max_decision_delay_ns
        {
            return Err(Refusal::ContinuityExceeded);
        }
    }
    selector.delivered_sequence = sequence;
    selector.last_delivery_time_ns = at_ns;
    selector.pending_delivery = false;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use oa_migration::{
        AuthorityApproval as M1Approval, AuthorityRole as M1Role, Comparison as M1Comparison,
        EvidenceQuality as M1Quality, MetricCriterion as M1Criterion, MetricEvidence as M1Metric,
        RollbackRule as M1RollbackRule, RollbackTriggerPolicy as M1RollbackPolicy,
        Stage as M1Stage, SwapUnitKey as M1Key, TransitionInput as M1Input,
        TransitionPolicy as M1Policy,
    };

    fn m1_key(key: SwapUnitKey) -> M1Key {
        M1Key {
            function_id: key.function_id,
            scope_id: key.scope_id,
            window_start_ns: key.window_start_ns,
            window_end_ns: key.window_end_ns,
        }
    }

    fn m1_policy(key: M1Key, from: M1Stage, to: M1Stage, policy: UnitPolicy) -> M1Policy {
        M1Policy {
            key,
            from,
            to,
            policy_version: policy.policy_version,
            policy_digest: policy.policy_digest,
            source_epoch_digest: policy.source_epoch_digest,
            maximum_evidence_age_ns: 10_000,
            required_role_mask: oa_migration::ROLE_SUPERVISOR | oa_migration::ROLE_ASSESSOR,
            criterion_count: 1,
            criteria: [M1Criterion {
                metric_id: 4,
                comparison: M1Comparison::AtMost,
                threshold: 3,
                minimum_samples: 1,
                minimum_denominator: 1,
                minimum_exposure_ns: 1,
            }; oa_migration::MAX_CRITERIA],
            dependency_count: 0,
            dependencies: [oa_migration::DependencyRequirement {
                key,
                minimum_stage: M1Stage::S0Shadow,
                policy_version: 1,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
            }; oa_migration::MAX_DEPENDENCIES],
            rollback_policy: M1RollbackPolicy {
                blocking_difference: M1RollbackRule {
                    enabled: true,
                    maximum_response_ns: 1_000,
                    required_role_mask: oa_migration::ROLE_SUPERVISOR,
                },
                controller_request: M1RollbackRule {
                    enabled: true,
                    maximum_response_ns: 1_000,
                    required_role_mask: oa_migration::ROLE_CONTROLLER_ON_POSITION,
                },
                health_fault: M1RollbackRule {
                    enabled: true,
                    maximum_response_ns: 1_000,
                    required_role_mask: oa_migration::ROLE_TECH_OPS,
                },
            },
        }
    }

    fn m1_evidence(
        key: M1Key,
        policy: UnitPolicy,
        stage: M1Stage,
        start: i64,
        end: i64,
        observed: i64,
    ) -> [M1Metric; 1] {
        [M1Metric {
            key,
            measured_stage: stage,
            metric_id: 4,
            quality: M1Quality::Complete,
            policy_version: policy.policy_version,
            samples: 5,
            denominator: 8,
            observed,
            exposure_ns: u64::try_from(end - start).unwrap(),
            observation_start_ns: start,
            observation_end_ns: end,
            observed_at_ns: end,
            policy_digest: policy.policy_digest,
            source_epoch_digest: policy.source_epoch_digest,
            artifact_handle: [20; 32],
            evidence_set_handle: policy.evidence_set_handle,
            observation_origin_key: key,
            origin_stage_activation_ns: 0,
            origin_receipt_handle: [0; 32],
        }]
    }

    fn m1_approvals(
        key: M1Key,
        policy: UnitPolicy,
        from: M1Stage,
        to: M1Stage,
        now: i64,
    ) -> [M1Approval; 2] {
        [
            M1Approval {
                key,
                from,
                to,
                role: M1Role::FacilitySupervisor,
                approved: true,
                principal_id: 8,
                approved_at_ns: now,
                policy_version: policy.policy_version,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                authority_record_handle: [21; 32],
            },
            M1Approval {
                key,
                from,
                to,
                role: M1Role::IndependentAssessor,
                approved: true,
                principal_id: 9,
                approved_at_ns: now,
                policy_version: policy.policy_version,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                authority_record_handle: [22; 32],
            },
        ]
    }

    fn m1_transition(
        selector: &Selector,
        evaluated_at_ns: i64,
        key: Option<SwapUnitKey>,
        digest: Option<[u8; 32]>,
        source_epoch_digest: Option<[u8; 32]>,
        force_refusal: bool,
    ) -> oa_migration::TransitionResult {
        let mut policy = selector.policy;
        let key = m1_key(key.unwrap_or(selector.unit));
        if let Some(value) = digest {
            policy.policy_digest = value;
        }
        if let Some(value) = source_epoch_digest {
            policy.source_epoch_digest = value;
        }
        let source_epoch_digest = policy.source_epoch_digest;
        let mut audit = oa_migration::empty_audit_log();
        let unit = oa_migration::new_unit(
            key,
            policy.policy_version,
            policy.policy_digest,
            source_epoch_digest,
            key.window_start_ns,
        )
        .unwrap();
        let first_policy = m1_policy(key, M1Stage::S0Shadow, M1Stage::S1Advisory, policy);
        let first_metrics = m1_evidence(
            key,
            policy,
            M1Stage::S0Shadow,
            key.window_start_ns,
            key.window_start_ns + 9,
            2,
        );
        let first_approvals = m1_approvals(
            key,
            policy,
            M1Stage::S0Shadow,
            M1Stage::S1Advisory,
            key.window_start_ns + 10,
        );
        let first = oa_migration::evaluate_transition(
            &unit,
            M1Input {
                policy: &first_policy,
                metrics: &first_metrics,
                dependencies: &[],
                approvals: &first_approvals,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                now_ns: key.window_start_ns + 10,
            },
            &mut audit,
        );
        assert_eq!(
            oa_migration::transition_event(&first).refusal,
            oa_migration::Refusal::None
        );
        let second_policy = m1_policy(key, M1Stage::S1Advisory, M1Stage::S2Window, policy);
        let second_metrics = m1_evidence(
            key,
            policy,
            M1Stage::S1Advisory,
            key.window_start_ns + 10,
            evaluated_at_ns - 1,
            if force_refusal { 4 } else { 2 },
        );
        let second_approvals = m1_approvals(
            key,
            policy,
            M1Stage::S1Advisory,
            M1Stage::S2Window,
            evaluated_at_ns,
        );
        oa_migration::evaluate_transition(
            &oa_migration::transition_unit(&first),
            M1Input {
                policy: &second_policy,
                metrics: &second_metrics,
                dependencies: &[],
                approvals: &second_approvals,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                now_ns: evaluated_at_ns,
            },
            &mut audit,
        )
    }

    fn promotion(selector: &Selector, evaluated_at_ns: i64) -> WindowAuthorization {
        let result = m1_transition(selector, evaluated_at_ns, None, None, None, false);
        authorize_window(selector, &result).unwrap()
    }

    fn promotion_with_window(
        selector: &Selector,
        evaluated_at_ns: i64,
        start_ns: i64,
    ) -> Result<WindowAuthorization, Refusal> {
        let mut key = selector.unit;
        key.window_start_ns = start_ns;
        let result = m1_transition(selector, evaluated_at_ns, Some(key), None, None, false);
        authorize_window(selector, &result)
    }

    fn promotion_with_wrong_policy(
        selector: &Selector,
        evaluated_at_ns: i64,
    ) -> Result<WindowAuthorization, Refusal> {
        let result = m1_transition(selector, evaluated_at_ns, None, Some([99; 32]), None, false);
        authorize_window(selector, &result)
    }

    fn promotion_with_wrong_source(
        selector: &Selector,
        evaluated_at_ns: i64,
    ) -> Result<WindowAuthorization, Refusal> {
        let result = m1_transition(selector, evaluated_at_ns, None, None, Some([98; 32]), false);
        authorize_window(selector, &result)
    }

    fn intent(
        selector: &Selector,
        readiness: &CandidateReadiness,
        authorization: &WindowAuthorization,
        sequence: u64,
        evaluated_at_ns: i64,
    ) -> PromotionIntent {
        let event_key = [u8::try_from(sequence).unwrap_or(1); 32];
        new_promotion_intent(
            selector,
            readiness,
            authorization,
            PromotionIntentRequest {
                sequence,
                evaluated_at_ns,
                principal_role_id: 1,
                principal_id: 8,
                event_key,
            },
        )
        .unwrap()
    }

    fn fixture() -> (Selector, Snapshot, Snapshot) {
        let unit = SwapUnitKey {
            function_id: 1,
            scope_id: 2,
            window_start_ns: 10,
            window_end_ns: 10_000,
        };
        let policy = UnitPolicy {
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
        };
        let mut l = Snapshot {
            unit,
            policy,
            evidence: EvidenceKind::Synthetic,
            side: Side::Legacy,
            source_epoch: 11,
            state_version: 1,
            sequence: 1,
            source_time_ns: 20,
            observed_time_ns: 20,
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
        let mut c = l.clone();
        c.side = Side::Candidate;
        c.source_epoch = 12;
        let key = |family| BusinessKey {
            entity_key: [9; 32],
            key_status: FieldStatus::Present,
            family,
            ordinal: 1,
        };
        let source_identity = SourceIdentity {
            stream: 1,
            decoded_offset: 0,
            row: 1,
            operation_index: 0,
        };
        l.tracks[0] = Some(TrackState {
            key: key(EntityFamily::Track),
            source_identity,
            time_basis_id: 1,
            source_time_ns: 20,
            observed_time_ns: 20,
            status: FieldStatus::Present,
            lat_e7: 1,
            lon_e7: 2,
            altitude_ft: 3,
            speed_dkt: 4,
            track_ddeg: 5,
            flags: 0,
            field_mask: 1,
            projection_digest: [13; 32],
        });
        c.tracks = l.tracks;
        c.tracks[0].as_mut().unwrap().source_identity.stream = 2;
        l.flights[0] = Some(FlightState {
            key: key(EntityFamily::Flight),
            source_identity,
            time_basis_id: 1,
            source_time_ns: 20,
            observed_time_ns: 20,
            status: FieldStatus::Present,
            flight_plan_digest: [1; 32],
            flags: 0,
            field_mask: 1,
            projection_digest: [14; 32],
        });
        c.flights = l.flights;
        c.flights[0].as_mut().unwrap().source_identity.stream = 2;
        l.handoffs[0] = Some(HandoffState {
            key: key(EntityFamily::Handoff),
            source_identity,
            time_basis_id: 1,
            source_time_ns: 20,
            observed_time_ns: 20,
            status: FieldStatus::Present,
            lifecycle: 2,
            owner: 7,
            recipient: 8,
            flags: 0,
            field_mask: 1,
            projection_digest: [15; 32],
        });
        c.handoffs = l.handoffs;
        c.handoffs[0].as_mut().unwrap().source_identity.stream = 2;
        l.alerts[0] = Some(AlertState {
            key: key(EntityFamily::Alert),
            source_identity,
            time_basis_id: 1,
            source_time_ns: 20,
            observed_time_ns: 20,
            status: FieldStatus::Present,
            active: 1,
            acknowledged: 1,
            suppressed: 0,
            kind: 4,
            field_mask: 1,
            projection_digest: [16; 32],
        });
        c.alerts = l.alerts;
        c.alerts[0].as_mut().unwrap().source_identity.stream = 2;
        let selector = new_selector(unit, policy, EvidenceKind::Synthetic, 10).unwrap();
        (selector, l, c)
    }

    // REQ: OA-MIG-201, OA-MIG-203, OA-MIG-204, OA-MIG-205
    #[test]
    fn h1_h5_make_before_break_and_rollback() {
        let (mut s, l, c) = fixture();
        assert_eq!(prepare_candidate(&s, 25, 100), Err(Refusal::Incomplete));
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let intent = intent(&s, &ready, &authorization, 1, 24);
        select_candidate(&mut s, &ready, &authorization, &intent, 25, 100).unwrap();
        assert_eq!(selected(&s), Output::Candidate);
        observe_delivery(&mut s, 1, Output::Candidate, 1, 26).unwrap();
        let unit = s.unit;
        rollback(
            &mut s,
            RollbackReceipt {
                unit,
                kind: RollbackKind::ControllerRequest,
                principal_id: [8; 32],
                principal_role_id: 1,
                policy_version: 1,
                policy_digest: [3; 32],
                source_epoch_digest: [4; 32],
                evidence_set_handle: [10; 32],
                approval_set_handle: [11; 32],
                sequence: 2,
                legacy_epoch: 11,
                candidate_epoch: 12,
                legacy_state_version: 1,
                candidate_state_version: 1,
                legacy_sequence: 1,
                candidate_sequence: 1,
                legacy_health: Health::Healthy,
                legacy_source_available: true,
                legacy_health_observed_ns: 25,
                event_id: [7; 32],
                observed_ns: 27,
            },
            27,
            100,
        )
        .unwrap();
        assert_eq!(selected(&s), Output::Legacy);
        assert_eq!(
            select_candidate(&mut s, &ready, &authorization, &intent, 28, 100),
            Err(Refusal::Unauthorized)
        );
        assert_eq!(
            select_candidate(&mut s, &ready, &authorization, &intent, 28, 100),
            Err(Refusal::Unauthorized)
        );
        let (records, count) = trace(&s);
        assert_eq!(count, 2);
        assert_eq!(
            records.first().copied().flatten().map(|r| r.to),
            Some(Output::Candidate)
        );
        assert_eq!(
            observe_delivery(&mut s, 1, Output::Candidate, 2, 28),
            Err(Refusal::StateMismatch)
        );
        assert_eq!(
            observe_delivery(&mut s, 1, Output::Legacy, 3, 28),
            Err(Refusal::OutputGap)
        );
        observe_delivery(&mut s, 1, Output::Legacy, 2, 28).unwrap();
    }

    // REQ: OA-MIG-202, OA-MIG-203
    #[test]
    fn stale_epoch_and_acknowledgement_mismatch_refuse() {
        let (mut s, l, mut c) = fixture();
        ingest(&mut s, l.clone(), 25, 100).unwrap();
        c.alerts[0].as_mut().unwrap().acknowledged = 0;
        ingest(&mut s, c, 25, 100).unwrap();
        let report = reconcile(&s, 25, 100).unwrap();
        assert!(!report.ready);
        assert_eq!(report.differences, 1);
        assert_eq!(
            report
                .diffs
                .first()
                .copied()
                .flatten()
                .map(|diff| diff.kind),
            Some(DiffKind::Alert)
        );
        let mut newer = l;
        newer.sequence = 2;
        newer.state_version = 2;
        newer.source_epoch = 99;
        assert_eq!(ingest(&mut s, newer, 30, 100), Err(Refusal::WrongUnit));
    }

    // REQ: OA-MIG-202, OA-MIG-204, OA-MIG-205
    #[test]
    fn input_sequence_gaps_and_unauthorized_switches_refuse() {
        let (mut s, mut l, c) = fixture();
        l.sequence = 2;
        assert_eq!(ingest(&mut s, l, 25, 100), Err(Refusal::SequenceGap));
        ingest(&mut s, fixture().1, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let _ = prepare_candidate(&s, 25, 100).unwrap();
        let denied = m1_transition(&s, 25, None, None, None, true);
        assert_eq!(authorize_window(&s, &denied), Err(Refusal::Unauthorized));
    }

    // REQ: OA-MIG-204
    #[test]
    fn readiness_is_bound_to_both_mirror_versions() {
        let (mut s, l, c) = fixture();
        ingest(&mut s, l.clone(), 25, 100).unwrap();
        ingest(&mut s, c.clone(), 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let stale_intent = intent(&s, &ready, &authorization, 1, 29);
        let mut l_next = l;
        l_next.sequence = 2;
        l_next.state_version = 2;
        l_next.source_time_ns = 30;
        l_next.observed_time_ns = 30;
        let mut c_next = c.clone();
        c_next.sequence = 2;
        c_next.state_version = 2;
        c_next.source_time_ns = 30;
        c_next.observed_time_ns = 30;
        ingest(&mut s, l_next, 30, 100).unwrap();
        ingest(&mut s, c_next, 30, 100).unwrap();
        assert_eq!(
            select_candidate(&mut s, &ready, &authorization, &stale_intent, 30, 100),
            Err(Refusal::TokenStale)
        );
    }

    // REQ: OA-MIG-206
    #[test]
    fn readiness_intent_refuses_a_changed_source_epoch() {
        let (mut s, l, c) = fixture();
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let intent = intent(&s, &ready, &authorization, 1, 24);
        let mut changed = s.candidate.as_ref().unwrap().clone();
        changed.source_epoch = 99;
        s.candidate = Some(changed);
        assert_eq!(
            select_candidate(&mut s, &ready, &authorization, &intent, 25, 100),
            Err(Refusal::TokenStale)
        );
    }

    // REQ: OA-MIG-204, OA-MIG-206
    #[test]
    fn one_m1_window_grant_supports_fresh_m2_intents() {
        let (mut s, l, c) = fixture();
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, first_ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let first_intent = intent(&s, &first_ready, &authorization, 1, 24);
        select_candidate(&mut s, &first_ready, &authorization, &first_intent, 25, 100).unwrap();
        observe_delivery(&mut s, 1, Output::Candidate, 1, 26).unwrap();
        let unit = s.unit;
        rollback(
            &mut s,
            RollbackReceipt {
                unit,
                kind: RollbackKind::HealthFault,
                principal_id: [8; 32],
                principal_role_id: 1,
                policy_version: 1,
                policy_digest: [3; 32],
                source_epoch_digest: [4; 32],
                evidence_set_handle: [10; 32],
                approval_set_handle: [11; 32],
                sequence: 2,
                legacy_epoch: 11,
                candidate_epoch: 12,
                legacy_state_version: 1,
                candidate_state_version: 1,
                legacy_sequence: 1,
                candidate_sequence: 1,
                legacy_health: Health::Healthy,
                legacy_source_available: true,
                legacy_health_observed_ns: 25,
                event_id: [7; 32],
                observed_ns: 27,
            },
            27,
            100,
        )
        .unwrap();
        observe_delivery(&mut s, 1, Output::Legacy, 2, 28).unwrap();
        let (_, second_ready) = prepare_candidate(&s, 28, 100).unwrap();
        assert_eq!(
            new_promotion_intent(
                &s,
                &second_ready,
                &authorization,
                PromotionIntentRequest {
                    sequence: 3,
                    evaluated_at_ns: 28,
                    principal_role_id: 1,
                    principal_id: 8,
                    event_key: [1; 32],
                },
            ),
            Err(Refusal::Unauthorized)
        );
        let second_intent = intent(&s, &second_ready, &authorization, 3, 28);
        select_candidate(
            &mut s,
            &second_ready,
            &authorization,
            &second_intent,
            28,
            100,
        )
        .unwrap();
        observe_delivery(&mut s, 1, Output::Candidate, 3, 29).unwrap();
        let mut repeated_rollback = RollbackReceipt {
            unit: s.unit,
            kind: RollbackKind::HealthFault,
            principal_id: [8; 32],
            principal_role_id: 1,
            policy_version: 1,
            policy_digest: [3; 32],
            source_epoch_digest: [4; 32],
            evidence_set_handle: [10; 32],
            approval_set_handle: [11; 32],
            sequence: 4,
            legacy_epoch: 11,
            candidate_epoch: 12,
            legacy_state_version: 1,
            candidate_state_version: 1,
            legacy_sequence: 1,
            candidate_sequence: 1,
            legacy_health: Health::Healthy,
            legacy_source_available: true,
            legacy_health_observed_ns: 29,
            event_id: [7; 32],
            observed_ns: 30,
        };
        assert_eq!(
            rollback(&mut s, repeated_rollback, 30, 100),
            Err(Refusal::Unauthorized)
        );
        repeated_rollback.event_id = [9; 32];
        rollback(&mut s, repeated_rollback, 30, 100).unwrap();
        let (records, count) = trace(&s);
        assert_eq!(count, 4);
        assert_eq!(records[0].map(|record| record.m1_window_sequence), Some(2));
        assert_eq!(records[1].map(|record| record.m1_window_sequence), Some(0));
        assert_eq!(records[2].map(|record| record.m1_window_sequence), Some(2));
        assert_eq!(records[3].map(|record| record.m1_window_sequence), Some(0));
    }

    // REQ: OA-MIG-202, OA-MIG-204, OA-MIG-205
    #[test]
    fn immediate_health_rollback_is_allowed_before_candidate_delivery() {
        let (mut s, l, c) = fixture();
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let intent = intent(&s, &ready, &authorization, 1, 24);
        select_candidate(&mut s, &ready, &authorization, &intent, 25, 100).unwrap();
        let unit = s.unit;
        let cause = RollbackReceipt {
            unit,
            kind: RollbackKind::HealthFault,
            principal_id: [8; 32],
            principal_role_id: 1,
            policy_version: 1,
            policy_digest: [3; 32],
            source_epoch_digest: [4; 32],
            evidence_set_handle: [10; 32],
            approval_set_handle: [11; 32],
            sequence: 2,
            legacy_epoch: 11,
            candidate_epoch: 12,
            legacy_state_version: 1,
            candidate_state_version: 1,
            legacy_sequence: 1,
            candidate_sequence: 1,
            legacy_health: Health::Healthy,
            legacy_source_available: true,
            legacy_health_observed_ns: 25,
            event_id: [6; 32],
            observed_ns: 25,
        };
        let mut unhealthy = cause;
        unhealthy.legacy_health = Health::Unknown;
        assert_eq!(
            rollback(&mut s, unhealthy, 25, 100),
            Err(Refusal::Unhealthy)
        );
        let mut unavailable = cause;
        unavailable.legacy_source_available = false;
        assert_eq!(
            rollback(&mut s, unavailable, 25, 100),
            Err(Refusal::Unhealthy)
        );
        rollback(&mut s, cause, 25, 100).unwrap();
        assert_eq!(selected(&s), Output::Legacy);
        assert_eq!(observe_delivery(&mut s, 1, Output::Legacy, 1, 26), Ok(()));
    }

    // REQ: OA-MIG-206, OA-MIG-207
    #[test]
    fn typed_m1_result_rejects_wrong_window_policy_refusal_and_replay() {
        let (mut s, l, c) = fixture();
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let wrong_window = promotion_with_window(&s, 24, 11);
        assert_eq!(wrong_window, Err(Refusal::Unauthorized));
        let wrong_policy = promotion_with_wrong_policy(&s, 24);
        assert_eq!(wrong_policy, Err(Refusal::Unauthorized));
        assert_eq!(
            promotion_with_wrong_source(&s, 24),
            Err(Refusal::Unauthorized)
        );
        let valid = promotion(&s, 24);
        let valid_intent = intent(&s, &ready, &valid, 1, 24);
        let other_grant = promotion(&s, 23);
        assert_eq!(
            select_candidate(&mut s, &ready, &other_grant, &valid_intent, 25, 100),
            Err(Refusal::Unauthorized)
        );
        assert_eq!(
            new_promotion_intent(
                &s,
                &ready,
                &valid,
                PromotionIntentRequest {
                    sequence: 1,
                    evaluated_at_ns: 24,
                    principal_role_id: 2,
                    principal_id: 8,
                    event_key: [44; 32],
                },
            ),
            Err(Refusal::Unauthorized)
        );
        assert_eq!(
            new_promotion_intent(
                &s,
                &ready,
                &valid,
                PromotionIntentRequest {
                    sequence: 1,
                    evaluated_at_ns: 24,
                    principal_role_id: 1,
                    principal_id: 99,
                    event_key: [44; 32],
                },
            ),
            Err(Refusal::Unauthorized)
        );
        let window_end_ns = s.unit.window_end_ns;
        assert_eq!(
            select_candidate(&mut s, &ready, &valid, &valid_intent, window_end_ns, 100),
            Err(Refusal::Unauthorized)
        );
        let refused_m1 = m1_transition(&s, 24, None, None, None, true);
        assert_eq!(
            oa_migration::transition_event(&refused_m1).refusal,
            oa_migration::Refusal::MetricFailed
        );
        assert_eq!(
            authorize_window(&s, &refused_m1),
            Err(Refusal::Unauthorized)
        );
        select_candidate(&mut s, &ready, &valid, &valid_intent, 25, 100).unwrap();
        observe_delivery(&mut s, 1, Output::Candidate, 1, 26).unwrap();
        let unit = s.unit;
        rollback(
            &mut s,
            RollbackReceipt {
                unit,
                kind: RollbackKind::HealthFault,
                principal_id: [8; 32],
                principal_role_id: 1,
                policy_version: 1,
                policy_digest: [3; 32],
                source_epoch_digest: [4; 32],
                evidence_set_handle: [10; 32],
                approval_set_handle: [11; 32],
                sequence: 2,
                legacy_epoch: 11,
                candidate_epoch: 12,
                legacy_state_version: 1,
                candidate_state_version: 1,
                legacy_sequence: 1,
                candidate_sequence: 1,
                legacy_health: Health::Healthy,
                legacy_source_available: true,
                legacy_health_observed_ns: 25,
                event_id: [7; 32],
                observed_ns: 27,
            },
            27,
            100,
        )
        .unwrap();
        assert_eq!(
            select_candidate(&mut s, &ready, &valid, &valid_intent, 28, 100),
            Err(Refusal::Unauthorized)
        );
    }

    // REQ: OA-MIG-205
    #[test]
    fn promotion_reserves_the_final_trace_slot_for_emergency_rollback() {
        let (mut s, l, c) = fixture();
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        for slot in 0..(MAX_TRACE - 2) {
            s.trace[slot] = Some(TraceRecord {
                sequence: u64::try_from(slot + 1).unwrap(),
                authority_sequence: 0,
                m1_window_sequence: 0,
                event_key: [1; 32],
                at_ns: 10,
                from: Output::Legacy,
                to: Output::Legacy,
                reason: 0,
            });
        }
        s.trace_len = u16::try_from(MAX_TRACE - 2).unwrap();
        let authorization = promotion(&s, 24);
        let intent = intent(&s, &ready, &authorization, 1, 24);
        select_candidate(&mut s, &ready, &authorization, &intent, 25, 100).unwrap();
        assert_eq!(usize::from(s.trace_len), MAX_TRACE - 1);
        assert_eq!(
            s.trace[MAX_TRACE - 2].map(|r| r.m1_window_sequence),
            Some(2)
        );
        let rollback_receipt = RollbackReceipt {
            unit: s.unit,
            kind: RollbackKind::HealthFault,
            principal_id: [8; 32],
            principal_role_id: 1,
            policy_version: 1,
            policy_digest: [3; 32],
            source_epoch_digest: [4; 32],
            evidence_set_handle: [10; 32],
            approval_set_handle: [11; 32],
            sequence: 2,
            legacy_epoch: 11,
            candidate_epoch: 12,
            legacy_state_version: 1,
            candidate_state_version: 1,
            legacy_sequence: 1,
            candidate_sequence: 1,
            legacy_health: Health::Healthy,
            legacy_source_available: true,
            legacy_health_observed_ns: 25,
            event_id: [6; 32],
            observed_ns: 25,
        };
        rollback(&mut s, rollback_receipt, 25, 100).unwrap();
        assert_eq!(selected(&s), Output::Legacy);
        assert_eq!(usize::from(s.trace_len), MAX_TRACE);
        assert_eq!(
            s.trace[MAX_TRACE - 1].map(|r| (r.to, r.reason)),
            Some((Output::Legacy, 3))
        );
        assert_eq!(
            s.trace[MAX_TRACE - 1].map(|r| r.m1_window_sequence),
            Some(0)
        );
        assert_eq!(
            select_candidate(&mut s, &ready, &authorization, &intent, 26, 100),
            Err(Refusal::TraceFull)
        );
    }

    // REQ: OA-MIG-202, OA-MIG-203
    #[test]
    fn incomplete_projection_and_wrong_consumer_route_refuse() {
        let (mut s, l, mut c) = fixture();
        ingest(&mut s, l, 25, 100).unwrap();
        c.alerts[0].as_mut().unwrap().field_mask = 0;
        assert_eq!(ingest(&mut s, c.clone(), 25, 100), Err(Refusal::Incomplete));
        c.alerts[0].as_mut().unwrap().field_mask = 1;
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let intent = intent(&s, &ready, &authorization, 1, 24);
        select_candidate(&mut s, &ready, &authorization, &intent, 25, 100).unwrap();
        assert_eq!(
            observe_delivery(&mut s, 9, Output::Candidate, 1, 26),
            Err(Refusal::WrongUnit)
        );
    }
}

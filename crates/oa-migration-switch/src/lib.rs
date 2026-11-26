#![forbid(unsafe_code)]
#![no_std]

//! Recorded/synthetic model of output selection and state reconciliation.
//! It does not connect to or certify the health of an operational system.

extern crate alloc;

use alloc::vec::Vec;

// User approved the typed M1-to-M2 authority edge on 2026-11-26 (private F1 decision).

#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-migration-switch core supports 64-bit targets only");

pub const MAX_TRACKS: usize = 64;
pub const MAX_FLIGHTS: usize = 64;
pub const MAX_HANDOFFS: usize = 64;
pub const MAX_ALERTS: usize = 64;
pub const MAX_TRACE: usize = 128;
// One record per configured recipient for every possible output-transition trace slot.
pub const MAX_INTERFACED_FACILITIES: usize = 16;
// C2: this exact product prevents notification-record capacity from trailing trace capacity.
pub const MAX_COORDINATION_RECORDS: usize = MAX_TRACE * MAX_INTERFACED_FACILITIES;
pub const MAX_DIFFS: usize = (MAX_TRACKS + MAX_FLIGHTS + MAX_HANDOFFS + MAX_ALERTS) * 2;
pub const REPLAY_ROWS_PER_FAMILY: usize = 4_096;
pub const REPLAY_TRACE_RECORDS: usize = 201;
pub const REPLAY_COORDINATION_RECORDS: usize = 3_200;
pub const REPLAY_MAX_DIFFS: usize = (REPLAY_ROWS_PER_FAMILY * 4) * 2;

/// Explicit allocation budget for one selector. Construction reserves all storage; no selector
/// operation grows these buffers. Default capacities preserve the existing small replay profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapacityProfile {
    pub tracks: usize,
    pub flights: usize,
    pub handoffs: usize,
    pub alerts: usize,
    pub trace_records: usize,
    pub coordination_records: usize,
}

impl CapacityProfile {
    pub const DEFAULT: Self = Self {
        tracks: MAX_TRACKS,
        flights: MAX_FLIGHTS,
        handoffs: MAX_HANDOFFS,
        alerts: MAX_ALERTS,
        trace_records: MAX_TRACE,
        coordination_records: MAX_COORDINATION_RECORDS,
    };
    pub const REPLAY_4096: Self = Self {
        tracks: REPLAY_ROWS_PER_FAMILY,
        flights: REPLAY_ROWS_PER_FAMILY,
        handoffs: REPLAY_ROWS_PER_FAMILY,
        alerts: REPLAY_ROWS_PER_FAMILY,
        trace_records: REPLAY_TRACE_RECORDS,
        coordination_records: REPLAY_COORDINATION_RECORDS,
    };
}

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
    /// Plain audit-log principal identifier; it is a caller claim, not authentication.
    pub audit_principal_id: u64,
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

/// Read-only, contiguous family view used by both the fixed-size compatibility API and bounded
/// replay profiles. Each family slice must be key-sorted and contain only `Some` entries.
#[derive(Clone, Copy)]
pub struct SnapshotView<'a> {
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
    pub tracks: &'a [Option<TrackState>],
    pub flights: &'a [Option<FlightState>],
    pub handoffs: &'a [Option<HandoffState>],
    pub alerts: &'a [Option<AlertState>],
}

impl Snapshot {
    pub fn view(&self) -> SnapshotView<'_> {
        SnapshotView {
            unit: self.unit,
            policy: self.policy,
            evidence: self.evidence,
            side: self.side,
            source_epoch: self.source_epoch,
            state_version: self.state_version,
            sequence: self.sequence,
            source_time_ns: self.source_time_ns,
            observed_time_ns: self.observed_time_ns,
            health: self.health,
            track_coverage: self.track_coverage,
            flight_coverage: self.flight_coverage,
            handoff_coverage: self.handoff_coverage,
            alert_coverage: self.alert_coverage,
            tracks: &self.tracks,
            flights: &self.flights,
            handoffs: &self.handoffs,
            alerts: &self.alerts,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SnapshotHeader {
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evidence: EvidenceKind,
    side: Side,
    source_epoch: u64,
    state_version: u64,
    sequence: u64,
    source_time_ns: i64,
    observed_time_ns: i64,
    health: Health,
    track_coverage: FamilyCoverage,
    flight_coverage: FamilyCoverage,
    handoff_coverage: FamilyCoverage,
    alert_coverage: FamilyCoverage,
}

// C1/C2: all four family arrays are reserved once with the selector's validated profile.
// Their maximum aggregate row count is 16,384 for REPLAY_4096; no later operation grows them.
#[derive(Debug, Eq, PartialEq)]
struct SnapshotStorage {
    header: Option<SnapshotHeader>,
    tracks: Vec<Option<TrackState>>,
    flights: Vec<Option<FlightState>>,
    handoffs: Vec<Option<HandoffState>>,
    alerts: Vec<Option<AlertState>>,
    track_len: usize,
    flight_len: usize,
    handoff_len: usize,
    alert_len: usize,
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
    CoordinationFull,
    ContinuityExceeded,
    TimeOverflow,
    StorageAllocationFailed,
    CapacityExceeded,
    AuditRequired,
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

/// Bounded reconcile metadata for caller-owned diff output buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct ReconcileSummary {
    pub compared_tracks: u16,
    pub compared_flights: u16,
    pub compared_handoffs: u16,
    pub compared_alerts: u16,
    pub candidate_tracks: u16,
    pub candidate_flights: u16,
    pub candidate_handoffs: u16,
    pub candidate_alerts: u16,
    pub differences: u16,
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

/// Plain pre-switch notification record for one configured interfaced facility.
/// This core records the required notification; it does not transmit it.
// A7.2: cold struct, size guard waived; one audit record per facility per output transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct CoordinationRecord {
    pub output_sequence: u64,
    pub unit: SwapUnitKey,
    pub at_ns: i64,
    pub facility_id: u32,
    pub from: Output,
    pub to: Output,
    pub reason: u8,
}

const EMPTY_COORDINATION_RECORD: CoordinationRecord = CoordinationRecord {
    output_sequence: 0,
    unit: SwapUnitKey {
        function_id: 0,
        scope_id: 0,
        window_start_ns: 0,
        window_end_ns: 0,
    },
    at_ns: 0,
    facility_id: 0,
    from: Output::Legacy,
    to: Output::Legacy,
    reason: 0,
};

#[derive(Debug, Eq, PartialEq)]
pub struct Selector {
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evidence: EvidenceKind,
    profile: CapacityProfile,
    legacy: SnapshotStorage,
    candidate: SnapshotStorage,
    selected: Output,
    output_sequence: u64,
    delivered_sequence: u64,
    last_authority_sequence: u64,
    last_audit_sequence: u64,
    last_audit_digest: [u8; 32],
    last_output_time_ns: i64,
    last_delivery_time_ns: i64,
    pending_delivery: bool,
    trace_len: u16,
    trace: Vec<Option<TraceRecord>>,
    interfaced_facilities: [u32; MAX_INTERFACED_FACILITIES],
    interfaced_facility_count: u8,
    coordination_len: u16,
    coordination: Vec<CoordinationRecord>,
}

const _: () = assert!(core::mem::size_of::<SwapUnitKey>() == 24);
const _: () = assert!(core::mem::size_of::<UnitPolicy>() == 256);
// A7.1: carries the caller-claimed audit principal needed to bind rollback authorization to the
// durable shell receipt; the record grows by one u64 plus alignment padding.
const _: () = assert!(core::mem::size_of::<RollbackReceipt>() == 320);
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
const _: () = assert!(core::mem::size_of::<CoordinationRecord>() == 48);
const _: () = assert!(core::mem::size_of::<PromotionIntentRequest>() == 56);
// A3/A7.2: report fixed admitted byte counts; compile-time checks reject ABI/layout drift.
const OPTION_TRACK_STATE_BYTES: usize = 160;
const OPTION_FLIGHT_STATE_BYTES: usize = 176;
const OPTION_HANDOFF_STATE_BYTES: usize = 152;
const OPTION_ALERT_STATE_BYTES: usize = 144;
const OPTION_TRACE_RECORD_BYTES: usize = 64;
const OPTION_DIFF_BYTES: usize = 64;
const _: () = assert!(core::mem::size_of::<Option<TrackState>>() == OPTION_TRACK_STATE_BYTES);
const _: () = assert!(core::mem::size_of::<Option<FlightState>>() == OPTION_FLIGHT_STATE_BYTES);
const _: () = assert!(core::mem::size_of::<Option<HandoffState>>() == OPTION_HANDOFF_STATE_BYTES);
const _: () = assert!(core::mem::size_of::<Option<AlertState>>() == OPTION_ALERT_STATE_BYTES);
const _: () = assert!(core::mem::size_of::<Option<TraceRecord>>() == OPTION_TRACE_RECORD_BYTES);
const _: () = assert!(core::mem::size_of::<Option<Diff>>() == OPTION_DIFF_BYTES);
// A7.2: selector is a cold control record; row/receipt capacity resides in startup-reserved buffers.
const _: () = assert!(core::mem::size_of::<Selector>() == 1_440);

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
    interfaced_facilities: &[u32],
) -> Result<Selector, Refusal> {
    new_selector_with_capacity(
        unit,
        policy,
        evidence,
        start_ns,
        interfaced_facilities,
        CapacityProfile::DEFAULT,
    )
}

/// Allocate one bounded selector profile. All vectors are reserved and filled here; later
/// operations only access existing slots. `REPLAY_4096` reserves 5,343,808 payload bytes
/// (~5.1 MiB) for both snapshots, 12,864 bytes for the transition trace, and up to 153,600 bytes
/// (~150 KiB) for 200×16 coordination rows. The budget excludes `Vec` headers and allocator
/// bookkeeping; row counts and all operations remain bounded by the declared capacities.
pub fn new_selector_with_capacity(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evidence: EvidenceKind,
    start_ns: i64,
    interfaced_facilities: &[u32],
    profile: CapacityProfile,
) -> Result<Selector, Refusal> {
    if !valid_unit(unit)
        || start_ns < unit.window_start_ns
        || !in_window(unit, start_ns)
        || interfaced_facilities.len() > MAX_INTERFACED_FACILITIES
        || !valid_capacity_profile(profile, interfaced_facilities.len())
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
    let mut configured_facilities = [0; MAX_INTERFACED_FACILITIES];
    for (index, facility_id) in interfaced_facilities.iter().copied().enumerate() {
        if facility_id == 0
            || interfaced_facilities
                .get(..index)
                .is_some_and(|previous| previous.contains(&facility_id))
        {
            return Err(Refusal::InvalidUnit);
        }
        *configured_facilities
            .get_mut(index)
            .ok_or(Refusal::InvalidUnit)? = facility_id;
    }
    let legacy = SnapshotStorage::new(profile)?;
    let candidate = SnapshotStorage::new(profile)?;
    let trace = filled_buffer(profile.trace_records, None)?;
    let coordination = filled_buffer(profile.coordination_records, EMPTY_COORDINATION_RECORD)?;
    Ok(Selector {
        unit,
        policy,
        evidence,
        profile,
        legacy,
        candidate,
        selected: Output::Legacy,
        output_sequence: 0,
        delivered_sequence: 0,
        last_authority_sequence: 0,
        last_audit_sequence: 0,
        last_audit_digest: [0; 32],
        last_output_time_ns: start_ns,
        last_delivery_time_ns: start_ns,
        pending_delivery: false,
        trace_len: 0,
        trace,
        interfaced_facilities: configured_facilities,
        interfaced_facility_count: u8::try_from(interfaced_facilities.len())
            .map_err(|_| Refusal::InvalidUnit)?,
        coordination_len: 0,
        coordination,
    })
}

fn valid_capacity_profile(profile: CapacityProfile, facility_count: usize) -> bool {
    profile.tracks > 0
        && profile.tracks <= REPLAY_ROWS_PER_FAMILY
        && profile.flights > 0
        && profile.flights <= REPLAY_ROWS_PER_FAMILY
        && profile.handoffs > 0
        && profile.handoffs <= REPLAY_ROWS_PER_FAMILY
        && profile.alerts > 0
        && profile.alerts <= REPLAY_ROWS_PER_FAMILY
        && profile.trace_records >= 2
        && profile.trace_records <= REPLAY_TRACE_RECORDS
        && profile.coordination_records <= REPLAY_COORDINATION_RECORDS
        && profile
            .trace_records
            .checked_sub(1)
            .and_then(|n| n.checked_mul(facility_count))
            .is_some_and(|required| profile.coordination_records >= required)
}

/// Startup memory reserved by the selector itself, excluding caller-owned reconcile diff output.
pub fn capacity_storage_budget_bytes(profile: CapacityProfile) -> Result<usize, Refusal> {
    if !valid_capacity_profile(profile, 0) {
        return Err(Refusal::InvalidUnit);
    }
    let per_snapshot = profile
        .tracks
        .checked_mul(OPTION_TRACK_STATE_BYTES)
        .and_then(|n| n.checked_add(profile.flights.checked_mul(OPTION_FLIGHT_STATE_BYTES)?))
        .and_then(|n| n.checked_add(profile.handoffs.checked_mul(OPTION_HANDOFF_STATE_BYTES)?))
        .and_then(|n| n.checked_add(profile.alerts.checked_mul(OPTION_ALERT_STATE_BYTES)?))
        .ok_or(Refusal::InvalidUnit)?;
    per_snapshot
        .checked_mul(2)
        .and_then(|n| {
            n.checked_add(
                profile
                    .trace_records
                    .checked_mul(OPTION_TRACE_RECORD_BYTES)?,
            )
        })
        .and_then(|n| {
            n.checked_add(
                profile
                    .coordination_records
                    .checked_mul(core::mem::size_of::<CoordinationRecord>())?,
            )
        })
        .ok_or(Refusal::InvalidUnit)
}

/// Caller-owned diff storage required for the profile's worst-case one-sided differences.
/// `REPLAY_4096` requires 2,097,152 payload bytes (2 MiB).
pub fn capacity_diff_budget_bytes(profile: CapacityProfile) -> Result<usize, Refusal> {
    if !valid_capacity_profile(profile, 0) {
        return Err(Refusal::InvalidUnit);
    }
    let differences = profile
        .tracks
        .checked_add(profile.flights)
        .and_then(|n| n.checked_add(profile.handoffs))
        .and_then(|n| n.checked_add(profile.alerts))
        .and_then(|n| n.checked_mul(2))
        .ok_or(Refusal::InvalidUnit)?;
    differences
        .checked_mul(OPTION_DIFF_BYTES)
        .ok_or(Refusal::InvalidUnit)
}

fn filled_buffer<T: Copy>(len: usize, value: T) -> Result<Vec<T>, Refusal> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(len)
        .map_err(|_| Refusal::StorageAllocationFailed)?;
    buffer.resize(len, value);
    Ok(buffer)
}

impl SnapshotStorage {
    fn new(profile: CapacityProfile) -> Result<Self, Refusal> {
        Ok(Self {
            header: None,
            tracks: filled_buffer(profile.tracks, None)?,
            flights: filled_buffer(profile.flights, None)?,
            handoffs: filled_buffer(profile.handoffs, None)?,
            alerts: filled_buffer(profile.alerts, None)?,
            track_len: 0,
            flight_len: 0,
            handoff_len: 0,
            alert_len: 0,
        })
    }

    fn view(&self) -> Option<SnapshotView<'_>> {
        let header = self.header?;
        Some(SnapshotView {
            unit: header.unit,
            policy: header.policy,
            evidence: header.evidence,
            side: header.side,
            source_epoch: header.source_epoch,
            state_version: header.state_version,
            sequence: header.sequence,
            source_time_ns: header.source_time_ns,
            observed_time_ns: header.observed_time_ns,
            health: header.health,
            track_coverage: header.track_coverage,
            flight_coverage: header.flight_coverage,
            handoff_coverage: header.handoff_coverage,
            alert_coverage: header.alert_coverage,
            tracks: self.tracks.get(..self.track_len)?,
            flights: self.flights.get(..self.flight_len)?,
            handoffs: self.handoffs.get(..self.handoff_len)?,
            alerts: self.alerts.get(..self.alert_len)?,
        })
    }

    fn store(&mut self, snapshot: SnapshotView<'_>) -> Result<(), Refusal> {
        let track_len = array_count(snapshot.tracks)?;
        let flight_len = array_count(snapshot.flights)?;
        let handoff_len = array_count(snapshot.handoffs)?;
        let alert_len = array_count(snapshot.alerts)?;
        if track_len > self.tracks.len()
            || flight_len > self.flights.len()
            || handoff_len > self.handoffs.len()
            || alert_len > self.alerts.len()
        {
            return Err(Refusal::CapacityExceeded);
        }
        let source_tracks = snapshot
            .tracks
            .get(..track_len)
            .ok_or(Refusal::BadSnapshot)?;
        let source_flights = snapshot
            .flights
            .get(..flight_len)
            .ok_or(Refusal::BadSnapshot)?;
        let source_handoffs = snapshot
            .handoffs
            .get(..handoff_len)
            .ok_or(Refusal::BadSnapshot)?;
        let source_alerts = snapshot
            .alerts
            .get(..alert_len)
            .ok_or(Refusal::BadSnapshot)?;
        // Preflight every source and destination range before copying any family. Stale values
        // beyond a new active length are never exposed by `view()`.
        self.tracks.get(..track_len).ok_or(Refusal::Incomplete)?;
        self.flights.get(..flight_len).ok_or(Refusal::Incomplete)?;
        self.handoffs
            .get(..handoff_len)
            .ok_or(Refusal::Incomplete)?;
        self.alerts.get(..alert_len).ok_or(Refusal::Incomplete)?;
        self.tracks
            .get_mut(..track_len)
            .ok_or(Refusal::Incomplete)?
            .copy_from_slice(source_tracks);
        self.flights
            .get_mut(..flight_len)
            .ok_or(Refusal::Incomplete)?
            .copy_from_slice(source_flights);
        self.handoffs
            .get_mut(..handoff_len)
            .ok_or(Refusal::Incomplete)?
            .copy_from_slice(source_handoffs);
        self.alerts
            .get_mut(..alert_len)
            .ok_or(Refusal::Incomplete)?
            .copy_from_slice(source_alerts);
        self.track_len = track_len;
        self.flight_len = flight_len;
        self.handoff_len = handoff_len;
        self.alert_len = alert_len;
        self.header = Some(SnapshotHeader {
            unit: snapshot.unit,
            policy: snapshot.policy,
            evidence: snapshot.evidence,
            side: snapshot.side,
            source_epoch: snapshot.source_epoch,
            state_version: snapshot.state_version,
            sequence: snapshot.sequence,
            source_time_ns: snapshot.source_time_ns,
            observed_time_ns: snapshot.observed_time_ns,
            health: snapshot.health,
            track_coverage: snapshot.track_coverage,
            flight_coverage: snapshot.flight_coverage,
            handoff_coverage: snapshot.handoff_coverage,
            alert_coverage: snapshot.alert_coverage,
        });
        Ok(())
    }
}

fn array_count<T: Copy>(items: &[Option<T>]) -> Result<usize, Refusal> {
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

fn valid_rows<T: Copy>(
    items: &[Option<T>],
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
    s: SnapshotView<'_>,
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
        s.tracks,
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
        s.flights,
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
        s.handoffs,
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
        s.alerts,
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
        (s.track_coverage, array_count(s.tracks)?),
        (s.flight_coverage, array_count(s.flights)?),
        (s.handoff_coverage, array_count(s.handoffs)?),
        (s.alert_coverage, array_count(s.alerts)?),
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
    ingest_view(selector, snapshot.view(), now_ns, max_age_ns)
}

/// Ingests a large or caller-backed normalized snapshot without constructing a multi-megabyte
/// stack `Snapshot`. The selector copies only validated rows into its preallocated family arrays.
pub fn ingest_view(
    selector: &mut Selector,
    snapshot: SnapshotView<'_>,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    if snapshot.tracks.len() > selector.profile.tracks
        || snapshot.flights.len() > selector.profile.flights
        || snapshot.handoffs.len() > selector.profile.handoffs
        || snapshot.alerts.len() > selector.profile.alerts
    {
        return Err(Refusal::CapacityExceeded);
    }
    validate_snapshot(
        snapshot,
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
    let storage = match snapshot.side {
        Side::Legacy => &mut selector.legacy,
        Side::Candidate => &mut selector.candidate,
    };
    if let Some(previous) = storage.view() {
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
    storage.store(snapshot)
}

// The two caller-owned output arguments replace the old embedded fixed-size report array.
#[allow(clippy::too_many_arguments)]
fn diff_rows<T: Copy>(
    a: &[Option<T>],
    b: &[Option<T>],
    kind: DiffKind,
    key: impl Fn(T) -> BusinessKey,
    status: impl Fn(T) -> FieldStatus,
    mask: impl Fn(T, T) -> u32,
    output: &mut [Option<Diff>],
    difference_count: &mut u16,
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
                        push_diff(
                            output,
                            difference_count,
                            Diff {
                                kind,
                                key: xk,
                                legacy_status: status(x),
                                candidate_status: status(y),
                                field_mask: fields,
                            },
                        )?;
                    }
                    i = i.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                    j = j.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                } else if xk < yk {
                    push_diff(
                        output,
                        difference_count,
                        Diff {
                            kind,
                            key: xk,
                            legacy_status: status(x),
                            candidate_status: FieldStatus::Absent,
                            field_mask: u32::MAX,
                        },
                    )?;
                    i = i.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                } else {
                    push_diff(
                        output,
                        difference_count,
                        Diff {
                            kind,
                            key: yk,
                            legacy_status: FieldStatus::Absent,
                            candidate_status: status(y),
                            field_mask: u32::MAX,
                        },
                    )?;
                    j = j.checked_add(1).ok_or(Refusal::BadSnapshot)?;
                }
            }
            (Some(x), None) => {
                push_diff(
                    output,
                    difference_count,
                    Diff {
                        kind,
                        key: key(x),
                        legacy_status: status(x),
                        candidate_status: FieldStatus::Absent,
                        field_mask: u32::MAX,
                    },
                )?;
                i = i.checked_add(1).ok_or(Refusal::BadSnapshot)?;
            }
            (None, Some(y)) => {
                push_diff(
                    output,
                    difference_count,
                    Diff {
                        kind,
                        key: key(y),
                        legacy_status: FieldStatus::Absent,
                        candidate_status: status(y),
                        field_mask: u32::MAX,
                    },
                )?;
                j = j.checked_add(1).ok_or(Refusal::BadSnapshot)?;
            }
        }
    }
}

fn push_diff(
    output: &mut [Option<Diff>],
    difference_count: &mut u16,
    diff: Diff,
) -> Result<(), Refusal> {
    let index = usize::from(*difference_count);
    *output.get_mut(index).ok_or(Refusal::DifferenceOverflow)? = Some(diff);
    *difference_count = difference_count
        .checked_add(1)
        .ok_or(Refusal::DifferenceOverflow)?;
    Ok(())
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
    output: &mut [Option<Diff>],
) -> Result<(ReconcileSummary, Option<ReconcileToken>), Refusal> {
    let required_diffs = selector
        .profile
        .tracks
        .checked_add(selector.profile.flights)
        .and_then(|n| n.checked_add(selector.profile.handoffs))
        .and_then(|n| n.checked_add(selector.profile.alerts))
        .and_then(|n| n.checked_mul(2))
        .ok_or(Refusal::DifferenceOverflow)?;
    if output.len() < required_diffs {
        return Err(Refusal::DifferenceOverflow);
    }
    output.fill(None);
    let l = selector.legacy.view().ok_or(Refusal::Incomplete)?;
    let c = selector.candidate.view().ok_or(Refusal::Incomplete)?;
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
    let mut report = ReconcileSummary {
        compared_tracks: u16::try_from(array_count(l.tracks)?).map_err(|_| Refusal::BadSnapshot)?,
        compared_flights: u16::try_from(array_count(l.flights)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        compared_handoffs: u16::try_from(array_count(l.handoffs)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        compared_alerts: u16::try_from(array_count(l.alerts)?).map_err(|_| Refusal::BadSnapshot)?,
        candidate_tracks: u16::try_from(array_count(c.tracks)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        candidate_flights: u16::try_from(array_count(c.flights)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        candidate_handoffs: u16::try_from(array_count(c.handoffs)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        candidate_alerts: u16::try_from(array_count(c.alerts)?)
            .map_err(|_| Refusal::BadSnapshot)?,
        differences: 0,
        as_of_ns: core::cmp::min(l.observed_time_ns, c.observed_time_ns),
        ready: false,
    };
    diff_rows(
        l.tracks,
        c.tracks,
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
        output,
        &mut report.differences,
    )?;
    diff_rows(
        l.flights,
        c.flights,
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
        output,
        &mut report.differences,
    )?;
    diff_rows(
        l.handoffs,
        c.handoffs,
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
        output,
        &mut report.differences,
    )?;
    diff_rows(
        l.alerts,
        c.alerts,
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
        output,
        &mut report.differences,
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

fn to_reconcile_report(
    summary: ReconcileSummary,
    diffs: [Option<Diff>; MAX_DIFFS],
) -> ReconcileReport {
    ReconcileReport {
        compared_tracks: summary.compared_tracks,
        compared_flights: summary.compared_flights,
        compared_handoffs: summary.compared_handoffs,
        compared_alerts: summary.compared_alerts,
        candidate_tracks: summary.candidate_tracks,
        candidate_flights: summary.candidate_flights,
        candidate_handoffs: summary.candidate_handoffs,
        candidate_alerts: summary.candidate_alerts,
        differences: summary.differences,
        diffs,
        as_of_ns: summary.as_of_ns,
        ready: summary.ready,
    }
}

pub fn reconcile(
    selector: &Selector,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<ReconcileReport, Refusal> {
    if selector.profile.tracks != MAX_TRACKS
        || selector.profile.flights != MAX_FLIGHTS
        || selector.profile.handoffs != MAX_HANDOFFS
        || selector.profile.alerts != MAX_ALERTS
        || selector
            .profile
            .tracks
            .checked_add(selector.profile.flights)
            .and_then(|n| n.checked_add(selector.profile.handoffs))
            .and_then(|n| n.checked_add(selector.profile.alerts))
            .and_then(|n| n.checked_mul(2))
            != Some(MAX_DIFFS)
    {
        return Err(Refusal::DifferenceOverflow);
    }
    let mut diffs = [None; MAX_DIFFS];
    let (summary, _) = reconcile_inner(selector, now_ns, max_age_ns, &mut diffs)?;
    Ok(to_reconcile_report(summary, diffs))
}

/// The profile's maximum pairwise diff count, checked against its declared bounds.
pub fn profile_diff_capacity(profile: CapacityProfile) -> Result<usize, Refusal> {
    if !valid_capacity_profile(profile, 0) {
        return Err(Refusal::InvalidUnit);
    }
    profile
        .tracks
        .checked_add(profile.flights)
        .and_then(|n| n.checked_add(profile.handoffs))
        .and_then(|n| n.checked_add(profile.alerts))
        .and_then(|n| n.checked_mul(2))
        .ok_or(Refusal::InvalidUnit)
}

pub fn reconcile_into(
    selector: &Selector,
    now_ns: i64,
    max_age_ns: i64,
    diffs: &mut [Option<Diff>],
) -> Result<ReconcileSummary, Refusal> {
    reconcile_inner(selector, now_ns, max_age_ns, diffs).map(|(summary, _)| summary)
}

fn token_matches(selector: &Selector, token: ReconcileToken) -> bool {
    matches!((selector.legacy.view(), selector.candidate.view()), (Some(l), Some(c)) if token.unit == selector.unit && token.policy == selector.policy && token.legacy_epoch == l.source_epoch && token.candidate_epoch == c.source_epoch && token.legacy_version == l.state_version && token.candidate_version == c.state_version && token.legacy_sequence == l.sequence && token.candidate_sequence == c.sequence)
}

pub fn prepare_candidate(
    selector: &Selector,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(ReconcileReport, CandidateReadiness), Refusal> {
    if profile_diff_capacity(selector.profile)? != MAX_DIFFS {
        return Err(Refusal::DifferenceOverflow);
    }
    let mut diffs = [None; MAX_DIFFS];
    let (summary, token) = reconcile_inner(selector, now_ns, max_age_ns, &mut diffs)?;
    let token = token.ok_or(Refusal::StateMismatch)?;
    Ok((
        to_reconcile_report(summary, diffs),
        CandidateReadiness { token },
    ))
}

pub fn prepare_candidate_into(
    selector: &Selector,
    now_ns: i64,
    max_age_ns: i64,
    diffs: &mut [Option<Diff>],
) -> Result<(ReconcileSummary, CandidateReadiness), Refusal> {
    let (summary, token) = reconcile_inner(selector, now_ns, max_age_ns, diffs)?;
    let token = token.ok_or(Refusal::StateMismatch)?;
    Ok((summary, CandidateReadiness { token }))
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

fn switch_audit_event(
    selector: &Selector,
    action: oa_audit_core::Action,
    event_key: [u8; 32],
    principal: u64,
    time_ns: i64,
    authority_sequence: u64,
) -> Result<oa_audit_core::Event, Refusal> {
    let time_ns = u64::try_from(time_ns).map_err(|_| Refusal::TimeOverflow)?;
    let (target, target_len) = oa_audit_core::digest_target(&event_key);
    let mut payload = [0u8; 108];
    let mut pos = 0usize;
    append_audit_bytes(
        &mut payload,
        &mut pos,
        &u32::to_le_bytes(selector.unit.function_id),
    )?;
    append_audit_bytes(
        &mut payload,
        &mut pos,
        &u32::to_le_bytes(selector.unit.scope_id),
    )?;
    append_audit_bytes(
        &mut payload,
        &mut pos,
        &i64::to_le_bytes(selector.unit.window_start_ns),
    )?;
    append_audit_bytes(
        &mut payload,
        &mut pos,
        &i64::to_le_bytes(selector.unit.window_end_ns),
    )?;
    append_audit_bytes(
        &mut payload,
        &mut pos,
        &u32::to_le_bytes(selector.policy.policy_version),
    )?;
    append_audit_bytes(&mut payload, &mut pos, &selector.policy.policy_digest)?;
    append_audit_bytes(&mut payload, &mut pos, &selector.policy.source_epoch_digest)?;
    append_audit_bytes(
        &mut payload,
        &mut pos,
        &u64::to_le_bytes(authority_sequence),
    )?;
    let (details, details_len) =
        oa_audit_core::details(payload.get(..pos).ok_or(Refusal::AuditRequired)?)
            .map_err(|_| Refusal::AuditRequired)?;
    Ok(oa_audit_core::Event {
        principal,
        role: oa_audit_core::Role::Configure,
        action,
        target,
        target_len,
        time_ns,
        details,
        details_len,
    })
}

fn append_audit_bytes(dst: &mut [u8], pos: &mut usize, bytes: &[u8]) -> Result<(), Refusal> {
    let end = pos.checked_add(bytes.len()).ok_or(Refusal::AuditRequired)?;
    dst.get_mut(*pos..end)
        .ok_or(Refusal::AuditRequired)?
        .copy_from_slice(bytes);
    *pos = end;
    Ok(())
}

/// Construct the exact plain audit intent a shell must durably append before selection.
pub fn candidate_audit_event(
    selector: &Selector,
    intent: &PromotionIntent,
    now_ns: i64,
) -> Result<oa_audit_core::Event, Refusal> {
    if intent.sequence == 0
        || intent.principal_id == 0
        || intent.principal_role_id != M1_PROMOTION_SUPERVISOR_ROLE_ID
        || intent.event_key == [0; 32]
    {
        return Err(Refusal::Unauthorized);
    }
    switch_audit_event(
        selector,
        oa_audit_core::Action::MigrationFlip,
        intent.event_key,
        u64::from(intent.principal_id),
        now_ns,
        intent.sequence,
    )
}

/// Construct the exact plain audit intent a shell must durably append before rollback.
pub fn rollback_audit_event(
    selector: &Selector,
    cause: RollbackReceipt,
    now_ns: i64,
) -> Result<oa_audit_core::Event, Refusal> {
    if cause.event_id == [0; 32] || cause.audit_principal_id == 0 {
        return Err(Refusal::Unauthorized);
    }
    switch_audit_event(
        selector,
        oa_audit_core::Action::MigrationRollback,
        cause.event_id,
        cause.audit_principal_id,
        now_ns,
        cause.sequence,
    )
}

fn validate_switch_audit(
    selector: &Selector,
    receipt: &oa_audit_core::AuditReceipt,
    event: oa_audit_core::Event,
) -> Result<(), Refusal> {
    let next_sequence = selector
        .last_audit_sequence
        .checked_add(1)
        .ok_or(Refusal::AuditRequired)?;
    if receipt.sequence() != next_sequence
        || receipt.previous() != selector.last_audit_digest
        || receipt.action() != event.action
        || receipt.role() != oa_audit_core::Role::Configure
        || receipt.principal() != event.principal
        || !oa_audit_core::receipt_matches(receipt, event)
    {
        return Err(Refusal::AuditRequired);
    }
    Ok(())
}

/// Expected predecessor for the selector's dedicated migration audit chain.
pub fn audit_checkpoint(selector: &Selector) -> oa_audit_core::Checkpoint {
    oa_audit_core::Checkpoint {
        count: selector.last_audit_sequence,
        tail: selector.last_audit_digest,
    }
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
    audit: oa_audit_core::AuditReceipt,
) -> Result<(), Refusal> {
    let audit_event = candidate_audit_event(selector, intent, now_ns)?;
    validate_switch_audit(selector, &audit, audit_event)?;
    selector.last_audit_sequence = audit.sequence();
    selector.last_audit_digest = audit.digest();
    if selector.selected != Output::Legacy {
        return Err(Refusal::StateMismatch);
    }
    // Keep one trace slot available for an immediate rollback after promotion.
    if usize::from(selector.trace_len) >= selector.profile.trace_records - 1 {
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
    let _ = selector.candidate.view().ok_or(Refusal::Incomplete)?;
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
    audit: oa_audit_core::AuditReceipt,
) -> Result<(), Refusal> {
    let audit_event = rollback_audit_event(selector, cause, now_ns)?;
    validate_switch_audit(selector, &audit, audit_event)?;
    selector.last_audit_sequence = audit.sequence();
    selector.last_audit_digest = audit.digest();
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
    let l = selector.legacy.view().ok_or(Refusal::Incomplete)?;
    let c = selector.candidate.view().ok_or(Refusal::Incomplete)?;
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
    if i >= selector.trace.len() {
        return Err(Refusal::TraceFull);
    }
    let coordination_start = usize::from(selector.coordination_len);
    let coordination_count = usize::from(selector.interfaced_facility_count);
    let needed = coordination_start
        .checked_add(coordination_count)
        .ok_or(Refusal::CoordinationFull)?;
    // A promotion leaves room for every configured facility to receive the immediate rollback
    // record as well. Otherwise it could switch successfully and then strand rollback evidence.
    let reserved = if to == Output::Candidate {
        coordination_count
    } else {
        0
    };
    let required = needed
        .checked_add(reserved)
        .ok_or(Refusal::CoordinationFull)?;
    if required > selector.coordination.len() {
        return Err(Refusal::CoordinationFull);
    }
    // Append one bounded plain record per configured facility before changing the selected side.
    let mut facility_index = 0usize;
    while facility_index < coordination_count {
        let facility_id = *selector
            .interfaced_facilities
            .get(facility_index)
            .ok_or(Refusal::CoordinationFull)?;
        let record = CoordinationRecord {
            output_sequence: sequence,
            unit: selector.unit,
            at_ns,
            facility_id,
            from: selector.selected,
            to,
            reason,
        };
        *selector
            .coordination
            .get_mut(coordination_start + facility_index)
            .ok_or(Refusal::CoordinationFull)? = record;
        facility_index = facility_index
            .checked_add(1)
            .ok_or(Refusal::CoordinationFull)?;
    }
    selector.coordination_len = u16::try_from(needed).map_err(|_| Refusal::CoordinationFull)?;
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
pub fn trace(selector: &Selector) -> (&[Option<TraceRecord>], u16) {
    (&selector.trace, selector.trace_len)
}

/// Returns coordination notices in deterministic append order. These records are emitted as
/// local evidence only; sending anything to an interfaced facility is outside this core.
pub fn coordination_records(selector: &Selector) -> (&[CoordinationRecord], u16) {
    (&selector.coordination, selector.coordination_len)
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
    extern crate std;

    use super::*;
    use oa_migration::{
        AuthorityApproval as M1Approval, AuthorityRole as M1Role, Comparison as M1Comparison,
        EvidenceQuality as M1Quality, MetricCriterion as M1Criterion, MetricEvidence as M1Metric,
        RollbackRule as M1RollbackRule, RollbackTriggerPolicy as M1RollbackPolicy,
        Stage as M1Stage, SwapUnitKey as M1Key, TransitionInput as M1Input,
        TransitionPolicy as M1Policy,
    };

    fn test_audit_receipt(
        selector: &Selector,
        event: oa_audit_core::Event,
    ) -> Result<oa_audit_core::AuditReceipt, Refusal> {
        let expected = audit_checkpoint(selector);
        let sequence = expected
            .count
            .checked_add(1)
            .ok_or(Refusal::AuditRequired)?;
        let mut digest = [0; 32];
        digest
            .get_mut(..8)
            .ok_or(Refusal::AuditRequired)?
            .copy_from_slice(&u64::to_le_bytes(sequence));
        Ok(oa_audit_core::receipt_claim(
            sequence,
            digest,
            expected.tail,
            event,
        ))
    }

    fn audited_select_candidate(
        selector: &mut Selector,
        readiness: &CandidateReadiness,
        authorization: &WindowAuthorization,
        intent: &PromotionIntent,
        now_ns: i64,
        max_age_ns: i64,
    ) -> Result<(), Refusal> {
        let event = candidate_audit_event(selector, intent, now_ns)?;
        let audit = test_audit_receipt(selector, event)?;
        super::select_candidate(
            selector,
            readiness,
            authorization,
            intent,
            now_ns,
            max_age_ns,
            audit,
        )
    }

    fn audited_rollback(
        selector: &mut Selector,
        cause: RollbackReceipt,
        now_ns: i64,
        max_age_ns: i64,
    ) -> Result<(), Refusal> {
        let event = rollback_audit_event(selector, cause, now_ns)?;
        let audit = test_audit_receipt(selector, event)?;
        super::rollback(selector, cause, now_ns, max_age_ns, audit)
    }

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
        let selector =
            new_selector(unit, policy, EvidenceKind::Synthetic, 10, &[1001, 1002]).unwrap();
        (selector, l, c)
    }

    // REQ: OA-MIG-201, OA-MIG-203, OA-MIG-204, OA-MIG-205, OA-AUDIT-003, OA-AUDIT-004, OA-AUDIT-005
    #[test]
    fn h1_h5_make_before_break_and_rollback() {
        let (mut s, l, c) = fixture();
        assert_eq!(prepare_candidate(&s, 25, 100), Err(Refusal::Incomplete));
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let intent = intent(&s, &ready, &authorization, 1, 24);
        let event = candidate_audit_event(&s, &intent, 25).unwrap();
        let mut wrong_event = event;
        if let Some(first) = wrong_event.target.first_mut() {
            *first = b'X';
        }
        let wrong_receipt =
            oa_audit_core::receipt_claim(1, [1; 32], oa_audit_core::genesis().tail, wrong_event);
        assert_eq!(
            select_candidate(
                &mut s,
                &ready,
                &authorization,
                &intent,
                25,
                100,
                wrong_receipt
            ),
            Err(Refusal::AuditRequired)
        );
        assert_eq!(selected(&s), Output::Legacy);
        assert_eq!(trace(&s).1, 0);
        let audit = test_audit_receipt(&s, event).unwrap();
        select_candidate(&mut s, &ready, &authorization, &intent, 25, 100, audit).unwrap();
        assert_eq!(
            select_candidate(&mut s, &ready, &authorization, &intent, 25, 100, audit),
            Err(Refusal::AuditRequired)
        );
        assert_eq!(selected(&s), Output::Candidate);
        let (notices, notice_count) = coordination_records(&s);
        assert_eq!(notice_count, 2);
        assert_eq!(notices[0].facility_id, 1001);
        assert_eq!(notices[1].facility_id, 1002);
        assert_eq!(notices[0].to, Output::Candidate);
        assert_eq!(notices[0].output_sequence, 1);
        assert_eq!(notices[1].output_sequence, 1);
        observe_delivery(&mut s, 1, Output::Candidate, 1, 26).unwrap();
        let unit = s.unit;
        audited_rollback(
            &mut s,
            RollbackReceipt {
                unit,
                kind: RollbackKind::ControllerRequest,
                principal_id: [8; 32],
                principal_role_id: 1,
                audit_principal_id: 11,
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
        let (notices, notice_count) = coordination_records(&s);
        assert_eq!(notice_count, 4);
        assert_eq!(notices[2].facility_id, 1001);
        assert_eq!(notices[3].facility_id, 1002);
        assert_eq!(notices[2].to, Output::Legacy);
        assert_eq!(notices[2].reason, 2);
        assert_eq!(notices[2].output_sequence, 2);
        assert_eq!(
            audited_select_candidate(&mut s, &ready, &authorization, &intent, 28, 100),
            Err(Refusal::Unauthorized)
        );
        assert_eq!(
            audited_select_candidate(&mut s, &ready, &authorization, &intent, 28, 100),
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

    // REQ: OA-MIG-208
    #[test]
    fn coordination_capacity_refuses_flip_before_any_notice_or_selection() {
        let (mut s, l, c) = fixture();
        ingest(&mut s, l, 25, 100).unwrap();
        ingest(&mut s, c, 25, 100).unwrap();
        let (_, ready) = prepare_candidate(&s, 25, 100).unwrap();
        let authorization = promotion(&s, 24);
        let intent = intent(&s, &ready, &authorization, 1, 24);
        s.coordination_len = u16::try_from(MAX_COORDINATION_RECORDS - 3).unwrap();
        assert_eq!(
            audited_select_candidate(&mut s, &ready, &authorization, &intent, 25, 100),
            Err(Refusal::CoordinationFull)
        );
        assert_eq!(selected(&s), Output::Legacy);
        assert_eq!(trace(&s).1, 0);
        assert_eq!(
            coordination_records(&s).1,
            u16::try_from(MAX_COORDINATION_RECORDS - 3).unwrap()
        );
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
            audited_select_candidate(&mut s, &ready, &authorization, &stale_intent, 30, 100),
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
        s.candidate.header.as_mut().unwrap().source_epoch = 99;
        assert_eq!(
            audited_select_candidate(&mut s, &ready, &authorization, &intent, 25, 100),
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
        audited_select_candidate(&mut s, &first_ready, &authorization, &first_intent, 25, 100)
            .unwrap();
        observe_delivery(&mut s, 1, Output::Candidate, 1, 26).unwrap();
        let unit = s.unit;
        audited_rollback(
            &mut s,
            RollbackReceipt {
                unit,
                kind: RollbackKind::HealthFault,
                principal_id: [8; 32],
                principal_role_id: 1,
                audit_principal_id: 11,
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
        audited_select_candidate(
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
            audit_principal_id: 11,
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
            audited_rollback(&mut s, repeated_rollback, 30, 100),
            Err(Refusal::Unauthorized)
        );
        repeated_rollback.event_id = [9; 32];
        audited_rollback(&mut s, repeated_rollback, 30, 100).unwrap();
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
        audited_select_candidate(&mut s, &ready, &authorization, &intent, 25, 100).unwrap();
        let unit = s.unit;
        let cause = RollbackReceipt {
            unit,
            kind: RollbackKind::HealthFault,
            principal_id: [8; 32],
            principal_role_id: 1,
            audit_principal_id: 11,
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
            audited_rollback(&mut s, unhealthy, 25, 100),
            Err(Refusal::Unhealthy)
        );
        let mut unavailable = cause;
        unavailable.legacy_source_available = false;
        assert_eq!(
            audited_rollback(&mut s, unavailable, 25, 100),
            Err(Refusal::Unhealthy)
        );
        audited_rollback(&mut s, cause, 25, 100).unwrap();
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
            audited_select_candidate(&mut s, &ready, &other_grant, &valid_intent, 25, 100),
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
            audited_select_candidate(&mut s, &ready, &valid, &valid_intent, window_end_ns, 100),
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
        audited_select_candidate(&mut s, &ready, &valid, &valid_intent, 25, 100).unwrap();
        observe_delivery(&mut s, 1, Output::Candidate, 1, 26).unwrap();
        let unit = s.unit;
        audited_rollback(
            &mut s,
            RollbackReceipt {
                unit,
                kind: RollbackKind::HealthFault,
                principal_id: [8; 32],
                principal_role_id: 1,
                audit_principal_id: 11,
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
            audited_select_candidate(&mut s, &ready, &valid, &valid_intent, 28, 100),
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
        audited_select_candidate(&mut s, &ready, &authorization, &intent, 25, 100).unwrap();
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
            audit_principal_id: 11,
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
        audited_rollback(&mut s, rollback_receipt, 25, 100).unwrap();
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
            audited_select_candidate(&mut s, &ready, &authorization, &intent, 26, 100),
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
        audited_select_candidate(&mut s, &ready, &authorization, &intent, 25, 100).unwrap();
        assert_eq!(
            observe_delivery(&mut s, 9, Output::Candidate, 1, 26),
            Err(Refusal::WrongUnit)
        );
    }

    #[test]
    fn replay_profile_accepts_max_rows_in_each_family() {
        let (default_selector, legacy, candidate) = fixture();
        let mut legacy_rows = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        let mut candidate_rows = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        let mut legacy_flights = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        let mut candidate_flights = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        let mut legacy_handoffs = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        let mut candidate_handoffs = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        let mut legacy_alerts = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        let mut candidate_alerts = Vec::with_capacity(REPLAY_ROWS_PER_FAMILY);
        for ordinal in 1..=u64::try_from(REPLAY_ROWS_PER_FAMILY).unwrap() {
            let mut lrow = legacy.tracks[0].unwrap();
            lrow.key.ordinal = ordinal;
            lrow.source_identity.decoded_offset = ordinal;
            lrow.source_identity.row = u32::try_from(ordinal).unwrap();
            let mut crow = candidate.tracks[0].unwrap();
            crow.key.ordinal = ordinal;
            crow.source_identity.decoded_offset = ordinal;
            crow.source_identity.row = u32::try_from(ordinal).unwrap();
            legacy_rows.push(Some(lrow));
            candidate_rows.push(Some(crow));

            let mut lrow = legacy.flights[0].unwrap();
            lrow.key.ordinal = ordinal;
            lrow.source_identity.decoded_offset = ordinal;
            lrow.source_identity.row = u32::try_from(ordinal).unwrap();
            let mut crow = candidate.flights[0].unwrap();
            crow.key.ordinal = ordinal;
            crow.source_identity.decoded_offset = ordinal;
            crow.source_identity.row = u32::try_from(ordinal).unwrap();
            legacy_flights.push(Some(lrow));
            candidate_flights.push(Some(crow));

            let mut lrow = legacy.handoffs[0].unwrap();
            lrow.key.ordinal = ordinal;
            lrow.source_identity.decoded_offset = ordinal;
            lrow.source_identity.row = u32::try_from(ordinal).unwrap();
            let mut crow = candidate.handoffs[0].unwrap();
            crow.key.ordinal = ordinal;
            crow.source_identity.decoded_offset = ordinal;
            crow.source_identity.row = u32::try_from(ordinal).unwrap();
            legacy_handoffs.push(Some(lrow));
            candidate_handoffs.push(Some(crow));

            let mut lrow = legacy.alerts[0].unwrap();
            lrow.key.ordinal = ordinal;
            lrow.source_identity.decoded_offset = ordinal;
            lrow.source_identity.row = u32::try_from(ordinal).unwrap();
            let mut crow = candidate.alerts[0].unwrap();
            crow.key.ordinal = ordinal;
            crow.source_identity.decoded_offset = ordinal;
            crow.source_identity.row = u32::try_from(ordinal).unwrap();
            legacy_alerts.push(Some(lrow));
            candidate_alerts.push(Some(crow));
        }
        let mut legacy_view = legacy.view();
        legacy_view.tracks = &legacy_rows;
        legacy_view.flights = &legacy_flights;
        legacy_view.handoffs = &legacy_handoffs;
        legacy_view.alerts = &legacy_alerts;
        let mut candidate_view = candidate.view();
        candidate_view.tracks = &candidate_rows;
        candidate_view.flights = &candidate_flights;
        candidate_view.handoffs = &candidate_handoffs;
        candidate_view.alerts = &candidate_alerts;
        let mut default_selector = default_selector;
        assert_eq!(
            ingest_view(&mut default_selector, legacy_view, 25, 100),
            Err(Refusal::CapacityExceeded)
        );

        let selector = new_selector_with_capacity(
            legacy.unit,
            legacy.policy,
            legacy.evidence,
            10,
            &[1001, 1002],
            CapacityProfile::REPLAY_4096,
        )
        .unwrap();
        let mut selector = selector;
        ingest_view(&mut selector, legacy_view, 25, 100).unwrap();
        ingest_view(&mut selector, candidate_view, 25, 100).unwrap();
        let mut diffs = Vec::with_capacity(REPLAY_MAX_DIFFS);
        diffs.resize(REPLAY_MAX_DIFFS, None);
        let summary = reconcile_into(&selector, 25, 100, &mut diffs).unwrap();
        assert_eq!(summary.compared_tracks, 4_096);
        assert_eq!(summary.candidate_tracks, 4_096);
        assert_eq!(summary.compared_flights, 4_096);
        assert_eq!(summary.candidate_flights, 4_096);
        assert_eq!(summary.compared_handoffs, 4_096);
        assert_eq!(summary.candidate_handoffs, 4_096);
        assert_eq!(summary.compared_alerts, 4_096);
        assert_eq!(summary.candidate_alerts, 4_096);
        assert!(summary.ready);
        assert_eq!(
            capacity_storage_budget_bytes(CapacityProfile::REPLAY_4096),
            Ok(5_343_808)
        );
        assert_eq!(
            capacity_diff_budget_bytes(CapacityProfile::REPLAY_4096),
            Ok(2_097_152)
        );
    }

    #[test]
    fn replay_profile_retains_receipts_for_one_hundred_switch_and_rollback_cycles() {
        let (_, mut legacy, mut candidate) = fixture();
        let mut selector = new_selector_with_capacity(
            legacy.unit,
            legacy.policy,
            legacy.evidence,
            10,
            &[
                1001, 1002, 1003, 1004, 1005, 1006, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014,
                1015, 1016,
            ],
            CapacityProfile::REPLAY_4096,
        )
        .unwrap();
        let mut diffs = Vec::with_capacity(REPLAY_MAX_DIFFS);
        diffs.resize(REPLAY_MAX_DIFFS, None);
        let authorization = promotion(&selector, 24);
        for cycle in 0..100u64 {
            let now = 25 + i64::try_from(cycle * 2).unwrap();
            let next_sequence = cycle + 1;
            legacy.sequence = next_sequence;
            candidate.sequence = next_sequence;
            legacy.state_version = next_sequence;
            candidate.state_version = next_sequence;
            legacy.source_time_ns = now;
            candidate.source_time_ns = now;
            legacy.observed_time_ns = now;
            candidate.observed_time_ns = now;
            ingest_view(&mut selector, legacy.view(), now, 100).unwrap();
            ingest_view(&mut selector, candidate.view(), now, 100).unwrap();
            let (_, readiness) = prepare_candidate_into(&selector, now, 100, &mut diffs).unwrap();
            let intent_sequence = cycle * 2 + 1;
            let switch_intent = intent(&selector, &readiness, &authorization, intent_sequence, now);
            select_candidate(
                &mut selector,
                &readiness,
                &authorization,
                &switch_intent,
                now,
                100,
            )
            .unwrap();
            observe_delivery(&mut selector, 1, Output::Candidate, intent_sequence, now).unwrap();
            let rollback_sequence = intent_sequence + 1;
            let rollback_at = now + 1;
            let unit = selector.unit;
            let policy = selector.policy;
            rollback(
                &mut selector,
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
                    sequence: rollback_sequence,
                    legacy_epoch: legacy.source_epoch,
                    candidate_epoch: candidate.source_epoch,
                    legacy_state_version: legacy.state_version,
                    candidate_state_version: candidate.state_version,
                    legacy_sequence: legacy.sequence,
                    candidate_sequence: candidate.sequence,
                    legacy_health: Health::Healthy,
                    legacy_source_available: true,
                    legacy_health_observed_ns: rollback_at,
                    event_id: [u8::try_from(cycle + 1).unwrap(); 32],
                    observed_ns: rollback_at,
                },
                rollback_at,
                100,
            )
            .unwrap();
            observe_delivery(
                &mut selector,
                1,
                Output::Legacy,
                rollback_sequence,
                rollback_at,
            )
            .unwrap();
        }
        let (records, trace_count) = trace(&selector);
        let (notices, notice_count) = coordination_records(&selector);
        assert_eq!(trace_count, 200);
        assert_eq!(notice_count, 3_200);
        assert_eq!(records.iter().take(200).flatten().count(), 200);
        assert_eq!(notices.len(), REPLAY_COORDINATION_RECORDS);
    }
}

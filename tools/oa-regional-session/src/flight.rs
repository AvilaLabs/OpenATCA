//! Adapter from normalized controller operations to the authoritative `oa-flight` table.
//! Shell (RULESET B1, B3, B7): timestamps and event identity are checked here; state transitions,
//! stamps, refusal counts and conflicts belong to `oa-flight`.

use oa_flight::{
    flight_get, flights_apply, flights_claiming_address, flights_len, flights_new, key_gufi,
    op_new, Alt, Flight, FlightConfig, FlightTable, Outcome, Stamp, ALT_NIL, ALT_NONE, ALT_SIMPLE,
    C_COUNT, HO_NONE, MAX_VIEWS, OP_ADDRESS, OP_ASSIGN, OP_CANCEL, OP_COUNT, OP_HO_ACCEPT,
    OP_HO_FAIL, OP_HO_INITIATE, OP_HO_RETRACT, OP_HO_TAKE, OP_HO_UPDATE, OP_IDENTITY, OP_INTERIM,
    OP_OWNER, OP_SET_PLAN, R_COUNT, ST_ACTIVE, ST_PROPOSED,
};

use crate::input::{EventKind, FlightOperation, HandoffOperation, NormalizedEvent, SourceKind};

/// The driver never admits more than this many flights. A regional demonstration is deliberately
/// small; larger deployment sizing belongs to a separate explicit configuration review.
pub const MAX_DRIVER_FLIGHTS: usize = 32;
/// Maximum opt-in offline table capacity; the default driver remains capped at 32.
pub const MAX_OFFLINE_DRIVER_FLIGHTS: usize = 8192;
/// `oa-flight` key-index cells at the maximum offline capacity: `next_power_of_two(2 * entries)`.
pub const MAX_OFFLINE_DRIVER_KEY_INDEX_CELLS: usize =
    (MAX_OFFLINE_DRIVER_FLIGHTS * 2).next_power_of_two();
/// Maximum entries in either externally visible flight log.
pub const MAX_DRIVER_LOG_ENTRIES: usize = 1024;

/// Cold configuration for the bounded adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlightDriverConfig {
    pub flight_capacity: usize,
    pub refusal_log_capacity: usize,
    pub conflict_log_capacity: usize,
}

/// Construction or input refusal. A7.2: cold shell enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightDriverError {
    InvalidConfig,
    InvalidTimestamp,
    SealedTimestamp,
    NonMillisecondAdapterTimestamp,
    WrongSourceKind,
    InvalidBatchOrder,
    ReceiptBatchTooLarge,
}

/// A controller adapter refusal that could not be represented by an existing core transition.
/// It is counted separately, with a stable reason, rather than forging a core operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterRefusal {
    OwnerChangeRequiresTake,
    AcceptingUnitIsNotOfferedTarget,
    TakingUnitIsNotOfferedTarget,
    OfferRequiresCurrentOwner,
    FailingUnitIsNotOfferingOwner,
}

/// Result of a normalized event. An explicit address/identity event may also be refused by the
/// core; ordinary events have only `operation`. A7.2: cold shell value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlightApplyOutcome {
    pub operation: Option<Outcome>,
    pub adapter_refusal: Option<AdapterRefusal>,
    pub ignored_non_flight_event: bool,
}

/// Counters owned by this adapter, including adapter-level refusals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlightDriverStats {
    pub events_seen: u64,
    pub flight_events: u64,
    pub adapter_refusals: u64,
    pub invalid_timestamps: u64,
    pub source_mismatch_refusals: u64,
    pub timestamp_precision_refusals: u64,
    pub event_order_refusals: u64,
    pub future_snapshot_refusals: u64,
    pub unknown_centre_queries: u64,
    pub unknown_flight_queries: u64,
    pub output_capacity_refusals: u64,
    pub owner_change_requires_take: u64,
    pub accepting_unit_mismatches: u64,
    pub taking_unit_mismatches: u64,
    pub offers_without_owner: u64,
    pub failing_unit_mismatches: u64,
}

/// Complete bounded-flight accounting snapshot, including core log overflow counters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlightDriverReport {
    pub adapter: FlightDriverStats,
    pub flights: usize,
    pub operations: u64,
    pub applied: u64,
    pub unchanged: u64,
    pub refused_by_operation_reason: [u64; OP_COUNT * R_COUNT],
    pub refusals_logged: usize,
    pub refusals_unlisted: u64,
    pub conflicts_by_kind: [u64; C_COUNT],
    pub conflicts_logged: usize,
    pub conflicts_unlisted: u64,
}

/// Bounded bridge state. The core table and all of its indexes remain encapsulated by this
/// subsystem; only full GUFIs, addresses, centres and copied plain views cross the boundary.
pub struct FlightDriver {
    table: FlightTable,
    stats: FlightDriverStats,
    last_receipt_ns: Option<i64>,
    capacity: usize,
}

/// Read-only ordering state for a receipt batch. Rejected items do not advance this cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlightOrderCursor {
    prior_receipt_ns: Option<i64>,
    batch_receipt_ns: Option<i64>,
    prior_source: Option<SourceKind>,
    prior_sequence: [Option<u64>; 5],
}

pub fn flight_order_cursor(driver: &FlightDriver) -> FlightOrderCursor {
    FlightOrderCursor {
        prior_receipt_ns: driver.last_receipt_ns,
        batch_receipt_ns: None,
        prior_source: None,
        prior_sequence: [None; 5],
    }
}

fn source_order_slot(source: SourceKind) -> usize {
    match source {
        SourceKind::Surveillance => 0,
        SourceKind::Flight => 1,
        SourceKind::Route => 2,
        SourceKind::Weather => 3,
        SourceKind::Adaptation => 4,
    }
}

/// Check timestamp, category, and source-order rules without updating driver or cursor state.
pub fn flight_stage_order(
    driver: &FlightDriver,
    cursor: &FlightOrderCursor,
    event: &NormalizedEvent,
) -> Result<(), FlightDriverError> {
    validate_flight_event(event, cursor.prior_receipt_ns.or(driver.last_receipt_ns))?;
    if cursor
        .batch_receipt_ns
        .is_some_and(|receipt| receipt != event.receipt_ns)
        || cursor
            .prior_source
            .is_some_and(|source| event.source < source)
    {
        return Err(FlightDriverError::InvalidBatchOrder);
    }
    let sequence = cursor
        .prior_sequence
        .get(source_order_slot(event.source))
        .copied()
        .flatten();
    if sequence.is_some_and(|prior| event.sequence <= prior) {
        return Err(FlightDriverError::InvalidBatchOrder);
    }
    Ok(())
}

/// Predicts adapter-level state-dependent refusals without advancing receipt/order state.
pub fn flight_stage_adapter_refusal(
    driver: &FlightDriver,
    event: &NormalizedEvent,
) -> Option<AdapterRefusal> {
    let EventKind::Flight(operation) = event.kind else {
        return None;
    };
    let key = match operation {
        FlightOperation::SetPlanByKey { key, .. }
        | FlightOperation::CancelPlanByKey { key, .. }
        | FlightOperation::SetOwnerByKey { key, .. }
        | FlightOperation::HandoffByKey { key, .. } => key,
        _ => key_gufi(event.identity.flight_gufi),
    };
    let existing = flight_get(&driver.table, &key);
    match operation {
        FlightOperation::SetOwner { unit }
            if existing.is_some_and(|flight| flight.owner != [0; 8] && flight.owner != unit) =>
        {
            Some(AdapterRefusal::OwnerChangeRequiresTake)
        }
        FlightOperation::SetOwnerByKey { unit, .. }
            if existing.is_some_and(|flight| flight.owner != [0; 8] && flight.owner != unit) =>
        {
            Some(AdapterRefusal::OwnerChangeRequiresTake)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Update,
            unit,
        } if existing.is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_to != unit) => {
            Some(AdapterRefusal::AcceptingUnitIsNotOfferedTarget)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Offer,
            ..
        } if existing.is_some_and(|flight| flight.owner == [0; 8]) => {
            Some(AdapterRefusal::OfferRequiresCurrentOwner)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Accept,
            unit,
        } if existing.is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_to != unit) => {
            Some(AdapterRefusal::AcceptingUnitIsNotOfferedTarget)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Take,
            unit,
        } if existing.is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_to != unit) => {
            Some(AdapterRefusal::TakingUnitIsNotOfferedTarget)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Fail,
            unit,
        } if existing
            .is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_from != unit) =>
        {
            Some(AdapterRefusal::FailingUnitIsNotOfferingOwner)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Retract,
            unit,
        } if existing
            .is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_from != unit) =>
        {
            Some(AdapterRefusal::FailingUnitIsNotOfferingOwner)
        }
        FlightOperation::HandoffByKey {
            operation: HandoffOperation::Offer,
            from_unit,
            ..
        } if existing.is_none_or(|flight| flight.owner != from_unit || from_unit == [0; 8]) => {
            Some(AdapterRefusal::OfferRequiresCurrentOwner)
        }
        FlightOperation::HandoffByKey {
            operation: HandoffOperation::Update | HandoffOperation::Accept | HandoffOperation::Take,
            to_unit,
            ..
        } if existing
            .is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_to != to_unit) =>
        {
            Some(AdapterRefusal::AcceptingUnitIsNotOfferedTarget)
        }
        FlightOperation::HandoffByKey {
            operation: HandoffOperation::Retract | HandoffOperation::Fail,
            from_unit,
            ..
        } if existing
            .is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_from != from_unit) =>
        {
            Some(AdapterRefusal::FailingUnitIsNotOfferingOwner)
        }
        _ => None,
    }
}

/// Records a staged adapter refusal without applying an operation or advancing the receipt clock.
pub fn flight_record_adapter_refusal(driver: &mut FlightDriver, reason: AdapterRefusal) {
    driver.stats.events_seen = driver.stats.events_seen.saturating_add(1);
    driver.stats.flight_events = driver.stats.flight_events.saturating_add(1);
    let _ = record_adapter_refusal(driver, reason);
}

/// Advance ordering state only after this item has been accepted and committed.
pub fn flight_commit_order(cursor: &mut FlightOrderCursor, event: &NormalizedEvent) {
    cursor.prior_receipt_ns = Some(event.receipt_ns);
    cursor.batch_receipt_ns = Some(event.receipt_ns);
    cursor.prior_source = Some(event.source);
    if let Some(sequence) = cursor
        .prior_sequence
        .get_mut(source_order_slot(event.source))
    {
        *sequence = Some(event.sequence);
    }
}

pub fn flight_record_item_refusal(driver: &mut FlightDriver, error: FlightDriverError) {
    driver.stats.events_seen = driver.stats.events_seen.saturating_add(1);
    record_input_refusal(&mut driver.stats, error);
}

pub fn flight_record_external_refusal(driver: &mut FlightDriver) {
    driver.stats.events_seen = driver.stats.events_seen.saturating_add(1);
}

pub fn flight_record_capacity_refusal(driver: &mut FlightDriver) {
    driver.stats.events_seen = driver.stats.events_seen.saturating_add(1);
    driver.stats.output_capacity_refusals = driver.stats.output_capacity_refusals.saturating_add(1);
}

/// Detect the known core table-capacity refusal before the event can advance accepted timestamps.
pub fn flight_table_capacity_refusal(driver: &FlightDriver, event: &NormalizedEvent) -> bool {
    let EventKind::Flight(operation) = event.kind else {
        return false;
    };
    let (is_plan, key) = match operation {
        FlightOperation::SetPlan { .. } => (true, key_gufi(event.identity.flight_gufi)),
        FlightOperation::SetPlanByKey { key, .. } => (true, key),
        _ => (false, key_gufi(event.identity.flight_gufi)),
    };
    if !is_plan {
        return false;
    }
    flight_get(&driver.table, &key).is_none() && flights_len(&driver.table) >= driver.capacity
}

/// Altitude interpretation kept explicit, including forms the shared trajectory adapter cannot
/// reduce to one numeric effective clearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SharedAltitude {
    pub form: u8,
    pub reserved_form: [u8; 3],
    pub a_ft: i32,
    pub b_ft: i32,
}
const _: () = assert!(core::mem::size_of::<SharedAltitude>() == 12);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct EffectiveClearance {
    /// `ALT_SIMPLE` has one usable value; all other forms remain explicitly unsupported here.
    pub kind: u8,
    pub reserved_before_altitude: [u8; 3],
    pub altitude_ft: i32,
    pub supported_as_single_level: bool,
    pub reserved_after_supported: [u8; 3],
}
const _: () = assert!(core::mem::size_of::<EffectiveClearance>() == 12);

/// Frozen public view of one flight at one centre. No table slot, index-chain link, or private
/// implementation address is exposed. Stamps retain source time, availability time and message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SharedFlightView {
    pub gufi: u128,
    pub address: u32,
    pub centre: [u8; 4],
    pub plan_id: [u8; 12],
    pub plan_state: u8,
    pub reserved_after_plan_state: [u8; 3],
    pub plan_stamp: Stamp,
    pub assigned: SharedAltitude,
    pub reserved_after_assigned: [u8; 4],
    pub assigned_stamp: Stamp,
    pub interim: SharedAltitude,
    pub reserved_after_interim: [u8; 4],
    pub interim_stamp: Stamp,
    pub effective_clearance: EffectiveClearance,
    pub callsign: [u8; 8],
    pub departure: [u8; 12],
    pub arrival: [u8; 12],
    pub reserved_before_identity_stamp: [u8; 4],
    pub identity_stamp: Stamp,
    pub address_stamp: Stamp,
    pub owner_unit: [u8; 8],
    pub owner_stamp: Stamp,
    pub handoff_state: u8,
    pub handoff_from: [u8; 8],
    pub handoff_to: [u8; 8],
    pub handoff_accepted_by: [u8; 8],
    pub reserved_before_handoff_stamp: [u8; 7],
    pub handoff_stamp: Stamp,
    pub reserved_tail: [u8; 8],
}
// Budget: snapshots copied per admitted receipt frame are bounded by the selected driver's
// capacity (32 for ordinary sessions; larger only for explicit offline profiles).
const _: () = assert!(core::mem::size_of::<SharedFlightView>() == 336);

/// Why a view query did not produce a complete, unambiguous set of frozen rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightViewStatus {
    Found,
    AmbiguousAddress,
    UnknownFlight,
    UnknownCentre,
    FutureSnapshot,
    OutputCapacity,
}

/// Query result and counters. `claimants` counts flights, not plan-centre views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlightViewResult {
    pub status: FlightViewStatus,
    pub claimants: usize,
    pub views_written: usize,
}

/// Constructs an actual flight table with lag floor explicitly set to zero for this normalized
/// research driver. This does not make an SFDPS freshness claim; source and receipt stamps remain
/// visible on every copied view.
pub fn flight_driver_new(config: FlightDriverConfig) -> Result<FlightDriver, FlightDriverError> {
    flight_driver_new_bounded(config, MAX_DRIVER_FLIGHTS)
}

/// Constructs a flight driver with a caller-selected validated cap for an offline profile.
pub fn flight_driver_new_bounded(
    config: FlightDriverConfig,
    maximum_flights: usize,
) -> Result<FlightDriver, FlightDriverError> {
    if config.flight_capacity == 0
        || maximum_flights == 0
        || maximum_flights > MAX_OFFLINE_DRIVER_FLIGHTS
        || config.flight_capacity > maximum_flights
        || config.refusal_log_capacity == 0
        || config.refusal_log_capacity > MAX_DRIVER_LOG_ENTRIES
        || config.conflict_log_capacity == 0
        || config.conflict_log_capacity > MAX_DRIVER_LOG_ENTRIES
    {
        return Err(FlightDriverError::InvalidConfig);
    }
    let table = flights_new(&FlightConfig {
        capacity: config.flight_capacity,
        lag_floor_ms: 0,
        max_future_ms: 0,
        refusal_log: config.refusal_log_capacity,
        conflict_log: config.conflict_log_capacity,
        removal_log: 1,
    })
    .map_err(|_| FlightDriverError::InvalidConfig)?;
    Ok(FlightDriver {
        table,
        stats: FlightDriverStats::default(),
        last_receipt_ns: None,
        capacity: config.flight_capacity,
    })
}

pub fn flight_driver_capacity(driver: &FlightDriver) -> usize {
    driver.capacity
}

/// Returns an immutable snapshot of adapter counters.
pub fn flight_driver_stats(driver: &FlightDriver) -> FlightDriverStats {
    driver.stats
}

/// Number of flights in the authoritative table.
pub fn flight_driver_len(driver: &FlightDriver) -> usize {
    flights_len(&driver.table)
}

/// Copies all flight and adapter counters; log saturation remains visible in the returned values.
pub fn flight_driver_report(driver: &FlightDriver) -> FlightDriverReport {
    FlightDriverReport {
        adapter: driver.stats,
        flights: flights_len(&driver.table),
        operations: driver.table.stats.ops,
        applied: driver.table.stats.applied,
        unchanged: driver.table.stats.unchanged,
        refused_by_operation_reason: driver.table.stats.refused,
        refusals_logged: driver.table.refusals.len(),
        refusals_unlisted: driver.table.stats.refusals_unlisted,
        conflicts_by_kind: driver.table.stats.conflicts,
        conflicts_logged: driver.table.conflicts.len(),
        conflicts_unlisted: driver.table.stats.conflicts_unlisted,
    }
}

/// Applies one normalized controller operation to the authoritative flight table. `SetOwner` is
/// accepted as an authoritative external-record bootstrap/update only; interactive commands must
/// not be mapped to it because this normalized operation carries no authenticated actor identity.
/// Time fields must be whole milliseconds so an input stamp is never silently truncated; availability uses
/// the receipt time with the table's configured zero lag floor.
pub fn flight_apply_event(
    driver: &mut FlightDriver,
    event: &NormalizedEvent,
) -> Result<FlightApplyOutcome, FlightDriverError> {
    driver.stats.events_seen = driver.stats.events_seen.saturating_add(1);
    if let Err(error) = validate_flight_event(event, driver.last_receipt_ns) {
        record_input_refusal(&mut driver.stats, error);
        return Err(error);
    }
    Ok(flight_apply_validated_event(driver, event))
}

fn validate_flight_event(
    event: &NormalizedEvent,
    prior_receipt_ns: Option<i64>,
) -> Result<(), FlightDriverError> {
    if event.source_ns < 0 || event.receipt_ns < 0 || event.source_ns > event.receipt_ns {
        return Err(FlightDriverError::InvalidTimestamp);
    }
    if oa_sealed::in_sealed_window(event.source_ns) || oa_sealed::in_sealed_window(event.receipt_ns)
    {
        return Err(FlightDriverError::SealedTimestamp);
    }
    if prior_receipt_ns.is_some_and(|prior| event.receipt_ns < prior) {
        return Err(FlightDriverError::InvalidTimestamp);
    }
    let expected_source = match event.kind {
        EventKind::Position { .. } => Some(SourceKind::Surveillance),
        EventKind::Flight(_) | EventKind::Operator(_) => Some(SourceKind::Flight),
        EventKind::Route { .. } | EventKind::FiledRoute { .. } | EventKind::ActiveCursor { .. } => {
            Some(SourceKind::Route)
        }
        EventKind::FeedState { .. } => None,
    };
    if expected_source.is_some_and(|expected| event.source != expected) {
        return Err(FlightDriverError::WrongSourceKind);
    }
    let stamped_event = matches!(
        event.kind,
        EventKind::Flight(_)
            | EventKind::Operator(_)
            | EventKind::Route { .. }
            | EventKind::FiledRoute { .. }
            | EventKind::ActiveCursor { .. }
    );
    if stamped_event && (event.source_ns % 1_000_000 != 0 || event.receipt_ns % 1_000_000 != 0) {
        return Err(FlightDriverError::NonMillisecondAdapterTimestamp);
    }
    Ok(())
}

fn record_input_refusal(stats: &mut FlightDriverStats, error: FlightDriverError) {
    match error {
        FlightDriverError::InvalidTimestamp | FlightDriverError::SealedTimestamp => {
            stats.invalid_timestamps = stats.invalid_timestamps.saturating_add(1);
        }
        FlightDriverError::WrongSourceKind => {
            stats.source_mismatch_refusals = stats.source_mismatch_refusals.saturating_add(1);
        }
        FlightDriverError::NonMillisecondAdapterTimestamp => {
            stats.timestamp_precision_refusals =
                stats.timestamp_precision_refusals.saturating_add(1);
        }
        FlightDriverError::InvalidBatchOrder => {
            stats.event_order_refusals = stats.event_order_refusals.saturating_add(1);
        }
        FlightDriverError::InvalidConfig | FlightDriverError::ReceiptBatchTooLarge => {}
    }
}

fn flight_apply_validated_event(
    driver: &mut FlightDriver,
    event: &NormalizedEvent,
) -> FlightApplyOutcome {
    driver.last_receipt_ns = Some(event.receipt_ns);
    let operation = match event.kind {
        EventKind::Flight(operation) => operation,
        EventKind::Operator(_) => {
            return FlightApplyOutcome {
                operation: None,
                adapter_refusal: None,
                ignored_non_flight_event: true,
            };
        }
        _ => {
            return FlightApplyOutcome {
                operation: None,
                adapter_refusal: None,
                ignored_non_flight_event: true,
            };
        }
    };
    driver.stats.flight_events = driver.stats.flight_events.saturating_add(1);
    let source_ms = event.source_ns / 1_000_000;
    let receipt_ms = event.receipt_ns / 1_000_000;
    let key = match operation {
        FlightOperation::SetPlanByKey { key, .. }
        | FlightOperation::CancelPlanByKey { key, .. }
        | FlightOperation::SetOwnerByKey { key, .. }
        | FlightOperation::HandoffByKey { key, .. } => key,
        _ => key_gufi(event.identity.flight_gufi),
    };
    let existing = flight_get(&driver.table, &key);
    let (kind, state, plan_id, alt, unit_a, unit_b, address, identity) = match operation {
        FlightOperation::SetPlan { plan_id, status } => {
            let state = match status {
                1 => ST_PROPOSED,
                2 => ST_ACTIVE,
                other => other,
            };
            (
                OP_SET_PLAN,
                state,
                plan_id,
                None,
                [0; 8],
                [0; 8],
                None,
                None,
            )
        }
        FlightOperation::SetPlanByKey {
            plan_id, status, ..
        } => {
            let state = match status {
                1 => ST_PROPOSED,
                2 => ST_ACTIVE,
                other => other,
            };
            (
                OP_SET_PLAN,
                state,
                plan_id,
                None,
                [0; 8],
                [0; 8],
                None,
                None,
            )
        }
        FlightOperation::CancelPlanByKey { plan_id, .. } => {
            (OP_CANCEL, 0, plan_id, None, [0; 8], [0; 8], None, None)
        }
        FlightOperation::AssignAltitude { altitude_ft } => (
            OP_ASSIGN,
            0,
            [0; 12],
            Some(Alt {
                form: ALT_SIMPLE,
                a_ft: altitude_ft,
                b_ft: 0,
            }),
            [0; 8],
            [0; 8],
            None,
            None,
        ),
        FlightOperation::ClearAssignedAltitude => (
            OP_ASSIGN,
            0,
            [0; 12],
            Some(Alt {
                form: ALT_NIL,
                a_ft: 0,
                b_ft: 0,
            }),
            [0; 8],
            [0; 8],
            None,
            None,
        ),
        FlightOperation::SetInterimAltitude { altitude_ft } => (
            OP_INTERIM,
            0,
            [0; 12],
            Some(match altitude_ft {
                Some(a_ft) => Alt {
                    form: ALT_SIMPLE,
                    a_ft,
                    b_ft: 0,
                },
                None => Alt {
                    form: ALT_NIL,
                    a_ft: 0,
                    b_ft: 0,
                },
            }),
            [0; 8],
            [0; 8],
            None,
            None,
        ),
        FlightOperation::SetOwner { unit } => {
            if existing.is_some_and(|flight| flight.owner != [0; 8] && flight.owner != unit) {
                return record_adapter_refusal(driver, AdapterRefusal::OwnerChangeRequiresTake);
            }
            (OP_OWNER, 0, [0; 12], None, unit, [0; 8], None, None)
        }
        FlightOperation::SetOwnerByKey { unit, .. } => {
            if existing.is_some_and(|flight| flight.owner != [0; 8] && flight.owner != unit) {
                return record_adapter_refusal(driver, AdapterRefusal::OwnerChangeRequiresTake);
            }
            (OP_OWNER, 0, [0; 12], None, unit, [0; 8], None, None)
        }
        FlightOperation::SetAddress => (
            OP_ADDRESS,
            0,
            [0; 12],
            None,
            [0; 8],
            [0; 8],
            Some(event.identity.aircraft_address),
            None,
        ),
        FlightOperation::SetIdentity {
            callsign,
            departure,
            arrival,
        } => (
            OP_IDENTITY,
            0,
            [0; 12],
            None,
            [0; 8],
            [0; 8],
            None,
            Some((callsign, departure, arrival)),
        ),
        FlightOperation::Handoff {
            operation: HandoffOperation::Update,
            unit,
        } => (OP_HO_UPDATE, 0, [0; 12], None, [0; 8], unit, None, None),
        FlightOperation::Handoff {
            operation: HandoffOperation::Offer,
            unit,
        } => {
            let from = match existing {
                Some(flight) if flight.owner != [0; 8] => flight.owner,
                Some(_) => {
                    return record_adapter_refusal(
                        driver,
                        AdapterRefusal::OfferRequiresCurrentOwner,
                    );
                }
                None => [0; 8],
            };
            (OP_HO_INITIATE, 0, [0; 12], None, from, unit, None, None)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Accept,
            unit,
        } => {
            if existing.is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_to != unit) {
                return record_adapter_refusal(
                    driver,
                    AdapterRefusal::AcceptingUnitIsNotOfferedTarget,
                );
            }
            (OP_HO_ACCEPT, 0, [0; 12], None, [0; 8], unit, None, None)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Take,
            unit,
        } => {
            if existing.is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_to != unit) {
                return record_adapter_refusal(
                    driver,
                    AdapterRefusal::TakingUnitIsNotOfferedTarget,
                );
            }
            (OP_HO_TAKE, 0, [0; 12], None, [0; 8], unit, None, None)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Fail,
            unit,
        } => {
            if existing.is_some_and(|flight| flight.ho_state != HO_NONE && flight.ho_from != unit) {
                return record_adapter_refusal(
                    driver,
                    AdapterRefusal::FailingUnitIsNotOfferingOwner,
                );
            }
            (OP_HO_FAIL, 0, [0; 12], None, [0; 8], [0; 8], None, None)
        }
        FlightOperation::Handoff {
            operation: HandoffOperation::Retract,
            ..
        } => (OP_HO_RETRACT, 0, [0; 12], None, [0; 8], [0; 8], None, None),
        FlightOperation::HandoffByKey {
            operation,
            from_unit,
            to_unit,
            ..
        } => match operation {
            HandoffOperation::Update => {
                (OP_HO_UPDATE, 0, [0; 12], None, [0; 8], to_unit, None, None)
            }
            HandoffOperation::Offer => {
                if existing.is_none_or(|flight| flight.owner != from_unit || from_unit == [0; 8]) {
                    return record_adapter_refusal(
                        driver,
                        AdapterRefusal::OfferRequiresCurrentOwner,
                    );
                }
                (
                    OP_HO_INITIATE,
                    0,
                    [0; 12],
                    None,
                    from_unit,
                    to_unit,
                    None,
                    None,
                )
            }
            HandoffOperation::Accept => {
                (OP_HO_ACCEPT, 0, [0; 12], None, [0; 8], to_unit, None, None)
            }
            HandoffOperation::Take => (OP_HO_TAKE, 0, [0; 12], None, [0; 8], to_unit, None, None),
            HandoffOperation::Retract => (
                OP_HO_RETRACT,
                0,
                [0; 12],
                None,
                from_unit,
                [0; 8],
                None,
                None,
            ),
            HandoffOperation::Fail => (OP_HO_FAIL, 0, [0; 12], None, from_unit, [0; 8], None, None),
        },
    };
    let mut op = op_new(
        kind,
        key,
        event.centre,
        source_ms,
        receipt_ms,
        event.sequence,
    );
    op.state = state;
    op.plan_id = plan_id;
    if let Some(altitude) = alt {
        op.alt = altitude;
    }
    op.unit_a = unit_a;
    op.unit_b = unit_b;
    if let Some(address) = address {
        op.address = address;
    }
    if let Some((callsign, dep, arr)) = identity {
        op.callsign = callsign;
        op.dep = dep;
        op.arr = arr;
    }
    let result = flights_apply(&mut driver.table, &op);
    FlightApplyOutcome {
        operation: Some(result),
        adapter_refusal: None,
        ignored_non_flight_event: false,
    }
}

fn record_adapter_refusal(driver: &mut FlightDriver, reason: AdapterRefusal) -> FlightApplyOutcome {
    driver.stats.adapter_refusals = driver.stats.adapter_refusals.saturating_add(1);
    match reason {
        AdapterRefusal::OwnerChangeRequiresTake => {
            driver.stats.owner_change_requires_take =
                driver.stats.owner_change_requires_take.saturating_add(1);
        }
        AdapterRefusal::AcceptingUnitIsNotOfferedTarget => {
            driver.stats.accepting_unit_mismatches =
                driver.stats.accepting_unit_mismatches.saturating_add(1);
        }
        AdapterRefusal::TakingUnitIsNotOfferedTarget => {
            driver.stats.taking_unit_mismatches =
                driver.stats.taking_unit_mismatches.saturating_add(1);
        }
        AdapterRefusal::OfferRequiresCurrentOwner => {
            driver.stats.offers_without_owner = driver.stats.offers_without_owner.saturating_add(1);
        }
        AdapterRefusal::FailingUnitIsNotOfferingOwner => {
            driver.stats.failing_unit_mismatches =
                driver.stats.failing_unit_mismatches.saturating_add(1);
        }
    }
    FlightApplyOutcome {
        operation: None,
        adapter_refusal: Some(reason),
        ignored_non_flight_event: false,
    }
}

/// Validates the whole closed receipt batch, including common receipt time, stable source order
/// and per-source sequence order, before applying any event. A malformed later event therefore
/// cannot leave a partially applied prefix in the authoritative table.
pub fn flight_preflight_batch(
    driver: &mut FlightDriver,
    events: &[NormalizedEvent],
) -> Result<(), FlightDriverError> {
    if events.len() > crate::session::MAX_EVENTS_PER_RECEIPT {
        return Err(FlightDriverError::ReceiptBatchTooLarge);
    }
    let mut prior_receipt_ns = driver.last_receipt_ns;
    let mut batch_receipt_ns = None;
    let mut prior_source = None;
    let mut prior_sequence = [None; 5];
    for event in events {
        if let Err(error) = validate_flight_event(event, prior_receipt_ns) {
            driver.stats.events_seen = driver
                .stats
                .events_seen
                .saturating_add(u64::try_from(events.len()).unwrap_or(u64::MAX));
            record_input_refusal(&mut driver.stats, error);
            return Err(error);
        }
        if batch_receipt_ns.is_some_and(|receipt| receipt != event.receipt_ns)
            || prior_source.is_some_and(|source| event.source < source)
        {
            driver.stats.events_seen = driver
                .stats
                .events_seen
                .saturating_add(u64::try_from(events.len()).unwrap_or(u64::MAX));
            record_input_refusal(&mut driver.stats, FlightDriverError::InvalidBatchOrder);
            return Err(FlightDriverError::InvalidBatchOrder);
        }
        let source_slot = match event.source {
            SourceKind::Surveillance => 0,
            SourceKind::Flight => 1,
            SourceKind::Route => 2,
            SourceKind::Weather => 3,
            SourceKind::Adaptation => 4,
        };
        let slot = prior_sequence
            .get_mut(source_slot)
            .ok_or(FlightDriverError::InvalidBatchOrder)?;
        if slot.is_some_and(|sequence| event.sequence <= sequence) {
            driver.stats.events_seen = driver
                .stats
                .events_seen
                .saturating_add(u64::try_from(events.len()).unwrap_or(u64::MAX));
            record_input_refusal(&mut driver.stats, FlightDriverError::InvalidBatchOrder);
            return Err(FlightDriverError::InvalidBatchOrder);
        }
        batch_receipt_ns = Some(event.receipt_ns);
        prior_source = Some(event.source);
        *slot = Some(event.sequence);
        prior_receipt_ns = Some(event.receipt_ns);
    }
    Ok(())
}

/// Applies a fully preflighted batch in input order. Runtime composition uses the same preflight
/// followed by individual updates interleaved with local command dispatch, preserving sequence.
pub fn flight_apply_batch(
    driver: &mut FlightDriver,
    events: &[NormalizedEvent],
) -> Result<Vec<FlightApplyOutcome>, FlightDriverError> {
    flight_preflight_batch(driver, events)?;
    driver.stats.events_seen = driver
        .stats
        .events_seen
        .saturating_add(u64::try_from(events.len()).unwrap_or(u64::MAX));
    let mut outcomes = Vec::with_capacity(events.len());
    for event in events {
        outcomes.push(flight_apply_validated_event(driver, event));
    }
    Ok(outcomes)
}

/// Copies all live flights that positively claim `address` at exactly `centre` into `out`.
/// Address collisions remain visible: every claimant is returned, and the status is ambiguous.
/// If any selected state has an availability stamp after `cutoff_ms`, the output is cleared so a
/// caller cannot read a future snapshot as of an earlier time. `out` is caller-owned and is
/// bounded to 32 rows by this interface.
pub fn flight_views_for_address(
    driver: &mut FlightDriver,
    address: u32,
    centre: [u8; 4],
    cutoff_ms: i64,
    out: &mut Vec<SharedFlightView>,
) -> FlightViewResult {
    out.clear();
    let mut claimed = Vec::with_capacity(driver.capacity);
    let claimant_info =
        flights_claiming_address(&driver.table, address, &mut claimed, driver.capacity);
    if claimant_info.total == 0 {
        driver.stats.unknown_flight_queries = driver.stats.unknown_flight_queries.saturating_add(1);
        return view_result(FlightViewStatus::UnknownFlight, 0, 0);
    }
    if out.capacity() > MAX_OFFLINE_DRIVER_FLIGHTS || out.capacity() < claimed.len() {
        driver.stats.output_capacity_refusals =
            driver.stats.output_capacity_refusals.saturating_add(1);
        return view_result(FlightViewStatus::OutputCapacity, claimant_info.total, 0);
    }
    let mut matched = 0usize;
    let mut future = false;
    for flight in &claimed {
        if flight.key.kind != oa_flight::KEY_GUFI {
            continue;
        }
        if let Some(view) = view_for_flight(flight, centre) {
            matched += 1;
            if !stamps_within_cutoff(&view, cutoff_ms) {
                future = true;
            }
            out.push(view);
        }
    }
    if future {
        out.clear();
        driver.stats.future_snapshot_refusals =
            driver.stats.future_snapshot_refusals.saturating_add(1);
        return view_result(FlightViewStatus::FutureSnapshot, claimant_info.total, 0);
    }
    if matched == 0 {
        driver.stats.unknown_centre_queries = driver.stats.unknown_centre_queries.saturating_add(1);
        return view_result(FlightViewStatus::UnknownCentre, claimant_info.total, 0);
    }
    if claimant_info.total > 1 {
        view_result(
            FlightViewStatus::AmbiguousAddress,
            claimant_info.total,
            out.len(),
        )
    } else {
        view_result(FlightViewStatus::Found, claimant_info.total, out.len())
    }
}

/// Looks up a GUFI by its full 128-bit value and freezes only the requested centre's current view.
pub fn flight_view_for_gufi(
    driver: &mut FlightDriver,
    gufi: u128,
    centre: [u8; 4],
    cutoff_ms: i64,
) -> Result<SharedFlightView, FlightViewStatus> {
    let Some(flight) = flight_get(&driver.table, &key_gufi(gufi)) else {
        driver.stats.unknown_flight_queries = driver.stats.unknown_flight_queries.saturating_add(1);
        return Err(FlightViewStatus::UnknownFlight);
    };
    let Some(view) = view_for_flight(&flight, centre) else {
        driver.stats.unknown_centre_queries = driver.stats.unknown_centre_queries.saturating_add(1);
        return Err(FlightViewStatus::UnknownCentre);
    };
    if !stamps_within_cutoff(&view, cutoff_ms) {
        driver.stats.future_snapshot_refusals =
            driver.stats.future_snapshot_refusals.saturating_add(1);
        return Err(FlightViewStatus::FutureSnapshot);
    }
    Ok(view)
}

/// Looks up one explicitly known full flight key without requiring an observed surveillance
/// address. This keeps flight-plan and handoff awareness available during sensor outages.
pub fn flight_view_for_key(
    driver: &mut FlightDriver,
    key: oa_flight::FlightKey,
    centre: [u8; 4],
    cutoff_ms: i64,
) -> Result<SharedFlightView, FlightViewStatus> {
    let Some(flight) = flight_get(&driver.table, &key) else {
        driver.stats.unknown_flight_queries = driver.stats.unknown_flight_queries.saturating_add(1);
        return Err(FlightViewStatus::UnknownFlight);
    };
    let Some(view) = view_for_flight(&flight, centre) else {
        driver.stats.unknown_centre_queries = driver.stats.unknown_centre_queries.saturating_add(1);
        return Err(FlightViewStatus::UnknownCentre);
    };
    if !stamps_within_cutoff(&view, cutoff_ms) {
        driver.stats.future_snapshot_refusals =
            driver.stats.future_snapshot_refusals.saturating_add(1);
        return Err(FlightViewStatus::FutureSnapshot);
    }
    Ok(view)
}

/// Whether the authoritative table still owns this exact full key, without affecting query
/// counters. Used to reclaim retired keys from the bounded offline view ledger.
pub fn flight_driver_has_key(driver: &FlightDriver, key: oa_flight::FlightKey) -> bool {
    flight_get(&driver.table, &key).is_some()
}

/// Maximum index probes one exact-key lookup can perform in this driver's allocated key index.
/// `oa-flight::index_find` checks at most `cells.len()` cells before it returns.
pub fn flight_driver_key_index_cells(driver: &FlightDriver) -> usize {
    driver.table.by_key.cells.len()
}

fn view_for_flight(flight: &Flight, centre: [u8; 4]) -> Option<SharedFlightView> {
    let view = flight
        .views
        .iter()
        .take(usize::from(flight.n_views).min(MAX_VIEWS))
        .find(|view| view.centre == centre)?;
    // Explicit NIL/never-stated interim values expose the assigned clearance. Every other
    // interim form remains authoritative, even when this view cannot reduce it to one level.
    let effective_altitude = if flight.interim.form == ALT_NIL || flight.interim.form == ALT_NONE {
        flight.assigned
    } else {
        flight.interim
    };
    Some(SharedFlightView {
        gufi: flight.key.gufi,
        address: flight.address,
        centre,
        plan_id: view.plan_id,
        plan_state: view.state,
        reserved_after_plan_state: [0; 3],
        plan_stamp: view.stamp,
        assigned: shared_altitude(flight.assigned),
        reserved_after_assigned: [0; 4],
        assigned_stamp: *flight.stamps.get(oa_flight::F_ASSIGNED)?,
        interim: shared_altitude(flight.interim),
        reserved_after_interim: [0; 4],
        interim_stamp: *flight.stamps.get(oa_flight::F_INTERIM)?,
        effective_clearance: EffectiveClearance {
            kind: effective_altitude.form,
            reserved_before_altitude: [0; 3],
            altitude_ft: effective_altitude.a_ft,
            supported_as_single_level: effective_altitude.form == ALT_SIMPLE,
            reserved_after_supported: [0; 3],
        },
        callsign: flight.callsign,
        departure: flight.dep,
        arrival: flight.arr,
        reserved_before_identity_stamp: [0; 4],
        identity_stamp: *flight.stamps.get(oa_flight::F_IDENTITY)?,
        address_stamp: *flight.stamps.get(oa_flight::F_ADDRESS)?,
        owner_unit: flight.owner,
        owner_stamp: *flight.stamps.get(oa_flight::F_OWNER)?,
        handoff_state: flight.ho_state,
        handoff_from: flight.ho_from,
        handoff_to: flight.ho_to,
        handoff_accepted_by: flight.ho_accepted_by,
        reserved_before_handoff_stamp: [0; 7],
        handoff_stamp: *flight.stamps.get(oa_flight::F_HANDOFF)?,
        reserved_tail: [0; 8],
    })
}

fn shared_altitude(altitude: Alt) -> SharedAltitude {
    SharedAltitude {
        form: altitude.form,
        reserved_form: [0; 3],
        a_ft: altitude.a_ft,
        b_ft: altitude.b_ft,
    }
}

fn stamps_within_cutoff(view: &SharedFlightView, cutoff_ms: i64) -> bool {
    cutoff_ms >= 0
        && view.plan_stamp.avail_ms <= cutoff_ms
        && view.assigned_stamp.avail_ms <= cutoff_ms
        && view.interim_stamp.avail_ms <= cutoff_ms
        && view.identity_stamp.avail_ms <= cutoff_ms
        && view.address_stamp.avail_ms <= cutoff_ms
        && view.owner_stamp.avail_ms <= cutoff_ms
        && view.handoff_stamp.avail_ms <= cutoff_ms
}

fn view_result(
    status: FlightViewStatus,
    claimants: usize,
    views_written: usize,
) -> FlightViewResult {
    FlightViewResult {
        status,
        claimants,
        views_written,
    }
}

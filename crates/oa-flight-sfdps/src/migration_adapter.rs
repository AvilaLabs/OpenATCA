//! Development-only projection from validated SFDPS source facts to M4 normalized records.
//!
//! This shell adapter allocates nothing and writes at most [`MAX_ROW_RECORDS`] into a
//! caller-owned slice. It does not decode rows, compare sides, invent a business key or ordinal,
//! infer controller intent, or turn snapshots into amendments. A caller may supply a stable
//! event key only for an explicit source handoff event. Source snapshots of `controllingUnit` and
//! flight fields have `EventState::Unknown` and `KeyState::Missing`; they preserve evidence and
//! denominators but are not comparable as controller actions or flight-data amendment deltas.
//! The handoff phase field carries the parsed `oa_fixm::HANDOFF_*` code. Unit values retain the
//! parser's packed unit+sector bytes; a controller snapshot does not prove event-time ownership.
//! A directly constructed source fact is an observed handoff only if its phase value is present
//! and belongs to that closed `HANDOFF_*` vocabulary.
//! Point-outs and alerts are not present in the parser projection and produce no records.
//!
//! RULESET F1: Connor Avila approved the `oa-migration-shadow` workspace dependency on
//! 2026-11-26 for this bounded offline projection into its existing normalized schema. The
//! dependency is used only for plain record types; this module never invokes the comparator.

use oa_migration_shadow::{
    BusinessKey, EventState, Family, FieldId, FieldState, FieldValue, KeyState, Observation,
    SourceIdentity, ValueKind,
};

use crate::audit::RowIdentity;
use crate::migration_projection::{AssignedAltitude, SfdpsRowFacts, SourceField, SourceFieldState};

pub const MAX_ROW_RECORDS: usize = 11;
pub const PROJECTION_VERSION: u16 = 1;
const MAX_FIELDS: usize = 6;
// Adapter v1 value for the source element family `<handoff>`; event phase is separate.
const HANDOFF_KIND_SOURCE_FIXM: i64 = 1;
pub const SNAPSHOT_FIELD_FLIGHT_STATUS: u16 = 1;
pub const SNAPSHOT_FIELD_FLIGHT_PLAN_ID: u16 = 2;
pub const SNAPSHOT_FIELD_CALLSIGN: u16 = 3;
pub const SNAPSHOT_FIELD_DEPARTURE: u16 = 4;
pub const SNAPSHOT_FIELD_ARRIVAL: u16 = 5;
pub const SNAPSHOT_FIELD_ASSIGNED_ALTITUDE: u16 = 6;
pub const SNAPSHOT_FIELD_INTERIM_ALTITUDE: u16 = 7;
pub const SNAPSHOT_FIELD_REQUESTED_ALTITUDE: u16 = 8;
pub const SNAPSHOT_FIELD_ROUTE: u16 = 9;

/// Caller-owned normalized record. The fixed field array bounds storage and remains available
/// with its provenance after projection. `operation_index` identifies a source-fact slot for
/// repeated FlightAmendment-family snapshots; it is diagnostic metadata only, never a match key.
// A7.2: offline projected observation, size guard waived.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectedObservation {
    pub source: SourceIdentity,
    pub key_state: KeyState,
    pub key: BusinessKey,
    pub source_time_ns: i64,
    pub observed_time_ns: i64,
    pub event_state: EventState,
    pub field_count: u8,
    pub fields: [FieldValue; MAX_FIELDS],
}

/// Empty non-evidence value for initializing a caller-owned projection buffer.
pub const fn empty_projected_observation() -> ProjectedObservation {
    ProjectedObservation {
        source: SourceIdentity {
            stream: 0,
            decoded_offset: 0,
            row: 0,
            operation_index: 0,
        },
        key_state: KeyState::Missing,
        key: BusinessKey {
            entity_key: [0; 32],
            family: Family::Handoff,
            ordinal: 0,
        },
        source_time_ns: 0,
        observed_time_ns: 0,
        event_state: EventState::Unknown,
        field_count: 0,
        fields: [EMPTY_FIELD; MAX_FIELDS],
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    OutputCapacity = 1,
    RowIdentityRange = 2,
    ClockOverflow = 3,
    KeyFamilyMismatch = 4,
    UnexpectedEventKey = 5,
}

const EMPTY_FIELD: FieldValue = FieldValue {
    id: 0,
    state: FieldState::Unknown,
    kind: ValueKind::None,
    integer: 0,
    bytes: [0; 32],
};

fn source_identity(
    row: RowIdentity,
    operation_index: u16,
) -> Result<SourceIdentity, ProjectionError> {
    Ok(SourceIdentity {
        stream: row.stream,
        decoded_offset: row.decoded_offset,
        row: u32::try_from(row.row).map_err(|_| ProjectionError::RowIdentityRange)?,
        operation_index,
    })
}

fn source_clock_ms_to_ns(value: i64) -> Result<i64, ProjectionError> {
    value
        .checked_mul(1_000_000)
        .ok_or(ProjectionError::ClockOverflow)
}

fn key_for(
    family: Family,
    key: Option<BusinessKey>,
) -> Result<(KeyState, BusinessKey), ProjectionError> {
    match key {
        Some(key) if key.family == family && key.entity_key.iter().any(|byte| *byte != 0) => {
            Ok((KeyState::Valid, key))
        }
        Some(key) if key.family == family => Ok((KeyState::Invalid, key)),
        Some(_) => Err(ProjectionError::KeyFamilyMismatch),
        None => Ok((
            KeyState::Missing,
            BusinessKey {
                entity_key: [0; 32],
                family,
                ordinal: 0,
            },
        )),
    }
}

fn empty_field(id: FieldId, state: FieldState) -> FieldValue {
    FieldValue {
        id: id as u16,
        state,
        ..EMPTY_FIELD
    }
}

fn integer_field(id: FieldId, value: i64) -> FieldValue {
    FieldValue {
        id: id as u16,
        state: FieldState::Present,
        kind: ValueKind::Integer,
        integer: value,
        bytes: [0; 32],
    }
}

fn bytes_field(id: FieldId, bytes: &[u8]) -> FieldValue {
    if bytes.len() > 32 {
        return empty_field(id, FieldState::Unsupported);
    }
    let mut value = [0; 32];
    if let Some(dst) = value.get_mut(..bytes.len()) {
        dst.copy_from_slice(bytes);
    }
    FieldValue {
        id: id as u16,
        state: FieldState::Present,
        kind: ValueKind::Bytes32,
        integer: 0,
        bytes: value,
    }
}

fn field_state(state: SourceFieldState) -> FieldState {
    match state {
        SourceFieldState::Absent => FieldState::Absent,
        SourceFieldState::Present => FieldState::Present,
        SourceFieldState::Malformed => FieldState::Invalid,
    }
}

fn supported_handoff_phase(value: u8) -> bool {
    matches!(
        value,
        oa_fixm::HANDOFF_INITIATION
            | oa_fixm::HANDOFF_ACCEPTANCE
            | oa_fixm::HANDOFF_UPDATE
            | oa_fixm::HANDOFF_RETRACTION
            | oa_fixm::HANDOFF_FAILURE
            | oa_fixm::HANDOFF_TAKE_CONTROL
    )
}

fn integer_source<T: Copy>(
    id: FieldId,
    field: SourceField<T>,
    convert: impl FnOnce(T) -> i64,
) -> FieldValue {
    match (field.state, field.value) {
        (SourceFieldState::Present, Some(value)) => integer_field(id, convert(value)),
        (SourceFieldState::Present, None) => empty_field(id, FieldState::Invalid),
        (state, _) => empty_field(id, field_state(state)),
    }
}

fn bytes_source<const N: usize>(id: FieldId, field: SourceField<[u8; N]>) -> FieldValue {
    match (field.state, field.value) {
        (SourceFieldState::Present, Some(value)) => bytes_field(id, &value),
        (SourceFieldState::Present, None) => empty_field(id, FieldState::Invalid),
        (state, _) => empty_field(id, field_state(state)),
    }
}

fn projected(
    facts: &SfdpsRowFacts<'_>,
    family: Family,
    key: Option<BusinessKey>,
    event_state: EventState,
    operation_index: u16,
    fields: &[FieldValue],
) -> Result<ProjectedObservation, ProjectionError> {
    let (key_state, key) = key_for(family, key)?;
    if fields.len() > MAX_FIELDS {
        return Err(ProjectionError::OutputCapacity);
    }
    let mut all_fields = [EMPTY_FIELD; MAX_FIELDS];
    if let Some(dst) = all_fields.get_mut(..fields.len()) {
        dst.copy_from_slice(fields);
    }
    Ok(ProjectedObservation {
        source: source_identity(facts.identity, operation_index)?,
        key_state,
        key,
        source_time_ns: source_clock_ms_to_ns(facts.source_time_ms)?,
        observed_time_ns: source_clock_ms_to_ns(facts.observed_time_ms)?,
        event_state,
        field_count: u8::try_from(fields.len()).map_err(|_| ProjectionError::OutputCapacity)?,
        fields: all_fields,
    })
}

/// Borrow a normalized comparison view from one owned projected record. Invalid field count is
/// impossible through `project_row`, but checked here because the record is public plain data.
pub fn observation(record: &ProjectedObservation) -> Result<Observation<'_>, ProjectionError> {
    let count = usize::from(record.field_count);
    if count > MAX_FIELDS {
        return Err(ProjectionError::OutputCapacity);
    }
    let fields = record
        .fields
        .get(..count)
        .ok_or(ProjectionError::OutputCapacity)?;
    Ok(Observation {
        source: record.source,
        key_state: record.key_state,
        key: record.key,
        source_time_ns: Some(record.source_time_ns),
        observed_time_ns: Some(record.observed_time_ns),
        event_state: record.event_state,
        fields,
    })
}

/// Project supported facts from a single already-validated SFDPS row. The caller must provide
/// room for the full fixed upper bound before work begins. Output consists of at most one
/// explicit handoff event, one controlling-unit snapshot, and source-field snapshots. Business
/// keys are accepted only when supplied by an independent, versioned mapping; all-zero entity
/// keys are retained as invalid; this function
/// never derives them from GUFI, timestamps, message order, or parser row order.
pub fn project_row(
    facts: &SfdpsRowFacts<'_>,
    handoff_key: Option<BusinessKey>,
    output: &mut [ProjectedObservation],
) -> Result<usize, ProjectionError> {
    if output.len() < MAX_ROW_RECORDS {
        return Err(ProjectionError::OutputCapacity);
    }
    // Validate shared provenance conversions before writing any slot, so invalid row/time
    // metadata cannot leave a partially projected output buffer.
    let _ = source_identity(facts.identity, 0)?;
    let _ = source_clock_ms_to_ns(facts.source_time_ms)?;
    let _ = source_clock_ms_to_ns(facts.observed_time_ms)?;
    let mut written = 0usize;

    if facts.handoff_event.state != SourceFieldState::Absent {
        let phase = match (facts.handoff_event.state, facts.handoff_event.value) {
            (SourceFieldState::Present, Some(value)) if supported_handoff_phase(value) => {
                integer_field(FieldId::HandoffPhase, i64::from(value))
            }
            (SourceFieldState::Malformed, _) => {
                empty_field(FieldId::HandoffPhase, FieldState::Invalid)
            }
            (SourceFieldState::Absent, _) => empty_field(FieldId::HandoffPhase, FieldState::Absent),
            _ => empty_field(FieldId::HandoffPhase, FieldState::Invalid),
        };
        let owner = bytes_source(FieldId::HandoffOwner, facts.controlling_unit);
        let fields = [
            integer_field(FieldId::HandoffKind, HANDOFF_KIND_SOURCE_FIXM),
            bytes_source(FieldId::HandoffTransferringUnit, facts.handoff_transferring),
            bytes_source(FieldId::HandoffReceivingUnit, facts.handoff_receiving),
            bytes_source(FieldId::HandoffAcceptingUnit, facts.handoff_accepting),
            phase,
            owner,
        ];
        let state = if matches!(
            (facts.handoff_event.state, facts.handoff_event.value),
            (SourceFieldState::Present, Some(value)) if supported_handoff_phase(value)
        ) {
            EventState::Observed
        } else {
            EventState::Invalid
        };
        let record = projected(facts, Family::Handoff, handoff_key, state, 0, &fields)?;
        let slot = output
            .get_mut(written)
            .ok_or(ProjectionError::OutputCapacity)?;
        *slot = record;
        written += 1;
    } else if handoff_key.is_some() {
        return Err(ProjectionError::UnexpectedEventKey);
    }

    if facts.controlling_unit.state != SourceFieldState::Absent {
        let (owner, sector) = match facts.controlling_unit.value {
            Some(value) if facts.controlling_unit.state == SourceFieldState::Present => {
                let owner = bytes_field(FieldId::SectorOwnerUnit, value.get(..4).unwrap_or(&[]));
                let sector_bytes = value.get(4..).unwrap_or(&[]);
                let sector = if sector_bytes.iter().all(|byte| *byte == 0) {
                    empty_field(FieldId::SectorIdentifier, FieldState::Unknown)
                } else {
                    bytes_field(FieldId::SectorIdentifier, sector_bytes)
                };
                (owner, sector)
            }
            _ => {
                let state = if facts.controlling_unit.state == SourceFieldState::Present {
                    FieldState::Invalid
                } else {
                    field_state(facts.controlling_unit.state)
                };
                (
                    empty_field(FieldId::SectorOwnerUnit, state),
                    empty_field(FieldId::SectorIdentifier, state),
                )
            }
        };
        let fields = [owner, sector];
        let record = projected(
            facts,
            Family::SectorAssignment,
            None,
            EventState::Unknown,
            0,
            &fields,
        )?;
        let slot = output
            .get_mut(written)
            .ok_or(ProjectionError::OutputCapacity)?;
        *slot = record;
        written += 1;
    }

    // The decoder reports current flight fields, not amendment deltas. These rows retain each
    // supported source value as a FlightAmendment-family diagnostic, but Unknown event state and
    // Missing key prevent them from becoming matched amendment evidence.
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_FLIGHT_STATUS,
        facts.flight_status.state,
        integer_source(FieldId::AmendmentValue, facts.flight_status, i64::from),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_FLIGHT_PLAN_ID,
        facts.flight_plan_id.state,
        bytes_source(FieldId::AmendmentValue, facts.flight_plan_id),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_CALLSIGN,
        facts.callsign.state,
        bytes_source(FieldId::AmendmentValue, facts.callsign),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_DEPARTURE,
        facts.departure.state,
        bytes_source(FieldId::AmendmentValue, facts.departure),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_ARRIVAL,
        facts.arrival.state,
        bytes_source(FieldId::AmendmentValue, facts.arrival),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_ASSIGNED_ALTITUDE,
        facts.assigned_altitude.state,
        assigned_altitude_value(facts.assigned_altitude),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_INTERIM_ALTITUDE,
        facts.interim_altitude.state,
        interim_altitude_value(facts.interim_altitude),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_REQUESTED_ALTITUDE,
        facts.requested_altitude.state,
        requested_altitude_value(facts.requested_altitude),
    )?;
    append_snapshot(
        facts,
        output,
        &mut written,
        SNAPSHOT_FIELD_ROUTE,
        facts.route.state,
        route_value(facts.route),
    )?;

    Ok(written)
}

fn append_snapshot(
    facts: &SfdpsRowFacts<'_>,
    output: &mut [ProjectedObservation],
    written: &mut usize,
    source_field_id: u16,
    state: SourceFieldState,
    value: FieldValue,
) -> Result<(), ProjectionError> {
    if state == SourceFieldState::Absent {
        return Ok(());
    }
    if *written >= MAX_ROW_RECORDS {
        return Err(ProjectionError::OutputCapacity);
    }
    let fields = [
        empty_field(FieldId::AmendmentKind, FieldState::Unsupported),
        integer_field(FieldId::AmendmentField, i64::from(source_field_id)),
        value,
    ];
    let record = projected(
        facts,
        Family::FlightAmendment,
        None,
        EventState::Unknown,
        source_field_id,
        &fields,
    )?;
    let slot = output
        .get_mut(*written)
        .ok_or(ProjectionError::OutputCapacity)?;
    *slot = record;
    *written = (*written)
        .checked_add(1)
        .ok_or(ProjectionError::OutputCapacity)?;
    Ok(())
}

fn assigned_altitude_value(field: SourceField<AssignedAltitude>) -> FieldValue {
    match (field.state, field.value) {
        (SourceFieldState::Present, Some(altitude)) => {
            let mut bytes = [0; 32];
            bytes[0] = altitude.form;
            match (
                altitude.value_ft,
                altitude.block_above_ft,
                altitude.block_below_ft,
                altitude.fix_pre_ft,
                altitude.fix_post_ft,
                altitude.fix_point,
            ) {
                (Some(value), None, None, None, None, None)
                    if matches!(
                        altitude.form,
                        oa_fixm::ALT_FORM_SIMPLE | oa_fixm::ALT_FORM_VFR_PLUS
                    ) =>
                {
                    if let Some(dst) = bytes.get_mut(1..5) {
                        dst.copy_from_slice(&value.to_le_bytes());
                    }
                    FieldValue {
                        id: FieldId::AmendmentValue as u16,
                        state: FieldState::Present,
                        kind: ValueKind::Bytes32,
                        integer: 0,
                        bytes,
                    }
                }
                (None, None, None, None, None, None) if altitude.form == oa_fixm::ALT_FORM_VFR => {
                    FieldValue {
                        id: FieldId::AmendmentValue as u16,
                        state: FieldState::Present,
                        kind: ValueKind::Bytes32,
                        integer: 0,
                        bytes,
                    }
                }
                // Block and at-fix forms carry more than one value and are left explicit rather
                // than truncating their canonical source facts to the comparator's 32-byte value.
                _ => empty_field(FieldId::AmendmentValue, FieldState::Unsupported),
            }
        }
        (SourceFieldState::Present, None) => {
            empty_field(FieldId::AmendmentValue, FieldState::Invalid)
        }
        (state, _) => empty_field(FieldId::AmendmentValue, field_state(state)),
    }
}

fn interim_altitude_value(
    field: SourceField<crate::migration_projection::InterimAltitude>,
) -> FieldValue {
    match (field.state, field.value) {
        (SourceFieldState::Present, Some(altitude))
            if altitude.is_nil && altitude.value_ft.is_none()
                || !altitude.is_nil && altitude.value_ft.is_some() =>
        {
            let mut bytes = [0; 32];
            bytes[0] = u8::from(altitude.is_nil);
            if let Some(value) = altitude.value_ft {
                if let Some(dst) = bytes.get_mut(1..5) {
                    dst.copy_from_slice(&value.to_le_bytes());
                }
            }
            FieldValue {
                id: FieldId::AmendmentValue as u16,
                state: FieldState::Present,
                kind: ValueKind::Bytes32,
                integer: 0,
                bytes,
            }
        }
        (SourceFieldState::Present, Some(_)) => {
            empty_field(FieldId::AmendmentValue, FieldState::Invalid)
        }
        (SourceFieldState::Present, None) => {
            empty_field(FieldId::AmendmentValue, FieldState::Invalid)
        }
        (state, _) => empty_field(FieldId::AmendmentValue, field_state(state)),
    }
}

fn requested_altitude_value(field: SourceField<(u8, Option<i32>)>) -> FieldValue {
    match (field.state, field.value) {
        (SourceFieldState::Present, Some((form, value))) => {
            let mut bytes = [0; 32];
            bytes[0] = form;
            bytes[1] = u8::from(value.is_some());
            if let (Some(value), Some(dst)) = (value, bytes.get_mut(2..6)) {
                dst.copy_from_slice(&value.to_le_bytes());
            }
            FieldValue {
                id: FieldId::AmendmentValue as u16,
                state: FieldState::Present,
                kind: ValueKind::Bytes32,
                integer: 0,
                bytes,
            }
        }
        (SourceFieldState::Present, None) => {
            empty_field(FieldId::AmendmentValue, FieldState::Invalid)
        }
        (state, _) => empty_field(FieldId::AmendmentValue, field_state(state)),
    }
}

fn route_value(field: SourceField<&str>) -> FieldValue {
    match (field.state, field.value) {
        (SourceFieldState::Present, Some(value)) => {
            bytes_field(FieldId::AmendmentValue, value.as_bytes())
        }
        (SourceFieldState::Present, None) => {
            empty_field(FieldId::AmendmentValue, FieldState::Invalid)
        }
        (state, _) => empty_field(FieldId::AmendmentValue, field_state(state)),
    }
}

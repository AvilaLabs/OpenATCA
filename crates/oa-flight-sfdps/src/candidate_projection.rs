//! Development-only diagnostic projection of actual `oa-flight` state for M4.
//!
//! This is a state-snapshot export, not a controller action/event exporter. Flight fields and
//! sector/handoff state are emitted with `EventState::Unknown` and `KeyState::Missing`; this
//! deliberately makes them unmatchable by the M4 event comparator. A future state comparison
//! profile must define synchronized snapshot keys and its own time/matching policy before these
//! rows can be compared. It does not infer SFDPS history or use `Stamp::msg` as an event key.
//! Fixed output bound is 25 rows: sector, handoff, up to eight plan views, three altitude fields,
//! and three identity fields. Route is not represented by `oa-flight` and is counted as absent
//! from this projection rather than fabricated.
//!
//! Class: shell/offline projection. It has no I/O, allocation, clocks, or external dependencies.

use oa_flight::{
    Alt, Flight, Stamp, View, ALT_AT_FIX, ALT_BLOCK, ALT_NIL, ALT_NONE, ALT_SIMPLE, ALT_VFR,
    ALT_VFR_ON_TOP_PLUS, F_ASSIGNED, F_HANDOFF, F_IDENTITY, F_INTERIM, F_OWNER, F_REQUESTED,
    MAX_VIEWS,
};
use oa_migration_shadow::{
    BusinessKey, EventState, Family, FieldId, FieldState, FieldValue, KeyState, Observation,
    SourceIdentity, ValueKind,
};

use crate::migration_adapter::{
    empty_projected_observation, observation, ProjectedObservation, ProjectionError,
    SNAPSHOT_FIELD_ARRIVAL, SNAPSHOT_FIELD_ASSIGNED_ALTITUDE, SNAPSHOT_FIELD_CALLSIGN,
    SNAPSHOT_FIELD_DEPARTURE, SNAPSHOT_FIELD_FLIGHT_STATUS, SNAPSHOT_FIELD_INTERIM_ALTITUDE,
    SNAPSHOT_FIELD_REQUESTED_ALTITUDE,
};

pub const MAX_CANDIDATE_FLIGHT_RECORDS: usize = 16;

/// Counts source-state records and explicit omissions in one candidate flight snapshot.
// A7.2: offline projection summary, size guard waived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CandidateProjectionSummary {
    pub records_written: u8,
    pub unstamped_fields: u8,
    pub route_not_modeled: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateProjectionError {
    OutputCapacity = 1,
    ViewCountInvalid = 2,
    ClockInvalid = 3,
    RowLimit = 4,
}

const EMPTY_FIELD: FieldValue = FieldValue {
    id: 0,
    state: FieldState::Unknown,
    kind: ValueKind::None,
    integer: 0,
    bytes: [0; 32],
};

fn empty(id: FieldId, state: FieldState) -> FieldValue {
    FieldValue {
        id: id as u16,
        state,
        ..EMPTY_FIELD
    }
}

fn integer(id: FieldId, value: i64) -> FieldValue {
    FieldValue {
        id: id as u16,
        state: FieldState::Present,
        kind: ValueKind::Integer,
        integer: value,
        bytes: [0; 32],
    }
}

fn bytes(id: FieldId, input: &[u8]) -> FieldValue {
    if input.len() > 32 {
        return empty(id, FieldState::Unsupported);
    }
    let mut value = [0; 32];
    if let Some(target) = value.get_mut(..input.len()) {
        target.copy_from_slice(input);
    }
    FieldValue {
        id: id as u16,
        state: FieldState::Present,
        kind: ValueKind::Bytes32,
        integer: 0,
        bytes: value,
    }
}

fn stamp_is_empty(stamp: Stamp) -> bool {
    stamp == Stamp::default()
}

fn stamp_ns(stamp: Stamp) -> Result<(i64, i64), CandidateProjectionError> {
    if stamp.ts_ms < 0 || stamp.avail_ms < stamp.ts_ms {
        return Err(CandidateProjectionError::ClockInvalid);
    }
    let source = stamp
        .ts_ms
        .checked_mul(1_000_000)
        .ok_or(CandidateProjectionError::ClockInvalid)?;
    let observed = stamp
        .avail_ms
        .checked_mul(1_000_000)
        .ok_or(CandidateProjectionError::ClockInvalid)?;
    Ok((source, observed))
}

fn record(
    family: Family,
    source: SourceIdentity,
    stamp: Stamp,
    fields: &[FieldValue],
) -> Result<ProjectedObservation, CandidateProjectionError> {
    let (source_time_ns, observed_time_ns) = stamp_ns(stamp)?;
    if fields.len() > 6 {
        return Err(CandidateProjectionError::RowLimit);
    }
    let mut projected = empty_projected_observation();
    projected.source = source;
    projected.key_state = KeyState::Missing;
    projected.key = BusinessKey {
        entity_key: [0; 32],
        family,
        ordinal: 0,
    };
    projected.source_time_ns = source_time_ns;
    projected.observed_time_ns = observed_time_ns;
    projected.event_state = EventState::Unknown;
    projected.field_count =
        u8::try_from(fields.len()).map_err(|_| CandidateProjectionError::RowLimit)?;
    if let Some(target) = projected.fields.get_mut(..fields.len()) {
        target.copy_from_slice(fields);
    }
    Ok(projected)
}

fn write(
    out: &mut [ProjectedObservation],
    written: &mut usize,
    item: ProjectedObservation,
) -> Result<(), CandidateProjectionError> {
    let target = out
        .get_mut(*written)
        .ok_or(CandidateProjectionError::OutputCapacity)?;
    *target = item;
    *written = written
        .checked_add(1)
        .ok_or(CandidateProjectionError::RowLimit)?;
    Ok(())
}

fn has_bytes(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| *byte != 0)
}

fn amendment(
    source: SourceIdentity,
    stamp: Stamp,
    slot: u16,
    value: FieldValue,
) -> Result<ProjectedObservation, CandidateProjectionError> {
    record(
        Family::FlightAmendment,
        source,
        stamp,
        &[
            empty(FieldId::AmendmentKind, FieldState::Unsupported),
            integer(FieldId::AmendmentField, i64::from(slot)),
            value,
        ],
    )
}

fn put_byte(bytes: &mut [u8; 32], index: usize, value: u8) -> Result<(), CandidateProjectionError> {
    let target = bytes
        .get_mut(index)
        .ok_or(CandidateProjectionError::RowLimit)?;
    *target = value;
    Ok(())
}

fn altitude_value(
    id: FieldId,
    value: Alt,
    requested: bool,
) -> Result<FieldValue, CandidateProjectionError> {
    let mut packed = [0; 32];
    let mapped = match value.form {
        ALT_SIMPLE => Some(oa_fixm::ALT_FORM_SIMPLE),
        ALT_VFR_ON_TOP_PLUS => Some(oa_fixm::ALT_FORM_VFR_PLUS),
        ALT_VFR => Some(oa_fixm::ALT_FORM_VFR),
        ALT_NIL if !requested => Some(6),
        ALT_NONE => None,
        ALT_NIL | ALT_BLOCK | ALT_AT_FIX => return Ok(empty(id, FieldState::Unsupported)),
        _ => return Ok(empty(id, FieldState::Invalid)),
    };
    let Some(form) = mapped else {
        return Ok(empty(id, FieldState::Absent));
    };
    put_byte(&mut packed, 0, form)?;
    match value.form {
        ALT_SIMPLE | ALT_VFR_ON_TOP_PLUS => {
            let offset = if requested { 2..6 } else { 1..5 };
            if let Some(dst) = packed.get_mut(offset) {
                dst.copy_from_slice(&value.a_ft.to_le_bytes());
            } else {
                return Err(CandidateProjectionError::RowLimit);
            }
            if requested {
                put_byte(&mut packed, 1, 1)?;
            }
        }
        ALT_VFR => {
            if requested {
                put_byte(&mut packed, 1, 0)?;
            }
        }
        ALT_NIL => {
            put_byte(&mut packed, 0, 1)?;
        }
        _ => return Ok(empty(id, FieldState::Unsupported)),
    }
    Ok(FieldValue {
        id: id as u16,
        state: FieldState::Present,
        kind: ValueKind::Bytes32,
        integer: 0,
        bytes: packed,
    })
}

fn view_value(view: View) -> Result<FieldValue, CandidateProjectionError> {
    let mut value = [0; 32];
    value
        .get_mut(..4)
        .ok_or(CandidateProjectionError::RowLimit)?
        .copy_from_slice(&view.centre);
    *value.get_mut(4).ok_or(CandidateProjectionError::RowLimit)? = view.state;
    value
        .get_mut(5..17)
        .ok_or(CandidateProjectionError::RowLimit)?
        .copy_from_slice(&view.plan_id);
    Ok(FieldValue {
        id: FieldId::AmendmentValue as u16,
        state: FieldState::Present,
        kind: ValueKind::Bytes32,
        integer: 0,
        bytes: value,
    })
}

/// Export the state held by one real `oa-flight::Flight` as bounded diagnostic snapshots.
///
/// Every emitted record is explicitly keyless and has unknown event state. The snapshot's source
/// and availability clocks come from that field's actual OA stamp. The caller supplies the
/// side-local capture identity; it is preserved, not converted into a match key. A route omission
/// is always reported because `oa-flight::Flight` has no route field.
pub fn project_candidate_flight(
    flight: &Flight,
    source: SourceIdentity,
    output: &mut [ProjectedObservation],
) -> Result<CandidateProjectionSummary, CandidateProjectionError> {
    if output.len() < MAX_CANDIDATE_FLIGHT_RECORDS {
        return Err(CandidateProjectionError::OutputCapacity);
    }
    let view_count = usize::from(flight.n_views);
    if view_count > MAX_VIEWS {
        return Err(CandidateProjectionError::ViewCountInvalid);
    }

    // Preflight all stamps and the maximum record count before writing any output.
    let stamps = [
        *flight
            .stamps
            .get(F_OWNER)
            .ok_or(CandidateProjectionError::ClockInvalid)?,
        *flight
            .stamps
            .get(F_HANDOFF)
            .ok_or(CandidateProjectionError::ClockInvalid)?,
        *flight
            .stamps
            .get(F_ASSIGNED)
            .ok_or(CandidateProjectionError::ClockInvalid)?,
        *flight
            .stamps
            .get(F_INTERIM)
            .ok_or(CandidateProjectionError::ClockInvalid)?,
        *flight
            .stamps
            .get(F_REQUESTED)
            .ok_or(CandidateProjectionError::ClockInvalid)?,
        *flight
            .stamps
            .get(F_IDENTITY)
            .ok_or(CandidateProjectionError::ClockInvalid)?,
    ];
    for stamp in stamps {
        if !stamp_is_empty(stamp) {
            let _ = stamp_ns(stamp)?;
        }
    }
    for view in flight.views.iter().take(view_count) {
        if !stamp_is_empty(view.stamp) {
            let _ = stamp_ns(view.stamp)?;
        }
    }

    let mut written = 0usize;
    let mut summary = CandidateProjectionSummary {
        route_not_modeled: 1,
        ..CandidateProjectionSummary::default()
    };

    let owner_stamp = *flight
        .stamps
        .get(F_OWNER)
        .ok_or(CandidateProjectionError::ClockInvalid)?;
    if stamp_is_empty(owner_stamp) {
        summary.unstamped_fields = summary.unstamped_fields.saturating_add(1);
    } else {
        let owner = if has_bytes(&flight.owner) {
            bytes(
                FieldId::SectorOwnerUnit,
                flight
                    .owner
                    .get(..4)
                    .ok_or(CandidateProjectionError::RowLimit)?,
            )
        } else {
            empty(FieldId::SectorOwnerUnit, FieldState::Invalid)
        };
        let sector = if !has_bytes(&flight.owner) {
            empty(FieldId::SectorIdentifier, FieldState::Invalid)
        } else {
            match flight.owner.get(4..) {
                Some(sector_bytes) if has_bytes(sector_bytes) => {
                    bytes(FieldId::SectorIdentifier, sector_bytes)
                }
                Some(_) => empty(FieldId::SectorIdentifier, FieldState::Unknown),
                None => return Err(CandidateProjectionError::RowLimit),
            }
        };
        let item = record(
            Family::SectorAssignment,
            source,
            owner_stamp,
            &[owner, sector],
        )?;
        write(output, &mut written, item)?;
    }

    let handoff_stamp = *flight
        .stamps
        .get(F_HANDOFF)
        .ok_or(CandidateProjectionError::ClockInvalid)?;
    if stamp_is_empty(handoff_stamp) {
        summary.unstamped_fields = summary.unstamped_fields.saturating_add(1);
    } else {
        let handoff = [
            empty(FieldId::HandoffKind, FieldState::Unsupported),
            if has_bytes(&flight.ho_from) {
                bytes(FieldId::HandoffTransferringUnit, &flight.ho_from)
            } else {
                empty(FieldId::HandoffTransferringUnit, FieldState::Absent)
            },
            if has_bytes(&flight.ho_to) {
                bytes(FieldId::HandoffReceivingUnit, &flight.ho_to)
            } else {
                empty(FieldId::HandoffReceivingUnit, FieldState::Absent)
            },
            if has_bytes(&flight.ho_accepted_by) {
                bytes(FieldId::HandoffAcceptingUnit, &flight.ho_accepted_by)
            } else {
                empty(FieldId::HandoffAcceptingUnit, FieldState::Absent)
            },
            empty(FieldId::HandoffPhase, FieldState::Unsupported),
            if has_bytes(&flight.owner) {
                bytes(FieldId::HandoffOwner, &flight.owner)
            } else {
                empty(FieldId::HandoffOwner, FieldState::Absent)
            },
        ];
        let item = record(Family::Handoff, source, handoff_stamp, &handoff)?;
        write(output, &mut written, item)?;
    }

    for view in flight.views.iter().take(view_count) {
        if stamp_is_empty(view.stamp) {
            summary.unstamped_fields = summary.unstamped_fields.saturating_add(1);
            continue;
        }
        let item = amendment(
            source,
            view.stamp,
            SNAPSHOT_FIELD_FLIGHT_STATUS,
            view_value(*view)?,
        )?;
        write(output, &mut written, item)?;
    }

    for (field, stamp_index, value, requested) in [
        (
            SNAPSHOT_FIELD_ASSIGNED_ALTITUDE,
            F_ASSIGNED,
            flight.assigned,
            false,
        ),
        (
            SNAPSHOT_FIELD_INTERIM_ALTITUDE,
            F_INTERIM,
            flight.interim,
            false,
        ),
        (
            SNAPSHOT_FIELD_REQUESTED_ALTITUDE,
            F_REQUESTED,
            flight.requested,
            true,
        ),
    ] {
        let stamp = *flight
            .stamps
            .get(stamp_index)
            .ok_or(CandidateProjectionError::ClockInvalid)?;
        if stamp_is_empty(stamp) {
            summary.unstamped_fields = summary.unstamped_fields.saturating_add(1);
            continue;
        }
        let item = amendment(
            source,
            stamp,
            field,
            altitude_value(FieldId::AmendmentValue, value, requested)?,
        )?;
        write(output, &mut written, item)?;
    }

    let identity_stamp = *flight
        .stamps
        .get(F_IDENTITY)
        .ok_or(CandidateProjectionError::ClockInvalid)?;
    if stamp_is_empty(identity_stamp) {
        summary.unstamped_fields = summary.unstamped_fields.saturating_add(1);
    } else {
        for (field, value) in [
            (SNAPSHOT_FIELD_CALLSIGN, flight.callsign.as_slice()),
            (SNAPSHOT_FIELD_DEPARTURE, flight.dep.as_slice()),
            (SNAPSHOT_FIELD_ARRIVAL, flight.arr.as_slice()),
        ] {
            let value = if has_bytes(value) {
                bytes(FieldId::AmendmentValue, value)
            } else {
                empty(FieldId::AmendmentValue, FieldState::Absent)
            };
            let item = amendment(source, identity_stamp, field, value)?;
            write(output, &mut written, item)?;
        }
    }

    summary.records_written =
        u8::try_from(written).map_err(|_| CandidateProjectionError::RowLimit)?;
    Ok(summary)
}

/// Borrow the standard comparator view from a candidate snapshot projection record.
pub fn candidate_observation(
    record: &ProjectedObservation,
) -> Result<Observation<'_>, ProjectionError> {
    observation(record)
}

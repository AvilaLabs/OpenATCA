//! Typed facts from one decoded SFDPS row for offline migration adapters. Shell only.
//!
//! This module deliberately does not import or reproduce the M4 comparator. It preserves the
//! parser's per-field presence/error state, the source message clock, the recorder receive clock,
//! and existing H2 row identity so a reviewed caller can map supported facts without inventing
//! values or matching ordinals. Point-outs and alerts are not represented by `oa_fixm::Events`.

use std::io;

use oa_fixm::{self, Events, Field};

use crate::audit::RowIdentity;

/// Parser-level status for one typed source field. `Absent` is only source-row absence; it does
/// not mean an operational state is cleared. `Malformed` comes from the decoder error bitmap.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFieldState {
    Absent = 1,
    Present = 2,
    Malformed = 3,
}

/// A field value paired with its actual decoder status. A7.2: offline source record, size guard
/// waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceField<T> {
    pub state: SourceFieldState,
    pub value: Option<T>,
}

/// An assigned altitude keeps its parsed form and all form-specific numeric/fix values without
/// interpreting the operational meaning of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssignedAltitude {
    pub form: u8,
    pub value_ft: Option<i32>,
    pub block_above_ft: Option<i32>,
    pub block_below_ft: Option<i32>,
    pub fix_pre_ft: Option<i32>,
    pub fix_post_ft: Option<i32>,
    pub fix_point: Option<[u8; 12]>,
}

/// Interim altitude preserves explicit nil separately from an integer value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterimAltitude {
    pub is_nil: bool,
    pub value_ft: Option<i32>,
}

/// The typed facts exposed by one parser row. This record includes no inferred M4 business key;
/// `msg_seq` and `plan_seq` are source metadata only. A7.2: offline evidence row, size guard
/// waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SfdpsRowFacts<'a> {
    pub identity: RowIdentity,
    pub gufi: u128,
    pub source_time_ms: i64,
    pub observed_time_ms: i64,
    pub message_sequence: SourceField<u64>,
    pub plan_sequence: SourceField<u32>,
    pub handoff_event: SourceField<u8>,
    pub handoff_transferring: SourceField<[u8; 8]>,
    pub handoff_receiving: SourceField<[u8; 8]>,
    pub handoff_accepting: SourceField<[u8; 8]>,
    pub controlling_unit: SourceField<[u8; 8]>,
    pub flight_status: SourceField<u8>,
    pub flight_plan_id: SourceField<[u8; 12]>,
    pub callsign: SourceField<[u8; 8]>,
    pub departure: SourceField<[u8; 12]>,
    pub arrival: SourceField<[u8; 12]>,
    pub assigned_altitude: SourceField<AssignedAltitude>,
    pub interim_altitude: SourceField<InterimAltitude>,
    pub requested_altitude: SourceField<(u8, Option<i32>)>,
    pub route: SourceField<&'a str>,
}

fn field<T: Copy>(
    events: &Events,
    row: usize,
    field: Field,
    value: Option<T>,
) -> io::Result<SourceField<T>> {
    if oa_fixm::field_error(events, row, field) {
        return Ok(SourceField {
            state: SourceFieldState::Malformed,
            value: None,
        });
    }
    if !oa_fixm::has_field(events, row, field) {
        return Ok(SourceField {
            state: SourceFieldState::Absent,
            value: None,
        });
    }
    let value = value.ok_or_else(|| {
        io::Error::other(format!(
            "oa-fixm column missing for present field {field:?} at row {row}; INCOMPLETE"
        ))
    })?;
    Ok(SourceField {
        state: SourceFieldState::Present,
        value: Some(value),
    })
}

fn assigned_altitude(events: &Events, row: usize) -> io::Result<SourceField<AssignedAltitude>> {
    let form_error = oa_fixm::field_error(events, row, Field::AssignedForm);
    let value_error = oa_fixm::field_error(events, row, Field::AssignedAlt);
    if form_error || value_error {
        return Ok(SourceField {
            state: SourceFieldState::Malformed,
            value: None,
        });
    }
    if !oa_fixm::has_field(events, row, Field::AssignedForm) {
        if oa_fixm::has_field(events, row, Field::AssignedAlt) {
            return Ok(SourceField {
                state: SourceFieldState::Malformed,
                value: None,
            });
        }
        return Ok(SourceField {
            state: SourceFieldState::Absent,
            value: None,
        });
    }
    let form = events
        .assigned_form
        .get(row)
        .copied()
        .ok_or_else(|| io::Error::other("assigned-form column missing; INCOMPLETE"))?;
    let numeric = |field: Field, column: Option<i32>| -> io::Result<Option<i32>> {
        if oa_fixm::has_field(events, row, field) {
            column
                .ok_or_else(|| io::Error::other("assigned-altitude column missing; INCOMPLETE"))
                .map(Some)
        } else {
            Ok(None)
        }
    };
    let required_number = |column: Option<i32>| {
        column.ok_or_else(|| io::Error::other("assigned-altitude column missing; INCOMPLETE"))
    };
    let value_ft = match form {
        oa_fixm::ALT_FORM_SIMPLE | oa_fixm::ALT_FORM_VFR_PLUS => {
            numeric(Field::AssignedAlt, events.assigned_ft.get(row).copied())?
        }
        oa_fixm::ALT_FORM_VFR | oa_fixm::ALT_FORM_BLOCK | oa_fixm::ALT_FORM_AT_FIX => None,
        _ => {
            return Ok(SourceField {
                state: SourceFieldState::Malformed,
                value: None,
            });
        }
    };
    let (block_above_ft, block_below_ft) = if form == oa_fixm::ALT_FORM_BLOCK {
        (
            Some(required_number(events.block_above_ft.get(row).copied())?),
            Some(required_number(events.block_below_ft.get(row).copied())?),
        )
    } else {
        (None, None)
    };
    let (fix_pre_ft, fix_post_ft, fix_point) =
        if form == oa_fixm::ALT_FORM_AT_FIX {
            let pre = events.fixalt_pre_ft.get(row).copied().ok_or_else(|| {
                io::Error::other("assigned-fix altitude column missing; INCOMPLETE")
            })?;
            let post = events.fixalt_post_ft.get(row).copied().ok_or_else(|| {
                io::Error::other("assigned-fix altitude column missing; INCOMPLETE")
            })?;
            let point =
                events.fixalt_point.get(row).copied().ok_or_else(|| {
                    io::Error::other("assigned-fix point column missing; INCOMPLETE")
                })?;
            (Some(pre), Some(post), Some(point))
        } else {
            (None, None, None)
        };
    let alt = AssignedAltitude {
        form,
        value_ft,
        block_above_ft,
        block_below_ft,
        fix_pre_ft,
        fix_post_ft,
        fix_point,
    };
    Ok(SourceField {
        state: SourceFieldState::Present,
        value: Some(alt),
    })
}

/// Preserve parser facts for one already-decoded row. A missing mandatory decoder column is an
/// error; it is never converted to an absent operational value.
pub fn row_facts<'a>(
    events: &'a Events,
    row: usize,
    identity: RowIdentity,
    observed_time_ms: i64,
) -> io::Result<SfdpsRowFacts<'a>> {
    if row >= events.len {
        return Err(io::Error::other(
            "SFDPS projection row outside decoded length",
        ));
    }
    // The decoder API intentionally treats a missing bitmap row as all bits clear. That is
    // useful for low-level callers, but would turn truncated parser output into false absence
    // here. A row is projectable only when both status columns cover it.
    if events.present.get(row).is_none() {
        return Err(io::Error::other(
            "SFDPS present bitmap row missing; INCOMPLETE",
        ));
    }
    if events.errors.get(row).is_none() {
        return Err(io::Error::other(
            "SFDPS error bitmap row missing; INCOMPLETE",
        ));
    }
    let gufi = *events
        .gufi
        .get(row)
        .ok_or_else(|| io::Error::other("SFDPS GUFI column missing; INCOMPLETE"))?;
    let source_time_ms = *events
        .ts_ms
        .get(row)
        .ok_or_else(|| io::Error::other("SFDPS timestamp column missing; INCOMPLETE"))?;
    let requested = field(
        events,
        row,
        Field::RequestedForm,
        events
            .requested_form
            .get(row)
            .copied()
            .map(|form| (form, events.requested_ft.get(row).copied())),
    )?;
    let requested_altitude = match requested.value {
        Some((form, raw_value)) => {
            let value = match form {
                oa_fixm::ALT_FORM_SIMPLE | oa_fixm::ALT_FORM_VFR_PLUS => {
                    Some(raw_value.ok_or_else(|| {
                        io::Error::other("requested-altitude column missing; INCOMPLETE")
                    })?)
                }
                oa_fixm::ALT_FORM_VFR => None,
                _ => {
                    return Err(io::Error::other(
                        "requested-altitude form outside decoder vocabulary; INCOMPLETE",
                    ));
                }
            };
            SourceField {
                state: requested.state,
                value: Some((form, value)),
            }
        }
        None => SourceField {
            state: requested.state,
            value: None,
        },
    };
    let interim_altitude = field(
        events,
        row,
        Field::InterimAlt,
        events
            .interim_ft
            .get(row)
            .copied()
            .map(|value| InterimAltitude {
                is_nil: false,
                value_ft: Some(value),
            }),
    )?;
    let interim_altitude = match interim_altitude.state {
        SourceFieldState::Present => {
            let flags = events
                .flags
                .get(row)
                .copied()
                .ok_or_else(|| io::Error::other("SFDPS flags row missing; INCOMPLETE"))?;
            let is_nil = flags & oa_fixm::FLAG_INTERIM_NIL != 0;
            SourceField {
                state: SourceFieldState::Present,
                value: Some(InterimAltitude {
                    is_nil,
                    value_ft: if is_nil {
                        None
                    } else {
                        interim_altitude
                            .value
                            .and_then(|altitude| altitude.value_ft)
                    },
                }),
            }
        }
        _ => interim_altitude,
    };

    let route = field(events, row, Field::Route, route_value(events, row)?)?;

    Ok(SfdpsRowFacts {
        identity,
        gufi,
        source_time_ms,
        observed_time_ms,
        message_sequence: field(events, row, Field::MsgSeq, events.msg_seq.get(row).copied())?,
        plan_sequence: field(
            events,
            row,
            Field::PlanSeq,
            events.plan_seq.get(row).copied(),
        )?,
        handoff_event: field(
            events,
            row,
            Field::HandoffEvent,
            events.handoff_event.get(row).copied(),
        )?,
        handoff_transferring: field(
            events,
            row,
            Field::HandoffTransferring,
            events.xfer.get(row).copied(),
        )?,
        handoff_receiving: field(
            events,
            row,
            Field::HandoffReceiving,
            events.recv.get(row).copied(),
        )?,
        handoff_accepting: field(
            events,
            row,
            Field::HandoffAccepting,
            events.acpt.get(row).copied(),
        )?,
        controlling_unit: field(
            events,
            row,
            Field::ControllingUnit,
            events.ctrl.get(row).copied(),
        )?,
        flight_status: field(events, row, Field::Status, events.status.get(row).copied())?,
        flight_plan_id: field(
            events,
            row,
            Field::FlightPlanId,
            events.plan_id.get(row).copied(),
        )?,
        callsign: field(
            events,
            row,
            Field::Callsign,
            events.callsign.get(row).copied(),
        )?,
        departure: field(
            events,
            row,
            Field::Departure,
            events.departure.get(row).copied(),
        )?,
        arrival: field(
            events,
            row,
            Field::Arrival,
            events.arrival.get(row).copied(),
        )?,
        assigned_altitude: assigned_altitude(events, row)?,
        interim_altitude,
        requested_altitude,
        route,
    })
}

fn route_value(events: &Events, row: usize) -> io::Result<Option<&str>> {
    if oa_fixm::field_error(events, row, Field::Route)
        || !oa_fixm::has_field(events, row, Field::Route)
    {
        return Ok(None);
    }
    let offset = *events
        .route_off
        .get(row)
        .ok_or_else(|| io::Error::other("SFDPS route offset missing; INCOMPLETE"))?
        as usize;
    let len = *events
        .route_len
        .get(row)
        .ok_or_else(|| io::Error::other("SFDPS route length missing; INCOMPLETE"))?
        as usize;
    // oa-fixm rejects zero-byte route text as malformed, so a mutated present bit with an
    // empty range must not become a valid empty route here.
    if len == 0 {
        return Err(io::Error::other(
            "SFDPS present route has empty range; INCOMPLETE",
        ));
    }
    let end = offset
        .checked_add(len)
        .filter(|end| *end <= events.strings_used && *end <= events.strings.len())
        .ok_or_else(|| io::Error::other("SFDPS route range invalid; INCOMPLETE"))?;
    let bytes = events
        .strings
        .get(offset..end)
        .ok_or_else(|| io::Error::other("SFDPS route range missing; INCOMPLETE"))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| io::Error::other("SFDPS route is not UTF-8; INCOMPLETE"))?;
    Ok(Some(text))
}

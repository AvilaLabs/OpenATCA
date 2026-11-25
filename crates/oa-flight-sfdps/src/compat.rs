//! Bounded H2 compatibility aggregates, separate from the frozen G3 v1 event stream.
//!
//! This reports decoder field availability, source-derived operation-kind expectations, and
//! engine operation outcomes. It compares decoded handoff declarations with adapter output using
//! a separate source-side mapping. Results are operation-level compatibility evidence, not
//! field-by-field state deltas or independent controller agreement: the feed does not expose
//! controller decisions or policy.

use std::io;

use oa_fixm::{
    sector_code, unit_code, Events, Field, ALL_FIELDS, ALT_FORM_AT_FIX, ALT_FORM_BLOCK,
    ALT_FORM_SIMPLE, ALT_FORM_VFR, ALT_FORM_VFR_PLUS, FIELD_COUNT, FIELD_NAMES, HANDOFF_ACCEPTANCE,
    HANDOFF_FAILURE, HANDOFF_INITIATION, HANDOFF_RETRACTION, HANDOFF_TAKE_CONTROL, HANDOFF_UPDATE,
    STATUS_ACTIVE, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_DROPPED, STATUS_PROPOSED,
};
use oa_flight::{
    Op, Outcome, OP_ADDRESS, OP_ASSIGN, OP_CANCEL, OP_COMPLETE, OP_COUNT, OP_DEPART, OP_DROP,
    OP_HO_ACCEPT, OP_HO_FAIL, OP_HO_INITIATE, OP_HO_RETRACT, OP_HO_TAKE, OP_HO_UPDATE, OP_IDENTITY,
    OP_INTERIM, OP_NAMES, OP_OWNER, OP_REQUEST, OP_SET_PLAN, R_COUNT, R_NAMES,
};

pub const COMPATIBILITY_HEADER: &str = "H2-COMPATIBILITY-1\tscope=SFDPS-decoder-adapter-replay-compatibility\tindependent_predictions=not-measured\tindependent_agreement_denominator=not-observable";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldAvailability {
    pub absent: u64,
    pub valid: u64,
    pub malformed: u64,
}

/// Fixed-size aggregates only: this collector retains no row identity, aircraft identifier, or
/// source value. Counters use checked arithmetic and refuse overflow instead of wrapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatibilityAudit {
    pub rows: u64,
    pub fields: [FieldAvailability; FIELD_COUNT],
    pub handoff_event_absent: u64,
    pub handoff_event_malformed: u64,
    pub handoff_event_unsupported: u64,
    pub handoff_events_requiring_roles: u64,
    pub handoff_required_role_complete: u64,
    pub handoff_required_role_problem_events: u64,
    pub handoff_required_role_missing: u64,
    pub handoff_required_role_unknown: u64,
    pub handoff_receiving_only_rows: u64,
    /// Six recognized event types, indexed by their stable FIXM handoff code.
    pub handoff_event_types: [u64; 7],
    /// For each role (transferring, receiving, accepting), absent/valid/unknown/malformed.
    pub handoff_roles: [[u64; 4]; 3],
    /// Expected source-derived and actually emitted adapter handoff operations, by operation kind.
    pub expected_handoff_ops: [u64; OP_COUNT],
    pub emitted_handoff_ops: [u64; OP_COUNT],
    pub handoff_applied: [u64; OP_COUNT],
    pub handoff_unchanged: [u64; OP_COUNT],
    pub handoff_refused: [[u64; R_COUNT]; OP_COUNT],
    pub expected_ops: [u64; OP_COUNT],
    pub emitted_ops: [u64; OP_COUNT],
    pub operation_mapping_mismatch_rows: u64,
    pub expected_ops_missing: [u64; OP_COUNT],
    pub unexpected_ops: [u64; OP_COUNT],
    pub operations_applied: [u64; OP_COUNT],
    pub operations_unchanged: [u64; OP_COUNT],
    pub operations_refused: [[u64; R_COUNT]; OP_COUNT],
    /// Nonexclusive source-field associations to expected/emitted operations and their outcomes.
    pub field_expected_operations: [u64; FIELD_COUNT],
    pub field_emitted_associations: [u64; FIELD_COUNT],
    pub field_applied_changes: [u64; FIELD_COUNT],
    pub field_unchanged_restatements: [u64; FIELD_COUNT],
    pub field_refused: [[u64; R_COUNT]; FIELD_COUNT],
    pub owner_unit_known: u64,
    pub owner_unit_unknown: u64,
    pub owner_sector_empty: u64,
    pub owner_sector_nonempty: u64,
    /// Rows whose independently derived event/role mapping differs from adapter output.
    pub handoff_mapping_mismatch_rows: u64,
    /// Recognized, structurally complete source events that did not yield their expected op.
    pub expected_handoff_missing_rows: u64,
    /// Adapter handoff ops emitted without an independently derived expected operation.
    pub unexpected_handoff_rows: u64,
}

impl Default for CompatibilityAudit {
    fn default() -> Self {
        Self {
            rows: 0,
            fields: [FieldAvailability::default(); FIELD_COUNT],
            handoff_event_absent: 0,
            handoff_event_malformed: 0,
            handoff_event_unsupported: 0,
            handoff_events_requiring_roles: 0,
            handoff_required_role_complete: 0,
            handoff_required_role_problem_events: 0,
            handoff_required_role_missing: 0,
            handoff_required_role_unknown: 0,
            handoff_receiving_only_rows: 0,
            handoff_event_types: [0; 7],
            handoff_roles: [[0; 4]; 3],
            expected_handoff_ops: [0; OP_COUNT],
            emitted_handoff_ops: [0; OP_COUNT],
            handoff_applied: [0; OP_COUNT],
            handoff_unchanged: [0; OP_COUNT],
            handoff_refused: [[0; R_COUNT]; OP_COUNT],
            expected_ops: [0; OP_COUNT],
            emitted_ops: [0; OP_COUNT],
            operation_mapping_mismatch_rows: 0,
            expected_ops_missing: [0; OP_COUNT],
            unexpected_ops: [0; OP_COUNT],
            operations_applied: [0; OP_COUNT],
            operations_unchanged: [0; OP_COUNT],
            operations_refused: [[0; R_COUNT]; OP_COUNT],
            field_expected_operations: [0; FIELD_COUNT],
            field_emitted_associations: [0; FIELD_COUNT],
            field_applied_changes: [0; FIELD_COUNT],
            field_unchanged_restatements: [0; FIELD_COUNT],
            field_refused: [[0; R_COUNT]; FIELD_COUNT],
            owner_unit_known: 0,
            owner_unit_unknown: 0,
            owner_sector_empty: 0,
            owner_sector_nonempty: 0,
            handoff_mapping_mismatch_rows: 0,
            expected_handoff_missing_rows: 0,
            unexpected_handoff_rows: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Presence {
    Absent,
    Valid,
    Malformed,
}

fn presence(events: &Events, row: usize, field: Field) -> Presence {
    let bit = 1u64 << field as u8;
    if events.errors.get(row).is_some_and(|word| word & bit != 0) {
        Presence::Malformed
    } else if events.present.get(row).is_some_and(|word| word & bit != 0) {
        Presence::Valid
    } else {
        Presence::Absent
    }
}

fn bump(value: &mut u64) -> io::Result<()> {
    *value = value
        .checked_add(1)
        .ok_or_else(|| io::Error::other("H2 compatibility counter overflow; INCOMPLETE"))?;
    Ok(())
}

fn for_each_field(mut bits: u64, mut apply: impl FnMut(usize) -> io::Result<()>) -> io::Result<()> {
    while bits != 0 {
        let field = usize::try_from(bits.trailing_zeros())
            .map_err(|_| io::Error::other("field index overflow"))?;
        apply(field)?;
        bits &= bits - 1;
    }
    Ok(())
}

fn recognized_event(event: u8) -> Option<u8> {
    match event {
        HANDOFF_INITIATION => Some(OP_HO_INITIATE),
        HANDOFF_UPDATE => Some(OP_HO_UPDATE),
        HANDOFF_ACCEPTANCE => Some(OP_HO_ACCEPT),
        HANDOFF_RETRACTION => Some(OP_HO_RETRACT),
        HANDOFF_FAILURE => Some(OP_HO_FAIL),
        HANDOFF_TAKE_CONTROL => Some(OP_HO_TAKE),
        _ => None,
    }
}

fn event_name(event: u8) -> &'static str {
    match event {
        HANDOFF_INITIATION => "initiation",
        HANDOFF_UPDATE => "update",
        HANDOFF_ACCEPTANCE => "acceptance",
        HANDOFF_RETRACTION => "retraction",
        HANDOFF_FAILURE => "failure",
        HANDOFF_TAKE_CONTROL => "take_control",
        _ => "unsupported",
    }
}

fn role_state(events: &Events, row: usize, field: Field, bytes: Option<&[u8; 8]>) -> usize {
    match presence(events, row, field) {
        Presence::Absent => 0,
        Presence::Malformed => 3,
        Presence::Valid => match bytes {
            Some(unit) if unit_code(unit) == "???" => 2,
            Some(_) => 1,
            None => 3,
        },
    }
}

fn expected_handoff_op(events: &Events, row: usize) -> Option<u8> {
    if presence(events, row, Field::HandoffEvent) != Presence::Valid {
        return None;
    }
    let event = *events.handoff_event.get(row)?;
    let op = recognized_event(event)?;
    let xfer = role_state(
        events,
        row,
        Field::HandoffTransferring,
        events.xfer.get(row),
    );
    let recv = role_state(events, row, Field::HandoffReceiving, events.recv.get(row));
    let acpt = role_state(events, row, Field::HandoffAccepting, events.acpt.get(row));
    let complete = match event {
        HANDOFF_INITIATION => xfer == 1 && recv == 1,
        HANDOFF_UPDATE => recv == 1,
        HANDOFF_ACCEPTANCE | HANDOFF_TAKE_CONTROL => acpt == 1,
        HANDOFF_RETRACTION | HANDOFF_FAILURE => true,
        _ => false,
    };
    complete.then_some(op)
}

/// Source-side operation inventory and nonexclusive field contributors. This intentionally does
/// not call the adapter; operation kinds and contributing fields are independently derived from
/// decoded fields and the documented source mapping.
fn expected_contributors(events: &Events, row: usize) -> [u64; OP_COUNT] {
    let mut contributors = [0u64; OP_COUNT];
    let valid = |field| presence(events, row, field) == Presence::Valid;
    let mask = |fields: &[Field]| {
        fields
            .iter()
            .fold(0u64, |bits, field| bits | (1u64 << *field as u8))
    };
    let mut add = |kind: u8, fields: u64| {
        if let Some(contributors) = contributors.get_mut(usize::from(kind)) {
            *contributors |= fields;
        }
    };
    let status = if valid(Field::Status) {
        events.status.get(row).copied().unwrap_or(0)
    } else {
        0
    };
    match status {
        STATUS_PROPOSED | STATUS_ACTIVE => {
            let fields = mask(&[Field::Status]);
            let fields = fields
                | if valid(Field::FlightPlanId) {
                    mask(&[Field::FlightPlanId])
                } else {
                    0
                };
            add(OP_SET_PLAN, fields);
            if valid(Field::DepartureActual) && status == STATUS_ACTIVE {
                add(OP_DEPART, mask(&[Field::Status, Field::DepartureActual]));
            }
        }
        STATUS_COMPLETED => add(OP_COMPLETE, mask(&[Field::Status])),
        STATUS_CANCELLED => add(OP_CANCEL, mask(&[Field::Status])),
        STATUS_DROPPED => add(OP_DROP, mask(&[Field::Status])),
        _ => {}
    }
    if valid(Field::AssignedForm) {
        let form = events.assigned_form.get(row).copied().unwrap_or(0);
        let convertible = match form {
            ALT_FORM_SIMPLE | ALT_FORM_VFR_PLUS | ALT_FORM_VFR => true,
            ALT_FORM_BLOCK => {
                events.block_above_ft.get(row).is_some() && events.block_below_ft.get(row).is_some()
            }
            ALT_FORM_AT_FIX => {
                events.fixalt_pre_ft.get(row).is_some() && events.fixalt_post_ft.get(row).is_some()
            }
            _ => false,
        };
        if convertible {
            let fields = mask(&[Field::AssignedForm]);
            let fields = fields
                | if valid(Field::AssignedAlt) {
                    mask(&[Field::AssignedAlt])
                } else {
                    0
                };
            add(OP_ASSIGN, fields);
        }
    }
    if valid(Field::InterimAlt) {
        add(OP_INTERIM, mask(&[Field::InterimAlt]));
    }
    if valid(Field::RequestedForm) {
        match events.requested_form.get(row).copied().unwrap_or(0) {
            ALT_FORM_SIMPLE | ALT_FORM_VFR_PLUS | ALT_FORM_VFR => {
                add(OP_REQUEST, mask(&[Field::RequestedForm]));
            }
            _ => {}
        }
    }
    if valid(Field::ControllingUnit)
        && events
            .ctrl
            .get(row)
            .is_some_and(|unit| unit_code(unit) != "???")
    {
        add(OP_OWNER, mask(&[Field::ControllingUnit]));
    }
    if let Some(kind) = expected_handoff_op(events, row) {
        let event = events.handoff_event.get(row).copied().unwrap_or(0);
        let fields = match event {
            HANDOFF_INITIATION => mask(&[
                Field::HandoffEvent,
                Field::HandoffTransferring,
                Field::HandoffReceiving,
            ]),
            HANDOFF_UPDATE => mask(&[Field::HandoffEvent, Field::HandoffReceiving]),
            HANDOFF_ACCEPTANCE | HANDOFF_TAKE_CONTROL => {
                mask(&[Field::HandoffEvent, Field::HandoffAccepting])
            }
            HANDOFF_RETRACTION | HANDOFF_FAILURE => mask(&[Field::HandoffEvent]),
            _ => 0,
        };
        add(kind, fields);
    }
    if valid(Field::AircraftAddress) && events.address.get(row).is_some() {
        add(OP_ADDRESS, mask(&[Field::AircraftAddress]));
    }
    let identity_fields = [Field::Callsign, Field::Departure, Field::Arrival]
        .into_iter()
        .filter_map(|field| {
            let mapped = valid(field)
                && match field {
                    Field::Callsign => events.callsign.get(row).is_some(),
                    Field::Departure => events.departure.get(row).is_some(),
                    Field::Arrival => events.arrival.get(row).is_some(),
                    _ => false,
                };
            mapped.then_some(1u64 << field as u8)
        })
        .fold(0u64, |bits, bit| bits | bit);
    if identity_fields != 0 {
        add(OP_IDENTITY, identity_fields);
    }
    contributors
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RequiredRoleStatus {
    missing: bool,
    unknown: bool,
}

fn required_role_status(events: &Events, row: usize) -> Option<RequiredRoleStatus> {
    if presence(events, row, Field::HandoffEvent) != Presence::Valid {
        return None;
    }
    let event = *events.handoff_event.get(row)?;
    recognized_event(event)?;
    let requirements: &[(Field, Option<&[u8; 8]>)] = match event {
        HANDOFF_INITIATION => &[
            (Field::HandoffTransferring, events.xfer.get(row)),
            (Field::HandoffReceiving, events.recv.get(row)),
        ],
        HANDOFF_UPDATE => &[(Field::HandoffReceiving, events.recv.get(row))],
        HANDOFF_ACCEPTANCE | HANDOFF_TAKE_CONTROL => {
            &[(Field::HandoffAccepting, events.acpt.get(row))]
        }
        HANDOFF_RETRACTION | HANDOFF_FAILURE => return None,
        _ => return None,
    };
    let mut missing = false;
    let mut unknown = false;
    for (field, value) in requirements {
        match role_state(events, row, *field, *value) {
            0 | 3 => missing = true,
            2 => unknown = true,
            _ => {}
        }
    }
    Some(RequiredRoleStatus { missing, unknown })
}

impl CompatibilityAudit {
    /// Observe one decoded row and its adapter output. Expected handoff operation kinds are
    /// derived directly from raw decoder fields and the documented event-role requirements above;
    /// this code intentionally does not call `row_ops` to construct its expectation.
    pub fn observe_row(&mut self, events: &Events, row: usize, ops: &[Op]) -> io::Result<()> {
        bump(&mut self.rows)?;
        for (index, field) in ALL_FIELDS.iter().enumerate() {
            let counter = self
                .fields
                .get_mut(index)
                .ok_or_else(|| io::Error::other("field inventory mismatch"))?;
            match presence(events, row, *field) {
                Presence::Absent => bump(&mut counter.absent)?,
                Presence::Valid => bump(&mut counter.valid)?,
                Presence::Malformed => bump(&mut counter.malformed)?,
            }
        }

        let roles = [
            (Field::HandoffTransferring, events.xfer.get(row)),
            (Field::HandoffReceiving, events.recv.get(row)),
            (Field::HandoffAccepting, events.acpt.get(row)),
        ];
        for (index, (field, bytes)) in roles.into_iter().enumerate() {
            let state = role_state(events, row, field, bytes);
            bump(
                self.handoff_roles
                    .get_mut(index)
                    .and_then(|v| v.get_mut(state))
                    .ok_or_else(|| io::Error::other("handoff role inventory mismatch"))?,
            )?;
        }

        let event_presence = presence(events, row, Field::HandoffEvent);
        match event_presence {
            Presence::Absent => bump(&mut self.handoff_event_absent)?,
            Presence::Malformed => bump(&mut self.handoff_event_malformed)?,
            Presence::Valid => {
                let event = events.handoff_event.get(row).copied().unwrap_or(0);
                if recognized_event(event).is_some() {
                    let count = self
                        .handoff_event_types
                        .get_mut(usize::from(event))
                        .ok_or_else(|| io::Error::other("handoff event inventory mismatch"))?;
                    bump(count)?;
                } else {
                    bump(&mut self.handoff_event_unsupported)?;
                }
            }
        }

        if let Some(status) = required_role_status(events, row) {
            bump(&mut self.handoff_events_requiring_roles)?;
            // The missing/unknown counters are independent event flags and can overlap when
            // one required role is absent while another is explicitly unknown.
            if status.missing {
                bump(&mut self.handoff_required_role_missing)?;
            }
            if status.unknown {
                bump(&mut self.handoff_required_role_unknown)?;
            }
            if status.missing || status.unknown {
                bump(&mut self.handoff_required_role_problem_events)?;
            }
            if !status.missing && !status.unknown {
                bump(&mut self.handoff_required_role_complete)?;
            }
        }
        if event_presence != Presence::Valid
            && role_state(events, row, Field::HandoffReceiving, events.recv.get(row)) == 1
        {
            bump(&mut self.handoff_receiving_only_rows)?;
        }
        if presence(events, row, Field::ControllingUnit) == Presence::Valid {
            if let Some(owner) = events.ctrl.get(row) {
                if unit_code(owner) == "???" {
                    bump(&mut self.owner_unit_unknown)?;
                } else {
                    bump(&mut self.owner_unit_known)?;
                }
                if sector_code(owner).is_empty() {
                    bump(&mut self.owner_sector_empty)?;
                } else {
                    bump(&mut self.owner_sector_nonempty)?;
                }
            }
        }

        let expected = expected_handoff_op(events, row);
        let contributors = expected_contributors(events, row);
        let mut actual = [0u8; OP_COUNT];
        for op in ops {
            let index = usize::from(op.kind);
            if index == 0 || index >= OP_COUNT {
                return Err(io::Error::other("unknown adapter operation; INCOMPLETE"));
            }
            let count = actual
                .get_mut(index)
                .ok_or_else(|| io::Error::other("unknown adapter handoff operation; INCOMPLETE"))?;
            *count = count
                .checked_add(1)
                .ok_or_else(|| io::Error::other("row handoff operation overflow"))?;
            bump(
                self.emitted_ops
                    .get_mut(index)
                    .ok_or_else(|| io::Error::other("adapter op inventory mismatch"))?,
            )?;
            if is_handoff(op.kind) {
                bump(
                    self.emitted_handoff_ops
                        .get_mut(index)
                        .ok_or_else(|| io::Error::other("handoff op inventory mismatch"))?,
                )?;
            }
            let field_bits = *contributors
                .get(index)
                .ok_or_else(|| io::Error::other("source contributor inventory mismatch"))?;
            for_each_field(field_bits, |field| {
                bump(
                    self.field_emitted_associations
                        .get_mut(field)
                        .ok_or_else(|| {
                            io::Error::other("field emitted-operation inventory mismatch")
                        })?,
                )
            })?;
        }
        for kind in 1..OP_COUNT {
            let field_bits = *contributors
                .get(kind)
                .ok_or_else(|| io::Error::other("source contributor inventory mismatch"))?;
            if field_bits != 0 {
                bump(
                    self.expected_ops
                        .get_mut(kind)
                        .ok_or_else(|| io::Error::other("expected operation inventory mismatch"))?,
                )?;
                for_each_field(field_bits, |field| {
                    bump(
                        self.field_expected_operations
                            .get_mut(field)
                            .ok_or_else(|| {
                                io::Error::other("field expected-operation inventory mismatch")
                            })?,
                    )
                })?;
                if is_handoff(
                    u8::try_from(kind).map_err(|_| io::Error::other("operation kind overflow"))?,
                ) {
                    bump(self.expected_handoff_ops.get_mut(kind).ok_or_else(|| {
                        io::Error::other("expected handoff operation inventory mismatch")
                    })?)?;
                }
            }
        }
        let mut row_mismatch = false;
        for kind in 1..OP_COUNT {
            let actual_count = *actual
                .get(kind)
                .ok_or_else(|| io::Error::other("adapter operation inventory mismatch"))?;
            let expected_count = u8::from(
                *contributors
                    .get(kind)
                    .ok_or_else(|| io::Error::other("source operation inventory mismatch"))?
                    != 0,
            );
            if expected_count != actual_count {
                row_mismatch = true;
                for _ in 0..expected_count.saturating_sub(actual_count) {
                    bump(self.expected_ops_missing.get_mut(kind).ok_or_else(|| {
                        io::Error::other("missing operation inventory mismatch")
                    })?)?;
                }
                for _ in 0..actual_count.saturating_sub(expected_count) {
                    bump(self.unexpected_ops.get_mut(kind).ok_or_else(|| {
                        io::Error::other("unexpected operation inventory mismatch")
                    })?)?;
                }
            }
        }
        if row_mismatch {
            bump(&mut self.operation_mapping_mismatch_rows)?;
        }
        let actual_handoff_count: u8 = actual
            .iter()
            .enumerate()
            .filter(|(kind, _)| is_handoff(*kind as u8))
            .try_fold(0u8, |sum, (_, n)| sum.checked_add(*n))
            .ok_or_else(|| io::Error::other("row handoff operation total overflow"))?;
        let kind_matches = expected.map_or(actual_handoff_count == 0, |kind| {
            actual.get(usize::from(kind)).copied() == Some(1) && actual_handoff_count == 1
        });
        if !kind_matches {
            bump(&mut self.handoff_mapping_mismatch_rows)?;
            if expected.is_some()
                && actual
                    .get(usize::from(expected.unwrap_or(0)))
                    .copied()
                    .unwrap_or(0)
                    == 0
            {
                bump(&mut self.expected_handoff_missing_rows)?;
            }
            let unexpected = expected.map_or(actual_handoff_count > 0, |kind| {
                actual_handoff_count != 1 || actual.get(usize::from(kind)).copied() != Some(1)
            });
            if unexpected && actual_handoff_count > 0 {
                bump(&mut self.unexpected_handoff_rows)?;
            }
        }
        Ok(())
    }

    pub fn observe_operation_outcome(
        &mut self,
        events: &Events,
        row: usize,
        kind: u8,
        outcome: Outcome,
    ) -> io::Result<()> {
        let index = usize::from(kind);
        if index == 0 || index >= OP_COUNT {
            return Err(io::Error::other("unknown operation outcome; INCOMPLETE"));
        }
        let field_bits = *expected_contributors(events, row)
            .get(index)
            .ok_or_else(|| io::Error::other("source contributor inventory mismatch"))?;
        match outcome {
            Outcome::Applied => {
                bump(
                    self.operations_applied
                        .get_mut(index)
                        .ok_or_else(|| io::Error::other("operation outcome inventory mismatch"))?,
                )?;
                if is_handoff(kind) {
                    bump(
                        self.handoff_applied.get_mut(index).ok_or_else(|| {
                            io::Error::other("handoff outcome inventory mismatch")
                        })?,
                    )?;
                }
                for_each_field(field_bits, |field| {
                    bump(
                        self.field_applied_changes
                            .get_mut(field)
                            .ok_or_else(|| io::Error::other("field outcome inventory mismatch"))?,
                    )
                })?;
                Ok(())
            }
            Outcome::Unchanged => {
                bump(
                    self.operations_unchanged
                        .get_mut(index)
                        .ok_or_else(|| io::Error::other("operation outcome inventory mismatch"))?,
                )?;
                if is_handoff(kind) {
                    bump(
                        self.handoff_unchanged.get_mut(index).ok_or_else(|| {
                            io::Error::other("handoff outcome inventory mismatch")
                        })?,
                    )?;
                }
                for_each_field(field_bits, |field| {
                    bump(
                        self.field_unchanged_restatements
                            .get_mut(field)
                            .ok_or_else(|| io::Error::other("field outcome inventory mismatch"))?,
                    )
                })?;
                Ok(())
            }
            Outcome::Refused(reason) => {
                let reason = usize::from(reason);
                bump(
                    self.operations_refused
                        .get_mut(index)
                        .and_then(|v| v.get_mut(reason))
                        .ok_or_else(|| io::Error::other("operation refusal inventory mismatch"))?,
                )?;
                if is_handoff(kind) {
                    bump(
                        self.handoff_refused
                            .get_mut(index)
                            .and_then(|v| v.get_mut(reason))
                            .ok_or_else(|| {
                                io::Error::other("handoff refusal inventory mismatch")
                            })?,
                    )?;
                }
                for_each_field(field_bits, |field| {
                    bump(
                        self.field_refused
                            .get_mut(field)
                            .and_then(|v| v.get_mut(reason))
                            .ok_or_else(|| io::Error::other("field refusal inventory mismatch"))?,
                    )
                })?;
                Ok(())
            }
        }
    }

    pub fn validate_completed(&self) -> io::Result<()> {
        let invalid = || {
            io::Error::other("H2 compatibility complete-output reconciliation failed; INCOMPLETE")
        };
        for field in 0..FIELD_COUNT {
            let counts = self.fields.get(field).ok_or_else(invalid)?;
            if counts
                .absent
                .checked_add(counts.valid)
                .and_then(|n| n.checked_add(counts.malformed))
                != Some(self.rows)
            {
                return Err(invalid());
            }
            let applied = *self.field_applied_changes.get(field).ok_or_else(invalid)?;
            let unchanged = *self
                .field_unchanged_restatements
                .get(field)
                .ok_or_else(invalid)?;
            let refused = self.field_refused.get(field).ok_or_else(invalid)?;
            let emitted = *self
                .field_emitted_associations
                .get(field)
                .ok_or_else(invalid)?;
            let outcomes = applied
                .checked_add(unchanged)
                .and_then(|n| {
                    refused
                        .iter()
                        .try_fold(n, |sum, count| sum.checked_add(*count))
                })
                .ok_or_else(invalid)?;
            if outcomes != emitted {
                return Err(invalid());
            }
        }
        let handoff_events = self
            .handoff_event_types
            .iter()
            .try_fold(self.handoff_event_absent, |sum, count| {
                sum.checked_add(*count)
            })
            .and_then(|n| n.checked_add(self.handoff_event_malformed))
            .and_then(|n| n.checked_add(self.handoff_event_unsupported))
            .ok_or_else(invalid)?;
        if handoff_events != self.rows
            || self
                .handoff_required_role_complete
                .checked_add(self.handoff_required_role_problem_events)
                != Some(self.handoff_events_requiring_roles)
        {
            return Err(invalid());
        }
        for role in &self.handoff_roles {
            let total = role
                .iter()
                .try_fold(0u64, |sum, count| sum.checked_add(*count))
                .ok_or_else(invalid)?;
            if total != self.rows {
                return Err(invalid());
            }
        }
        let owner_rows = self
            .owner_unit_known
            .checked_add(self.owner_unit_unknown)
            .ok_or_else(invalid)?;
        let sector_rows = self
            .owner_sector_empty
            .checked_add(self.owner_sector_nonempty)
            .ok_or_else(invalid)?;
        let controlling_unit = self
            .fields
            .get(usize::from(Field::ControllingUnit as u8))
            .ok_or_else(invalid)?;
        if owner_rows != controlling_unit.valid || sector_rows != owner_rows {
            return Err(invalid());
        }
        let mut missing_handoff = 0u64;
        let mut extra_handoff = 0u64;
        for kind in 1..OP_COUNT {
            let applied = *self.operations_applied.get(kind).ok_or_else(invalid)?;
            let unchanged = *self.operations_unchanged.get(kind).ok_or_else(invalid)?;
            let refused = self.operations_refused.get(kind).ok_or_else(invalid)?;
            let emitted = *self.emitted_ops.get(kind).ok_or_else(invalid)?;
            let expected = *self.expected_ops.get(kind).ok_or_else(invalid)?;
            let unexpected = *self.unexpected_ops.get(kind).ok_or_else(invalid)?;
            let expected_missing = *self.expected_ops_missing.get(kind).ok_or_else(invalid)?;
            let expected_handoff = *self.expected_handoff_ops.get(kind).ok_or_else(invalid)?;
            let emitted_handoff = *self.emitted_handoff_ops.get(kind).ok_or_else(invalid)?;
            let emitted_reconciled = applied
                .checked_add(unchanged)
                .and_then(|n| {
                    refused
                        .iter()
                        .try_fold(n, |sum, count| sum.checked_add(*count))
                })
                .ok_or_else(invalid)?;
            let mapped_expected = expected.checked_add(unexpected).ok_or_else(invalid)?;
            let mapped_emitted = emitted.checked_add(expected_missing).ok_or_else(invalid)?;
            if emitted_reconciled != emitted || mapped_expected != mapped_emitted {
                return Err(invalid());
            }
            if is_handoff(u8::try_from(kind).map_err(|_| invalid())?) {
                if expected_handoff != expected || emitted_handoff != emitted {
                    return Err(invalid());
                }
                missing_handoff = missing_handoff
                    .checked_add(expected_missing)
                    .ok_or_else(invalid)?;
                extra_handoff = extra_handoff.checked_add(unexpected).ok_or_else(invalid)?;
            }
        }
        if missing_handoff != self.expected_handoff_missing_rows
            || extra_handoff != self.unexpected_handoff_rows
        {
            return Err(invalid());
        }
        Ok(())
    }

    /// Stable, bounded records. No source identifier or unit value is serialized.
    pub fn records(&self) -> io::Result<String> {
        use std::fmt::Write as _;
        let mut out = String::from(COMPATIBILITY_HEADER);
        out.push('\n');
        writeln!(out, "DENOMINATOR\tdecoded_rows={}\tindependent_predictions=0\tindependent_agreement=not-observable\tfield_outcome_attribution=nonexclusive_operation_contributors", self.rows)
            .map_err(|_| io::Error::other("formatting compatibility summary"))?;
        for (index, f) in self.fields.iter().enumerate() {
            let refusals = self
                .field_refused
                .get(index)
                .ok_or_else(|| io::Error::other("field refusal inventory mismatch"))?;
            let expected = *self
                .field_expected_operations
                .get(index)
                .ok_or_else(|| io::Error::other("field operation inventory mismatch"))?;
            let emitted = *self
                .field_emitted_associations
                .get(index)
                .ok_or_else(|| io::Error::other("field association inventory mismatch"))?;
            let applied = *self
                .field_applied_changes
                .get(index)
                .ok_or_else(|| io::Error::other("field outcome inventory mismatch"))?;
            let unchanged = *self
                .field_unchanged_restatements
                .get(index)
                .ok_or_else(|| io::Error::other("field outcome inventory mismatch"))?;
            let refused_total = refusals
                .iter()
                .try_fold(0u64, |sum, count| sum.checked_add(*count))
                .ok_or_else(|| io::Error::other("field refusal total overflow"))?;
            writeln!(
                out,
                "FIELD\tcode={index}\tname={}\trows={}\tabsent={}\tvalid={}\tmalformed={}\texpected_operation_associations={}\temitted_operation_associations={}\tapplied_operation_associations={}\tunchanged_operation_associations={}\trefused_operation_associations={refused_total}",
                FIELD_NAMES.get(index).copied().unwrap_or("unknown"), self.rows, f.absent, f.valid, f.malformed, expected,
                emitted, applied, unchanged
            )
            .map_err(|_| io::Error::other("formatting compatibility field"))?;
            for (reason, count) in refusals.iter().enumerate() {
                let label = R_NAMES.get(reason).copied().unwrap_or("unknown");
                writeln!(
                    out,
                    "FIELD_REFUSAL\tfield={index}\treason={reason}\tlabel={label}\tcount={count}"
                )
                .map_err(|_| io::Error::other("formatting field refusal"))?;
            }
        }
        writeln!(out, "HANDOFF\tevent_denominator_rows={}\tevent_absent={}\tevent_malformed={}\tevent_unsupported={}\trequired_role_events={}\trequired_role_complete={}\trequired_role_problem_events={}\trequired_role_missing_flags={}\trequired_role_unknown_flags={}\treceiving_only_rows={}\tmapping_denominator_rows={}\tmapping_mismatch_rows={}\texpected_missing_rows={}\tunexpected_rows={}", self.rows, self.handoff_event_absent, self.handoff_event_malformed, self.handoff_event_unsupported, self.handoff_events_requiring_roles, self.handoff_required_role_complete, self.handoff_required_role_problem_events, self.handoff_required_role_missing, self.handoff_required_role_unknown, self.handoff_receiving_only_rows, self.rows, self.handoff_mapping_mismatch_rows, self.expected_handoff_missing_rows, self.unexpected_handoff_rows)
            .map_err(|_| io::Error::other("formatting compatibility handoff"))?;
        for event in HANDOFF_INITIATION..=HANDOFF_TAKE_CONTROL {
            let count = self
                .handoff_event_types
                .get(usize::from(event))
                .ok_or_else(|| io::Error::other("handoff event inventory mismatch"))?;
            writeln!(
                out,
                "HANDOFF_EVENT\tcode={event}\tname={}\trows={count}",
                event_name(event)
            )
            .map_err(|_| io::Error::other("formatting compatibility event"))?;
        }
        for role in 0..self.handoff_roles.len() {
            let counts = self
                .handoff_roles
                .get(role)
                .ok_or_else(|| io::Error::other("handoff role inventory mismatch"))?;
            let name = ["transferring", "receiving", "accepting"]
                .get(role)
                .copied()
                .unwrap_or("unknown");
            let absent = counts
                .first()
                .copied()
                .ok_or_else(|| io::Error::other("handoff role inventory mismatch"))?;
            let valid_known = counts
                .get(1)
                .copied()
                .ok_or_else(|| io::Error::other("handoff role inventory mismatch"))?;
            let valid_unknown = counts
                .get(2)
                .copied()
                .ok_or_else(|| io::Error::other("handoff role inventory mismatch"))?;
            let malformed = counts
                .get(3)
                .copied()
                .ok_or_else(|| io::Error::other("handoff role inventory mismatch"))?;
            writeln!(out, "HANDOFF_ROLE\trole={name}\trows={}\tabsent={absent}\tvalid_known={valid_known}\tvalid_unknown={valid_unknown}\tmalformed={malformed}", self.rows)
                .map_err(|_| io::Error::other("formatting compatibility role"))?;
        }
        for kind in [
            OP_HO_INITIATE,
            OP_HO_UPDATE,
            OP_HO_ACCEPT,
            OP_HO_RETRACT,
            OP_HO_FAIL,
            OP_HO_TAKE,
        ] {
            let index = usize::from(kind);
            let expected = *self
                .expected_handoff_ops
                .get(index)
                .ok_or_else(|| io::Error::other("handoff operation inventory mismatch"))?;
            let emitted = *self
                .emitted_handoff_ops
                .get(index)
                .ok_or_else(|| io::Error::other("handoff operation inventory mismatch"))?;
            let applied = *self
                .handoff_applied
                .get(index)
                .ok_or_else(|| io::Error::other("handoff outcome inventory mismatch"))?;
            let unchanged = *self
                .handoff_unchanged
                .get(index)
                .ok_or_else(|| io::Error::other("handoff outcome inventory mismatch"))?;
            let refused = self
                .handoff_refused
                .get(index)
                .ok_or_else(|| io::Error::other("handoff refusal inventory mismatch"))?;
            writeln!(
                out,
                "HANDOFF_OP\tkind={kind}\texpected_source={}\temitted_adapter={}",
                expected, emitted
            )
            .map_err(|_| io::Error::other("formatting compatibility operation"))?;
            writeln!(
                out,
                "HANDOFF_OUTCOME\tkind={kind}\tapplied={}\tunchanged={}",
                applied, unchanged
            )
            .map_err(|_| io::Error::other("formatting compatibility outcome"))?;
            for (reason, count) in refused.iter().enumerate() {
                writeln!(
                    out,
                    "HANDOFF_REFUSAL\tkind={kind}\treason={reason}\tcount={count}"
                )
                .map_err(|_| io::Error::other("formatting compatibility refusal"))?;
            }
        }
        let owner_rows = self
            .owner_unit_known
            .checked_add(self.owner_unit_unknown)
            .ok_or_else(|| io::Error::other("owner denominator overflow"))?;
        let sector_rows = self
            .owner_sector_empty
            .checked_add(self.owner_sector_nonempty)
            .ok_or_else(|| io::Error::other("sector denominator overflow"))?;
        writeln!(
            out,
            "OWNER\tunit_valid_rows={owner_rows}\tunit_known={}\tunit_unknown={}\tsector_rows={sector_rows}\tsector_empty={}\tsector_nonempty={}",
            self.owner_unit_known,
            self.owner_unit_unknown,
            self.owner_sector_empty,
            self.owner_sector_nonempty
        )
        .map_err(|_| io::Error::other("formatting owner assignment summary"))?;
        writeln!(
            out,
            "OP_MAPPING\tdenominator_rows={}\tmismatch_rows={}",
            self.rows, self.operation_mapping_mismatch_rows
        )
        .map_err(|_| io::Error::other("formatting operation mapping summary"))?;
        for kind in 1..OP_COUNT {
            let refused = self
                .operations_refused
                .get(kind)
                .ok_or_else(|| io::Error::other("operation outcome inventory mismatch"))?;
            let refused_total = refused
                .iter()
                .try_fold(0u64, |sum, count| sum.checked_add(*count))
                .ok_or_else(|| io::Error::other("operation refusal total overflow"))?;
            let name = OP_NAMES.get(kind).copied().unwrap_or("unknown");
            let expected = self
                .expected_ops
                .get(kind)
                .copied()
                .ok_or_else(|| io::Error::other("operation inventory mismatch"))?;
            let emitted = self
                .emitted_ops
                .get(kind)
                .copied()
                .ok_or_else(|| io::Error::other("operation inventory mismatch"))?;
            let missing = self
                .expected_ops_missing
                .get(kind)
                .copied()
                .ok_or_else(|| io::Error::other("operation inventory mismatch"))?;
            let unexpected = self
                .unexpected_ops
                .get(kind)
                .copied()
                .ok_or_else(|| io::Error::other("operation inventory mismatch"))?;
            let applied = self
                .operations_applied
                .get(kind)
                .copied()
                .ok_or_else(|| io::Error::other("operation inventory mismatch"))?;
            let unchanged = self
                .operations_unchanged
                .get(kind)
                .copied()
                .ok_or_else(|| io::Error::other("operation inventory mismatch"))?;
            writeln!(out, "OP_COMPAT\tkind={kind}\tname={name}\texpected_source={expected}\temitted_adapter={emitted}\tmissing_adapter={missing}\textra_adapter={unexpected}\tapplied_change={applied}\tunchanged_restatement={unchanged}\trefused={refused_total}")
                .map_err(|_| io::Error::other("formatting operation comparison"))?;
            for (reason, count) in refused.iter().enumerate() {
                let label = R_NAMES.get(reason).copied().unwrap_or("unknown");
                writeln!(
                    out,
                    "OP_COMPAT_REFUSAL\tkind={kind}\treason={reason}\tlabel={label}\tcount={count}"
                )
                .map_err(|_| io::Error::other("formatting operation refusal"))?;
            }
        }
        Ok(out)
    }
}

fn is_handoff(kind: u8) -> bool {
    matches!(
        kind,
        OP_HO_INITIATE | OP_HO_UPDATE | OP_HO_ACCEPT | OP_HO_RETRACT | OP_HO_FAIL | OP_HO_TAKE
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::adapter::{row_ops, AdapterStats};

    fn source_row(event: u8, roles: &[Field]) -> Events {
        let mut events = oa_fixm::new_events(1, 0);
        events.len = 1;
        let mut present = 1u64 << Field::HandoffEvent as u8;
        for role in roles {
            present |= 1u64 << *role as u8;
        }
        events.present.push(present);
        events.errors.push(0);
        events.gufi.push(1);
        events.ts_ms.push(10);
        events.handoff_event.push(event);
        events.xfer.push(*b"ABC\0\0\0\0\0");
        events.recv.push(*b"DEF\0\0\0\0\0");
        events.acpt.push(*b"GHI\0\0\0\0\0");
        events
    }

    #[test]
    fn handoff_source_expectation_is_checked_against_adapter_output() {
        let events = source_row(
            HANDOFF_INITIATION,
            &[Field::HandoffTransferring, Field::HandoffReceiving],
        );
        let mut adapter = AdapterStats::default();
        let mut ops = Vec::new();
        row_ops(&events, 0, 11, 12, &mut adapter, &mut ops);
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&events, 0, &ops).unwrap();
        assert_eq!(compat.expected_handoff_ops[usize::from(OP_HO_INITIATE)], 1);
        assert_eq!(compat.emitted_handoff_ops[usize::from(OP_HO_INITIATE)], 1);
        assert_eq!(compat.handoff_mapping_mismatch_rows, 0);
        assert_eq!(
            compat.fields[usize::from(Field::HandoffEvent as u8)].valid,
            1
        );
        assert!(compat.records().unwrap().starts_with(COMPATIBILITY_HEADER));
    }

    #[test]
    fn every_supported_handoff_event_has_its_own_source_mapping() {
        let cases = [
            (
                HANDOFF_INITIATION,
                &[Field::HandoffTransferring, Field::HandoffReceiving][..],
                OP_HO_INITIATE,
            ),
            (HANDOFF_UPDATE, &[Field::HandoffReceiving][..], OP_HO_UPDATE),
            (
                HANDOFF_ACCEPTANCE,
                &[Field::HandoffAccepting][..],
                OP_HO_ACCEPT,
            ),
            (HANDOFF_RETRACTION, &[][..], OP_HO_RETRACT),
            (HANDOFF_FAILURE, &[][..], OP_HO_FAIL),
            (
                HANDOFF_TAKE_CONTROL,
                &[Field::HandoffAccepting][..],
                OP_HO_TAKE,
            ),
        ];
        let mut compat = CompatibilityAudit::default();
        for (event, roles, kind) in cases {
            let events = source_row(event, roles);
            let mut adapter = AdapterStats::default();
            let mut ops = Vec::new();
            row_ops(&events, 0, 11, 12, &mut adapter, &mut ops);
            compat.observe_row(&events, 0, &ops).unwrap();
            assert_eq!(compat.expected_handoff_ops[usize::from(kind)], 1);
            assert_eq!(compat.emitted_handoff_ops[usize::from(kind)], 1);
        }
        assert_eq!(compat.handoff_mapping_mismatch_rows, 0);
        assert_eq!(compat.handoff_events_requiring_roles, 4);
        assert_eq!(compat.handoff_required_role_complete, 4);
    }

    #[test]
    fn handoff_missing_and_unknown_required_units_are_not_conflated() {
        let missing = source_row(HANDOFF_INITIATION, &[Field::HandoffTransferring]);
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&missing, 0, &[]).unwrap();
        assert_eq!(compat.handoff_required_role_missing, 1);
        assert_eq!(compat.handoff_required_role_unknown, 0);

        let unknown = source_row(
            HANDOFF_INITIATION,
            &[Field::HandoffTransferring, Field::HandoffReceiving],
        );
        let mut unknown = unknown;
        unknown.recv[0] = *b"???\0\0\0\0\0";
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&unknown, 0, &[]).unwrap();
        assert_eq!(compat.handoff_required_role_missing, 0);
        assert_eq!(compat.handoff_required_role_unknown, 1);
        assert_eq!(compat.handoff_roles[1][2], 1);
    }

    #[test]
    fn missing_malformed_and_unsupported_events_have_separate_denominators() {
        let mut missing = source_row(0, &[Field::HandoffReceiving]);
        missing.present[0] &= !(1u64 << Field::HandoffEvent as u8);
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&missing, 0, &[]).unwrap();
        assert_eq!(compat.handoff_event_absent, 1);
        assert_eq!(compat.handoff_receiving_only_rows, 1);

        let mut malformed = source_row(HANDOFF_INITIATION, &[]);
        malformed.errors[0] |= 1u64 << Field::HandoffEvent as u8;
        malformed.present[0] &= !(1u64 << Field::HandoffEvent as u8);
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&malformed, 0, &[]).unwrap();
        assert_eq!(compat.handoff_event_malformed, 1);
        assert_eq!(compat.handoff_event_absent, 0);

        let unsupported = source_row(255, &[]);
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&unsupported, 0, &[]).unwrap();
        assert_eq!(compat.handoff_event_unsupported, 1);
        assert_eq!(compat.handoff_mapping_mismatch_rows, 0);
    }

    #[test]
    fn absent_adapter_operation_is_reported_as_mapping_gap_not_agreement() {
        let events = source_row(
            HANDOFF_INITIATION,
            &[Field::HandoffTransferring, Field::HandoffReceiving],
        );
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&events, 0, &[]).unwrap();
        assert_eq!(compat.expected_handoff_ops[usize::from(OP_HO_INITIATE)], 1);
        assert_eq!(compat.handoff_mapping_mismatch_rows, 1);
        assert_eq!(compat.expected_handoff_missing_rows, 1);
        assert_eq!(compat.unexpected_handoff_rows, 0);
    }

    #[test]
    fn owner_sector_mapping_and_operation_outcomes_keep_separate_denominators() {
        let mut events = source_row(0, &[]);
        events.present[0] &= !(1u64 << Field::HandoffEvent as u8);
        events.present[0] |= 1u64 << Field::ControllingUnit as u8;
        events.ctrl.push(*b"ZNY\0A1\0\0");
        let mut adapter = AdapterStats::default();
        let mut ops = Vec::new();
        row_ops(&events, 0, 11, 12, &mut adapter, &mut ops);
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&events, 0, &ops).unwrap();
        assert_eq!(compat.expected_ops[usize::from(OP_OWNER)], 1);
        assert_eq!(compat.emitted_ops[usize::from(OP_OWNER)], 1);
        assert_eq!(compat.owner_unit_known, 1);
        assert_eq!(compat.owner_sector_nonempty, 1);
        compat
            .observe_operation_outcome(&events, 0, OP_OWNER, Outcome::Applied)
            .unwrap();
        compat
            .observe_operation_outcome(&events, 0, OP_OWNER, Outcome::Unchanged)
            .unwrap();
        compat
            .observe_operation_outcome(&events, 0, OP_OWNER, Outcome::Refused(oa_flight::R_STALE))
            .unwrap();
        assert_eq!(compat.operations_applied[usize::from(OP_OWNER)], 1);
        assert_eq!(compat.operations_unchanged[usize::from(OP_OWNER)], 1);
        assert_eq!(
            compat.operations_refused[usize::from(OP_OWNER)][usize::from(oa_flight::R_STALE)],
            1
        );
        let owner_field = usize::from(Field::ControllingUnit as u8);
        assert_eq!(compat.field_expected_operations.get(owner_field), Some(&1));
        assert_eq!(compat.field_emitted_associations.get(owner_field), Some(&1));
        assert_eq!(compat.field_applied_changes.get(owner_field), Some(&1));
        assert_eq!(
            compat.field_unchanged_restatements.get(owner_field),
            Some(&1)
        );
        assert_eq!(
            compat
                .field_refused
                .get(owner_field)
                .and_then(|counts| counts.get(usize::from(oa_flight::R_STALE))),
            Some(&1)
        );
    }

    #[test]
    fn amendment_source_fields_map_to_operation_counts_without_reusing_adapter_as_oracle() {
        let fields = [
            Field::Status,
            Field::DepartureActual,
            Field::AssignedForm,
            Field::InterimAlt,
            Field::RequestedForm,
            Field::Callsign,
            Field::Departure,
            Field::Arrival,
            Field::AircraftAddress,
        ];
        let mut events = source_row(0, &[]);
        events.present[0] &= !(1u64 << Field::HandoffEvent as u8);
        for field in fields {
            events.present[0] |= 1u64 << field as u8;
        }
        events.status.push(STATUS_ACTIVE);
        events.dep_actual_ms.push(11);
        events.assigned_form.push(ALT_FORM_SIMPLE);
        events.assigned_ft.push(30_000);
        events.interim_ft.push(28_000);
        events.requested_form.push(ALT_FORM_SIMPLE);
        events.requested_ft.push(27_000);
        events.callsign.push(*b"AB123   ");
        events.departure.push(*b"KJFK\0\0\0\0\0\0\0\0");
        events.arrival.push(*b"KBOS\0\0\0\0\0\0\0\0");
        events.address.push(0x123456);

        let mut adapter = AdapterStats::default();
        let mut ops = Vec::new();
        row_ops(&events, 0, 12, 13, &mut adapter, &mut ops);
        let mut compat = CompatibilityAudit::default();
        compat.observe_row(&events, 0, &ops).unwrap();
        for kind in [
            OP_SET_PLAN,
            OP_DEPART,
            OP_ASSIGN,
            OP_INTERIM,
            OP_REQUEST,
            OP_IDENTITY,
            OP_ADDRESS,
        ] {
            assert_eq!(compat.expected_ops[usize::from(kind)], 1);
            assert_eq!(compat.emitted_ops[usize::from(kind)], 1);
        }
        assert_eq!(compat.operation_mapping_mismatch_rows, 0);
    }
}

//! The SFDPS adapter: one decoded row in, the operations it stands for out. Pure (it reads a row
//! of `oa_fixm::Events` and appends to a list), but it names the decoder crate, so it is shell
//! (RULESET B8).
//!
//! SFDPS publishes ERAM's state, not the controller's entries, so each message is turned into the
//! operations the record is built from (`oa-flight`): the plan status, the departure, the
//! altitudes, the controlling unit, the handoff event, the address and the identity. A field the
//! message does not carry makes no operation; a field that was present but malformed (a field
//! error of the decoder) makes none either, and the decoder counted it.

use oa_fixm::{
    has_field, unit_code, Events, Field, ALT_FORM_AT_FIX, ALT_FORM_BLOCK, ALT_FORM_SIMPLE,
    ALT_FORM_VFR, ALT_FORM_VFR_PLUS, FLAG_INTERIM_NIL, HANDOFF_ACCEPTANCE, HANDOFF_FAILURE,
    HANDOFF_INITIATION, HANDOFF_RETRACTION, HANDOFF_TAKE_CONTROL, HANDOFF_UPDATE, STATUS_ACTIVE,
    STATUS_CANCELLED, STATUS_COMPLETED, STATUS_DROPPED, STATUS_PROPOSED,
};
use oa_flight::{
    key_gufi, op_new, Alt, Op, ALT_AT_FIX, ALT_BLOCK, ALT_NIL, ALT_SIMPLE, ALT_VFR,
    ALT_VFR_ON_TOP_PLUS, OP_ADDRESS, OP_ASSIGN, OP_CANCEL, OP_COMPLETE, OP_COUNT, OP_DEPART,
    OP_DROP, OP_HO_ACCEPT, OP_HO_FAIL, OP_HO_INITIATE, OP_HO_RETRACT, OP_HO_TAKE, OP_HO_UPDATE,
    OP_IDENTITY, OP_INTERIM, OP_OWNER, OP_REQUEST, OP_SET_PLAN, ST_ACTIVE, ST_PROPOSED,
};

/// Operations one message can make: a plan, a departure, three altitudes, the owner, a handoff,
/// the address and the identity.
pub const MAX_OPS_PER_MESSAGE: usize = 10;

/// What the adapter did, counted over every row. A7.2: cold struct, size guard waived.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterStats {
    pub rows: u64,
    /// Operations made, by operation code.
    pub ops: [u64; OP_COUNT],
    /// Rows with no usable plan status, so no plan operation (the decoder counted a bad one).
    pub no_status: u64,
    /// A controlling unit the FAA gives as `???` (unknown): it says nothing about who has the
    /// flight, so it makes no owner operation.
    pub owner_unknown: u64,
    /// A handoff event with a unit it needs missing, so no operation.
    pub handoff_incomplete: u64,
    /// Handoff `receivingUnit` elements with no event (the form on track messages): not an
    /// operation, counted.
    pub handoff_receiving_only: u64,
    /// Departure times seen on a row whose status is not active (they make no departure).
    pub departure_not_active: u64,
}

fn alt_from(form: u8, value: i32, a: Option<i32>, b: Option<i32>) -> Option<Alt> {
    Some(match form {
        ALT_FORM_SIMPLE => Alt {
            form: ALT_SIMPLE,
            a_ft: value,
            b_ft: 0,
        },
        ALT_FORM_VFR_PLUS => Alt {
            form: ALT_VFR_ON_TOP_PLUS,
            a_ft: value,
            b_ft: 0,
        },
        ALT_FORM_VFR => Alt {
            form: ALT_VFR,
            a_ft: 0,
            b_ft: 0,
        },
        ALT_FORM_BLOCK | ALT_FORM_AT_FIX => Alt {
            form: if form == ALT_FORM_BLOCK {
                ALT_BLOCK
            } else {
                ALT_AT_FIX
            },
            a_ft: a?,
            b_ft: b?,
        },
        _ => return None,
    })
}

/// Appends the operations of row `row` of `ev` to `out`. `seen_ms` is when the message arrived and
/// `msg` the driver's number of the message; every operation carries the message's own time. The
/// flight is keyed by the GUFI (an SFDPS message always has one).
pub fn row_ops(
    ev: &Events,
    row: usize,
    seen_ms: i64,
    msg: u64,
    st: &mut AdapterStats,
    out: &mut Vec<Op>,
) {
    let (Some(&gufi), Some(&ts)) = (ev.gufi.get(row), ev.ts_ms.get(row)) else {
        return;
    };
    st.rows += 1;
    let key = key_gufi(gufi);
    let centre: [u8; 4] = ev.centre.get(row).copied().unwrap_or([0; 4]);
    let flags = ev.flags.get(row).copied().unwrap_or(0);
    let ok = |f: Field| has_field(ev, row, f);
    let mk = |kind: u8| op_new(kind, key, centre, ts, seen_ms, msg);
    let mut push = |o: Op, st: &mut AdapterStats| {
        if let Some(c) = st.ops.get_mut(usize::from(o.kind)) {
            *c += 1;
        }
        out.push(o);
    };

    // The plan status, and the departure that makes an active plan departed.
    let status = if ok(Field::Status) {
        ev.status.get(row).copied().unwrap_or(0)
    } else {
        0
    };
    let plan_id: [u8; 12] = if ok(Field::FlightPlanId) {
        ev.plan_id.get(row).copied().unwrap_or([0; 12])
    } else {
        [0; 12]
    };
    match status {
        STATUS_PROPOSED | STATUS_ACTIVE => {
            let mut o = mk(OP_SET_PLAN);
            o.state = if status == STATUS_PROPOSED {
                ST_PROPOSED
            } else {
                ST_ACTIVE
            };
            o.plan_id = plan_id;
            push(o, st);
            if ok(Field::DepartureActual) {
                if status == STATUS_ACTIVE {
                    push(mk(OP_DEPART), st);
                } else {
                    st.departure_not_active += 1;
                }
            }
        }
        STATUS_COMPLETED => push(mk(OP_COMPLETE), st),
        STATUS_CANCELLED => push(mk(OP_CANCEL), st),
        STATUS_DROPPED => push(mk(OP_DROP), st),
        _ => st.no_status += 1,
    }

    // Altitudes.
    if ok(Field::AssignedForm) {
        let form = ev.assigned_form.get(row).copied().unwrap_or(0);
        let value = ev.assigned_ft.get(row).copied().unwrap_or(0);
        let (a, b) = if form == ALT_FORM_BLOCK {
            (
                ev.block_above_ft.get(row).copied(),
                ev.block_below_ft.get(row).copied(),
            )
        } else {
            (
                ev.fixalt_pre_ft.get(row).copied(),
                ev.fixalt_post_ft.get(row).copied(),
            )
        };
        if let Some(alt) = alt_from(form, value, a, b) {
            let mut o = mk(OP_ASSIGN);
            o.alt = alt;
            if form == ALT_FORM_AT_FIX {
                o.fix = ev.fixalt_point.get(row).copied().unwrap_or([0; 12]);
            }
            push(o, st);
        }
    }
    if ok(Field::InterimAlt) {
        let alt = if flags & FLAG_INTERIM_NIL != 0 {
            Alt {
                form: ALT_NIL,
                a_ft: 0,
                b_ft: 0,
            }
        } else {
            Alt {
                form: ALT_SIMPLE,
                a_ft: ev.interim_ft.get(row).copied().unwrap_or(0),
                b_ft: 0,
            }
        };
        let mut o = mk(OP_INTERIM);
        o.alt = alt;
        push(o, st);
    }
    if ok(Field::RequestedForm) {
        let form = ev.requested_form.get(row).copied().unwrap_or(0);
        let value = ev.requested_ft.get(row).copied().unwrap_or(0);
        if let Some(alt) = alt_from(form, value, None, None) {
            let mut o = mk(OP_REQUEST);
            o.alt = alt;
            push(o, st);
        }
    }

    // Ownership and handoff.
    if ok(Field::ControllingUnit) {
        let u: [u8; 8] = ev.ctrl.get(row).copied().unwrap_or([0; 8]);
        if unit_code(&u) == "???" {
            st.owner_unknown += 1;
        } else {
            let mut o = mk(OP_OWNER);
            o.unit_a = u;
            push(o, st);
        }
    }
    let unit = |f: Field, col: &[[u8; 8]]| -> Option<[u8; 8]> {
        if ok(f) {
            col.get(row).copied()
        } else {
            None
        }
    };
    let (xfer, recv, acpt) = (
        unit(Field::HandoffTransferring, &ev.xfer),
        unit(Field::HandoffReceiving, &ev.recv),
        unit(Field::HandoffAccepting, &ev.acpt),
    );
    if ok(Field::HandoffEvent) {
        let event = ev.handoff_event.get(row).copied().unwrap_or(0);
        let made = match event {
            HANDOFF_INITIATION => xfer.zip(recv).map(|(x, r)| {
                let mut o = mk(OP_HO_INITIATE);
                o.unit_a = x;
                o.unit_b = r;
                o
            }),
            HANDOFF_UPDATE => recv.map(|r| {
                let mut o = mk(OP_HO_UPDATE);
                o.unit_b = r;
                o
            }),
            HANDOFF_ACCEPTANCE => acpt.map(|a| {
                let mut o = mk(OP_HO_ACCEPT);
                o.unit_b = a;
                o
            }),
            HANDOFF_TAKE_CONTROL => acpt.map(|a| {
                let mut o = mk(OP_HO_TAKE);
                o.unit_b = a;
                o
            }),
            HANDOFF_RETRACTION => Some(mk(OP_HO_RETRACT)),
            HANDOFF_FAILURE => Some(mk(OP_HO_FAIL)),
            _ => None,
        };
        match made {
            Some(o) => push(o, st),
            None => st.handoff_incomplete += 1,
        }
    } else if recv.is_some() {
        st.handoff_receiving_only += 1;
    }

    // Identity.
    if ok(Field::AircraftAddress) {
        if let Some(&a) = ev.address.get(row) {
            let mut o = mk(OP_ADDRESS);
            o.address = a;
            push(o, st);
        }
    }
    let callsign = ok(Field::Callsign)
        .then(|| ev.callsign.get(row).copied())
        .flatten();
    let dep = ok(Field::Departure)
        .then(|| ev.departure.get(row).copied())
        .flatten();
    let arr = ok(Field::Arrival)
        .then(|| ev.arrival.get(row).copied())
        .flatten();
    if callsign.is_some() || dep.is_some() || arr.is_some() {
        let mut o = mk(OP_IDENTITY);
        o.callsign = callsign.unwrap_or([0; 8]);
        o.dep = dep.unwrap_or([0; 12]);
        o.arr = arr.unwrap_or([0; 12]);
        push(o, st);
    }
}

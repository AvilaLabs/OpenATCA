#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-flight: the per-flight record of flight data processing (`docs/M4_DESIGN.md` sections 1 and
//! 2). Class: core (RULESET B1, B7).
//!
//! The record is built from operations a controller performs, not from one feed's messages: a
//! plan is set, departs, completes, is cancelled or dropped; an altitude is assigned, an interim
//! altitude given or cleared, one requested; a unit takes ownership; a handoff is initiated,
//! updated, accepted, retracted, fails or ends in taking control. A feed adapter (SFDPS is the
//! first, in `oa-flight-sfdps`) turns its messages into these operations, so a deployed system
//! that receives the controller entries directly feeds the same record.
//!
//! * One [`Flight`] per key: the GUFI where there is one, else the TFMS key (callsign,
//!   departure, arrival) (RULESET E7: a key part that is empty is no key). Each flight keeps one
//!   plan view per reporting centre, because each ERAM reports its own status for the national
//!   plan and the views interleave (a second centre's proposal arrives after the first one's
//!   activation).
//! * Every field the record holds is stamped with the time it last changed, the time it became
//!   available to us and the message it came from ([`Stamp`]). A value restated unchanged keeps
//!   its stamp.
//! * The lag: an operation is available at `max(seen, stamped + lag floor)` and never before an
//!   earlier operation was, so no function reads SFDPS as current before its time (UK-004). The
//!   driver feeds operations in the order they were seen.
//! * An operation that does not fit the state, is out of date, or names a flight that is not
//!   there is refused, counted by operation and reason and listed ([`Refusal`]); nothing is
//!   applied quietly. A second plan for one flight, and one aircraft address or flight key claimed
//!   by two live flights, are listed as [`Conflict`]s. A flight leaves the table only through
//!   [`flights_remove`] or [`flights_sweep`], each of which writes a [`Removal`] first.
//! * Memory is fixed at [`flights_new`]: flights, three hash indexes and three bounded logs. A
//!   full table refuses new flights (it never evicts); a full log counts what it could not
//!   list; the counts stay exact.
//!
//! No function allocates after construction, none panics, and the same operations in the same
//! order give the same record bit for bit ([`flights_digest`], [`flights_check`] audits every
//! invariant).

// The width of `usize` is fixed by construction: this crate does not build for a target whose
// pointers are not 64 bits (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

extern crate alloc;

mod apply;
mod index;
mod table;
mod types;

pub use apply::{flights_apply, flights_op_avail, op_new};
pub use table::{
    flight_by_address, flight_get, flight_is_flying, flight_is_live, flights_check, flights_digest,
    flights_flying_by_callsign, flights_len, flights_log_clear, flights_new, flights_remove,
    flights_sweep, view_of,
};
pub use types::{
    key_gufi, key_tfms, Alt, Conflict, Flight, FlightConfig, FlightError, FlightKey, FlightStats,
    FlightTable, Op, Outcome, Refusal, Removal, Stamp, View, ALT_AT_FIX, ALT_BLOCK, ALT_MAX_FT,
    ALT_MIN_FT, ALT_NIL, ALT_NONE, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS, C_ADDRESS_TWICE,
    C_COUNT, C_NAMES, C_PLAN_CHANGED, C_PLAN_REFILED, C_TFMS_KEY_TWICE, FLIGHT_FREE, F_ADDRESS,
    F_ASSIGNED, F_COUNT, F_HANDOFF, F_IDENTITY, F_INTERIM, F_NAMES, F_OWNER, F_REQUESTED,
    HO_ACCEPTED, HO_NONE, HO_OFFERED, KEY_FREE, KEY_GUFI, KEY_TFMS, MAX_VIEWS, OP_ADDRESS,
    OP_ASSIGN, OP_CANCEL, OP_COMPLETE, OP_COUNT, OP_DEPART, OP_DROP, OP_HO_ACCEPT, OP_HO_FAIL,
    OP_HO_INITIATE, OP_HO_RETRACT, OP_HO_TAKE, OP_HO_UPDATE, OP_IDENTITY, OP_INTERIM, OP_NAMES,
    OP_OWNER, OP_REQUEST, OP_SET_PLAN, REM_COUNT, REM_EXPLICIT, REM_NAMES, REM_RETIRED, REM_SILENT,
    R_BAD_KEY, R_BAD_TIME, R_BAD_TRANSITION, R_BAD_VALUE, R_COUNT, R_HANDOFF_PENDING, R_NAMES,
    R_NO_HANDOFF, R_NO_VIEW, R_SAME_STAMP_DIFFERENT, R_STALE, R_TABLE_FULL, R_TERMINAL,
    R_UNKNOWN_FLIGHT, R_VIEWS_FULL, ST_ACTIVE, ST_CANCELLED, ST_COMPLETED, ST_COUNT, ST_DEPARTED,
    ST_DROPPED, ST_NAMES, ST_NONE, ST_PROPOSED,
};

//! Plain data of the flight record. Core (pure).

use alloc::vec::Vec;

/// [`FlightKey::kind`]: a slot with no flight in it (never a key a caller may use).
pub const KEY_FREE: u8 = 255;
/// [`FlightKey::kind`]: keyed by the GUFI.
pub const KEY_GUFI: u8 = 0;
/// [`FlightKey::kind`]: keyed by the TFMS key (callsign, departure, arrival), the documented
/// fallback when a source has no GUFI.
pub const KEY_TFMS: u8 = 1;

/// The identity of one flight. Two flights are the same when their keys are equal in full
/// (RULESET E7): a GUFI key uses only `gufi`, a TFMS key only the three codes, and the unused
/// parts are zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FlightKey {
    pub gufi: u128,
    pub callsign: [u8; 8],
    pub dep: [u8; 12],
    pub arr: [u8; 12],
    pub kind: u8,
}
// Budget: held once per flight, compared on every operation.
const _: () = assert!(core::mem::size_of::<FlightKey>() == 64);

/// A GUFI key.
pub const fn key_gufi(gufi: u128) -> FlightKey {
    FlightKey {
        gufi,
        callsign: [0; 8],
        dep: [0; 12],
        arr: [0; 12],
        kind: KEY_GUFI,
    }
}

/// A TFMS key.
pub const fn key_tfms(callsign: [u8; 8], dep: [u8; 12], arr: [u8; 12]) -> FlightKey {
    FlightKey {
        gufi: 0,
        callsign,
        dep,
        arr,
        kind: KEY_TFMS,
    }
}

/// The altitude forms. They are alternatives, and a number never stands for a state: VFR has no
/// altitude, and an interim altitude that was cleared is [`ALT_NIL`], not zero.
/// Not stated (the field was never given).
pub const ALT_NONE: u8 = 0;
/// One altitude, `a_ft`.
pub const ALT_SIMPLE: u8 = 1;
/// VFR on top plus `a_ft` (the reading of the feed's `vfrPlus`, unconfirmed).
pub const ALT_VFR_ON_TOP_PLUS: u8 = 2;
/// VFR: no altitude.
pub const ALT_VFR: u8 = 3;
/// A block, `a_ft` (lower) to `b_ft` (upper).
pub const ALT_BLOCK: u8 = 4;
/// An altitude at a fix: `a_ft` before the fix, `b_ft` after it, the fix named beside it. The
/// reading of before and after is the feed's `pre` and `post`, unconfirmed.
pub const ALT_AT_FIX: u8 = 5;
/// Explicitly none: the source said this clearance field is cleared; this is distinct from a field
/// the source has never stated (`ALT_NONE`).
pub const ALT_NIL: u8 = 6;

/// An altitude, in feet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Alt {
    pub form: u8,
    pub a_ft: i32,
    pub b_ft: i32,
}
// Budget: three per flight.
const _: () = assert!(core::mem::size_of::<Alt>() == 12);

/// The range an altitude value may have, feet (the decoder's own).
pub const ALT_MIN_FT: i32 = -2_000;
pub const ALT_MAX_FT: i32 = 100_000;

/// Plan view states (one per flight and reporting centre). 0 is "no view".
pub const ST_NONE: u8 = 0;
pub const ST_PROPOSED: u8 = 1;
pub const ST_ACTIVE: u8 = 2;
pub const ST_DEPARTED: u8 = 3;
pub const ST_COMPLETED: u8 = 4;
pub const ST_CANCELLED: u8 = 5;
pub const ST_DROPPED: u8 = 6;
/// Number of state codes, for tables indexed by state.
pub const ST_COUNT: usize = 7;

/// Display names, indexed by state code.
pub const ST_NAMES: [&str; ST_COUNT] = [
    "none",
    "proposed",
    "active",
    "departed",
    "completed",
    "cancelled",
    "dropped",
];

/// Handoff states of a flight.
pub const HO_NONE: u8 = 0;
/// Offered by the transferring unit to the receiving unit.
pub const HO_OFFERED: u8 = 1;
/// Accepted by the accepting unit; ownership has not moved yet.
pub const HO_ACCEPTED: u8 = 2;

/// Plan views one flight can hold: one per reporting centre. A flight crosses at most the
/// centres of the country in a day (seven on the longest routes); eight is one more.
pub const MAX_VIEWS: usize = 8;

/// The state of one centre's view of a flight's plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct View {
    pub centre: [u8; 4],
    /// `ST_*`.
    pub state: u8,
    /// The plan identifier this view holds, zero padded; all zero when not yet stated.
    pub plan_id: [u8; 12],
    /// When the state last changed.
    pub stamp: Stamp,
}
// Budget: eight per flight.
const _: () = assert!(core::mem::size_of::<View>() == 48);

/// When a field last changed and where the change came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Stamp {
    /// The source's own time of the change (ERAM's), Unix ms.
    pub ts_ms: i64,
    /// When the change became available to us: never before `ts_ms` plus the lag floor and never
    /// before the one before it. Unix ms.
    pub avail_ms: i64,
    /// The message the change came from, as the driver numbered it; opaque here.
    pub msg: u64,
}
// Budget: eleven per flight.
const _: () = assert!(core::mem::size_of::<Stamp>() == 24);

/// Fields that carry a stamp, indexes into [`Flight::stamps`].
pub const F_ASSIGNED: usize = 0;
pub const F_INTERIM: usize = 1;
pub const F_REQUESTED: usize = 2;
pub const F_OWNER: usize = 3;
pub const F_HANDOFF: usize = 4;
pub const F_ADDRESS: usize = 5;
pub const F_IDENTITY: usize = 6;
/// Number of stamped fields.
pub const F_COUNT: usize = 7;

/// Display names, indexed by field.
pub const F_NAMES: [&str; F_COUNT] = [
    "assigned_altitude",
    "interim_altitude",
    "requested_altitude",
    "owner",
    "handoff",
    "address",
    "identity",
];

/// One flight's record.
// A3 exception, recorded: one array of structs, because every operation reads and writes several
// fields of one flight and no loop scans one field across all flights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Flight {
    pub key: FlightKey,
    pub assigned: Alt,
    pub interim: Alt,
    pub requested: Alt,
    /// The fix of an assigned altitude at a fix, zero padded.
    pub assigned_fix: [u8; 12],
    /// The unit and sector that owns the flight (the controlling unit), eight bytes as in
    /// `oa-fixm`: unit, sector. All zero when not yet stated.
    pub owner: [u8; 8],
    /// `HO_*`, and while one is pending the transferring unit, the receiving unit and, once
    /// accepted, the accepting unit.
    pub ho_state: u8,
    pub ho_from: [u8; 8],
    pub ho_to: [u8; 8],
    pub ho_accepted_by: [u8; 8],
    /// The 24-bit ICAO (Mode S) address; 0 when not stated.
    pub address: u32,
    /// The older flight (slot plus one, 0 for none) that claimed the same address, and the one
    /// that has the same callsign, departure and arrival: the address and TFMS-key indexes hold
    /// the newest claimant and each chain runs back to the oldest. Slots stay inside the table.
    pub addr_next: u32,
    pub tfms_next: u32,
    pub callsign: [u8; 8],
    pub dep: [u8; 12],
    pub arr: [u8; 12],
    /// Plan views in the order the centres were first seen; `n_views` of them are used.
    pub n_views: u8,
    pub views: [View; MAX_VIEWS],
    /// Stamps by `F_*`.
    pub stamps: [Stamp; F_COUNT],
    /// Operations applied to this flight (a restated value counts as unchanged, not applied).
    pub ops_applied: u32,
    /// The availability time of the newest applied operation, Unix ms.
    pub last_avail_ms: i64,
}
// Budget: held once per flight (tens of thousands live); read and written by every operation.
const _: () = assert!(core::mem::size_of::<Flight>() == 768);

/// The slot of a flight not in use.
pub const FLIGHT_FREE: Flight = Flight {
    key: FlightKey {
        gufi: 0,
        callsign: [0; 8],
        dep: [0; 12],
        arr: [0; 12],
        kind: KEY_FREE,
    },
    assigned: Alt {
        form: ALT_NONE,
        a_ft: 0,
        b_ft: 0,
    },
    interim: Alt {
        form: ALT_NONE,
        a_ft: 0,
        b_ft: 0,
    },
    requested: Alt {
        form: ALT_NONE,
        a_ft: 0,
        b_ft: 0,
    },
    assigned_fix: [0; 12],
    owner: [0; 8],
    ho_state: HO_NONE,
    ho_from: [0; 8],
    ho_to: [0; 8],
    ho_accepted_by: [0; 8],
    address: 0,
    addr_next: 0,
    tfms_next: 0,
    callsign: [0; 8],
    dep: [0; 12],
    arr: [0; 12],
    n_views: 0,
    views: [View {
        centre: [0; 4],
        state: ST_NONE,
        plan_id: [0; 12],
        stamp: Stamp {
            ts_ms: 0,
            avail_ms: 0,
            msg: 0,
        },
    }; MAX_VIEWS],
    stamps: [Stamp {
        ts_ms: 0,
        avail_ms: 0,
        msg: 0,
    }; F_COUNT],
    ops_applied: 0,
    last_avail_ms: 0,
};

/// Operation kinds. 0 is not an operation.
/// Sets the plan of the op's centre: proposed or active (`Op::state`), with its plan identifier.
pub const OP_SET_PLAN: u8 = 1;
pub const OP_DEPART: u8 = 2;
pub const OP_COMPLETE: u8 = 3;
pub const OP_CANCEL: u8 = 4;
pub const OP_DROP: u8 = 5;
/// Assigns an altitude (`Op::alt`, `Op::fix` for an altitude at a fix).
pub const OP_ASSIGN: u8 = 6;
/// Gives an interim altitude, or clears it (`ALT_NIL`).
pub const OP_INTERIM: u8 = 7;
pub const OP_REQUEST: u8 = 8;
/// A unit takes ownership (`Op::unit_a`).
pub const OP_OWNER: u8 = 9;
/// A handoff from `unit_a` to `unit_b`.
pub const OP_HO_INITIATE: u8 = 10;
/// The receiving unit of the pending handoff becomes `unit_b`.
pub const OP_HO_UPDATE: u8 = 11;
/// The pending handoff is accepted by `unit_b`.
pub const OP_HO_ACCEPT: u8 = 12;
pub const OP_HO_RETRACT: u8 = 13;
pub const OP_HO_FAIL: u8 = 14;
/// `unit_b` takes control of the flight: the pending handoff ends and it owns the flight.
pub const OP_HO_TAKE: u8 = 15;
/// The aircraft address (`Op::address`).
pub const OP_ADDRESS: u8 = 16;
/// The callsign, departure and arrival (`Op::callsign`, `dep`, `arr`; an all-zero part is not
/// stated and changes nothing).
pub const OP_IDENTITY: u8 = 17;
/// Number of operation codes.
pub const OP_COUNT: usize = 18;

/// Display names, indexed by operation code.
pub const OP_NAMES: [&str; OP_COUNT] = [
    "invalid",
    "set_plan",
    "depart",
    "complete",
    "cancel",
    "drop",
    "assign_altitude",
    "interim_altitude",
    "request_altitude",
    "owner",
    "handoff_initiate",
    "handoff_update",
    "handoff_accept",
    "handoff_retract",
    "handoff_fail",
    "take_control",
    "address",
    "identity",
];

/// One operation on a flight, as plain data. A7.2: cold struct, size guard waived (built and
/// consumed one at a time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Op {
    pub kind: u8,
    pub key: FlightKey,
    /// The reporting centre (the plan view the operation is about), zero padded.
    pub centre: [u8; 4],
    /// The source's time of the change, Unix ms.
    pub ts_ms: i64,
    /// When the driver received it, Unix ms.
    pub seen_ms: i64,
    /// The message it came from; opaque here.
    pub msg: u64,
    /// [`ST_PROPOSED`] or [`ST_ACTIVE`] for [`OP_SET_PLAN`].
    pub state: u8,
    pub alt: Alt,
    pub fix: [u8; 12],
    pub unit_a: [u8; 8],
    pub unit_b: [u8; 8],
    pub plan_id: [u8; 12],
    pub address: u32,
    pub callsign: [u8; 8],
    pub dep: [u8; 12],
    pub arr: [u8; 12],
}

/// Why an operation was refused.
pub const R_BAD_KEY: u8 = 0;
/// Its time is later than the receive time by more than the allowed clock difference.
pub const R_BAD_TIME: u8 = 1;
/// A value out of range, a form the field cannot hold, or an altitude operation with no altitude.
pub const R_BAD_VALUE: u8 = 2;
/// The flight is not in the table and the operation cannot make it.
pub const R_UNKNOWN_FLIGHT: u8 = 3;
/// The table is full: a new flight is refused, never made by evicting another.
pub const R_TABLE_FULL: u8 = 4;
/// More centres than [`MAX_VIEWS`] report this flight.
pub const R_VIEWS_FULL: u8 = 5;
/// The centre has no plan view of the flight, so there is nothing to depart, complete, cancel or
/// drop.
pub const R_NO_VIEW: u8 = 6;
/// The state does not allow the transition (a backward step, or a step that skips one).
pub const R_BAD_TRANSITION: u8 = 7;
/// Every plan of the flight has ended (completed or cancelled): no amendment fits.
pub const R_TERMINAL: u8 = 8;
/// The operation is older than what the field holds.
pub const R_STALE: u8 = 9;
/// The operation has the time the field already holds and a different value; the earlier one
/// stays and the order of arrival decides nothing else.
pub const R_SAME_STAMP_DIFFERENT: u8 = 10;
/// A handoff step with no handoff pending.
pub const R_NO_HANDOFF: u8 = 11;
/// A handoff initiated while another is pending.
pub const R_HANDOFF_PENDING: u8 = 12;
/// Number of refusal reasons.
pub const R_COUNT: usize = 13;

/// Display names, indexed by reason.
pub const R_NAMES: [&str; R_COUNT] = [
    "bad key",
    "time after receipt",
    "bad value",
    "unknown flight",
    "table full",
    "views full",
    "no plan view of the centre",
    "bad transition",
    "plan ended",
    "stale",
    "same time, different value",
    "no handoff pending",
    "handoff already pending",
];

/// What applying an operation did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The record changed.
    Applied,
    /// The operation restated what the record holds; nothing changed, no stamp moved.
    Unchanged,
    /// The operation was refused, counted and listed, for this reason (`R_*`).
    Refused(u8),
}

/// A refused operation, as listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Refusal {
    pub key: FlightKey,
    pub ts_ms: i64,
    pub avail_ms: i64,
    pub msg: u64,
    pub op: u8,
    pub reason: u8,
    /// The state of the op's plan view when it was refused, [`ST_NONE`] if there was none.
    pub state: u8,
}
// Budget: one per listed refusal, bounded by the log capacity.
const _: () = assert!(core::mem::size_of::<Refusal>() == 96);

/// Conflict kinds.
/// A flight's plan view gets a different plan identifier while its plan is live: two plans for
/// one flight.
pub const C_PLAN_CHANGED: u8 = 0;
/// A new plan identifier starts on a view whose earlier plan had ended: the flight was filed again
/// under the same key.
pub const C_PLAN_REFILED: u8 = 1;
/// An aircraft address is claimed by two live flights.
pub const C_ADDRESS_TWICE: u8 = 2;
/// Two live flights have the same callsign, departure and arrival.
pub const C_TFMS_KEY_TWICE: u8 = 3;
/// Number of conflict kinds.
pub const C_COUNT: usize = 4;

/// Display names, indexed by conflict kind.
pub const C_NAMES: [&str; C_COUNT] = [
    "plan changed",
    "plan filed again",
    "address on two live flights",
    "same callsign, departure and arrival on two live flights",
];

/// A conflict, as listed: `a` is the flight the operation was about and `b` the other one (the
/// same flight for the plan conflicts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Conflict {
    pub a: FlightKey,
    pub b: FlightKey,
    pub ts_ms: i64,
    pub msg: u64,
    pub kind: u8,
}
// Budget: one per listed conflict, bounded by the log capacity.
const _: () = assert!(core::mem::size_of::<Conflict>() == 160);

/// Why a flight left the table.
/// [`flights_remove`], called by the owner of the table.
pub const REM_EXPLICIT: u8 = 0;
/// Every plan view had ended and the flight was older than the retention.
pub const REM_RETIRED: u8 = 1;
/// A live flight with no operation for longer than the silence limit.
pub const REM_SILENT: u8 = 2;
/// Number of removal reasons.
pub const REM_COUNT: usize = 3;

/// Display names, indexed by removal reason.
pub const REM_NAMES: [&str; REM_COUNT] =
    ["explicit", "retired after the end of its plans", "silent"];

/// A flight's removal from the table, written before it is removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Removal {
    pub key: FlightKey,
    pub at_ms: i64,
    /// The flight's availability time of its newest operation.
    pub last_avail_ms: i64,
    pub reason: u8,
    /// The most advanced plan state of the flight.
    pub state: u8,
}
// Budget: one per listed removal, bounded by the log capacity.
const _: () = assert!(core::mem::size_of::<Removal>() == 96);

/// The settings of a table. Every value is the caller's; there are no defaults. A7.2: cold
/// struct, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlightConfig {
    /// Flights the table can hold. Peak: the national day holds about 45,000 flights in the
    /// development recording; the caller states its margin.
    pub capacity: usize,
    /// The measured lag floor of the feed, ms: an operation is available no earlier than its
    /// stamped time plus this (UK-004: SFDPS arrives 58 to 61 s after its timestamp).
    pub lag_floor_ms: i64,
    /// An operation stamped later than its receive time by more than this is refused (clock
    /// difference), ms.
    pub max_future_ms: i64,
    /// Capacity of each log: refusals, conflicts, removals. A full log counts what it cannot list.
    pub refusal_log: usize,
    pub conflict_log: usize,
    pub removal_log: usize,
}

/// Counts. Every refusal is in `refused` by operation and reason, so the sums are exact whatever
/// the logs hold. A7.2: cold struct, size guard waived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlightStats {
    pub ops: u64,
    pub applied: u64,
    pub unchanged: u64,
    /// Refusals, `[op * R_COUNT + reason]`.
    pub refused: [u64; OP_COUNT * R_COUNT],
    /// Operations whose availability was raised to stamped time plus the lag floor.
    pub lag_raised: u64,
    /// Operations whose availability was raised to the clock because an earlier operation was
    /// available later (the clock never goes back).
    pub clock_held: u64,
    /// Flights made, and removed (each removal has a log entry).
    pub created: u64,
    pub removed: u64,
    /// Removals not made because the removal log was full.
    pub removals_blocked: u64,
    /// Plan views a flight gained (a flight reported by one more centre).
    pub views_added: u64,
    /// A dropped view that became active again.
    pub reinstated: u64,
    /// Handoffs that ended in the owner taking over the accepted unit, and handoffs the owner
    /// changed under without an acceptance seen.
    pub handoffs_completed: u64,
    pub handoffs_implicit: u64,
    pub handoffs_retracted: u64,
    pub handoffs_failed: u64,
    /// Owner operations that left a pending handoff alone because they were older than it.
    pub handoff_clears_skipped: u64,
    pub conflicts: [u64; C_COUNT],
    /// Entries a full log could not list.
    pub refusals_unlisted: u64,
    pub conflicts_unlisted: u64,
}

/// The table: flights, indexes and logs, allocated by [`flights_new`]. Fields are the owner's
/// data; the interface is the free functions.
// A7.2: cold struct (one per run), size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlightTable {
    pub cfg: FlightConfig,
    pub flights: Vec<Flight>,
    /// Free slots, a stack.
    pub free: Vec<u32>,
    pub(crate) by_key: crate::index::Index,
    pub(crate) by_addr: crate::index::Index,
    pub(crate) by_tfms: crate::index::Index,
    /// The replay clock: the availability time of the newest operation.
    pub clock_ms: i64,
    pub stats: FlightStats,
    pub refusals: Vec<Refusal>,
    pub conflicts: Vec<Conflict>,
    pub removals: Vec<Removal>,
}

/// Why a configuration or a table was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightError {
    /// A configuration value out of range; the text names it.
    Config(&'static str),
    /// An invariant of the table does not hold; the text names it (the audit, [`flights_check`]).
    Invariant(&'static str),
}

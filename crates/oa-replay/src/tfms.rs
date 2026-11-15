//! The TFMS tables: the latest flow-management view of every flight (keyed by TFMS flight
//! reference) and of every traffic management programme (restriction, advisory, FCA/FEA, airport
//! configuration), applied from `oa_tfms` rows. Core (pure, B1): no I/O, no clock (time is an
//! argument, B4), no allocation after [`new_tfms_table`] and [`new_flow_table`] (C1).
//!
//! **Flights.** One slot per distinct `flightRef`. A row fills the values it carries and leaves
//! the others, so each time (estimated departure and arrival, actual, scheduled, original,
//! runway, gate, fix and TMI times) is the latest message that had it. A row whose
//! `sourceTimeStamp` is older than the flight's latest is refused whole and counted, the same
//! rule as TBFM (OA-RPL-004); an equal time applies. Identity (acid, departure, arrival, the
//! 10-character identifier) fills an empty column and a different value is a counted conflict;
//! a TMI flight list entry gives its airports as FAA identifiers (`IAD`, where every other message
//! has `KIAD`), so its airports are not merged (on the 2026-11-14 sample, merging them
//! made nearly all of the identity disagreements).
//! A row with no `flightRef` is not applied.
//!
//! **Link to SFDPS.** `oa_tfms::link` finds that the TFMS `gufi` equals SFDPS
//! `flightPlan@identifier`, which the flight table indexes, so a flight with a `gufi` is linked
//! through [`find_plan`]; only a flight with no `gufi` falls back to acid, departure and arrival,
//! linked when exactly one flight in the flight table has them (a key that names several is
//! counted as ambiguous and left unlinked). That fallback scans the flight table (at most
//! [`crate::DEFAULT_CAPACITY`] comparisons) on the flight's 1st, 2nd, 4th, 8th ... row while it is
//! not linked, and only for flights that never carry a `gufi`. The link is the SFDPS GUFI, not a slot, so it survives the
//! flight table recycling slots; it is retried when the linked flight has been evicted.
//!
//! **Programmes.** One slot per `(kind, program, id, number, title, element)` of a flow row, so
//! the latest row for a restriction, advisory, FCA or airport configuration replaces the held
//! one. Title and element are kept to their first 24 bytes (the key distinguishes only those).
//!
//! **Expiry.** A sweep on the replay-time cadence evicts a flight silent for
//! [`TFMS_FLIGHT_MAX_AGE_NS`] and a programme whose end time has passed (a programme with no
//! end time goes after [`TFMS_FLOW_MAX_AGE_NS`] of silence).
//!
//! The state is hashed by [`tfms_hash_into`] in key order, so it does not depend on the order in
//! which flights were first seen.

use oa_sha256::Sha256;
use oa_tfms::rows::flag;
use oa_tfms::{flag_get, flight_at, flow_at, text, Rows, StrRef, NO_NUM, NO_TIME};

use crate::hash::stale_hash_into;
use crate::index::{hash_bytes, mix64};
use crate::slots::{new_slots, slot_live, slots_alloc, slots_find, slots_release, Slots};
use crate::table::{find_plan, is_occupied, slot_of_gufi, FlightTable, Slot};

/// Flights the TFMS table holds: 65,536. A day of the NAS is about 50,000 flights with a TFMS
/// reference, and a flight is evicted six hours after its last message.
pub const TFMS_CAPACITY: usize = 1 << 16;
/// Programmes the flow table holds: 4,096. The 2026-11-14 sample has one or two live per
/// restriction, FCA or airport and a few hundred in all.
pub const FLOW_CAPACITY: usize = 4096;
/// Silence after which a TFMS flight is evicted: six hours of replay time.
pub const TFMS_FLIGHT_MAX_AGE_NS: i64 = 6 * 3600 * 1_000_000_000;
/// Silence after which a programme with no end time is evicted: 24 hours of replay time.
pub const TFMS_FLOW_MAX_AGE_NS: i64 = 24 * 3600 * 1_000_000_000;
/// Replay-time spacing of the sweeps: one minute.
pub const TFMS_SWEEP_NS: i64 = 60 * 1_000_000_000;

/// Tag and version of the TFMS flight part of the canonical serialisation.
pub const TFMS_HASH_TAG: &[u8; 5] = b"OATF1";
/// Tag and version of the TFMS programme part.
pub const FLOW_HASH_TAG: &[u8; 5] = b"OATW1";

/// Time columns of a flight, the index into [`TfmsTable::times`].
pub mod tcol {
    /// `eta@timeValue`.
    pub const ETA: usize = 0;
    /// `etd@timeValue`.
    pub const ETD: usize = 1;
    /// `timeOfDeparture`.
    pub const TOD: usize = 2;
    /// `timeOfArrival`.
    pub const TOA: usize = 3;
    /// `flightTimeData@originalDeparture`.
    pub const ORIG_DEP: usize = 4;
    /// `flightTimeData@originalArrival`.
    pub const ORIG_ARR: usize = 5;
    /// `runwayDeparture`.
    pub const RWY_DEP: usize = 6;
    /// `runwayArrival`.
    pub const RWY_ARR: usize = 7;
    /// `gateDeparture`.
    pub const GATE_DEP: usize = 8;
    /// `gateArrival`.
    pub const GATE_ARR: usize = 9;
    /// `airlineOutTime`.
    pub const OUT: usize = 10;
    /// `airlineOffTime`.
    pub const OFF: usize = 11;
    /// `airlineOnTime`.
    pub const ON: usize = 12;
    /// `airlineInTime`.
    pub const IN: usize = 13;
    /// `arrivalFixAndTime@arrTime`.
    pub const ARR_FIX: usize = 14;
    /// `departureFixAndTime@arrTime`.
    pub const DEP_FIX: usize = 15;
    /// `coordinationTime`.
    pub const COORD: usize = 16;
    /// `tmi@lastUpdateTime`.
    pub const TMI: usize = 17;
    /// Number of time columns.
    pub const COUNT: usize = 18;
}

/// [`TfmsTable::link_kind`]: not linked.
pub const LINK_NONE: u8 = 0;
/// Linked through the identifier equal to the SFDPS flight-plan identifier.
pub const LINK_PLAN: u8 = 1;
/// Linked through acid, departure and arrival (the flight has no identifier).
pub const LINK_ACID: u8 = 2;

/// Running totals of the flight table, kept for the whole run.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TfmsStats {
    /// Flight rows applied.
    pub applied: u64,
    /// Flights added.
    pub created: u64,
    /// Rows refused because they were older than the flight's latest.
    pub out_of_order: u64,
    /// Rows refused because the table was full and the flight is new.
    pub refused_full: u64,
    /// Rows with no flight reference: not applied.
    pub no_ref: u64,
    /// Identity fields that disagreed with a value already held (the held value is kept).
    pub conflicts: u64,
    /// Links made through the flight-plan identifier.
    pub linked_plan: u64,
    /// Links made through acid, departure and arrival.
    pub linked_acid: u64,
    /// Flights whose acid, departure and arrival named several SFDPS flights (left unlinked).
    pub ambiguous: u64,
    /// Flights evicted by age.
    pub evicted: u64,
    /// Sweeps run.
    pub sweeps: u64,
}

/// What became of one flight row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TfmsApplied {
    /// Applied; the flight was new.
    New,
    /// Applied to a known flight.
    Updated,
    /// Older than the flight's latest message; refused whole.
    OutOfOrder,
    /// New flight and the table is full; refused.
    TableFull,
    /// The row has no flight reference.
    NoRef,
    /// The row index was outside the rows (cannot happen from the pipeline).
    BadRow,
}

/// The flight table. Plain data (A1); every column has length `slots.capacity`.
// A7.2: cold struct, size guard waived (one per pipeline)
#[derive(Clone, Debug)]
pub struct TfmsTable {
    /// Slot bookkeeping, keyed by flight reference.
    pub slots: Slots,
    /// Replay time of the next sweep, or `i64::MIN` before the first frame has scheduled one.
    pub next_sweep_ns: i64,
    /// Totals.
    pub stats: TfmsStats,
    /// Flight reference.
    pub flight_ref: Vec<u64>,
    /// Largest `sourceTimeStamp` applied, Unix ms.
    pub last_src_ms: Vec<i64>,
    /// Replay time of the last applied row, ns.
    pub last_update_ns: Vec<i64>,
    /// Whether the row was last updated before a confirmed step of the replay clock and has not
    /// been updated since: its age is the time since the step, not since it was heard.
    pub stale: Vec<bool>,
    /// The 10-character identifier (equal to the SFDPS flight-plan identifier); all zero if none.
    pub gufi: Vec<[u8; 16]>,
    /// Aircraft identification.
    pub acid: Vec<[u8; 8]>,
    /// Departure airport.
    pub dep: Vec<[u8; 8]>,
    /// Arrival airport.
    pub arr: Vec<[u8; 8]>,
    /// Latest value of each time column ([`tcol`]); `NO_TIME` if never seen.
    pub times: Vec<[i64; tcol::COUNT]>,
    /// `etaType` and `etdType` of the held estimates, 0 if none.
    pub eta_type: Vec<u8>,
    /// See `eta_type`.
    pub etd_type: Vec<u8>,
    /// `flightStatus`, 0 if never seen.
    pub status: Vec<u8>,
    /// `tmi@updateType` of the latest TMI flight list entry, 0 if none.
    pub tmi_update: Vec<u8>,
    /// The tri-state estimate flags of `timeOfDeparture` and `timeOfArrival` (`oa_tfms::flag`).
    pub flags: Vec<u16>,
    /// `fxaFlight` entries of the latest TMI flight list entry.
    pub n_fxa: Vec<u16>,
    /// The first FCA of the latest TMI flight list entry, first 16 bytes; all zero if none.
    pub fca: Vec<[u8; 16]>,
    /// The arrival fix and departure fix names.
    pub arr_fix: Vec<[u8; 8]>,
    /// See `arr_fix`.
    pub dep_fix: Vec<[u8; 8]>,
    /// The SFDPS GUFI of the linked flight; meaningful when `link_kind` is not [`LINK_NONE`].
    pub link: Vec<u128>,
    /// [`LINK_NONE`], [`LINK_PLAN`] or [`LINK_ACID`].
    pub link_kind: Vec<u8>,
    /// Rows applied to this flight.
    pub rows: Vec<u32>,
    /// Rows for this flight refused as older.
    pub out_of_order: Vec<u32>,
}

/// An empty flight table of `capacity` flights (use [`TFMS_CAPACITY`]).
pub fn new_tfms_table(capacity: usize) -> TfmsTable {
    let n = capacity;
    TfmsTable {
        slots: new_slots(n),
        next_sweep_ns: i64::MIN,
        stats: TfmsStats::default(),
        flight_ref: vec![0; n],
        last_src_ms: vec![NO_TIME; n],
        last_update_ns: vec![0; n],
        stale: vec![false; n],
        gufi: vec![[0; 16]; n],
        acid: vec![[0; 8]; n],
        dep: vec![[0; 8]; n],
        arr: vec![[0; 8]; n],
        times: vec![[NO_TIME; tcol::COUNT]; n],
        eta_type: vec![0; n],
        etd_type: vec![0; n],
        status: vec![0; n],
        tmi_update: vec![0; n],
        flags: vec![0; n],
        n_fxa: vec![0; n],
        fca: vec![[0; 16]; n],
        arr_fix: vec![[0; 8]; n],
        dep_fix: vec![[0; 8]; n],
        link: vec![0; n],
        link_kind: vec![LINK_NONE; n],
        rows: vec![0; n],
        out_of_order: vec![0; n],
    }
}

fn ref_hash(r: u64) -> u64 {
    mix64(r)
}

/// The slot of flight reference `r`, if held.
pub fn tfms_slot_of(t: &TfmsTable, r: u64) -> Option<usize> {
    slots_find(&t.slots, ref_hash(r), |s| t.flight_ref.get(s) == Some(&r))
}

/// Number of flights held.
pub fn tfms_live(t: &TfmsTable) -> usize {
    t.slots.live
}

/// Flights that carry a link to an SFDPS flight.
pub fn tfms_linked(t: &TfmsTable) -> usize {
    (0..t.slots.capacity)
        .filter(|&i| {
            slot_live(&t.slots, i) && t.link_kind.get(i).copied().unwrap_or(LINK_NONE) != LINK_NONE
        })
        .count()
}

fn set<T>(col: &mut [T], slot: usize, v: T) {
    if let Some(c) = col.get_mut(slot) {
        *c = v;
    }
}

/// `src` zero-padded (or cut) to `N` bytes.
pub(crate) fn pad<const N: usize>(src: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    for (o, s) in out.iter_mut().zip(src) {
        *o = *s;
    }
    out
}

fn blank_flight(t: &mut TfmsTable, s: usize) {
    set(&mut t.flight_ref, s, 0);
    set(&mut t.last_src_ms, s, NO_TIME);
    set(&mut t.last_update_ns, s, 0);
    set(&mut t.stale, s, false);
    set(&mut t.gufi, s, [0; 16]);
    set(&mut t.acid, s, [0; 8]);
    set(&mut t.dep, s, [0; 8]);
    set(&mut t.arr, s, [0; 8]);
    set(&mut t.times, s, [NO_TIME; tcol::COUNT]);
    set(&mut t.eta_type, s, 0);
    set(&mut t.etd_type, s, 0);
    set(&mut t.status, s, 0);
    set(&mut t.tmi_update, s, 0);
    set(&mut t.flags, s, 0);
    set(&mut t.n_fxa, s, 0);
    set(&mut t.fca, s, [0; 16]);
    set(&mut t.arr_fix, s, [0; 8]);
    set(&mut t.dep_fix, s, [0; 8]);
    set(&mut t.link, s, 0);
    set(&mut t.link_kind, s, LINK_NONE);
    set(&mut t.rows, s, 0);
    set(&mut t.out_of_order, s, 0);
}

/// Fills an empty identity column from `v`; a different non-empty value is a conflict (counted,
/// held value kept).
fn fill_name<const N: usize>(col: &mut [[u8; N]], s: usize, v: &[u8], conflicts: &mut u64) {
    if v.iter().all(|&b| b == 0) {
        return;
    }
    let new: [u8; N] = pad(v);
    if let Some(c) = col.get_mut(s) {
        if *c == [0; N] {
            *c = new;
        } else if *c != new {
            *conflicts += 1;
        }
    }
}

/// Applies flight row `i` of `rows` at replay time `now_ns`. `flights` is the SFDPS flight table
/// the link is resolved against.
pub fn apply_tfms_flight(
    t: &mut TfmsTable,
    flights: &FlightTable,
    rows: &Rows,
    i: usize,
    now_ns: i64,
) -> TfmsApplied {
    let Some(r) = flight_at(&rows.flights, i) else {
        return TfmsApplied::BadRow;
    };
    if r.flight_ref == 0 {
        t.stats.no_ref += 1;
        return TfmsApplied::NoRef;
    }
    let (s, is_new) = match tfms_slot_of(t, r.flight_ref) {
        Some(s) => (s, false),
        None => {
            let Some(s) = slots_alloc(&mut t.slots, ref_hash(r.flight_ref)) else {
                t.stats.refused_full += 1;
                return TfmsApplied::TableFull;
            };
            blank_flight(t, s);
            set(&mut t.flight_ref, s, r.flight_ref);
            t.stats.created += 1;
            (s, true)
        }
    };
    if !is_new && t.last_src_ms.get(s).is_some_and(|&held| r.src_ms < held) {
        t.stats.out_of_order += 1;
        if let Some(c) = t.out_of_order.get_mut(s) {
            *c = c.saturating_add(1);
        }
        return TfmsApplied::OutOfOrder;
    }
    if t.last_src_ms.get(s).is_some_and(|&held| r.src_ms > held) {
        set(&mut t.last_src_ms, s, r.src_ms);
    }
    set(&mut t.last_update_ns, s, now_ns);
    set(&mut t.stale, s, false);
    let mut conflicts = 0;
    fill_name(&mut t.gufi, s, &r.gufi, &mut conflicts);
    fill_name(&mut t.acid, s, &r.acid, &mut conflicts);
    // A TMI flight list names its airports by FAA identifier (`IAD`), every other message by ICAO
    // (`KIAD`): they are not merged (the same split as TBFM and SFDPS, OA-RPL-005).
    if r.kind != oa_tfms::rows::kind::TMI_LIST {
        fill_name(&mut t.dep, s, &r.dep, &mut conflicts);
        fill_name(&mut t.arr, s, &r.arr, &mut conflicts);
    }
    t.stats.conflicts += conflicts;
    if let Some(row_times) = t.times.get_mut(s) {
        for (col, v) in [
            (tcol::ETA, r.eta_ms),
            (tcol::ETD, r.etd_ms),
            (tcol::TOD, r.tod_ms),
            (tcol::TOA, r.toa_ms),
            (tcol::ORIG_DEP, r.orig_dep_ms),
            (tcol::ORIG_ARR, r.orig_arr_ms),
            (tcol::RWY_DEP, r.rwy_dep_ms),
            (tcol::RWY_ARR, r.rwy_arr_ms),
            (tcol::GATE_DEP, r.gate_dep_ms),
            (tcol::GATE_ARR, r.gate_arr_ms),
            (tcol::OUT, r.out_ms),
            (tcol::OFF, r.off_ms),
            (tcol::ON, r.on_ms),
            (tcol::IN, r.in_ms),
            (tcol::ARR_FIX, r.arrfix_ms),
            (tcol::DEP_FIX, r.depfix_ms),
            (tcol::COORD, r.coord_ms),
            (tcol::TMI, r.tmi_ms),
        ] {
            if v != NO_TIME {
                if let Some(c) = row_times.get_mut(col) {
                    *c = v;
                }
            }
        }
    }
    if r.eta_ms != NO_TIME && r.eta_type != 0 {
        set(&mut t.eta_type, s, r.eta_type);
    }
    if r.etd_ms != NO_TIME && r.etd_type != 0 {
        set(&mut t.etd_type, s, r.etd_type);
    }
    if r.status != 0 {
        set(&mut t.status, s, r.status);
    }
    if r.tmi_update != 0 {
        set(&mut t.tmi_update, s, r.tmi_update);
        set(&mut t.n_fxa, s, r.n_fxa);
        set(&mut t.fca, s, pad(fca_bytes(rows, r.fca)));
    }
    if let Some(f) = t.flags.get_mut(s) {
        for pos in [flag::TOD_EST, flag::TOA_EST] {
            let v = flag_get(r.flags, pos);
            if v != flag::ABSENT {
                *f = (*f & !(3 << pos)) | (v << pos);
            }
        }
    }
    if r.arr_fix != [0; 8] {
        set(&mut t.arr_fix, s, r.arr_fix);
    }
    if r.dep_fix != [0; 8] {
        set(&mut t.dep_fix, s, r.dep_fix);
    }
    if let Some(c) = t.rows.get_mut(s) {
        *c = c.saturating_add(1);
    }
    link_flight(t, s, flights);
    t.stats.applied += 1;
    if is_new {
        TfmsApplied::New
    } else {
        TfmsApplied::Updated
    }
}

fn fca_bytes(rows: &Rows, s: StrRef) -> &[u8] {
    text(rows, s).as_bytes()
}

/// Resolves slot `s`'s link to the SFDPS flight table (see the module comment).
fn link_flight(t: &mut TfmsTable, s: usize, flights: &FlightTable) {
    let held = t.link_kind.get(s).copied().unwrap_or(LINK_NONE);
    if held != LINK_NONE {
        let g = t.link.get(s).copied().unwrap_or(0);
        if slot_of_gufi(flights, g).is_some() {
            return;
        }
        set(&mut t.link_kind, s, LINK_NONE);
    }
    let gufi = t.gufi.get(s).copied().unwrap_or([0; 16]);
    if gufi != [0; 16] {
        let plan: [u8; 12] = pad(&gufi);
        // A longer identifier cannot equal a 12-byte plan identifier.
        if gufi
            .get(12..)
            .is_some_and(|rest| rest.iter().all(|&b| b == 0))
        {
            if let Some(f) = find_plan(flights, &plan) {
                if let Some(&g) = flights.gufi.get(f.0 as usize) {
                    set(&mut t.link, s, g);
                    set(&mut t.link_kind, s, LINK_PLAN);
                    t.stats.linked_plan += 1;
                }
            }
        }
        return;
    }
    let acid = t.acid.get(s).copied().unwrap_or([0; 8]);
    let dep = t.dep.get(s).copied().unwrap_or([0; 8]);
    let arr = t.arr.get(s).copied().unwrap_or([0; 8]);
    if acid == [0; 8] || dep == [0; 8] || arr == [0; 8] {
        return;
    }
    // The scan visits the whole flight table, so it is tried on the flight's 1st, 2nd, 4th, 8th
    // ... row only (at most log2(rows) + 1 scans per flight), and not at all while the flight
    // table is empty.
    let n = t.rows.get(s).copied().unwrap_or(0);
    if flights.live == 0 || n & n.wrapping_sub(1) != 0 {
        return;
    }
    let (dep, arr): ([u8; 12], [u8; 12]) = (pad(&dep), pad(&arr));
    let mut found: Option<usize> = None;
    for i in 0..flights.capacity {
        if !is_occupied(flights, Slot(i as u32)) {
            continue;
        }
        if flights.callsign.get(i) == Some(&acid)
            && flights.departure.get(i) == Some(&dep)
            && flights.arrival.get(i) == Some(&arr)
        {
            if found.is_some() {
                t.stats.ambiguous += 1;
                return;
            }
            found = Some(i);
        }
    }
    if let Some(&g) = found.and_then(|i| flights.gufi.get(i)) {
        set(&mut t.link, s, g);
        set(&mut t.link_kind, s, LINK_ACID);
        t.stats.linked_acid += 1;
    }
}

/// Evicts every flight silent for more than [`TFMS_FLIGHT_MAX_AGE_NS`] before `now_ns`; returns
/// the number evicted.
pub fn tfms_sweep(t: &mut TfmsTable, now_ns: i64) -> usize {
    let mut n = 0;
    for i in 0..t.slots.capacity {
        if !slot_live(&t.slots, i) {
            continue;
        }
        let last = t.last_update_ns.get(i).copied().unwrap_or(0);
        if now_ns.saturating_sub(last) > TFMS_FLIGHT_MAX_AGE_NS {
            slots_release(&mut t.slots, i);
            blank_flight(t, i);
            t.stats.evicted += 1;
            n += 1;
        }
    }
    t.stats.sweeps += 1;
    n
}

/// Moves the last-update time of every flight by `delta_ns`, for a step of the replay clock (the
/// time since each was heard stays what it was). Every flight is marked stale until it is
/// updated; returns the number marked.
pub fn tfms_shift_times(t: &mut TfmsTable, delta_ns: i64) -> usize {
    let mut n = 0;
    for i in 0..t.slots.capacity {
        if slot_live(&t.slots, i) {
            if let Some(v) = t.last_update_ns.get_mut(i) {
                *v = v.saturating_add(delta_ns);
            }
            if let Some(v) = t.stale.get_mut(i) {
                *v = true;
                n += 1;
            }
        }
    }
    n
}

/// The number of rows that are stale: last updated before a confirmed step of the replay clock
/// and not updated since.
pub fn tfms_stale_count(t: &TfmsTable) -> usize {
    (0..t.slots.capacity)
        .filter(|&i| slot_live(&t.slots, i) && t.stale.get(i).copied().unwrap_or(false))
        .count()
}

/// The same for the programme table.
pub fn flow_shift_times(t: &mut FlowTable, delta_ns: i64) -> usize {
    let mut n = 0;
    for i in 0..t.slots.capacity {
        if slot_live(&t.slots, i) {
            if let Some(v) = t.last_update_ns.get_mut(i) {
                *v = v.saturating_add(delta_ns);
            }
            if let Some(v) = t.stale.get_mut(i) {
                *v = true;
                n += 1;
            }
        }
    }
    n
}

/// The number of rows that are stale: last updated before a confirmed step of the replay clock
/// and not updated since.
pub fn flow_stale_count(t: &FlowTable) -> usize {
    (0..t.slots.capacity)
        .filter(|&i| slot_live(&t.slots, i) && t.stale.get(i).copied().unwrap_or(false))
        .count()
}

fn next_boundary(now: i64, step: i64) -> i64 {
    now.div_euclid(step).saturating_add(1).saturating_mul(step)
}

/// Whether a sweep is due at replay time `now_ns` on the cadence `next_sweep_ns` keeps (the first
/// call only schedules the first). Sweeps fall on multiples of [`TFMS_SWEEP_NS`], so when they
/// run does not depend on when the replay started.
pub fn tfms_due(next_sweep_ns: &mut i64, now_ns: i64) -> bool {
    if *next_sweep_ns == i64::MIN {
        *next_sweep_ns = next_boundary(now_ns, TFMS_SWEEP_NS);
        return false;
    }
    if now_ns < *next_sweep_ns {
        return false;
    }
    *next_sweep_ns = next_boundary(now_ns, TFMS_SWEEP_NS);
    true
}

fn put(h: &mut Sha256, b: &[u8]) {
    // The only failure is a message over 2^61 bytes.
    let _ = h.update(b);
}

/// Writes an identifier that can be ten digits wide. A value below `u32::MAX` is its four bytes
/// little-endian, exactly as when the identifiers were `u32` (so a checkpoint of ordinary data
/// keeps its hash); a larger one is `u32::MAX` as an escape followed by its eight bytes, which
/// keeps the encoding prefix-free (no value can be confused with the start of another).
fn put_id(h: &mut Sha256, v: u64) {
    match u32::try_from(v) {
        Ok(n) if n != u32::MAX => put(h, &n.to_le_bytes()),
        _ => {
            put(h, &u32::MAX.to_le_bytes());
            put(h, &v.to_le_bytes());
        }
    }
}

/// Writes the flight table's canonical serialisation to the hasher: the tag, the live count, the
/// totals, then every flight in ascending flight-reference order with every column, integers
/// little-endian at fixed width. Independent of slot assignment. Sorting allocates; this runs at
/// checkpoints only (C6).
pub fn tfms_hash_into(h: &mut Sha256, t: &TfmsTable) {
    let mut order: Vec<(u64, usize)> = (0..t.slots.capacity)
        .filter(|&s| slot_live(&t.slots, s))
        .filter_map(|s| t.flight_ref.get(s).map(|&k| (k, s)))
        .collect();
    order.sort_unstable();
    put(h, TFMS_HASH_TAG);
    put(h, &(order.len() as u64).to_le_bytes());
    let st = &t.stats;
    for v in [
        st.applied,
        st.created,
        st.out_of_order,
        st.refused_full,
        st.no_ref,
        st.conflicts,
        st.linked_plan,
        st.linked_acid,
        st.ambiguous,
        st.evicted,
    ] {
        put(h, &v.to_le_bytes());
    }
    for &(k, s) in &order {
        put_id(h, k);
        if let Some(v) = t.last_src_ms.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.last_update_ns.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.gufi.get(s) {
            put(h, v);
        }
        for col in [&t.acid, &t.dep, &t.arr, &t.arr_fix, &t.dep_fix] {
            if let Some(v) = col.get(s) {
                put(h, v);
            }
        }
        if let Some(v) = t.times.get(s) {
            for x in v {
                put(h, &x.to_le_bytes());
            }
        }
        for col in [
            &t.eta_type,
            &t.etd_type,
            &t.status,
            &t.tmi_update,
            &t.link_kind,
        ] {
            if let Some(v) = col.get(s) {
                put(h, &[*v]);
            }
        }
        if let Some(v) = t.flags.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.n_fxa.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.fca.get(s) {
            put(h, v);
        }
        if let Some(v) = t.link.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.rows.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.out_of_order.get(s) {
            put(h, &v.to_le_bytes());
        }
    }
    if tfms_stale_count(t) > 0 {
        stale_hash_into(h, &t.stale, order.iter().map(|&(_, s)| s));
    }
}

// ---- programmes -------------------------------------------------------------------------------

/// Running totals of the programme table.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowStats {
    /// Programme rows applied.
    pub applied: u64,
    /// Programmes added.
    pub created: u64,
    /// Rows refused because they were older than the held programme row.
    pub out_of_order: u64,
    /// Rows refused because the table was full and the programme is new.
    pub refused_full: u64,
    /// Programmes evicted at or after their end time (or by age when they have none).
    pub evicted: u64,
    /// Sweeps run.
    pub sweeps: u64,
    /// Applied rows by `oa_tfms::rows::prog` program type.
    pub by_program: [u64; oa_tfms::rows::prog::COUNT],
}

/// What became of one programme row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowApplied {
    /// Applied; the programme was new.
    New,
    /// Applied, replacing the held row of the same programme.
    Updated,
    /// Older than the held row; refused whole.
    OutOfOrder,
    /// New programme and the table is full; refused.
    TableFull,
    /// The row index was outside the rows (cannot happen from the pipeline).
    BadRow,
}

/// The programme table. Plain data (A1); every column has length `slots.capacity`.
// A7.2: cold struct, size guard waived (one per pipeline)
#[derive(Clone, Debug)]
pub struct FlowTable {
    /// Slot bookkeeping, keyed by the programme key.
    pub slots: Slots,
    /// Totals.
    pub stats: FlowStats,
    /// Key: message kind, program type, restriction id, number.
    pub key_kind: Vec<u8>,
    /// See `key_kind`.
    pub key_program: Vec<u8>,
    /// See `key_kind`.
    pub key_id: Vec<u64>,
    /// See `key_kind`.
    pub key_number: Vec<u64>,
    /// Key: title and element text, first 24 bytes each, zero padded.
    pub title: Vec<[u8; 24]>,
    /// See `title`.
    pub element: Vec<[u8; 24]>,
    /// `sourceTimeStamp` of the held row, Unix ms.
    pub src_ms: Vec<i64>,
    /// Start, end and event time, Unix ms; `NO_TIME` if absent.
    pub start_ms: Vec<i64>,
    /// See `start_ms`.
    pub end_ms: Vec<i64>,
    /// See `start_ms`.
    pub event_ms: Vec<i64>,
    /// Replay time of the last applied row, ns.
    pub last_update_ns: Vec<i64>,
    /// Whether the row was last updated before a confirmed step of the replay clock and has not
    /// been updated since: its age is the time since the step, not since it was heard.
    pub stale: Vec<bool>,
    /// `rate`, `rate_dep`, `rate_strat`; `NO_NUM` if absent.
    pub rates: Vec<[i32; 3]>,
    /// `referenceRestrictionId`.
    pub ref_id: Vec<u64>,
    /// `action`.
    pub action: Vec<u16>,
    /// `scope`, `status`, `flags` (`oa_tfms::rows::fflag`).
    pub state: Vec<[u8; 3]>,
    /// `sourceFacility`.
    pub facility: Vec<[u8; 8]>,
    /// Airports text, first 24 bytes.
    pub airports: Vec<[u8; 24]>,
    /// Rows applied to this programme.
    pub rows: Vec<u32>,
}

/// An empty programme table of `capacity` programmes (use [`FLOW_CAPACITY`]).
pub fn new_flow_table(capacity: usize) -> FlowTable {
    let n = capacity;
    FlowTable {
        slots: new_slots(n),
        stats: FlowStats::default(),
        key_kind: vec![0; n],
        key_program: vec![0; n],
        key_id: vec![0; n],
        key_number: vec![0; n],
        title: vec![[0; 24]; n],
        element: vec![[0; 24]; n],
        src_ms: vec![NO_TIME; n],
        start_ms: vec![NO_TIME; n],
        end_ms: vec![NO_TIME; n],
        event_ms: vec![NO_TIME; n],
        last_update_ns: vec![0; n],
        stale: vec![false; n],
        rates: vec![[NO_NUM; 3]; n],
        ref_id: vec![0; n],
        action: vec![0; n],
        state: vec![[0; 3]; n],
        facility: vec![[0; 8]; n],
        airports: vec![[0; 24]; n],
        rows: vec![0; n],
    }
}

type FlowKey = (u8, u8, u64, u64, [u8; 24], [u8; 24]);

fn flow_hash(k: &FlowKey) -> u64 {
    let mut b = [0u8; 66];
    b[0] = k.0;
    b[1] = k.1;
    b[2..10].copy_from_slice(&k.2.to_le_bytes());
    b[10..18].copy_from_slice(&k.3.to_le_bytes());
    b[18..42].copy_from_slice(&k.4);
    b[42..66].copy_from_slice(&k.5);
    hash_bytes(&b)
}

fn flow_key_of(t: &FlowTable, s: usize) -> Option<FlowKey> {
    Some((
        *t.key_kind.get(s)?,
        *t.key_program.get(s)?,
        *t.key_id.get(s)?,
        *t.key_number.get(s)?,
        *t.title.get(s)?,
        *t.element.get(s)?,
    ))
}

/// The slot of the programme with this key, if held.
pub fn flow_slot_of(t: &FlowTable, k: &FlowKey) -> Option<usize> {
    slots_find(&t.slots, flow_hash(k), |s| {
        flow_key_of(t, s).as_ref() == Some(k)
    })
}

/// Number of programmes held.
pub fn flow_live(t: &FlowTable) -> usize {
    t.slots.live
}

fn blank_flow(t: &mut FlowTable, s: usize) {
    set(&mut t.key_kind, s, 0);
    set(&mut t.key_program, s, 0);
    set(&mut t.key_id, s, 0);
    set(&mut t.key_number, s, 0);
    set(&mut t.title, s, [0; 24]);
    set(&mut t.element, s, [0; 24]);
    set(&mut t.src_ms, s, NO_TIME);
    set(&mut t.start_ms, s, NO_TIME);
    set(&mut t.end_ms, s, NO_TIME);
    set(&mut t.event_ms, s, NO_TIME);
    set(&mut t.last_update_ns, s, 0);
    set(&mut t.stale, s, false);
    set(&mut t.rates, s, [NO_NUM; 3]);
    set(&mut t.ref_id, s, 0);
    set(&mut t.action, s, 0);
    set(&mut t.state, s, [0; 3]);
    set(&mut t.facility, s, [0; 8]);
    set(&mut t.airports, s, [0; 24]);
    set(&mut t.rows, s, 0);
}

/// Applies flow row `i` of `rows` at replay time `now_ns`.
pub fn apply_tfms_flow(t: &mut FlowTable, rows: &Rows, i: usize, now_ns: i64) -> FlowApplied {
    let Some(r) = flow_at(&rows.flows, i) else {
        return FlowApplied::BadRow;
    };
    let key: FlowKey = (
        r.kind,
        r.program,
        r.id,
        r.number,
        pad(text(rows, r.title).as_bytes()),
        pad(text(rows, r.element).as_bytes()),
    );
    let (s, is_new) = match flow_slot_of(t, &key) {
        Some(s) => (s, false),
        None => {
            let Some(s) = slots_alloc(&mut t.slots, flow_hash(&key)) else {
                t.stats.refused_full += 1;
                return FlowApplied::TableFull;
            };
            blank_flow(t, s);
            set(&mut t.key_kind, s, key.0);
            set(&mut t.key_program, s, key.1);
            set(&mut t.key_id, s, key.2);
            set(&mut t.key_number, s, key.3);
            set(&mut t.title, s, key.4);
            set(&mut t.element, s, key.5);
            t.stats.created += 1;
            (s, true)
        }
    };
    if !is_new && t.src_ms.get(s).is_some_and(|&held| r.src_ms < held) {
        t.stats.out_of_order += 1;
        return FlowApplied::OutOfOrder;
    }
    set(&mut t.src_ms, s, r.src_ms);
    set(&mut t.last_update_ns, s, now_ns);
    set(&mut t.stale, s, false);
    for (col, v) in [
        (&mut t.start_ms, r.start_ms),
        (&mut t.end_ms, r.end_ms),
        (&mut t.event_ms, r.event_ms),
    ] {
        if v != NO_TIME {
            set(col, s, v);
        }
    }
    if let Some(rates) = t.rates.get_mut(s) {
        for (h, v) in rates.iter_mut().zip([r.rate, r.rate_dep, r.rate_strat]) {
            if v != NO_NUM {
                *h = v;
            }
        }
    }
    if r.ref_id != 0 {
        set(&mut t.ref_id, s, r.ref_id);
    }
    if r.action != 0 {
        set(&mut t.action, s, r.action);
    }
    set(&mut t.state, s, [r.scope, r.status, r.flags]);
    if r.facility != [0; 8] {
        set(&mut t.facility, s, r.facility);
    }
    let airports = text(rows, r.airports);
    if !airports.is_empty() {
        set(&mut t.airports, s, pad(airports.as_bytes()));
    }
    if let Some(c) = t.rows.get_mut(s) {
        *c = c.saturating_add(1);
    }
    t.stats.applied += 1;
    if let Some(c) = t.stats.by_program.get_mut(r.program as usize) {
        *c += 1;
    }
    if is_new {
        FlowApplied::New
    } else {
        FlowApplied::Updated
    }
}

/// Evicts every programme whose end time is before `now_ns` (or, with no end time, that has been
/// silent for [`TFMS_FLOW_MAX_AGE_NS`]); returns the number evicted.
pub fn flow_sweep(t: &mut FlowTable, now_ns: i64) -> usize {
    let now_ms = now_ns.div_euclid(1_000_000);
    let mut n = 0;
    for i in 0..t.slots.capacity {
        if !slot_live(&t.slots, i) {
            continue;
        }
        let end = t.end_ms.get(i).copied().unwrap_or(NO_TIME);
        let last = t.last_update_ns.get(i).copied().unwrap_or(0);
        let gone = if end != NO_TIME {
            now_ms > end
        } else {
            now_ns.saturating_sub(last) > TFMS_FLOW_MAX_AGE_NS
        };
        if gone {
            slots_release(&mut t.slots, i);
            blank_flow(t, i);
            t.stats.evicted += 1;
            n += 1;
        }
    }
    t.stats.sweeps += 1;
    n
}

/// Writes the programme table's canonical serialisation to the hasher, in key order.
pub fn flow_hash_into(h: &mut Sha256, t: &FlowTable) {
    let mut order: Vec<(FlowKey, usize)> = (0..t.slots.capacity)
        .filter(|&s| slot_live(&t.slots, s))
        .filter_map(|s| flow_key_of(t, s).map(|k| (k, s)))
        .collect();
    order.sort_unstable();
    put(h, FLOW_HASH_TAG);
    put(h, &(order.len() as u64).to_le_bytes());
    let st = &t.stats;
    for v in [
        st.applied,
        st.created,
        st.out_of_order,
        st.refused_full,
        st.evicted,
    ] {
        put(h, &v.to_le_bytes());
    }
    for v in st.by_program {
        put(h, &v.to_le_bytes());
    }
    for &(k, s) in &order {
        put(h, &[k.0, k.1]);
        put_id(h, k.2);
        put_id(h, k.3);
        put(h, &k.4);
        put(h, &k.5);
        for col in [
            &t.src_ms,
            &t.start_ms,
            &t.end_ms,
            &t.event_ms,
            &t.last_update_ns,
        ] {
            if let Some(v) = col.get(s) {
                put(h, &v.to_le_bytes());
            }
        }
        if let Some(v) = t.rates.get(s) {
            for x in v {
                put(h, &x.to_le_bytes());
            }
        }
        if let Some(&v) = t.ref_id.get(s) {
            put_id(h, v);
        }
        if let Some(v) = t.action.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.state.get(s) {
            put(h, v);
        }
        if let Some(v) = t.facility.get(s) {
            put(h, v);
        }
        if let Some(v) = t.airports.get(s) {
            put(h, v);
        }
        if let Some(v) = t.rows.get(s) {
            put(h, &v.to_le_bytes());
        }
    }
    if flow_stale_count(t) > 0 {
        stale_hash_into(h, &t.stale, order.iter().map(|&(_, s)| s));
    }
}

//! The flight table: one record per live flight, as a struct-of-arrays of fixed capacity. Core
//! (pure, B1): no I/O, no clock (time is an argument, B4), and no allocation after
//! [`new_table`] (C1).
//!
//! * Keyed by GUFI (a UUID as `u128`) through [`Index`], a fixed-size open-addressing hash
//!   index (see `index.rs` for the stated load bound and why it is not adversarial-robust).
//!   Two more indexes of the same kind find a flight from the other feed's keys: the SFDPS
//!   flight-plan identifier (which TBFM calls `gufi`, see `apply.rs`) and the TBFM `(centre,
//!   tmaId)` pair.
//! * Capacity is fixed at [`new_table`] (default [`DEFAULT_CAPACITY`], 32,768). A flight that
//!   does not fit is refused and counted, never made room for by an early eviction (INTERFACES
//!   IF-R1, C2).
//! * Eviction is by age and state: a flight whose last update is older than its state allows
//!   goes at a sweep, which runs at a fixed cadence of replay time and visits slots in slot
//!   order ([`advance_time`]). The state is the class of the latest SFDPS flight status
//!   ([`CLASS_ACTIVE`], [`CLASS_PROPOSED`], [`CLASS_FINISHED`], or none yet, kept in `flags`),
//!   and the age allowed is [`TableConfig::active_age_ns`] (2 hours: the measured active flight
//!   is heard every 8 s, but a flight over the ocean or in a quiet sector reports far less
//!   often), [`TableConfig::proposed_age_ns`] (4 hours: a filed flight is amended rarely and
//!   departs hours later), [`TableConfig::terminal_age_ns`] (10 minutes: a completed, cancelled
//!   or dropped flight has nothing more to say) and, for a flight with no status seen (first
//!   seen by TBFM, say), [`TableConfig::max_age_ns`] (30 minutes).
//! * A3: every column is a `Vec` allocated once at `capacity` and indexed by [`Slot`]. A4/A5: a
//!   [`Slot`] never leaves this crate's table modules.

use oa_tbfm::NO_TIME;

use crate::index::{
    hash_bytes, hash_u128, index_find, index_insert, index_remove, new_index, Index,
};

/// Default number of flights the table can hold. The NAS carries up to about 5,400 flights in
/// the air at the busiest moment and SFDPS also publishes proposed flights for the next hours;
/// 32,768 is about six times the airborne peak, and costs under 10 MB of columns.
pub const DEFAULT_CAPACITY: usize = 32_768;
/// Default age, in replay time, after which a silent flight whose state is not known is evicted:
/// 30 minutes.
pub const DEFAULT_MAX_AGE_NS: i64 = 30 * 60 * NS_PER_S;
/// Default silence allowed an active flight: 2 hours.
pub const DEFAULT_ACTIVE_AGE_NS: i64 = 2 * 3600 * NS_PER_S;
/// Default silence allowed a proposed flight: 4 hours.
pub const DEFAULT_PROPOSED_AGE_NS: i64 = 4 * 3600 * NS_PER_S;
/// Default silence allowed a completed, cancelled or dropped flight: 10 minutes.
pub const DEFAULT_TERMINAL_AGE_NS: i64 = 10 * 60 * NS_PER_S;
/// Default spacing of eviction sweeps in replay time: one minute.
pub const DEFAULT_SWEEP_NS: i64 = 60 * NS_PER_S;
/// Nanoseconds per second.
pub const NS_PER_S: i64 = 1_000_000_000;
/// Largest capacity accepted: the slot numbers and index cells are `u32`.
pub const MAX_CAPACITY: usize = 1 << 24;

/// `next_sweep_ns` before the first frame has scheduled a sweep.
pub const SWEEP_UNSET: i64 = i64::MIN;

/// `flags` bit: the row has a position (`lat_e7`, `lon_e7`).
pub const HAS_POS: u8 = 1;
/// `flags` bit: the row has a reported altitude.
pub const HAS_ALT: u8 = 2;
/// `flags` bit: the row has a ground speed.
pub const HAS_GS: u8 = 4;
/// `flags` bit: the row has a track velocity.
pub const HAS_VEL: u8 = 8;
/// `flags` bit: TBFM says the schedule (STA) is frozen.
pub const FROZEN: u8 = 16;
/// `flags` bit: the flight was first seen in TBFM, so its key is a tagged flight-plan id, not a
/// GUFI. SFDPS replaces the key with the real GUFI when it sees the same flight plan.
pub const TBFM_ORIGIN: u8 = 32;
/// `flags` bits 6 and 7: the state class of the latest SFDPS flight status. 0 is none seen.
pub const CLASS_MASK: u8 = 192;
/// State class: the flight is active.
pub const CLASS_ACTIVE: u8 = 64;
/// State class: the flight is proposed.
pub const CLASS_PROPOSED: u8 = 128;
/// State class: the flight is completed, cancelled or dropped.
pub const CLASS_FINISHED: u8 = 192;

/// The state class of an `oa_fixm` `STATUS_*` code (0 for a code that is none of them).
pub fn class_of_status(status: u8) -> u8 {
    match status {
        oa_fixm::STATUS_ACTIVE => CLASS_ACTIVE,
        oa_fixm::STATUS_PROPOSED => CLASS_PROPOSED,
        oa_fixm::STATUS_COMPLETED | oa_fixm::STATUS_CANCELLED | oa_fixm::STATUS_DROPPED => {
            CLASS_FINISHED
        }
        _ => 0,
    }
}

/// `last_feed` values.
pub const FEED_NONE: u8 = 0;
/// The last update came from SFDPS; `last_source` is its two-letter source code.
pub const FEED_SFDPS: u8 = 1;
/// The last update came from TBFM; `last_source` is zero.
pub const FEED_TBFM: u8 = 2;

/// Settings of a table (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableConfig {
    /// Flights the table can hold.
    pub capacity: usize,
    /// A flight with no known state is evicted when `now - last_update` exceeds this, in
    /// nanoseconds.
    pub max_age_ns: i64,
    /// The same for an active flight.
    pub active_age_ns: i64,
    /// The same for a proposed flight.
    pub proposed_age_ns: i64,
    /// The same for a completed, cancelled or dropped flight.
    pub terminal_age_ns: i64,
    /// Replay-time spacing of eviction sweeps, in nanoseconds.
    pub sweep_ns: i64,
}

impl Default for TableConfig {
    fn default() -> TableConfig {
        TableConfig {
            capacity: DEFAULT_CAPACITY,
            max_age_ns: DEFAULT_MAX_AGE_NS,
            active_age_ns: DEFAULT_ACTIVE_AGE_NS,
            proposed_age_ns: DEFAULT_PROPOSED_AGE_NS,
            terminal_age_ns: DEFAULT_TERMINAL_AGE_NS,
            sweep_ns: DEFAULT_SWEEP_NS,
        }
    }
}

/// A bad [`TableConfig`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Capacity is 0 or above [`MAX_CAPACITY`].
    Capacity(usize),
    /// `max_age_ns` is not positive.
    MaxAge(i64),
    /// `active_age_ns`, `proposed_age_ns` or `terminal_age_ns` is not positive.
    StateAge(i64),
    /// `sweep_ns` is not positive.
    Sweep(i64),
}

/// A slot number in one flight table (A4). Never leaves the table modules (A5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot(pub(crate) u32);

/// Running totals of the table itself, kept across evictions.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableStats {
    /// SFDPS rows applied.
    pub sfdps_applied: u64,
    /// TBFM rows applied.
    pub tbfm_applied: u64,
    /// Rows refused because the flight's position (SFDPS) or message (TBFM) time was older than
    /// what the flight already holds.
    pub out_of_order: u64,
    /// Identity fields that disagreed with a value already held (the held value is kept).
    pub conflicts: u64,
    /// Rows refused because the table was full and the flight is new.
    pub refused_full: u64,
    /// Flights evicted by age.
    pub evicted: u64,
    /// Eviction sweeps run.
    pub sweeps: u64,
    /// TBFM rows linked to an existing flight by its flight-plan identifier (TBFM `gufi`).
    pub tbfm_linked_gufi: u64,
    /// TBFM rows with no `gufi`, linked by `(centre, tmaId)`.
    pub tbfm_linked_tma: u64,
    /// TBFM rows whose `gufi` matched no flight and which made a new flight.
    pub tbfm_created: u64,
    /// TBFM rows with no `gufi` and no matching `(centre, tmaId)`: not applied.
    pub tbfm_unlinked: u64,
    /// TBFM-first flights given their real GUFI when SFDPS reported the same flight plan.
    pub promoted: u64,
    /// A flight-plan identifier or `(centre, tmaId)` taken over by a newer flight, because the
    /// FAA reused it.
    pub key_reassigned: u64,
    /// SFDPS rows older (by the message's own timestamp) than the newest already applied to the
    /// flight: status, controlling unit, computer id and plan ownership were left as they were.
    pub stale_amendments: u64,
    /// Flight-plan identifiers an older SFDPS message did not take from a flight whose newest
    /// message is newer.
    pub plan_refused_old: u64,
    /// SFDPS rows with no usable timestamp, applied as before (no ordering by source time).
    pub no_source_time: u64,
}

/// The flight table. Plain data (A1, A3); behaviour is in the free functions of this crate.
// A3: struct-of-arrays, one Vec per column, every Vec of length `capacity`. A7: the columns are
// not one struct held in quantity; the bytes per flight are stated in `BYTES_PER_FLIGHT`.
#[derive(Clone, Debug)]
pub struct FlightTable {
    /// Flights the table can hold.
    pub capacity: usize,
    /// Age limit of a flight with no known state, ns.
    pub max_age_ns: i64,
    /// Age limit of an active flight, ns.
    pub active_age_ns: i64,
    /// Age limit of a proposed flight, ns.
    pub proposed_age_ns: i64,
    /// Age limit of a completed, cancelled or dropped flight, ns.
    pub terminal_age_ns: i64,
    /// Sweep spacing, ns.
    pub sweep_ns: i64,
    /// Replay time of the next sweep, or [`SWEEP_UNSET`].
    pub next_sweep_ns: i64,
    /// Live flights.
    pub live: usize,
    /// Totals.
    pub stats: TableStats,
    /// Occupancy bitset, bit `slot`.
    pub occupied: Vec<u64>,
    /// Free slots, a stack: the next to be used is the last. Initially lowest slot on top.
    pub free: Vec<u32>,
    /// GUFI to slot.
    pub by_gufi: Index,
    /// Flight-plan identifier to slot.
    pub by_plan: Index,
    /// `(centre, tmaId)` to slot.
    pub by_tma: Index,

    /// Identity.
    pub gufi: Vec<u128>,
    /// Aircraft identification, zero padded.
    pub callsign: Vec<[u8; 8]>,
    /// Host computer id.
    pub computer_id: Vec<[u8; 4]>,
    /// Departure point.
    pub departure: Vec<[u8; 12]>,
    /// Arrival point.
    pub arrival: Vec<[u8; 12]>,
    /// Aircraft type, the ICAO designator.
    pub aircraft_type: Vec<[u8; 4]>,
    /// Flight-plan identifier (SFDPS `flightPlan@identifier`, TBFM `gufi`); all zero if none.
    pub plan_id: Vec<[u8; 12]>,

    /// Last position, 1e-7 degree.
    pub lat_e7: Vec<i32>,
    /// Longitude, 1e-7 degree.
    pub lon_e7: Vec<i32>,
    /// Reported altitude, feet.
    pub alt_ft: Vec<i32>,
    /// Ground speed, 0.1 knot.
    pub gs_dkt: Vec<u16>,
    /// Track velocity east component, 0.1 knot.
    pub vx_dkt: Vec<i16>,
    /// Track velocity north component, 0.1 knot.
    pub vy_dkt: Vec<i16>,
    /// Time of the last position, Unix ms, or [`NO_TIME`].
    pub pos_time_ms: Vec<i64>,
    /// Source time (the message's own timestamp) of the newest SFDPS message applied, Unix ms,
    /// or [`NO_TIME`].
    pub src_ms: Vec<i64>,

    /// Replay time of the last applied update, ns.
    pub last_update_ns: Vec<i64>,
    /// Whether the row was last updated before a confirmed step of the replay clock and has not
    /// been updated since: its age is the time since the step, not since it was heard.
    pub stale: Vec<bool>,
    /// SFDPS source code of the last update when `last_feed` is [`FEED_SFDPS`].
    pub last_source: Vec<[u8; 2]>,
    /// Feed of the last update.
    pub last_feed: Vec<u8>,
    /// Controlling unit (bytes 0 to 3) and sector (4 to 7).
    pub ctrl: Vec<[u8; 8]>,

    /// TBFM centre of the flight's TBFM id.
    pub tma_center: Vec<[u8; 4]>,
    /// TBFM's own flight id, zero if none.
    pub tma_id: Vec<[u8; 8]>,
    /// Meter fix.
    pub meter_fix: Vec<[u8; 8]>,
    /// Scheduled time of arrival at the meter fix, Unix ms, or [`NO_TIME`].
    pub sta_ms: Vec<i64>,
    /// Estimated time of arrival at the meter fix, Unix ms, or [`NO_TIME`].
    pub eta_ms: Vec<i64>,
    /// Landing runway.
    pub runway: Vec<[u8; 4]>,
    /// `msgTime` of the last applied TBFM row, Unix ms, or [`NO_TIME`].
    pub tbfm_msg_ms: Vec<i64>,

    /// Bit flags: [`HAS_POS`] and the others above.
    pub flags: Vec<u8>,
    /// Updates applied to this flight.
    pub updates: Vec<u32>,
    /// Updates refused as older than the flight's last.
    pub out_of_order: Vec<u32>,
    /// Identity fields that disagreed.
    pub conflicts: Vec<u32>,
}

/// Bytes of column data per flight (A7 budget, stated): identity 16 + 8 + 4 + 12 + 12 + 4 + 12,
/// position 4 + 4 + 4 + 2 + 2 + 2 + 8, update 8 + 2 + 1 + 8, TBFM 4 + 8 + 8 + 8 + 8 + 4 + 8, flags
/// and counters 1 + 4 + 4 + 4, SFDPS source time 8. The three indexes add 3 x 2 x 4 = 24 bytes per flight (two 4-byte
/// cells per flight each, at the 1/2 load sizing, before rounding up to a power of two), so a
/// default table is about 6.5 MB.
pub const BYTES_PER_FLIGHT: usize = 182;

/// Borrowed view of one slot's columns, read-only.
pub(crate) struct SlotRef<'a> {
    pub gufi: &'a u128,
    pub callsign: &'a [u8; 8],
    pub computer_id: &'a [u8; 4],
    pub departure: &'a [u8; 12],
    pub arrival: &'a [u8; 12],
    pub aircraft_type: &'a [u8; 4],
    pub plan_id: &'a [u8; 12],
    pub lat_e7: &'a i32,
    pub lon_e7: &'a i32,
    pub alt_ft: &'a i32,
    pub gs_dkt: &'a u16,
    pub vx_dkt: &'a i16,
    pub vy_dkt: &'a i16,
    pub pos_time_ms: &'a i64,
    pub src_ms: &'a i64,
    pub last_update_ns: &'a i64,
    pub last_source: &'a [u8; 2],
    pub last_feed: &'a u8,
    pub ctrl: &'a [u8; 8],
    pub tma_center: &'a [u8; 4],
    pub tma_id: &'a [u8; 8],
    pub meter_fix: &'a [u8; 8],
    pub sta_ms: &'a i64,
    pub eta_ms: &'a i64,
    pub runway: &'a [u8; 4],
    pub tbfm_msg_ms: &'a i64,
    pub flags: &'a u8,
    pub updates: &'a u32,
    pub out_of_order: &'a u32,
    pub conflicts: &'a u32,
}

/// Borrowed view of one slot's columns, writable.
pub(crate) struct SlotMut<'a> {
    pub gufi: &'a mut u128,
    pub callsign: &'a mut [u8; 8],
    pub computer_id: &'a mut [u8; 4],
    pub departure: &'a mut [u8; 12],
    pub arrival: &'a mut [u8; 12],
    pub aircraft_type: &'a mut [u8; 4],
    pub plan_id: &'a mut [u8; 12],
    pub lat_e7: &'a mut i32,
    pub lon_e7: &'a mut i32,
    pub alt_ft: &'a mut i32,
    pub gs_dkt: &'a mut u16,
    pub vx_dkt: &'a mut i16,
    pub vy_dkt: &'a mut i16,
    pub pos_time_ms: &'a mut i64,
    pub src_ms: &'a mut i64,
    pub last_update_ns: &'a mut i64,
    pub stale: &'a mut bool,
    pub last_source: &'a mut [u8; 2],
    pub last_feed: &'a mut u8,
    pub ctrl: &'a mut [u8; 8],
    pub tma_center: &'a mut [u8; 4],
    pub tma_id: &'a mut [u8; 8],
    pub meter_fix: &'a mut [u8; 8],
    pub sta_ms: &'a mut i64,
    pub eta_ms: &'a mut i64,
    pub runway: &'a mut [u8; 4],
    pub tbfm_msg_ms: &'a mut i64,
    pub flags: &'a mut u8,
    pub updates: &'a mut u32,
    pub out_of_order: &'a mut u32,
    pub conflicts: &'a mut u32,
}

/// Builds an empty table. All memory is allocated here.
pub fn new_table(cfg: TableConfig) -> Result<FlightTable, ConfigError> {
    if cfg.capacity == 0 || cfg.capacity > MAX_CAPACITY {
        return Err(ConfigError::Capacity(cfg.capacity));
    }
    if cfg.max_age_ns <= 0 {
        return Err(ConfigError::MaxAge(cfg.max_age_ns));
    }
    for a in [cfg.active_age_ns, cfg.proposed_age_ns, cfg.terminal_age_ns] {
        if a <= 0 {
            return Err(ConfigError::StateAge(a));
        }
    }
    if cfg.sweep_ns <= 0 {
        return Err(ConfigError::Sweep(cfg.sweep_ns));
    }
    let n = cfg.capacity;
    Ok(FlightTable {
        capacity: n,
        max_age_ns: cfg.max_age_ns,
        active_age_ns: cfg.active_age_ns,
        proposed_age_ns: cfg.proposed_age_ns,
        terminal_age_ns: cfg.terminal_age_ns,
        sweep_ns: cfg.sweep_ns,
        next_sweep_ns: SWEEP_UNSET,
        live: 0,
        stats: TableStats::default(),
        occupied: vec![0; n.div_ceil(64)],
        free: (0..n as u32).rev().collect(),
        by_gufi: new_index(n),
        by_plan: new_index(n),
        by_tma: new_index(n),
        gufi: vec![0; n],
        callsign: vec![[0; 8]; n],
        computer_id: vec![[0; 4]; n],
        departure: vec![[0; 12]; n],
        arrival: vec![[0; 12]; n],
        aircraft_type: vec![[0; 4]; n],
        plan_id: vec![[0; 12]; n],
        lat_e7: vec![0; n],
        lon_e7: vec![0; n],
        alt_ft: vec![0; n],
        gs_dkt: vec![0; n],
        vx_dkt: vec![0; n],
        vy_dkt: vec![0; n],
        pos_time_ms: vec![NO_TIME; n],
        src_ms: vec![NO_TIME; n],
        last_update_ns: vec![0; n],
        stale: vec![false; n],
        last_source: vec![[0; 2]; n],
        last_feed: vec![FEED_NONE; n],
        ctrl: vec![[0; 8]; n],
        tma_center: vec![[0; 4]; n],
        tma_id: vec![[0; 8]; n],
        meter_fix: vec![[0; 8]; n],
        sta_ms: vec![NO_TIME; n],
        eta_ms: vec![NO_TIME; n],
        runway: vec![[0; 4]; n],
        tbfm_msg_ms: vec![NO_TIME; n],
        flags: vec![0; n],
        updates: vec![0; n],
        out_of_order: vec![0; n],
        conflicts: vec![0; n],
    })
}

/// Whether `slot` is occupied.
pub(crate) fn is_occupied(t: &FlightTable, s: Slot) -> bool {
    t.occupied
        .get((s.0 / 64) as usize)
        .is_some_and(|w| (w >> (s.0 % 64)) & 1 == 1)
}

fn set_occupied(t: &mut FlightTable, s: Slot, on: bool) {
    if let Some(w) = t.occupied.get_mut((s.0 / 64) as usize) {
        let bit = 1u64 << (s.0 % 64);
        if on {
            *w |= bit;
        } else {
            *w &= !bit;
        }
    }
}

/// Read-only view of slot `s`; `None` if it is out of range.
pub(crate) fn slot_ref(t: &FlightTable, s: Slot) -> Option<SlotRef<'_>> {
    let i = s.0 as usize;
    Some(SlotRef {
        gufi: t.gufi.get(i)?,
        callsign: t.callsign.get(i)?,
        computer_id: t.computer_id.get(i)?,
        departure: t.departure.get(i)?,
        arrival: t.arrival.get(i)?,
        aircraft_type: t.aircraft_type.get(i)?,
        plan_id: t.plan_id.get(i)?,
        lat_e7: t.lat_e7.get(i)?,
        lon_e7: t.lon_e7.get(i)?,
        alt_ft: t.alt_ft.get(i)?,
        gs_dkt: t.gs_dkt.get(i)?,
        vx_dkt: t.vx_dkt.get(i)?,
        vy_dkt: t.vy_dkt.get(i)?,
        pos_time_ms: t.pos_time_ms.get(i)?,
        src_ms: t.src_ms.get(i)?,
        last_update_ns: t.last_update_ns.get(i)?,
        last_source: t.last_source.get(i)?,
        last_feed: t.last_feed.get(i)?,
        ctrl: t.ctrl.get(i)?,
        tma_center: t.tma_center.get(i)?,
        tma_id: t.tma_id.get(i)?,
        meter_fix: t.meter_fix.get(i)?,
        sta_ms: t.sta_ms.get(i)?,
        eta_ms: t.eta_ms.get(i)?,
        runway: t.runway.get(i)?,
        tbfm_msg_ms: t.tbfm_msg_ms.get(i)?,
        flags: t.flags.get(i)?,
        updates: t.updates.get(i)?,
        out_of_order: t.out_of_order.get(i)?,
        conflicts: t.conflicts.get(i)?,
    })
}

/// Writable view of slot `s`; `None` if it is out of range.
pub(crate) fn slot_mut(t: &mut FlightTable, s: Slot) -> Option<SlotMut<'_>> {
    let i = s.0 as usize;
    Some(SlotMut {
        gufi: t.gufi.get_mut(i)?,
        callsign: t.callsign.get_mut(i)?,
        computer_id: t.computer_id.get_mut(i)?,
        departure: t.departure.get_mut(i)?,
        arrival: t.arrival.get_mut(i)?,
        aircraft_type: t.aircraft_type.get_mut(i)?,
        plan_id: t.plan_id.get_mut(i)?,
        lat_e7: t.lat_e7.get_mut(i)?,
        lon_e7: t.lon_e7.get_mut(i)?,
        alt_ft: t.alt_ft.get_mut(i)?,
        gs_dkt: t.gs_dkt.get_mut(i)?,
        vx_dkt: t.vx_dkt.get_mut(i)?,
        vy_dkt: t.vy_dkt.get_mut(i)?,
        pos_time_ms: t.pos_time_ms.get_mut(i)?,
        src_ms: t.src_ms.get_mut(i)?,
        last_update_ns: t.last_update_ns.get_mut(i)?,
        stale: t.stale.get_mut(i)?,
        last_source: t.last_source.get_mut(i)?,
        last_feed: t.last_feed.get_mut(i)?,
        ctrl: t.ctrl.get_mut(i)?,
        tma_center: t.tma_center.get_mut(i)?,
        tma_id: t.tma_id.get_mut(i)?,
        meter_fix: t.meter_fix.get_mut(i)?,
        sta_ms: t.sta_ms.get_mut(i)?,
        eta_ms: t.eta_ms.get_mut(i)?,
        runway: t.runway.get_mut(i)?,
        tbfm_msg_ms: t.tbfm_msg_ms.get_mut(i)?,
        flags: t.flags.get_mut(i)?,
        updates: t.updates.get_mut(i)?,
        out_of_order: t.out_of_order.get_mut(i)?,
        conflicts: t.conflicts.get_mut(i)?,
    })
}

/// Sets every column of the slot to its empty value.
fn blank(c: &mut SlotMut<'_>) {
    *c.gufi = 0;
    *c.callsign = [0; 8];
    *c.computer_id = [0; 4];
    *c.departure = [0; 12];
    *c.arrival = [0; 12];
    *c.aircraft_type = [0; 4];
    *c.plan_id = [0; 12];
    *c.lat_e7 = 0;
    *c.lon_e7 = 0;
    *c.alt_ft = 0;
    *c.gs_dkt = 0;
    *c.vx_dkt = 0;
    *c.vy_dkt = 0;
    *c.pos_time_ms = NO_TIME;
    *c.src_ms = NO_TIME;
    *c.last_update_ns = 0;
    *c.stale = false;
    *c.last_source = [0; 2];
    *c.last_feed = FEED_NONE;
    *c.ctrl = [0; 8];
    *c.tma_center = [0; 4];
    *c.tma_id = [0; 8];
    *c.meter_fix = [0; 8];
    *c.sta_ms = NO_TIME;
    *c.eta_ms = NO_TIME;
    *c.runway = [0; 4];
    *c.tbfm_msg_ms = NO_TIME;
    *c.flags = 0;
    *c.updates = 0;
    *c.out_of_order = 0;
    *c.conflicts = 0;
}

/// A key of the second index: nonzero `tma_id` bytes. The centre is part of the key.
pub(crate) fn tma_hash(center: &[u8; 4], id: &[u8; 8]) -> u64 {
    let mut k = [0u8; 12];
    let (a, b) = k.split_at_mut(4);
    a.copy_from_slice(center);
    b.copy_from_slice(id);
    hash_bytes(&k)
}

/// The flight with GUFI `g`.
pub(crate) fn find_gufi(t: &FlightTable, g: u128) -> Option<Slot> {
    index_find(&t.by_gufi, hash_u128(g), |s| {
        t.gufi.get(s as usize) == Some(&g)
    })
    .map(Slot)
}

/// The flight whose flight-plan identifier is `p` (not all zero).
pub(crate) fn find_plan(t: &FlightTable, p: &[u8; 12]) -> Option<Slot> {
    index_find(&t.by_plan, hash_bytes(p), |s| {
        t.plan_id.get(s as usize) == Some(p)
    })
    .map(Slot)
}

/// The flight whose TBFM id is `(center, id)`.
pub(crate) fn find_tma(t: &FlightTable, center: &[u8; 4], id: &[u8; 8]) -> Option<Slot> {
    index_find(&t.by_tma, tma_hash(center, id), |s| {
        let i = s as usize;
        t.tma_id.get(i) == Some(id) && t.tma_center.get(i) == Some(center)
    })
    .map(Slot)
}

/// Takes a free slot for the new flight `g`, blank, with its GUFI indexed. `None` if the table
/// is full: the caller counts the refusal.
pub(crate) fn create_flight(t: &mut FlightTable, g: u128) -> Option<Slot> {
    let s = Slot(t.free.pop()?);
    if !index_insert(&mut t.by_gufi, hash_u128(g), s.0) {
        t.free.push(s.0);
        return None;
    }
    set_occupied(t, s, true);
    t.live += 1;
    if let Some(mut c) = slot_mut(t, s) {
        blank(&mut c);
        *c.gufi = g;
    }
    Some(s)
}

/// Gives the flight in `s` a new GUFI (a TBFM-first flight meeting its SFDPS identity).
pub(crate) fn rekey_flight(t: &mut FlightTable, s: Slot, new: u128) -> bool {
    let Some(&old) = t.gufi.get(s.0 as usize) else {
        return false;
    };
    let gufi = &t.gufi;
    let removed = index_remove(&mut t.by_gufi, hash_u128(old), s.0, |o| {
        hash_u128(gufi.get(o as usize).copied().unwrap_or(0))
    });
    if !removed || !index_insert(&mut t.by_gufi, hash_u128(new), s.0) {
        return false;
    }
    if let Some(g) = t.gufi.get_mut(s.0 as usize) {
        *g = new;
    }
    true
}

/// Sets the flight-plan identifier of `s` and indexes it. If another flight already holds the
/// identifier, the newer flight takes it over (the FAA reuses identifiers) and the other loses
/// it; this is counted in `stats.key_reassigned`.
///
/// `src_ms` is the source time of the SFDPS message making the claim, or [`NO_TIME`] for a claim
/// with none (TBFM, or an SFDPS row with no timestamp), which always takes the identifier. A
/// claim with a source time takes it from another flight only if that flight's newest SFDPS
/// message is older (a flight with none held counts as older); otherwise the claim is refused and
/// counted in `stats.plan_refused_old`, and `false` is returned.
pub(crate) fn claim_plan(t: &mut FlightTable, s: Slot, p: &[u8; 12], src_ms: i64) -> bool {
    if t.plan_id.get(s.0 as usize) == Some(p) {
        return true;
    }
    if let Some(other) = find_plan(t, p) {
        let owner_src = t.src_ms.get(other.0 as usize).copied().unwrap_or(NO_TIME);
        if src_ms != NO_TIME && owner_src != NO_TIME && owner_src >= src_ms {
            t.stats.plan_refused_old += 1;
            return false;
        }
        drop_plan(t, other);
        t.stats.key_reassigned += 1;
    }
    drop_plan(t, s);
    if let Some(d) = t.plan_id.get_mut(s.0 as usize) {
        *d = *p;
    }
    index_insert(&mut t.by_plan, hash_bytes(p), s.0);
    true
}

/// Removes the flight-plan identifier of `s` from the index and the column.
fn drop_plan(t: &mut FlightTable, s: Slot) {
    let Some(&p) = t.plan_id.get(s.0 as usize) else {
        return;
    };
    if p == [0; 12] {
        return;
    }
    let col = &t.plan_id;
    index_remove(&mut t.by_plan, hash_bytes(&p), s.0, |o| {
        hash_bytes(col.get(o as usize).unwrap_or(&[0; 12]))
    });
    if let Some(d) = t.plan_id.get_mut(s.0 as usize) {
        *d = [0; 12];
    }
}

/// Sets the TBFM id of `s` and indexes it, with the same newest-takes-over rule as
/// [`claim_plan`].
pub(crate) fn claim_tma(t: &mut FlightTable, s: Slot, center: &[u8; 4], id: &[u8; 8]) {
    let i = s.0 as usize;
    if t.tma_id.get(i) == Some(id) && t.tma_center.get(i) == Some(center) {
        return;
    }
    if let Some(other) = find_tma(t, center, id) {
        drop_tma(t, other);
        t.stats.key_reassigned += 1;
    }
    drop_tma(t, s);
    if let Some(d) = t.tma_id.get_mut(i) {
        *d = *id;
    }
    if let Some(d) = t.tma_center.get_mut(i) {
        *d = *center;
    }
    index_insert(&mut t.by_tma, tma_hash(center, id), s.0);
}

fn drop_tma(t: &mut FlightTable, s: Slot) {
    let i = s.0 as usize;
    let (Some(&id), Some(&center)) = (t.tma_id.get(i), t.tma_center.get(i)) else {
        return;
    };
    if id == [0; 8] {
        return;
    }
    let (ids, centers) = (&t.tma_id, &t.tma_center);
    index_remove(&mut t.by_tma, tma_hash(&center, &id), s.0, |o| {
        let k = o as usize;
        tma_hash(
            centers.get(k).unwrap_or(&[0; 4]),
            ids.get(k).unwrap_or(&[0; 8]),
        )
    });
    if let Some(d) = t.tma_id.get_mut(i) {
        *d = [0; 8];
    }
    if let Some(d) = t.tma_center.get_mut(i) {
        *d = [0; 4];
    }
}

/// Removes the flight in `s`: out of every index, columns blank, slot back on the free stack.
fn evict_slot(t: &mut FlightTable, s: Slot) {
    let Some(&g) = t.gufi.get(s.0 as usize) else {
        return;
    };
    drop_plan(t, s);
    drop_tma(t, s);
    let gufi = &t.gufi;
    index_remove(&mut t.by_gufi, hash_u128(g), s.0, |o| {
        hash_u128(gufi.get(o as usize).copied().unwrap_or(0))
    });
    if let Some(mut c) = slot_mut(t, s) {
        blank(&mut c);
    }
    set_occupied(t, s, false);
    t.free.push(s.0);
    t.live = t.live.saturating_sub(1);
    t.stats.evicted += 1;
}

/// How long a flight with `flags` may stay silent, ns.
fn age_limit(t: &FlightTable, flags: u8) -> i64 {
    match flags & CLASS_MASK {
        CLASS_ACTIVE => t.active_age_ns,
        CLASS_PROPOSED => t.proposed_age_ns,
        CLASS_FINISHED => t.terminal_age_ns,
        _ => t.max_age_ns,
    }
}

/// Moves every flight's last-update time by `delta_ns`, for a step of the replay clock: the time
/// since each flight was last heard stays what it was. Every flight is marked stale (see
/// [`FlightTable::stale`]) until it is updated; returns the number marked.
pub fn shift_times(t: &mut FlightTable, delta_ns: i64) -> usize {
    let mut n = 0;
    for i in 0..t.capacity {
        if is_occupied(t, Slot(i as u32)) {
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

/// Whether the flight in slot `slot` (as `slot_of_gufi` gives it) was last updated before a confirmed step of the replay clock and has
/// not been updated since. False for a slot that is out of range or free.
pub fn is_stale(t: &FlightTable, slot: usize) -> bool {
    u32::try_from(slot).is_ok_and(|i| is_occupied(t, Slot(i)))
        && t.stale.get(slot).copied().unwrap_or(false)
}

/// The number of flights that are stale ([`is_stale`]).
pub fn stale_count(t: &FlightTable) -> usize {
    (0..t.capacity).filter(|&i| is_stale(t, i)).count()
}

/// Evicts every flight whose last update is older than its state allows (see the module
/// documentation) before `now_ns`, visiting slots in slot order. Returns the number evicted.
pub fn sweep(t: &mut FlightTable, now_ns: i64) -> usize {
    let mut n = 0;
    for i in 0..t.capacity as u32 {
        let s = Slot(i);
        if !is_occupied(t, s) {
            continue;
        }
        let last = t.last_update_ns.get(i as usize).copied().unwrap_or(0);
        let flags = t.flags.get(i as usize).copied().unwrap_or(0);
        if now_ns.saturating_sub(last) > age_limit(t, flags) {
            evict_slot(t, s);
            n += 1;
        }
    }
    t.stats.sweeps += 1;
    n
}

/// The first multiple of `step` after `now`.
fn next_boundary(now: i64, step: i64) -> i64 {
    now.div_euclid(step).saturating_add(1).saturating_mul(step)
}

/// Moves the table's clock to replay time `now_ns`: runs a sweep if the cadence says one is due.
/// The first call only schedules the first sweep. Sweeps fall on multiples of `sweep_ns`, so
/// when they run does not depend on when the replay started. Returns the number evicted.
pub fn advance_time(t: &mut FlightTable, now_ns: i64) -> usize {
    if t.next_sweep_ns == SWEEP_UNSET {
        t.next_sweep_ns = next_boundary(now_ns, t.sweep_ns);
        return 0;
    }
    if now_ns < t.next_sweep_ns {
        return 0;
    }
    let n = sweep(t, now_ns);
    t.next_sweep_ns = next_boundary(now_ns, t.sweep_ns);
    n
}

/// Number of live flights.
pub fn flights_live(t: &FlightTable) -> usize {
    t.live
}

/// The slot of the flight with GUFI `g`, as the column index, for readers of the columns.
pub fn slot_of_gufi(t: &FlightTable, g: u128) -> Option<usize> {
    find_gufi(t, g).map(|s| s.0 as usize)
}

//! The track table and everything around the filters: keys, initiation, coasting, dropping,
//! time ordering, re-filtering of late measurements, and the fixed-rate output. Core (pure):
//! time is an input (the time of applicability of each measurement, and [`tracker_advance`]),
//! there is no clock, no randomness and no hash map (the key index is an open-addressing table
//! with a fixed hash), so the same measurements give the same output byte for byte.
//!
//! # Memory
//!
//! Every table is allocated once in [`tracker_new`] for `max_tracks` tracks: the per-track
//! columns, the filter of each track, a ring of the last [`RING_DEPTH`] applied measurements of
//! each track, one anchor filter state per track and the key index. After that nothing
//! allocates: output rows go to a [`TrackOutput`] and track records to a [`TrackLog`], both
//! preallocated and bounded, overflow counted.
//!
//! The anchor is the filter state after the oldest measurement in the ring. The state after any
//! newer ring entry is the anchor with the ring's measurements applied again in order, which
//! gives exactly the state the filter had then (a measurement in the ring was applied, and
//! applying is a pure function of the state and the measurement). So the ring holds
//! `RING_DEPTH` measurements and one filter state instead of `RING_DEPTH` of each: the memory of
//! a track falls from about 8.9 kB to about 2.2 kB. Keeping the anchor costs one more filter
//! application per update once the ring is full (the oldest entry's successor is applied to it).
//!
//! # Time ordering
//!
//! The tracker's clock is the newest time of applicability seen, decided by `crate::time_gate`.
//! A measurement more than the step limit (the out-of-order window) ahead of it is held until a
//! run of them from enough distinct sensors and addresses confirms a step forward. A measurement
//! older than the clock by more than the out-of-order window is counted and dropped; a long run
//! of those, from enough distinct sensors and addresses, means the clock was wrong-ahead and
//! steps it back: the tracks updated after the new clock are dropped and counted, and the rows
//! that follow carry earlier times than rows already emitted. A measurement older than
//! its own track's newest update but inside the window is applied by re-filtering: the ring
//! gives the newest stored update at or before its time, the state after it is rebuilt from the
//! anchor, the measurement is applied to that, and the stored measurements that followed are
//! applied again in order. One older than every stored update of its track is counted and
//! dropped.
//!
//! # Initiation, coasting, dropping
//!
//! A track is keyed by its 24-bit address. A new address starts a tentative track (filtered but
//! not output). It is confirmed when `init_m` reports have arrived within `init_n` expected
//! intervals of the first; if the second report implies a ground speed above the plausible
//! maximum, or the window passes without confirmation, the track restarts from the new report
//! (counted). A confirmed track is output at every update and at every tick; it is flagged
//! coasting when its newest update is older than `coast_after_intervals` expected intervals (its
//! own, if the caller set one, else the configured one) and
//! dropped (logged, slot freed) when older than `drop_after_ns`.
//!
//! # Fixed-rate output
//!
//! Ticks are whole multiples of `tick_ns` of the time axis. A tick is emitted when the clock
//! passes it (before the measurement that moved the clock is applied) or when the caller
//! advances time; the row is the filter predicted to the tick time. A late measurement changes
//! the ticks after it, not those already emitted.

use alloc::{vec, vec::Vec};

use oa_adsb_model::{
    category_observe, jump_verdict, CategoryState, JumpPair, JumpVerdict, CAP_ORDINARY_KT,
    NONE_U16, SPEED_ENVELOPE_MPS,
};

use crate::confirm::{confirm_window_ns, move_is_large, resolve_decide, Resolve, Seen, NO_VOTE_NS};
use crate::filter::{
    filter_altitude_forecast, filter_apply, filter_association_output_checked, filter_init,
    filter_miss_m, filter_output, filter_output_checked, filter_replay, filter_row_distance_m,
    Applied, Filter,
};
use crate::hfilter::{h_combine, HFilter};
use crate::params::{CHI2_2_95, FAIL_CLOSED_R95_M, NS_PER_S, PLAUSIBLE_SLACK_M, RING_DEPTH};
use crate::time_gate::{
    gate_decide, gate_new, gate_note_applied, gate_run_restarted, GateCfg, GateState, Verdict,
    RUN_BUF,
};
use crate::types::{
    check_config, log_new, output_push, Measurement, OutRow, PredictError, TrackError, TrackLog,
    TrackOutput, TrackerConfig, TrackerStats, END_DROPPED, END_FLUSHED, END_TENTATIVE, M_HAS_VEL,
    NONE_I32, NO_ADDR, NO_IDENT, NO_IDENT_NS, O_COAST, O_IDENTITY_UNCONFIRMED, O_MOVE_UNCONFIRMED,
    O_RADAR_DISAGREES, O_REACQUIRED, O_TICK, O_UNCERTAIN, RADAR_KEY_BASE, REF_ALTITUDE,
    REF_IDENTITY, REF_NO_ADDRESS, REF_POSITION, REF_POS_COV, REF_TIME, REF_VELOCITY, TIME_LIMIT_NS,
};
use crate::vfilter::{VFilter, VModel};

const FREE: u8 = 0;
const TENTATIVE: u8 = 1;
const CONFIRMED: u8 = 2;
/// Empty slot of the key index.
const EMPTY: u32 = u32::MAX;
const MPS_PER_KT: f64 = 0.514_444_444_444;

/// The tracker. A subsystem (D5): its fields are private to this module and its interface is
/// the free functions below. Cold container; the columns are the hot data (A7.2 for the
/// container, A7 on [`Filter`] and [`Measurement`]).
// A7.2: cold struct, size guard waived
pub struct Tracker {
    cfg: TrackerConfig,
    stats: TrackerStats,
    log: TrackLog,
    /// Newest time of applicability seen.
    clock_ns: i64,
    /// The clock decision: runs of far-ahead and of late measurements, the sensor table.
    gate: GateState,
    /// The far-ahead measurements of the run being confirmed, oldest first (the last `RUN_BUF`),
    /// and how many: applied when the run confirms, discarded when it is refuted.
    held: Vec<Measurement>,
    held_n: usize,
    /// The newest `last_ns` any track has had, ever (`i64::MIN` before the first): an upper bound
    /// on the newest live update, kept so a tick need not scan every slot to find it.
    newest_update_ns: i64,
    /// The next tick to emit; `i64::MIN` until the first measurement.
    next_tick_ns: i64,
    /// One past the highest slot ever used (the scan bound).
    high: usize,
    active: usize,
    // Per-track columns (struct-of-arrays), indexed by slot.
    key: Vec<u32>,
    /// Last valid source-observed Mode 3/A code by stable track key; no code is an explicit
    /// sentinel, not a value inferred from flight identity.
    ident: Vec<u16>,
    /// Applicability time of the last valid source-observed code.
    ident_ns: Vec<i64>,
    ident_conflict: Vec<u8>,
    state: Vec<u8>,
    first_ns: Vec<i64>,
    confirmed_ns: Vec<i64>,
    last_ns: Vec<i64>,
    /// Rows of the track up to this time carry [`O_RADAR_DISAGREES`]; `i64::MIN` for none.
    disagree_until_ns: Vec<i64>,
    /// The track's own expected update interval; 0 for the configuration's (the fusion layer sets
    /// it from the scan periods of the radars that update the track).
    expected_ns: Vec<i64>,
    updates: Vec<u32>,
    hits: Vec<u8>,
    cur: Vec<Filter>,
    ring_len: Vec<u8>,
    ring_head: Vec<u8>,
    /// The emitter category trusted for each track and the candidate that may replace it.
    cat: Vec<CategoryState>,
    /// The filter state after the oldest measurement of each track's ring.
    anchor: Vec<Filter>,
    /// `max_tracks * RING_DEPTH` stored measurements.
    ring_m: Vec<Measurement>,
    /// Each track's unconfirmed large move (RT-239): 1 while there is one.
    mv_on: Vec<u8>,
    /// The time of the report that made the move (the latest one, if a later report made
    /// another before the first was decided).
    mv_start_ns: Vec<i64>,
    /// The time of applicability and sensor of the report that began the move (the first one,
    /// whatever later ones moved it again): the entry a late re-filter rebuilds the rollback
    /// point before.
    mv_first_ns: Vec<i64>,
    mv_first_sens: Vec<u16>,
    /// The time of the earlier report that agreed with the old path, [`NO_VOTE_NS`] if none: a
    /// rollback needs a second one (RT-273).
    mv_old_vote_ns: Vec<i64>,
    /// After a resynchronisation ended the move: the time up to which the rows stay marked and
    /// uncertain (the move's window would have ended then), `i64::MIN` if none.
    mv_resync_until_ns: Vec<i64>,
    /// The pool entry holding the state from before the move, [`NO_SNAP`] if there is none.
    mv_snap: Vec<u32>,
    /// The pool of rollback snapshots: `confirm_pool` entries (at most `max_tracks`).
    snaps: Vec<TrackSnap>,
    /// Free pool entries (a stack, so reuse order is fixed).
    snap_free: Vec<u32>,
    /// Tracks with an unconfirmed move now.
    mv_live: usize,
    /// The large-move rule is on for the decision being made: only the first decision of an
    /// in-order measurement. Off for a history replayed after a late insertion and for a report
    /// applied after a rollback.
    hold_on: bool,
    /// Free slots (a stack, so reuse order is fixed).
    free: Vec<u32>,
    /// Open-addressing key index: slot numbers, [`EMPTY`] if unused; `mask + 1` entries.
    index: Vec<u32>,
    mask: usize,
}

fn blank_filter() -> Filter {
    let m = VModel {
        x: [0.0; 2],
        p: [0.0; 3],
    };
    let blank_h = crate::hfilter::HModel {
        x: [0.0; 5],
        p: [[0.0; 5]; 5],
    };
    Filter {
        h: HFilter {
            m: [blank_h, blank_h],
            mu: [1.0, 0.0],
        },
        v: VFilter {
            m: [m, m],
            mu: [1.0, 0.0],
        },
        t_ns: 0,
        lat_e7: 0,
        lon_e7: 0,
        trig: [0.0, 1.0],
        rej_t_ns: 0,
        rej_lat_e7: 0,
        rej_lon_e7: 0,
        rej_h: 0,
        rej_v: 0,
        has_alt: 0,
        last_baro_alt_ft: NONE_I32,
        last_baro_t_ns: 0,
        last_baro_quant_ft: 0,
        h_mode: 0,
        rej_c: 0,
        reacq: 0,
    }
}

fn blank_measurement() -> Measurement {
    Measurement {
        toa_ns: 0,
        pos_cov_m2: [0.0; 3],
        vel_cov_m2s2: [0.0; 3],
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        lat_e7: 0,
        lon_e7: 0,
        alt_ft: NONE_I32,
        vrate_fpm: NONE_I32,
        addr: NO_ADDR,
        alt_quant_ft: 0,
        sensor: 0,
        ident: crate::types::NO_IDENT,
        flags: 0,
        category: 0,
        vel_lag_s: 0.0,
    }
}

/// A tracker with every table allocated for the configuration's capacity.
pub fn tracker_new(cfg: &TrackerConfig) -> Result<Tracker, TrackError> {
    check_config(cfg)?;
    let n = cfg.max_tracks;
    let index_len = (2 * n).next_power_of_two();
    let pool_n = cfg.confirm_pool.min(n);
    Ok(Tracker {
        cfg: *cfg,
        stats: TrackerStats::default(),
        log: log_new(cfg.log_capacity),
        clock_ns: i64::MIN,
        gate: gate_new(),
        held: vec![blank_measurement(); RUN_BUF],
        held_n: 0,
        newest_update_ns: i64::MIN,
        next_tick_ns: i64::MIN,
        high: 0,
        active: 0,
        key: vec![NO_ADDR; n],
        ident: vec![NO_IDENT; n],
        ident_ns: vec![NO_IDENT_NS; n],
        ident_conflict: vec![0; n],
        state: vec![FREE; n],
        first_ns: vec![0; n],
        confirmed_ns: vec![i64::MIN; n],
        last_ns: vec![0; n],
        disagree_until_ns: vec![i64::MIN; n],
        expected_ns: vec![0; n],
        updates: vec![0; n],
        hits: vec![0; n],
        cur: vec![blank_filter(); n],
        ring_len: vec![0; n],
        ring_head: vec![0; n],
        cat: vec![CategoryState::default(); n],
        anchor: vec![blank_filter(); n],
        ring_m: vec![blank_measurement(); n * RING_DEPTH],
        mv_on: vec![0; n],
        mv_start_ns: vec![0; n],
        mv_first_ns: vec![0; n],
        mv_first_sens: vec![0; n],
        mv_old_vote_ns: vec![NO_VOTE_NS; n],
        mv_resync_until_ns: vec![i64::MIN; n],
        mv_snap: vec![NO_SNAP; n],
        snaps: vec![blank_snap(); pool_n],
        snap_free: (0..pool_n as u32).rev().collect(),
        mv_live: 0,
        hold_on: false,
        // Popped from the end: slot 0 first.
        free: (0..n as u32).rev().collect(),
        index: vec![EMPTY; index_len],
        mask: index_len - 1,
    })
}

/// The counters.
pub fn tracker_stats(t: &Tracker) -> &TrackerStats {
    &t.stats
}

/// The log of ended tracks.
pub fn tracker_log(t: &Tracker) -> &TrackLog {
    &t.log
}

/// The configuration the tracker was built with.
pub fn tracker_config(t: &Tracker) -> &TrackerConfig {
    &t.cfg
}

/// `(tentative, confirmed)` tracks now.
pub fn tracker_counts(t: &Tracker) -> (usize, usize) {
    let mut c = (0, 0);
    for s in t.state.iter().take(t.high) {
        match *s {
            TENTATIVE => c.0 += 1,
            CONFIRMED => c.1 += 1,
            _ => {}
        }
    }
    c
}

/// Capacities of the tracker's buffers in stable field order. This fixed array does not allocate,
/// inspect pointer identity, or detect same-capacity reallocation. Complete allocation evidence
/// is provided by the separate counting-allocator gate. Test-only because allocator-selected
/// capacities are not a pure algorithm output (RULESET B8).
#[cfg(test)]
fn tracker_buffer_capacities(t: &Tracker) -> [usize; 36] {
    [
        t.held.capacity(),
        t.key.capacity(),
        t.ident.capacity(),
        t.ident_ns.capacity(),
        t.ident_conflict.capacity(),
        t.state.capacity(),
        t.first_ns.capacity(),
        t.confirmed_ns.capacity(),
        t.last_ns.capacity(),
        t.disagree_until_ns.capacity(),
        t.expected_ns.capacity(),
        t.updates.capacity(),
        t.hits.capacity(),
        t.cur.capacity(),
        t.ring_len.capacity(),
        t.ring_head.capacity(),
        t.cat.capacity(),
        t.anchor.capacity(),
        t.ring_m.capacity(),
        t.mv_on.capacity(),
        t.mv_start_ns.capacity(),
        t.mv_first_ns.capacity(),
        t.mv_first_sens.capacity(),
        t.mv_old_vote_ns.capacity(),
        t.mv_resync_until_ns.capacity(),
        t.mv_snap.capacity(),
        t.snaps.capacity(),
        t.snap_free.capacity(),
        t.free.capacity(),
        t.index.capacity(),
        t.log.addr.capacity(),
        t.log.first_ns.capacity(),
        t.log.confirmed_ns.capacity(),
        t.log.last_ns.capacity(),
        t.log.updates.capacity(),
        t.log.reason.capacity(),
    ]
}

/// Initialized element payload only for a test bound; excludes allocator reservation, metadata and
/// process memory, and is never reported as measured allocation or tracker memory.
#[cfg(test)]
fn tracker_payload_bytes_theoretical(t: &Tracker) -> usize {
    t.key.len() * core::mem::size_of::<u32>()
        + t.ident.len() * core::mem::size_of::<u16>()
        + t.ident_ns.len() * core::mem::size_of::<i64>()
        + t.ident_conflict.len() * core::mem::size_of::<u8>()
        + t.state.len() * core::mem::size_of::<u8>()
        + t.first_ns.len() * core::mem::size_of::<i64>()
        + t.confirmed_ns.len() * core::mem::size_of::<i64>()
        + t.last_ns.len() * core::mem::size_of::<i64>()
        + t.disagree_until_ns.len() * core::mem::size_of::<i64>()
        + t.expected_ns.len() * core::mem::size_of::<i64>()
        + t.updates.len() * core::mem::size_of::<u32>()
        + t.hits.len() * core::mem::size_of::<u8>()
        + t.cur.len() * core::mem::size_of::<Filter>()
        + t.ring_len.len() * core::mem::size_of::<u8>()
        + t.ring_head.len() * core::mem::size_of::<u8>()
        + t.cat.len() * core::mem::size_of::<CategoryState>()
        + t.anchor.len() * core::mem::size_of::<Filter>()
        + t.ring_m.len() * core::mem::size_of::<Measurement>()
        + t.mv_on.len() * core::mem::size_of::<u8>()
        + t.mv_start_ns.len() * core::mem::size_of::<i64>()
        + t.mv_first_ns.len() * core::mem::size_of::<i64>()
        + t.mv_first_sens.len() * core::mem::size_of::<u16>()
        + t.mv_old_vote_ns.len() * core::mem::size_of::<i64>()
        + t.mv_resync_until_ns.len() * core::mem::size_of::<i64>()
        + t.mv_snap.len() * core::mem::size_of::<u32>()
        + t.snaps.len() * core::mem::size_of::<TrackSnap>()
        + t.snap_free.len() * core::mem::size_of::<u32>()
        + t.free.len() * core::mem::size_of::<u32>()
        + t.index.len() * core::mem::size_of::<u32>()
        + t.log.addr.len() * core::mem::size_of::<u32>()
        + t.log.first_ns.len() * core::mem::size_of::<i64>()
        + t.log.confirmed_ns.len() * core::mem::size_of::<i64>()
        + t.log.last_ns.len() * core::mem::size_of::<i64>()
        + t.log.updates.len() * core::mem::size_of::<u32>()
        + t.log.reason.len() * core::mem::size_of::<u8>()
        + t.held.len() * core::mem::size_of::<Measurement>()
}

/// Bytes of the move's per-track columns from their guarded element sizes.
#[cfg(test)]
const MOVE_COLUMN_BYTES: usize = 1 + 8 + 4 + 8 + 2 + 8 + 8;

// ---- key index ------------------------------------------------------------------------

fn hash(key: u32, mask: usize) -> usize {
    (key.wrapping_mul(0x9E37_79B1) >> 7) as usize & mask
}

fn find_pos(t: &Tracker, key: u32) -> Option<usize> {
    let mut i = hash(key, t.mask);
    for _ in 0..=t.mask {
        let s = t.index.get(i).copied().unwrap_or(EMPTY);
        if s == EMPTY {
            return None;
        }
        if t.key.get(s as usize).copied() == Some(key) {
            return Some(i);
        }
        i = (i + 1) & t.mask;
    }
    None
}

fn lookup(t: &Tracker, key: u32) -> Option<usize> {
    find_pos(t, key).and_then(|p| t.index.get(p).copied().map(|s| s as usize))
}

fn index_insert(t: &mut Tracker, key: u32, slot: u32) {
    let mut i = hash(key, t.mask);
    for _ in 0..=t.mask {
        if t.index.get(i).copied() == Some(EMPTY) {
            if let Some(c) = t.index.get_mut(i) {
                *c = slot;
            }
            return;
        }
        i = (i + 1) & t.mask;
    }
}

/// Removes the entry at index position `pos` by backward shift, so no tombstones accumulate.
fn index_remove(t: &mut Tracker, pos: usize) {
    let mut i = pos;
    let mut j = pos;
    loop {
        j = (j + 1) & t.mask;
        let s = t.index.get(j).copied().unwrap_or(EMPTY);
        if s == EMPTY {
            break;
        }
        let k = hash(t.key.get(s as usize).copied().unwrap_or(0), t.mask);
        // The entry at j may move to i if its ideal position k is not in the cyclic range
        // (i, j].
        let in_range = if i <= j {
            k > i && k <= j
        } else {
            k > i || k <= j
        };
        if !in_range {
            if let Some(c) = t.index.get_mut(i) {
                *c = s;
            }
            i = j;
        }
    }
    if let Some(c) = t.index.get_mut(i) {
        *c = EMPTY;
    }
}

// ---- column helpers -------------------------------------------------------------------

fn col<T: Copy>(v: &[T], i: usize, default: T) -> T {
    v.get(i).copied().unwrap_or(default)
}

fn put<T>(v: &mut [T], i: usize, x: T) {
    if let Some(c) = v.get_mut(i) {
        *c = x;
    }
}

fn ring_ix(t: &Tracker, slot: usize, k: usize) -> usize {
    let head = usize::from(col(&t.ring_head, slot, 0));
    slot * RING_DEPTH + (head + k) % RING_DEPTH
}

fn ring_len(t: &Tracker, slot: usize) -> usize {
    usize::from(col(&t.ring_len, slot, 0))
}

fn ring_clear(t: &mut Tracker, slot: usize) {
    put(&mut t.ring_len, slot, 0);
    put(&mut t.ring_head, slot, 0);
}

/// Stores a measurement that was applied; the track's filter (`cur`) is the state after it. The
/// first entry of an empty ring sets the anchor from `cur`; when the ring is full the oldest
/// entry is dropped and the anchor moves on by applying the next one to it.
fn ring_push(t: &mut Tracker, slot: usize, m: &Measurement) -> bool {
    let len = ring_len(t, slot);
    let mut next_anchor = col(&t.anchor, slot, blank_filter());
    if len == RING_DEPTH {
        let next = col(&t.ring_m, ring_ix(t, slot, 1), blank_measurement());
        let mut a = next_anchor;
        // The counters were taken when the measurement was first applied.
        if filter_replay(&mut a, &next, &mut TrackerStats::default(), &t.cfg)
            == Applied::NumericalRefused
        {
            return false;
        }
        next_anchor = a;
    } else if len == 0 {
        next_anchor = col(&t.cur, slot, blank_filter());
    }
    if len == RING_DEPTH {
        put(&mut t.anchor, slot, next_anchor);
        let head = usize::from(col(&t.ring_head, slot, 0));
        put(&mut t.ring_head, slot, ((head + 1) % RING_DEPTH) as u8);
    } else {
        put(&mut t.ring_len, slot, (len + 1) as u8);
        if len == 0 {
            put(&mut t.anchor, slot, next_anchor);
        }
    }
    let k = ring_len(t, slot).saturating_sub(1);
    let at = ring_ix(t, slot, k);
    put(&mut t.ring_m, at, *m);
    true
}

/// The filter state after ring entry `k` (0 is the oldest): the anchor with entries `1..=k`
/// applied again, counters discarded.
fn ring_state(t: &Tracker, slot: usize, k: usize) -> Option<Filter> {
    let mut f = col(&t.anchor, slot, blank_filter());
    let mut scratch = TrackerStats::default();
    for j in 1..=k {
        let m = col(&t.ring_m, ring_ix(t, slot, j), blank_measurement());
        if filter_replay(&mut f, &m, &mut scratch, &t.cfg) == Applied::NumericalRefused {
            return None;
        }
    }
    Some(f)
}

/// Whether the ring holds a measurement with this time of applicability from this sensor: a
/// repeat of a report already applied. Any entry counts, not just the newest or the one before
/// the measurement's time: two sensors may report at one instant, and the repeat of the first's
/// report then arrives after the second's.
fn ring_has(t: &Tracker, slot: usize, toa_ns: i64, sensor: u16) -> bool {
    (0..ring_len(t, slot)).any(|k| {
        let m = col(&t.ring_m, ring_ix(t, slot, k), blank_measurement());
        m.toa_ns == toa_ns && m.sensor == sensor
    })
}

// ---- measurement validation -----------------------------------------------------------

/// Checks a measurement (E5): the first failing rule's `REF_*` index.
pub fn measurement_check(m: &Measurement) -> Result<(), usize> {
    if m.toa_ns.unsigned_abs() > TIME_LIMIT_NS as u64 {
        return Err(REF_TIME);
    }
    if !(-900_000_000..=900_000_000).contains(&m.lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&m.lon_e7)
    {
        return Err(REF_POSITION);
    }
    let cov_ok = |c: &[f32; 3]| {
        c.iter().all(|v| v.is_finite())
            && c[0] > 0.0
            && c[2] > 0.0
            && (c[0] as f64) * (c[2] as f64) > (c[1] as f64) * (c[1] as f64)
    };
    if !cov_ok(&m.pos_cov_m2) {
        return Err(REF_POS_COV);
    }
    if m.flags & crate::types::M_HAS_VEL != 0 {
        // Finite single-precision components, so the squares cannot overflow a double.
        let (ve, vn) = (f64::from(m.vel_east_mps), f64::from(m.vel_north_mps));
        let v_ok = m.vel_east_mps.is_finite()
            && m.vel_north_mps.is_finite()
            && oa_ieee::sqrt(ve * ve + vn * vn) <= SPEED_ENVELOPE_MPS
            && cov_ok(&m.vel_cov_m2s2)
            && (0.0..=crate::params::VEL_LAG_MAX_S).contains(&f64::from(m.vel_lag_s));
        if !v_ok {
            return Err(REF_VELOCITY);
        }
    }
    if m.alt_ft != NONE_I32 && !(-2_000..=100_000).contains(&m.alt_ft) {
        return Err(REF_ALTITUDE);
    }
    if m.vrate_fpm != NONE_I32 && !(-30_000..=30_000).contains(&m.vrate_fpm) {
        return Err(REF_ALTITUDE);
    }
    if m.addr == NO_ADDR {
        return Err(REF_NO_ADDRESS);
    }
    if m.ident != NO_IDENT && m.ident > 0o7777 {
        return Err(REF_IDENTITY);
    }
    Ok(())
}

// ---- track lifecycle ------------------------------------------------------------------

fn init_window_ns(t: &Tracker) -> i64 {
    i64::from(t.cfg.init_n).saturating_mul(t.cfg.init_interval_ns)
}

/// Logs and frees a track.
fn end_track(t: &mut Tracker, slot: usize, reason: u8) {
    let addr = col(&t.key, slot, NO_ADDR);
    if t.log.addr.len() < t.log.capacity {
        t.log.addr.push(addr);
        t.log.first_ns.push(col(&t.first_ns, slot, 0));
        t.log
            .confirmed_ns
            .push(col(&t.confirmed_ns, slot, i64::MIN));
        t.log.last_ns.push(col(&t.last_ns, slot, 0));
        t.log.updates.push(col(&t.updates, slot, 0));
        t.log.reason.push(reason);
    } else {
        t.log.overflow = t.log.overflow.saturating_add(1);
    }
    if let Some(c) = t.stats.ended.get_mut(usize::from(reason)) {
        *c += 1;
    }
    if let Some(p) = find_pos(t, addr) {
        index_remove(t, p);
    }
    put(&mut t.state, slot, FREE);
    put(&mut t.key, slot, NO_ADDR);
    end_move(t, slot, MoveEnd::Ended);
    put(&mut t.mv_resync_until_ns, slot, i64::MIN);
    ring_clear(t, slot);
    t.free.push(slot as u32);
    t.active = t.active.saturating_sub(1);
}

/// Sets a track's newest update time, and the tracker's bound on all of them.
fn set_last(t: &mut Tracker, slot: usize, ns: i64) {
    put(&mut t.last_ns, slot, ns);
    t.newest_update_ns = t.newest_update_ns.max(ns);
}

/// Sets the expected update interval of the track of `key` to `ns` (it coasts after
/// `coast_after_intervals` of them); `ns <= 0` returns it to the configured one. Returns whether
/// there is such a track.
pub fn tracker_set_expected_interval(t: &mut Tracker, key: u32, ns: i64) -> bool {
    let Some(slot) = lookup(t, key) else {
        return false;
    };
    put(&mut t.expected_ns, slot, ns.max(0));
    true
}

/// Marks the track of `key` as contradicted by a radar until `until_ns`: its rows at or before
/// then carry [`O_RADAR_DISAGREES`] (a later call can only extend it). Nothing about the track's
/// state changes. Returns whether there is such a track.
pub fn tracker_note_radar_disagreement(t: &mut Tracker, key: u32, until_ns: i64) -> bool {
    let Some(slot) = lookup(t, key) else {
        return false;
    };
    let until = until_ns.max(col(&t.disagree_until_ns, slot, i64::MIN));
    put(&mut t.disagree_until_ns, slot, until);
    true
}

/// The newest update time any track has had (`i64::MIN` before any): at least that of the newest
/// live track, and equal to it until the track that had it ends.
pub fn tracker_newest_update_ns(t: &Tracker) -> i64 {
    t.newest_update_ns
}

/// Starts (or restarts) a tentative track in `slot` from `m`.
fn track_begin(t: &mut Tracker, slot: usize, m: &Measurement) -> bool {
    put(&mut t.cur, slot, filter_init(m, t.cfg.h_mode));
    put(&mut t.cat, slot, CategoryState::default());
    if m.ident != NO_IDENT {
        put(&mut t.ident, slot, m.ident);
        put(&mut t.ident_ns, slot, m.toa_ns);
        put(&mut t.ident_conflict, slot, 0);
    } else {
        put(&mut t.ident, slot, NO_IDENT);
        put(&mut t.ident_ns, slot, NO_IDENT_NS);
        put(&mut t.ident_conflict, slot, 0);
    }
    put(&mut t.mv_resync_until_ns, slot, i64::MIN);
    note_category(t, slot, m);
    ring_clear(t, slot);
    if !ring_push(t, slot, m) {
        return false;
    }
    put(&mut t.first_ns, slot, m.toa_ns);
    set_last(t, slot, m.toa_ns);
    put(&mut t.updates, slot, 1);
    put(&mut t.hits, slot, 1);
    put(&mut t.confirmed_ns, slot, i64::MIN);
    put(&mut t.disagree_until_ns, slot, i64::MIN);
    put(&mut t.expected_ns, slot, 0);
    put(&mut t.state, slot, TENTATIVE);
    maybe_confirm(t, slot, m.toa_ns);
    true
}

fn maybe_confirm(t: &mut Tracker, slot: usize, at_ns: i64) {
    if col(&t.state, slot, FREE) == TENTATIVE && col(&t.hits, slot, 0) >= t.cfg.init_m {
        put(&mut t.state, slot, CONFIRMED);
        put(&mut t.confirmed_ns, slot, at_ns);
        t.stats.confirmed += 1;
    }
}

#[derive(Clone, Copy)]
enum EmissionKind {
    Measurement,
    Tick,
    Snapshot,
}

fn emit(t: &mut Tracker, slot: usize, at_ns: i64, flags: u8, out: &mut TrackOutput) {
    let kind = if flags & O_TICK != 0 {
        EmissionKind::Tick
    } else {
        EmissionKind::Measurement
    };
    emit_kind(t, slot, at_ns, flags, kind, out);
}

fn emit_snapshot(t: &mut Tracker, slot: usize, at_ns: i64, out: &mut TrackOutput) {
    let flags = if track_is_coasting(t, slot, at_ns) {
        O_COAST
    } else {
        0
    };
    emit_kind(t, slot, at_ns, flags, EmissionKind::Snapshot, out);
}

fn emit_kind(
    t: &mut Tracker,
    slot: usize,
    at_ns: i64,
    flags: u8,
    kind: EmissionKind,
    out: &mut TrackOutput,
) {
    t.stats.rows_computed += 1;
    let mut f = col(&t.cur, slot, blank_filter());
    // A position that cannot be represented is never a row, and never a coordinate of 0.
    let Some(mut row) = filter_output(&f, at_ns) else {
        t.stats.rows_unrepresentable += 1;
        return;
    };
    let mut r95 = r95_m(&row.pos_cov_m2);
    if r95.is_nan() {
        // Its uncertainty is not known: never a row, counted as loss.
        t.stats.nan_covariance += 1;
        return;
    }
    row.addr = col(&t.key, slot, NO_ADDR);
    let ident_ns = col(&t.ident_ns, slot, NO_IDENT_NS);
    if ident_ns <= at_ns {
        row.ident_ns = ident_ns;
        row.ident = col(&t.ident, slot, NO_IDENT);
        row.ident_conflict = col(&t.ident_conflict, slot, 0);
    }
    row.flags |= flags;
    let r95_own = r95;
    if move_active(t, slot) {
        let old = old_path_row(t, slot, at_ns);
        if !mark_row(&mut row, &mut r95, old.as_ref()) {
            t.stats.move_widen_failed += 1;
        }
    } else if at_ns <= col(&t.mv_resync_until_ns, slot, i64::MIN) {
        // After a resynchronisation that ended a move: marked, and uncertain (no old path is
        // left, so the radius is only lifted over the uncertainty threshold).
        row.flags |= O_MOVE_UNCONFIRMED;
        widen_over(&mut row, &mut r95, t.cfg.coast_max_r95_m);
    }
    // The reacquisition arming follows the filter's own radius, not the widened one, so a
    // marked row leaves no trace on the rows after the mark is gone.
    let uncertain = r95 > t.cfg.coast_max_r95_m;
    let armed_uncertain = r95_own > t.cfg.coast_max_r95_m;
    if uncertain {
        row.flags |= O_UNCERTAIN;
        t.stats.rows_uncertain += 1;
    }
    // Snapshots report current state without consuming reacquisition state. Update rows consume
    // it once; periodic ticks preserve it and can arm it from uncertainty.
    match kind {
        EmissionKind::Measurement => {
            if f.reacq != 0 {
                row.flags |= O_REACQUIRED;
            }
            f.reacq = u8::from(armed_uncertain);
            put(&mut t.cur, slot, f);
        }
        EmissionKind::Tick => {
            f.reacq |= u8::from(armed_uncertain);
            put(&mut t.cur, slot, f);
        }
        EmissionKind::Snapshot => {
            if f.reacq != 0 {
                row.flags |= O_REACQUIRED;
            }
        }
    }
    if row.addr >= RADAR_KEY_BASE {
        row.flags |= O_IDENTITY_UNCONFIRMED;
    }
    if at_ns <= col(&t.disagree_until_ns, slot, i64::MIN) {
        row.flags |= O_RADAR_DISAGREES;
    }
    output_push(out, &row);
}

fn track_is_coasting(t: &Tracker, slot: usize, at_ns: i64) -> bool {
    let own = col(&t.expected_ns, slot, 0);
    let limit = if own > 0 {
        t.cfg.coast_after_intervals.saturating_mul(own)
    } else {
        t.cfg
            .coast_after_intervals
            .saturating_mul(t.cfg.expected_interval_ns)
    };
    at_ns
        .checked_sub(col(&t.last_ns, slot, 0))
        .is_none_or(|age| age > limit)
}

/// Scales the row's position covariance so that its 95% radius is just over `floor_m` if it was
/// not, keeping its shape (a circle of the fail-closed radius if the scaling is not finite).
fn widen_over(row: &mut OutRow, r95: &mut f64, floor_m: f64) {
    if *r95 > floor_m {
        return;
    }
    let target = floor_m * (1.0 + 1.0e-6);
    let scaled = if *r95 > 0.0 {
        let k = (target / *r95) * (target / *r95) * (1.0 + 1.0e-6);
        row.pos_cov_m2.map(|c| (f64::from(c) * k) as f32)
    } else {
        let v = (target * target / CHI2_2_95 * (1.0 + 1.0e-6)) as f32;
        [v, 0.0, v]
    };
    let r = r95_m(&scaled);
    if r.is_finite() && r > floor_m {
        row.pos_cov_m2 = scaled;
        *r95 = r;
    } else {
        row.pos_cov_m2 = FAIL_CLOSED_COV;
        *r95 = r95_m(&FAIL_CLOSED_COV);
    }
}

/// The old path's row at `at_ns`: the snapshot's filter predicted to the same time. `None` if
/// the move has no snapshot or the old path's position cannot be represented.
fn old_path_row(t: &Tracker, slot: usize, at_ns: i64) -> Option<OutRow> {
    let e = t.snaps.get(move_snap_ix(t, slot)?)?;
    filter_output(&e.cur, at_ns)
}

/// Marks a row of a track with an unconfirmed move (RT-239) and widens its position covariance
/// so that its 95% radius, recomputed from the stored `f32` covariance, is at least the distance
/// from the old path's row to this one plus the old path's own radius: the row then covers where
/// the track would be had the move been a forged report. The covariance is scaled, so its shape
/// is kept. If that cannot be computed (no old path, an old position that cannot be
/// represented, a result that is not finite) the row gets a circle of `FAIL_CLOSED_R95_M` (500 km), so that the radius is
/// never smaller than the truth and stays finite: returns false then. The radius,
/// the unwidened input and the widened result, are in `r95`.
fn mark_row(row: &mut OutRow, r95: &mut f64, old: Option<&OutRow>) -> bool {
    row.flags |= O_MOVE_UNCONFIRMED;
    let wide = old
        .map(|o| filter_row_distance_m(row, o) + r95_m(&o.pos_cov_m2))
        .filter(|w| w.is_finite());
    if let Some(wide) = wide {
        if wide <= *r95 {
            return true;
        }
        // A hair over, for the rounding of the stored covariance.
        let scaled = if *r95 > 0.0 {
            let k = (wide / *r95) * (wide / *r95) * (1.0 + 1.0e-6);
            if !k.is_finite() {
                row.pos_cov_m2 = FAIL_CLOSED_COV;
                *r95 = r95_m(&FAIL_CLOSED_COV);
                return false;
            }
            row.pos_cov_m2.map(|c| (f64::from(c) * k) as f32)
        } else {
            let v = (wide * wide / CHI2_2_95 * (1.0 + 1.0e-6)) as f32;
            [v, 0.0, v]
        };
        let r = r95_m(&scaled);
        if r.is_finite() {
            row.pos_cov_m2 = scaled;
            *r95 = r;
            return true;
        }
    }
    row.pos_cov_m2 = FAIL_CLOSED_COV;
    *r95 = r95_m(&FAIL_CLOSED_COV);
    false
}

/// The position covariance of a row whose widening failed: a circle of at least
/// [`FAIL_CLOSED_R95_M`] 95% radius, finite everywhere it is read.
const FAIL_CLOSED_COV: [f32; 3] = {
    let v = (FAIL_CLOSED_R95_M * FAIL_CLOSED_R95_M / CHI2_2_95 * (1.0 + 1.0e-6)) as f32;
    [v, 0.0, v]
};

/// The 95% horizontal uncertainty radius of a position covariance `[ee, en, nn]` (m^2), metres:
/// the semi-major axis of the 95% ellipse, so the circle of this radius contains the ellipse.
/// The value behind [`O_UNCERTAIN`], for a consumer to read from a row's `pos_cov_m2`.
pub fn r95_m_of(c: &[f32; 3]) -> f64 {
    r95_m(c)
}

fn r95_m(c: &[f32; 3]) -> f64 {
    let (ee, en, nn) = (f64::from(c[0]), f64::from(c[1]), f64::from(c[2]));
    let half = 0.5 * (ee - nn);
    let lmax = 0.5 * (ee + nn) + oa_ieee::sqrt(half * half + en * en);
    oa_ieee::sqrt(CHI2_2_95 * lmax)
}

/// A measurement's ground speed in 0.1 kt for the jump rule, [`NONE_U16`] without a velocity.
fn gs_dkt(m: &Measurement) -> u16 {
    if m.flags & M_HAS_VEL == 0 {
        return NONE_U16;
    }
    let kt = oa_math::hypot(f64::from(m.vel_east_mps), f64::from(m.vel_north_mps)) / MPS_PER_KT;
    // Saturates one below the absent value.
    oa_math::clamp_f64(oa_ieee::round(kt * 10.0), 0.0, f64::from(NONE_U16 - 1)) as u16
}

/// The filter's estimated ground speed in 0.1 kt, [`NONE_U16`] if it is not a number.
fn estimated_gs_dkt(f: &Filter) -> u16 {
    let (x, _) = h_combine(&f.h);
    let v = oa_math::hypot(
        x.get(2).copied().unwrap_or(0.0),
        x.get(3).copied().unwrap_or(0.0),
    );
    let dkt = v / MPS_PER_KT * 10.0;
    if dkt.is_finite() {
        // Saturates one below the absent value.
        oa_math::clamp_f64(oa_ieee::round(dkt), 0.0, f64::from(NONE_U16 - 1)) as u16
    } else {
        NONE_U16
    }
}

/// The track's position to the measurement is not a displacement the speed rule
/// (`oa_adsb_model::jump_verdict`) allows: the measurement is not the same aircraft's next report.
/// The rule takes the category and the ground speeds of the track's newest report and of the
/// measurement; the category is the track's trusted one with the measurement's printed category
/// taken into account by the persistence rule, as it would be if the measurement is applied.
fn implausible(t: &Tracker, slot: usize, m: &Measurement) -> bool {
    let f = col(&t.cur, slot, blank_filter());
    let frame = crate::filter::filter_frame(&f);
    let (x, y) = oa_geo::aeqd::project(
        &frame,
        f64::from(m.lat_e7) / 1.0e7,
        f64::from(m.lon_e7) / 1.0e7,
    );
    let n = ring_len(t, slot);
    let prev = n
        .checked_sub(1)
        .map(|k| col(&t.ring_m, ring_ix(t, slot, k), blank_measurement()));
    let mut trial = col(&t.cat, slot, CategoryState::default());
    let category = category_observe(&mut trial, m.category);
    let pair = JumpPair {
        // `project` returns coordinates within half the earth's circumference, so the squares
        // cannot overflow.
        dist_m: oa_ieee::sqrt(x * x + y * y),
        dt_s: (m.toa_ns - f.t_ns).max(0) as f64 / NS_PER_S as f64,
        slack_m: PLAUSIBLE_SLACK_M,
        category,
        gs_a_dkt: prev.map_or(NONE_U16, |p| match gs_dkt(&p) {
            // The newest report gives no speed: the filter's own estimate stands in for it once
            // it has had two updates to form one (the first leaves only the prior).
            NONE_U16 if col(&t.updates, slot, 0) >= 2 => estimated_gs_dkt(&f),
            g => g,
        }),
        gs_b_dkt: gs_dkt(m),
    };
    jump_verdict(&pair, CAP_ORDINARY_KT) != JumpVerdict::Plausible
}

/// Adds an applied measurement's printed category to the track's.
fn note_category(t: &mut Tracker, slot: usize, m: &Measurement) {
    let mut c = col(&t.cat, slot, CategoryState::default());
    category_observe(&mut c, m.category);
    put(&mut t.cat, slot, c);
}

/// Fixed-size rollback point for one track and its replay history.
#[derive(Clone, Copy)]
#[repr(C)]
struct TrackTxn {
    cur: Filter,
    anchor: Filter,
    ring: [Measurement; RING_DEPTH],
    stats: TrackerStats,
    first_ns: i64,
    confirmed_ns: i64,
    last_ns: i64,
    disagree_until_ns: i64,
    expected_ns: i64,
    newest_update_ns: i64,
    cat: CategoryState,
    updates: u32,
    state: u8,
    hits: u8,
    ring_len: u8,
    ring_head: u8,
}
// Budget: two 664-byte filters, twelve 72-byte measurements, 488-byte statistics (328 before
// the eighteen large-move counters of RT-239), six 8-byte times, 8-byte category, 4-byte update
// count and four byte fields.
// A7.1: +16 bytes carry a source Mode 3/A range-refusal counter and same-time conflict counter.
// One bounded stack checkpoint; no track key changes during an update or its replay.
const _: () = assert!(core::mem::size_of::<TrackTxn>() == 2744);

/// The pool entry of a move with no snapshot.
const NO_SNAP: u32 = u32::MAX;

/// A track's state from before a large move (RT-239), the rollback point: the measurement state
/// of the track, as [`TrackTxn`] captures it, without the tracker's counters and bound (not the
/// track's) and without the two columns written from outside the update path
/// (`disagree_until_ns` by the fusion layer, `expected_ns` by the fusion layer from the scan
/// periods): a rollback must not undo what a radar said in between.
#[derive(Clone, Copy)]
#[repr(C)]
struct TrackSnap {
    cur: Filter,
    anchor: Filter,
    ring: [Measurement; RING_DEPTH],
    first_ns: i64,
    confirmed_ns: i64,
    last_ns: i64,
    cat: CategoryState,
    updates: u32,
    state: u8,
    hits: u8,
    ring_len: u8,
    ring_head: u8,
}
// Budget: two 664-byte filters, twelve 72-byte measurements, three 8-byte times, 8-byte
// category, 4-byte update count and four byte fields.
const _: () = assert!(core::mem::size_of::<TrackSnap>() == 2232);

fn blank_snap() -> TrackSnap {
    TrackSnap {
        cur: blank_filter(),
        anchor: blank_filter(),
        ring: [blank_measurement(); RING_DEPTH],
        first_ns: 0,
        confirmed_ns: i64::MIN,
        last_ns: 0,
        cat: CategoryState::default(),
        updates: 0,
        state: FREE,
        hits: 0,
        ring_len: 0,
        ring_head: 0,
    }
}

/// The snapshot of what a [`TrackTxn`] captured (the update's own checkpoint is the move's
/// rollback point: no second capture).
fn snap_of_txn(x: &TrackTxn) -> TrackSnap {
    TrackSnap {
        cur: x.cur,
        anchor: x.anchor,
        ring: x.ring,
        first_ns: x.first_ns,
        confirmed_ns: x.confirmed_ns,
        last_ns: x.last_ns,
        cat: x.cat,
        updates: x.updates,
        state: x.state,
        hits: x.hits,
        ring_len: x.ring_len,
        ring_head: x.ring_head,
    }
}

/// The track's measurement state as it is now.
fn snap_take(t: &Tracker, slot: usize) -> TrackSnap {
    let mut ring = [blank_measurement(); RING_DEPTH];
    let start = slot * RING_DEPTH;
    for (k, value) in ring.iter_mut().enumerate() {
        *value = col(&t.ring_m, start + k, blank_measurement());
    }
    TrackSnap {
        cur: col(&t.cur, slot, blank_filter()),
        anchor: col(&t.anchor, slot, blank_filter()),
        ring,
        first_ns: col(&t.first_ns, slot, 0),
        confirmed_ns: col(&t.confirmed_ns, slot, i64::MIN),
        last_ns: col(&t.last_ns, slot, 0),
        cat: col(&t.cat, slot, CategoryState::default()),
        updates: col(&t.updates, slot, 0),
        state: col(&t.state, slot, FREE),
        hits: col(&t.hits, slot, 0),
        ring_len: col(&t.ring_len, slot, 0),
        ring_head: col(&t.ring_head, slot, 0),
    }
}

/// Puts the track's measurement state back as `s` held it. Not the columns written from outside
/// the update path, not the tracker's counters or its bound on the newest update.
fn snap_restore(t: &mut Tracker, slot: usize, s: &TrackSnap) {
    put(&mut t.state, slot, s.state);
    put(&mut t.first_ns, slot, s.first_ns);
    put(&mut t.confirmed_ns, slot, s.confirmed_ns);
    put(&mut t.last_ns, slot, s.last_ns);
    put(&mut t.updates, slot, s.updates);
    put(&mut t.hits, slot, s.hits);
    put(&mut t.cur, slot, s.cur);
    put(&mut t.ring_len, slot, s.ring_len);
    put(&mut t.ring_head, slot, s.ring_head);
    put(&mut t.cat, slot, s.cat);
    put(&mut t.anchor, slot, s.anchor);
    let start = slot * RING_DEPTH;
    for (k, value) in s.ring.iter().enumerate() {
        put(&mut t.ring_m, start + k, *value);
    }
}

/// How an unconfirmed move ended: every one ends in exactly one of these, each counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveEnd {
    Confirmed,
    RolledBack,
    Undecided,
    Ended,
    Restarted,
    Resync,
    Demoted,
}

fn move_active(t: &Tracker, slot: usize) -> bool {
    t.mv_on.get(slot).is_some_and(|&on| on != 0)
}

/// The pool entry of the track's move, if it has a snapshot.
fn move_snap_ix(t: &Tracker, slot: usize) -> Option<usize> {
    match t.mv_snap.get(slot) {
        Some(&i) if i != NO_SNAP => Some(i as usize),
        _ => None,
    }
}

/// Gives the track's snapshot back to the pool.
fn release_snap(t: &mut Tracker, slot: usize) {
    if let Some(i) = move_snap_ix(t, slot) {
        t.snap_free.push(i as u32);
    }
    put(&mut t.mv_snap, slot, NO_SNAP);
}

/// Ends the track's unconfirmed move, if it has one, in the outcome `why` (counted).
fn end_move(t: &mut Tracker, slot: usize, why: MoveEnd) {
    if !move_active(t, slot) {
        return;
    }
    if why == MoveEnd::Resync {
        // The old path goes with the resynchronised filter, and the track is now where five
        // refused reports put it: not silent. The rows stay marked and uncertain until the
        // window the move would have had is over.
        let window = confirm_window_ns(&t.cfg, col(&t.expected_ns, slot, 0));
        let until = col(&t.mv_start_ns, slot, 0).saturating_add(window);
        put(&mut t.mv_resync_until_ns, slot, until);
        t.stats.moves_resync_marked += 1;
    }
    release_snap(t, slot);
    put(&mut t.mv_old_vote_ns, slot, NO_VOTE_NS);
    put(&mut t.mv_on, slot, 0);
    t.mv_live = t.mv_live.saturating_sub(1);
    let c = match why {
        MoveEnd::Confirmed => &mut t.stats.moves_confirmed,
        MoveEnd::RolledBack => &mut t.stats.moves_rolled_back,
        MoveEnd::Undecided => &mut t.stats.moves_undecided,
        MoveEnd::Ended => &mut t.stats.moves_abandoned_ended,
        MoveEnd::Restarted => &mut t.stats.moves_abandoned_restarted,
        MoveEnd::Resync => &mut t.stats.moves_abandoned_resync,
        MoveEnd::Demoted => &mut t.stats.moves_abandoned_demoted,
    };
    *c += 1;
}

/// Starts the track's unconfirmed move, made by the report `first`, at the report time `start_ns`,
/// with `before` as its rollback point. A track that already has one keeps its original snapshot (a later move that
/// is not decided in between rolls back to before the first) and only moves the start, the time
/// the window runs from. With the pool full the move has no snapshot (`move_no_snapshot`).
fn start_move(
    t: &mut Tracker,
    slot: usize,
    before: &TrackSnap,
    first: &Measurement,
    start_ns: i64,
) {
    put(&mut t.mv_start_ns, slot, start_ns);
    // A move at a new place starts the count of old-path reports again, and the marking that
    // followed an earlier move's resynchronisation is over: this move's own mark rules now
    // (it is cleared with the move's decision, not held past it by the earlier one).
    put(&mut t.mv_old_vote_ns, slot, NO_VOTE_NS);
    put(&mut t.mv_resync_until_ns, slot, i64::MIN);
    if move_active(t, slot) {
        return;
    }
    put(&mut t.mv_first_ns, slot, first.toa_ns);
    put(&mut t.mv_first_sens, slot, first.sensor);
    put(&mut t.mv_on, slot, 1);
    t.mv_live += 1;
    t.stats.moves_peak_active = t.stats.moves_peak_active.max(t.mv_live as u64);
    match t.snap_free.pop() {
        Some(i) => {
            if let Some(e) = t.snaps.get_mut(i as usize) {
                *e = *before;
            }
            put(&mut t.mv_snap, slot, i);
        }
        None => {
            put(&mut t.mv_snap, slot, NO_SNAP);
            t.stats.move_no_snapshot += 1;
        }
    }
}

/// Tracks with an unconfirmed move now.
pub fn tracker_moves_active(t: &Tracker) -> usize {
    t.mv_live
}

fn track_txn(t: &Tracker, slot: usize) -> TrackTxn {
    let mut ring = [blank_measurement(); RING_DEPTH];
    let start = slot * RING_DEPTH;
    for (k, value) in ring.iter_mut().enumerate() {
        *value = col(&t.ring_m, start + k, blank_measurement());
    }
    TrackTxn {
        state: col(&t.state, slot, FREE),
        first_ns: col(&t.first_ns, slot, 0),
        confirmed_ns: col(&t.confirmed_ns, slot, i64::MIN),
        last_ns: col(&t.last_ns, slot, 0),
        disagree_until_ns: col(&t.disagree_until_ns, slot, i64::MIN),
        expected_ns: col(&t.expected_ns, slot, 0),
        updates: col(&t.updates, slot, 0),
        hits: col(&t.hits, slot, 0),
        cur: col(&t.cur, slot, blank_filter()),
        ring_len: col(&t.ring_len, slot, 0),
        ring_head: col(&t.ring_head, slot, 0),
        cat: col(&t.cat, slot, CategoryState::default()),
        anchor: col(&t.anchor, slot, blank_filter()),
        ring,
        newest_update_ns: t.newest_update_ns,
        stats: t.stats,
    }
}

/// Puts the track's own columns back as `snapshot` held them: not the tracker's counters or its
/// bound on the newest update.
fn restore_track_columns(t: &mut Tracker, slot: usize, snapshot: &TrackTxn) {
    put(&mut t.state, slot, snapshot.state);
    put(&mut t.first_ns, slot, snapshot.first_ns);
    put(&mut t.confirmed_ns, slot, snapshot.confirmed_ns);
    put(&mut t.last_ns, slot, snapshot.last_ns);
    put(&mut t.disagree_until_ns, slot, snapshot.disagree_until_ns);
    put(&mut t.expected_ns, slot, snapshot.expected_ns);
    put(&mut t.updates, slot, snapshot.updates);
    put(&mut t.hits, slot, snapshot.hits);
    put(&mut t.cur, slot, snapshot.cur);
    put(&mut t.ring_len, slot, snapshot.ring_len);
    put(&mut t.ring_head, slot, snapshot.ring_head);
    put(&mut t.cat, slot, snapshot.cat);
    put(&mut t.anchor, slot, snapshot.anchor);
    let start = slot * RING_DEPTH;
    for (k, value) in snapshot.ring.iter().enumerate() {
        put(&mut t.ring_m, start + k, *value);
    }
}

fn restore_track_txn(t: &mut Tracker, slot: usize, snapshot: TrackTxn) {
    restore_track_columns(t, slot, &snapshot);
    t.newest_update_ns = snapshot.newest_update_ns;
    t.stats = snapshot.stats;
    t.stats.numerical_refused = t.stats.numerical_refused.saturating_add(1);
}

#[derive(Clone, Copy)]
struct OutputTxn {
    overflow: u64,
    lens: [usize; 18],
}
// Private rollback snapshot: host-sized vector lengths are never serialized or exposed.

fn output_txn(out: &TrackOutput) -> OutputTxn {
    OutputTxn {
        overflow: out.overflow,
        lens: [
            out.t_ns.len(),
            out.addr.len(),
            out.ident.len(),
            out.ident_ns.len(),
            out.ident_conflict.len(),
            out.lat_e7.len(),
            out.lon_e7.len(),
            out.vel_east_mps.len(),
            out.vel_north_mps.len(),
            out.pos_cov_m2.len(),
            out.vel_cov_m2s2.len(),
            out.turn_rate_rps.len(),
            out.alt_ft.len(),
            out.alt_var_ft2.len(),
            out.vrate_fpm.len(),
            out.mu_h.len(),
            out.mu_v.len(),
            out.flags.len(),
        ],
    }
}

fn restore_output_txn(out: &mut TrackOutput, snapshot: OutputTxn) {
    out.overflow = snapshot.overflow;
    let [t_ns, addr, ident, ident_ns, ident_conflict, lat_e7, lon_e7, vel_east_mps, vel_north_mps, pos_cov_m2, vel_cov_m2s2, turn_rate_rps, alt_ft, alt_var_ft2, vrate_fpm, mu_h, mu_v, flags] =
        snapshot.lens;
    out.t_ns.truncate(t_ns);
    out.addr.truncate(addr);
    out.ident.truncate(ident);
    out.ident_ns.truncate(ident_ns);
    out.ident_conflict.truncate(ident_conflict);
    out.lat_e7.truncate(lat_e7);
    out.lon_e7.truncate(lon_e7);
    out.vel_east_mps.truncate(vel_east_mps);
    out.vel_north_mps.truncate(vel_north_mps);
    out.pos_cov_m2.truncate(pos_cov_m2);
    out.vel_cov_m2s2.truncate(vel_cov_m2s2);
    out.turn_rate_rps.truncate(turn_rate_rps);
    out.alt_ft.truncate(alt_ft);
    out.alt_var_ft2.truncate(alt_var_ft2);
    out.vrate_fpm.truncate(vrate_fpm);
    out.mu_h.truncate(mu_h);
    out.mu_v.truncate(mu_v);
    out.flags.truncate(flags);
}

/// What [`decide`] did with a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The track restarted from the measurement (initiation window or plausibility).
    Begun,
    /// Applied to the filter.
    Applied,
    /// Rejected by the position gate (the filter keeps the run of rejections).
    Rejected,
    /// Too many rejections in a row: the filter started again at the measurement.
    Resynced,
    /// The filter refused a numerical prediction; callers roll back the update.
    NumericalRefused,
    /// Applied, and a move of more than `confirm_move_m` (RT-239): the caller keeps the state
    /// from before it as the track's rollback point.
    Moved,
}

/// The per-measurement decision, the same for a measurement that arrives in order and for each
/// one of a history replayed after a late insertion: initiation window and plausibility of a
/// tentative track, the position gate, the rejection streak and the resync. Updates the track's
/// columns; counts the decision's own events (restarts, rejections, resyncs) but not
/// `applied`, which the callers count as their path requires. The measurement is at or after the
/// track's newest update.
fn decide(t: &mut Tracker, slot: usize, m: &Measurement) -> Step {
    if col(&t.state, slot, FREE) == TENTATIVE {
        if m.toa_ns - col(&t.first_ns, slot, 0) > init_window_ns(t) {
            t.stats.init_window_expired += 1;
            return if track_begin(t, slot, m) {
                Step::Begun
            } else {
                Step::NumericalRefused
            };
        }
        if implausible(t, slot, m) {
            t.stats.init_implausible += 1;
            return if track_begin(t, slot, m) {
                Step::Begun
            } else {
                Step::NumericalRefused
            };
        }
    }
    let mut f = col(&t.cur, slot, blank_filter());
    // The rule is off for a replay, for a report applied after a rollback, and for a track not
    // yet confirmed (a second report with no velocity moves far from its prediction as a matter
    // of course; the plausibility rule judges it).
    let want = t.cfg.confirm_moves && t.hold_on && col(&t.state, slot, FREE) == CONFIRMED;
    let local;
    let cfg = if want == t.cfg.confirm_moves {
        &t.cfg
    } else {
        local = TrackerConfig {
            confirm_moves: want,
            ..t.cfg
        };
        &local
    };
    let mut verdict = filter_apply(&mut f, m, &mut t.stats, cfg);
    let moved = verdict == Applied::Large;
    if moved {
        // Applied as a tracker without the rule applies it, bit for bit.
        let plain = TrackerConfig {
            confirm_moves: false,
            ..t.cfg
        };
        verdict = filter_apply(&mut f, m, &mut t.stats, &plain);
    }
    match verdict {
        Applied::NumericalRefused => Step::NumericalRefused,
        Applied::Large => {
            // The filter reports a large move only with the rule on, and the second call has it
            // off: not reachable; a refusal, counted, never an application.
            t.stats.moves_internal_refused += 1;
            Step::NumericalRefused
        }
        Applied::Rejected => {
            // Keeps the count of rejections in a row, so repeated ones re-initialise.
            put(&mut t.cur, slot, f);
            Step::Rejected
        }
        Applied::Yes => {
            put(&mut t.cur, slot, f);
            note_category(t, slot, m);
            if !ring_push(t, slot, m) {
                return Step::NumericalRefused;
            }
            set_last(t, slot, m.toa_ns);
            let u = col(&t.updates, slot, 0).saturating_add(1);
            put(&mut t.updates, slot, u);
            let h = col(&t.hits, slot, 0).saturating_add(1);
            put(&mut t.hits, slot, h);
            maybe_confirm(t, slot, m.toa_ns);
            if moved {
                Step::Moved
            } else {
                Step::Applied
            }
        }
        Applied::Resync => {
            put(&mut t.cur, slot, f);
            put(&mut t.cat, slot, CategoryState::default());
            note_category(t, slot, m);
            ring_clear(t, slot);
            if !ring_push(t, slot, m) {
                return Step::NumericalRefused;
            }
            set_last(t, slot, m.toa_ns);
            put(&mut t.hits, slot, 1);
            put(&mut t.updates, slot, 1);
            Step::Resynced
        }
    }
}

/// Counts and emits what [`decide`] did for one measurement; false if it must be rolled back.
fn settle(t: &mut Tracker, slot: usize, step: Step, out: &mut TrackOutput) -> bool {
    match step {
        Step::NumericalRefused => return false,
        Step::Begun => emit_if_confirmed(t, slot, out),
        Step::Applied | Step::Resynced => {
            t.stats.applied += 1;
            emit_if_confirmed(t, slot, out);
        }
        Step::Moved => {
            t.stats.applied += 1;
            t.stats.moves_unconfirmed += 1;
            emit_if_confirmed(t, slot, out);
        }
        Step::Rejected => {}
    }
    true
}

/// What `m` is measured against by the move's decision: its distances from the track's predicted
/// position and from the old path's (the unconfirmed move's snapshot predicted the same way), at
/// the report's time, metres; whether the move has a snapshot; the time of the earlier report that
/// agreed with the old path. The old path's distance is not a number if the move has no snapshot
/// or the snapshot cannot be predicted (told apart by `has_old`).
fn move_geometry(t: &Tracker, slot: usize, m: &Measurement) -> Seen {
    let cur = col(&t.cur, slot, blank_filter());
    let snap = move_snap_ix(t, slot).and_then(|i| t.snaps.get(i));
    Seen {
        cur_m: filter_miss_m(&cur, m),
        old_m: snap.map_or(f64::NAN, |v| filter_miss_m(&v.cur, m)),
        has_old: snap.is_some(),
        old_vote_ns: col(&t.mv_old_vote_ns, slot, NO_VOTE_NS),
    }
}

/// Applies a measurement newer than or equal to the track's newest update, with the large-move
/// rule on (RT-239). If the track has an unconfirmed move, the measurement decides it
/// (`crate::confirm::resolve_decide`), but only if it is itself applied: the normal path runs
/// and the decision is committed by what it did, before the row of the report is made (so that
/// row is the decided track's). Confirmed: applied to the moved track. Rolled back, only for a
/// second report on the old path apart from the first (the first, an old-path vote, decides
/// nothing, RT-273): the track is put back as the snapshot held it and the measurement applied to
/// that; if it is then not applied, the rollback is undone and the move stays. An old path that
/// cannot be predicted agrees with nothing (RT-276): the move stands, marked. Undecided: the move stands once the
/// measurement is applied (if that measurement is a large move too, the track stays marked with
/// the original snapshot, the window running from the later move). A measurement that is not
/// applied decides nothing (`moves_decision_rejected`).
fn update_in_order_held(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) {
    let on = t.cfg.confirm_moves;
    if !(on && move_active(t, slot)) {
        t.hold_on = on;
        update_in_order(t, slot, m, out);
        t.hold_on = false;
        return;
    }
    let seen = move_geometry(t, slot, m);
    let start = col(&t.mv_start_ns, slot, 0);
    let window = confirm_window_ns(&t.cfg, col(&t.expected_ns, slot, 0));
    let verdict = resolve_decide(start, m.toa_ns, &seen, window, &t.cfg);
    match verdict {
        // Too soon to be a second report: applied as it is, the move stays unconfirmed.
        Resolve::Wait => {
            t.hold_on = false;
            update_in_order(t, slot, m, out);
        }
        // The first report on the old path: one report decides nothing (RT-273). Applied as it
        // is; the move stays, marked, and the report is the old path's vote. A report the gate
        // refused counts as the vote too (the moved track usually refuses it), also when it is
        // one of a run of refusals that is heading for a resynchronisation.
        Resolve::OldVote => {
            t.hold_on = false;
            let step = update_in_order(t, slot, m, out);
            if step != Step::NumericalRefused && move_active(t, slot) {
                put(&mut t.mv_old_vote_ns, slot, m.toa_ns);
                t.stats.moves_old_path_votes += 1;
            }
        }
        // An old path that cannot be predicted is no agreement with anything (RT-276): counted,
        // and the move stands, marked, until the window ends (a report agreeing with the moved
        // track does not confirm it meanwhile).
        Resolve::OldUnpredictable => {
            t.hold_on = false;
            // Counted when the report is applied; a refused one decides nothing and is counted
            // as the refusal it is (`moves_decision_rejected`).
            let step = update_in_order(t, slot, m, out);
            if !decision_rejected(t, slot, step) {
                t.stats.moves_old_path_unpredictable += 1;
            }
        }
        Resolve::Confirm => {
            t.hold_on = false;
            let step = update_in_order_ending(t, slot, m, out, Some(MoveEnd::Confirmed));
            decision_rejected(t, slot, step);
        }
        Resolve::Rollback => {
            let moved = snap_take(t, slot);
            if let Some(old) = move_snap_ix(t, slot).and_then(|i| t.snaps.get(i)).copied() {
                snap_restore(t, slot, &old);
            }
            t.hold_on = false;
            let step = update_in_order_ending(t, slot, m, out, Some(MoveEnd::RolledBack));
            if decision_rejected(t, slot, step) {
                // Not applied: the rollback is undone, the move stays as it was. The refused
                // report still counts in the moved track's run of refusals, as it would
                // without the rule (the run that re-synchronises a filter).
                snap_restore(t, slot, &moved);
                if step == Step::Rejected {
                    note_refusal_on_moved(t, slot, m);
                }
            }
        }
        Resolve::Undecided => {
            t.hold_on = true;
            let step = update_in_order_ending(t, slot, m, out, Some(MoveEnd::Undecided));
            if step == Step::Moved {
                // A large move of its own: the track stays marked with the original snapshot;
                // the first move's outcome is that it stands.
                t.stats.moves_undecided += 1;
            } else {
                decision_rejected(t, slot, step);
            }
        }
    }
    t.hold_on = false;
}

/// Counts the report `m`, refused after a rollback was tried, in the run of refusals of the
/// track as it stands (the moved one): the filter takes it as the rule-off tracker would, and
/// only a refusal is kept (an application or a restart is not: the move stays unconfirmed).
fn note_refusal_on_moved(t: &mut Tracker, slot: usize, m: &Measurement) {
    let mut f = col(&t.cur, slot, blank_filter());
    let plain = TrackerConfig {
        confirm_moves: false,
        ..t.cfg
    };
    if filter_apply(&mut f, m, &mut TrackerStats::default(), &plain) == Applied::Rejected {
        put(&mut t.cur, slot, f);
    }
}

/// Counts a deciding report that was not applied (its move is still unconfirmed): true then.
fn decision_rejected(t: &mut Tracker, slot: usize, step: Step) -> bool {
    if move_active(t, slot) && step != Step::Applied {
        t.stats.moves_decision_rejected += 1;
        return true;
    }
    false
}

/// Applies a measurement newer than or equal to the track's newest update. Returns what the
/// decision did (`NumericalRefused` if the update was rolled back).
fn update_in_order(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) -> Step {
    update_in_order_ending(t, slot, m, out, None)
}

/// [`update_in_order`] that ends the track's unconfirmed move in `ending` if the measurement is
/// applied, before the report's row is made.
fn update_in_order_ending(
    t: &mut Tracker,
    slot: usize,
    m: &Measurement,
    out: &mut TrackOutput,
    ending: Option<MoveEnd>,
) -> Step {
    let snapshot = track_txn(t, slot);
    let output = output_txn(out);
    let step = decide(t, slot, m);
    if step == Step::NumericalRefused {
        // Nothing of the move was touched: `decide` leaves it to this commit.
        restore_track_txn(t, slot, snapshot);
        restore_output_txn(out, output);
        return Step::NumericalRefused;
    }
    if matches!(
        step,
        Step::Applied | Step::Begun | Step::Resynced | Step::Moved
    ) {
        update_observed_ident(t, slot, m);
    }
    // Committed: the move's change goes with it, before the report's row is made.
    match (step, ending) {
        // The state from before the move is the rollback point (the update's own checkpoint);
        // every row from here on is marked.
        (Step::Moved, _) => start_move(t, slot, &snap_of_txn(&snapshot), m, m.toa_ns),
        (Step::Applied, Some(why)) => end_move(t, slot, why),
        // The track began again or the filter was re-synchronised: nothing of the move's
        // old path is left to compare with.
        (Step::Begun, _) => end_move(t, slot, MoveEnd::Restarted),
        (Step::Resynced, _) => end_move(t, slot, MoveEnd::Resync),
        _ => {}
    }
    // `settle` fails only for a refused step, which returned above.
    let _ = settle(t, slot, step, out);
    step
}

/// Retains the newest accepted source-observed Mode 3/A code. A missing code means no new
/// evidence: the prior code stays with its original timestamp so display callers can show its age
/// and never present it as a fresh current reading.
fn update_observed_ident(t: &mut Tracker, slot: usize, m: &Measurement) {
    if m.ident == NO_IDENT {
        return;
    }
    let known_ns = col(&t.ident_ns, slot, NO_IDENT_NS);
    if m.toa_ns > known_ns {
        put(&mut t.ident, slot, m.ident);
        put(&mut t.ident_ns, slot, m.toa_ns);
        put(&mut t.ident_conflict, slot, 0);
    } else if m.toa_ns == known_ns && col(&t.ident, slot, NO_IDENT) != m.ident {
        // Equal-time sensors disagree: no arrival-order winner is selected. A non-sentinel time
        // with NO_IDENT is the explicit same-time-conflict state until a later code is observed.
        if col(&t.ident_conflict, slot, 0) == 0 {
            t.stats.ident_conflicts = t.stats.ident_conflicts.saturating_add(1);
        }
        put(&mut t.ident, slot, NO_IDENT);
        put(&mut t.ident_conflict, slot, 1);
    }
}

fn emit_if_confirmed(t: &mut Tracker, slot: usize, out: &mut TrackOutput) {
    emit_if_confirmed_at(t, slot, col(&t.last_ns, slot, 0), out);
}

fn emit_if_confirmed_at(t: &mut Tracker, slot: usize, at_ns: i64, out: &mut TrackOutput) {
    if col(&t.state, slot, FREE) == CONFIRMED {
        emit(t, slot, at_ns, 0, out);
    }
}

/// Applies a measurement older than the track's newest update by re-filtering from the ring: the
/// track is put back as it was after the newest stored update at or before the measurement, and
/// the measurement and the stored ones after it are decided again in time order, through
/// [`decide`], as if they had arrived in that order. So restarts, confirmation, rejections and
/// resyncs do not depend on the order of arrival (while the ring holds the whole history since
/// the track began; past that the trusted category is the live one).
fn update_late(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) {
    let transaction = track_txn(t, slot);
    let output = output_txn(out);
    let len = ring_len(t, slot);
    // The newest stored update at or before the measurement (the state after an update is at
    // the update's own time).
    let mut base: Option<usize> = None;
    // KANI-LOOP-MARKER: late-replay-prefix
    for k in 0..len {
        let at = ring_ix(t, slot, k);
        if col(&t.ring_m, at, blank_measurement()).toa_ns <= m.toa_ns {
            base = Some(k);
        } else {
            break;
        }
    }
    let Some(e) = base else {
        t.stats.late_ring += 1;
        return;
    };
    if ring_has(t, slot, m.toa_ns, m.sensor) {
        t.stats.duplicates += 1;
        return;
    }
    // The measurement, then the stored ones after the base, in time order.
    let mut seq = [*m; RING_DEPTH + 1];
    let mut n_seq = 1usize;
    // KANI-LOOP-MARKER: late-replay-suffix
    for k in (e + 1)..len {
        let at = ring_ix(t, slot, k);
        if let Some(c) = seq.get_mut(n_seq) {
            *c = col(&t.ring_m, at, blank_measurement());
            n_seq += 1;
        }
    }
    // Restore live-filter rejected-report counters unless replay rejection, re-synchronization,
    // or restart means chronological replay state must stand.
    let live = col(&t.cur, slot, blank_filter());
    let (rej_h, rej_v) = (live.rej_h, live.rej_v);
    let live_reacq = live.reacq;
    let (rej_c, rej_t_ns, rej_at) = (
        live.rej_c,
        live.rej_t_ns,
        (live.rej_lat_e7, live.rej_lon_e7),
    );
    // The track as it was after the base entry: the filter, the counts, the category, and
    // tentative again if it was confirmed only by a later entry.
    let base_ns = col(&t.ring_m, ring_ix(t, slot, e), blank_measurement()).toa_ns;
    let Some(bf) = ring_state(t, slot, e) else {
        restore_track_txn(t, slot, transaction);
        restore_output_txn(out, output);
        return;
    };
    let kept = len - (e + 1);
    let updates = col(&t.updates, slot, 0);
    let mut cat = if (updates as usize) <= len {
        CategoryState::default()
    } else {
        // Entries older than the ring were dropped: their trusted category stands.
        CategoryState {
            trusted: col(&t.cat, slot, CategoryState::default()).trusted,
            ..CategoryState::default()
        }
    };
    // KANI-LOOP-MARKER: late-replay-category
    for k in 0..=e {
        let em = col(&t.ring_m, ring_ix(t, slot, k), blank_measurement());
        category_observe(&mut cat, em.category);
    }
    put(&mut t.cat, slot, cat);
    put(
        &mut t.updates,
        slot,
        updates.saturating_sub(kept as u32).max(1),
    );
    let hits = col(&t.hits, slot, 0);
    put(
        &mut t.hits,
        slot,
        hits.saturating_sub(kept.min(255) as u8).max(1),
    );
    if col(&t.state, slot, FREE) == CONFIRMED && col(&t.confirmed_ns, slot, i64::MIN) > base_ns {
        put(&mut t.state, slot, TENTATIVE);
        put(&mut t.confirmed_ns, slot, i64::MIN);
        t.stats.confirmed = t.stats.confirmed.saturating_sub(1);
    }
    put(&mut t.ring_len, slot, (e + 1) as u8);
    put(&mut t.cur, slot, bf);
    set_last(t, slot, base_ns);
    // `m` and then the stored later ones, in time order, through the in-order decision.
    // `n_seq` starts at one and increases only after a successful `get_mut`, so it cannot
    // exceed the fixed sequence array's length. Keep this explicit bounded walk simple for
    // symbolic verification; an unexpected missing entry fails closed via transaction restore.
    let mut disturbed = false;
    let mut k = 0;
    // KANI-LOOP-MARKER: late-replay-decisions
    while k < n_seq {
        let Some(cm) = seq.get(k) else {
            restore_track_txn(t, slot, transaction);
            restore_output_txn(out, output);
            return;
        };
        let step = decide(t, slot, cm);
        match step {
            Step::Applied if k == 0 => {
                t.stats.applied += 1;
                t.stats.refiltered += 1;
            }
            Step::Applied => t.stats.replayed += 1,
            Step::Resynced if k == 0 => {
                disturbed = true;
                t.stats.applied += 1;
                t.stats.refiltered += 1;
            }
            Step::Resynced | Step::Begun | Step::Rejected | Step::Moved => disturbed = true,
            Step::NumericalRefused => {
                restore_track_txn(t, slot, transaction);
                restore_output_txn(out, output);
                return;
            }
        }
        if matches!(
            step,
            Step::Applied | Step::Begun | Step::Resynced | Step::Moved
        ) {
            update_observed_ident(t, slot, cm);
        }
        if k == 0 && matches!(step, Step::Applied | Step::Begun | Step::Resynced) {
            emit_if_confirmed_at(t, slot, m.toa_ns, out);
            // Kani-only reachability evidence that later replay failure rolls back a real attempt.
            #[cfg(all(kani, kani_late_replay_covers))]
            {
                kani::cover!(
                    out.t_ns.len() > output.lens[0] || out.overflow > output.overflow,
                    "late replay attempts current report before later failure"
                );
            }
        }
        k += 1;
    }
    if !disturbed {
        let mut f = col(&t.cur, slot, blank_filter());
        f.rej_h = rej_h;
        f.rej_v = rej_v;
        (f.rej_c, f.rej_t_ns) = (rej_c, rej_t_ns);
        f.reacq |= live_reacq;
        (f.rej_lat_e7, f.rej_lon_e7) = rej_at;
        put(&mut t.cur, slot, f);
    }
}

/// The track's measurement state as it was before the ring entry `first` (time of applicability
/// and sensor): the state after the entry before it, derived as [`update_late`] derives the
/// state after its base entry (the anchor with the entries applied again, the counts and the
/// category from the entries held). Nothing is written. `None` if the entry is not in the ring,
/// is its oldest (nothing before it is held), the track is not confirmed, or a numerical
/// refusal: the rollback point cannot be derived then.
fn snap_before_entry(t: &Tracker, slot: usize, first: (i64, u16)) -> Option<TrackSnap> {
    let len = ring_len(t, slot);
    let at = |j: usize| col(&t.ring_m, ring_ix(t, slot, j), blank_measurement());
    let k = (0..len).find(|&j| (at(j).toa_ns, at(j).sensor) == first)?;
    let e = k.checked_sub(1)?;
    let base_ns = at(e).toa_ns;
    if col(&t.state, slot, FREE) != CONFIRMED || col(&t.confirmed_ns, slot, i64::MIN) > base_ns {
        return None;
    }
    let mut s = snap_take(t, slot);
    s.cur = ring_state(t, slot, e)?;
    let updates = col(&t.updates, slot, 0);
    let mut cat = if (updates as usize) <= len {
        CategoryState::default()
    } else {
        CategoryState {
            trusted: col(&t.cat, slot, CategoryState::default()).trusted,
            ..CategoryState::default()
        }
    };
    for j in 0..=e {
        category_observe(&mut cat, at(j).category);
    }
    let kept = len - (e + 1);
    s.cat = cat;
    s.updates = updates.saturating_sub(kept as u32).max(1);
    s.hits = col(&t.hits, slot, 0)
        .saturating_sub(kept.min(255) as u8)
        .max(1);
    s.ring_len = (e + 1) as u8;
    s.last_ns = base_ns;
    Some(s)
}

/// Marks the row `i` the late replay emitted (before its move started), and widens it like any
/// marked row: its radius at least the distance to the old path's position at the row's time plus
/// the old path's own radius (RT-275). The old path here is the track as it was before the late
/// report `m`, predicted to the row's time (derived from the ring as the rollback point is); if
/// that cannot be derived the row gets the fail-closed radius. Only this row changes, and
/// `rows_uncertain` follows its flag. Note which path is which: the widened row covers the
/// pre-report state, while the move's rollback target (its snapshot) is the pre-replay one, the
/// track as it was before the whole replay; the two differ when the replay changed other rows
/// before the late report.
fn widen_replay_row(
    t: &mut Tracker,
    slot: usize,
    out: &mut TrackOutput,
    i: usize,
    m: &Measurement,
) {
    let Some(mut row) = out_row_at(out, i) else {
        // Unreachable: `i` indexes a row the replay just emitted, and every column of the table
        // has that row. Not a fail-closed branch for that reason (there is nothing to widen).
        debug_assert!(false, "a replay row that is not in the output table");
        return;
    };
    let old = snap_before_entry(t, slot, (m.toa_ns, m.sensor))
        .and_then(|s| filter_output(&s.cur, row.t_ns));
    let mut r95 = r95_m(&row.pos_cov_m2);
    if !mark_row(&mut row, &mut r95, old.as_ref()) {
        t.stats.move_widen_failed += 1;
    }
    if let Some(c) = out.pos_cov_m2.get_mut(i) {
        *c = row.pos_cov_m2;
    }
    if r95 > t.cfg.coast_max_r95_m && row.flags & O_UNCERTAIN == 0 {
        row.flags |= O_UNCERTAIN;
        t.stats.rows_uncertain += 1;
    }
    if let Some(f) = out.flags.get_mut(i) {
        *f = row.flags;
    }
}

/// Row `i` of an output table as an [`OutRow`].
fn out_row_at(out: &TrackOutput, i: usize) -> Option<OutRow> {
    Some(OutRow {
        t_ns: *out.t_ns.get(i)?,
        addr: *out.addr.get(i)?,
        ident: *out.ident.get(i)?,
        ident_ns: *out.ident_ns.get(i)?,
        ident_conflict: *out.ident_conflict.get(i)?,
        lat_e7: *out.lat_e7.get(i)?,
        lon_e7: *out.lon_e7.get(i)?,
        vel_east_mps: *out.vel_east_mps.get(i)?,
        vel_north_mps: *out.vel_north_mps.get(i)?,
        pos_cov_m2: *out.pos_cov_m2.get(i)?,
        vel_cov_m2s2: *out.vel_cov_m2s2.get(i)?,
        turn_rate_rps: *out.turn_rate_rps.get(i)?,
        alt_ft: *out.alt_ft.get(i)?,
        alt_var_ft2: *out.alt_var_ft2.get(i)?,
        vrate_fpm: *out.vrate_fpm.get(i)?,
        mu_h: *out.mu_h.get(i)?,
        mu_v: *out.mu_v.get(i)?,
        flags: *out.flags.get(i)?,
    })
}

/// [`update_late`] with the large-move rule (RT-239) around it. A late replay that moves the
/// current estimate of a confirmed track by more than `confirm_move_m` from where it was marks
/// the track with the state from before the replay (`moves_late`, and the rows the replay
/// emitted). A replay while a move is unconfirmed makes its snapshot wrong for the new history:
/// the mark stays and the snapshot goes (`moves_rollback_lost_late`); a replay that demotes the
/// track to tentative ends the move. Kept apart from `update_late` so that proofs of the replay
/// are of the replay alone.
fn update_late_marked(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) {
    if !t.cfg.confirm_moves || col(&t.state, slot, FREE) != CONFIRMED {
        update_late(t, slot, m, out);
        return;
    }
    let before = snap_take(t, slot);
    let rows = out.t_ns.len();
    let (lr, dup, nr) = (
        t.stats.late_ring,
        t.stats.duplicates,
        t.stats.numerical_refused,
    );
    let (rs, iw, ip) = (
        t.stats.resyncs,
        t.stats.init_window_expired,
        t.stats.init_implausible,
    );
    update_late(t, slot, m, out);
    // Nothing was replayed (older than the ring, a duplicate, or rolled back): nothing changed,
    // the move included (the replay does not touch it).
    if (lr, dup, nr)
        != (
            t.stats.late_ring,
            t.stats.duplicates,
            t.stats.numerical_refused,
        )
    {
        return;
    }
    if move_active(t, slot) {
        if col(&t.state, slot, FREE) != CONFIRMED {
            end_move(t, slot, MoveEnd::Demoted);
        } else if t.stats.resyncs != rs {
            end_move(t, slot, MoveEnd::Resync);
        } else if (t.stats.init_window_expired, t.stats.init_implausible) != (iw, ip) {
            end_move(t, slot, MoveEnd::Restarted);
        } else if let Some(ix) = move_snap_ix(t, slot) {
            // The history changed: the rollback point is derived again, as the track was
            // before the report that made the move, from the ring now.
            let first = (col(&t.mv_first_ns, slot, 0), col(&t.mv_first_sens, slot, 0));
            match snap_before_entry(t, slot, first) {
                Some(s) => {
                    if let Some(e) = t.snaps.get_mut(ix) {
                        *e = s;
                    }
                }
                None => {
                    // The report has left the ring (or nothing before it is held).
                    release_snap(t, slot);
                    // No old path left to vote for.
                    put(&mut t.mv_old_vote_ns, slot, NO_VOTE_NS);
                    t.stats.moves_rollback_lost_late += 1;
                }
            }
        }
        return;
    }
    if col(&t.state, slot, FREE) != CONFIRMED {
        return;
    }
    let after = col(&t.cur, slot, blank_filter());
    let at = before.cur.t_ns.max(after.t_ns);
    let miss = match (filter_output(&before.cur, at), filter_output(&after, at)) {
        (Some(a), Some(b)) => filter_row_distance_m(&a, &b),
        _ => f64::NAN,
    };
    if move_is_large(miss, t.cfg.confirm_move_m) {
        start_move(t, slot, &before, m, col(&t.last_ns, slot, 0));
        t.stats.moves_unconfirmed += 1;
        t.stats.moves_late += 1;
        let key = col(&t.key, slot, NO_ADDR);
        for i in rows..out.t_ns.len() {
            if out.addr.get(i) == Some(&key) {
                widen_replay_row(t, slot, out, i, m);
            }
        }
    }
}

#[cfg(kani)]
mod proofs;

/// The first tick of the grid at or after `at_ns`. `at_ns` and the tick are bounded
/// (`TIME_LIMIT_NS`, `MAX_TICK_NS`): no overflow.
fn first_tick_at_or_after(t: &Tracker, at_ns: i64) -> i64 {
    i64::div_euclid(at_ns, t.cfg.tick_ns) * t.cfg.tick_ns
        + if i64::rem_euclid(at_ns, t.cfg.tick_ns) == 0 {
            0
        } else {
            t.cfg.tick_ns
        }
}

/// Moves the clock back to `to_ns`, the clock having been wrong-ahead: the tick grid restarts at
/// the first tick after it and every track updated after it, which was built on the wrong clock,
/// is dropped and counted.
fn step_back(t: &mut Tracker, to_ns: i64) {
    t.clock_ns = to_ns;
    t.next_tick_ns = first_tick_at_or_after(t, to_ns);
    t.stats.time_steps_back += 1;
    for slot in 0..t.high {
        if col(&t.state, slot, FREE) != FREE && col(&t.last_ns, slot, 0) > to_ns {
            end_track(t, slot, END_DROPPED);
            t.stats.time_future_dropped += 1;
        }
    }
    // The kept bound on the newest live update, from what is left.
    let mut newest = i64::MIN;
    for slot in 0..t.high {
        if col(&t.state, slot, FREE) != FREE {
            newest = newest.max(col(&t.last_ns, slot, 0));
        }
    }
    t.newest_update_ns = newest;
}

/// Offers one measurement. Counters say what became of it; rows for a confirmed track go to
/// `out`.
///
/// The clock decision is `crate::time_gate`. After a step back (counted in `time_steps_back`)
/// the rows that follow carry earlier times than rows already emitted: a consumer must expect
/// the time of the output to go backwards once, by as much as the clock had been moved ahead.
pub fn tracker_update(t: &mut Tracker, m: &Measurement, out: &mut TrackOutput) {
    t.stats.offered += 1;
    if let Err(why) = measurement_check(m) {
        if let Some(c) = t.stats.refused.get_mut(why) {
            *c += 1;
        }
        return;
    }
    let toa = m.toa_ns;
    let gate_cfg = GateCfg {
        limit_ns: t.cfg.clock_step_limit_ns,
        ooo_ns: t.cfg.ooo_window_ns,
        drop_ns: t.cfg.drop_after_ns,
    };
    // Every live track is this address (or there is none): only its own track is at stake.
    let own_addr_only = t.active == 0 || (t.active == 1 && lookup(t, m.addr).is_some());
    let verdict = gate_decide(
        &mut t.gate,
        &gate_cfg,
        t.clock_ns,
        toa,
        m.addr,
        m.sensor,
        t.active,
        own_addr_only,
    );
    t.stats.time_sensor_evicted = t.gate.evicted;
    match verdict {
        Verdict::Late => {
            t.stats.late_window += 1;
        }
        // A far-ahead measurement alone could expire every other track and make the tick loop run
        // over the whole gap: it waits, with the others of its run, for the run that confirms it.
        Verdict::Held => {
            t.stats.time_held += 1;
            if gate_run_restarted(&t.gate) {
                t.held_n = 0;
            }
            held_push(t, m);
        }
        Verdict::StepForward => {
            t.stats.time_steps += 1;
            if gate_run_restarted(&t.gate) {
                t.held_n = 0;
            }
            // Confirmation delays the measurements of the run, it does not lose them: they are
            // applied in arrival order at the new clock, before this one.
            let n = t.held_n;
            t.held_n = 0;
            for i in 0..n {
                let h = col(&t.held, i, blank_measurement());
                gate_note_applied(&mut t.gate, h.sensor, h.toa_ns);
                t.stats.time_held_applied += 1;
                apply(t, &h, out);
            }
            apply(t, m, out);
        }
        Verdict::StepBack { to_ns } => {
            t.held_n = 0;
            step_back(t, to_ns);
            apply(t, m, out);
        }
        Verdict::Accept => {
            t.held_n = 0;
            apply(t, m, out);
        }
    }
}

/// Keeps a held measurement; past the buffer the oldest is discarded.
fn held_push(t: &mut Tracker, m: &Measurement) {
    // An exact repeat of a held measurement is not held again.
    let repeat = t
        .held
        .iter()
        .take(t.held_n)
        .any(|h| h.toa_ns == m.toa_ns && h.addr == m.addr && h.sensor == m.sensor);
    if repeat {
        return;
    }
    if t.held_n >= t.held.len() {
        t.held.rotate_left(1);
        t.held_n = t.held.len() - 1;
    }
    put(&mut t.held, t.held_n, *m);
    t.held_n += 1;
}

/// Applies a measurement the gate let through: moves the clock forward if it is newer, then the
/// track's own ordering rules (in order, duplicate, late) decide.
fn apply(t: &mut Tracker, m: &Measurement, out: &mut TrackOutput) {
    let toa = m.toa_ns;
    // After a step the measurement (or a held one) may be older than the new clock's window.
    if t.clock_ns != i64::MIN && toa < t.clock_ns.saturating_sub(t.cfg.ooo_window_ns) {
        t.stats.late_window += 1;
        return;
    }
    if toa > t.clock_ns {
        if t.next_tick_ns == i64::MIN {
            t.next_tick_ns = first_tick_at_or_after(t, toa);
        }
        emit_ticks(t, toa - 1, out);
        t.clock_ns = toa;
    }
    match lookup(t, m.addr) {
        None => {
            let Some(slot) = t.free.pop() else {
                t.stats.capacity_refused += 1;
                return;
            };
            let slot = slot as usize;
            put(&mut t.key, slot, m.addr);
            index_insert(t, m.addr, slot as u32);
            t.high = t.high.max(slot + 1);
            t.active += 1;
            t.stats.started += 1;
            t.stats.applied += 1;
            if !track_begin(t, slot, m) {
                t.stats.numerical_refused += 1;
                return;
            }
            emit_if_confirmed(t, slot, out);
        }
        Some(slot) => {
            let last = col(&t.last_ns, slot, 0);
            if toa > last {
                update_in_order_held(t, slot, m, out);
            } else if toa == last && ring_has(t, slot, toa, m.sensor) {
                t.stats.duplicates += 1;
            } else if toa == last {
                update_in_order_held(t, slot, m, out);
            } else {
                update_late_marked(t, slot, m, out);
            }
        }
    }
}

/// Drops and expires tracks that are too old at `now_ns`.
fn sweep(t: &mut Tracker, now_ns: i64) {
    let window = init_window_ns(t);
    for slot in 0..t.high {
        let st = col(&t.state, slot, FREE);
        if st == FREE {
            continue;
        }
        let age = now_ns - col(&t.last_ns, slot, 0);
        if st == TENTATIVE && age > window {
            end_track(t, slot, END_TENTATIVE);
        } else if age > t.cfg.drop_after_ns {
            end_track(t, slot, END_DROPPED);
        }
    }
}

/// The ticks from the next one to `emit_to`, when the output table is full: none of their rows
/// can be taken, so none is computed. The rows lost are counted in the table's `overflow`, from
/// each confirmed track's own last tick (the drop age), one pass over the tracks and no
/// work per tick. The tracks are then swept at the last of the ticks.
fn skip_ticks_full(t: &mut Tracker, emit_to: i64, out: &mut TrackOutput) {
    let tick_ns = i128::from(t.cfg.tick_ns);
    let first = i128::from(t.next_tick_ns);
    let ticks = (i128::from(emit_to) - first) / tick_ns + 1;
    let mut rows: i128 = 0;
    for slot in 0..t.high {
        if col(&t.state, slot, FREE) != CONFIRMED {
            continue;
        }
        // The track is on the table through the tick at which its age is the drop age.
        let end = (i128::from(col(&t.last_ns, slot, 0)) + i128::from(t.cfg.drop_after_ns))
            .min(i128::from(emit_to));
        if end >= first {
            rows += (end - first) / tick_ns + 1;
        }
    }
    out.overflow = out
        .overflow
        .saturating_add(u64::try_from(rows).unwrap_or(u64::MAX));
    t.stats.ticks = t
        .stats
        .ticks
        .saturating_add(u64::try_from(ticks).unwrap_or(u64::MAX));
    let last = first + (ticks - 1) * tick_ns;
    let next = last + tick_ns;
    // Both are within the bounded time range (`MAX_TICK_NS`, `TIME_LIMIT_NS`).
    t.next_tick_ns = i64::try_from(next).unwrap_or(i64::MAX);
    sweep(t, i64::try_from(last).unwrap_or(i64::MAX));
}

/// Emits every tick up to and including `limit_ns`.
fn emit_ticks(t: &mut Tracker, limit_ns: i64, out: &mut TrackOutput) {
    if t.next_tick_ns == i64::MIN {
        return;
    }
    // Past the newest update plus the drop age every track is gone and a tick emits nothing: the
    // ticks after that are jumped over, with one sweep, so a long gap costs no more than the
    // drop age does. The newest update is the kept bound, not a scan of the slots: it can be
    // newer than any live track's (that one ended), which jumps less, never over a live row.
    let useful = if t.newest_update_ns == i64::MIN {
        i64::MIN
    } else {
        t.newest_update_ns.saturating_add(t.cfg.drop_after_ns)
    };
    let emit_to = limit_ns.min(useful);
    while t.next_tick_ns <= emit_to {
        if out.t_ns.len() >= out.capacity {
            // No row can be taken for the rest of this advance: count them, compute none.
            skip_ticks_full(t, emit_to, out);
            break;
        }
        let tick = t.next_tick_ns;
        sweep(t, tick);
        for slot in 0..t.high {
            if col(&t.state, slot, FREE) != CONFIRMED {
                continue;
            }
            if out.t_ns.len() >= out.capacity {
                // Full part-way through the tick: the rest of its rows are lost, uncomputed.
                out.overflow = out.overflow.saturating_add(1);
                continue;
            }
            let coast = track_is_coasting(t, slot, tick);
            emit(t, slot, tick, O_TICK | if coast { O_COAST } else { 0 }, out);
        }
        t.stats.ticks += 1;
        t.next_tick_ns = tick.saturating_add(t.cfg.tick_ns);
        if t.next_tick_ns <= tick {
            break;
        }
    }
    if t.next_tick_ns <= limit_ns {
        sweep(t, limit_ns);
        let skipped = (limit_ns - t.next_tick_ns) / t.cfg.tick_ns + 1;
        t.stats.ticks_skipped += skipped as u64;
        t.next_tick_ns = t
            .next_tick_ns
            .saturating_add(skipped.saturating_mul(t.cfg.tick_ns));
    }
}

/// Counts and refuses an invalid clock value before the caller changes any other state.
#[doc(hidden)]
pub fn check_advance_time(t: &mut Tracker, now_ns: i64) -> bool {
    if now_ns.unsigned_abs() > TIME_LIMIT_NS as u64 {
        if let Some(c) = t.stats.refused.get_mut(REF_TIME) {
            *c += 1;
        }
        return false;
    }
    true
}

/// Advances time to `now_ns` without a measurement: emits the ticks up to it and expires old
/// tracks. An equal-time call drains a tick deferred by a measurement at that exact timestamp;
/// after that tick is consumed, another equal-time call emits nothing. Time never goes backwards;
/// an earlier `now_ns` does nothing. The step limit does not apply here: the caller's clock (the
/// replay clock) has its own confirmed rules.
pub fn tracker_advance(t: &mut Tracker, now_ns: i64, out: &mut TrackOutput) {
    if !check_advance_time(t, now_ns) {
        return;
    }
    if now_ns >= t.clock_ns {
        if t.next_tick_ns == i64::MIN {
            return;
        }
        emit_ticks(t, now_ns, out);
        t.clock_ns = now_ns;
    }
}

/// Emits one current-time snapshot of each confirmed live track at a non-grid frame cutoff.
/// Call after [`tracker_advance`]; snapshots use the same filter, coast, uncertainty, move,
/// identity and disagreement logic as periodic output, but do not claim `O_TICK` or consume
/// reacquisition state. The stored measurement time is unchanged. Returns false without emitting
/// a row unless `now_ns` is the tracker's already-advanced, non-grid clock. A refused call does
/// not change source time or reacquisition state; refusal accounting may still be updated.
pub fn tracker_snapshot_at(t: &mut Tracker, now_ns: i64, out: &mut TrackOutput) -> bool {
    if now_ns != t.clock_ns || i64::rem_euclid(now_ns, t.cfg.tick_ns) == 0 {
        return false;
    }
    if !check_advance_time(t, now_ns) {
        return false;
    }
    sweep(t, now_ns);
    for slot in 0..t.high {
        if col(&t.state, slot, FREE) == CONFIRMED {
            emit_snapshot(t, slot, now_ns, out);
        }
    }
    true
}

/// Ends the run at `now_ns`: emits the remaining ticks and logs every live track as flushed.
pub fn tracker_flush(t: &mut Tracker, now_ns: i64, out: &mut TrackOutput) {
    if !check_advance_time(t, now_ns) {
        return;
    }
    tracker_advance(t, now_ns, out);
    for slot in 0..t.high {
        if col(&t.state, slot, FREE) != FREE {
            end_track(t, slot, END_FLUSHED);
        }
    }
}

/// The address and newest-update time of every live confirmed track, in slot order, for the
/// caller's inspection (tests, reports).
pub fn tracker_live(t: &Tracker) -> Vec<(u32, i64)> {
    tracker_live_iter(t).collect()
}

/// [`tracker_live`] without building a vector: the runtime path walks the tracks with it.
pub fn tracker_live_iter(t: &Tracker) -> impl Iterator<Item = (u32, i64)> + '_ {
    (0..t.high)
        .filter(|&s| col(&t.state, s, FREE) == CONFIRMED)
        .map(|s| (col(&t.key, s, NO_ADDR), col(&t.last_ns, s, 0)))
}

/// A live track as the fusion layer sees it: the track keyed `key` predicted to `at_ns`, whether
/// it is confirmed, and its newest update time. `None` if there is no such track, or if its position at `at_ns`
/// cannot be represented. Nothing is stored; the filter is predicted on a copy, as for an output row.
/// The row is as an output row is for display: a track with an unconfirmed move has the flag and
/// the widened covariance (see [`tracker_forecast`]).
pub fn tracker_predict(t: &Tracker, key: u32, at_ns: i64) -> Option<(OutRow, bool, i64)> {
    let (mut row, confirmed, last, wide) = tracker_forecast(t, key, at_ns).ok()?;
    if let Some(w) = wide {
        row.pos_cov_m2 = w;
    }
    Some((row, confirmed, last))
}

/// Forecast with a typed reason for absence or failure, for association: the row carries the
/// filter's own covariance, and for a track with an unconfirmed move the flag too. Earlier query
/// times use the newest stored state, as in `tracker_predict`. This read-only operation does not
/// change counters; callers that discard work must record numerical or unrepresentable
/// failures separately.
pub fn tracker_predict_checked(
    t: &Tracker,
    key: u32,
    at_ns: i64,
) -> Result<(OutRow, bool, i64), PredictError> {
    tracker_forecast(t, key, at_ns).map(|(row, confirmed, last, _)| (row, confirmed, last))
}

/// Forecasts only the row fields radar association reads. This avoids presentation-only velocity
/// covariance and move-display work while preserving the exact position and altitude gate inputs.
#[doc(hidden)]
pub fn tracker_predict_for_association(
    t: &Tracker,
    key: u32,
    at_ns: i64,
) -> Result<(OutRow, i64), PredictError> {
    let slot = lookup(t, key).ok_or(PredictError::NoTrack)?;
    if col(&t.state, slot, FREE) == FREE {
        return Err(PredictError::NoTrack);
    }
    let f = col(&t.cur, slot, blank_filter());
    let mut row = filter_association_output_checked(&f, at_ns.max(f.t_ns))?;
    row.addr = key;
    if key >= RADAR_KEY_BASE {
        row.flags |= O_IDENTITY_UNCONFIRMED;
    }
    Ok((row, col(&t.last_ns, slot, 0)))
}

/// Plain precise-altitude forecast data for a caller-side association gate.
pub type AltitudeForecast = (f32, f32, i64);

/// Returns altitude, variance and last-update time when a precise vertical forecast is available.
/// Predicate/application policy belongs to the caller; this core API returns only data.
#[doc(hidden)]
pub fn tracker_altitude_forecast(
    t: &Tracker,
    key: u32,
    at_ns: i64,
) -> Result<Option<AltitudeForecast>, PredictError> {
    let slot = lookup(t, key).ok_or(PredictError::NoTrack)?;
    if col(&t.state, slot, FREE) == FREE {
        return Err(PredictError::NoTrack);
    }
    let f = col(&t.cur, slot, blank_filter());
    Ok(filter_altitude_forecast(&f, at_ns.max(f.t_ns))?
        .map(|(altitude, variance)| (altitude, variance, col(&t.last_ns, slot, 0))))
}

/// A forecast: the row, whether the track is confirmed, its newest update time, and the widened
/// covariance of a track with an unconfirmed move.
pub type Forecast = (OutRow, bool, i64, Option<[f32; 3]>);

/// A forecast with both covariances: the row has the filter's own (what association and the
/// radar's disagreement test must use, so that a forged move can still be refuted by a plot on
/// the old path), and for a track with an unconfirmed move the flag and, as the last element,
/// the widened covariance for output and display (`None` if there is no move). A failure to
/// widen is fail closed here, not counted: the call is read only.
pub fn tracker_forecast(t: &Tracker, key: u32, at_ns: i64) -> Result<Forecast, PredictError> {
    let slot = lookup(t, key).ok_or(PredictError::NoTrack)?;
    if col(&t.state, slot, FREE) == FREE {
        return Err(PredictError::NoTrack);
    }
    let f = col(&t.cur, slot, blank_filter());
    let mut row = filter_output_checked(&f, at_ns.max(f.t_ns))?;
    row.addr = key;
    if key >= RADAR_KEY_BASE {
        row.flags |= O_IDENTITY_UNCONFIRMED;
    }
    let mut wide = None;
    if move_active(t, slot) {
        let mut marked = row;
        let mut r95 = r95_m(&row.pos_cov_m2);
        let old = old_path_row(t, slot, at_ns.max(f.t_ns));
        mark_row(&mut marked, &mut r95, old.as_ref());
        row.flags |= O_MOVE_UNCONFIRMED;
        wide = Some(marked.pos_cov_m2);
    }
    Ok((
        row,
        col(&t.state, slot, FREE) == CONFIRMED,
        col(&t.last_ns, slot, 0),
        wide,
    ))
}

/// Existence is a key lookup, independent of whether a forecast can be computed.
#[doc(hidden)]
pub fn tracker_has_key(t: &Tracker, key: u32) -> bool {
    lookup(t, key).is_some_and(|slot| col(&t.state, slot, FREE) != FREE)
}

/// Whether a keyed track is confirmed, independent of forecastability.
#[doc(hidden)]
pub fn tracker_is_confirmed(t: &Tracker, key: u32) -> bool {
    lookup(t, key).is_some_and(|slot| col(&t.state, slot, FREE) == CONFIRMED)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::params::CONFIRM_POOL;
    use crate::types::{output_new, DEFAULT_CONFIG, M_ALT_GEOMETRIC, M_HAS_VEL, O_HAS_ALT};

    const S: i64 = NS_PER_S;
    const ADDR: u32 = 0xABC123;

    fn cfg(init_m: u8, h_mode: u8) -> TrackerConfig {
        TrackerConfig {
            max_tracks: 8,
            out_capacity: 1 << 12,
            log_capacity: 64,
            init_m,
            init_n: 3,
            h_mode,
            ..DEFAULT_CONFIG
        }
    }

    /// A position-only report at `t_s` seconds, `east_m` metres east of (0, 0) on the equator and
    /// `lon_e7` further, with position variance `var`.
    fn at(t_s: i64, lon_e7: i32, var: f32) -> Measurement {
        Measurement {
            toa_ns: 1_000 * S + t_s * S,
            pos_cov_m2: [var, 0.0, var],
            lon_e7,
            addr: ADDR,
            sensor: 1,
            ..blank_measurement()
        }
    }

    #[test]
    fn observed_mode_a_updates_conflicts_ages_and_clears_with_track_lifecycle() {
        let mut t = tracker_new(&cfg(2, 0)).unwrap();
        let mut observed = at(0, 0, 100.0);
        observed.ident = 0o0000;
        track_begin(&mut t, 0, &observed);
        update_observed_ident(&mut t, 0, &observed);
        assert_eq!(
            (t.ident[0], t.ident_ns[0], t.ident_conflict[0]),
            (0, observed.toa_ns, 0)
        );

        let missing = at(1, 0, 100.0);
        update_observed_ident(&mut t, 0, &missing);
        assert_eq!((t.ident[0], t.ident_ns[0]), (0, observed.toa_ns));

        let mut conflict = observed;
        conflict.ident = 0o0123;
        update_observed_ident(&mut t, 0, &conflict);
        assert_eq!((t.ident[0], t.ident_conflict[0]), (NO_IDENT, 1));
        assert_eq!(t.stats.ident_conflicts, 1);

        let mut reverse = tracker_new(&cfg(2, 0)).unwrap();
        track_begin(&mut reverse, 0, &observed);
        update_observed_ident(&mut reverse, 0, &conflict);
        update_observed_ident(&mut reverse, 0, &observed);
        assert_eq!(
            (
                reverse.ident[0],
                reverse.ident_ns[0],
                reverse.ident_conflict[0]
            ),
            (NO_IDENT, observed.toa_ns, 1)
        );
        assert_eq!(reverse.stats.ident_conflicts, 1);

        let mut newer = at(2, 0, 100.0);
        newer.ident = 0o4567;
        update_observed_ident(&mut t, 0, &newer);
        assert_eq!(
            (t.ident[0], t.ident_ns[0], t.ident_conflict[0]),
            (0o4567, newer.toa_ns, 0)
        );

        let no_code = at(3, 0, 100.0);
        track_begin(&mut t, 0, &no_code);
        assert_eq!(
            (t.ident[0], t.ident_ns[0], t.ident_conflict[0]),
            (NO_IDENT, NO_IDENT_NS, 0)
        );
    }

    fn baro(t_ns: i64, altitude_ft: i32, rate_fpm: i32) -> Measurement {
        Measurement {
            toa_ns: t_ns,
            alt_ft: altitude_ft,
            vrate_fpm: rate_fpm,
            alt_quant_ft: 25,
            ..at(0, 0, 100.0)
        }
    }

    // REQ: OA-TRK-069
    #[test]
    fn tick_ceiling_uses_euclidean_boundaries_on_both_sides_of_epoch() {
        let t = tracker_new(&cfg(2, 0)).unwrap();
        let step = t.cfg.tick_ns;
        assert!(step > 0);
        assert_eq!(first_tick_at_or_after(&t, -step - 1), -step);
        assert_eq!(first_tick_at_or_after(&t, -step), -step);
        assert_eq!(first_tick_at_or_after(&t, -1), 0);
        assert_eq!(first_tick_at_or_after(&t, 0), 0);
        assert_eq!(first_tick_at_or_after(&t, step + 1), 2 * step);
    }

    // REQ: OA-TRK-017
    #[test]
    fn all_tracker_buffer_capacities_stay_fixed_through_bounded_updates_and_flush() {
        let mut c = cfg(2, 0);
        c.out_capacity = 16;
        c.log_capacity = 3;
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        let before = tracker_buffer_capacities(&t);

        for second in 0..40 {
            for aircraft in 0..12u32 {
                let mut m = at(second, aircraft as i32 * 1_000, 100.0);
                m.addr = 0x100000 + aircraft;
                tracker_update(&mut t, &m, &mut out);
            }
        }
        let mut late = at(39, 7_000, 100.0);
        late.addr = 0x100007;
        late.toa_ns -= S / 2;
        tracker_update(&mut t, &late, &mut out);
        tracker_advance(&mut t, 1_100 * S, &mut out);
        for second in 100..102 {
            for aircraft in 0..12u32 {
                let mut m = at(second, aircraft as i32 * 1_000, 100.0);
                m.addr = 0x100000 + aircraft;
                tracker_update(&mut t, &m, &mut out);
            }
        }
        tracker_advance(&mut t, 1_200 * S, &mut out);
        tracker_flush(&mut t, 1_201 * S, &mut out);

        assert!(out.overflow > 0);
        assert!(t.log.overflow > 0);
        assert!(t.stats.capacity_refused > 0);
        assert!(t.stats.refiltered > 0);
        assert_eq!(before.len(), 36);
        assert_eq!(before, tracker_buffer_capacities(&t));
    }

    // Kills mutation: tracker_advance skips the tick deferred at an update's exact timestamp, or
    // emits that tick more than once when called again at the same time.
    // REQ: OA-TRK-338
    #[test]
    fn equal_time_advance_drains_pending_tick_once() {
        let c = cfg(1, 0);
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        let boundary = at(0, 0, 100.0);

        tracker_update(&mut t, &boundary, &mut out);
        crate::types::output_clear(&mut out);
        tracker_advance(&mut t, boundary.toa_ns - 1, &mut out);
        assert!(out.t_ns.is_empty(), "earlier time must remain a no-op");

        tracker_advance(&mut t, boundary.toa_ns, &mut out);
        assert_eq!(out.t_ns.len(), 1);
        assert_eq!(out.t_ns.first(), Some(&boundary.toa_ns));
        assert!(out.flags.first().is_some_and(|flags| flags & O_TICK != 0));

        tracker_advance(&mut t, boundary.toa_ns, &mut out);
        assert_eq!(out.t_ns.len(), 1, "equal-time tick must be idempotent");
    }

    // Kills mutation: omit a frame-current projection between whole-second tracker ticks, or
    // advance the source timestamp/reacquisition state to make that projection look fresh.
    // REQ: OA-TRK-013, OA-TRK-343, OA-TRK-012
    #[test]
    fn non_grid_snapshot_projects_current_state_without_refreshing_source_or_reacquisition() {
        let mut c = cfg(2, 0);
        c.coast_after_intervals = 1;
        c.coast_max_r95_m = 1.0;
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        let mut first = at(0, 0, 100.0);
        first.flags |= M_HAS_VEL;
        first.ident = 0;
        first.vel_east_mps = 10.0;
        first.vel_cov_m2s2 = [1.0, 0.0, 1.0];
        first.vel_lag_s = 0.0;
        tracker_update(&mut t, &first, &mut out);
        let mut second = at(1, 900, 100.0);
        second.flags |= M_HAS_VEL;
        second.ident = 0;
        second.vel_east_mps = 10.0;
        second.vel_cov_m2s2 = [1.0, 0.0, 1.0];
        second.vel_lag_s = 0.0;
        tracker_update(&mut t, &second, &mut out);
        assert!(tracker_is_confirmed(&t, ADDR));
        assert!(tracker_set_expected_interval(&mut t, ADDR, S / 10));

        let now_ns = second.toa_ns + 3 * S / 5;
        tracker_advance(&mut t, now_ns, &mut out);
        crate::types::output_clear(&mut out);
        let source_ns = tracker_live_iter(&t)
            .find(|(address, _)| *address == ADDR)
            .map(|(_, last_ns)| last_ns)
            .unwrap();
        let slot = lookup(&t, ADDR).unwrap();
        let reacq_before = col(&t.cur, slot, blank_filter()).reacq;
        assert_eq!(source_ns, second.toa_ns);

        assert!(!tracker_snapshot_at(&mut t, now_ns + S / 10, &mut out));
        assert!(out.t_ns.is_empty());
        assert_eq!(
            tracker_live_iter(&t)
                .find(|(address, _)| *address == ADDR)
                .unwrap()
                .1,
            source_ns
        );
        assert_eq!(col(&t.cur, slot, blank_filter()).reacq, reacq_before);

        assert!(tracker_snapshot_at(&mut t, now_ns, &mut out));
        let index = out.t_ns.iter().position(|&time| time == now_ns).unwrap();
        assert_eq!(out.addr.get(index), Some(&ADDR));
        assert_eq!(out.flags.get(index).copied().unwrap_or(0) & O_TICK, 0);
        assert_ne!(out.flags.get(index).copied().unwrap_or(0) & O_COAST, 0);
        assert_ne!(out.flags.get(index).copied().unwrap_or(0) & O_UNCERTAIN, 0);
        assert_ne!(out.lon_e7.get(index), Some(&second.lon_e7));
        assert_eq!(out.ident.get(index), Some(&0));
        assert_eq!(out.ident_ns.get(index), Some(&second.toa_ns));
        assert_eq!(
            tracker_live_iter(&t)
                .find(|(address, _)| *address == ADDR)
                .unwrap()
                .1,
            source_ns
        );
        assert_eq!(col(&t.cur, slot, blank_filter()).reacq, reacq_before);

        let mut full = output_new(1);
        let existing = tracker_predict(&t, ADDR, now_ns).unwrap().0;
        output_push(&mut full, &existing);
        assert!(tracker_snapshot_at(&mut t, now_ns, &mut full));
        assert_eq!(full.overflow, 1);
        assert_eq!(full.t_ns.len(), 1);
    }

    // Kills reversing the pending-reacquisition condition in the Snapshot output path.
    // REQ: OA-TRK-343, OA-TRK-344
    #[test]
    fn snapshot_flags_only_a_pending_reacquisition() {
        let mut uncertain_config = cfg(2, 0);
        uncertain_config.coast_max_r95_m = 1.0;
        let mut uncertain = tracker_new(&uncertain_config).unwrap();
        let mut out = output_new(uncertain_config.out_capacity);
        tracker_update(&mut uncertain, &at(0, 0, 100.0), &mut out);
        tracker_update(&mut uncertain, &at(1, 20, 100.0), &mut out);
        assert!(tracker_is_confirmed(&uncertain, ADDR));
        crate::types::output_clear(&mut out);

        let snapshot_ns = at(1, 20, 100.0).toa_ns + S / 2;
        tracker_advance(&mut uncertain, snapshot_ns, &mut out);
        crate::types::output_clear(&mut out);
        let uncertain_slot = lookup(&uncertain, ADDR).expect("confirmed track remains live");
        assert_ne!(col(&uncertain.cur, uncertain_slot, blank_filter()).reacq, 0);
        assert!(tracker_snapshot_at(&mut uncertain, snapshot_ns, &mut out));
        assert_eq!(out.t_ns.len(), 1);
        assert_ne!(out.flags[0] & O_REACQUIRED, 0);
        assert_eq!(out.flags[0] & O_TICK, 0);
        assert_ne!(col(&uncertain.cur, uncertain_slot, blank_filter()).reacq, 0);

        let mut certain_config = cfg(2, 0);
        certain_config.coast_max_r95_m = 1_000.0;
        let mut certain = tracker_new(&certain_config).unwrap();
        let mut certain_out = output_new(certain_config.out_capacity);
        tracker_update(&mut certain, &at(0, 0, 100.0), &mut certain_out);
        tracker_update(&mut certain, &at(1, 20, 100.0), &mut certain_out);
        assert!(tracker_is_confirmed(&certain, ADDR));
        crate::types::output_clear(&mut certain_out);

        tracker_advance(&mut certain, snapshot_ns, &mut certain_out);
        crate::types::output_clear(&mut certain_out);
        let certain_slot = lookup(&certain, ADDR).expect("confirmed track remains live");
        assert_eq!(col(&certain.cur, certain_slot, blank_filter()).reacq, 0);
        assert!(tracker_snapshot_at(
            &mut certain,
            snapshot_ns,
            &mut certain_out
        ));
        assert_eq!(certain_out.t_ns.len(), 1);
        assert_eq!(certain_out.flags[0] & O_REACQUIRED, 0);
        assert_eq!(certain_out.flags[0] & O_TICK, 0);
        assert_eq!(col(&certain.cur, certain_slot, blank_filter()).reacq, 0);
    }

    // REQ: OA-TRK-042
    #[test]
    fn initialized_payload_accounting_scales_without_claiming_heap_bytes() {
        let payload = |max_tracks: usize, confirm_pool: usize| {
            let t = tracker_new(&TrackerConfig {
                max_tracks,
                out_capacity: 16,
                log_capacity: 16,
                confirm_pool,
                ..DEFAULT_CONFIG
            })
            .unwrap();
            tracker_payload_bytes_theoretical(&t)
        };
        let (small, large) = (payload(4096, 0), payload(38_000, 0));
        let per_track = |bytes: usize, tracks: usize| bytes as f64 / tracks as f64;
        assert!((per_track(small, 4096) - per_track(large, 38_000)).abs() < 12.0);
        // With a 12-entry ring, the pre-A7.1 per-track vectors totaled 2,295 bytes: two 664-byte
        // filters, twelve 72-byte measurements, and 103 bytes of other columns/free-list entries.
        // A7.1 adds 11 bytes per track for observed Mode A (u16), source time (i64), and conflict
        // state (u8).
        // The 38,000-track index rounds 76,000 entries to 131,072 (13.80 bytes per track, under
        // the strict 16-byte-per-track bound). The fixed 8-row hold buffer and 16-entry log add
        // under 0.04 bytes per track; the rollback pool is explicitly zero here. This is a bound
        // on initialized Vec element payload, not allocator reservation or process memory.
        assert!(per_track(large, 38_000) < 2_323.0);
        assert_eq!(
            payload(4096, CONFIRM_POOL) - small,
            CONFIRM_POOL * (core::mem::size_of::<TrackSnap>() + core::mem::size_of::<u32>())
        );
    }

    // REQ: OA-TRK-355
    #[test]
    fn contradictory_barometric_rate_is_discarded_without_losing_altitude() {
        let c = cfg(1, 0);
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..8 {
            tracker_update(&mut t, &baro(k * S / 2, 2_700, -15_000), &mut out);
        }
        let (row, _, _) = tracker_predict(&t, ADDR, 4 * S).unwrap();
        assert!(row.flags & O_HAS_ALT != 0);
        assert!((row.alt_ft - 2_700.0).abs() < 100.0);
        assert!(row.vrate_fpm.abs() < 1_000.0);
    }

    // REQ: OA-TRK-355
    #[test]
    fn coherent_rapid_barometric_descent_keeps_its_rate() {
        let c = cfg(1, 0);
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        // At 10 Hz, the displacement from just one interval is inside measurement
        // uncertainty. A retained anchor lets subsequent reports corroborate the descent.
        for k in 0..=80 {
            tracker_update(
                &mut t,
                &baro(k * S / 10, 10_000 - 25 * k as i32, -15_000),
                &mut out,
            );
        }
        let (row, _, _) = tracker_predict(&t, ADDR, 8 * S + S / 10).unwrap();
        assert!(row.flags & O_HAS_ALT != 0);
        assert!((row.vrate_fpm + 15_000.0).abs() < 2_000.0);
    }

    // REQ: OA-TRK-355
    #[test]
    fn a_fast_descent_after_a_long_gap_builds_new_local_rate_evidence() {
        let c = cfg(1, 0);
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        tracker_update(&mut t, &baro(0, 10_000, -15_000), &mut out);
        tracker_update(&mut t, &baro(20 * S, 10_000, -15_000), &mut out);
        let slot = lookup(&t, ADDR).unwrap();
        assert_eq!(t.cur[slot].last_baro_t_ns, 20 * S);

        // The normal altitude gate still requires repeated reports if descent starts
        // abruptly from level flight. Corroboration must survive that re-anchor.
        for k in 1..=8 {
            tracker_update(
                &mut t,
                &baro((20 + k) * S, 10_000 - 250 * k as i32, -15_000),
                &mut out,
            );
        }
        let (row, _, _) = tracker_predict(&t, ADDR, 29 * S).unwrap();
        assert!(row.flags & O_HAS_ALT != 0);
        assert!((row.vrate_fpm + 15_000.0).abs() < 2_000.0);
    }

    // REQ: OA-TRK-355
    #[test]
    fn contradictory_rate_then_missing_altitude_and_coast_preserve_reacquired_fast_vertical_motion()
    {
        for direction in [-1, 1] {
            let c = cfg(1, 0);
            let mut t = tracker_new(&c).unwrap();
            let mut out = output_new(c.out_capacity);
            // Flat barometric altitude contradicts the reported descent rate.
            for k in 0..8 {
                tracker_update(&mut t, &baro(k * S, 10_000, -15_000), &mut out);
            }
            let (level, _, _) = tracker_predict(&t, ADDR, 8 * S).unwrap();
            assert!(level.flags & O_HAS_ALT != 0);
            assert!((level.alt_ft - 10_000.0).abs() < 100.0);
            assert!(level.vrate_fpm.abs() < 1_000.0);

            // Geometric-only reports do not supply barometric evidence; the track then coasts.
            for k in 8..10 {
                let mut m = at(0, 0, 100.0);
                m.toa_ns = k * S;
                m.alt_ft = 10_000 + 5_000 * k as i32;
                m.alt_quant_ft = 25;
                m.flags |= M_ALT_GEOMETRIC;
                tracker_update(&mut t, &m, &mut out);
            }
            let (geometric_only, _, _) = tracker_predict(&t, ADDR, 10 * S).unwrap();
            assert!((geometric_only.alt_ft - 10_000.0).abs() < 100.0);
            assert!(geometric_only.vrate_fpm.abs() < 1_000.0);
            assert!(tracker_set_expected_interval(&mut t, ADDR, S));
            tracker_advance(&mut t, 20 * S, &mut out);
            assert!(out.flags.iter().any(|flags| flags & O_COAST != 0));

            // After the old raw-altitude anchor expires, consistent reports establish a real
            // rapid descent or climb despite the earlier contradictory rate.
            for k in 0..=8 {
                tracker_update(
                    &mut t,
                    &baro(
                        28 * S + k * S,
                        10_000 + direction * 250 * k as i32,
                        direction * 15_000,
                    ),
                    &mut out,
                );
            }
            let (row, _, _) = tracker_predict(&t, ADDR, 37 * S).unwrap();
            assert!(row.flags & O_HAS_ALT != 0);
            assert!((row.alt_ft - (10_000.0 + direction as f32 * 2_250.0)).abs() < 250.0);
            assert!((row.vrate_fpm - direction as f32 * 15_000.0).abs() < 2_000.0);
        }
    }

    // REQ: OA-TRK-355
    #[test]
    fn contradictory_rate_does_not_erase_established_altitude_derived_descent() {
        let c = cfg(1, 0);
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..8 {
            let rate = if k == 5 { -15_000 } else { -2_400 };
            tracker_update(&mut t, &baro(k * S, 10_000 - 40 * k as i32, rate), &mut out);
            if k == 5 {
                let (row, _, _) = tracker_predict(&t, ADDR, k * S).unwrap();
                assert!((row.vrate_fpm + 2_400.0).abs() < 1_000.0);
            }
        }
        let (row, _, _) = tracker_predict(&t, ADDR, 8 * S).unwrap();
        assert!(row.flags & O_HAS_ALT != 0);
        assert!((row.alt_ft - 9_680.0).abs() < 150.0);
        assert!((row.vrate_fpm + 2_400.0).abs() < 1_000.0);
    }

    /// Everything the order of arrival must not change about the track.
    #[derive(Debug, PartialEq)]
    struct Snap {
        state: u8,
        hits: u8,
        updates: u32,
        first_ns: i64,
        last_ns: i64,
        confirmed_ns: i64,
        cur: Filter,
        ring: Vec<Measurement>,
    }

    fn snap(t: &Tracker) -> Snap {
        let slot = lookup(t, ADDR).unwrap();
        Snap {
            state: col(&t.state, slot, FREE),
            hits: col(&t.hits, slot, 0),
            updates: col(&t.updates, slot, 0),
            first_ns: col(&t.first_ns, slot, 0),
            last_ns: col(&t.last_ns, slot, 0),
            confirmed_ns: col(&t.confirmed_ns, slot, 0),
            // The reacquisition mark is output state, cleared by the row that carries it.
            cur: Filter {
                reacq: 0,
                ..col(&t.cur, slot, blank_filter())
            },
            ring: (0..ring_len(t, slot))
                .map(|k| col(&t.ring_m, ring_ix(t, slot, k), blank_measurement()))
                .collect(),
        }
    }

    fn run(c: &TrackerConfig, ms: &[Measurement]) -> Tracker {
        let mut t = tracker_new(c).unwrap();
        let mut out = output_new(c.out_capacity);
        for m in ms {
            tracker_update(&mut t, m, &mut out);
        }
        t
    }

    fn assert_same_outcome(c: &TrackerConfig, chrono: &[Measurement], late: &[Measurement]) {
        let (a, b) = (run(c, chrono), run(c, late));
        assert_eq!(snap(&a), snap(&b));
        let (sa, sb) = (tracker_stats(&a), tracker_stats(&b));
        assert_eq!(sa.init_implausible, sb.init_implausible);
        assert_eq!(sa.init_window_expired, sb.init_window_expired);
        assert_eq!(sa.confirmed, sb.confirmed);
        assert_eq!(sa.jump_rejected, sb.jump_rejected);
        assert_eq!(sa.resyncs, sb.resyncs);
    }

    // REQ: OA-TRK-027
    #[test]
    fn confirmed_in_order_resync_emits_only_its_current_report_row() {
        let mut c = cfg(2, 0);
        c.tick_ns = 60 * S;
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        tracker_update(&mut t, &at(0, 0, 25.0), &mut out);
        tracker_update(&mut t, &at(1, 0, 25.0), &mut out);
        assert!(tracker_is_confirmed(&t, ADDR));

        // Isolate report-time emissions from the two reports that confirmed the track.
        crate::types::output_clear(&mut out);
        let before = *tracker_stats(&t);
        let mut resync_report = at(2, 1_000_000, 25.0);
        for rejection in 0..crate::params::RESYNC_AFTER {
            resync_report.toa_ns = 1_000 * S + (2 + i64::from(rejection)) * S;
            tracker_update(&mut t, &resync_report, &mut out);
            if rejection + 1 < crate::params::RESYNC_AFTER {
                assert_eq!(out.t_ns.len(), 0, "a rejected report is not emitted");
            }
        }

        let after = tracker_stats(&t);
        assert_eq!(after.applied, before.applied + 1);
        assert_eq!(after.resyncs, before.resyncs + 1);
        assert_eq!(
            after.jump_rejected,
            before.jump_rejected + u64::from(crate::params::RESYNC_AFTER)
        );
        assert_eq!(
            out.t_ns.len(),
            1,
            "the resync report emits exactly one report-time row"
        );
        let i = 0;
        assert_eq!(out.t_ns[i], resync_report.toa_ns);
        assert_eq!(out.addr[i], ADDR);
        assert_eq!(out.flags[i] & O_TICK, 0, "the report row is not a tick");

        // Resync is a fresh filter initialized at this report, including its covariance.
        let expected = filter_output(&filter_init(&resync_report, c.h_mode), resync_report.toa_ns)
            .expect("the resync report must have a representable state");
        assert_eq!(out.lat_e7[i], expected.lat_e7);
        assert_eq!(out.lon_e7[i], expected.lon_e7);
        assert_eq!(out.pos_cov_m2[i], expected.pos_cov_m2);
        assert_eq!(out.vel_cov_m2s2[i], expected.vel_cov_m2s2);
        assert_eq!(out.mu_h[i], expected.mu_h);

        // Advancing later emits ticks only; it must not duplicate the report-time row.
        crate::types::output_clear(&mut out);
        tracker_advance(&mut t, resync_report.toa_ns + 60 * S, &mut out);
        assert!(out.flags.iter().all(|flags| flags & O_TICK != 0));
        assert_eq!(
            out.t_ns
                .iter()
                .filter(|&&time| time == resync_report.toa_ns)
                .count(),
            0,
            "a later advance must not duplicate the report-time row"
        );
    }

    // REQ: OA-TRK-230
    #[test]
    fn a_late_report_gets_the_same_plausibility_check_as_one_in_order() {
        let c = cfg(3, 1);
        // B is about 100 km from A; its huge covariance passes the statistical gate.
        let a0 = at(0, 0, 100.0);
        let b1 = at(1, 9_000_000, 1.0e9);
        let a2 = at(2, 0, 100.0);
        assert_same_outcome(&c, &[a0, b1, a2], &[a0, a2, b1]);
        let t = run(&c, &[a0, a2, b1]);
        assert_eq!(tracker_stats(&t).init_implausible, 2);
        assert_eq!(tracker_stats(&t).confirmed, 0);
        assert_eq!(snap(&t).hits, 1);
    }

    /// A0 with a loose variance, a precise report `LATE_OFF_E7` (700 m) east at 1 s, then `n` precise
    /// reports back at the origin that the late one's state rejects.
    const LATE_OFF_E7: i32 = 62_900;

    fn streak_world(n: i64) -> (Vec<Measurement>, Vec<Measurement>) {
        let a0 = at(0, 0, 100.0);
        let l1 = at(1, LATE_OFF_E7, 1.0);
        let ns: Vec<Measurement> = (2..2 + n).map(|s| at(s, 0, 1.0)).collect();
        let mut chrono = vec![a0, l1];
        chrono.extend(&ns);
        let mut late = vec![a0];
        late.extend(&ns);
        late.push(l1);
        (chrono, late)
    }

    // REQ: OA-TRK-231
    #[test]
    fn a_late_report_leaves_the_rejection_streak_chronological_arrival_gives() {
        let c = cfg(2, 1);
        let (chrono, late) = streak_world(3);
        assert_same_outcome(&c, &chrono, &late);
        let s = snap(&run(&c, &late));
        assert_eq!(s.cur.rej_h, 3);
        assert_eq!(tracker_stats(&run(&c, &late)).jump_rejected, 3);
    }

    // REQ: OA-TRK-231
    #[test]
    fn a_late_report_gives_the_resync_chronological_arrival_gives() {
        let c = cfg(2, 1);
        let (chrono, late) = streak_world(5);
        assert_same_outcome(&c, &chrono, &late);
        let t = run(&c, &late);
        assert_eq!(tracker_stats(&t).resyncs, 1);
        let s = snap(&t);
        assert_eq!((s.hits, s.ring.len(), s.cur.rej_h), (1, 1, 0));
    }

    // REQ: OA-TRK-338, OA-TRK-343, OA-TRK-344, OA-TRK-345
    #[test]
    fn a_coasting_track_is_never_absent_and_says_when_it_is_uncertain() {
        let c = TrackerConfig {
            tick_ns: S,
            drop_after_ns: 200 * S,
            reacquire_gap_ns: i64::MAX,
            ..cfg(1, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..3 {
            tracker_update(&mut t, &at(5 * k, 100 * k as i32, 100.0), &mut out);
        }
        crate::types::output_clear(&mut out);
        let before = tracker_stats(&t).rows_uncertain;
        tracker_advance(&mut t, 1_000 * S + 150 * S, &mut out);
        // One row per tick for the whole 150 s: nothing is withheld (it was, past 926 m, before).
        let ticks: Vec<usize> = (0..out.t_ns.len())
            .filter(|&i| out.flags[i] & O_TICK != 0)
            .collect();
        let times: Vec<i64> = ticks.iter().map(|&i| out.t_ns[i]).collect();
        assert!(times.windows(2).all(|w| w[1] - w[0] == S), "{times:?}");
        assert!(times.len() >= 140, "{} tick rows", times.len());
        assert_eq!(*times.last().unwrap(), 1_000 * S + 150 * S);
        // The flag is exactly the radius test, on every row; it starts after the coast does.
        let mut uncertain = 0u64;
        let (mut first_coast, mut first_uncertain) = (None, None);
        for &i in &ticks {
            let r = r95_m(&out.pos_cov_m2[i]);
            let flagged = out.flags[i] & O_UNCERTAIN != 0;
            assert_eq!(flagged, r > c.coast_max_r95_m, "row {i}: r95 {r}");
            uncertain += u64::from(flagged);
            if out.flags[i] & O_COAST != 0 && first_coast.is_none() {
                first_coast = Some(out.t_ns[i]);
            }
            if flagged && first_uncertain.is_none() {
                first_uncertain = Some(out.t_ns[i]);
            }
        }
        assert!(uncertain > 0, "150 s of coasting never became uncertain");
        assert!(first_coast.unwrap() < first_uncertain.unwrap());
        assert_eq!(tracker_stats(&t).rows_uncertain - before, uncertain);
        assert_eq!(tracker_stats(&t).nan_covariance, 0);
        let slot = lookup(&t, ADDR).expect("coasting track remains live");
        assert_ne!(
            col(&t.cur, slot, blank_filter()).reacq,
            0,
            "an uncertain tick arms the next measurement even when gap reacquisition is disabled"
        );
        // The next update is published, and says the track was reacquired.
        crate::types::output_clear(&mut out);
        tracker_update(&mut t, &at(150, 3_000, 100.0), &mut out);
        assert_eq!(out.t_ns.len(), 1);
        assert_ne!(out.flags[0] & O_REACQUIRED, 0);
        assert_eq!(out.flags[0] & O_UNCERTAIN, 0);
        // The flag is carried once.
        crate::types::output_clear(&mut out);
        tracker_update(&mut t, &at(151, 3_020, 100.0), &mut out);
        assert_eq!(out.flags[0] & O_REACQUIRED, 0);
    }

    // REQ: OA-TRK-344
    #[test]
    fn synthetic_stale_turn_dropout_compares_model_to_straight_extrapolation() {
        use crate::types::M_HAS_VEL;
        use oa_geo::aeqd::{frame_new, project, unproject};

        // Keep this analytic test fixture in the pure tracker crate. It deliberately models a
        // constant-speed turn, but does not depend on the split oa-truth/synth_truth shell.
        #[derive(Clone)]
        struct Seg {
            dur_s: f64,
            turn_dps: f64,
            vrate_fpm: f64,
        }
        struct Traj {
            addr: u32,
            t0_s: f64,
            x0: f64,
            y0: f64,
            heading_deg: f64,
            speed_mps: f64,
            alt_ft: f64,
            segs: Vec<Seg>,
        }
        #[derive(Clone, Copy)]
        struct State {
            e: f64,
            n: f64,
            ve: f64,
            vn: f64,
        }
        fn state_at(path: &Traj, elapsed_s: f64) -> State {
            let _altitude_at_start = path.alt_ft;
            let (mut east, mut north, mut heading) =
                (path.x0, path.y0, path.heading_deg.to_radians());
            let mut remaining = (elapsed_s - path.t0_s).max(0.0);
            for segment in &path.segs {
                let _vertical_rate = segment.vrate_fpm;
                let step = remaining.min(segment.dur_s);
                let omega = segment.turn_dps.to_radians();
                let next_heading = heading + omega * step;
                if omega.abs() < 1.0e-12 {
                    east += path.speed_mps * oa_math::sin(heading) * step;
                    north += path.speed_mps * oa_math::cos(heading) * step;
                } else {
                    east += path.speed_mps / omega
                        * (oa_math::cos(heading) - oa_math::cos(next_heading));
                    north += path.speed_mps / omega
                        * (oa_math::sin(next_heading) - oa_math::sin(heading));
                }
                heading = next_heading;
                remaining -= step;
                if remaining <= 0.0 {
                    break;
                }
            }
            State {
                e: east,
                n: north,
                ve: path.speed_mps * oa_math::sin(heading),
                vn: path.speed_mps * oa_math::cos(heading),
            }
        }

        let frame = frame_new(0.0, 0.0);
        let measurement = |path: &Traj, time_s: i64| {
            let truth = state_at(path, time_s as f64);
            // ADS-B's velocity is one second behind position through a turn. The final straight
            // segment is an independent analytic rollout, not an output of the tracker model.
            let vel_time_s = (time_s - 1).max(0) as f64;
            let reported_velocity = state_at(path, vel_time_s);
            let (lat, lon) = unproject(&frame, truth.e, truth.n).expect("synthetic position");
            Measurement {
                toa_ns: time_s * S,
                addr: path.addr,
                lat_e7: (lat * 1.0e7).round() as i32,
                lon_e7: (lon * 1.0e7).round() as i32,
                pos_cov_m2: [25.0, 0.0, 25.0],
                vel_cov_m2s2: [4.0, 0.0, 4.0],
                vel_east_mps: reported_velocity.ve as f32,
                vel_north_mps: reported_velocity.vn as f32,
                flags: M_HAS_VEL,
                vel_lag_s: if time_s > 0 { 1.0 } else { 0.0 },
                sensor: 1,
                ..blank_measurement()
            }
        };
        let run_case = |path: &Traj, warm_s: i64, gap_s: i64| {
            let c = TrackerConfig {
                max_tracks: 2,
                out_capacity: 1 << 12,
                tick_ns: S,
                expected_interval_ns: S,
                init_interval_ns: S,
                coast_after_intervals: 3,
                drop_after_ns: 300 * S,
                init_m: 1,
                init_n: 1,
                ..DEFAULT_CONFIG
            };
            let mut t = tracker_new(&c).expect("valid synthetic tracker config");
            let mut out = output_new(c.out_capacity);
            for time_s in 0..=warm_s {
                tracker_update(&mut t, &measurement(path, time_s), &mut out);
            }
            let last = tracker_predict_checked(&t, ADDR, warm_s * S)
                .expect("forecast at last report")
                .0;
            let last_ll = (
                f64::from(last.lat_e7) / 1.0e7,
                f64::from(last.lon_e7) / 1.0e7,
            );
            let last_en = project(&frame, last_ll.0, last_ll.1);
            crate::types::output_clear(&mut out);
            let end_s = warm_s + gap_s;
            tracker_advance(&mut t, end_s * S, &mut out);

            let truth = state_at(path, end_s as f64);
            let truth_cross = |east: f64, north: f64| {
                let error_e = east - truth.e;
                let error_n = north - truth.n;
                let speed = oa_math::hypot(truth.ve, truth.vn);
                (error_e * -truth.vn + error_n * truth.ve).abs() / speed
            };
            let (forecast, confirmed, last_ns) = match tracker_predict_checked(&t, ADDR, end_s * S)
            {
                Ok((row, confirmed, last_ns)) => (Some(row), confirmed, last_ns),
                Err(_) => (None, false, i64::MIN),
            };
            let tick_indices: Vec<usize> = (0..out.t_ns.len())
                .filter(|&i| {
                    out.addr[i] == ADDR
                        && out.flags[i] & O_TICK != 0
                        && out.t_ns[i] > warm_s * S
                        && out.t_ns[i] <= end_s * S
                })
                .collect();
            let coast_ticks = tick_indices
                .iter()
                .filter(|&&i| out.flags[i] & O_COAST != 0)
                .count();
            let uncertain_ticks = tick_indices
                .iter()
                .filter(|&&i| out.flags[i] & O_UNCERTAIN != 0)
                .count();
            let endpoint_tick = tick_indices
                .iter()
                .copied()
                .find(|&i| out.t_ns[i] == end_s * S);
            let forecast_error = forecast.map(|row| {
                let lat = f64::from(row.lat_e7) / 1.0e7;
                let lon = f64::from(row.lon_e7) / 1.0e7;
                let (east, north) = project(&frame, lat, lon);
                (truth_cross(east, north), r95_m(&row.pos_cov_m2))
            });
            let straight_e = last_en.0 + f64::from(last.vel_east_mps) * gap_s as f64;
            let straight_n = last_en.1 + f64::from(last.vel_north_mps) * gap_s as f64;
            let straight_cross = truth_cross(straight_e, straight_n);
            let identities = tracker_live_iter(&t).collect::<Vec<_>>();
            (
                forecast_error,
                straight_cross,
                coast_ticks,
                uncertain_ticks,
                tick_indices.len(),
                endpoint_tick.map(|i| (out.flags[i], r95_m(&out.pos_cov_m2[i]))),
                confirmed,
                last_ns,
                identities,
            )
        };

        let trajectories = [
            (
                "steady_turn",
                20,
                Traj {
                    addr: ADDR,
                    t0_s: 0.0,
                    x0: 0.0,
                    y0: 0.0,
                    heading_deg: 0.0,
                    speed_mps: 220.0,
                    alt_ft: 30_000.0,
                    segs: vec![Seg {
                        dur_s: 100.0,
                        turn_dps: 0.6,
                        vrate_fpm: 0.0,
                    }],
                },
            ),
            (
                "rollout",
                35,
                Traj {
                    addr: ADDR,
                    t0_s: 0.0,
                    x0: 0.0,
                    y0: 0.0,
                    heading_deg: 0.0,
                    speed_mps: 220.0,
                    alt_ft: 30_000.0,
                    segs: vec![
                        Seg {
                            dur_s: 30.0,
                            turn_dps: 0.6,
                            vrate_fpm: 0.0,
                        },
                        Seg {
                            dur_s: 60.0,
                            turn_dps: 0.0,
                            vrate_fpm: 0.0,
                        },
                    ],
                },
            ),
        ];
        let mut forecasts_absent = 0;
        let mut identities_preserved = 0;
        println!("phase,gap_s,model_cross_m,straight_cross_m,r95_m,coast_ticks,uncertain_ticks,ticks_seen,forecast,identity");
        for (phase, warm_s, path) in &trajectories {
            for gap_s in [5, 10, 20] {
                let (
                    forecast_error,
                    straight_cross,
                    coast_ticks,
                    uncertain_ticks,
                    tick_count,
                    endpoint_tick,
                    confirmed,
                    last_ns,
                    identities,
                ) = run_case(path, *warm_s, gap_s);
                let Some((model_cross, r95)) = forecast_error else {
                    forecasts_absent += 1;
                    println!("{phase},{gap_s},absent,{straight_cross:.2},absent,{coast_ticks},{uncertain_ticks},{tick_count}/{gap_s},no,no");
                    continue;
                };
                let identity_ok =
                    confirmed && last_ns == *warm_s * S && identities == [(ADDR, *warm_s * S)];
                identities_preserved += usize::from(identity_ok);
                let endpoint = endpoint_tick.expect("one-second endpoint tick");
                println!(
                    "{phase},{gap_s},{model_cross:.2},{straight_cross:.2},{r95:.1},{coast_ticks},{uncertain_ticks},{tick_count}/{gap_s},yes,{}",
                    if identity_ok { "yes" } else { "no" }
                );
                assert!(identity_ok, "{phase}, gap {gap_s}: {identities:?}");
                assert_eq!(tick_count, gap_s as usize, "{phase}, gap {gap_s}");
                assert!(endpoint.0 & O_COAST != 0, "{phase}, gap {gap_s}");
                assert_eq!(
                    endpoint.0 & O_UNCERTAIN != 0,
                    endpoint.1 > DEFAULT_CONFIG.coast_max_r95_m
                );
            }
        }
        assert_eq!(forecasts_absent, 0);
        assert_eq!(identities_preserved, 6);
        println!(
            "# absent_forecasts={forecasts_absent}; identity_preserved={identities_preserved}/6"
        );
    }

    // REQ: OA-TRK-344
    #[test]
    fn an_update_after_a_gap_over_fifteen_seconds_is_flagged_reacquired_and_a_short_gap_is_not() {
        let c = TrackerConfig {
            tick_ns: S,
            drop_after_ns: 200 * S,
            ..cfg(1, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        let flags_of = |t: &mut Tracker, out: &mut TrackOutput, m: &Measurement| {
            crate::types::output_clear(out);
            tracker_update(t, m, out);
            (0..out.t_ns.len())
                .filter(|&i| out.flags[i] & O_TICK == 0)
                .map(|i| out.flags[i])
                .next_back()
                .unwrap_or(0)
        };
        for k in 0..3 {
            tracker_update(&mut t, &at(k, 20 * k as i32, 100.0), &mut out);
        }
        // 10 s later: no. 20 s after that: yes, though not uncertain. Right after: no.
        assert_eq!(
            flags_of(&mut t, &mut out, &at(12, 240, 100.0)) & O_REACQUIRED,
            0
        );
        let f = flags_of(&mut t, &mut out, &at(32, 640, 100.0));
        assert_eq!((f & O_REACQUIRED != 0, f & O_UNCERTAIN != 0), (true, false));
        assert_eq!(
            flags_of(&mut t, &mut out, &at(33, 660, 100.0)) & O_REACQUIRED,
            0
        );
        // With the gap rule off (as the pinned tests do) a long gap is not flagged.
        let off = TrackerConfig {
            reacquire_gap_ns: i64::MAX,
            ..c
        };
        let mut t2 = tracker_new(&off).unwrap();
        for k in 0..3 {
            tracker_update(&mut t2, &at(k, 20 * k as i32, 100.0), &mut out);
        }
        assert_eq!(
            flags_of(&mut t2, &mut out, &at(32, 640, 100.0)) & O_REACQUIRED,
            0
        );
    }

    // REQ: OA-TRK-345
    #[test]
    fn a_row_with_a_covariance_that_is_not_a_number_is_refused_and_counted_as_loss() {
        let c = cfg(1, 0);
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        tracker_update(&mut t, &at(0, 0, 100.0), &mut out);
        crate::types::output_clear(&mut out);
        let slot = lookup(&t, ADDR).unwrap();
        let mut f = col(&t.cur, slot, blank_filter());
        for m in &mut f.h.m {
            m.p[0][0] = f64::NAN;
        }
        put(&mut t.cur, slot, f);
        let before = *tracker_stats(&t);
        emit(&mut t, slot, 1_000 * S, 0, &mut out);
        let after = *tracker_stats(&t);
        assert_eq!(out.t_ns.len(), 0);
        assert_eq!(after.nan_covariance, before.nan_covariance + 1);
        assert_eq!(after.rows_computed, before.rows_computed + 1);
        assert_eq!(after.rows_uncertain, before.rows_uncertain);
    }

    // REQ: OA-TRK-302
    #[test]
    fn a_track_with_its_own_expected_interval_coasts_by_it() {
        let c = TrackerConfig {
            tick_ns: S,
            expected_interval_ns: 12 * S,
            drop_after_ns: 200 * S,
            ..cfg(1, 0)
        };
        let first_coast = |own: i64| {
            let mut t = tracker_new(&c).unwrap();
            let mut out = output_new(c.out_capacity);
            for k in 0..3 {
                tracker_update(&mut t, &at(5 * k, 100 * k as i32, 100.0), &mut out);
            }
            if own > 0 {
                assert!(tracker_set_expected_interval(&mut t, ADDR, own));
            }
            crate::types::output_clear(&mut out);
            tracker_advance(&mut t, 1_000 * S + 100 * S, &mut out);
            let last = 1_000 * S + 10 * S;
            (0..out.t_ns.len())
                .filter(|&i| out.flags[i] & O_COAST != 0)
                .map(|i| (out.t_ns[i] - last) / S)
                .min()
        };
        // The configured 12 s: after 36 s. Its own 4.8 s: after 14.4 s, so the first coasting
        // tick is the 15th second.
        assert_eq!(first_coast(0), Some(37));
        assert_eq!(first_coast(4_800_000_000), Some(15));
        assert!(!tracker_set_expected_interval(
            &mut tracker_new(&c).unwrap(),
            ADDR,
            S
        ));
    }

    fn assert_same_output_bits(a: &TrackOutput, b: &TrackOutput) {
        assert_eq!((a.capacity, a.overflow), (b.capacity, b.overflow));
        assert_eq!(a.t_ns, b.t_ns);
        assert_eq!(a.addr, b.addr);
        assert_eq!(a.lat_e7, b.lat_e7);
        assert_eq!(a.lon_e7, b.lon_e7);
        assert_eq!(a.flags, b.flags);
        // Absent altitude is NaN; compare its bits, not NaN's non-reflexive PartialEq.
        for (x, y) in [
            (&a.vel_east_mps, &b.vel_east_mps),
            (&a.vel_north_mps, &b.vel_north_mps),
            (&a.turn_rate_rps, &b.turn_rate_rps),
            (&a.alt_ft, &b.alt_ft),
            (&a.alt_var_ft2, &b.alt_var_ft2),
            (&a.vrate_fpm, &b.vrate_fpm),
        ] {
            assert_eq!(
                x.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                y.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
        for (x, y) in [
            (&a.pos_cov_m2, &b.pos_cov_m2),
            (&a.vel_cov_m2s2, &b.vel_cov_m2s2),
        ] {
            assert_eq!(
                x.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>(),
                y.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
        for (x, y) in [(&a.mu_h, &b.mu_h), (&a.mu_v, &b.mu_v)] {
            assert_eq!(
                x.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>(),
                y.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
    }

    #[derive(Debug, PartialEq)]
    struct HistorySnap {
        track: Snap,
        anchor: Filter,
        category: CategoryState,
        ring_head: u8,
        ring_len: u8,
        ring_physical: Vec<Measurement>,
        newest_update_ns: i64,
    }

    fn history_snap(t: &Tracker, slot: usize) -> HistorySnap {
        let start = slot * RING_DEPTH;
        HistorySnap {
            track: snap(t),
            anchor: col(&t.anchor, slot, blank_filter()),
            category: col(&t.cat, slot, CategoryState::default()),
            ring_head: col(&t.ring_head, slot, 0),
            ring_len: col(&t.ring_len, slot, 0),
            ring_physical: t.ring_m.get(start..start + RING_DEPTH).unwrap().to_vec(),
            newest_update_ns: t.newest_update_ns,
        }
    }

    fn full_ring_tracker() -> (Tracker, TrackOutput, usize) {
        let c = TrackerConfig {
            tick_ns: 100_000 * S,
            ..cfg(1, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..RING_DEPTH {
            tracker_update(&mut t, &at(k as i64, 0, 100.0), &mut out);
        }
        let slot = lookup(&t, ADDR).unwrap();
        assert_eq!(ring_len(&t, slot), RING_DEPTH);
        // Retain pre-existing output rows to check prefix preservation on rollback.
        (t, out, slot)
    }

    fn poison_anchor_velocity_covariance(t: &mut Tracker, slot: usize) {
        let anchor = t.anchor.get_mut(slot).unwrap();
        for model in &mut anchor.h.m {
            model.p[2][2] = -1.0e6;
        }
    }

    fn assert_only_numerical_refusal(t: &Tracker, before: TrackerStats) {
        let mut expected = before;
        expected.offered += 1;
        expected.numerical_refused += 1;
        assert_eq!(t.stats, expected);
    }

    // REQ: OA-TRK-331
    #[test]
    fn invalid_anchor_refuses_ring_full_update_without_partial_commit_and_recovers() {
        let (mut t, mut out, slot) = full_ring_tracker();
        let good_anchor = col(&t.anchor, slot, blank_filter());
        poison_anchor_velocity_covariance(&mut t, slot);
        let before = history_snap(&t, slot);
        let before_stats = t.stats;
        let before_out = out.clone();

        let next = at(RING_DEPTH as i64, 0, 100.0);
        tracker_update(&mut t, &next, &mut out);

        assert_eq!(history_snap(&t, slot), before);
        assert_same_output_bits(&out, &before_out);
        assert_only_numerical_refusal(&t, before_stats);

        // A valid anchor lets the same update proceed, with no second refusal.
        put(&mut t.anchor, slot, good_anchor);
        let before_recovery = t.stats;
        tracker_update(&mut t, &next, &mut out);
        assert_eq!(t.stats.numerical_refused, before_recovery.numerical_refused);
        assert_eq!(t.stats.applied, before_recovery.applied + 1);
        assert_eq!(col(&t.last_ns, slot, 0), next.toa_ns);
        assert_eq!(ring_len(&t, slot), RING_DEPTH);
    }

    // REQ: OA-TRK-332
    #[test]
    fn checked_forecast_distinguishes_a_failed_prediction_from_an_absent_key() {
        let (mut t, _out, slot) = full_ring_tracker();
        let at_ns = col(&t.last_ns, slot, 0) + S;
        assert!(matches!(
            tracker_predict_checked(&t, ADDR + 1, at_ns),
            Err(PredictError::NoTrack)
        ));
        assert!(tracker_predict_checked(&t, ADDR, at_ns).is_ok());
        let cur = t.cur.get_mut(slot).unwrap();
        for model in &mut cur.h.m {
            model.p[2][2] = -1.0e6;
        }
        let before = history_snap(&t, slot);
        let stats = t.stats;
        assert!(matches!(
            tracker_predict_checked(&t, ADDR, at_ns),
            Err(PredictError::Numerical)
        ));
        assert!(tracker_predict(&t, ADDR, at_ns).is_none());
        assert!(tracker_has_key(&t, ADDR));
        assert_eq!(history_snap(&t, slot), before);
        assert_eq!(t.stats, stats);
    }

    // REQ: OA-TRK-331
    #[test]
    fn invalid_anchor_refuses_late_rebuild_without_mutation_and_recovers() {
        let (mut t, mut out, slot) = full_ring_tracker();
        let good_anchor = col(&t.anchor, slot, blank_filter());
        poison_anchor_velocity_covariance(&mut t, slot);
        let before = history_snap(&t, slot);
        let before_stats = t.stats;
        let before_out = out.clone();
        let mut late = at(2, 0, 100.0);
        late.toa_ns += S / 2;

        tracker_update(&mut t, &late, &mut out);

        assert_eq!(history_snap(&t, slot), before);
        assert_same_output_bits(&out, &before_out);
        assert_only_numerical_refusal(&t, before_stats);

        put(&mut t.anchor, slot, good_anchor);
        let before_recovery = t.stats;
        tracker_update(&mut t, &late, &mut out);
        assert_eq!(t.stats.numerical_refused, before_recovery.numerical_refused);
        assert_eq!(t.stats.refiltered, before_recovery.refiltered + 1);
        assert_eq!(ring_len(&t, slot), RING_DEPTH);
        assert_eq!(col(&t.last_ns, slot, 0), before.track.last_ns);
    }

    // REQ: OA-TRK-331
    #[test]
    fn numerical_failure_after_partial_late_replay_rolls_back_track_stats_and_rows() {
        let (mut t, mut out, slot) = full_ring_tracker();
        out.capacity = out.t_ns.len();
        let mut late = at(1, 0, 100.0);
        late.toa_ns += S / 2;
        late.category = 0xA6;
        assert!(late.toa_ns >= t.clock_ns - t.cfg.ooo_window_ns);

        // Inject a negative position covariance that h_update accepts but that makes the next
        // replay prediction invalid. The same replay steps are used by update_late below.
        let anchor = ring_state(&t, slot, 1).unwrap();
        let mut after_late = anchor;
        let mut scratch = TrackerStats::default();
        assert_eq!(
            filter_replay(&mut after_late, &late, &mut scratch, &t.cfg),
            Applied::Yes
        );
        let ring_one = col(&t.ring_m, ring_ix(&t, slot, 2), blank_measurement());
        let ring_two = col(&t.ring_m, ring_ix(&t, slot, 3), blank_measurement());
        let dt = (ring_one.toa_ns - late.toa_ns) as f64 / S as f64;
        let predicted = crate::hfilter::h_predict(
            &after_late.h,
            dt,
            crate::hfilter::dwell_decay(dt),
            after_late.h_mode,
        )
        .unwrap();
        let variance = predicted.m[1].p[0][0].min(predicted.m[1].p[1][1]);
        // R is negative half the smaller predicted position variance: the innovation
        // remains invertible, but this injected history fault makes a later covariance invalid.
        let mut bad = ring_one;
        let negative = (-0.5 * variance) as f32;
        bad.pos_cov_m2 = [negative, 0.0, negative];
        let mut after_bad = after_late;
        let mut scratch = TrackerStats::default();
        assert_eq!(
            filter_replay(&mut after_bad, &bad, &mut scratch, &t.cfg),
            Applied::Yes
        );
        let next_dt = (ring_two.toa_ns - bad.toa_ns) as f64 / S as f64;
        assert!(crate::hfilter::h_predict(
            &after_bad.h,
            next_dt,
            crate::hfilter::dwell_decay(next_dt),
            after_bad.h_mode,
        )
        .is_none());
        let bad_ix = ring_ix(&t, slot, 2);
        put(&mut t.ring_m, bad_ix, bad);
        t.cat[slot] = CategoryState {
            trusted: 0xA3,
            cand: 0xA6,
            cand_n: 2,
        };
        let before = history_snap(&t, slot);
        let before_stats = t.stats;
        let before_out = out.clone();

        tracker_update(&mut t, &late, &mut out);

        assert_eq!(history_snap(&t, slot), before);
        assert_same_output_bits(&out, &before_out);
        assert_only_numerical_refusal(&t, before_stats);
    }

    /// A stationary report at `t_ns` (since the start of the test clock), `dlat_e7` north.
    fn north(t_ms: i64, dlat_e7: i32) -> Measurement {
        let mut m = at(0, 0, 100.0);
        m.toa_ns += t_ms * (S / 1_000);
        m.lat_e7 = dlat_e7;
        m
    }

    // REQ: OA-TRK-348
    #[test]
    fn a_late_replay_that_resyncs_and_then_refuses_numerically_keeps_the_whole_move() {
        // A forged report at 40 s after a gap holds an unconfirmed move; reports a tenth of a
        // second apart follow it (too soon to decide it). A late report then replays them, two
        // of them are made to disagree with the replayed track in a way that re-synchronises the
        // filter (at the second entry of the replay, not the first), and a later one is made to
        // fail numerically: the replay is undone as a whole, the move with it (not ended by the
        // resync that was undone, not unmarked).
        let c = TrackerConfig {
            tick_ns: 100_000 * S,
            resync_consistent_k: 2,
            ..cfg(1, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..=20 {
            tracker_update(&mut t, &at(k, 0, 100.0), &mut out);
        }
        for (ms, d) in [
            (40_000, 225_000),
            (40_100, 225_000),
            (40_200, 225_000),
            (40_300, 225_000),
        ] {
            tracker_update(&mut t, &north(ms, d), &mut out);
        }
        let slot = lookup(&t, ADDR).unwrap();
        assert!(move_active(&t, slot));
        assert_eq!(t.stats.moves_unconfirmed, 1, "{:?}", t.stats);
        // The ring now: ..., 20 s, 40.0, 40.1, 40.2, 40.3. The late report is the base 20 s's
        // successor, so the replay is [late, 40.0, 40.1, 40.2, 40.3].
        let len = ring_len(&t, slot);
        let at_ix = [3, 2, 1, 0].map(|back| ring_ix(&t, slot, len - 1 - back));
        let ix = |back: usize| at_ix[3 - back];
        // 40.0 and 40.1: 250 m off the replayed track, consistent with each other (rejected,
        // then re-synchronised at the second).
        for (back, ms) in [(3, 40_000), (2, 40_100)] {
            let mut e = col(&t.ring_m, ix(back), blank_measurement());
            e.lat_e7 = 22_500;
            assert_eq!(e.toa_ns, north(ms, 0).toa_ns);
            put(&mut t.ring_m, ix(back), e);
        }
        // 40.2: negative covariance, applied after the resync; 40.3 then fails to predict.
        let anchor = filter_init(&col(&t.ring_m, ix(2), blank_measurement()), 0);
        let mut bad = col(&t.ring_m, ix(1), blank_measurement());
        bad.lat_e7 = 22_500;
        let dt = (bad.toa_ns - col(&t.ring_m, ix(2), blank_measurement()).toa_ns) as f64 / S as f64;
        let predicted = crate::hfilter::h_predict(
            &anchor.h,
            dt,
            crate::hfilter::dwell_decay(dt),
            anchor.h_mode,
        )
        .unwrap();
        let negative = (-0.5 * predicted.m[1].p[0][0].min(predicted.m[1].p[1][1])) as f32;
        bad.pos_cov_m2 = [negative, 0.0, negative];
        put(&mut t.ring_m, ix(1), bad);
        let mut last = col(&t.ring_m, ix(0), blank_measurement());
        last.lat_e7 = 22_500;
        put(&mut t.ring_m, ix(0), last);
        let mut late = at(39, 0, 100.0);
        late.sensor = 2;
        let moves = |t: &Tracker| {
            (
                t.mv_on.clone(),
                t.mv_start_ns.clone(),
                t.mv_snap.clone(),
                t.mv_live,
                t.snap_free.clone(),
                t.snaps
                    .iter()
                    .map(|e| (e.cur, e.updates, e.last_ns, e.ring_len))
                    .collect::<Vec<_>>(),
            )
        };
        let (before, before_stats, before_moves) = (snap(&t), t.stats, moves(&t));
        let before_out = out.clone();

        tracker_update(&mut t, &late, &mut out);

        assert_eq!(
            t.stats.numerical_refused,
            before_stats.numerical_refused + 1
        );
        assert_eq!(
            t.stats.resyncs, before_stats.resyncs,
            "the replay was undone"
        );
        assert_eq!(snap(&t), before);
        assert!(move_active(&t, slot));
        assert!(
            moves(&t) == before_moves,
            "the move, its snapshot and the pool are as they were"
        );
        assert_eq!(out.t_ns.len(), before_out.t_ns.len());
        assert_eq!(
            t.stats.moves_unconfirmed,
            t.stats.moves_confirmed
                + t.stats.moves_rolled_back
                + t.stats.moves_undecided
                + t.stats.moves_abandoned_ended
                + t.stats.moves_abandoned_restarted
                + t.stats.moves_abandoned_resync
                + t.stats.moves_abandoned_demoted
                + tracker_moves_active(&t) as u64
        );
    }

    // REQ: OA-TRK-353
    #[test]
    fn an_old_path_that_cannot_be_predicted_confirms_nothing_and_the_move_stays_marked() {
        // RT-276: with a snapshot present, an old path that cannot be predicted is no
        // agreement: a report on the moved track does not confirm the move (it would have, as
        // "the old path is unknown", with no snapshot at all); the move stays, marked, counted,
        // until the window ends.
        let c = TrackerConfig {
            tick_ns: 100_000 * S,
            ..cfg(1, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..=20 {
            tracker_update(&mut t, &at(k, 0, 100.0), &mut out);
        }
        tracker_update(&mut t, &north(40_000, 225_000), &mut out);
        let slot = lookup(&t, ADDR).unwrap();
        assert!(move_active(&t, slot));
        let ix = move_snap_ix(&t, slot).unwrap();
        t.snaps[ix].cur.h.m[1].x[0] = f64::NAN;

        // Reports that the moved track applies (a wide variance, so that the gate passes them):
        // counted, no confirmation, the mark stays.
        for sec in 41..=43 {
            let mut m = north(sec * 1_000, 225_000);
            m.pos_cov_m2 = [1.0e6, 0.0, 1.0e6];
            tracker_update(&mut t, &m, &mut out);
            assert_eq!(t.stats.moves_old_path_unpredictable, (sec - 40) as u64);
            assert_eq!(t.stats.moves_confirmed, 0);
            assert!(move_active(&t, slot), "{sec} {:?}", t.stats);
            assert_ne!(*out.flags.last().unwrap() & O_MOVE_UNCONFIRMED, 0);
        }
    }

    // REQ: OA-TRK-354
    #[test]
    fn the_snapshot_of_the_capture_equals_the_snapshot_taken_in_every_field() {
        // A track with a wrapped ring (more updates than it holds), a changed category and a
        // run of refusals: every field of the two snapshots is compared, the filters and the
        // ring included.
        let c = TrackerConfig {
            tick_ns: 100_000 * S,
            ..cfg(1, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..(RING_DEPTH as i64 + 5) {
            let mut m = at(k, (k * 37) as i32, 100.0 + k as f32);
            m.category = (k % 3) as u8;
            tracker_update(&mut t, &m, &mut out);
        }
        let slot = lookup(&t, ADDR).unwrap();
        t.cat[slot] = CategoryState {
            trusted: 0xA3,
            cand: 0xA6,
            cand_n: 2,
        };
        assert_ne!(t.ring_head[slot], 0, "the ring has wrapped");
        let (a, b) = (snap_of_txn(&track_txn(&t, slot)), snap_take(&t, slot));
        assert!(a.cur == b.cur && a.anchor == b.anchor);
        assert!(a.ring == b.ring);
        assert!(a.cat == b.cat);
        assert_eq!(
            (
                a.first_ns,
                a.confirmed_ns,
                a.last_ns,
                a.updates,
                a.state,
                a.hits
            ),
            (
                b.first_ns,
                b.confirmed_ns,
                b.last_ns,
                b.updates,
                b.state,
                b.hits
            )
        );
        assert_eq!((a.ring_len, a.ring_head), (b.ring_len, b.ring_head));
    }

    // REQ: OA-TRK-349
    #[test]
    fn a_late_replay_that_releases_the_snapshot_clears_the_old_path_vote() {
        // A forged move, a report on the old path (the vote), then a genuine report from before
        // the forged one arrives late: in time order the forged report is refused, the rollback
        // point cannot be derived, the snapshot goes, and the vote with it (no old path is left
        // to vote for).
        let c = TrackerConfig {
            ooo_window_ns: 30 * S,
            clock_step_limit_ns: 30 * S,
            drop_after_ns: 60 * S,
            tick_ns: 100_000 * S,
            ..cfg(1, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..=20 {
            tracker_update(&mut t, &at(k, 0, 100.0), &mut out);
        }
        tracker_update(&mut t, &north(40_000, 225_000), &mut out);
        let slot = lookup(&t, ADDR).unwrap();
        tracker_update(&mut t, &north(41_000, 0), &mut out);
        assert_eq!(t.stats.moves_old_path_votes, 1, "{:?}", t.stats);
        assert_eq!(t.mv_old_vote_ns[slot], 1_000 * S + 41 * S);
        tracker_update(&mut t, &north(39_600, 0), &mut out);
        assert_eq!(t.stats.moves_rollback_lost_late, 1, "{:?}", t.stats);
        assert!(move_active(&t, slot));
        assert_eq!(t.mv_snap[slot], NO_SNAP);
        assert_eq!(t.mv_old_vote_ns[slot], NO_VOTE_NS);
    }

    // REQ: OA-TRK-349
    #[test]
    fn a_late_replay_that_leaves_the_track_tentative_ends_the_move_as_demoted() {
        // The track is confirmed by its third report; a report from between the first ones
        // arrives late while a forged move is unconfirmed. The replay un-confirms the track and,
        // with the initiation count raised past what the history can give (the only way to make
        // the replay fall short here), does not confirm it again: the move is abandoned as
        // demoted, counted once, and every move still ends once.
        let c = TrackerConfig {
            ooo_window_ns: 30 * S,
            clock_step_limit_ns: 30 * S,
            drop_after_ns: 60 * S,
            tick_ns: 100_000 * S,
            ..cfg(3, 0)
        };
        let mut t = tracker_new(&c).unwrap();
        let mut out = output_new(c.out_capacity);
        for k in 0..=5 {
            tracker_update(&mut t, &at(k, 0, 100.0), &mut out);
        }
        tracker_update(&mut t, &north(25_000, 225_000), &mut out);
        let slot = lookup(&t, ADDR).unwrap();
        assert!(move_active(&t, slot));
        t.cfg.init_m = 99;
        tracker_update(&mut t, &north(1_500, 0), &mut out);
        assert_eq!(t.stats.moves_abandoned_demoted, 1, "{:?}", t.stats);
        assert!(!move_active(&t, slot));
        assert_eq!(t.state[slot], TENTATIVE);
        assert_eq!(tracker_moves_active(&t), 0);
        let s = &t.stats;
        assert_eq!(
            s.moves_unconfirmed,
            s.moves_confirmed
                + s.moves_rolled_back
                + s.moves_undecided
                + s.moves_abandoned_ended
                + s.moves_abandoned_restarted
                + s.moves_abandoned_resync
                + s.moves_abandoned_demoted
        );
    }

    // REQ: OA-TRK-347
    #[test]
    fn the_per_track_bytes_of_the_move_columns_are_the_sum_of_the_columns_themselves() {
        // Fixed-width field sizes are written once in `MOVE_COLUMN_BYTES` and documented below.
        let t = tracker_new(&cfg(1, 0)).unwrap();
        assert_eq!(MOVE_COLUMN_BYTES, 39);
        // All the columns are as long as the track table.
        let n = t.mv_on.len();
        for len in [
            t.mv_start_ns.len(),
            t.mv_snap.len(),
            t.mv_first_ns.len(),
            t.mv_first_sens.len(),
            t.mv_old_vote_ns.len(),
            t.mv_resync_until_ns.len(),
        ] {
            assert_eq!(len, n);
        }
    }
}

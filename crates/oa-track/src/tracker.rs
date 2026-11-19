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
//! a track falls from about 8.9 kB to about 2.1 kB. Keeping the anchor costs one more filter
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

use oa_adsb::{
    category_observe, jump_verdict, CategoryState, JumpPair, JumpVerdict, CAP_ORDINARY_KT,
    NONE_U16, SPEED_ENVELOPE_MPS,
};

use crate::confirm::{confirm_window_ns, resolve_decide, Resolve};
use crate::filter::{
    filter_apply, filter_init, filter_miss_m, filter_output, filter_output_checked, filter_replay,
    filter_row_distance_m, Applied, Filter,
};
use crate::hfilter::{h_combine, HFilter};
use crate::params::{CHI2_2_95, NS_PER_S, PLAUSIBLE_SLACK_M, RING_DEPTH};
use crate::time_gate::{
    gate_decide, gate_new, gate_note_applied, gate_run_restarted, GateCfg, GateState, Verdict,
    RUN_BUF,
};
use crate::types::{
    check_config, log_new, output_push, Measurement, OutRow, PredictError, TrackError, TrackLog,
    TrackOutput, TrackerConfig, TrackerStats, END_DROPPED, END_FLUSHED, END_TENTATIVE, M_HAS_VEL,
    NONE_I32, NO_ADDR, O_COAST, O_IDENTITY_UNCONFIRMED, O_MOVE_UNCONFIRMED, O_RADAR_DISAGREES,
    O_REACQUIRED, O_TICK, O_UNCERTAIN, RADAR_KEY_BASE, REF_ALTITUDE, REF_NO_ADDRESS, REF_POSITION,
    REF_POS_COV, REF_TIME, REF_VELOCITY, TIME_LIMIT_NS,
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
    /// Each track's unconfirmed large move (RT-239): its state from before the move, to roll back
    /// to if the next report refutes it. One per track, fixed size.
    mv: Vec<MoveState>,
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
    }
}

/// A tracker with every table allocated for the configuration's capacity.
pub fn tracker_new(cfg: &TrackerConfig) -> Result<Tracker, TrackError> {
    check_config(cfg)?;
    let n = cfg.max_tracks;
    let index_len = (2 * n).next_power_of_two();
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
        mv: vec![no_move(); n],
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

/// Bytes of memory the tables hold (the columns, rings and index), for the report.
pub fn tracker_bytes(t: &Tracker) -> usize {
    let n = t.cfg.max_tracks;
    n * (4
        + 1
        + 8 * 3
        + 4
        + 1
        + core::mem::size_of::<Filter>()
        + 2
        + 4
        + core::mem::size_of::<CategoryState>())
        + t.anchor.len() * core::mem::size_of::<Filter>()
        + t.ring_m.len() * core::mem::size_of::<Measurement>()
        + t.mv.len() * core::mem::size_of::<MoveState>()
        + t.index.len() * 4
}

/// The address and capacity of every buffer the tracker owns, in a fixed order. A test compares
/// it before and after a run: equal means nothing was reallocated (RULESET C5; a counting
/// allocator needs `unsafe`, which A0 forbids here, so the check is on the buffers themselves).
pub fn tracker_buffers(t: &Tracker) -> Vec<(usize, usize)> {
    fn of<T>(v: &Vec<T>) -> (usize, usize) {
        (v.as_ptr() as usize, v.capacity())
    }
    vec![
        of(&t.key),
        of(&t.state),
        of(&t.first_ns),
        of(&t.confirmed_ns),
        of(&t.last_ns),
        of(&t.expected_ns),
        of(&t.updates),
        of(&t.hits),
        of(&t.cur),
        of(&t.ring_len),
        of(&t.ring_head),
        of(&t.cat),
        of(&t.anchor),
        of(&t.ring_m),
        of(&t.mv),
        of(&t.free),
        of(&t.index),
        of(&t.log.addr),
        of(&t.log.first_ns),
        of(&t.log.confirmed_ns),
        of(&t.log.last_ns),
        of(&t.log.updates),
        of(&t.log.reason),
    ]
}

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
            && (ve * ve + vn * vn).sqrt() <= SPEED_ENVELOPE_MPS
            && cov_ok(&m.vel_cov_m2s2);
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
    clear_move(t, slot);
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
    note_category(t, slot, m);
    clear_move(t, slot);
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

fn emit(t: &mut Tracker, slot: usize, at_ns: i64, flags: u8, out: &mut TrackOutput) {
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
    row.flags |= flags;
    let r95_own = r95;
    if move_active(t, slot) {
        mark_unconfirmed(t, slot, at_ns, &mut row, &mut r95);
    }
    // The reacquisition arming follows the filter's own radius, not the widened one, so a
    // marked row leaves no trace on the rows after the mark is gone.
    let uncertain = r95 > t.cfg.coast_max_r95_m;
    let armed_uncertain = r95_own > t.cfg.coast_max_r95_m;
    if uncertain {
        row.flags |= O_UNCERTAIN;
        t.stats.rows_uncertain += 1;
    }
    // An update row carries the reacquisition flag once; a tick row after an uncertain one
    // arms it for the next update row.
    let armed = if flags & O_TICK == 0 {
        if f.reacq != 0 {
            row.flags |= O_REACQUIRED;
        }
        u8::from(armed_uncertain)
    } else {
        f.reacq | u8::from(armed_uncertain)
    };
    if armed != f.reacq {
        f.reacq = armed;
        put(&mut t.cur, slot, f);
    }
    if row.addr >= RADAR_KEY_BASE {
        row.flags |= O_IDENTITY_UNCONFIRMED;
    }
    if at_ns <= col(&t.disagree_until_ns, slot, i64::MIN) {
        row.flags |= O_RADAR_DISAGREES;
    }
    output_push(out, &row);
}

/// Marks a row of a track with an unconfirmed move (RT-239) and widens its position covariance
/// so that its 95% radius is at least the distance from the old path's prediction to the row's
/// position plus the old path's own radius: the row then covers where the track would be had the
/// move been a forged report. The covariance is scaled, so its shape is kept.
fn mark_unconfirmed(t: &Tracker, slot: usize, at_ns: i64, row: &mut OutRow, r95: &mut f64) {
    row.flags |= O_MOVE_UNCONFIRMED;
    let Some(old) =
        t.mv.get(slot)
            .and_then(|v| filter_output(&v.txn.cur, at_ns))
    else {
        return;
    };
    let wide = filter_row_distance_m(row, &old) + r95_m(&old.pos_cov_m2);
    if wide.is_nan() || wide <= *r95 {
        return;
    }
    if *r95 > 0.0 {
        let k = (wide / *r95) * (wide / *r95);
        for c in &mut row.pos_cov_m2 {
            *c = (f64::from(*c) * k) as f32;
        }
    } else {
        let v = (wide * wide / CHI2_2_95) as f32;
        row.pos_cov_m2 = [v, 0.0, v];
    }
    *r95 = wide;
}

/// The 95% horizontal uncertainty radius of a position covariance `[ee, en, nn]` (m^2), metres:
/// the semi-major axis of the 95% ellipse, so the circle of this radius contains the ellipse.
/// The value behind [`O_UNCERTAIN`], for a consumer to read from a row's `pos_cov_m2`.
pub fn r95_m_of(c: &[f32; 3]) -> f64 {
    r95_m(c)
}

fn r95_m(c: &[f32; 3]) -> f64 {
    let (ee, en, nn) = (f64::from(c[0]), f64::from(c[1]), f64::from(c[2]));
    let half = 0.5 * (ee - nn);
    let lmax = 0.5 * (ee + nn) + (half * half + en * en).sqrt();
    (CHI2_2_95 * lmax).sqrt()
}

/// A measurement's ground speed in 0.1 kt for the jump rule, [`NONE_U16`] without a velocity.
fn gs_dkt(m: &Measurement) -> u16 {
    if m.flags & M_HAS_VEL == 0 {
        return NONE_U16;
    }
    let kt = oa_math::hypot(f64::from(m.vel_east_mps), f64::from(m.vel_north_mps)) / MPS_PER_KT;
    // Saturates one below the absent value.
    (kt * 10.0).round().clamp(0.0, f64::from(NONE_U16 - 1)) as u16
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
        dkt.round().clamp(0.0, f64::from(NONE_U16 - 1)) as u16
    } else {
        NONE_U16
    }
}

/// The track's position to the measurement is not a displacement the speed rule
/// (`oa_adsb::jump_verdict`) allows: the measurement is not the same aircraft's next report.
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
        dist_m: (x * x + y * y).sqrt(),
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
// Budget: two 648-byte filters, twelve 72-byte measurements, 360-byte statistics (328 before
// the four large-move counters of RT-239), six 8-byte times, 8-byte category, 4-byte update
// count and four byte fields.
// One bounded stack checkpoint; no track key changes during an update or its replay.
const _: () = assert!(core::mem::size_of::<TrackTxn>() == 2584);

/// A track's unconfirmed large move (RT-239): the rollback point from before the move, the
/// capture of [`track_txn`] itself, and the time of the report that made the move. One per
/// track, fixed size.
#[derive(Clone, Copy)]
#[repr(C)]
struct MoveState {
    txn: TrackTxn,
    start_ns: i64,
    active: u8,
}
// Budget: one 2584-byte checkpoint, an 8-byte time and a flag, 2600 with alignment. Per track:
// the tracker's tables grow by 2600 bytes (they were 2.2 kB), 10.6 MB at 4096 tracks.
const _: () = assert!(core::mem::size_of::<MoveState>() == 2600);

fn blank_txn() -> TrackTxn {
    TrackTxn {
        cur: blank_filter(),
        anchor: blank_filter(),
        ring: [blank_measurement(); RING_DEPTH],
        stats: TrackerStats::default(),
        first_ns: 0,
        confirmed_ns: i64::MIN,
        last_ns: 0,
        disagree_until_ns: i64::MIN,
        expected_ns: 0,
        newest_update_ns: i64::MIN,
        cat: CategoryState::default(),
        updates: 0,
        state: FREE,
        hits: 0,
        ring_len: 0,
        ring_head: 0,
    }
}

fn no_move() -> MoveState {
    MoveState {
        txn: blank_txn(),
        start_ns: 0,
        active: 0,
    }
}

/// Forgets the track's unconfirmed move (it ended, restarted, resynchronised or was decided).
fn clear_move(t: &mut Tracker, slot: usize) {
    if let Some(m) = t.mv.get_mut(slot) {
        m.active = 0;
    }
}

fn move_active(t: &Tracker, slot: usize) -> bool {
    t.mv.get(slot).is_some_and(|m| m.active != 0)
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
#[repr(C)]
struct OutputTxn {
    overflow: u64,
    lens: [usize; 15],
}
// Budget: one 8-byte counter and fifteen native 8-byte lengths on the supported platform.
const _: () = assert!(core::mem::size_of::<OutputTxn>() == 128);

fn output_txn(out: &TrackOutput) -> OutputTxn {
    OutputTxn {
        overflow: out.overflow,
        lens: [
            out.t_ns.len(),
            out.addr.len(),
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
    let [t_ns, addr, lat_e7, lon_e7, vel_east_mps, vel_north_mps, pos_cov_m2, vel_cov_m2s2, turn_rate_rps, alt_ft, alt_var_ft2, vrate_fpm, mu_h, mu_v, flags] =
        snapshot.lens;
    out.t_ns.truncate(t_ns);
    out.addr.truncate(addr);
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
    let cfg = TrackerConfig {
        confirm_moves: t.cfg.confirm_moves && t.hold_on && col(&t.state, slot, FREE) == CONFIRMED,
        ..t.cfg
    };
    let mut verdict = filter_apply(&mut f, m, &mut t.stats, &cfg);
    let moved = verdict == Applied::Large;
    if moved {
        // Applied as a tracker without the rule applies it, bit for bit.
        let plain = TrackerConfig {
            confirm_moves: false,
            ..cfg
        };
        verdict = filter_apply(&mut f, m, &mut t.stats, &plain);
    }
    match verdict {
        Applied::NumericalRefused => Step::NumericalRefused,
        Applied::Large => Step::Rejected,
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
            clear_move(t, slot);
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

/// The distances of `m` from the track's predicted position and from the old path's (the
/// unconfirmed move's snapshot predicted the same way), at the report's time, metres.
fn move_geometry(t: &Tracker, slot: usize, m: &Measurement) -> (f64, f64) {
    let cur = col(&t.cur, slot, blank_filter());
    let old =
        t.mv.get(slot)
            .map_or(f64::NAN, |v| filter_miss_m(&v.txn.cur, m));
    (filter_miss_m(&cur, m), old)
}

/// Applies a measurement newer than or equal to the track's newest update, with the large-move
/// rule on (RT-239). If the track has an unconfirmed move, the measurement decides it first
/// (`crate::confirm::resolve_decide`): confirm (the mark is cleared), roll back (the track is put
/// back as the snapshot held it and the measurement applied to that), or leave it standing; each
/// counted.
fn update_in_order_held(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) {
    let mut rule = true;
    if t.cfg.confirm_moves && move_active(t, slot) {
        let (cur_m, old_m) = move_geometry(t, slot, m);
        let start = t.mv.get(slot).map_or(0, |v| v.start_ns);
        let window = confirm_window_ns(&t.cfg, col(&t.expected_ns, slot, 0));
        match resolve_decide(start, m.toa_ns, cur_m, old_m, window, &t.cfg) {
            // Too soon to be a second report: applied as it is, the move stays unconfirmed.
            Resolve::Wait => rule = false,
            Resolve::Confirm => {
                clear_move(t, slot);
                t.stats.moves_confirmed += 1;
                rule = false;
            }
            Resolve::Rollback => {
                if let Some(v) = t.mv.get(slot).copied() {
                    restore_track_columns(t, slot, &v.txn);
                }
                clear_move(t, slot);
                t.stats.moves_rolled_back += 1;
                rule = false;
            }
            // The move stands; this report is judged afresh (it may be a move of its own).
            Resolve::Undecided => {
                clear_move(t, slot);
                t.stats.moves_undecided += 1;
            }
        }
    }
    t.hold_on = rule;
    update_in_order(t, slot, m, out);
    t.hold_on = false;
}

/// Applies a measurement newer than or equal to the track's newest update.
fn update_in_order(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) {
    let snapshot = track_txn(t, slot);
    let output = output_txn(out);
    let step = decide(t, slot, m);
    if step == Step::Moved {
        // The state from before the move is the rollback point; every row from here on is marked.
        if let Some(v) = t.mv.get_mut(slot) {
            *v = MoveState {
                txn: snapshot,
                start_ns: m.toa_ns,
                active: 1,
            };
        }
    }
    if !settle(t, slot, step, out) {
        restore_track_txn(t, slot, snapshot);
        restore_output_txn(out, output);
        if step == Step::Moved {
            clear_move(t, slot);
        }
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
    for k in (e + 1)..len {
        let at = ring_ix(t, slot, k);
        if let Some(c) = seq.get_mut(n_seq) {
            *c = col(&t.ring_m, at, blank_measurement());
            n_seq += 1;
        }
    }
    // The run of rejected reports since the newest update belongs to the live filter, not to a
    // state rebuilt from the ring (whose counters are zero): it is put back at the end, unless
    // the replay itself rejected, re-synchronised or restarted the track, when what the
    // chronological order gives stands.
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

#[cfg(kani)]
#[path = "tracker_proofs.rs"]
mod proofs;

/// The first tick of the grid at or after `at_ns`. `at_ns` and the tick are bounded
/// (`TIME_LIMIT_NS`, `MAX_TICK_NS`): no overflow.
fn first_tick_at_or_after(t: &Tracker, at_ns: i64) -> i64 {
    at_ns.div_euclid(t.cfg.tick_ns) * t.cfg.tick_ns
        + if at_ns.rem_euclid(t.cfg.tick_ns) == 0 {
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
                update_late(t, slot, m, out);
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
    let coast_ns = t
        .cfg
        .coast_after_intervals
        .saturating_mul(t.cfg.expected_interval_ns);
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
            let own = col(&t.expected_ns, slot, 0);
            let limit = if own > 0 {
                t.cfg.coast_after_intervals.saturating_mul(own)
            } else {
                coast_ns
            };
            let coast = tick - col(&t.last_ns, slot, 0) > limit;
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
pub(crate) fn check_advance_time(t: &mut Tracker, now_ns: i64) -> bool {
    if now_ns.unsigned_abs() > TIME_LIMIT_NS as u64 {
        if let Some(c) = t.stats.refused.get_mut(REF_TIME) {
            *c += 1;
        }
        return false;
    }
    true
}

/// Advances time to `now_ns` without a measurement: emits the ticks up to it and expires old
/// tracks. Time never goes backwards; an earlier `now_ns` does nothing. The step limit does not
/// apply here: the caller's clock (the replay clock) has its own confirmed rules.
pub fn tracker_advance(t: &mut Tracker, now_ns: i64, out: &mut TrackOutput) {
    if !check_advance_time(t, now_ns) {
        return;
    }
    if now_ns > t.clock_ns {
        if t.next_tick_ns == i64::MIN {
            return;
        }
        emit_ticks(t, now_ns, out);
        t.clock_ns = now_ns;
    }
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
pub fn tracker_predict(t: &Tracker, key: u32, at_ns: i64) -> Option<(OutRow, bool, i64)> {
    tracker_predict_checked(t, key, at_ns).ok()
}

/// Forecast with a typed reason for absence or failure. Earlier query times use the newest
/// stored state, as in `tracker_predict`. This read-only operation does not change counters;
/// callers that discard work must record numerical or unrepresentable failures separately.
pub fn tracker_predict_checked(
    t: &Tracker,
    key: u32,
    at_ns: i64,
) -> Result<(OutRow, bool, i64), PredictError> {
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
    Ok((
        row,
        col(&t.state, slot, FREE) == CONFIRMED,
        col(&t.last_ns, slot, 0),
    ))
}

/// Existence is a key lookup, independent of whether a forecast can be computed.
pub(crate) fn tracker_has_key(t: &Tracker, key: u32) -> bool {
    lookup(t, key).is_some_and(|slot| col(&t.state, slot, FREE) != FREE)
}

/// Whether a keyed track is confirmed, independent of forecastability.
pub(crate) fn tracker_is_confirmed(t: &Tracker, key: u32) -> bool {
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
    use crate::types::{output_new, DEFAULT_CONFIG};

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
}

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
//! The tracker's clock is the newest time of applicability seen. A measurement older than the
//! clock by more than the out-of-order window is counted and dropped. A measurement older than
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
//! coasting when its newest update is older than `coast_after_intervals` expected intervals and
//! dropped (logged, slot freed) when older than `drop_after_ns`.
//!
//! # Fixed-rate output
//!
//! Ticks are whole multiples of `tick_ns` of the time axis. A tick is emitted when the clock
//! passes it (before the measurement that moved the clock is applied) or when the caller
//! advances time; the row is the filter predicted to the tick time. A late measurement changes
//! the ticks after it, not those already emitted.

use crate::filter::{filter_apply, filter_init, filter_output, filter_replay, Applied, Filter};
use crate::hfilter::HFilter;
use crate::params::{MAX_PLAUSIBLE_KT, NS_PER_S, PLAUSIBLE_SLACK_M, RING_DEPTH};
use crate::types::{
    check_config, log_new, output_push, Measurement, OutRow, TrackError, TrackLog, TrackOutput,
    TrackerConfig, TrackerStats, END_DROPPED, END_FLUSHED, END_TENTATIVE, NONE_I32, NO_ADDR,
    O_COAST, O_TICK, REF_ALTITUDE, REF_NO_ADDRESS, REF_POSITION, REF_POS_COV, REF_VELOCITY,
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
    updates: Vec<u32>,
    hits: Vec<u8>,
    cur: Vec<Filter>,
    ring_len: Vec<u8>,
    ring_head: Vec<u8>,
    /// The filter state after the oldest measurement of each track's ring.
    anchor: Vec<Filter>,
    /// `max_tracks * RING_DEPTH` stored measurements.
    ring_m: Vec<Measurement>,
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
        rej_h: 0,
        rej_v: 0,
        has_alt: 0,
        h_mode: 0,
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
        next_tick_ns: i64::MIN,
        high: 0,
        active: 0,
        key: vec![NO_ADDR; n],
        state: vec![FREE; n],
        first_ns: vec![0; n],
        confirmed_ns: vec![i64::MIN; n],
        last_ns: vec![0; n],
        updates: vec![0; n],
        hits: vec![0; n],
        cur: vec![blank_filter(); n],
        ring_len: vec![0; n],
        ring_head: vec![0; n],
        anchor: vec![blank_filter(); n],
        ring_m: vec![blank_measurement(); n * RING_DEPTH],
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
    n * (4 + 1 + 8 * 3 + 4 + 1 + core::mem::size_of::<Filter>() + 2 + 4)
        + t.anchor.len() * core::mem::size_of::<Filter>()
        + t.ring_m.len() * core::mem::size_of::<Measurement>()
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
        of(&t.updates),
        of(&t.hits),
        of(&t.cur),
        of(&t.ring_len),
        of(&t.ring_head),
        of(&t.anchor),
        of(&t.ring_m),
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
fn ring_push(t: &mut Tracker, slot: usize, m: &Measurement) {
    let len = ring_len(t, slot);
    if len == 0 {
        put(&mut t.anchor, slot, col(&t.cur, slot, blank_filter()));
    }
    if len == RING_DEPTH {
        let next = col(&t.ring_m, ring_ix(t, slot, 1), blank_measurement());
        let mut a = col(&t.anchor, slot, blank_filter());
        // The counters were taken when the measurement was first applied.
        let _ = filter_replay(&mut a, &next, &mut TrackerStats::default());
        put(&mut t.anchor, slot, a);
        let head = usize::from(col(&t.ring_head, slot, 0));
        put(&mut t.ring_head, slot, ((head + 1) % RING_DEPTH) as u8);
    } else {
        put(&mut t.ring_len, slot, (len + 1) as u8);
    }
    let k = ring_len(t, slot).saturating_sub(1);
    let at = ring_ix(t, slot, k);
    put(&mut t.ring_m, at, *m);
}

/// The filter state after ring entry `k` (0 is the oldest): the anchor with entries `1..=k`
/// applied again, counters discarded.
fn ring_state(t: &Tracker, slot: usize, k: usize) -> Filter {
    let mut f = col(&t.anchor, slot, blank_filter());
    let mut scratch = TrackerStats::default();
    for j in 1..=k {
        let m = col(&t.ring_m, ring_ix(t, slot, j), blank_measurement());
        let _ = filter_replay(&mut f, &m, &mut scratch);
    }
    f
}

// ---- measurement validation -----------------------------------------------------------

/// Checks a measurement (E5): the first failing rule's `REF_*` index.
pub fn measurement_check(m: &Measurement) -> Result<(), usize> {
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
        let v_ok = m.vel_east_mps.is_finite()
            && m.vel_north_mps.is_finite()
            && m.vel_east_mps.hypot(m.vel_north_mps) < 1500.0
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
    ring_clear(t, slot);
    t.free.push(slot as u32);
    t.active = t.active.saturating_sub(1);
}

/// Starts (or restarts) a tentative track in `slot` from `m`.
fn track_begin(t: &mut Tracker, slot: usize, m: &Measurement) {
    put(&mut t.cur, slot, filter_init(m, t.cfg.h_mode));
    ring_clear(t, slot);
    ring_push(t, slot, m);
    put(&mut t.first_ns, slot, m.toa_ns);
    put(&mut t.last_ns, slot, m.toa_ns);
    put(&mut t.updates, slot, 1);
    put(&mut t.hits, slot, 1);
    put(&mut t.confirmed_ns, slot, i64::MIN);
    put(&mut t.state, slot, TENTATIVE);
    maybe_confirm(t, slot, m.toa_ns);
}

fn maybe_confirm(t: &mut Tracker, slot: usize, at_ns: i64) {
    if col(&t.state, slot, FREE) == TENTATIVE && col(&t.hits, slot, 0) >= t.cfg.init_m {
        put(&mut t.state, slot, CONFIRMED);
        put(&mut t.confirmed_ns, slot, at_ns);
        t.stats.confirmed += 1;
    }
}

fn emit(t: &Tracker, slot: usize, at_ns: i64, flags: u8, out: &mut TrackOutput) {
    let f = col(&t.cur, slot, blank_filter());
    let mut row: OutRow = filter_output(&f, at_ns);
    row.addr = col(&t.key, slot, NO_ADDR);
    row.flags |= flags;
    output_push(out, &row);
}

/// Implied ground speed from the track's position to the measurement exceeds the plausible
/// maximum (with the slack): the measurement is not the same aircraft's next report.
fn implausible(t: &Tracker, slot: usize, m: &Measurement) -> bool {
    let f = col(&t.cur, slot, blank_filter());
    let frame = oa_geo::aeqd::frame_new(f64::from(f.lat_e7) / 1.0e7, f64::from(f.lon_e7) / 1.0e7);
    let (x, y) = oa_geo::aeqd::project(
        &frame,
        f64::from(m.lat_e7) / 1.0e7,
        f64::from(m.lon_e7) / 1.0e7,
    );
    let dt = (m.toa_ns - f.t_ns).max(0) as f64 / NS_PER_S as f64;
    x.hypot(y) > MAX_PLAUSIBLE_KT * MPS_PER_KT * dt + PLAUSIBLE_SLACK_M
}

/// Applies a measurement newer than or equal to the track's newest update.
fn update_in_order(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) {
    if col(&t.state, slot, FREE) == TENTATIVE {
        if m.toa_ns - col(&t.first_ns, slot, 0) > init_window_ns(t) {
            t.stats.init_window_expired += 1;
            track_begin(t, slot, m);
            emit_if_confirmed(t, slot, out);
            return;
        }
        if implausible(t, slot, m) {
            t.stats.init_implausible += 1;
            track_begin(t, slot, m);
            emit_if_confirmed(t, slot, out);
            return;
        }
    }
    let mut f = col(&t.cur, slot, blank_filter());
    match filter_apply(&mut f, m, &mut t.stats) {
        Applied::Rejected => {
            // Keeps the count of rejections in a row, so repeated ones re-initialise.
            put(&mut t.cur, slot, f);
        }
        Applied::Yes => {
            put(&mut t.cur, slot, f);
            ring_push(t, slot, m);
            put(&mut t.last_ns, slot, m.toa_ns);
            let u = col(&t.updates, slot, 0).saturating_add(1);
            put(&mut t.updates, slot, u);
            let h = col(&t.hits, slot, 0).saturating_add(1);
            put(&mut t.hits, slot, h);
            t.stats.applied += 1;
            maybe_confirm(t, slot, m.toa_ns);
            emit_if_confirmed(t, slot, out);
        }
        Applied::Resync => {
            put(&mut t.cur, slot, f);
            ring_clear(t, slot);
            ring_push(t, slot, m);
            put(&mut t.last_ns, slot, m.toa_ns);
            put(&mut t.hits, slot, 1);
            put(&mut t.updates, slot, 1);
            t.stats.applied += 1;
        }
    }
}

fn emit_if_confirmed(t: &Tracker, slot: usize, out: &mut TrackOutput) {
    if col(&t.state, slot, FREE) == CONFIRMED {
        emit(t, slot, col(&t.last_ns, slot, 0), 0, out);
    }
}

/// Applies a measurement older than the track's newest update by re-filtering from the ring.
fn update_late(t: &mut Tracker, slot: usize, m: &Measurement, out: &mut TrackOutput) {
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
    let be = ring_ix(t, slot, e);
    let bm = col(&t.ring_m, be, blank_measurement());
    if bm.toa_ns == m.toa_ns && bm.sensor == m.sensor {
        t.stats.duplicates += 1;
        return;
    }
    // The stored measurements after the base, to be applied again after `m`.
    let mut later = [blank_measurement(); RING_DEPTH];
    let mut n_later = 0usize;
    for k in (e + 1)..len {
        let at = ring_ix(t, slot, k);
        if let Some(c) = later.get_mut(n_later) {
            *c = col(&t.ring_m, at, blank_measurement());
            n_later += 1;
        }
    }
    let bf = ring_state(t, slot, e);
    put(&mut t.ring_len, slot, (e + 1) as u8);
    put(&mut t.cur, slot, bf);
    let mut f = bf;
    let applied = matches!(filter_apply(&mut f, m, &mut t.stats), Applied::Yes);
    if applied {
        put(&mut t.cur, slot, f);
        ring_push(t, slot, m);
        t.stats.applied += 1;
        t.stats.refiltered += 1;
        let u = col(&t.updates, slot, 0).saturating_add(1);
        put(&mut t.updates, slot, u);
        let h = col(&t.hits, slot, 0).saturating_add(1);
        put(&mut t.hits, slot, h);
        maybe_confirm(t, slot, m.toa_ns);
        if col(&t.state, slot, FREE) == CONFIRMED {
            emit(t, slot, m.toa_ns, 0, out);
        }
    }
    let mut newest = col(&t.cur, slot, blank_filter()).t_ns;
    for lm in later.iter().take(n_later) {
        let mut f = col(&t.cur, slot, blank_filter());
        if matches!(filter_apply(&mut f, lm, &mut t.stats), Applied::Yes) {
            put(&mut t.cur, slot, f);
            ring_push(t, slot, lm);
            t.stats.replayed += 1;
            newest = lm.toa_ns;
        }
    }
    put(&mut t.last_ns, slot, newest);
}

/// Offers one measurement. Counters say what became of it; rows for a confirmed track go to
/// `out`.
pub fn tracker_update(t: &mut Tracker, m: &Measurement, out: &mut TrackOutput) {
    t.stats.offered += 1;
    if let Err(why) = measurement_check(m) {
        if let Some(c) = t.stats.refused.get_mut(why) {
            *c += 1;
        }
        return;
    }
    let toa = m.toa_ns;
    if t.clock_ns != i64::MIN && toa < t.clock_ns.saturating_sub(t.cfg.ooo_window_ns) {
        t.stats.late_window += 1;
        return;
    }
    if toa > t.clock_ns {
        if t.next_tick_ns == i64::MIN {
            t.next_tick_ns = toa.div_euclid(t.cfg.tick_ns) * t.cfg.tick_ns
                + if toa.rem_euclid(t.cfg.tick_ns) == 0 {
                    0
                } else {
                    t.cfg.tick_ns
                };
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
            track_begin(t, slot, m);
            emit_if_confirmed(t, slot, out);
        }
        Some(slot) => {
            let last = col(&t.last_ns, slot, 0);
            if toa > last {
                update_in_order(t, slot, m, out);
            } else if toa == last
                && col(
                    &t.ring_m,
                    ring_ix(t, slot, ring_len(t, slot).saturating_sub(1)),
                    blank_measurement(),
                )
                .sensor
                    == m.sensor
            {
                t.stats.duplicates += 1;
            } else if toa == last {
                update_in_order(t, slot, m, out);
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

/// Emits every tick up to and including `limit_ns`.
fn emit_ticks(t: &mut Tracker, limit_ns: i64, out: &mut TrackOutput) {
    if t.next_tick_ns == i64::MIN {
        return;
    }
    let coast_ns = t
        .cfg
        .coast_after_intervals
        .saturating_mul(t.cfg.expected_interval_ns);
    while t.next_tick_ns <= limit_ns {
        let tick = t.next_tick_ns;
        sweep(t, tick);
        for slot in 0..t.high {
            if col(&t.state, slot, FREE) != CONFIRMED {
                continue;
            }
            let coast = tick - col(&t.last_ns, slot, 0) > coast_ns;
            emit(t, slot, tick, O_TICK | if coast { O_COAST } else { 0 }, out);
        }
        t.stats.ticks += 1;
        t.next_tick_ns = tick.saturating_add(t.cfg.tick_ns);
    }
}

/// Advances time to `now_ns` without a measurement: emits the ticks up to it and expires old
/// tracks. Time never goes backwards; an earlier `now_ns` does nothing.
pub fn tracker_advance(t: &mut Tracker, now_ns: i64, out: &mut TrackOutput) {
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
    (0..t.high)
        .filter(|&s| col(&t.state, s, FREE) == CONFIRMED)
        .map(|s| (col(&t.key, s, NO_ADDR), col(&t.last_ns, s, 0)))
        .collect()
}

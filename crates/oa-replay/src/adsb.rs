//! The ADS-B aircraft table: the latest report of every aircraft address seen, and its counts.
//! Core (pure, B1): no I/O, no clock, no allocation after [`new_adsb_table`].
//!
//! A struct-of-arrays of fixed capacity ([`ADSB_CAPACITY`]) with the same open-addressing index
//! as the flight table, keyed by the report's address word (the 24-bit address, plus
//! `oa_adsb::NON_ICAO` for addresses that are not ICAO addresses, so the two never collide). A
//! report for a new address when the table is full is refused and counted, never grown into
//! (C1, IF-R1). A sweep on the cadence of [`ADSB_SWEEP_NS`] of replay time frees the slot of every
//! aircraft whose held report is more than [`ADSB_MAX_AGE_NS`] older than the replay clock, so the
//! table holds the aircraft heard in the last half hour and a run of any length, or a flood of
//! made-up addresses, fills it only for as long as the flood lasts. Freed slots are reused
//! (the free stack, lowest slot first, as in the flight table); the hash does not depend on which
//! slot an aircraft is in.
//!
//! A report whose time is more than [`ADSB_MAX_AHEAD_NS`] after the replay clock is refused and
//! counted ([`AdsbApplied::TooFarAhead`]) before it can take a slot or become the held report, so
//! a feeder with a wrong clock cannot make the real reports of an aircraft look out of order. A
//! recorded frame spans at most a minute of report time and is stamped with its first report, so
//! two minutes is the largest honest lead.
//!
//! Per aircraft the table keeps the latest report: time, latitude, longitude, barometric
//! altitude, ground speed, track, flags and source type as the report gave them (an absent value
//! is the `oa_adsb` `NONE_*` constant, so a report on the ground replaces a held altitude by
//! "absent"). The fields that come from the aircraft object, which readsb prints on some rows
//! only (callsign, squawk, category, the integrity and accuracy fields), are carried forward: a
//! report that has one replaces the held value, a report without one leaves it. A report older
//! than the held one is refused whole and counted (an equal time applies); this is the ADS-B
//! counterpart of OA-RPL-003.
//!
//! A confirmed step of the replay clock shifts every held report time by the step
//! ([`adsb_shift_times`]), so the age of each aircraft, and its place against the clock, are what
//! they were before the step, and marks each aircraft stale until a report is accepted for it.
//!
//! The state is hashed by [`adsb_hash_into`] in address order, so it does not depend on the order
//! aircraft were first seen.

use oa_adsb::{reports_row, Reports, NONE_I32, NONE_U16, NONE_U8};
use oa_sha256::Sha256;

use crate::hash::stale_hash_into;
use crate::index::{index_find, index_insert, index_remove, mix64, new_index, Index};

/// Aircraft the table holds: 65,536. One centre's box sees a few thousand aircraft a day.
pub const ADSB_CAPACITY: usize = 1 << 16;

/// Silence after which an aircraft is evicted: 30 minutes of replay time. Aircraft in range are
/// heard every few seconds; a gap this long means the aircraft landed or left.
pub const ADSB_MAX_AGE_NS: i64 = 30 * 60 * 1_000_000_000;
/// How far past the replay clock a report's time may be: 2 minutes.
pub const ADSB_MAX_AHEAD_NS: i64 = 120 * 1_000_000_000;
/// Replay-time spacing of the sweeps: one minute.
pub const ADSB_SWEEP_NS: i64 = 60 * 1_000_000_000;

/// Tag and version of the ADS-B part of the canonical serialisation.
pub const ADSB_HASH_TAG: &[u8; 5] = b"OAAD1";
/// The tag used instead once an aircraft has been evicted or a report refused for its time: the
/// two counts are then written after the others. A replay with neither hashes as it always did.
pub const ADSB_HASH_TAG_EVICT: &[u8; 5] = b"OAAD2";

/// Running totals of the table, kept for the whole run.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdsbStats {
    /// Reports applied.
    pub applied: u64,
    /// Reports refused because they were older than the aircraft's held report.
    pub out_of_order: u64,
    /// Reports refused because the table was full and the address was new.
    pub refused_full: u64,
    /// Addresses added.
    pub aircraft: u64,
    /// Aircraft evicted for not being heard for [`ADSB_MAX_AGE_NS`].
    pub evicted: u64,
    /// Reports refused because their time was more than [`ADSB_MAX_AHEAD_NS`] past the replay
    /// clock.
    pub future_refused: u64,
}

/// What became of one report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdsbApplied {
    /// Applied; the address was new.
    New,
    /// Applied to a known address.
    Updated,
    /// Older than the held report; refused whole.
    OutOfOrder,
    /// New address and the table is full; refused.
    TableFull,
    /// The report's time is too far past the replay clock; refused.
    TooFarAhead,
    /// The report index was outside the reports (cannot happen from the pipeline).
    BadRow,
}

/// The table. Plain data (A1); all columns have length [`AdsbTable::capacity`]; `occupied` says
/// which slots are in use and `free` is the stack of the others.
// A7.2: cold struct, size guard waived (one per pipeline)
#[derive(Clone, Debug)]
pub struct AdsbTable {
    /// Address word, [`oa_adsb::NON_ICAO`] included.
    pub key: Vec<u32>,
    /// Time of the held report, Unix ns.
    pub time_ns: Vec<i64>,
    /// Latitude, 1e-7 degree.
    pub lat_e7: Vec<i32>,
    /// Longitude, 1e-7 degree.
    pub lon_e7: Vec<i32>,
    /// Barometric altitude, feet; `NONE_I32` if absent.
    pub baro_alt_ft: Vec<i32>,
    /// Ground speed, 0.1 kt; `NONE_U16` if absent.
    pub gs_dkt: Vec<u16>,
    /// Track, 0.1 degree; `NONE_U16` if absent.
    pub track_ddeg: Vec<u16>,
    /// `oa_adsb::F_*` bits of the held report.
    pub flags: Vec<u8>,
    /// Source type of the held report; `NONE_U8` if absent.
    pub source: Vec<u8>,
    /// Callsign (carried forward); all NUL if never seen.
    pub callsign: Vec<[u8; 8]>,
    /// Squawk (carried forward); `NONE_U16` if never seen.
    pub squawk: Vec<u16>,
    /// Emitter category (carried forward); 0 if never seen.
    pub category: Vec<u8>,
    /// Quality fields (carried forward; `NONE_U8` if never seen): `nic`, `nac_p`, `nac_v`, `sil`,
    /// `sil_type`, `version`, `gva`, `sda`, in that order.
    pub quality: Vec<[u8; 8]>,
    /// Reports applied to this aircraft.
    pub reports: Vec<u32>,
    /// Reports for this aircraft refused as older than the held one.
    pub out_of_order: Vec<u32>,
    /// Whether the held report was last updated before a confirmed step of the replay clock and
    /// no accepted report has come since: its time is the time it was shifted to, not the time it
    /// was heard.
    pub stale: Vec<bool>,
    /// Whether slot `i` is in use.
    pub occupied: Vec<bool>,
    /// Free slots, a stack: the next to be used is the last; initially the lowest slot on top.
    pub free: Vec<u32>,
    /// Replay time of the next sweep, or `i64::MIN` before the first frame.
    pub next_sweep_ns: i64,
    /// Slots in use.
    pub live: usize,
    /// Slots in total.
    pub capacity: usize,
    /// Totals.
    pub stats: AdsbStats,
    index: Index,
}

/// An empty table of `capacity` aircraft (use [`ADSB_CAPACITY`]). Everything is allocated here.
pub fn new_adsb_table(capacity: usize) -> AdsbTable {
    let n = capacity;
    AdsbTable {
        key: vec![0; n],
        time_ns: vec![0; n],
        lat_e7: vec![0; n],
        lon_e7: vec![0; n],
        baro_alt_ft: vec![NONE_I32; n],
        gs_dkt: vec![NONE_U16; n],
        track_ddeg: vec![NONE_U16; n],
        flags: vec![0; n],
        source: vec![NONE_U8; n],
        callsign: vec![[0; 8]; n],
        squawk: vec![NONE_U16; n],
        category: vec![0; n],
        quality: vec![[NONE_U8; 8]; n],
        reports: vec![0; n],
        out_of_order: vec![0; n],
        stale: vec![false; n],
        occupied: vec![false; n],
        free: (0..n as u32).rev().collect(),
        next_sweep_ns: i64::MIN,
        live: 0,
        capacity: n,
        stats: AdsbStats::default(),
        index: new_index(n),
    }
}

fn key_hash(k: u32) -> u64 {
    mix64(u64::from(k))
}

/// The slot holding `key`, if any.
pub fn adsb_slot_of(t: &AdsbTable, key: u32) -> Option<usize> {
    index_find(&t.index, key_hash(key), |s| {
        t.key.get(s as usize) == Some(&key)
    })
    .map(|s| s as usize)
}

/// Applies report `i` of `r` at replay time `now_ns`.
pub fn apply_adsb(t: &mut AdsbTable, r: &Reports, i: usize, now_ns: i64) -> AdsbApplied {
    let Some(row) = reports_row(r, i) else {
        return AdsbApplied::BadRow;
    };
    if row.time_ns > now_ns.saturating_add(ADSB_MAX_AHEAD_NS) {
        t.stats.future_refused += 1;
        return AdsbApplied::TooFarAhead;
    }
    let (slot, is_new) = match adsb_slot_of(t, row.icao) {
        Some(s) => (s, false),
        None => {
            let Some(s) = t.free.pop() else {
                t.stats.refused_full += 1;
                return AdsbApplied::TableFull;
            };
            if !index_insert(&mut t.index, key_hash(row.icao), s) {
                t.free.push(s);
                t.stats.refused_full += 1;
                return AdsbApplied::TableFull;
            }
            let s = s as usize;
            set(&mut t.occupied, s, true);
            t.live += 1;
            t.stats.aircraft += 1;
            if let Some(k) = t.key.get_mut(s) {
                *k = row.icao;
            }
            if let Some(v) = t.time_ns.get_mut(s) {
                *v = i64::MIN;
            }
            (s, true)
        }
    };
    if !is_new && t.time_ns.get(slot).is_some_and(|&held| row.time_ns < held) {
        t.stats.out_of_order += 1;
        if let Some(c) = t.out_of_order.get_mut(slot) {
            *c = c.saturating_add(1);
        }
        return AdsbApplied::OutOfOrder;
    }
    set(&mut t.time_ns, slot, row.time_ns);
    set(&mut t.stale, slot, false);
    set(&mut t.lat_e7, slot, row.lat_e7);
    set(&mut t.lon_e7, slot, row.lon_e7);
    set(&mut t.baro_alt_ft, slot, row.baro_alt_ft);
    set(&mut t.gs_dkt, slot, row.gs_dkt);
    set(&mut t.track_ddeg, slot, row.track_ddeg);
    set(&mut t.flags, slot, row.flags);
    set(&mut t.source, slot, row.source);
    if row.callsign != [0; 8] {
        set(&mut t.callsign, slot, row.callsign);
    }
    if row.squawk != NONE_U16 {
        set(&mut t.squawk, slot, row.squawk);
    }
    if row.category != 0 {
        set(&mut t.category, slot, row.category);
    }
    let q = [
        row.nic,
        row.nac_p,
        row.nac_v,
        row.sil,
        row.sil_type,
        row.version,
        row.gva,
        row.sda,
    ];
    if let Some(held) = t.quality.get_mut(slot) {
        for (h, v) in held.iter_mut().zip(q) {
            if v != NONE_U8 {
                *h = v;
            }
        }
    }
    if let Some(c) = t.reports.get_mut(slot) {
        *c = c.saturating_add(1);
    }
    t.stats.applied += 1;
    if is_new {
        AdsbApplied::New
    } else {
        AdsbApplied::Updated
    }
}

/// Frees slot `s`: out of the index, columns blank, back on the free stack.
fn evict(t: &mut AdsbTable, s: usize) {
    let Some(&k) = t.key.get(s) else {
        return;
    };
    let keys = &t.key;
    index_remove(&mut t.index, key_hash(k), s as u32, |o| {
        key_hash(keys.get(o as usize).copied().unwrap_or(0))
    });
    set(&mut t.key, s, 0);
    set(&mut t.time_ns, s, 0);
    set(&mut t.lat_e7, s, 0);
    set(&mut t.lon_e7, s, 0);
    set(&mut t.baro_alt_ft, s, NONE_I32);
    set(&mut t.gs_dkt, s, NONE_U16);
    set(&mut t.track_ddeg, s, NONE_U16);
    set(&mut t.flags, s, 0);
    set(&mut t.source, s, NONE_U8);
    set(&mut t.callsign, s, [0; 8]);
    set(&mut t.squawk, s, NONE_U16);
    set(&mut t.category, s, 0);
    set(&mut t.quality, s, [NONE_U8; 8]);
    set(&mut t.reports, s, 0);
    set(&mut t.out_of_order, s, 0);
    set(&mut t.stale, s, false);
    set(&mut t.occupied, s, false);
    t.free.push(s as u32);
    t.live = t.live.saturating_sub(1);
    t.stats.evicted += 1;
}

/// Frees every aircraft whose held report is more than [`ADSB_MAX_AGE_NS`] older than `now_ns`,
/// visiting slots in slot order. Returns the number freed.
pub fn adsb_sweep(t: &mut AdsbTable, now_ns: i64) -> usize {
    let mut n = 0;
    for s in 0..t.capacity {
        if !t.occupied.get(s).copied().unwrap_or(false) {
            continue;
        }
        let last = t.time_ns.get(s).copied().unwrap_or(0);
        if now_ns.saturating_sub(last) > ADSB_MAX_AGE_NS {
            evict(t, s);
            n += 1;
        }
    }
    n
}

/// Runs a sweep if the cadence says one is due at replay time `now_ns`; the first call only
/// schedules the first. Sweeps fall on multiples of [`ADSB_SWEEP_NS`], so when they run does not
/// depend on when the replay started. Returns the number freed.
pub fn adsb_advance(t: &mut AdsbTable, now_ns: i64) -> usize {
    let next = now_ns
        .div_euclid(ADSB_SWEEP_NS)
        .saturating_add(1)
        .saturating_mul(ADSB_SWEEP_NS);
    if t.next_sweep_ns == i64::MIN {
        t.next_sweep_ns = next;
        return 0;
    }
    if now_ns < t.next_sweep_ns {
        return 0;
    }
    let n = adsb_sweep(t, now_ns);
    t.next_sweep_ns = next;
    n
}

/// The number of aircraft that are stale: last updated before a confirmed step of the replay
/// clock and not updated since.
pub fn adsb_stale_count(t: &AdsbTable) -> usize {
    (0..t.capacity)
        .filter(|&s| {
            t.occupied.get(s).copied().unwrap_or(false) && t.stale.get(s).copied().unwrap_or(false)
        })
        .count()
}

/// Moves every aircraft's report time by `delta_ns`, for a step of the replay clock: the time
/// since each was heard stays what it was. Every aircraft is marked stale until a report is
/// accepted for it; returns the number marked.
pub fn adsb_shift_times(t: &mut AdsbTable, delta_ns: i64) -> usize {
    let mut n = 0;
    for s in 0..t.capacity {
        if t.occupied.get(s).copied().unwrap_or(false) {
            if let Some(v) = t.time_ns.get_mut(s) {
                *v = v.saturating_add(delta_ns);
            }
            if let Some(v) = t.stale.get_mut(s) {
                *v = true;
                n += 1;
            }
        }
    }
    n
}

fn set<T>(col: &mut [T], slot: usize, v: T) {
    if let Some(c) = col.get_mut(slot) {
        *c = v;
    }
}

fn put(h: &mut Sha256, b: &[u8]) {
    // The only failure is a message over 2^61 bytes.
    let _ = h.update(b);
}

/// Writes the table's canonical serialisation to the hasher: the tag [`ADSB_HASH_TAG`], the live
/// count, the totals, then every aircraft in ascending address order with every column, integers
/// little-endian at fixed width. Independent of slot assignment. Sorting allocates; this runs at
/// checkpoints only (C6).
pub fn adsb_hash_into(h: &mut Sha256, t: &AdsbTable) {
    let mut order: Vec<(u32, usize)> = (0..t.capacity)
        .filter(|&s| t.occupied.get(s).copied().unwrap_or(false))
        .filter_map(|s| t.key.get(s).map(|&k| (k, s)))
        .collect();
    order.sort_unstable();
    let extra = t.stats.evicted > 0 || t.stats.future_refused > 0;
    put(
        h,
        if extra {
            ADSB_HASH_TAG_EVICT
        } else {
            ADSB_HASH_TAG
        },
    );
    put(h, &(order.len() as u64).to_le_bytes());
    for v in [
        t.stats.applied,
        t.stats.out_of_order,
        t.stats.refused_full,
        t.stats.aircraft,
    ] {
        put(h, &v.to_le_bytes());
    }
    if extra {
        put(h, &t.stats.evicted.to_le_bytes());
        put(h, &t.stats.future_refused.to_le_bytes());
    }
    for &(k, s) in &order {
        put(h, &k.to_le_bytes());
        if let Some(v) = t.time_ns.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.lat_e7.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.lon_e7.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.baro_alt_ft.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.gs_dkt.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.track_ddeg.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.flags.get(s) {
            put(h, &[*v]);
        }
        if let Some(v) = t.source.get(s) {
            put(h, &[*v]);
        }
        if let Some(v) = t.callsign.get(s) {
            put(h, v);
        }
        if let Some(v) = t.squawk.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.category.get(s) {
            put(h, &[*v]);
        }
        if let Some(v) = t.quality.get(s) {
            put(h, v);
        }
        if let Some(v) = t.reports.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.out_of_order.get(s) {
            put(h, &v.to_le_bytes());
        }
    }
    if adsb_stale_count(t) > 0 {
        stale_hash_into(h, &t.stale, order.iter().map(|&(_, s)| s));
    }
}

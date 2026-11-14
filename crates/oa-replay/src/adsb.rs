//! The ADS-B aircraft table: the latest report of every aircraft address seen, and its counts.
//! Core (pure, B1): no I/O, no clock, no allocation after [`new_adsb_table`].
//!
//! A struct-of-arrays of fixed capacity ([`ADSB_CAPACITY`]) with the same open-addressing index
//! as the flight table, keyed by the report's address word (the 24-bit address, plus
//! `oa_adsb::NON_ICAO` for addresses that are not ICAO addresses, so the two never collide). A
//! report for a new address when the table is full is refused and counted, never grown into
//! (C1, IF-R1). Nothing is evicted: ADS-B tracks here are a day's history, not a live picture, and
//! the capacity is far above the aircraft of one centre's box in a day.
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
//! The state is hashed by [`adsb_hash_into`] in address order, so it does not depend on the order
//! aircraft were first seen.

use oa_adsb::{reports_row, Reports, NONE_I32, NONE_U16, NONE_U8};
use oa_sha256::Sha256;

use crate::index::{index_find, index_insert, mix64, new_index, Index};

/// Aircraft the table holds: 65,536. One centre's box sees a few thousand aircraft a day.
pub const ADSB_CAPACITY: usize = 1 << 16;

/// Tag and version of the ADS-B part of the canonical serialisation.
pub const ADSB_HASH_TAG: &[u8; 5] = b"OAAD1";

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
    /// The report index was outside the reports (cannot happen from the pipeline).
    BadRow,
}

/// The table. Plain data (A1); all columns have length [`AdsbTable::capacity`], the first `live`
/// slots are in use (slots are never freed).
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

/// Applies report `i` of `r`.
pub fn apply_adsb(t: &mut AdsbTable, r: &Reports, i: usize) -> AdsbApplied {
    let Some(row) = reports_row(r, i) else {
        return AdsbApplied::BadRow;
    };
    let (slot, is_new) = match adsb_slot_of(t, row.icao) {
        Some(s) => (s, false),
        None => {
            if t.live >= t.capacity {
                t.stats.refused_full += 1;
                return AdsbApplied::TableFull;
            }
            let s = t.live;
            if !index_insert(&mut t.index, key_hash(row.icao), s as u32) {
                t.stats.refused_full += 1;
                return AdsbApplied::TableFull;
            }
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
    let mut order: Vec<(u32, usize)> = (0..t.live)
        .filter_map(|s| t.key.get(s).map(|&k| (k, s)))
        .collect();
    order.sort_unstable();
    put(h, ADSB_HASH_TAG);
    put(h, &(order.len() as u64).to_le_bytes());
    for v in [
        t.stats.applied,
        t.stats.out_of_order,
        t.stats.refused_full,
        t.stats.aircraft,
    ] {
        put(h, &v.to_le_bytes());
    }
    for (k, s) in order {
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
}

//! The FNS table: the NOTAMs in force, applied from `oa_notam` rows. Core (pure, B1): no I/O, no
//! clock (time is an argument, B4), no allocation after [`new_fns_table`] (C1).
//!
//! **Identity.** A NOTAM is its series, number and year (`A0042/26`). A NOTAM in the local format
//! has no series (47 of the 112 messages of the 2026-11-14 sample) and its number is numbered per
//! location, so its series is taken to be `!` followed by its location code (`!QQA`), which cannot
//! collide with a real series letter. A row with neither a series nor a location has no identity.
//! Rows are applied by type:
//!
//! * new (or no type): stored. A row for a NOTAM already held replaces it unless its `issued`
//!   time is older than the held one's, which is refused whole and counted;
//! * replace: the NOTAM named by the row's referred series, number and year is removed and the
//!   row is stored; a replacement naming nothing held is counted and stored all the same;
//! * cancel: the NOTAM named by the referred series, number and year is removed; the cancelling
//!   NOTAM itself is not stored. A cancellation naming nothing held is counted (it is ordinary
//!   when the target expired first, or was issued before the recording began).
//!
//! A row with no usable identity is not applied. A field that the decoder marked
//! malformed ([`oa_notam::field_error`]) is treated as absent.
//!
//! **What is kept** per NOTAM: the event id, the type, classification, traffic, purpose and scope
//! codes, the issued, start, end and cancelled times, the location, FIR, ICAO location, Q code,
//! the most specific affected feature (kind and designator), airport, runway, navaid and unit,
//! the circle (centre and radius), the altitude limits with their references, and the polygons as
//! a count of polygons and vertices with the bounding box of all vertices (the vertices
//! themselves are not kept: the table has a fixed size and a polygon can have hundreds). The NOTAM
//! text is not kept.
//!
//! **Expiry.** A NOTAM is in force from its start time (a reader asks [`fns_in_force`] at a time)
//! and leaves the table at the first sweep after its end time, or its cancelled time if that is
//! earlier. A NOTAM with no end time (permanent, or with no usable end) leaves after
//! [`FNS_OPEN_AGE_NS`] of replay time without an update. Sweeps run on the cadence of
//! [`crate::tfms::tfms_due`].
//!
//! The state is hashed by [`fns_hash_into`] in key order.

use oa_notam::{
    code_str, field_error, has_field, polygon_vertices, polygons_of, row_at, Field, Notams,
    ALT_UNLIMITED, FLAG_PERMANENT, TYPE_CANCEL, TYPE_REPLACE,
};
use oa_sha256::Sha256;

use crate::index::hash_bytes;
use crate::slots::{new_slots, slot_live, slots_alloc, slots_find, slots_release, Slots};

/// NOTAMs the table holds: 32,768. The United States has a few thousand NOTAMs in force at any
/// time; this is several times that.
pub const FNS_CAPACITY: usize = 1 << 15;
/// Replay time after which a NOTAM with no end time and no update is dropped: 30 days.
pub const FNS_OPEN_AGE_NS: i64 = 30 * 24 * 3600 * 1_000_000_000;
/// Absent time (Unix ms), altitude, latitude and longitude.
pub const FNS_NO_TIME: i64 = i64::MIN;
/// Absent altitude, latitude or longitude in the table's columns.
pub const FNS_NO_NUM: i32 = i32::MIN;

/// Tag and version of the FNS part of the canonical serialisation.
pub const FNS_HASH_TAG: &[u8; 5] = b"OAFN1";

/// Running totals of the table, kept for the whole run.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FnsStats {
    /// Rows applied (stored, or a cancellation that removed a NOTAM).
    pub applied: u64,
    /// NOTAMs added.
    pub created: u64,
    /// Rows that replaced a held NOTAM with the same identity.
    pub reissued: u64,
    /// Held NOTAMs removed by a replacement.
    pub replaced: u64,
    /// Replacements that named no held NOTAM.
    pub replace_unmatched: u64,
    /// Held NOTAMs removed by a cancellation.
    pub cancelled: u64,
    /// Cancellations that named no held NOTAM.
    pub cancel_unmatched: u64,
    /// Rows refused because they were issued before the held NOTAM of the same identity.
    pub out_of_order: u64,
    /// Rows refused because the table was full.
    pub refused_full: u64,
    /// Rows with no usable identity (series or location, number, year).
    pub no_key: u64,
    /// NOTAMs evicted at or after their end time.
    pub expired: u64,
    /// Sweeps run.
    pub sweeps: u64,
}

/// What became of one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnsApplied {
    /// Stored; the NOTAM was new.
    New,
    /// Stored over a held NOTAM of the same identity.
    Reissued,
    /// Stored, and the NOTAM it names was removed.
    Replaced,
    /// A cancellation: the NOTAM it names was removed.
    Cancelled,
    /// A cancellation that names no held NOTAM; nothing changed.
    CancelUnmatched,
    /// Issued before the held NOTAM of the same identity; refused whole.
    OutOfOrder,
    /// The table is full; refused.
    TableFull,
    /// No usable identity; not applied.
    NoKey,
    /// The row index was outside the rows (cannot happen from the pipeline).
    BadRow,
}

/// A NOTAM's identity: series, number, year.
pub type FnsKey = ([u8; 12], u32, u16);

/// The NOTAM table. Plain data (A1); every column has length `slots.capacity`.
// A7.2: cold struct, size guard waived (one per pipeline)
#[derive(Clone, Debug)]
pub struct FnsTable {
    /// Slot bookkeeping, keyed by [`FnsKey`].
    pub slots: Slots,
    /// Totals.
    pub stats: FnsStats,
    /// Series.
    pub series: Vec<[u8; 12]>,
    /// Number.
    pub number: Vec<u32>,
    /// Year.
    pub year: Vec<u16>,
    /// The event's UUID.
    pub event_id: Vec<[u8; 16]>,
    /// `oa_notam::TYPE_NEW` and its neighbours; 0 if absent.
    pub ntype: Vec<u8>,
    /// Classification, traffic, purpose, scope (`oa_notam::CLASS_*` and so on); 0 if absent.
    pub codes: Vec<[u8; 4]>,
    /// `oa_notam::FLAG_PERMANENT` and `FLAG_ESTIMATED`.
    pub flags: Vec<u32>,
    /// Issued, start, end and cancelled time, Unix ms; [`FNS_NO_TIME`] if absent.
    pub times: Vec<[i64; 4]>,
    /// Replay time of the last applied row, ns.
    pub last_update_ns: Vec<i64>,
    /// Location, affected FIR, ICAO location.
    pub location: Vec<[u8; 4]>,
    /// See `location`.
    pub fir: Vec<[u8; 4]>,
    /// See `location`.
    pub icao: Vec<[u8; 4]>,
    /// The Q code.
    pub selection: Vec<[u8; 5]>,
    /// The most specific affected feature kind (`oa_notam::FEAT_*`), 0 for none, and designator.
    pub feat_kind: Vec<u8>,
    /// See `feat_kind`.
    pub feat_desig: Vec<[u8; 12]>,
    /// Airport, runway, navaid, unit designators.
    pub apt: Vec<[u8; 8]>,
    /// See `apt`.
    pub rwy: Vec<[u8; 12]>,
    /// See `apt`.
    pub navaid: Vec<[u8; 8]>,
    /// See `apt`.
    pub unit: Vec<[u8; 8]>,
    /// Circle centre latitude and longitude (1e-7 degree, [`FNS_NO_NUM`] if no circle) and radius
    /// (m, 0 if none).
    pub circle: Vec<[i32; 3]>,
    /// Altitude floor and ceiling, feet ([`FNS_NO_NUM`] if absent, `ALT_UNLIMITED` for UNL).
    pub alt_ft: Vec<[i32; 2]>,
    /// Their references (`oa_notam::REF_*`), 0 if absent.
    pub alt_ref: Vec<[u8; 2]>,
    /// Polygons and vertices of the NOTAM.
    pub polys: Vec<[u32; 2]>,
    /// Bounding box of all polygon vertices: minimum latitude, minimum longitude, maximum
    /// latitude, maximum longitude, 1e-7 degree; [`FNS_NO_NUM`] when there is no polygon.
    pub bbox: Vec<[i32; 4]>,
}

/// An empty table of `capacity` NOTAMs (use [`FNS_CAPACITY`]).
pub fn new_fns_table(capacity: usize) -> FnsTable {
    let n = capacity;
    FnsTable {
        slots: new_slots(n),
        stats: FnsStats::default(),
        series: vec![[0; 12]; n],
        number: vec![0; n],
        year: vec![0; n],
        event_id: vec![[0; 16]; n],
        ntype: vec![0; n],
        codes: vec![[0; 4]; n],
        flags: vec![0; n],
        times: vec![[FNS_NO_TIME; 4]; n],
        last_update_ns: vec![0; n],
        location: vec![[0; 4]; n],
        fir: vec![[0; 4]; n],
        icao: vec![[0; 4]; n],
        selection: vec![[0; 5]; n],
        feat_kind: vec![0; n],
        feat_desig: vec![[0; 12]; n],
        apt: vec![[0; 8]; n],
        rwy: vec![[0; 12]; n],
        navaid: vec![[0; 8]; n],
        unit: vec![[0; 8]; n],
        circle: vec![[FNS_NO_NUM, FNS_NO_NUM, 0]; n],
        alt_ft: vec![[FNS_NO_NUM; 2]; n],
        alt_ref: vec![[0; 2]; n],
        polys: vec![[0; 2]; n],
        bbox: vec![[FNS_NO_NUM; 4]; n],
    }
}

fn key_hash(k: &FnsKey) -> u64 {
    let mut b = [0u8; 18];
    b[..12].copy_from_slice(&k.0);
    b[12..16].copy_from_slice(&k.1.to_le_bytes());
    b[16..18].copy_from_slice(&k.2.to_le_bytes());
    hash_bytes(&b)
}

fn key_of(t: &FnsTable, s: usize) -> Option<FnsKey> {
    Some((*t.series.get(s)?, *t.number.get(s)?, *t.year.get(s)?))
}

/// The slot of NOTAM `k`, if held.
pub fn fns_slot_of(t: &FnsTable, k: &FnsKey) -> Option<usize> {
    slots_find(&t.slots, key_hash(k), |s| key_of(t, s).as_ref() == Some(k))
}

/// Number of NOTAMs held.
pub fn fns_live(t: &FnsTable) -> usize {
    t.slots.live
}

/// NOTAMs held whose start time is at or before `now_ms` and that have not ended or been
/// cancelled by then (an absent start counts as started).
pub fn fns_in_force(t: &FnsTable, now_ms: i64) -> usize {
    (0..t.slots.capacity)
        .filter(|&s| slot_live(&t.slots, s))
        .filter(|&s| {
            let [_, start, end, canc] = t.times.get(s).copied().unwrap_or([FNS_NO_TIME; 4]);
            (start == FNS_NO_TIME || start <= now_ms)
                && (end == FNS_NO_TIME || now_ms <= end)
                && (canc == FNS_NO_TIME || now_ms < canc)
        })
        .count()
}

fn set<T>(col: &mut [T], slot: usize, v: T) {
    if let Some(c) = col.get_mut(slot) {
        *c = v;
    }
}

fn blank(t: &mut FnsTable, s: usize) {
    set(&mut t.series, s, [0; 12]);
    set(&mut t.number, s, 0);
    set(&mut t.year, s, 0);
    set(&mut t.event_id, s, [0; 16]);
    set(&mut t.ntype, s, 0);
    set(&mut t.codes, s, [0; 4]);
    set(&mut t.flags, s, 0);
    set(&mut t.times, s, [FNS_NO_TIME; 4]);
    set(&mut t.last_update_ns, s, 0);
    set(&mut t.location, s, [0; 4]);
    set(&mut t.fir, s, [0; 4]);
    set(&mut t.icao, s, [0; 4]);
    set(&mut t.selection, s, [0; 5]);
    set(&mut t.feat_kind, s, 0);
    set(&mut t.feat_desig, s, [0; 12]);
    set(&mut t.apt, s, [0; 8]);
    set(&mut t.rwy, s, [0; 12]);
    set(&mut t.navaid, s, [0; 8]);
    set(&mut t.unit, s, [0; 8]);
    set(&mut t.circle, s, [FNS_NO_NUM, FNS_NO_NUM, 0]);
    set(&mut t.alt_ft, s, [FNS_NO_NUM; 2]);
    set(&mut t.alt_ref, s, [0; 2]);
    set(&mut t.polys, s, [0; 2]);
    set(&mut t.bbox, s, [FNS_NO_NUM; 4]);
}

fn remove(t: &mut FnsTable, s: usize) {
    slots_release(&mut t.slots, s);
    blank(t, s);
}

/// Whether field `f` of row `i` is present and not marked malformed.
fn usable(n: &Notams, i: usize, f: Field) -> bool {
    has_field(n, i, f) && !field_error(n, i, f)
}

/// Applies row `i` of `n` at replay time `now_ns`.
pub fn apply_fns(t: &mut FnsTable, n: &Notams, i: usize, now_ns: i64) -> FnsApplied {
    let Some(r) = row_at(n, i) else {
        return FnsApplied::BadRow;
    };
    let series: [u8; 12] = if usable(n, i, Field::Series) {
        r.series
    } else if usable(n, i, Field::Location) {
        let mut s = [0u8; 12];
        s[0] = b'!';
        for (d, b) in s.iter_mut().skip(1).zip(r.location) {
            *d = b;
        }
        s
    } else {
        [0; 12]
    };
    if series == [0; 12] || !usable(n, i, Field::Number) || !usable(n, i, Field::Year) {
        t.stats.no_key += 1;
        return FnsApplied::NoKey;
    }
    let key: FnsKey = (series, r.number, r.year);
    let target: Option<FnsKey> = if usable(n, i, Field::Ref) {
        Some((r.ref_series, r.ref_number, r.ref_year))
    } else {
        None
    };
    let ntype = if usable(n, i, Field::Type) {
        r.ntype
    } else {
        0
    };
    if ntype == TYPE_CANCEL {
        let hit = target.as_ref().and_then(|k| fns_slot_of(t, k));
        return match hit {
            Some(s) => {
                remove(t, s);
                t.stats.cancelled += 1;
                t.stats.applied += 1;
                FnsApplied::Cancelled
            }
            None => {
                t.stats.cancel_unmatched += 1;
                FnsApplied::CancelUnmatched
            }
        };
    }
    let issued = if usable(n, i, Field::Issued) {
        r.issued_ms
    } else {
        FNS_NO_TIME
    };
    let held = fns_slot_of(t, &key);
    if let Some(s) = held {
        let held_issued = t
            .times
            .get(s)
            .map_or(FNS_NO_TIME, |x| x.first().copied().unwrap_or(FNS_NO_TIME));
        if issued < held_issued {
            t.stats.out_of_order += 1;
            return FnsApplied::OutOfOrder;
        }
    }
    let mut out = if held.is_some() {
        FnsApplied::Reissued
    } else {
        FnsApplied::New
    };
    if ntype == TYPE_REPLACE {
        match target.as_ref().and_then(|k| fns_slot_of(t, k)) {
            Some(old) if Some(old) != held => {
                remove(t, old);
                t.stats.replaced += 1;
                out = FnsApplied::Replaced;
            }
            Some(_) => {}
            None => t.stats.replace_unmatched += 1,
        }
    }
    let s = match held {
        Some(s) => s,
        None => {
            let Some(s) = slots_alloc(&mut t.slots, key_hash(&key)) else {
                t.stats.refused_full += 1;
                return FnsApplied::TableFull;
            };
            blank(t, s);
            set(&mut t.series, s, key.0);
            set(&mut t.number, s, key.1);
            set(&mut t.year, s, key.2);
            t.stats.created += 1;
            s
        }
    };
    if held.is_some() {
        t.stats.reissued += 1;
    }
    store(t, s, n, i, now_ns, ntype);
    t.stats.applied += 1;
    out
}

/// Writes the row's values to slot `s` (the key is already there). Values the row lacks are
/// blanked, so a re-issue replaces the held NOTAM whole.
fn store(t: &mut FnsTable, s: usize, n: &Notams, i: usize, now_ns: i64, ntype: u8) {
    let Some(r) = row_at(n, i) else { return };
    let time = |f: Field, v: i64| if usable(n, i, f) { v } else { FNS_NO_TIME };
    let code = |f: Field, v: u8| if usable(n, i, f) { v } else { 0 };
    set(&mut t.event_id, s, r.event_id);
    set(&mut t.ntype, s, ntype);
    set(
        &mut t.codes,
        s,
        [
            code(Field::Classification, r.class),
            code(Field::Traffic, r.traffic),
            code(Field::Purpose, r.purpose),
            code(Field::Scope, r.scope),
        ],
    );
    set(&mut t.flags, s, r.flags);
    set(
        &mut t.times,
        s,
        [
            time(Field::Issued, r.issued_ms),
            time(Field::Start, r.start_ms),
            time(Field::End, r.end_ms),
            time(Field::Canceled, r.canceled_ms),
        ],
    );
    set(&mut t.last_update_ns, s, now_ns);
    let name = |f: Field| usable(n, i, f);
    let cp = |name_ok: bool, v: &[u8]| -> Vec<u8> {
        if name_ok {
            v.to_vec()
        } else {
            Vec::new()
        }
    };
    set(
        &mut t.location,
        s,
        pad4(&cp(name(Field::Location), &r.location)),
    );
    set(&mut t.fir, s, pad4(&cp(name(Field::Fir), &r.fir)));
    set(
        &mut t.icao,
        s,
        pad4(&cp(name(Field::IcaoLocation), &r.icao_location)),
    );
    set(
        &mut t.selection,
        s,
        crate::tfms::pad(&cp(name(Field::Selection), &r.selection)),
    );
    set(&mut t.feat_kind, s, r.feat_kind);
    set(&mut t.feat_desig, s, r.feat_desig);
    set(
        &mut t.apt,
        s,
        crate::tfms::pad(&cp(name(Field::Airport), &r.apt)),
    );
    set(
        &mut t.rwy,
        s,
        crate::tfms::pad(&cp(name(Field::Runway), &r.rwy)),
    );
    set(
        &mut t.navaid,
        s,
        crate::tfms::pad(&cp(name(Field::Navaid), &r.navaid)),
    );
    set(
        &mut t.unit,
        s,
        crate::tfms::pad(&cp(name(Field::Unit), &r.unit)),
    );
    let circle = usable(n, i, Field::Centre) && usable(n, i, Field::Radius);
    set(
        &mut t.circle,
        s,
        if circle {
            [
                r.lat_e7,
                r.lon_e7,
                i32::try_from(r.radius_m).unwrap_or(i32::MAX),
            ]
        } else {
            [FNS_NO_NUM, FNS_NO_NUM, 0]
        },
    );
    let alt = |f: Field, v: i32| if usable(n, i, f) { v } else { FNS_NO_NUM };
    set(
        &mut t.alt_ft,
        s,
        [
            alt(Field::Floor, r.floor_ft),
            alt(Field::Ceiling, r.ceil_ft),
        ],
    );
    set(
        &mut t.alt_ref,
        s,
        [
            code(Field::Floor, r.floor_ref),
            code(Field::Ceiling, r.ceil_ref),
        ],
    );
    let (mut polys, mut verts) = (0u32, 0u32);
    let mut bbox = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    if usable(n, i, Field::Polygons) {
        for p in polygons_of(n, i) {
            polys += 1;
            let (lat, lon) = polygon_vertices(n, p);
            for (&la, &lo) in lat.iter().zip(lon) {
                verts += 1;
                bbox = [
                    bbox[0].min(la),
                    bbox[1].min(lo),
                    bbox[2].max(la),
                    bbox[3].max(lo),
                ];
            }
        }
    }
    set(&mut t.polys, s, [polys, verts]);
    set(
        &mut t.bbox,
        s,
        if verts == 0 { [FNS_NO_NUM; 4] } else { bbox },
    );
}

fn pad4(v: &[u8]) -> [u8; 4] {
    crate::tfms::pad(v)
}

/// The time, Unix ms, at which slot `s` leaves the table, if it has one.
fn leaves_at(t: &FnsTable, s: usize) -> Option<i64> {
    let [_, _, end, canc] = t.times.get(s).copied().unwrap_or([FNS_NO_TIME; 4]);
    let permanent = t.flags.get(s).is_some_and(|f| f & FLAG_PERMANENT != 0);
    match (end != FNS_NO_TIME && !permanent, canc != FNS_NO_TIME) {
        (true, true) => Some(end.min(canc)),
        (true, false) => Some(end),
        (false, true) => Some(canc),
        (false, false) => None,
    }
}

/// Evicts every NOTAM past its end or cancelled time at replay time `now_ns`, and every NOTAM
/// with neither that has not been updated for [`FNS_OPEN_AGE_NS`]; returns the number evicted.
pub fn fns_sweep(t: &mut FnsTable, now_ns: i64) -> usize {
    let now_ms = now_ns.div_euclid(1_000_000);
    let mut n = 0;
    for i in 0..t.slots.capacity {
        if !slot_live(&t.slots, i) {
            continue;
        }
        let gone = match leaves_at(t, i) {
            Some(at) => now_ms > at,
            None => {
                now_ns.saturating_sub(t.last_update_ns.get(i).copied().unwrap_or(0))
                    > FNS_OPEN_AGE_NS
            }
        };
        if gone {
            remove(t, i);
            t.stats.expired += 1;
            n += 1;
        }
    }
    t.stats.sweeps += 1;
    n
}

/// Moves the last-update time of every NOTAM by `delta_ns`, for a step of the replay clock.
pub fn fns_shift_times(t: &mut FnsTable, delta_ns: i64) {
    for i in 0..t.slots.capacity {
        if slot_live(&t.slots, i) {
            if let Some(v) = t.last_update_ns.get_mut(i) {
                *v = v.saturating_add(delta_ns);
            }
        }
    }
}

fn put(h: &mut Sha256, b: &[u8]) {
    // The only failure is a message over 2^61 bytes.
    let _ = h.update(b);
}

/// Writes the table's canonical serialisation to the hasher: the tag, the live count, the totals,
/// then every NOTAM in ascending (series, number, year) order with every column, integers
/// little-endian at fixed width. Independent of slot assignment. Sorting allocates; this runs at
/// checkpoints only (C6).
pub fn fns_hash_into(h: &mut Sha256, t: &FnsTable) {
    let mut order: Vec<(FnsKey, usize)> = (0..t.slots.capacity)
        .filter(|&s| slot_live(&t.slots, s))
        .filter_map(|s| key_of(t, s).map(|k| (k, s)))
        .collect();
    order.sort_unstable();
    put(h, FNS_HASH_TAG);
    put(h, &(order.len() as u64).to_le_bytes());
    let st = &t.stats;
    for v in [
        st.applied,
        st.created,
        st.reissued,
        st.replaced,
        st.replace_unmatched,
        st.cancelled,
        st.cancel_unmatched,
        st.out_of_order,
        st.refused_full,
        st.no_key,
        st.expired,
    ] {
        put(h, &v.to_le_bytes());
    }
    for (k, s) in order {
        put(h, &k.0);
        put(h, &k.1.to_le_bytes());
        put(h, &k.2.to_le_bytes());
        if let Some(v) = t.event_id.get(s) {
            put(h, v);
        }
        if let Some(v) = t.ntype.get(s) {
            put(h, &[*v]);
        }
        if let Some(v) = t.codes.get(s) {
            put(h, v);
        }
        if let Some(v) = t.flags.get(s) {
            put(h, &v.to_le_bytes());
        }
        if let Some(v) = t.times.get(s) {
            for x in v {
                put(h, &x.to_le_bytes());
            }
        }
        if let Some(v) = t.last_update_ns.get(s) {
            put(h, &v.to_le_bytes());
        }
        for col in [&t.location, &t.fir, &t.icao] {
            if let Some(v) = col.get(s) {
                put(h, v);
            }
        }
        if let Some(v) = t.selection.get(s) {
            put(h, v);
        }
        if let Some(v) = t.feat_kind.get(s) {
            put(h, &[*v]);
        }
        if let Some(v) = t.feat_desig.get(s) {
            put(h, v);
        }
        for col in [&t.apt, &t.navaid, &t.unit] {
            if let Some(v) = col.get(s) {
                put(h, v);
            }
        }
        if let Some(v) = t.rwy.get(s) {
            put(h, v);
        }
        if let Some(v) = t.circle.get(s) {
            for x in v {
                put(h, &x.to_le_bytes());
            }
        }
        if let Some(v) = t.alt_ft.get(s) {
            for x in v {
                put(h, &x.to_le_bytes());
            }
        }
        if let Some(v) = t.alt_ref.get(s) {
            put(h, v);
        }
        if let Some(v) = t.polys.get(s) {
            for x in v {
                put(h, &x.to_le_bytes());
            }
        }
        if let Some(v) = t.bbox.get(s) {
            for x in v {
                put(h, &x.to_le_bytes());
            }
        }
    }
}

/// The ICAO location text of slot `s` (for readers and reports), empty when absent.
pub fn fns_icao_str(t: &FnsTable, s: usize) -> &str {
    t.icao.get(s).map_or("", |c| code_str(c))
}

/// Whether the NOTAM in slot `s` has unlimited ceiling.
pub fn fns_ceiling_unlimited(t: &FnsTable, s: usize) -> bool {
    t.alt_ft.get(s).is_some_and(|a| a[1] == ALT_UNLIMITED)
}

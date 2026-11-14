#![forbid(unsafe_code)]
//! oa-adsb: reads adsb.lol / readsb `globe_history` day archives into ADS-B position reports.
//! The decode functions are core (pure functions of bytes); [`scan_archive`] and the
//! `oa-adsb-clip` binary are shell (they read streams and write files) (RULESET B1).
//!
//! Source of the format: readsb (wiedehopf/readsb) at commit
//! `88877b96dff8f82cf75a7759f67dccabc85edcfb` (2026-11-11): `README-json.md` section "trace
//! jsons", the writer `sprintTracePoint` and `generateTraceJson`-side code in `json_out.c`
//! (the top-level object), `sprintAircraftObject` for the per-row object, `toString.h`
//! (`addrtype_enum_string`, `sil_type_enum_string`), `readsb.h` (`addrtype_t`,
//! `MODES_NON_ICAO_ADDRESS`) and `track.h` (`struct state`: the fixed-point factors). Data
//! licence: adsb.lol history is ODbL; this crate carries no data.
//!
//! # The trace file
//!
//! A day archive (a tar, often split in parts; see `oa-tar`) holds one gzip-compressed JSON file
//! per aircraft, `traces/<xx>/trace_full_<hex>.json`. The top level is an object:
//! `icao` (six hex digits, `~` prefix when the address is not an ICAO address),
//! `timestamp` (seconds since the epoch, three decimals; the time every row is relative to),
//! `trace` (array of rows), and descriptive keys (`r`, `t`, `dbFlags`, `desc`, `ownOp`, `year`,
//! `version`, `noRegData`) that are ignored. A row, from the writer (`sprintTracePoint`):
//!
//! | idx | content |
//! |---|---|
//! | 0 | seconds after `timestamp`, two decimals |
//! | 1, 2 | latitude, longitude, degrees, six decimals |
//! | 3 | barometric altitude in feet (integer), or `"ground"`, or `null`; when the aircraft has no valid barometric altitude but a valid geometric one, this is the geometric altitude and flag bit 3 is set |
//! | 4 | ground speed, knots, one decimal, or `null` |
//! | 5 | track in degrees (true heading when on the ground), one decimal, or `null` |
//! | 6 | flags: bit 0 stale (no position for 20 s before this one), bit 1 start of a new leg, bit 2 the vertical rate is geometric, bit 3 the altitude is geometric |
//! | 7 | vertical rate fpm (baro, or geometric if bit 2), or `null` |
//! | 8 | aircraft object (see below) or `null` |
//! | 9 | source type string (`adsb_icao`, `adsb_icao_nt`, `adsr_icao`, `tisb_icao`, `adsc`, `mlat`, `other`, `mode_s`, `adsb_other`, `adsr_other`, `tisb_trackfile`, `tisb_other`, `mode_ac`, `unknown`) |
//! | 10 | geometric altitude ft or `null` |
//! | 11 | geometric vertical rate fpm or `null` |
//! | 12 | indicated airspeed kt or `null` |
//! | 13 | roll angle degrees or `null` |
//! | 14 | (only in builds with TRACKS_UUID) receiver id string |
//!
//! Files from before 2022 have only columns 0 to 8. Columns 11 to 14 are read and ignored.
//! The aircraft object is printed only on some rows; its keys are those of `aircraft.json`
//! minus `hex`, `lat`, `lon`: `type`, `flight` (callsign, space padded to eight), `squawk`
//! (four octal digits as text), `category` (two hex digits as text, e.g. `"A3"`), `nic`, `rc`,
//! `version`, `nic_baro`, `nac_p`, `nac_v`, `sil`, `sil_type` (`unknown`, `perhour`,
//! `persample`), `gva`, `sda`, `alert`, `spi` and more; unknown keys are ignored.
//!
//! # Output
//!
//! [`Reports`] is struct-of-arrays, one entry per kept position. Units are fixed point (A9):
//! latitude and longitude in 1e-7 degree (`i32`), time in Unix nanoseconds (`i64`, the
//! sources have 10 ms resolution), altitudes and rates in feet and feet per minute, ground
//! speed in 0.1 kt, track in 0.1 degree. An absent value is the column's `NONE_*` constant.
//! `baro_alt_ft` is absent when the row's altitude is `"ground"` (flag [`F_GROUND`] is set) or
//! geometric (flag bit 3). Fields from the aircraft object are absent on rows without one.
//!
//! # Bounds and refusal
//!
//! Every limit is a constant: [`MAX_TRACE_ROWS`], [`MAX_ROW_LEN`], [`MAX_OBJECT_MEMBERS`],
//! [`MAX_TRACE_BYTES`] (decompressed file), plus the `oa-json` and `oa-tar` limits. A trace
//! that breaks a structural rule (not an object, no `icao`, `timestamp` or `trace`, too many
//! rows, bad gzip, JSON error) is refused whole and counted in [`Stats`]; a row that breaks a
//! value rule (a coordinate out of range, an unknown source type, a malformed object field,
//! ...) is refused alone and counted by reason in [`Stats::rejected`]. Nothing refused reaches
//! the output (E5), and nothing is dropped without being counted.
//!
//! Cold, offline code: A7.2 applies to the records here; [`Reports`] is the quantity
//! collection, one column per field.

mod archive;
mod frames;
mod summary;
mod trace;

pub use archive::{scan_archive, ScanEnd, ScanError, ScanResult};
pub use frames::{
    decode_payload, encode_payload, frame_ranges, PayloadError, DEFAULT_FRAME_MS, MAX_FRAME_MS,
    MAX_ROWS_PER_FRAME, MIN_FRAME_MS, PAYLOAD_HEADER_LEN, PAYLOAD_MAGIC, RECORD_LEN,
};
pub use summary::{summarize, Summary, MAX_JUMP_KT, MIN_JUMP_NM};
pub use trace::{decode_trace, TraceError};

/// Limit: rows in one trace. A busy day's longest real trace is tens of thousands of rows; at
/// about 100 bytes a row the 32 MiB file limit already bounds this near 330 000.
pub const MAX_TRACE_ROWS: usize = 500_000;
/// Limit: elements in one row (the format has 14; one more with TRACKS_UUID).
pub const MAX_ROW_LEN: usize = 32;
/// Limit: members in one aircraft object (the writer emits about 40).
pub const MAX_OBJECT_MEMBERS: usize = 128;
/// Limit: JSON nodes in one trace file, 8 Mi (two `oa-json` defaults): [`MAX_TRACE_ROWS`] rows
/// of 8 nodes each, plus the objects, fit.
pub const MAX_TRACE_NODES: usize = 8 * 1024 * 1024;
/// Limit: decompressed bytes of one trace file, equal to the JSON input limit.
pub const MAX_TRACE_BYTES: usize = oa_json::MAX_INPUT_BYTES;
/// Limit: compressed-to-decompressed ratio of one gzip file. Repetitive trace JSON compresses
/// about 10 to 1; 200 is the ratio limit oa-zip uses for archives.
pub const MAX_GZIP_RATIO: u64 = 200;

/// Absent `i32` value (altitudes, rates).
pub const NONE_I32: i32 = i32::MIN;
/// Absent `u16` value (speed, track, squawk).
pub const NONE_U16: u16 = u16::MAX;
/// Absent `u8` value (the aircraft-object integers, source type). Category uses 0 for absent.
pub const NONE_U8: u8 = u8::MAX;

/// Bit 0 of [`Reports::flags`]: the position is stale (readsb flag bit 0).
pub const F_STALE: u8 = 1;
/// Bit 1: start of a new leg (readsb flag bit 1).
pub const F_NEW_LEG: u8 = 2;
/// Bit 2: the vertical rate is geometric, not barometric (readsb flag bit 2).
pub const F_RATE_GEOM: u8 = 4;
/// Bit 3: the altitude of the source row was geometric, not barometric (readsb flag bit 3).
pub const F_ALT_GEOM: u8 = 8;
/// Bit 4 (ours, not readsb's): the row's altitude was `"ground"`.
pub const F_GROUND: u8 = 16;

/// Set in [`Reports::icao`] when the address is not an ICAO address (readsb prints `~`;
/// `MODES_NON_ICAO_ADDRESS` is `1 << 24`).
pub const NON_ICAO: u32 = 1 << 24;

/// `sil_type` values.
pub const SIL_UNKNOWN: u8 = 0;
pub const SIL_PER_HOUR: u8 = 1;
pub const SIL_PER_SAMPLE: u8 = 2;

/// Source type names in readsb's `addrtype_t` order: the index is the stored `source` value.
pub const SOURCE_NAMES: [&str; 14] = [
    "adsb_icao",
    "adsb_icao_nt",
    "adsr_icao",
    "tisb_icao",
    "adsc",
    "mlat",
    "other",
    "mode_s",
    "adsb_other",
    "adsr_other",
    "tisb_trackfile",
    "tisb_other",
    "mode_ac",
    "unknown",
];

/// ADS-B position reports, struct-of-arrays: entry `i` of every column is one report (A3).
#[derive(Debug, Clone, Default, PartialEq)]
// A7.2: collection of columns; the per-row sizes are fixed by the wire record (RECORD_LEN)
pub struct Reports {
    /// Address; [`NON_ICAO`] set for non-ICAO addresses.
    pub icao: Vec<u32>,
    /// Unix time, nanoseconds.
    pub time_ns: Vec<i64>,
    /// Latitude, 1e-7 degree.
    pub lat_e7: Vec<i32>,
    /// Longitude, 1e-7 degree.
    pub lon_e7: Vec<i32>,
    /// Barometric altitude, feet; [`NONE_I32`] if absent (ground, geometric-only, unknown).
    pub baro_alt_ft: Vec<i32>,
    /// Geometric altitude, feet; [`NONE_I32`] if absent.
    pub geom_alt_ft: Vec<i32>,
    /// Ground speed, 0.1 kt; [`NONE_U16`] if absent.
    pub gs_dkt: Vec<u16>,
    /// Track (true heading on the ground), 0.1 degree in [0, 3600); [`NONE_U16`] if absent.
    pub track_ddeg: Vec<u16>,
    /// Vertical rate, feet per minute (geometric if [`F_RATE_GEOM`]); [`NONE_I32`] if absent.
    pub vrate_fpm: Vec<i32>,
    /// `F_*` bits.
    pub flags: Vec<u8>,
    /// Source type: index into [`SOURCE_NAMES`], or [`NONE_U8`] for files without the column.
    pub source: Vec<u8>,
    /// Callsign, ASCII, trailing spaces removed, NUL padded; all NUL if absent.
    pub callsign: Vec<[u8; 8]>,
    /// Squawk as four decimal digits read as a number (`"7700"` is 7700); [`NONE_U16`] if
    /// absent.
    pub squawk: Vec<u16>,
    /// Emitter category byte (`"A3"` is 0xA3); 0 if absent.
    pub category: Vec<u8>,
    /// Navigation integrity category (position), 0 to 11.
    pub nic: Vec<u8>,
    /// Navigation accuracy category for position, 0 to 15 (defined 0 to 11).
    pub nac_p: Vec<u8>,
    /// Navigation accuracy category for velocity, 0 to 7 (defined 0 to 4).
    pub nac_v: Vec<u8>,
    /// Source integrity level, 0 to 3.
    pub sil: Vec<u8>,
    /// [`SIL_UNKNOWN`], [`SIL_PER_HOUR`] or [`SIL_PER_SAMPLE`]; [`NONE_U8`] if absent.
    pub sil_type: Vec<u8>,
    /// ADS-B version, 0 to 7; [`NONE_U8`] if absent.
    pub version: Vec<u8>,
    /// Geometric vertical accuracy, 0 to 3.
    pub gva: Vec<u8>,
    /// System design assurance, 0 to 3.
    pub sda: Vec<u8>,
}

/// One report, assembled by the decoder before it is filtered and appended (A1: plain fields).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Row {
    pub time_ns: i64,
    pub icao: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub baro_alt_ft: i32,
    pub geom_alt_ft: i32,
    pub vrate_fpm: i32,
    pub gs_dkt: u16,
    pub track_ddeg: u16,
    pub squawk: u16,
    pub flags: u8,
    pub source: u8,
    pub callsign: [u8; 8],
    pub category: u8,
    pub nic: u8,
    pub nac_p: u8,
    pub nac_v: u8,
    pub sil: u8,
    pub sil_type: u8,
    pub version: u8,
    pub gva: u8,
    pub sda: u8,
}
// Budget: one per decoded row, transient: 8 + 6 * 4 + 3 * 2 + 2 + 8 + 9 = 57, padded to 64.
const _: () = assert!(core::mem::size_of::<Row>() == 64);

/// A [`Row`] with every optional field absent; the decoder fills in what the row has.
pub const ROW_EMPTY: Row = Row {
    time_ns: 0,
    icao: 0,
    lat_e7: 0,
    lon_e7: 0,
    baro_alt_ft: NONE_I32,
    geom_alt_ft: NONE_I32,
    vrate_fpm: NONE_I32,
    gs_dkt: NONE_U16,
    track_ddeg: NONE_U16,
    squawk: NONE_U16,
    flags: 0,
    source: NONE_U8,
    callsign: [0; 8],
    category: 0,
    nic: NONE_U8,
    nac_p: NONE_U8,
    nac_v: NONE_U8,
    sil: NONE_U8,
    sil_type: NONE_U8,
    version: NONE_U8,
    gva: NONE_U8,
    sda: NONE_U8,
};

/// Number of reports.
pub fn reports_len(r: &Reports) -> usize {
    r.icao.len()
}

/// Removes every report, keeping the columns' capacity.
pub fn reports_clear(r: &mut Reports) {
    r.icao.clear();
    r.time_ns.clear();
    r.lat_e7.clear();
    r.lon_e7.clear();
    r.baro_alt_ft.clear();
    r.geom_alt_ft.clear();
    r.gs_dkt.clear();
    r.track_ddeg.clear();
    r.vrate_fpm.clear();
    r.flags.clear();
    r.source.clear();
    r.callsign.clear();
    r.squawk.clear();
    r.category.clear();
    r.nic.clear();
    r.nac_p.clear();
    r.nac_v.clear();
    r.sil.clear();
    r.sil_type.clear();
    r.version.clear();
    r.gva.clear();
    r.sda.clear();
}

/// Appends one report.
pub fn reports_push(r: &mut Reports, row: &Row) {
    r.icao.push(row.icao);
    r.time_ns.push(row.time_ns);
    r.lat_e7.push(row.lat_e7);
    r.lon_e7.push(row.lon_e7);
    r.baro_alt_ft.push(row.baro_alt_ft);
    r.geom_alt_ft.push(row.geom_alt_ft);
    r.gs_dkt.push(row.gs_dkt);
    r.track_ddeg.push(row.track_ddeg);
    r.vrate_fpm.push(row.vrate_fpm);
    r.flags.push(row.flags);
    r.source.push(row.source);
    r.callsign.push(row.callsign);
    r.squawk.push(row.squawk);
    r.category.push(row.category);
    r.nic.push(row.nic);
    r.nac_p.push(row.nac_p);
    r.nac_v.push(row.nac_v);
    r.sil.push(row.sil);
    r.sil_type.push(row.sil_type);
    r.version.push(row.version);
    r.gva.push(row.gva);
    r.sda.push(row.sda);
}

/// Report `i` as a [`Row`], or `None` if out of range.
pub fn reports_row(r: &Reports, i: usize) -> Option<Row> {
    Some(Row {
        time_ns: *r.time_ns.get(i)?,
        icao: *r.icao.get(i)?,
        lat_e7: *r.lat_e7.get(i)?,
        lon_e7: *r.lon_e7.get(i)?,
        baro_alt_ft: *r.baro_alt_ft.get(i)?,
        geom_alt_ft: *r.geom_alt_ft.get(i)?,
        vrate_fpm: *r.vrate_fpm.get(i)?,
        gs_dkt: *r.gs_dkt.get(i)?,
        track_ddeg: *r.track_ddeg.get(i)?,
        squawk: *r.squawk.get(i)?,
        flags: *r.flags.get(i)?,
        source: *r.source.get(i)?,
        callsign: *r.callsign.get(i)?,
        category: *r.category.get(i)?,
        nic: *r.nic.get(i)?,
        nac_p: *r.nac_p.get(i)?,
        nac_v: *r.nac_v.get(i)?,
        sil: *r.sil.get(i)?,
        sil_type: *r.sil_type.get(i)?,
        version: *r.version.get(i)?,
        gva: *r.gva.get(i)?,
        sda: *r.sda.get(i)?,
    })
}

/// Appends every report of `src` to `dst`.
pub fn reports_extend(dst: &mut Reports, src: &Reports) {
    for i in 0..reports_len(src) {
        if let Some(row) = reports_row(src, i) {
            reports_push(dst, &row);
        }
    }
}

/// What to keep: a latitude/longitude box and a time window, applied while decoding. A report
/// is kept when its position is inside the box (edges included) and its time is in
/// `[from_ns, to_ns)`. Plain record (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Filter {
    pub lat_min_e7: i32,
    pub lat_max_e7: i32,
    pub lon_min_e7: i32,
    pub lon_max_e7: i32,
    pub from_ns: i64,
    pub to_ns: i64,
}

/// The ZAU (Chicago ARTCC) clip box: 38.5 to 46.5 N, 83.5 to 93.5 W; no time limit.
pub const ZAU_BOX: Filter = Filter {
    lat_min_e7: 385_000_000,
    lat_max_e7: 465_000_000,
    lon_min_e7: -935_000_000,
    lon_max_e7: -835_000_000,
    from_ns: i64::MIN,
    to_ns: i64::MAX,
};

/// Why a row was refused, indexing [`Stats::rejected`].
pub const REJECT_NAMES: [&str; REJECT_KINDS] = [
    "shape", "time", "position", "altitude", "speed", "track", "flags", "rate", "source", "object",
];
/// Number of row-refusal reasons.
pub const REJECT_KINDS: usize = 10;
pub const REJ_SHAPE: usize = 0;
pub const REJ_TIME: usize = 1;
pub const REJ_POSITION: usize = 2;
pub const REJ_ALTITUDE: usize = 3;
pub const REJ_SPEED: usize = 4;
pub const REJ_TRACK: usize = 5;
pub const REJ_FLAGS: usize = 6;
pub const REJ_RATE: usize = 7;
pub const REJ_SOURCE: usize = 8;
pub const REJ_OBJECT: usize = 9;

/// Refusal counts for a scan. Plain record (A1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Stats {
    /// Trace files decoded without a structural error.
    pub traces_read: u64,
    /// Trace files refused whole (gzip, JSON or structure).
    pub traces_refused: u64,
    /// Rows seen in the traces read.
    pub rows_seen: u64,
    /// Rows refused by reason ([`REJECT_NAMES`]).
    pub rejected: [u64; REJECT_KINDS],
    /// Rows outside the box.
    pub outside_box: u64,
    /// Rows inside the box but outside the time window.
    pub outside_time: u64,
    /// Rows kept.
    pub kept: u64,
}

/// Adds the counts of `b` into `a`.
pub fn stats_add(a: &mut Stats, b: &Stats) {
    a.traces_read += b.traces_read;
    a.traces_refused += b.traces_refused;
    a.rows_seen += b.rows_seen;
    for (x, y) in a.rejected.iter_mut().zip(b.rejected.iter()) {
        *x += *y;
    }
    a.outside_box += b.outside_box;
    a.outside_time += b.outside_time;
    a.kept += b.kept;
}

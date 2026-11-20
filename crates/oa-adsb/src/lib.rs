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
//! Files from before 2022 have only columns 0 to 8. Columns 11 and 14 are ignored; column 12
//! (`ias`) and 13 (`roll`) are kept as measurements (below).
//! The aircraft object is printed only on some rows; its keys are those of `aircraft.json`
//! minus `hex`, `lat`, `lon`: `type`, `flight` (callsign, space padded to eight), `squawk`
//! (four octal digits as text), `category` (two hex digits as text, e.g. `"A3"`), `nic`, `rc`,
//! `version`, `nic_baro`, `nac_p`, `nac_v`, `sil`, `sil_type` (`unknown`, `perhour`,
//! `persample`), `gva`, `sda`, `alert`, `spi` and more; unknown keys are ignored. The measurement
//! keys `ws`, `wd`, `oat`, `tat`, `tas`, `ias`, `mach`, `mag_heading`, `true_heading`, `roll` and
//! `track_rate` are kept (next section).
//!
//! # Measurements and what readsb's wind means
//!
//! readsb (`json_out.c`, `sprintAircraftObject`; `track.c`, `calc_wind`, `calc_temp`,
//! `updateDeclination`; `README-json.md`) prints these keys in the aircraft object. Raw from
//! the aircraft's Mode S Comm-B replies (and ADS-B velocity messages): `tas` (knots),
//! `ias` (knots), `mach`, `mag_heading` (degrees from magnetic north), `roll` (degrees, negative
//! left), `track_rate` (degrees per second), `true_heading` (degrees from true north; in the air
//! readsb derives it from `mag_heading` plus the declination of the WMM model at the
//! aircraft's position, altitude and date, refreshed every 5 s, and only when the heading
//! is within 45 degrees of the track). Derived by readsb:
//!
//! * `ws`, `wd`: wind speed (knots) and direction (degrees true, the direction the wind blows
//!   **from**, whole numbers printed with `%.0f`). Computed from true heading, true airspeed,
//!   ground speed and ground track: with `crab = heading - track`, headwind component
//!   `hw = tas - gs * cos(crab)` and crosswind `cw = gs * sin(crab)`, `ws = hypot(hw, cw)`,
//!   `wd = heading + atan2(cw, hw)`. Computed whenever a new true heading, track or TAS
//!   arrives, but only if the aircraft is not on the ground, TAS and ground speed are less
//!   than 2.5 s old and true heading and track less than 1.25 s old, the roll (if known)
//!   is at most 10 degrees and the track rate (if known) at most 0.5 degree per second in
//!   magnitude (readsb notes that wind computed in a turn is often bad), at most once per
//!   0.7 s, and the result is at most 250 kt. A value is printed for up to 60 s after it was
//!   computed, provided the barometric altitude is within 500 ft of the altitude it was
//!   computed at, so a row can show a wind up to a minute old, and rows between computations
//!   repeat it (this crate keeps it per row as printed; it never copies it to rows without it).
//!   The `tas` is rejected when below 0.7 of the indicated airspeed (a wrong register).
//! * `oat`, `tat`: static and total air temperature, whole degrees C, from Mach and TAS:
//!   `oat = 288.15 * (tas / (661.47 * mach))^2 - 273.15`, and
//!   `tat = (oat + 273.15) * (1 + 0.2 * mach^2) - 273.15`. Not computed below Mach 0.395, and results outside -100 to 60 C are
//!   discarded. readsb says the result is typically somewhat inaccurate at lower altitudes
//!   and Mach numbers below 0.5.
//!
//! Known limits: the true heading, hence the wind, inherits the declination model's error;
//! the wind error is roughly `tas * sin(heading error)` plus the speed errors, which add
//! directly, so a wind (a small difference of two large vectors) is relatively worst at low
//! speed (at 450 kt one degree of heading is about 8 kt of wind); a turn or sideslip biases the
//! derived wind and the roll and track-rate gate catches only a turn that is known; heading,
//! TAS and track come from different messages and are not simultaneous (ages above); and the
//! pressure of the aircraft is not given, only its barometric altitude. Values are
//! per-observation and are not smoothed here.
//!
//! # Output
//!
//! [`Reports`] is struct-of-arrays, one entry per kept position. Units are fixed point (A9):
//! latitude and longitude in 1e-7 degree (`i32`), time in Unix nanoseconds (`i64`, the
//! sources have 10 ms resolution), altitudes and rates in feet and feet per minute, ground
//! speed in 0.1 kt, track in 0.1 degree. An absent value is the column's `NONE_*` constant.
//! `baro_alt_ft` is absent when the row's altitude is `"ground"` (flag [`F_GROUND`] is set) or
//! geometric (flag bit 3). Fields from the aircraft object are absent on rows without one. The
//! measurements have their own fixed-point units (see [`Reports`]); `ias` and `roll` also come
//! from the row's columns 12 and 13, the aircraft object overriding them with its extra digits.
//! A malformed measurement is left absent and counted in [`Stats::bad_measurements`]; unlike
//! an aircraft-object field of the older kind, it does not refuse the row.
//!
//! # Bounds and refusal
//!
//! Every limit is a constant: [`MAX_TRACE_ROWS`], [`MAX_ROW_LEN`], [`MAX_OBJECT_MEMBERS`],
//! [`MAX_TRACE_BYTES`] (decompressed file), plus the `oa-json` and `oa-tar` limits. A trace
//! that breaks a structural rule (not an object, no `icao`, `timestamp` or `trace`, too many
//! rows, bad gzip, JSON error) is refused whole and counted in [`Stats`]. A row is refused alone
//! (counted by reason in [`Stats::rejected`]) only when its time, position, shape or flags cannot
//! be read: without them the position has no meaning. Any other field that is malformed, out of
//! range, null or an enumeration value this reader does not know is left absent (`unknown` for the
//! source type and `sil_type`), the row and its position are kept, and the occurrence is counted
//! per field and value ([`Stats::unknown_fields`], [`Stats::unknown_values`]), so a format change
//! in readsb shows up as a count and does not drop aircraft. Traces of the reserved addresses
//! 0x000000 and 0xFFFFFF (misconfigured transponders) are counted and not output. Nothing refused
//! reaches the output (E5), and nothing is dropped without being counted.
//!
//! # Is a displacement possible
//!
//! [`jump_verdict`] judges two reports of one aircraft from the distance, the time gap, the
//! emitter category and the reported ground speeds (see its module for the sources of the
//! limits); it replaces the single speed limit that rejected real fast aircraft. The category it
//! is given comes from [`category_observe`], which makes a cap-raising category persist first.
//!
//! Cold, offline code: A7.2 applies to the records here; [`Reports`] is the quantity
//! collection, one column per field.

mod archive;
mod category;
pub mod cli;
mod frames;
mod speed;
mod summary;
mod trace;

pub use archive::{scan_archive, scan_archive_counted, ScanEnd, ScanError, ScanResult};
pub use category::{category_is_reserved, category_observe, CategoryState, CATEGORY_PERSIST};
pub use frames::{
    decode_payload, decode_payload_lenient, encode_payload, frame_ranges, PayloadCounts,
    PayloadError, DEFAULT_FRAME_MS, LEGACY_MAX_ROWS, LEGACY_PAYLOAD_MAGIC, LEGACY_RECORD_LEN,
    MAX_FRAME_MS, MAX_ROWS_PER_FRAME, MIN_FRAME_MS, PAYLOAD_HEADER_LEN, PAYLOAD_MAGIC,
    PREVIOUS_PAYLOAD_MAGIC, RECORD_LEN, TIME_MAX_NS, TIME_MIN_NS,
};
pub use speed::{
    category_ceiling_kt, category_hard_cap_kt, is_jump, is_reserved_address, jump_verdict,
    JumpPair, JumpVerdict, CAP_HIGH_KT, CAP_ORDINARY_KT, CAP_SPACE_KT, CONSISTENCY_ACCEL_KT_S,
    CONSISTENCY_BASE_KT, CONSISTENCY_FACTOR, SPEED_ENVELOPE_KT, SPEED_ENVELOPE_MPS,
};
pub use summary::{summarize, Summary, MIN_JUMP_NM};
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
/// Absent `i16` value (temperatures, roll, track rate).
pub const NONE_I16: i16 = i16::MIN;
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

/// The stored `source` value for a source type this reader does not know: the index of
/// `unknown` in [`SOURCE_NAMES`].
pub const SOURCE_UNKNOWN: u8 = 13;

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
    /// Navigation integrity category for barometric altitude, 0 or 1; [`NONE_U8`] if absent.
    pub nic_baro: Vec<u8>,
    // Per-observation values: printed on rows with an aircraft object (and `ias`, `roll` also in
    // the row's own columns), never carried forward.
    /// Wind speed derived by readsb, knots; [`NONE_U16`] if absent.
    pub ws_kt: Vec<u16>,
    /// Wind direction derived by readsb, degrees true the wind blows from, 0 to 359; [`NONE_U16`] if absent.
    pub wd_deg: Vec<u16>,
    /// True airspeed, knots; [`NONE_U16`] if absent.
    pub tas_kt: Vec<u16>,
    /// Indicated airspeed, knots; [`NONE_U16`] if absent.
    pub ias_kt: Vec<u16>,
    /// Mach number in 0.001; [`NONE_U16`] if absent.
    pub mach_milli: Vec<u16>,
    /// Magnetic heading, 0.01 degree in [0, 36000); [`NONE_U16`] if absent.
    pub mag_heading_cdeg: Vec<u16>,
    /// True heading, 0.01 degree in [0, 36000); [`NONE_U16`] if absent.
    pub true_heading_cdeg: Vec<u16>,
    /// Outer (static) air temperature derived by readsb, whole degrees C; [`NONE_I16`] if absent.
    pub oat_c: Vec<i16>,
    /// Total air temperature derived by readsb, whole degrees C; [`NONE_I16`] if absent.
    pub tat_c: Vec<i16>,
    /// Roll angle, 0.01 degree, negative is left; [`NONE_I16`] if absent.
    pub roll_cdeg: Vec<i16>,
    /// Rate of change of track, 0.01 degree per second; [`NONE_I16`] if absent.
    pub track_rate_cdps: Vec<i16>,
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
    /// NIC_baro, 0 or 1.
    pub nic_baro: u8,
    pub ws_kt: u16,
    pub wd_deg: u16,
    pub tas_kt: u16,
    pub ias_kt: u16,
    pub mach_milli: u16,
    pub mag_heading_cdeg: u16,
    pub true_heading_cdeg: u16,
    pub oat_c: i16,
    pub tat_c: i16,
    pub roll_cdeg: i16,
    pub track_rate_cdps: i16,
}
// Budget: one per decoded row, transient: 8 + 6 * 4 + 10 * 2 + 4 * 2 + 20 = 80 (the 20 bytes: flags, source, callsign 8, ten quality bytes).
const _: () = assert!(core::mem::size_of::<Row>() == 80);

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
    nic_baro: NONE_U8,
    ws_kt: NONE_U16,
    wd_deg: NONE_U16,
    tas_kt: NONE_U16,
    ias_kt: NONE_U16,
    mach_milli: NONE_U16,
    mag_heading_cdeg: NONE_U16,
    true_heading_cdeg: NONE_U16,
    oat_c: NONE_I16,
    tat_c: NONE_I16,
    roll_cdeg: NONE_I16,
    track_rate_cdps: NONE_I16,
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
    r.nic_baro.clear();
    r.ws_kt.clear();
    r.wd_deg.clear();
    r.tas_kt.clear();
    r.ias_kt.clear();
    r.mach_milli.clear();
    r.mag_heading_cdeg.clear();
    r.true_heading_cdeg.clear();
    r.oat_c.clear();
    r.tat_c.clear();
    r.roll_cdeg.clear();
    r.track_rate_cdps.clear();
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
    r.nic_baro.push(row.nic_baro);
    r.ws_kt.push(row.ws_kt);
    r.wd_deg.push(row.wd_deg);
    r.tas_kt.push(row.tas_kt);
    r.ias_kt.push(row.ias_kt);
    r.mach_milli.push(row.mach_milli);
    r.mag_heading_cdeg.push(row.mag_heading_cdeg);
    r.true_heading_cdeg.push(row.true_heading_cdeg);
    r.oat_c.push(row.oat_c);
    r.tat_c.push(row.tat_c);
    r.roll_cdeg.push(row.roll_cdeg);
    r.track_rate_cdps.push(row.track_rate_cdps);
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
        nic_baro: *r.nic_baro.get(i)?,
        ws_kt: *r.ws_kt.get(i)?,
        wd_deg: *r.wd_deg.get(i)?,
        tas_kt: *r.tas_kt.get(i)?,
        ias_kt: *r.ias_kt.get(i)?,
        mach_milli: *r.mach_milli.get(i)?,
        mag_heading_cdeg: *r.mag_heading_cdeg.get(i)?,
        true_heading_cdeg: *r.true_heading_cdeg.get(i)?,
        oat_c: *r.oat_c.get(i)?,
        tat_c: *r.tat_c.get(i)?,
        roll_cdeg: *r.roll_cdeg.get(i)?,
        track_rate_cdps: *r.track_rate_cdps.get(i)?,
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

/// Appends the reports of `src`, the reports of one frame, to `dst`, unless any of them is timed in
/// a sealed window (`oa_sealed::in_sealed_window`, OA-SEAL-014): then the whole frame is left out,
/// as the pipeline leaves it out, and the number of its reports is returned (0 when the frame is
/// kept); the number is also added to the process-wide count a tool prints.
pub fn reports_extend_unsealed(dst: &mut Reports, src: &Reports) -> u64 {
    let n = reports_len(src);
    if src
        .time_ns
        .iter()
        .take(n)
        .any(|t| oa_sealed::in_sealed_window(*t))
    {
        oa_sealed::note_window_drops(n as u64);
        return n as u64;
    }
    for i in 0..n {
        if let Some(row) = reports_row(src, i) {
            reports_push(dst, &row);
        }
    }
    0
}

/// What a frame dropped because it was received in a sealed window counts for in the drop line:
/// the rows its payload holds when it decodes (at least 1), else 1.
pub fn dropped_frame_units(payload: &[u8]) -> u64 {
    let mut r = Reports::default();
    match decode_payload_lenient(payload, &mut r) {
        Ok(_) => (reports_len(&r) as u64).max(1),
        Err(_) => 1,
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

/// Why a row was refused, indexing [`Stats::rejected`]. Only what the position's meaning needs
/// refuses a row: its time, its position, the shape of the row and the flags (stale and
/// new-leg say whether the position is current and continuous, so a flags value that cannot be
/// read leaves the position without its meaning). Every other field that is malformed, unknown
/// or null is left absent (or `unknown` for an enumeration) and counted in
/// [`Stats::unknown_fields`] and [`Stats::unknown_values`], and the row is kept.
pub const REJECT_NAMES: [&str; REJECT_KINDS] = ["shape", "time", "position", "flags"];
/// Number of row-refusal reasons.
pub const REJECT_KINDS: usize = 4;
pub const REJ_SHAPE: usize = 0;
pub const REJ_TIME: usize = 1;
pub const REJ_POSITION: usize = 2;
pub const REJ_FLAGS: usize = 3;

/// Fields of a row whose unknown, malformed or null values are counted, indexing
/// [`Stats::unknown_fields`] and named by [`FIELD_NAMES`].
pub const FIELD_NAMES: [&str; FIELD_KINDS] = [
    "altitude", "geom_alt", "speed", "track", "rate", "source", "object", "flight", "squawk",
    "category", "nic", "nac_p", "nac_v", "sil", "gva", "sda", "version", "sil_type", "flags",
    "nic_baro",
];
/// Number of counted fields.
pub const FIELD_KINDS: usize = 20;
pub const FLD_ALTITUDE: usize = 0;
pub const FLD_GEOM_ALT: usize = 1;
pub const FLD_SPEED: usize = 2;
pub const FLD_TRACK: usize = 3;
pub const FLD_RATE: usize = 4;
pub const FLD_SOURCE: usize = 5;
pub const FLD_OBJECT: usize = 6;
pub const FLD_FLIGHT: usize = 7;
pub const FLD_SQUAWK: usize = 8;
pub const FLD_CATEGORY: usize = 9;
pub const FLD_NIC: usize = 10;
pub const FLD_NAC_P: usize = 11;
pub const FLD_NAC_V: usize = 12;
pub const FLD_SIL: usize = 13;
pub const FLD_GVA: usize = 14;
pub const FLD_SDA: usize = 15;
pub const FLD_VERSION: usize = 16;
pub const FLD_SIL_TYPE: usize = 17;
/// Flag bits above the four readsb defines (the row is kept with them cleared).
pub const FLD_FLAGS: usize = 18;
pub const FLD_NIC_BARO: usize = 19;

/// Distinct (field, value) pairs [`Stats`] keeps counts for; later ones add to
/// [`Stats::unknown_overflow`].
pub const MAX_UNKNOWN_VALUES: usize = 32;
/// Bytes of a value kept in an [`UnknownValue`]; longer text is cut.
pub const UNKNOWN_TEXT_LEN: usize = 24;

/// How often one value of one field was unknown, malformed or null. Plain record (A1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct UnknownValue {
    /// Index into [`FIELD_NAMES`].
    pub field: u8,
    /// Bytes of `text` in use.
    pub len: u8,
    /// The value as text (a string as it was, a number as printed, `null`, or the kind of a
    /// structure), cut at [`UNKNOWN_TEXT_LEN`] bytes.
    pub text: [u8; UNKNOWN_TEXT_LEN],
    /// Times seen; 0 marks a free slot.
    pub count: u64,
}

/// The text of `v` (cut on a character boundary, so always valid).
pub fn unknown_value_text(v: &UnknownValue) -> &str {
    let b = v.text.get(..usize::from(v.len)).unwrap_or(&[]);
    std::str::from_utf8(b).unwrap_or("")
}

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
    /// Rows that would be kept but whose time is in a sealed window (OA-SEAL-014): not output,
    /// counted here.
    pub sealed_window_dropped: u64,
    /// Rows of traces whose address is reserved (0x000000 or 0xFFFFFF, not flagged non-ICAO):
    /// a misconfigured transponder's reports are not one aircraft's, so none are output.
    pub reserved_address: u64,
    /// Rows kept.
    pub kept: u64,
    /// Rows kept in which an optional measurement (wind, temperature, speeds, headings, roll)
    /// was malformed or out of range and was left absent instead of refusing the row.
    pub bad_measurements: u64,
    /// Times a field of a kept row was unknown, malformed or null, by field ([`FIELD_NAMES`]).
    pub unknown_fields: [u64; FIELD_KINDS],
    /// The same by field and value, for the first [`MAX_UNKNOWN_VALUES`] distinct pairs.
    pub unknown_values: [UnknownValue; MAX_UNKNOWN_VALUES],
    /// Occurrences of pairs that did not fit in `unknown_values`.
    pub unknown_overflow: u64,
}

/// Counts one unknown, malformed or null `value` of `field` (an index into [`FIELD_NAMES`]).
pub fn stats_note_unknown(st: &mut Stats, field: usize, value: &str) {
    if let Some(c) = st.unknown_fields.get_mut(field) {
        *c += 1;
    }
    add_unknown(st, field as u8, value.as_bytes(), 1);
}

fn add_unknown(st: &mut Stats, field: u8, text: &[u8], count: u64) {
    // Cut on a character boundary: back off while the next byte is a continuation byte.
    let mut len = text.len().min(UNKNOWN_TEXT_LEN);
    while len > 0 && text.get(len).is_some_and(|&b| b & 0xc0 == 0x80) {
        len -= 1;
    }
    let text = text.get(..len).unwrap_or(&[]);
    for slot in st.unknown_values.iter_mut() {
        if slot.count > 0
            && slot.field == field
            && slot.text.get(..len) == Some(text)
            && usize::from(slot.len) == len
        {
            slot.count += count;
            return;
        }
        if slot.count == 0 {
            slot.field = field;
            slot.len = len as u8;
            for (d, b) in slot.text.iter_mut().zip(text) {
                *d = *b;
            }
            slot.count = count;
            return;
        }
    }
    st.unknown_overflow += count;
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
    a.sealed_window_dropped += b.sealed_window_dropped;
    a.reserved_address += b.reserved_address;
    a.kept += b.kept;
    a.bad_measurements += b.bad_measurements;
    for (x, y) in a.unknown_fields.iter_mut().zip(b.unknown_fields.iter()) {
        *x += *y;
    }
    for v in b.unknown_values.iter().filter(|v| v.count > 0) {
        let text = v.text.get(..usize::from(v.len)).unwrap_or(&[]);
        add_unknown(a, v.field, text, v.count);
    }
    a.unknown_overflow += b.unknown_overflow;
}

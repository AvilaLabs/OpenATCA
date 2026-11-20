//! Core (pure, B1, B7): one deduplicated ADS-B report to the values of one CAT021 Ed 2.4 record
//! ([`AdsbValues`]: the counts of each item's LSB, as Ed 2.4 defines them). The items are the
//! ones the source has; what it does not have is left out and named in [`AdsbConverted::notes`]
//! (one bit per reason, [`ADSB_NOTE_NAMES`]), and a value that does not fit its item is left out,
//! never clipped (E5, E6). The shell counts the notes.
//!
//! # What each item is made from
//!
//! * I010: the shell's data source (SAC 255, SIC 13).
//! * I040: ATP 0 for an ICAO address and 3 (anonymous) for a non-ICAO one; GBS from the data
//!   (1 when the report says ground, 0 when it carries an altitude, so is airborne; a report with
//!   neither sets GBS 0 because the item has no unknown value, and is counted as "ground state
//!   unknown"). ARC 2 (unknown), CL 2 (no information), and RC, RAB, DCR, SIM, TST, SAA and the
//!   error-condition extension at the specification's default 0 are DEFAULTS, not facts: the source
//!   has none of them. The first extension is always sent (it carries GBS and CL).
//! * I080: the 24-bit address.
//! * I073, time of message reception of position: the report's own time, which is when the
//!   receiver network stamped the report. 1/128 s, rounded to nearest; the error is at most
//!   1/256 s. The source has no avionics time of applicability (I071) and no separate time of
//!   reception of the velocity (I075): both are left out. The high-precision I074 is not used.
//! * I131 (not I130): 180/2^30 degrees, three times finer than the 1e-7 degree of the source, so
//!   the position error is at most half an LSB of the source plus half an LSB of the item
//!   (0.84e-7 degree). I130 (2.1e-5 degree) would lose precision the source has. A longitude of
//!   +180 degrees is written as -180 and noted.
//! * I145 flight level from the barometric altitude (pressure altitude, 1013.25 hPa), 1/4 FL
//!   (25 ft), -15 FL to 1500 FL; none on the ground.
//! * I140 geometric height, 6.25 ft, -1500 ft to 150 000 ft.
//! * I155 barometric or I157 geometric vertical rate (whichever the source row says it is),
//!   6.25 ft/min, 15 bits.
//! * I160 airborne ground vector (5.2.30): ground speed (2^-14 NM/s) and track angle (360/2^16
//!   degrees); both or neither, and none for a report on the ground.
//! * I170 identification, I020 emitter category (table below).
//! * I090 quality, in every record (Ed 2.4 5.2.16: it shall be present in every record). The
//!   primary subfield is always written: an unknown NACv or NIC is written as the code 0, which the
//!   ADS-B MOPS defines as "unknown" for both (NACv 0: unknown or 10 m/s or more; NIC 0: Rc
//!   unknown), and each case is counted. NIC is the source's own (readsb's `nic` already folds in
//!   the NIC supplements) and is written as NUCp/NIC; under MOPS version 2 Ed 2.4 says the NIC
//!   values "shall be mapped accordingly", and readsb's value is written unchanged (counted,
//!   UNVERIFIED). readsb's `nic` is a NIC, not a NUCp, so for MOPS version 0 the NUCp is written 0
//!   (unknown) and counted, and a version 0 row is never mapped through the NUCp column of the PIC
//!   table. Beyond the primary subfield, no zero stands in for a value the source lacks: each
//!   extension is written only while every field in it is known: first NICBARO, SIL and NACp;
//!   second SILS, SDA and GVA; third PIC. The chain stops at the first extension with an unknown
//!   field, and each row cut short is counted at that extension. PIC is written (when the chain
//!   reaches it) only where the NIC and the MOPS version map to exactly one PIC row of the Ed 2.4
//!   table ([`pic_of`]; NIC 6 maps to none, because the PIC then depends on NIC supplements that
//!   the NIC alone does not give), and is counted otherwise; PIC 0 is never generated (the
//!   specification: no third extension). An extension whose fields are all known and all 0 is not
//!   sent unless a later one is (5.2.16 allows that), so a decoder cannot tell a known 0 from an
//!   extension not sent. NIC and NACp above 11 and NACv above 4 are outside what the MOPS defines:
//!   they are not encoded and count as unknown.
//! * I210: VN from the report's version, 0 to 2 (Ed 2.4 does not define 3); LTT from the source
//!   type: `adsb_icao` and `adsb_icao_nt` are written 2 (1090 ES), which is ASSUMED from the source
//!   type and not proven (such a report can come through a UAT-to-ES converter); a UAT source would
//!   be 1 (the clip's source types have none). A clip without the source type column, or a type
//!   that does not say (`adsb_other`), is written LTT 0 (other) and counted, because the item has
//!   no unknown code.
//!
//! Emitter category (ADS-B set and code, to I021/020 in Ed 2.4 5.2.5): A1 1, A2 2, A3 3, A4 4, A5
//! 5, A6 6, A7 10, B1 11, B2 12, B3 16, B4 15, B6 13, B7 14, C1 20, C2 21, C3 22, C4 23, C5 24.
//! A0, B0, C0, B5, C6, C7 and the D sets have no code and the report has none.

use oa_ieee::round;

use super::time::{day_time_of_ns, DayTime};

/// Number of notes.
pub const ADSB_NOTES: usize = 33;

pub const ANOTE_TRACK_NUMBER: usize = 0;
pub const ANOTE_TOA_POSITION: usize = 1;
pub const ANOTE_TOMR_VELOCITY: usize = 2;
pub const ANOTE_DESCRIPTOR_DEFAULTS: usize = 3;
pub const ANOTE_ATP_ANONYMOUS: usize = 4;
pub const ANOTE_FL_ABSENT: usize = 5;
pub const ANOTE_FL_RANGE: usize = 6;
pub const ANOTE_HEIGHT_ABSENT: usize = 7;
pub const ANOTE_HEIGHT_RANGE: usize = 8;
pub const ANOTE_RATE_ABSENT: usize = 9;
pub const ANOTE_RATE_RANGE: usize = 10;
pub const ANOTE_VECTOR_ABSENT: usize = 11;
pub const ANOTE_VECTOR_RANGE: usize = 12;
pub const ANOTE_IDENT_ABSENT: usize = 13;
pub const ANOTE_IDENT_CHARS: usize = 14;
pub const ANOTE_CATEGORY_ABSENT: usize = 15;
pub const ANOTE_CATEGORY_NO_CODE: usize = 16;
/// I090 NACv unknown, written as the code 0.
pub const ANOTE_NACV_UNKNOWN: usize = 17;
/// I090 cut short at the first extension.
pub const ANOTE_EXT1_CUT: usize = 18;
/// The NIC and version map to no single PIC row.
pub const ANOTE_PIC_UNMAPPED: usize = 19;
/// I090 cut short at the second extension.
pub const ANOTE_EXT2_CUT: usize = 20;
pub const ANOTE_MOPS_ABSENT: usize = 21;
pub const ANOTE_MOPS_VERSION: usize = 22;
pub const ANOTE_LON_WRAPPED: usize = 23;
/// LTT written 0: the clip has no source type column.
pub const ANOTE_LTT_NO_COLUMN: usize = 24;
/// I090 cut short at the third extension.
pub const ANOTE_EXT3_CUT: usize = 25;
pub const ANOTE_QUALITY_RANGE: usize = 26;
pub const ANOTE_VECTOR_GROUND: usize = 27;
/// GBS written 0 with no ground flag and no altitude.
pub const ANOTE_GROUND_UNKNOWN: usize = 28;
/// LTT written 0: the source type does not say which link.
pub const ANOTE_LTT_NOT_SAID: usize = 29;
/// I090 NIC unknown, written as the code 0.
pub const ANOTE_NIC_UNKNOWN: usize = 30;
/// MOPS version 0: readsb gives a NIC, not a NUCp; NUCp written 0.
pub const ANOTE_V0_NUCP: usize = 31;
/// MOPS version 2 NIC written as readsb gives it, UNVERIFIED mapping.
pub const ANOTE_NIC_V2: usize = 32;

/// What each note means, in index order. The first words name the item.
pub const ADSB_NOTE_NAMES: [&str; ADSB_NOTES] = [
    "I161 track number: ADS-B reports carry none (item omitted)",
    "I071 time of applicability for position: the source has no avionics time (item omitted; I073 is sent)",
    "I075 time of message reception of velocity: the source has one time per report (item omitted)",
    "I040 ARC, RC, RAB, DCR, SIM, TST, SAA, the error conditions and CL: the source has none and the item has no absent value; written as the specification's defaults (ARC 2 unknown, CL 2 no information, the rest 0: no differential correction, actual target, not a test target, equipment capable of selected altitude, no range check pending, transponder report, no error); DEFAULTS, not facts about the target",
    "I040 ATP: the address is not an ICAO address (written as 3, anonymous)",
    "I145 flight level: report has no barometric altitude or is on the ground (item omitted)",
    "I145 flight level: outside -15 FL to 1500 FL (item omitted)",
    "I140 geometric height: report has none (item omitted)",
    "I140 geometric height: outside -1500 ft to 150 000 ft (item omitted)",
    "I155/I157 vertical rate: report has none (item omitted)",
    "I155/I157 vertical rate: outside the 15-bit range of the item (item omitted)",
    "I160 airborne ground vector: report lacks the ground speed or the track (item omitted)",
    "I160 airborne ground vector: value outside the range of the item (item omitted)",
    "I170 target identification: report has none (item omitted)",
    "I170 target identification: a character outside A-Z, 0-9 and space (item omitted)",
    "I020 emitter category: report has none (item omitted)",
    "I020 emitter category: the category has no I020 code (item omitted)",
    "I090 NACv unknown: written as the code 0, which the MOPS defines as unknown (or 10 m/s or more)",
    "I090 cut short at the first extension: NICBARO, SIL or NACp unknown (the extension and every one after it are omitted)",
    "I090 PIC: the NIC with the MOPS version maps to no single PIC row of the Ed 2.4 table (NIC 6, or no version), so no PIC is written",
    "I090 cut short at the second extension: SILS, SDA or GVA unknown (the extension and the PIC extension after it are omitted)",
    "I210 MOPS version: report has none (item omitted)",
    "I210 MOPS version: 3 or more, not defined in Ed 2.4 (item omitted)",
    "I131 longitude: +180 degrees (written as -180)",
    "I210 LTT link technology: the clip has no source type column (written 0 = other; the item has no unknown code)",
    "I090 cut short at the third extension: PIC not written although the first two extensions were (see the PIC note)",
    "I090 NIC or NACp above 11 or NACv above 4: outside the values the MOPS defines (not encoded; counts as unknown)",
    "I160 airborne ground vector: report is on the ground and 5.2.30 defines the airborne vector only (item omitted)",
    "I040 GBS: ground state unknown (no ground flag and no altitude in the report); written 0 because the item has no unknown value",
    "I210 LTT link technology: the source type (adsb_other) does not say which link carried the report (written 0 = other; the item has no unknown code)",
    "I090 NIC unknown: written as the code 0, which the MOPS defines as unknown (Rc unknown)",
    "I090 version 0, NUCp not available: readsb's nic is a NIC, not a NUCp, so the NUCp is written 0 (unknown) and the row is not mapped through the NUCp column of the PIC table",
    "I090 NIC with MOPS version 2: Ed 2.4 says version 2 NIC values \"shall be mapped accordingly\"; readsb's NIC is written unchanged, the mapping is UNVERIFIED",
];

/// Which link carried a report, from its source type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold enum, size guard waived
pub enum LinkSource {
    /// `adsb_icao` or `adsb_icao_nt`: 1090 ES.
    Es1090,
    /// A UAT source.
    Uat,
    /// The clip has no source type column.
    NoColumn,
    /// A source type that does not say.
    NotSaid,
}

/// One ADS-B report as the conversion needs it: the optional fields opened out and the address
/// split from its type. A7.2: transient, one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AdsbRow {
    /// Unix nanoseconds.
    pub time_ns: i64,
    /// The 24 address bits.
    pub address: u32,
    /// The address is not an ICAO address.
    pub non_icao: bool,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Pressure altitude, ft.
    pub baro_alt_ft: Option<i32>,
    /// Height above the WGS-84 ellipsoid, ft.
    pub geom_alt_ft: Option<i32>,
    /// The report says the aircraft is on the ground.
    pub ground: bool,
    pub vrate_fpm: Option<i32>,
    /// The vertical rate is geometric, not barometric.
    pub rate_geometric: bool,
    /// Ground speed, 0.1 kt.
    pub gs_dkt: Option<u16>,
    /// Ground track, 0.1 degree.
    pub track_ddeg: Option<u16>,
    /// Eight characters, padded with spaces or NULs; all zero if the report has none.
    pub callsign: [u8; 8],
    /// ADS-B emitter category byte (`0xA3` for A3), 0 if the report has none.
    pub category: u8,
    pub nic: Option<u8>,
    pub nac_p: Option<u8>,
    pub nac_v: Option<u8>,
    pub sil: Option<u8>,
    /// SIL per sample (true) or per hour (false).
    pub sil_per_sample: Option<bool>,
    pub version: Option<u8>,
    pub gva: Option<u8>,
    pub sda: Option<u8>,
    /// NIC_baro (0 or 1); the source has none.
    pub nic_baro: Option<u8>,
    /// The link, from the source type.
    pub link: LinkSource,
}

/// I090 first extension. A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct QualityExt1 {
    pub nicbaro: u8,
    pub sil: u8,
    pub nacp: u8,
}

/// I090 second extension. A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct QualityExt2 {
    pub sils: bool,
    pub sda: u8,
    pub gva: u8,
}

/// The I090 values: the primary subfield (an unknown NACv or NIC as the code 0), and each
/// extension only while every field of it and of those before it is known (`ext2` only with `ext1`, `pic` only with `ext2`). A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct QualityValues {
    pub nacv: u8,
    pub nic: u8,
    pub ext1: Option<QualityExt1>,
    pub ext2: Option<QualityExt2>,
    /// PIC, 1 to 15 (PIC 0 is never generated).
    pub pic: Option<u8>,
}

/// The values of one CAT021 record, in counts of each item's LSB. A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AdsbValues {
    /// I073, 1/128 s since midnight UTC.
    pub time: u32,
    /// I040 ATP.
    pub atp: u8,
    /// I040 GBS.
    pub ground: bool,
    pub address: u32,
    /// I131, counts of 180/2^30 degrees.
    pub lat: i32,
    pub lon: i32,
    /// I140, 6.25 ft.
    pub geometric_height: Option<i16>,
    /// I145, 1/4 FL.
    pub flight_level: Option<i16>,
    /// I155, 6.25 ft/min.
    pub baro_rate: Option<i16>,
    /// I157, 6.25 ft/min.
    pub geometric_rate: Option<i16>,
    /// I160: ground speed (2^-14 NM/s), track (360/2^16 degrees).
    pub ground_vector: Option<(u16, u16)>,
    pub ident: Option<[u8; 8]>,
    pub category: Option<u8>,
    /// I090, in every record.
    pub quality: QualityValues,
    /// I210 VN.
    pub mops_version: Option<u8>,
    /// I210 LTT: 0 other, 1 UAT, 2 1090 ES.
    pub link: u8,
}

/// A converted report. A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AdsbConverted {
    pub day: i64,
    pub values: AdsbValues,
    /// Bit `i` set: note `i` of [`ADSB_NOTE_NAMES`] applies.
    pub notes: u64,
}

/// Why a report was not converted at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold enum, size guard waived
pub enum AdsbRefusal {
    /// Latitude beyond 90 degrees, longitude beyond 180 degrees.
    Position = 0,
    /// The time does not fit a day count.
    Time = 1,
}

const fn bit(i: usize) -> u64 {
    1u64 << i
}

/// Adds one to the counter of every note set in `mask`.
pub fn count_adsb_notes(counts: &mut [u64; ADSB_NOTES], mask: u64) {
    for (i, c) in counts.iter_mut().enumerate() {
        if mask & bit(i) != 0 {
            *c = c.saturating_add(1);
        }
    }
}

/// `x` rounded to nearest, `None` if it is outside `lo..=hi`.
fn count_in(x: f64, lo: i32, hi: i32) -> Option<i32> {
    let r = round(x);
    (r >= f64::from(lo) && r <= f64::from(hi)).then_some(r as i32)
}

/// The I020 code of an ADS-B emitter category byte (see the module documentation).
pub fn emitter_code(category: u8) -> Option<u8> {
    match category {
        0xA1 => Some(1),
        0xA2 => Some(2),
        0xA3 => Some(3),
        0xA4 => Some(4),
        0xA5 => Some(5),
        0xA6 => Some(6),
        0xA7 => Some(10),
        0xB1 => Some(11),
        0xB2 => Some(12),
        0xB3 => Some(16),
        0xB4 => Some(15),
        0xB6 => Some(13),
        0xB7 => Some(14),
        0xC1 => Some(20),
        0xC2 => Some(21),
        0xC3 => Some(22),
        0xC4 => Some(23),
        0xC5 => Some(24),
        _ => None,
    }
}

/// `c` with its trailing NUL octets (the padding of a callsign shorter than eight characters)
/// turned into spaces, the padding of I021/170. A NUL before a character stays and is refused.
fn padded_with_spaces(c: &[u8; 8]) -> [u8; 8] {
    let mut out = *c;
    for b in out.iter_mut().rev() {
        if *b == 0 {
            *b = b' ';
        } else {
            break;
        }
    }
    out
}

/// `A` to `Z`, `0` to `9` and space: the six-bit character set of I021/170.
fn ident_ok(c: &[u8; 8]) -> bool {
    c.iter()
        .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' ')
}

/// The PIC (I021/090 third extension, Ed 2.4 5.2.16) of a NIC under a MOPS version, from the
/// specification's conversion table; `None` when the NIC maps to no single PIC row: no version,
/// MOPS version 0 (readsb's `nic` is a NIC, so the NUCp column is never used), a NIC the version
/// does not define, and NIC 6, which is PIC 7, 8 or 9 according to the NIC supplements.
pub fn pic_of(version: Option<u8>, nic: u8) -> Option<u8> {
    match version? {
        // NIC 0 to 11, NIC 6 ambiguous: PIC 0, 1, 3, 4, 5, 6, -, 10, 11, 12, 13, 14.
        1 | 2 => match nic {
            0 | 1 => Some(nic),
            2..=5 => Some(nic + 1),
            7..=11 => Some(nic + 3),
            _ => None,
        },
        _ => None,
    }
}

/// The values of `r`, or why it cannot be converted.
pub fn adsb_record(r: &AdsbRow) -> Result<AdsbConverted, AdsbRefusal> {
    if r.lat_e7.unsigned_abs() > 900_000_000 || r.lon_e7.unsigned_abs() > 1_800_000_000 {
        return Err(AdsbRefusal::Position);
    }
    let DayTime { day, units } = day_time_of_ns(r.time_ns).ok_or(AdsbRefusal::Time)?;
    let mut notes = bit(ANOTE_TRACK_NUMBER)
        | bit(ANOTE_TOA_POSITION)
        | bit(ANOTE_TOMR_VELOCITY)
        | bit(ANOTE_DESCRIPTOR_DEFAULTS);
    // 2^30 / 1.8e9 counts per 1e-7 degree; at most 2^30 counts, so the casts cannot truncate.
    let to_count = |e7: i32| round(f64::from(e7) * (1_073_741_824.0 / 1_800_000_000.0)) as i32;
    let lat = to_count(r.lat_e7);
    let mut lon = to_count(r.lon_e7);
    if lon >= 1 << 30 {
        lon = -(1 << 30);
        notes |= bit(ANOTE_LON_WRAPPED);
    }
    if r.non_icao {
        notes |= bit(ANOTE_ATP_ANONYMOUS);
    }
    // GBS from the data: the report says ground, or carries an altitude (so is airborne), or the
    // ground state is unknown and the bit is written 0 for want of an unknown value.
    if !r.ground && r.baro_alt_ft.is_none() && r.geom_alt_ft.is_none() {
        notes |= bit(ANOTE_GROUND_UNKNOWN);
    }
    // I145: 25 ft, -15 FL to 1500 FL.
    let flight_level = match r.baro_alt_ft {
        Some(ft) if !r.ground => match count_in(f64::from(ft) / 25.0, -60, 6000) {
            Some(c) => Some(c as i16),
            None => {
                notes |= bit(ANOTE_FL_RANGE);
                None
            }
        },
        _ => {
            notes |= bit(ANOTE_FL_ABSENT);
            None
        }
    };
    // I140: 6.25 ft, -1500 ft to 150 000 ft.
    let geometric_height = match r.geom_alt_ft {
        Some(ft) => match count_in(f64::from(ft) / 6.25, -240, 24_000) {
            Some(c) => Some(c as i16),
            None => {
                notes |= bit(ANOTE_HEIGHT_RANGE);
                None
            }
        },
        None => {
            notes |= bit(ANOTE_HEIGHT_ABSENT);
            None
        }
    };
    // I155 / I157: 6.25 ft/min in 15 bits two's complement.
    let (mut baro_rate, mut geometric_rate) = (None, None);
    match r.vrate_fpm {
        Some(v) => match count_in(f64::from(v) / 6.25, -16_384, 16_383) {
            Some(c) if r.rate_geometric => geometric_rate = Some(c as i16),
            Some(c) => baro_rate = Some(c as i16),
            None => notes |= bit(ANOTE_RATE_RANGE),
        },
        None => notes |= bit(ANOTE_RATE_ABSENT),
    }
    // I160: ground speed 2^-14 NM/s (0.1 kt = 1/36000 NM/s), track 360/2^16 degrees (0.1 degree
    // = 2^16/3600 counts).
    let ground_vector = match (r.gs_dkt, r.track_ddeg) {
        _ if r.ground => {
            notes |= bit(ANOTE_VECTOR_GROUND);
            None
        }
        (Some(gs), Some(tr)) => {
            let s = count_in(f64::from(gs) * (16_384.0 / 36_000.0), 0, 32_767);
            let a = count_in(f64::from(tr) * (65_536.0 / 3600.0), 0, 65_535);
            match (s, a) {
                (Some(s), Some(a)) if tr < 3600 => Some((s as u16, a as u16)),
                _ => {
                    notes |= bit(ANOTE_VECTOR_RANGE);
                    None
                }
            }
        }
        _ => {
            notes |= bit(ANOTE_VECTOR_ABSENT);
            None
        }
    };
    // The stored callsign is padded with NUL, or with spaces; trailing padding is spaces in the item.
    let callsign = padded_with_spaces(&r.callsign);
    let ident = if callsign == *b"        " {
        notes |= bit(ANOTE_IDENT_ABSENT);
        None
    } else if ident_ok(&callsign) {
        Some(callsign)
    } else {
        notes |= bit(ANOTE_IDENT_CHARS);
        None
    };
    let category = if r.category == 0 {
        notes |= bit(ANOTE_CATEGORY_ABSENT);
        None
    } else {
        let c = emitter_code(r.category);
        if c.is_none() {
            notes |= bit(ANOTE_CATEGORY_NO_CODE);
        }
        c
    };
    // A value outside what the MOPS defines counts as absent (and is noted): NIC and NACp 0 to 11,
    // NACv 0 to 4; SIL, GVA and SDA are two bits.
    let fit = |v: Option<u8>, max: u8| v.filter(|&x| x <= max);
    let (nic, nac_p, nac_v) = (fit(r.nic, 11), fit(r.nac_p, 11), fit(r.nac_v, 4));
    if r.nic.is_some_and(|v| v > 11)
        || r.nac_p.is_some_and(|v| v > 11)
        || r.nac_v.is_some_and(|v| v > 4)
    {
        notes |= bit(ANOTE_QUALITY_RANGE);
    }
    let (sil, gva, sda) = (fit(r.sil, 3), fit(r.gva, 3), fit(r.sda, 3));
    let nic_baro = fit(r.nic_baro, 1);
    let version = r.version.filter(|&v| v <= 2);
    let v0 = r.version == Some(0);
    // The primary subfield is always written; an unknown value is the MOPS code 0, counted.
    let nacv_code = nac_v.unwrap_or_else(|| {
        notes |= bit(ANOTE_NACV_UNKNOWN);
        0
    });
    let nic_code = if v0 {
        // readsb's nic is a NIC, not a NUCp.
        notes |= bit(ANOTE_V0_NUCP);
        0
    } else {
        nic.unwrap_or_else(|| {
            notes |= bit(ANOTE_NIC_UNKNOWN);
            0
        })
    };
    if version == Some(2) && nic.is_some() {
        notes |= bit(ANOTE_NIC_V2);
    }
    let pic = nic.and_then(|n| pic_of(version, n));
    if !v0 && nic.is_some() && pic.is_none() {
        notes |= bit(ANOTE_PIC_UNMAPPED);
    }
    let mut quality = QualityValues {
        nacv: nacv_code,
        nic: nic_code,
        ext1: None,
        ext2: None,
        pic: None,
    };
    match (nic_baro, sil, nac_p) {
        (Some(nicbaro), Some(sil), Some(nacp)) => {
            quality.ext1 = Some(QualityExt1 { nicbaro, sil, nacp });
            match (r.sil_per_sample, sda, gva) {
                (Some(sils), Some(sda), Some(gva)) => {
                    quality.ext2 = Some(QualityExt2 { sils, sda, gva });
                    // PIC 0 means no integrity: no third extension is generated.
                    match pic {
                        Some(p) if p > 0 => quality.pic = Some(p),
                        Some(_) => {}
                        None => notes |= bit(ANOTE_EXT3_CUT),
                    }
                }
                _ => notes |= bit(ANOTE_EXT2_CUT),
            }
        }
        _ => notes |= bit(ANOTE_EXT1_CUT),
    }
    let mut link = 0;
    let mops_version = match r.version {
        None => {
            notes |= bit(ANOTE_MOPS_ABSENT);
            None
        }
        Some(v) if v > 2 => {
            notes |= bit(ANOTE_MOPS_VERSION);
            None
        }
        Some(v) => {
            match r.link {
                LinkSource::Es1090 => link = 2,
                LinkSource::Uat => link = 1,
                LinkSource::NoColumn => notes |= bit(ANOTE_LTT_NO_COLUMN),
                LinkSource::NotSaid => notes |= bit(ANOTE_LTT_NOT_SAID),
            }
            Some(v)
        }
    };
    Ok(AdsbConverted {
        day,
        values: AdsbValues {
            time: units,
            atp: if r.non_icao { 3 } else { 0 },
            ground: r.ground,
            address: r.address & 0xFF_FFFF,
            lat,
            lon,
            geometric_height,
            flight_level,
            baro_rate,
            geometric_rate,
            ground_vector,
            ident,
            category,
            quality,
            mops_version,
            link,
        },
        notes,
    })
}

//! Core (pure, B1, B7): one deduplicated ADS-B report to the values of one CAT021 Ed 2.4 record
//! ([`AdsbValues`]: the counts of each item's LSB, as Ed 2.4 defines them). The items are the
//! ones the source has; what it does not have is left out and named in [`AdsbConverted::notes`]
//! (one bit per reason, [`ADSB_NOTE_NAMES`]), and a value that does not fit its item is left out,
//! never clipped (E5, E6). The shell counts the notes.
//!
//! # What each item is made from
//!
//! * I010: the shell's data source (SAC 255, SIC 13).
//! * I040: ATP 0 for an ICAO address and 3 (anonymous) for a non-ICAO one; ARC 2 (unknown: the
//!   source has no altitude reporting capability); GBS from the report's ground flag; CL 2 (no
//!   information: the source has no confidence level); GBS 0 asserts "ground bit not set" for a
//!   report that is not flagged on the ground, which the source does not say in so many words;
//!   RC, RAB, DCR, SIM, TST, SAA and the
//!   error-condition extension at the specification's default 0 because the source has none of
//!   them. The first extension is always sent (it carries GBS and CL).
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
//! * I090 quality, in every record (5.2.16 "shall be present in every ASTERIX record"): NACv and
//!   NIC in the primary subfield; extensions only when a bit in them is 1 (the encoding rule): the
//!   first holds NICBARO (not in the source and the item has no unknown code: written 0 = not
//!   cross-checked, the conservative value), SIL and NACp; the second SILS, SDA and GVA (SILS has
//!   no unknown code either: a missing SIL type is written 0 = per flight-hour). A field the
//!   source lacks is written 0; that 0 means "unknown" for NIC, NACp, NACv, SIL, SDA and GVA in
//!   the MOPS (ED-102A/DO-260B), not in Ed 2.4. With nothing known the item is the primary
//!   subfield at NACv 0, NIC 0. NIC and NACp above 11 and NACv above 4 are outside what the MOPS
//!   defines and are not encoded. PIC (third extension) is not in the source and is left out.
//!   Under MOPS version 2 Ed 2.4 says the NIC values "shall be mapped accordingly" without a
//!   table in the documents held: the raw NIC is written and the mapping is unverified. How NACv
//!   and NIC are to be read depends on the MOPS version (I210/VN), which is why I210 is sent.
//! * I210: VN from the report's version, 0 to 2 (Ed 2.4 does not define 3); LTT 0 (other),
//!   because the source does not say whether the report came by 1090 ES, UAT or VDL 4 and the item
//!   has no unknown code (0 = other claims none of the three).
//!
//! Emitter category (ADS-B set and code, to I021/020 in Ed 2.4 5.2.5): A1 1, A2 2, A3 3, A4 4, A5
//! 5, A6 6, A7 10, B1 11, B2 12, B3 16, B4 15, B6 13, B7 14, C1 20, C2 21, C3 22, C4 23, C5 24.
//! A0, B0, C0, B5, C6, C7 and the D sets have no code and the report has none.

use oa_ieee::round;

use super::time::{day_time_of_ns, DayTime};

/// Number of notes.
pub const ADSB_NOTES: usize = 30;

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
pub const ANOTE_QUALITY_ABSENT: usize = 17;
pub const ANOTE_NICBARO: usize = 18;
pub const ANOTE_PIC: usize = 19;
pub const ANOTE_QUALITY_PARTIAL: usize = 20;
pub const ANOTE_MOPS_ABSENT: usize = 21;
pub const ANOTE_MOPS_VERSION: usize = 22;
pub const ANOTE_LON_WRAPPED: usize = 23;
pub const ANOTE_LTT: usize = 24;
pub const ANOTE_SILS: usize = 25;
pub const ANOTE_QUALITY_RANGE: usize = 26;
pub const ANOTE_VECTOR_GROUND: usize = 27;
pub const ANOTE_GBS: usize = 28;
pub const ANOTE_NIC_V2: usize = 29;

/// What each note means, in index order. The first words name the item.
pub const ADSB_NOTE_NAMES: [&str; ADSB_NOTES] = [
    "I161 track number: ADS-B reports carry none (item omitted)",
    "I071 time of applicability for position: the source has no avionics time (item omitted; I073 is sent)",
    "I075 time of message reception of velocity: the source has one time per report (item omitted)",
    "I040 DCR, SIM, TST, SAA, RC, RAB and the error conditions: source has none, and the item has no absent value (written 0, the specification's default, which asserts: no differential correction, actual target, not a test target, equipment capable of selected altitude, no range check pending, transponder report, no error; CL written 2, no information)",
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
    "I090 quality indicators: report has none of NIC, NACp, NACv, SIL, SDA, GVA; the item must be in every record, so the primary subfield is written with NACv 0 and NIC 0 (the MOPS unknown codes) and no extension",
    "I090 NICBARO: source has no NIC_baro; the item has no unknown code, so written 0 = not cross-checked, the conservative value",
    "I090 PIC: not in the source (third extension omitted)",
    "I090 NIC, NACp, NACv, SIL, SDA or GVA: at least one the report lacks (written 0; '0 = unknown' for these comes from the MOPS, ED-102A/DO-260B, not from Ed 2.4)",
    "I210 MOPS version: report has none (item omitted)",
    "I210 MOPS version: 3 or more, not defined in Ed 2.4 (item omitted)",
    "I131 longitude: +180 degrees (written as -180)",
    "I210 LTT link technology: source does not say; the item has no unknown code, so written 0 = other (claims neither 1090 ES, UAT nor VDL 4)",
    "I090 SILS: the report has no SIL type; the bit has no unknown code, so written 0 = SIL per flight-hour, the default (a claim)",
    "I090 NIC or NACp above 11 or NACv above 4: outside the values the MOPS defines (not encoded; written 0)",
    "I160 airborne ground vector: report is on the ground and 5.2.30 defines the airborne vector only (item omitted)",
    "I040 GBS: written 0 = ground bit not set for a report whose ground flag is not set; the source flags ground only when it says ground, so 0 asserts \"not on ground\" without the source saying so",
    "I090 NIC with MOPS version 2: Ed 2.4 says version 2 NIC values \"shall be mapped accordingly\"; no mapping is in the documents held, so the raw NIC is written and the mapping is unverified",
];

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
}

/// The I090 values. A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct QualityValues {
    pub nacv: u8,
    pub nic: u8,
    pub nacp: u8,
    pub sil: u8,
    pub sils: bool,
    pub sda: u8,
    pub gva: u8,
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
    /// I090, always present.
    pub quality: Option<QualityValues>,
    /// I210 VN.
    pub mops_version: Option<u8>,
}

/// A converted report. A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AdsbConverted {
    pub day: i64,
    pub values: AdsbValues,
    /// Bit `i` set: note `i` of [`ADSB_NOTE_NAMES`] applies.
    pub notes: u32,
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

const fn bit(i: usize) -> u32 {
    1u32 << i
}

/// Adds one to the counter of every note set in `mask`.
pub fn count_adsb_notes(counts: &mut [u64; ADSB_NOTES], mask: u32) {
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

/// The values of `r`, or why it cannot be converted.
pub fn adsb_record(r: &AdsbRow) -> Result<AdsbConverted, AdsbRefusal> {
    if r.lat_e7.unsigned_abs() > 900_000_000 || r.lon_e7.unsigned_abs() > 1_800_000_000 {
        return Err(AdsbRefusal::Position);
    }
    let DayTime { day, units } = day_time_of_ns(r.time_ns).ok_or(AdsbRefusal::Time)?;
    let mut notes = bit(ANOTE_TRACK_NUMBER)
        | bit(ANOTE_TOA_POSITION)
        | bit(ANOTE_TOMR_VELOCITY)
        | bit(ANOTE_DESCRIPTOR_DEFAULTS)
        | bit(ANOTE_PIC)
        | bit(ANOTE_NICBARO);
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
    if !r.ground {
        notes |= bit(ANOTE_GBS);
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
    let known = [nic, nac_p, nac_v, sil, gva, sda];
    // I090 must be in every record (5.2.16): with nothing known it is the primary subfield at the
    // unknown codes.
    if known.iter().all(Option::is_none) {
        notes |= bit(ANOTE_QUALITY_ABSENT);
    } else if known.iter().any(Option::is_none) {
        notes |= bit(ANOTE_QUALITY_PARTIAL);
    }
    if r.sil_per_sample.is_none() && sil.is_some() {
        notes |= bit(ANOTE_SILS);
    }
    if r.version == Some(2) && nic.is_some() {
        notes |= bit(ANOTE_NIC_V2);
    }
    let quality = Some(QualityValues {
        nacv: nac_v.unwrap_or(0),
        nic: nic.unwrap_or(0),
        nacp: nac_p.unwrap_or(0),
        sil: sil.unwrap_or(0),
        sils: r.sil_per_sample.unwrap_or(false),
        sda: sda.unwrap_or(0),
        gva: gva.unwrap_or(0),
    });
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
            notes |= bit(ANOTE_LTT);
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
        },
        notes,
    })
}

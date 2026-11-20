//! The SMES (Surface Movement Event Service) ASDE-X Cat11 `positionReport`, inside an `asdexMsg`.
//! Pure functions (crate class shell).
//!
//! Structure (FAA FIXM-mediated STDDS data overview v2, section 2.3.1): a root `asdexMsg` with
//! `airport` and a `positionReport` holding `time`, `flightId` (`acAddress`, `aircraftId`,
//! `mode3ACode`), `flightInfo` and `manual` (aircraft type, fix, call number, scratchpads),
//! `movement` (`heading`, `speed`, `vx`, `vy`, `ax`, `ay`), `position` (`altitude`, `latitude`,
//! `longitude`), `runway`, `seqNum`, `track`, `status` (`vs`, `vertRate`, `ud`, `sil`, `nic`,
//! `NACp`), `tgtType` and `enhancedData`. The Cat10 `mlatReport` and `adsbReport` are
//! classified only ([`crate::Kind`]).
//!
//! Units ASSUMED, as for TAIS (see [`crate::tais`]): `latitude` and `longitude` in decimal
//! degrees; `altitude` in feet; `vx`, `vy` and `speed` in metres per second (the FIXM mapping
//! converts velocity from m/s); `heading` in degrees; `vertRate` in feet per minute; `time`
//! ISO-8601 UTC. A delta-encoded message may carry only one of `latitude` and `longitude`; the
//! half is refused as an incomplete pair. An element with `r="1"` is invalidated by the sender.

use crate::kind::{children, in_message_ns, name_of, Kind};
use crate::walk::{elem_offset, prescan, text_index, walk, Got, Sink, Slot, Spec, Val};
use crate::{FieldError, FieldErrorKind, Limits, Stats, StdError};
use oa_xml::{Doc, ElemIx};

/// The fields of a position report, in bit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SmesField {
    Time = 0,
    AcAddress,
    AircraftId,
    Mode3A,
    AcType,
    Heading,
    Speed,
    Vx,
    Vy,
    Altitude,
    Latitude,
    Longitude,
    Runway,
    SeqNum,
    Track,
    VertRate,
    TgtType,
}

/// Number of [`SmesField`] values.
pub const SMES_FIELD_COUNT: usize = 17;

/// Field names, in order: the element path below `positionReport`.
pub const SMES_FIELD_NAMES: [&str; SMES_FIELD_COUNT] = [
    "time",
    "flightId/acAddress",
    "flightId/aircraftId",
    "flightId/mode3ACode",
    "flightInfo/acType",
    "movement/heading",
    "movement/speed",
    "movement/vx",
    "movement/vy",
    "position/altitude",
    "position/latitude",
    "position/longitude",
    "runway",
    "seqNum",
    "track",
    "status/vertRate",
    "tgtType",
];

const VEL: Val = Val::Scaled {
    scale: 2,
    lo: -300_000,
    hi: 300_000,
};

const SPECS: [Spec; SMES_FIELD_COUNT] = [
    Spec {
        path: &["time"],
        val: Val::TimeMs,
    },
    Spec {
        path: &["flightId", "acAddress"],
        val: Val::Hex24,
    },
    Spec {
        path: &["flightId", "aircraftId"],
        val: Val::Callsign,
    },
    Spec {
        path: &["flightId", "mode3ACode"],
        val: Val::Beacon,
    },
    Spec {
        path: &["flightInfo", "acType"],
        val: Val::Code,
    },
    Spec {
        path: &["movement", "heading"],
        val: Val::Heading,
    },
    Spec {
        path: &["movement", "speed"],
        val: Val::Scaled {
            scale: 2,
            lo: 0,
            hi: 300_000,
        },
    },
    Spec {
        path: &["movement", "vx"],
        val: VEL,
    },
    Spec {
        path: &["movement", "vy"],
        val: VEL,
    },
    Spec {
        path: &["position", "altitude"],
        val: Val::Scaled {
            scale: 0,
            lo: -2_000,
            hi: 100_000,
        },
    },
    Spec {
        path: &["position", "latitude"],
        val: Val::DegE7 { max_deg: 90 },
    },
    Spec {
        path: &["position", "longitude"],
        val: Val::DegE7 { max_deg: 180 },
    },
    Spec {
        path: &["runway"],
        val: Val::Runway,
    },
    Spec {
        path: &["seqNum"],
        val: Val::Uint(10),
    },
    Spec {
        path: &["track"],
        val: Val::Uint(6),
    },
    Spec {
        path: &["status", "vertRate"],
        val: Val::Int {
            lo: -30_000,
            hi: 30_000,
        },
    },
    Spec {
        path: &["tgtType"],
        val: Val::Code,
    },
];

/// The message-level fields, read with the same walker as a report's: `airport`.
const ROOT_SPECS: [Spec; 1] = [Spec {
    path: &["airport"],
    val: Val::Code,
}];
const ROOT_NAMES: [&str; 1] = ["airport"];
/// Under the root, the reports are read apart.
const ROOT_SKIP: [&[&str]; 1] = [&["positionReport"]];

/// Known elements deliberately not read.
const SKIP: [&[&str]; 12] = [
    &["flightInfo", "fix"],
    &["manual"],
    &["movement", "ax"],
    &["movement", "ay"],
    &["status", "vs"],
    &["status", "ud"],
    &["status", "sil"],
    &["status", "nic"],
    &["status", "NACp"],
    &["enhancedData"],
    &["wake"],
    &["flightInfo", "wake"],
];

/// One `positionReport` in fixed units; see [`crate::tais::TaisRecord`] for the meaning of the
/// bit words.
// A7.2: cold struct, size guard waived (a parse result held per message, not a hot collection)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmesRecord {
    pub present: u64,
    pub errors: u64,
    pub invalidated: u64,
    pub present_empty: u64,
    /// Byte offset of the `positionReport` element's name.
    pub offset: u32,
    /// `time` as Unix milliseconds (UTC).
    pub time_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// `position/altitude` in feet (ASSUMED feet).
    pub altitude_ft: i32,
    /// `movement/heading` in 0.01 degree (ASSUMED degrees).
    pub heading_cdeg: i32,
    /// `movement/speed` in centimetres per second (ASSUMED m/s).
    pub speed_cmps: i32,
    /// `movement/vx` in centimetres per second (ASSUMED m/s).
    pub vx_cmps: i32,
    /// `movement/vy` in centimetres per second (ASSUMED m/s).
    pub vy_cmps: i32,
    /// `status/vertRate` in feet per minute (ASSUMED).
    pub vvert_ftmin: i32,
    pub seq_num: u32,
    pub track_num: u32,
    pub mode_s_addr: u32,
    pub mode3a: u16,
    pub aircraft_id: [u8; 8],
    pub ac_type: [u8; 8],
    pub runway: [u8; 8],
    pub tgt_type: [u8; 8],
}

/// A whole `asdexMsg` with a position report.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct SmesMessage {
    /// `airport`, zero padded; meaningful only if `airport_present`.
    pub airport: [u8; 8],
    pub airport_present: bool,
    pub records: Vec<SmesRecord>,
    pub errors: Vec<FieldError>,
    pub errors_dropped: u64,
    pub unknown_elements: u64,
}

/// A bit for field `f`.
pub const fn smes_bit(f: SmesField) -> u64 {
    1u64 << (f as u32)
}

fn int(slots: &[Slot], f: SmesField) -> Option<i64> {
    match slots.get(f as usize) {
        Some(Slot::Ok(Got::Int(v))) => Some(*v),
        _ => None,
    }
}

fn code(slots: &[Slot], f: SmesField) -> Option<[u8; 8]> {
    match slots.get(f as usize) {
        Some(Slot::Ok(Got::Code(c))) => Some(*c),
        _ => None,
    }
}

/// Parses a message that [`crate::classify`] called [`Kind::AsdexPositionReport`]. A report
/// with a refused field is kept with the field absent. Counts reports, fields and unknown
/// elements into `stats`, including the reports read before a [`StdError::TooManyRecords`] or
/// [`StdError::TooDeep`] refusal; the refusal itself is counted in `stats.errors`. It does not
/// count the message: [`crate::parse_message`] does, once, when it classifies.
pub fn parse_smes(
    doc: &Doc<'_>,
    limits: &Limits,
    stats: &mut Stats,
) -> Result<SmesMessage, StdError> {
    let k = crate::classify(doc);
    if k != Kind::AsdexPositionReport {
        let e = StdError::WrongKind { kind: k };
        stats.observe_error(&e);
        return Err(e);
    }
    let tx = text_index(doc);
    // The first step, before any count: a message with a report timed in the sealed window is
    // dropped whole (OA-SEAL-014).
    let gone = prescan(
        doc,
        &tx,
        limits.drop_window,
        b"positionReport",
        &[&[b"time"]],
    );
    if gone > 0 {
        stats.sealed_window_dropped += gone;
        return Ok(SmesMessage {
            airport: [0; 8],
            airport_present: false,
            records: Vec::new(),
            errors: Vec::new(),
            errors_dropped: 0,
            unknown_elements: 0,
        });
    }
    let root = ElemIx(0);
    let mut msg = SmesMessage {
        airport: [0; 8],
        airport_present: false,
        records: Vec::new(),
        errors: Vec::new(),
        errors_dropped: 0,
        unknown_elements: 0,
    };
    let mut unknown: Vec<(String, u64)> = Vec::new();
    let mut errors = Vec::new();
    let mut dropped = 0u64;
    let mut sink = Sink {
        errors: &mut errors,
        max_errors: limits.max_errors,
        errors_dropped: &mut dropped,
        unknown: &mut unknown,
        unknown_dropped: 0,
        invalidated: 0,
    };
    let mut root_slots = [Slot::Empty; 1];
    // The root's own depth is one level, so the walk cannot be too deep.
    let _ = walk(
        doc,
        &tx,
        root,
        &ROOT_SPECS,
        &ROOT_NAMES,
        &ROOT_SKIP,
        &mut root_slots,
        &mut sink,
    );
    if let Some(Slot::Ok(Got::Code(v))) = root_slots.first() {
        msg.airport = *v;
        msg.airport_present = true;
    }
    let mut refusal = None;
    for c in children(doc, root) {
        if name_of(doc, c) != b"positionReport" || !in_message_ns(doc, c) {
            continue;
        }
        if msg.records.len() >= limits.max_records {
            refusal = Some(StdError::TooManyRecords {
                limit: limits.max_records,
                offset: elem_offset(doc, c),
            });
            break;
        }
        match report(doc, &tx, c, &mut sink) {
            Ok(r) => msg.records.push(r),
            Err(off) => {
                refusal = Some(StdError::TooDeep { offset: off });
                break;
            }
        }
    }
    let invalidated = sink.invalidated;
    let unknown_dropped = sink.unknown_dropped;
    msg.errors = errors;
    msg.errors_dropped = dropped;
    msg.unknown_elements = unknown.iter().map(|(_, n)| *n).sum::<u64>() + unknown_dropped;
    stats.observe_smes(&msg, &unknown, unknown_dropped, invalidated, limits);
    match refusal {
        Some(e) => {
            stats.observe_error(&e);
            Err(e)
        }
        None => Ok(msg),
    }
}

fn report(
    doc: &Doc<'_>,
    tx: &crate::walk::TextIndex,
    rec: ElemIx,
    sink: &mut Sink<'_>,
) -> Result<SmesRecord, u32> {
    let mut slots = [Slot::Empty; SMES_FIELD_COUNT];
    walk(
        doc,
        tx,
        rec,
        &SPECS,
        &SMES_FIELD_NAMES,
        &SKIP,
        &mut slots,
        sink,
    )?;
    let offset = elem_offset(doc, rec);
    for (a, b) in [
        (SmesField::Latitude, SmesField::Longitude),
        (SmesField::Longitude, SmesField::Latitude),
    ] {
        if matches!(slots.get(a as usize), Some(Slot::Ok(_)))
            && matches!(slots.get(b as usize), Some(Slot::Empty | Slot::Invalid))
        {
            if let Some(s) = slots.get_mut(a as usize) {
                *s = Slot::Bad;
            }
            sink.error(
                SMES_FIELD_NAMES.get(a as usize).copied().unwrap_or("?"),
                FieldErrorKind::PairIncomplete,
                offset,
            );
        }
    }
    let (mut present, mut errors, mut invalidated, mut present_empty) = (0u64, 0u64, 0u64, 0u64);
    for (i, s) in slots.iter().enumerate() {
        let bit = 1u64 << i;
        match s {
            Slot::Ok(_) => present |= bit,
            Slot::Bad => errors |= bit,
            Slot::Invalid => invalidated |= bit,
            Slot::Blank => present_empty |= bit,
            Slot::Empty => {}
        }
    }
    let i32_of = |f| {
        int(&slots, f)
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(0)
    };
    let u32_of = |f| {
        int(&slots, f)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0)
    };
    let c = |f| code(&slots, f).unwrap_or([0; 8]);
    Ok(SmesRecord {
        present,
        errors,
        invalidated,
        present_empty,
        offset,
        time_ms: int(&slots, SmesField::Time).unwrap_or(0),
        lat_e7: i32_of(SmesField::Latitude),
        lon_e7: i32_of(SmesField::Longitude),
        altitude_ft: i32_of(SmesField::Altitude),
        heading_cdeg: i32_of(SmesField::Heading),
        speed_cmps: i32_of(SmesField::Speed),
        vx_cmps: i32_of(SmesField::Vx),
        vy_cmps: i32_of(SmesField::Vy),
        vvert_ftmin: i32_of(SmesField::VertRate),
        seq_num: u32_of(SmesField::SeqNum),
        track_num: u32_of(SmesField::Track),
        mode_s_addr: u32_of(SmesField::AcAddress),
        mode3a: int(&slots, SmesField::Mode3A)
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(0),
        aircraft_id: c(SmesField::AircraftId),
        ac_type: c(SmesField::AcType),
        runway: c(SmesField::Runway),
        tgt_type: c(SmesField::TgtType),
    })
}

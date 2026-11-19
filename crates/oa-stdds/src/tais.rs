//! The TAIS `TATrackAndFlightPlan` message: STARS terminal radar tracks, each with the flight
//! plan it is associated with. Core (pure).
//!
//! Structure (FAA FIXM-mediated STDDS data overview v2, section 2.3.4, which maps the native
//! message to FIXM): a root `TATrackAndFlightPlan` holding `src` (the producing TRACON, mapped
//! to `provenance@tracon`) and any number of `record`s, each with a `track` (`trackNum`,
//! `mrtTime`, `lat`, `lon`, `reportedAltitude`, `vx`, `vy`, `vVert`, `reportedBeaconCode`,
//! `acAddress`), a `flightPlan` (`acid`, `acType`, `assignedBeaconCode`, `requestedAltitude`,
//! `assignedAltitude` and equipment suffix since STDDS R6P2, `airport`, `runway`, `flightRules`,
//! `rawFlightRules` since R6P2, `ECID`, `sfpn`, `ptdTime`, `entryFix`, `exitFix`, `type`,
//! `status`, scratchpads, `delete`, `suspended`, `rnav`) and `enhancedData` (GUFIs). STDDS R6P2
//! publishes latitude and longitude from STARS AIG210 where present, otherwise computed from
//! the STARS Cartesian position by a point-of-tangency calculation; either way the published
//! position is geodetic, so no site-relative x/y is read here.
//!
//! Units (the FIXM mapping converts velocity from m/s to km/h, so the native unit is m/s;
//! the FIXM sample shows altitude in feet and vertical rate in feet per minute):
//! * `lat`, `lon`: decimal degrees, kept as 1e-7 degree. ASSUMED WGS-84 degrees.
//! * `mrtTime`: ISO-8601 UTC, kept as Unix milliseconds. ASSUMED `xs:dateTime` with `Z`.
//! * `reportedAltitude` and the plan altitudes: feet. ASSUMED feet, not hundreds of feet.
//! * `vx`, `vy`: metres per second east and north, kept as centimetres per second. ASSUMED m/s
//!   and ASSUMED x east, y north (the FIXM mapping calls them track velocity x and y).
//! * `vVert`: feet per minute. ASSUMED.
//! * `acAddress`: the 24-bit Mode S address as hexadecimal. ASSUMED hex text.
//! * beacon codes: four octal digits, kept as the digits read as a decimal number.
//! * `trackNum`: the STARS track number, decimal.
//!
//! Values come from element text. An element may carry `r="1"` meaning the sender no longer
//! holds the value valid (documented for SMES, accepted here too): the field is absent and
//! counted, not an error.

use crate::kind::{children, name_of, Kind};
use crate::walk::{
    elem_offset, note_unknown, read_one, text_index, walk, Got, Sink, Slot, Spec, Val,
};
use crate::{FieldError, FieldErrorKind, Limits, Stats, StdError};
use oa_xml::{Doc, ElemIx};

/// The fields of a TAIS record, in bit order of [`TaisRecord::present`] and friends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TaisField {
    TrackNum = 0,
    MrtTime,
    Lat,
    Lon,
    ReportedAltitude,
    Vx,
    Vy,
    VVert,
    ReportedBeacon,
    AcAddress,
    TrackStatus,
    Acid,
    AcType,
    AssignedBeacon,
    RequestedAltitude,
    AssignedAltitude,
    Airport,
    Runway,
    FlightRules,
    RawFlightRules,
    Ecid,
    Sfpn,
    PtdTime,
    EntryFix,
    ExitFix,
    PlanType,
    PlanStatus,
}

/// Number of [`TaisField`] values.
pub const TAIS_FIELD_COUNT: usize = 27;

/// Field names, in order; the element path below `record` with the unit where it has one.
pub const TAIS_FIELD_NAMES: [&str; TAIS_FIELD_COUNT] = [
    "track/trackNum",
    "track/mrtTime",
    "track/lat",
    "track/lon",
    "track/reportedAltitude",
    "track/vx",
    "track/vy",
    "track/vVert",
    "track/reportedBeaconCode",
    "track/acAddress",
    "track/status",
    "flightPlan/acid",
    "flightPlan/acType",
    "flightPlan/assignedBeaconCode",
    "flightPlan/requestedAltitude",
    "flightPlan/assignedAltitude",
    "flightPlan/airport",
    "flightPlan/runway",
    "flightPlan/flightRules",
    "flightPlan/rawFlightRules",
    "flightPlan/ECID",
    "flightPlan/sfpn",
    "flightPlan/ptdTime",
    "flightPlan/entryFix",
    "flightPlan/exitFix",
    "flightPlan/type",
    "flightPlan/status",
];

const ALT: Val = Val::Scaled {
    scale: 0,
    lo: -2_000,
    hi: 100_000,
};
const VEL: Val = Val::Scaled {
    scale: 2,
    lo: -300_000,
    hi: 300_000,
};

const SPECS: [Spec; TAIS_FIELD_COUNT] = [
    Spec {
        path: &["track", "trackNum"],
        val: Val::Uint(6),
    },
    Spec {
        path: &["track", "mrtTime"],
        val: Val::TimeMs,
    },
    Spec {
        path: &["track", "lat"],
        val: Val::DegE7 { max_deg: 90 },
    },
    Spec {
        path: &["track", "lon"],
        val: Val::DegE7 { max_deg: 180 },
    },
    Spec {
        path: &["track", "reportedAltitude"],
        val: ALT,
    },
    Spec {
        path: &["track", "vx"],
        val: VEL,
    },
    Spec {
        path: &["track", "vy"],
        val: VEL,
    },
    Spec {
        path: &["track", "vVert"],
        val: Val::Scaled {
            scale: 0,
            lo: -30_000,
            hi: 30_000,
        },
    },
    Spec {
        path: &["track", "reportedBeaconCode"],
        val: Val::Beacon,
    },
    Spec {
        path: &["track", "acAddress"],
        val: Val::Hex24,
    },
    Spec {
        path: &["track", "status"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "acid"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "acType"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "assignedBeaconCode"],
        val: Val::Beacon,
    },
    Spec {
        path: &["flightPlan", "requestedAltitude"],
        val: ALT,
    },
    Spec {
        path: &["flightPlan", "assignedAltitude"],
        val: ALT,
    },
    Spec {
        path: &["flightPlan", "airport"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "runway"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "flightRules"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "rawFlightRules"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "ECID"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "sfpn"],
        val: Val::Uint(8),
    },
    Spec {
        path: &["flightPlan", "ptdTime"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "entryFix"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "exitFix"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "type"],
        val: Val::Code,
    },
    Spec {
        path: &["flightPlan", "status"],
        val: Val::Code,
    },
];

/// Known elements that are deliberately not read (not counted as unknown).
const SKIP: [&[&str]; 8] = [
    &["flightPlan", "scratchPad1"],
    &["flightPlan", "scratchPad2"],
    &["flightPlan", "delete"],
    &["flightPlan", "suspended"],
    &["flightPlan", "rnav"],
    &["flightPlan", "equipmentSuffix"],
    &["enhancedData"],
    &["track", "coast"],
];

/// One `record`: a track and the flight plan it is associated with, in fixed units. A field is
/// meaningful only if its bit is set in `present`; `errors` has the bit of every field that was
/// present but refused (see the message's error list); `invalidated` has the bit of every
/// field the sender marked `r="1"`.
// A7.2: cold struct, size guard waived (a parse result held per message, not a hot collection)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaisRecord {
    pub present: u32,
    pub errors: u32,
    pub invalidated: u32,
    /// Byte offset of the `record` element's name in the message.
    pub offset: u32,
    /// `track/mrtTime` as Unix milliseconds (UTC).
    pub mrt_time_ms: i64,
    /// `track/lat` in 1e-7 degree.
    pub lat_e7: i32,
    /// `track/lon` in 1e-7 degree.
    pub lon_e7: i32,
    /// `track/reportedAltitude` in feet (ASSUMED feet).
    pub reported_alt_ft: i32,
    /// `flightPlan/requestedAltitude` in feet (ASSUMED feet).
    pub requested_alt_ft: i32,
    /// `flightPlan/assignedAltitude` in feet (ASSUMED feet).
    pub assigned_alt_ft: i32,
    /// `track/vx` in centimetres per second (ASSUMED published in m/s, east).
    pub vx_cmps: i32,
    /// `track/vy` in centimetres per second (ASSUMED published in m/s, north).
    pub vy_cmps: i32,
    /// `track/vVert` in feet per minute (ASSUMED).
    pub vvert_ftmin: i32,
    /// `track/trackNum`.
    pub track_num: u32,
    /// `flightPlan/sfpn`.
    pub sfpn: u32,
    /// `track/acAddress`, the 24-bit Mode S address (ASSUMED hex text).
    pub mode_s_addr: u32,
    /// `track/reportedBeaconCode`: four octal digits read as a decimal number.
    pub reported_beacon: u16,
    /// `flightPlan/assignedBeaconCode`: four octal digits read as a decimal number.
    pub assigned_beacon: u16,
    pub track_status: [u8; 8],
    pub acid: [u8; 8],
    pub ac_type: [u8; 8],
    pub airport: [u8; 8],
    pub runway: [u8; 8],
    pub flight_rules: [u8; 8],
    pub raw_flight_rules: [u8; 8],
    pub ecid: [u8; 8],
    pub ptd_time: [u8; 8],
    pub entry_fix: [u8; 8],
    pub exit_fix: [u8; 8],
    pub plan_type: [u8; 8],
    pub plan_status: [u8; 8],
}

/// A whole TAIS message.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct TaisMessage {
    /// `src`, the producing TRACON, zero padded; meaningful only if `src_present`.
    pub src: [u8; 8],
    pub src_present: bool,
    pub records: Vec<TaisRecord>,
    /// Every refused field, in document order, up to [`Limits::max_errors`].
    pub errors: Vec<FieldError>,
    /// Field errors past the limit (counted, not kept).
    pub errors_dropped: u64,
    /// Elements outside the known structure (topmost of each subtree).
    pub unknown_elements: u64,
}

/// A bit for field `f`.
pub const fn tais_bit(f: TaisField) -> u32 {
    1u32 << (f as u32)
}

fn int(slots: &[Slot], f: TaisField) -> Option<i64> {
    match slots.get(f as usize) {
        Some(Slot::Ok(Got::Int(v))) => Some(*v),
        _ => None,
    }
}

fn code(slots: &[Slot], f: TaisField) -> Option<[u8; 8]> {
    match slots.get(f as usize) {
        Some(Slot::Ok(Got::Code(c))) => Some(*c),
        _ => None,
    }
}

/// Parses a message that [`crate::classify`] called [`Kind::TaisTrackAndFlightPlan`]. A record
/// with a refused field is kept with the field absent. Counts into `stats`.
pub fn parse_tais(
    doc: &Doc<'_>,
    limits: &Limits,
    stats: &mut Stats,
) -> Result<TaisMessage, StdError> {
    let k = crate::classify(doc);
    if k != Kind::TaisTrackAndFlightPlan {
        let e = StdError::WrongKind { kind: k };
        stats.observe_error(&e);
        return Err(e);
    }
    let tx = text_index(doc);
    let root = ElemIx(0);
    let mut msg = TaisMessage {
        src: [0; 8],
        src_present: false,
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
    for c in children(doc, root) {
        match name_of(doc, c) {
            b"record" => {
                if msg.records.len() >= limits.max_records {
                    let e = StdError::TooManyRecords {
                        limit: limits.max_records,
                        offset: elem_offset(doc, c),
                    };
                    stats.observe_error(&e);
                    return Err(e);
                }
                match record(doc, &tx, c, &mut sink) {
                    Ok(r) => msg.records.push(r),
                    Err(off) => {
                        let e = StdError::TooDeep { offset: off };
                        stats.observe_error(&e);
                        return Err(e);
                    }
                }
            }
            b"src" => match read_one(doc, &tx, c, Val::Code) {
                Ok(Got::Code(v)) => {
                    msg.src = v;
                    msg.src_present = true;
                }
                Ok(Got::Int(_)) => {}
                Err(kind) => sink.error("src", kind, elem_offset(doc, c)),
            },
            other => note_unknown(&mut sink, &[other]),
        }
    }
    let invalidated = sink.invalidated;
    let unknown_dropped = sink.unknown_dropped;
    msg.errors = errors;
    msg.errors_dropped = dropped;
    msg.unknown_elements = unknown.iter().map(|(_, n)| *n).sum::<u64>() + unknown_dropped;
    stats.observe_tais(&msg, &unknown, unknown_dropped, invalidated, limits);
    Ok(msg)
}

fn record(
    doc: &Doc<'_>,
    tx: &crate::walk::TextIndex,
    rec: ElemIx,
    sink: &mut Sink<'_>,
) -> Result<TaisRecord, u32> {
    let mut slots = [Slot::Empty; TAIS_FIELD_COUNT];
    walk(
        doc,
        tx,
        rec,
        &SPECS,
        &TAIS_FIELD_NAMES,
        &SKIP,
        &mut slots,
        sink,
    )?;
    let offset = elem_offset(doc, rec);
    // A position is a pair: one half alone (a delta-encoded message) is refused.
    for (a, b) in [
        (TaisField::Lat, TaisField::Lon),
        (TaisField::Lon, TaisField::Lat),
    ] {
        if matches!(slots.get(a as usize), Some(Slot::Ok(_)))
            && matches!(slots.get(b as usize), Some(Slot::Empty | Slot::Invalid))
        {
            if let Some(s) = slots.get_mut(a as usize) {
                *s = Slot::Bad;
            }
            sink.error(
                TAIS_FIELD_NAMES.get(a as usize).copied().unwrap_or("?"),
                FieldErrorKind::PairIncomplete,
                offset,
            );
        }
    }
    let (mut present, mut errors, mut invalidated) = (0u32, 0u32, 0u32);
    for (i, s) in slots.iter().enumerate() {
        let bit = 1u32 << i;
        match s {
            Slot::Ok(_) => present |= bit,
            Slot::Bad => errors |= bit,
            Slot::Invalid => invalidated |= bit,
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
    let u16_of = |f| {
        int(&slots, f)
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(0)
    };
    let c = |f| code(&slots, f).unwrap_or([0; 8]);
    Ok(TaisRecord {
        present,
        errors,
        invalidated,
        offset,
        mrt_time_ms: int(&slots, TaisField::MrtTime).unwrap_or(0),
        lat_e7: i32_of(TaisField::Lat),
        lon_e7: i32_of(TaisField::Lon),
        reported_alt_ft: i32_of(TaisField::ReportedAltitude),
        requested_alt_ft: i32_of(TaisField::RequestedAltitude),
        assigned_alt_ft: i32_of(TaisField::AssignedAltitude),
        vx_cmps: i32_of(TaisField::Vx),
        vy_cmps: i32_of(TaisField::Vy),
        vvert_ftmin: i32_of(TaisField::VVert),
        track_num: u32_of(TaisField::TrackNum),
        sfpn: u32_of(TaisField::Sfpn),
        mode_s_addr: u32_of(TaisField::AcAddress),
        reported_beacon: u16_of(TaisField::ReportedBeacon),
        assigned_beacon: u16_of(TaisField::AssignedBeacon),
        track_status: c(TaisField::TrackStatus),
        acid: c(TaisField::Acid),
        ac_type: c(TaisField::AcType),
        airport: c(TaisField::Airport),
        runway: c(TaisField::Runway),
        flight_rules: c(TaisField::FlightRules),
        raw_flight_rules: c(TaisField::RawFlightRules),
        ecid: c(TaisField::Ecid),
        ptd_time: c(TaisField::PtdTime),
        entry_fix: c(TaisField::EntryFix),
        exit_fix: c(TaisField::ExitFix),
        plan_type: c(TaisField::PlanType),
        plan_status: c(TaisField::PlanStatus),
    })
}

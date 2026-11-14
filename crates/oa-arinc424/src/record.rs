//! Decoding of one data record.

use crate::airport::{parse_airport, parse_localizer, parse_runway, Airport, Localizer, Runway};
use crate::airspace::{
    parse_airspace, parse_airspace_continuation, Airspace, AirspaceContinuation, AirspaceKind,
};
use crate::airway::{parse_airway_fix, AirwayFix};
use crate::error::ErrorKind;
use crate::field::Rec;
use crate::mora::{parse_mora, GridMora};
use crate::navaid::{parse_ndb, parse_vhf, Ndb, VhfNavaid};
use crate::procedure::{parse_leg, parse_leg_continuation, Leg, LegContinuation, ProcedureKind};
use crate::types::Meta;
use crate::waypoint::{parse_waypoint, Waypoint, WaypointKind};
use crate::RECORD_LEN;

/// One decoded data record.
// Records are decoded one at a time and moved straight into the output vectors, so boxing
// the large variants would only add an allocation per leg.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    Airport(Airport),
    Runway(Runway),
    Localizer(Localizer),
    VhfNavaid(VhfNavaid),
    Ndb(Ndb),
    Waypoint(Waypoint),
    AirwayFix(AirwayFix),
    Leg(Leg),
    LegContinuation(LegContinuation),
    /// A single boundary record, as a one-point airspace.
    Airspace(Airspace),
    AirspaceContinuation(AirspaceContinuation),
    GridMora(GridMora),
    /// A section/subsection this crate does not decode (`HA`, `PN`, `PP`, `PS`, ...).
    Other {
        code: String,
        meta: Meta,
    },
}

/// The common fields of a record.
pub fn record_meta(record: &Record) -> &Meta {
    match record {
        Record::Airport(x) => &x.meta,
        Record::Runway(x) => &x.meta,
        Record::Localizer(x) => &x.meta,
        Record::VhfNavaid(x) => &x.meta,
        Record::Ndb(x) => &x.meta,
        Record::Waypoint(x) => &x.meta,
        Record::AirwayFix(x) => &x.meta,
        Record::Leg(x) => &x.meta,
        Record::LegContinuation(x) => &x.meta,
        Record::Airspace(x) => &x.meta,
        Record::AirspaceContinuation(x) => &x.meta,
        Record::GridMora(x) => &x.meta,
        Record::Other { meta, .. } => meta,
    }
}

/// Section and subsection codes of a record. The subsection is in col 13 for airport-related
/// sections (P, H) and col 6 otherwise; terminal NDBs (`PN`) are the exception, with the
/// subsection in col 6 and col 13 blank.
fn section_sub(r: &Rec) -> (char, char) {
    let sec = r.ch(5);
    let sub = match sec {
        'P' | 'H' => match r.ch(13) {
            ' ' => r.ch(6),
            c => c,
        },
        _ => r.ch(6),
    };
    (sec, sub)
}

/// Section code used for counting: section plus subsection, trailing blanks dropped.
pub(crate) fn section_code(text: &str) -> String {
    let (sec, sub) = section_sub(&Rec::new(text));
    let mut s = String::with_capacity(2);
    s.push(sec);
    s.push(sub);
    s.truncate(s.trim_end().len());
    s
}

/// Decodes one 132-character record. `line` is only stored in [`Meta`].
pub fn parse_record(line: usize, text: &str) -> Result<Record, ErrorKind> {
    if text.len() != RECORD_LEN {
        return Err(ErrorKind::WrongLength {
            len: text.len(),
            expected: RECORD_LEN,
        });
    }
    if let Some(i) = text.bytes().position(|b| !(0x20..0x7f).contains(&b)) {
        return Err(ErrorKind::BadByte { column: i + 1 });
    }
    let r = Rec::new(text);
    match r.ch(1) {
        'S' | 'T' => {}
        found => return Err(ErrorKind::BadRecordType { found }),
    }
    let meta = r.meta(line)?;
    let (sec, sub) = section_sub(&r);
    Ok(match (sec, sub) {
        ('P', 'A') => Record::Airport(parse_airport(&r, meta)?),
        ('P', 'G') => Record::Runway(parse_runway(&r, meta)?),
        ('P', 'I') => Record::Localizer(parse_localizer(&r, meta)?),
        ('P', 'C') => Record::Waypoint(parse_waypoint(&r, meta, WaypointKind::Terminal)?),
        ('P', 'D' | 'E' | 'F') => {
            let kind = match sub {
                'D' => ProcedureKind::Sid,
                'E' => ProcedureKind::Star,
                _ => ProcedureKind::Approach,
            };
            if matches!(r.ch(39), '0' | '1') {
                Record::Leg(parse_leg(&r, meta, kind)?)
            } else {
                Record::LegContinuation(parse_leg_continuation(&r, meta))
            }
        }
        ('E', 'A') => Record::Waypoint(parse_waypoint(&r, meta, WaypointKind::Enroute)?),
        ('E', 'R') => Record::AirwayFix(parse_airway_fix(&r, meta)?),
        ('D', ' ') => Record::VhfNavaid(parse_vhf(&r, meta)?),
        ('D', 'B') => Record::Ndb(parse_ndb(&r, meta)?),
        ('U', 'C' | 'R') => {
            let kind = if sub == 'C' {
                AirspaceKind::Controlled
            } else {
                AirspaceKind::SpecialUse
            };
            match r.ch(25) {
                '0' | '1' => Record::Airspace(parse_airspace(&r, meta, kind)?),
                _ if kind == AirspaceKind::SpecialUse => {
                    Record::AirspaceContinuation(parse_airspace_continuation(&r, meta))
                }
                number => return Err(ErrorKind::UnsupportedContinuation { number }),
            }
        }
        ('A', 'S') => Record::GridMora(parse_mora(&r, meta)?),
        _ => Record::Other {
            code: section_code(text),
            meta,
        },
    })
}

#![forbid(unsafe_code)]
//! oa-stdds: parser for FAA SWIM STDDS (SWIM Terminal Data Distribution System) messages
//! delivered through SCDS. Core (pure): every function here is a function of the parsed
//! document and the bytes it came from, no I/O, no clock (RULESET B1, B2). Class `shell` like
//! its sibling SWIM parsers (`oa-fixm`, `oa-tbfm`): the crate's only I/O is the example.
//!
//! * [`kind`]: classifies a message by root element and namespace into a [`Kind`]. A message of
//!   no known kind is counted, with its root name and namespace, never an error (IF-R6).
//! * [`tais`]: parses the TAIS `TATrackAndFlightPlan` message (STARS terminal radar tracks with
//!   their flight plans) into typed [`TaisRecord`]s, every field in a fixed unit named in the
//!   field.
//! * [`smes`]: parses the SMES/ASDE-X Cat11 `positionReport` of an `asdexMsg` to the same level.
//!   The Cat10 `mlatReport` and `adsbReport`, the FIXM-mediated forms and the other STDDS
//!   products (APDS, ISMC, RVR, TDES) are classified only.
//! * [`stats`]: cumulative counts (kinds, errors, per-field presence, unknown elements).
//!
//! The element names come from the FAA's public FIXM-mediated STDDS data overview (its mapping
//! tables, REQUIREMENTS.md OA-STD-001). No real STDDS message was available when this was
//! written: every unit marked ASSUMED in a field's doc comment must be confirmed against real
//! data before the value is trusted.
//!
//! Interface rules (INTERFACES.md): a field that is present but unreadable is a counted field
//! error that names the field and the byte offset (IF-R5), the record is kept with the field
//! absent and nothing defaulted; a message whose structure is unreadable is refused whole; the
//! output has a stated capacity and reaching it is an error, never truncation (IF-R1); unknown
//! elements are counted, not fatal (IF-R6). No floating point is used: values are scaled
//! integers.

pub mod kind;
pub mod scalar;
pub mod smes;
pub mod stats;
pub mod tais;
mod walk;

use oa_xml::{error_offset, Parser, XmlError};
use std::fmt;

pub use kind::{classify, kind_name, Kind, KIND_COUNT, KIND_NAMES};
pub use smes::{
    parse_smes, SmesField, SmesMessage, SmesRecord, SMES_FIELD_COUNT, SMES_FIELD_NAMES,
};
pub use stats::{new_stats, Stats, UnknownPath};
pub use tais::{
    parse_tais, TaisField, TaisMessage, TaisRecord, TAIS_FIELD_COUNT, TAIS_FIELD_NAMES,
};

/// Bounds on the work and the output of one message.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Most records (TAIS) or reports (SMES) one message may hold; more is
    /// [`StdError::TooManyRecords`], nothing is kept.
    pub max_records: usize,
    /// Most field errors kept per message; further ones are only counted in
    /// [`TaisMessage::errors_dropped`] and the stats.
    pub max_errors: usize,
    /// Most distinct unknown element paths kept in [`Stats`]; further ones are counted as overflow.
    pub max_unknown_paths: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_records: 4096,
            max_errors: 256,
            max_unknown_paths: 128,
        }
    }
}

/// Why one field was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FieldErrorKind {
    /// The element has no text.
    Empty = 0,
    /// The element has child elements where a value was expected.
    NotLeaf,
    /// The text is longer than the field can hold.
    TooLong,
    /// The text is not the syntax of the field (digits, hex, octal, timestamp, code).
    Syntax,
    /// The value parsed but is outside the field's range.
    Range,
    /// The field appears twice in one record; the first is kept, the field is marked in error.
    Duplicate,
    /// Latitude without longitude or longitude without latitude (a delta-encoded message).
    PairIncomplete,
    /// The text could not be decoded (a bad reference or text that does not fit the buffer).
    Text,
}

/// Number of [`FieldErrorKind`] values.
pub const ERROR_KIND_COUNT: usize = 8;

/// Display names of the field error kinds, in order.
pub const ERROR_KIND_NAMES: [&str; ERROR_KIND_COUNT] = [
    "empty",
    "not a leaf",
    "too long",
    "bad syntax",
    "out of range",
    "duplicate",
    "incomplete position pair",
    "undecodable text",
];

/// One refused field: which, why and where (the byte offset of its element name).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldError {
    /// The field's name (see [`TAIS_FIELD_NAMES`], [`SMES_FIELD_NAMES`]).
    pub field: &'static str,
    pub kind: FieldErrorKind,
    /// Byte offset in the message of the field's element name.
    pub offset: u32,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = ERROR_KIND_NAMES
            .get(self.kind as usize)
            .copied()
            .unwrap_or("?");
        write!(f, "field {} {} at byte {}", self.field, name, self.offset)
    }
}

/// Why a whole message was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdError {
    /// The XML parser refused the bytes; the error names its own offset.
    Xml(XmlError),
    /// The message is not of the kind the called parser reads.
    WrongKind { kind: Kind },
    /// More records than [`Limits::max_records`]; `offset` is the first one over the limit.
    TooManyRecords { limit: usize, offset: u32 },
    /// An element nests deeper than the walker follows.
    TooDeep { offset: u32 },
}

/// Number of [`StdError`] variants, for per-reason counts.
pub const STD_ERROR_KINDS: usize = 4;

/// Display names of the [`StdError`] variants, in order of [`std_error_index`].
pub const STD_ERROR_NAMES: [&str; STD_ERROR_KINDS] =
    ["xml", "wrong kind", "too many records", "too deep"];

/// The index of `e` into [`STD_ERROR_NAMES`].
pub fn std_error_index(e: &StdError) -> usize {
    match e {
        StdError::Xml(_) => 0,
        StdError::WrongKind { .. } => 1,
        StdError::TooManyRecords { .. } => 2,
        StdError::TooDeep { .. } => 3,
    }
}

/// The byte offset an error refers to, if it has one (`WrongKind` has none).
pub fn std_error_offset(e: &StdError) -> Option<u32> {
    match e {
        StdError::Xml(x) => Some(error_offset(x)),
        StdError::WrongKind { .. } => None,
        StdError::TooManyRecords { offset, .. } | StdError::TooDeep { offset } => Some(*offset),
    }
}

impl fmt::Display for StdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StdError::Xml(x) => write!(f, "xml error at byte {}: {x:?}", error_offset(x)),
            StdError::WrongKind { kind } => write!(f, "message is of kind {}", kind_name(*kind)),
            StdError::TooManyRecords { limit, offset } => {
                write!(f, "more than {limit} records, first over at byte {offset}")
            }
            StdError::TooDeep { offset } => write!(f, "element nested too deep at byte {offset}"),
        }
    }
}

impl std::error::Error for StdError {}

/// A parsed message: the typed form for the kinds this crate reads, the kind alone otherwise.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub enum Message {
    Tais(TaisMessage),
    Smes(SmesMessage),
    /// A message of a kind that is classified only (or unknown), counted in the stats.
    Other(Kind),
}

/// Parses one complete message: XML, then classification, then the typed parse for the kinds
/// that have one. Every outcome is counted in `stats`. Nothing here panics on any input.
pub fn parse_message(
    parser: &mut Parser,
    bytes: &[u8],
    limits: &Limits,
    stats: &mut Stats,
) -> Result<Message, StdError> {
    let doc = match parser.parse(bytes) {
        Ok(d) => d,
        Err(x) => {
            let e = StdError::Xml(x);
            stats.observe_error(&e);
            return Err(e);
        }
    };
    let k = classify(&doc);
    stats.observe_kind(&doc, k);
    match k {
        Kind::TaisTrackAndFlightPlan => parse_tais(&doc, limits, stats).map(Message::Tais),
        Kind::AsdexPositionReport => parse_smes(&doc, limits, stats).map(Message::Smes),
        other => Ok(Message::Other(other)),
    }
}

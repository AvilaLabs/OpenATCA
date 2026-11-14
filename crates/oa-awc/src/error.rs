//! Typed refusals: one per record (counted by reason) and one per file.

use oa_json::JsonError;
use std::fmt;

/// Why one record was refused (RULESET E5). Counted per reason, never passed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    /// The array element is not an object.
    NotObject = 0,
    /// A required field is absent or null.
    MissingField = 1,
    /// A field has the wrong JSON type or an unparsable text.
    BadType = 2,
    /// A number outside its stated bound.
    OutOfRange = 3,
    /// A station or aircraft identifier that is not 3 to 4 upper-case letters or digits.
    BadIdent = 4,
    /// A timestamp that is not ISO 8601 UTC, or outside the accepted years.
    BadTime = 5,
    /// A code word (intensity, frequency, type) this crate does not know.
    UnknownCode = 6,
    /// A base above its top, or a validity end before its start.
    ReversedBand = 7,
    /// A polygon ring with too few points, or too many points or rings.
    BadGeometry = 8,
}

/// Number of [`Reason`] values; the length of a counts array.
pub const REASON_COUNT: usize = 9;

/// Names for reports, indexed by `Reason as usize`.
pub const REASON_NAMES: [&str; REASON_COUNT] = [
    "not-object",
    "missing-field",
    "bad-type",
    "out-of-range",
    "bad-ident",
    "bad-time",
    "unknown-code",
    "reversed-band",
    "bad-geometry",
];

/// Counts of refused records, indexed by `Reason as usize`.
pub type RefusedCounts = [u32; REASON_COUNT];

/// A refused record: the reason and the JSON field that caused it.
// A7.2: cold struct (one per refused record), size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub reason: Reason,
    pub field: &'static str,
}

/// Shorthand used by the record parsers.
pub fn refuse<T>(reason: Reason, field: &'static str) -> Result<T, Refusal> {
    Err(Refusal { reason, field })
}

/// Adds one refusal to a counts array.
pub fn count_refusal(counts: &mut RefusedCounts, r: Refusal) {
    if let Some(c) = counts.get_mut(r.reason as usize) {
        *c = c.saturating_add(1);
    }
}

/// Why a whole file was refused; nothing from it is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The JSON text is invalid or over an `oa-json` limit.
    Json(JsonError),
    /// The top-level value is not an array.
    NotArray,
    /// More records than [`crate::MAX_RECORDS_PER_FILE`].
    TooManyRecords,
    /// The table would exceed [`crate::MAX_TABLE_ROWS`].
    TableFull,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Json(e) => write!(f, "{e}"),
            ParseError::NotArray => write!(f, "top-level value is not an array"),
            ParseError::TooManyRecords => write!(f, "more records than the per-file limit"),
            ParseError::TableFull => write!(f, "table row limit reached"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<JsonError> for ParseError {
    fn from(e: JsonError) -> Self {
        ParseError::Json(e)
    }
}

/// Why [`crate::altimeter_at`] gave no altimeter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltimeterError {
    /// The station id is not a valid identifier, or has no observation in the table.
    UnknownStation,
    /// Every observation of the station is after the requested time.
    NoObservationYet,
    /// The station has observations not after the time, but none carries a trusted altimeter.
    NoAltimeter,
    /// The latest altimeter is older than the allowed age.
    Stale { age_s: i64 },
}

impl fmt::Display for AltimeterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AltimeterError::UnknownStation => write!(f, "unknown station"),
            AltimeterError::NoObservationYet => write!(f, "no observation at or before the time"),
            AltimeterError::NoAltimeter => write!(f, "no trusted altimeter setting"),
            AltimeterError::Stale { age_s } => write!(f, "altimeter is {age_s} s old"),
        }
    }
}

impl std::error::Error for AltimeterError {}

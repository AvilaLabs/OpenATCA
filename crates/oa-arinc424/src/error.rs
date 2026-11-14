//! Error types. Every error names the source, the 1-based line and (for limits) the limit.

use std::fmt;

/// What went wrong with one line (or with the file as a whole).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A line is longer than [`crate::MAX_LINE_LEN`] bytes, terminator included (OA-GEN-003).
    LineTooLong { limit: usize },
    /// More lines than [`crate::MAX_RECORDS`] (OA-GEN-003).
    TooManyRecords { limit: usize },
    /// More continuation records after one primary than [`crate::MAX_CONTINUATIONS_PER_PRIMARY`].
    TooManyContinuations { limit: usize },
    /// More members in one group (airway fixes, route legs, boundary points, ...).
    TooManyInGroup { what: &'static str, limit: usize },
    /// More header records than [`crate::MAX_HEADER_RECORDS`].
    TooManyHeaderRecords { limit: usize },
    /// The reader failed.
    Io(String),
    /// A record is not [`crate::RECORD_LEN`] characters long.
    WrongLength { len: usize, expected: usize },
    /// A byte outside printable ASCII (1-based column of the first one).
    BadByte { column: usize },
    /// Column 1 is neither `S` nor `T`.
    BadRecordType { found: char },
    /// A field does not hold what the layout allows.
    Field {
        field: &'static str,
        first_col: usize,
        last_col: usize,
        value: String,
        reason: &'static str,
    },
    /// The file does not start with a header record.
    MissingHeader,
    /// A header record in the middle of the data.
    HeaderAfterData,
    /// The header (HDR01) is malformed.
    BadHeader(&'static str),
    /// HDR01 declares a different number of data records than the file holds.
    RecordCountMismatch { declared: usize, found: usize },
    /// A data record carries a cycle later than the header's cycle (OA-424-001).
    CycleAfterHeader { header: String, record: String },
    /// A continuation record that does not follow a matching primary record.
    OrphanContinuation,
    /// A continuation record on a record type that has no continuation layout here.
    UnsupportedContinuation { number: char },
    /// Sequence numbers within a group are not strictly increasing.
    SequenceNotIncreasing { previous: u32, found: u32 },
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineTooLong { limit } => {
                write!(f, "line is longer than the limit of {limit} bytes")
            }
            Self::TooManyRecords { limit } => write!(f, "more than {limit} records (limit)"),
            Self::TooManyContinuations { limit } => {
                write!(
                    f,
                    "more than {limit} continuation records for one primary (limit)"
                )
            }
            Self::TooManyInGroup { what, limit } => {
                write!(f, "more than {limit} {what} in one group (limit)")
            }
            Self::TooManyHeaderRecords { limit } => {
                write!(f, "more than {limit} header records (limit)")
            }
            Self::Io(e) => write!(f, "read error: {e}"),
            Self::WrongLength { len, expected } => {
                write!(f, "record is {len} characters, expected {expected}")
            }
            Self::BadByte { column } => {
                write!(f, "non-printable or non-ASCII byte at column {column}")
            }
            Self::BadRecordType { found } => {
                write!(f, "column 1 is {found:?}, expected 'S' or 'T'")
            }
            Self::Field {
                field,
                first_col,
                last_col,
                value,
                reason,
            } => write!(
                f,
                "{field} (columns {first_col}-{last_col}) is {value:?}: {reason}"
            ),
            Self::MissingHeader => write!(f, "file does not start with a header record (HDR01)"),
            Self::HeaderAfterData => write!(f, "header record after data records"),
            Self::BadHeader(why) => write!(f, "bad header record: {why}"),
            Self::RecordCountMismatch { declared, found } => write!(
                f,
                "header declares {declared} data records but the file holds {found}"
            ),
            Self::CycleAfterHeader { header, record } => write!(
                f,
                "record cycle {record} is later than the header cycle {header} (mixed cycles)"
            ),
            Self::OrphanContinuation => {
                write!(f, "continuation record without a matching primary record")
            }
            Self::UnsupportedContinuation { number } => {
                write!(
                    f,
                    "continuation record number {number:?} on a record type without continuations"
                )
            }
            Self::SequenceNotIncreasing { previous, found } => write!(
                f,
                "sequence number {found} does not follow {previous} (must be strictly increasing)"
            ),
        }
    }
}

/// An [`ErrorKind`] with its source name and 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub source: String,
    pub line: usize,
    pub kind: ErrorKind,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.source, self.line, self.kind)
    }
}

impl std::error::Error for Error {}

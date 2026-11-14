#![forbid(unsafe_code)]
//! oa-nasr: typed parsers for the FAA NASR CSV subscription files. See docs/M0_DESIGN.md.
//!
//! * Columns are always resolved by header name, never by position.
//! * [`schema`] loads the FAA's own `*_CSV_DATA_STRUCTURE.csv` and validates every row of a data
//!   file against it. Violations are collected, never fatal.
//! * Every file parser returns a [`Parsed`]: the records that parsed plus a list of
//!   [`RowError`]s for rows that were rejected (bad number, bad hemisphere, duplicate key, ...).
//!   Rejected rows are never dropped silently. Problems that stop the stream (CSV syntax, a
//!   missing column, a size limit, an I/O failure) are a [`NasrError`].
//! * Positions are computed here from degrees/minutes/seconds/hemisphere and compared with the
//!   published decimal value; see [`Position`]. Plain `f64` degrees are used; the airspace model
//!   converts to its own coordinate type.
//! * Text fields are trimmed of surrounding whitespace (several FAA type codes are blank-padded,
//!   for example `"WP   "`). Schema validation sees the raw, untrimmed text.
//!
//! Limits (record size, field size, field count, record count) are those of [`oa_csv`]; the
//! schema validator adds [`MAX_VIOLATIONS_PER_FILE`].

use std::fmt;

pub mod apt;
pub mod arb;
pub mod awy;
pub mod cls;
pub mod fix;
pub mod nav;
pub mod position;
pub mod procedure;
mod row;
pub mod schema;

pub use position::{dms_to_decimal, Position};

/// Reading a file's rows failed in a way that stops the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NasrError {
    /// CSV syntax, field count, missing column or size limit; carries source and line.
    Csv(oa_csv::CsvError),
    /// The file could not be opened or read.
    Io {
        /// File that failed.
        file: String,
        /// The operating system's message.
        message: String,
    },
    /// A `*_CSV_DATA_STRUCTURE.csv` row could not be understood.
    Schema {
        /// Structure file name.
        file: String,
        /// 1-based line of the row.
        line: u64,
        /// What is wrong with it.
        message: String,
    },
}

impl fmt::Display for NasrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NasrError::Csv(e) => write!(f, "{e}"),
            NasrError::Io { file, message } => write!(f, "{file}: {message}"),
            NasrError::Schema {
                file,
                line,
                message,
            } => write!(f, "{file}:{line}: {message}"),
        }
    }
}

impl std::error::Error for NasrError {}

impl From<oa_csv::CsvError> for NasrError {
    fn from(e: oa_csv::CsvError) -> Self {
        NasrError::Csv(e)
    }
}

/// Why one row was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowErrorKind {
    /// A required value is empty.
    Missing,
    /// A numeric field is not a plain decimal number.
    NotANumber(String),
    /// A flag field is not one of its allowed values.
    BadFlag(String),
    /// A field holds a value outside the known set.
    UnknownValue(String),
    /// A DMS component or decimal degree is out of range, or the hemisphere letter is wrong.
    InvalidPosition(String),
    /// Some, but not all, of a position's fields are present.
    PartialPosition,
    /// Two rows share a key that must be unique.
    DuplicateKey(String),
    /// A child row names a parent that is not in the parent file.
    OrphanRecord(String),
}

/// A rejected row: file, 1-based line, column and reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowError {
    /// Source file name.
    pub file: String,
    /// 1-based physical line of the row.
    pub line: u64,
    /// Column the problem is in (a key description for cross-row problems).
    pub column: String,
    /// What is wrong.
    pub kind: RowErrorKind,
}

impl fmt::Display for RowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: column {}: ", self.file, self.line, self.column)?;
        match &self.kind {
            RowErrorKind::Missing => write!(f, "required value is empty"),
            RowErrorKind::NotANumber(v) => write!(f, "{v:?} is not a number"),
            RowErrorKind::BadFlag(v) => write!(f, "{v:?} is not a valid flag value"),
            RowErrorKind::UnknownValue(v) => write!(f, "unknown value {v:?}"),
            RowErrorKind::InvalidPosition(m) => write!(f, "invalid position: {m}"),
            RowErrorKind::PartialPosition => write!(f, "position is only partly present"),
            RowErrorKind::DuplicateKey(k) => write!(f, "duplicate key {k}"),
            RowErrorKind::OrphanRecord(k) => write!(f, "no parent record for {k}"),
        }
    }
}

/// The result of parsing one file: the records that parsed and the rows that were rejected.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed<T> {
    /// Parsed records, in file order (grouped parsers document their own order).
    pub records: Vec<T>,
    /// Rows that were rejected, in file order.
    pub rejected: Vec<RowError>,
}

impl<T> Default for Parsed<T> {
    fn default() -> Self {
        Parsed {
            records: Vec::new(),
            rejected: Vec::new(),
        }
    }
}

/// Most schema violations kept per file by the validator. Further violations are counted but
/// not stored, so one badly broken file cannot exhaust memory.
pub const MAX_VIOLATIONS_PER_FILE: usize = 100_000;

/// Opens a file for reading, mapping failures to [`NasrError::Io`].
pub fn open(path: &std::path::Path) -> Result<std::io::BufReader<std::fs::File>, NasrError> {
    std::fs::File::open(path)
        .map(std::io::BufReader::new)
        .map_err(|e| NasrError::Io {
            file: path.display().to_string(),
            message: e.to_string(),
        })
}

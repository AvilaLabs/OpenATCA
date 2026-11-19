//! Typed errors of the shell: [`LoadError`] for building an hour from a file and [`QueryError`]
//! for the archive query that can fail either way. Shell (B1, B7): `LoadError` carries the
//! `oa-grib2` decoder's error, a shell type core may not name (B8). [`Refusal`], the plain
//! value, is core and is re-exported here so `error::*` still holds all three. Nothing is ever
//! passed on after a refusal (RULESET E5).

use oa_grib2::Grib2Error;
use std::fmt;

pub use crate::core::error::Refusal;

/// Why an hour could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub enum LoadError {
    /// `oa-grib2` refused the file or a message the hour needs.
    Decode(Grib2Error),
    /// Fewer than two isobaric levels with all of U, V, T and height.
    TooFewLevels(usize),
    /// More isobaric levels than [`crate::MAX_LEVELS`].
    TooManyLevels(usize),
    /// A level has some but not all of UGRD, VGRD, TMP and HGT.
    MissingField { field: &'static str, level_pa: u32 },
    /// The same parameter appears twice on one level.
    DuplicateField { field: &'static str, level_pa: u32 },
    /// A field holds an infinite value (NaN marks a missing value and is allowed).
    NonFinite { field: &'static str, level_pa: u32 },
    /// Wanted fields come from a statistical-interval product template (4.8 or 4.11); the hour
    /// model is instantaneous. `fields` counts them; the first is named.
    StatisticalInterval {
        fields: usize,
        field: &'static str,
        level_pa: u32,
    },
    /// The `.idx` sidecar disagrees with the file it sits beside; the payload says where. The
    /// archive counts these and reads the whole file instead.
    IndexMismatch(&'static str),
    /// A level is not a positive whole number of Pa.
    BadLevel,
    /// Two fields do not share one grid, point count or valid time.
    Inconsistent(&'static str),
    /// A field is smaller than a 2 x 2 grid, or the hour exceeds [`crate::MAX_HOUR_VALUES`].
    BadSize(u64),
    /// The valid time cannot be formed (unsupported time unit).
    BadTime,
    /// Two hour files have the same valid time.
    DuplicateHour(i64),
    /// More hour files than [`crate::MAX_ARCHIVE_FILES`].
    TooManyFiles(usize),
    /// A file could not be read, or is over [`crate::MAX_FILE_BYTES`]. `kind` is the
    /// `std::io::ErrorKind` of the failure (`InvalidInput` for a file over the cap).
    Io {
        path: String,
        kind: std::io::ErrorKind,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Decode(e) => write!(f, "GRIB2 decode: {e}"),
            LoadError::TooFewLevels(n) => write!(f, "only {n} complete isobaric levels"),
            LoadError::TooManyLevels(n) => write!(f, "{n} isobaric levels is over the limit"),
            LoadError::MissingField { field, level_pa } => {
                write!(f, "{field} missing at {level_pa} Pa")
            }
            LoadError::DuplicateField { field, level_pa } => {
                write!(f, "{field} appears twice at {level_pa} Pa")
            }
            LoadError::NonFinite { field, level_pa } => {
                write!(f, "{field} at {level_pa} Pa holds an infinite value")
            }
            LoadError::StatisticalInterval {
                fields,
                field,
                level_pa,
            } => write!(
                f,
                "{fields} statistical-interval fields (first {field} at {level_pa} Pa); \
                 the hour model is instantaneous"
            ),
            LoadError::IndexMismatch(w) => write!(f, "idx disagrees with its file: {w}"),
            LoadError::BadLevel => write!(f, "isobaric level is not a positive whole Pa"),
            LoadError::Inconsistent(w) => write!(f, "fields disagree: {w}"),
            LoadError::BadSize(n) => write!(f, "grid or hour size {n} out of bounds"),
            LoadError::BadTime => write!(f, "valid time cannot be formed"),
            LoadError::DuplicateHour(t) => write!(f, "two files are valid at {t}"),
            LoadError::TooManyFiles(n) => write!(f, "{n} files is over the limit"),
            LoadError::Io { path, kind } => write!(f, "{path}: {kind}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<Grib2Error> for LoadError {
    fn from(e: Grib2Error) -> Self {
        LoadError::Decode(e)
    }
}

/// Error of an archive query: the query was refused, or an hour file could not be loaded.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub enum QueryError {
    Refused(Refusal),
    Load(LoadError),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::Refused(r) => write!(f, "refused: {r}"),
            QueryError::Load(e) => write!(f, "load: {e}"),
        }
    }
}

impl std::error::Error for QueryError {}

impl From<Refusal> for QueryError {
    fn from(r: Refusal) -> Self {
        QueryError::Refused(r)
    }
}

impl From<LoadError> for QueryError {
    fn from(e: LoadError) -> Self {
        QueryError::Load(e)
    }
}

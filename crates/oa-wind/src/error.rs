//! Typed errors: [`LoadError`] for building an hour from a file, [`Refusal`] for a query that
//! cannot be answered without extrapolating or guessing, [`QueryError`] for the archive query
//! that can fail either way. Nothing is ever passed on after a refusal (RULESET E5).

use oa_grib2::Grib2Error;
use std::fmt;

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
    /// A file could not be read, or is over [`crate::MAX_FILE_BYTES`].
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

/// Why a query was refused. The aircraft is never given a number the model does not support.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub enum Refusal {
    /// A query value is not finite or is outside its definition (latitude beyond a pole).
    BadInput(&'static str),
    /// The pressure altitude is outside the range the ISA formulas here cover
    /// (-5000 m to 20 000 m geopotential).
    AltitudeOutsideIsa { alt_m: f64 },
    /// The position is outside the grid.
    OutsideGrid,
    /// The pressure is higher than the lowest level (below the level range), hPa.
    BelowLevels { pressure_hpa: f64, limit_hpa: f64 },
    /// The pressure is lower than the highest level (above the level range), hPa.
    AboveLevels { pressure_hpa: f64, limit_hpa: f64 },
    /// The time is before the first or after the last analysis hour (Unix seconds).
    OutsideTime {
        time_s: f64,
        first_s: i64,
        last_s: i64,
    },
    /// No analysis hour is available.
    NoHours,
    /// A value needed at a grid corner is missing in the file.
    MissingValue,
    /// A grid corner holds an infinite value, or the interpolated answer would not be finite.
    NonFinite,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::BadInput(w) => write!(f, "bad input: {w}"),
            Refusal::AltitudeOutsideIsa { alt_m } => {
                write!(f, "pressure altitude {alt_m:.0} m outside the ISA range")
            }
            Refusal::OutsideGrid => write!(f, "position outside the grid"),
            Refusal::BelowLevels {
                pressure_hpa,
                limit_hpa,
            } => write!(
                f,
                "{pressure_hpa:.1} hPa is below the lowest level {limit_hpa:.1} hPa"
            ),
            Refusal::AboveLevels {
                pressure_hpa,
                limit_hpa,
            } => write!(
                f,
                "{pressure_hpa:.1} hPa is above the highest level {limit_hpa:.1} hPa"
            ),
            Refusal::OutsideTime {
                time_s,
                first_s,
                last_s,
            } => write!(f, "time {time_s:.0} outside {first_s}..{last_s}"),
            Refusal::NoHours => write!(f, "no analysis hours"),
            Refusal::MissingValue => write!(f, "missing value at a grid corner"),
            Refusal::NonFinite => write!(f, "a value in the answer is not finite"),
        }
    }
}

impl std::error::Error for Refusal {}

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

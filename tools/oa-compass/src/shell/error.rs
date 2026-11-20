//! Shell (B1): the ways an export is refused (E5). Every one stops the export and removes its
//! partial files; none is passed on as a smaller file.

use std::fmt;

use oa_asterix::AsterixError;

/// A refusal or failure of the export.
// A7.2: cold struct, size guard waived
#[derive(Debug)]
pub enum ExportError {
    /// The encoder refused a record or a block.
    Asterix(AsterixError),
    /// A file operation failed.
    Io(String),
    /// A block decoded to something other than what was encoded, or a written file does not read
    /// back as written.
    Verify(String),
    /// A record is earlier than the one before it in the same file.
    OutOfOrder {
        system: &'static str,
        prev: u32,
        now: u32,
    },
    /// More distinct tracks in one file than the 16-bit track number holds.
    TrackNumbers { system: &'static str, day: String },
    /// A row falls on another UTC day than the requested one and `--split-days` was not given.
    CrossesMidnight {
        system: &'static str,
        wanted: String,
        found: String,
    },
    /// The input could not be read or is not usable.
    Input(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::Asterix(e) => write!(f, "encoder refused: {e:?}"),
            ExportError::Io(s) | ExportError::Input(s) => f.write_str(s),
            ExportError::Verify(s) => write!(f, "verification failed: {s}"),
            ExportError::OutOfOrder { system, prev, now } => write!(
                f,
                "{system}: a record at time {now} (1/128 s) follows one at {prev}; the rows are not in time order"
            ),
            ExportError::TrackNumbers { system, day } => write!(
                f,
                "{system}: more than 65535 track segments (including restarts) on {day}; the 16-bit track number cannot hold them"
            ),
            ExportError::CrossesMidnight {
                system,
                wanted,
                found,
            } => write!(
                f,
                "{system}: rows on {found} but the day asked for is {wanted}; I062/070 is a time of day, so a file may hold one UTC day (use --split-days to write one set of files per day)"
            ),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<AsterixError> for ExportError {
    fn from(e: AsterixError) -> ExportError {
        ExportError::Asterix(e)
    }
}

/// An I/O error with the path it concerns.
pub fn io_err(path: &std::path::Path, e: &std::io::Error) -> ExportError {
    ExportError::Io(format!("{}: {e}", path.display()))
}

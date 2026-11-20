//! Shell (B1): the sealed hold-out data. Two checks stand between this tool and it, and neither
//! is complete alone.
//!
//! By path: the rule and the list live in the shared guard (`oa-sealed`,
//! `crates/oa-sealed/sealed_paths.txt`); this module maps its refusals to the export's error. Every
//! path is resolved and checked there, as given and after links, and every file the run opens is
//! opened through it (checked before and after the open).
//!
//! By content: a hard link, a copy, a renamed folder or a bind mount of a sealed file has no sealed
//! component in its path. That is why the time pre-scan (`job::prescan_sealed`) exists: it refuses
//! any input time that is on the sealed day (2026-11-19), whatever file it came from.

use std::io::Read;
use std::path::Path;

use super::error::ExportError;

/// The sealed hold-out day, 2026-11-19, as days since 1970-01-01. The one place it is named: the
/// date argument and the row check use it.
pub const SEALED_DAY: i64 = 20_776;
/// The sealed day as text, for messages.
pub const SEALED_DAY_TEXT: &str = "2026-11-19";

/// Refuses an export of the sealed day by its date.
pub fn refuse_sealed_date(day: i64) -> Result<(), ExportError> {
    if day == SEALED_DAY {
        return Err(ExportError::Input(format!(
            "{SEALED_DAY_TEXT} is the sealed hold-out day; it is never exported"
        )));
    }
    Ok(())
}

/// Refuses a row whose UTC day is the sealed day: the whole run fails, nothing is committed.
pub fn refuse_sealed_row(system: &str, day: i64) -> Result<(), ExportError> {
    if day == SEALED_DAY {
        return Err(ExportError::Input(format!(
            "{system}: an input row is on {SEALED_DAY_TEXT}, the sealed hold-out day; it is never exported"
        )));
    }
    Ok(())
}

/// The opener every input file goes through: `oa_bench`'s file opener is itself guarded.
pub fn guarded_opener() -> oa_bench::input::Opener {
    oa_bench::input::file_opener()
}

/// Refuses `path` if it is, is inside, resolves into or holds a sealed recording. The path may
/// not exist yet.
pub fn refuse_holdout(path: &Path) -> Result<(), ExportError> {
    oa_sealed::refuse(path).map_err(ExportError::Input)
}

/// Refuses an output path that is, is inside or resolves into sealed data (the same rule: a path
/// still to be created is resolved through its nearest existing ancestor).
pub fn refuse_holdout_out(path: &Path) -> Result<(), ExportError> {
    refuse_holdout(path)
}

/// Opens `path` for reading through the shared guard (checked before and after the open).
pub fn open(path: &Path) -> std::io::Result<std::fs::File> {
    oa_sealed::open(path)
}

/// Resolves `path` once and refuses it if the path as given or its resolved form is sealed. A path
/// that cannot be resolved is refused, not passed. Returns the resolved path.
pub fn resolve_checked(path: &Path) -> Result<std::path::PathBuf, ExportError> {
    let canonical = std::fs::canonicalize(path).map_err(|e| {
        ExportError::Input(format!(
            "{}: cannot be resolved, so it cannot be checked against the sealed hold-out data: {e}",
            path.display()
        ))
    })?;
    refuse_holdout(path)?;
    refuse_holdout(&canonical)?;
    Ok(canonical)
}

/// Opens a file by its resolved path, after the check.
pub fn open_guarded(path: &Path) -> Result<std::fs::File, ExportError> {
    let p = resolve_checked(path)?;
    open(&p).map_err(|e| super::error::io_err(&p, &e))
}

/// Reads a text file by its resolved path, after the check.
pub fn read_to_string_guarded(path: &Path) -> Result<String, ExportError> {
    let mut s = String::new();
    open_guarded(path)?
        .read_to_string(&mut s)
        .map_err(|e| super::error::io_err(path, &e))?;
    Ok(s)
}

/// Copies a file from its resolved path, after the check, through the guarded opener.
pub fn copy_guarded(from: &Path, to: &Path) -> Result<(), ExportError> {
    let mut src = open_guarded(from)?;
    let mut dst = std::fs::File::create(to).map_err(|e| super::error::io_err(to, &e))?;
    std::io::copy(&mut src, &mut dst)
        .map(|_| ())
        .map_err(|e| super::error::io_err(from, &e))
}

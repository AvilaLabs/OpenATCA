//! Shell (B1): the sealed hold-out recordings of 2026-11-19. Two checks stand between this tool
//! and them, and neither is complete alone.
//!
//! By path: a path whose components include `scds` followed by `rec-20261119`, or a component
//! `stdds-20261119-holdout`, or one that starts with `stdds-smoke-20261119`, is refused, on the
//! path as given and on its canonical form (so a symbolic link into one is caught). Every file
//! the run opens is resolved once, checked, and then opened by the resolved path.
//!
//! The limits of the path check: a hard link, a copy, a renamed folder or a bind mount of a
//! sealed file has no sealed component in its path and is not detectable by path. That is why
//! the time pre-scan (`job::prescan_sealed`) exists: it refuses any input time that is on the
//! sealed day, whatever file it came from.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path};
use std::rc::Rc;

use oa_bench::input::Opener;

use super::error::ExportError;

/// The sealed hold-out day, 2026-11-19, as days since 1970-01-01. The one place it is named: the
/// date argument, the path guard's day and the row check all use it.
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

/// An opener that refuses any file whose resolved path is in the sealed recording, for the
/// loaders that open each file of a directory themselves.
pub fn guarded_opener() -> Opener {
    Rc::new(move |p: &Path| {
        let resolved = resolve_checked(p).map_err(|e| io::Error::other(e.to_string()))?;
        File::open(&resolved)
            .map(|f| Box::new(f) as Box<dyn Read>)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", resolved.display())))
    })
}

fn names(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Whether the components of a path name a sealed folder.
fn holds_sealed(p: &Path) -> bool {
    let n = names(p);
    let pair = n
        .windows(2)
        .any(|w| matches!(w, [a, b] if a == "scds" && b == "rec-20261119"));
    pair || n
        .iter()
        .any(|c| c == "stdds-20261119-holdout" || c.starts_with("stdds-smoke-20261119"))
}

/// Resolves `path` once and refuses it if the path as given or its resolved form is, or is
/// inside, a sealed recording. A path that cannot be resolved is refused, not passed. Returns the
/// resolved path, which is the one to open.
pub fn resolve_checked(path: &Path) -> Result<std::path::PathBuf, ExportError> {
    let canonical = std::fs::canonicalize(path).map_err(|e| {
        ExportError::Input(format!(
            "{}: cannot be resolved, so it cannot be checked against the sealed hold-out recordings: {e}",
            path.display()
        ))
    })?;
    if holds_sealed(path) || holds_sealed(&canonical) {
        return Err(sealed_path_error(path));
    }
    Ok(canonical)
}

fn sealed_path_error(path: &Path) -> ExportError {
    ExportError::Input(format!(
        "{}: this is a sealed hold-out recording (scds/rec-20261119 or a stdds sealed folder); it is never read",
        path.display()
    ))
}

/// Opens a file by its resolved path, after the check.
pub fn open_guarded(path: &Path) -> Result<File, ExportError> {
    let p = resolve_checked(path)?;
    File::open(&p).map_err(|e| super::error::io_err(&p, &e))
}

/// Reads a text file by its resolved path, after the check.
pub fn read_to_string_guarded(path: &Path) -> Result<String, ExportError> {
    let mut s = String::new();
    open_guarded(path)?
        .read_to_string(&mut s)
        .map_err(|e| super::error::io_err(path, &e))?;
    Ok(s)
}

/// Copies a file from its resolved path, after the check.
pub fn copy_guarded(from: &Path, to: &Path) -> Result<(), ExportError> {
    let p = resolve_checked(from)?;
    std::fs::copy(&p, to)
        .map(|_| ())
        .map_err(|e| super::error::io_err(&p, &e))
}

/// Refuses `path` if it is, or is inside, a sealed recording, by the path as given and by its
/// resolved form when it resolves. For arguments, which may not exist yet; the files themselves
/// are opened only through [`open_guarded`] and [`guarded_opener`], which refuse what cannot be
/// resolved.
pub fn refuse_holdout(path: &Path) -> Result<(), ExportError> {
    let canonical = std::fs::canonicalize(path).ok();
    if holds_sealed(path) || canonical.as_deref().is_some_and(holds_sealed) {
        return Err(sealed_path_error(path));
    }
    Ok(())
}

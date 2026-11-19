//! Shell (B1): the sealed hold-out recording. The recording of 2026-11-19 lives under
//! `scds/rec-20261119` and is never read by this tool: any input path that has `scds` followed by
//! `rec-20261119` as two consecutive components (exact match, on the path as given and on its
//! canonical form, so a link into it is caught) is refused.

use std::io;
use std::path::{Component, Path};
use std::rc::Rc;

use oa_bench::input::{file_opener, Opener};

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
    let inner = file_opener();
    Rc::new(move |p: &Path| {
        refuse_holdout(p).map_err(|e| io::Error::other(e.to_string()))?;
        inner(p)
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

fn holds_sealed(p: &Path) -> bool {
    let n = names(p);
    n.windows(2)
        .any(|w| matches!(w, [a, b] if a == "scds" && b == "rec-20261119"))
}

/// Refuses `path` if it is, or is inside, the sealed recording.
pub fn refuse_holdout(path: &Path) -> Result<(), ExportError> {
    let canonical = std::fs::canonicalize(path).ok();
    if holds_sealed(path) || canonical.as_deref().is_some_and(holds_sealed) {
        return Err(ExportError::Input(format!(
            "{}: this is the sealed hold-out recording (scds/rec-20261119); it is never read",
            path.display()
        )));
    }
    Ok(())
}

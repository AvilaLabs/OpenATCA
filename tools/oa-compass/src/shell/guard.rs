//! Shell (B1): the sealed hold-out recording. The recording of 2026-11-19 lives under
//! `scds/rec-20261119` and is never read by this tool: any input path that has `scds` followed by
//! `rec-20261119` as two consecutive components (exact match, on the path as given and on its
//! canonical form, so a link into it is caught) is refused.

use std::path::{Component, Path};

use super::error::ExportError;

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

//! The sealed recordings of 2026-11-19 are never read. A path that has the components `scds`
//! followed by `rec-20261119` or `stdds-20261119-holdout`, or by a name starting
//! `stdds-smoke-20261119`, is refused: on the path as given and on its canonical form (so a link
//! into one is caught). Nothing is opened, listed or read to decide this.

use std::path::{Component, Path};

fn names(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn is_sealed_name(n: &str) -> bool {
    n == "rec-20261119" || n == "stdds-20261119-holdout" || n.starts_with("stdds-smoke-20261119")
}

fn holds_sealed(p: &Path) -> bool {
    let n = names(p);
    n.windows(2)
        .any(|w| matches!(w, [a, b] if a == "scds" && is_sealed_name(b)))
}

/// Refuses `path` if it is, or is inside, a sealed recording.
pub fn refuse_sealed(path: &Path) -> Result<(), String> {
    let canonical = std::fs::canonicalize(path).ok();
    if holds_sealed(path) || canonical.as_deref().is_some_and(holds_sealed) {
        return Err(format!(
            "{}: this is a sealed 2026-11-19 recording; it is never read",
            path.display()
        ));
    }
    Ok(())
}

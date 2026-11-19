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

/// The canonical form of `path`; for a path that does not exist yet (an output directory), the
/// canonical form of its nearest existing ancestor with the missing components appended, so a
/// link above the new directory is resolved too.
fn resolved(path: &Path) -> Option<std::path::PathBuf> {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(c) = std::fs::canonicalize(&cur) {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        let name = cur.file_name()?.to_os_string();
        rest.push(name);
        if !cur.pop() {
            return None;
        }
        if cur.as_os_str().is_empty() {
            cur = std::path::PathBuf::from(".");
        }
    }
}

/// Refuses `path` if it is, or is inside, a sealed recording.
pub fn refuse_sealed(path: &Path) -> Result<(), String> {
    if holds_sealed(path) || resolved(path).as_deref().is_some_and(holds_sealed) {
        return Err(format!(
            "{}: this is a sealed 2026-11-19 recording; it is never read",
            path.display()
        ));
    }
    Ok(())
}

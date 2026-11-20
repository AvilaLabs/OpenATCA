//! The sealed recordings of 2026-11-19 are never read: a path with a component that contains a
//! sealed name, as given or with links resolved, is refused before anything is opened or listed.
//! A prefix that cannot be resolved fails closed.

use std::path::{Path, PathBuf};

/// Names of the sealed hold-out and smoke recordings.
pub const SEALED: [&str; 3] = [
    "rec-20261119",
    "stdds-20261119-holdout",
    "stdds-smoke-20261119",
];

/// Whether any component of `p` contains a sealed name anywhere in it.
pub fn names_sealed(p: &Path) -> bool {
    p.components().any(|c| {
        let c = c.as_os_str().to_string_lossy();
        SEALED.iter().any(|s| c.contains(s))
    })
}

/// `Err` if `path` names a sealed recording as given, or any prefix of it does when resolved,
/// or a prefix cannot be resolved.
pub fn refuse_sealed(path: &Path) -> Result<(), String> {
    let mut bad = names_sealed(path);
    let mut prefix = PathBuf::new();
    for comp in path.components() {
        prefix.push(comp);
        match std::fs::canonicalize(&prefix) {
            Ok(real) => bad |= names_sealed(&real),
            Err(e) => {
                return Err(format!(
                    "cannot resolve the path ({e}), so it cannot be checked"
                ))
            }
        }
    }
    if bad {
        return Err("the sealed hold-out recording is not read".to_string());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // REQ: OA-CEN-007
    #[test]
    fn sealed_names_are_refused_before_any_io() {
        for p in [
            "/x/scds/rec-20261119",
            "/x/scds/stdds-20261119-holdout/rec",
            "/x/scds/stdds-smoke-20261119T192936Z",
            "rec-20261119/a.oaf",
        ] {
            assert!(refuse_sealed(Path::new(p)).is_err(), "{p}");
        }
        assert!(!names_sealed(Path::new("/x/scds/stdds-day-20261120/rec")));
    }

    #[cfg(unix)]
    // REQ: OA-CEN-007
    #[test]
    fn a_link_into_a_sealed_folder_is_refused() {
        let base = std::env::temp_dir().join(format!("oa-census-guard-{}", std::process::id()));
        let sealed = base.join("rec-20261119");
        std::fs::create_dir_all(&sealed).unwrap();
        let link = base.join("innocent");
        std::os::unix::fs::symlink(&sealed, &link).unwrap();
        assert!(refuse_sealed(&link).is_err());
        assert!(refuse_sealed(&base.join("missing")).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}

//! The sealed hold-out data is never read. The rule and the list live in the shared guard
//! (`oa-sealed`, `crates/oa-sealed/sealed_paths.txt`); this is only the census's use of it. Every
//! path (the recording directory, each directory of the walk, each file, the ADS-B clip) is
//! resolved and checked there, as given and after links, before anything is listed or opened.

use oa_sealed::Sealed;
use std::fs::File;
use std::path::Path;

/// Refuses `path` if it is, is inside, resolves into or holds a sealed entry. The error names
/// the path.
pub fn refuse_sealed(g: &Sealed, path: &Path) -> Result<(), String> {
    g.check(path).map_err(|r| r.to_string())
}

/// Checks `path`, opens it for reading and verifies the opened file; the error names the path.
pub fn open_guarded(g: &Sealed, path: &Path) -> Result<File, String> {
    g.open(path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
pub(crate) mod tests {
    use super::*;

    /// A guard whose list also seals an ADS-B day, under a fake root.
    pub(crate) fn guard(root: &Path) -> Sealed {
        let text = format!(
            "root {}\nseal scds/rec-20261119\nseal scds/stdds-20261119-holdout\nseal scds/stdds-smoke-20261119*\nseal raw/adsblol/2026.11.20\n",
            root.display()
        );
        Sealed::with_list(&text, None)
    }

    fn tree(tag: &str) -> std::path::PathBuf {
        let base =
            std::env::temp_dir().join(format!("oa-census-guard-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base.canonicalize().unwrap()
    }

    // REQ: OA-CEN-007
    #[test]
    fn sealed_folders_and_adsb_day_are_refused_and_an_unsealed_one_accepted() {
        let root = tree("names");
        let g = guard(&root);
        for p in [
            "scds/rec-20261119",
            "scds/stdds-20261119-holdout/rec",
            "scds/stdds-smoke-20261119T192936Z",
            "raw/adsblol/2026.11.20",
            "raw/adsblol/2026.11.20/clip.oaf",
        ] {
            let full = root.join(p);
            std::fs::create_dir_all(&full).unwrap();
            let e = refuse_sealed(&g, &full).unwrap_err();
            assert!(e.contains(&*full.to_string_lossy()), "{e}");
            assert!(e.contains("OA-SEAL-001"), "{e}");
        }
        let ok = root.join("scds").join("day-20261124");
        std::fs::create_dir_all(&ok).unwrap();
        assert!(refuse_sealed(&g, &ok).is_ok());
        let other = root.join("raw").join("adsblol").join("2026.11.24");
        std::fs::create_dir_all(&other).unwrap();
        assert!(refuse_sealed(&g, &other).is_ok());
        // The data root holds sealed entries and is refused whole.
        assert!(refuse_sealed(&g, &root.join("scds")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    // REQ: OA-CEN-007
    #[test]
    fn a_link_from_an_unsealed_name_to_a_sealed_folder_is_refused() {
        let root = tree("link");
        let g = guard(&root);
        let sealed = root.join("scds").join("stdds-20261119-holdout");
        let day = root.join("raw").join("adsblol").join("2026.11.20");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(sealed.join("a.oaf"), b"x").unwrap();
        let link = root.join("innocent");
        std::os::unix::fs::symlink(&sealed, &link).unwrap();
        let link2 = root.join("clip");
        std::os::unix::fs::symlink(&day, &link2).unwrap();
        assert!(refuse_sealed(&g, &link).is_err());
        assert!(refuse_sealed(&g, &link.join("a.oaf")).is_err());
        assert!(open_guarded(&g, &link.join("a.oaf")).is_err());
        assert!(refuse_sealed(&g, &link2).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}

//! The sealed hold-out data is never read. The rule and the list live in the shared guard
//! (`oa-sealed`, `crates/oa-sealed/sealed_paths.txt`); this is only this tool's use of it. Every
//! path (the clip, the configuration, the output directory) is resolved and checked there, as given
//! and after links, before anything is opened or created; a file read is opened and then verified.

use oa_sealed::Sealed;
use std::fs::File;
use std::path::Path;

/// Refuses `path` if it is, is inside, resolves into or holds a sealed entry. The error names
/// the path.
pub fn refuse_sealed(g: &Sealed, path: &Path) -> Result<(), String> {
    g.check(path).map_err(|r| r.to_string())
}

/// Why a guarded open failed.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenFail {
    /// The guard refused the path, before or after the open.
    Refused(String),
    Io(String),
}

/// Whether an error text is a refusal by the shared guard.
pub fn is_refusal(msg: &str) -> bool {
    msg.contains("OA-SEAL-001")
}

/// Checks `path`, opens it for reading and verifies the opened file; the error names the path.
pub fn open_guarded(g: &Sealed, path: &Path) -> Result<File, OpenFail> {
    // seal-allow: the opener of the guarded open, called only after the guard has passed the path
    open_guarded_with(g, path, |p| File::open(p))
}

/// [`open_guarded`] with the opening done by `open` (a test changes the path's target between
/// the check and the open, as a racing writer would).
pub fn open_guarded_with(
    g: &Sealed,
    path: &Path,
    open: impl FnOnce(&Path) -> std::io::Result<File>,
) -> Result<File, OpenFail> {
    refuse_sealed(g, path).map_err(OpenFail::Refused)?;
    let file = open(path).map_err(|e| OpenFail::Io(format!("{}: {e}", path.display())))?;
    g.verify_open(path, &file).map_err(|e| {
        let m = format!("{}: {e}", path.display());
        if is_refusal(&m) {
            OpenFail::Refused(m)
        } else {
            OpenFail::Io(m)
        }
    })?;
    Ok(file)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
pub(crate) mod tests {
    use super::*;
    #[path = "../../test-support/sealed_guard.rs"]
    mod shared_guard;
    use shared_guard::guard;

    fn tree(tag: &str) -> std::path::PathBuf {
        let base =
            std::env::temp_dir().join(format!("oa-ca-guard-guard-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base.canonicalize().unwrap()
    }

    // REQ: OA-CA-030
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
    // REQ: OA-CA-030
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
        assert!(matches!(
            open_guarded(&g, &link.join("a.oaf")),
            Err(OpenFail::Refused(_))
        ));
        assert!(refuse_sealed(&g, &link2).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    // REQ: OA-CA-030
    #[test]
    fn a_link_swapped_after_the_check_is_refused_at_the_open() {
        let root = tree("swap");
        let g = guard(&root);
        let sealed = root.join("scds").join("stdds-20261119-holdout");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::write(sealed.join("a.oaf"), b"x").unwrap();
        std::fs::write(root.join("fine.oaf"), b"y").unwrap();
        let link = root.join("l.oaf");
        std::os::unix::fs::symlink(root.join("fine.oaf"), &link).unwrap();
        // The check sees the innocent target; the link then points into sealed data.
        let swap = |p: &Path| {
            std::fs::remove_file(p)?;
            std::os::unix::fs::symlink(sealed.join("a.oaf"), p)?;
            File::open(p)
        };
        let r = open_guarded_with(&g, &link, swap);
        assert!(matches!(r, Err(OpenFail::Refused(_))), "{r:?}");
        // Unswapped, the same path opens.
        let l2 = root.join("m.oaf");
        std::os::unix::fs::symlink(root.join("fine.oaf"), &l2).unwrap();
        assert!(open_guarded(&g, &l2).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }

    // REQ: OA-CA-030
    #[test]
    fn the_clip_loader_refuses_a_sealed_path_in_a_form_the_tool_maps_to_exit_2() {
        let p = std::env::temp_dir()
            .join(format!("oa-ca-guard-clip-{}", std::process::id()))
            .join("scds")
            .join("rec-20261119")
            .join("day2")
            .join("c.oaf");
        let e = oa_bench::input::load_adsb(&p).err().unwrap();
        assert!(is_refusal(&e.to_string()), "{e}");
        assert!(e.to_string().contains("rec-20261119"));
    }
}

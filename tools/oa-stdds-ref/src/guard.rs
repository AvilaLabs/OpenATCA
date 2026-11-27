//! The sealed hold-out data is never read. The rule and the list live in the shared guard
//! (`oa-sealed`, `crates/oa-sealed/sealed_paths.txt`); this is only this tool's use of it. Every
//! path it is given (each STDDS file, the ADS-B clip, the output folder) is resolved and checked
//! there, as given and after links, before it is read or created, and every file is opened with
//! check, open, verify. The tool walks no directory.

use oa_sealed::Sealed;
use std::fs::File;
use std::io;
use std::path::Path;

/// Refuses `path` if it is, is inside, resolves into or holds a sealed entry. The error names
/// the path.
pub fn refuse_sealed(g: &Sealed, path: &Path) -> Result<(), String> {
    g.check(path).map_err(|r| r.to_string())
}

/// Whether an error text is a refusal by the shared guard.
pub fn is_refusal(msg: &str) -> bool {
    msg.contains("OA-SEAL-001")
}

/// Checks `path`, opens it for reading and verifies the opened file; the error names the path.
pub fn open_guarded(g: &Sealed, path: &Path) -> io::Result<File> {
    // seal-allow: the guard's opener: checked first, verified on the open file
    open_guarded_with(g, path, |p| File::open(p))
}

/// [`open_guarded`] with the opening done by `open` (a test changes the path's target between
/// the check and the open, as a racing writer would).
pub fn open_guarded_with(
    g: &Sealed,
    path: &Path,
    open: impl FnOnce(&Path) -> io::Result<File>,
) -> io::Result<File> {
    refuse_sealed(g, path).map_err(io::Error::other)?;
    let file =
        open(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    g.verify_open(path, &file)
        .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
    Ok(file)
}

/// Creates the output file `path` for writing: checked through the guard first, then created with
/// `create_new`, so an existing file (or link, even a dangling one) is refused, never overwritten
/// or followed. The error names the path; a sealed path is a refusal (`OA-SEAL-001`).
pub fn create_output(g: &Sealed, path: &Path) -> io::Result<File> {
    refuse_sealed(g, path).map_err(io::Error::other)?;
    // seal-allow: creates the tool's own output (create_new) after the guard check; reads nothing
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            let why = if e.kind() == io::ErrorKind::AlreadyExists {
                "output file exists (never overwritten)".to_string()
            } else {
                e.to_string()
            };
            io::Error::new(e.kind(), format!("{}: {why}", path.display()))
        })?;
    g.verify_open(path, &file)
        .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
    Ok(file)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    mod shared_guard {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test-support/sealed_guard.rs"
        ));
    }
    pub(crate) use shared_guard::guard;

    fn tree(tag: &str) -> std::path::PathBuf {
        let base =
            std::env::temp_dir().join(format!("oa-stdds-ref-guard-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base.canonicalize().unwrap()
    }

    // REQ: OA-SRF-010
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
            assert!(is_refusal(&e), "{e}");
        }
        let ok = root.join("scds").join("day-20261124");
        std::fs::create_dir_all(&ok).unwrap();
        assert!(refuse_sealed(&g, &ok).is_ok());
        let other = root.join("raw").join("adsblol").join("2026.11.24");
        std::fs::create_dir_all(&other).unwrap();
        assert!(refuse_sealed(&g, &other).is_ok());
        // The folder that holds sealed entries is refused whole.
        assert!(refuse_sealed(&g, &root.join("scds")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    // REQ: OA-SRF-010, OA-SRF-013
    #[test]
    fn a_link_from_an_unsealed_name_to_sealed_data_is_refused_and_never_opened() {
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
        let e = open_guarded(&g, &link.join("a.oaf")).unwrap_err();
        assert!(is_refusal(&e.to_string()), "{e}");
        assert!(refuse_sealed(&g, &link2).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    // REQ: OA-SRF-013
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
        let e = open_guarded_with(&g, &link, swap).unwrap_err();
        assert!(is_refusal(&e.to_string()), "{e}");
        assert!(e.to_string().contains(&*link.to_string_lossy()), "{e}");
        // Unswapped, the same path opens.
        let l2 = root.join("m.oaf");
        std::os::unix::fs::symlink(root.join("fine.oaf"), &l2).unwrap();
        assert!(open_guarded(&g, &l2).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }

    // REQ: OA-SRF-016
    #[test]
    fn an_output_is_created_never_overwritten_and_never_into_sealed_data() {
        let root = tree("out");
        let g = guard(&root);
        let p = root.join("o.txt");
        std::fs::write(&p, b"keep").unwrap();
        let e = create_output(&g, &p).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(e.to_string().contains(&*p.to_string_lossy()), "{e}");
        assert_eq!(std::fs::read(&p).unwrap(), b"keep");
        // A dangling link is not followed to create the file behind it.
        let dangling = root.join("d.txt");
        std::os::unix::fs::symlink(root.join("nowhere"), &dangling).unwrap();
        assert!(create_output(&g, &dangling).is_err());
        assert!(!root.join("nowhere").exists());
        // A link into sealed data is refused by the guard, and nothing is created there.
        let sealed = root.join("scds").join("stdds-20261119-holdout");
        std::fs::create_dir_all(&sealed).unwrap();
        let link = root.join("l.txt");
        std::os::unix::fs::symlink(sealed.join("new.txt"), &link).unwrap();
        let e = create_output(&g, &link).unwrap_err();
        assert!(is_refusal(&e.to_string()), "{e}");
        assert!(!sealed.join("new.txt").exists());
        // A fresh path is created.
        assert!(create_output(&g, &root.join("fresh.txt")).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }

    // REQ: OA-SRF-010, OA-SRF-013
    #[test]
    fn the_loaders_refuse_a_sealed_file_and_clip_before_opening() {
        let root = tree("loaders");
        let g = guard(&root);
        // The files do not exist: a refusal, not "no such file", proves nothing was opened.
        let f = root
            .join("scds")
            .join("rec-20261119")
            .join("day2")
            .join("s.oaf");
        let e = crate::load::load_file(&g, &f, 0, &mut Vec::new()).unwrap_err();
        assert!(is_refusal(&e.to_string()), "{e}");
        let clip = root
            .join("raw")
            .join("adsblol")
            .join("2026.11.20")
            .join("c.oaf");
        let out = root.join("out");
        let e = crate::run::build_refs(&g, &clip, &Default::default(), &out, &mut 0)
            .err()
            .unwrap();
        assert!(
            is_refusal(&e) && e.contains(&*clip.to_string_lossy()),
            "{e}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}

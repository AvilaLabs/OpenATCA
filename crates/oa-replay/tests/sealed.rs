//! `oa-replay` and its source helpers refuse a link into sealed data (fake tree in a temp dir).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Command;

use oa_replay::{list_recordings, spec_from_paths, FeedKind};

fn tree(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sealed_{name}"));
    let _ = fs::remove_dir_all(&t);
    let sealed = t.join("scds/stdds-20261119-holdout");
    fs::create_dir_all(&sealed).unwrap();
    fs::write(sealed.join("a.oaf"), b"x").unwrap();
    let open = t.join("open");
    fs::create_dir_all(open.join("dir")).unwrap();
    symlink(sealed.join("a.oaf"), open.join("dir/a.oaf")).unwrap();
    symlink(sealed.join("a.oaf"), open.join("file.oaf")).unwrap();
    symlink(&sealed, open.join("folder")).unwrap();
    (
        t.clone(),
        open.join("dir"),
        open.join("file.oaf"),
        open.join("folder"),
    )
}

// REQ: OA-SEAL-005
#[test]
fn the_source_helpers_refuse_links_into_sealed_data() {
    let (t, dir_with_link, file, folder) = tree("src");
    for d in [&dir_with_link, &folder] {
        let e = list_recordings(d).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001"), "{e}");
    }
    let mut spec = spec_from_paths(FeedKind::Adsb, vec![file]);
    let e = (spec.open)().unwrap().err().unwrap().to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-SEAL-005
#[test]
fn the_program_exits_2_for_a_link_into_sealed_data() {
    let (t, dir_with_link, file, folder) = tree("bin");
    for args in [
        ["--adsb".to_string(), file.display().to_string()],
        ["--sfdps-dir".to_string(), folder.display().to_string()],
        [
            "--tbfm-dir".to_string(),
            dir_with_link.display().to_string(),
        ],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_oa-replay"))
            .args(&args)
            .output()
            .unwrap();
        // A link at the argument is refused up front (2); one found while listing a folder is a
        // run failure (1). Either way the rule is named and the run does not go on.
        assert!(matches!(o.status.code(), Some(1 | 2)), "{args:?}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
    }
    let _ = fs::remove_dir_all(&t);
}

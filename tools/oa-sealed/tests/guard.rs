//! The sealed hold-out guard on fake data roots in temp directories (never the real ones).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use oa_sealed::{parse_list, Sealed, LIST};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-sealed-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// A fake data root with the three sealed folders and one development folder.
fn fake_root(name: &str) -> PathBuf {
    let root = scratch(name).join("data");
    for d in [
        "scds/rec-20261119/sfdps",
        "scds/stdds-20261119-holdout",
        "scds/stdds-smoke-20261119T192936Z",
        "scds/day-20261118/rec",
    ] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(root.join("scds/rec-20261119/sfdps/a.bin"), b"x").unwrap();
    fs::write(root.join("scds/day-20261118/rec/b.bin"), b"y").unwrap();
    root
}

// REQ: OA-SEAL-001
#[test]
fn the_list_has_the_three_sealed_folders_and_the_python_guard_parses_it_to_the_same_set() {
    let l = parse_list(LIST).unwrap();
    let texts: Vec<&str> = l.entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "scds/rec-20261119",
            "scds/stdds-20261119-holdout",
            "scds/stdds-smoke-20261119*"
        ]
    );
    let py = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sealed_guard.py");
    let out = Command::new("python3")
        .arg("-I")
        .arg(&py)
        .arg("--dump")
        .output()
        .expect("python3 is needed to compare the two parsers");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), l.dump());
}

// REQ: OA-SEAL-001
#[test]
fn a_list_that_does_not_parse_is_an_error_and_the_guard_then_refuses_everything() {
    for bad in [
        "",
        "root /d\n",
        "seal scds/x\n",
        "root /d\nseal scds/../x\n",
        "root /d\nroot /e\nseal a\n",
        "root d\nseal a\n",
        "root /d\nseal *\n",
        "root /d\nfoo bar\n",
        "root /d\nseal\n",
    ] {
        assert!(parse_list(bad).is_err(), "{bad:?}");
    }
    let g = Sealed::with_list("garbage", None);
    let e = g.check(Path::new("/tmp/anything")).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
}

// REQ: OA-SEAL-002
#[test]
fn sealed_names_are_refused_as_given_with_the_rule_named_and_near_misses_are_not() {
    let g = Sealed::with_root(&fake_root("names"));
    for p in [
        "x/scds/rec-20261119",
        "/data/scds/rec-20261119/sub/file.oaf",
        "/data/scds/stdds-20261119-holdout/a",
        "/data/scds/stdds-smoke-20261119T192936Z/b.bin",
        "/data/scds/stdds-smoke-20261119",
    ] {
        let e = g.check(Path::new(p)).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001") && e.contains(p), "{e}");
    }
    for p in [
        "/data/scds/day-20261114/rec/sfdps",
        "/data/scds/rec-20261119x",
        "/data/rec-20261119",
        "/data/scds/x/rec-20261119",
        "/data/scds/stdds-smoke-20261118T1",
        "/data/scds/stdds-bin",
    ] {
        assert!(g.check(Path::new(p)).is_ok(), "{p}");
    }
}

// REQ: OA-SEAL-002
#[test]
fn a_path_is_resolved_before_it_is_checked() {
    let root = fake_root("resolve");
    let g = Sealed::with_root(&root);
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    let sealed = root.join("scds/rec-20261119");
    // A link to the folder, to a file in it, a link to a link, and a symlinked parent directory.
    symlink(&sealed, work.join("dir")).unwrap();
    symlink(sealed.join("sfdps/a.bin"), work.join("file")).unwrap();
    symlink(work.join("dir"), work.join("chain")).unwrap();
    for p in [
        work.join("dir"),
        work.join("dir/sfdps/a.bin"),
        work.join("file"),
        work.join("chain/sfdps"),
        // `..` that leads into the folder, and a path through a dir that is not there yet.
        root.join("scds/day-20261118/../rec-20261119/sfdps"),
        work.join("dir/new-output"),
        work.join("dir/not/yet/here"),
        root.join("scds/day-20261118/nothing/../../rec-20261119"),
    ] {
        let e = g.check(&p).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001"), "{} {e}", p.display());
    }
    // A relative path (from the working directory) is resolved too.
    let cwd = std::env::current_dir().unwrap();
    let up = "../".repeat(cwd.components().count());
    let rel = PathBuf::from(up).join(sealed.strip_prefix("/").unwrap());
    assert!(g.check(&rel).is_err());
    // A link to development data, and plain unsealed paths, are fine.
    symlink(root.join("scds/day-20261118"), work.join("dev")).unwrap();
    assert!(g.check(&work.join("dev/rec/b.bin")).is_ok());
    assert!(g.check(&work.join("new-file")).is_ok());
    assert!(g.check(&root.join("scds/day-20261118/rec/b.bin")).is_ok());
}

// REQ: OA-SEAL-002
#[test]
fn a_link_that_cannot_be_resolved_is_refused() {
    let root = fake_root("loop");
    let g = Sealed::with_root(&root);
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    symlink(work.join("b"), work.join("a")).unwrap();
    symlink(work.join("a"), work.join("b")).unwrap();
    assert!(g.check(&work.join("a")).is_err());
    // A dangling link whose target lies in a sealed folder (not yet created) is refused too.
    symlink(
        root.join("scds/stdds-20261119-holdout/later"),
        work.join("dangling"),
    )
    .unwrap();
    assert!(g.check(&work.join("dangling")).is_err());
    // A dangling link elsewhere is not a sealed path.
    symlink(work.join("nowhere"), work.join("plain")).unwrap();
    assert!(g.check(&work.join("plain")).is_ok());
}

// REQ: OA-SEAL-002
#[test]
fn a_directory_that_holds_a_sealed_folder_is_refused_and_one_below_it_is_not() {
    let root = fake_root("anc");
    let g = Sealed::with_root(&root);
    assert!(g.check(&root).is_err());
    assert!(g.check(&root.join("scds")).is_err());
    assert!(g.check(&root.join("scds/day-20261118")).is_ok());
    assert!(g.check(root.parent().unwrap()).is_ok());
    // The same on a root reached through a link.
    let via = root.parent().unwrap().join("via");
    symlink(&root, &via).unwrap();
    assert!(g.check(&via.join("scds/rec-20261119")).is_err());
    assert!(g.check(&via.join("scds/day-20261118/rec")).is_ok());
}

// REQ: OA-SEAL-003
#[test]
fn every_file_a_walk_opens_goes_through_the_opener() {
    let root = fake_root("walk");
    let g = Sealed::with_root(&root);
    // A development directory with an ordinary file and a link to a sealed file: a walker that
    // opens each entry through the guard reads the first and is refused on the second.
    let dev = root.join("scds/day-20261118/rec");
    symlink(
        root.join("scds/rec-20261119/sfdps/a.bin"),
        dev.join("sneaky"),
    )
    .unwrap();
    let mut read = 0;
    let mut refused = 0;
    let mut entries: Vec<_> = fs::read_dir(&dev)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        match g.open(&p) {
            Ok(_) => read += 1,
            Err(e) => {
                assert!(e.to_string().contains("OA-SEAL-001"));
                refused += 1;
            }
        }
    }
    assert_eq!((read, refused), (1, 1));
}

// REQ: OA-SEAL-004
#[test]
fn nothing_switches_the_guard_off() {
    let root = fake_root("nooff");
    let g = Sealed::with_root(&root);
    let sealed = root.join("scds/rec-20261119/sfdps/a.bin");
    // Whatever the environment says, the answer is the same: the guard reads no environment.
    let src = include_str!("../src/lib.rs");
    for word in ["env::var", "env::args", "getenv", "OA_SEAL"] {
        assert!(!src.contains(word), "{word}");
    }
    assert!(g.check(&sealed).is_err());
    let py = include_str!("../../sealed_guard.py");
    for word in ["os.environ", "getenv", "argparse"] {
        assert!(!py.contains(word), "{word}");
    }
}

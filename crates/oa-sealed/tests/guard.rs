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
        "scds/rec-20261119/day2/sfdps",
        "scds/rec-20261119/day1",
        "scds/rec-20261119/bin",
        "scds/stdds-20261119-holdout",
        "scds/stdds-smoke-20261119T192936Z",
        "scds/day-20261118/rec",
    ] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(root.join("scds/rec-20261119/day2/sfdps/a.bin"), b"x").unwrap();
    fs::write(root.join("scds/day-20261118/rec/b.bin"), b"y").unwrap();
    root
}

// REQ: OA-SEAL-001
#[test]
fn the_list_has_the_sealed_folders_and_the_python_guard_parses_it_to_the_same_set() {
    let l = parse_list(LIST).unwrap();
    let texts: Vec<&str> = l.entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "scds/rec-20261119/day2*",
            "scds/rec-20261119/day3*",
            "scds/rec-20261119/day4*",
            "scds/rec-20261119/runs*",
            "scds/stdds-20261119-holdout",
            "scds/stdds-smoke-20261119*",
            "raw/adsblol/2026.11.20",
            "raw/adsblol/2026.11.21",
            "raw/adsblol/2026.11.22"
        ]
    );
    let py = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/sealed_guard.py");
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
        "x/scds/rec-20261119/day2",
        "/data/scds/rec-20261119/day2/sub/file.oaf",
        "/data/scds/rec-20261119/day2x",
        "/data/scds/rec-20261119/day3.partial",
        "/data/scds/rec-20261119/runs.log",
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
        "/data/scds/rec-20261119/day1/x",
        "/data/rec-20261119",
        "/data/scds/x/rec-20261119",
        "/data/scds/stdds-smoke-20261118T1",
        "/data/scds/stdds-bin",
    ] {
        assert!(g.check(Path::new(p)).is_ok(), "{p}");
    }
}

// REQ: OA-SEAL-001
// REQ: OA-SEAL-002
#[test]
fn only_days_two_to_four_and_the_run_log_of_the_recording_are_sealed() {
    let root = fake_root("tune");
    let g = Sealed::with_root(&root);
    let rec = root.join("scds/rec-20261119");
    fs::write(rec.join("day1/x"), b"x").unwrap();
    fs::create_dir_all(rec.join("day3")).unwrap();
    fs::create_dir_all(rec.join("day4")).unwrap();
    fs::write(rec.join("bin/run-days.sh"), b"x").unwrap();
    for p in ["day1/x", "bin/run-days.sh", "bin", "day1", "day1x", "day5"] {
        assert!(g.check(&rec.join(p)).is_ok(), "{p}");
    }
    // The folder itself holds sealed entries, so it is refused as a holding directory (as
    // `scds` is): a tool names day1 or bin directly.
    let e = g.check(&rec).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001") && e.contains("holds"), "{e}");
    for p in [
        "day2/x",
        "day3",
        "day3/x",
        "day4/x",
        "runs.log",
        "runs.log.1",
        "day2.tmp",
        "day30",
        "day1/../day2/x",
    ] {
        let e = g.check(&rec.join(p)).unwrap_err().to_string();
        assert!(e.contains("OA-SEAL-001"), "{p} {e}");
    }
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    // A link inside day1 into day2, and a link named day1 elsewhere to day2.
    symlink(rec.join("day2/sfdps/a.bin"), rec.join("day1/link")).unwrap();
    assert!(g.check(&rec.join("day1/link")).is_err());
    symlink(rec.join("day2"), work.join("day1")).unwrap();
    assert!(g.check(&work.join("day1")).is_err());
    assert!(g.check(&work.join("day1/sfdps/a.bin")).is_err());
    // A copy under another root: the names decide.
    assert!(g
        .check(Path::new("/elsewhere/scds/rec-20261119/day2/x"))
        .is_err());
    assert!(g
        .check(Path::new("/elsewhere/scds/rec-20261119/runs.log"))
        .is_err());
    assert!(g
        .check(Path::new("/elsewhere/scds/rec-20261119/day1/x"))
        .is_ok());
}

// REQ: OA-SEAL-002
#[test]
fn a_path_is_resolved_before_it_is_checked() {
    let root = fake_root("resolve");
    let g = Sealed::with_root(&root);
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    let sealed = root.join("scds/rec-20261119/day2");
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
        root.join("scds/day-20261118/../rec-20261119/day2/sfdps"),
        work.join("dir/new-output"),
        work.join("dir/not/yet/here"),
        root.join("scds/day-20261118/nothing/../../rec-20261119/day2"),
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
    assert!(g.check(&via.join("scds/rec-20261119/day2")).is_err());
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
        root.join("scds/rec-20261119/day2/sfdps/a.bin"),
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
    let sealed = root.join("scds/rec-20261119/day2/sfdps/a.bin");
    // Whatever the environment says, the answer is the same: the guard reads no environment.
    let src = include_str!("../src/lib.rs");
    for word in ["env::var", "env::args", "getenv", "OA_SEAL"] {
        assert!(!src.contains(word), "{word}");
    }
    assert!(g.check(&sealed).is_err());
    let py = include_str!("../../../tools/sealed_guard.py");
    for word in ["os.environ", "getenv", "argparse"] {
        assert!(!py.contains(word), "{word}");
    }
}

// REQ: OA-SEAL-007
#[test]
fn a_file_opened_after_a_swapped_in_link_is_caught_on_the_descriptor() {
    let root = fake_root("toctou");
    let g = Sealed::with_root(&root);
    let work = root.parent().unwrap().join("work");
    fs::create_dir_all(&work).unwrap();
    let sealed_file = root.join("scds/rec-20261119/day2/sfdps/a.bin");
    symlink(&sealed_file, work.join("late")).unwrap();
    // As if the path check had passed before the link was put there: the file is opened without
    // the check and then verified.
    let f = fs::File::open(work.join("late")).unwrap();
    let e = g
        .verify_open(&work.join("fine"), &f)
        .unwrap_err()
        .to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
    // A file that is not sealed passes the same verification.
    let ok = fs::File::open(root.join("scds/day-20261118/rec/b.bin")).unwrap();
    g.verify_open(&work.join("fine"), &ok).unwrap();
    // The opener does both steps and refuses the link (the error has the rule and no path).
    let e = g.open(&work.join("late")).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001") && !e.contains("late"), "{e}");
}

// REQ: OA-SEAL-008
#[test]
fn odd_list_files_are_refused_by_both_parsers_and_good_ones_read_the_same() {
    let dir = scratch("parity");
    let py = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/sealed_guard.py");
    let cases: Vec<(&str, bool)> = vec![
        ("root /d\nseal a/b\n", true),
        (
            "# a comment ~!@ with odd printable characters\nroot /d\nseal a/b*",
            true,
        ),
        ("root /d\n\n\nseal a\nseal b/c\n", true),
        ("root /d\nseal a\tb\n", false),
        ("root /d\r\nseal a\n", false),
        ("root  /d\nseal a\n", false),
        ("root /d\nseal a \n", false),
        ("root /d\n seal a\n", false),
        ("root /d\n \nseal a\n", false),
        ("root /d\nseal \u{0}a\n", false),
        ("root /d\nseal a\u{b}b\n", false),
        ("root /d\nseal a\u{c}b\n", false),
        ("root /d\nseal a\u{1c}b\n", false),
        ("root /d\nseal a\u{85}b\n", false),
        ("root /d\nseal \u{e9}\n", false),
        ("root /d\nseal a\u{2028}b\n", false),
        ("root /d\nseal a//b\n", false),
        ("root /d\nseal ../b\n", false),
        ("root d\nseal a\n", false),
    ];
    for (i, (text, good)) in cases.iter().enumerate() {
        let f = dir.join(format!("case{i}.txt"));
        fs::write(&f, text.as_bytes()).unwrap();
        let rust = parse_list(text);
        assert_eq!(rust.is_ok(), *good, "rust {text:?}");
        let out = Command::new("python3")
            .arg("-I")
            .arg(&py)
            .arg("--parse-list")
            .arg(&f)
            .output()
            .unwrap();
        assert_eq!(out.status.success(), *good, "python {text:?}");
        if let Ok(l) = rust {
            assert_eq!(String::from_utf8(out.stdout).unwrap(), l.dump(), "{text:?}");
        } else {
            assert_eq!(out.status.code(), Some(2), "{text:?}");
        }
    }
}

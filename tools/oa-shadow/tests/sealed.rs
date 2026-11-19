//! The sealed 2026-11-19 recordings are refused, as given and through links, with exit 2.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use oa_shadow::args::parse;
use oa_shadow::guard::refuse_sealed;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-shadow-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// REQ: OA-SHD-016
#[test]
fn the_three_sealed_recordings_are_refused_as_given() {
    for p in [
        "x/scds/rec-20261119",
        "x/scds/rec-20261119/sub/file.oaf",
        "/data/scds/stdds-20261119-holdout",
        "/data/scds/stdds-20261119-holdout/a",
        "/data/scds/stdds-smoke-20261119T192936Z",
        "/data/scds/stdds-smoke-20261119",
        "/data/scds/stdds-smoke-20261119T192936Z/b.bin",
    ] {
        assert!(refuse_sealed(Path::new(p)).is_err(), "{p}");
    }
    // Development data and near misses are not refused.
    for p in [
        "/data/scds/day-20261114/rec/sfdps",
        "/data/scds/rec",
        "/data/scds/rec-20261118",
        "/data/rec-20261119",
        "/data/scds/stdds-bin",
        "/data/scds/stdds-smoke-20261118T1",
        "/data/scds/x/rec-20261119",
    ] {
        assert!(refuse_sealed(Path::new(p)).is_ok(), "{p}");
    }
}

// REQ: OA-SHD-016
#[test]
#[cfg(unix)]
fn a_link_into_a_sealed_recording_is_refused_after_resolving() {
    let root = scratch("link");
    for sealed in [
        "rec-20261119",
        "stdds-20261119-holdout",
        "stdds-smoke-20261119T000000Z",
    ] {
        let dir = root.join("scds").join(sealed);
        std::fs::create_dir_all(&dir).unwrap();
        let link = root.join(format!("innocent-{sealed}"));
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(refuse_sealed(&link).is_err(), "{sealed}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

// REQ: OA-SHD-016
#[test]
fn the_program_exits_2_for_a_sealed_path_in_any_argument_and_opens_nothing() {
    let exe = env!("CARGO_BIN_EXE_oa-shadow");
    let out = scratch("exit");
    let fine = out.join("fine");
    for (adsb, sfdps, o) in [
        (
            "/x/scds/rec-20261119/a.oaf",
            fine.to_str().unwrap(),
            out.join("o"),
        ),
        (
            fine.to_str().unwrap(),
            "/x/scds/stdds-20261119-holdout",
            out.join("o"),
        ),
        (
            fine.to_str().unwrap(),
            fine.to_str().unwrap(),
            PathBuf::from("/x/scds/stdds-smoke-20261119T1/o"),
        ),
    ] {
        let r = Command::new(exe)
            .args([
                "--adsb",
                adsb,
                "--sfdps-dir",
                sfdps,
                "--out",
                o.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert_eq!(r.status.code(), Some(2), "{adsb} {sfdps} {o:?}");
        assert!(String::from_utf8_lossy(&r.stderr).contains("sealed"));
    }
    // Nothing was created for the refused runs.
    assert!(!out.join("o").exists());
    // A usage error is also 2.
    let r = Command::new(exe).args(["--adsb", "a"]).output().unwrap();
    assert_eq!(r.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&out);
}

// REQ: OA-SHD-017
#[test]
fn arguments_are_strict() {
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let ok = a(&["--adsb", "a", "--sfdps-dir", "b", "--out", "c"]).unwrap();
    assert_eq!(
        (ok.adsb, ok.sfdps_dir, ok.out),
        ("a".into(), "b".into(), "c".into())
    );
    assert!(a(&["--adsb", "a", "--sfdps-dir", "b"]).is_err());
    assert!(a(&[
        "--adsb",
        "a",
        "--sfdps-dir",
        "b",
        "--out",
        "c",
        "--out",
        "d"
    ])
    .is_err());
    assert!(a(&["--adsb", "--sfdps-dir", "b", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "a", "--sfdps-dir", "b", "--out"]).is_err());
    assert!(a(&["--adsb", "a", "--sfdps-dir", "b", "--out", "c", "--x", "y"]).is_err());
    assert!(a(&[]).is_err());
}

// REQ: OA-SHD-022
#[test]
#[cfg(unix)]
fn every_file_opened_is_canonicalised_so_a_link_in_an_unsealed_directory_is_refused() {
    use oa_shadow::run::guarded_opener;
    let root = scratch("opener");
    let sealed = root.join("scds").join("rec-20261119");
    std::fs::create_dir_all(&sealed).unwrap();
    let target = sealed.join("a.msg");
    std::fs::write(&target, b"x").unwrap();
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("ok.msg"), b"x").unwrap();
    let link = plain.join("innocent.msg");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let open = guarded_opener();
    assert!(open(&link).is_err());
    assert!(open(&plain.join("ok.msg")).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

// REQ: OA-SHD-022
#[test]
fn run_itself_refuses_a_sealed_argument_before_creating_anything() {
    use oa_shadow::args::Args;
    let out = scratch("runrefuse");
    let made = out.join("o");
    let r = oa_shadow::run::run(&Args {
        adsb: PathBuf::from("/x/scds/rec-20261119/a.oaf"),
        sfdps_dir: out.join("fine"),
        out: made.clone(),
    });
    assert!(r.unwrap_err().contains("sealed"));
    assert!(!made.exists());
    let _ = std::fs::remove_dir_all(&out);
}

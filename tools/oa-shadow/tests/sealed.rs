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
        "x/scds/rec-20261119/day2",
        "x/scds/rec-20261119/day2/sub/file.oaf",
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
        "rec-20261119/day2",
        "stdds-20261119-holdout",
        "stdds-smoke-20261119T000000Z",
    ] {
        let dir = root.join("scds").join(sealed);
        std::fs::create_dir_all(&dir).unwrap();
        let link = root.join(format!("innocent-{}", sealed.replace('/', "-")));
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
            "/x/scds/rec-20261119/day2/a.oaf",
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
    let sealed = root.join("scds").join("rec-20261119").join("day2");
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
        adsb: PathBuf::from("/x/scds/rec-20261119/day2/a.oaf"),
        sfdps_dir: out.join("fine"),
        out: made.clone(),
    });
    assert!(r.unwrap_err().contains("sealed"));
    assert!(!made.exists());
    let _ = std::fs::remove_dir_all(&out);
}

// REQ: OA-SHD-028
#[test]
#[cfg(unix)]
fn an_output_directory_that_does_not_exist_yet_is_resolved_through_its_existing_parent() {
    let root = scratch("newdir");
    let sealed = root.join("scds").join("rec-20261119").join("day2");
    std::fs::create_dir_all(&sealed).unwrap();
    let link = root.join("innocent");
    std::os::unix::fs::symlink(&sealed, &link).unwrap();
    // `innocent/out/deeper` does not exist; its parent resolves into the sealed folder.
    assert!(refuse_sealed(&link.join("out").join("deeper")).is_err());
    assert!(refuse_sealed(&root.join("fresh").join("out")).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

// REQ: OA-SEAL-005
#[test]
#[cfg(unix)]
fn links_into_sealed_data_are_refused_for_each_argument_and_inside_the_sfdps_folder() {
    use std::os::unix::fs::symlink;
    let exe = env!("CARGO_BIN_EXE_oa-shadow");
    let t = scratch("links");
    let sealed = t.join("scds/rec-20261119/day2");
    std::fs::create_dir_all(&sealed).unwrap();
    std::fs::write(sealed.join("a.oaf"), b"x").unwrap();
    std::fs::write(t.join("fine.oaf"), b"x").unwrap();
    symlink(sealed.join("a.oaf"), t.join("clip-link.oaf")).unwrap();
    symlink(&sealed, t.join("dir-link")).unwrap();
    let inner = t.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    symlink(sealed.join("a.oaf"), inner.join("x.oaf")).unwrap();
    let out = t.join("out");
    for (adsb, sfdps, o) in [
        (t.join("clip-link.oaf"), t.join("inner"), out.clone()),
        (t.join("fine.oaf"), t.join("dir-link"), out.clone()),
        (t.join("fine.oaf"), t.join("inner"), out.clone()),
        (t.join("fine.oaf"), t.join("inner"), t.join("dir-link/new")),
    ] {
        let r = Command::new(exe)
            .arg("--adsb")
            .arg(&adsb)
            .arg("--sfdps-dir")
            .arg(&sfdps)
            .arg("--out")
            .arg(&o)
            .output()
            .unwrap();
        assert!(!r.status.success(), "{adsb:?} {sfdps:?} {o:?}");
        assert!(String::from_utf8_lossy(&r.stderr).contains("OA-SEAL-001"));
        assert!(!out.exists() && !sealed.join("new").exists());
    }
    let _ = std::fs::remove_dir_all(&t);
}

// REQ: OA-SEAL-014
#[test]
fn the_program_counts_sfdps_frames_and_clip_rows_in_a_sealed_window_once() {
    // The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes), Unix nanoseconds.
    const FROM: i64 = 1_795_131_900_000_000_000;
    let root = scratch("window-count");
    let sfdps = root.join("sfdps");
    std::fs::create_dir_all(&sfdps).unwrap();
    let frame = |t: i64, payload: &[u8]| {
        let mut buf = vec![0u8; payload.len() + 256];
        let n = oa_feed::encode_frame(
            &mut buf,
            1,
            oa_feed::FLAG_BYTES,
            0,
            t,
            t / 1_000_000,
            payload,
        )
        .unwrap();
        buf[..n].to_vec()
    };
    // One SFDPS frame received in the window (dropped by receive time, never parsed).
    std::fs::write(sfdps.join("a.oaf"), frame(FROM, b"<x/>")).unwrap();
    // A clip of two rows: one before the window, one at its first instant (dropped).
    let mut r = oa_adsb::Reports::default();
    for (k, t) in [FROM - 1_000_000_000, FROM].into_iter().enumerate() {
        let mut row = oa_adsb::ROW_EMPTY;
        row.time_ns = t;
        row.icao = 0xA00001 + k as u32;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        oa_adsb::reports_push(&mut r, &row);
    }
    // One row per frame: a frame with a row in the window is dropped whole.
    let mut bytes = Vec::new();
    for k in 0..2 {
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, k, k + 1).unwrap();
        bytes.extend(frame(FROM - 1_000_000_000, &payload));
    }
    let clip = root.join("neutral.oaf");
    std::fs::write(&clip, bytes).unwrap();
    let out = root.join("out");
    // The two recordings do not overlap in time, so the run ends with an error after its reads;
    // the count line is on standard error once, before it.
    let o = Command::new(env!("CARGO_BIN_EXE_oa-shadow"))
        .arg("--adsb")
        .arg(&clip)
        .arg("--sfdps-dir")
        .arg(&sfdps)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    // One SFDPS frame by receive time plus one clip row: 2.
    assert_eq!(
        err.matches("sealed-window rows dropped: 2").count(),
        1,
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

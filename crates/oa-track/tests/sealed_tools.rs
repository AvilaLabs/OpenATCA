//! The tools of this crate that read recordings refuse a sealed path (OA-SEAL-005): exit 2, the
//! rule named, nothing created. The paths are fake ones that only name a sealed folder.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

const CLIP: &str = "/nonexistent-fake/scds/rec-20261119/day2/clip.oaf";
const SFDPS: &str = "/nonexistent-fake/scds/stdds-smoke-20261119T192936Z/sfdps";

/// An example binary built beside the test binary (`target/<profile>/examples/<name>`).
fn example(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(name)
}

fn out_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sealed_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn refused(mut c: Command, out: &Path) {
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{c:?}");
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!out.exists(), "{}", out.display());
}

// REQ: OA-SEAL-005
#[test]
fn oa_track_run_refuses_a_sealed_clip_or_output() {
    let bin = env!("CARGO_BIN_EXE_oa-track-run");
    let out = out_dir("run");
    let mut c = Command::new(bin);
    c.args(["--adsb", CLIP, "--out"]).arg(&out);
    refused(c, &out);
    let mut c = Command::new(bin);
    c.args([
        "--adsb",
        "/nonexistent-fake/x.oaf",
        "--out",
        "/nonexistent-fake/scds/stdds-20261119-holdout/o",
    ]);
    refused(c, &out);
    let mut c = Command::new(bin);
    c.args(["--adsb", "/nonexistent-fake/x.oaf", "--no-score", "--out"])
        .arg(&out)
        .args(["--rows", CLIP]);
    refused(c, &out);
}

// REQ: OA-SEAL-005
#[test]
fn oa_track_fusion_refuses_a_sealed_clip() {
    let out = out_dir("fusion");
    let mut c = Command::new(env!("CARGO_BIN_EXE_oa-track-fusion"));
    c.args(["--quick", "--clip", CLIP, "--out"]).arg(&out);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
    assert!(!out.exists());
}

// REQ: OA-SEAL-005
#[test]
fn the_examples_that_read_recordings_refuse_a_sealed_path() {
    for name in ["p002", "vs_eram"] {
        let out = out_dir(name);
        for (adsb, sfdps) in [
            (CLIP, "/nonexistent-fake/sfdps"),
            ("/nonexistent-fake/x.oaf", SFDPS),
        ] {
            let mut c = Command::new(example(name));
            c.args(["--adsb", adsb, "--sfdps-dir", sfdps, "--out"])
                .arg(&out);
            if name == "p002" {
                c.arg("--development");
            }
            refused(c, &out);
        }
    }
    let out = out_dir("vel");
    let mut c = Command::new(example("velocity_diag"));
    c.args(["--adsb", CLIP, "--out"]).arg(&out);
    refused(c, &out);
    let out = out_dir("pred");
    let mut c = Command::new(example("prediction_trace"));
    c.args(["--adsb", CLIP, "--addr", "1", "--out"]).arg(&out);
    let o = c.output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
    assert!(!out.exists());
}

// REQ: OA-REP-004
#[test]
fn p002_export_and_export_scoring_refuse_a_sealed_path_and_create_nothing() {
    let bin = example("p002");
    let out = out_dir("export");
    let file = out.join("eram.txt");
    let fine = "/nonexistent-fake/sfdps";
    let sealed_out = "/nonexistent-fake/scds/rec-20261119/day3/eram.txt";
    let runs: [Vec<&std::ffi::OsStr>; 5] = [
        // The recording to export, the export file, the drop list.
        vec![
            "--export-eram".as_ref(),
            file.as_os_str(),
            "--sfdps-dir".as_ref(),
            SFDPS.as_ref(),
        ],
        vec![
            "--export-eram".as_ref(),
            sealed_out.as_ref(),
            "--sfdps-dir".as_ref(),
            fine.as_ref(),
        ],
        vec![
            "--export-eram".as_ref(),
            file.as_os_str(),
            "--sfdps-dir".as_ref(),
            fine.as_ref(),
            "--drop-addresses".as_ref(),
            CLIP.as_ref(),
        ],
        // Scoring from an export that is a sealed path, and a sealed drop list.
        vec![
            "--development".as_ref(),
            "--adsb".as_ref(),
            "/nonexistent-fake/x.oaf".as_ref(),
            "--eram-export".as_ref(),
            sealed_out.as_ref(),
            "--out".as_ref(),
            out.as_os_str(),
        ],
        vec![
            "--development".as_ref(),
            "--adsb".as_ref(),
            "/nonexistent-fake/x.oaf".as_ref(),
            "--eram-export".as_ref(),
            file.as_os_str(),
            "--drop-addresses".as_ref(),
            SFDPS.as_ref(),
            "--out".as_ref(),
            out.as_os_str(),
        ],
    ];
    for args in runs {
        let mut c = Command::new(&bin);
        c.args(args);
        refused(c, &out);
    }
}

// REQ: OA-REP-005
#[test]
fn p002_refuses_two_sources_none_and_an_export_in_a_hold_out_run() {
    let bin = example("p002");
    let out = out_dir("export2");
    for args in [
        // Both sources, no source, an export for a hold-out run (no --development).
        vec![
            "--adsb",
            "/x.oaf",
            "--sfdps-dir",
            "/d",
            "--eram-export",
            "/e",
            "--development",
        ],
        vec!["--adsb", "/x.oaf", "--development"],
        vec!["--adsb", "/x.oaf", "--eram-export", "/e", "--ledger", "/l"],
        // Export mode takes no scoring options.
        vec![
            "--export-eram",
            "/e",
            "--sfdps-dir",
            "/d",
            "--adsb",
            "/x.oaf",
        ],
        vec!["--export-eram", "/e"],
    ] {
        let o = Command::new(&bin)
            .args(&args)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
    assert!(!out.exists());
}

// REQ: OA-SEAL-005
#[test]
fn the_tools_refuse_links_into_sealed_data_and_loaders_refuse_links_inside_a_folder() {
    use std::os::unix::fs::symlink;
    let t = out_dir("links");
    let sealed = t.join("scds/rec-20261119/day2");
    std::fs::create_dir_all(&sealed).unwrap();
    std::fs::write(sealed.join("a.oaf"), b"x").unwrap();
    symlink(sealed.join("a.oaf"), t.join("clip.oaf")).unwrap();
    symlink(&sealed, t.join("dir")).unwrap();
    let out = t.join("out");
    let mut c = Command::new(env!("CARGO_BIN_EXE_oa-track-run"));
    c.args(["--adsb"])
        .arg(t.join("clip.oaf"))
        .arg("--out")
        .arg(&out);
    refused(c, &out);
    for name in ["p002", "vs_eram"] {
        // The clip is fine; the SFDPS folder is a link, or holds a link into sealed data.
        std::fs::write(t.join("fine.oaf"), b"x").unwrap();
        {
            let sfdps = t.join("dir");
            let mut c = Command::new(example(name));
            c.arg("--adsb")
                .arg(t.join("fine.oaf"))
                .arg("--sfdps-dir")
                .arg(&sfdps)
                .arg("--out")
                .arg(&out);
            if name == "p002" {
                c.arg("--development");
            }
            let o = c.output().unwrap();
            assert!(!o.status.success(), "{name} {sfdps:?}");
            assert!(
                String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"),
                "{name}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            assert!(!out.exists());
        }
    }
}

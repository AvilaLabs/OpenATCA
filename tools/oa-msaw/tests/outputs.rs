//! The output files are written whole or not at all, and a failed run leaves none of an earlier
//! run's (a temp dir, an empty clip made here).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const DEV_CFG: &str = include_str!("../testdata/dev.cfg");

// REQ: OA-MSAW-035
#[test]
fn a_failed_run_leaves_no_output_of_an_earlier_run_and_a_good_one_leaves_no_temporary_file() {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("msaw-outputs");
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    let clip = t.join("empty.oaf");
    fs::write(&clip, b"").unwrap();
    let cfg = t.join("dev.cfg");
    fs::write(&cfg, DEV_CFG).unwrap();
    let out = t.join("out");
    let run = |cfgp: &PathBuf| {
        Command::new(env!("CARGO_BIN_EXE_oa-msaw"))
            .arg("--adsb")
            .arg(&clip)
            .arg("--config")
            .arg(cfgp)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap()
    };
    let names = |d: &PathBuf| {
        let mut v: Vec<String> = fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let o = run(&cfg);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // Exactly the six files: no temporary name is left.
    assert_eq!(
        names(&out),
        [
            "advisory.csv",
            "alerts.csv",
            "below_mva.csv",
            "inhibitions.csv",
            "skipped.csv",
            "summary.txt"
        ]
    );
    // A run that fails (a bad configuration) removes the five, so none looks like its result.
    let bad = t.join("bad.cfg");
    fs::write(&bad, "not a configuration\n").unwrap();
    let o = run(&bad);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(names(&out), Vec::<String>::new());
    let _ = fs::remove_dir_all(&t);
}

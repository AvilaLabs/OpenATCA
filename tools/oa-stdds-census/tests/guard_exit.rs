//! The binary refuses a sealed path with exit code 2 and a message naming it, before reading.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

// REQ: OA-CEN-007
#[test]
fn a_sealed_directory_and_a_sealed_clip_exit_2_naming_the_path() {
    let t = std::env::temp_dir().join(format!("oa-census-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    let sealed = t.join("scds").join("stdds-20261119-holdout");
    let ok = t.join("scds").join("day-20261124");
    std::fs::create_dir_all(&sealed).unwrap();
    std::fs::create_dir_all(&ok).unwrap();
    let bin = env!("CARGO_BIN_EXE_oa-stdds-census");
    let run = |args: &[&std::path::Path]| Command::new(bin).args(args).output().unwrap();
    let o = run(&[&sealed]);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains(&*sealed.to_string_lossy()) && err.contains("refused"),
        "{err}"
    );
    let clip = t.join("scds").join("rec-20261119").join("clip.oaf");
    let o = run(&[&ok, std::path::Path::new("--adsb"), &clip]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("rec-20261119"));
    std::fs::remove_dir_all(&t).unwrap();
}

// REQ: OA-CEN-013
#[test]
fn a_non_finite_window_is_a_usage_error() {
    // Without the finite filter "NaN" would parse and the run would go on to the directory;
    // with it the arguments are a usage error and the usage text is printed.
    let bin = env!("CARGO_BIN_EXE_oa-stdds-census");
    for a in [
        ["--from", "NaN", "--to", "5"],
        ["--from", "1", "--to", "inf"],
    ] {
        let o = Command::new(bin).arg("x").args(a).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{a:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("usage:"),
            "{a:?}"
        );
    }
}

//! The command line and the sealed hold-out day (OA-CMP-047).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::{Path, PathBuf};

use oa_compass::shell::args::{parse_export, parse_slice};
use oa_compass::shell::export::Layout;
use oa_compass::shell::guard::refuse_holdout;
use oa_compass::shell::job::{run_job, EramSource, Job};
use oa_compass::shell::run::{EramAddress, InputsMode, OpenatcaRows};
use oa_compass::shell::slice::slice_dir;

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_string()).collect()
}

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cmp-args-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

// REQ: OA-CMP-047
#[test]
fn a_value_that_looks_like_a_flag_is_a_missing_value_and_a_repeated_flag_is_an_error() {
    let ok = parse_export(&v(&[
        "--date",
        "2026.11.15",
        "--out",
        "o",
        "--adsb",
        "c.oaf",
    ]))
    .unwrap();
    assert_eq!(ok.eram_address, EramAddress::Keep);
    let d = parse_export(&v(&[
        "--date",
        "2026.11.15",
        "--out",
        "o",
        "--adsb",
        "c",
        "--eram-contradicted-address",
        "drop",
        "--split-days",
    ]))
    .unwrap();
    assert_eq!(d.eram_address, EramAddress::Drop);
    assert!(d.split);
    let e = parse_export(&v(&["--date", "2026.11.15", "--out", "--adsb", "c"])).unwrap_err();
    assert!(e.contains("--out needs a value"), "{e}");
    let e = parse_export(&v(&["--date", "2026.11.15", "--out"])).unwrap_err();
    assert!(e.contains("--out needs a value"), "{e}");
    let e = parse_export(&v(&[
        "--date",
        "2026.11.15",
        "--date",
        "2026.11.16",
        "--out",
        "o",
        "--adsb",
        "c",
    ]))
    .unwrap_err();
    assert!(e.contains("--date is given twice"), "{e}");
    let e = parse_export(&v(&[
        "--split-days",
        "--split-days",
        "--date",
        "2026.11.15",
        "--out",
        "o",
        "--adsb",
        "c",
    ]))
    .unwrap_err();
    assert!(e.contains("given twice"), "{e}");
    assert!(parse_export(&v(&[
        "--date",
        "2026.11.15",
        "--out",
        "o",
        "--adsb",
        "c",
        "--eram-contradicted-address",
        "x"
    ]))
    .is_err());
    assert!(parse_export(&v(&["--date", "2026.11.15", "--out", "o"])).is_err());
    // slice
    let s = parse_slice(&v(&[
        "--in", "i", "--out", "o", "--from", "12:00:00", "--to", "13:00:00",
    ]))
    .unwrap();
    assert_eq!((s.from, s.to), (12 * 3600 * 128, 13 * 3600 * 128));
    let e = parse_slice(&v(&[
        "--in", "--out", "o", "--from", "12:00:00", "--to", "13:00:00",
    ]))
    .unwrap_err();
    assert!(e.contains("--in needs a value"), "{e}");
    let e = parse_slice(&v(&[
        "--in", "i", "--in", "j", "--out", "o", "--from", "12:00:00", "--to", "13:00:00",
    ]))
    .unwrap_err();
    assert!(e.contains("given twice"), "{e}");
}

// REQ: OA-CMP-047
#[test]
fn any_input_path_through_the_sealed_recording_is_refused_and_nothing_is_created() {
    assert!(refuse_holdout(Path::new("/data/scds/rec-20261119/sfdps")).is_err());
    assert!(refuse_holdout(Path::new("scds/rec-20261119")).is_err());
    // Only the exact component pair: neighbours are not it.
    assert!(refuse_holdout(Path::new("/data/scds/rec-20261119x")).is_ok());
    assert!(refuse_holdout(Path::new("/data/scds/day-20261114/rec/sfdps")).is_ok());
    assert!(refuse_holdout(Path::new("/data/rec-20261119")).is_ok());
    let work = tmp("hold");
    let out = work.join("new").join("out");
    let mk = |adsb: Option<PathBuf>, sfdps: Option<PathBuf>| Job {
        layout: Layout {
            out: out.clone(),
            day: 20_772,
            split: false,
        },
        adsb,
        sfdps: sfdps.map(EramSource::Dir),
        mode: InputsMode::All,
        rows: OpenatcaRows::Ticks,
        eram_address: EramAddress::Keep,
        self_test_eram_offset_e7: 0,
        self_test_adsb_offset_e7: 0,
    };
    for j in [
        mk(Some(PathBuf::from("x/scds/rec-20261119/clip.oaf")), None),
        mk(None, Some(PathBuf::from("x/scds/rec-20261119/rec/sfdps"))),
    ] {
        let e = run_job(j, &mut |_| Ok(()), &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(e.contains("sealed hold-out"), "{e}");
        assert!(!work.join("new").exists());
    }
    // A link into it is caught by the resolved path.
    #[cfg(unix)]
    {
        let real = work.join("scds").join("rec-20261119");
        fs::create_dir_all(&real).unwrap();
        let link = work.join("innocent");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(refuse_holdout(&link).is_err());
        assert!(slice_dir(&link, &work.join("o2"), 0, 128).is_err());
        assert!(!work.join("o2").exists());
    }
}

// REQ: OA-CMP-047
#[test]
fn the_sealed_date_is_refused_in_every_accepted_syntax() {
    for d in [
        "2026.11.19",
        "2026-11-19",
        "2026.11.019",
        "2026-011-019",
        "+2026-11-19",
    ] {
        let e = parse_export(&v(&["--date", d, "--out", "o", "--adsb", "c"])).unwrap_err();
        assert!(
            e.contains("2026-11-19 is the sealed hold-out day; it is never exported"),
            "{d}: {e}"
        );
    }
    // Forms that are not a date at all are rejected as such, not accepted as the sealed day.
    for d in ["20261119", "2026-11-19T00:00Z", "1795046400", "2026/11/19"] {
        let e = parse_export(&v(&["--date", d, "--out", "o", "--adsb", "c"])).unwrap_err();
        assert!(
            e.contains("is not a date") && !e.contains("sealed"),
            "{d}: {e}"
        );
    }
    assert!(parse_export(&v(&["--date", "2026.11.18", "--out", "o", "--adsb", "c"])).is_ok());
}

// REQ: OA-CMP-047
#[test]
#[cfg(unix)]
fn a_link_in_an_innocent_directory_into_the_sealed_recording_is_refused_for_every_file() {
    let work = tmp("links");
    let real = work.join("scds").join("rec-20261119");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("secret.msg"), b"x").unwrap();
    let inn = work.join("innocent");
    fs::create_dir_all(&inn).unwrap();
    let mk = |out: PathBuf, adsb: Option<PathBuf>, sfdps: Option<PathBuf>| Job {
        layout: Layout {
            out,
            day: 20_772,
            split: false,
        },
        adsb,
        sfdps: sfdps.map(EramSource::Dir),
        mode: InputsMode::All,
        rows: OpenatcaRows::Ticks,
        eram_address: EramAddress::Keep,
        self_test_eram_offset_e7: 0,
        self_test_adsb_offset_e7: 0,
    };
    // An SFDPS directory that looks innocent whose one file is a link into the sealed one.
    std::os::unix::fs::symlink(real.join("secret.msg"), inn.join("a.msg")).unwrap();
    let j = mk(work.join("out"), None, Some(inn.clone()));
    let e = run_job(j, &mut |_| Ok(()), &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(e.contains("sealed hold-out recording"), "{e}");
    assert!(!work.join("out").exists());
    // An ADS-B clip that is a link.
    std::os::unix::fs::symlink(real.join("secret.msg"), inn.join("c.oaf")).unwrap();
    let j = mk(work.join("out2"), Some(inn.join("c.oaf")), None);
    let e = run_job(j, &mut |_| Ok(()), &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(e.contains("sealed hold-out recording"), "{e}");
    assert!(!work.join("out2").exists());
    // A slice source with one .ast that is a link.
    let src = work.join("slicesrc");
    fs::create_dir_all(&src).unwrap();
    std::os::unix::fs::symlink(real.join("secret.msg"), src.join("a.ast")).unwrap();
    let e = slice_dir(&src, &work.join("o3"), 0, 128)
        .unwrap_err()
        .to_string();
    assert!(e.contains("sealed hold-out recording"), "{e}");
    assert!(!work.join("o3").exists());
}

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

// REQ: OA-MSAW-082
#[test]
fn rows_of_writes_only_the_selected_aircraft_clip_measurement_and_tracker_rows() {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("msaw-rows-of");
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    let target = 0x00ab_cdef;
    let other = 0x0012_3456;
    let mut reports = oa_adsb::Reports::default();
    for (addr, seconds, altitude, lat_offset) in [
        (target, 0, 5_000, 0),
        (other, 1, 8_000, 2_000),
        (target, 2, 5_100, 4_000),
        (other, 3, 8_100, 6_000),
        (target, 4, 5_200, 8_000),
    ] {
        let mut row = oa_adsb::ROW_EMPTY;
        row.time_ns = 1_700_000_000_000_000_000 + seconds * 1_000_000_000;
        row.icao = addr;
        row.lat_e7 = 410_000_000 + lat_offset;
        row.lon_e7 = -880_000_000;
        row.baro_alt_ft = altitude;
        row.geom_alt_ft = altitude;
        row.vrate_fpm = 100;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        oa_adsb::reports_push(&mut reports, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &reports, 0, oa_adsb::reports_len(&reports)).unwrap();
    let mut frame = vec![0u8; payload.len() + 256];
    let n = oa_feed::encode_frame(&mut frame, 1, oa_feed::FLAG_BYTES, 0, 0, 0, &payload).unwrap();
    let clip = t.join("synthetic.oaf");
    fs::write(&clip, &frame[..n]).unwrap();
    let cfg = t.join("dev.cfg");
    fs::write(&cfg, DEV_CFG).unwrap();
    let out = t.join("out");
    // SHA-256 prefix of the big-endian four-byte synthetic address 00 ab cd ef.
    let run = Command::new(env!("CARGO_BIN_EXE_oa-msaw"))
        .arg("--adsb")
        .arg(&clip)
        .arg("--config")
        .arg(&cfg)
        .arg("--out")
        .arg(&out)
        .arg("--rows-of")
        .arg("861c9aac")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let trace = fs::read_to_string(out.join("rows_of.csv")).unwrap();
    assert!(trace.starts_with(
        "# clip: source,time_s,baro_alt_ft,geom_alt_ft,vertical_rate_fpm,rate_kind,nac_p\n"
    ));
    for (time, altitude) in [(0, 5_000), (2, 5_100), (4, 5_200)] {
        assert!(trace.contains(&format!(
            "clip,{time}.0,{altitude},{altitude},100,rate_is_barometric,10\n"
        )));
        assert!(trace.contains(&format!("measurement,{time}.0,{altitude},100,")));
    }
    assert!(!trace.contains("clip,1.0,8000"));
    assert!(!trace.contains("clip,3.0,8100"));
    assert!(!trace.contains("measurement,1.0,8000"));
    assert!(!trace.contains("measurement,3.0,8100"));
    assert!(
        trace.lines().any(|line| line.starts_with("tracker,")),
        "{trace}"
    );

    let invalid = Command::new(env!("CARGO_BIN_EXE_oa-msaw"))
        .args([
            "--adsb",
            "unused.oaf",
            "--config",
            "unused.cfg",
            "--out",
            "unused",
        ])
        .args(["--rows-of", "861"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("exactly 8 hexadecimal digits"));
    let _ = fs::remove_dir_all(&t);
}

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

//! The `nas_hour` example (the M9 national-hour measurement) on a small clip made here: the
//! census counts distinct airborne addresses by hour and minute, the track mode reports its
//! capacity counters and measures only from the start time, and a sealed path is refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// The built example, next to the test binary's `deps` directory.
fn example() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("nas_hour")
}

/// 2026-11-19T21:00:00Z, Unix seconds.
const HOUR: i64 = 1_795_122_000;

/// A clip of `n` aircraft, each reporting every second for `secs` seconds from `from_s`, one
/// frame per second; aircraft 0 is on the ground throughout, the others airborne.
fn clip(path: &PathBuf, from_s: i64, secs: i64, n: u32) {
    let mut bytes = Vec::new();
    for s in 0..secs {
        let mut r = oa_adsb::Reports::default();
        for k in 0..n {
            let mut row = oa_adsb::ROW_EMPTY;
            row.time_ns = (from_s + s) * 1_000_000_000;
            row.icao = 0xA00001 + k;
            row.lat_e7 = 410_000_000 + (k as i32) * 100_000;
            row.lon_e7 = -880_000_000 + (s as i32) * 200;
            if k == 0 {
                row.flags = oa_adsb::F_GROUND;
            } else {
                row.baro_alt_ft = 30_000 + 1000 * k as i32;
            }
            row.gs_dkt = 4500;
            row.track_ddeg = 900;
            row.nic = 8;
            row.nac_p = 10;
            row.nac_v = 2;
            row.sil = 3;
            row.version = 2;
            oa_adsb::reports_push(&mut r, &row);
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, 0, n as usize).unwrap();
        let mut buf = vec![0u8; payload.len() + 256];
        let w = oa_feed::encode_frame(&mut buf, 1, oa_feed::FLAG_BYTES, 0, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..w]);
    }
    fs::write(path, &bytes).unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    t
}

// REQ: OA-CA-032
#[test]
fn the_census_counts_distinct_airborne_addresses_by_hour_and_minute() {
    let t = scratch("nas-hour-census");
    let c = t.join("c.oaf");
    // 120 s from 21:00:00Z: three aircraft, one of them on the ground, so two are airborne.
    clip(&c, HOUR, 120, 3);
    // seal-allow: runs the example, which checks every path in the guard before opening it
    let o = Command::new(example())
        .args(["census"])
        .arg(&c)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("rows 360, damaged frames 0"), "{out}");
    assert!(
        out.contains(&format!("busiest_hour_unix_s {HOUR} distinct_airborne 2")),
        "{out}"
    );
    assert!(
        out.contains(&format!("peak_minute_unix_s {HOUR} distinct_airborne 2")),
        "{out}"
    );
    // No address is printed.
    assert!(!out.to_lowercase().contains("a00001"), "{out}");
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-CA-032
#[test]
fn the_track_mode_measures_from_the_start_time_and_reports_its_capacity_counters() {
    let t = scratch("nas-hour-track");
    let c = t.join("c.oaf");
    // 60 s of warm-up before the hour and 90 s of the hour, four aircraft airborne.
    clip(&c, HOUR - 60, 150, 5);
    // seal-allow: runs the example, which checks every path in the guard before opening it
    let o = Command::new(example())
        .args(["track"])
        .arg(&c)
        .args(["--start", &HOUR.to_string(), "--max-tracks", "1024"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // 90 measured seconds of 4 airborne aircraft plus 1 on the ground: at most 5 a second, and
    // none of the 60 s of warm-up counted.
    let line = out
        .lines()
        .find(|l| l.starts_with("measured_reports"))
        .unwrap();
    let n: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!((1..=450).contains(&n), "{out}");
    assert!(out.contains("capacity_refused 0"), "{out}");
    assert!(out.contains("out_overflow 0"), "{out}");
    assert!(out.contains("tracker_cycle_1s"), "{out}");
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-CA-032
#[test]
fn a_sealed_path_and_a_missing_start_are_refused() {
    // seal-allow: runs the example, which checks every path in the guard before opening it
    let o = Command::new(example())
        .args([
            "census",
            "/home/connoravila/Documents/Avila-Labs/openatca-data/raw/adsblol/2026.11.20/a.oaf",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("sealed"));
    let t = scratch("nas-hour-args");
    let c = t.join("c.oaf");
    clip(&c, HOUR, 5, 2);
    // seal-allow: runs the example, which checks every path in the guard before opening it
    let o = Command::new(example())
        .arg("track")
        .arg(&c)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--start is required"));
    let _ = fs::remove_dir_all(&t);
}

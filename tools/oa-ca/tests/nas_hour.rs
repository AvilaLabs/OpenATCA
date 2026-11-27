//! The `nas_hour` example (the M9 national-hour measurement) on a small clip made here: the
//! census counts distinct airborne addresses by hour and minute, the track mode reports its
//! capacity counters and measures only from the start time, and a sealed path is refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[path = "../../../crates/oa-sealed/src/test_support/sealed_data_root.rs"]
mod sealed_data_root;

use std::fs;
use std::path::{Path, PathBuf};
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

fn assert_command_refused(output: &std::process::Output, code: i32, reason: &str) {
    assert_eq!(output.status.code(), Some(code));
    assert!(String::from_utf8_lossy(&output.stderr).contains(reason));
}

fn run_example_with_path(mode: &str, path: &Path) -> std::process::Output {
    Command::new(example())
        .arg(mode)
        .arg(path)
        .output()
        .unwrap()
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
            row.lon_e7 = -880_000_000 + (s as i32) * 27_500;
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
    for (seconds, complete) in [(90, false), (3600, true)] {
        // The five aircraft each report once a second after 60 s of warm-up.
        clip(&c, HOUR - 60, 60 + seconds, 5);
        // seal-allow: runs the example, which checks every path in the guard before opening it
        let o = Command::new(example())
            .args(["track"])
            .arg(&c)
            .args(["--start", &HOUR.to_string(), "--max-tracks", "1024"])
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let line = out
            .lines()
            .find(|l| l.starts_with("measured_reports"))
            .unwrap();
        let n: i64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert_eq!(n, seconds * 5, "{out}");
        assert!(out.contains("capacity_refused 0"), "{out}");
        assert!(out.contains("out_overflow 0"), "{out}");
        assert!(out.contains("tracker_cycle_1s"), "{out}");
        assert!(
            out.contains(&format!(
                "measured_hour_coverage observed_seconds {seconds} expected_seconds 3600 input_intact true"
            )),
            "{out}"
        );
        let factor = out
            .lines()
            .find(|line| line.starts_with("real_time_factor_measured_window "))
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap();
        if complete {
            assert!(factor.parse::<f64>().is_ok_and(f64::is_finite), "{out}");
        } else {
            assert_eq!(factor, "unavailable", "{out}");
        }
    }
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-CA-032
#[test]
fn a_sealed_path_and_a_missing_start_are_refused() {
    let sealed = sealed_data_root::data_root(env!("CARGO_MANIFEST_DIR"))
        .join("raw/adsblol/2026.11.20/a.oaf");
    // seal-allow: runs the example, which checks every path in the guard before opening it.
    let o = run_example_with_path("census", &sealed);
    assert_command_refused(&o, 1, "sealed");
    let t = scratch("nas-hour-args");
    let c = t.join("c.oaf");
    clip(&c, HOUR, 5, 2);
    // seal-allow: runs the example, which checks every path in the guard before opening it
    let o = run_example_with_path("track", &c);
    assert_command_refused(&o, 1, "--start is required");
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-CA-032
#[test]
fn the_ca_mode_measures_every_cycle_and_prints_the_budget_counters() {
    let t = scratch("nas-hour-ca");
    let c = t.join("c.oaf");
    clip(&c, HOUR - 60, 150, 5);
    let cfg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/dev.cfg");
    // seal-allow: runs the example, which checks every path in the guard before opening it
    let o = Command::new(example())
        .arg("ca")
        .arg(&c)
        .args(["--start", &HOUR.to_string(), "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // One cycle a second over the 90 measured seconds, and the cycles of the tracks coasting out
    // after the last report; none over a second, none short of work, none with tracks unscanned.
    let line = out.lines().find(|l| l.starts_with("ca_cycle ")).unwrap();
    let n: u64 = line.split_whitespace().nth(2).unwrap().parse().unwrap();
    assert!(n >= 90, "{out}");
    assert!(out.contains("ca_cycles_over_1s 0"), "{out}");
    assert!(
        out.contains("ca_cycles_work_short 0 cycles_tracks_unscanned 0"),
        "{out}"
    );
    assert!(out.contains("ca_pairs_evaluated_per_cycle max"), "{out}");
    assert!(out.contains("ca_totals_all_cycles"), "{out}");
    assert!(out.contains("not_evaluated: 0"), "{out}");
    let _ = fs::remove_dir_all(&t);
}

fn pt(addr: u32, t_s: f64, lat: i32, alt: i32, cov: f32) -> oa_track::Measurement {
    oa_track::Measurement {
        toa_ns: (t_s * 1e9) as i64,
        pos_cov_m2: [cov; 3],
        vel_cov_m2s2: [1.0; 3],
        vel_east_mps: lat as f32 / 1e6,
        vel_north_mps: 0.0,
        vel_lag_s: 0.0,
        lat_e7: lat,
        lon_e7: -880_000_000,
        alt_ft: alt,
        vrate_fpm: oa_track::NONE_I32,
        addr,
        alt_quant_ft: 25,
        sensor: 0,
        ident: oa_track::NO_IDENT,
        flags: oa_track::M_HAS_VEL,
        category: 0xA3,
    }
}

// REQ: OA-CA-032
#[test]
fn the_rate_stream_interpolates_at_the_rate_and_never_extrapolates_or_bridges_a_long_gap() {
    use oa_ca_run::synth::synth;
    // A: 10 s apart. B: exactly 60 s apart (bridged). C: 61 s apart (not). D: one point.
    // Different covariances on the two points of A: the earlier one is carried.
    let mut m = vec![
        pt(1, 0.0, 400_000_000, 30_000, 5.0),
        pt(1, 10.0, 410_000_000, 31_000, 9.0),
        pt(2, 0.0, 400_000_000, 30_000, 5.0),
        pt(2, 60.0, 400_600_000, 30_000, 5.0),
        pt(3, 0.0, 400_000_000, 30_000, 5.0),
        pt(3, 61.0, 400_610_000, 30_000, 5.0),
        pt(4, 3.0, 400_000_000, 30_000, 5.0),
    ];
    m.sort_by_key(|x| x.toa_ns);
    let s: Vec<_> = synth(&m, 2.0).unwrap().collect();
    let of = |a: u32| s.iter().filter(|x| x.addr == a).collect::<Vec<_>>();
    // Time order overall.
    assert!(s.windows(2).all(|w| w[0].toa_ns <= w[1].toa_ns));
    // A: the two kept points and 19 between, every half second, none past 10 s.
    let a = of(1);
    assert_eq!(a.len(), 21);
    assert_eq!(a.last().unwrap().toa_ns, 10_000_000_000);
    for (k, p) in a.iter().enumerate() {
        assert_eq!(p.toa_ns, k as i64 * 500_000_000);
        // Linear in position and altitude; the quality is the earlier point's, except the last.
        assert_eq!(p.lat_e7, 400_000_000 + k as i32 * 500_000);
        assert_eq!(p.alt_ft, 30_000 + k as i32 * 50);
        if k < 20 {
            assert_eq!(p.pos_cov_m2, [5.0; 3]);
        }
    }
    // The velocity is interpolated too (the half-way value of 400 and 410).
    assert!((a[10].vel_east_mps - 405.0).abs() < 1e-3);
    // B (60 s): bridged, 119 points between; C (61 s) and D (one point): kept points only.
    assert_eq!(of(2).len(), 2 + 119);
    assert_eq!(of(3).len(), 2);
    assert_eq!(of(4).len(), 1);
    // A rate that does not divide the gap: the last synthetic point is before the kept one.
    let s3: Vec<_> = synth(&m[..2], 0.3).unwrap().collect();
    assert!(s3.iter().all(|x| x.toa_ns <= 10_000_000_000));
    // A bad rate is refused.
    assert!(synth(&m, 0.0).is_err());
    assert!(synth(&m, f64::NAN).is_err());
}

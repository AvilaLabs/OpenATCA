//! The `oa-sensorsim` binary on a synthetic OAF1 clip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::process::Command;

use oa_adsb::{encode_payload, reports_len, reports_push, Reports, ROW_EMPTY};
use oa_asterix::{decode_stream, Block};
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn clip() -> Vec<u8> {
    let mut r = Reports::default();
    for addr in [0xa00001u32, 0xa00002] {
        for k in 0..200 {
            let mut row = ROW_EMPTY;
            row.icao = addr;
            row.time_ns = 1_700_000_000_000_000_000 + i64::from(k) * 2_000_000_000;
            row.lat_e7 = 411_000_000 + k * 300 + (addr as i32 & 3) * 1_000_000;
            row.lon_e7 = -880_000_000 + k * 400;
            row.nic = 8;
            row.nac_p = 10;
            row.sil = 3;
            row.version = 2;
            row.source = 0;
            row.baro_alt_ft = 12_000;
            reports_push(&mut r, &row);
        }
    }
    let n = reports_len(&r);
    let mut bytes = Vec::new();
    let (mut at, mut seq) = (0, 0u64);
    while at < n {
        let end = (at + 100).min(n);
        let mut payload = Vec::new();
        encode_payload(&mut payload, &r, at, end).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..len]);
        seq += 1;
        at = end;
    }
    bytes
}

// REQ: OA-SIM-020
#[test]
fn a_bad_record_costs_the_clip_that_record_and_is_counted() {
    let mut r = Reports::default();
    for k in 0..10 {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00001;
        row.time_ns = 1_700_000_000_000_000_000 + i64::from(k) * 2_000_000_000;
        row.lat_e7 = 411_000_000;
        row.lon_e7 = -880_000_000 + k * 400;
        reports_push(&mut r, &row);
    }
    let mut payload = Vec::new();
    encode_payload(&mut payload, &r, 0, 10).unwrap();
    let o = 16 + oa_adsb::RECORD_LEN + 4;
    payload[o] = 0x7f;
    payload[o + 3] = 0x7f;
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let len = encode_frame(&mut buf, 1, FLAG_BYTES, 0, 0, 0, &payload).unwrap();
    let path = tmp("sensorsim-bad.oaf");
    std::fs::write(&path, &buf[..len]).unwrap();
    let (reports, bad, dropped) = oa_sensorsim::clip::read_clip(path.to_str().unwrap()).unwrap();
    assert_eq!((reports_len(&reports), bad, dropped), (9, 0, 1));
}

// REQ: OA-SIM-019
#[test]
fn binary_writes_plots_for_a_clip_and_demands_the_biases() {
    let path = tmp("sensorsim.oaf");
    std::fs::write(&path, clip()).unwrap();
    let exe = env!("CARGO_BIN_EXE_oa-sensorsim");
    let run = |dir: &str, extra: &[&str]| {
        let out = tmp(dir);
        let mut c = Command::new(exe);
        c.arg(&path).arg(&out).args([
            "--site",
            "custom",
            "--lat",
            "41.0",
            "--lon",
            "-88.0",
            "--height-m",
            "30",
            "--period-s",
            "4.8",
            "--coverage-nm",
            "100",
            "--split-dr-m",
            "300",
            "--split-daz-deg",
            "0.3",
        ]);
        c.args(extra);
        (c.output().unwrap(), out)
    };
    let (o, out1) = run(
        "sim_a",
        &[
            "--bias-range-m",
            "50",
            "--bias-az-deg",
            "0.1",
            "--seed",
            "5",
        ],
    );
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "{stdout} {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(stdout.contains("plots:"), "{stdout}");
    let bytes = std::fs::read(out1.join("custom.cat048")).unwrap();
    let n: usize = decode_stream(&bytes)
        .unwrap()
        .iter()
        .map(|b| match b {
            Block::Cat048(r) => r.len(),
            _ => 0,
        })
        .sum();
    assert!(n > 50, "{n}");
    let csv = std::fs::read_to_string(out1.join("custom.plots.csv")).unwrap();
    assert_eq!(csv.lines().count(), n + 1);
    // Same arguments, same bytes.
    let (_, out2) = run(
        "sim_b",
        &[
            "--bias-range-m",
            "50",
            "--bias-az-deg",
            "0.1",
            "--seed",
            "5",
        ],
    );
    assert_eq!(bytes, std::fs::read(out2.join("custom.cat048")).unwrap());
    // No sourced bias, no run.
    let (o, _) = run("sim_c", &["--bias-az-deg", "0.1"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("--bias-range-m is required"));
}

// REQ: OA-SIM-023
#[test]
fn a_clip_that_ends_inside_a_frame_keeps_its_valid_prefix_and_counts_the_partial_frame() {
    let whole = clip();
    // The clip is whole frames; cut the last one to a partial header and to a partial body.
    let frame_len = whole.len() / 4;
    let extra = &whole[..frame_len];
    for (name, cut, bad) in [("whole", 0, 0), ("header", 10, 1), ("body", 60, 1)] {
        let mut bytes = whole.clone();
        bytes.extend_from_slice(&extra[..cut]);
        let path = tmp(&format!("sensorsim-tail-{name}.oaf"));
        std::fs::write(&path, &bytes).unwrap();
        let (reports, nbad, dropped) =
            oa_sensorsim::clip::read_clip(path.to_str().unwrap()).unwrap();
        assert_eq!(
            (reports_len(&reports), nbad, dropped),
            (400, bad, 0),
            "{name}"
        );
    }
}

// REQ: OA-SIM-027
#[test]
fn a_non_finite_numeric_option_is_refused() {
    let path = tmp("sensorsim-nonfinite.oaf");
    std::fs::write(&path, clip()).unwrap();
    let exe = env!("CARGO_BIN_EXE_oa-sensorsim");
    let base = [
        ("--lat", "41"),
        ("--lon", "-88"),
        ("--height-m", "30"),
        ("--period-s", "4.8"),
        ("--coverage-nm", "100"),
        ("--bias-range-m", "50"),
        ("--bias-az-deg", "0.1"),
        ("--split-dr-m", "300"),
        ("--split-daz-deg", "0.3"),
        ("--earth-k", "1.33"),
    ];
    for (opt, v) in [
        ("--lat", "nan"),
        ("--lon", "inf"),
        ("--height-m", "-inf"),
        ("--period-s", "1e999"),
        ("--bias-az-deg", "NaN"),
        ("--earth-k", "inf"),
    ] {
        let mut c = Command::new(exe);
        c.arg(&path)
            .arg(tmp("sim_nonfinite"))
            .args(["--site", "custom"]);
        for (k, w) in base {
            c.arg(k).arg(if k == opt { v } else { w });
        }
        let o = c.output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{opt} {v}");
        assert!(err.contains("not a finite number"), "{opt} {v}: {err}");
    }
}

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

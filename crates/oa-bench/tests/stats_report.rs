//! Statistics definitions, splits, the report and the binary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::report::{code_version, render_csv, render_text, Header, InputHash};
use oa_bench::stats::{nearest_rank, split_rows, stats_of, Splits};
use oa_bench::synth::{run_world, self_test_config, Injected};

// REQ: OA-BENCH-001
#[test]
fn percentiles_are_nearest_rank_and_independent_of_order() {
    // 1..=100 in a scrambled order: rank ceil(p*n/100) is exactly p.
    let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
    v.sort_by_key(|x| (*x as u64 * 37) % 101);
    let s = stats_of(&v);
    assert_eq!(
        (s.n, s.median, s.p95, s.p99, s.max),
        (100, 50.0, 95.0, 99.0, 100.0)
    );
    assert_eq!(s.mean, 50.5);
    // Seven values: ranks ceil(3.5)=4, ceil(6.65)=7, ceil(6.93)=7.
    let s = stats_of(&[7.0, 1.0, 5.0, 3.0, 2.0, 6.0, 4.0]);
    assert_eq!((s.median, s.p95, s.p99, s.max), (4.0, 7.0, 7.0, 7.0));
    // Mean is signed, the rest are absolute values.
    let s = stats_of(&[-3.0, 1.0, -2.0, 4.0]);
    assert_eq!(s.mean, 0.0);
    assert_eq!(s.max, 4.0);
    assert_eq!(s.median, 2.0);
    assert!((s.rms - (30.0f64 / 4.0).sqrt()).abs() < 1e-12);
    // One value, and nothing.
    let s = stats_of(&[-2.5]);
    assert_eq!(
        (s.n, s.median, s.p99, s.max, s.mean),
        (1, 2.5, 2.5, 2.5, -2.5)
    );
    assert_eq!(stats_of(&[]).n, 0);
    // Non-finite values are skipped, not propagated.
    let s = stats_of(&[1.0, f64::NAN, f64::INFINITY, 3.0]);
    assert_eq!((s.n, s.mean), (2, 2.0));
    assert_eq!(nearest_rank(&[], 50), 0.0);
    // The same values in two orders give identical results bit for bit.
    let a = stats_of(&[0.1, 0.7, -0.3, 2.2, 5.5, 0.01]);
    let b = stats_of(&[5.5, 0.01, 2.2, 0.7, -0.3, 0.1]);
    assert_eq!(a.median.to_bits(), b.median.to_bits());
    assert_eq!(a.p95.to_bits(), b.p95.to_bits());
}

// REQ: OA-BENCH-002
#[test]
fn every_metric_is_split_by_phase_altitude_band_coast_and_method() {
    let vals = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let sp = Splits {
        phase: &[0, 0, 1, 1, 2, 3],
        band: &[0, 1, 2, 3, 4, 5],
        coast: &[0, 0, 0, 0, 1, 1],
        method: &[1, 1, 2, 2, 3, 3],
    };
    let rows = split_rows(&vals, &sp);
    let n = |l: &str| rows.iter().find(|r| r.label == l).map(|r| r.stats.n);
    assert_eq!(n("all"), Some(6));
    assert_eq!(n("phase=climb"), Some(2));
    assert_eq!(n("phase=cruise"), Some(2));
    assert_eq!(n("phase=descent"), Some(1));
    assert_eq!(n("phase=unknown"), Some(1));
    assert_eq!(n("altitude=no altitude"), Some(1));
    assert_eq!(n("coast=coasting"), Some(2));
    assert_eq!(n("coast=not coasting"), Some(4));
    assert_eq!(n("method=address"), Some(2));
    assert_eq!(n("method=callsign"), Some(2));
    assert_eq!(n("method=address hint"), Some(2));
    // Each split partitions the samples.
    for key in ["phase=", "altitude=", "coast=", "method="] {
        let total: u64 = rows
            .iter()
            .filter(|r| r.label.starts_with(key))
            .map(|r| r.stats.n)
            .sum();
        assert_eq!(total, 6, "{key}");
    }
    // The altitude bands break at 10,000, 18,000, 29,000 and 41,000 feet.
    use oa_bench::stats::band;
    assert_eq!(
        [9_999, 10_000, 17_999, 18_000, 28_999, 29_000, 40_999, 41_000].map(|a| band::of(Some(a))),
        [0, 1, 1, 2, 2, 3, 3, 4]
    );
    assert_eq!(band::of(None), 5);
}

fn header() -> Header {
    Header {
        mode: "synthetic".into(),
        code_version: code_version(),
        inputs: vec![InputHash {
            label: "sfdps (x)".into(),
            files: 2,
            bytes: 10,
            sha256: "ab".repeat(32),
        }],
        replay_hash: "cd".repeat(32),
        counts: vec![("flights".into(), 3)],
        notes: vec!["a note, with a comma".into()],
        truth: Vec::new(),
    }
}

// REQ: OA-BENCH-020
#[test]
fn the_report_carries_hashes_versions_every_metric_with_n_and_the_exclusions() {
    let (_, out) = run_world(&Injected::default(), &self_test_config());
    let h = header();
    let text = render_text(&h, &out);
    let csv = render_csv(&h, &out);
    assert!(text.contains(&"ab".repeat(32)));
    assert!(text.contains(&"cd".repeat(32)));
    assert!(text.contains("oa-bench metrics v1"));
    assert!(text.contains("Exclusions"));
    assert!(
        text.contains("internal track"),
        "the scope statement is in the header"
    );
    assert!(
        text.contains("nearest-rank"),
        "the percentile method is in the header"
    );
    for m in &out.metrics {
        assert!(
            text.contains(&format!("[{}]", m.id)),
            "{} missing from the text",
            m.id
        );
        // Every stats row prints its N.
        for r in &m.rows {
            assert!(
                csv.contains(&format!("stat,{},{},{},", m.id, r.label, r.stats.n)),
                "{} {}",
                m.id,
                r.label
            );
        }
    }
    assert!(csv.starts_with("kind,metric,label,n,mean,rms,median,p95,p99,max\n"));
    assert!(csv.contains(&format!("header,,replay_state_hash,{}", "cd".repeat(32))));
    assert!(csv.contains(&format!("input,,sfdps (x),2,10,{}", "ab".repeat(32))));
    // Fields with a comma are quoted.
    assert!(csv.contains("note,,\"a note, with a comma\""));
    // Every exclusion appears with its count.
    for ((scope, reason), n) in &out.exclusions {
        assert!(
            csv.contains(&format!("exclusion,{scope},{reason},{n}")),
            "{scope} {reason}"
        );
    }
    assert!(!out.exclusions.is_empty());
    // A withheld metric prints its reason.
    let drift = Injected {
        tau_ramp: 0.004,
        ..Injected::default()
    };
    let (_, out2) = run_world(&drift, &self_test_config());
    assert!(render_text(&h, &out2).contains("WITHHELD: the offset drifts"));
    assert!(render_csv(&h, &out2).contains("withheld,T2-offset,"));
}

// REQ: OA-BENCH-021
#[test]
fn identical_inputs_give_byte_identical_reports() {
    let a = run_world(&Injected::default(), &self_test_config()).1;
    let b = run_world(&Injected::default(), &self_test_config()).1;
    let h = header();
    assert_eq!(render_text(&h, &a), render_text(&h, &b));
    assert_eq!(render_csv(&h, &a), render_csv(&h, &b));
    // A different injection changes the report (it is not constant).
    let c = run_world(
        &Injected {
            cross_bias_m: 50.0,
            ..Injected::default()
        },
        &self_test_config(),
    )
    .1;
    assert_ne!(render_csv(&h, &a), render_csv(&h, &c));
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("oa-bench-bin-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

// REQ: OA-BENCH-023
#[test]
fn the_binary_runs_the_synthetic_world_and_refuses_bad_arguments() {
    let exe = env!("CARGO_BIN_EXE_oa-bench");
    // An ADS-B run needs --out; a gate override must be a positive number.
    let out = std::process::Command::new(exe)
        .args(["--adsb", "clip.oaf"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = std::process::Command::new(exe)
        .args(["--truth-bracket-s", "-3", "--out", "x"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));

    let dir = scratch("synthetic");
    let run = |d: &std::path::Path| {
        std::process::Command::new(exe)
            .args(["--synthetic", "--out"])
            .arg(d)
            .output()
            .unwrap()
    };
    let o1 = run(&dir);
    assert!(
        o1.status.success(),
        "{}",
        String::from_utf8_lossy(&o1.stderr)
    );
    assert!(String::from_utf8_lossy(&o1.stdout).contains("0 failed"));
    let dir2 = scratch("synthetic2");
    assert!(run(&dir2).status.success());
    for f in ["report.txt", "report.csv"] {
        let (a, b) = (
            std::fs::read(dir.join(f)).unwrap(),
            std::fs::read(dir2.join(f)).unwrap(),
        );
        assert!(!a.is_empty());
        assert_eq!(a, b, "{f} differs between two runs");
    }
    let text = std::fs::read_to_string(dir.join("report.txt")).unwrap();
    assert!(text.contains("check T9 association errors without a swap"));
    // No mode: usage error.
    let none = std::process::Command::new(exe).output().unwrap();
    assert_eq!(none.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

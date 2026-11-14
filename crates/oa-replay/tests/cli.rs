//! Tests of the command line and of the `oa-replay` binary end to end.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_replay::cli::{parse_args, Input, DEFAULT_CHUNK};
use oa_replay::{FeedKind, NS_PER_S};
use std::path::PathBuf;
use std::process::Command;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

// REQ: OA-RPL-023
#[test]
fn command_line_gives_each_file_its_feed_kind_and_validates_numbers() {
    let c = parse_args(&args(&[
        "--sfdps", "a.oaf", "--tbfm", "b.oaf.gz", "--sfdps", "c.oaf",
    ]))
    .unwrap();
    let inp = |feed, p: &str, dir| Input {
        feed,
        path: PathBuf::from(p),
        dir,
    };
    assert_eq!(
        c.inputs,
        vec![
            inp(FeedKind::Sfdps, "a.oaf", false),
            inp(FeedKind::Tbfm, "b.oaf.gz", false),
            inp(FeedKind::Sfdps, "c.oaf", false),
        ]
    );
    let d = parse_args(&args(&[
        "--sfdps-dir",
        "s",
        "--tbfm-dir",
        "t",
        "--tfms-dir",
        "f",
        "--fns-dir",
        "n",
        "--tfms",
        "x",
        "--fns",
        "y",
        "--progress-minutes",
        "3",
    ]))
    .unwrap();
    assert_eq!(
        d.inputs,
        vec![
            inp(FeedKind::Sfdps, "s", true),
            inp(FeedKind::Tbfm, "t", true),
            inp(FeedKind::Tfms, "f", true),
            inp(FeedKind::Fns, "n", true),
            inp(FeedKind::Tfms, "x", false),
            inp(FeedKind::Fns, "y", false),
        ]
    );
    assert_eq!(d.progress_ns, 180 * NS_PER_S);
    assert_eq!(c.progress_ns, 600 * NS_PER_S);
    assert_eq!(c.cfg.checkpoint_ns, 10 * NS_PER_S);
    assert_eq!(c.cfg.table.max_age_ns, 30 * 60 * NS_PER_S);
    assert_eq!(c.cfg.table.capacity, 32_768);
    assert_eq!((c.chunk, c.quiet), (DEFAULT_CHUNK, false));
    let c = parse_args(&args(&[
        "--tbfm",
        "x",
        "--checkpoint-secs",
        "5",
        "--max-age-secs",
        "60",
        "--sweep-secs",
        "7",
        "--capacity",
        "100",
        "--chunk-bytes",
        "9",
        "--quiet",
    ]))
    .unwrap();
    assert_eq!(c.cfg.checkpoint_ns, 5 * NS_PER_S);
    assert_eq!(c.cfg.table.max_age_ns, 60 * NS_PER_S);
    assert_eq!(c.cfg.table.sweep_ns, 7 * NS_PER_S);
    assert_eq!((c.cfg.table.capacity, c.chunk, c.quiet), (100, 9, true));
    for bad in [
        vec![],
        vec!["--quiet"],
        vec!["--sfdps"],
        vec!["--sfdps", "a", "--capacity", "0"],
        vec!["--sfdps", "a", "--checkpoint-secs", "x"],
        vec!["--sfdps", "a", "--max-age-secs"],
        vec!["--sfdps", "a", "--bogus"],
        vec!["--sfdps-dir"],
        vec!["--sfdps", "a", "--progress-minutes", "0"],
        vec!["--sfdps", "a", "--checkpoint-secs", "99999999999999999999"],
    ] {
        assert!(parse_args(&args(&bad)).is_err(), "{bad:?}");
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-replay-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_bin(a: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_oa-replay"))
        .args(a)
        .output()
        .unwrap();
    (
        o.status.code().unwrap(),
        String::from_utf8(o.stdout).unwrap(),
        String::from_utf8(o.stderr).unwrap(),
    )
}

/// The output without the throughput line, which is the only one that depends on the clock.
fn stable(out: &str) -> String {
    out.lines()
        .filter(|l| !l.starts_with("timing"))
        .collect::<Vec<_>>()
        .join("\n")
}

// REQ: OA-RPL-024
#[test]
fn binary_prints_checkpoints_and_a_summary_that_is_identical_run_to_run() {
    let dir = scratch("bin");
    let mut s = Vec::new();
    let mut t = Vec::new();
    let t0 = DAY * 1_000_000 + 3_500_000_000;
    for k in 0..60i64 {
        let sp = sf_doc(&[sf((k % 7) as u32 + 1, k * 1000)
            .plan(&format!("KA{:06}AB", k % 7 + 1))
            .call("TST1")
            .pos(30.0, -80.0, k * 1000)]);
        s.extend(frame(1, k as u64, t0 + k * 1_500_000_000, sp.as_bytes()));
        let tp = tb_doc(
            k * 1000,
            "ZTT",
            &[
                tb(k * 1000, Some(&format!("KA{:06}AB", k % 7 + 1)), "T00001")
                    .mrp("MFXA1", "09L", "NEW"),
            ],
        );
        t.extend(frame(1, k as u64, t0 + k * 1_700_000_000, tp.as_bytes()));
    }
    let sf_path = dir.join("a.oaf");
    let tb_path = dir.join("b.oaf");
    std::fs::write(&sf_path, &s).unwrap();
    std::fs::write(&tb_path, &t).unwrap();
    let (sp, tp) = (sf_path.to_str().unwrap(), tb_path.to_str().unwrap());
    let (code, out, _) = run_bin(&["--sfdps", sp, "--tbfm", tp]);
    assert_eq!(code, 0);
    assert!(out.lines().filter(|l| l.starts_with("checkpoint ")).count() >= 10);
    assert!(out.contains("summary files=2 "));
    assert!(out.contains("errors none"));
    assert!(out.lines().any(|l| l.starts_with("timing ")));
    let hash_line = out.lines().find(|l| l.starts_with("final ")).unwrap();
    assert!(hash_line.contains("state_hash="));
    let (_, out2, _) = run_bin(&["--sfdps", sp, "--tbfm", tp]);
    assert_eq!(stable(&out), stable(&out2));
    // The flags in another order, and a different read size: the same text.
    let (_, out3, _) = run_bin(&["--chunk-bytes", "37", "--tbfm", tp, "--sfdps", sp]);
    assert_eq!(stable(&out), stable(&out3));
    // --quiet drops only the checkpoint lines.
    let (_, out4, _) = run_bin(&["--quiet", "--sfdps", sp, "--tbfm", tp]);
    assert!(!out4.contains("checkpoint "));
    let body: Vec<&str> = out
        .lines()
        .filter(|l| !l.starts_with("checkpoint ") && !l.starts_with("timing"))
        .collect();
    assert_eq!(stable(&out4).lines().collect::<Vec<_>>(), body);
    // The same recordings as rotated files in a directory, one compressed, named in order:
    // chained into one stream, the same replay (OA-RPL-028). Another directory for TBFM.
    let (sdir, tdir) = (dir.join("sf"), dir.join("tb"));
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::create_dir_all(&tdir).unwrap();
    let cut = |bytes: &[u8]| -> Vec<Vec<u8>> {
        let mut parts = Vec::new();
        let (mut at, mut start, mut n) = (0usize, 0usize, 0);
        while at < bytes.len() {
            at += 40 + u32::from_le_bytes(bytes[at + 32..at + 36].try_into().unwrap()) as usize;
            n += 1;
            if n % 20 == 0 || at == bytes.len() {
                parts.push(bytes[start..at].to_vec());
                start = at;
            }
        }
        parts
    };
    for (d, bytes) in [(&sdir, &s), (&tdir, &t)] {
        for (i, part) in cut(bytes).iter().enumerate() {
            std::fs::write(d.join(format!("scds-x-{:04}.oaf", i + 1)), part).unwrap();
        }
    }
    std::fs::write(sdir.join("notes.txt"), b"not a recording").unwrap();
    let (_, outd, _) = run_bin(&[
        "--sfdps-dir",
        sdir.to_str().unwrap(),
        "--tbfm-dir",
        tdir.to_str().unwrap(),
    ]);
    let norm = |o: &str| -> Vec<String> {
        stable(o)
            .lines()
            .filter(|l| !l.starts_with("summary "))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(norm(&out), norm(&outd));
    let (code, _, err) = run_bin(&["--sfdps-dir", dir.join("nodir").to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("nodir"));
    // Usage and read errors.
    let (code, _, err) = run_bin(&[]);
    assert_eq!(code, 2);
    assert!(err.contains("usage"));
    let (code, _, err) = run_bin(&["--sfdps", dir.join("missing.oaf").to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("missing.oaf"));
    std::fs::remove_dir_all(&dir).unwrap();
}

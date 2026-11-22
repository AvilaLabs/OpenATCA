//! Tests of the command line and of the `oa-replay` binary end to end.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_deflate::GzipWriter;
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
    assert!(!c.input_fingerprint);
    assert!(
        parse_args(&args(&["--sfdps", "x", "--input-fingerprint"]))
            .unwrap()
            .input_fingerprint
    );
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

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut writer = GzipWriter::new(6);
    let mut sink = |bytes: &[u8]| -> Result<(), ()> {
        out.extend_from_slice(bytes);
        Ok(())
    };
    writer.write(data, &mut sink).unwrap();
    writer.finish(&mut sink).unwrap();
    out
}

/// The output without the throughput line, which is the only one that depends on the clock.
fn stable(out: &str) -> String {
    out.lines()
        .filter(|l| !l.starts_with("timing"))
        .collect::<Vec<_>>()
        .join("\n")
}

// REQ: OA-SEAL-014
#[test]
fn a_receive_time_drop_marks_the_input_fingerprint_partial() {
    let (sealed_from_ms, _) = oa_sealed::sealed_window_ms().unwrap();
    let sealed_from_ns = sealed_from_ms * 1_000_000;
    let dir = scratch("fingerprint-window");
    let path = dir.join("synthetic.oaf");
    let mut bytes = frame(3, 0, sealed_from_ns - NS_PER_S, b"state=up\n");
    bytes.extend(frame(3, 1, sealed_from_ns, b"state=up\n"));
    std::fs::write(&path, bytes).unwrap();
    let (code, _, err) = run_bin(&["--sfdps", path.to_str().unwrap(), "--input-fingerprint"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("status=partial pipeline_frames=1"), "{err}");
    assert!(
        err.contains("source_sealed_drops=1 semantic_sealed_drops=0"),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_future_adsb_refusal_marks_the_input_fingerprint_partial() {
    let dir = scratch("fingerprint-future");
    let path = dir.join("synthetic.oaf");
    let t0 = DAY * 1_000_000;
    let mut reports = oa_adsb::Reports::default();
    oa_adsb::reports_push(
        &mut reports,
        &oa_adsb::Row {
            icao: 0xAB_0000,
            time_ns: t0 + oa_replay::ADSB_MAX_AHEAD_NS + 1,
            lat_e7: 420_000_000,
            lon_e7: -880_000_000,
            baro_alt_ft: 30_000,
            ..oa_adsb::ROW_EMPTY
        },
    );
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &reports, 0, 1).unwrap();
    std::fs::write(&path, frame(1, 0, t0, &payload)).unwrap();
    let (code, out, err) = run_bin(&["--adsb", path.to_str().unwrap(), "--input-fingerprint"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("future_refused=1"), "{out}");
    assert!(err.contains("status=partial pipeline_frames=1"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-RPL-024, OA-SEAL-014
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
    let (code, out, err) = run_bin(&["--sfdps", sp, "--tbfm", tp]);
    assert_eq!(code, 0);
    assert!(!err.contains("input_fingerprint"));
    // The count line, printed once.
    assert_eq!(
        err.matches("sealed-window rows dropped: 0").count(),
        1,
        "{err}"
    );
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
            let compressed = i % 2 == 0;
            let name = if compressed {
                format!("scds-x-{:04}.oaf.gz", i + 1)
            } else {
                format!("scds-x-{:04}.oaf", i + 1)
            };
            std::fs::write(
                d.join(name),
                if compressed { gzip(part) } else { part.clone() },
            )
            .unwrap();
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
    let (_, fingerprint_out, raw_fingerprint) =
        run_bin(&["--input-fingerprint", "--sfdps", sp, "--tbfm", tp]);
    assert_eq!(stable(&out), stable(&fingerprint_out));
    let (_, _, split_fingerprint) = run_bin(&[
        "--input-fingerprint",
        "--sfdps-dir",
        sdir.to_str().unwrap(),
        "--tbfm-dir",
        tdir.to_str().unwrap(),
        "--chunk-bytes",
        "37",
    ]);
    fn input_line(err: &str) -> &str {
        err.lines()
            .find(|line| line.starts_with("input_fingerprint "))
            .unwrap()
    }
    assert_eq!(input_line(&raw_fingerprint), input_line(&split_fingerprint));
    assert!(input_line(&raw_fingerprint).contains("status=complete"));
    assert!(input_line(&raw_fingerprint).contains("pipeline_frames=120"));
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

// REQ: OA-RPL-038
#[test]
fn adsb_flags_name_files_and_directories_and_the_binary_replays_them() {
    let c = parse_args(&args(&[
        "--adsb",
        "a.oaf",
        "--adsb-dir",
        "d",
        "--sfdps",
        "s.oaf",
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
            inp(FeedKind::Adsb, "a.oaf", false),
            inp(FeedKind::Adsb, "d", true),
            inp(FeedKind::Sfdps, "s.oaf", false),
        ]
    );
    assert!(parse_args(&args(&["--adsb"])).is_err());
    assert!(parse_args(&args(&["--adsb-dir"])).is_err());
    assert!(oa_replay::cli::USAGE.contains("--adsb"));

    let dir = scratch("adsb");
    let t0 = DAY * 1_000_000 + 3_500_000_000;
    let mut a = Vec::new();
    for k in 0..30i64 {
        let mut r = oa_adsb::Reports::default();
        for j in 0..3u32 {
            oa_adsb::reports_push(
                &mut r,
                &oa_adsb::Row {
                    icao: 0xAB_0000 + j,
                    time_ns: t0 + k * 2_000_000_000,
                    lat_e7: 420_000_000,
                    lon_e7: -880_000_000,
                    baro_alt_ft: 30_000,
                    ..oa_adsb::ROW_EMPTY
                },
            );
        }
        let mut p = Vec::new();
        oa_adsb::encode_payload(&mut p, &r, 0, 3).unwrap();
        a.extend(frame(1, k as u64, t0 + k * 2_000_000_000, &p));
    }
    a.extend(frame(1, 30, t0 + 61_000_000_000, b"garbage"));
    let file = dir.join("adsb.oaf");
    std::fs::write(&file, &a).unwrap();
    let (code, out, _) = run_bin(&["--adsb", file.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(out.contains(
        "feed adsb frames=31 messages=31 other_frames=0 not_applied=0 rows=90 applied=90"
    ));
    assert!(out.contains("adsb aircraft=3 capacity=65536 applied=90 out_of_order=0 refused_full=0"));
    assert!(out.contains("errors adsb-length=1"));
    // A directory of rotated files is the same stream; the same text apart from the file count.
    let d = dir.join("rot");
    std::fs::create_dir_all(&d).unwrap();
    let mid = a.len() / 2;
    let mut cut = 0;
    let mut at = 0;
    while at < mid {
        at += 40 + u32::from_le_bytes(a[at + 32..at + 36].try_into().unwrap()) as usize;
        cut = at;
    }
    std::fs::write(d.join("adsb-0001.oaf"), &a[..cut]).unwrap();
    std::fs::write(d.join("adsb-0002.oaf"), &a[cut..]).unwrap();
    let (_, outd, _) = run_bin(&["--adsb-dir", d.to_str().unwrap()]);
    let norm = |o: &str| -> Vec<String> {
        stable(o)
            .lines()
            .filter(|l| !l.starts_with("summary "))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(norm(&out), norm(&outd));
    let (_, out2, _) = run_bin(&["--chunk-bytes", "41", "--adsb", file.to_str().unwrap()]);
    assert_eq!(stable(&out), stable(&out2));
    std::fs::remove_dir_all(&dir).unwrap();
}

// REQ: OA-RPL-254
#[test]
fn progress_arithmetic_survives_the_widest_clock_span() {
    let dir = scratch("span");
    let mut bytes = Vec::new();
    let times = [i64::MIN + 1, 0, 0, 0, i64::MAX, i64::MAX, i64::MAX];
    for (i, t) in times.iter().enumerate() {
        bytes.extend(frame(3, i as u64, *t, b"state=connected\n"));
    }
    let file = dir.join("span.oaf");
    std::fs::write(&file, &bytes).unwrap();
    let (code, out, err) = run_bin(&["--sfdps", file.to_str().unwrap(), "--quiet"]);
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(out.contains("summary "), "{out}");
    std::fs::remove_dir_all(&dir).unwrap();
}

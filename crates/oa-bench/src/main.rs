#![forbid(unsafe_code)]
//! `oa-bench`: runs the M2 benchmark. Shell (B3): arguments, files, stdout.
//!
//! ```text
//! oa-bench --out <dir> (--synthetic | [--sfdps-dir <dir>] [--tbfm-dir <dir>] [--tfms-dir <dir>] --adsb <clip.oaf>)
//! ```
//!
//! Truth (`--adsb`) comes from `oa-truth`, which is not wired in yet: `--adsb` is refused with a
//! message saying so. `--synthetic` runs the self-test world and writes the report. With only the
//! legacy directories (no truth), `--extract-only` prints the extraction counts.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use oa_bench::input::{load, FeedInput};
use oa_bench::report::{code_version, render_csv, render_text, Header};
use oa_bench::run::run_benchmark;
use oa_bench::synth::{
    build_world, check_ok, env, self_test, self_test_config, truth_at, Injected,
};
use oa_bench::truth::Truth;
use oa_replay::FeedKind;

const USAGE: &str = "usage: oa-bench --out <dir> --synthetic\n       oa-bench --extract-only [--sfdps-dir <dir>] [--tbfm-dir <dir>] [--tfms-dir <dir>]\n       oa-bench --sfdps-dir <dir> --tbfm-dir <dir> --tfms-dir <dir> --adsb <clip.oaf> --out <dir>  (needs oa-truth: not yet available)";

fn main() -> ExitCode {
    let mut out: Option<PathBuf> = None;
    let mut feeds: Vec<FeedInput> = Vec::new();
    let mut adsb: Option<PathBuf> = None;
    let (mut synthetic, mut extract_only) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| -> Option<PathBuf> {
            let v = args.next();
            if v.is_none() {
                eprintln!("oa-bench: {name} needs a value\n{USAGE}");
            }
            v.map(PathBuf::from)
        };
        match a.as_str() {
            "--synthetic" => synthetic = true,
            "--extract-only" => extract_only = true,
            "--out" => match val("--out") {
                Some(v) => out = Some(v),
                None => return ExitCode::from(2),
            },
            "--adsb" => match val("--adsb") {
                Some(v) => adsb = Some(v),
                None => return ExitCode::from(2),
            },
            "--sfdps-dir" | "--tbfm-dir" | "--tfms-dir" => {
                let feed = match a.as_str() {
                    "--sfdps-dir" => FeedKind::Sfdps,
                    "--tbfm-dir" => FeedKind::Tbfm,
                    _ => FeedKind::Tfms,
                };
                match val(&a) {
                    Some(dir) => feeds.push(FeedInput { feed, dir }),
                    None => return ExitCode::from(2),
                }
            }
            _ => {
                eprintln!("oa-bench: unknown argument {a}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if adsb.is_some() {
        eprintln!(
            "oa-bench: --adsb is not available yet: truth comes from oa-truth, which is not wired in. \
             Run with --synthetic, or --extract-only for the legacy extraction counts."
        );
        return ExitCode::from(2);
    }
    if extract_only {
        return match load(&feeds) {
            Ok(l) => {
                print!("{}", counts_text(&l));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("oa-bench: {e}");
                ExitCode::from(1)
            }
        };
    }
    if !synthetic {
        eprintln!("oa-bench: nothing to do: give --synthetic or --extract-only\n{USAGE}");
        return ExitCode::from(2);
    }
    let Some(out) = out else {
        eprintln!("oa-bench: --out is required\n{USAGE}");
        return ExitCode::from(2);
    };
    let w = build_world(&Injected::default());
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let result = run_benchmark(&w.legacy, &truth, &env(), &self_test_config());
    let checks = self_test();
    let failed = checks.iter().filter(|c| !check_ok(c)).count();
    let mut notes = vec![
        "synthetic world: truth is closed-form, the legacy stream has injected errors".to_string(),
    ];
    for c in &checks {
        notes.push(format!(
            "check {} {}: injected {:.4}, recovered {:.4}, tolerance {:.4}: {}",
            c.metric,
            c.what,
            c.injected,
            c.recovered,
            c.tolerance,
            if check_ok(c) { "ok" } else { "FAILED" }
        ));
    }
    let header = Header {
        mode: "synthetic".to_string(),
        code_version: code_version(),
        inputs: Vec::new(),
        replay_hash: "not applicable (synthetic)".to_string(),
        counts: counts_of(&w.legacy),
        notes,
    };
    if let Err(e) = fs::create_dir_all(&out)
        .and_then(|_| fs::write(out.join("report.txt"), render_text(&header, &result)))
        .and_then(|_| fs::write(out.join("report.csv"), render_csv(&header, &result)))
    {
        eprintln!("oa-bench: {}: {e}", out.display());
        return ExitCode::from(1);
    }
    println!(
        "wrote report.txt and report.csv to {}; {} checks, {failed} failed",
        out.display(),
        checks.len()
    );
    if failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn counts_of(l: &oa_bench::legacy::Legacy) -> Vec<(String, u64)> {
    oa_bench::report::extraction_counts(l)
}

fn counts_text(l: &oa_bench::input::Loaded) -> String {
    let mut s = String::new();
    for i in &l.inputs {
        s.push_str(&format!(
            "{}: {} files, {} bytes, sha256 {}\n",
            i.label, i.files, i.bytes, i.sha256
        ));
    }
    s.push_str(&format!("replay state hash: {}\n", l.replay_hash));
    for (k, v) in counts_of(&l.legacy) {
        s.push_str(&format!("{k}: {v}\n"));
    }
    for n in &l.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    s
}

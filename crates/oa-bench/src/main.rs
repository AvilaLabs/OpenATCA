#![forbid(unsafe_code)]
//! `oa-bench`: runs the M2 benchmark. Shell (B3): arguments, files, stdout.
//!
//! ```text
//! oa-bench --out <dir> --synthetic
//! oa-bench --out <dir> [--sfdps-dir <dir>] [--tbfm-dir <dir>] [--tfms-dir <dir>] --adsb <clip.oaf>
//!          [--airspace <snapshot>] [--truth-bracket-s <s>] [--truth-max-sigma-m <m>]
//!          [--truth-reason <text>] [--no-loo]
//! ```
//!
//! `--adsb` builds the truth with `oa-truth` (gates from its `TruthConfig`, the design's 10 s and
//! 50 m unless overridden; an override is recorded in the report header, with `--truth-reason`
//! when given) and scores the legacy streams against it. `--airspace` names an `oa-adapt`
//! snapshot for the fix, airport, zone and unit lookups; without it those lookups are empty and
//! the report says so. `--synthetic` runs the self-test world and writes the report. With only
//! the legacy directories (no truth), `--extract-only` prints the extraction counts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use oa_bench::adsb_truth::{self, config_with, truth_section};
use oa_bench::airspace_env::lookup_from_model;
use oa_bench::input::{load, load_adsb, FeedInput};
use oa_bench::report::{code_version, render_csv, render_text, Header};
use oa_bench::run::{run_benchmark, RunConfig, RunOutput};
use oa_bench::synth::{
    build_world, check_ok, env, self_test, self_test_config, truth_at, Injected,
};
use oa_bench::truth::{empty_env, Env, Truth};
use oa_replay::FeedKind;

const USAGE: &str = "usage: oa-bench --out <dir> --synthetic\n       oa-bench --extract-only [--sfdps-dir <dir>] [--tbfm-dir <dir>] [--tfms-dir <dir>]\n       oa-bench --out <dir> [--sfdps-dir <dir>] [--tbfm-dir <dir>] [--tfms-dir <dir>] --adsb <clip.oaf> [--airspace <snapshot>] [--truth-bracket-s <s>] [--truth-max-sigma-m <m>] [--truth-reason <text>] [--no-loo]";

/// Parses a positive finite number option value.
fn positive(name: &str, v: Option<String>) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("{name} needs a value"))?;
    match v.parse::<f64>() {
        Ok(x) if x.is_finite() && x > 0.0 => Ok(x),
        _ => Err(format!("{name} needs a positive number, got {v:?}")),
    }
}

fn main() -> ExitCode {
    let mut out: Option<PathBuf> = None;
    let mut feeds: Vec<FeedInput> = Vec::new();
    let mut adsb: Option<PathBuf> = None;
    let mut airspace: Option<PathBuf> = None;
    let (mut bracket_s, mut max_sigma_m): (Option<f64>, Option<f64>) = (None, None);
    let mut reason: Option<String> = None;
    let mut no_loo = false;
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
            "--no-loo" => no_loo = true,
            "--out" => match val("--out") {
                Some(v) => out = Some(v),
                None => return ExitCode::from(2),
            },
            "--adsb" => match val("--adsb") {
                Some(v) => adsb = Some(v),
                None => return ExitCode::from(2),
            },
            "--airspace" => match val("--airspace") {
                Some(v) => airspace = Some(v),
                None => return ExitCode::from(2),
            },
            "--truth-reason" => match val("--truth-reason") {
                Some(v) => reason = Some(v.to_string_lossy().into_owned()),
                None => return ExitCode::from(2),
            },
            "--truth-bracket-s" | "--truth-max-sigma-m" => {
                let v = args.next();
                match positive(&a, v) {
                    Ok(x) if a == "--truth-bracket-s" => bracket_s = Some(x),
                    Ok(x) => max_sigma_m = Some(x),
                    Err(e) => {
                        eprintln!("oa-bench: {e}\n{USAGE}");
                        return ExitCode::from(2);
                    }
                }
            }
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
    if let Some(clip) = adsb {
        let Some(out) = out else {
            eprintln!("oa-bench: --out is required\n{USAGE}");
            return ExitCode::from(2);
        };
        let opts = RecordedOpts {
            clip,
            airspace,
            bracket_s,
            max_sigma_m,
            reason,
            no_loo,
        };
        return match recorded(&feeds, &opts, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("oa-bench: {e}");
                ExitCode::from(1)
            }
        };
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
        truth: Vec::new(),
    };
    if let Err(e) = write_reports(&out, &header, &result) {
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

fn write_reports(out: &Path, header: &Header, result: &RunOutput) -> std::io::Result<()> {
    fs::create_dir_all(out)?;
    fs::write(out.join("report.txt"), render_text(header, result))?;
    fs::write(out.join("report.csv"), render_csv(header, result))
}

/// Options of a recorded (`--adsb`) run.
struct RecordedOpts {
    clip: PathBuf,
    airspace: Option<PathBuf>,
    bracket_s: Option<f64>,
    max_sigma_m: Option<f64>,
    reason: Option<String>,
    no_loo: bool,
}

/// Reads the recorded inputs, builds truth from the ADS-B clip, runs the benchmark and writes the
/// report.
fn recorded(feeds: &[FeedInput], o: &RecordedOpts, out: &Path) -> Result<(), String> {
    let loaded = load(feeds).map_err(|e| e.to_string())?;
    let clip = load_adsb(&o.clip).map_err(|e| e.to_string())?;
    let mut tcfg = config_with(o.bracket_s, o.max_sigma_m);
    tcfg.loo = !o.no_loo;
    let started = Instant::now();
    let built = adsb_truth::build(&clip.reports, &tcfg).map_err(|e| format!("truth: {e}"))?;
    let truth_secs = started.elapsed().as_secs_f64();
    let lookup = match &o.airspace {
        Some(p) => {
            let bytes = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let model = oa_airspace::snapshot::decode(&bytes)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            Some(lookup_from_model(&model))
        }
        None => None,
    };
    let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
    let truth = Truth {
        query: &q,
        legs: &built.legs,
    };
    let fix = |n: &str| lookup.as_ref().and_then(|l| l.fix(n));
    let airport = |n: &str| lookup.as_ref().and_then(|l| l.airport(n));
    let in_zone = |la: f64, lo: f64| lookup.as_ref().is_some_and(|l| l.in_zone(la, lo));
    let unit_at =
        |la: f64, lo: f64, _alt: Option<i32>| lookup.as_ref().and_then(|l| l.unit_at(la, lo));
    let env: Env<'_> = if lookup.is_some() {
        Env {
            fix: &fix,
            airport: &airport,
            in_zone: &in_zone,
            unit_at: &unit_at,
        }
    } else {
        empty_env()
    };
    let mut rcfg = RunConfig::default();
    // The pairing gate is the truth uncertainty gate: one value, one place in the report.
    rcfg.pair.max_sigma_m = tcfg.max_sigma95_h_m;
    let result = run_benchmark(&loaded.legacy, &truth, &env, &rcfg);

    let mut notes = loaded.notes.clone();
    // Wall-clock time goes to stderr only: the report must be the same bytes on every run.
    eprintln!("truth built in {truth_secs:.2} s");
    notes.push(format!(
        "truth: ADS-B clip, {} reports decoded, {} frames refused",
        clip.reports.icao.len(),
        clip.bad_frames
    ));
    let mut overrides = Vec::new();
    if o.bracket_s.is_some() {
        overrides.push(format!(
            "bracket {} s (design 10 s)",
            tcfg.bracket_ms as f64 / 1000.0
        ));
    }
    if o.max_sigma_m.is_some() {
        overrides.push(format!(
            "95% horizontal radius gate {} m (design 50 m), applied to truth queries and to pairing",
            tcfg.max_sigma95_h_m
        ));
    }
    if overrides.is_empty() {
        notes.push("truth gates: the design's values (10 s bracket, 50 m)".to_string());
    } else {
        notes.push(format!(
            "truth gates OVERRIDDEN: {}; reason: {}",
            overrides.join("; "),
            o.reason.as_deref().unwrap_or("not recorded")
        ));
    }
    notes.push(
        "leg callsigns are the most frequent callsign of the decoded reports of the leg (oa-truth carries none)"
            .to_string(),
    );
    match &lookup {
        Some(l) => {
            let (f, a, u) = l.counts();
            notes.push(format!(
                "airspace: {} model, {f} fixes ({} duplicate identifiers, first row used), {a} airports, {u} ARTCC polygon groups, zone {}{}; units are ARTCCs (no sector geometry in the model)",
                l.artcc,
                l.duplicate_fixes,
                l.artcc,
                if l.has_zone() { "" } else { " MISSING" }
            ));
        }
        None => notes.push(
            "airspace: no model given (--airspace); fix, airport, zone and unit lookups are empty, so T7, T8, F3, R1 and H1 are limited"
                .to_string(),
        ),
    }
    let mut inputs = loaded.inputs.clone();
    inputs.push(clip.hash.clone());
    let header = Header {
        mode: "recorded".to_string(),
        code_version: code_version(),
        inputs,
        replay_hash: loaded.replay_hash.clone(),
        counts: counts_of(&loaded.legacy),
        notes,
        truth: truth_section(&built, &tcfg),
    };
    write_reports(out, &header, &result).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "wrote report.txt and report.csv to {}; {} legs, {} associated flights, {} paired samples",
        out.display(),
        built.legs.len(),
        result
            .assoc
            .addr
            .iter()
            .filter(|&&a| a != oa_bench::legacy::NONE)
            .count(),
        result.pairs
    );
    Ok(())
}

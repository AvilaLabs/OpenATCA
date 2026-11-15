#![forbid(unsafe_code)]
//! `oa-truth-check`: builds the ADS-B truth of an OAF1 clip (docs/ADSB_PAYLOAD.md) and prints
//! what it kept, what it removed and why, the leave-one-out accuracy table and the share of
//! query times under the uncertainty gate. Shell: owns the file, the clock and stdout (B3).
//!
//! ```text
//! oa-truth-check <clip.oaf> [--accel-q <v>] [--sigma-scale <v>] [--no-loo]
//! ```

use std::fs::File;
use std::io::Read;
use std::process::ExitCode;
use std::time::Instant;

use oa_adsb::{decode_payload_lenient, reports_len, Reports};
use oa_feed::{FrameReader, Kind, MAX_FRAME_LEN};
use oa_sha256::{to_hex, Sha256};
use oa_truth::{
    build_truth, truth_hash_into, Dist, Fraction, LooStats, TruthConfig, TruthStats,
    DEFAULT_CONFIG, REMOVE_NAMES, SPLIT_NAMES,
};

fn usage() -> &'static str {
    "usage: oa-truth-check <clip.oaf> [--accel-q <v>] [--sigma-scale <v>] [--no-loo]"
}

/// Reads every ADS-B payload frame of an OAF1 file into one set of reports. Returns the reports
/// the number of frames that did not decode and the number of records dropped from frames that
/// did (a bad record does not refuse its frame).
fn read_clip(path: &str) -> Result<(Reports, u64, u64), String> {
    let mut f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut rd = FrameReader::new(&mut buf).map_err(|e| format!("{e:?}"))?;
    let mut out = Reports::default();
    let mut bad = 0u64;
    let mut dropped = 0u64;
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut chunk).map_err(|e| format!("{path}: {e}"))?;
        if n == 0 {
            break;
        }
        let mut at = 0usize;
        while at < n {
            let Some(rest) = chunk.get(at..n) else { break };
            let took = rd.push(rest);
            at += took;
            while let Some(fr) = rd.next() {
                match fr {
                    Ok(fr) if fr.kind == Kind::Message => {
                        match decode_payload_lenient(fr.payload, &mut out) {
                            Ok(c) => dropped += c.dropped as u64,
                            Err(_) => bad += 1,
                        }
                    }
                    Ok(_) => {}
                    Err(_) => bad += 1,
                }
            }
            if took == 0 {
                break;
            }
        }
    }
    Ok((out, bad, dropped))
}

fn dist_row(name: &str, unit: &str, d: &Dist) {
    println!(
        "  {name:<14} {:>9} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>10.2}  {unit}",
        d.n, d.mean, d.rms, d.median, d.p95, d.p99, d.max
    );
}

fn pct(f: &Fraction) -> String {
    if f.n == 0 {
        "n/a".to_string()
    } else {
        format!(
            "{:.2} % ({} of {})",
            100.0 * f.hit as f64 / f.n as f64,
            f.hit,
            f.n
        )
    }
}

fn print_loo(l: &LooStats) {
    println!(
        "leave-one-out: every {}-th report hidden, {} hidden, {} not predicted (no real report within the bracket on both sides)",
        l.stride, l.hidden, l.no_bracket
    );
    println!(
        "  {:<14} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>10}",
        "residual", "N", "mean", "RMS", "median", "p95", "p99", "max"
    );
    dist_row("horizontal", "m", &l.horizontal);
    dist_row("cross-track", "m", &l.cross);
    dist_row("along-track", "m", &l.along);
    dist_row("altitude", "ft", &l.alt);
    println!(
        "  (mean is signed; RMS, median, p95, p99, max are of the absolute value; nearest rank)"
    );
    println!(
        "  predicted 95 % horizontal radius within gate at hidden times: {}",
        pct(&l.under_gate)
    );
    println!(
        "  hidden report inside its predicted 95 % radius (calibration, ~95 % if honest): {}",
        pct(&l.coverage95)
    );
}

fn print_stats(s: &TruthStats, secs: f64) {
    println!("rows in            {}", s.rows_in);
    println!("aircraft in        {}", s.aircraft_in);
    println!("rows kept          {}", s.rows_kept);
    println!("aircraft with truth {}", s.aircraft_out);
    println!("legs               {}", s.legs);
    println!("leg starts by split:");
    for (n, c) in SPLIT_NAMES.iter().zip(s.splits.iter()) {
        println!("  {n:<20} {c}");
    }
    println!("rows removed, by first failing rule:");
    let mut total = 0u64;
    for (n, c) in REMOVE_NAMES.iter().zip(s.removed.iter()) {
        println!("  {n:<20} {c}");
        total += c;
    }
    println!(
        "  {:<20} {total}   (kept + removed = {}, rows in = {})",
        "total",
        s.rows_kept + total,
        s.rows_in
    );
    println!(
        "rows that passed on a carried-forward quality value: {}",
        s.quality_carried
    );
    println!(
        "95 % horizontal radius within gate, at report times:      {}",
        pct(&s.unc_at_reports)
    );
    println!(
        "95 % horizontal radius within gate, at interval midpoints: {}",
        pct(&s.unc_at_midpoints)
    );
    print_loo(&s.loo);
    println!("build time {secs:.2} s");
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg: TruthConfig = DEFAULT_CONFIG;
    let mut path: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--accel-q" => {
                cfg.accel_q = it.next().and_then(|v| v.parse().ok()).ok_or(usage())?;
            }
            "--sigma-scale" => {
                cfg.sigma_scale = it.next().and_then(|v| v.parse().ok()).ok_or(usage())?;
            }
            "--no-loo" => cfg.loo = false,
            s if s.starts_with("--") => return Err(usage().to_string()),
            s => {
                if path.replace(s.to_string()).is_some() {
                    return Err(usage().to_string());
                }
            }
        }
    }
    let path = path.ok_or(usage())?;
    let (reports, bad, dropped) = read_clip(&path)?;
    println!(
        "clip {path}: {} reports decoded, {bad} frames refused",
        reports_len(&reports)
    );
    if dropped > 0 {
        println!(
            "{dropped} records dropped from frames that decoded (position or flags out of range)"
        );
    }
    let start = Instant::now();
    let (truth, stats) = build_truth(&reports, &cfg).map_err(|e| e.to_string())?;
    let secs = start.elapsed().as_secs_f64();
    print_stats(&stats, secs);
    let mut h = Sha256::new();
    truth_hash_into(&mut h, &truth);
    println!("truth digest {}", to_hex(&h.finalize()));
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-truth-check: {e}");
            ExitCode::from(2)
        }
    }
}

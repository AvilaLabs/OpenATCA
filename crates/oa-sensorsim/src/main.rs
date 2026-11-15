#![forbid(unsafe_code)]
//! `oa-sensorsim`: simulates one radar over an OAF1 clip's ADS-B truth and writes its plots.
//! Shell: owns the file, the clock and stdout (B3).
//!
//! ```text
//! oa-sensorsim <clip.oaf> <out-dir> --site terminal|enroute|custom --profile NAME
//!     --bias-range-m V --bias-az-deg V --coverage-nm V --split-dr-m V --split-daz-deg V
//!     [--seed N] [--period-s V] [--lat V --lon V --height-m V] [--earth-k V] [--name NAME]
//! ```
//!
//! The biases, coverage and split geometry have no sourced value and so no default (docs/
//! BENCHMARKS.md 3.1 "Gaps"); every run must state them. Sites: `terminal` is the O'Hare
//! airport reference point (NASR 2026-10-29: 41.97694027 N, 87.90814972 W, 680 ft), period 4.8
//! s; `enroute` is the Peoria (PIA) airport reference point (40.66419444 N, 89.69325 W, 660.7
//! ft), period 12 s. Both are approximations of a radar's real position (an ARP is not an
//! antenna), standing in until a sited radar list is sourced. Writes `<name>.plots.csv`,
//! `<name>.cat048` and prints a summary.

use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use oa_asterix::SourceId;
use oa_sensorsim::clip::read_clip;
use oa_sensorsim::geom::{FT_M, NM_M};
use oa_sensorsim::plots::{plots_len, plots_row, to_cat048, NONE_ADDR, NONE_ALT, NONE_CODE};
use oa_sensorsim::profile::{
    profile_by_name, EARTH_K_STANDARD, MSSR_EUROCONTROL_1997, SCAN_EN_ROUTE_S, SCAN_TERMINAL_S,
};
use oa_sensorsim::radar::{simulate_radar, RadarSite};
use oa_truth::{build_truth, DEFAULT_CONFIG};

fn usage() -> &'static str {
    "usage: oa-sensorsim <clip.oaf> <out-dir> --site terminal|enroute|custom --bias-range-m V \
--bias-az-deg V --coverage-nm V --split-dr-m V --split-daz-deg V [--profile NAME] [--seed N] \
[--period-s V] [--lat V --lon V --height-m V] [--earth-k V] [--name NAME]"
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn num(args: &[String], name: &str) -> Result<Option<f64>, String> {
    match flag(args, name) {
        None => Ok(None),
        Some(v) => match v.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(Some(x)),
            _ => Err(format!("{name}: not a finite number: {v}")),
        },
    }
}

fn need(args: &[String], name: &str) -> Result<f64, String> {
    num(args, name)?.ok_or_else(|| format!("{name} is required (no sourced default)\n{}", usage()))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(clip), Some(out_dir)) = (args.first(), args.get(1)) else {
        return Err(usage().to_string());
    };
    let site_kind = flag(&args, "--site").ok_or_else(|| usage().to_string())?;
    let (lat, lon, h_ft, period, name) = match site_kind.as_str() {
        "terminal" => (
            41.976_940_27,
            -87.908_149_72,
            680.0,
            SCAN_TERMINAL_S,
            "terminal",
        ),
        "enroute" => (40.664_194_44, -89.693_25, 660.7, SCAN_EN_ROUTE_S, "enroute"),
        "custom" => (
            need(&args, "--lat")?,
            need(&args, "--lon")?,
            need(&args, "--height-m")? / FT_M,
            need(&args, "--period-s")?,
            "custom",
        ),
        _ => return Err(usage().to_string()),
    };
    let profile = match flag(&args, "--profile") {
        Some(n) => profile_by_name(&n).ok_or_else(|| format!("unknown profile {n}"))?,
        None => MSSR_EUROCONTROL_1997,
    };
    let seed: u64 = flag(&args, "--seed")
        .map(|s| s.parse().map_err(|_| "--seed: not an integer".to_string()))
        .transpose()?
        .unwrap_or(1);
    let name = flag(&args, "--name").unwrap_or_else(|| name.to_string());
    let t_total = Instant::now();
    let (reports, bad, dropped) = read_clip(clip)?;
    println!(
        "clip: {} reports ({bad} frames not decoded, {dropped} records dropped)",
        oa_adsb::reports_len(&reports)
    );
    let t0 = Instant::now();
    let (truth, ts) = build_truth(&reports, &DEFAULT_CONFIG).map_err(|e| e.to_string())?;
    println!(
        "truth: {} aircraft, {} legs, {} reports kept of {} ({:.1} s)",
        ts.aircraft_out,
        ts.legs,
        ts.rows_kept,
        ts.rows_in,
        t0.elapsed().as_secs_f64()
    );
    let first = reports.time_ns.iter().copied().min().unwrap_or(0);
    let last = reports.time_ns.iter().copied().max().unwrap_or(0);
    let site = RadarSite {
        lat_deg: lat,
        lon_deg: lon,
        height_m: h_ft * FT_M,
        rotation_period_s: num(&args, "--period-s")?.unwrap_or(period),
        initial_az_deg: 0.0,
        epoch_ns: first,
        coverage_range_m: need(&args, "--coverage-nm")? * NM_M,
        earth_k: num(&args, "--earth-k")?.unwrap_or(EARTH_K_STANDARD),
        profile,
        bias_range_m: need(&args, "--bias-range-m")?,
        bias_az_deg: need(&args, "--bias-az-deg")?,
        split_dr_m: need(&args, "--split-dr-m")?,
        split_daz_deg: need(&args, "--split-daz-deg")?,
        sac: 0,
        sic: 1,
    };
    println!("site: {site:?}");
    let t1 = Instant::now();
    let (plots, st) =
        simulate_radar(&truth, &site, seed, first, last + 1).map_err(|e| e.to_string())?;
    let sim_s = t1.elapsed().as_secs_f64();
    let bytes = to_cat048(
        &plots,
        SourceId {
            sac: site.sac,
            sic: site.sic,
        },
    )
    .map_err(|e| e.to_string())?;
    fs::create_dir_all(out_dir).map_err(|e| format!("{out_dir}: {e}"))?;
    let base = Path::new(out_dir);
    fs::write(base.join(format!("{name}.cat048")), &bytes).map_err(|e| e.to_string())?;
    let mut csv = String::from("time_ns,scan,kind,range_m,az_deg,alt_ft,mode_a,addr,truth_addr,truth_range_m,truth_az_deg\n");
    for i in 0..plots_len(&plots) {
        let Some(p) = plots_row(&plots, i) else { break };
        let opt_i = |v: i64, none: bool| if none { String::new() } else { v.to_string() };
        csv.push_str(&format!(
            "{},{},{},{:.3},{:.5},{},{},{},{},{:.3},{:.5}\n",
            p.time_ns,
            p.scan,
            p.kind,
            p.range_m,
            p.az_deg,
            opt_i(i64::from(p.alt_ft), p.alt_ft == NONE_ALT),
            if p.mode_a == NONE_CODE {
                String::new()
            } else {
                format!("{:04o}", p.mode_a)
            },
            opt_i(i64::from(p.addr), p.addr == NONE_ADDR),
            opt_i(i64::from(p.truth_addr), p.truth_addr == NONE_ADDR),
            p.truth_range_m,
            p.truth_az_deg
        ));
    }
    fs::write(base.join(format!("{name}.plots.csv")), csv).map_err(|e| e.to_string())?;
    let cov_den = (st.in_coverage + st.out_of_range + st.below_horizon).max(1);
    println!("profile {}, seed {seed}", site.profile.name);
    println!(
        "plots: {} total ({} true, {} split, {} false), {} bytes CAT048",
        plots_len(&plots),
        st.detected,
        st.split_plots,
        st.false_plots,
        bytes.len()
    );
    println!(
        "crossings {}: no truth {}, unsolved {}, out of range {}, below horizon {}, in coverage {} ({:.1}% of those with truth)",
        st.crossings, st.no_truth, st.unsolved, st.out_of_range, st.below_horizon, st.in_coverage,
        100.0 * st.in_coverage as f64 / cov_den as f64
    );
    println!(
        "detection: {} of {} in coverage ({:.2}%), missed {}",
        st.detected,
        st.in_coverage,
        100.0 * st.detected as f64 / st.in_coverage.max(1) as f64,
        st.missed
    );
    println!(
        "time: simulate {sim_s:.1} s, total {:.1} s",
        t_total.elapsed().as_secs_f64()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

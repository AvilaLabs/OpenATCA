//! `oa-wind-check adsb [data-dir] [--gap-s <seconds>]`: RAP against the wind and temperature
//! aircraft report over ADS-B (readsb's `ws`, `wd`, `oat`), for every adsb.lol day archive in
//! `raw/adsblol` whose day overlaps the RAP hours in `raw/rap`. Shell: owns files and output.

use crate::list;
use oa_adsb::{scan_archive, Filter, Reports, ScanEnd, ScanError};
use oa_wind::adsb::{adsb_obs, all_groups, compare_adsb, AdsbObs, DEFAULT_MIN_GAP_S, MAX_OBS};
use oa_wind::{archive_files, archive_sample, index_files};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The contiguous US and a margin; RAP refuses what is outside its own grid.
const BOX_E7: (i32, i32, i32, i32) = (150_000_000, 600_000_000, -1_400_000_000, -500_000_000);
const KT_PER_MS: f64 = 3600.0 / 1852.0;

/// Unix day number (days since 1970-01-01) of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The day of an archive part named `vYYYY.MM.DD-...`, as Unix seconds at its start.
fn day_start(name: &str) -> Option<i64> {
    let rest = name.strip_prefix('v')?;
    let mut it = rest.get(..10)?.split('.');
    let (y, m, d) = (
        it.next()?.parse::<i64>().ok()?,
        it.next()?.parse::<i64>().ok()?,
        it.next()?.parse::<i64>().ok()?,
    );
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d) * 86_400)
}

/// Archive parts under `dir` grouped by day archive (name up to `.tar`), each group's parts in
/// name order (`.aa`, `.ab`, ...).
fn day_groups(dir: &Path) -> Vec<(String, Vec<PathBuf>)> {
    let mut groups: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for p in list(dir, "v", "") {
        let Some(name) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        let Some(at) = name.find(".tar") else {
            continue;
        };
        let key = name.get(..at).unwrap_or("").to_string();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(p),
            None => groups.push((key, vec![p])),
        }
    }
    groups
}

fn fmt_group(g: &oa_wind::adsb::GroupStats) -> String {
    let wind = match &g.wind {
        Some(w) => format!(
            "wind N {} median {:.1} p95 {:.1} m/s ({:.1} / {:.1} kt), speed bias {:+.1} m/s ({:+.1} kt), u {:+.1} v {:+.1} m/s",
            w.n, w.median, w.p95, w.median * KT_PER_MS, w.p95 * KT_PER_MS,
            g.speed_bias, g.speed_bias * KT_PER_MS, g.bias_u, g.bias_v
        ),
        None => "wind N 0".to_string(),
    };
    let temp = match &g.temp_abs {
        Some(t) => format!(
            "; T N {} bias {:+.1} K median |dT| {:.1} p95 {:.1} K",
            t.n, g.temp_bias, t.median, t.p95
        ),
        None => "; T N 0".to_string(),
    };
    format!("  {:<20} {wind}{temp}", g.name)
}

pub fn run(root: &Path, args: &[String]) -> ExitCode {
    let mut gap = DEFAULT_MIN_GAP_S;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--gap-s" {
            match it.next().and_then(|v| v.parse::<f64>().ok()) {
                Some(g) if g.is_finite() && g >= 0.0 => gap = g,
                _ => {
                    eprintln!("oa-wind-check adsb: --gap-s needs a number of seconds, 0 or more");
                    return ExitCode::from(2);
                }
            }
        }
    }
    let raw = root.join("raw");
    let rap = list(&raw.join("rap"), "rap.", ".grib2");
    if rap.is_empty() {
        println!(
            "oa-wind-check adsb: no RAP files under {}; skipped",
            raw.display()
        );
        return ExitCode::SUCCESS;
    }
    let mut archive = match index_files(&rap).and_then(archive_files) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("oa-wind-check adsb: {e}");
            return ExitCode::FAILURE;
        }
    };
    let times = archive.times();
    let (Some(&first), Some(&last)) = (times.first(), times.last()) else {
        return ExitCode::SUCCESS;
    };
    println!(
        "RAP: {} hours valid {first}..{last} (Unix s); ADS-B one observation per aircraft per {gap} s",
        times.len()
    );
    let filter = Filter {
        lat_min_e7: BOX_E7.0,
        lat_max_e7: BOX_E7.1,
        lon_min_e7: BOX_E7.2,
        lon_max_e7: BOX_E7.3,
        from_ns: first * 1_000_000_000,
        to_ns: (last + 1) * 1_000_000_000,
    };
    let mut obs: Vec<AdsbObs> = Vec::new();
    let mut dropped = 0usize;
    let mut scanned = 0usize;
    for (key, parts) in day_groups(&raw.join("adsblol")) {
        let Some(d0) = day_start(&key) else {
            println!("  {key}: day not readable from the name; skipped");
            continue;
        };
        if d0 > last || d0 + 86_400 <= first {
            println!(
                "  {key}: day {d0}..{} does not overlap the RAP hours; not scanned",
                d0 + 86_400
            );
            continue;
        }
        let files: Result<Vec<File>, _> = parts.iter().map(File::open).collect();
        let files = match files {
            Ok(f) => f,
            Err(e) => {
                eprintln!("oa-wind-check adsb: {key}: {e}");
                return ExitCode::FAILURE;
            }
        };
        scanned += 1;
        let res = scan_archive(files, &filter, &mut |r: &Reports| -> Result<(), String> {
            dropped += adsb_obs(r, gap, &mut obs);
            Ok(())
        });
        match res {
            Ok(r) => println!(
                "  {key}: {} traces read, {} refused, {} rows kept in box and window{}",
                r.stats.traces_read,
                r.stats.traces_refused,
                r.stats.kept,
                match r.end {
                    ScanEnd::Complete => "",
                    ScanEnd::Truncated(_) => " (archive ended early)",
                }
            ),
            Err(ScanError::Tar(e)) => {
                eprintln!("oa-wind-check adsb: {key}: {e}");
                return ExitCode::FAILURE;
            }
            Err(ScanError::Sink(e)) => {
                eprintln!("oa-wind-check adsb: {key}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if scanned == 0 || obs.is_empty() {
        println!(
            "ADS-B vs RAP: no adsb.lol day overlaps the RAP hours (or none has a wind or temperature in them); nothing to compare. The comparison runs when an archive for a day the RAP files cover is in raw/adsblol."
        );
        return ExitCode::SUCCESS;
    }
    if dropped > 0 {
        println!("  {dropped} observations over the limit of {MAX_OBS} were not kept");
    }
    obs.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    let with_wind = obs.iter().filter(|o| o.wind.is_some()).count();
    let with_oat = obs.iter().filter(|o| o.oat_c.is_some()).count();
    println!(
        "ADS-B observations {} ({with_wind} with wind, {with_oat} with oat)",
        obs.len()
    );
    let c = match compare_adsb(&obs, &mut |q| archive_sample(&mut archive, q)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("oa-wind-check adsb: load failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "compared {} of {} (RAP minus readsb); refused by model: {}",
        c.rows.len(),
        c.offered,
        if c.refused.is_empty() {
            "none".to_string()
        } else {
            c.refused
                .iter()
                .map(|(n, k)| format!("{n} {k}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    for g in all_groups(&c) {
        println!("{}", fmt_group(&g));
    }
    println!(
        "  note: the turning split uses roll and track rate with readsb's own gates; readsb withholds a new wind while they are over the gate but may still print an older one for up to 60 s"
    );
    ExitCode::SUCCESS
}

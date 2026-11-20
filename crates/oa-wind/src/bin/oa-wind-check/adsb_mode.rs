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

/// Days in month `m` (1..=12) of year `y`; 0 for any other month.
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`), for years 1970
/// to 9999 and dates that exist; `None` for any other year, month or day.
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
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
    days_from_civil(y, m, d)?.checked_mul(86_400)
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
        let files: Result<Vec<File>, _> = parts.iter().map(|p| oa_sealed::open(p)).collect();
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
    obs.sort_by(|a, b| oa_math::cmp_canon_f64(a.time_s, b.time_s));
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod calendar_tests {
    use super::*;

    // (year, month, day, Unix day or None): the same table is applied in every crate that has
    // its own calendar helper.
    const VECTORS: [(i64, i64, i64, Option<i64>); 20] = [
        (1970, 1, 1, Some(0)),
        (2000, 2, 29, Some(11_016)),
        (2024, 2, 29, Some(19_782)),
        (2026, 11, 15, Some(20_772)),
        (9999, 12, 31, Some(2_932_896)),
        (2023, 2, 29, None),
        (2100, 2, 29, None),
        (2026, 4, 31, None),
        (2026, 6, 31, None),
        (2026, 9, 31, None),
        (2026, 1, 32, None),
        (2026, 1, 0, None),
        (2026, 0, 1, None),
        (2026, 13, 1, None),
        (-1, 1, 1, None),
        (1969, 12, 31, None),
        (10_000, 1, 1, None),
        (i64::MAX, 1, 1, None),
        (i64::MIN, 1, 1, None),
        (2026, i64::MAX, 1, None),
    ];

    // REQ: OA-WIND-058
    #[test]
    fn impossible_dates_and_years_out_of_range_are_refused() {
        for (y, m, d, want) in VECTORS {
            assert_eq!(days_from_civil(y, m, d), want, "{y}-{m}-{d}");
        }
        assert_eq!(day_start("v2026.11.15-x"), Some(20_772 * 86_400));
        assert_eq!(day_start("v2026.02.29-x"), None);
        assert_eq!(day_start("v2026.04.31-x"), None);
        assert_eq!(day_start("v1969.12.31-x"), None);
        assert_eq!(day_start("v9999.99.99-x"), None);
    }
}

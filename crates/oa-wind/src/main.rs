#![forbid(unsafe_code)]
//! `oa-wind-check [data-dir]`: validates `oa-wind` on the real 2026-11-14 files and prints the
//! summary (default `$OPENATCA_DATA`). Each check is skipped with a note when its data are
//! absent. Shell: owns the files, the environment and standard output (B3).
//!
//! 1. The model against `oa-grib2`'s own decoded values at grid points (exact).
//! 2. RAP against HRRR at the same time and level (t12z, 250 hPa).
//! 3. RAP against pilot-reported winds (AWC PIREPs with wind direction, speed and level).
//!
//! `oa-wind-check adsb [data-dir] [--gap-s <seconds>]` is a separate mode: RAP against the wind
//! and temperature aircraft report over ADS-B (adsb.lol archives, readsb `ws`/`wd`/`oat`).

mod adsb_mode;

use oa_awc::{finalize_pirep, new_pirep_table, parse_pirep, pirep_len, PirepStats, PirepTable};
use oa_grib2::{decode_stream, Field, Parameter};
use oa_wind::check::{check_grid_points, compare_obs, compare_to_fields, ObsWind, VectorCompare};
use oa_wind::{
    archive_files, hour_bytes, hour_levels, index_files, read_hour_file, Hour, MAX_HOURS_HELD,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

fn list(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

fn fmt_summary(c: &VectorCompare) -> String {
    match &c.diff {
        Some(s) => format!(
            "N {} median {:.2} p95 {:.2} mean {:.2} max {:.2} m/s; bias (model-ref) u {:+.2} v {:+.2}",
            s.n, s.median, s.p95, s.mean, s.max, c.bias_u, c.bias_v
        ),
        None => "N 0".to_string(),
    }
}

fn refusals(c: &VectorCompare) -> String {
    if c.refused.is_empty() {
        return "none".to_string();
    }
    c.refused
        .iter()
        .map(|(n, k)| format!("{n} {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn load_pireps(dir: &Path) -> Result<PirepTable, String> {
    let mut t = new_pirep_table();
    let mut stats = PirepStats::default();
    for p in list(dir, "pirep_", ".json") {
        let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        parse_pirep(&bytes, &mut t, &mut stats).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let d = finalize_pirep(&mut t);
    println!(
        "  PIREP files {} records {} accepted {} kept after de-dup {}",
        stats.files, stats.records, stats.accepted, d.rows_kept
    );
    Ok(t)
}

fn wind_obs(t: &PirepTable) -> Vec<ObsWind> {
    let none = u16::MAX;
    let mut v = Vec::new();
    for i in 0..pirep_len(t) {
        let (Some(&dir), Some(&spd), Some(&fl)) =
            (t.wdir_deg.get(i), t.wspd_kt.get(i), t.fl.get(i))
        else {
            continue;
        };
        if dir == none || spd == none || fl == none {
            continue;
        }
        let (Some(&obs), Some(&la), Some(&lo)) = (t.obs_s.get(i), t.lat_e5.get(i), t.lon_e5.get(i))
        else {
            continue;
        };
        v.push(ObsWind {
            lat: f64::from(la) * 1e-5,
            lon: f64::from(lo) * 1e-5,
            alt_ft: f64::from(fl) * 100.0,
            time_s: obs as f64,
            wdir_deg: f64::from(dir),
            wspd_kt: f64::from(spd),
        });
    }
    v.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    v
}

fn field(fields: &[Field], p: Parameter, pa: f32) -> Option<&Field> {
    fields.iter().find(|f| {
        f.parameter == p && f.level.kind == oa_grib2::LEVEL_ISOBARIC && f.level.value == pa
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let adsb = args.get(1).is_some_and(|a| a == "adsb");
    let first = if adsb { 2 } else { 1 };
    let root = match args.get(first).filter(|a| !a.starts_with("--")) {
        Some(d) => PathBuf::from(d),
        None => match std::env::var_os("OPENATCA_DATA") {
            Some(d) => PathBuf::from(d),
            None => {
                eprintln!("usage: oa-wind-check [adsb] [data-dir]   (or set OPENATCA_DATA)");
                return ExitCode::from(2);
            }
        },
    };
    if adsb {
        let rest = args.get(first..).unwrap_or(&[]);
        let rest = if rest.first().is_some_and(|a| !a.starts_with("--")) {
            rest.get(1..).unwrap_or(&[])
        } else {
            rest
        };
        return adsb_mode::run(&root, rest);
    }
    let raw = root.join("raw");
    let rap = list(&raw.join("rap"), "rap.", ".grib2");
    if rap.is_empty() {
        println!(
            "oa-wind-check: no RAP files under {}; skipped",
            raw.display()
        );
        return ExitCode::SUCCESS;
    }
    let files = match index_files(&rap) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("oa-wind-check: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut archive = match archive_files(files) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("oa-wind-check: {e}");
            return ExitCode::FAILURE;
        }
    };
    let times = archive.times();
    println!(
        "RAP: {} hours, valid {}..{} (Unix s), cache holds at most {MAX_HOURS_HELD} hours",
        times.len(),
        times.first().copied().unwrap_or(0),
        times.last().copied().unwrap_or(0)
    );
    let mut ok = true;

    // Pick t12z when present, else the middle hour.
    let pick: Option<PathBuf> = archive
        .files()
        .iter()
        .find(|f| f.path.to_string_lossy().contains(".t12z."))
        .or_else(|| archive.files().get(archive.files().len() / 2))
        .map(|f| f.path.clone());
    let Some(pick) = pick else {
        return ExitCode::SUCCESS;
    };
    let t0 = Instant::now();
    let hour: Hour = match read_hour_file(&pick) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("oa-wind-check: {}: {e}", pick.display());
            return ExitCode::FAILURE;
        }
    };
    println!(
        "  hour {} loaded in {:.2} s: {} levels, grid {} x {}, {:.1} MB of arrays per hour",
        hour.valid_s,
        t0.elapsed().as_secs_f64(),
        hour_levels(&hour),
        hour.nx,
        hour.ny,
        hour_bytes(&hour) as f64 / 1e6
    );

    // 1. against oa-grib2's own values
    println!("(1) against oa-grib2 decoded values, {}", pick.display());
    match std::fs::read(&pick) {
        Ok(data) => {
            let dec = decode_stream(&data);
            let r = check_grid_points(&hour, &dec.fields, 4);
            println!(
                "  grid points compared {} (every 4th column/row, every level)",
                r.n
            );
            println!(
                "  stored arrays vs decoded values: max abs diff {:e} (exact when 0)",
                r.raw_max
            );
            println!(
                "  query at grid point vs oa-grib2 wind_at/bilinear: wind {:.2e} m/s, T {:.2e} K, height {:.2e} gpm",
                r.wind_max, r.temp_max, r.height_max
            );
            if r.raw_max != 0.0 || r.wind_max > 1e-3 || r.temp_max > 1e-3 || r.height_max > 1e-2 {
                println!("  FAIL");
                ok = false;
            } else {
                println!("  PASS");
            }
            // 2. HRRR
            let hrrr = list(&raw.join("hrrr"), "hrrr.", ".grib2");
            match hrrr.first() {
                None => println!("(2) RAP vs HRRR: no HRRR file; skipped"),
                Some(hp) => {
                    println!("(2) RAP vs HRRR, {}", hp.display());
                    match std::fs::read(hp) {
                        Ok(hd) => {
                            let hf = decode_stream(&hd);
                            let pa = hf
                                .fields
                                .iter()
                                .find(|f| f.parameter == Parameter::Ugrd)
                                .map(|f| f.level.value);
                            let (u, v) = match pa {
                                Some(pa) => (
                                    field(&hf.fields, Parameter::Ugrd, pa),
                                    field(&hf.fields, Parameter::Vgrd, pa),
                                ),
                                None => (None, None),
                            };
                            if let (Some(u), Some(v)) = (u, v) {
                                let c = compare_to_fields(&hour, u, v, 20);
                                println!(
                                    "  level {} hPa, every 20th HRRR point, RAP hour {} vs HRRR",
                                    u.level.value / 100.0,
                                    hour.valid_s
                                );
                                println!("  rotated   : {}", fmt_summary(&c));
                                if let Some(s) = &c.diff_unrotated {
                                    println!(
                                        "  unrotated : N {} median {:.2} p95 {:.2} m/s (grid-relative components, for comparison)",
                                        s.n, s.median, s.p95
                                    );
                                }
                                if let Some(s) = &c.speed_diff {
                                    println!(
                                        "  speed |RAP-HRRR|: median {:.2} p95 {:.2} m/s",
                                        s.median, s.p95
                                    );
                                }
                                println!("  refused by model: {}", refusals(&c));
                            } else {
                                println!("  HRRR file has no UGRD/VGRD pair; skipped");
                            }
                        }
                        Err(e) => println!("  cannot read {}: {e}", hp.display()),
                    }
                }
            }
        }
        Err(e) => println!("  cannot read {}: {e}", pick.display()),
    }

    // 3. PIREPs
    let awc = raw.join("awc").join("20261114");
    if list(&awc, "pirep_", ".json").is_empty() {
        println!("(3) RAP vs PIREP winds: no PIREP files; skipped");
    } else {
        println!("(3) RAP vs PIREP winds, {}", awc.display());
        match load_pireps(&awc) {
            Ok(t) => {
                let all = wind_obs(&t);
                println!("  reports with direction, speed and level: {}", all.len());
                let hi: Vec<ObsWind> = all
                    .iter()
                    .copied()
                    .filter(|o| o.alt_ft >= 18000.0)
                    .collect();
                let lo: Vec<ObsWind> = all.iter().copied().filter(|o| o.alt_ft < 18000.0).collect();
                if all.len() <= 20 {
                    for o in &all {
                        let q = oa_wind::Query {
                            lat: o.lat,
                            lon: o.lon,
                            alt_ft: o.alt_ft,
                            time_s: o.time_s,
                        };
                        let (ou, ov) = oa_wind::check::obs_components(o.wdir_deg, o.wspd_kt);
                        match oa_wind::archive_sample(&mut archive, &q) {
                            Ok(s) => println!(
                                "  report {:.2},{:.2} FL{:.0} t {:.0}: from {:.0}/{:.0} kt = ({ou:.1},{ov:.1}) m/s; model ({:.1},{:.1}) m/s T {:.1} K, hours {} {} w {:.2}",
                                o.lat, o.lon, o.alt_ft / 100.0, o.time_s, o.wdir_deg, o.wspd_kt,
                                s.u_ms, s.v_ms, s.temp_k, s.hour0_s, s.hour1_s, s.time_weight
                            ),
                            Err(e) => println!("  report {:.2},{:.2}: {e}", o.lat, o.lon),
                        }
                    }
                }
                for (name, set) in [
                    ("all levels", &all),
                    ("FL180 and above", &hi),
                    ("below FL180", &lo),
                ] {
                    match compare_obs(&mut archive, set) {
                        Ok(c) => {
                            println!("  {name}: {}", fmt_summary(&c));
                            if let Some(s) = &c.speed_diff {
                                println!(
                                    "    speed |model-report|: median {:.2} p95 {:.2} m/s",
                                    s.median, s.p95
                                );
                            }
                            println!("    refused by model: {}", refusals(&c));
                        }
                        Err(e) => {
                            println!("  {name}: load failed: {e}");
                            ok = false;
                        }
                    }
                }
                println!(
                    "  note: FL is pressure altitude at and above FL180; below it the report gives an indicated altitude, so the low set carries that error too"
                );
            }
            Err(e) => {
                println!("  {e}");
                ok = false;
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

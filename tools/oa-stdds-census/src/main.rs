#![forbid(unsafe_code)]
//! `oa-stdds-census DIR [--adsb CLIP.oaf [--gate-nm N] [--gate-ft N]]` prints the census of the
//! recordings in DIR, and with an ADS-B clip the position cross-check. Exit 0 on success, 1 on a
//! failed run, 2 on a usage error, a bad gate or window, a memory estimate past the budget, a sealed
//! path (given, met in the walk, or found when a file is opened or the clip is loaded) or a directory
//! that is a `rec` folder or a capture day without `rec/stdds`, 3 if the frame reader stalled or a
//! gzip file was damaged (the census is incomplete, printed all the same).

use oa_sealed::Sealed;
use oa_stdds_census::census::Census;
use oa_stdds_census::census::DEFAULT_BUDGET;

/// The memory budget: 4 GB, or the value of `OA_STDDS_CENSUS_BUDGET_BYTES` (for tests only).
fn budget() -> u64 {
    std::env::var("OA_STDDS_CENSUS_BUDGET_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_BUDGET)
}
use oa_stdds_census::guard::{is_refusal, refuse_sealed};
use oa_stdds_census::read::{read_dir_counted, ReadError};
use oa_stdds_census::report::render;
use oa_stdds_census::xcheck::{build, estimate_build_bytes, Params, Window};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: oa-stdds-census DIR [--from S --to S] [--adsb CLIP.oaf [--gate-nm N] [--gate-ft N] [--max-gap-s S]]\n  with --adsb the census also compares the updates' positions with ADS-B; the horizontal gate N\n  (default 1 NM) is tried with 0.5 and 2 as well, the vertical gate defaults to 500 ft; --max-gap-s (default 5) is the widest gap between two ADS-B reports that is interpolated;\n  --from/--to restrict both sides to a time of the UTC day, in seconds; the run is refused\n  if its memory estimate is above 4 GB";

/// Whether `g` is a whole number of seconds whose milliseconds fit in an `i64`.
fn whole_seconds(g: f64) -> bool {
    g.fract() == 0.0 && g.abs() <= (i64::MAX / 1000) as f64 - 1.0
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    // Printed when `main` returns: on every exit, an error or refusal included.
    let mut drops = oa_sealed::DropReport::new();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((dir, rest)) = argv.split_first() else {
        return usage();
    };
    let mut clip: Option<&String> = None;
    let mut params = Params::default();
    let (mut from, mut to) = (None, None);
    let mut gate_given = false;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        // Only finite numbers are taken (NaN and infinity are a usage error).
        let num = |v: Option<&String>| {
            v.and_then(|v| v.parse::<f64>().ok())
                .filter(|x| x.is_finite())
        };
        match flag.as_str() {
            "--adsb" => match it.next() {
                Some(v) => clip = Some(v),
                None => return usage(),
            },
            "--gate-nm" => match num(it.next()) {
                Some(g) => {
                    params = params.with_primary_gate_nm(g);
                    gate_given = true;
                }
                None => return usage(),
            },
            "--gate-ft" => match num(it.next()) {
                Some(g) => {
                    params.v_gate_ft = g;
                    gate_given = true;
                }
                None => return usage(),
            },
            "--max-gap-s" => match num(it.next()) {
                Some(g) if g > 0.0 && g <= 30.0 => {
                    params.max_gap_ms = (g * 1000.0).round() as i64;
                }
                _ => return usage(),
            },
            // A window bound is whole seconds that fit in milliseconds as an i64; a fractional or
            // larger one is a usage error, not truncated or saturated.
            "--from" => match num(it.next()).filter(|g| whole_seconds(*g)) {
                Some(g) => from = Some(g as i64),
                None => return usage(),
            },
            "--to" => match num(it.next()).filter(|g| whole_seconds(*g)) {
                Some(g) => to = Some(g as i64),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    if gate_given && clip.is_none() {
        eprintln!("oa-stdds-census: --gate-nm and --gate-ft need --adsb");
        return ExitCode::from(2);
    }
    let window = match (from, to) {
        (None, None) => None,
        (Some(f), Some(t)) => match Window::new(f, t) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("oa-stdds-census: {e}");
                return ExitCode::from(2);
            }
        },
        _ => return usage(),
    };
    for p in std::iter::once(dir).chain(clip) {
        if let Err(e) = refuse_sealed(Sealed::production(), Path::new(p)) {
            eprintln!("oa-stdds-census: refused: {e}");
            return ExitCode::from(2);
        }
    }
    let t0 = std::time::Instant::now();
    let mut clip_sealed = 0u64;
    let index = match clip {
        Some(c) => {
            // The loader is the one oa-encounter and the bench use.
            let loaded = match oa_bench::input::load_adsb(Path::new(c)) {
                Ok(l) => l,
                Err(e) => {
                    if is_refusal(&e.to_string()) {
                        eprintln!("oa-stdds-census: refused: {e}");
                        return ExitCode::from(2);
                    }
                    eprintln!("oa-stdds-census: cannot read the ADS-B clip: {e}");
                    return ExitCode::from(1);
                }
            };
            eprintln!(
                "ADS-B clip: bad frames {} dropped records {}",
                loaded.bad_frames, loaded.dropped_records
            );
            clip_sealed = loaded.sealed_window_dropped;
            drops.add(clip_sealed);
            let est = estimate_build_bytes(&loaded.reports, &params, window);
            if est > budget() {
                eprintln!(
                    "oa-stdds-census: refused: building the ADS-B index is estimated at {} MB from {} reports (an estimate, not a bound), past the 4 GB budget (use --from and --to)",
                    est / 1_000_000,
                    loaded.reports.icao.len()
                );
                return ExitCode::from(2);
            }
            match build(&loaded.reports, params, window) {
                Ok(i) if i.bytes() > budget() => {
                    eprintln!(
                        "oa-stdds-census: refused: the ADS-B index alone is estimated at {} MB (an estimate, not a bound), past the 4 GB budget (use --from and --to)",
                        i.bytes() / 1_000_000
                    );
                    return ExitCode::from(2);
                }
                Ok(i) => Some(i),
                Err(e) => {
                    eprintln!("oa-stdds-census: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        None => None,
    };
    let mut census = index
        .map_or_else(Census::new, Census::with_xcheck)
        .with_window(window)
        .with_budget(budget());
    let mut read_dropped = 0u64;
    let read = read_dir_counted(
        Sealed::production(),
        Path::new(dir),
        &mut census,
        &mut read_dropped,
    );
    drops.add(read_dropped);
    let rs = match read {
        Ok(mut rs) => {
            // The clip's rows and the recording's frames and records, in one count.
            rs.sealed_window_dropped += clip_sealed;
            rs
        }
        Err(e @ (ReadError::Refused(_) | ReadError::Layout(_))) => {
            eprintln!("oa-stdds-census: refused: {e}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("oa-stdds-census: {e}");
            return ExitCode::from(1);
        }
    };
    let results = census.finish();
    if let Some(x) = &results.xc {
        if let Err(e) = x.check_overlap() {
            eprintln!("oa-stdds-census: {e}");
            return ExitCode::from(2);
        }
    }
    print!("{}", render(&rs, &results));
    eprintln!("run time {:.1} s", t0.elapsed().as_secs_f64());
    if rs.stalled_bytes > 0 {
        eprintln!("oa-stdds-census: the frame reader stalled; the census is incomplete");
        return ExitCode::from(3);
    }
    if rs.files_gzip_damaged > 0 {
        eprintln!(
            "oa-stdds-census: {} gzip file(s) damaged; the census is incomplete",
            rs.files_gzip_damaged
        );
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

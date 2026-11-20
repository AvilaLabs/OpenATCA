#![forbid(unsafe_code)]
//! `oa-stdds-census DIR [--adsb CLIP.oaf [--gate-nm N] [--gate-ft N]]` prints the census of the
//! recordings in DIR, and with an ADS-B clip the position cross-check. Exit 0 on success, 1 on a
//! failed run, 2 on a usage error, a sealed path, a bad gate or window, or a memory estimate past the budget, 3 if the frame reader stalled
//! (the census is incomplete).

use oa_stdds_census::census::Census;
use oa_stdds_census::census::DEFAULT_BUDGET;
use oa_stdds_census::guard::refuse_sealed;
use oa_stdds_census::read::{read_dir, ReadError};
use oa_stdds_census::report::render;
use oa_stdds_census::xcheck::{build, Params, Window};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: oa-stdds-census DIR [--from S --to S] [--adsb CLIP.oaf [--gate-nm N] [--gate-ft N]]\n  with --adsb the census also compares the updates' positions with ADS-B; the horizontal gate N\n  (default 1 NM) is tried with 0.5 and 2 as well, the vertical gate defaults to 500 ft;\n  --from/--to restrict both sides to a time of the UTC day, in seconds; the run is refused\n  if its memory estimate is above 4 GB";

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
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
        let num = |v: Option<&String>| v.and_then(|v| v.parse::<f64>().ok());
        match flag.as_str() {
            "--adsb" => match it.next() {
                Some(v) => clip = Some(v),
                None => return usage(),
            },
            "--gate-nm" => match num(it.next()) {
                Some(g) => {
                    params.gates_nm = [0.5, g, 2.0];
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
            "--from" => match num(it.next()) {
                Some(g) => from = Some(g as i64),
                None => return usage(),
            },
            "--to" => match num(it.next()) {
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
        if let Err(e) = refuse_sealed(Path::new(p)) {
            eprintln!("oa-stdds-census: refused: {e}");
            return ExitCode::from(2);
        }
    }
    let t0 = std::time::Instant::now();
    let index = match clip {
        Some(c) => {
            // The loader is the one oa-encounter and the bench use.
            let loaded = match oa_bench::input::load_adsb(Path::new(c)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("oa-stdds-census: cannot read the ADS-B clip: {e}");
                    return ExitCode::from(1);
                }
            };
            eprintln!(
                "ADS-B clip: bad frames {} dropped records {}",
                loaded.bad_frames, loaded.dropped_records
            );
            match build(&loaded.reports, params, window) {
                Ok(i) if i.bytes() > DEFAULT_BUDGET => {
                    eprintln!(
                        "oa-stdds-census: refused: the ADS-B index alone is estimated at {} MB, past the 4 GB budget (use --from and --to)",
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
        .with_window(window);
    let rs = match read_dir(Path::new(dir), &mut census) {
        Ok(rs) => rs,
        Err(e @ ReadError::Refused(_)) => {
            eprintln!("oa-stdds-census: refused: {e}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("oa-stdds-census: {e}");
            return ExitCode::from(1);
        }
    };
    let results = census.finish();
    print!("{}", render(&rs, &results));
    eprintln!("run time {:.1} s", t0.elapsed().as_secs_f64());
    if rs.stalled_bytes > 0 {
        eprintln!("oa-stdds-census: the frame reader stalled; the census is incomplete");
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

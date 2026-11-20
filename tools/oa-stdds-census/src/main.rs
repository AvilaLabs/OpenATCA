#![forbid(unsafe_code)]
//! `oa-stdds-census DIR` prints the census of the recordings in DIR. Exit 0 on success, 1 on
//! a failed run, 2 on a usage error, a sealed path or the unfinished second mode, 3 if the
//! frame reader stalled (the census is incomplete).

use oa_stdds_census::census::Census;
use oa_stdds_census::guard::refuse_sealed;
use oa_stdds_census::read::read_dir;
use oa_stdds_census::report::render;
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: oa-stdds-census DIR\n       oa-stdds-census DIR --crosscheck-adsb CLIP   (not implemented yet)";

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (dir, cross) = match argv.as_slice() {
        [d] => (d, None),
        [d, flag, clip] if flag == "--crosscheck-adsb" => (d, Some(clip)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    for p in std::iter::once(dir).chain(cross) {
        if let Err(e) = refuse_sealed(Path::new(p)) {
            eprintln!("oa-stdds-census: refused: {e}");
            return ExitCode::from(2);
        }
    }
    if cross.is_some() {
        // TODO: second mode. No ADS-B clip for 2026-11-20 exists yet; see crosscheck.rs.
        eprintln!("oa-stdds-census: the ADS-B cross-check is a stub (TODO: needs an ADS-B clip of the same day)");
        return ExitCode::from(2);
    }
    let t0 = std::time::Instant::now();
    let mut census = Census::new();
    let rs = match read_dir(Path::new(dir), &mut census) {
        Ok(rs) => rs,
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

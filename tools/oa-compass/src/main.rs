#![forbid(unsafe_code)]
//! `oa-compass`: writes one UTC day of OpenATCA tracker output and ERAM tracks as ASTERIX CAT062
//! Ed 1.21, and the ADS-B reports offered to the tracker's adapter (a superset of those it used) as CAT021 Ed 2.4, for OpenATS COMPASS. Shell (B3): arguments, files, the clock and stdout.
//!
//! ```text
//! oa-compass --date <YYYY.MM.DD> --out <dir> [--adsb <clip.oaf>] [--sfdps-dir <dir>] [--split-days]
//!             [--inputs split|all] [--openatca-rows ticks|updates]
//!             [--eram-contradicted-address keep|drop]
//! ```
//!
//! Writes `openatca.ast` (from `--adsb`: the tracker is run on the clip as the scorer runs it) and
//! `adsb.ast` (the ADS-B reports offered to the tracker's adapter (a superset of those it used), deduplicated by the scorer's own code),
//! `eram.ast` (from `--sfdps-dir`: the ERAM tracks the scorer reads), `data_sources.json` and
//! `README.txt` in `<dir>`. A file holds one UTC day: rows on another day than `--date` refuse the
//! export unless `--split-days` is given, which writes one directory per day. The output
//! directory must be new or empty. `--inputs all` (the default) offers the tracker's adapter every
//! ADS-B report of the clip, like ERAM, `split` offers it set A of the scorer's split only.
//! `--openatca-rows ticks` (the default) writes the tracker's one-second tick rows, `updates` its
//! update rows (one per applied report, at the report's time). Only development days may be exported:
//! any input path the shared sealed list refuses, as given or after resolving links, is refused.
//! `--eram-contradicted-address keep` (the default) writes every ERAM address as published and
//! only reports the flights whose track contradicts theirs; `drop` writes those flights without
//! the address, and the README says the file was altered.

use std::process::ExitCode;

use oa_compass::shell::args::{parse_export, parse_slice, SLICE_USAGE, USAGE};
use oa_compass::shell::error::ExportError;
use oa_compass::shell::export::Layout;
use oa_compass::shell::job::{run_job_tallied, EramSource, Job};
use oa_compass::shell::slice;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("slice") {
        return slice_main(argv.get(1..).unwrap_or(&[]));
    }
    let a = match parse_export(&argv) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("oa-compass: {m}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let job = Job {
        layout: Layout {
            out: a.out,
            day: a.day,
            split: a.split,
        },
        adsb: a.adsb,
        sfdps: a.sfdps.map(EramSource::Dir),
        mode: a.mode,
        rows: a.rows,
        eram_address: a.eram_address,
        self_test_eram_offset_e7: 0,
        self_test_adsb_offset_e7: 0,
    };
    match run(job) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-compass: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(job: Job) -> Result<(), ExportError> {
    // The clip's count and the ERAM loader's, printed (on standard output, as before) when this
    // function returns, an error included.
    let mut drops = oa_sealed::DropReport::to_stdout();
    let mut sealed = 0u64;
    let done = run_job_tallied(job, &mut |_| Ok(()), &mut |l| eprintln!("{l}"), &mut sealed);
    drops.add(sealed);
    for l in done? {
        println!("{l}");
    }
    Ok(())
}

/// `oa-compass slice`: a time window of the `.ast` files of a finished export.
fn slice_main(args: &[String]) -> ExitCode {
    let a = match parse_slice(args) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("oa-compass slice: {m}\n{SLICE_USAGE}");
            return ExitCode::from(2);
        }
    };
    let (inp, out, from, to) = (a.input, a.out, a.from, a.to);
    match slice::slice_dir(&inp, &out, from, to) {
        Ok(files) => {
            for f in &files {
                println!(
                    "{}: {} in, {} kept, {} blocks, {} octets",
                    f.name, f.records_in, f.records_kept, f.blocks, f.bytes
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oa-compass slice: {e}");
            ExitCode::from(1)
        }
    }
}

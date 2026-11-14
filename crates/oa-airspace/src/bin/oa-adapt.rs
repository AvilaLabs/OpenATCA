//! `oa-adapt`: builds and compares airspace snapshots. Shell: see `oa_airspace::cli`.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = std::io::stdout().lock();
    match oa_airspace::cli::run(&args, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-adapt: {e}");
            ExitCode::FAILURE
        }
    }
}

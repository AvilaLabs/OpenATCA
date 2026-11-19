//! `oa-scope-replay`: renders tracker output over the airspace map, one image per step. Shell:
//! see `oa_scope_replay::cli`.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = std::io::stdout().lock();
    match oa_scope_replay::cli::run(&args, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-scope-replay: {e}");
            ExitCode::FAILURE
        }
    }
}

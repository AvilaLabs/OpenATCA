//! `oa-scope-replay`: renders tracker output over the airspace map, one image per step. Shell:
//! see `oa_scope_replay::cli`. Exit code 0 success, 1 failure, 2 usage error, 3 work was lost
//! (the frames and hashes are written; `--allow-loss` accepts it).
#![forbid(unsafe_code)]

use oa_scope_replay::cli::CliError;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = std::io::stdout().lock();
    match oa_scope_replay::cli::run(&args, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-scope-replay: {e}");
            ExitCode::from(match e {
                CliError::Failed(_) => 1,
                CliError::Usage(_) => 2,
                CliError::Loss(_) => 3,
            })
        }
    }
}

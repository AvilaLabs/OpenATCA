//! `oa-scope`: renders the static airspace map to an image. Shell: see `oa_display::cli`.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = std::io::stdout().lock();
    match oa_display::cli::run(&args, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-scope: {e}");
            ExitCode::FAILURE
        }
    }
}

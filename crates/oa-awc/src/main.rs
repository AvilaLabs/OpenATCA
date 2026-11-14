#![forbid(unsafe_code)]
//! `oa-awc-summary [dir]`: parses every AWC JSON file under `dir` (default
//! `$OPENATCA_DATA/raw/awc`) and prints record counts, refusals by reason, de-duplication
//! counts, METAR altimeter cross-check results and station altimeter availability. Shell: owns
//! the files, the environment and standard output (B3).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let dir = match std::env::args_os().nth(1) {
        Some(d) => PathBuf::from(d),
        None => match std::env::var_os("OPENATCA_DATA") {
            Some(d) => PathBuf::from(d).join("raw").join("awc"),
            None => {
                eprintln!("usage: oa-awc-summary [dir]   (or set OPENATCA_DATA)");
                return ExitCode::from(2);
            }
        },
    };
    match oa_awc::summary::scan_dir(&dir) {
        Ok(s) => {
            print!("{}", oa_awc::summary::format_summary(&s));
            if s.file_errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("oa-awc-summary: {e}");
            ExitCode::from(2)
        }
    }
}

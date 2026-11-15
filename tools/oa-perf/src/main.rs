#![forbid(unsafe_code)]
//! `oa-perf` command line; see `oa_perf::cli`.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match oa_perf::cli::run(&args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-perf: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

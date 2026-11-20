#![forbid(unsafe_code)]
//! `oa-ca` (shell, B3): arguments, files, the clock and stdout. Exit 0 on success, 1 on a failed
//! run, 2 on a usage error or a sealed input or output path.

use std::process::ExitCode;
use std::time::Instant;

use oa_ca_run::args::{parse, USAGE};
use oa_ca_run::guard::refuse_sealed;
use oa_ca_run::run::run;
use oa_sealed::Sealed;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("oa-ca: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Every path is checked, as given and resolved, before anything is opened or created.
    for p in [&args.adsb, &args.config, &args.out] {
        if let Err(e) = refuse_sealed(Sealed::production(), p) {
            eprintln!("oa-ca: {e}");
            return ExitCode::from(2);
        }
    }
    let t0 = Instant::now();
    match run(&args) {
        Ok(summary) => {
            print!("{summary}");
            eprintln!("run time {:.1} s", t0.elapsed().as_secs_f64());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oa-ca: {e}");
            ExitCode::from(1)
        }
    }
}

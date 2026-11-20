#![forbid(unsafe_code)]
//! `oa-encounter` (shell, B3): arguments, files, the clock and stdout. Exit 0 on success, 1 on a
//! failed run, 2 on a usage error or a sealed input or output path.

use std::process::ExitCode;
use std::time::Instant;

use oa_encounter_run::args::{parse, USAGE};
use oa_encounter_run::guard::refuse_sealed;
use oa_encounter_run::run::run;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("oa-encounter: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Every path is checked, as given and resolved, before anything is opened or created.
    for p in [
        Some(&args.adsb),
        Some(&args.out),
        args.nasr.as_ref(),
        args.sfdps.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Err(e) = refuse_sealed(p) {
            eprintln!("oa-encounter: {e}");
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
            eprintln!("oa-encounter: {e}");
            ExitCode::from(1)
        }
    }
}

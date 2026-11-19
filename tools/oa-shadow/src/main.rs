#![forbid(unsafe_code)]
//! `oa-shadow`: the offline shadow comparator (see the library). Shell (B3): arguments, files,
//! the clock and stdout. Exit 0 on success, 1 on a failed run, 2 on a usage error or a sealed
//! input path.

use std::process::ExitCode;
use std::time::Instant;

use oa_shadow::args::{parse, USAGE};
use oa_shadow::guard::refuse_sealed;
use oa_shadow::run::{run, run_screen};

fn main() -> ExitCode {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    // `oa-shadow screen ...` runs the ADS-B persistent-error screen with the same arguments.
    let screen = argv.first().is_some_and(|a| a == "screen");
    if screen {
        argv.remove(0);
    }
    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("oa-shadow: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Every path is checked before anything is opened.
    for p in [&args.adsb, &args.sfdps_dir, &args.out] {
        if let Err(e) = refuse_sealed(p) {
            eprintln!("oa-shadow: {e}");
            return ExitCode::from(2);
        }
    }
    let t0 = Instant::now();
    let result = if screen {
        run_screen(&args)
    } else {
        run(&args)
    };
    match result {
        Ok(summary) => {
            print!("{summary}");
            eprintln!("run time {:.1} s", t0.elapsed().as_secs_f64());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oa-shadow: {e}");
            ExitCode::from(1)
        }
    }
}

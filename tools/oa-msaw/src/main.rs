#![forbid(unsafe_code)]
//! `oa-msaw` (shell, B3): arguments, files, the clock and stdout. Exit 0 on success, 1 on a failed
//! run, 2 on a usage error or a sealed input or output path.

use std::process::ExitCode;
use std::time::Instant;

use oa_msaw_run::args::{parse, USAGE};
use oa_msaw_run::guard::refuse_sealed;
use oa_msaw_run::run::run_counted;
use oa_sealed::Sealed;

fn main() -> ExitCode {
    // Printed when `main` returns: on every exit, an error and a refusal included.
    let mut drops = oa_sealed::DropReport::new();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("oa-msaw: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Every path is checked, as given and resolved, before anything is opened or created.
    let optional = [&args.terrain, &args.dof, &args.mva, &args.cifp];
    for p in [&args.adsb, &args.config, &args.out]
        .into_iter()
        .chain(optional.into_iter().flatten())
    {
        if let Err(e) = refuse_sealed(Sealed::production(), p) {
            eprintln!("oa-msaw: {e}");
            return ExitCode::from(2);
        }
    }
    let t0 = Instant::now();
    let mut sealed = 0u64;
    let result = run_counted(&args, &mut sealed);
    drops.add(sealed);
    match result {
        Ok(summary) => {
            print!("{summary}");
            eprintln!("run time {:.1} s", t0.elapsed().as_secs_f64());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oa-msaw: {e}");
            ExitCode::from(1)
        }
    }
}

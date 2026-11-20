#![forbid(unsafe_code)]
//! `oa-replay`: merges recorded SWIM feeds and ADS-B clips into one ordered stream, replays it through the
//! flight-state pipeline and prints the checkpoints and a final summary. Shell (B1): reads
//! files, reads a clock for the throughput line, writes stdout.
//!
//! Every line except the one starting `timing` is a function of the recordings and the settings
//! alone, so two runs over the same files print identical text there (RULESET B6). Exit status:
//! 0 after a complete replay (failures inside the data are counted in the summary, not fatal),
//! 1 if a file cannot be read or stdout cannot be written (the run stops at the failed write, with
//! one line on stderr), 2 for a usage error.

use std::process::ExitCode;
use std::time::Instant;

use oa_replay::cli::{parse_args, USAGE};
use oa_replay::report::{drain_checkpoint_lines, write_failure_message, write_summary};
use oa_replay::{
    frames_total, list_recordings, new_pipeline, run_streams_with, spec_from_paths, Pipeline,
    NS_PER_S,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("oa-replay: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Every input path is checked, as given and resolved, before anything is opened.
    for i in &cmd.inputs {
        if let Err(e) = oa_sealed::refuse(&i.path) {
            eprintln!("oa-replay: {e}");
            return ExitCode::from(2);
        }
    }
    let mut specs = Vec::with_capacity(cmd.inputs.len());
    for i in &cmd.inputs {
        let paths = if i.dir {
            match list_recordings(&i.path) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("oa-replay: {e}");
                    return ExitCode::from(1);
                }
            }
        } else {
            vec![i.path.clone()]
        };
        specs.push(spec_from_paths(i.feed, paths));
    }
    let mut p = match new_pipeline(cmd.cfg) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("oa-replay: bad settings: {e:?}");
            return ExitCode::from(2);
        }
    };
    let t0 = Instant::now();
    let mut out = std::io::stdout().lock();
    let mut first_ns = i64::MIN;
    let mut next_progress = i64::MIN;
    let quiet = cmd.quiet;
    let progress_ns = cmd.progress_ns;
    let mut write_err: Option<std::io::Error> = None;
    let mut hook = |p: &mut Pipeline| -> std::io::Result<()> {
        if let Err(e) = drain_checkpoint_lines(p, &mut out, quiet) {
            let stop = std::io::Error::new(e.kind(), "output write failed");
            write_err = Some(e);
            return Err(stop);
        }
        if first_ns == i64::MIN {
            first_ns = p.replay_ns;
            next_progress = p.replay_ns.saturating_add(progress_ns);
        } else if p.replay_ns >= next_progress {
            next_progress = p.replay_ns.saturating_add(progress_ns);
            let wall = t0.elapsed().as_secs_f64().max(1e-9);
            let replayed = p.replay_ns.saturating_sub(first_ns) as f64 / NS_PER_S as f64;
            eprintln!(
                "progress replay_s={} frames={} frames_per_s={:.0} speed={:.0}x rss_mb={:.0}",
                p.replay_ns.div_euclid(NS_PER_S),
                frames_total(p),
                frames_total(p) as f64 / wall,
                replayed / wall,
                status_kb("VmRSS:") as f64 / 1024.0
            );
        }
        Ok(())
    };
    let report = match run_streams_with(&mut p, specs, cmd.chunk, &mut hook) {
        Ok(r) => r,
        Err(e) => {
            return match write_err {
                Some(w) => fail_write(&w),
                None => fail_read(&e),
            }
        }
    };
    let elapsed = t0.elapsed();
    // The frames' last checkpoints are drained before the summary, so none is left unprinted.
    if let Err(e) = drain_checkpoint_lines(&mut p, &mut out, quiet) {
        return fail_write(&e);
    }
    let peak = status_kb("VmHWM:");
    if let Err(e) = write_summary(
        &p,
        &mut out,
        report.files,
        report.bytes,
        elapsed.as_secs_f64(),
        peak,
    ) {
        return fail_write(&e);
    }
    ExitCode::SUCCESS
}

/// A `kB` figure from `/proc/self/status` (0 where there is none): resident or peak memory.
fn status_kb(key: &str) -> u64 {
    // seal-allow: /proc, not an input
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

/// One line on stderr and exit status 1 for a file that cannot be read.
fn fail_read(e: &std::io::Error) -> ExitCode {
    eprintln!("oa-replay: {e}");
    ExitCode::from(1)
}

/// One line on stderr and exit status 1 for a write to stdout that failed (a closed pipe gets its
/// own short message).
fn fail_write(e: &std::io::Error) -> ExitCode {
    eprintln!("{}", write_failure_message(e));
    ExitCode::from(1)
}

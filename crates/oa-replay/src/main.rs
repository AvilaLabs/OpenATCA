#![forbid(unsafe_code)]
//! `oa-replay`: merges recorded SWIM feeds and ADS-B clips into one ordered stream, replays it through the
//! flight-state pipeline and prints the checkpoints and a final summary. Shell (B1): reads
//! files, reads a clock for the throughput line, writes stdout.
//!
//! Every line except the one starting `timing` is a function of the recordings and the settings
//! alone, so two runs over the same files print identical text there (RULESET B6). Exit status:
//! 0 after a complete replay (failures inside the data are counted in the summary, not fatal),
//! 1 on a read, stdout write, or replay verification failure, 2 for a usage error.

use std::cell::RefCell;
use std::process::ExitCode;
use std::time::Instant;

use oa_replay::cli::{parse_args, USAGE};
use oa_replay::report::{drain_checkpoint_lines, write_failure_message, write_summary};
use oa_replay::{
    errors_total, frames_total, input_fingerprint::InputFingerprint, list_recordings, new_pipeline,
    replay_state_digest, run_streams_counted, run_streams_observed, spec_from_paths, Pipeline,
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
    let verify_prefix = cmd.verify_prefix;
    let verify_end = cmd.verify_end;
    let verify_mode = verify_prefix.is_some() || verify_end.is_some();
    let verification = match (verify_prefix.is_some(), verify_end.is_some()) {
        (true, true) => "prefix and terminal",
        (true, false) => "prefix",
        _ => "terminal",
    };
    let fingerprint_enabled = cmd.input_fingerprint || verify_mode;
    let input_fingerprint = RefCell::new(fingerprint_enabled.then(InputFingerprint::default));
    // Every input path is checked, as given and resolved, before anything is opened.
    for i in &cmd.inputs {
        if let Err(e) = oa_sealed::refuse(&i.path) {
            eprintln!("oa-replay: {e}");
            if let Some(fingerprint) = input_fingerprint.borrow().as_ref() {
                print_fingerprint(fingerprint, "incomplete", Some(1), 0, 0, None);
            }
            return ExitCode::from(2);
        }
    }
    // Printed when `main` returns: on every exit once reading has started, an error included.
    let mut drops = oa_sealed::DropReport::new();
    let mut specs = Vec::with_capacity(cmd.inputs.len());
    for i in &cmd.inputs {
        let paths = if i.dir {
            match list_recordings(&i.path) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("oa-replay: {e}");
                    if let Some(fingerprint) = input_fingerprint.borrow().as_ref() {
                        print_fingerprint(fingerprint, "incomplete", None, 0, 0, None);
                    }
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
    let quiet = cmd.quiet || verify_mode;
    let progress_ns = cmd.progress_ns;
    let mut prefix_verified = false;
    let mut write_err: Option<std::io::Error> = None;
    let mut hook = |p: &mut Pipeline| -> std::io::Result<()> {
        if let Err(e) = drain_checkpoint_lines(p, &mut out, quiet) {
            let stop = std::io::Error::new(e.kind(), "output write failed");
            write_err = Some(e);
            return Err(stop);
        }
        if let Some(witness) = verify_prefix.filter(|_| !prefix_verified) {
            let observed = input_fingerprint.borrow();
            let fingerprint = observed
                .as_ref()
                .ok_or_else(|| std::io::Error::other("ordered-input digest unavailable"))?;
            if fingerprint.frames() == witness.frames {
                let input_digest = fingerprint.finalize().ok_or_else(|| {
                    std::io::Error::other("ordered-input digest unavailable at prefix boundary")
                })?;
                if input_digest != witness.input_digest {
                    return Err(std::io::Error::other(
                        "ordered-input digest mismatch at prefix boundary",
                    ));
                }
                if replay_state_digest(p) != witness.state_digest {
                    return Err(std::io::Error::other(
                        "replay-state digest mismatch at prefix boundary",
                    ));
                }
                prefix_verified = true;
            } else if fingerprint.frames() > witness.frames {
                return Err(std::io::Error::other(
                    "prefix boundary was not observed exactly",
                ));
            }
        }
        if verify_mode {
            return Ok(());
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
    let mut sealed_frames = 0u64;
    let ran = if fingerprint_enabled {
        run_streams_observed(
            &mut p,
            specs,
            cmd.chunk,
            &mut hook,
            &mut sealed_frames,
            &mut |frame| {
                if let Some(fingerprint) = input_fingerprint.borrow_mut().as_mut() {
                    fingerprint.observe(frame);
                }
            },
        )
    } else {
        run_streams_counted(&mut p, specs, cmd.chunk, &mut hook, &mut sealed_frames)
    };
    let input_fingerprint = input_fingerprint.into_inner();
    // Frames by receive time, plus SFDPS, TBFM and TFMS rows by their own time, added before the
    // result is looked at, so the line is printed on every exit after this point.
    drops.add(sealed_frames);
    drops.add(p.sealed_window_dropped);
    let report = match ran {
        Ok(r) => r,
        Err(e) => {
            if verify_mode {
                eprintln!("oa-replay: {verification} verification refused: {e}");
                return ExitCode::from(1);
            }
            if let Some(fingerprint) = &input_fingerprint {
                print_fingerprint(
                    fingerprint,
                    "incomplete",
                    None,
                    sealed_frames,
                    p.sealed_window_dropped,
                    None,
                );
            }
            return match write_err {
                Some(w) => fail_write(&w),
                None => fail_read(&e),
            };
        }
    };
    let fingerprint_quality = input_fingerprint.as_ref().map(|fingerprint| {
        let source_rejected = errors_total(&report.errors);
        let semantic_errors = errors_total(&p.errors).saturating_sub(source_rejected);
        let partial = source_rejected > 0
            || report.sealed_window_dropped > 0
            || p.sealed_window_dropped > 0
            || semantic_errors > 0
            || fingerprint.bridge_reject_frames() > 0
            || p.adsb.stats.future_refused > 0
            || p.rows_out_of_order.iter().any(|&n| n != 0)
            || p.rows_refused_full.iter().any(|&n| n != 0);
        (source_rejected, semantic_errors, partial)
    });
    if let Some(witness) = verify_prefix.filter(|_| !prefix_verified) {
        eprintln!(
            "oa-replay: prefix verification refused: expected frame boundary {} was not reached (observed {})",
            witness.frames,
            input_fingerprint.as_ref().map_or(0, InputFingerprint::frames)
        );
        return ExitCode::from(1);
    }
    if verify_mode {
        let Some((observed_frames, observed_input)) = input_fingerprint
            .as_ref()
            .and_then(|fingerprint| Some((fingerprint.frames(), fingerprint.finalize()?)))
        else {
            eprintln!(
                "oa-replay: {verification} verification refused: full ordered-input digest unavailable"
            );
            return ExitCode::from(1);
        };
        if fingerprint_quality.is_none_or(|(_, _, partial)| partial) {
            eprintln!(
                "oa-replay: {verification} verification refused: replay is partial or incomplete"
            );
            return ExitCode::from(1);
        }
        if let Some(witness) = verify_end {
            if observed_frames != witness.frames {
                eprintln!(
                    "oa-replay: terminal verification refused: expected {} admitted frames, observed {observed_frames}",
                    witness.frames
                );
                return ExitCode::from(1);
            }
            if observed_input != witness.input_digest {
                eprintln!(
                    "oa-replay: terminal verification refused: ordered-input digest mismatch"
                );
                return ExitCode::from(1);
            }
            if replay_state_digest(&p) != witness.state_digest {
                eprintln!("oa-replay: terminal verification refused: replay-state digest mismatch");
                return ExitCode::from(1);
            }
        }
    }
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
    if let (Some(fingerprint), Some((source_rejected, semantic_errors, partial))) =
        (&input_fingerprint, fingerprint_quality)
    {
        let status = if partial { "partial" } else { "complete" };
        print_fingerprint(
            fingerprint,
            status,
            Some(source_rejected),
            report.sealed_window_dropped,
            p.sealed_window_dropped,
            Some(semantic_errors),
        );
    }
    if let Some(witness) = verify_prefix {
        eprintln!("verified replay prefix frames={}", witness.frames);
    }
    if let Some(witness) = verify_end {
        eprintln!("verified replay end frames={}", witness.frames);
    }
    ExitCode::SUCCESS
}

/// One opt-in bounded diagnostic line; it never includes payload data.
fn print_fingerprint(
    fingerprint: &InputFingerprint,
    status: &str,
    source_rejected: Option<u64>,
    source_sealed: u64,
    semantic_sealed: u64,
    semantic_errors: Option<u64>,
) {
    let digest = fingerprint.finalize();
    let status = if digest.is_some() {
        status
    } else {
        "unavailable"
    };
    let digest = digest
        .map(|d| oa_sha256::to_hex(&d))
        .unwrap_or_else(|| "unavailable".to_string());
    let source_rejected = source_rejected
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let semantic_errors = semantic_errors
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    eprintln!(
        "input_fingerprint version=1 status={status} pipeline_frames={} pipeline_payload_bytes={} sha256={digest} source_rejected={source_rejected} source_sealed_drops={source_sealed} semantic_sealed_drops={semantic_sealed} semantic_errors={semantic_errors} bridge_reject_frames={} note=diagnostic_only_not_completeness_or_cross_clock_equivalence",
        fingerprint.frames(),
        fingerprint.payload_bytes(),
        fingerprint.bridge_reject_frames(),
    );
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

#![forbid(unsafe_code)]
//! `oa-replay-diff`: compares two tracker runs of one clip from their digest files (and, to say
//! how they differ, their rows files). Shell (B3): arguments, files, stdout and the exit code.
//!
//! ```text
//! oa-replay-diff <digest-a> <digest-b> [--rows-a <file> --rows-b <file>]
//!                [--expect <file>] [--summary <file>] [--max-detail <n>]
//! ```
//!
//! Exit code 0: identical (or, with `--expect`, every difference is declared); 1: different;
//! 2: error (a malformed or inconsistent input, an unwritable output, a closed pipe).

use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use oa_replay_diff::{
    compare_digests, compare_rows, digest_rows, digest_seconds_in_window, load_digest_text,
    load_expect_text, load_witness_text, nothing_to_compare, open_rows, parse_digest, parse_expect,
    render_report, render_summary, row_in_window, verdict, write_out, Comparison, Group,
};
use oa_sealed::DropWindow;
use oa_track::canon::from_hex32;

const USAGE: &str = "usage: oa-replay-diff <digest-a> <digest-b> [--rows-a <file> --rows-b <file>] [--expect <file>] [--summary <file>] [--max-detail <n>]\n       oa-replay-diff --compare-witness-files <witness-a> <witness-b>";

fn main() -> ExitCode {
    // Printed (on standard error) on every exit: the count of what was refused for its time.
    let mut drops = oa_sealed::DropReport::new();
    match run(&mut drops) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("oa-replay-diff: {e}");
            ExitCode::from(2)
        }
    }
}

/// Sets an option that may be given once; a repeat is a usage error, not a silent override.
fn once(slot: &mut Option<PathBuf>, name: &str, v: String) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} given twice\n{USAGE}"));
    }
    *slot = Some(PathBuf::from(v));
    Ok(())
}

fn run(drops: &mut oa_sealed::DropReport) -> Result<u8, String> {
    let mut args = std::env::args().skip(1).peekable();
    if args
        .peek()
        .is_some_and(|arg| arg == "--compare-witness-files")
    {
        let _mode = args.next();
        let witness_args: Vec<_> = args.collect();
        return run_witness_comparison(&witness_args);
    }
    let (mut pos, mut rows_a, mut rows_b) = (Vec::new(), None, None);
    let (mut expect, mut summary, mut max_detail) = (None, None, 50usize);
    let mut seen_max_detail = false;
    while let Some(a) = args.next() {
        if !a.starts_with("--") {
            pos.push(PathBuf::from(a));
            continue;
        }
        let v = args
            .next()
            .ok_or_else(|| format!("{a} needs a value\n{USAGE}"))?;
        match a.as_str() {
            "--rows-a" => once(&mut rows_a, &a, v)?,
            "--rows-b" => once(&mut rows_b, &a, v)?,
            "--expect" => once(&mut expect, &a, v)?,
            "--summary" => once(&mut summary, &a, v)?,
            "--max-detail" => {
                if seen_max_detail {
                    return Err(format!("{a} given twice\n{USAGE}"));
                }
                seen_max_detail = true;
                max_detail = v.parse().map_err(|_| format!("bad --max-detail `{v}`"))?;
            }
            _ => return Err(format!("unknown argument {a}\n{USAGE}")),
        }
    }
    let [pa, pb] = pos.as_slice() else {
        return Err(format!("two digest files are required\n{USAGE}"));
    };
    if rows_a.is_some() != rows_b.is_some() {
        return Err("--rows-a and --rows-b go together".to_string());
    }
    // Every path is checked, as given and resolved, before anything is opened or created.
    for p in pos
        .iter()
        .chain(rows_a.iter())
        .chain(rows_b.iter())
        .chain(expect.iter())
        .chain(summary.iter())
    {
        oa_sealed::refuse(p)?;
    }
    let read = |p: &PathBuf| {
        load_digest_text(p)
            .and_then(|t| parse_digest(&t).map_err(|e| format!("{}: {e}", p.display())))
    };
    let (la, lb) = (read(pa)?, read(pb)?);
    // An input that holds a second or a row in the sealed window is refused whole: nothing of it
    // is compared or printed, only the count.
    let window = DropWindow::sealed();
    let seconds = digest_seconds_in_window(&la, window) + digest_seconds_in_window(&lb, window);
    if seconds > 0 {
        drops.add(seconds);
        return Err(format!(
            "refused: {seconds} digest seconds are in the sealed window; nothing compared"
        ));
    }
    let allowed = match &expect {
        Some(p) => {
            if rows_a.is_none() {
                return Err("--expect needs the rows files to know what differs".to_string());
            }
            let t = load_expect_text(p)?;
            Some(parse_expect(&t).map_err(|e| format!("{}: {e}", p.display()))?)
        }
        None => None,
    };
    nothing_to_compare(&la, &lb)?;
    let digest = compare_digests(&la, &lb);
    // Rows files, when given, are verified against their digests whether or not the digests
    // differ.
    let rows = match (&rows_a, &rows_b) {
        (Some(x), Some(y)) => {
            let (ra, rb) = (
                open_rows(x, digest_rows(&la))?,
                open_rows(y, digest_rows(&lb))?,
            );
            // Every row is counted as it streams past; a count above zero refuses the result,
            // whatever the comparison found (nothing is printed).
            let sealed = Cell::new(0u64);
            let count = |g: Result<Group, String>| {
                if let Ok((_, rows)) = &g {
                    let n = rows.iter().filter(|r| row_in_window(window, r)).count();
                    sealed.set(sealed.get().saturating_add(n as u64));
                }
                g
            };
            let r = compare_rows(&digest, &la, &lb, ra.map(count), rb.map(count));
            if sealed.get() > 0 {
                drops.add(sealed.get());
                return Err(format!(
                    "refused: {} rows are in the sealed window; nothing compared",
                    sealed.get()
                ));
            }
            Some(r?)
        }
        _ => None,
    };
    let c = Comparison { digest, rows };
    let text = render_report(&c, max_detail);
    let sum = render_summary(&c, allowed.as_ref());
    if let Some(p) = summary {
        fs::write(&p, &sum).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    // Not `print!`: a closed pipe is an error and exit 2, never a panic.
    write_out(
        &mut std::io::stdout().lock(),
        &format!("{text}\n[summary]\n{sum}"),
    )?;
    Ok(verdict(&c, allowed.as_ref()))
}

#[derive(Clone, Copy)]
struct ReplayWitness {
    settings: [u8; 32],
    frames: u64,
    input: [u8; 32],
    state: [u8; 32],
}

fn witness_digest(field: &str, token: &str) -> Result<[u8; 32], String> {
    let value = token
        .strip_prefix(&format!("{field}="))
        .ok_or_else(|| format!("malformed witness {field}"))?;
    from_hex32(value).ok_or_else(|| format!("malformed witness {field}"))
}

fn witness_count(field: &str, token: &str) -> Result<u64, String> {
    token
        .strip_prefix(&format!("{field}="))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| format!("malformed witness {field}"))
}

fn parse_witness(text: &str) -> Result<ReplayWitness, String> {
    let mut lines = text.lines();
    let settings: Vec<_> = lines
        .next()
        .ok_or_else(|| "missing settings witness line".to_string())?
        .split_whitespace()
        .collect();
    let [marker, version, digest, note] = settings.as_slice() else {
        return Err("malformed settings witness line".to_string());
    };
    if *marker != "settings_fingerprint"
        || *version != "version=1"
        || *note != "note=settings_and_ordered_feed_stream_kinds_only"
    {
        return Err("malformed settings witness line".to_string());
    }
    let settings_digest = witness_digest("sha256", digest)?;

    let input: Vec<_> = lines
        .next()
        .ok_or_else(|| "missing input witness line".to_string())?
        .split_whitespace()
        .collect();
    let [marker, version, status, frames_token, payload_token, digest_token, source_rejected, source_sealed, semantic_sealed, semantic_errors, bridge_rejects, note] =
        input.as_slice()
    else {
        return Err("incomplete or malformed input witness line".to_string());
    };
    if *marker != "input_fingerprint"
        || *version != "version=1"
        || *status != "status=complete"
        || *note != "note=diagnostic_only_not_completeness_or_cross_clock_equivalence"
    {
        return Err("incomplete or malformed input witness line".to_string());
    }
    let frames = witness_count("pipeline_frames", frames_token)?;
    if frames == 0 {
        return Err("witness has no admitted input frames".to_string());
    }
    let _payload_bytes = witness_count("pipeline_payload_bytes", payload_token)?;
    let input_digest = witness_digest("sha256", digest_token)?;
    for (field, token) in [
        ("source_rejected", *source_rejected),
        ("source_sealed_drops", *source_sealed),
        ("semantic_sealed_drops", *semantic_sealed),
        ("semantic_errors", *semantic_errors),
        ("bridge_reject_frames", *bridge_rejects),
    ] {
        if witness_count(field, token)? != 0 {
            return Err(format!("witness has nonzero {field}"));
        }
    }

    let state: Vec<_> = lines
        .next()
        .ok_or_else(|| "missing final state witness line".to_string())?
        .split_whitespace()
        .collect();
    let [marker, digest_token] = state.as_slice() else {
        return Err("malformed final state witness line".to_string());
    };
    if *marker != "final" {
        return Err("malformed final state witness line".to_string());
    }
    let state_digest = witness_digest("state_digest", digest_token)?;
    if lines.next().is_some() {
        return Err("extra witness lines".to_string());
    }
    Ok(ReplayWitness {
        settings: settings_digest,
        frames,
        input: input_digest,
        state: state_digest,
    })
}

fn run_witness_comparison(args: &[String]) -> Result<u8, String> {
    let [a, b] = args else {
        return Err(format!("two witness files are required\n{USAGE}"));
    };
    let (a, b) = (PathBuf::from(a), PathBuf::from(b));
    // Check both paths before either file is opened. The read helper repeats the guard at open.
    oa_sealed::refuse(&a)?;
    oa_sealed::refuse(&b)?;
    let left = parse_witness(&load_witness_text(&a)?)?;
    let right = parse_witness(&load_witness_text(&b)?)?;
    let (result, code) = if left.settings != right.settings {
        ("SETTINGS MISMATCH", 1)
    } else if left.frames != right.frames || left.input != right.input {
        ("INPUT MISMATCH", 1)
    } else if left.state != right.state {
        ("STATE MISMATCH", 1)
    } else {
        ("MATCH", 0)
    };
    write_out(&mut std::io::stdout().lock(), &format!("{result}\n"))?;
    Ok(code)
}

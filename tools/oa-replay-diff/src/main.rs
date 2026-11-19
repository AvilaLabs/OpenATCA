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

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use oa_replay_diff::{
    compare_digests, compare_rows, digest_rows, load_digest_text, load_expect_text, open_rows,
    parse_digest, parse_expect, render_report, render_summary, verdict, write_out, Comparison,
};

const USAGE: &str = "usage: oa-replay-diff <digest-a> <digest-b> [--rows-a <file> --rows-b <file>] [--expect <file>] [--summary <file>] [--max-detail <n>]";

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("oa-replay-diff: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<u8, String> {
    let (mut pos, mut rows_a, mut rows_b) = (Vec::new(), None, None);
    let (mut expect, mut summary, mut max_detail) = (None, None, 50usize);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if !a.starts_with("--") {
            pos.push(PathBuf::from(a));
            continue;
        }
        let v = args
            .next()
            .ok_or_else(|| format!("{a} needs a value\n{USAGE}"))?;
        match a.as_str() {
            "--rows-a" => rows_a = Some(PathBuf::from(v)),
            "--rows-b" => rows_b = Some(PathBuf::from(v)),
            "--expect" => expect = Some(PathBuf::from(v)),
            "--summary" => summary = Some(PathBuf::from(v)),
            "--max-detail" => {
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
    let read = |p: &PathBuf| {
        load_digest_text(p)
            .and_then(|t| parse_digest(&t).map_err(|e| format!("{}: {e}", p.display())))
    };
    let (la, lb) = (read(pa)?, read(pb)?);
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
    let digest = compare_digests(&la, &lb);
    let rows = match (&rows_a, &rows_b) {
        (Some(x), Some(y)) if !digest.identical => {
            let (ra, rb) = (
                open_rows(x, digest_rows(&la))?,
                open_rows(y, digest_rows(&lb))?,
            );
            Some(compare_rows(&digest, &la, &lb, ra, rb)?)
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

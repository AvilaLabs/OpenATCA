#![forbid(unsafe_code)]
//! `oa-feed-gaps`: the damage check of a scored window (P-006 section 6). Shell (B3).
//!
//! ```text
//! oa-feed-gaps --sfdps-dir <dir> --adsb <clip.oaf> --from <ISO time> --to <ISO time> [--clip-log <file>] [--out <file>]
//! ```
//!
//! Exit 0 when clean, 1 when damaged, 2 on a usage or read error.

use std::io::Write;
use std::process::ExitCode;

use oa_feed_gaps::shell::args::{parse, USAGE};
use oa_feed_gaps::shell::run::{check, write_atomic};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&argv) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("oa-feed-gaps: {m}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let (text, damaged) = match check(&a) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("oa-feed-gaps: {e}");
            return ExitCode::from(2);
        }
    };
    let wrote = match &a.out {
        Some(p) => write_atomic(p, &text),
        None => std::io::stdout().write_all(text.as_bytes()),
    };
    if let Err(e) = wrote {
        eprintln!("oa-feed-gaps: writing the report: {e}");
        return ExitCode::from(2);
    }
    ExitCode::from(u8::from(damaged))
}

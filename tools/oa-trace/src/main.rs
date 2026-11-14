#![forbid(unsafe_code)]
//! `oa-trace [--root <dir>] [--markdown <out.md>]`

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut root = PathBuf::from(".");
    let mut markdown: Option<PathBuf> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        let target = match a.to_str() {
            Some("--root") => &mut root,
            Some("--markdown") => markdown.get_or_insert_with(PathBuf::new),
            _ => {
                eprintln!("usage: oa-trace [--root <workspace dir>] [--markdown <out.md>]");
                return ExitCode::from(2);
            }
        };
        match args.next() {
            Some(v) => *target = PathBuf::from(v),
            None => {
                eprintln!("missing value for {}", a.to_string_lossy());
                return ExitCode::from(2);
            }
        }
    }
    let rep = oa_trace::analyze(&root);
    print!("{}", oa_trace::render_text(&rep));
    if let Some(path) = markdown {
        if let Err(e) = std::fs::write(&path, oa_trace::render_markdown(&rep)) {
            eprintln!("cannot write {}: {e}", path.display());
            return ExitCode::from(2);
        }
    }
    if rep.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

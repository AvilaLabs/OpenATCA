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
    let classes = oa_trace::classcheck::check(&root);
    print!("\n{}", oa_trace::classcheck::render_text(&classes));
    let accreditation = oa_trace::accreditation::check(&root);
    println!("\nAccreditation source references:");
    if accreditation.is_empty() {
        println!("  OK");
    } else {
        for error in &accreditation {
            println!("  ERROR: {error}");
        }
    }
    let hazard_errors = oa_trace::hazard_trace::check(&root);
    if hazard_errors.is_empty() {
        println!("\nHazard trace: declared hazard/requirement/test links pass.");
    } else {
        println!("\nHazard trace: {} issue(s)", hazard_errors.len());
        for error in &hazard_errors {
            println!("  {error}");
        }
    }
    if rep.ok() && classes.ok() && accreditation.is_empty() && hazard_errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

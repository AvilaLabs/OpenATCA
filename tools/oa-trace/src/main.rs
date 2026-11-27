#![forbid(unsafe_code)]
//! `oa-trace [--root <dir>] [--markdown <out.md>]`

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut root = PathBuf::from(".");
    let mut markdown: Option<PathBuf> = None;
    let mut verify_test_refs = Vec::new();
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        if a == "--verify-test" {
            match args.next().and_then(|value| value.into_string().ok()) {
                Some(value) => verify_test_refs.push(value),
                None => {
                    eprintln!("missing UTF-8 value for --verify-test");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        let target = match a.to_str() {
            Some("--root") => &mut root,
            Some("--markdown") => markdown.get_or_insert_with(PathBuf::new),
            _ => {
                eprintln!("usage: oa-trace [--root <workspace dir>] [--markdown <out.md>] [--verify-test path::function ...]");
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
    if !verify_test_refs.is_empty() {
        let mut errors = Vec::new();
        for reference in &verify_test_refs {
            if let Err(error) = oa_trace::accreditation::verify_test_reference(&root, reference) {
                errors.push(format!("{reference}: {error}"));
            }
        }
        if errors.is_empty() {
            println!("validated {} test reference(s)", verify_test_refs.len());
            return ExitCode::SUCCESS;
        }
        for error in errors {
            eprintln!("invalid test reference: {error}");
        }
        return ExitCode::FAILURE;
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

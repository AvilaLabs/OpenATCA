//! The crate source must not call anything that reaches the platform maths library.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

/// Method or associated-function names that are not guaranteed correctly rounded by IEEE-754
/// (or that some targets lower to a libm call). `sqrt`, `abs` and bit casts are allowed.
const BANNED: &[&str] = &[
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "sinh",
    "cosh",
    "tanh",
    "asinh",
    "acosh",
    "atanh",
    "exp",
    "exp2",
    "exp_m1",
    "ln",
    "ln_1p",
    "log",
    "log2",
    "log10",
    "powf",
    "powi",
    "cbrt",
    "hypot",
    "mul_add",
    "floor",
    "ceil",
    "round",
    "round_ties_even",
    "trunc",
    "fract",
    "rem_euclid",
    "div_euclid",
    "min",
    "max",
    "clamp",
    "copysign",
    "signum",
    "sin_cos",
    "recip",
    "to_degrees",
    "to_radians",
    "abs_sub",
    "next_up",
    "next_down",
];

fn sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            out.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                text,
            ));
        }
    }
    out
}

/// The line without its `//` comment (doc comments included).
fn code_part(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

// REQ: OA-MATH-019
#[test]
fn sources_call_no_platform_maths_function() {
    let files = sources();
    assert!(files.len() >= 10, "found {} source files", files.len());
    let mut hits = Vec::new();
    for (name, text) in &files {
        for (n, line) in text.lines().enumerate() {
            let code = code_part(line);
            for b in BANNED {
                for pat in [format!(".{b}("), format!("f64::{b}("), format!("f32::{b}(")] {
                    if code.contains(&pat) {
                        hits.push(format!("{name}:{}: {pat}", n + 1));
                    }
                }
            }
            for word in [
                "libm",
                "intrinsics::",
                "extern \"C\"",
                "core::arch",
                "std::arch",
                "unsafe {",
                "unsafe fn",
                "unsafe impl",
            ] {
                if code.contains(word) {
                    hits.push(format!("{name}:{}: {word}", n + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "forbidden calls in oa-math sources: {hits:#?}"
    );
}

// REQ: OA-MATH-019
#[test]
fn the_check_itself_finds_a_planted_call() {
    // a guard on the guard: the matcher flags the calls it exists to catch
    let planted = "let y = x.sin() + f64::powf(x, 2.0);";
    let found: Vec<&&str> = BANNED
        .iter()
        .filter(|b| planted.contains(&format!(".{b}(")) || planted.contains(&format!("f64::{b}(")))
        .collect();
    assert_eq!(found.len(), 2);
}

// REQ: OA-MATH-019
#[test]
fn manifest_has_no_dependencies() {
    let toml =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let after = toml.split("[dependencies]").nth(1).unwrap();
    assert!(
        after
            .lines()
            .all(|l| l.trim().is_empty() || l.trim().starts_with('#')),
        "oa-math must have no dependencies, found: {after}"
    );
    assert!(!toml.contains("dev-dependencies") && !toml.contains("build-dependencies"));
}

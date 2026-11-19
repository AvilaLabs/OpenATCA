//! The crate source must not call anything that reaches the platform maths library.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

/// Method or associated-function names that are not guaranteed correctly rounded by IEEE-754
/// (or that some targets lower to a libm call), and the methods `core` lacks, which the crate
/// reaches through `oa_ieee` instead (`sqrt`, `floor`, `ceil`, `trunc`, `round`). `abs` and bit
/// casts are allowed.
const BANNED: &[&str] = &[
    "sqrt",
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

/// The lines that use `%` on integers (bit positions in the Payne-Hanek code).
const INTEGER_REM: &[&str] = &["let s = pos % 64;", "let s = lo % 64;", "let s = lz % 64;"];

/// The one float `%` allowed: `fmod` is an exact operation, and `rem_euclid` is built on it.
const EXACT_FLOAT_REM: &str = "let r = a % b;";

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
    // how many times the one allowed float remainder appears, in rem.rs only
    let mut exact_rem = 0;
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
            // A bare `f64::sin` used as a value (`.map(f64::sin)`) reaches libm too.
            for b in BANNED {
                for ty in ["f64::", "f32::"] {
                    let pat = format!("{ty}{b}");
                    for (at, _) in code.match_indices(&pat) {
                        let next = code[at + pat.len()..].chars().next();
                        if !next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            hits.push(format!("{name}:{}: {pat} as a value or call", n + 1));
                        }
                    }
                }
            }
            // `%` on floats lowers to fmod. The only `%` allowed are the integer shifts below
            // and the exact `fmod` of `rem_euclid`.
            if name == "rem.rs" && code.trim() == EXACT_FLOAT_REM {
                exact_rem += 1;
            } else if code.contains('%') && !INTEGER_REM.contains(&code.trim()) {
                hits.push(format!("{name}:{}: `%` (float remainder is fmod)", n + 1));
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
    // exactly one occurrence, so a second `a % b` in rem.rs, or a file named rem.rs elsewhere,
    // is not allowed by the same exception
    if exact_rem != 1 {
        hits.push(format!(
            "`{EXACT_FLOAT_REM}` appears {exact_rem} times in rem.rs, expected exactly 1"
        ));
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
fn the_check_also_finds_a_bare_path_and_a_float_remainder() {
    let value = "let v: Vec<f64> = xs.iter().copied().map(f64::sin).collect();";
    // the same matcher as the source scan: `f64::sin` followed by a non-identifier character
    let found = value.match_indices("f64::sin").any(|(at, m)| {
        let next = value[at + m.len()..].chars().next();
        !next.is_some_and(|c| c.is_alphanumeric() || c == '_')
    });
    assert!(found);
    assert!(!INTEGER_REM.contains(&"let r = x % 2.0;"));
    // the one allowed float remainder is that exact line only, and only in rem.rs
    assert_ne!("let r = x % 2.0;", EXACT_FLOAT_REM);
}

// REQ: OA-MATH-019
#[test]
fn manifest_depends_only_on_oa_ieee() {
    let toml =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    // the [dependencies] table only, up to the next table header
    let after = toml.split("[dependencies]").nth(1).unwrap();
    let after = after.split("\n[").next().unwrap();
    assert!(
        after.lines().all(|l| l.trim().is_empty()
            || l.trim().starts_with('#')
            || l.trim() == "oa-ieee = { path = \"../oa-ieee\" }"),
        "oa-math may depend only on oa-ieee, found: {after}"
    );
    assert!(!toml.contains("dev-dependencies") && !toml.contains("build-dependencies"));
}

// REQ: OA-MATH-019
#[test]
fn the_library_is_no_std_outside_tests() {
    let lib =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![cfg_attr(not(test), no_std)]"));
    assert!(lib.contains("#![forbid(unsafe_code)]"));
}

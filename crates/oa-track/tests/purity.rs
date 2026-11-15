//! Production sources must not call the platform maths library (RULESET B6): its results differ
//! between machines, which breaks bit-exact replay. The elementary functions come from `oa-math`.
//! `sqrt`, `abs`, `min`, `max`, `floor`, `ceil`, `round`, `trunc`, `clamp`, `rem_euclid` and the
//! like are exact under IEEE-754 and stay; `to_radians` and `to_degrees` are one multiply by a
//! constant of the standard library and stay. Test code (from the first line-start
//! `#[cfg(test)]`) may use `std`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

/// Names that reach the platform libm (or are a fused multiply-add, whose result depends on
/// the target's hardware support).
const BANNED: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sinh", "cosh", "tanh", "asinh", "acosh",
    "atanh", "exp", "exp2", "exp_m1", "ln", "ln_1p", "log", "log2", "log10", "powf", "powi",
    "cbrt", "hypot", "mul_add", "sin_cos", "abs_sub",
];

/// The banned calls in `text`, as (line, name), outside comments and after the cut for test code.
fn hits(text: &str) -> Vec<(usize, String)> {
    let mut body = text;
    for (pos, _) in text.match_indices("#[cfg(test)]") {
        if pos == 0 || text.as_bytes().get(pos - 1) == Some(&b'\n') {
            body = &text[..pos];
            break;
        }
    }
    let mut out = Vec::new();
    for (n, line) in body.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        for name in BANNED {
            for sep in [".", "::"] {
                let needle = format!("{sep}{name}(");
                let mut from = 0;
                while let Some(at) = code[from..].find(&needle) {
                    let abs = from + at;
                    from = abs + needle.len();
                    // `oa_math::sin(` is the replacement, not a hit.
                    if sep == "::" && code[..abs].ends_with("oa_math") {
                        continue;
                    }
                    out.push((n + 1, (*name).to_string()));
                }
            }
        }
    }
    out
}

// REQ: OA-TRK-293
#[test]
fn production_sources_call_no_platform_maths() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut bad = Vec::new();
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            files += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            for (line, name) in hits(&text) {
                bad.push(format!("{}:{line}: {name}", path.display()));
            }
        }
    }
    assert!(files > 3, "no sources found in {}", dir.display());
    assert!(
        bad.is_empty(),
        "platform maths in production code:\n{}",
        bad.join("\n")
    );
}

// REQ: OA-TRK-293
#[test]
fn the_scan_finds_a_call_and_skips_comments_replacements_and_test_code() {
    let src = "fn f(x: f64) -> f64 {\n    let a = x.sin();\n    let b = f64::powi(x, 2);\n    \
               let c = oa_math::cos(x); // x.exp()\n    a + b + c + x.sqrt()\n}\n\
               #[cfg(test)]\nmod t { fn g(x: f64) -> f64 { x.tan() } }\n";
    let h = hits(src);
    assert_eq!(h, vec![(2, "sin".to_string()), (3, "powi".to_string())]);
}

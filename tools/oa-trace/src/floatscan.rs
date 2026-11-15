//! Raw float-from-external-input scan (OA-TRC-103).
//!
//! A float read from file bytes, text, arguments or settings is checked finite (and in range where
//! the field has one) where it is parsed. This scan lists every line of `crates/*/src` that builds
//! a float from text or bytes by hand, so a new one is reviewed: it goes through the crate's checked
//! helper, or it is added to [`ALLOW`] with the reason it is safe. Test modules (everything from a
//! `#[cfg(test)]` line on, and files named `tests.rs` or `*_tests.rs`) are not scanned.

use std::fs;
use std::path::{Path, PathBuf};

/// Text that marks a raw float read.
pub const PATTERNS: &[&str] = &[
    "parse::<f64>",
    "parse::<f32>",
    "f64::from_str",
    "f32::from_str",
    "f64::from_bits",
    "f32::from_bits",
    "f64::from_le_bytes",
    "f32::from_le_bytes",
    "f64::from_be_bytes",
    "f32::from_be_bytes",
    "f64::from_ne_bytes",
    "f32::from_ne_bytes",
];

/// One reviewed raw read: any line of `file` that contains `needle` is allowed.
// A7.2: cold struct, size guard waived
pub struct Allow {
    /// Path relative to the workspace root, with `/` separators.
    pub file: &'static str,
    pub needle: &'static str,
    /// Why the value cannot reach the program as NaN, infinity or an out-of-range number.
    pub reason: &'static str,
}

/// A raw read that is neither allowed nor in a test module.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Finding {
    pub file: String,
    /// 1-based.
    pub line: usize,
    pub text: String,
}

/// Whether `lines[i]` is a raw float read: a pattern on the line, or an untyped `.parse()` with
/// `f64` or `f32` on the line or the two before it (the type that `parse` infers).
fn is_raw(lines: &[&str], i: usize) -> bool {
    let Some(&line) = lines.get(i) else {
        return false;
    };
    if PATTERNS.iter().any(|p| line.contains(p)) {
        return true;
    }
    if !line.contains(".parse()") {
        return false;
    }
    let from = i.saturating_sub(2);
    lines
        .get(from..=i)
        .unwrap_or(&[])
        .iter()
        .any(|l| l.contains("f64") || l.contains("f32"))
}

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "tests.rs" || n.ends_with("_tests.rs"))
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            collect(&path, out);
        } else if ft.is_file() && path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The raw reads in `crates/*/src` under `root` that `allow` does not cover, in path then line
/// order, and the entries of `allow` that matched no line (stale).
pub fn scan(root: &Path, allow: &[Allow]) -> (Vec<Finding>, Vec<String>) {
    let mut files = Vec::new();
    let mut crates: Vec<PathBuf> = fs::read_dir(root.join("crates"))
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    crates.sort();
    for c in &crates {
        collect(&c.join("src"), &mut files);
    }
    files.sort();
    let mut used = vec![false; allow.len()];
    let mut found = Vec::new();
    for path in &files {
        if is_test_file(path) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let lines: Vec<&str> = text.lines().collect();
        let end = lines
            .iter()
            .position(|l| l.trim() == "#[cfg(test)]")
            .unwrap_or(lines.len());
        let lines = lines.get(..end).unwrap_or(&[]);
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") || !is_raw(lines, i) {
                continue;
            }
            let mut ok = false;
            for (k, a) in allow.iter().enumerate() {
                if a.file == rel && line.contains(a.needle) {
                    if let Some(u) = used.get_mut(k) {
                        *u = true;
                    }
                    ok = true;
                }
            }
            if !ok {
                found.push(Finding {
                    file: rel.clone(),
                    line: i + 1,
                    text: line.trim().to_string(),
                });
            }
        }
    }
    let stale = allow
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|(a, _)| format!("{}: {:?}", a.file, a.needle))
        .collect();
    (found, stale)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("oa-trace-floats-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join("crates/oa-x/src")).unwrap();
        p
    }

    // REQ: OA-TRC-103
    #[test]
    fn raw_reads_are_found_and_allowlisted_ones_are_not() {
        let root = tmp("a");
        let src = "fn a(s: &str) -> f64 { s.parse::<f64>().unwrap_or(0.0) }\n\
                   fn b(b: [u8; 8]) -> f64 { f64::from_le_bytes(b) }\n\
                   fn c(s: &str) -> f32 { f32::from_bits(s.len() as u32) }\n\
                   fn d(s: &str) {\n    let v: f64 = s\n        .parse()\n        .unwrap_or(0.0);\n}\n\
                   fn e(s: &str) -> Option<u32> { s.parse().ok() }\n\
                   // f64::from_bits in a comment\n\
                   #[cfg(test)]\nmod t { fn x(s: &str) -> f64 { s.parse::<f64>().unwrap() } }\n";
        fs::write(root.join("crates/oa-x/src/lib.rs"), src).unwrap();
        fs::write(root.join("crates/oa-x/src/tests.rs"), src).unwrap();
        let (found, stale) = scan(&root, &[]);
        let lines: Vec<usize> = found.iter().map(|f| f.line).collect();
        assert_eq!(lines, [1, 2, 3, 6], "{found:?}");
        assert!(stale.is_empty());
        let allow = [
            Allow {
                file: "crates/oa-x/src/lib.rs",
                needle: "parse::<f64>",
                reason: "test",
            },
            Allow {
                file: "crates/oa-x/src/lib.rs",
                needle: "no such text",
                reason: "test",
            },
        ];
        let (found, stale) = scan(&root, &allow);
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(stale.len(), 1, "{stale:?}");
        let _ = fs::remove_dir_all(&root);
    }
}

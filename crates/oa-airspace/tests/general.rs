//! Workspace-wide rules that apply to this crate: no unsafe code, no outside dependencies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![crate_dir().join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("source");
                out.push((path.display().to_string(), text));
            }
        }
    }
    out.sort();
    out
}

// REQ: OA-GEN-001
#[test]
fn the_crate_and_its_binary_forbid_unsafe_code() {
    let files = sources();
    assert!(files.len() >= 10);
    let lib = files
        .iter()
        .find(|(n, _)| n.ends_with("src/lib.rs"))
        .expect("lib.rs");
    assert!(lib.1.starts_with("#![forbid(unsafe_code)]"));
    let bin = files
        .iter()
        .find(|(n, _)| n.ends_with("oa-adapt.rs"))
        .expect("binary");
    assert!(bin.1.contains("#![forbid(unsafe_code)]"));
    let keyword = ["un", "safe"].concat();
    for (name, text) in &files {
        for (i, l) in text.lines().enumerate() {
            let code = l.split("//").next().unwrap_or("");
            let uses = code
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|w| w == keyword);
            let allowed = code.contains("forbid(unsafe_code)");
            assert!(!uses || allowed, "{name}:{} uses the unsafe keyword", i + 1);
        }
    }
}

// REQ: OA-GEN-002
#[test]
fn the_crate_depends_only_on_workspace_crates_and_has_no_dev_dependencies() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml")).expect("manifest");
    let mut section = String::new();
    let mut deps = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        assert!(
            section != "[dev-dependencies]" && section != "[build-dependencies]",
            "no dev or build dependencies: {line}"
        );
        if section == "[dependencies]" {
            deps.push(line.to_string());
        }
    }
    assert!(!deps.is_empty());
    for d in &deps {
        let name = d.split('=').next().unwrap_or("").trim();
        assert!(name.starts_with("oa-"), "{d}");
        assert!(
            d.contains("path = \"../"),
            "{d} must be a path dependency inside the workspace"
        );
        assert!(!d.contains("version") && !d.contains("git"), "{d}");
    }
}

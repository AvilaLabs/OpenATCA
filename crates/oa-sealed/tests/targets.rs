//! Every bin and example target of the workspace uses the sealed guard or is listed, with a
//! reason, in `exempt_targets.txt`. The targets are found from the manifests and the source
//! layout, not from a list kept here, so a new target with neither fails this test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The members of the root workspace.
fn members() -> Vec<PathBuf> {
    let text = fs::read_to_string(repo().join("Cargo.toml")).unwrap();
    let start = text.find("members = [").unwrap();
    let body = &text[start..];
    let body = &body[..body.find(']').unwrap()];
    body.split('"')
        .skip(1)
        .step_by(2)
        .map(|m| repo().join(m))
        .collect()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.map(|e| e.unwrap().path()) {
        if e.is_dir() {
            rs_files(&e, out);
        } else if e.extension().is_some_and(|x| x == "rs") {
            out.push(e);
        }
    }
}

struct Target {
    key: String,
    entry: PathBuf,
    /// The crate directory.
    krate: PathBuf,
    is_bin: bool,
}

fn targets() -> Vec<Target> {
    let mut out = Vec::new();
    for dir in members() {
        let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        let pkg = manifest
            .lines()
            .find_map(|l| l.strip_prefix("name = \"")?.strip_suffix('"'))
            .unwrap()
            .to_string();
        let mut add = |key: String, entry: PathBuf, is_bin: bool| {
            out.push(Target {
                key,
                entry,
                krate: dir.clone(),
                is_bin,
            });
        };
        // [[bin]] and [[example]] blocks: name and path.
        let mut declared = BTreeSet::new();
        for kind in ["bin", "example"] {
            let head = format!("[[{kind}]]");
            for block in manifest.split(&head).skip(1) {
                let block = block.split("\n[").next().unwrap();
                let get = |k: &str| {
                    block.lines().find_map(|l| {
                        l.trim()
                            .strip_prefix(k)?
                            .trim_start()
                            .strip_prefix("= \"")?
                            .strip_suffix('"')
                            .map(str::to_string)
                    })
                };
                let name = get("name").unwrap();
                let path = get("path").unwrap_or_else(|| format!("src/bin/{name}.rs"));
                declared.insert(dir.join(&path));
                add(format!("{kind}:{name}"), dir.join(path), kind == "bin");
            }
        }
        // Implicit targets.
        let main = dir.join("src/main.rs");
        if main.exists() && !declared.contains(&main) {
            add(format!("bin:{pkg}"), main, true);
        }
        for (sub, is_bin, kind) in [("src/bin", true, "bin"), ("examples", false, "example")] {
            let Ok(rd) = fs::read_dir(dir.join(sub)) else {
                continue;
            };
            for e in rd.map(|e| e.unwrap().path()) {
                let (name, entry) = if e.is_dir() {
                    (
                        e.file_name().unwrap().to_string_lossy().into_owned(),
                        e.join("main.rs"),
                    )
                } else {
                    (
                        e.file_stem().unwrap().to_string_lossy().into_owned(),
                        e.clone(),
                    )
                };
                if entry.exists() && !declared.contains(&entry) {
                    add(format!("{kind}:{name}"), entry, is_bin);
                }
            }
        }
    }
    out
}

fn uses_guard(t: &Target, all: &[Target]) -> bool {
    let mentions = |p: &Path| fs::read_to_string(p).unwrap().contains("oa_sealed");
    if !t.is_bin {
        return mentions(&t.entry);
    }
    // A bin: its entry file, and the crate's other source files that are not the entry of a
    // target (those are other programs of the crate).
    let entries: BTreeSet<&PathBuf> = all
        .iter()
        .filter(|o| o.krate == t.krate)
        .map(|o| &o.entry)
        .collect();
    let mut files = Vec::new();
    rs_files(&t.krate.join("src"), &mut files);
    files
        .iter()
        .filter(|f| **f == t.entry || !entries.contains(f))
        .any(|f| mentions(f))
}

fn exempt() -> BTreeMap<String, String> {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("exempt_targets.txt"))
        .unwrap();
    let mut m = BTreeMap::new();
    for l in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (k, reason) = l.split_once(' ').expect("`kind:name reason`");
        assert!(!reason.trim().is_empty(), "{k}: a reason is required");
        assert!(
            m.insert(k.to_string(), reason.to_string()).is_none(),
            "{k} twice"
        );
    }
    m
}

// REQ: OA-SEAL-006
#[test]
fn every_bin_and_example_target_uses_the_guard_or_is_exempt_with_a_reason() {
    let all = targets();
    assert!(all.len() > 20, "found only {} targets", all.len());
    let exempt = exempt();
    let mut bad = Vec::new();
    let mut seen = BTreeSet::new();
    for t in &all {
        let used = uses_guard(t, &all);
        seen.insert(t.key.clone());
        match (used, exempt.contains_key(&t.key)) {
            (false, false) => bad.push(format!("{}: neither uses oa_sealed nor is exempt", t.key)),
            (true, true) => bad.push(format!(
                "{}: exempt but uses oa_sealed (stale entry)",
                t.key
            )),
            _ => {}
        }
    }
    for k in exempt.keys() {
        if !seen.contains(k) {
            bad.push(format!("{k}: exempt but no such target"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

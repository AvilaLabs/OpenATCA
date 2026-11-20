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

/// The calls that open an input path without the guard.
const BARE: [&str; 4] = [
    "File::open(",
    "fs::read(",
    "fs::read_to_string(",
    "fs::read_dir(",
];

/// The comment that allows a bare call on its line or the line before: `// seal-allow: <why>`.
const ALLOW: &str = "// seal-allow:";

/// Lines of `text` that hold a bare call without an allow comment, stopping at the test module.
fn bare_calls(text: &str) -> Vec<(usize, String, bool)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("#[cfg(test)]") {
            break;
        }
        let code = l.split("//").next().unwrap_or("");
        if code.trim_start().starts_with("///") || !BARE.iter().any(|b| code.contains(b)) {
            continue;
        }
        let prev = i
            .checked_sub(1)
            .and_then(|j| lines.get(j))
            .copied()
            .unwrap_or("");
        let allowed = l.contains(ALLOW) || prev.trim_start().starts_with(ALLOW);
        out.push((i + 1, l.trim().to_string(), allowed));
    }
    out
}

/// The files the bare-call rule covers: the sources of every crate that has a target that is not
/// exempt (its `src/` without test modules, and `examples/`), and the library openers.
fn covered_files(all: &[Target], exempt: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut crates = BTreeSet::new();
    for t in all {
        if !exempt.contains_key(&t.key) {
            crates.insert(t.krate.clone());
        }
    }
    for lib in OPENER_CRATES {
        crates.insert(repo().join("crates").join(lib));
    }
    let mut files = Vec::new();
    for k in crates {
        for sub in ["src", "examples"] {
            rs_files(&k.join(sub), &mut files);
        }
    }
    files.retain(|f| {
        let name = f.file_name().unwrap().to_string_lossy();
        !name.starts_with("tests") && !f.components().any(|c| c.as_os_str() == "tests")
    });
    files.sort();
    files
}

/// The crates whose library functions open a path they are given.
const OPENER_CRATES: [&str; 4] = ["oa-zip", "oa-nasr", "oa-dof", "oa-tiff"];

// REQ: OA-SEAL-006
#[test]
fn a_bare_open_or_read_of_an_input_path_in_a_tool_or_a_library_opener_needs_an_allow_comment() {
    let all = targets();
    let exempt = exempt();
    let mut bad = Vec::new();
    let mut allowed = 0;
    for f in covered_files(&all, &exempt) {
        let text = fs::read_to_string(&f).unwrap();
        for (n, line, ok) in bare_calls(&text) {
            if ok {
                allowed += 1;
            } else {
                bad.push(format!(
                    "{}:{n}: {line}",
                    f.strip_prefix(repo()).unwrap().display()
                ));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "a bare open or read needs `{ALLOW} <why>` (a write, a temp dir, a /proc file) or the guard:\n{bad:#?}"
    );
    assert!(
        allowed <= 40,
        "the allow list has grown to {allowed}; keep it small"
    );
}

// REQ: OA-SEAL-006
#[test]
fn the_bare_call_scan_finds_a_call_and_honours_the_allow_comment_and_the_test_module() {
    let text = "fn a() { let f = File::open(p); }\n// seal-allow: temp dir\nlet g = fs::read(q);\nlet h = fs::read_dir(d); // seal-allow: output\n/// doc: fs::read(x)\nlet i = oa_sealed::open(p);\n#[cfg(test)]\nmod t { fn x() { File::open(z); } }\n";
    let got = bare_calls(text);
    let flags: Vec<(usize, bool)> = got.iter().map(|(n, _, ok)| (*n, *ok)).collect();
    assert_eq!(flags, [(1, false), (3, true), (4, true)]);
}

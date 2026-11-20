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

/// The calls that open, read, copy or run something from a path without the guard.
const BARE: [&str; 10] = [
    "File::open(",
    "File::open_buffered(",
    "File::options(",
    "OpenOptions",
    "fs::read(",
    "fs::read_to_string(",
    "fs::read_dir(",
    "fs::copy(",
    "fs::hard_link(",
    "Command::new(",
];

/// The comment that allows a bare call on its line or within the two lines before:
/// `// seal-allow: <why>`.
const ALLOW: &str = "// seal-allow:";

/// The allow comments in the sources the rule covers, pinned exactly: adding one is a decision
/// made here, not slack in a budget.
const ALLOW_COUNT: usize = 42;

/// `line` with the string literals emptied and any `//` comment cut off.
fn code_of(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => {
                    in_str = false;
                    out.push('"');
                }
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push('"');
            }
            // A char literal such as '"' is not a string start.
            '\'' => {
                let rest: String = chars.clone().take(3).collect();
                if rest.chars().nth(1) == Some('\'') || rest.starts_with("\\") {
                    // Skip the literal: `'x'` or `'\x'`.
                    let mut n = 0;
                    for d in chars.by_ref() {
                        n += 1;
                        if d == '\'' && n > 0 {
                            break;
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            '/' if chars.peek() == Some(&'/') => break,
            _ => out.push(c),
        }
    }
    out
}

/// What `use std::fs...` brings in: the names to call bare (`read`, `read_to_string`, ...,
/// under any alias), the names of `File` and `OpenOptions`, and aliases of the module itself.
#[derive(Default, Debug)]
struct FsImports {
    funcs: Vec<String>,
    types: Vec<String>,
    modules: Vec<String>,
}

fn fs_imports(text: &str) -> FsImports {
    let mut imp = FsImports::default();
    let mut rest = text;
    while let Some(i) = rest.find("use std::fs") {
        rest = &rest[i + "use std::fs".len()..];
        let stmt = rest.split(';').next().unwrap_or("");
        let stmt = stmt.trim();
        if let Some(alias) = stmt.strip_prefix("as ") {
            imp.modules.push(alias.trim().to_string());
            continue;
        }
        let Some(items) = stmt.strip_prefix("::") else {
            continue;
        };
        let items = items.trim().trim_start_matches('{').trim_end_matches('}');
        for item in items.split(',') {
            let item = item.trim();
            let (name, alias) = match item.split_once(" as ") {
                Some((n, a)) => (n.trim(), a.trim()),
                None => (item, item),
            };
            match name {
                "read" | "read_to_string" | "read_dir" | "copy" | "hard_link" => {
                    imp.funcs.push(alias.to_string());
                }
                "File" | "OpenOptions" => imp.types.push(alias.to_string()),
                "self" if alias != "self" => imp.modules.push(alias.to_string()),
                _ => {}
            }
        }
    }
    imp
}

/// Whether `code` calls `name(` as a bare identifier (not a method, not a path tail).
fn calls_bare(code: &str, name: &str) -> bool {
    let pat = format!("{name}(");
    let mut from = 0;
    while let Some(i) = code[from..].find(&pat) {
        let at = from + i;
        let before = code[..at].chars().next_back();
        let ok = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.' || c == ':'));
        if ok {
            return true;
        }
        from = at + pat.len();
    }
    false
}

/// Whether the code line holds a bare call, given the file's `use std::fs` imports.
fn is_bare(code: &str, imp: &FsImports) -> bool {
    if BARE.iter().any(|b| code.contains(b)) {
        return true;
    }
    if imp.funcs.iter().any(|f| calls_bare(code, f)) {
        return true;
    }
    imp.types.iter().any(|t| {
        code.contains(&format!("{t}::open("))
            || code.contains(&format!("{t}::options("))
            || code.contains(&format!("{t}::new("))
    }) || imp.modules.iter().any(|m| {
        [
            "read(",
            "read_to_string(",
            "read_dir(",
            "copy(",
            "hard_link(",
        ]
        .iter()
        .any(|f| code.contains(&format!("{m}::{f}")))
    })
}

/// One bare call: line number, the trimmed line, and whether an allow comment covers it.
type Call = (usize, String, bool);

/// What the scan of one file found: the bare calls, and the allow comments that name no call
/// within two lines or give no reason.
struct Scan {
    calls: Vec<Call>,
    allows: usize,
    stale: Vec<(usize, String)>,
}

/// Lines of `text` that hold a bare call, with whether an allow comment on the line or within the
/// two lines before covers it. A `#[cfg(test)]` followed by a `mod` item is skipped with its
/// body; any other `#[cfg(test)]` item is scanned.
fn scan(text: &str) -> Scan {
    let lines: Vec<&str> = text.lines().collect();
    let imp = fs_imports(text);
    let mut skip_depth: Option<i64> = None;
    let mut pending_cfg_test = false;
    let mut attr_depth = 0i64;
    let mut calls: Vec<Call> = Vec::new();
    let mut allow_at: Vec<usize> = Vec::new();
    let mut call_lines: Vec<usize> = Vec::new();
    let mut bad_reason: Vec<(usize, String)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(pos) = l.find(ALLOW) {
            // An allow is a comment, not text inside a string.
            if l[..pos].matches('"').count() % 2 == 0 {
                allow_at.push(i);
                if l[pos + ALLOW.len()..].trim().is_empty() {
                    bad_reason.push((i + 1, "the allow has no reason".to_string()));
                }
            }
        }
        let code = code_of(l);
        let t = code.trim();
        if let Some(depth) = skip_depth.as_mut() {
            *depth += code.matches('{').count() as i64 - code.matches('}').count() as i64;
            if *depth <= 0 {
                skip_depth = None;
            }
            continue;
        }
        if t.starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            continue;
        }
        // Attributes between the cfg and the item, also over several lines.
        if pending_cfg_test && (attr_depth > 0 || t.starts_with("#[")) {
            attr_depth += code.matches('[').count() as i64 - code.matches(']').count() as i64;
            continue;
        }
        if pending_cfg_test && !t.is_empty() {
            pending_cfg_test = false;
            if t.starts_with("mod ") || t.contains(" mod ") {
                let depth = code.matches('{').count() as i64 - code.matches('}').count() as i64;
                if depth > 0 || !t.ends_with(';') {
                    skip_depth = Some(depth);
                    if depth <= 0 && t.ends_with('}') {
                        skip_depth = None;
                    }
                }
                continue;
            }
        }
        if t.starts_with("///") || t.starts_with("//!") || t.starts_with("use ") {
            continue;
        }
        if is_bare(&code, &imp) {
            call_lines.push(i);
            let allowed = (i.saturating_sub(2)..=i).any(|j| allow_at.contains(&j));
            calls.push((i + 1, l.trim().to_string(), allowed));
        }
    }
    let mut stale: Vec<(usize, String)> = bad_reason;
    for &a in &allow_at {
        if !call_lines.iter().any(|&c| (a..=a + 2).contains(&c)) {
            stale.push((
                a + 1,
                "an allow with no bare call on its line or the two after it (stale)".to_string(),
            ));
        }
    }
    Scan {
        calls,
        allows: allow_at.len(),
        stale,
    }
}

/// Tools that read only the repository's own source and register files, never data, with the
/// reason; their sources are not scanned.
const SOURCE_TOOLS: [(&str, &str); 1] = [(
    "oa-trace",
    "walks the repository's own sources and requirement registers, and reads no input data",
)];

/// The files the bare-call rule covers: `src/` and `examples/` of every member crate (core
/// crates included), without test modules, and the accepted exceptions are checked apart.
fn covered_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for k in members() {
        let name = k.file_name().unwrap().to_string_lossy().into_owned();
        if EXCEPTION_CRATES.iter().any(|(n, _)| *n == name)
            || SOURCE_TOOLS.iter().any(|(n, _)| *n == name)
        {
            continue;
        }
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

// REQ: OA-SEAL-006
#[test]
fn a_bare_open_or_read_in_any_crate_needs_an_allow_comment_with_a_reason_and_no_allow_is_stale() {
    let mut bad = Vec::new();
    let mut allows = 0;
    for f in covered_files() {
        let text = fs::read_to_string(&f).unwrap();
        let sc = scan(&text);
        allows += sc.allows;
        let rel = f.strip_prefix(repo()).unwrap().display().to_string();
        for (n, line, ok) in sc.calls {
            if !ok {
                bad.push(format!("{rel}:{n}: {line}"));
            }
        }
        for (n, why) in sc.stale {
            bad.push(format!("{rel}:{n}: {why}"));
        }
    }
    assert!(
        bad.is_empty(),
        "a bare open, read, copy, link or command needs `{ALLOW} <why>` (a write, a temp dir, a /proc file) or the guard:\n{bad:#?}"
    );
    assert_eq!(
        allows, ALLOW_COUNT,
        "the allow comments are pinned exactly: {allows} found, {ALLOW_COUNT} pinned; change ALLOW_COUNT with the allow"
    );
}

// REQ: OA-SEAL-006
#[test]
fn the_bare_call_scan_finds_calls_imports_and_aliases_and_honours_allows_strings_and_test_modules()
{
    let text = "fn a() { let f = File::open(p); }\n// seal-allow: temp dir\nlet g = fs::read(q);\nlet h = fs::read_dir(d); // seal-allow: output\n/// doc: fs::read(x)\nlet i = oa_sealed::open(p);\nlet s = \"fs::read( // not code\";\n#[cfg(test)]\nmod t { fn x() { File::open(z); } }\nlet j = OpenOptions::new();\nlet k = std::process::Command::new(\"x\");\n";
    let sc = scan(text);
    let flags: Vec<(usize, bool)> = sc.calls.iter().map(|(n, _, ok)| (*n, *ok)).collect();
    assert_eq!(
        flags,
        [(1, false), (3, true), (4, true), (10, false), (11, false)]
    );
    assert_eq!(sc.allows, 2);
    assert!(sc.stale.is_empty(), "{:?}", sc.stale);
    // `use std::fs::{...}` imports and an alias are caught.
    let text = "use std::fs::{read_to_string as slurp, File as F};\nuse std::fs as filesystem;\nfn a() {\n let a = slurp(p);\n let b = F::open(q);\n let c = filesystem::read(r);\n let d = x.read_to_string();\n}\n";
    let lines: Vec<usize> = scan(text).calls.iter().map(|c| c.0).collect();
    assert_eq!(lines, [4, 5, 6]);
    // A cfg(test) item that is not a module is scanned; the test module after it is not.
    let text = "#[cfg(test)]\nfn helper() { fs::read(p); }\n#[cfg(test)]\nmod tests {\n fn t() { fs::read(q); }\n}\nfn live() { fs::read(r); }\n";
    let lines: Vec<usize> = scan(text).calls.iter().map(|c| c.0).collect();
    assert_eq!(lines, [2, 7]);
    // A stale allow and an allow without a reason are reported.
    let text = "// seal-allow: gone\nlet a = 1;\nlet b = 2;\nlet c = 3;\n// seal-allow:\nlet d = fs::read(p);\n";
    let sc = scan(text);
    assert_eq!(sc.stale.len(), 2, "{:?}", sc.stale);
}

/// The accepted exceptions: oa-arinc424 must stay free of dependencies (so it cannot use
/// oa-sealed) and oa-geo is a core crate (core crates do not depend on the shell crate oa-sealed).
/// Their opens are in tests only, on fixtures and on the env-named inputs `OPENATCA_CIFP` and
/// `OPENATCA_GEODTEST`; each file and its count of bare calls is pinned here.
const EXCEPTION_CRATES: [(&str, &[(&str, usize)]); 2] = [
    (
        "oa-arinc424",
        &[
            ("tests/cifp_full.rs", 1),
            ("tests/common/mod.rs", 1),
            ("tests/file_and_limits.rs", 3),
            ("tests/nasr_cross.rs", 2),
        ],
    ),
    (
        "oa-geo",
        &[
            ("tests/general.rs", 3),
            ("tests/geodesic.rs", 1),
            ("tests/purity.rs", 2),
        ],
    ),
];

// REQ: OA-SEAL-006
#[test]
fn the_arinc424_and_geo_exceptions_gain_no_bare_open_beyond_the_pinned_test_lines() {
    for (krate, pinned) in EXCEPTION_CRATES {
        let root = repo().join("crates").join(krate);
        let mut files = Vec::new();
        for sub in ["src", "examples", "tests", "benches"] {
            rs_files(&root.join(sub), &mut files);
        }
        let mut found: BTreeMap<String, usize> = BTreeMap::new();
        for f in files {
            // A test module inside a source file is not cut off here: every bare call counts.
            let text = std::fs::read_to_string(&f).unwrap();
            let n = text
                .lines()
                .filter(|l| {
                    let code = l.split("//").next().unwrap_or("");
                    !code.trim_start().starts_with("///") && BARE.iter().any(|b| code.contains(b))
                })
                .count();
            if n > 0 {
                let rel = f.strip_prefix(&root).unwrap().to_string_lossy().to_string();
                found.insert(rel, n);
            }
        }
        let want: BTreeMap<String, usize> =
            pinned.iter().map(|(f, n)| (f.to_string(), *n)).collect();
        assert_eq!(
            found, want,
            "{krate}: a bare open or read was added (accepted exceptions are pinned, OA-SEAL-006)"
        );
    }
}

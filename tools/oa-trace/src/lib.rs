#![forbid(unsafe_code)]
//! oa-trace — requirements traceability checker (OA-TRC-001). See docs/M0_DESIGN.md.
//!
//! Requirements are list items `- **OA-XXX-NNN**` in `REQUIREMENTS.md`. Tests carry a
//! `// REQ: ID[, ID...]` line directly above `#[test]`. [`analyze`] joins the two.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A requirement as defined in the register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub id: String,
    /// 1-based line in the register.
    pub line: usize,
    pub withdrawn: bool,
}

/// A test that names a requirement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TestRef {
    /// Path relative to the root, with `/` separators.
    pub file: String,
    pub line: usize,
    pub name: String,
}

/// A tag problem (malformed ID, or not followed by `#[test]`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TagError {
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// Result of scanning a workspace.
#[derive(Debug, Default)]
pub struct Report {
    /// Requirement ID -> tests naming it (sorted).
    pub covered: BTreeMap<String, Vec<TestRef>>,
    /// Active requirements with no test (sorted).
    pub untested: Vec<String>,
    /// Withdrawn requirements (sorted).
    pub withdrawn: Vec<String>,
    /// Unknown ID -> tests naming it.
    pub unknown: BTreeMap<String, Vec<TestRef>>,
    /// Warnings: tests naming a withdrawn requirement.
    pub withdrawn_tested: BTreeMap<String, Vec<TestRef>>,
    pub malformed: Vec<TagError>,
    /// Duplicate ID -> lines where it is defined.
    pub duplicates: BTreeMap<String, Vec<usize>>,
    /// I/O problems (unreadable files); treated as failures.
    pub io_errors: Vec<String>,
}

impl Report {
    /// True when the check passes.
    pub fn ok(&self) -> bool {
        self.untested.is_empty()
            && self.unknown.is_empty()
            && self.malformed.is_empty()
            && self.duplicates.is_empty()
            && self.io_errors.is_empty()
    }
}

/// True if `s` is `OA-<UPPER/DIGITS>+-<3 digits>`.
pub fn is_valid_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("OA-") else {
        return false;
    };
    let Some((area, num)) = rest.split_once('-') else {
        return false;
    };
    !area.is_empty()
        && area
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && num.len() == 3
        && num.bytes().all(|b| b.is_ascii_digit())
}

/// Parses requirement definitions out of register text.
pub fn parse_requirements(text: &str) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(rest) = line.trim_start().strip_prefix("- **") {
            if let Some((id, tail)) = rest.split_once("**") {
                if is_valid_id(id) {
                    out.push(Requirement {
                        id: id.to_string(),
                        line: i + 1,
                        withdrawn: tail.contains("WITHDRAWN"),
                    });
                    continue;
                }
            }
        }
        // Continuation line of the previous item (indented, non-empty).
        if line.starts_with(' ') && line.contains("WITHDRAWN") {
            if let Some(last) = out.last_mut() {
                last.withdrawn = true;
            }
        }
    }
    out
}

/// A tag found in a source file.
#[derive(Debug, PartialEq, Eq)]
pub struct Tag {
    pub line: usize,
    pub ids: Vec<String>,
    pub test_name: String,
}

/// Extracts tags from one source file; problems go to `errors`.
pub fn parse_tags(file: &str, text: &str, errors: &mut Vec<TagError>) -> Vec<Tag> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tags = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        let Some(list) = raw.trim().strip_prefix("// REQ:") else {
            continue;
        };
        let line = i + 1;
        let mut err = |message: String| {
            errors.push(TagError {
                file: file.to_string(),
                line,
                message,
            })
        };
        let mut ids = Vec::new();
        let mut bad = false;
        for tok in list.split(',') {
            let tok = tok.trim();
            if is_valid_id(tok) {
                ids.push(tok.to_string());
            } else {
                bad = true;
                err(format!("malformed requirement ID {tok:?}"));
            }
        }
        let followed = lines.get(i + 1).is_some_and(|l| l.trim() == "#[test]");
        if !followed {
            bad = true;
            err("tag is not directly followed by #[test]".to_string());
        }
        if bad {
            continue;
        }
        let name = lines[i + 2..]
            .iter()
            .take(8)
            .find_map(|l| fn_name(l))
            .unwrap_or_else(|| "<unknown>".to_string());
        tags.push(Tag {
            line,
            ids,
            test_name: name,
        });
    }
    tags
}

fn fn_name(line: &str) -> Option<String> {
    let t = line.trim();
    let idx = if t.starts_with("fn ") {
        0
    } else {
        t.find(" fn ")? + 1
    };
    let rest = &t[idx + 3..];
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>, io: &mut Vec<String>) {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            io.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let name = entry.file_name();
            if name == "target" || name == ".git" {
                continue;
            }
            collect_rs(&path, out, io);
        } else if ft.is_file() && path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Scans the workspace at `root` and joins requirements to tests.
pub fn analyze(root: &Path) -> Report {
    let mut rep = Report::default();
    let reg_path = root.join("REQUIREMENTS.md");
    let reqs = match fs::read(&reg_path) {
        Ok(b) => parse_requirements(&String::from_utf8_lossy(&b)),
        Err(e) => {
            rep.io_errors.push(format!("{}: {e}", reg_path.display()));
            Vec::new()
        }
    };
    let mut defs: BTreeMap<String, (Vec<usize>, bool)> = BTreeMap::new();
    for r in &reqs {
        let e = defs.entry(r.id.clone()).or_default();
        e.0.push(r.line);
        e.1 |= r.withdrawn;
    }
    for (id, (lines, _)) in &defs {
        if lines.len() > 1 {
            rep.duplicates.insert(id.clone(), lines.clone());
        }
    }

    let mut files = Vec::new();
    collect_rs(root, &mut files, &mut rep.io_errors);
    files.sort();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let text = match fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                rep.io_errors.push(format!("{rel}: {e}"));
                continue;
            }
        };
        for tag in parse_tags(&rel, &text, &mut rep.malformed) {
            for id in &tag.ids {
                let t = TestRef {
                    file: rel.clone(),
                    line: tag.line,
                    name: tag.test_name.clone(),
                };
                let bucket = match defs.get(id) {
                    None => &mut rep.unknown,
                    Some((_, true)) => &mut rep.withdrawn_tested,
                    Some(_) => &mut rep.covered,
                };
                let v = bucket.entry(id.clone()).or_default();
                if !v.contains(&t) {
                    v.push(t);
                }
            }
        }
    }
    for v in rep
        .covered
        .values_mut()
        .chain(rep.unknown.values_mut())
        .chain(rep.withdrawn_tested.values_mut())
    {
        v.sort();
    }
    rep.malformed.sort();
    for (id, (_, withdrawn)) in &defs {
        if *withdrawn {
            rep.withdrawn.push(id.clone());
        } else if !rep.covered.contains_key(id) {
            rep.untested.push(id.clone());
        }
    }
    rep
}

fn fmt_tests(out: &mut String, tests: &[TestRef]) {
    for t in tests {
        out.push_str(&format!("    {}:{} {}\n", t.file, t.line, t.name));
    }
}

/// Plain-text report, deterministic.
pub fn render_text(rep: &Report) -> String {
    let mut o = String::new();
    o.push_str(&format!(
        "Requirements with tests ({}):\n",
        rep.covered.len()
    ));
    for (id, tests) in &rep.covered {
        o.push_str(&format!("  {id}\n"));
        fmt_tests(&mut o, tests);
    }
    o.push_str(&format!(
        "\nRequirements WITHOUT tests ({}):\n",
        rep.untested.len()
    ));
    for id in &rep.untested {
        o.push_str(&format!("  {id}\n"));
    }
    o.push_str(&format!(
        "\nTags naming unknown IDs ({}):\n",
        rep.unknown.len()
    ));
    for (id, tests) in &rep.unknown {
        o.push_str(&format!("  {id}\n"));
        fmt_tests(&mut o, tests);
    }
    o.push_str(&format!("\nMalformed tags ({}):\n", rep.malformed.len()));
    for e in &rep.malformed {
        o.push_str(&format!("  {}:{}: {}\n", e.file, e.line, e.message));
    }
    o.push_str(&format!(
        "\nDuplicate requirement IDs ({}):\n",
        rep.duplicates.len()
    ));
    for (id, lines) in &rep.duplicates {
        o.push_str(&format!("  {id} defined at lines {lines:?}\n"));
    }
    if !rep.io_errors.is_empty() {
        o.push_str(&format!("\nI/O errors ({}):\n", rep.io_errors.len()));
        for e in &rep.io_errors {
            o.push_str(&format!("  {e}\n"));
        }
    }
    if !rep.withdrawn_tested.is_empty() {
        o.push_str("\nWarnings: tests naming WITHDRAWN requirements:\n");
        for (id, tests) in &rep.withdrawn_tested {
            o.push_str(&format!("  {id}\n"));
            fmt_tests(&mut o, tests);
        }
    }
    o.push_str(&format!(
        "\nResult: {}\n",
        if rep.ok() { "PASS" } else { "FAIL" }
    ));
    o
}

/// Markdown traceability matrix (requirement -> tests), deterministic.
pub fn render_markdown(rep: &Report) -> String {
    let mut o = String::from("# Traceability matrix\n\n");
    o.push_str("| Requirement | Status | Tests |\n|---|---|---|\n");
    let mut ids: Vec<&String> = rep
        .covered
        .keys()
        .chain(rep.untested.iter())
        .chain(rep.withdrawn.iter())
        .collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        if let Some(tests) = rep.covered.get(id) {
            let cell = tests
                .iter()
                .map(|t| format!("`{}` ({}:{})", t.name, t.file, t.line))
                .collect::<Vec<_>>()
                .join("<br>");
            o.push_str(&format!("| {id} | verified | {cell} |\n"));
        } else if rep.withdrawn.contains(id) {
            o.push_str(&format!("| {id} | withdrawn | |\n"));
        } else {
            o.push_str(&format!("| {id} | **NO TEST** | |\n"));
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "oa-trace-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const REG: &str =
        "- **OA-AAA-001** one\n- **OA-AAA-002** two\n  more\n- **OA-424-003** WITHDRAWN\n";

    fn tag(ids: &str, next: &str) -> String {
        format!("    // REQ: {ids}\n    {next}\n    fn t_one() {{}}\n")
    }

    // REQ: OA-TRC-001
    #[test]
    fn id_validation() {
        assert!(is_valid_id("OA-GEN-001"));
        assert!(is_valid_id("OA-424-007"));
        assert!(!is_valid_id("OA-gen-001"));
        assert!(!is_valid_id("OA-GEN-01"));
        assert!(!is_valid_id("OA--001"));
        assert!(!is_valid_id("GEN-001"));
        assert!(!is_valid_id(""));
    }

    // REQ: OA-TRC-001
    #[test]
    fn covered_untested_and_withdrawn() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", REG);
        t.write("a/src/lib.rs", &tag("OA-AAA-001", "#[test]"));
        t.write("target/x.rs", &tag("OA-AAA-002", "#[test]"));
        let r = analyze(&t.0);
        assert_eq!(r.covered["OA-AAA-001"][0].name, "t_one");
        assert_eq!(r.covered["OA-AAA-001"][0].file, "a/src/lib.rs");
        assert_eq!(r.untested, vec!["OA-AAA-002"]);
        assert_eq!(r.withdrawn, vec!["OA-424-003"]);
        assert!(!r.ok());
    }

    // REQ: OA-TRC-001
    #[test]
    fn all_covered_passes_and_withdrawn_is_warning() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", REG);
        t.write(
            "a.rs",
            &tag("OA-AAA-001, OA-AAA-002, OA-424-003", "#[test]"),
        );
        let r = analyze(&t.0);
        assert!(r.ok());
        assert!(r.withdrawn_tested.contains_key("OA-424-003"));
        assert!(render_text(&r).contains("Warnings"));
    }

    // REQ: OA-TRC-001
    #[test]
    fn unknown_and_malformed_tags() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", REG);
        t.write("a.rs", &tag("OA-ZZZ-009", "#[test]"));
        t.write("b.rs", &tag("bogus", "#[test]"));
        t.write("c.rs", &tag("OA-AAA-001", "#[ignore]"));
        t.write("d.rs", "// REQ:\n");
        let r = analyze(&t.0);
        assert!(r.unknown.contains_key("OA-ZZZ-009"));
        let files: Vec<&str> = r.malformed.iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, vec!["b.rs", "c.rs", "d.rs", "d.rs"]);
        assert!(!r.ok());
    }

    // REQ: OA-TRC-001
    #[test]
    fn duplicates_detected() {
        let t = Tmp::new();
        t.write(
            "REQUIREMENTS.md",
            "- **OA-AAA-001** x\n- **OA-AAA-001** y\n",
        );
        let r = analyze(&t.0);
        assert_eq!(r.duplicates["OA-AAA-001"], vec![1, 2]);
        assert!(!r.ok());
    }

    // REQ: OA-TRC-001
    #[test]
    fn missing_register_and_non_utf8_do_not_panic() {
        let t = Tmp::new();
        t.write("x.rs", "");
        fs::write(t.0.join("bad.rs"), [0xff, 0xfe, 0x00]).unwrap();
        let r = analyze(&t.0);
        assert!(!r.ok());
        assert!(!r.io_errors.is_empty());
    }

    // REQ: OA-TRC-001
    #[test]
    fn output_is_deterministic_and_markdown_lists_all() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", REG);
        t.write("b.rs", &tag("OA-AAA-001", "#[test]"));
        t.write("a.rs", &tag("OA-AAA-001", "#[test]"));
        let r1 = analyze(&t.0);
        let r2 = analyze(&t.0);
        assert_eq!(render_text(&r1), render_text(&r2));
        let md = render_markdown(&r1);
        assert!(md.contains("| OA-AAA-001 | verified | `t_one` (a.rs:1)<br>`t_one` (b.rs:1) |"));
        assert!(md.contains("| OA-AAA-002 | **NO TEST** |"));
        assert!(md.contains("| OA-424-003 | withdrawn |"));
    }

    // REQ: OA-TRC-001
    #[test]
    fn fn_name_forms() {
        assert_eq!(fn_name("pub async fn foo_1() {"), Some("foo_1".into()));
        assert_eq!(fn_name("fn "), None);
        assert_eq!(fn_name("let x = 1;"), None);
    }
}

#![forbid(unsafe_code)]
//! oa-trace — requirements traceability checker (OA-TRC-001). See docs/M0_DESIGN.md.
//!
//! Requirements are list items `- **OA-XXX-NNN**` in `REQUIREMENTS.md`. Rust tests carry a
//! `// REQ: ID[, ID...]` line directly above `#[test]`; Python tests in `test_*.py` modules carry
//! `# REQ: ID[, ID...]` directly above `def test_*`. A Kani proof harness carries its Rust tag directly
//! above `#[kani::proof]` (with an optional `#[kani::unwind(n)]` on either side of it) and
//! counts as a proof, reported apart from tests. [`analyze`] joins the two.

pub mod classcheck;
pub mod floatscan;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A requirement as defined in the register.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Requirement {
    pub id: String,
    /// 1-based line in the register.
    pub line: usize,
    pub withdrawn: bool,
}

/// A test or proof harness that names a requirement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
// A7.2: cold struct, size guard waived
pub struct TestRef {
    /// Path relative to the root, with `/` separators.
    pub file: String,
    pub line: usize,
    pub name: String,
    /// True for a Kani proof harness, false for a test.
    pub proof: bool,
}

/// A tag problem (malformed ID, or not followed by `#[test]` or `#[kani::proof]`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
// A7.2: cold struct, size guard waived
pub struct TagError {
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// Result of scanning a workspace.
#[derive(Debug, Default)]
// A7.2: cold struct, size guard waived
pub struct Report {
    /// Requirement ID -> tests and proofs naming it (sorted).
    pub covered: BTreeMap<String, Vec<TestRef>>,
    /// Active requirements with no test and no proof (sorted).
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
// A7.2: cold struct, size guard waived
pub struct Tag {
    pub line: usize,
    pub ids: Vec<String>,
    pub test_name: String,
    /// True when the tag sits on a Kani proof harness.
    pub proof: bool,
}

/// What the attribute lines after a tag make the item: a test, a proof harness, or neither.
fn tagged_kind(lines: &[&str], i: usize) -> Option<bool> {
    let next = |k: usize| lines.get(i + k).map(|l| l.trim());
    match next(1) {
        Some("#[test]") => Some(false),
        Some("#[kani::proof]") => Some(true),
        Some(l) if l.starts_with("#[kani::unwind(") && next(2) == Some("#[kani::proof]") => {
            Some(true)
        }
        _ => None,
    }
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
        let kind = tagged_kind(&lines, i);
        if kind.is_none() {
            bad = true;
            err("tag is not directly followed by #[test] or #[kani::proof]".to_string());
        }
        if bad {
            continue;
        }
        let name = lines
            .get(i + 2..)
            .unwrap_or(&[])
            .iter()
            .take(8)
            .find_map(|l| fn_name(l))
            .unwrap_or_else(|| "<unknown>".to_string());
        tags.push(Tag {
            line,
            ids,
            test_name: name,
            proof: kind == Some(true),
        });
    }
    tags
}

/// Extract strict Python unit-test tags from one test module.
fn parse_python_tags(file: &str, text: &str, errors: &mut Vec<TagError>) -> Vec<Tag> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tags = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        let Some(list) = raw.trim().strip_prefix("# REQ:") else {
            continue;
        };
        let line = i + 1;
        let mut bad = false;
        let mut ids = Vec::new();
        for tok in list.split(',') {
            let tok = tok.trim();
            if is_valid_id(tok) {
                ids.push(tok.to_string());
            } else {
                bad = true;
                errors.push(TagError {
                    file: file.to_string(),
                    line,
                    message: format!("malformed requirement ID {tok:?}"),
                });
            }
        }
        let next = lines.get(i + 1).map(|value| value.trim());
        let name = next.and_then(python_test_name);
        if name.is_none() {
            bad = true;
            errors.push(TagError {
                file: file.to_string(),
                line,
                message: "tag is not directly followed by def test_*".to_string(),
            });
        }
        if !bad {
            if let Some(test_name) = name {
                tags.push(Tag {
                    line,
                    ids,
                    test_name,
                    proof: false,
                });
            }
        }
    }
    tags
}

fn python_test_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("def ")?;
    let name = rest.split_once('(')?.0;
    if name.starts_with("test_")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Some(name.to_string())
    } else {
        None
    }
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

fn collect_test_sources(dir: &Path, out: &mut Vec<PathBuf>, io: &mut Vec<String>) {
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
            if name == "target" || name == ".git" || name == "__pycache__" {
                continue;
            }
            collect_test_sources(&path, out, io);
        } else if ft.is_file()
            && (path.extension().is_some_and(|e| e == "rs")
                || (path.extension().is_some_and(|e| e == "py")
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("test_"))))
        {
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
    collect_test_sources(root, &mut files, &mut rep.io_errors);
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
        let tags = if path.extension().is_some_and(|ext| ext == "py") {
            parse_python_tags(&rel, &text, &mut rep.malformed)
        } else {
            parse_tags(&rel, &text, &mut rep.malformed)
        };
        for tag in tags {
            for id in &tag.ids {
                let t = TestRef {
                    file: rel.clone(),
                    line: tag.line,
                    name: tag.test_name.clone(),
                    proof: tag.proof,
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
        let kind = if t.proof { " (proof)" } else { "" };
        out.push_str(&format!("    {}:{} {}{kind}\n", t.file, t.line, t.name));
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
        "\nTraceability (every requirement linked, ids unique; no test or proof run): {}\n",
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
                .map(|t| {
                    let kind = if t.proof { "proof " } else { "" };
                    format!("{kind}`{}` ({}:{})", t.name, t.file, t.line)
                })
                .collect::<Vec<_>>()
                .join("<br>");
            let status = match (
                tests.iter().any(|t| !t.proof),
                tests.iter().any(|t| t.proof),
            ) {
                (true, true) => "test and proof harness linked",
                (false, true) => "proof harness linked",
                _ => "test linked",
            };
            o.push_str(&format!("| {id} | {status} | {cell} |\n"));
        } else if rep.withdrawn.contains(id) {
            o.push_str(&format!("| {id} | withdrawn | |\n"));
        } else {
            o.push_str(&format!("| {id} | **NO TEST** | |\n"));
        }
    }
    o
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
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
        assert!(md.contains("| OA-AAA-001 | test linked | `t_one` (a.rs:1)<br>`t_one` (b.rs:1) |"));
        assert!(md.contains("| OA-AAA-002 | **NO TEST** |"));
        assert!(md.contains("| OA-424-003 | withdrawn |"));
    }

    // REQ: OA-TRC-100
    #[test]
    fn kani_proof_tags_are_proofs() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", REG);
        t.write(
            "a.rs",
            &tag("OA-AAA-001", "#[kani::proof]").replace("t_one", "p_one"),
        );
        t.write(
            "b.rs",
            "// REQ: OA-AAA-001, OA-AAA-002\n#[kani::unwind(9)]\n#[kani::proof]\nfn p_two() {}\n",
        );
        t.write(
            "c.rs",
            "// REQ: OA-AAA-002\n#[kani::proof]\n#[kani::unwind(9)]\nfn p_three() {}\n",
        );
        // An unwind bound with no proof after it is not a harness.
        t.write(
            "d.rs",
            "// REQ: OA-AAA-001\n#[kani::unwind(9)]\nfn x() {}\n",
        );
        t.write(
            "e.rs",
            &tag("OA-AAA-001", "#[test]").replace("t_one", "t_two"),
        );
        let r = analyze(&t.0);
        assert_eq!(r.malformed.len(), 1);
        assert_eq!(r.malformed[0].file, "d.rs");
        let a = &r.covered["OA-AAA-001"];
        assert_eq!(a.iter().filter(|x| x.proof).count(), 2);
        assert_eq!(a.iter().filter(|x| !x.proof).count(), 1);
        assert!(r.covered["OA-AAA-002"].iter().all(|x| x.proof));
        let md = render_markdown(&r);
        assert!(md.contains("| OA-AAA-001 | test and proof harness linked |"));
        assert!(md.contains(
            "| OA-AAA-002 | proof harness linked | proof `p_two` (b.rs:1)<br>proof `p_three` (c.rs:1) |"
        ));
        assert!(render_text(&r).contains("p_two (proof)"));
    }

    // REQ: OA-TRC-104
    #[test]
    fn python_test_modules_link_direct_tags_as_tests() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", "- **OA-AAA-001** one\n");
        t.write(
            "test_routes.py",
            "# REQ: OA-AAA-001\ndef test_source_backed_route_evidence():\n    pass\n",
        );
        // Helpers are deliberately outside the collector's Python test-module pattern.
        t.write(
            "route_helpers.py",
            "# REQ: OA-AAA-001\ndef test_helper_must_not_link():\n    pass\n",
        );
        let report = analyze(&t.0);
        assert!(
            report.covered.contains_key("OA-AAA-001"),
            "Python test tag was not linked"
        );
        let linked = &report.covered["OA-AAA-001"];
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].file, "test_routes.py");
        assert_eq!(linked[0].name, "test_source_backed_route_evidence");
        assert!(!linked[0].proof);
        assert!(report.malformed.is_empty());
    }

    // REQ: OA-TRC-104
    #[test]
    fn python_tags_reject_invalid_ids_non_tests_and_nonadjacent_defs() {
        let t = Tmp::new();
        t.write(
            "REQUIREMENTS.md",
            "- **OA-AAA-001** one\n- **OA-AAA-002** two\n",
        );
        t.write(
            "test_bad_tags.py",
            "# REQ: OA-AAA-001\ndef helper():\n    pass\n\n# REQ: BAD-ID\ndef test_bad_id():\n    pass\n\n# REQ: OA-AAA-002\n\ndef test_not_adjacent():\n    pass\n",
        );
        let report = analyze(&t.0);
        assert_eq!(report.malformed.len(), 3, "{:?}", report.malformed);
        assert_eq!(report.untested, vec!["OA-AAA-001", "OA-AAA-002"]);
        assert!(report.covered.is_empty());
    }

    // REQ: OA-TRC-101
    #[test]
    fn false_harness_is_only_linked() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", REG);
        t.write(
            "a.rs",
            "// REQ: OA-AAA-001\n#[kani::proof]\nfn p_false() { assert!(false); }\n",
        );
        let md = render_markdown(&analyze(&t.0));
        assert!(md.contains("| OA-AAA-001 | proof harness linked |"), "{md}");
        assert!(!md.contains("proved"), "{md}");
    }

    // REQ: OA-TRC-102
    #[test]
    fn summary_line_says_what_pass_means() {
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", "- **OA-AAA-001** one\n");
        t.write("a.rs", &tag("OA-AAA-001", "#[test]"));
        let text = render_text(&analyze(&t.0));
        let last = text.lines().last().unwrap_or("");
        assert!(last.starts_with("Traceability"), "{last}");
        assert!(last.ends_with("PASS"), "{last}");
        let t = Tmp::new();
        t.write("REQUIREMENTS.md", "- **OA-AAA-001** one\n");
        let text = render_text(&analyze(&t.0));
        let last = text.lines().last().unwrap_or("");
        assert!(last.ends_with("FAIL"), "{last}");
    }

    // REQ: OA-TRC-001
    #[test]
    fn fn_name_forms() {
        assert_eq!(fn_name("pub async fn foo_1() {"), Some("foo_1".into()));
        assert_eq!(fn_name("fn "), None);
        assert_eq!(fn_name("let x = 1;"), None);
    }
}

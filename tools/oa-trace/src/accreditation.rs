//! Source-reference checks for the four proposed accreditation records.

use std::fs;
use std::path::{Component, Path};

const RECORDS: [&str; 4] = [
    "docs/assurance/accreditation/oa-perf.md",
    "docs/assurance/accreditation/oa-replay.md",
    "docs/assurance/accreditation/oa-sensorsim.md",
    "docs/assurance/accreditation/p002-scorer.md",
];

/// Checks that every cited Rust test function in the accreditation records exists as a test.
/// Citation syntax is `- Test: `relative/path.rs::function_name``.
pub fn check(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    for record in RECORDS {
        let mut record_citations = 0usize;
        let path = root.join(record);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{record}: cannot read record: {error}"));
                continue;
            }
        };
        for (line_index, line) in text.lines().enumerate() {
            let Some(rest) = line.trim().strip_prefix("- Test: `") else {
                continue;
            };
            let Some((reference, _)) = rest.split_once('`') else {
                errors.push(format!(
                    "{record}:{}: malformed test citation",
                    line_index + 1
                ));
                continue;
            };
            record_citations += 1;
            let Some((file, function)) = reference.rsplit_once("::") else {
                errors.push(format!(
                    "{record}:{}: expected path::test_name",
                    line_index + 1
                ));
                continue;
            };
            if function.is_empty()
                || !function
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || Path::new(file)
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                errors.push(format!(
                    "{record}:{}: invalid test citation path/name",
                    line_index + 1
                ));
                continue;
            }
            let source_path = root.join(file);
            let canonical_root = match root.canonicalize() {
                Ok(path) => path,
                Err(error) => {
                    errors.push(format!("{record}: cannot resolve workspace root: {error}"));
                    continue;
                }
            };
            let canonical_source = match source_path.canonicalize() {
                Ok(path) if path.starts_with(&canonical_root) => path,
                Ok(_) => {
                    errors.push(format!(
                        "{record}:{}: source path escapes workspace",
                        line_index + 1
                    ));
                    continue;
                }
                Err(error) => {
                    errors.push(format!(
                        "{record}:{}: {file}: cannot resolve source: {error}",
                        line_index + 1
                    ));
                    continue;
                }
            };
            let source = match fs::read_to_string(&canonical_source) {
                Ok(source) => source,
                Err(error) => {
                    errors.push(format!(
                        "{record}:{}: {file}: cannot read source: {error}",
                        line_index + 1
                    ));
                    continue;
                }
            };
            if !contains_test(&source, function) {
                errors.push(format!(
                    "{record}:{}: {file}::{function} is not a #[test] function",
                    line_index + 1
                ));
            }
        }
        if record_citations == 0 {
            errors.push(format!("{record}: no `- Test:` citations"));
        }
    }
    errors
}

fn contains_test(source: &str, function: &str) -> bool {
    let sanitized = strip_comments_and_literals(source);
    let lines: Vec<&str> = sanitized.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let declaration = line.trim_start();
        let Some(rest) = declaration.strip_prefix("fn ") else {
            continue;
        };
        let name = rest
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .next()
            .unwrap_or("");
        if name == function {
            let mut previous = index;
            while let Some(prior_index) = previous.checked_sub(1) {
                previous = prior_index;
                let prior = lines.get(prior_index).map(|line| line.trim()).unwrap_or("");
                if prior == "#[test]" {
                    return true;
                }
                if prior.is_empty() || (prior.starts_with("#[") && prior.ends_with(']')) {
                    continue;
                }
                return false;
            }
            return false;
        }
    }
    false
}

/// Blanks Rust comments and string literals while preserving line boundaries for item matching.
fn strip_comments_and_literals(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let mut index = 0usize;
    let mut block_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let next = bytes.get(index + 1).copied();
        if block_depth > 0 {
            if bytes.get(index) == Some(&b'/') && next == Some(b'*') {
                block_depth += 1;
                blank(&mut output, index);
                blank(&mut output, index + 1);
                index += 2;
            } else if bytes.get(index) == Some(&b'*') && next == Some(b'/') {
                block_depth -= 1;
                blank(&mut output, index);
                blank(&mut output, index + 1);
                index += 2;
            } else {
                blank(&mut output, index);
                index += 1;
            }
            continue;
        }
        if in_string {
            let byte = bytes.get(index).copied().unwrap_or_default();
            blank(&mut output, index);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if bytes.get(index) == Some(&b'/') && next == Some(b'/') {
            while index < bytes.len() && bytes.get(index) != Some(&b'\n') {
                blank(&mut output, index);
                index += 1;
            }
            continue;
        }
        if bytes.get(index) == Some(&b'/') && next == Some(b'*') {
            block_depth = 1;
            blank(&mut output, index);
            blank(&mut output, index + 1);
            index += 2;
            continue;
        }
        if bytes.get(index) == Some(&b'"') {
            in_string = true;
            blank(&mut output, index);
        }
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn blank(bytes: &mut [u8], index: usize) {
    if let Some(byte) = bytes.get_mut(index) {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::{check, contains_test};
    use std::fs;

    fn scratch() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "oa-trace-accreditation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(path.join("docs/assurance/accreditation")).unwrap();
        path
    }

    fn records(root: &std::path::Path, reference: &str) {
        for name in ["oa-perf", "oa-replay", "oa-sensorsim", "p002-scorer"] {
            fs::write(
                root.join(format!("docs/assurance/accreditation/{name}.md")),
                format!("- Test: `{reference}`\n"),
            )
            .unwrap();
        }
    }

    #[test]
    fn citations_require_an_existing_test_function() {
        let root = scratch();
        records(&root, "tests/example.rs::works");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/example.rs"), "#[test]\nfn works() {}\n").unwrap();
        assert!(check(&root).is_empty());
        fs::write(root.join("tests/example.rs"), "#[test]\nfn other() {}\n").unwrap();
        assert_eq!(check(&root).len(), 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_and_escaping_paths_are_refused() {
        assert!(!contains_test("fn no_test() {}", "no_test"));
        let root = scratch();
        records(&root, "../outside.rs::escape");
        assert_eq!(check(&root).len(), 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_attribute_must_belong_to_the_cited_function() {
        assert!(contains_test("#[test]\nfn actual() {}", "actual"));
        assert!(!contains_test(
            "#[test]\nfn actual() {}\nfn ordinary() {}",
            "ordinary"
        ));
        assert!(!contains_test(
            "/*\n#[test]\nfn fake() {}\n*/\nfn actual() {}",
            "fake"
        ));
    }

    #[test]
    fn every_record_needs_at_least_one_citation() {
        let root = scratch();
        records(&root, "tests/example.rs::works");
        fs::write(
            root.join("docs/assurance/accreditation/oa-perf.md"),
            "No citation.\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/example.rs"), "#[test]\nfn works() {}\n").unwrap();
        assert_eq!(check(&root).len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn checked_in_accreditation_references_resolve() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(check(&root).is_empty(), "{:?}", check(&root));
    }
}

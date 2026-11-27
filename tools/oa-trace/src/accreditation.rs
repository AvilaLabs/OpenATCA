//! Source-reference checks for proposed accreditation records and the control evidence map.

use std::fs;
use std::io::Read;
use std::path::{Component, Path};

use super::hazard_trace::{parse_document, JsonValue};

const ACCREDITATION_TOOL_COUNT: usize = 4;
const RECORDS: [&str; ACCREDITATION_TOOL_COUNT] = [
    "docs/assurance/accreditation/oa-perf.md",
    "docs/assurance/accreditation/oa-replay.md",
    "docs/assurance/accreditation/oa-sensorsim.md",
    "docs/assurance/accreditation/p002-scorer.md",
];
const RECORD_DIRECTORY: &str = "docs/assurance/accreditation";
const RECORD_INDEX: &str = "INDEX.md";
const MAX_RECORD_DIRECTORY_ENTRIES: usize = 64;
// The largest checked-in record is 5,509 bytes. 16 KiB leaves room for future citations while
// bounding each of the four fixed records before allocating its text buffer.
const MAX_RECORD_BYTES: usize = 16 * 1024;
// The largest file currently cited by these records/control-map entries is 280,584 bytes.
// 1 MiB gives that source about 3.7x headroom without letting a citation trigger an unbounded
// repository-file read.
const MAX_CITED_SOURCE_BYTES: usize = 1024 * 1024;
const CONTROL_MAP: &str = "docs/security/control-map.json";
const MAX_CONTROL_MAP_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTROL_ROWS: usize = 1_000;
const MAX_EVIDENCE_REFS: usize = 32;

/// Checks that every cited Rust test function in the accreditation records exists as a test.
/// Citation syntax is `- Test: `relative/path.rs::function_name``.
pub fn check(root: &Path) -> Vec<String> {
    let mut errors = check_record_inventory(root, &RECORDS);
    for &record in &RECORDS {
        let mut record_citations = 0usize;
        let path = root.join(record);
        let text = match read_bounded_utf8(&path, MAX_RECORD_BYTES) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!(
                    "{record}: cannot read bounded UTF-8 record: {error}"
                ));
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
            let source = match read_bounded_utf8(&canonical_source, MAX_CITED_SOURCE_BYTES) {
                Ok(source) => source,
                Err(error) => {
                    errors.push(format!(
                        "{record}:{}: {file}: cannot read bounded UTF-8 source: {error}",
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
    errors.extend(check_control_map(root));
    errors
}

/// Require the compiled four-record inventory to exactly match the checked-in Markdown tool
/// records. The directory scan is independent of `RECORDS`, so a duplicated or omitted const
/// entry cannot make a missing tool record pass.
fn check_record_inventory(root: &Path, records: &[&str]) -> Vec<String> {
    let mut errors = Vec::new();
    let directory = root.join(RECORD_DIRECTORY);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!(
                "{RECORD_DIRECTORY}: cannot enumerate records: {error}"
            ));
            return errors;
        }
    };
    let mut discovered = Vec::with_capacity(ACCREDITATION_TOOL_COUNT);
    let mut inspected = 0usize;
    for entry in entries {
        inspected = inspected.saturating_add(1);
        if inspected > MAX_RECORD_DIRECTORY_ENTRIES {
            errors.push(format!(
                "{RECORD_DIRECTORY}: exceeds {MAX_RECORD_DIRECTORY_ENTRIES} entry limit"
            ));
            return errors;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("{RECORD_DIRECTORY}: cannot inspect entry: {error}"));
                continue;
            }
        };
        let name = match entry.file_name().to_str() {
            Some(name) => name.to_string(),
            None => {
                errors.push(format!("{RECORD_DIRECTORY}: non-UTF-8 entry name"));
                continue;
            }
        };
        if !name.ends_with(".md") || name == RECORD_INDEX {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_file() => discovered.push(format!("{RECORD_DIRECTORY}/{name}")),
            Ok(_) => errors.push(format!(
                "{RECORD_DIRECTORY}/{name}: record must be a regular file"
            )),
            Err(error) => errors.push(format!(
                "{RECORD_DIRECTORY}/{name}: cannot inspect: {error}"
            )),
        }
    }
    if records.len() > ACCREDITATION_TOOL_COUNT {
        errors.push("accreditation record inventory exceeds the four-tool limit".into());
        return errors;
    }
    let mut listed = Vec::with_capacity(ACCREDITATION_TOOL_COUNT);
    listed.extend_from_slice(records);
    listed.sort_unstable();
    discovered.sort_unstable();
    if listed.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(first, second)| first == second)
    }) {
        errors.push("accreditation record inventory contains a duplicate path".into());
    }
    for path in &listed {
        if !discovered.iter().any(|found| found == path) {
            errors.push(format!("{path}: listed accreditation record is missing"));
        }
    }
    for path in &discovered {
        if !listed.iter().any(|listed_path| listed_path == path) {
            errors.push(format!("{path}: unexpected accreditation tool record"));
        }
    }
    if listed.len() != ACCREDITATION_TOOL_COUNT {
        errors.push(format!(
            "accreditation record inventory has {} entries; exactly four tools are required",
            listed.len()
        ));
    }
    errors
}

fn check_control_map(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let path = root.join(CONTROL_MAP);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            errors.push(format!("{CONTROL_MAP}: must be a regular file"));
            return errors;
        }
        Err(error) => {
            errors.push(format!("{CONTROL_MAP}: cannot inspect map: {error}"));
            return errors;
        }
    };
    if metadata.len() > MAX_CONTROL_MAP_BYTES as u64 {
        errors.push(format!(
            "{CONTROL_MAP}: exceeds {MAX_CONTROL_MAP_BYTES} byte limit"
        ));
        return errors;
    }
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            errors.push(format!("{CONTROL_MAP}: cannot read map: {error}"));
            return errors;
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file
        .take(MAX_CONTROL_MAP_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
    {
        errors.push(format!("{CONTROL_MAP}: cannot read map: {error}"));
        return errors;
    }
    if bytes.len() > MAX_CONTROL_MAP_BYTES {
        errors.push(format!(
            "{CONTROL_MAP}: exceeds {MAX_CONTROL_MAP_BYTES} byte limit"
        ));
        return errors;
    }
    let document = match parse_document(&bytes) {
        Ok(document) => document,
        Err(error) => {
            errors.push(format!("{CONTROL_MAP}: invalid JSON: {error}"));
            return errors;
        }
    };
    let Some(map) = as_object(&document) else {
        errors.push(format!("{CONTROL_MAP}: top-level value must be an object"));
        return errors;
    };
    if as_string(map.get("schema")) != Some("openatca.oscal-control-map")
        || as_number(map.get("version")) != Some("1")
    {
        errors.push(format!("{CONTROL_MAP}: unsupported schema or version"));
        return errors;
    }
    let Some(rows) = map.get("controls").and_then(as_array) else {
        errors.push(format!("{CONTROL_MAP}: controls must be an array"));
        return errors;
    };
    if rows.len() > MAX_CONTROL_ROWS {
        errors.push(format!(
            "{CONTROL_MAP}: more than {MAX_CONTROL_ROWS} control rows"
        ));
        return errors;
    }
    let canonical_root = match root.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            errors.push(format!(
                "{CONTROL_MAP}: cannot resolve workspace root: {error}"
            ));
            return errors;
        }
    };
    let mut seen = std::collections::BTreeSet::new();
    for (row_index, row_value) in rows.iter().enumerate() {
        let Some(row) = as_object(row_value) else {
            errors.push(format!(
                "{CONTROL_MAP}: controls[{row_index}] must be an object"
            ));
            continue;
        };
        let control_id = as_string(row.get("oscal_id")).unwrap_or("");
        if control_id.is_empty() || !seen.insert(control_id.to_string()) {
            errors.push(format!(
                "{CONTROL_MAP}: controls[{row_index}] has empty or duplicate oscal_id"
            ));
            continue;
        }
        let status = as_string(row.get("status")).unwrap_or("");
        if !matches!(status, "implemented" | "partial" | "planned" | "inherited") {
            errors.push(format!(
                "{CONTROL_MAP}: {control_id} has unknown status {status:?}"
            ));
            continue;
        }
        let Some(evidence_refs) = row.get("evidence_refs").and_then(as_array) else {
            errors.push(format!(
                "{CONTROL_MAP}: {control_id} evidence_refs must be an array"
            ));
            continue;
        };
        let Some(implemented_refs) = row.get("implemented_refs").and_then(as_array) else {
            errors.push(format!(
                "{CONTROL_MAP}: {control_id} implemented_refs must be an array"
            ));
            continue;
        };
        if evidence_refs.len() + implemented_refs.len() > MAX_EVIDENCE_REFS {
            errors.push(format!(
                "{CONTROL_MAP}: {control_id} exceeds evidence reference cap"
            ));
            continue;
        }
        if status == "implemented" && implemented_refs.is_empty() {
            errors.push(format!("{CONTROL_MAP}: {control_id} implemented status needs an exact test or CI reference"));
        }
        if status != "implemented" && !implemented_refs.is_empty() {
            errors.push(format!("{CONTROL_MAP}: {control_id} may have implemented_refs only with implemented status"));
        }
        for (field, refs) in [
            ("evidence_refs", evidence_refs),
            ("implemented_refs", implemented_refs),
        ] {
            for (ref_index, reference) in refs.iter().enumerate() {
                if let Err(error) = check_evidence_ref(root, &canonical_root, reference) {
                    errors.push(format!(
                        "{CONTROL_MAP}: {control_id} {field}[{ref_index}]: {error}"
                    ));
                }
            }
        }
    }
    errors
}

fn check_evidence_ref(root: &Path, canonical_root: &Path, value: &JsonValue) -> Result<(), String> {
    let reference = as_object(value).ok_or_else(|| "reference must be an object".to_string())?;
    let kind = as_string(reference.get("kind")).unwrap_or("");
    let file = as_string(reference.get("path")).unwrap_or("");
    if file.is_empty()
        || Path::new(file)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("path must be a nonempty relative path without traversal".to_string());
    }
    let source_path = root.join(file);
    let canonical_source = match source_path.canonicalize() {
        Ok(path) if path.starts_with(canonical_root) => path,
        Ok(_) => return Err("source path escapes workspace".to_string()),
        Err(error) => return Err(format!("cannot resolve {file}: {error}")),
    };
    let source = read_bounded_utf8(&canonical_source, MAX_CITED_SOURCE_BYTES)
        .map_err(|error| format!("cannot read bounded UTF-8 source {file}: {error}"))?;
    match kind {
        "rust_test" => {
            let function = as_string(reference.get("test_name")).unwrap_or("");
            if function.is_empty()
                || !function
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err("test_name is missing or invalid".to_string());
            }
            if contains_test(&source, function) {
                Ok(())
            } else {
                Err(format!("{file}::{function} is not a #[test] function"))
            }
        }
        "ci_job" => {
            let job = as_string(reference.get("job")).unwrap_or("");
            let step = as_string(reference.get("step")).unwrap_or("");
            if job.is_empty() || step.is_empty() {
                return Err("CI job and step must be nonempty".to_string());
            }
            if contains_ci_step(&source, job, step) {
                Ok(())
            } else {
                Err(format!(
                    "{file} has no exact workflow job {job:?} / step {step:?}"
                ))
            }
        }
        _ => Err(format!("unsupported evidence kind {kind:?}")),
    }
}

/// Read a regular file into a UTF-8 string without allocating beyond the declared byte cap.
/// Metadata rejects known oversize files early; the bounded read also handles file growth after
/// that check. UTF-8 validation occurs only after the cap has been enforced.
fn read_bounded_utf8(path: &Path, max_bytes: usize) -> Result<String, String> {
    let (mut file, file_bytes) = oa_sealed::Sealed::production()
        .open_regular(path)
        .map_err(|error| error.to_string())?;
    if file_bytes > max_bytes as u64 {
        return Err(format!("exceeds {max_bytes} byte limit"));
    }
    let read_limit = max_bytes
        .checked_add(1)
        .ok_or_else(|| "byte limit cannot be incremented".to_string())?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(read_limit)
        .map_err(|error| format!("cannot reserve bounded read buffer: {error}"))?;
    file.by_ref()
        .take(read_limit as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > max_bytes {
        return Err(format!("exceeds {max_bytes} byte limit"));
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

fn contains_ci_step(source: &str, job: &str, step: &str) -> bool {
    let mut in_job = false;
    let mut in_steps = false;
    let mut block_scalar_indent = None;
    for line in source.lines() {
        let indent = line.bytes().take_while(|byte| *byte == b' ').count();
        if let Some(block_indent) = block_scalar_indent {
            if line.trim().is_empty() || indent > block_indent {
                continue;
            }
            block_scalar_indent = None;
        }
        if line.starts_with("  ") && !line.starts_with("    ") && line.ends_with(':') {
            in_job = line == format!("  {job}:");
            in_steps = false;
            continue;
        }
        if in_job && indent == 4 && line.trim() == "steps:" {
            in_steps = true;
            continue;
        }
        if in_job && indent == 4 && line.trim_end().ends_with(':') && line.trim() != "steps:" {
            in_steps = false;
        }
        if in_job && in_steps && line == format!("      - name: {step}") {
            return true;
        }
        if let Some(value) = yaml_mapping_value(line) {
            if is_block_scalar_indicator(value) {
                block_scalar_indent = Some(indent);
            }
        }
    }
    false
}

fn yaml_mapping_value(line: &str) -> Option<&str> {
    let mut single_quoted = false;
    let mut double_quoted = false;
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = *bytes.get(index)?;
        match byte {
            b'\'' if !double_quoted => {
                if single_quoted && bytes.get(index + 1) == Some(&b'\'') {
                    index += 2;
                    continue;
                }
                single_quoted = !single_quoted;
            }
            b'"' if !single_quoted => {
                let escaped = index > 0 && bytes.get(index - 1) == Some(&b'\\');
                if !escaped {
                    double_quoted = !double_quoted;
                }
            }
            b'#' if !single_quoted && !double_quoted => {
                return line
                    .get(..index)?
                    .split_once(':')
                    .map(|(_, value)| value.trim());
            }
            b':' if !single_quoted && !double_quoted => {
                return line.get(index + 1..).map(str::trim);
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn is_block_scalar_indicator(value: &str) -> bool {
    let token = value.split('#').next().unwrap_or("").trim();
    let Some(style) = token.as_bytes().first().copied() else {
        return false;
    };
    if style != b'|' && style != b'>' {
        return false;
    }
    token[1..]
        .bytes()
        .all(|byte| matches!(byte, b'+' | b'-' | b'1'..=b'9'))
}

fn as_object(value: &JsonValue) -> Option<&std::collections::BTreeMap<String, JsonValue>> {
    match value {
        JsonValue::Object(object) => Some(object),
        _ => None,
    }
}

fn as_array(value: &JsonValue) -> Option<&[JsonValue]> {
    match value {
        JsonValue::Array(array) => Some(array),
        _ => None,
    }
}

fn as_string(value: Option<&JsonValue>) -> Option<&str> {
    match value {
        Some(JsonValue::String(value)) => Some(value),
        _ => None,
    }
}

fn as_number(value: Option<&JsonValue>) -> Option<&str> {
    match value {
        Some(JsonValue::Number(value)) => Some(value),
        _ => None,
    }
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

/// Validates one named Rust test citation using the same comment/literal-aware parser as the
/// accreditation citation checker. The path is relative to `root` and must remain inside it.
pub fn verify_test_reference(root: &Path, reference: &str) -> Result<(), String> {
    let Some((file, function)) = reference.rsplit_once("::") else {
        return Err("expected relative/path.rs::test_function".into());
    };
    if file.is_empty()
        || function.is_empty()
        || !function
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || Path::new(file)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("test reference path or name is invalid".into());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "workspace root cannot be resolved".to_string())?;
    let path = root.join(file);
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "test source cannot be resolved".to_string())?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("test source escapes workspace".into());
    }
    let source = read_bounded_utf8(&canonical_path, MAX_CITED_SOURCE_BYTES)
        .map_err(|_| "test source is not a bounded UTF-8 file".to_string())?;
    if !contains_test(&source, function) {
        return Err("citation is not a real #[test] function".into());
    }
    Ok(())
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
        if let Some((quote_index, hash_count)) = raw_string_open(bytes, index) {
            if let Some(end) = raw_string_end(bytes, quote_index, hash_count) {
                blank_range(&mut output, index, end);
                index = end;
            } else {
                blank_range(&mut output, index, bytes.len());
                break;
            }
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

fn raw_string_open(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let previous = start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .copied();
    if previous.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return None;
    }
    let pair_end = start.checked_add(2)?;
    let prefix_len =
        if bytes.get(start..pair_end) == Some(b"br") || bytes.get(start..pair_end) == Some(b"cr") {
            2
        } else if bytes.get(start) == Some(&b'r') {
            1
        } else {
            return None;
        };
    let mut quote_index = start.checked_add(prefix_len)?;
    let mut hash_count = 0usize;
    while bytes.get(quote_index) == Some(&b'#') {
        hash_count = hash_count.checked_add(1)?;
        quote_index = quote_index.checked_add(1)?;
    }
    (bytes.get(quote_index) == Some(&b'"')).then_some((quote_index, hash_count))
}

fn raw_string_end(bytes: &[u8], quote_index: usize, hash_count: usize) -> Option<usize> {
    let mut index = quote_index.checked_add(1)?;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'"') {
            let end = index.checked_add(1)?.checked_add(hash_count)?;
            let hashes_start = index.checked_add(1)?;
            let hashes = bytes.get(hashes_start..end)?;
            if hashes.iter().all(|byte| *byte == b'#') && bytes.get(end) != Some(&b'#') {
                return Some(end);
            }
        }
        index = index.checked_add(1)?;
    }
    None
}

fn blank_range(bytes: &mut [u8], start: usize, end: usize) {
    for index in start..end {
        blank(bytes, index);
    }
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

    use super::{
        check, check_record_inventory, contains_ci_step, contains_test, read_bounded_utf8,
        CONTROL_MAP, MAX_CITED_SOURCE_BYTES, MAX_RECORD_BYTES, RECORDS,
    };
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
        fs::create_dir_all(path.join("docs/security")).unwrap();
        fs::write(
            path.join(CONTROL_MAP),
            r#"{"schema":"openatca.oscal-control-map","version":1,"controls":[]}"#,
        )
        .unwrap();
        path
    }

    fn map_with_reference(reference: &str) -> String {
        format!(
            r#"{{"schema":"openatca.oscal-control-map","version":1,"controls":[{{"oscal_id":"ac-1","status":"implemented","evidence_refs":[],"implemented_refs":[{reference}]}}]}}"#
        )
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
    fn oversized_accreditation_record_is_refused_before_unbounded_read() {
        let root = scratch();
        records(&root, "tests/example.rs::works");
        fs::write(root.join(RECORDS[0]), vec![b'x'; MAX_RECORD_BYTES + 1]).unwrap();
        let errors = check(&root);
        assert!(errors.iter().any(|error| {
            error.contains("oa-perf.md: cannot read bounded UTF-8 record")
                && error.contains("exceeds 16384 byte limit")
        }));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    #[test]
    fn fifo_cited_source_is_refused_without_waiting_for_a_writer() {
        let root = scratch();
        let path = root.join("source.rs");
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let _ = sender.send(read_bounded_utf8(&path, MAX_CITED_SOURCE_BYTES));
        });
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("a source FIFO must refuse before waiting for a writer");
        assert!(result.unwrap_err().contains("not a regular file"));
        reader.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_cited_source_is_refused_before_unbounded_read() {
        let root = scratch();
        records(&root, "tests/example.rs::works");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("tests/example.rs"),
            vec![b'x'; MAX_CITED_SOURCE_BYTES + 1],
        )
        .unwrap();
        let errors = check(&root);
        assert!(errors.iter().any(|error| {
            error.contains("tests/example.rs: cannot read bounded UTF-8 source")
                && error.contains("exceeds 1048576 byte limit")
        }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bounded_utf8_reader_accepts_exact_cap_and_refuses_invalid_utf8() {
        let root = scratch();
        let exact = root.join("exact.txt");
        fs::write(&exact, vec![b'a'; 8]).unwrap();
        assert_eq!(read_bounded_utf8(&exact, 8).as_deref(), Ok("aaaaaaaa"));

        let invalid = root.join("invalid.txt");
        fs::write(&invalid, [b'a', 0xff]).unwrap();
        assert!(read_bounded_utf8(&invalid, 8)
            .unwrap_err()
            .contains("invalid utf-8"));
        let _ = fs::remove_dir_all(root);
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
    fn test_reference_api_refuses_comment_raw_string_and_non_test_helpers() {
        let root = scratch();
        fs::create_dir_all(root.join("tests")).unwrap();
        let path = root.join("tests/example.rs");
        fs::write(
            &path,
            "// #[test]\n// fn comment_fake() {}\nconst TEXT: &str = r###\"#[test]\\nfn raw_fake() {}\"###;\nfn helper() {}\n#[test]\nfn real_test() {}\n",
        )
        .unwrap();
        assert!(super::verify_test_reference(&root, "tests/example.rs::real_test").is_ok());
        assert!(super::verify_test_reference(&root, "tests/example.rs::comment_fake").is_err());
        assert!(super::verify_test_reference(&root, "tests/example.rs::raw_fake").is_err());
        assert!(super::verify_test_reference(&root, "tests/example.rs::helper").is_err());
        assert!(super::verify_test_reference(&root, "../outside.rs::escape").is_err());
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
    fn raw_string_contents_cannot_supply_test_references() {
        for (prefix, too_few_hashes, close) in [
            ("r##", "\"#", "\"##"),
            ("br#", "\"", "\"#"),
            ("cr###", "\"##", "\"###"),
        ] {
            let source = format!(
                "const TEXT: &str = {prefix}\"\n{too_few_hashes}\n#[test]\nfn fake() {{}}\n{close};"
            );
            assert!(!contains_test(&source, "fake"), "{source}");
        }
        let source = "const TEXT: &str = r##\"#[test]\nfn fake() {}\"##;\n#[test]\nfn real() {}";
        assert!(!contains_test(source, "fake"));
        assert!(contains_test(source, "real"));
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

        let mut duplicate_tool_mutant = RECORDS;
        duplicate_tool_mutant[3] = RECORDS[0];
        let errors = check_record_inventory(&root, &duplicate_tool_mutant);
        assert!(errors.iter().any(|error| error.contains("duplicate path")));
        assert!(errors
            .iter()
            .any(|error| error.contains("p002-scorer.md: unexpected accreditation tool record")));
    }

    // REQ: OA-E1-002
    #[test]
    fn implemented_control_test_references_must_resolve() {
        let root = scratch();
        records(&root, "tests/example.rs::works");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/example.rs"), "#[test]\nfn works() {}\n").unwrap();
        let reference = r#"{"kind":"rust_test","path":"tests/example.rs","test_name":"works"}"#;
        fs::write(root.join(CONTROL_MAP), map_with_reference(reference)).unwrap();
        assert!(check(&root).is_empty(), "{:?}", check(&root));
        fs::write(root.join("tests/example.rs"), "#[test]\nfn other() {}\n").unwrap();
        assert!(check(&root)
            .iter()
            .any(|error| error.contains("control-map.json: ac-1 implemented_refs[0]")));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-E1-002
    #[test]
    fn implemented_control_requires_an_exact_reference() {
        let root = scratch();
        records(&root, "tests/example.rs::works");
        fs::write(
            root.join(CONTROL_MAP),
            r#"{"schema":"openatca.oscal-control-map","version":1,"controls":[{"oscal_id":"ac-1","status":"implemented","evidence_refs":[],"implemented_refs":[]}]}"#,
        )
        .unwrap();
        assert!(check(&root)
            .iter()
            .any(|error| error
                .contains("ac-1 implemented status needs an exact test or CI reference")));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-E1-002
    #[test]
    fn ci_evidence_requires_the_named_job_and_step() {
        assert!(contains_ci_step(
            "jobs:\n  check:\n    steps:\n      - name: Test\n  fuzz:\n    steps:\n      - name: Test\n",
            "check",
            "Test"
        ));
        assert!(!contains_ci_step(
            "jobs:\n  check:\n    steps:\n      - name: Test\n  fuzz:\n    steps:\n      - name: Other\n",
            "missing",
            "Test"
        ));
        let literal_block = "jobs:\n  check:\n    steps:\n      - run: |\n          - name: Fake\n      - name: Test\n";
        assert!(contains_ci_step(literal_block, "check", "Test"));
        assert!(!contains_ci_step(literal_block, "check", "Fake"));
    }
}

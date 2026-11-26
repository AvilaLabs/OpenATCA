//! Bounded H3 hazard-to-requirement-to-verification trace checker.
//!
//! This checker validates only the declared safety scope in `docs/safety/hazard-log.json`. It
//! does not assert that the scope is a complete system hazard assessment or that tests or proofs passed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

const HAZARD_LOG_PATH: &str = "docs/safety/hazard-log.json";
const MAX_LOG_BYTES: usize = 2 * 1024 * 1024;
// The per-ID engineering disposition rows add bounded schema nodes; this remains a hard cap
// well below what a 2 MiB document could encode as tiny scalar values.
const MAX_NODES: usize = 50_000;
const MAX_DEPTH: usize = 32;
const MAX_HAZARDS: usize = 256;
const MAX_REQUIREMENTS: usize = 10_000;
// The composed train has 132 flight-data verification refs; 160 leaves 28 further links while
// keeping source/proof refs at 128. Its full log is 1,190 refs, 1,703,765 bytes and 19,299 nodes,
// within the generator's 1,600-ref, 2 MiB document and 50,000-node bounds.
const MAX_VERIFICATION_REFS_PER_HAZARD: usize = 160;
const MAX_REFS_PER_HAZARD: usize = 128;

#[derive(Debug)]
pub(crate) enum JsonValue {
    Null,
    Bool,
    Number(String),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    offset: usize,
    nodes: usize,
}

impl<'a> JsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            nodes: 0,
        }
    }

    fn parse(mut self) -> Result<JsonValue, String> {
        let value = self.value(0)?;
        self.space();
        if self.offset != self.bytes.len() {
            return Err("trailing bytes after JSON value".to_string());
        }
        Ok(value)
    }

    fn value(&mut self, depth: usize) -> Result<JsonValue, String> {
        if depth > MAX_DEPTH {
            return Err("JSON depth cap exceeded".to_string());
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| "JSON node counter overflow".to_string())?;
        if self.nodes > MAX_NODES {
            return Err("JSON node cap exceeded".to_string());
        }
        self.space();
        match self.peek() {
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string().map(JsonValue::String),
            Some(b't') => {
                self.literal(b"true")?;
                Ok(JsonValue::Bool)
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(JsonValue::Bool)
            }
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b'-' | b'0'..=b'9') => self.number().map(JsonValue::Number),
            _ => Err("expected JSON value".to_string()),
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.expect(b'{')?;
        let mut values = BTreeMap::new();
        self.space();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Ok(JsonValue::Object(values));
        }
        loop {
            self.space();
            let key = self.string()?;
            self.space();
            self.expect(b':')?;
            let value = self.value(depth)?;
            if values.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate JSON object key {key:?}"));
            }
            self.space();
            match self.peek() {
                Some(b',') => self.offset += 1,
                Some(b'}') => {
                    self.offset += 1;
                    break;
                }
                _ => return Err("expected comma or object close".to_string()),
            }
        }
        Ok(JsonValue::Object(values))
    }

    fn array(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.space();
        if self.peek() == Some(b']') {
            self.offset += 1;
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.value(depth)?);
            self.space();
            match self.peek() {
                Some(b',') => self.offset += 1,
                Some(b']') => {
                    self.offset += 1;
                    break;
                }
                _ => return Err("expected comma or array close".to_string()),
            }
        }
        Ok(JsonValue::Array(values))
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| "unterminated JSON string".to_string())?;
            self.offset += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escaped = self
                        .peek()
                        .ok_or_else(|| "unterminated JSON escape".to_string())?;
                    self.offset += 1;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return Err("invalid JSON escape".to_string()),
                    }
                }
                0..=0x1f => return Err("control byte in JSON string".to_string()),
                0x20..=0x7f => out.push(char::from(byte)),
                _ => return Err("hazard log JSON must contain ASCII strings".to_string()),
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let digit = self
                .peek()
                .and_then(|byte| char::from(byte).to_digit(16))
                .ok_or_else(|| "invalid unicode escape".to_string())?;
            value = value
                .checked_mul(16)
                .and_then(|part| part.checked_add(digit))
                .ok_or_else(|| "unicode escape overflow".to_string())?;
            self.offset += 1;
        }
        char::from_u32(value).ok_or_else(|| "unsupported unicode escape".to_string())
    }

    fn number(&mut self) -> Result<String, String> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        match self.peek() {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.offset += 1;
                }
            }
            _ => return Err("invalid JSON number".to_string()),
        }
        if self.peek() == Some(b'.') {
            self.offset += 1;
            let fraction_start = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.offset += 1;
            }
            if self.offset == fraction_start {
                return Err("invalid JSON fraction".to_string());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            let exponent_start = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.offset += 1;
            }
            if self.offset == exponent_start {
                return Err("invalid JSON exponent".to_string());
            }
        }
        if self.offset == start {
            return Err("invalid JSON number".to_string());
        }
        let token = self
            .bytes
            .get(start..self.offset)
            .ok_or_else(|| "invalid JSON number bounds".to_string())?;
        std::str::from_utf8(token)
            .map(str::to_string)
            .map_err(|_| "JSON number is not ASCII".to_string())
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), String> {
        for expected in literal {
            if self.peek() != Some(*expected) {
                return Err("invalid JSON literal".to_string());
            }
            self.offset += 1;
        }
        Ok(())
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.peek() != Some(expected) {
            return Err(format!("expected JSON byte {:?}", char::from(expected)));
        }
        self.offset += 1;
        Ok(())
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }
}

pub(crate) fn parse_document(bytes: &[u8]) -> Result<JsonValue, String> {
    JsonParser::new(bytes).parse()
}

fn object(value: &JsonValue) -> Option<&BTreeMap<String, JsonValue>> {
    match value {
        JsonValue::Object(value) => Some(value),
        _ => None,
    }
}

fn array(value: &JsonValue) -> Option<&[JsonValue]> {
    match value {
        JsonValue::Array(value) => Some(value),
        _ => None,
    }
}

fn text<'a>(value: &'a JsonValue, field: &str) -> Result<&'a str, String> {
    match value {
        JsonValue::String(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("{field}: expected nonempty string")),
    }
}

fn string_array(value: &JsonValue, where_: &str, cap: usize) -> Result<Vec<String>, String> {
    let values = array(value).ok_or_else(|| format!("{where_}: expected array"))?;
    if values.len() > cap {
        return Err(format!("{where_}: item cap exceeded"));
    }
    values
        .iter()
        .map(|value| text(value, where_).map(str::to_string))
        .collect()
}

fn reject_unknown_keys(
    object: &BTreeMap<String, JsonValue>,
    allowed: &[&str],
    where_: &str,
    errors: &mut Vec<String>,
) {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(format!("{where_}: unknown field {key}"));
        }
    }
}

/// Checks hazard coverage against the requirement register and current test/proof links.
pub fn check(root: &Path) -> Vec<String> {
    let path = root.join(HAZARD_LOG_PATH);
    let mut errors = Vec::new();
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => return vec![format!("{}: {error}", path.display())],
    };
    let mut raw = Vec::new();
    if let Err(error) = file.take((MAX_LOG_BYTES + 1) as u64).read_to_end(&mut raw) {
        return vec![format!("{}: {error}", path.display())];
    }
    if raw.len() > MAX_LOG_BYTES {
        return vec![format!("{}: byte cap exceeded", path.display())];
    }
    let parsed = match JsonParser::new(&raw).parse() {
        Ok(value) => value,
        Err(error) => return vec![format!("{}: {error}", path.display())],
    };
    let Some(log) = object(&parsed) else {
        return vec![format!("{}: root must be an object", path.display())];
    };
    reject_unknown_keys(
        log,
        &[
            "schema",
            "version",
            "assessment_status",
            "coverage_status",
            "classification_basis",
            "safety_requirement_ids",
            "safety_requirement_assessments",
            "non_safety_requirement_assessments",
            "unresolved_requirement_assessments",
            "withdrawn_requirement_assessments",
            "unclassified_requirement_policy",
            "source_documents",
            "open_items",
            "hazards",
        ],
        "hazard log",
        &mut errors,
    );
    if text_field(log, "schema") != Some("openatca.hazard-log") {
        errors.push("hazard log schema must be openatca.hazard-log".to_string());
    }
    if !matches!(log.get("version"), Some(JsonValue::Number(version)) if version == "1") {
        errors.push("hazard log version must be exactly 1".to_string());
    }
    if text_field(log, "assessment_status") != Some("DRAFT_NOT_ASSESSED") {
        errors.push("hazard log must disclose DRAFT_NOT_ASSESSED".to_string());
    }
    if text_field(log, "coverage_status") != Some("DECLARED_FUNCTION_SCOPE_ONLY") {
        errors.push("hazard log must disclose declared-scope-only coverage".to_string());
    }
    let requirement_values = match log.get("safety_requirement_ids") {
        Some(value) => string_array(value, "safety_requirement_ids", MAX_REQUIREMENTS),
        None => Err("missing safety_requirement_ids".to_string()),
    };
    let requirements = match requirement_values {
        Ok(values) => values,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    let requirement_set: BTreeSet<String> = requirements.iter().cloned().collect();
    if requirement_set.len() != requirements.len() {
        errors.push("safety_requirement_ids contains duplicates".to_string());
    }
    let safety_assessments = disposition_rows(
        log,
        "safety_requirement_assessments",
        &["id", "function", "hazard_id", "rationale"],
        MAX_REQUIREMENTS,
        &mut errors,
    );
    let mut safety_assessment_ids = BTreeSet::new();
    let mut safety_hazard_by_id = BTreeMap::new();
    for assessment in safety_assessments {
        let id = text_field(assessment, "id").unwrap_or("");
        if id.is_empty() || !safety_assessment_ids.insert(id.to_string()) {
            errors.push(format!("safety assessment {id:?} is empty or duplicated"));
        }
        if text_field(assessment, "function").is_none_or(str::is_empty)
            || text_field(assessment, "rationale").is_none_or(str::is_empty)
        {
            errors.push(format!(
                "safety assessment {id:?} needs function and rationale"
            ));
        }
        let hazard_id = text_field(assessment, "hazard_id").unwrap_or("");
        if hazard_id.is_empty() {
            errors.push(format!("safety assessment {id:?} needs hazard_id"));
        } else if safety_hazard_by_id
            .insert(id.to_string(), hazard_id.to_string())
            .is_some()
        {
            errors.push(format!("safety assessment {id:?} is duplicated"));
        }
    }
    if safety_assessment_ids != requirement_set {
        errors.push(
            "safety_requirement_assessments must exactly cover safety_requirement_ids".to_string(),
        );
    }
    let definitions = fs::read(root.join("REQUIREMENTS.md"))
        .ok()
        .map(|bytes| super::parse_requirements(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default();
    let active_definitions: BTreeSet<String> = definitions
        .iter()
        .filter(|requirement| !requirement.withdrawn)
        .map(|requirement| requirement.id.clone())
        .collect();
    for id in &safety_assessment_ids {
        if !active_definitions.contains(id) {
            errors.push(format!(
                "safety assessment {id} is not an active requirement"
            ));
        }
    }
    let non_safety_values = match log.get("non_safety_requirement_assessments") {
        Some(value) => match array(value) {
            Some(rows) if rows.len() <= MAX_REQUIREMENTS => rows,
            Some(_) => {
                errors.push("non_safety_requirement_assessments cap exceeded".to_string());
                &[]
            }
            None => {
                errors.push("non_safety_requirement_assessments must be an array".to_string());
                &[]
            }
        },
        None => {
            errors.push("missing non_safety_requirement_assessments".to_string());
            &[]
        }
    };
    let mut non_safety_ids = BTreeSet::new();
    for (index, value) in non_safety_values.iter().enumerate() {
        let Some(disposition) = object(value) else {
            errors.push(format!(
                "non_safety_requirement_assessments[{index}] must be an object"
            ));
            continue;
        };
        reject_unknown_keys(
            disposition,
            &["id", "function", "rationale", "register_clause"],
            "non-safety disposition",
            &mut errors,
        );
        let id = text_field(disposition, "id").unwrap_or("");
        if id.is_empty()
            || !active_definitions.contains(id)
            || !non_safety_ids.insert(id.to_string())
        {
            errors.push(format!(
                "non-safety disposition {id:?} is invalid, inactive, or duplicated"
            ));
        }
        if text_field(disposition, "function").is_none_or(str::is_empty)
            || text_field(disposition, "rationale").is_none_or(str::is_empty)
            || text_field(disposition, "register_clause").is_none_or(str::is_empty)
        {
            errors.push(format!(
                "non-safety disposition {id:?} needs function and rationale"
            ));
        }
    }
    let unresolved_assessments = disposition_rows(
        log,
        "unresolved_requirement_assessments",
        &["id", "function", "rationale"],
        MAX_REQUIREMENTS,
        &mut errors,
    );
    let mut unresolved_assessment_ids = BTreeSet::new();
    for assessment in unresolved_assessments {
        let id = text_field(assessment, "id").unwrap_or("");
        if id.is_empty()
            || !active_definitions.contains(id)
            || !unresolved_assessment_ids.insert(id.to_string())
        {
            errors.push(format!(
                "unresolved assessment {id:?} is invalid, inactive, or duplicated"
            ));
        }
        if text_field(assessment, "function").is_none_or(str::is_empty)
            || text_field(assessment, "rationale").is_none_or(str::is_empty)
        {
            errors.push(format!(
                "unresolved assessment {id:?} needs function and rationale"
            ));
        }
    }
    let withdrawn_values = match log.get("withdrawn_requirement_assessments") {
        Some(value) => match array(value) {
            Some(rows) if rows.len() <= MAX_REQUIREMENTS => rows,
            Some(_) => {
                errors.push("withdrawn_requirement_assessments cap exceeded".to_string());
                &[]
            }
            None => {
                errors.push("withdrawn_requirement_assessments must be an array".to_string());
                &[]
            }
        },
        None => {
            errors.push("missing withdrawn_requirement_assessments".to_string());
            &[]
        }
    };
    let withdrawn_definitions: BTreeSet<String> = definitions
        .iter()
        .filter(|requirement| requirement.withdrawn)
        .map(|requirement| requirement.id.clone())
        .collect();
    let mut withdrawn_ids = BTreeSet::new();
    for (index, value) in withdrawn_values.iter().enumerate() {
        let Some(disposition) = object(value) else {
            errors.push(format!(
                "withdrawn_requirement_assessments[{index}] must be an object"
            ));
            continue;
        };
        reject_unknown_keys(
            disposition,
            &[
                "id",
                "source_document",
                "source_clause",
                "superseded_by",
                "rationale",
            ],
            "withdrawn disposition",
            &mut errors,
        );
        let id = text_field(disposition, "id").unwrap_or("");
        if id.is_empty()
            || !withdrawn_definitions.contains(id)
            || !withdrawn_ids.insert(id.to_string())
        {
            errors.push(format!(
                "withdrawn disposition {id:?} is invalid, active, or duplicated"
            ));
        }
        if text_field(disposition, "source_document") != Some("REQUIREMENTS.md")
            || text_field(disposition, "source_clause")
                .is_none_or(|clause| !clause.starts_with("WITHDRAWN:"))
            || text_field(disposition, "rationale").is_none_or(str::is_empty)
        {
            errors.push(format!(
                "withdrawn disposition {id:?} must identify REQUIREMENTS.md, the withdrawn clause, and rationale"
            ));
        }
        match disposition.get("superseded_by") {
            Some(value) => {
                match string_array(value, "withdrawn.superseded_by", MAX_REQUIREMENTS) {
                    Ok(ids) => {
                        if ids.is_empty() {
                            errors.push(format!("withdrawn disposition {id:?} must name its replacement requirement(s)"));
                        }
                        for replacement in ids {
                            if !active_definitions.contains(&replacement) {
                                errors.push(format!(
                                "withdrawn disposition {id:?} replacement {replacement} is not active"
                            ));
                            }
                        }
                    }
                    Err(error) => errors.push(error),
                }
            }
            None => errors.push(format!("withdrawn disposition {id:?} needs superseded_by")),
        }
    }
    if withdrawn_ids != withdrawn_definitions {
        let missing = withdrawn_definitions
            .difference(&withdrawn_ids)
            .cloned()
            .collect::<Vec<_>>();
        errors.push(format!(
            "withdrawn register IDs need explicit dispositions: {}",
            missing.join(",")
        ));
    }
    let empty_policy = BTreeMap::new();
    let policy = match log.get("unclassified_requirement_policy").and_then(object) {
        Some(policy) => policy,
        None => {
            errors.push("missing unclassified_requirement_policy".to_string());
            &empty_policy
        }
    };
    reject_unknown_keys(
        policy,
        &["classification", "function", "rationale"],
        "unclassified policy",
        &mut errors,
    );
    if text_field(policy, "classification") != Some("unresolved")
        || text_field(policy, "function") != Some("unassigned")
        || text_field(policy, "rationale").is_none_or(str::is_empty)
    {
        errors.push("unclassified policy must disclose unresolved status, unassigned function, and rationale".to_string());
    }
    if requirement_set.iter().any(|id| non_safety_ids.contains(id)) {
        errors.push("a requirement cannot be both safety and non-safety".to_string());
    }
    if withdrawn_ids
        .iter()
        .any(|id| requirement_set.contains(id) || non_safety_ids.contains(id))
    {
        errors.push("a withdrawn requirement cannot be safety or non-safety".to_string());
    }
    let unresolved: Vec<&String> = active_definitions
        .iter()
        .filter(|id| !requirement_set.contains(*id) && !non_safety_ids.contains(*id))
        .collect();
    if !unresolved.is_empty() {
        let unresolved_ids = unresolved
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        errors.push(format!(
            "UNRESOLVED_REQUIREMENT_CLASSIFICATION: {} active register ID(s) use the declared unresolved policy (function=unassigned); IDs={unresolved_ids}",
            unresolved.len(),
        ));
    }
    let unresolved_ids: BTreeSet<String> = unresolved.iter().map(|id| (*id).clone()).collect();
    if unresolved_assessment_ids != unresolved_ids {
        errors.push(
            "unresolved_requirement_assessments must exactly cover the unresolved active IDs"
                .to_string(),
        );
    }
    let report = super::analyze(root);
    for requirement in &requirements {
        if !super::is_valid_id(requirement) || !active_definitions.contains(requirement) {
            errors.push(format!(
                "safety requirement {requirement} is not an active register ID"
            ));
        }
        if report
            .covered
            .get(requirement)
            .is_none_or(|refs| refs.is_empty())
        {
            errors.push(format!(
                "safety requirement {requirement} has no linked test or proof"
            ));
        }
    }

    let hazards = match log.get("hazards").and_then(array) {
        Some(hazards) if !hazards.is_empty() && hazards.len() <= MAX_HAZARDS => hazards,
        Some(_) => {
            errors.push("hazards must be a nonempty array within its cap".to_string());
            &[]
        }
        None => {
            errors.push("hazards must be an array".to_string());
            &[]
        }
    };
    let allowed_classes = [
        "Catastrophic",
        "Hazardous",
        "Major",
        "Minor",
        "NoEffect",
        "Unassessed",
    ];
    let mut hazard_ids = BTreeSet::new();
    let mut linked_requirements = BTreeSet::new();
    for (index, value) in hazards.iter().enumerate() {
        let Some(hazard) = object(value) else {
            errors.push(format!("hazards[{index}] must be an object"));
            continue;
        };
        reject_unknown_keys(
            hazard,
            &[
                "id",
                "function",
                "cause",
                "effect",
                "severity_class",
                "severity_status",
                "severity_rationale",
                "mitigation_requirement_ids",
                "source_refs",
                "verification_refs",
                "proof_refs",
            ],
            "hazard record",
            &mut errors,
        );
        let hazard_id = text_field(hazard, "id").unwrap_or("");
        if hazard_id.is_empty() || !hazard_ids.insert(hazard_id.to_string()) {
            errors.push(format!("hazards[{index}] has an empty or duplicate id"));
        }
        for name in [
            "function",
            "cause",
            "effect",
            "severity_status",
            "severity_rationale",
        ] {
            if text_field(hazard, name).is_none() {
                errors.push(format!("hazard {hazard_id}: missing nonempty {name}"));
            }
        }
        let class = text_field(hazard, "severity_class").unwrap_or("");
        if !allowed_classes.contains(&class) {
            errors.push(format!(
                "hazard {hazard_id}: severity_class is not an allowed class"
            ));
        }
        let severity_status = text_field(hazard, "severity_status").unwrap_or("");
        if !["engineering_candidate", "unassessed"].contains(&severity_status) {
            errors.push(format!(
                "hazard {hazard_id}: severity_status must be candidate or unassessed"
            ));
        }
        if (class == "Unassessed") != (severity_status == "unassessed") {
            errors.push(format!(
                "hazard {hazard_id}: severity class and status disagree"
            ));
        }
        let mitigation_ids = match hazard.get("mitigation_requirement_ids") {
            Some(value) => string_array(value, "mitigation_requirement_ids", MAX_REQUIREMENTS),
            None => Err("missing mitigation_requirement_ids".to_string()),
        };
        let mitigation_ids = match mitigation_ids {
            Ok(values) if !values.is_empty() => values,
            Ok(_) => {
                errors.push(format!(
                    "hazard {hazard_id}: at least one requirement is required"
                ));
                Vec::new()
            }
            Err(error) => {
                errors.push(format!("hazard {hazard_id}: {error}"));
                Vec::new()
            }
        };
        let mut mitigation_set = BTreeSet::new();
        for requirement in &mitigation_ids {
            if !mitigation_set.insert(requirement.clone()) {
                errors.push(format!(
                    "hazard {hazard_id}: duplicate mitigation requirement {requirement}"
                ));
            }
            if !requirement_set.contains(requirement) {
                errors.push(format!(
                    "hazard {hazard_id}: requirement {requirement} is not in declared safety scope"
                ));
            }
            linked_requirements.insert(requirement.clone());
            if safety_hazard_by_id
                .get(requirement)
                .is_some_and(|assessment_hazard| assessment_hazard != hazard_id)
            {
                errors.push(format!(
                    "hazard {hazard_id}: assessment for {requirement} names a different hazard"
                ));
            }
        }
        let verification_refs = match hazard.get("verification_refs").and_then(array) {
            Some(refs) if !refs.is_empty() && refs.len() <= MAX_VERIFICATION_REFS_PER_HAZARD => {
                refs
            }
            Some(refs) if refs.len() > MAX_VERIFICATION_REFS_PER_HAZARD => {
                errors.push(format!(
                    "hazard {hazard_id}: verification_refs cap exceeded (maximum {})",
                    MAX_VERIFICATION_REFS_PER_HAZARD
                ));
                &[]
            }
            _ => {
                errors.push(format!(
                    "hazard {hazard_id}: at least one bounded verification_ref is required"
                ));
                &[]
            }
        };
        for (ref_index, ref_value) in verification_refs.iter().enumerate() {
            let Some(reference) = object(ref_value) else {
                errors.push(format!(
                    "hazard {hazard_id}: verification_refs[{ref_index}] must be an object"
                ));
                continue;
            };
            reject_unknown_keys(
                reference,
                &["requirement_id", "file", "test_name"],
                "verification ref",
                &mut errors,
            );
            let requirement = match reference.get("requirement_id") {
                Some(value) => text(value, "verification_ref.requirement_id").unwrap_or(""),
                None => "",
            };
            let file = match reference.get("file") {
                Some(value) => text(value, "verification_ref.file").unwrap_or(""),
                None => "",
            };
            let test_name = match reference.get("test_name") {
                Some(value) => text(value, "verification_ref.test_name").unwrap_or(""),
                None => "",
            };
            if requirement.is_empty() || file.is_empty() || test_name.is_empty() {
                errors.push(format!(
                    "hazard {hazard_id}: verification ref is incomplete"
                ));
                continue;
            }
            if !mitigation_ids.iter().any(|id| id == requirement) {
                errors.push(format!(
                    "hazard {hazard_id}: verification ref requirement is not a mitigation"
                ));
            }
            let exists = report.covered.get(requirement).is_some_and(|tests| {
                tests
                    .iter()
                    .any(|item| !item.proof && item.file == file && item.name == test_name)
            });
            if !exists {
                errors.push(format!("hazard {hazard_id}: {file}::{test_name} is not a linked test for {requirement}"));
            }
        }
        let proof_refs = match hazard.get("proof_refs").and_then(array) {
            Some(refs) if refs.len() <= MAX_REFS_PER_HAZARD => refs,
            Some(_) => {
                errors.push(format!("hazard {hazard_id}: proof_refs cap exceeded"));
                &[]
            }
            None => {
                errors.push(format!("hazard {hazard_id}: proof_refs must be an array"));
                &[]
            }
        };
        for (ref_index, ref_value) in proof_refs.iter().enumerate() {
            let Some(reference) = object(ref_value) else {
                errors.push(format!(
                    "hazard {hazard_id}: proof_refs[{ref_index}] must be an object"
                ));
                continue;
            };
            reject_unknown_keys(
                reference,
                &["requirement_id", "file", "proof_name"],
                "proof ref",
                &mut errors,
            );
            let requirement = text_field(reference, "requirement_id").unwrap_or("");
            let file = text_field(reference, "file").unwrap_or("");
            let proof_name = text_field(reference, "proof_name").unwrap_or("");
            if requirement.is_empty() || file.is_empty() || proof_name.is_empty() {
                errors.push(format!("hazard {hazard_id}: proof ref is incomplete"));
                continue;
            }
            if !mitigation_ids.iter().any(|id| id == requirement) {
                errors.push(format!(
                    "hazard {hazard_id}: proof ref requirement is not a mitigation"
                ));
            }
            let exists = report.covered.get(requirement).is_some_and(|refs| {
                refs.iter()
                    .any(|item| item.proof && item.file == file && item.name == proof_name)
            });
            if !exists {
                errors.push(format!("hazard {hazard_id}: {file}::{proof_name} is not a linked proof for {requirement}"));
            }
        }
    }
    for requirement in &requirements {
        if !linked_requirements.contains(requirement) {
            errors.push(format!(
                "safety requirement {requirement} has no hazard link"
            ));
        }
    }
    let mut source_errors = check_source_refs(log);
    errors.append(&mut source_errors);
    errors.sort();
    errors
}

fn disposition_rows<'a>(
    log: &'a BTreeMap<String, JsonValue>,
    key: &str,
    allowed_keys: &[&str],
    cap: usize,
    errors: &mut Vec<String>,
) -> Vec<&'a BTreeMap<String, JsonValue>> {
    let Some(values) = log.get(key).and_then(array) else {
        errors.push(format!("{key} must be an array"));
        return Vec::new();
    };
    if values.len() > cap {
        errors.push(format!("{key} cap exceeded"));
        return Vec::new();
    }
    let mut rows = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let Some(row) = object(value) else {
            errors.push(format!("{key}[{index}] must be an object"));
            continue;
        };
        reject_unknown_keys(row, allowed_keys, key, errors);
        rows.push(row);
    }
    rows
}

fn text_field<'a>(object: &'a BTreeMap<String, JsonValue>, name: &str) -> Option<&'a str> {
    object.get(name).and_then(|value| match value {
        JsonValue::String(value) => Some(value.as_str()),
        _ => None,
    })
}

fn check_source_refs(log: &BTreeMap<String, JsonValue>) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(hazards) = log.get("hazards").and_then(array) else {
        return errors;
    };
    for (index, value) in hazards.iter().enumerate() {
        let Some(hazard) = object(value) else {
            continue;
        };
        let hazard_id = text_field(hazard, "id").unwrap_or("");
        match hazard.get("source_refs").and_then(array) {
            Some(refs) if !refs.is_empty() && refs.len() <= MAX_REFS_PER_HAZARD => {
                for (ref_index, source) in refs.iter().enumerate() {
                    let Some(source) = object(source) else {
                        errors.push(format!(
                            "hazard {hazard_id}: source_refs[{ref_index}] must be an object"
                        ));
                        continue;
                    };
                    reject_unknown_keys(
                        source,
                        &["document", "locator"],
                        "source ref",
                        &mut errors,
                    );
                    if text_field(source, "document").is_none_or(str::is_empty)
                        || text_field(source, "locator").is_none_or(str::is_empty)
                    {
                        errors.push(format!(
                            "hazard {hazard_id}: source ref needs document and locator"
                        ));
                    }
                }
            }
            _ => errors.push(format!(
                "hazards[{index}] {hazard_id}: source_refs must be nonempty and bounded"
            )),
        }
    }
    errors
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

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct TempRoot(std::path::PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let suffix = NEXT.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir()
                .join(format!("oa-hazard-trace-{}-{suffix}", std::process::id()));
            fs::create_dir_all(path.join("docs/safety")).unwrap();
            fs::create_dir_all(path.join("tests")).unwrap();
            fs::write(
                path.join("REQUIREMENTS.md"),
                "- **OA-AAA-001** synthetic safety requirement\n",
            )
            .unwrap();
            fs::write(
                path.join("tests/safety.rs"),
                "// REQ: OA-AAA-001\n#[test]\nfn safety_fixture() {}\n",
            )
            .unwrap();
            Self(path)
        }

        fn write_log(&self, content: &str) {
            fs::write(self.0.join(HAZARD_LOG_PATH), content).unwrap();
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> String {
        r#"{
          "schema":"openatca.hazard-log",
          "version":1,
          "assessment_status":"DRAFT_NOT_ASSESSED",
          "coverage_status":"DECLARED_FUNCTION_SCOPE_ONLY",
          "safety_requirement_ids":["OA-AAA-001"],
          "safety_requirement_assessments":[{"id":"OA-AAA-001","function":"synthetic","hazard_id":"HZ-SYN-1","rationale":"Synthetic requirement-to-hazard assessment."}],
          "non_safety_requirement_assessments":[],
          "unresolved_requirement_assessments":[],
          "withdrawn_requirement_assessments":[],
          "unclassified_requirement_policy":{"classification":"unresolved","function":"unassigned","rationale":"synthetic fixture"},
          "hazards":[{
            "id":"HZ-SYN-1",
            "function":"synthetic",
            "cause":"synthetic input fault",
            "effect":"synthetic test effect",
            "severity_class":"Major",
            "severity_status":"engineering_candidate",
            "severity_rationale":"synthetic fixture only",
            "mitigation_requirement_ids":["OA-AAA-001"],
            "source_refs":[{"document":"synthetic","locator":"fixture"}],
            "verification_refs":[{"requirement_id":"OA-AAA-001","file":"tests/safety.rs","test_name":"safety_fixture"}]
            ,"proof_refs":[]
          }]
        }"#
        .to_string()
    }

    fn fixture_with_verification_count(count: usize) -> String {
        let reference = r#"{"requirement_id":"OA-AAA-001","file":"tests/safety.rs","test_name":"safety_fixture"}"#;
        let refs = std::iter::repeat_n(reference, count)
            .collect::<Vec<_>>()
            .join(",");
        fixture().replace(
            &format!("\"verification_refs\":[{reference}]"),
            &format!("\"verification_refs\":[{refs}]"),
        )
    }

    // REQ: OA-MIG-602
    #[test]
    fn declared_hazard_requirement_and_test_links_resolve() {
        let root = TempRoot::new();
        root.write_log(&fixture());
        assert!(check(&root.0).is_empty());
    }

    // REQ: OA-MIG-602
    #[test]
    fn verification_reference_cap_accepts_160_and_refuses_161() {
        let root = TempRoot::new();
        root.write_log(&fixture_with_verification_count(
            MAX_VERIFICATION_REFS_PER_HAZARD,
        ));
        assert!(check(&root.0).is_empty());

        root.write_log(&fixture_with_verification_count(
            MAX_VERIFICATION_REFS_PER_HAZARD + 1,
        ));
        assert!(check(&root.0)
            .iter()
            .any(|error| error.contains("verification_refs cap exceeded (maximum 160)")));
    }

    // REQ: OA-MIG-602
    #[test]
    fn proof_only_requirement_uses_a_proof_ref_and_hazard_keeps_a_direct_test() {
        let root = TempRoot::new();
        fs::write(
            root.0.join("REQUIREMENTS.md"),
            "- **OA-AAA-001** synthetic safety requirement\n- **OA-BBB-001** proof-backed safety requirement\n",
        )
        .unwrap();
        fs::write(
            root.0.join("tests/safety.rs"),
            "// REQ: OA-AAA-001\n#[test]\nfn safety_fixture() {}\n// REQ: OA-BBB-001\n#[kani::proof]\nfn proof_fixture() {}\n",
        )
        .unwrap();
        let mut log = fixture();
        log = log
            .replace("\"OA-AAA-001\"],", "\"OA-AAA-001\",\"OA-BBB-001\"],")
            .replace(
                "\"safety_requirement_assessments\":[{\"id\":\"OA-AAA-001\",\"function\":\"synthetic\",\"hazard_id\":\"HZ-SYN-1\",\"rationale\":\"Synthetic requirement-to-hazard assessment.\"}]",
                "\"safety_requirement_assessments\":[{\"id\":\"OA-AAA-001\",\"function\":\"synthetic\",\"hazard_id\":\"HZ-SYN-1\",\"rationale\":\"Synthetic requirement-to-hazard assessment.\"},{\"id\":\"OA-BBB-001\",\"function\":\"synthetic\",\"hazard_id\":\"HZ-SYN-1\",\"rationale\":\"Synthetic proof-backed requirement-to-hazard assessment.\"}]",
            )
            .replace(
                "\"proof_refs\":[]",
                "\"proof_refs\":[{\"requirement_id\":\"OA-BBB-001\",\"file\":\"tests/safety.rs\",\"proof_name\":\"proof_fixture\"}]",
            );
        root.write_log(&log);
        assert!(check(&root.0).is_empty());
    }

    // REQ: OA-MIG-602
    #[test]
    fn withdrawn_register_clause_requires_a_separate_source_bound_disposition() {
        let root = TempRoot::new();
        fs::write(
            root.0.join("REQUIREMENTS.md"),
            "- **OA-AAA-001** WITHDRAWN: replaced by OA-BBB-001\n- **OA-BBB-001** active requirement\n",
        )
        .unwrap();
        fs::write(
            root.0.join("tests/safety.rs"),
            "// REQ: OA-BBB-001\n#[test]\nfn safety_fixture() {}\n",
        )
        .unwrap();
        let log = fixture()
            .replace("OA-AAA-001", "OA-BBB-001")
            .replace(
                "\"withdrawn_requirement_assessments\":[]",
                "\"withdrawn_requirement_assessments\":[{\"id\":\"OA-AAA-001\",\"source_document\":\"REQUIREMENTS.md\",\"source_clause\":\"WITHDRAWN: replaced by OA-BBB-001\",\"superseded_by\":[\"OA-BBB-001\"],\"rationale\":\"Register marks the clause withdrawn and names the replacement.\"}]",
            );
        root.write_log(&log);
        assert!(check(&root.0).is_empty());
        root.write_log(&log.replace(
            "OA-BBB-001\"],\"rationale",
            "OA-NOT-ACTIVE-001\"],\"rationale",
        ));
        assert!(check(&root.0)
            .iter()
            .any(|error| error.contains("replacement OA-NOT-ACTIVE-001 is not active")));
    }

    // REQ: OA-MIG-602
    #[test]
    fn missing_hazard_link_and_wrong_test_refuse() {
        let root = TempRoot::new();
        let invalid = fixture().replace("safety_fixture", "not_a_test").replace(
            "\"mitigation_requirement_ids\":[\"OA-AAA-001\"]",
            "\"mitigation_requirement_ids\":[]",
        );
        root.write_log(&invalid);
        let errors = check(&root.0);
        assert!(errors
            .iter()
            .any(|error| error.contains("at least one requirement")));
        assert!(errors.iter().any(|error| error.contains("no hazard link")));
        assert!(errors
            .iter()
            .any(|error| error.contains("is not a linked test")));

        // Keep every other field valid so removing the assessment-to-hazard consistency guard
        // makes this same test fail.
        let mismatched_hazard =
            fixture().replace("\"hazard_id\":\"HZ-SYN-1\"", "\"hazard_id\":\"HZ-OTHER-1\"");
        root.write_log(&mismatched_hazard);
        let errors = check(&root.0);
        assert!(errors
            .iter()
            .any(|error| { error.contains("assessment for OA-AAA-001 names a different hazard") }));

        // Keep the log and hazard links internally coherent, but make its safety requirement
        // absent from the active register so removing the active-requirement guard is detected.
        fs::write(
            root.0.join("REQUIREMENTS.md"),
            "- **OA-BBB-001** unrelated active requirement\n",
        )
        .unwrap();
        root.write_log(&fixture());
        let errors = check(&root.0);
        assert!(errors.iter().any(|error| {
            error.contains("safety assessment OA-AAA-001 is not an active requirement")
        }));
    }

    // REQ: OA-MIG-602
    #[test]
    fn parser_rejects_duplicate_fields_and_resource_caps() {
        let duplicate = JsonParser::new(br#"{"a":1,"a":2}"#).parse();
        assert!(duplicate.is_err());
        let root = TempRoot::new();
        root.write_log(&" ".repeat(MAX_LOG_BYTES + 1));
        assert!(check(&root.0)
            .iter()
            .any(|error| error.contains("byte cap exceeded")));
    }

    // REQ: OA-MIG-602
    #[test]
    fn unsupported_schema_version_is_refused() {
        let root = TempRoot::new();
        root.write_log(&fixture().replace("\"version\":1", "\"version\":2"));
        assert!(check(&root.0)
            .iter()
            .any(|error| error.contains("version must be exactly 1")));
    }
}

#![forbid(unsafe_code)]
//! Requirements: OA-VEC-001 through OA-VEC-004 (REQUIREMENTS.md).

use oa_asterix::{
    decode_block_with, Block, Cat021Edition, ErrorKind, NO_ITEM, NO_OFFSET, NO_RECORD,
};
use oa_json::{Doc, Kind, Limits, NodeIx};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_CASE_BYTES: usize = 64 * 1024;
const MAX_BINARY_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_BINARY_BYTES: usize = 64 * 1024 * 1024;
const MAX_CASES: usize = 2048;
const MAX_DEPTH: usize = 32;
const MAX_MANIFEST_NODES: usize = 32_768;
const MAX_INVENTORY_ENTRIES: usize = MAX_CASES * (MAX_DEPTH + 1) + 1;
const MAX_NODES: usize = 16_384;
const MAX_STRING_BYTES: usize = 8 * 1024;
const MAX_PATH_BYTES: usize = 192;

fn fail<T>(message: impl Into<String>) -> Result<T, String> {
    Err(message.into())
}

fn field(doc: &Doc<'_>, object: NodeIx, key: &str) -> Result<NodeIx, String> {
    doc.get(object, key)
        .ok_or_else(|| format!("missing field {key:?}"))
}

fn string<'a>(doc: &'a Doc<'_>, node: NodeIx, what: &str) -> Result<&'a str, String> {
    doc.str(node)
        .ok_or_else(|| format!("{what} must be a string"))
}

fn unsigned(doc: &Doc<'_>, node: NodeIx, what: &str) -> Result<u64, String> {
    let text = doc
        .number_text(node)
        .ok_or_else(|| format!("{what} must be an integer"))?;
    if text.starts_with('-') {
        return fail(format!("{what} must be unsigned"));
    }
    text.parse::<u64>()
        .map_err(|_| format!("{what} is outside u64"))
}

fn exact_fields(
    doc: &Doc<'_>,
    object: NodeIx,
    expected: &[&str],
    what: &str,
) -> Result<(), String> {
    if doc.kind(object) != Some(Kind::Object) {
        return fail(format!("{what} must be an object"));
    }
    let mut found = BTreeSet::new();
    for (key, _) in doc.members(object) {
        let key = string(doc, key, "object key")?;
        if !found.insert(key) {
            return fail(format!("{what} has duplicate key {key:?}"));
        }
        if !expected.contains(&key) {
            return fail(format!("{what} has unknown field {key:?}"));
        }
    }
    for key in expected {
        if !found.contains(key) {
            return fail(format!("{what} is missing field {key:?}"));
        }
    }
    Ok(())
}

fn duplicate_scan(doc: &Doc<'_>, node: NodeIx, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return fail("JSON nesting exceeds oacv v1 limit");
    }
    match doc.kind(node) {
        Some(Kind::Object) => {
            let mut found = BTreeSet::new();
            for (key, value) in doc.members(node) {
                let text = string(doc, key, "object key")?;
                if !found.insert(text) {
                    return fail(format!("duplicate JSON object key {text:?}"));
                }
                duplicate_scan(doc, value, depth + 1)?;
            }
        }
        Some(Kind::Array) => {
            for child in doc.elements(node) {
                duplicate_scan(doc, child, depth + 1)?;
            }
        }
        Some(_) => {}
        None => return fail("invalid JSON node"),
    }
    Ok(())
}

fn parse_json<'a>(
    bytes: &'a [u8],
    max_bytes: usize,
    max_nodes: usize,
    what: &str,
) -> Result<Doc<'a>, String> {
    if bytes.len() > max_bytes {
        return fail(format!("{what} exceeds {max_bytes} bytes"));
    }
    let limits = Limits {
        max_input_bytes: max_bytes,
        max_depth: MAX_DEPTH,
        max_string_bytes: MAX_STRING_BYTES,
        max_nodes,
    };
    let doc = oa_json::parse(bytes, &limits).map_err(|e| format!("{what}: {e}"))?;
    duplicate_scan(&doc, NodeIx::ROOT, 0).map_err(|e| format!("{what}: {e}"))?;
    Ok(doc)
}

fn safe_rel_path(text: &str) -> Result<PathBuf, String> {
    if text.is_empty()
        || text.len() > MAX_PATH_BYTES
        || !text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._/-".contains(&b))
    {
        return fail(format!("invalid corpus path {text:?}"));
    }
    let path = Path::new(text);
    if path.is_absolute()
        || path.components().count() > MAX_DEPTH
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return fail(format!("unsafe corpus path {text:?}"));
    }
    Ok(path.to_path_buf())
}

fn read_member(root: &Path, relative: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let rel = safe_rel_path(relative)?;
    let mut path = root.to_path_buf();
    for part in rel.components() {
        let Component::Normal(name) = part else {
            return fail("non-normal corpus path component");
        };
        path.push(name);
        let metadata =
            fs::symlink_metadata(&path).map_err(|e| format!("corpus member {relative:?}: {e}"))?;
        if metadata.file_type().is_symlink() {
            return fail(format!("symlink in corpus path {relative:?}"));
        }
    }
    let file = fs::File::open(&path).map_err(|e| format!("corpus member {relative:?}: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("corpus member {relative:?}: {e}"))?;
    if !metadata.is_file() || metadata.len() > max_bytes as u64 {
        return fail(format!(
            "corpus member {relative:?} is not a bounded regular file"
        ));
    }
    let cap = u64::try_from(max_bytes)
        .map_err(|_| "corpus byte limit does not fit u64")?
        .checked_add(1)
        .ok_or("corpus byte limit overflow")?;
    let mut bytes = Vec::new();
    file.take(cap)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("corpus member {relative:?}: {e}"))?;
    if bytes.len() > max_bytes {
        return fail(format!(
            "corpus member {relative:?} grew beyond its byte limit"
        ));
    }
    Ok(bytes)
}

fn hex_digest(doc: &Doc<'_>, node: NodeIx, what: &str) -> Result<String, String> {
    let text = string(doc, node, what)?;
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return fail(format!("{what} must be 64 lowercase hexadecimal digits"));
    }
    Ok(text.to_owned())
}

fn valid_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 192
        && !text.starts_with('.')
        && !text.ends_with('.')
        && !text.contains("..")
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_')
}

fn verify_digest(bytes: &[u8], expected: &str, what: &str) -> Result<(), String> {
    let actual = oa_sha256::to_hex(&oa_sha256::sha256(bytes));
    if actual != expected {
        return fail(format!("{what} SHA-256 mismatch"));
    }
    Ok(())
}

fn run_sha256(doc: &Doc<'_>, case: NodeIx, input: &[u8]) -> Result<(), String> {
    if string(doc, field(doc, case, "module")?, "module")? != "sha256"
        || string(doc, field(doc, case, "operation")?, "operation")? != "digest"
    {
        return fail("unsupported SHA-256 case identity");
    }
    let expected = field(doc, field(doc, case, "expected")?, "value")?;
    exact_fields(doc, expected, &["sha256_hex"], "SHA-256 expected value")?;
    let want = hex_digest(doc, field(doc, expected, "sha256_hex")?, "sha256_hex")?;
    let got = oa_sha256::to_hex(&oa_sha256::sha256(input));
    if got != want {
        return fail("SHA-256 known-answer mismatch");
    }
    Ok(())
}

fn expected_unsigned(doc: &Doc<'_>, object: NodeIx, key: &str, actual: u64) -> Result<(), String> {
    let expected = unsigned(doc, field(doc, object, key)?, key)?;
    if actual != expected {
        return fail(format!("{key} mismatch: got {actual}, expected {expected}"));
    }
    Ok(())
}

fn expected_signed(doc: &Doc<'_>, object: NodeIx, key: &str, actual: i64) -> Result<(), String> {
    let node = field(doc, object, key)?;
    let expected = doc
        .as_i64(node)
        .ok_or_else(|| format!("{key} must be an i64"))?;
    if actual != expected {
        return fail(format!("{key} mismatch: got {actual}, expected {expected}"));
    }
    Ok(())
}

fn expected_bool(doc: &Doc<'_>, object: NodeIx, key: &str, actual: bool) -> Result<(), String> {
    let expected = doc
        .as_bool(field(doc, object, key)?)
        .ok_or_else(|| format!("{key} must be a boolean"))?;
    if actual != expected {
        return fail(format!("{key} mismatch: got {actual}, expected {expected}"));
    }
    Ok(())
}

fn run_asterix(doc: &Doc<'_>, case: NodeIx, input: &[u8]) -> Result<(), String> {
    if string(doc, field(doc, case, "module")?, "module")? != "asterix"
        || string(doc, field(doc, case, "operation")?, "operation")? != "decode"
    {
        return fail("unsupported ASTERIX vector identity");
    }
    match string(doc, field(doc, case, "edition")?, "edition")? {
        "CAT021/2.4" => run_cat021(doc, case, input, Cat021Edition::Ed24),
        "CAT021/2.7" => run_cat021(doc, case, input, Cat021Edition::Ed27),
        "CAT048/1.32" => run_cat048(doc, case, input),
        "CAT062/1.21" => run_cat062(doc, case, input),
        _ => fail("unsupported ASTERIX edition"),
    }
}

fn run_cat021(
    doc: &Doc<'_>,
    case: NodeIx,
    input: &[u8],
    edition: Cat021Edition,
) -> Result<(), String> {
    let expected = field(doc, case, "expected")?;
    match string(doc, field(doc, expected, "kind")?, "expected.kind")? {
        "success" => {
            let value = field(doc, expected, "value")?;
            exact_fields(
                doc,
                value,
                &["projection", "category", "record_count", "records"],
                "CAT021 projection",
            )?;
            if string(doc, field(doc, value, "projection")?, "projection")? != "cat021-minimal-v1" {
                return fail("unsupported CAT021 normalized projection");
            }
            let (block, consumed) = decode_block_with(input, edition)
                .map_err(|error| format!("unexpected ASTERIX refusal: {error}"))?;
            if consumed != input.len() {
                return fail("CAT021 vector must contain exactly one block with no trailing bytes");
            }
            expected_unsigned(doc, value, "category", 21)?;
            let Block::Cat021(records) = block else {
                return fail("CAT021 decoder returned a different category");
            };
            expected_unsigned(
                doc,
                value,
                "record_count",
                u64::try_from(records.len()).map_err(|_| "record count does not fit u64")?,
            )?;
            if records.len() != 1 {
                return fail("the initial CAT021 projection requires exactly one record");
            }
            let record_expected = doc
                .elements(field(doc, value, "records")?)
                .next()
                .ok_or("CAT021 projection has no expected record")?;
            if doc.len(field(doc, value, "records")?) != 1 {
                return fail("CAT021 projection expected records must contain exactly one row");
            }
            exact_fields(
                doc,
                record_expected,
                &[
                    "source_sac",
                    "source_sic",
                    "descriptor_atp",
                    "descriptor_arc",
                    "descriptor_range_check",
                    "descriptor_field_monitor",
                    "descriptor_extensions",
                    "position_hr_lat_raw",
                    "position_hr_lon_raw",
                    "address_24bit",
                    "quality_nacv",
                    "quality_nic",
                    "quality_extensions",
                    "mode3a_code_raw",
                    "flight_level_quarters",
                    "raw_item_count",
                ],
                "CAT021 normalized record projection",
            )?;
            let record = records
                .first()
                .ok_or("decoded CAT021 block has no record")?;
            expected_unsigned(
                doc,
                record_expected,
                "source_sac",
                u64::from(record.source.sac),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "source_sic",
                u64::from(record.source.sic),
            )?;
            let descriptor = record
                .descriptor
                .as_ref()
                .ok_or("CAT021 descriptor absent")?;
            expected_unsigned(
                doc,
                record_expected,
                "descriptor_atp",
                u64::from(descriptor.atp),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "descriptor_arc",
                u64::from(descriptor.arc),
            )?;
            expected_bool(
                doc,
                record_expected,
                "descriptor_range_check",
                descriptor.range_check,
            )?;
            expected_bool(
                doc,
                record_expected,
                "descriptor_field_monitor",
                descriptor.field_monitor,
            )?;
            compare_u8_array(
                doc,
                field(doc, record_expected, "descriptor_extensions")?,
                &descriptor.ext,
                "descriptor_extensions",
            )?;
            let position = record
                .position_hr
                .ok_or("CAT021 high-resolution position absent")?;
            expected_signed(
                doc,
                record_expected,
                "position_hr_lat_raw",
                i64::from(position.lat),
            )?;
            expected_signed(
                doc,
                record_expected,
                "position_hr_lon_raw",
                i64::from(position.lon),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "address_24bit",
                u64::from(record.address.ok_or("CAT021 address absent")?),
            )?;
            let quality = record.quality.as_ref().ok_or("CAT021 quality absent")?;
            expected_unsigned(
                doc,
                record_expected,
                "quality_nacv",
                u64::from(quality.nacv),
            )?;
            expected_unsigned(doc, record_expected, "quality_nic", u64::from(quality.nic))?;
            compare_u8_array(
                doc,
                field(doc, record_expected, "quality_extensions")?,
                &quality.ext,
                "quality_extensions",
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "mode3a_code_raw",
                u64::from(record.mode3a.ok_or("CAT021 Mode 3/A absent")?),
            )?;
            expected_signed(
                doc,
                record_expected,
                "flight_level_quarters",
                i64::from(record.flight_level.ok_or("CAT021 flight level absent")?),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "raw_item_count",
                u64::try_from(record.raw.len()).map_err(|_| "raw item count does not fit u64")?,
            )?;
            Ok(())
        }
        "refusal" => {
            let error_expected = field(doc, expected, "error")?;
            exact_fields(
                doc,
                error_expected,
                &[
                    "code",
                    "category",
                    "record_index",
                    "item_number",
                    "subitem_number",
                    "byte_offset",
                ],
                "ASTERIX refusal expectation",
            )?;
            let error = decode_block_with(input, edition)
                .err()
                .ok_or("expected ASTERIX refusal, decoder accepted input")?;
            let code = asterix_error_code(error.kind);
            if string(doc, field(doc, error_expected, "code")?, "error.code")? != code {
                return fail(format!("ASTERIX refusal code mismatch: got {code}"));
            }
            expected_unsigned(doc, error_expected, "category", u64::from(error.cat))?;
            compare_optional_u64(
                doc,
                error_expected,
                "record_index",
                u64::from(error.record),
                u64::from(NO_RECORD),
            )?;
            compare_optional_u64(
                doc,
                error_expected,
                "item_number",
                u64::from(error.item),
                u64::from(NO_ITEM),
            )?;
            expected_unsigned(doc, error_expected, "subitem_number", u64::from(error.sub))?;
            if error.offset == NO_OFFSET {
                return fail("decoder refusal unexpectedly has no input byte offset");
            }
            expected_unsigned(
                doc,
                error_expected,
                "byte_offset",
                u64::try_from(error.offset).map_err(|_| "ASTERIX offset does not fit u64")?,
            )?;
            Ok(())
        }
        other => fail(format!("unsupported ASTERIX expected outcome {other:?}")),
    }
}

fn run_cat048(doc: &Doc<'_>, case: NodeIx, input: &[u8]) -> Result<(), String> {
    let expected = field(doc, case, "expected")?;
    match string(doc, field(doc, expected, "kind")?, "expected.kind")? {
        "success" => {
            let value = field(doc, expected, "value")?;
            exact_fields(
                doc,
                value,
                &["projection", "category", "record_count", "records"],
                "CAT048 projection",
            )?;
            if string(doc, field(doc, value, "projection")?, "projection")? != "cat048-minimal-v1" {
                return fail("unsupported CAT048 normalized projection");
            }
            let (block, consumed) = decode_block_with(input, Cat021Edition::Ed27)
                .map_err(|error| format!("unexpected ASTERIX refusal: {error}"))?;
            if consumed != input.len() {
                return fail("CAT048 vector must contain exactly one block with no trailing bytes");
            }
            expected_unsigned(doc, value, "category", 48)?;
            let Block::Cat048(records) = block else {
                return fail("CAT048 decoder returned a different category");
            };
            expected_unsigned(
                doc,
                value,
                "record_count",
                u64::try_from(records.len()).map_err(|_| "record count does not fit u64")?,
            )?;
            if records.len() != 1 || doc.len(field(doc, value, "records")?) != 1 {
                return fail("the initial CAT048 projection requires one record");
            }
            let record_expected = doc
                .elements(field(doc, value, "records")?)
                .next()
                .ok_or("CAT048 projection has no expected record")?;
            exact_fields(
                doc,
                record_expected,
                &[
                    "source_sac",
                    "source_sic",
                    "time_of_day_1_128_s",
                    "descriptor_type",
                    "descriptor_simulated",
                    "descriptor_rdp_chain2",
                    "descriptor_spi",
                    "descriptor_field_monitor",
                    "descriptor_extensions",
                    "polar_range_1_256_nm",
                    "polar_azimuth_1_65536_turn",
                    "mode3a_not_validated",
                    "mode3a_garbled",
                    "mode3a_l_or_change",
                    "mode3a_code_raw",
                    "flight_level_not_validated",
                    "flight_level_garbled",
                    "flight_level_quarters",
                    "plot_ssr_runlength",
                    "plot_ssr_replies",
                    "plot_ssr_amplitude_dbm",
                    "plot_psr_runlength",
                    "plot_psr_amplitude_dbm",
                    "plot_range_difference_1_256_nm",
                    "plot_azimuth_difference_1_16384_turn",
                    "address_24bit",
                    "ident_ascii_bytes",
                    "track_number",
                    "cartesian_x_1_128_nm",
                    "cartesian_y_1_128_nm",
                    "track_tentative",
                    "track_radar_type",
                    "track_doubtful",
                    "track_manoeuvre",
                    "track_confirmation",
                    "track_extensions",
                    "raw_item_count",
                ],
                "CAT048 normalized record projection",
            )?;
            let record = records
                .first()
                .ok_or("decoded CAT048 block has no record")?;
            expected_unsigned(
                doc,
                record_expected,
                "source_sac",
                u64::from(record.source.sac),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "source_sic",
                u64::from(record.source.sic),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "time_of_day_1_128_s",
                u64::from(record.time_of_day.ok_or("CAT048 time of day absent")?),
            )?;
            let descriptor = record
                .descriptor
                .as_ref()
                .ok_or("CAT048 descriptor absent")?;
            expected_unsigned(
                doc,
                record_expected,
                "descriptor_type",
                u64::from(descriptor.typ),
            )?;
            expected_bool(
                doc,
                record_expected,
                "descriptor_simulated",
                descriptor.simulated,
            )?;
            expected_bool(
                doc,
                record_expected,
                "descriptor_rdp_chain2",
                descriptor.rdp_chain2,
            )?;
            expected_bool(doc, record_expected, "descriptor_spi", descriptor.spi)?;
            expected_bool(
                doc,
                record_expected,
                "descriptor_field_monitor",
                descriptor.field_monitor,
            )?;
            compare_u8_array(
                doc,
                field(doc, record_expected, "descriptor_extensions")?,
                &descriptor.ext,
                "descriptor_extensions",
            )?;
            let polar = record.polar.ok_or("CAT048 polar position absent")?;
            expected_unsigned(
                doc,
                record_expected,
                "polar_range_1_256_nm",
                u64::from(polar.rho),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "polar_azimuth_1_65536_turn",
                u64::from(polar.theta),
            )?;
            let mode3a = record.mode3a.ok_or("CAT048 Mode 3/A absent")?;
            expected_bool(
                doc,
                record_expected,
                "mode3a_not_validated",
                mode3a.not_validated,
            )?;
            expected_bool(doc, record_expected, "mode3a_garbled", mode3a.garbled)?;
            expected_bool(
                doc,
                record_expected,
                "mode3a_l_or_change",
                mode3a.l_or_change,
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "mode3a_code_raw",
                u64::from(mode3a.code),
            )?;
            let flight_level = record.flight_level.ok_or("CAT048 flight level absent")?;
            expected_bool(
                doc,
                record_expected,
                "flight_level_not_validated",
                flight_level.not_validated,
            )?;
            expected_bool(
                doc,
                record_expected,
                "flight_level_garbled",
                flight_level.garbled,
            )?;
            expected_signed(
                doc,
                record_expected,
                "flight_level_quarters",
                i64::from(flight_level.raw),
            )?;
            let plot = record.plot.ok_or("CAT048 plot characteristics absent")?;
            compare_optional_u8(
                doc,
                record_expected,
                "plot_ssr_runlength",
                plot.ssr_runlength,
            )?;
            compare_optional_u8(doc, record_expected, "plot_ssr_replies", plot.ssr_replies)?;
            compare_optional_i8(
                doc,
                record_expected,
                "plot_ssr_amplitude_dbm",
                plot.ssr_amplitude,
            )?;
            compare_optional_u8(
                doc,
                record_expected,
                "plot_psr_runlength",
                plot.psr_runlength,
            )?;
            compare_optional_i8(
                doc,
                record_expected,
                "plot_psr_amplitude_dbm",
                plot.psr_amplitude,
            )?;
            compare_optional_i8(
                doc,
                record_expected,
                "plot_range_difference_1_256_nm",
                plot.range_diff,
            )?;
            compare_optional_i8(
                doc,
                record_expected,
                "plot_azimuth_difference_1_16384_turn",
                plot.azimuth_diff,
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "address_24bit",
                u64::from(record.address.ok_or("CAT048 address absent")?),
            )?;
            let ident = record.ident.ok_or("CAT048 identification absent")?;
            compare_u8_array(
                doc,
                field(doc, record_expected, "ident_ascii_bytes")?,
                &ident,
                "ident_ascii_bytes",
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "track_number",
                u64::from(record.track_number.ok_or("CAT048 track number absent")?),
            )?;
            let cartesian = record.cartesian.ok_or("CAT048 Cartesian position absent")?;
            expected_signed(
                doc,
                record_expected,
                "cartesian_x_1_128_nm",
                i64::from(cartesian.x),
            )?;
            expected_signed(
                doc,
                record_expected,
                "cartesian_y_1_128_nm",
                i64::from(cartesian.y),
            )?;
            let status = record
                .track_status
                .as_ref()
                .ok_or("CAT048 track status absent")?;
            expected_bool(doc, record_expected, "track_tentative", status.tentative)?;
            expected_unsigned(
                doc,
                record_expected,
                "track_radar_type",
                u64::from(status.rad),
            )?;
            expected_bool(doc, record_expected, "track_doubtful", status.doubtful)?;
            expected_bool(doc, record_expected, "track_manoeuvre", status.manoeuvre)?;
            expected_unsigned(
                doc,
                record_expected,
                "track_confirmation",
                u64::from(status.cdm),
            )?;
            compare_u8_array(
                doc,
                field(doc, record_expected, "track_extensions")?,
                &status.ext,
                "track_extensions",
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "raw_item_count",
                u64::try_from(record.raw.len()).map_err(|_| "raw item count does not fit u64")?,
            )?;
            Ok(())
        }
        "refusal" => {
            let error_expected = field(doc, expected, "error")?;
            exact_fields(
                doc,
                error_expected,
                &[
                    "code",
                    "category",
                    "record_index",
                    "item_number",
                    "subitem_number",
                    "byte_offset",
                ],
                "ASTERIX refusal expectation",
            )?;
            let error = decode_block_with(input, Cat021Edition::Ed27)
                .err()
                .ok_or("expected ASTERIX refusal, decoder accepted input")?;
            let code = asterix_error_code(error.kind);
            if string(doc, field(doc, error_expected, "code")?, "error.code")? != code {
                return fail(format!("ASTERIX refusal code mismatch: got {code}"));
            }
            expected_unsigned(doc, error_expected, "category", u64::from(error.cat))?;
            compare_optional_u64(
                doc,
                error_expected,
                "record_index",
                u64::from(error.record),
                u64::from(NO_RECORD),
            )?;
            compare_optional_u64(
                doc,
                error_expected,
                "item_number",
                u64::from(error.item),
                u64::from(NO_ITEM),
            )?;
            expected_unsigned(doc, error_expected, "subitem_number", u64::from(error.sub))?;
            if error.offset == NO_OFFSET {
                return fail("decoder refusal unexpectedly has no input byte offset");
            }
            expected_unsigned(
                doc,
                error_expected,
                "byte_offset",
                u64::try_from(error.offset).map_err(|_| "ASTERIX offset does not fit u64")?,
            )?;
            Ok(())
        }
        other => fail(format!("unsupported ASTERIX expected outcome {other:?}")),
    }
}

fn run_cat062(doc: &Doc<'_>, case: NodeIx, input: &[u8]) -> Result<(), String> {
    let expected = field(doc, case, "expected")?;
    match string(doc, field(doc, expected, "kind")?, "expected.kind")? {
        "success" => {
            let value = field(doc, expected, "value")?;
            exact_fields(
                doc,
                value,
                &["projection", "category", "record_count", "records"],
                "CAT062 projection",
            )?;
            if string(doc, field(doc, value, "projection")?, "projection")? != "cat062-minimal-v1" {
                return fail("unsupported CAT062 normalized projection");
            }
            let (block, consumed) = decode_block_with(input, Cat021Edition::Ed27)
                .map_err(|error| format!("unexpected ASTERIX refusal: {error}"))?;
            if consumed != input.len() {
                return fail("CAT062 vector must contain exactly one block with no trailing bytes");
            }
            expected_unsigned(doc, value, "category", 62)?;
            let Block::Cat062(records) = block else {
                return fail("CAT062 decoder returned a different category");
            };
            expected_unsigned(
                doc,
                value,
                "record_count",
                u64::try_from(records.len()).map_err(|_| "record count does not fit u64")?,
            )?;
            if records.len() != 1 || doc.len(field(doc, value, "records")?) != 1 {
                return fail("the initial CAT062 projection requires one record");
            }
            let record_expected = doc
                .elements(field(doc, value, "records")?)
                .next()
                .ok_or("CAT062 projection has no expected record")?;
            exact_fields(
                doc,
                record_expected,
                &[
                    "source_sac",
                    "source_sic",
                    "time_of_day_1_128_s",
                    "position_lat_180_over_2pow25_deg",
                    "position_lon_180_over_2pow25_deg",
                    "cartesian_x_0_5_m",
                    "cartesian_y_0_5_m",
                    "velocity_x_0_25_m_s",
                    "velocity_y_0_25_m_s",
                    "mode3a_not_validated",
                    "mode3a_garbled",
                    "mode3a_change",
                    "mode3a_code_raw",
                    "derived_address_24bit",
                    "derived_ident_ascii_bytes",
                    "selected_altitude_source_provided",
                    "selected_altitude_source",
                    "selected_altitude_25_ft",
                    "derived_other_subitem_count",
                    "track_number",
                    "track_monosensor",
                    "track_spi",
                    "track_geometric_more_reliable",
                    "track_altitude_source",
                    "track_tentative",
                    "track_extensions",
                    "age_track_quarter_s",
                    "age_psr_quarter_s",
                    "age_ssr_quarter_s",
                    "age_mode_s_quarter_s",
                    "age_ads_c_quarter_s",
                    "age_ads_b_es_quarter_s",
                    "age_ads_b_vdl_quarter_s",
                    "age_ads_b_uat_quarter_s",
                    "age_loop_quarter_s",
                    "age_mlat_quarter_s",
                    "measured_flight_level_quarters",
                    "geometric_altitude_6_25_ft",
                    "climb_rate_6_25_ft_min",
                    "raw_item_count",
                ],
                "CAT062 normalized record projection",
            )?;
            let record = records
                .first()
                .ok_or("decoded CAT062 block has no record")?;
            expected_unsigned(
                doc,
                record_expected,
                "source_sac",
                u64::from(record.source.sac),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "source_sic",
                u64::from(record.source.sic),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "time_of_day_1_128_s",
                u64::from(record.time.ok_or("CAT062 time absent")?),
            )?;
            let position = record.position.ok_or("CAT062 position absent")?;
            expected_signed(
                doc,
                record_expected,
                "position_lat_180_over_2pow25_deg",
                i64::from(position.lat),
            )?;
            expected_signed(
                doc,
                record_expected,
                "position_lon_180_over_2pow25_deg",
                i64::from(position.lon),
            )?;
            let cart = record.cartesian.ok_or("CAT062 Cartesian position absent")?;
            expected_signed(doc, record_expected, "cartesian_x_0_5_m", i64::from(cart.x))?;
            expected_signed(doc, record_expected, "cartesian_y_0_5_m", i64::from(cart.y))?;
            let velocity = record.velocity.ok_or("CAT062 velocity absent")?;
            expected_signed(
                doc,
                record_expected,
                "velocity_x_0_25_m_s",
                i64::from(velocity.vx),
            )?;
            expected_signed(
                doc,
                record_expected,
                "velocity_y_0_25_m_s",
                i64::from(velocity.vy),
            )?;
            let mode3a = record.mode3a.ok_or("CAT062 Mode 3/A absent")?;
            expected_bool(
                doc,
                record_expected,
                "mode3a_not_validated",
                mode3a.not_validated,
            )?;
            expected_bool(doc, record_expected, "mode3a_garbled", mode3a.garbled)?;
            expected_bool(doc, record_expected, "mode3a_change", mode3a.l_or_change)?;
            expected_unsigned(
                doc,
                record_expected,
                "mode3a_code_raw",
                u64::from(mode3a.code),
            )?;
            let derived = record
                .derived
                .as_ref()
                .ok_or("CAT062 derived data absent")?;
            expected_unsigned(
                doc,
                record_expected,
                "derived_address_24bit",
                u64::from(derived.address.ok_or("CAT062 derived address absent")?),
            )?;
            let ident = derived
                .ident
                .ok_or("CAT062 derived identification absent")?;
            compare_u8_array(
                doc,
                field(doc, record_expected, "derived_ident_ascii_bytes")?,
                &ident,
                "derived_ident_ascii_bytes",
            )?;
            let selected = derived
                .selected_altitude
                .ok_or("CAT062 selected altitude absent")?;
            expected_bool(
                doc,
                record_expected,
                "selected_altitude_source_provided",
                selected.source_provided,
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "selected_altitude_source",
                u64::from(selected.source),
            )?;
            expected_signed(
                doc,
                record_expected,
                "selected_altitude_25_ft",
                i64::from(selected.altitude),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "derived_other_subitem_count",
                u64::try_from(derived.other.len())
                    .map_err(|_| "derived subitem count does not fit u64")?,
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "track_number",
                u64::from(record.track_number.ok_or("CAT062 track number absent")?),
            )?;
            let status = record
                .track_status
                .as_ref()
                .ok_or("CAT062 track status absent")?;
            expected_bool(doc, record_expected, "track_monosensor", status.monosensor)?;
            expected_bool(doc, record_expected, "track_spi", status.spi)?;
            expected_bool(
                doc,
                record_expected,
                "track_geometric_more_reliable",
                status.geometric_more_reliable,
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "track_altitude_source",
                u64::from(status.altitude_source),
            )?;
            expected_bool(doc, record_expected, "track_tentative", status.tentative)?;
            compare_u8_array(
                doc,
                field(doc, record_expected, "track_extensions")?,
                &status.ext,
                "track_extensions",
            )?;
            let ages = record.update_ages.ok_or("CAT062 update ages absent")?;
            compare_optional_u8(doc, record_expected, "age_track_quarter_s", ages.track)?;
            compare_optional_u8(doc, record_expected, "age_psr_quarter_s", ages.psr)?;
            compare_optional_u8(doc, record_expected, "age_ssr_quarter_s", ages.ssr)?;
            compare_optional_u8(doc, record_expected, "age_mode_s_quarter_s", ages.mode_s)?;
            compare_optional_u16(doc, record_expected, "age_ads_c_quarter_s", ages.ads_c)?;
            compare_optional_u8(
                doc,
                record_expected,
                "age_ads_b_es_quarter_s",
                ages.ads_b_es,
            )?;
            compare_optional_u8(
                doc,
                record_expected,
                "age_ads_b_vdl_quarter_s",
                ages.ads_b_vdl,
            )?;
            compare_optional_u8(
                doc,
                record_expected,
                "age_ads_b_uat_quarter_s",
                ages.ads_b_uat,
            )?;
            compare_optional_u8(doc, record_expected, "age_loop_quarter_s", ages.loop_age)?;
            compare_optional_u8(doc, record_expected, "age_mlat_quarter_s", ages.mlat)?;
            expected_signed(
                doc,
                record_expected,
                "measured_flight_level_quarters",
                i64::from(
                    record
                        .measured_fl
                        .ok_or("CAT062 measured flight level absent")?,
                ),
            )?;
            expected_signed(
                doc,
                record_expected,
                "geometric_altitude_6_25_ft",
                i64::from(
                    record
                        .geometric_altitude
                        .ok_or("CAT062 geometric altitude absent")?,
                ),
            )?;
            expected_signed(
                doc,
                record_expected,
                "climb_rate_6_25_ft_min",
                i64::from(record.climb_rate.ok_or("CAT062 climb rate absent")?),
            )?;
            expected_unsigned(
                doc,
                record_expected,
                "raw_item_count",
                u64::try_from(record.raw.len()).map_err(|_| "raw item count does not fit u64")?,
            )?;
            Ok(())
        }
        "refusal" => {
            let error_expected = field(doc, expected, "error")?;
            exact_fields(
                doc,
                error_expected,
                &[
                    "code",
                    "category",
                    "record_index",
                    "item_number",
                    "subitem_number",
                    "byte_offset",
                ],
                "ASTERIX refusal expectation",
            )?;
            let error = decode_block_with(input, Cat021Edition::Ed27)
                .err()
                .ok_or("expected ASTERIX refusal, decoder accepted input")?;
            let code = asterix_error_code(error.kind);
            if string(doc, field(doc, error_expected, "code")?, "error.code")? != code {
                return fail(format!("ASTERIX refusal code mismatch: got {code}"));
            }
            expected_unsigned(doc, error_expected, "category", u64::from(error.cat))?;
            compare_optional_u64(
                doc,
                error_expected,
                "record_index",
                u64::from(error.record),
                u64::from(NO_RECORD),
            )?;
            compare_optional_u64(
                doc,
                error_expected,
                "item_number",
                u64::from(error.item),
                u64::from(NO_ITEM),
            )?;
            expected_unsigned(doc, error_expected, "subitem_number", u64::from(error.sub))?;
            if error.offset == NO_OFFSET {
                return fail("decoder refusal unexpectedly has no input byte offset");
            }
            expected_unsigned(
                doc,
                error_expected,
                "byte_offset",
                u64::try_from(error.offset).map_err(|_| "ASTERIX offset does not fit u64")?,
            )?;
            Ok(())
        }
        other => fail(format!("unsupported ASTERIX expected outcome {other:?}")),
    }
}

fn compare_u8_array(doc: &Doc<'_>, node: NodeIx, actual: &[u8], what: &str) -> Result<(), String> {
    if doc.kind(node) != Some(Kind::Array) || doc.len(node) != actual.len() {
        return fail(format!("{what} array length mismatch"));
    }
    for (index, expected) in doc.elements(node).enumerate() {
        let expected = unsigned(doc, expected, what)?;
        let actual = actual.get(index).ok_or("byte array index out of bounds")?;
        if expected != u64::from(*actual) {
            return fail(format!("{what}[{index}] mismatch"));
        }
    }
    Ok(())
}

fn compare_optional_u64(
    doc: &Doc<'_>,
    object: NodeIx,
    key: &str,
    actual: u64,
    absent: u64,
) -> Result<(), String> {
    let node = field(doc, object, key)?;
    if actual == absent {
        if doc.kind(node) != Some(Kind::Null) {
            return fail(format!("{key} must be null for this ASTERIX sentinel"));
        }
        return Ok(());
    }
    expected_unsigned(doc, object, key, actual)
}

fn compare_optional_u8(
    doc: &Doc<'_>,
    object: NodeIx,
    key: &str,
    actual: Option<u8>,
) -> Result<(), String> {
    match actual {
        Some(value) => expected_unsigned(doc, object, key, u64::from(value)),
        None => {
            if doc.kind(field(doc, object, key)?) == Some(Kind::Null) {
                Ok(())
            } else {
                fail(format!("{key} must be null"))
            }
        }
    }
}

fn compare_optional_i8(
    doc: &Doc<'_>,
    object: NodeIx,
    key: &str,
    actual: Option<i8>,
) -> Result<(), String> {
    match actual {
        Some(value) => expected_signed(doc, object, key, i64::from(value)),
        None => {
            if doc.kind(field(doc, object, key)?) == Some(Kind::Null) {
                Ok(())
            } else {
                fail(format!("{key} must be null"))
            }
        }
    }
}

fn compare_optional_u16(
    doc: &Doc<'_>,
    object: NodeIx,
    key: &str,
    actual: Option<u16>,
) -> Result<(), String> {
    match actual {
        Some(value) => expected_unsigned(doc, object, key, u64::from(value)),
        None => {
            if doc.kind(field(doc, object, key)?) == Some(Kind::Null) {
                Ok(())
            } else {
                fail(format!("{key} must be null"))
            }
        }
    }
}

fn asterix_error_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Truncated => "asterix.truncated",
        ErrorKind::BlockLengthBelowHeader => "asterix.block_length_below_header",
        ErrorKind::BlockLengthExceedsInput => "asterix.block_length_exceeds_input",
        ErrorKind::EmptyBlock => "asterix.empty_block",
        ErrorKind::TooManyRecords => "asterix.too_many_records",
        ErrorKind::UnsupportedCategory => "asterix.unsupported_category",
        ErrorKind::EmptyFspec => "asterix.empty_fspec",
        ErrorKind::FspecTooLong => "asterix.fspec_too_long",
        ErrorKind::FspecNotMinimal => "asterix.fspec_not_minimal",
        ErrorKind::UnknownUapSlot { .. } => "asterix.unknown_uap_slot",
        ErrorKind::MissingMandatory { .. } => "asterix.missing_mandatory",
        ErrorKind::ExtensionUnterminated => "asterix.extension_unterminated",
        ErrorKind::ZeroRepetition => "asterix.zero_repetition",
        ErrorKind::ExplicitLengthInvalid => "asterix.explicit_length_invalid",
        ErrorKind::EmptyCompound => "asterix.empty_compound",
        ErrorKind::UnknownSubitem => "asterix.unknown_subitem",
        ErrorKind::CompoundNotMinimal => "asterix.compound_not_minimal",
        ErrorKind::SpareBitSet => "asterix.spare_bit_set",
        ErrorKind::InvalidValue => "asterix.invalid_value",
        ErrorKind::ValueOutOfRange => "asterix.value_out_of_range",
        ErrorKind::DuplicateItem => "asterix.duplicate_item",
        ErrorKind::BlockTooLong => "asterix.block_too_long",
        ErrorKind::UnknownFrn { .. } => "asterix.unknown_frn",
    }
}

fn f64_bits(doc: &Doc<'_>, object: NodeIx, key: &str) -> Result<f64, String> {
    let text = string(doc, field(doc, object, key)?, key)?;
    if text.len() != 16
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return fail(format!("{key} must be 16 lowercase hexadecimal digits"));
    }
    let bits = u64::from_str_radix(text, 16).map_err(|_| format!("invalid f64 bits in {key}"))?;
    Ok(f64::from_bits(bits))
}

fn decimal_reference(doc: &Doc<'_>, object: NodeIx, key: &str) -> Result<f64, String> {
    let text = string(doc, field(doc, object, key)?, key)?;
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) if !fraction.is_empty() => (whole, Some(fraction)),
        Some(_) => return fail(format!("{key} has an empty decimal fraction")),
        None => (text, None),
    };
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || fraction.is_some_and(|part| !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return fail(format!(
            "{key} must be a canonical nonnegative decimal string"
        ));
    }
    let value: f64 = text
        .parse()
        .map_err(|_| format!("invalid decimal value in {key}"))?;
    if !value.is_finite() || value < 0.0 {
        return fail(format!("{key} must be finite and nonnegative"));
    }
    Ok(value)
}

fn run_geo(doc: &Doc<'_>, case: NodeIx) -> Result<(), String> {
    if string(doc, field(doc, case, "module")?, "module")? != "geo" {
        return fail("unsupported geodesy module");
    }
    match string(doc, field(doc, case, "operation")?, "operation")? {
        "inverse" => run_geo_inverse(doc, case),
        "direct" => run_geo_direct(doc, case),
        "rhumb_inverse" => run_geo_rhumb_inverse(doc, case),
        other => fail(format!("unsupported geodesy operation {other:?}")),
    }
}

fn run_geo_direct(doc: &Doc<'_>, case: NodeIx) -> Result<(), String> {
    let input = field(doc, field(doc, case, "input")?, "value")?;
    exact_fields(
        doc,
        input,
        &[
            "lat1_deg_f64_bits",
            "lon1_deg_f64_bits",
            "azimuth_deg_f64_bits",
            "distance_m_f64_bits",
        ],
        "direct input",
    )?;
    let lat = f64_bits(doc, input, "lat1_deg_f64_bits")?;
    let lon = f64_bits(doc, input, "lon1_deg_f64_bits")?;
    let azimuth = f64_bits(doc, input, "azimuth_deg_f64_bits")?;
    let distance = f64_bits(doc, input, "distance_m_f64_bits")?;
    let expected = field(doc, case, "expected")?;
    match string(doc, field(doc, expected, "kind")?, "expected.kind")? {
        "success" => {
            let value = field(doc, expected, "value")?;
            exact_fields(
                doc,
                value,
                &[
                    "lat2_deg_reference_decimal",
                    "lat2_deg_absolute_tolerance",
                    "lat2_deg_unit",
                    "lon2_deg_reference_decimal",
                    "lon2_deg_absolute_tolerance",
                    "lon2_deg_unit",
                    "azimuth2_deg_reference_decimal",
                    "azimuth2_deg_absolute_tolerance",
                    "azimuth2_deg_unit",
                ],
                "direct expected value",
            )?;
            for key in ["lat2_deg_unit", "lon2_deg_unit", "azimuth2_deg_unit"] {
                if string(doc, field(doc, value, key)?, key)? != "deg" {
                    return fail(format!("{key} must be deg"));
                }
            }
            let start = oa_geo::LatLon::new(lat, lon)
                .map_err(|e| format!("unexpected direct start-point refusal: {e}"))?;
            let result = oa_geo::wgs84()
                .direct(start, azimuth, distance)
                .map_err(|e| format!("unexpected direct refusal: {e}"))?;
            for (key, tol_key, actual) in [
                (
                    "lat2_deg_reference_decimal",
                    "lat2_deg_absolute_tolerance",
                    result.position.lat(),
                ),
                (
                    "lon2_deg_reference_decimal",
                    "lon2_deg_absolute_tolerance",
                    result.position.lon(),
                ),
                (
                    "azimuth2_deg_reference_decimal",
                    "azimuth2_deg_absolute_tolerance",
                    result.azimuth2_deg,
                ),
            ] {
                let reference = decimal_reference(doc, value, key)?;
                let tolerance = decimal_reference(doc, value, tol_key)?;
                if (actual - reference).abs() > tolerance {
                    return fail(format!("{key} outside declared tolerance: {actual}"));
                }
            }
            Ok(())
        }
        "refusal" => {
            let error = field(doc, expected, "error")?;
            exact_fields(doc, error, &["code", "location"], "direct refusal error")?;
            let location = field(doc, error, "location")?;
            exact_fields(doc, location, &["kind", "value"], "direct refusal location")?;
            if string(doc, field(doc, location, "kind")?, "location.kind")? != "argument_index" {
                return fail("direct refusals require argument_index locations");
            }
            let index = unsigned(doc, field(doc, location, "value")?, "location.value")?;
            let start = oa_geo::LatLon::new(lat, lon)
                .map_err(|e| format!("unexpected direct start-point refusal: {e}"))?;
            let result = oa_geo::wgs84().direct(start, azimuth, distance);
            match result {
                Err(oa_geo::GeoError::NonFinite("azimuth")) if index == 2 => {
                    if string(doc, field(doc, error, "code")?, "error.code")? != "geo.non_finite" {
                        return fail("direct azimuth refusal code mismatch");
                    }
                    Ok(())
                }
                Err(oa_geo::GeoError::NonFinite("distance")) if index == 3 => {
                    if string(doc, field(doc, error, "code")?, "error.code")? != "geo.non_finite" {
                        return fail("direct distance refusal code mismatch");
                    }
                    Ok(())
                }
                Err(other) => fail(format!("unexpected direct refusal: {other}")),
                Ok(_) => fail("expected direct refusal, but inputs were accepted"),
            }
        }
        other => fail(format!("unsupported direct expected outcome {other:?}")),
    }
}

fn run_geo_inverse(doc: &Doc<'_>, case: NodeIx) -> Result<(), String> {
    let input = field(doc, field(doc, case, "input")?, "value")?;
    exact_fields(
        doc,
        input,
        &[
            "lat1_deg_f64_bits",
            "lon1_deg_f64_bits",
            "lat2_deg_f64_bits",
            "lon2_deg_f64_bits",
        ],
        "geodesy input",
    )?;
    let expected_node = field(doc, case, "expected")?;
    match string(doc, field(doc, expected_node, "kind")?, "expected.kind")? {
        "success" => {
            let expected = field(doc, expected_node, "value")?;
            exact_fields(
                doc,
                expected,
                &[
                    "distance_m_reference_decimal",
                    "distance_m_absolute_tolerance",
                    "distance_m_unit",
                ],
                "geodesy expected value",
            )?;
            let p1 = oa_geo::LatLon::new(
                f64_bits(doc, input, "lat1_deg_f64_bits")?,
                f64_bits(doc, input, "lon1_deg_f64_bits")?,
            )
            .map_err(|e| format!("unexpected first-coordinate refusal: {e}"))?;
            let p2 = oa_geo::LatLon::new(
                f64_bits(doc, input, "lat2_deg_f64_bits")?,
                f64_bits(doc, input, "lon2_deg_f64_bits")?,
            )
            .map_err(|e| format!("unexpected second-coordinate refusal: {e}"))?;
            if string(
                doc,
                field(doc, expected, "distance_m_unit")?,
                "distance_m_unit",
            )? != "m"
            {
                return fail("geodesy reference distance unit must be m");
            }
            let reference = decimal_reference(doc, expected, "distance_m_reference_decimal")?;
            let tolerance = decimal_reference(doc, expected, "distance_m_absolute_tolerance")?;
            let result = oa_geo::wgs84()
                .inverse(p1, p2)
                .map_err(|e| format!("WGS-84 inverse failed: {e}"))?;
            if (result.distance_m - reference).abs() > tolerance {
                return fail(format!(
                    "geodesy distance outside declared tolerance: {} m",
                    result.distance_m
                ));
            }
            Ok(())
        }
        "refusal" => {
            let error = field(doc, expected_node, "error")?;
            exact_fields(doc, error, &["code", "location"], "geodesy refusal error")?;
            let expected_code = string(doc, field(doc, error, "code")?, "error.code")?;
            let location = field(doc, error, "location")?;
            exact_fields(
                doc,
                location,
                &["kind", "value"],
                "geodesy refusal location",
            )?;
            if string(doc, field(doc, location, "kind")?, "location.kind")? != "argument_index" {
                return fail("geodesy coordinate refusals require argument_index locations");
            }
            let expected_index = unsigned(doc, field(doc, location, "value")?, "location.value")?;
            if expected_index > 3 {
                return fail("geodesy coordinate argument index must be 0..=3");
            }
            let values = [
                f64_bits(doc, input, "lat1_deg_f64_bits")?,
                f64_bits(doc, input, "lon1_deg_f64_bits")?,
                f64_bits(doc, input, "lat2_deg_f64_bits")?,
                f64_bits(doc, input, "lon2_deg_f64_bits")?,
            ];
            let mut refusal = None;
            for (pair, args) in values.chunks_exact(2).enumerate() {
                let latitude = *args.first().ok_or("coordinate pair lacks latitude")?;
                let longitude = *args.get(1).ok_or("coordinate pair lacks longitude")?;
                match oa_geo::LatLon::new(latitude, longitude) {
                    Ok(_) => {}
                    Err(oa_geo::GeoError::NonFinite(axis)) => {
                        let index = if axis == "longitude" {
                            pair * 2 + 1
                        } else {
                            pair * 2
                        };
                        refusal = Some(("geo.non_finite", index));
                        break;
                    }
                    Err(oa_geo::GeoError::LatitudeOutOfRange(_)) => {
                        refusal = Some(("geo.latitude_out_of_range", pair * 2));
                        break;
                    }
                    Err(oa_geo::GeoError::LongitudeOutOfRange(_)) => {
                        refusal = Some(("geo.longitude_out_of_range", pair * 2 + 1));
                        break;
                    }
                    Err(_) => return fail("unexpected geodesy refusal category"),
                }
            }
            let Some((actual_code, actual_index)) = refusal else {
                return fail("expected geodesy refusal, but both coordinates were accepted");
            };
            let actual_index =
                u64::try_from(actual_index).map_err(|_| "geodesy argument index overflow")?;
            if expected_code != actual_code || expected_index != actual_index {
                return fail(format!(
                    "geodesy refusal mismatch: got {actual_code} at argument {actual_index}"
                ));
            }
            Ok(())
        }
        other => fail(format!("unsupported geodesy expected outcome {other:?}")),
    }
}

fn run_geo_rhumb_inverse(doc: &Doc<'_>, case: NodeIx) -> Result<(), String> {
    let input = field(doc, field(doc, case, "input")?, "value")?;
    exact_fields(
        doc,
        input,
        &[
            "lat1_deg_f64_bits",
            "lon1_deg_f64_bits",
            "lat2_deg_f64_bits",
            "lon2_deg_f64_bits",
        ],
        "rhumb inverse input",
    )?;
    let p1_lat = f64_bits(doc, input, "lat1_deg_f64_bits")?;
    let p1_lon = f64_bits(doc, input, "lon1_deg_f64_bits")?;
    let p2_lat = f64_bits(doc, input, "lat2_deg_f64_bits")?;
    let p2_lon = f64_bits(doc, input, "lon2_deg_f64_bits")?;
    let expected = field(doc, case, "expected")?;
    match string(doc, field(doc, expected, "kind")?, "expected.kind")? {
        "success" => {
            let value = field(doc, expected, "value")?;
            exact_fields(
                doc,
                value,
                &[
                    "distance_m_reference_decimal",
                    "distance_m_absolute_tolerance",
                    "distance_m_unit",
                    "course_deg_reference_decimal",
                    "course_deg_absolute_tolerance",
                    "course_deg_unit",
                ],
                "rhumb inverse expected value",
            )?;
            let p1 = oa_geo::LatLon::new(p1_lat, p1_lon)
                .map_err(|e| format!("unexpected first point refusal: {e}"))?;
            let p2 = oa_geo::LatLon::new(p2_lat, p2_lon)
                .map_err(|e| format!("unexpected second point refusal: {e}"))?;
            if string(
                doc,
                field(doc, value, "distance_m_unit")?,
                "distance_m_unit",
            )? != "m"
                || string(
                    doc,
                    field(doc, value, "course_deg_unit")?,
                    "course_deg_unit",
                )? != "deg"
            {
                return fail("rhumb reference units must be m and deg");
            }
            let expected_distance = decimal_reference(doc, value, "distance_m_reference_decimal")?;
            let distance_tolerance =
                decimal_reference(doc, value, "distance_m_absolute_tolerance")?;
            let expected_course = decimal_reference(doc, value, "course_deg_reference_decimal")?;
            let course_tolerance = decimal_reference(doc, value, "course_deg_absolute_tolerance")?;
            let result = oa_geo::wgs84()
                .rhumb_inverse(p1, p2)
                .map_err(|e| format!("unexpected rhumb refusal: {e}"))?;
            if (result.distance_m - expected_distance).abs() > distance_tolerance {
                return fail(format!(
                    "rhumb distance outside declared tolerance: {} m",
                    result.distance_m
                ));
            }
            if (result.course_deg - expected_course).abs() > course_tolerance {
                return fail(format!(
                    "rhumb course outside declared tolerance: {} deg",
                    result.course_deg
                ));
            }
            Ok(())
        }
        "refusal" => {
            let error = field(doc, expected, "error")?;
            exact_fields(doc, error, &["code", "location"], "rhumb refusal error")?;
            let code = string(doc, field(doc, error, "code")?, "error.code")?;
            let location = field(doc, error, "location")?;
            exact_fields(doc, location, &["kind", "value"], "rhumb refusal location")?;
            if string(doc, field(doc, location, "kind")?, "location.kind")? != "argument_index" {
                return fail("rhumb endpoint refusals require argument_index locations");
            }
            let expected_index = unsigned(doc, field(doc, location, "value")?, "location.value")?;
            if expected_index != 0 && expected_index != 2 {
                return fail("rhumb endpoint argument index must be 0 or 2");
            }
            let p1 = oa_geo::LatLon::new(p1_lat, p1_lon)
                .map_err(|e| format!("unexpected first-coordinate refusal: {e}"))?;
            let p2 = oa_geo::LatLon::new(p2_lat, p2_lon)
                .map_err(|e| format!("unexpected second-coordinate refusal: {e}"))?;
            match oa_geo::wgs84().rhumb_inverse(p1, p2) {
                Err(oa_geo::GeoError::RhumbPole) => {
                    if code != "geo.rhumb_pole" {
                        return fail("rhumb pole refusal code mismatch");
                    }
                    let actual_index = if p1.lat().abs() >= 90.0 { 0 } else { 2 };
                    if expected_index != actual_index {
                        return fail(format!(
                            "rhumb pole refusal location mismatch: got argument {actual_index}"
                        ));
                    }
                    Ok(())
                }
                Err(other) => fail(format!("unexpected rhumb refusal: {other}")),
                Ok(_) => fail("expected rhumb pole refusal, but endpoints were accepted"),
            }
        }
        other => fail(format!("unsupported rhumb expected outcome {other:?}")),
    }
}

fn validate_case(
    root: &Path,
    case_rel: &str,
    case_bytes: &[u8],
    id: &str,
    total_binary: &mut usize,
    expected_files: &mut BTreeSet<String>,
) -> Result<(), String> {
    let doc = parse_json(case_bytes, MAX_CASE_BYTES, MAX_NODES, case_rel)?;
    let case = NodeIx::ROOT;
    exact_fields(
        &doc,
        case,
        &[
            "schema",
            "id",
            "module",
            "operation",
            "edition",
            "limits_profile",
            "input",
            "expected",
            "provenance",
        ],
        "case document",
    )?;
    if string(&doc, field(&doc, case, "schema")?, "schema")? != "oacv-case/1"
        || string(&doc, field(&doc, case, "id")?, "id")? != id
        || !valid_id(id)
        || string(&doc, field(&doc, case, "limits_profile")?, "limits_profile")? != "default-v1"
    {
        return fail(format!(
            "case identity/schema/profile mismatch in {case_rel}"
        ));
    }
    let provenance = field(&doc, case, "provenance")?;
    exact_fields(
        &doc,
        provenance,
        &["kind", "license", "author", "reference", "method"],
        "provenance",
    )?;
    if string(&doc, field(&doc, provenance, "license")?, "license")? != "CC0-1.0"
        || string(&doc, field(&doc, provenance, "kind")?, "kind")? != "synthetic-spec-derived"
    {
        return fail(format!("case {id} lacks approved synthetic CC0 provenance"));
    }
    for key in ["author", "reference", "method"] {
        if string(&doc, field(&doc, provenance, key)?, key)?.is_empty() {
            return fail(format!("case {id} has empty provenance field {key}"));
        }
    }

    let input = field(&doc, case, "input")?;
    let input_kind = string(&doc, field(&doc, input, "kind")?, "input.kind")?;
    let input_bytes = if input_kind == "binary" {
        exact_fields(
            &doc,
            input,
            &["kind", "file", "bytes", "sha256"],
            "binary input",
        )?;
        let rel = string(&doc, field(&doc, input, "file")?, "input.file")?;
        let bytes = unsigned(&doc, field(&doc, input, "bytes")?, "input.bytes")?;
        let digest = hex_digest(&doc, field(&doc, input, "sha256")?, "input.sha256")?;
        if bytes > MAX_BINARY_BYTES as u64 {
            return fail(format!("binary input for {id} exceeds per-file cap"));
        }
        let remaining = MAX_TOTAL_BINARY_BYTES
            .checked_sub(*total_binary)
            .ok_or("binary total already exceeds v1 cap")?;
        if bytes > remaining as u64 {
            return fail(format!(
                "binary input for {id} exceeds remaining corpus byte cap"
            ));
        }
        let path = Path::new(case_rel)
            .parent()
            .ok_or("case has no directory")?
            .join(safe_rel_path(rel)?);
        let path_text = path.to_str().ok_or("non-UTF8 corpus path")?;
        safe_rel_path(path_text)?;
        if !expected_files.insert(path_text.to_owned()) {
            return fail(format!("duplicate corpus member reference {path_text:?}"));
        }
        let data = read_member(root, path_text, MAX_BINARY_BYTES)?;
        if data.len() as u64 != bytes {
            return fail(format!("binary input size mismatch for {id}"));
        }
        verify_digest(&data, &digest, &format!("{id} input"))?;
        *total_binary = (*total_binary)
            .checked_add(data.len())
            .ok_or("binary byte counter overflow")?;
        Some(data)
    } else if input_kind == "structured" {
        exact_fields(&doc, input, &["kind", "value"], "structured input")?;
        None
    } else {
        return fail(format!("unsupported input kind {input_kind:?}"));
    };

    let expected = field(&doc, case, "expected")?;
    let expected_kind = string(&doc, field(&doc, expected, "kind")?, "expected.kind")?;
    match expected_kind {
        "success" => exact_fields(&doc, expected, &["kind", "value"], "expected outcome")?,
        "refusal" => exact_fields(&doc, expected, &["kind", "error"], "expected outcome")?,
        _ => return fail(format!("unsupported expected outcome {expected_kind:?}")),
    }
    match string(&doc, field(&doc, case, "module")?, "module")? {
        "sha256" => {
            if expected_kind != "success" {
                return fail("SHA-256 digest vectors currently require a successful outcome");
            }
            if string(&doc, field(&doc, case, "edition")?, "edition")? != "FIPS-180-4/SHA-256" {
                return fail("unsupported SHA-256 edition");
            }
            run_sha256(
                &doc,
                case,
                input_bytes
                    .as_deref()
                    .ok_or("SHA-256 vector requires binary input")?,
            )
        }
        "geo" => {
            if string(&doc, field(&doc, case, "edition")?, "edition")? != "WGS84-Karney2013" {
                return fail("unsupported geodesy edition");
            }
            run_geo(&doc, case)
        }
        "asterix" => run_asterix(
            &doc,
            case,
            input_bytes
                .as_deref()
                .ok_or("ASTERIX decode vector requires binary input")?,
        ),
        module => fail(format!("unsupported vector module {module:?}")),
    }
}

fn collect_files(
    root: &Path,
    current: &Path,
    found: &mut BTreeSet<String>,
    entries_seen: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return fail("corpus directory nesting exceeds v1 limit");
    }
    for entry in fs::read_dir(current).map_err(|e| format!("walk corpus: {e}"))? {
        *entries_seen = (*entries_seen)
            .checked_add(1)
            .ok_or("corpus entry counter overflow")?;
        if *entries_seen > MAX_INVENTORY_ENTRIES {
            return fail("corpus file inventory exceeds v1 bound");
        }
        let entry = entry.map_err(|e| format!("walk corpus: {e}"))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("walk corpus: {e}"))?;
        if meta.file_type().is_symlink() {
            return fail(format!("symlink in corpus: {}", path.display()));
        }
        if meta.is_dir() {
            collect_files(root, &path, found, entries_seen, depth + 1)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| "corpus path escaped root")?;
            let text = rel
                .to_str()
                .ok_or("non-UTF8 corpus path")?
                .replace('\\', "/");
            safe_rel_path(&text)?;
            found.insert(text);
            if found.len() > MAX_CASES * 2 + 1 {
                return fail("corpus file inventory exceeds v1 bound");
            }
        } else {
            return fail(format!("non-regular corpus entry: {}", path.display()));
        }
    }
    Ok(())
}

fn validate(root: &Path) -> Result<usize, String> {
    let manifest_bytes = read_member(root, "manifest.json", MAX_MANIFEST_BYTES)?;
    let manifest = parse_json(
        &manifest_bytes,
        MAX_MANIFEST_BYTES,
        MAX_MANIFEST_NODES,
        "manifest.json",
    )?;
    let top = NodeIx::ROOT;
    exact_fields(&manifest, top, &["schema", "license", "cases"], "manifest")?;
    if string(&manifest, field(&manifest, top, "schema")?, "schema")? != "oacv-manifest/1"
        || string(&manifest, field(&manifest, top, "license")?, "license")? != "CC0-1.0"
    {
        return fail("manifest schema/license mismatch");
    }
    let cases = field(&manifest, top, "cases")?;
    if manifest.kind(cases) != Some(Kind::Array) || manifest.len(cases) > MAX_CASES {
        return fail("manifest cases must be an array within the v1 count limit");
    }
    let mut ids = BTreeSet::new();
    let mut expected_files = BTreeSet::from(["manifest.json".to_owned()]);
    let mut total_binary = 0usize;
    let mut count = 0usize;
    for entry in manifest.elements(cases) {
        exact_fields(
            &manifest,
            entry,
            &["id", "case", "bytes", "sha256"],
            "manifest case entry",
        )?;
        let id = string(&manifest, field(&manifest, entry, "id")?, "case id")?;
        if !valid_id(id) || !ids.insert(id.to_owned()) {
            return fail(format!("invalid or duplicate case id {id:?}"));
        }
        let case_rel = string(&manifest, field(&manifest, entry, "case")?, "case path")?;
        let safe_rel = safe_rel_path(case_rel)?;
        let case_bytes = unsigned(&manifest, field(&manifest, entry, "bytes")?, "case bytes")?;
        let case_sha = hex_digest(&manifest, field(&manifest, entry, "sha256")?, "case sha256")?;
        if case_bytes > MAX_CASE_BYTES as u64 || !expected_files.insert(case_rel.to_owned()) {
            return fail(format!("duplicate or oversized case document {case_rel:?}"));
        }
        let bytes = read_member(
            root,
            safe_rel.to_str().ok_or("non-UTF8 case path")?,
            MAX_CASE_BYTES,
        )?;
        if bytes.len() as u64 != case_bytes {
            return fail(format!("case byte size mismatch for {case_rel}"));
        }
        verify_digest(&bytes, &case_sha, case_rel)?;
        validate_case(
            root,
            case_rel,
            &bytes,
            id,
            &mut total_binary,
            &mut expected_files,
        )?;
        if total_binary > MAX_TOTAL_BINARY_BYTES {
            return fail("corpus total binary bytes exceeds v1 limit");
        }
        count += 1;
    }
    let mut actual = BTreeSet::new();
    let mut entries_seen = 0usize;
    collect_files(root, root, &mut actual, &mut entries_seen, 0)?;
    if actual != expected_files {
        return fail("corpus file inventory differs from manifest references");
    }
    Ok(count)
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vectors");
    match validate(&root) {
        Ok(count) => println!("oacv/1: validated {count} synthetic vectors"),
        Err(error) => {
            eprintln!("oacv/1: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_rel_path, validate};
    use std::path::Path;

    // REQ: OA-VEC-001, OA-VEC-002, OA-VEC-003, OA-VEC-004
    #[test]
    fn checked_in_initial_corpus_is_closed_and_supported() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vectors");
        let result = validate(&root);
        assert!(
            result.as_ref().is_ok_and(|count| *count == 18),
            "initial corpus validation failed: {result:?}"
        );
    }

    // REQ: OA-VEC-002
    #[test]
    fn corpus_paths_reject_escape_and_noncanonical_components() {
        assert!(safe_rel_path("geo/wgs84/case.json").is_ok());
        for path in [
            "../manifest.json",
            "/absolute/path",
            "geo//case.json",
            "geo/./case.json",
            "geo/../case.json",
            "geo\\case.json",
        ] {
            assert!(
                safe_rel_path(path).is_err(),
                "accepted unsafe path {path:?}"
            );
        }
    }
}

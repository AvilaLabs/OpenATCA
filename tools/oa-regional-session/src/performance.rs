//! Guarded performance-envelope manifest reader (**shell**, B1/B3/C6). It retains the exact
//! bounded input bytes and hashes them; it does not qualify measured or empirical claims.
//!
//! The header is `OA-PERFORMANCE<TAB>1`. Rows have fourteen fields:
//! `v1`, 32-digit GUFI hex, six-digit address hex, four-byte centre, 12 uppercase plan ID,
//! source and availability milliseconds, message ID, maximum age milliseconds, climb and descent
//! feet/minute bounds, groundspeed millimetres/second bound, source (`MEASURED` or
//! `EMPIRICAL_MODEL`), and status (`VALID` or `CONTRADICTORY`). No cutoff is applied here.

use oa_legacy_traj::core::trajectory::{
    FlightKey, PerformanceEnvelope, PerformanceSource, PerformanceStatus,
};
use std::fs::File;
use std::io::{Read, Take};
use std::path::Path;

/// Maximum complete manifest size, including header and line endings.
pub const MAX_PERFORMANCE_MANIFEST_BYTES: usize = 64 * 1024;
/// Maximum performance records retained in one manifest.
pub const MAX_PERFORMANCE_ROWS: usize = 32;
/// Maximum bytes in one row, excluding its line terminator.
pub const MAX_PERFORMANCE_ROW_BYTES: usize = 1024;
const READ_LIMIT: u64 = (MAX_PERFORMANCE_MANIFEST_BYTES as u64) + 1;
const MAX_AIRCRAFT_ADDRESS: u32 = 0x01ff_ffff;
const MS_TO_NS: i64 = 1_000_000;

/// Parsed manifest with its original bytes and content identity retained for caller evidence.
/// A7.2: cold shell object; maximum retained source is 64 KiB plus 32 envelopes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerformanceInputs {
    pub envelopes: Vec<PerformanceEnvelope>,
    pub sha256: [u8; 32],
    pub bytes: Vec<u8>,
}

/// Reads a regular, unsealed manifest through the workspace path guard and enforces the size cap
/// even if a file grows after its metadata length is sampled.
pub fn load_performance(path: &Path) -> Result<PerformanceInputs, String> {
    let (file, length) = oa_sealed::open_regular(path)
        .map_err(|error| format!("guarded performance manifest: {error}"))?;
    if length > u64::try_from(MAX_PERFORMANCE_MANIFEST_BYTES).map_err(|_| "manifest size width")? {
        return Err("performance manifest exceeds 64 KiB".into());
    }
    let bytes = read_limited(file, READ_LIMIT)?;
    parse_performance_manifest(&bytes)
}

fn read_limited(file: File, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut limited: Take<File> = file.take(limit);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read performance manifest: {error}"))?;
    if u64::try_from(bytes.len()).map_err(|_| "manifest length width")? >= limit {
        return Err("performance manifest grew beyond 64 KiB".into());
    }
    Ok(bytes)
}

/// Parses the versioned TSV without a cutoff argument. Source and availability are checked for
/// ordering and sealed windows before any key, rate, source-kind or status payload is inspected.
/// A non-sealed future envelope is retained as evidence; the runtime applies its receipt cutoff.
pub fn parse_performance_manifest(bytes: &[u8]) -> Result<PerformanceInputs, String> {
    if bytes.len() > MAX_PERFORMANCE_MANIFEST_BYTES {
        return Err("performance manifest exceeds 64 KiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "performance manifest is not UTF-8")?;
    if !text.is_ascii() {
        return Err("performance manifest must be ASCII".into());
    }
    let mut lines = text.split_inclusive('\n').map(|chunk| match chunk.strip_suffix('\n') {
        Some(line) => strip_cr(line),
        None => chunk,
    });
    let header = lines.next().ok_or("performance manifest is empty")?;
    if header != "OA-PERFORMANCE\t1" {
        return Err("expected OA-PERFORMANCE version 1 header".into());
    }

    let mut envelopes = Vec::with_capacity(MAX_PERFORMANCE_ROWS);
    for (zero_index, raw_line) in lines.enumerate() {
        let line = raw_line;
        if line.len() > MAX_PERFORMANCE_ROW_BYTES {
            return Err(format!("performance row {} exceeds 1024 bytes", zero_index + 1));
        }
        if line.is_empty() {
            return Err(format!("performance row {} is empty", zero_index + 1));
        }
        if envelopes.len() >= MAX_PERFORMANCE_ROWS {
            return Err("performance manifest exceeds 32 rows".into());
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 14 || fields.first().copied() != Some("v1") {
            return Err(format!(
                "performance row {} must have 14 fields and version v1",
                zero_index + 1
            ));
        }

        // Causal gates precede identity and capability parsing. These are milliseconds; convert
        // with checked arithmetic before asking the sealed-path time guard.
        let source_ms = parse_i64(field(&fields, 5)?, "source_ms")?;
        let available_ms = parse_i64(field(&fields, 6)?, "available_ms")?;
        validate_times(source_ms, available_ms, zero_index + 1)?;

        let gufi = parse_hex_u128(field(&fields, 1)?, "GUFI")?;
        if gufi == 0 {
            return Err(format!("performance row {} has a zero GUFI", zero_index + 1));
        }
        let address = parse_hex_address(field(&fields, 2)?, zero_index + 1)?;
        let centre = parse_centre(field(&fields, 3)?, zero_index + 1)?;
        let plan_id = parse_plan_id(field(&fields, 4)?, zero_index + 1)?;
        let key = FlightKey {
            gufi: gufi.to_be_bytes(),
            aircraft_address: address,
            centre,
            plan_id,
        };

        let message_id = parse_u64(field(&fields, 7)?, "message_id")?;
        if message_id == 0 {
            return Err(format!("performance row {} has a zero message_id", zero_index + 1));
        }
        let max_age_ms = parse_i64(field(&fields, 8)?, "max_age_ms")?;
        if max_age_ms < 0 {
            return Err(format!("performance row {} has negative max_age_ms", zero_index + 1));
        }
        let max_climb_fpm = parse_u32(field(&fields, 9)?, "max_climb_fpm")?;
        let max_descent_fpm = parse_u32(field(&fields, 10)?, "max_descent_fpm")?;
        let max_groundspeed_mmps = parse_u32(field(&fields, 11)?, "max_groundspeed_mmps")?;
        let source = match field(&fields, 12)? {
            "MEASURED" => PerformanceSource::Measured,
            "EMPIRICAL_MODEL" => PerformanceSource::EmpiricalModel,
            _ => return Err(format!("performance row {} has unknown source", zero_index + 1)),
        };
        let status = match field(&fields, 13)? {
            "VALID" => PerformanceStatus::Valid,
            "CONTRADICTORY" => PerformanceStatus::Contradictory,
            _ => return Err(format!("performance row {} has unknown status", zero_index + 1)),
        };
        envelopes.push(PerformanceEnvelope {
            key,
            reserved_key_align: [0; 4],
            source_ms,
            available_ms,
            message_id,
            max_age_ms,
            max_climb_fpm,
            max_descent_fpm,
            max_groundspeed_mmps,
            reserved_speed_cap: 0,
            source,
            status,
            reserved: [0; 6],
        });
    }

    Ok(PerformanceInputs {
        envelopes,
        sha256: oa_sha256::sha256(bytes),
        bytes: bytes.to_vec(),
    })
}

fn strip_cr(line: &str) -> &str {
    match line.strip_suffix('\r') {
        Some(without_cr) => without_cr,
        None => line,
    }
}

fn field<'a>(fields: &[&'a str], index: usize) -> Result<&'a str, String> {
    fields
        .get(index)
        .copied()
        .ok_or_else(|| "performance row field missing".into())
}

fn parse_i64(text: &str, name: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|_| format!("invalid performance {name}"))
}

fn parse_u64(text: &str, name: &str) -> Result<u64, String> {
    text.parse::<u64>()
        .map_err(|_| format!("invalid performance {name}"))
}

fn parse_u32(text: &str, name: &str) -> Result<u32, String> {
    text.parse::<u32>()
        .map_err(|_| format!("invalid performance {name}"))
}

fn parse_hex_u128(text: &str, name: &str) -> Result<u128, String> {
    if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("performance {name} must be 32 hexadecimal digits"));
    }
    u128::from_str_radix(text, 16).map_err(|_| format!("invalid performance {name}"))
}

fn parse_hex_address(text: &str, row: usize) -> Result<u32, String> {
    if text.len() != 6 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("performance row {row} address must be six hexadecimal digits"));
    }
    let address = u32::from_str_radix(text, 16)
        .map_err(|_| format!("performance row {row} address is invalid"))?;
    if address == 0 || address > MAX_AIRCRAFT_ADDRESS {
        return Err(format!("performance row {row} address is outside the aircraft range"));
    }
    Ok(address)
}

fn parse_centre(text: &str, row: usize) -> Result<[u8; 4], String> {
    if text.len() != 4 || !text.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(format!("performance row {row} centre must be four ASCII alphanumerics"));
    }
    let mut centre = [0; 4];
    for (slot, byte) in centre.iter_mut().zip(text.bytes()) {
        *slot = byte;
    }
    Ok(centre)
}

fn parse_plan_id(text: &str, row: usize) -> Result<[u8; 12], String> {
    if text.len() != 12
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(format!("performance row {row} plan_id must be 12 uppercase alphanumerics"));
    }
    let mut plan_id = [0; 12];
    for (slot, byte) in plan_id.iter_mut().zip(text.bytes()) {
        *slot = byte;
    }
    Ok(plan_id)
}

fn validate_times(source_ms: i64, available_ms: i64, row: usize) -> Result<(), String> {
    if source_ms < 0 || available_ms < 0 || source_ms > available_ms {
        return Err(format!("performance row {row} has invalid source/availability ordering"));
    }
    let source_ns = source_ms
        .checked_mul(MS_TO_NS)
        .ok_or_else(|| format!("performance row {row} source_ms overflows nanoseconds"))?;
    let available_ns = available_ms
        .checked_mul(MS_TO_NS)
        .ok_or_else(|| format!("performance row {row} available_ms overflows nanoseconds"))?;
    if oa_sealed::in_sealed_window(source_ns) || oa_sealed::in_sealed_window(available_ns) {
        return Err(format!("performance row {row} has a sealed timestamp"));
    }
    Ok(())
}

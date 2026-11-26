#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process;

const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;

fn fail(message: &str) -> ! {
    eprintln!("oa-demo-wasm: {message}");
    process::exit(1);
}

fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair.first().copied()? as char).to_digit(16)? as u8;
        let low = (pair.get(1).copied()? as char).to_digit(16)? as u8;
        let slot = digest.get_mut(index)?;
        *slot = (high << 4) | low;
    }
    Some(digest)
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let pair = bytes.get(offset..end)?;
    Some(u16::from_le_bytes([*pair.first()?, *pair.get(1)?]))
}

/// Reject non-OASD input before embedding; runtime still performs the complete schema and row checks.
fn has_oasd_v1_header(bytes: &[u8]) -> bool {
    bytes.get(0..4) == Some(b"OASD")
        && le_u16(bytes, 4) == Some(1)
        && le_u16(bytes, 6) == Some(352)
        && le_u16(bytes, 8) == Some(32)
}

fn main() {
    println!("cargo:rerun-if-env-changed=OA_DEMO_SCENARIO_ASSET");
    println!("cargo:rerun-if-env-changed=OA_DEMO_SCENARIO_ASSET_SHA256");
    println!("cargo:rerun-if-env-changed=OA_F2_SOURCE_COMMIT");

    let target_is_wasm32_unknown_unknown = env::var("CARGO_CFG_TARGET_ARCH").ok().as_deref()
        == Some("wasm32")
        && env::var("CARGO_CFG_TARGET_POINTER_WIDTH").ok().as_deref() == Some("32")
        && env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("unknown")
        && env::var("CARGO_CFG_TARGET_VENDOR").ok().as_deref() == Some("unknown");
    if !target_is_wasm32_unknown_unknown {
        // Native workspace checks compile no ABI module and require no fabricated asset.
        return;
    }

    let asset_path = env::var_os("OA_DEMO_SCENARIO_ASSET")
        .map(PathBuf::from)
        .unwrap_or_else(|| fail("OA_DEMO_SCENARIO_ASSET must name the prepared OASD v1 bundle"));
    let expected_text = env::var("OA_DEMO_SCENARIO_ASSET_SHA256").unwrap_or_else(|_| {
        fail("OA_DEMO_SCENARIO_ASSET_SHA256 must bind the prepared OASD bundle")
    });
    let expected = parse_sha256(&expected_text).unwrap_or_else(|| {
        fail("OA_DEMO_SCENARIO_ASSET_SHA256 must be 64 lowercase hex characters")
    });
    let source_commit = env::var("OA_F2_SOURCE_COMMIT")
        .unwrap_or_else(|_| fail("OA_F2_SOURCE_COMMIT must bind the reviewed source commit"));
    if source_commit.len() != 40
        || !source_commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        fail("OA_F2_SOURCE_COMMIT must be 40 lowercase hexadecimal characters");
    }
    let asset_path_text = match asset_path.to_str() {
        Some(path) if !path.contains('\n') && !path.contains('\r') => path,
        _ => fail("the prepared scenario asset path must be UTF-8 without line breaks"),
    };
    let (asset_file, asset_len) = oa_sealed::open_regular(&asset_path)
        .unwrap_or_else(|_| fail("the prepared scenario asset is refused or unreadable"));
    if asset_len == 0 || asset_len > MAX_ASSET_BYTES as u64 {
        fail("the prepared scenario asset must be a regular file between 1 byte and 16 MiB");
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(MAX_ASSET_BYTES + 1)
        .unwrap_or_else(|_| fail("could not reserve the bounded scenario-asset read buffer"));
    let mut bounded_reader = asset_file.take(MAX_ASSET_BYTES as u64 + 1);
    bounded_reader
        .read_to_end(&mut bytes)
        .unwrap_or_else(|_| fail("the prepared scenario asset could not be read"));
    if bytes.is_empty() || bytes.len() > MAX_ASSET_BYTES {
        fail("the prepared scenario asset changed size outside its 1-byte to 16 MiB bound while reading");
    }
    if !has_oasd_v1_header(&bytes) {
        fail("the prepared scenario asset does not have the OASD v1 header");
    }
    let actual = oa_sha256::sha256(&bytes);
    if actual != expected {
        fail("the prepared scenario asset SHA-256 does not match the declared value");
    }

    let out_dir = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| fail("Cargo did not provide OUT_DIR"));
    let embedded = out_dir.join("scenario.oasd");
    fs::write(&embedded, &bytes)
        .unwrap_or_else(|_| fail("could not stage the verified scenario asset for embedding"));
    let generated = format!(
        "pub const ASSET_SHA256: [u8; 32] = {expected:?};\npub const SOURCE_COMMIT: &str = {source_commit:?};\n"
    );
    fs::write(out_dir.join("asset_binding.rs"), generated)
        .unwrap_or_else(|_| fail("could not write the scenario asset hash binding"));
    println!("cargo:rerun-if-changed={asset_path_text}");
    println!("cargo:rustc-link-arg-cdylib=--export-memory");
}

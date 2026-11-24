//! Bounded performance-manifest grammar, causal timestamp ordering and guarded file reads.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use oa_legacy_traj::core::trajectory::{PerformanceSource, PerformanceStatus};
use oa_regional_session::performance::{
    load_performance, parse_performance_manifest, MAX_PERFORMANCE_MANIFEST_BYTES,
    MAX_PERFORMANCE_ROWS,
};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

const HEADER: &str = "OA-PERFORMANCE\t1";

fn row(
    gufi: &str,
    address: &str,
    centre: &str,
    plan: &str,
    source_ms: i64,
    available_ms: i64,
    message_id: u64,
    max_age_ms: i64,
    climb: u32,
    descent: u32,
    groundspeed: u32,
    source: &str,
    status: &str,
) -> String {
    format!(
        "v1\t{gufi}\t{address}\t{centre}\t{plan}\t{source_ms}\t{available_ms}\t{message_id}\t{max_age_ms}\t{climb}\t{descent}\t{groundspeed}\t{source}\t{status}"
    )
}

fn valid_row() -> String {
    row(
        "00000000000000000000000000000001",
        "ABC101",
        "ZAU1",
        "PLAN00000001",
        1_000,
        1_100,
        19,
        5_000,
        0,
        0,
        0,
        "MEASURED",
        "VALID",
    )
}

fn bytes_with_rows(rows: &[String]) -> Vec<u8> {
    let mut text = format!("{HEADER}\n");
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    text.into_bytes()
}

// REQ: OA-REG-014
#[test]
fn parser_preserves_hashed_bytes_and_explicit_zero_rates() {
    let row = valid_row();
    let bytes = bytes_with_rows(&[row]);
    let parsed = parse_performance_manifest(&bytes).unwrap();

    assert_eq!(parsed.bytes, bytes);
    assert_eq!(parsed.sha256, oa_sha256::sha256(&bytes));
    assert_eq!(parsed.envelopes.len(), 1);
    let envelope = parsed.envelopes.first().unwrap();
    assert_eq!(envelope.key.gufi, 1u128.to_be_bytes());
    assert_eq!(envelope.key.aircraft_address, 0xabc101);
    assert_eq!(envelope.key.centre, *b"ZAU1");
    assert_eq!(envelope.key.plan_id, *b"PLAN00000001");
    assert_eq!(envelope.source_ms, 1_000);
    assert_eq!(envelope.available_ms, 1_100);
    assert_eq!(envelope.message_id, 19);
    assert_eq!(envelope.max_climb_fpm, 0);
    assert_eq!(envelope.max_descent_fpm, 0);
    assert_eq!(envelope.max_groundspeed_mmps, 0);
    assert_eq!(envelope.source, PerformanceSource::Measured);
    assert_eq!(envelope.status, PerformanceStatus::Valid);
    assert_eq!(envelope.reserved, [0; 6]);
}

// REQ: OA-REG-014
#[test]
fn parser_preserves_full_non_icao_address_width() {
    let row = row(
        "00000000000000000000000000000001",
        "1ABCDEF",
        "ZAU1",
        "PLAN00000001",
        1_000,
        1_000,
        19,
        5_000,
        0,
        0,
        0,
        "MEASURED",
        "VALID",
    );
    let parsed = parse_performance_manifest(&bytes_with_rows(&[row])).unwrap();
    assert_eq!(parsed.envelopes.first().unwrap().key.aircraft_address, 0x1ab_cdef);
}

// REQ: OA-REG-014
#[test]
fn parser_retains_nonsealed_future_rows_without_a_cutoff_policy() {
    // These numeric values are syntactically typed but exceed the vertical core's sanity caps;
    // manifest parsing preserves caller evidence and leaves profile validity to that core.
    let future_ms = 4_102_444_800_000i64;
    let future = row(
        "00000000000000000000000000000002",
        "ABC102",
        "ZAU1",
        "PLAN00000002",
        future_ms,
        future_ms + 1,
        20,
        0,
        60_001,
        60_000,
        1_000_001,
        "EMPIRICAL_MODEL",
        "CONTRADICTORY",
    );
    let parsed = parse_performance_manifest(&bytes_with_rows(&[future])).unwrap();
    let envelope = parsed.envelopes.first().unwrap();
    assert_eq!(envelope.source_ms, future_ms);
    assert_eq!(envelope.available_ms, future_ms + 1);
    assert_eq!(envelope.max_climb_fpm, 60_001);
    assert_eq!(envelope.max_groundspeed_mmps, 1_000_001);
    assert_eq!(envelope.source, PerformanceSource::EmpiricalModel);
    assert_eq!(envelope.status, PerformanceStatus::Contradictory);
}

// REQ: OA-REG-014
#[test]
fn timestamps_are_checked_before_identity_or_capability_payload() {
    let (sealed_ms, _) = oa_sealed::sealed_window_ms().unwrap();
    let hidden_bad_payload = row(
        "not-a-gufi",
        "not-an-address",
        "!bad",
        "lowercaseid1",
        sealed_ms,
        sealed_ms,
        0,
        -1,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        "not-a-source",
        "not-a-status",
    );
    let error = parse_performance_manifest(&bytes_with_rows(&[hidden_bad_payload])).unwrap_err();
    assert!(error.contains("sealed timestamp"), "{error}");
    assert!(!error.contains("GUFI"), "{error}");

    let malformed_order = row(
        "not-a-gufi",
        "not-an-address",
        "!bad",
        "lowercaseid1",
        1_200,
        1_100,
        0,
        -1,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        "not-a-source",
        "not-a-status",
    );
    let error = parse_performance_manifest(&bytes_with_rows(&[malformed_order])).unwrap_err();
    assert!(error.contains("ordering"), "{error}");
    assert!(!error.contains("GUFI"), "{error}");

    let overflowing_time = row(
        "not-a-gufi",
        "not-an-address",
        "!bad",
        "lowercaseid1",
        i64::MAX,
        i64::MAX,
        0,
        -1,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        "not-a-source",
        "not-a-status",
    );
    let error = parse_performance_manifest(&bytes_with_rows(&[overflowing_time])).unwrap_err();
    assert!(error.contains("overflows nanoseconds"), "{error}");
    assert!(!error.contains("GUFI"), "{error}");
}

// REQ: OA-REG-014
#[test]
fn malformed_fields_and_bounded_syntax_refuse_the_whole_manifest() {
    for malformed in [
        row("0", "ABC101", "ZAU1", "PLAN00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000000", "ABC101", "ZAU1", "PLAN00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000001", "000000", "ZAU1", "PLAN00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000001", "FFFFFF", "ZAU1", "PLAN00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000001", "ABC101", "!AU1", "PLAN00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000001", "ABC101", "ZAU1", "plan00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000001", "ABC101", "ZAU1", "PLAN00000001", 1_000, 1_100, 0, 5_000, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000001", "ABC101", "ZAU1", "PLAN00000001", 1_000, 1_100, 19, -1, 1, 1, 1, "MEASURED", "VALID"),
        row("00000000000000000000000000000001", "ABC101", "ZAU1", "PLAN00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "measured", "VALID"),
        row("00000000000000000000000000000001", "ABC101", "ZAU1", "PLAN00000001", 1_000, 1_100, 19, 5_000, 1, 1, 1, "MEASURED", "valid"),
    ] {
        assert!(parse_performance_manifest(&bytes_with_rows(&[malformed])).is_err());
    }

    let overflowing_speed = valid_row().replace("\t0\tMEASURED\tVALID", "\t4294967296\tMEASURED\tVALID");
    assert!(parse_performance_manifest(&bytes_with_rows(&[overflowing_speed])).is_err());

    let later_bad_row = vec![valid_row(), "v1\tbad".to_owned()];
    assert!(parse_performance_manifest(&bytes_with_rows(&later_bad_row)).is_err());

    let long_row = "x".repeat(1025);
    assert!(parse_performance_manifest(&bytes_with_rows(&[long_row])).is_err());
    let too_many_rows = vec![valid_row(); MAX_PERFORMANCE_ROWS + 1];
    assert!(parse_performance_manifest(&bytes_with_rows(&too_many_rows)).is_err());
    assert!(parse_performance_manifest(&vec![b'x'; MAX_PERFORMANCE_MANIFEST_BYTES + 1]).is_err());
    assert!(parse_performance_manifest(b"OA-PERFORMANCE\t2\n").is_err());
}

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn temporary_directory() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "oa-performance-manifest-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ))
}

// REQ: OA-REG-014
#[test]
fn file_reader_requires_a_guarded_regular_path_and_preserves_contents() {
    let directory = temporary_directory();
    fs::create_dir(&directory).unwrap();
    let path = directory.join("performance.tsv");
    let bytes = bytes_with_rows(&[valid_row()]);
    fs::write(&path, &bytes).unwrap();

    let parsed = load_performance(&path).unwrap();
    assert_eq!(parsed.bytes, bytes);
    assert_eq!(parsed.sha256, oa_sha256::sha256(&parsed.bytes));
    assert!(load_performance(&directory).is_err());

    fs::write(&path, vec![b'x'; MAX_PERFORMANCE_MANIFEST_BYTES + 1]).unwrap();
    assert!(load_performance(&path).is_err());
    fs::remove_dir_all(directory).unwrap();
}

// This integration test exercises the unsafe C pointer contract directly.
#![allow(unsafe_code)]

use openatca_core_ffi::{oa_deflate_raw_inflate, OaErrorInfo};

#[test]
fn raw_deflate_matches_shared_synthetic_vectors() {
    let vectors = include_str!("fixtures/raw-deflate-v1.tsv");
    for line in vectors
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 7, "{line}");
        let input = decode_hex(fields.get(1).copied().unwrap_or_default());
        let max_output = fields
            .get(2)
            .copied()
            .unwrap_or_default()
            .parse::<u64>()
            .unwrap_or_default();
        let expected_status = fields
            .get(3)
            .copied()
            .unwrap_or_default()
            .parse::<u32>()
            .unwrap_or_default();
        let capacity = fields
            .get(4)
            .copied()
            .unwrap_or_default()
            .parse::<usize>()
            .unwrap_or_default();
        let expected = decode_hex(fields.get(5).copied().unwrap_or_default());
        let expected_consumed = fields
            .get(6)
            .copied()
            .unwrap_or_default()
            .parse::<i64>()
            .unwrap_or_default();
        let mut output = vec![0xa5; capacity];
        let mut output_length = 777u64;
        let mut consumed = 777u64;
        let mut error = OaErrorInfo {
            code: 0,
            location_kind: 0,
            location: 0,
        };
        // SAFETY: every pointer references a live, correctly sized Rust allocation; metadata
        // is disjoint from input/output. Empty input/output uses null pointers per ABI contract.
        let status = unsafe {
            oa_deflate_raw_inflate(
                if input.is_empty() {
                    core::ptr::null()
                } else {
                    input.as_ptr()
                },
                input.len() as u64,
                max_output,
                if output.is_empty() {
                    core::ptr::null_mut()
                } else {
                    output.as_mut_ptr()
                },
                output.len() as u64,
                &mut output_length,
                &mut consumed,
                &mut error,
            )
        };
        assert_eq!(
            status,
            expected_status,
            "{}",
            fields.first().copied().unwrap_or_default()
        );
        if status == 0 {
            assert_eq!(
                output_length as usize,
                expected.len(),
                "{}",
                fields.first().copied().unwrap_or_default()
            );
            assert_eq!(
                consumed as i64,
                expected_consumed,
                "{}",
                fields.first().copied().unwrap_or_default()
            );
            assert_eq!(
                output.get(..expected.len()).unwrap_or_default(),
                expected.as_slice(),
                "{}",
                fields.first().copied().unwrap_or_default()
            );
        } else if status == 6 {
            assert_eq!(
                output_length as usize,
                expected.len(),
                "{}",
                fields.first().copied().unwrap_or_default()
            );
            assert_eq!(
                consumed as i64,
                expected_consumed,
                "{}",
                fields.first().copied().unwrap_or_default()
            );
            assert!(
                output.iter().all(|byte| *byte == 0xa5),
                "{}",
                fields.first().copied().unwrap_or_default()
            );
        } else {
            assert_eq!(
                output_length,
                777,
                "{}",
                fields.first().copied().unwrap_or_default()
            );
            assert_eq!(
                consumed,
                777,
                "{}",
                fields.first().copied().unwrap_or_default()
            );
            assert!(
                output.iter().all(|byte| *byte == 0xa5),
                "{}",
                fields.first().copied().unwrap_or_default()
            );
        }
    }
}

fn decode_hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digits = core::str::from_utf8(pair).unwrap_or("");
            u8::from_str_radix(digits, 16).unwrap_or_default()
        })
        .collect()
}

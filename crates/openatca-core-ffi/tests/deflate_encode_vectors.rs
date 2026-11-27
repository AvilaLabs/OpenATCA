// The test calls the unsafe C ABI with valid, separately allocated pointer regions.
#![allow(unsafe_code)]

use openatca_core_ffi::{oa_deflate_raw_compress, OaErrorInfo};

#[test]
fn raw_deflate_encoder_matches_shared_synthetic_vectors() {
    let vectors = include_str!("fixtures/raw-deflate-encode-v1.tsv");
    for line in vectors
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.split('\t');
        let name = fields.next().unwrap_or_default();
        let input = decode_hex(fields.next().unwrap_or_default());
        let level = fields
            .next()
            .unwrap_or_default()
            .parse::<u32>()
            .unwrap_or_default();
        let max_output = fields
            .next()
            .unwrap_or_default()
            .parse::<u64>()
            .unwrap_or_default();
        let expected_status = fields
            .next()
            .unwrap_or_default()
            .parse::<u32>()
            .unwrap_or_default();
        let capacity = fields
            .next()
            .unwrap_or_default()
            .parse::<usize>()
            .unwrap_or_default();
        let expected_consumed = fields
            .next()
            .unwrap_or_default()
            .parse::<i64>()
            .unwrap_or_default();
        let expected_level = fields
            .next()
            .unwrap_or_default()
            .parse::<u32>()
            .unwrap_or_default();
        let expected_plain = decode_hex(fields.next().unwrap_or_default());
        let mut output = vec![0xa5; capacity];
        let mut output_length = 777u64;
        let mut consumed = 777u64;
        let mut actual_level = 777u32;
        let mut error = OaErrorInfo {
            code: 0,
            location_kind: 0,
            location: 0,
        };
        // SAFETY: pointers refer to live allocations with the declared lengths and do not overlap.
        let status = unsafe {
            oa_deflate_raw_compress(
                if input.is_empty() {
                    core::ptr::null()
                } else {
                    input.as_ptr()
                },
                input.len() as u64,
                level,
                max_output,
                if output.is_empty() {
                    core::ptr::null_mut()
                } else {
                    output.as_mut_ptr()
                },
                output.len() as u64,
                &mut output_length,
                &mut consumed,
                &mut actual_level,
                &mut error,
            )
        };
        assert_eq!(status, expected_status, "{name}");
        if status == 0 {
            assert_eq!(consumed as i64, expected_consumed, "{name}");
            assert_eq!(actual_level, expected_level, "{name}");
            let encoded = output.get(..output_length as usize).unwrap_or_default();
            let decoded_result =
                oa_zip::inflate(encoded, expected_plain.len(), expected_plain.len());
            assert!(decoded_result.is_ok(), "{name}: {decoded_result:?}");
            assert_eq!(decoded_result.unwrap_or_default(), expected_plain, "{name}");
        } else if status == 6 {
            assert!(output_length > 0, "{name}");
            assert_eq!(consumed as i64, expected_consumed, "{name}");
            assert_eq!(actual_level, expected_level, "{name}");
            assert!(output.iter().all(|byte| *byte == 0xa5), "{name}");
            let mut exact_output = vec![0u8; output_length as usize];
            let mut exact_length = 777u64;
            let mut exact_consumed = 777u64;
            let mut exact_level = 777u32;
            // SAFETY: retry uses fresh output storage disjoint from input and all metadata.
            let retry_status = unsafe {
                oa_deflate_raw_compress(
                    if input.is_empty() {
                        core::ptr::null()
                    } else {
                        input.as_ptr()
                    },
                    input.len() as u64,
                    level,
                    max_output,
                    exact_output.as_mut_ptr(),
                    exact_output.len() as u64,
                    &mut exact_length,
                    &mut exact_consumed,
                    &mut exact_level,
                    &mut error,
                )
            };
            assert_eq!(retry_status, 0, "{name}");
            assert_eq!(exact_length as usize, exact_output.len(), "{name}");
            assert_eq!(exact_consumed as i64, expected_consumed, "{name}");
            assert_eq!(exact_level, expected_level, "{name}");
            let decoded_result =
                oa_zip::inflate(&exact_output, expected_plain.len(), expected_plain.len());
            assert!(decoded_result.is_ok(), "{name}: {decoded_result:?}");
            assert_eq!(decoded_result.unwrap_or_default(), expected_plain, "{name}");
        } else {
            assert_eq!(output_length, 777, "{name}");
            assert_eq!(consumed, 777, "{name}");
            assert_eq!(actual_level, 777, "{name}");
            assert!(output.iter().all(|byte| *byte == 0xa5), "{name}");
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

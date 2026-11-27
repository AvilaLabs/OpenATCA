// The test calls the unsafe C ABI with valid, separate pointer regions.
#![allow(unsafe_code)]

use openatca_core_ffi::{oa_tar_parse, OaTarEntry};

#[test]
fn tar_metadata_matches_shared_synthetic_vectors() {
    let vectors = include_str!("fixtures/tar-metadata-v1.tsv");
    for line in vectors
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.split('\t');
        let name = fields.next().unwrap_or_default();
        let archive = decode_hex(fields.next().unwrap_or_default());
        let max_entries = fields
            .next()
            .unwrap_or_default()
            .parse::<u32>()
            .unwrap_or_default();
        let expected_status = fields
            .next()
            .unwrap_or_default()
            .parse::<u32>()
            .unwrap_or_default();
        let entry_capacity = fields
            .next()
            .unwrap_or_default()
            .parse::<usize>()
            .unwrap_or_default();
        let name_capacity = fields
            .next()
            .unwrap_or_default()
            .parse::<usize>()
            .unwrap_or_default();
        let expected_count = fields
            .next()
            .unwrap_or_default()
            .parse::<i64>()
            .unwrap_or_default();
        let expected_names = decode_hex(fields.next().unwrap_or_default());
        let sentinel = OaTarEntry {
            name_offset: u64::MAX,
            name_length: u64::MAX,
            size: u64::MAX,
            mtime: u64::MAX,
            header_offset: u64::MAX,
            typeflag: u8::MAX,
            reserved: [u8::MAX; 7],
        };
        let mut output_entries = vec![sentinel; entry_capacity];
        let mut output_names = vec![0xa5; name_capacity];
        let mut entry_count = 777u64;
        let mut name_length = 777u64;
        let mut error = openatca_core_ffi::OaErrorInfo {
            code: 0,
            location_kind: 0,
            location: 0,
        };
        // SAFETY: pointer regions reference live, correctly sized and non-overlapping vectors.
        let status = unsafe {
            oa_tar_parse(
                if archive.is_empty() {
                    core::ptr::null()
                } else {
                    archive.as_ptr()
                },
                archive.len() as u64,
                max_entries,
                if output_entries.is_empty() {
                    core::ptr::null_mut()
                } else {
                    output_entries.as_mut_ptr()
                },
                output_entries.len() as u64,
                &mut entry_count,
                if output_names.is_empty() {
                    core::ptr::null_mut()
                } else {
                    output_names.as_mut_ptr()
                },
                output_names.len() as u64,
                &mut name_length,
                &mut error,
            )
        };
        assert_eq!(status, expected_status, "{name}");
        if status == 0 {
            assert_eq!(entry_count as i64, expected_count, "{name}");
            assert_eq!(name_length as usize, expected_names.len(), "{name}");
            assert_eq!(
                output_names.get(..expected_names.len()).unwrap_or_default(),
                expected_names,
                "{name}"
            );
            if name == "valid_duplicate_unknown" {
                assert_eq!(
                    output_entries.first().copied().unwrap_or_default(),
                    OaTarEntry {
                        name_offset: 0,
                        name_length: 4,
                        size: 3,
                        mtime: 0o14000000000,
                        header_offset: 0,
                        typeflag: b'0',
                        reserved: [0; 7],
                    }
                );
                assert_eq!(
                    output_entries.get(1).copied().unwrap_or_default(),
                    OaTarEntry {
                        name_offset: 4,
                        name_length: 4,
                        size: 0,
                        mtime: 0o14000000000,
                        header_offset: 1024,
                        typeflag: b'?',
                        reserved: [0; 7],
                    }
                );
                assert_eq!(
                    output_entries.get(2).copied().unwrap_or_default(),
                    OaTarEntry {
                        name_offset: 8,
                        name_length: 9,
                        size: 0,
                        mtime: 0o14000000000,
                        header_offset: 1536,
                        typeflag: b'0',
                        reserved: [0; 7],
                    }
                );
            }
        } else if status == 6 {
            assert_eq!(entry_count as i64, expected_count, "{name}");
            assert_eq!(name_length as usize, expected_names.len(), "{name}");
            assert!(
                output_entries.iter().all(|entry| *entry == sentinel),
                "{name}"
            );
            assert!(output_names.iter().all(|byte| *byte == 0xa5), "{name}");
            let mut exact_entries = vec![OaTarEntry::default(); entry_count as usize];
            let mut exact_names = vec![0u8; name_length as usize];
            let mut exact_count = 777u64;
            let mut exact_name_length = 777u64;
            // SAFETY: retry outputs are fresh and disjoint from input and metadata.
            let retry = unsafe {
                oa_tar_parse(
                    archive.as_ptr(),
                    archive.len() as u64,
                    max_entries,
                    exact_entries.as_mut_ptr(),
                    exact_entries.len() as u64,
                    &mut exact_count,
                    exact_names.as_mut_ptr(),
                    exact_names.len() as u64,
                    &mut exact_name_length,
                    &mut error,
                )
            };
            assert_eq!(retry, 0, "{name}");
            assert_eq!(exact_count, entry_count, "{name}");
            assert_eq!(exact_name_length, name_length, "{name}");
            assert_eq!(exact_names, expected_names, "{name}");
            assert_eq!(
                exact_entries
                    .first()
                    .map(|entry| entry.typeflag)
                    .unwrap_or_default(),
                b'0',
                "{name}"
            );
            assert_eq!(
                exact_entries
                    .get(1)
                    .map(|entry| entry.typeflag)
                    .unwrap_or_default(),
                b'?',
                "{name}"
            );
            assert_eq!(
                exact_entries
                    .get(2)
                    .map(|entry| entry.typeflag)
                    .unwrap_or_default(),
                b'0',
                "{name}"
            );
        } else {
            assert_eq!(entry_count, 777, "{name}");
            assert_eq!(name_length, 777, "{name}");
            assert!(
                output_entries.iter().all(|entry| *entry == sentinel),
                "{name}"
            );
            assert!(output_names.iter().all(|byte| *byte == 0xa5), "{name}");
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

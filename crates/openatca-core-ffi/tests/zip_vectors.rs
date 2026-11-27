#![allow(
    unsafe_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

// Direct C ABI calls below use valid, disjoint pointers with checked capacities.
use openatca_core_ffi::{oa_zip_extract_entry, oa_zip_list, OaErrorInfo, OaZipEntry};

const VECTORS: &str = include_str!("fixtures/zip-v1.tsv");

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                _ => panic!("invalid hex digit"),
            };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect()
}

fn run_list(name: &str, archive: &[u8], expected_status: u32, expected_names: &[u8]) {
    let mut count = u64::MAX;
    let mut names_length = u64::MAX;
    let mut error = OaErrorInfo {
        code: 99,
        location_kind: 99,
        location: 99,
    };
    // SAFETY: input and metadata buffers are valid, non-overlapping Rust-owned regions.
    let status = unsafe {
        oa_zip_list(
            archive.as_ptr(),
            archive.len() as u64,
            std::ptr::null_mut(),
            0,
            &mut count,
            std::ptr::null_mut(),
            0,
            &mut names_length,
            &mut error,
        )
    };
    let expected_query_status = if expected_status == 0 {
        6
    } else {
        expected_status
    };
    assert_eq!(status, expected_query_status, "{name}");
    if expected_status == 0 {
        assert_eq!(count, 1, "{name}");
        assert_eq!(names_length, expected_names.len() as u64, "{name}");
        let mut entries = vec![OaZipEntry::default(); count as usize];
        let mut pool = vec![0xa5; names_length as usize];
        // SAFETY: capacities exactly match validated sizes and outputs are disjoint.
        let status = unsafe {
            oa_zip_list(
                archive.as_ptr(),
                archive.len() as u64,
                entries.as_mut_ptr(),
                entries.len() as u64,
                &mut count,
                pool.as_mut_ptr(),
                pool.len() as u64,
                &mut names_length,
                &mut error,
            )
        };
        assert_eq!(status, 0, "{name}");
        assert_eq!(pool, expected_names, "{name}");
        assert_eq!(entries[0].name_offset, 0, "{name}");
        assert_eq!(
            entries[0].name_length,
            expected_names.len() as u64,
            "{name}"
        );
        assert_eq!(entries[0].reserved, [0; 6], "{name}");
        assert_eq!(
            entries[0].utf8_name_flag,
            u8::from(entries[0].flags & (1 << 11) != 0),
            "{name}"
        );
    } else {
        assert_eq!(count, u64::MAX, "{name}: count changed on parse failure");
        assert_eq!(
            names_length,
            u64::MAX,
            "{name}: pool size changed on parse failure"
        );
    }
}

#[test]
fn c_abi_matches_shared_cc0_zip_vectors() {
    for line in VECTORS
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 8, "vector schema");
        let name = fields[0];
        let archive = unhex(fields[1]);
        let list_status = fields[2].parse::<u32>().unwrap();
        let names = if fields[3] == "-" {
            Vec::new()
        } else {
            unhex(fields[3])
        };
        run_list(name, &archive, list_status, &names);
        if fields[4] == "-" {
            continue;
        }
        let index = fields[4].parse::<u64>().unwrap();
        let expected_extract = fields[5].parse::<u32>().unwrap();
        let expected_bytes = if fields[6] == "-" {
            Vec::new()
        } else {
            unhex(fields[6])
        };
        let expected_declared = fields[7].parse::<u64>().unwrap();
        let mut required = u64::MAX;
        let mut error = OaErrorInfo {
            code: 99,
            location_kind: 99,
            location: 99,
        };
        // SAFETY: the borrowed input and output metadata are valid and disjoint.
        let query = unsafe {
            oa_zip_extract_entry(
                archive.as_ptr(),
                archive.len() as u64,
                index,
                std::ptr::null_mut(),
                0,
                &mut required,
                &mut error,
            )
        };
        if expected_extract == 0 {
            let expected_query = if expected_declared == 0 { 0 } else { 6 };
            assert_eq!(query, expected_query, "{name}: extraction query");
            assert_eq!(required, expected_declared, "{name}");
            let mut output = vec![0xa5; required as usize];
            // SAFETY: output is exactly the declared capacity and disjoint from input.
            let status = unsafe {
                oa_zip_extract_entry(
                    archive.as_ptr(),
                    archive.len() as u64,
                    index,
                    if output.is_empty() {
                        std::ptr::null_mut()
                    } else {
                        output.as_mut_ptr()
                    },
                    output.len() as u64,
                    &mut required,
                    &mut error,
                )
            };
            assert_eq!(status, 0, "{name}");
            assert_eq!(output, expected_bytes, "{name}");
        } else {
            assert_eq!(query, 6, "{name}: nonempty extraction query");
            assert_eq!(required, expected_declared, "{name}");
            let mut output = vec![0xa5; required as usize];
            let before = output.clone();
            // SAFETY: output is exactly the declared capacity and disjoint from input.
            let status = unsafe {
                oa_zip_extract_entry(
                    archive.as_ptr(),
                    archive.len() as u64,
                    index,
                    output.as_mut_ptr(),
                    output.len() as u64,
                    &mut required,
                    &mut error,
                )
            };
            assert_eq!(status, expected_extract, "{name}");
            assert_eq!(output, before, "{name}: failed extraction modified output");
        }
    }
}

#[test]
fn zip_capacity_and_index_refusals_are_transactional() {
    let line = VECTORS
        .lines()
        .find(|line| line.starts_with("stored\t"))
        .unwrap();
    let fields: Vec<&str> = line.split('\t').collect();
    let archive = unhex(fields[1]);
    let names = unhex(fields[3]);
    let mut entries = [OaZipEntry {
        name_offset: 77,
        ..OaZipEntry::default()
    }];
    let mut pool = [0xa5u8; 2];
    let mut count = u64::MAX;
    let mut names_length = u64::MAX;
    let mut error = OaErrorInfo {
        code: 0,
        location_kind: 0,
        location: 0,
    };
    // SAFETY: all pointers reference disjoint valid test buffers.
    let status = unsafe {
        oa_zip_list(
            archive.as_ptr(),
            archive.len() as u64,
            entries.as_mut_ptr(),
            1,
            &mut count,
            pool.as_mut_ptr(),
            pool.len() as u64,
            &mut names_length,
            &mut error,
        )
    };
    assert_eq!(status, 6);
    assert_eq!(count, 1);
    assert_eq!(names_length, names.len() as u64);
    assert_eq!(entries[0].name_offset, 77);
    assert_eq!(pool, [0xa5; 2]);

    let mut output = [0xa5u8; 2];
    let before = output;
    let mut required = u64::MAX;
    // SAFETY: all pointers reference disjoint valid test buffers.
    let status = unsafe {
        oa_zip_extract_entry(
            archive.as_ptr(),
            archive.len() as u64,
            0,
            output.as_mut_ptr(),
            output.len() as u64,
            &mut required,
            &mut error,
        )
    };
    assert_eq!(status, 6);
    assert_eq!(required, 3);
    assert_eq!(output, before);

    let status = unsafe {
        oa_zip_extract_entry(
            archive.as_ptr(),
            archive.len() as u64,
            1,
            output.as_mut_ptr(),
            output.len() as u64,
            &mut required,
            &mut error,
        )
    };
    assert_eq!(status, 40);
    assert_eq!(output, before);
    assert_eq!(required, 3);
}

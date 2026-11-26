#![allow(
    unsafe_code,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use openatca_core_ffi::*;

const VECTORS: &str = include_str!("fixtures/text-v1.tsv");

fn hex_bytes(value: &str) -> Vec<u8> {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16).expect("hex digit");
        let low = (pair[1] as char).to_digit(16).expect("hex digit");
        output.push(((high << 4) | low) as u8);
    }
    output
}

fn expected_counts(value: &str) -> Vec<u64> {
    value
        .split(',')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().expect("count is decimal"))
        .collect()
}

// REQ: OA-FFI-001, OA-FFI-002, OA-FFI-003
#[test]
fn ffi_text_vectors_match_counts_pool_and_parser_refusals() {
    let mut records = 0usize;
    for line in VECTORS.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 8);
        let input = hex_bytes(fields[2]);
        let wanted_status = fields[3].parse::<u32>().expect("status is decimal");
        let wanted_kind = fields[4].parse::<u32>().expect("kind is decimal");
        let wanted_position = fields[5].parse::<u64>().expect("position is decimal");
        let wanted_counts = expected_counts(fields[6]);
        let wanted_pool = hex_bytes(fields[7]);
        let mut error = OaErrorInfo {
            code: 0,
            location_kind: 0,
            location: 0,
        };
        let mut parse_error = OaTextError::default();
        match fields[0] {
            "json" => {
                let mut counts = OaJsonCounts {
                    node_count: 0,
                    value_bytes: 0,
                };
                let status = unsafe {
                    oa_json_parse(
                        input.as_ptr(),
                        input.len() as u64,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null_mut(),
                        0,
                        &mut counts,
                        &mut parse_error,
                        &mut error,
                    )
                };
                if wanted_status == 0 {
                    assert_eq!(status, 6);
                    assert_eq!(
                        [counts.node_count, counts.value_bytes].as_slice(),
                        wanted_counts
                    );
                    let mut nodes = vec![OaJsonNode::default(); counts.node_count as usize];
                    let mut pool = vec![0u8; counts.value_bytes as usize];
                    let status = unsafe {
                        oa_json_parse(
                            input.as_ptr(),
                            input.len() as u64,
                            nodes.as_mut_ptr(),
                            nodes.len() as u64,
                            pool.as_mut_ptr(),
                            pool.len() as u64,
                            &mut counts,
                            &mut parse_error,
                            &mut error,
                        )
                    };
                    assert_eq!(status, 0);
                    assert_eq!(pool, wanted_pool);
                    assert_eq!(nodes.first().expect("JSON root exists").kind, 7);
                } else {
                    assert_eq!(status, wanted_status);
                    assert_eq!(parse_error.kind, wanted_kind);
                    assert_eq!(parse_error.position, wanted_position);
                }
            }
            "xml" => {
                let mut counts = OaXmlCounts {
                    elements: 0,
                    attributes: 0,
                    text_nodes: 0,
                    namespaces: 0,
                    namespace_uri_bytes: 0,
                };
                let status = unsafe {
                    oa_xml_parse(
                        input.as_ptr(),
                        input.len() as u64,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null_mut(),
                        0,
                        &mut counts,
                        &mut parse_error,
                        &mut error,
                    )
                };
                if wanted_status == 0 {
                    assert_eq!(status, 6);
                    assert_eq!(
                        [
                            counts.elements,
                            counts.attributes,
                            counts.text_nodes,
                            counts.namespaces,
                            counts.namespace_uri_bytes
                        ]
                        .as_slice(),
                        wanted_counts
                    );
                    let mut elements = vec![OaXmlElement::default(); counts.elements as usize];
                    let mut attributes =
                        vec![OaXmlAttribute::default(); counts.attributes as usize];
                    let mut texts = vec![OaXmlText::default(); counts.text_nodes as usize];
                    let mut namespaces =
                        vec![OaXmlNamespace::default(); counts.namespaces as usize];
                    let mut pool = vec![0u8; counts.namespace_uri_bytes as usize];
                    let status = unsafe {
                        oa_xml_parse(
                            input.as_ptr(),
                            input.len() as u64,
                            elements.as_mut_ptr(),
                            elements.len() as u64,
                            attributes.as_mut_ptr(),
                            attributes.len() as u64,
                            texts.as_mut_ptr(),
                            texts.len() as u64,
                            namespaces.as_mut_ptr(),
                            namespaces.len() as u64,
                            pool.as_mut_ptr(),
                            pool.len() as u64,
                            &mut counts,
                            &mut parse_error,
                            &mut error,
                        )
                    };
                    assert_eq!(status, 0);
                    assert_eq!(pool, wanted_pool);
                    assert_eq!(elements.len(), 2);
                } else {
                    assert_eq!(status, wanted_status);
                    assert_eq!(parse_error.kind, wanted_kind);
                    assert_eq!(parse_error.position, wanted_position);
                }
            }
            "csv" => {
                let mut counts = OaCsvCounts {
                    rows: 0,
                    fields: 0,
                    value_bytes: 0,
                };
                let status = unsafe {
                    oa_csv_parse(
                        input.as_ptr(),
                        input.len() as u64,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null_mut(),
                        0,
                        core::ptr::null_mut(),
                        0,
                        &mut counts,
                        &mut parse_error,
                        &mut error,
                    )
                };
                if wanted_status == 0 {
                    assert_eq!(status, 6);
                    assert_eq!(
                        [counts.rows, counts.fields, counts.value_bytes].as_slice(),
                        wanted_counts
                    );
                    let mut rows = vec![OaCsvRow::default(); counts.rows as usize];
                    let mut fields_out = vec![OaCsvField::default(); counts.fields as usize];
                    let mut pool = vec![0u8; counts.value_bytes as usize];
                    let status = unsafe {
                        oa_csv_parse(
                            input.as_ptr(),
                            input.len() as u64,
                            rows.as_mut_ptr(),
                            rows.len() as u64,
                            fields_out.as_mut_ptr(),
                            fields_out.len() as u64,
                            pool.as_mut_ptr(),
                            pool.len() as u64,
                            &mut counts,
                            &mut parse_error,
                            &mut error,
                        )
                    };
                    assert_eq!(status, 0);
                    assert_eq!(pool, wanted_pool);
                    assert_eq!(rows.len(), 2);
                } else {
                    assert_eq!(status, wanted_status);
                    assert_eq!(parse_error.kind, wanted_kind);
                    assert_eq!(parse_error.position, wanted_position);
                }
            }
            other => panic!("unknown vector module {other}"),
        }
        assert_eq!(
            error.code,
            if wanted_status == 0 { 0 } else { wanted_status }
        );
        records += 1;
    }
    assert_eq!(records, 6);
}

// REQ: OA-FFI-001, OA-FFI-003
#[test]
fn oversized_slice_extents_refuse_before_dereferencing_nonnull_dangling_pointers() {
    let oversized = (isize::MAX as u64)
        .checked_add(1)
        .expect("supported targets have a representable next extent");
    let dangling = core::ptr::NonNull::<u8>::dangling().as_ptr();
    let mut error = OaErrorInfo {
        code: 0,
        location_kind: 0,
        location: 0,
    };
    let mut parser = OaTextError::default();
    let mut json_counts = OaJsonCounts {
        node_count: 0,
        value_bytes: 0,
    };
    let mut status = unsafe {
        oa_json_parse(
            dangling,
            oversized,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
            &mut json_counts,
            &mut parser,
            &mut error,
        )
    };
    assert_eq!(status, 9);
    assert_eq!(error.code, 9);

    let mut xml_counts = OaXmlCounts::default();
    status = unsafe {
        oa_xml_parse(
            dangling,
            oversized,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
            &mut xml_counts,
            &mut parser,
            &mut error,
        )
    };
    assert_eq!(status, 9);
    assert_eq!(error.code, 9);

    let mut csv_counts = OaCsvCounts::default();
    status = unsafe {
        oa_csv_parse(
            dangling,
            oversized,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
            &mut csv_counts,
            &mut parser,
            &mut error,
        )
    };
    assert_eq!(status, 9);
    assert_eq!(error.code, 9);

    let mut written = u64::MAX;
    status = unsafe {
        oa_xml_decode_span(
            core::ptr::null(),
            0,
            0,
            0,
            1,
            dangling,
            oversized,
            &mut written,
            &mut parser,
            &mut error,
        )
    };
    assert_eq!(status, 9);
    assert_eq!(error.code, 9);
    assert_eq!(written, 0);

    written = u64::MAX;
    status = unsafe {
        oa_xml_decode_span(
            dangling,
            oversized,
            0,
            0,
            1,
            core::ptr::null_mut(),
            0,
            &mut written,
            &mut parser,
            &mut error,
        )
    };
    assert_eq!(status, 9);
    assert_eq!(error.code, 9);
    assert_eq!(written, 0);
}

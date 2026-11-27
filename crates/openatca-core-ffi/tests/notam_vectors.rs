#![allow(
    unsafe_code,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use oa_notam::{
    decode_bytes, new_decoder, new_notams_with_feature_capacity, new_stats, row_at, Limits,
};
use oa_time_window::DropWindow;
use oa_xml::{Limits as XmlLimits, Parser};
use openatca_core_ffi::{
    oa_notam_fns_decode, OaErrorInfo, OaNotamBuffers, OaNotamFeature, OaNotamPolygon,
    OaNotamSummary, OaNotamUnknownPath, OaNotamVertex, OA_STATUS_BUFFER_TOO_SMALL,
    OA_STATUS_INVALID_ARGUMENT, OA_STATUS_NOTAM_SEALED_DROP,
};

const VECTOR: &[u8] = include_bytes!("fixtures/notam-v1.xml");

fn query(input: &[u8], from_ms: i64, to_ms: i64) -> (u32, OaNotamSummary) {
    let mut summary = OaNotamSummary::default();
    let mut error = OaErrorInfo {
        code: u32::MAX,
        location_kind: u32::MAX,
        location: u64::MAX,
    };
    // SAFETY: input is an immutable valid slice; summary and error are distinct writable locals;
    // the null buffer descriptor requests a count-only pass.
    let status = unsafe {
        oa_notam_fns_decode(
            input.as_ptr(),
            input.len() as u64,
            from_ms,
            to_ms,
            core::ptr::null(),
            &mut summary,
            &mut error,
        )
    };
    assert_eq!(error.code, status);
    (status, summary)
}

// REQ: OA-FFI-004
#[test]
fn fns_notam_binding_matches_decoder_rows_features_and_geometry() {
    let (status, required) = query(VECTOR, 0, 0);
    assert_eq!(status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(required.status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(required.row_count, 1);
    assert!(required.feature_count > 0);
    assert_eq!(required.polygon_count, 1);
    assert_eq!(required.vertex_count, 4);
    assert_eq!(required.unknown_path_count, 1);
    assert_eq!(required.unknown_elements, 1);

    let mut row = oa_notam::BLANK_ROW;
    let mut features = vec![
        OaNotamFeature {
            kind: u8::MAX,
            ..oa_notam::FeatureSlice {
                kind: 0,
                time_kind: 0,
                interpretation: 0,
                meta: 0,
                time_meta: 0,
                errors: 0,
                reserved: [0; 2],
                begin_ms: 0,
                end_ms: 0,
                sequence_number: 0,
                correction_number: 0,
                designator: [0; 12],
                designator_len: 0,
                icao: [0; 4],
                icao_len: 0,
            }
        };
        required.feature_count as usize
    ];
    let mut polygons = vec![
        OaNotamPolygon {
            first_vertex: u32::MAX,
            vertex_count: u32::MAX,
            feature_index: u32::MAX,
            kind: u8::MAX,
            reserved: [u8::MAX; 3],
        };
        required.polygon_count as usize
    ];
    let mut vertices = vec![
        OaNotamVertex {
            lat_e7: i32::MAX,
            lon_e7: i32::MAX
        };
        required.vertex_count as usize
    ];
    let mut strings = vec![u8::MAX; required.string_bytes as usize];
    let mut unknown_paths =
        vec![OaNotamUnknownPath::default(); required.unknown_path_count as usize];
    let buffers = OaNotamBuffers {
        row: &mut row,
        row_capacity: 1,
        features: features.as_mut_ptr(),
        feature_capacity: features.len() as u64,
        polygons: polygons.as_mut_ptr(),
        polygon_capacity: polygons.len() as u64,
        vertices: vertices.as_mut_ptr(),
        vertex_capacity: vertices.len() as u64,
        strings: strings.as_mut_ptr(),
        string_capacity: strings.len() as u64,
        unknown_paths: unknown_paths.as_mut_ptr(),
        unknown_path_capacity: unknown_paths.len() as u64,
    };
    let mut summary = OaNotamSummary::default();
    let mut error = OaErrorInfo {
        code: 0,
        location_kind: 0,
        location: 0,
    };
    // SAFETY: all buffers have the queried exact capacities, are writable and disjoint from the
    // immutable input and result records for the duration of the call.
    let status = unsafe {
        oa_notam_fns_decode(
            VECTOR.as_ptr(),
            VECTOR.len() as u64,
            0,
            0,
            &buffers,
            &mut summary,
            &mut error,
        )
    };
    assert_eq!(status, 0);
    assert_eq!(error.code, 0);
    assert_eq!(summary.row_count, 1);

    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let limits = Limits::new(DropWindow::new(0, 0));
    let mut decoder = new_decoder(limits);
    let mut reference = new_notams_with_feature_capacity(
        1,
        2 * 8192,
        XmlLimits::default().max_elements,
        VECTOR.len() / 3,
        XmlLimits::default().max_elements,
    );
    let mut stats = new_stats(128);
    decode_bytes(
        &mut parser,
        &mut decoder,
        VECTOR,
        &mut reference,
        &mut stats,
    )
    .unwrap();
    assert_eq!(Some(row), row_at(&reference, 0));
    assert_eq!(features.len(), oa_notam::feature_slices_len(&reference));
    assert_eq!(polygons.len(), reference.poly_vlen.len());
    assert_eq!(vertices.len(), reference.vtx_lat.len());
    assert_eq!(unknown_paths.len(), stats.unknown.len());
    for (output, reference) in unknown_paths.iter().zip(stats.unknown.iter()) {
        assert_eq!(output.hash, reference.hash);
        assert_eq!(output.count, reference.count);
        assert_eq!(output.is_attribute, u8::from(reference.is_attr));
        assert_eq!(output.text_len, reference.len);
        assert_eq!(
            &output.text[..usize::from(output.text_len)],
            &reference.text[..usize::from(reference.len)]
        );
    }
    assert_eq!(strings, reference.strings[..reference.strings_used]);
    for (index, output) in features.iter().enumerate() {
        assert_eq!(Some(*output), oa_notam::feature_at(&reference, index));
    }
    for (index, output) in polygons.iter().enumerate() {
        assert_eq!(output.first_vertex, reference.poly_vstart[index]);
        assert_eq!(output.vertex_count, reference.poly_vlen[index]);
        assert_eq!(output.feature_index, reference.poly_feature[index]);
        assert_eq!(output.kind, reference.poly_kind[index]);
    }
    for (index, output) in vertices.iter().enumerate() {
        assert_eq!(output.lat_e7, reference.vtx_lat[index]);
        assert_eq!(output.lon_e7, reference.vtx_lon[index]);
    }
}

// REQ: OA-FFI-004
#[test]
fn fns_notam_binding_requires_explicit_filter_and_drops_inside_half_open_window() {
    let (status, summary) = query(VECTOR, 1_898_931_600_000, 1_898_931_600_001);
    assert_eq!(status, OA_STATUS_NOTAM_SEALED_DROP);
    assert_eq!(summary.row_count, 0);
    assert_eq!(summary.sealed_dropped, 1);
    let (status, summary) = query(VECTOR, 1_898_931_600_001, 1_898_931_600_002);
    assert_eq!(status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(summary.sealed_dropped, 0);
    assert_eq!(summary.row_count, 1);
    let (status, summary) = query(VECTOR, 2, 1);
    assert_eq!(status, OA_STATUS_INVALID_ARGUMENT);
    assert_eq!(summary.row_count, 0);
}

// REQ: OA-FFI-004
#[test]
fn fns_notam_binding_short_capacity_is_atomic_and_bad_fields_remain_distinguishable() {
    let mut row = oa_notam::BLANK_ROW;
    row.present = u64::MAX;
    let mut error = OaErrorInfo {
        code: 0,
        location_kind: 0,
        location: 0,
    };
    let mut summary = OaNotamSummary::default();
    let short = OaNotamBuffers {
        row: &mut row,
        row_capacity: 0,
        features: core::ptr::null_mut(),
        feature_capacity: 0,
        polygons: core::ptr::null_mut(),
        polygon_capacity: 0,
        vertices: core::ptr::null_mut(),
        vertex_capacity: 0,
        strings: core::ptr::null_mut(),
        string_capacity: 0,
        unknown_paths: core::ptr::null_mut(),
        unknown_path_capacity: 0,
    };
    // SAFETY: all regions are valid, non-overlapping locals; row has no writable capacity by
    // contract, so only summary and error may be written.
    let status = unsafe {
        oa_notam_fns_decode(
            VECTOR.as_ptr(),
            VECTOR.len() as u64,
            0,
            0,
            &short,
            &mut summary,
            &mut error,
        )
    };
    assert_eq!(status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(row.present, u64::MAX);

    let invalid_time = String::from_utf8(VECTOR.to_vec()).unwrap().replacen(
        "<event:issued>2030-03-05T09:00:00.000Z",
        "<event:issued>bad",
        1,
    );
    let (status, result) = query(invalid_time.as_bytes(), 0, 0);
    assert_eq!(status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(result.row_count, 1);
    assert_eq!(result.field_errors[oa_notam::Field::Issued as usize], 1);
}

// REQ: OA-FFI-004
#[test]
fn fns_notam_binding_keeps_missing_and_invalid_times_and_bad_polygon_distinct() {
    let xml = String::from_utf8(VECTOR.to_vec()).unwrap();
    let missing_time = xml.replace("<event:issued>2030-03-05T09:00:00.000Z</event:issued>", "");
    let (status, summary) = query(missing_time.as_bytes(), 0, 0);
    assert_eq!(status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(summary.row_count, 1);
    assert_eq!(summary.field_errors[oa_notam::Field::Issued as usize], 0);

    let invalid_polygon = xml.replace(
        "<gml:posList>30.10 -90.10 30.10 -90.20 30.20 -90.20 30.20 -90.10 30.10 -90.10</gml:posList>",
        "<gml:posList>30.10 -90.10 999 999</gml:posList>",
    );
    let (status, summary) = query(invalid_polygon.as_bytes(), 0, 0);
    assert_eq!(status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(summary.row_count, 1);
    assert_eq!(summary.field_errors[oa_notam::Field::Polygons as usize], 1);
    assert_eq!(summary.polygon_count, 0);
    assert_eq!(summary.vertex_count, 0);
}

// REQ: OA-FFI-004
#[test]
fn fns_notam_binding_rejects_oversized_input_before_parser_construction() {
    let oversized = vec![b' '; 1_048_577];
    let (status, summary) = query(&oversized, 0, 0);
    assert_eq!(status, oa_notam_binding_status::INPUT_LIMIT);
    assert_eq!(summary.row_count, 0);
}

// REQ: OA-FFI-004
#[test]
fn fns_notam_binding_reports_bounded_unknown_path_overflow() {
    let xml = String::from_utf8(VECTOR.to_vec()).unwrap();
    let extras = (0..129)
        .map(|index| format!("<event:extension{index}/>"))
        .collect::<String>();
    let input = xml.replacen(
        "<event:scenario>123</event:scenario>",
        &format!("<event:scenario>123</event:scenario>{extras}"),
        1,
    );
    let (status, summary) = query(input.as_bytes(), 0, 0);
    assert_eq!(status, OA_STATUS_BUFFER_TOO_SMALL);
    assert_eq!(summary.unknown_path_count, 128);
    assert_eq!(summary.unknown_elements, 130);
    assert_eq!(summary.unknown_overflow, 2);
}

mod oa_notam_binding_status {
    pub const INPUT_LIMIT: u32 = 42;
}

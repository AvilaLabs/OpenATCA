#![allow(
    unsafe_code,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use openatca_core_ffi::{
    oa_grib2_decode_message, oa_grib2_inspect_message, OaErrorInfo, OaGrib2Field,
};

const VECTORS: &str = include_str!("fixtures/grib2-bitmap-v1.tsv");

fn fixture() -> Vec<u8> {
    let hex = VECTORS
        .lines()
        .nth(1)
        .expect("synthetic row")
        .split('\t')
        .nth(1)
        .expect("hex column");
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let nibble = |b: u8| (b as char).to_digit(16).expect("hex digit") as u8;
            nibble(pair[0]) * 16 + nibble(pair[1])
        })
        .collect()
}

fn sections(data: &[u8]) -> Vec<(u8, usize)> {
    let mut result = Vec::new();
    let mut pos = 16;
    while pos + 5 <= data.len() - 4 {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        assert!(len >= 5 && pos + len <= data.len());
        result.push((data[pos + 4], pos));
        pos += len;
    }
    result
}

// REQ: OA-FFI-001, OA-FFI-002, OA-FFI-005
#[test]
fn bitmap_valid_truncated_unsupported_and_capacity_are_transactional() {
    let bitmap = fixture();
    let bitmap = bitmap.as_slice();
    let mut field_count = u64::MAX;
    let mut value_count = u64::MAX;
    let mut error = OaErrorInfo {
        code: 0,
        location_kind: 0,
        location: 0,
    };
    // SAFETY: synthetic fixture is readable; each output is a distinct valid scalar.
    let status = unsafe {
        oa_grib2_inspect_message(
            bitmap.as_ptr(),
            bitmap.len() as u64,
            &mut field_count,
            &mut value_count,
            &mut error,
        )
    };
    assert_eq!(status, 0);
    assert_eq!((field_count, value_count), (1, 12));

    let mut fields = [OaGrib2Field::default(); 1];
    let mut values = [f32::NAN; 12];
    // SAFETY: input and output arrays are valid and disjoint; capacities match query.
    let status = unsafe {
        oa_grib2_decode_message(
            bitmap.as_ptr(),
            bitmap.len() as u64,
            fields.as_mut_ptr(),
            1,
            values.as_mut_ptr(),
            12,
            &mut field_count,
            &mut value_count,
            &mut error,
        )
    };
    assert_eq!(status, 0);
    let f = fields[0];
    assert_eq!((f.discipline, f.category, f.number), (0, 2, 2));
    assert_eq!(
        (f.grid_template, f.product_template, f.packing_template),
        (0, 0, 0)
    );
    assert_eq!((f.nx, f.ny, f.missing_count), (4, 3, 5));
    assert_eq!(f.first_latitude_deg, 42.0);
    assert_eq!(f.first_longitude_deg, -90.0);
    assert_eq!((f.i_increment_deg, f.j_increment_deg), (0.25, 0.5));
    assert_eq!(
        (
            f.reference_time.year,
            f.reference_time.month,
            f.reference_time.day,
            f.reference_time.hour
        ),
        (2026, 11, 14, 18)
    );
    assert_eq!(f.forecast_time, 6);
    assert_eq!((f.value_offset, f.value_count), (0, 12));
    assert_eq!(values[0], 1.0);
    assert!(values[1].is_nan());
    assert_eq!(values[2], 2.0);
    assert_eq!(values[3], 3.0);
    assert!(values[4].is_nan());
    assert_eq!(values[5], 4.0);
    assert!(values[6].is_nan());
    assert_eq!(values[7], 5.0);
    assert_eq!(values[8], 6.0);
    assert!(values[9].is_nan());
    assert_eq!(values[10], 7.0);
    assert!(values[11].is_nan());

    let mut short_fields = [OaGrib2Field {
        nx: 999,
        ..OaGrib2Field::default()
    }];
    let mut short_values = [42.0f32; 12];
    // SAFETY: valid, disjoint input/output; deliberately insufficient field capacity.
    let status = unsafe {
        oa_grib2_decode_message(
            bitmap.as_ptr(),
            bitmap.len() as u64,
            short_fields.as_mut_ptr(),
            0,
            short_values.as_mut_ptr(),
            12,
            &mut field_count,
            &mut value_count,
            &mut error,
        )
    };
    assert_eq!(status, 6);
    assert_eq!(short_fields[0].nx, 999);
    assert_eq!(short_values, [42.0; 12]);
    assert_eq!((field_count, value_count), (1, 12));

    let mut truncated = bitmap.to_vec();
    truncated.pop();
    // SAFETY: valid shortened input and distinct writable scalars.
    let status = unsafe {
        oa_grib2_inspect_message(
            truncated.as_ptr(),
            truncated.len() as u64,
            &mut field_count,
            &mut value_count,
            &mut error,
        )
    };
    assert_ne!(status, 0);

    let mut unsupported = bitmap.to_vec();
    let (_, section5) = sections(&unsupported)
        .into_iter()
        .find(|(n, _)| *n == 5)
        .unwrap();
    unsupported[section5 + 9..section5 + 11].copy_from_slice(&41u16.to_be_bytes());
    let mut unsupported_field = OaGrib2Field::default();
    let mut unsupported_values = [0.0f32; 12];
    // SAFETY: valid input and disjoint output storage of the inspected sizes.
    let status = unsafe {
        oa_grib2_decode_message(
            unsupported.as_ptr(),
            unsupported.len() as u64,
            &mut unsupported_field,
            1,
            unsupported_values.as_mut_ptr(),
            12,
            &mut field_count,
            &mut value_count,
            &mut error,
        )
    };
    assert_eq!(status, 81);
}

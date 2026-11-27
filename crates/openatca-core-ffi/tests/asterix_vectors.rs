#![allow(
    unsafe_code,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

// This test crosses the raw-pointer ABI with owned, disjoint synthetic buffers.

use oa_asterix::{
    decode_block_with, Block, Cat021Edition, ErrorKind, NO_ITEM, NO_OFFSET, NO_RECORD,
};
use openatca_core_ffi::{
    oa_asterix_decode_block, OaAsterixCounts, OaAsterixError, OaAsterixRecord, OaErrorInfo,
};

const VECTORS: &str = include_str!("fixtures/asterix-v1.tsv");

fn hex_bytes(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).expect("hex digit");
            let low = (pair[1] as char).to_digit(16).expect("hex digit");
            ((high << 4) | low) as u8
        })
        .collect()
}

fn kind_code(kind: ErrorKind) -> u32 {
    match kind {
        ErrorKind::Truncated => 1,
        ErrorKind::BlockLengthBelowHeader => 2,
        ErrorKind::BlockLengthExceedsInput => 3,
        ErrorKind::EmptyBlock => 4,
        ErrorKind::TooManyRecords => 5,
        ErrorKind::UnsupportedCategory => 6,
        ErrorKind::EmptyFspec => 7,
        ErrorKind::FspecTooLong => 8,
        ErrorKind::FspecNotMinimal => 9,
        ErrorKind::UnknownUapSlot { .. } => 10,
        ErrorKind::MissingMandatory { .. } => 11,
        ErrorKind::ExtensionUnterminated => 12,
        ErrorKind::ZeroRepetition => 13,
        ErrorKind::ExplicitLengthInvalid => 14,
        ErrorKind::EmptyCompound => 15,
        ErrorKind::UnknownSubitem => 16,
        ErrorKind::CompoundNotMinimal => 17,
        ErrorKind::SpareBitSet => 18,
        ErrorKind::InvalidValue => 19,
        ErrorKind::ValueOutOfRange => 20,
        ErrorKind::DuplicateItem => 21,
        ErrorKind::BlockTooLong => 22,
        ErrorKind::UnknownFrn { .. } => 23,
    }
}

fn edition(selector: u8) -> Cat021Edition {
    if selector == 24 {
        Cat021Edition::Ed24
    } else {
        Cat021Edition::Ed27
    }
}

fn row_fields(row: &OaAsterixRecord) -> u64 {
    row.present_field_mask
}

fn check_projection(reference: &Block, row: &OaAsterixRecord, case: &str, index: usize) {
    match reference {
        Block::Cat021(records) => {
            let source = records.get(index).expect("CAT021 record");
            assert_eq!(
                (row.source_sac, row.source_sic),
                (source.source.sac, source.source.sic),
                "{case}"
            );
            assert_eq!(row.target_address, source.address.unwrap_or(0), "{case}");
            assert_eq!(row.mode3a_code, source.mode3a.unwrap_or(0), "{case}");
            assert_eq!(
                row.vertical_count,
                u8::from(source.flight_level.is_some()),
                "{case}"
            );
            if let Some(position) = source.position_hr {
                let output = row.positions.first().expect("low or high position slot");
                assert_eq!(output.item, 131, "{case}");
                assert_eq!(
                    (output.a_raw, output.b_raw),
                    (position.lat, position.lon),
                    "{case}"
                );
            }
            assert_eq!(
                row.position_count,
                u8::from(source.position.is_some()) + u8::from(source.position_hr.is_some()),
                "{case}"
            );
            assert_eq!(row.velocity_kind, 0, "{case}");
            assert_eq!(row.raw_untyped_subitem_mask, 0, "{case}");
        }
        Block::Cat048(records) => {
            let source = records.get(index).expect("CAT048 record");
            assert_eq!(
                (row.source_sac, row.source_sic),
                (source.source.sac, source.source.sic),
                "{case}"
            );
            assert_eq!(row.target_address, source.address.unwrap_or(0), "{case}");
            assert_eq!(row.target_ident, source.ident.unwrap_or([0; 8]), "{case}");
            assert_eq!(row.track_number, source.track_number.unwrap_or(0), "{case}");
            assert_eq!(row.time_raw, source.time_of_day.unwrap_or(0), "{case}");
            assert_eq!(
                row.mode3a_code,
                source.mode3a.map(|v| v.code).unwrap_or(0),
                "{case}"
            );
            assert_eq!(
                row.vertical_count,
                u8::from(source.flight_level.is_some()),
                "{case}"
            );
            assert_eq!(
                row.position_count,
                u8::from(source.polar.is_some()) + u8::from(source.cartesian.is_some()),
                "{case}"
            );
            if let Some(polar) = source.polar {
                let output = row.positions.first().expect("polar slot");
                assert_eq!(
                    (output.item, output.a_raw, output.b_raw),
                    (40, i32::from(polar.rho), i32::from(polar.theta)),
                    "{case}"
                );
            }
            if let Some(cartesian) = source.cartesian {
                let index = usize::from(source.polar.is_some());
                let output = row.positions.get(index).expect("Cartesian slot");
                assert_eq!(
                    (output.item, output.a_raw, output.b_raw),
                    (42, i32::from(cartesian.x), i32::from(cartesian.y)),
                    "{case}"
                );
            }
        }
        Block::Cat062(records) => {
            let source = records.get(index).expect("CAT062 record");
            assert_eq!(
                (row.source_sac, row.source_sic),
                (source.source.sac, source.source.sic),
                "{case}"
            );
            assert_eq!(
                row.target_address,
                source.derived.as_ref().and_then(|v| v.address).unwrap_or(0),
                "{case}"
            );
            assert_eq!(
                row.target_ident,
                source
                    .derived
                    .as_ref()
                    .and_then(|v| v.ident)
                    .unwrap_or([0; 8]),
                "{case}"
            );
            assert_eq!(row.track_number, source.track_number.unwrap_or(0), "{case}");
            assert_eq!(row.time_raw, source.time.unwrap_or(0), "{case}");
            assert_eq!(
                row.velocity_a_raw,
                source.velocity.map(|v| i32::from(v.vx)).unwrap_or(0),
                "{case}"
            );
            assert_eq!(
                row.velocity_b_raw,
                source.velocity.map(|v| i32::from(v.vy)).unwrap_or(0),
                "{case}"
            );
            assert_eq!(row.vertical_count, 4, "{case}");
            assert_eq!(
                row.position_count,
                u8::from(source.position.is_some()) + u8::from(source.cartesian.is_some()),
                "{case}"
            );
            assert_eq!(row.raw_untyped_subitem_mask, 0, "{case}");
        }
    }
    assert!(row.present_field_mask != 0, "{case}");
    let _ = row_fields(row);
}

fn unused_row() -> OaAsterixRecord {
    OaAsterixRecord {
        category: 201,
        ..OaAsterixRecord::default()
    }
}

fn raw_frn_mask(reference: &Block, index: usize) -> u64 {
    let raw = match reference {
        Block::Cat021(records) => records.get(index).map(|record| record.raw.as_slice()),
        Block::Cat048(records) => records.get(index).map(|record| record.raw.as_slice()),
        Block::Cat062(records) => records.get(index).map(|record| record.raw.as_slice()),
    }
    .expect("typed record exists");
    raw.iter()
        .fold(0u64, |mask, item| mask | (1u64 << (item.frn - 1)))
}

fn sentinel_counts() -> OaAsterixCounts {
    OaAsterixCounts {
        category: 201,
        edition: 201,
        reserved: 201,
        reserved_bytes: [201; 4],
        consumed_bytes: 201,
        required_record_count: 201,
        written_record_count: 201,
    }
}

fn sentinel_detail() -> OaAsterixError {
    OaAsterixError {
        status: 201,
        kind: 201,
        category: 201,
        subitem: 201,
        item: 201,
        record: 201,
        reserved_bytes: [201; 8],
        offset: 201,
    }
}

// REQ: OA-FFI-001, OA-FFI-003
#[test]
fn c_abi_matches_the_language_neutral_asterix_vectors() {
    assert_eq!(oa_asterix::MAX_RECORDS_PER_BLOCK, 21_844);
    for (line_number, line) in VECTORS.lines().enumerate().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 12, "vector line {}", line_number + 1);
        let case = fields[0];
        let selector = fields[1].parse::<u8>().expect("selector");
        let input = hex_bytes(fields[2]);
        let expected_status = fields[3].parse::<u32>().expect("status");
        let expected_kind = fields[4].parse::<u32>().expect("kind");
        let expected_category = fields[5].parse::<u8>().expect("category");
        let expected_records = fields[6].parse::<u64>().expect("record count");
        let expected_consumed = fields[7].parse::<u64>().expect("consumed count");
        let expected_record = fields[8].parse::<u32>().expect("record");
        let expected_item = fields[9].parse::<u16>().expect("item");
        let expected_subitem = fields[10].parse::<u8>().expect("subitem");
        let expected_offset = fields[11].parse::<u64>().expect("offset");
        let core_result = decode_block_with(&input, edition(selector));

        let mut rows = [unused_row(); 2];
        let mut counts = sentinel_counts();
        let mut detail = sentinel_detail();
        let mut generic = OaErrorInfo {
            code: 201,
            location_kind: 201,
            location: 201,
        };
        // SAFETY: input is an owned Vec; arrays and metadata are live, aligned, and disjoint.
        let status = unsafe {
            oa_asterix_decode_block(
                input.as_ptr(),
                input.len() as u64,
                selector,
                rows.as_mut_ptr(),
                rows.len() as u64,
                &mut counts,
                &mut detail,
                &mut generic,
            )
        };
        assert_eq!(status, expected_status, "{case}");
        assert_eq!(generic.code, expected_status, "{case}");
        if expected_status == 0 {
            let (reference, consumed) = core_result.expect("Rust reference accepts vector");
            assert_eq!(
                u64::try_from(consumed).expect("consumed fits"),
                expected_consumed,
                "{case}"
            );
            assert_eq!(counts.category, expected_category, "{case}");
            assert_eq!(counts.required_record_count, expected_records, "{case}");
            assert_eq!(counts.written_record_count, expected_records, "{case}");
            assert_eq!(counts.consumed_bytes, expected_consumed, "{case}");
            let raw = oa_asterix::parse_block_with(&input, 0, edition(selector))
                .expect("wire reference parses");
            for (index, row) in rows
                .iter()
                .take(usize::try_from(expected_records).expect("count fits"))
                .enumerate()
            {
                check_projection(&reference, row, case, index);
                let expected_present = raw
                    .records
                    .get(index)
                    .expect("wire record")
                    .spans
                    .iter()
                    .fold(0u64, |mask, span| mask | (1u64 << (span.frn - 1)));
                assert_eq!(
                    row.present_frn_mask, expected_present,
                    "{case} FRN presence"
                );
                assert_eq!(
                    row.raw_untyped_frn_mask,
                    raw_frn_mask(&reference, index),
                    "{case} raw FRN mask"
                );
                assert_eq!(
                    row.raw_untyped_frn_mask & !row.present_frn_mask,
                    0,
                    "{case} raw mask subset"
                );
                assert_eq!(
                    row.typed_unprojected_frn_mask & row.raw_untyped_frn_mask,
                    0,
                    "{case} typed/raw masks disjoint"
                );
            }
            // The same valid vector exercises size query and all-or-nothing capacity refusal.
            let mut query_counts = sentinel_counts();
            let mut query_detail = sentinel_detail();
            let mut query_error = OaErrorInfo {
                code: 0,
                location_kind: 0,
                location: 0,
            };
            // SAFETY: input and the two metadata records are owned, aligned and disjoint.
            let query_status = unsafe {
                oa_asterix_decode_block(
                    input.as_ptr(),
                    input.len() as u64,
                    selector,
                    core::ptr::null_mut(),
                    0,
                    &mut query_counts,
                    &mut query_detail,
                    &mut query_error,
                )
            };
            assert_eq!(query_status, 6, "{case} query");
            assert_eq!(
                query_counts.required_record_count, expected_records,
                "{case} query"
            );
            assert_eq!(query_counts.written_record_count, 0, "{case} query");
            assert_eq!(query_detail.kind, 1001, "{case} query");
            let mut short_rows = [unused_row(); 2];
            let mut short_counts = sentinel_counts();
            let mut short_detail = sentinel_detail();
            let mut short_error = OaErrorInfo {
                code: 0,
                location_kind: 0,
                location: 0,
            };
            // SAFETY: input, short output, and metadata are separate live storage.
            let short_status = unsafe {
                oa_asterix_decode_block(
                    input.as_ptr(),
                    input.len() as u64,
                    selector,
                    short_rows.as_mut_ptr(),
                    expected_records.saturating_sub(1),
                    &mut short_counts,
                    &mut short_detail,
                    &mut short_error,
                )
            };
            assert_eq!(short_status, 6, "{case} short capacity");
            assert!(
                short_rows.iter().all(|row| *row == unused_row()),
                "{case} short capacity"
            );
            assert_eq!(
                short_counts.required_record_count, expected_records,
                "{case} short capacity"
            );
            assert_eq!(
                short_counts.written_record_count, 0,
                "{case} short capacity"
            );
        } else {
            let reference = core_result.expect_err("Rust reference rejects refusal vector");
            assert_eq!(reference.cat, expected_category, "{case}");
            assert_eq!(reference.record, expected_record, "{case}");
            assert_eq!(reference.item, expected_item, "{case}");
            assert_eq!(reference.sub, expected_subitem, "{case}");
            assert_eq!(kind_code(reference.kind), expected_kind, "{case}");
            assert_eq!(
                reference.offset,
                usize::try_from(expected_offset).unwrap_or(usize::MAX),
                "{case}"
            );
            assert_eq!(detail.category, expected_category, "{case}");
            assert_eq!(detail.record, expected_record, "{case}");
            assert_eq!(detail.item, expected_item, "{case}");
            assert_eq!(detail.subitem, expected_subitem, "{case}");
            assert_eq!(detail.kind, expected_kind, "{case}");
            assert_eq!(detail.offset, expected_offset, "{case}");
            assert_eq!(counts, sentinel_counts(), "{case} refusal counts unchanged");
            assert!(
                rows.iter().all(|row| *row == unused_row()),
                "{case} refusal rows unchanged"
            );
        }
    }
    assert_eq!(NO_RECORD, u32::MAX);
    assert_eq!(NO_ITEM, 0);
    assert_eq!(NO_OFFSET, usize::MAX);
}

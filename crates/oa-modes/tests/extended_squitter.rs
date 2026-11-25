#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_modes::{
    decode_extended_squitter, AddressUse, AltitudeStatus, BodyKind, CprFields, CprKind,
    DecodeError, FRAME_LEN,
};

const TEST_GENERATOR: u32 = 0x01ff_f409;

fn set_bits(frame: &mut [u8; FRAME_LEN], start: usize, width: usize, value: u32) {
    for offset in 0..width {
        let source_shift = width - 1 - offset;
        let bit = (value >> source_shift) & 1;
        let target = start + offset;
        if bit != 0 {
            frame[target / 8] |= 1 << (7 - target % 8);
        }
    }
}

fn get_bit(frame: &[u8; FRAME_LEN], bit: usize) -> bool {
    frame[bit / 8] & (1 << (7 - bit % 8)) != 0
}

/// Test-only polynomial long division, intentionally separate from the production LFSR.
fn append_independent_parity(frame: &mut [u8; FRAME_LEN]) {
    let mut dividend = [false; FRAME_LEN * 8];
    for (bit, slot) in dividend.iter_mut().take(88).enumerate() {
        *slot = get_bit(frame, bit);
    }
    for lead in 0..88 {
        if dividend[lead] {
            for tap in 0..=24 {
                if (TEST_GENERATOR >> (24 - tap)) & 1 != 0 {
                    dividend[lead + tap] ^= true;
                }
            }
        }
    }
    for (offset, &bit) in dividend.iter().skip(88).take(24).enumerate() {
        if bit {
            let target = 88 + offset;
            frame[target / 8] |= 1 << (7 - target % 8);
        }
    }
}

fn synthetic_frame(
    format: u8,
    control: u8,
    address: u32,
    type_code: u8,
    category: u8,
    identification_codes: [u8; 8],
) -> [u8; FRAME_LEN] {
    let mut frame = [0u8; FRAME_LEN];
    set_bits(&mut frame, 0, 5, u32::from(format));
    set_bits(&mut frame, 5, 3, u32::from(control));
    set_bits(&mut frame, 8, 24, address);
    set_bits(&mut frame, 32, 5, u32::from(type_code));
    set_bits(&mut frame, 37, 3, u32::from(category));
    for (slot, code) in identification_codes.into_iter().enumerate() {
        set_bits(&mut frame, 40 + slot * 6, 6, u32::from(code));
    }
    append_independent_parity(&mut frame);
    frame
}

fn synthetic_altitude_frame(
    format: u8,
    control: u8,
    type_code: u8,
    altitude_code: u16,
) -> [u8; FRAME_LEN] {
    let mut frame = [0u8; FRAME_LEN];
    set_bits(&mut frame, 0, 5, u32::from(format));
    set_bits(&mut frame, 5, 3, u32::from(control));
    set_bits(&mut frame, 8, 24, 0xA1_B2_C3);
    set_bits(&mut frame, 32, 5, u32::from(type_code));
    set_bits(&mut frame, 40, 12, u32::from(altitude_code));
    append_independent_parity(&mut frame);
    frame
}

fn synthetic_cpr_frame(
    format: u8,
    control: u8,
    type_code: u8,
    cpr_format: u8,
    latitude_raw: u32,
    longitude_raw: u32,
) -> [u8; FRAME_LEN] {
    let mut frame = [0u8; FRAME_LEN];
    set_bits(&mut frame, 0, 5, u32::from(format));
    set_bits(&mut frame, 5, 3, u32::from(control));
    set_bits(&mut frame, 8, 24, 0xA1_B2_C3);
    set_bits(&mut frame, 32, 5, u32::from(type_code));
    set_bits(&mut frame, 53, 1, u32::from(cpr_format));
    set_bits(&mut frame, 54, 17, latitude_raw);
    set_bits(&mut frame, 71, 17, longitude_raw);
    append_independent_parity(&mut frame);
    frame
}

// REQ: OA-MODE-003, OA-MODE-004
#[test]
fn df17_identity_and_typecode_one_decode_from_direct_fields() {
    let frame = synthetic_frame(17, 5, 0xA1_B2_C3, 1, 4, [14, 49, 50, 51, 1, 2, 32, 32]);
    let decoded = decode_extended_squitter(&frame).unwrap();
    assert_eq!(decoded.raw, frame);
    assert_eq!(decoded.downlink_format, 17);
    assert_eq!(decoded.control, 5);
    assert_eq!(decoded.announced_address, 0xA1_B2_C3);
    assert_eq!(decoded.address_use, AddressUse::Icao24);
    assert_eq!(decoded.type_code, 1);
    assert_eq!(decoded.aircraft_category, 4);
    assert_eq!(decoded.body_kind, BodyKind::AircraftIdentification);
    assert_eq!(decoded.identification, *b"N123AB  ");
}

// REQ: OA-MODE-003, OA-MODE-004
#[test]
fn df18_cf_zero_exposes_direct_icao_address_and_identification() {
    let frame = synthetic_frame(18, 0, 0xAB_CDEF, 4, 2, [2, 1, 20, 32, 49, 50, 51, 32]);
    let decoded = decode_extended_squitter(&frame).unwrap();
    assert_eq!(decoded.downlink_format, 18);
    assert_eq!(decoded.address_use, AddressUse::Icao24);
    assert_eq!(decoded.announced_address, 0xAB_CDEF);
    assert_eq!(decoded.body_kind, BodyKind::AircraftIdentification);
    assert_eq!(decoded.identification, *b"BAT 123 ");
}

// REQ: OA-MODE-003, OA-MODE-004
#[test]
fn df18_all_control_fields_have_explicit_address_and_interpretation_status() {
    let expected = [
        (AddressUse::Icao24, BodyKind::AircraftIdentification),
        (AddressUse::NonIcao24, BodyKind::Uninterpreted),
        (AddressUse::TisB, BodyKind::Uninterpreted),
        (AddressUse::TisB, BodyKind::Uninterpreted),
        (AddressUse::TisB, BodyKind::Uninterpreted),
        (AddressUse::TisB, BodyKind::Uninterpreted),
        // CF=6 uses DF17 message formats, but this slice does not classify its AA field.
        (AddressUse::Unclassified, BodyKind::AircraftIdentification),
        (AddressUse::Unclassified, BodyKind::Uninterpreted),
    ];
    for (control, &(address_use, body_kind)) in expected.iter().enumerate() {
        let control = u8::try_from(control).unwrap();
        let frame = synthetic_frame(18, control, 0x12_3456, 1, 1, [1; 8]);
        let decoded = decode_extended_squitter(&frame).unwrap();
        assert_eq!(decoded.control, control);
        assert_eq!(decoded.address_use, address_use);
        assert_eq!(decoded.body_kind, body_kind);
        assert_eq!(
            decoded.identification,
            if control == 0 || control == 6 {
                *b"AAAAAAAA"
            } else {
                [0; 8]
            }
        );
    }
}

// REQ: OA-MODE-003, OA-MODE-004
#[test]
fn df18_cf_six_decodes_df17_identification_without_promoting_aa_identity() {
    let frame = synthetic_frame(18, 6, 0xA1_B2_C3, 1, 4, [14, 49, 50, 51, 1, 2, 32, 32]);
    let decoded = decode_extended_squitter(&frame).unwrap();
    assert_eq!(decoded.raw, frame);
    assert_eq!(decoded.address_use, AddressUse::Unclassified);
    assert_eq!(decoded.announced_address, 0xA1_B2_C3);
    assert_eq!(decoded.type_code, 1);
    assert_eq!(decoded.aircraft_category, 4);
    assert_eq!(decoded.body_kind, BodyKind::AircraftIdentification);
    assert_eq!(decoded.identification, *b"N123AB  ");
}

// REQ: OA-MODE-007
#[test]
fn airborne_position_q_one_altitude_decodes_exact_25_foot_values() {
    for type_code in 9..=18 {
        let frame = synthetic_altitude_frame(17, 0, type_code, 0x0fff);
        let decoded = decode_extended_squitter(&frame).unwrap();
        assert_eq!(decoded.type_code, type_code);
        assert_eq!(decoded.altitude_status, AltitudeStatus::Encoded25Ft);
        assert_eq!(decoded.altitude_ft, 50_175);
    }

    for (code, expected_ft) in [(0x0010, -1_000), (0x0011, -975)] {
        let frame = synthetic_altitude_frame(18, 6, 9, code);
        let decoded = decode_extended_squitter(&frame).unwrap();
        assert_eq!(decoded.address_use, AddressUse::Unclassified);
        assert_eq!(decoded.altitude_status, AltitudeStatus::Encoded25Ft);
        assert_eq!(decoded.altitude_ft, expected_ft);
    }
}

// REQ: OA-MODE-007
#[test]
fn airborne_position_missing_or_unsupported_altitude_stays_distinct_and_raw() {
    let unavailable = synthetic_altitude_frame(17, 0, 9, 0);
    let decoded_unavailable = decode_extended_squitter(&unavailable).unwrap();
    assert_eq!(
        decoded_unavailable.altitude_status,
        AltitudeStatus::Unavailable
    );
    assert_eq!(decoded_unavailable.altitude_ft, 0);
    assert_eq!(decoded_unavailable.raw, unavailable);

    let q_zero = synthetic_altitude_frame(17, 0, 9, 0x0100);
    let decoded_q_zero = decode_extended_squitter(&q_zero).unwrap();
    assert_eq!(
        decoded_q_zero.altitude_status,
        AltitudeStatus::OtherEncoding
    );
    assert_eq!(decoded_q_zero.altitude_ft, 0);
    assert_eq!(decoded_q_zero.raw, q_zero);

    let not_airborne_position = synthetic_altitude_frame(17, 0, 20, 0x0fff);
    let decoded_other_type = decode_extended_squitter(&not_airborne_position).unwrap();
    assert_eq!(
        decoded_other_type.altitude_status,
        AltitudeStatus::NotDecoded
    );
    assert_eq!(decoded_other_type.altitude_ft, 0);

    let non_adsb_cf = synthetic_altitude_frame(18, 1, 9, 0x0fff);
    let decoded_non_adsb_cf = decode_extended_squitter(&non_adsb_cf).unwrap();
    assert_eq!(
        decoded_non_adsb_cf.altitude_status,
        AltitudeStatus::NotDecoded
    );
    assert_eq!(decoded_non_adsb_cf.body_kind, BodyKind::Uninterpreted);
}

// REQ: OA-MODE-005, OA-MODE-008
#[test]
fn surface_and_airborne_position_keep_raw_cpr_fields_without_claiming_coordinates() {
    for (type_code, cpr_kind) in [
        (5, CprKind::Surface),
        (8, CprKind::Surface),
        (9, CprKind::Airborne),
        (18, CprKind::Airborne),
    ] {
        let frame = synthetic_cpr_frame(17, 0, type_code, 1, 0x1ffff, 0x12345);
        let decoded = decode_extended_squitter(&frame).unwrap();
        assert_eq!(
            decoded.cpr,
            CprFields {
                kind: cpr_kind,
                format: 1,
                latitude_raw: 0x1ffff,
                longitude_raw: 0x12345,
            }
        );
    }

    let even_surface = synthetic_cpr_frame(18, 6, 7, 0, 0, 0);
    let decoded = decode_extended_squitter(&even_surface).unwrap();
    assert_eq!(decoded.cpr.kind, CprKind::Surface);
    assert_eq!(decoded.cpr.format, 0);
    assert_eq!(decoded.cpr.latitude_raw, 0);
    assert_eq!(decoded.cpr.longitude_raw, 0);

    let non_adsb_control = synthetic_cpr_frame(18, 1, 9, 1, 1, 2);
    assert_eq!(
        decode_extended_squitter(&non_adsb_control)
            .unwrap()
            .cpr
            .kind,
        CprKind::NotPosition
    );
}

// REQ: OA-MODE-004, OA-MODE-006
#[test]
fn unsupported_typecode_is_preserved_as_raw_extended_squitter() {
    let frame = synthetic_frame(17, 0, 0x12_3456, 19, 0, [0; 8]);
    let decoded = decode_extended_squitter(&frame).unwrap();
    assert_eq!(decoded.type_code, 19);
    assert_eq!(decoded.body_kind, BodyKind::OtherExtendedSquitter);
    assert_eq!(decoded.raw, frame);
}

// REQ: OA-MODE-004
#[test]
fn all_zero_identification_codes_mean_absent_not_eight_spaces() {
    let frame = synthetic_frame(17, 0, 0x12_3456, 2, 3, [0; 8]);
    let decoded = decode_extended_squitter(&frame).unwrap();
    assert_eq!(decoded.body_kind, BodyKind::AircraftIdentificationAbsent);
    assert_eq!(decoded.identification, [0; 8]);
}

// REQ: OA-MODE-001, OA-MODE-002, OA-MODE-004, OA-MODE-006
#[test]
fn malformed_length_format_parity_and_character_have_typed_refusals() {
    let valid = synthetic_frame(17, 0, 0x12_3456, 1, 0, [1; 8]);
    assert_eq!(
        decode_extended_squitter(&valid[..13]),
        Err(DecodeError::Length { actual: 13 })
    );
    let trailing = valid;
    let mut longer = [0u8; FRAME_LEN + 1];
    longer[..FRAME_LEN].copy_from_slice(&trailing);
    assert_eq!(
        decode_extended_squitter(&longer),
        Err(DecodeError::Length { actual: 15 })
    );

    for format in 0..32u8 {
        if format == 17 || format == 18 {
            continue;
        }
        let unsupported_df = synthetic_frame(format, 0, 0x12_3456, 1, 0, [1; 8]);
        assert_eq!(
            decode_extended_squitter(&unsupported_df),
            Err(DecodeError::UnsupportedDownlinkFormat { format })
        );
    }

    for bit in [10usize, 32, 111] {
        let mut damaged = valid;
        damaged[bit / 8] ^= 1 << (7 - bit % 8);
        assert!(matches!(
            decode_extended_squitter(&damaged),
            Err(DecodeError::Parity { syndrome }) if syndrome != 0
        ));
    }

    let invalid_character = synthetic_frame(17, 0, 0x12_3456, 1, 0, [27, 1, 1, 1, 1, 1, 1, 1]);
    assert_eq!(
        decode_extended_squitter(&invalid_character),
        Err(DecodeError::InvalidIdentificationCharacter { slot: 0, code: 27 })
    );

    let invalid_cf6_character = synthetic_frame(18, 6, 0x12_3456, 1, 0, [63, 1, 1, 1, 1, 1, 1, 1]);
    assert_eq!(
        decode_extended_squitter(&invalid_cf6_character),
        Err(DecodeError::InvalidIdentificationCharacter { slot: 0, code: 63 })
    );
}

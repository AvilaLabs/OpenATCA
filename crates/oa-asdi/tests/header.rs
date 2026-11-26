#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_asdi::{
    inspect_packet, observe_sequence, parse_header, AsdiError, PacketType, PayloadLimits,
    PayloadStatus, SequenceStatus, DATA_TYPE_HEARTBEAT, DATA_TYPE_XML, HEADER_LEN,
    MAX_COMPRESSED_PAYLOAD_BYTES, MAX_DECOMPRESSED_PAYLOAD_BYTES, MAX_SEQUENCE_NUMBER,
    NO_EXPECTED_SEQUENCE,
};

const REGISTRY: &str = include_str!("../../../validation/interface-conformance-v1.json");

const XML_HEADER: [u8; 32] = [
    b'2', b'0', b'2', b'6', b'1', b'1', b'2', b'5', b'1', b'5', b'3', b'0', b'4', b'5', 0, b' ', 0,
    0, 0, 2, 0, 1, 0x86, 0xa0, 0, 0, 0, 7, 0, 0, 0, 123,
];

const HEARTBEAT_HEADER: [u8; 32] = [
    b'2', b'0', b'2', b'6', b'1', b'1', b'2', b'5', b'1', b'5', b'3', b'0', b'4', b'5', 0, b' ', 0,
    0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

const LIMITS: PayloadLimits = PayloadLimits {
    max_compressed_bytes: 64,
    max_decompressed_bytes: 256,
};

fn xml_packet() -> Vec<u8> {
    let mut packet = XML_HEADER.to_vec();
    packet.extend_from_slice(&[0; 7]);
    packet
}

// REQ: OA-ASDI-001
#[test]
fn parses_exact_golden_header_and_network_order_fields() {
    let header = parse_header(&XML_HEADER).unwrap();
    assert_eq!(header.timestamp.year, 2026);
    assert_eq!(header.timestamp.month, 11);
    assert_eq!(header.timestamp.day, 25);
    assert_eq!(header.timestamp.hour, 15);
    assert_eq!(header.timestamp.minute, 30);
    assert_eq!(header.timestamp.second, 45);
    assert_eq!(header.timestamp.reserved, 0);
    assert_eq!(header.packet_type, PacketType::Xml);
    assert_eq!(header.sequence_number, MAX_SEQUENCE_NUMBER);
    assert_eq!(header.compressed_size, 7);
    assert_eq!(header.decompressed_size, 123);
    assert_eq!(DATA_TYPE_HEARTBEAT, 1);
    assert_eq!(DATA_TYPE_XML, 2);
    assert_eq!(HEADER_LEN, 32);
}

// REQ: OA-ASDI-002
#[test]
fn heartbeat_is_header_only_and_xml_is_explicitly_unvalidated() {
    let heartbeat = inspect_packet(&HEARTBEAT_HEADER, LIMITS).unwrap();
    assert_eq!(heartbeat.payload_status, PayloadStatus::HeartbeatNoPayload);

    let xml = inspect_packet(&xml_packet(), LIMITS).unwrap();
    assert_eq!(
        xml.payload_status,
        PayloadStatus::XmlCompressedNotDecoded {
            compressed_bytes: 7,
            expected_decompressed_bytes: 123,
        }
    );
    // This layer has not established zlib validity or ASDI XSD/message validity.
}

// REQ: OA-ASDI-003
#[test]
fn exact_header_and_packet_lengths_are_required() {
    assert_eq!(
        parse_header(&XML_HEADER[..31]),
        Err(AsdiError::HeaderLength { actual: 31 })
    );
    assert_eq!(
        parse_header(&[]),
        Err(AsdiError::HeaderLength { actual: 0 })
    );
    assert_eq!(
        inspect_packet(&[], LIMITS),
        Err(AsdiError::HeaderLength { actual: 0 })
    );
    let mut too_long = XML_HEADER.to_vec();
    too_long.push(0);
    assert_eq!(
        parse_header(&too_long),
        Err(AsdiError::HeaderLength { actual: 33 })
    );
    assert_eq!(
        inspect_packet(&XML_HEADER, LIMITS),
        Err(AsdiError::PacketLengthMismatch {
            expected: 39,
            actual: 32,
        })
    );
    let mut extra = xml_packet();
    extra.push(0);
    assert_eq!(
        inspect_packet(&extra, LIMITS),
        Err(AsdiError::PacketLengthMismatch {
            expected: 39,
            actual: 40,
        })
    );
}

// REQ: OA-ASDI-004
#[test]
fn malformed_timestamp_type_and_sequence_are_refused() {
    let mut bad = XML_HEADER;
    bad[0] = b'x';
    assert_eq!(
        parse_header(&bad),
        Err(AsdiError::TimestampCharacter { offset: 0 })
    );
    let mut bad = XML_HEADER;
    bad[14] = b'!';
    assert_eq!(
        parse_header(&bad),
        Err(AsdiError::TimestampPadding { offset: 14 })
    );
    let mut bad = XML_HEADER;
    bad[6] = b'3';
    assert_eq!(parse_header(&bad), Err(AsdiError::InvalidCalendarDate));
    let mut leap_day = XML_HEADER;
    leap_day[0..4].copy_from_slice(b"2024");
    leap_day[4..8].copy_from_slice(b"0229");
    assert!(parse_header(&leap_day).is_ok());
    let mut non_leap_day = leap_day;
    non_leap_day[0..4].copy_from_slice(b"2025");
    assert_eq!(
        parse_header(&non_leap_day),
        Err(AsdiError::InvalidCalendarDate)
    );
    let mut bad = XML_HEADER;
    bad[12] = b'6';
    bad[13] = b'0';
    assert_eq!(parse_header(&bad), Err(AsdiError::InvalidTimeOfDay));
    let mut bad = XML_HEADER;
    bad[19] = 3;
    assert_eq!(
        parse_header(&bad),
        Err(AsdiError::UnknownPacketType { value: 3 })
    );
    let mut bad = XML_HEADER;
    bad[20..24].copy_from_slice(&100_001_u32.to_be_bytes());
    assert_eq!(
        parse_header(&bad),
        Err(AsdiError::SequenceOutOfRange { value: 100_001 })
    );
}

// REQ: OA-ASDI-005
#[test]
fn packet_sizes_are_bounded_and_adversarial_max_does_not_wrap() {
    assert_eq!(
        inspect_packet(
            &xml_packet(),
            PayloadLimits {
                max_compressed_bytes: 6,
                max_decompressed_bytes: 256,
            }
        ),
        Err(AsdiError::CompressedPayloadTooLarge {
            value: 7,
            maximum: 6,
        })
    );
    assert_eq!(
        inspect_packet(
            &xml_packet(),
            PayloadLimits {
                max_compressed_bytes: 64,
                max_decompressed_bytes: 122,
            }
        ),
        Err(AsdiError::DecompressedPayloadTooLarge {
            value: 123,
            maximum: 122,
        })
    );
    let mut at_hard_limit = XML_HEADER;
    at_hard_limit[24..28].copy_from_slice(&MAX_COMPRESSED_PAYLOAD_BYTES.to_be_bytes());
    assert_eq!(
        inspect_packet(
            &at_hard_limit,
            PayloadLimits {
                max_compressed_bytes: MAX_COMPRESSED_PAYLOAD_BYTES,
                max_decompressed_bytes: MAX_DECOMPRESSED_PAYLOAD_BYTES,
            }
        ),
        Err(AsdiError::PacketLengthMismatch {
            expected: 1_048_608,
            actual: 32,
        })
    );
    let mut huge = XML_HEADER;
    huge[24..28].copy_from_slice(&(MAX_COMPRESSED_PAYLOAD_BYTES + 1).to_be_bytes());
    assert_eq!(
        inspect_packet(
            &huge,
            PayloadLimits {
                max_compressed_bytes: MAX_COMPRESSED_PAYLOAD_BYTES,
                max_decompressed_bytes: MAX_DECOMPRESSED_PAYLOAD_BYTES,
            }
        ),
        Err(AsdiError::CompressedPayloadTooLarge {
            value: MAX_COMPRESSED_PAYLOAD_BYTES + 1,
            maximum: MAX_COMPRESSED_PAYLOAD_BYTES,
        })
    );
    assert_eq!(
        inspect_packet(
            &xml_packet(),
            PayloadLimits {
                max_compressed_bytes: MAX_COMPRESSED_PAYLOAD_BYTES + 1,
                max_decompressed_bytes: MAX_DECOMPRESSED_PAYLOAD_BYTES,
            }
        ),
        Err(AsdiError::CompressedLimitAboveHardCeiling {
            value: MAX_COMPRESSED_PAYLOAD_BYTES + 1,
            maximum: MAX_COMPRESSED_PAYLOAD_BYTES,
        })
    );
    assert_eq!(
        inspect_packet(
            &xml_packet(),
            PayloadLimits {
                max_compressed_bytes: MAX_COMPRESSED_PAYLOAD_BYTES,
                max_decompressed_bytes: MAX_DECOMPRESSED_PAYLOAD_BYTES + 1,
            }
        ),
        Err(AsdiError::DecompressedLimitAboveHardCeiling {
            value: MAX_DECOMPRESSED_PAYLOAD_BYTES + 1,
            maximum: MAX_DECOMPRESSED_PAYLOAD_BYTES,
        })
    );
    let mut invalid_heartbeat = HEARTBEAT_HEADER;
    invalid_heartbeat[31] = 1;
    assert_eq!(
        inspect_packet(&invalid_heartbeat, LIMITS),
        Err(AsdiError::HeartbeatHasPayload)
    );
}

// REQ: OA-ASDI-005
#[test]
fn sequence_wrap_gap_and_zero_are_reported_without_restart_diagnosis() {
    let first = observe_sequence(None, 0).unwrap();
    assert_eq!(first.status, SequenceStatus::FirstObserved);
    assert_eq!(first.expected_sequence, NO_EXPECTED_SEQUENCE);
    assert_eq!(first.reserved, [0; 3]);
    assert_eq!(
        observe_sequence(Some(0), 1).unwrap().status,
        SequenceStatus::InOrder
    );
    assert_eq!(
        observe_sequence(Some(99_999), 100_000).unwrap().status,
        SequenceStatus::InOrder
    );
    let wrapped = observe_sequence(Some(100_000), 1).unwrap();
    assert_eq!(wrapped.status, SequenceStatus::InOrder);
    assert_eq!(wrapped.expected_sequence, 1);

    let gap = observe_sequence(Some(99), 101).unwrap();
    assert_eq!(gap.status, SequenceStatus::Gap);
    assert_eq!(gap.expected_sequence, 100);
    assert_eq!(gap.observed, 101);
    assert_eq!(
        observe_sequence(Some(9), 9).unwrap().status,
        SequenceStatus::Gap,
        "duplicate/unexpected ordering is a discontinuity, not a diagnosed loss"
    );

    let zero = observe_sequence(Some(9), 0).unwrap();
    assert_eq!(zero.status, SequenceStatus::ZeroObserved);
    assert_eq!(zero.expected_sequence, 10);
    assert_eq!(zero.observed, 0);
    assert_eq!(
        observe_sequence(Some(100_000), 0).unwrap().status,
        SequenceStatus::ZeroObserved
    );
    assert_eq!(
        observe_sequence(Some(100_001), 1),
        Err(AsdiError::SequenceOutOfRange { value: 100_001 })
    );
}

// REQ: OA-ASDI-006
#[test]
fn registry_marks_zlib_and_xsd_as_unimplemented() {
    for marker in [
        "FAA-ASDI-XML-HEADER-1.8",
        "partial-tested-header-only",
        "XmlCompressedNotDecoded",
        "zlib payload validation",
        "ASDI XSD and message schema",
        "TFMS-to-ASDI Interface Control Document",
        "March 2010 Release 3 Final, Revision 1",
        "section 3.2.1.3.2.2.10, page 3-11",
        "does not claim whole-ICD conformance",
    ] {
        assert!(REGISTRY.contains(marker), "registry lacks {marker}");
    }
}

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use oa_regional_session::{parse_event_line, EventKind, InputError};

const POSITION: &str = "v1\t1000000000\t900000000\t1\tSURVEILLANCE\t00000000000000000000000000000385\tABC123\tZAU1\tPOS\t410000000\t-870000000\t35000\t180000\t0\t0";

#[test]
fn accepts_full_gufi_address_and_normalized_position() {
    let event = parse_event_line(POSITION).expect("fixture row is valid");
    assert_eq!(event.identity.flight_gufi, 0x385);
    assert_eq!(event.identity.aircraft_address, 0xabc123);
    assert_eq!(event.receipt_ns, 1_000_000_000);
    assert_eq!(event.source_ns, 900_000_000);
    assert!(matches!(
        event.kind,
        EventKind::Position {
            latitude_e7: 410_000_000,
            ..
        }
    ));
}

// REQ: OA-REG-013
#[test]
fn v2_preserves_native_adsb_source_quality_without_upgrading_it_to_reliability() {
    let row = "v2\t1000000000\t900000000\t1\tSURVEILLANCE\t00000000000000000000000000000385\tABC123\tZAU1\tPOS\t410000000\t-870000000\t35000\t180000\t0\t0\t0\t0\t2\t8\t9\t4\t2\t1\t2\t1\t1\t0\t1200\t4C494E4530313233";
    let event = parse_event_line(row).expect("bounded v2 position parses");
    let EventKind::PositionV2 { evidence, .. } = event.kind else {
        panic!("v2 position retains source fields");
    };
    assert_eq!(
        evidence,
        oa_regional_session::input::AdsbSourceEvidence {
            source_type: 0,
            category: 0,
            version: 2,
            nic: 8,
            nac_p: 9,
            nac_v: 4,
            sil: 2,
            sil_type: 1,
            sda: 2,
            gva: 1,
            nic_baro: 1,
            row_flags: 0,
            squawk: 1200,
            callsign: *b"LINE0123",
        }
    );
    let unknowns = row.replace(
        "\t1\t0\t1200\t4C494E4530313233",
        "\t-\t-\t-\t0000000000000000",
    );
    let event = parse_event_line(&unknowns).expect("absent source values remain explicit");
    let EventKind::PositionV2 { evidence, .. } = event.kind else {
        panic!("unknown values remain a v2 position");
    };
    assert_eq!(evidence.nic_baro, u8::MAX);
    assert_eq!(evidence.row_flags, u8::MAX);
    assert_eq!(evidence.squawk, u16::MAX);
    assert_eq!(evidence.callsign, [0; 8]);
    assert!(matches!(
        parse_event_line(&row.replace("\t2\t8\t9", "\t2\t12\t9")),
        Err(InputError::InvalidInteger)
    ));
    assert!(matches!(
        parse_event_line(&row.replace("\t1200\t", "\t1280\t")),
        Err(InputError::InvalidInteger)
    ));
    assert!(matches!(
        parse_event_line(POSITION)
            .expect("v1 remains supported")
            .kind,
        EventKind::Position { .. }
    ));
    let serialized = oa_regional_session::recordings::serialize_recorded_events(&[event])
        .expect("v2 source evidence serializes losslessly");
    assert_eq!(serialized, format!("OA-RS\t2\n{unknowns}\n"));
}

#[test]
fn rejects_future_source_time_and_invalid_full_identity() {
    let future = POSITION.replace("900000000", "1000000001");
    assert_eq!(
        parse_event_line(&future),
        Err(InputError::SourceAfterReceipt)
    );
    let no_gufi = POSITION.replace(
        "00000000000000000000000000000385",
        "00000000000000000000000000000000",
    );
    assert_eq!(
        parse_event_line(&no_gufi)
            .expect("unbound raw surveillance")
            .identity
            .flight_gufi,
        0
    );
    assert_eq!(
        parse_event_line(&POSITION.replace("ABC123", "0")),
        Err(InputError::EmptyIdentity)
    );
    assert_eq!(
        parse_event_line(
            "v1\t1000000000\t900000000\t2\tFLIGHT\t0\t0\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2"
        ),
        Err(InputError::EmptyIdentity)
    );
    let bad_address = POSITION.replace("ABC123", "2000000");
    assert_eq!(
        parse_event_line(&bad_address),
        Err(InputError::AddressOutOfRange)
    );
}

#[test]
fn rejects_bad_coordinates_and_wide_sector_codes() {
    let bad_lat = POSITION.replace("410000000", "900000001");
    assert_eq!(
        parse_event_line(&bad_lat),
        Err(InputError::InvalidCoordinate)
    );
    let owner = "v1\t1000000000\t900000000\t2\tFLIGHT\t00000000000000000000000000000385\tABC123\tZAU1\tFLIGHT\tOWNER\tZAU123456";
    assert_eq!(parse_event_line(owner), Err(InputError::InvalidLabel));
}

// REQ: OA-REG-006
#[test]
fn rejects_event_bodies_from_the_wrong_normalized_source() {
    let owner = "v1\t1000000000\t900000000\t1\tSURVEILLANCE\t00000000000000000000000000000385\tABC123\tZAU1\tFLIGHT\tOWNER\tZAU12";
    assert_eq!(
        parse_event_line(owner),
        Err(InputError::SourceEventMismatch)
    );
}

#[test]
fn rejects_oversized_line_before_tokenizing() {
    let line = "x".repeat(769);
    assert_eq!(parse_event_line(&line), Err(InputError::RowTooLong));
}

#[test]
fn accepts_the_synthetic_protocol_sample() {
    use oa_regional_session::{
        flush_receipt_group, push_event, session_cursor_new, session_total_events,
    };

    let mut cursor = session_cursor_new();
    let sample = include_str!("../examples/synthetic-normalized.tsv");
    for line in sample.lines().skip(1) {
        let event = parse_event_line(line).expect("sample row is valid");
        let _ = push_event(&mut cursor, event).expect("sample ordering is valid");
    }
    let _ = flush_receipt_group(&mut cursor).expect("sample flush succeeds");
    assert_eq!(session_total_events(&cursor), 21);
}

// REQ: OA-REG-005
#[test]
fn filed_route_preserves_bounded_source_text_without_guessing_geometry() {
    let row = "v1\t1000000000\t900000000\t1\tROUTE\t00000000000000000000000000000385\tABC123\tZAU1\tFILED_ROUTE\tPLAN90100001\tDEST01\tFIX:FIXA:ZAU1:ENROUTE:FIX AIRWAY:V12:FIXA:FIXB PROC:KORD:ZAU1:STAR:STAR1:TRANS";
    let event = parse_event_line(row).expect("filed route row is valid");
    match event.kind {
        EventKind::FiledRoute { text_len, text, .. } => {
            assert_eq!(
                &text[..usize::from(text_len)],
                b"FIX:FIXA:ZAU1:ENROUTE:FIX AIRWAY:V12:FIXA:FIXB PROC:KORD:ZAU1:STAR:STAR1:TRANS"
            );
            assert!(text[usize::from(text_len)..].iter().all(|byte| *byte == 0));
        }
        _ => panic!("filed route remains raw text"),
    }
}

// REQ: OA-REG-005
#[test]
fn rejects_filed_route_text_over_its_bound() {
    let route = "X".repeat(257);
    let row = format!("v1\t1000000000\t900000000\t1\tROUTE\t00000000000000000000000000000385\tABC123\tZAU1\tFILED_ROUTE\tPLAN90100001\tDEST01\t{route}");
    assert_eq!(parse_event_line(&row), Err(InputError::InvalidLabel));
}

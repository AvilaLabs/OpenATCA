//! STDDS parser tests over hand-built fixtures (`tests/fixtures`). Every identifier, callsign,
//! position and time in them is made up; only the element structure follows the FAA documents
//! cited in REQUIREMENTS.md OA-STD-001.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_stdds::scalar::{parse_beacon, parse_hex24, parse_iso_ms, parse_scaled};
use oa_stdds::smes::smes_bit;
use oa_stdds::tais::tais_bit;
use oa_stdds::{
    classify, new_stats, parse_message, parse_smes, parse_tais, std_error_offset, FieldErrorKind,
    Kind, Limits, Message, SmesField, StdError, TaisField, ERROR_KIND_COUNT, ERROR_KIND_NAMES,
    KIND_COUNT, KIND_NAMES, SMES_FIELD_COUNT, SMES_FIELD_NAMES, TAIS_FIELD_COUNT, TAIS_FIELD_NAMES,
};
use oa_xml::{Limits as XmlLimits, Parser};

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn text(name: &str) -> String {
    String::from_utf8(fixture(name)).unwrap()
}

fn parser() -> Parser {
    Parser::new(XmlLimits::default()).unwrap()
}

fn kind_of(name: &str) -> Kind {
    let mut p = parser();
    let bytes = fixture(name);
    classify(&p.parse(&bytes).unwrap())
}

fn tais(xml: &[u8]) -> oa_stdds::TaisMessage {
    let mut p = parser();
    let mut st = new_stats();
    match parse_message(&mut p, xml, &Limits::default(), &mut st).unwrap() {
        Message::Tais(m) => m,
        other => panic!("not TAIS: {other:?}"),
    }
}

fn code(c: [u8; 8]) -> String {
    let n = c.iter().position(|&b| b == 0).unwrap_or(8);
    String::from_utf8(c[..n].to_vec()).unwrap()
}

fn code16(c: [u8; 16]) -> String {
    let n = c.iter().position(|&b| b == 0).unwrap_or(16);
    String::from_utf8(c[..n].to_vec()).unwrap()
}

const FULL_TS_MS: i64 = 1_893_553_445_678;

// REQ: OA-STD-001
#[test]
fn every_fixture_kind_is_classified_by_root_and_namespace() {
    let want = [
        ("tais_full.xml", Kind::TaisTrackAndFlightPlan),
        ("tais_other_root.xml", Kind::TaisOther),
        ("smes_position.xml", Kind::AsdexPositionReport),
        ("asdex_adsb.xml", Kind::AsdexAdsbReport),
        ("asdex_mlat.xml", Kind::AsdexMlatReport),
        ("smes_event.xml", Kind::SmesOther),
        ("stdds_apds.xml", Kind::Apds),
        ("stdds_rvr.xml", Kind::Rvr),
        ("stdds_other.xml", Kind::StddsOther),
        ("fixm_tais.xml", Kind::FixmTais),
        ("fixm_asdex.xml", Kind::FixmAsdex),
        ("fixm_other.xml", Kind::FixmOther),
        ("unknown_namespace.xml", Kind::Unknown),
        ("unknown_root.xml", Kind::StddsOther),
    ];
    for (f, k) in want {
        assert_eq!(kind_of(f), k, "{f}");
    }
    assert_eq!(KIND_NAMES.len(), KIND_COUNT);
    assert_eq!(oa_stdds::kind_name(Kind::FixmTais), "fixm-tais");
}

// REQ: OA-STD-002
#[test]
fn unknown_kinds_are_counted_with_their_root_and_are_not_errors() {
    let mut p = parser();
    let mut st = new_stats();
    let m = parse_message(
        &mut p,
        &fixture("unknown_namespace.xml"),
        &Limits::default(),
        &mut st,
    )
    .unwrap();
    assert!(matches!(m, Message::Other(Kind::Unknown)));
    parse_message(
        &mut p,
        &fixture("unknown_namespace.xml"),
        &Limits::default(),
        &mut st,
    )
    .unwrap();
    assert_eq!(st.messages, 2);
    assert_eq!(st.kinds[Kind::Unknown as usize], 2);
    assert_eq!(st.errors, [0; 4]);
    assert_eq!(
        st.unknown_roots,
        vec![(
            "{urn:example:not:stdds} TATrackAndFlightPlan".to_string(),
            2
        )]
    );
}

// REQ: OA-STD-003
#[test]
fn a_full_tais_message_parses_every_field_in_its_unit() {
    let m = tais(&fixture("tais_full.xml"));
    assert_eq!(code(m.src), "ZZA");
    assert!(m.src_present);
    assert_eq!(m.records.len(), 2);
    assert!(m.errors.is_empty());
    assert_eq!(m.unknown_elements, 0);
    let r = &m.records[0];
    assert_eq!(r.present, (1u64 << TAIS_FIELD_COUNT) - 1, "all fields");
    assert_eq!((r.errors, r.invalidated), (0, 0));
    assert_eq!(r.track_num, 1234);
    assert_eq!(r.mrt_time_ms, FULL_TS_MS);
    assert_eq!((r.lat_e7, r.lon_e7), (401_234_567, -747_654_321));
    assert_eq!(r.reported_alt_ft, 8500);
    assert_eq!((r.vx_cmps, r.vy_cmps), (12_345, -6_789));
    assert_eq!(r.vvert_ftmin, -86);
    assert_eq!(r.reported_beacon, 1200);
    assert_eq!(r.mode_s_addr, 0xA1_B2C3);
    assert_eq!(code(r.track_status), "active");
    assert_eq!(code(r.acid), "TST123");
    assert_eq!(code(r.ac_type), "B738");
    assert_eq!(r.assigned_beacon, 4521);
    assert_eq!((r.requested_alt_ft, r.assigned_alt_ft), (11_000, 9_000));
    assert_eq!(code(r.airport), "ZZB");
    assert_eq!(code(r.runway), "04L");
    assert_eq!(code(r.flight_rules), "IFR");
    assert_eq!(code(r.raw_flight_rules), "E");
    assert_eq!(code(r.ecid), "103");
    assert_eq!(r.sfpn, 58);
    assert_eq!(code(r.ptd_time), "1952");
    assert_eq!(code(r.entry_fix), "ENTRY");
    assert_eq!(code(r.exit_fix), "EXITF");
    assert_eq!(code(r.plan_type), "ARRIVAL");
    assert_eq!(code16(r.plan_status), "ACTIVE");
    assert_eq!(
        (r.pseudo, r.frozen, r.is_new, r.adsb),
        (false, false, true, true)
    );
    assert_eq!(code(r.rec_type), "TAIS");
    assert_eq!(code(r.rec_stars_src_id), "ZZA1");
    assert_eq!(r.rec_safa_receipt_ms, FULL_TS_MS);
    // The second record has only a track, with whole-second time and a southern latitude.
    let r2 = &m.records[1];
    assert_eq!(r2.mrt_time_ms, 1_893_553_446_000);
    assert_eq!((r2.lat_e7, r2.lon_e7), (-335_000_000, 1_512_500_000));
    assert_eq!(r2.present & tais_bit(TaisField::Acid), 0);
    assert_eq!(
        r2.offset as usize,
        text("tais_full.xml").rfind("<record>").unwrap() + 1
    );
}

// REQ: OA-STD-004
#[test]
fn each_optional_field_absent_clears_only_its_bit() {
    let full = text("tais_full.xml");
    let base = tais(full.as_bytes()).records[0];
    for (i, name) in TAIS_FIELD_NAMES.iter().enumerate() {
        let leaf = name.rsplit('/').next().unwrap();
        let open = format!("<{leaf}>");
        // The field's line inside the first record (the track fields and plan fields have
        // distinct leaf names except `status`, which is in both: take the one in its parent).
        let parent = name.split('/').next().unwrap();
        let from = full.find(&format!("<{parent}>")).unwrap();
        let at = from + full[from..].find(&open).unwrap();
        let start = full[..at].rfind('\n').unwrap();
        let end = at + full[at..].find('\n').unwrap();
        let mut cut = String::new();
        cut.push_str(&full[..start]);
        cut.push_str(&full[end..]);
        let m = tais(cut.as_bytes());
        let r = &m.records[0];
        if leaf == "lat" || leaf == "lon" {
            // A position is a pair: the half left behind is refused, both bits go.
            let both = tais_bit(TaisField::Lat) | tais_bit(TaisField::Lon);
            assert_eq!(m.errors.len(), 1, "{name}");
            assert_eq!(m.errors[0].kind, FieldErrorKind::PairIncomplete, "{name}");
            assert_eq!(r.present, base.present & !both, "{name}");
            assert_eq!(r.errors, both & !(1u64 << i), "{name}");
            continue;
        }
        assert!(m.errors.is_empty(), "{name}: {:?}", m.errors);
        assert_eq!(r.present, base.present & !(1u64 << i), "{name}");
        assert_eq!(r.errors, 0, "{name}");
    }
}

// REQ: OA-STD-005
#[test]
fn a_malformed_field_is_a_named_error_with_its_offset_and_the_record_is_kept() {
    let xml = text("tais_malformed_fields.xml");
    let m = tais(xml.as_bytes());
    assert_eq!(m.records.len(), 1);
    let r = &m.records[0];
    let expect = [
        ("track/trackNum", FieldErrorKind::Syntax, "<trackNum>"),
        ("track/mrtTime", FieldErrorKind::Syntax, "<mrtTime>"),
        ("track/lat", FieldErrorKind::Range, "<lat>"),
        (
            "track/reportedAltitude",
            FieldErrorKind::Range,
            "<reportedAltitude>",
        ),
        ("track/vx", FieldErrorKind::Syntax, "<vx>"),
        (
            "track/reportedBeaconCode",
            FieldErrorKind::Syntax,
            "<reportedBeaconCode>",
        ),
        ("track/acAddress", FieldErrorKind::Syntax, "<acAddress>"),
        ("flightPlan/acid", FieldErrorKind::Empty, "<acid>"),
        ("flightPlan/acType", FieldErrorKind::TooLong, "<acType>"),
    ];
    assert_eq!(m.errors.len(), expect.len(), "{:?}", m.errors);
    for (e, (field, kind, tag)) in m.errors.iter().zip(expect) {
        assert_eq!(e.field, field);
        assert_eq!(e.kind, kind, "{field}");
        assert_eq!(e.offset as usize, xml.find(tag).unwrap() + 1, "{field}");
        let i = TAIS_FIELD_NAMES.iter().position(|n| *n == field).unwrap();
        assert_ne!(r.errors & (1 << i), 0, "{field}");
        assert_eq!(r.present & (1 << i), 0, "{field}");
    }
    // The fields that were fine are kept.
    assert_eq!(r.lon_e7, -745_000_000);
    assert_eq!(r.vy_cmps, 1_050);
    assert_eq!(
        r.present,
        tais_bit(TaisField::Lon) | tais_bit(TaisField::Vy)
    );
    assert!(format!("{}", m.errors[2]).contains("track/lat out of range at byte"));
    assert_eq!(ERROR_KIND_NAMES.len(), ERROR_KIND_COUNT);
}

// REQ: OA-STD-006
#[test]
fn a_field_given_twice_is_an_error_and_keeps_neither_value() {
    let xml = text("tais_duplicate_field.xml");
    let m = tais(xml.as_bytes());
    let r = &m.records[0];
    assert_eq!(m.errors.len(), 1);
    assert_eq!(m.errors[0].kind, FieldErrorKind::Duplicate);
    assert_eq!(m.errors[0].field, "track/trackNum");
    let second = xml.rfind("<trackNum>").unwrap() + 1;
    assert_eq!(m.errors[0].offset as usize, second);
    assert_eq!(r.present & tais_bit(TaisField::TrackNum), 0);
    assert_ne!(r.errors & tais_bit(TaisField::TrackNum), 0);
    assert_eq!(r.reported_alt_ft, 5000);
}

// REQ: OA-STD-007
#[test]
fn half_a_position_pair_is_refused_in_both_products() {
    let m = tais(&fixture("tais_lat_only.xml"));
    let r = &m.records[0];
    assert_eq!(m.errors.len(), 1);
    assert_eq!(m.errors[0].kind, FieldErrorKind::PairIncomplete);
    assert_eq!(m.errors[0].field, "track/lat");
    assert_eq!(r.present, tais_bit(TaisField::TrackNum));
    assert_ne!(r.errors & tais_bit(TaisField::Lat), 0);

    let mut p = parser();
    let mut st = new_stats();
    let Message::Smes(s) = parse_message(
        &mut p,
        &fixture("smes_delta_lat_only.xml"),
        &Limits::default(),
        &mut st,
    )
    .unwrap() else {
        panic!("not SMES");
    };
    assert_eq!(s.errors.len(), 1);
    assert_eq!(s.errors[0].kind, FieldErrorKind::PairIncomplete);
    assert_eq!(s.errors[0].field, "position/latitude");
    assert_eq!(
        s.records[0].present,
        smes_bit(SmesField::SeqNum),
        "the sequence number survives"
    );
}

// REQ: OA-STD-008
#[test]
fn values_are_range_and_syntax_checked_exactly() {
    assert_eq!(parse_beacon(b"1200"), Some(1200));
    assert_eq!(parse_beacon(b"0007"), Some(7));
    assert_eq!(parse_beacon(b"1289"), None);
    assert_eq!(parse_beacon(b"120"), None);
    assert_eq!(parse_hex24(b"A1b2C3"), Some(0xA1_B2C3));
    assert_eq!(parse_hex24(b"1"), Some(1));
    assert_eq!(parse_hex24(b""), None);
    assert_eq!(parse_hex24(b"1234567"), None);
    assert_eq!(parse_hex24(b"12G456"), None);
    // Scaled decimals round half away from zero and take no sign, exponent or spaces.
    assert_eq!(parse_scaled(b"40.12345675", 7), Some(401_234_568));
    assert_eq!(parse_scaled(b"-40.12345675", 7), Some(-401_234_568));
    assert_eq!(parse_scaled(b"12", 2), Some(1200));
    assert_eq!(parse_scaled(b"1e3", 2), None);
    assert_eq!(parse_scaled(b"+1", 2), None);
    assert_eq!(parse_scaled(b" 1", 2), None);
    assert_eq!(parse_scaled(b"1.", 2), None);
    assert_eq!(parse_scaled(b".5", 2), None);
    // Bounds, through the parser: the edges are accepted, one past is not.
    let one = |body: &str| {
        let xml = format!(
            r#"<a:TATrackAndFlightPlan xmlns:a="urn:us:gov:dot:faa:atm:terminal:entities:v4-0:tais:terminalautomationinformation"><record><track>{body}</track></record></a:TATrackAndFlightPlan>"#
        );
        tais(xml.as_bytes())
    };
    let ok = one("<lat>90</lat><lon>-180</lon><reportedAltitude>100000</reportedAltitude><vx>3000</vx><vy>-3000</vy><vVert>30000</vVert>");
    assert!(ok.errors.is_empty(), "{:?}", ok.errors);
    assert_eq!(ok.records[0].lat_e7, 900_000_000);
    assert_eq!(ok.records[0].lon_e7, -1_800_000_000);
    assert_eq!(
        (ok.records[0].vx_cmps, ok.records[0].vy_cmps),
        (300_000, -300_000)
    );
    let bad = one("<lat>90.0000001</lat><lon>180.0000001</lon><reportedAltitude>100001</reportedAltitude><vx>3000.01</vx><vy>-3000.01</vy><vVert>-30001</vVert>");
    assert_eq!(bad.errors.len(), 6);
    assert!(bad.errors.iter().all(|e| e.kind == FieldErrorKind::Range));
    let alt = one("<reportedAltitude>-2000</reportedAltitude>");
    assert_eq!(alt.records[0].reported_alt_ft, -2000);
    assert_eq!(
        one("<reportedAltitude>-2001</reportedAltitude>")
            .errors
            .len(),
        1
    );
}

// REQ: OA-STD-009
#[test]
fn timestamps_are_utc_real_dates_and_nothing_else() {
    assert_eq!(parse_iso_ms(b"2030-01-02T03:04:05.678Z"), Some(FULL_TS_MS));
    assert_eq!(
        parse_iso_ms(b"2030-01-02T03:04:05.678+00:00"),
        Some(FULL_TS_MS)
    );
    assert_eq!(
        parse_iso_ms(b"2030-01-02T03:04:05.6789999Z"),
        Some(FULL_TS_MS + 1),
        "a fraction past a millisecond is rounded half up, not truncated"
    );
    assert_eq!(parse_iso_ms(b"2030-01-02T03:04:05+01:00"), None);
    assert_eq!(parse_iso_ms(b"2030-01-02T03:04:05"), None);
    assert_eq!(parse_iso_ms(b"2030-01-02 03:04:05Z"), None);
    assert_eq!(parse_iso_ms(b"2030-01-02T03:04:60Z"), None);
    assert_eq!(parse_iso_ms(b"2030-02-30T03:04:05Z"), None);
    assert_eq!(parse_iso_ms(b"2030-01-02T03:04:05.Z"), None);
    assert_eq!(parse_iso_ms(b""), None);
}

// REQ: OA-STD-010
#[test]
fn a_field_the_sender_marks_invalid_is_absent_and_counted_not_an_error() {
    let mut p = parser();
    let mut st = new_stats();
    let Message::Tais(m) = parse_message(
        &mut p,
        &fixture("tais_invalidated.xml"),
        &Limits::default(),
        &mut st,
    )
    .unwrap() else {
        panic!("not TAIS");
    };
    let r = &m.records[0];
    assert!(m.errors.is_empty());
    assert_eq!(r.errors, 0);
    assert_eq!(r.invalidated, tais_bit(TaisField::ReportedAltitude));
    assert_eq!(r.present & tais_bit(TaisField::ReportedAltitude), 0);
    assert_eq!(st.invalidated, 1);
    assert_eq!(st.records_with_errors, 0);
}

// REQ: OA-STD-011
#[test]
fn record_and_error_limits_are_stated_and_reached_visibly() {
    let one = "<record><track><trackNum>1</trackNum></track></record>";
    let head = r#"<a:TATrackAndFlightPlan xmlns:a="urn:us:gov:dot:faa:atm:terminal:entities:v4-0:tais:terminalautomationinformation">"#;
    let xml = format!("{head}{}</a:TATrackAndFlightPlan>", one.repeat(5));
    let mut p = parser();
    let mut st = new_stats();
    let lim = Limits {
        max_records: 4,
        ..Limits::default()
    };
    let e = parse_message(&mut p, xml.as_bytes(), &lim, &mut st).unwrap_err();
    let fifth = head.len() + 4 * one.len() + 1;
    assert_eq!(
        e,
        StdError::TooManyRecords {
            limit: 4,
            offset: fifth as u32
        }
    );
    assert_eq!(std_error_offset(&e), Some(fifth as u32));
    assert_eq!(st.errors[2], 1);
    // The records read before the refusal are counted (the refusal is in `errors`, not `messages`).
    assert_eq!(st.records, 4);
    assert_eq!(st.messages, 1);
    // Exactly at the limit is fine.
    let xml4 = format!("{head}{}</a:TATrackAndFlightPlan>", one.repeat(4));
    assert!(parse_message(&mut p, xml4.as_bytes(), &lim, &mut st).is_ok());
    // Field errors past the limit are counted, not kept.
    let bad = "<record><track><lat>x</lat><lon>x</lon><vx>x</vx></track></record>";
    let xml = format!("{head}{}</a:TATrackAndFlightPlan>", bad.repeat(2));
    let lim = Limits {
        max_errors: 4,
        ..Limits::default()
    };
    let Message::Tais(m) = parse_message(&mut p, xml.as_bytes(), &lim, &mut st).unwrap() else {
        panic!("not TAIS");
    };
    assert_eq!(m.errors.len(), 4);
    assert_eq!(m.errors_dropped, 2);
    assert_eq!(m.records.len(), 2);
    assert_eq!(m.records[1].errors.count_ones(), 3, "bits survive the cap");
}

// REQ: OA-STD-012
#[test]
fn malformed_xml_is_refused_with_an_offset_and_no_input_panics() {
    let mut p = parser();
    let mut st = new_stats();
    for f in ["tais_truncated.xml", "tais_doctype.xml"] {
        let e = parse_message(&mut p, &fixture(f), &Limits::default(), &mut st).unwrap_err();
        assert!(matches!(e, StdError::Xml(_)), "{f}: {e:?}");
        assert!(std_error_offset(&e).is_some());
        assert!(format!("{e}").contains("byte"));
    }
    assert_eq!(st.errors[0], 2);
    assert_eq!(st.messages, 2);
    // Every truncation and every single-byte deletion of every fixture is a result, not a panic.
    let mut names: Vec<_> =
        std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
    names.sort();
    let mut results = 0usize;
    for n in &names {
        let b = fixture(n);
        for cut in 0..b.len() {
            let _ = parse_message(&mut p, &b[..cut], &Limits::default(), &mut st);
            let mut d = b.clone();
            d.remove(cut);
            let _ = parse_message(&mut p, &d, &Limits::default(), &mut st);
            results += 2;
        }
    }
    assert!(results > 10_000, "{results}");
}

// REQ: OA-STD-013
#[test]
fn unknown_elements_are_counted_per_path_topmost_only_and_the_table_is_bounded() {
    let mut p = parser();
    let mut st = new_stats();
    let Message::Tais(m) = parse_message(
        &mut p,
        &fixture("tais_unknown_elements.xml"),
        &Limits::default(),
        &mut st,
    )
    .unwrap() else {
        panic!("not TAIS");
    };
    assert!(m.errors.is_empty());
    // header (1), track/xPos (2), track/yPos (1), track/future (1, its child is not counted),
    // flightPlan/newField (1).
    assert_eq!(m.unknown_elements, 6);
    let got: Vec<(&str, u64)> = st
        .unknown
        .iter()
        .map(|u| (u.text.as_str(), u.count))
        .collect();
    assert_eq!(
        got,
        vec![
            ("header", 1),
            ("track/xPos", 2),
            ("track/yPos", 1),
            ("track/future", 1),
            ("flightPlan/newField", 1)
        ]
    );
    // A table of two keeps the first two paths and counts the rest as overflow.
    let mut st = new_stats();
    let lim = Limits {
        max_unknown_paths: 2,
        ..Limits::default()
    };
    parse_message(&mut p, &fixture("tais_unknown_elements.xml"), &lim, &mut st).unwrap();
    assert_eq!(st.unknown.len(), 2);
    assert_eq!(st.unknown_overflow, 3, "yPos, future and newField");
}

// REQ: OA-STD-014
#[test]
fn an_asdex_position_report_parses_every_field_in_its_unit() {
    let mut p = parser();
    let mut st = new_stats();
    let Message::Smes(m) = parse_message(
        &mut p,
        &fixture("smes_position.xml"),
        &Limits::default(),
        &mut st,
    )
    .unwrap() else {
        panic!("not SMES");
    };
    assert_eq!(code(m.airport), "ZZB");
    assert!(m.airport_present && m.errors.is_empty());
    assert_eq!(m.records.len(), 1);
    let r = &m.records[0];
    assert_eq!(r.present, (1u64 << SMES_FIELD_COUNT) - 1);
    assert_eq!(r.time_ms, 1_893_553_445_123);
    assert_eq!((r.lat_e7, r.lon_e7), (406_398_000, -737_789_000));
    assert_eq!(r.altitude_ft, 0);
    assert_eq!(r.heading_cdeg, 27_050);
    assert_eq!(r.speed_cmps, 725);
    assert_eq!((r.vx_cmps, r.vy_cmps), (-720, 50));
    assert_eq!(r.vvert_ftmin, 0);
    assert_eq!((r.seq_num, r.track_num), (1001, 321));
    assert_eq!(r.mode_s_addr, 0xAB_C123);
    assert_eq!(r.mode3a, 1234);
    assert_eq!(code(r.aircraft_id), "TST9");
    assert_eq!(code(r.ac_type), "A320");
    assert_eq!(code(r.runway), "04L");
    assert_eq!(code(r.tgt_type), "aircraft");
    // Known-but-unread elements (fix, ax, sil, enhancedData) are not unknown.
    assert_eq!(m.unknown_elements, 0);
    assert_eq!(SMES_FIELD_NAMES.len(), SMES_FIELD_COUNT);
    assert_eq!(st.smes_present, [1; SMES_FIELD_COUNT]);
}

// REQ: OA-STD-015
#[test]
fn products_that_are_classified_only_return_their_kind_and_count() {
    let mut p = parser();
    let mut st = new_stats();
    let only = [
        ("asdex_adsb.xml", Kind::AsdexAdsbReport),
        ("asdex_mlat.xml", Kind::AsdexMlatReport),
        ("fixm_tais.xml", Kind::FixmTais),
        ("fixm_asdex.xml", Kind::FixmAsdex),
        ("stdds_apds.xml", Kind::Apds),
        ("smes_event.xml", Kind::SmesOther),
    ];
    for (f, k) in only {
        match parse_message(&mut p, &fixture(f), &Limits::default(), &mut st).unwrap() {
            Message::Other(got) => assert_eq!(got, k, "{f}"),
            m => panic!("{f}: parsed as {m:?}"),
        }
    }
    assert_eq!(st.messages, 6);
    assert_eq!(st.kinds.iter().sum::<u64>(), 6);
    assert_eq!(st.records, 0);
}

// REQ: OA-STD-016
#[test]
fn stats_count_kinds_errors_and_per_field_presence_and_nothing_else() {
    let mut p = parser();
    let mut st = new_stats();
    let lim = Limits::default();
    for f in [
        "tais_full.xml",
        "tais_malformed_fields.xml",
        "tais_empty_record.xml",
        "tais_truncated.xml",
        "smes_position.xml",
    ] {
        let _ = parse_message(&mut p, &fixture(f), &lim, &mut st);
    }
    assert_eq!(st.messages, 5);
    assert_eq!(st.kinds[Kind::TaisTrackAndFlightPlan as usize], 3);
    assert_eq!(st.kinds[Kind::AsdexPositionReport as usize], 1);
    assert_eq!(st.errors, [1, 0, 0, 0]);
    // 2 + 1 + 1 TAIS records and 1 SMES report.
    assert_eq!(st.records, 5);
    assert_eq!(st.records_with_errors, 1);
    assert_eq!(st.empty_records, 1);
    // trackNum: present in both full records, refused once in the malformed one.
    assert_eq!(st.tais_present[TaisField::TrackNum as usize], 2);
    assert_eq!(st.tais_errors[TaisField::TrackNum as usize], 1);
    assert_eq!(st.tais_present[TaisField::Acid as usize], 1);
    assert_eq!(st.tais_present[TaisField::Lon as usize], 3);
    assert_eq!(st.field_error_kinds[FieldErrorKind::Syntax as usize], 5);
    assert_eq!(st.field_error_kinds[FieldErrorKind::Range as usize], 2);
    assert_eq!(st.field_error_kinds[FieldErrorKind::Empty as usize], 1);
    assert_eq!(st.field_error_kinds[FieldErrorKind::TooLong as usize], 1);
    assert_eq!(st.smes_present[SmesField::Latitude as usize], 1);
}

// REQ: OA-STD-017
#[test]
fn a_typed_parser_refuses_a_message_of_another_kind() {
    let mut p = parser();
    let mut st = new_stats();
    let bytes = fixture("smes_position.xml");
    let d = p.parse(&bytes).unwrap();
    let e = parse_tais(&d, &Limits::default(), &mut st).unwrap_err();
    assert_eq!(
        e,
        StdError::WrongKind {
            kind: Kind::AsdexPositionReport
        }
    );
    assert_eq!(std_error_offset(&e), None);
    let bytes = fixture("tais_full.xml");
    let d = p.parse(&bytes).unwrap();
    let e = parse_smes(&d, &Limits::default(), &mut st).unwrap_err();
    assert!(matches!(
        e,
        StdError::WrongKind {
            kind: Kind::TaisTrackAndFlightPlan
        }
    ));
    assert_eq!(st.errors[1], 2);
    assert!(format!("{e}").contains("tais-track-and-flight-plan"));
}

// REQ: OA-STD-018
#[test]
fn parsing_is_deterministic_and_a_parser_and_stats_are_reusable() {
    let mut p = parser();
    let lim = Limits::default();
    let mut a = new_stats();
    let mut b = new_stats();
    let first = parse_message(&mut p, &fixture("tais_full.xml"), &lim, &mut a).unwrap();
    // Other messages in between, good and bad, change nothing about the next parse.
    for f in [
        "tais_truncated.xml",
        "smes_position.xml",
        "tais_malformed_fields.xml",
    ] {
        let _ = parse_message(&mut p, &fixture(f), &lim, &mut b);
    }
    let again = parse_message(&mut p, &fixture("tais_full.xml"), &lim, &mut b).unwrap();
    let (Message::Tais(x), Message::Tais(y)) = (first, again) else {
        panic!("not TAIS");
    };
    assert_eq!(x.records, y.records);
    assert_eq!(x.src, y.src);
    assert_eq!(x.errors, y.errors);
}

// REQ: OA-STD-019
#[test]
fn a_record_with_no_known_field_is_kept_and_counted_empty() {
    let m = tais(&fixture("tais_empty_record.xml"));
    assert_eq!(m.records.len(), 1);
    let r = &m.records[0];
    assert_eq!((r.present, r.errors, r.invalidated), (0, 0, 0));
    assert!(m.errors.is_empty());
}

// REQ: OA-STD-020
#[test]
fn text_is_trimmed_and_references_and_cdata_are_decoded_before_parsing() {
    let m = tais(&fixture("tais_text_forms.xml"));
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    let r = &m.records[0];
    assert_eq!(r.track_num, 42);
    assert_eq!(r.lat_e7, 405_000_000);
    assert_eq!(r.lon_e7, 102_500_000);
    assert_eq!(r.reported_beacon, 7);
}

// REQ: OA-STD-022
#[test]
fn the_namespace_version_is_never_compared_but_its_shape_is() {
    let wrap = |ns: &str| {
        format!(r#"<a:TATrackAndFlightPlan xmlns:a="{ns}"><record/></a:TATrackAndFlightPlan>"#)
    };
    let kind = |ns: &str| {
        let mut p = parser();
        let b = wrap(ns).into_bytes();
        classify(&p.parse(&b).unwrap())
    };
    let base = "urn:us:gov:dot:faa:atm:terminal:entities";
    for v in ["v3-0", "v4-0", "v12-34", "v99-0"] {
        assert_eq!(
            kind(&format!("{base}:{v}:tais:terminalautomationinformation")),
            Kind::TaisTrackAndFlightPlan,
            "{v}"
        );
    }
    for bad in [
        format!("{base}:x4-0:tais:terminalautomationinformation"),
        format!("{base}:v4:tais:terminalautomationinformation"),
        format!("{base}:v4-0:tais"),
        format!("{base}:v4-0:tais:terminalautomationinformation:extra"),
        format!("{base}:v4-0::terminalautomationinformation"),
        "urn:us:gov:dot:faa:atm:terminal:entities:v4-0".to_string(),
        "urn:example:x".to_string(),
    ] {
        assert_eq!(kind(&bad), Kind::Unknown, "{bad}");
    }
    // The native root also needs the TAIS name: another root in the TAIS namespace is TaisOther.
    assert_eq!(kind_of("tais_other_root.xml"), Kind::TaisOther);
}

// REQ: OA-STD-023
#[test]
fn messages_framed_in_a_feed_parse_to_the_same_counts_as_direct_parsing() {
    use oa_feed::frame::{encode_frame, Kind as FrameKind, MAX_FRAME_LEN};
    use oa_feed::reader::FrameReader;
    let names = [
        "tais_full.xml",
        "smes_position.xml",
        "unknown_namespace.xml",
        "fixm_tais.xml",
        "tais_truncated.xml",
    ];
    let mut stream = Vec::new();
    let mut out = vec![0u8; MAX_FRAME_LEN];
    for (i, n) in names.iter().enumerate() {
        let len = encode_frame(&mut out, 1, 0, i as u64, 0, 0, &fixture(n)).unwrap();
        stream.extend_from_slice(&out[..len]);
    }
    // Optionally leave the synthetic feed on disk to smoke-test the summary example.
    if let Ok(path) = std::env::var("OA_STDDS_SYNTH_OUT") {
        std::fs::write(path, &stream).unwrap();
    }
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut reader = FrameReader::new(&mut storage).unwrap();
    assert_eq!(reader.push(&stream), stream.len());
    let (mut p, mut framed, mut direct) = (parser(), new_stats(), new_stats());
    let mut n = 0;
    while let Some(f) = reader.next() {
        let f = f.unwrap();
        assert_eq!(f.kind, FrameKind::Message);
        let _ = parse_message(&mut p, f.payload, &Limits::default(), &mut framed);
        n += 1;
    }
    assert_eq!(n, names.len());
    for f in names {
        let _ = parse_message(&mut p, &fixture(f), &Limits::default(), &mut direct);
    }
    assert_eq!(framed.kinds, direct.kinds);
    assert_eq!(framed.errors, direct.errors);
    assert_eq!(framed.tais_present, direct.tais_present);
    assert_eq!(framed.messages, 5);
}

// REQ: OA-STD-024
#[test]
fn the_summary_example_refuses_sealed_folders_as_given_and_through_links() {
    use std::os::unix::fs::symlink;
    use std::process::Command;
    let exe = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/stdds_summary");
    if !exe.exists() {
        panic!("example binary not built: {}", exe.display());
    }
    let run = |p: &std::path::Path| Command::new(&exe).arg(p).output().unwrap();
    let root = std::env::temp_dir().join(format!("oa-stdds-seal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for name in [
        "rec-20261119/day2",
        "stdds-20261119-holdout",
        "stdds-smoke-20261119",
    ] {
        let dir = root.join("scds").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.oaf");
        std::fs::write(&file, b"").unwrap();
        let name = name.replace('/', "-");
        // As given.
        let o = run(&file);
        assert_eq!(o.status.code(), Some(2), "{name}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
        // A link to the file, and a link to the folder, from outside `scds`.
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let l1 = outside.join(format!("file-{name}"));
        symlink(&file, &l1).unwrap();
        assert_eq!(run(&l1).status.code(), Some(2), "file link {name}");
        let l2 = outside.join(format!("dir-{name}"));
        symlink(&dir, &l2).unwrap();
        assert_eq!(
            run(&l2.join("a.oaf")).status.code(),
            Some(2),
            "dir link {name}"
        );
        // A link to a link.
        let l3 = outside.join(format!("chain-{name}"));
        symlink(&l2, &l3).unwrap();
        assert_eq!(
            run(&l3.join("a.oaf")).status.code(),
            Some(2),
            "chain {name}"
        );
    }
    // A similar but different folder is not refused (an empty file reads as an empty feed).
    let ok = root.join("scds").join("rec-20261118");
    std::fs::create_dir_all(&ok).unwrap();
    std::fs::write(ok.join("a.oaf"), b"").unwrap();
    assert_eq!(run(&ok.join("a.oaf")).status.code(), Some(0));
    // A path that cannot be opened exits 2 as well, naming the path.
    let o = run(&ok.join("none.oaf"));
    assert_eq!(o.status.code(), Some(2), "fail closed");
    assert!(String::from_utf8_lossy(&o.stderr).contains("unreadable"));
    std::fs::remove_dir_all(&root).unwrap();
}

const TAIS_NS: &str =
    "urn:us:gov:dot:faa:atm:terminal:entities:v4-0:tais:terminalautomationinformation";
const SMES_NS: &str = "urn:us:gov:dot:faa:atm:terminal:entities:v4-0:smes:surfacemovementevent";
const STATUS_NS: &str = "urn:us:gov:dot:faa:atm:terminal:entities:v4-0:status";

/// A TAIS message with one record holding `body`.
fn tais_rec(body: &str) -> oa_stdds::TaisMessage {
    let xml = format!(
        r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}"><record>{body}</record></a:TATrackAndFlightPlan>"#
    );
    tais(xml.as_bytes())
}

/// An SMES message with one report holding `body`.
fn smes_rec(body: &str) -> oa_stdds::SmesMessage {
    let xml = format!(
        r#"<a:asdexMsg xmlns:a="{SMES_NS}"><positionReport>{body}</positionReport></a:asdexMsg>"#
    );
    let mut p = parser();
    let mut st = new_stats();
    match parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap() {
        Message::Smes(m) => m,
        other => panic!("not SMES: {other:?}"),
    }
}

// REQ: OA-STD-025
#[test]
fn forms_the_real_feed_uses_are_accepted_and_damaged_text_is_still_refused() {
    // A heading printed as a binary double (13 fraction digits) is a heading.
    let m = smes_rec("<movement><heading>123.4567890123456</heading></movement>");
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(m.records[0].heading_cdeg, 12_346);
    // Still bounded: a heading past a circle is out of range.
    let m = smes_rec("<movement><heading>360.5000000000000</heading></movement>");
    assert_eq!(m.errors[0].kind, FieldErrorKind::Range);
    // A callsign with an inner space; a runway with `*` and `+`.
    let m = smes_rec("<flightId><aircraftId>TST 12</aircraftId></flightId>");
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(code(m.records[0].aircraft_id), "TST 12");
    let m = tais_rec("<flightPlan><runway>2*L</runway><status>terminated</status></flightPlan>");
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(code(m.records[0].runway), "2*L");
    assert_eq!(code16(m.records[0].plan_status), "terminated");
    let m = tais_rec("<flightPlan><runway>2+</runway></flightPlan>");
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    // The word a sender prints for no value is absent, not an error.
    let m = tais_rec(
        "<flightPlan><entryFix>unassigned</entryFix><exitFix>unassigned</exitFix></flightPlan>",
    );
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    let r = &m.records[0];
    assert_eq!(r.present, 0);
    assert_eq!(
        r.present_empty,
        tais_bit(TaisField::EntryFix) | tais_bit(TaisField::ExitFix)
    );
    let m = smes_rec("<status><vertRate>unavailable</vertRate></status>");
    assert!(m.errors.is_empty());
    assert_eq!(m.records[0].present_empty, smes_bit(SmesField::VertRate));
    // A word is not a number, and an encoding-damaged runway (U+FFFD) is refused.
    let m = smes_rec("<status><vertRate>unknown</vertRate></status>");
    assert_eq!(m.errors[0].kind, FieldErrorKind::Syntax);
    let m = tais_rec("<flightPlan><runway>\u{fffd}</runway></flightPlan>");
    assert_eq!(m.errors[0].kind, FieldErrorKind::Syntax);
    // A fix that is simply too long is still too long.
    let m = tais_rec("<flightPlan><entryFix>unassigned1</entryFix></flightPlan>");
    assert_eq!(m.errors[0].kind, FieldErrorKind::TooLong);
}

// REQ: OA-STD-026
#[test]
fn an_empty_raw_flight_rules_element_is_present_empty_and_other_empty_fields_stay_errors() {
    let m = tais_rec("<flightPlan><rawFlightRules/><acid></acid></flightPlan>");
    let r = &m.records[0];
    assert_eq!(r.present_empty, tais_bit(TaisField::RawFlightRules));
    assert_eq!(r.errors, tais_bit(TaisField::Acid));
    assert_eq!(m.errors.len(), 1);
    assert_eq!(m.errors[0].kind, FieldErrorKind::Empty);
    let mut p = parser();
    let mut st = new_stats();
    let xml = format!(
        r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}"><record><flightPlan><rawFlightRules> </rawFlightRules></flightPlan></record></a:TATrackAndFlightPlan>"#
    );
    parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    assert_eq!(st.tais_present_empty[TaisField::RawFlightRules as usize], 1);
    assert_eq!(st.tais_errors[TaisField::RawFlightRules as usize], 0);
    assert_eq!(st.field_error_kinds[FieldErrorKind::Empty as usize], 0);
    // A record with only a present-empty field is not an empty record.
    assert_eq!(st.empty_records, 0);
}

// REQ: OA-STD-043
#[test]
fn track_coast_decodes_true_false_absent_and_unreadable() {
    let t = tais_rec("<track><coast>true</coast></track>");
    assert!(
        t.errors.is_empty() && t.unknown_elements == 0,
        "{:?}",
        t.errors
    );
    assert!(t.records[0].coast);
    assert_ne!(
        t.records[0].present & (1 << TaisField::TrackCoast as u32),
        0
    );
    let f = tais_rec("<track><coast>0</coast></track>");
    assert!(!f.records[0].coast);
    assert_ne!(
        f.records[0].present & (1 << TaisField::TrackCoast as u32),
        0
    );
    // Absent stays absent: not present, and not false-counted.
    let a = tais_rec("<track><adsb>true</adsb></track>");
    assert!(!a.records[0].coast);
    assert_eq!(
        a.records[0].present & (1 << TaisField::TrackCoast as u32),
        0
    );
    // Present but unreadable is a counted field error, and the field is absent.
    let bad = tais_rec("<track><coast>maybe</coast></track>");
    assert_eq!(bad.errors[0].field, "track/coast");
    assert_eq!(bad.errors[0].kind, FieldErrorKind::Syntax);
    assert_eq!(
        bad.records[0].present & (1 << TaisField::TrackCoast as u32),
        0
    );
    // The stats keep the presence count and the value histogram.
    let mut p = parser();
    let mut st = new_stats();
    for (v, n) in [("true", 2), ("false", 1), ("x", 1)] {
        for _ in 0..n {
            let xml = format!(
                r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}"><record><track><coast>{v}</coast></track></record></a:TATrackAndFlightPlan>"#
            );
            parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
        }
    }
    assert_eq!(st.tais_present[TaisField::TrackCoast as usize], 3);
    assert_eq!(st.tais_errors[TaisField::TrackCoast as usize], 1);
    assert_eq!(st.tais_flags[4], [1, 2]);
}

// REQ: OA-STD-027
#[test]
fn track_flags_decode_and_the_stats_keep_a_histogram_of_them() {
    let m = tais_rec(
        "<track><pseudo>false</pseudo><frozen>1</frozen><new>true</new><adsb>false</adsb></track>",
    );
    let r = &m.records[0];
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(
        (r.pseudo, r.frozen, r.is_new, r.adsb),
        (false, true, true, false)
    );
    assert_eq!(m.unknown_elements, 0);
    let bad = tais_rec("<track><adsb>maybe</adsb></track>");
    assert_eq!(bad.errors[0].field, "track/adsb");
    assert_eq!(bad.errors[0].kind, FieldErrorKind::Syntax);
    assert_eq!(bad.records[0].present, 0);
    let mut p = parser();
    let mut st = new_stats();
    for (a, n) in [("true", 3), ("false", 1)] {
        let xml = format!(
            r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}"><record><track><adsb>{a}</adsb></track></record></a:TATrackAndFlightPlan>"#
        );
        for _ in 0..n {
            parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
        }
    }
    // [false, true] per flag; only adsb was given.
    assert_eq!(st.tais_flags[3], [1, 3]);
    assert_eq!(st.tais_flags[0], [0, 0]);
}

// REQ: OA-STD-028
#[test]
fn the_record_header_decodes_and_latency_and_distinct_sources_are_counted() {
    let hdr = |src: &str, stars: &str, safa: &str| {
        format!(
            "<recType>TAIS</recType><recSrc>ZZ1</recSrc><recSTARSSrcID>{src}</recSTARSSrcID><recSTARSTimestamp>{stars}</recSTARSTimestamp><recSAFAReceiptTime>{safa}</recSAFAReceiptTime>"
        )
    };
    // 03:04:05.678 is 11_045_678 ms into the day.
    let m = tais_rec(&hdr("ZZA1", "11045600", "2030-01-02T03:04:05.678Z"));
    let r = &m.records[0];
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(code(r.rec_type), "TAIS");
    assert_eq!(code(r.rec_src), "ZZ1");
    assert_eq!(code(r.rec_stars_src_id), "ZZA1");
    assert_eq!(r.rec_stars_tod_ms, 11_045_600);
    assert_eq!(r.rec_safa_receipt_ms, FULL_TS_MS);
    let mut p = parser();
    let mut st = new_stats();
    // Latencies 78 ms, 5 ms, -22 ms, 20 ms.
    for (src, stars, safa) in [
        ("ZZA1", "11045600", "2030-01-02T03:04:05.678Z"),
        ("ZZA1", "11045673", "2030-01-02T03:04:05.678Z"),
        ("ZZA2", "11045700", "2030-01-02T03:04:05.678Z"),
        ("ZZA2", "11045658", "2030-01-02T03:04:05.678Z"),
    ] {
        let xml = format!(
            r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}"><record>{}</record></a:TATrackAndFlightPlan>"#,
            hdr(src, stars, safa)
        );
        parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    }
    assert_eq!(st.src_ids.len(), 2);
    assert_eq!(st.latency[1], 1, "0-9 ms");
    assert_eq!(st.latency[2], 1, "10-49 ms");
    assert_eq!(st.latency[3], 1, "50-99 ms");
    assert_eq!(st.latency[0], 1, "a small negative stays negative");
    assert_eq!(st.latency_missing, 0);
    // One time missing: counted apart, no bucket.
    let m = tais_rec("<recSAFAReceiptTime>2030-01-02T03:04:05Z</recSAFAReceiptTime>");
    assert!(m.errors.is_empty());
    let mut st = new_stats();
    let xml = format!(
        r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}"><record><recSAFAReceiptTime>2030-01-02T03:04:05Z</recSAFAReceiptTime></record></a:TATrackAndFlightPlan>"#
    );
    parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    assert_eq!((st.latency_missing, st.latency.iter().sum::<u64>()), (1, 0));
    // A time of day past 24 h, and a receipt time that is not a time, are refused.
    let m = tais_rec("<recSTARSTimestamp>86400000</recSTARSTimestamp><recSAFAReceiptTime>soon</recSAFAReceiptTime>");
    assert_eq!(m.errors.len(), 2);
    assert_eq!(m.errors[0].kind, FieldErrorKind::Range);
    assert_eq!(m.errors[1].kind, FieldErrorKind::Syntax);
}

// REQ: OA-STD-029
#[test]
fn the_status_roots_become_stdds_status_with_service_and_state() {
    let mut p = parser();
    let mut st = new_stats();
    let lim = Limits::default();
    let svc = r#"<s:AirportDataServiceStatus xmlns:s="urn:us:gov:dot:faa:atm:terminal:entities:v4-0:status"><rvrLinks><rvrLink><linkStatus>Normal</linkStatus></rvrLink></rvrLinks><serviceStatus>Degraded</serviceStatus></s:AirportDataServiceStatus>"#;
    match parse_message(&mut p, svc.as_bytes(), &lim, &mut st).unwrap() {
        Message::Status(m) => {
            assert_eq!(m.root, "AirportDataServiceStatus");
            assert_eq!(m.states.len(), 1);
            assert_eq!(m.states[0].name, "AirportDataServiceStatus");
            assert_eq!(m.states[0].value, "Degraded");
        }
        other => panic!("{other:?}"),
    }
    let stdds = format!(
        r#"<s:STDDSStatus xmlns:s="{STATUS_NS}"><p k="ntpStatus" v="NORMAL"/><p k="DRSserviceStatus"><p k="x" v="1"/></p><p k="DRSserviceState" v="ENABLED"/><p k="swVersion" v="1.2"/></s:STDDSStatus>"#
    );
    match parse_message(&mut p, stdds.as_bytes(), &lim, &mut st).unwrap() {
        Message::Status(m) => {
            let got: Vec<_> = m
                .states
                .iter()
                .map(|s| (s.name.as_str(), s.value.as_str()))
                .collect();
            assert_eq!(
                got,
                [("ntpStatus", "NORMAL"), ("DRSserviceState", "ENABLED")]
            );
        }
        other => panic!("{other:?}"),
    }
    // Every root of the namespace that is one of the five names; other roots stay unknown.
    for root in oa_stdds::kind::STATUS_ROOTS {
        let xml = format!(r#"<s:{root} xmlns:s="{STATUS_NS}"/>"#);
        assert_eq!(
            classify(&parser().parse(xml.as_bytes()).unwrap()),
            Kind::StddsStatus,
            "{root}"
        );
    }
    let other = format!(r#"<s:SomethingElse xmlns:s="{STATUS_NS}"/>"#);
    assert_eq!(
        classify(&parser().parse(other.as_bytes()).unwrap()),
        Kind::Unknown
    );
    // Counted: kind, and the state histogram (service states, not identifying).
    assert_eq!(st.kinds[Kind::StddsStatus as usize], 2);
    assert!(st.status_values.contains(&(
        "AirportDataServiceStatus AirportDataServiceStatus=Degraded".to_string(),
        1
    )));
    assert_eq!(kind_name_of("stdds-status"), Kind::StddsStatus);
}

fn kind_name_of(n: &str) -> Kind {
    let i = KIND_NAMES.iter().position(|k| *k == n).unwrap();
    [Kind::StddsStatus, Kind::MmixmAsset]
        .into_iter()
        .find(|k| *k as usize == i)
        .unwrap()
}

// REQ: OA-STD-030
#[test]
fn an_mmixm_asset_message_is_counted_by_kind_and_not_decoded() {
    let xml =
        r#"<m:AssetMessage xmlns:m="https://mmixm.aero/base/1.1.1"><x>1</x></m:AssetMessage>"#;
    let mut p = parser();
    let mut st = new_stats();
    match parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap() {
        Message::Other(k) => assert_eq!(k, Kind::MmixmAsset),
        other => panic!("{other:?}"),
    }
    assert_eq!(st.kinds[Kind::MmixmAsset as usize], 1);
    assert!(st.unknown_roots.is_empty());
    // Another root in that namespace is unknown.
    let xml = r#"<m:Other xmlns:m="https://mmixm.aero/base/1.1.1"/>"#;
    assert_eq!(
        classify(&parser().parse(xml.as_bytes()).unwrap()),
        Kind::Unknown
    );
}

// REQ: OA-STD-031
#[test]
fn asdex_status_sub_elements_stay_unknown_and_counted() {
    let m = smes_rec(
        "<status><vertRate>-300</vertRate><mon>1</mon><src>x</src><nc>2</nc></status><targetExtent>1</targetExtent>",
    );
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!(m.records[0].vvert_ftmin, -300);
    assert_eq!(m.unknown_elements, 4);
    let mut p = parser();
    let mut st = new_stats();
    let xml = format!(
        r#"<a:asdexMsg xmlns:a="{SMES_NS}"><positionReport><status><mon>1</mon><mon>1</mon></status></positionReport></a:asdexMsg>"#
    );
    parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    assert_eq!(
        st.unknown,
        [oa_stdds::UnknownPath {
            text: "status/mon".to_string(),
            count: 2
        }]
    );
}

// REQ: OA-STD-032
#[test]
fn a_refused_field_is_diagnosed_by_shape_and_never_by_value() {
    use oa_stdds::Shape;
    let s = Shape::of(b"123.4567-ab");
    assert_eq!((s.len, s.pattern().as_str()), (11, "9.9-a"));
    assert_eq!(Shape::of(b"AB 12").pattern(), "A~9");
    assert_eq!(Shape::of("\u{fffd}".as_bytes()).pattern(), "?");
    assert_eq!(Shape::of(b"").len, 0);
    // Long text is cut to the first 12 classes but keeps its true length.
    let long = Shape::of(b"a1a1a1a1a1a1a1a1a1a1");
    assert_eq!((long.len, long.pattern().len()), (20, 12));
    let m = tais_rec("<flightPlan><runway>#12</runway><acType>Z9Z9Z9Z9Z9</acType></flightPlan>");
    assert_eq!(m.errors.len(), 2);
    assert_eq!(m.errors[0].shape.pattern(), "#9");
    assert_eq!(m.errors[1].shape.len, 10);
    let mut p = parser();
    let mut st = new_stats();
    let xml = format!(
        r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}"><record><flightPlan><runway>#12</runway></flightPlan></record><record><flightPlan><runway>#34</runway></flightPlan></record></a:TATrackAndFlightPlan>"#
    );
    parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    assert_eq!(st.shapes.len(), 1);
    assert_eq!(st.shapes[0].field, "flightPlan/runway");
    assert_eq!((st.shapes[0].len, st.shapes[0].count), (3, 2));
    assert_eq!(st.shapes[0].pattern, "#9");
}

fn tais_xml(inner: &str) -> String {
    format!(r#"<a:TATrackAndFlightPlan xmlns:a="{TAIS_NS}">{inner}</a:TATrackAndFlightPlan>"#)
}

// REQ: OA-STD-033
#[test]
fn a_message_is_counted_once_on_every_path_and_a_direct_parse_counts_none() {
    let mut p = parser();
    let mut st = new_stats();
    let tiny = Limits {
        max_records: 1,
        ..Limits::default()
    };
    let rec = "<record><track><trackNum>1</trackNum></track></record>";
    let msgs: Vec<(String, &Limits)> = vec![
        (tais_xml(rec), &tiny),
        (tais_xml(&rec.repeat(2)), &tiny),
        ("<a><b></a>".to_string(), &tiny),
        (
            String::from_utf8(fixture("smes_position.xml")).unwrap(),
            &tiny,
        ),
        (format!(r#"<s:STDDSStatus xmlns:s="{STATUS_NS}"/>"#), &tiny),
        (r#"<x:Unknown xmlns:x="urn:x"/>"#.to_string(), &tiny),
    ];
    for (i, (xml, lim)) in msgs.iter().enumerate() {
        let _ = parse_message(&mut p, xml.as_bytes(), lim, &mut st);
        assert_eq!(
            st.messages,
            st.kinds.iter().sum::<u64>() + st.errors[0],
            "after message {i}"
        );
        assert_eq!(st.messages, i as u64 + 1);
    }
    assert_eq!(st.errors, [1, 0, 1, 0]);
    // Direct calls count the refusal but never a message.
    let before = st.messages;
    let doc = p.parse(b"<a/>").unwrap();
    let e = parse_tais(&doc, &Limits::default(), &mut st).unwrap_err();
    assert!(matches!(e, StdError::WrongKind { .. }));
    assert_eq!((st.messages, st.errors[1]), (before, 1));
}

// REQ: OA-STD-034
#[test]
fn the_message_level_src_and_airport_follow_the_field_rules() {
    let src = |inner: &str| tais(tais_xml(&format!("{inner}<record/>")).as_bytes());
    let m = src("<src>ZZA</src>");
    assert!(m.src_present && m.errors.is_empty());
    // Twice: neither is kept, and the error names the field.
    let m = src("<src>ZZA</src><src>ZZB</src>");
    assert!(!m.src_present);
    assert_eq!(m.errors.len(), 1);
    assert_eq!(
        (m.errors[0].field, m.errors[0].kind),
        ("src", FieldErrorKind::Duplicate)
    );
    // Marked invalid by the sender: absent, counted.
    let mut p = parser();
    let mut st = new_stats();
    let xml = tais_xml(r#"<src r="1">ZZA</src>"#);
    match parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap() {
        Message::Tais(m) => assert!(!m.src_present && m.errors.is_empty()),
        other => panic!("{other:?}"),
    }
    assert_eq!(st.invalidated, 1);
    // A refused value is a counted field error.
    let m = src("<src>TOO-LONG-VALUE</src>");
    assert!(!m.src_present);
    assert_eq!(m.errors[0].kind, FieldErrorKind::TooLong);
    // The same for the airport of a position report.
    let smes = |inner: &str| {
        let xml = format!(
            r#"<a:asdexMsg xmlns:a="{SMES_NS}">{inner}<positionReport><track>1</track></positionReport></a:asdexMsg>"#
        );
        let mut p = parser();
        let mut st = new_stats();
        match parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap() {
            Message::Smes(m) => m,
            other => panic!("{other:?}"),
        }
    };
    assert!(smes("<airport>ZZB</airport>").airport_present);
    let m = smes("<airport>ZZB</airport><airport>ZZC</airport>");
    assert!(!m.airport_present);
    assert_eq!(m.errors[0].kind, FieldErrorKind::Duplicate);
    assert!(!smes(r#"<airport r="1">ZZB</airport>"#).airport_present);
}

// REQ: OA-STD-035
#[test]
fn latency_is_signed_around_a_day_wrap_and_a_small_skew_counts_below_zero() {
    let mut p = parser();
    let mut st = new_stats();
    let mut day = |stars: &str, safa: &str| {
        let xml = tais_xml(&format!(
            "<record><recSTARSTimestamp>{stars}</recSTARSTimestamp><recSAFAReceiptTime>{safa}</recSAFAReceiptTime></record>"
        ));
        parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    };
    // A true midnight crossing: stamped 86_399_990, received 00:00:00.020 the next day: 30 ms.
    day("86399990", "2030-01-03T00:00:00.020Z");
    // A skew: received 2 s before it was stamped.
    day("11047678", "2030-01-02T03:04:05.678Z");
    assert_eq!(st.latency[2], 1, "30 ms across midnight");
    assert_eq!(st.latency[0], 1, "-2 s is below zero");
    assert_eq!(st.latency.iter().sum::<u64>(), 2);
}

// REQ: OA-STD-036
#[test]
fn integer_fields_take_whole_numbers_only() {
    let alt = |v: &str| {
        tais_rec(&format!(
            "<track><reportedAltitude>{v}</reportedAltitude></track>"
        ))
    };
    assert!(alt("8500").errors.is_empty());
    assert!(alt("0").errors.is_empty());
    assert!(alt("-500").errors.is_empty());
    for bad in ["85.0", "-0", "08500", "00", "+5", "-", "1e3", " "] {
        let m = alt(bad);
        assert_eq!(m.errors.len(), 1, "{bad}");
        assert!(
            matches!(
                m.errors[0].kind,
                FieldErrorKind::Syntax | FieldErrorKind::Empty
            ),
            "{bad}"
        );
    }
    // 13 digits is out of range, not a syntax error.
    assert_eq!(alt("1234567890123").errors[0].kind, FieldErrorKind::Range);
    // Counters take no leading zero either; the SMES altitude is a decimal and keeps fractions.
    let m =
        tais_rec("<track><trackNum>007</trackNum></track><flightPlan><sfpn>05</sfpn></flightPlan>");
    assert_eq!(m.errors.len(), 2);
    let m = smes_rec(
        "<position><altitude>125.5</altitude></position><status><vertRate>12.5</vertRate></status>",
    );
    assert_eq!(m.records[0].altitude_ft, 126);
    assert_eq!(m.errors.len(), 1);
    assert_eq!(m.errors[0].field, "status/vertRate");
    let m = tais_rec("<recSTARSTimestamp>0</recSTARSTimestamp>");
    assert!(m.errors.is_empty());
}

// REQ: OA-STD-037
#[test]
fn bounds_are_checked_on_the_exact_value_and_a_heading_stays_below_360() {
    let heading = |v: &str| smes_rec(&format!("<movement><heading>{v}</heading></movement>"));
    assert_eq!(heading("0").records[0].heading_cdeg, 0);
    assert_eq!(heading("359.99").records[0].heading_cdeg, 35_999);
    // Under 360 by less than half the unit: in range, and the stored value wraps to 0.
    assert_eq!(heading("359.9951").records[0].heading_cdeg, 0);
    for bad in ["360", "360.0", "360.00000001", "-0.001", "361"] {
        assert_eq!(heading(bad).errors[0].kind, FieldErrorKind::Range, "{bad}");
    }
    // Latitude: 90.00000001 rounds to 90 at 1e-7 degree but is beyond it.
    let lat = |v: &str| tais_rec(&format!("<track><lat>{v}</lat><lon>0</lon></track>"));
    assert!(lat("90").errors.is_empty());
    assert!(lat("90.00000004")
        .errors
        .iter()
        .any(|e| e.kind == FieldErrorKind::Range));
    assert!(lat("-90").errors.is_empty());
    assert!(lat("-90.00000004")
        .errors
        .iter()
        .any(|e| e.kind == FieldErrorKind::Range));
    // A velocity edge: 3000.004 m/s is past 3000 although it rounds to the edge.
    let vx = |v: &str| tais_rec(&format!("<track><vx>{v}</vx></track>"));
    assert!(vx("3000").errors.is_empty());
    assert_eq!(vx("3000.004").errors[0].kind, FieldErrorKind::Range);
    assert_eq!(vx("-3000.004").errors[0].kind, FieldErrorKind::Range);
    // The scaled parser's parts compare exactly.
    let a = oa_stdds::scalar::scaled_parts(b"-0.0000001", 0).unwrap();
    assert!(a.cmp_exact(0).is_lt() && a.cmp_exact(-1).is_gt());
}

// REQ: OA-STD-038
#[test]
fn a_fraction_past_a_millisecond_is_rounded_half_up_with_a_carry() {
    let t = |s: &str| parse_iso_ms(s.as_bytes()).unwrap();
    let base = t("2030-01-02T03:04:05Z");
    assert_eq!(t("2030-01-02T03:04:05.9994Z"), base + 999);
    assert_eq!(
        t("2030-01-02T03:04:05.9995Z"),
        base + 1000,
        "carries into the next second"
    );
    assert_eq!(t("2030-01-02T03:04:05.9999999Z"), base + 1000);
    assert_eq!(t("2030-01-02T03:04:59.9995Z"), t("2030-01-02T03:05:00Z"));
    assert_eq!(t("2030-01-02T23:59:59.9995Z"), t("2030-01-03T00:00:00Z"));
    assert_eq!(t("2030-01-02T03:04:05.0004Z"), base);
    assert_eq!(t("2030-01-02T03:04:05.0005Z"), base + 1);
}

// REQ: OA-STD-039
#[test]
fn a_status_key_or_value_over_the_limit_is_counted_and_never_silent() {
    let long = "V".repeat(70);
    let xml = format!(
        r#"<s:STDDSStatus xmlns:s="{STATUS_NS}"><p k="aStatus" v="{long}"/><p k="{long}Status" v="X"/><p k="bState" v=""/><p k="cStatus" v="OK"/></s:STDDSStatus>"#
    );
    let mut p = parser();
    let mut st = new_stats();
    match parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap() {
        Message::Status(m) => {
            assert_eq!(m.states.len(), 1);
            assert_eq!((m.value_too_long, m.key_too_long, m.dropped), (1, 1, 1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        (
            st.status_value_too_long,
            st.status_key_too_long,
            st.status_dropped
        ),
        (1, 1, 1)
    );
    // A service root whose text is over the reader's buffer.
    let xml = format!(
        r#"<s:AirportDataServiceStatus xmlns:s="{STATUS_NS}"><serviceStatus>{long}</serviceStatus></s:AirportDataServiceStatus>"#
    );
    parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    assert_eq!(st.status_value_too_long, 2);
    // More distinct states than the table holds: counted apart from the per-message drops.
    let mut st = new_stats();
    for i in 0..70 {
        let xml = format!(
            r#"<s:STDDSStatus xmlns:s="{STATUS_NS}"><p k="aStatus" v="V{i}"/></s:STDDSStatus>"#
        );
        parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    }
    assert_eq!((st.status_table_overflow, st.status_dropped), (6, 0));
}

// REQ: OA-STD-040
#[test]
fn only_fields_in_the_root_namespace_or_none_are_read_and_others_are_unknown() {
    // Children unqualified (the real feed): read.
    let m = tais_rec("<track><trackNum>5</trackNum></track>");
    assert_eq!(m.records[0].track_num, 5);
    // A child in a foreign namespace is unknown and counted, with nothing below it read.
    let xml = tais_xml(
        r#"<record><track xmlns="urn:other"><trackNum>5</trackNum></track><f:track xmlns:f="urn:other"><trackNum>6</trackNum></f:track></record><x:record xmlns:x="urn:other"><track><trackNum>7</trackNum></track></x:record>"#,
    );
    let mut p = parser();
    let mut st = new_stats();
    match parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap() {
        Message::Tais(m) => {
            assert_eq!(m.records.len(), 1, "a foreign record is not a record");
            assert_eq!(m.records[0].present, 0);
            assert_eq!(
                m.unknown_elements, 3,
                "two in the record and the foreign record"
            );
        }
        other => panic!("{other:?}"),
    }
    // Children in the root's own namespace (a default namespace) are read too.
    let xml = format!(
        r#"<TATrackAndFlightPlan xmlns="{TAIS_NS}"><record><track><trackNum>9</trackNum></track></record></TATrackAndFlightPlan>"#
    );
    match parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap() {
        Message::Tais(m) => assert_eq!(m.records[0].track_num, 9),
        other => panic!("{other:?}"),
    }
}

// REQ: OA-STD-041
#[test]
fn an_r_attribute_other_than_one_or_zero_is_a_field_error_and_unknown_paths_are_capped() {
    let m = tais_rec(
        r#"<track><trackNum r="0">5</trackNum><lat r="2">1</lat><lon r="">1</lon></track>"#,
    );
    assert_eq!(m.records[0].track_num, 5);
    assert_eq!(m.errors.len(), 2);
    assert!(m.errors.iter().all(|e| e.kind == FieldErrorKind::Syntax));
    // An unknown path longer than the cap is stored cut, and the same cut path merges.
    let long = "n".repeat(125);
    let mut p = parser();
    let mut st = new_stats();
    for _ in 0..2 {
        let xml = tais_xml(&format!("<{long}/>"));
        parse_message(&mut p, xml.as_bytes(), &Limits::default(), &mut st).unwrap();
    }
    assert_eq!(st.unknown.len(), 1);
    assert_eq!((st.unknown[0].text.len(), st.unknown[0].count), (120, 2));
}

// REQ: OA-STD-042
#[test]
fn the_summary_prints_status_and_unknown_names_only_when_they_are_plain_text() {
    use oa_feed::frame::{encode_frame, MAX_FRAME_LEN};
    let msgs = [
        format!(
            r#"<s:AirportDataServiceStatus xmlns:s="{STATUS_NS}"><serviceStatus>Ok;!</serviceStatus></s:AirportDataServiceStatus>"#
        ),
        format!(
            r#"<s:AirportDataServiceStatus xmlns:s="{STATUS_NS}"><serviceStatus>Normal</serviceStatus></s:AirportDataServiceStatus>"#
        ),
        tais_xml("<record><\u{e9}trange>1</\u{e9}trange><plain>1</plain></record>"),
    ];
    let mut stream = Vec::new();
    let mut out = vec![0u8; MAX_FRAME_LEN];
    for (i, m) in msgs.iter().enumerate() {
        let len = encode_frame(&mut out, 1, 0, i as u64, 0, 0, m.as_bytes()).unwrap();
        stream.extend_from_slice(&out[..len]);
    }
    let file = std::env::temp_dir().join(format!("oa-stdds-names-{}.oaf", std::process::id()));
    std::fs::write(&file, &stream).unwrap();
    let exe = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/stdds_summary");
    let o = std::process::Command::new(&exe)
        .arg(&file)
        .output()
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    let out = String::from_utf8(o.stdout).unwrap();
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert!(
        out.contains("stdds-status AirportDataServiceStatus AirportDataServiceStatus=Normal: 1")
    );
    assert!(out.contains("stdds-status other (text outside [A-Za-z0-9 _-]): 1"));
    assert!(!out.contains("Ok;"));
    assert!(out.contains("unknown element plain: 1"));
    assert!(!out.contains("trange"));
    assert!(out.contains("unknown names printed as other: 1"));
    assert!(out.contains("frame reader stalled, bytes not read: 0"));
}

// REQ: OA-SEAL-014
#[test]
fn messages_parsed_into_scratch_accumulators_and_merged_equal_one_accumulator() {
    let mut p = parser();
    let lim = Limits::default();
    let mut whole = new_stats();
    let mut merged = new_stats();
    for name in [
        "tais_full.xml",
        "tais_malformed_fields.xml",
        "tais_other_root.xml",
        "smes_position.xml",
        "stdds_rvr.xml",
        "stdds_other.xml",
        "tais_full.xml",
    ] {
        let bytes = fixture(name);
        let _ = parse_message(&mut p, &bytes, &lim, &mut whole);
        let mut scratch = new_stats();
        let _ = parse_message(&mut p, &bytes, &lim, &mut scratch);
        merged.merge(&scratch, &lim);
    }
    assert!(whole.messages == 7 && whole.records > 0);
    assert_eq!(format!("{whole:?}"), format!("{merged:?}"));
}

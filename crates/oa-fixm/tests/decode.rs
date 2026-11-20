//! Decoder tests over hand-written fixtures. Callsigns, identifiers and positions are made up;
//! only the element structure follows real SFDPS messages.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_fixm::{
    clear_events, code_str, field_error, has_field, new_events, parse_iso_ms, parse_scaled,
    parse_uuid, route_text, sector_code, track_ddeg, unit_code, unknown_text, Decoder, Events,
    Field, FixmError, Limits, Reject, Summary, ALL_FIELDS, ALT_FORM_AT_FIX, ALT_FORM_BLOCK,
    ALT_FORM_SIMPLE, ALT_FORM_VFR, ALT_FORM_VFR_PLUS, FIELD_COUNT, FIELD_NAMES, FLAG_ASSIGNED_VFR,
    FLAG_ASSIGNED_VFR_PLUS, FLAG_INTERIM_NIL, FTYPE_SCHEDULED, REJECT_COUNT, REJECT_NAMES,
    STATUS_ACTIVE, STATUS_CANCELLED, STATUS_PROPOSED,
};
use oa_xml::{Limits as XmlLimits, Parser};

const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">"#;
const TAIL: &str = "</ns5:MessageCollection>";
const GUFI: &str = "11111111-2222-3333-4444-555555555555";
const GUFI_BITS: u128 = 0x1111_1111_2222_3333_4444_5555_5555_5555;
const TS: &str = "2030-01-02T03:04:05.678Z";
const TS_MS: i64 = 1_893_553_445_678;

fn gufi_el(g: &str) -> String {
    format!(r#"<gufi codeSpace="urn:uuid">{g}</gufi>"#)
}

fn message_with(flight_attrs: &str, body: &str) -> String {
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" {flight_attrs}>{body}</flight></message>"#
    )
}

fn message(source: &str, body: &str) -> String {
    message_with(
        &format!(r#"centre="ZXX" source="{source}" system="SLC" timestamp="{TS}""#),
        &format!("{}{body}", gufi_el(GUFI)),
    )
}

fn doc(msgs: &[String]) -> String {
    format!("{HEAD}{}{TAIL}", msgs.concat())
}

struct Run {
    ev: Events,
    sum: Summary,
    dec: Decoder,
}

fn run_with(xml: &str, cap: usize, strings: usize, limits: Limits) -> (Run, Result<(), FixmError>) {
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let d = parser.parse(xml.as_bytes()).expect("fixture parses");
    let mut dec = Decoder::new(limits);
    let mut ev = new_events(cap, strings);
    let r = dec.decode(&d, xml.as_bytes(), &mut ev);
    let (sum, res) = match r {
        Ok(s) => (s, Ok(())),
        Err(e) => (Summary::default(), Err(e)),
    };
    (Run { ev, sum, dec }, res)
}

fn run(xml: &str) -> Run {
    let (r, res) = run_with(
        xml,
        64,
        1 << 16,
        Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    res.expect("decodes");
    r
}

fn one(source: &str, body: &str) -> Run {
    let r = run(&doc(&[message(source, body)]));
    assert_eq!(r.ev.len, 1, "one row");
    r
}

fn err_field(r: &Run, f: Field) {
    assert!(
        field_error(&r.ev, 0, f),
        "{} should be an error",
        FIELD_NAMES[f as usize]
    );
    assert!(
        !has_field(&r.ev, 0, f),
        "{} should be absent",
        FIELD_NAMES[f as usize]
    );
    assert_eq!(r.dec.stats().field_errors[f as usize], 1);
}

const TH_BODY: &str = r#"
<arrival xsi:type="ns5:NasArrivalType" arrivalPoint="KTST"><runwayPositionAndTime><runwayTime><estimated time="2030-01-02T05:00:00Z"/></runwayTime></runwayPositionAndTime></arrival>
<controllingUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZXX" sectorIdentifier="08"/>
<departure xsi:type="ns5:NasDepartureType" departurePoint="KDEP"><runwayPositionAndTime><runwayTime><actual time="2030-01-02T02:00:00Z"/></runwayTime></runwayPositionAndTime></departure>
<enRoute xsi:type="ns5:NasEnRouteType">
<boundaryCrossings xsi:type="ns5:NasUnitBoundaryType"><handoff xsi:type="ns5:NasHandoffType"><receivingUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZYY" sectorIdentifier="03"/></handoff></boundaryCrossings>
<position xsi:type="ns5:NasAircraftPositionType" targetPositionTime="2030-01-02T03:04:00Z" positionTime="2030-01-02T03:04:00Z" reportSource="SURVEILLANCE">
<actualSpeed><surveillance uom="KNOTS">412.5</surveillance></actualSpeed>
<altitude uom="FEET">38000.0</altitude>
<position xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>32.6 -84.776389</pos></location></position>
<targetAltitude uom="FEET">38000.0</targetAltitude>
<trackVelocity><x uom="KNOTS">-190.0</x><y uom="KNOTS">366.0</y></trackVelocity>
</position></enRoute>
<flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="104" siteSpecificPlanId="400" aircraftIdentification="TST123"/>
<flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="ACTIVE"/>
<operator><operatingOrganization><organization name="TST"/></operatingOrganization></operator>
<supplementalData xsi:type="ns5:NasSupplementalDataType"><additionalFlightInformation>
<nameValue name="MSG_SEQ_NO" value="42141910"/><nameValue name="FLIGHT_PLAN_SEQ_NO" value="8"/>
<nameValue name="FDPS_GUFI" value="made.up/1"/></additionalFlightInformation></supplementalData>
<assignedAltitude><simple uom="FEET">36000.0</simple></assignedAltitude>
<flightPlan identifier="KR000000AB"/>"#;

// REQ: OA-FIXM-001
#[test]
fn track_message_th_decodes_every_column() {
    let xml = doc(&[message_with(
        &format!(
            r#"centre="ZXX" source="TH" system="SLC" timestamp="{TS}" flightType="SCHEDULED""#
        ),
        &format!("{}{TH_BODY}", gufi_el(GUFI)),
    )]);
    let r = run(&xml);
    let e = &r.ev;
    assert_eq!(r.sum.rows, 1);
    assert_eq!(r.sum.rejected, 0);
    assert_eq!(r.sum.field_errors, 0);
    assert_eq!(r.sum.unknown_elements + r.sum.unknown_attributes, 0);
    assert_eq!(code_str(&e.source[0]), "TH");
    assert_eq!(code_str(&e.centre[0]), "ZXX");
    assert_eq!(e.ts_ms[0], TS_MS);
    assert_eq!(e.gufi[0], GUFI_BITS);
    assert_eq!(code_str(&e.callsign[0]), "TST123");
    assert_eq!(code_str(&e.computer_id[0]), "104");
    assert_eq!(code_str(&e.plan_id[0]), "KR000000AB");
    assert_eq!(code_str(&e.departure[0]), "KDEP");
    assert_eq!(code_str(&e.arrival[0]), "KTST");
    assert_eq!(e.status[0], STATUS_ACTIVE);
    assert_eq!(e.flight_type[0], FTYPE_SCHEDULED);
    assert_eq!(e.assigned_ft[0], 36_000);
    assert_eq!(e.reported_ft[0], 38_000);
    assert_eq!(e.lat_e7[0], 326_000_000);
    assert_eq!(e.lon_e7[0], -847_763_890);
    assert_eq!(e.pos_time_ms[0], 1_893_553_440_000);
    assert_eq!(e.gs_dkt[0], 4125);
    assert_eq!((e.vx_dkt[0], e.vy_dkt[0]), (-1900, 3660));
    assert_eq!(code_str(&e.operator[0]), "TST");
    assert_eq!(unit_code(&e.ctrl[0]), "ZXX");
    assert_eq!(sector_code(&e.ctrl[0]), "08");
    assert_eq!(unit_code(&e.recv[0]), "ZYY");
    assert_eq!(sector_code(&e.recv[0]), "03");
    assert_eq!(e.dep_actual_ms[0], 1_893_549_600_000);
    assert_eq!(e.arr_est_ms[0], 1_893_560_400_000);
    assert_eq!(e.msg_seq[0], 42_141_910);
    assert_eq!(e.plan_seq[0], 8);
    for f in [
        Field::Source,
        Field::Centre,
        Field::Callsign,
        Field::ComputerId,
        Field::FlightPlanId,
        Field::Departure,
        Field::Arrival,
        Field::Status,
        Field::FlightType,
        Field::AssignedAlt,
        Field::ReportedAlt,
        Field::Position,
        Field::PositionTime,
        Field::GroundSpeed,
        Field::Velocity,
        Field::Operator,
        Field::ControllingUnit,
        Field::HandoffReceiving,
        Field::DepartureActual,
        Field::ArrivalEstimated,
        Field::MsgSeq,
        Field::PlanSeq,
    ] {
        assert!(has_field(e, 0, f), "{}", FIELD_NAMES[f as usize]);
        assert!(!field_error(e, 0, f));
    }
    for f in [
        Field::Coast,
        Field::AircraftType,
        Field::Route,
        Field::Beacon,
        Field::HandoffTransferring,
    ] {
        assert!(!has_field(e, 0, f), "{}", FIELD_NAMES[f as usize]);
    }
    // Track from the velocity vector: east -19 kt, north 36.6 kt is about 332.6 degrees.
    assert_eq!(track_ddeg(e, 0), Some(3_326));
}

// REQ: OA-FIXM-001
#[test]
fn position_report_hz_without_speed() {
    let body = r#"<enRoute xsi:type="ns5:NasEnRouteType"><position xsi:type="ns5:NasAircraftPositionType"><actualSpeed></actualSpeed><position xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>29.901944 -95.632778</pos></location></position></position></enRoute>
<flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="126" aircraftIdentification="TST9"/>
<flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="ACTIVE"/>
<assignedAltitude><simple uom="FEET">6000.0</simple></assignedAltitude>"#;
    let r = one("HZ", body);
    let e = &r.ev;
    assert_eq!(code_str(&e.source[0]), "HZ");
    assert!(has_field(e, 0, Field::Position));
    assert_eq!((e.lat_e7[0], e.lon_e7[0]), (299_019_440, -956_327_780));
    assert!(!has_field(e, 0, Field::GroundSpeed));
    assert!(!has_field(e, 0, Field::PositionTime));
    assert_eq!(e.assigned_ft[0], 6000);
    assert_eq!(track_ddeg(e, 0), None);
    assert_eq!(r.sum.field_errors, 0);
}

// REQ: OA-FIXM-001
#[test]
fn flight_plan_fh_with_route_and_aircraft() {
    let body = r#"<agreed><route xsi:type="ns5:NasRouteType" nasRouteText="KAAA.TESTA1.FIXAA..FIXBB..KBBB/1532" initialFlightRules="IFR"><estimatedElapsedTime elapsedTime="P0Y0M0DT0H25M0S"><location><region airspaceType="FIR">KZXX</region></location></estimatedElapsedTime></route></agreed>
<aircraftDescription xsi:type="ns5:NasAircraftType" equipmentQualifier="L" aircraftAddress="ABCDEF" registration="N1TST" wakeTurbulence="M"><aircraftType><icaoModelIdentifier>B38M</icaoModelIdentifier></aircraftType></aircraftDescription>
<arrival xsi:type="ns5:NasArrivalType" arrivalPoint="KBBB"><arrivalAerodromeAlternate xsi:type="ns2:IcaoAerodromeReferenceType" code="KCCC"/></arrival>
<departure xsi:type="ns5:NasDepartureType" departurePoint="KAAA"/>
<enRoute xsi:type="ns5:NasEnRouteType"><beaconCodeAssignment><currentBeaconCode>2604</currentBeaconCode></beaconCodeAssignment></enRoute>
<flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="037" aircraftIdentification="TST1"/>
<flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="PROPOSED"/>
<requestedAltitude><simple uom="FEET">35000.0</simple></requestedAltitude>"#;
    let r = one("FH", body);
    let e = &r.ev;
    assert_eq!(route_text(e, 0), "KAAA.TESTA1.FIXAA..FIXBB..KBBB/1532");
    assert_eq!(code_str(&e.aircraft_type[0]), "B38M");
    assert_eq!(e.status[0], STATUS_PROPOSED);
    assert_eq!(e.beacon[0], 2604);
    assert_eq!(e.strings_used, e.route_len[0] as usize);
    assert!(!has_field(e, 0, Field::Position));
    assert_eq!(r.sum.field_errors, 0);
    assert_eq!(r.sum.unknown_elements + r.sum.unknown_attributes, 0);
}

// REQ: OA-FIXM-001
#[test]
fn amendment_ah_carries_route_and_vfr_plus_altitude() {
    let body = r#"<agreed><route xsi:type="ns5:NasRouteType" nasRouteText="KAAA..KBBB"/></agreed>
<flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="037" aircraftIdentification="TST2"/>
<assignedAltitude><vfrPlus uom="FEET">5500.0</vfrPlus></assignedAltitude>"#;
    let r = one("AH", body);
    assert_eq!(route_text(&r.ev, 0), "KAAA..KBBB");
    assert_eq!(r.ev.assigned_ft[0], 5500);
    assert_eq!(r.ev.flags[0], FLAG_ASSIGNED_VFR_PLUS);
}

// REQ: OA-FIXM-001
#[test]
fn handoff_oh_with_transferring_and_receiving_units() {
    let body = r#"<enRoute xsi:type="ns5:NasEnRouteType"><boundaryCrossings xsi:type="ns5:NasUnitBoundaryType"><handoff xsi:type="ns5:NasHandoffType" event="INITIATION">
<receivingUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZYY" sectorIdentifier="1E"/>
<transferringUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="???" sectorIdentifier="12"/>
</handoff></boundaryCrossings></enRoute>"#;
    let r = one("OH", body);
    let e = &r.ev;
    assert_eq!(unit_code(&e.recv[0]), "ZYY");
    assert_eq!(sector_code(&e.recv[0]), "1E");
    // The FAA writes an unknown unit as question marks; it is kept as stated.
    assert_eq!(unit_code(&e.xfer[0]), "???");
    assert_eq!(e.handoff_event[0], oa_fixm::HANDOFF_INITIATION);
    assert_eq!(r.sum.unknown_attributes, 0);
}

// REQ: OA-FIXM-001
#[test]
fn departure_dh_cancel_cl_beacon_ba_and_vfr_flag() {
    let dh = r#"<departure xsi:type="ns5:NasDepartureType" departurePoint="KAAA"><runwayPositionAndTime><runwayTime><actual time="2030-01-02T02:00:00Z"/></runwayTime></runwayPositionAndTime></departure>"#;
    let r = one("DH", dh);
    assert_eq!(r.ev.dep_actual_ms[0], 1_893_549_600_000);

    let cl = r#"<flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="CANCELLED"/>"#;
    let r = one("CL", cl);
    assert_eq!(r.ev.status[0], STATUS_CANCELLED);

    let ba = r#"<enRoute xsi:type="ns5:NasEnRouteType"><beaconCodeAssignment><currentBeaconCode>0123</currentBeaconCode></beaconCodeAssignment></enRoute>"#;
    let r = one("BA", ba);
    assert_eq!(r.ev.beacon[0], 123);

    let vfr = r#"<assignedAltitude><vfr/></assignedAltitude><enRoute xsi:type="ns5:NasEnRouteType"><position><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>1 2</pos></location></position></position></enRoute>"#;
    let r = one("LH", vfr);
    assert_eq!(r.ev.flags[0], FLAG_ASSIGNED_VFR);
    assert!(!has_field(&r.ev, 0, Field::AssignedAlt));
    assert_eq!((r.ev.lat_e7[0], r.ev.lon_e7[0]), (10_000_000, 20_000_000));
}

// REQ: OA-FIXM-017
#[test]
fn several_messages_make_one_row_each_in_order() {
    let a = message(
        "TH",
        r#"<flightIdentification computerId="001" aircraftIdentification="AAA1"/>"#,
    );
    let b = message(
        "FH",
        r#"<flightIdentification computerId="002" aircraftIdentification="BBB2"/>"#,
    );
    let c = message(
        "HZ",
        r#"<flightIdentification computerId="003" aircraftIdentification="CCC3"/>"#,
    );
    let r = run(&doc(&[a, b, c]));
    assert_eq!(r.ev.len, 3);
    assert_eq!(r.sum.messages, 3);
    assert_eq!(r.sum.rows, 3);
    let names: Vec<_> = (0..3).map(|i| code_str(&r.ev.callsign[i])).collect();
    assert_eq!(names, ["AAA1", "BBB2", "CCC3"]);
    let src: Vec<_> = (0..3).map(|i| code_str(&r.ev.source[i])).collect();
    assert_eq!(src, ["TH", "FH", "HZ"]);
    assert_eq!(r.dec.stats().messages, 3);
}

// REQ: OA-FIXM-002
#[test]
fn iso_timestamps_are_strict_utc() {
    assert_eq!(parse_iso_ms(b"1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_iso_ms(b"1970-01-01T00:00:00.001Z"), Some(1));
    assert_eq!(parse_iso_ms(b"2000-01-01T00:00:00Z"), Some(946_684_800_000));
    assert_eq!(
        parse_iso_ms(b"2024-02-29T12:00:00Z"),
        Some(1_709_208_000_000)
    );
    assert_eq!(
        parse_iso_ms(b"2026-11-14T13:50:49.564Z"),
        Some(1_794_664_249_564)
    );
    assert_eq!(
        parse_iso_ms(b"9999-12-31T23:59:59Z"),
        Some(253_402_300_799_000)
    );
    // A fraction finer than a millisecond is truncated, not rounded.
    assert_eq!(
        parse_iso_ms(b"2000-01-01T00:00:00.9999Z"),
        Some(946_684_800_999)
    );
    assert_eq!(
        parse_iso_ms(b"2000-01-01T00:00:00.5Z"),
        Some(946_684_800_500)
    );
    for bad in [
        &b""[..],
        b"2000-01-01",
        b"2000-01-01T00:00:00",
        b"2000-01-01T00:00:00z",
        b"2000-01-01T00:00:00+00:00",
        b"2000-01-01 00:00:00Z",
        b"2000-01-01T00:00Z",
        b"2000-13-01T00:00:00Z",
        b"2000-00-01T00:00:00Z",
        b"2000-01-32T00:00:00Z",
        b"2000-01-00T00:00:00Z",
        b"2023-02-29T00:00:00Z",
        b"1900-02-29T00:00:00Z",
        b"2000-01-01T24:00:00Z",
        b"2000-01-01T00:60:00Z",
        b"2000-01-01T00:00:60Z",
        b"1969-12-31T23:59:59Z",
        b"2000-01-01T00:00:00.Z",
        b"2000-01-01T00:00:00.1234567890Z",
        b"2000-01-01T00:00:00Z ",
        b" 2000-01-01T00:00:00Z",
        b"2000-01-01T00:00:00ZZ",
        b"20000-01-01T00:00:00Z",
        b"2000-1-01T00:00:00Z",
        b"2000-01-01T0a:00:00Z",
        b"2000-01-01T00:00:00,5Z",
    ] {
        assert_eq!(parse_iso_ms(bad), None, "{}", String::from_utf8_lossy(bad));
    }
    assert_eq!(parse_iso_ms(b"2000-02-29T00:00:00Z"), Some(951_782_400_000));
}

// REQ: OA-FIXM-002
#[test]
fn uuid_and_decimal_parsers_are_strict() {
    assert_eq!(parse_uuid(GUFI.as_bytes()), Some(GUFI_BITS));
    assert_eq!(
        parse_uuid(b"ABCDEFAB-CDEF-ABCD-EFAB-CDEFABCDEFAB"),
        parse_uuid(b"abcdefab-cdef-abcd-efab-cdefabcdefab")
    );
    for bad in [
        "",
        "1111111122223333444455555555555",
        "11111111-2222-3333-4444-55555555555g",
        "11111111_2222-3333-4444-555555555555",
        "{11111111-2222-3333-4444-555555555555}",
    ] {
        assert_eq!(parse_uuid(bad.as_bytes()), None, "{bad}");
    }
    assert_eq!(parse_scaled(b"12.5", 1), Some(125));
    assert_eq!(parse_scaled(b"-0.25", 1), Some(-3)); // half away from zero
    assert_eq!(parse_scaled(b"0.24", 1), Some(2));
    assert_eq!(parse_scaled(b"7", 7), Some(70_000_000));
    assert_eq!(parse_scaled(b"-84.776389", 7), Some(-847_763_890));
    for bad in [
        "",
        "-",
        ".5",
        "5.",
        "+5",
        "1e3",
        "1,5",
        " 5",
        "5 ",
        "--5",
        "1.2.3",
        "NaN",
        "inf",
        "1234567890123",
    ] {
        assert_eq!(parse_scaled(bad.as_bytes(), 1), None, "{bad}");
    }
}

// REQ: OA-FIXM-003
#[test]
fn every_attribute_field_error_is_counted_and_marks_the_row() {
    // (source field, good fragment, bad fragment, field)
    let cases: &[(&str, &str, Field)] = &[
        (
            r#"aircraftIdentification="TST123""#,
            r#"aircraftIdentification="TOO-LONG-ID""#,
            Field::Callsign,
        ),
        (
            r#"computerId="104""#,
            r#"computerId="1 4""#,
            Field::ComputerId,
        ),
        (
            r#"identifier="KR000000AB""#,
            r#"identifier="KR000000ABCDEF""#,
            Field::FlightPlanId,
        ),
        (
            r#"fdpsFlightStatus="ACTIVE""#,
            r#"fdpsFlightStatus="EXPLODED""#,
            Field::Status,
        ),
        (
            r#"departurePoint="KDEP""#,
            r#"departurePoint="K D""#,
            Field::Departure,
        ),
        (
            r#"arrivalPoint="KTST""#,
            r#"arrivalPoint="""#,
            Field::Arrival,
        ),
        (
            r#"<actual time="2030-01-02T02:00:00Z"/>"#,
            r#"<actual time="2030-01-02T02:00:00"/>"#,
            Field::DepartureActual,
        ),
        (
            r#"<estimated time="2030-01-02T05:00:00Z"/>"#,
            r#"<estimated time="soon"/>"#,
            Field::ArrivalEstimated,
        ),
        (
            r#"positionTime="2030-01-02T03:04:00Z""#,
            r#"positionTime="2030-02-30T03:04:00Z""#,
            Field::PositionTime,
        ),
        (
            r#"unitIdentifier="ZXX" sectorIdentifier="08""#,
            r#"unitIdentifier="ZXX" sectorIdentifier="0 8""#,
            Field::ControllingUnit,
        ),
        (
            r#"unitIdentifier="ZYY" sectorIdentifier="03""#,
            r#"unitIdentifier="" sectorIdentifier="03""#,
            Field::HandoffReceiving,
        ),
        (
            r#"name="TST""#,
            r#"name="NAME THAT IS FAR TOO LONG TO FIT""#,
            Field::Operator,
        ),
        (r#"value="42141910""#, r#"value="42x""#, Field::MsgSeq),
        (r#"value="8""#, r#"value="99999999999""#, Field::PlanSeq),
    ];
    for (good, bad, f) in cases {
        assert!(TH_BODY.contains(good), "fixture lacks {good}");
        let body = TH_BODY.replace(good, bad);
        let r = one("TH", &body);
        err_field(&r, *f);
        assert_eq!(r.sum.field_errors, 1, "{}", FIELD_NAMES[*f as usize]);
        assert_eq!(r.sum.rejected, 0);
        // The other fields are untouched.
        assert!(has_field(&r.ev, 0, Field::Position));
        assert_eq!(r.ev.gufi[0], GUFI_BITS);
    }
}

// REQ: OA-FIXM-003
#[test]
fn flight_attribute_field_errors() {
    let g = gufi_el(GUFI);
    for (attrs, f) in [
        (r#"centre="ZXXXX" source="TH""#, Field::Centre),
        (r#"centre="ZXX" source="TH1""#, Field::Source),
        (
            r#"centre="ZXX" source="TH" flightType="BOGUS""#,
            Field::FlightType,
        ),
    ] {
        let m = message_with(&format!(r#"{attrs} timestamp="{TS}""#), &g);
        let r = run(&doc(&[m]));
        assert_eq!(r.ev.len, 1);
        err_field(&r, f);
    }
    let r = run(&doc(&[message_with(&format!(r#"timestamp="{TS}""#), &g)]));
    assert!(!has_field(&r.ev, 0, Field::Source));
    assert!(
        !field_error(&r.ev, 0, Field::Source),
        "an absent field is not an error"
    );
}

// REQ: OA-FIXM-003
#[test]
fn text_field_errors_empty_and_unreadable_values() {
    let alt_empty = TH_BODY.replace(
        r#"<altitude uom="FEET">38000.0</altitude>"#,
        r#"<altitude uom="FEET"/>"#,
    );
    err_field(&one("TH", &alt_empty), Field::ReportedAlt);
    let ac = r#"<aircraftDescription><aircraftType><icaoModelIdentifier>B38M 777</icaoModelIdentifier></aircraftType></aircraftDescription>"#;
    err_field(&one("FH", ac), Field::AircraftType);
    for code in ["2608", "260", "26040", "26a4", " 604"] {
        let b = format!(
            r#"<enRoute><beaconCodeAssignment><currentBeaconCode>{code}</currentBeaconCode></beaconCodeAssignment></enRoute>"#
        );
        err_field(&one("BA", &b), Field::Beacon);
    }
    let r = one(
        "BA",
        r#"<enRoute><position coastIndicator="COASTING"/></enRoute>"#,
    );
    assert!(has_field(&r.ev, 0, Field::Coast));
    let r = one(
        "BA",
        r#"<enRoute><position coastIndicator="MAYBE"/></enRoute>"#,
    );
    err_field(&r, Field::Coast);
    let xfer = r#"<enRoute><boundaryCrossings><handoff><transferringUnit unitIdentifier="ZYYYY"/></handoff></boundaryCrossings></enRoute>"#;
    err_field(&one("OH", xfer), Field::HandoffTransferring);
    // A duplicated single-valued element is an error, not a silent first-wins.
    let dup =
        r#"<flightStatus fdpsFlightStatus="ACTIVE"/><flightStatus fdpsFlightStatus="ACTIVE"/>"#;
    err_field(&one("TH", dup), Field::Status);
}

// REQ: OA-FIXM-004
#[test]
fn altitude_range_and_units() {
    let body = |alt: &str, uom: &str| {
        format!(r#"<assignedAltitude><simple uom="{uom}">{alt}</simple></assignedAltitude>"#)
    };
    for (alt, want) in [
        ("-2000", -2000),
        ("100000", 100_000),
        ("0", 0),
        ("35000.4", 35_000),
        ("35000.5", 35_001),
        ("-1500", -1500),
    ] {
        let r = one("TH", &body(alt, "FEET"));
        assert_eq!(r.ev.assigned_ft[0], want, "{alt}");
        assert_eq!(r.sum.field_errors, 0);
    }
    for alt in ["-2001", "100001", "999999", "abc", "", "1e4", "+5"] {
        let r = one("TH", &body(alt, "FEET"));
        err_field(&r, Field::AssignedAlt);
    }
    // Metres are converted to feet.
    let r = one("TH", &body("10000", "METERS"));
    assert_eq!(r.ev.assigned_ft[0], 32_808);
    // Unknown or missing units are field errors.
    for uom in ["FLIGHT_LEVEL", "feet", "FT", ""] {
        err_field(&one("TH", &body("5000", uom)), Field::AssignedAlt);
    }
    let nouom = r#"<assignedAltitude><simple>5000</simple></assignedAltitude>"#;
    err_field(&one("TH", nouom), Field::AssignedAlt);
    // The reported altitude follows the same rules.
    let rep = |alt: &str, uom: &str| {
        format!(r#"<enRoute><position><altitude uom="{uom}">{alt}</altitude></position></enRoute>"#)
    };
    assert_eq!(one("TH", &rep("41000.0", "FEET")).ev.reported_ft[0], 41_000);
    err_field(&one("TH", &rep("100001.0", "FEET")), Field::ReportedAlt);
    err_field(&one("TH", &rep("41000.0", "KNOTS")), Field::ReportedAlt);
}

// REQ: OA-FIXM-005
#[test]
fn position_range_and_reference_system() {
    let pos = |p: &str, srs: &str| {
        format!(
            r#"<enRoute><position><position><location srsName="{srs}"><pos>{p}</pos></location></position></position></enRoute>"#
        )
    };
    const EPSG: &str = "urn:ogc:def:crs:EPSG::4326";
    let r = one("TH", &pos("90 180", EPSG));
    assert_eq!(
        (r.ev.lat_e7[0], r.ev.lon_e7[0]),
        (900_000_000, 1_800_000_000)
    );
    let r = one("TH", &pos("-90.0 -179.9999999", EPSG));
    assert_eq!(
        (r.ev.lat_e7[0], r.ev.lon_e7[0]),
        (-900_000_000, -1_799_999_999)
    );
    for bad in [
        "90.0000001 0",
        "-91 0",
        "0 180.0000001",
        "0 -181",
        "12.5",
        "12.5 ",
        "12.5  30",
        "a b",
        "1,5 2",
        "1 2 3",
        "",
    ] {
        err_field(&one("TH", &pos(bad, EPSG)), Field::Position);
    }
    for srs in ["urn:ogc:def:crs:EPSG::3857", ""] {
        err_field(&one("TH", &pos("1 2", srs)), Field::Position);
    }
    let nosrs = r#"<enRoute><position><position><location><pos>1 2</pos></location></position></position></enRoute>"#;
    err_field(&one("TH", nosrs), Field::Position);
}

// REQ: OA-FIXM-018
#[test]
fn decimal_overflow_is_a_field_error_and_the_next_message_decodes() {
    let pos = |p: &str| {
        format!(
            r#"<enRoute><position><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>{p}</pos></location></position></position></enRoute>"#
        )
    };
    let xml = doc(&[
        message("TH", &pos("999999999999.9 0")),
        message("TH", &pos("10 20")),
    ]);
    let r = run(&xml);
    assert_eq!(r.ev.len, 2);
    assert!(field_error(&r.ev, 0, Field::Position));
    assert!(!has_field(&r.ev, 0, Field::Position));
    assert_eq!(r.dec.stats().field_errors[Field::Position as usize], 1);
    assert!(has_field(&r.ev, 1, Field::Position));
    assert_eq!((r.ev.lat_e7[1], r.ev.lon_e7[1]), (100_000_000, 200_000_000));
    // The scaler itself, at and past the i64 limits.
    assert_eq!(parse_scaled(b"999999999999", 0), Some(999_999_999_999));
    assert_eq!(parse_scaled(b"999999999999", 7), None);
    assert_eq!(parse_scaled(b"999999999999.9", 7), None);
    assert_eq!(parse_scaled(b"-999999999999.9", 7), None);
    assert_eq!(parse_scaled(b"922337203685.4775807", 7), Some(i64::MAX));
    assert_eq!(parse_scaled(b"922337203685.47758075", 7), None);
    assert_eq!(parse_scaled(b"922337203685.4775808", 7), None);
    assert_eq!(parse_scaled(b"1.5", 12), Some(1_500_000_000_000));
    assert_eq!(parse_scaled(b"999999999999.999999999999", 12), None);
}

// REQ: OA-FIXM-006
#[test]
fn speed_units_range_and_track() {
    let sp = |v: &str, u: &str| {
        format!(
            r#"<enRoute><position><actualSpeed><surveillance uom="{u}">{v}</surveillance></actualSpeed></position></enRoute>"#
        )
    };
    assert_eq!(one("TH", &sp("412.0", "KNOTS")).ev.gs_dkt[0], 4120);
    assert_eq!(one("TH", &sp("0", "KNOTS")).ev.gs_dkt[0], 0);
    assert_eq!(one("TH", &sp("2000.0", "KNOTS")).ev.gs_dkt[0], 20_000);
    assert_eq!(
        one("TH", &sp("100.0", "KILOMETERS_PER_HOUR")).ev.gs_dkt[0],
        540
    );
    assert_eq!(
        one("TH", &sp("100.0", "METERS_PER_SECOND")).ev.gs_dkt[0],
        1944
    );
    for (v, u) in [
        ("2000.1", "KNOTS"),
        ("-1.0", "KNOTS"),
        ("x", "KNOTS"),
        ("300", "MACH"),
        ("300", ""),
    ] {
        err_field(&one("TH", &sp(v, u)), Field::GroundSpeed);
    }
    let vel = |x: &str, y: &str, ux: &str| {
        format!(
            r#"<enRoute><position><trackVelocity><x uom="{ux}">{x}</x><y uom="KNOTS">{y}</y></trackVelocity></position></enRoute>"#
        )
    };
    let r = one("TH", &vel("0.0", "100.0", "KNOTS"));
    assert_eq!(track_ddeg(&r.ev, 0), Some(0));
    assert_eq!(
        track_ddeg(&one("TH", &vel("100.0", "0.0", "KNOTS")).ev, 0),
        Some(900)
    );
    assert_eq!(
        track_ddeg(&one("TH", &vel("0.0", "-100.0", "KNOTS")).ev, 0),
        Some(1800)
    );
    assert_eq!(
        track_ddeg(&one("TH", &vel("-100.0", "0.0", "KNOTS")).ev, 0),
        Some(2700)
    );
    assert_eq!(
        track_ddeg(&one("TH", &vel("0.0", "0.0", "KNOTS")).ev, 0),
        None
    );
    err_field(&one("TH", &vel("1", "2", "FURLONGS")), Field::Velocity);
    err_field(&one("TH", &vel("2001", "2", "KNOTS")), Field::Velocity);
    // Both components are needed.
    let half = r#"<enRoute><position><trackVelocity><x uom="KNOTS">5.0</x></trackVelocity></position></enRoute>"#;
    err_field(&one("TH", half), Field::Velocity);
    assert_eq!(track_ddeg(&one("TH", half).ev, 0), None);
}

// REQ: OA-FIXM-007
#[test]
fn whole_message_rejection_is_counted_by_reason() {
    let ok = message("TH", "");
    let cases: Vec<(String, Reject)> = vec![
        (message_with(&format!(r#"source="TH" timestamp="{TS}""#), ""), Reject::MissingGufi),
        (message_with(&format!(r#"source="TH" timestamp="{TS}""#), r#"<gufi codeSpace="urn:uuid"/>"#), Reject::MissingGufi),
        (message_with(&format!(r#"source="TH" timestamp="{TS}""#), &gufi_el("not-a-uuid")), Reject::BadGufi),
        (message_with(&format!(r#"source="TH" timestamp="{TS}""#), r#"<gufi codeSpace="other">11111111-2222-3333-4444-555555555555</gufi>"#), Reject::BadGufi),
        (message_with(&format!(r#"source="TH" timestamp="{TS}""#), &format!("{}{}", gufi_el(GUFI), gufi_el(GUFI))), Reject::BadGufi),
        (message_with(r#"source="TH""#, &gufi_el(GUFI)), Reject::MissingTimestamp),
        (message_with(r#"source="TH" timestamp="2030-01-02 03:04:05Z""#, &gufi_el(GUFI)), Reject::BadTimestamp),
        (message_with(r#"source="TH" timestamp="2030-01-02T03:04:05+01:00""#, &gufi_el(GUFI)), Reject::BadTimestamp),
        (
            format!(r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:OtherType"><flight source="TH" timestamp="{TS}">{}</flight></message>"#, gufi_el(GUFI)),
            Reject::NotFlightMessage,
        ),
        (
            format!(r#"<message><flight source="TH" timestamp="{TS}">{}</flight></message>"#, gufi_el(GUFI)),
            Reject::NotFlightMessage,
        ),
        (
            format!(r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight source="TH" timestamp="{TS}">{}</flight><flight source="TH" timestamp="{TS}"/></message>"#, gufi_el(GUFI)),
            Reject::DuplicateFlight,
        ),
        (r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"/>"#.to_string(), Reject::MissingGufi),
    ];
    for (bad, why) in cases {
        let r = run(&doc(&[ok.clone(), bad, ok.clone()]));
        assert_eq!(r.ev.len, 2, "{why:?}: the good messages around it stay");
        assert_eq!(
            (r.sum.messages, r.sum.rows, r.sum.rejected),
            (3, 2, 1),
            "{why:?}"
        );
        let st = r.dec.stats();
        assert_eq!(
            st.rejected[why as usize], 1,
            "{}",
            REJECT_NAMES[why as usize]
        );
        assert_eq!(st.rejected.iter().sum::<u64>(), 1);
        assert_eq!(st.rows, 2);
    }
    assert_eq!(REJECT_COUNT, REJECT_NAMES.len());
}

// REQ: OA-FIXM-008
#[test]
fn row_capacity_is_all_or_nothing_per_document() {
    let three = doc(&[message("TH", ""), message("TH", ""), message("TH", "")]);
    let (r, res) = run_with(
        &three,
        2,
        1024,
        Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    assert_eq!(res, Err(FixmError::RowCapacity { capacity: 2 }));
    assert_eq!(r.ev.len, 0);
    assert!(r.ev.gufi.is_empty() && r.ev.present.is_empty());
    let (r, res) = run_with(
        &three,
        3,
        1024,
        Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    assert_eq!(res, Ok(()));
    assert_eq!(r.ev.len, 3);
    assert_eq!(r.ev.capacity, 3);

    // Existing rows survive a refused document.
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let mut dec = Decoder::new(Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ));
    let mut ev = new_events(3, 1024);
    let one_doc = doc(&[message("TH", "")]);
    let d = parser.parse(one_doc.as_bytes()).unwrap();
    dec.decode(&d, one_doc.as_bytes(), &mut ev).unwrap();
    let d = parser.parse(three.as_bytes()).unwrap();
    assert!(matches!(
        dec.decode(&d, three.as_bytes(), &mut ev),
        Err(FixmError::RowCapacity { .. })
    ));
    assert_eq!(ev.len, 1);
    assert_eq!(ev.ts_ms.len(), 1);
    clear_events(&mut ev);
    assert_eq!(ev.len, 0);
    assert!(ev.ts_ms.is_empty());
}

// REQ: OA-FIXM-008
#[test]
fn string_table_capacity_rolls_the_whole_document_back() {
    let route = |t: &str| format!(r#"<agreed><route nasRouteText="{t}"/></agreed>"#);
    let two = doc(&[
        message("FH", &route("ABCDEFGHIJ")),
        message("FH", &route("KLMNOPQRST")),
    ]);
    let (r, res) = run_with(
        &two,
        8,
        15,
        Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    assert_eq!(res, Err(FixmError::StringCapacity { capacity: 15 }));
    assert_eq!(r.ev.len, 0);
    assert_eq!(r.ev.strings_used, 0);
    let (r, res) = run_with(
        &two,
        8,
        20,
        Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    assert_eq!(res, Ok(()));
    assert_eq!(r.ev.strings_used, 20);
    assert_eq!(route_text(&r.ev, 0), "ABCDEFGHIJ");
    assert_eq!(route_text(&r.ev, 1), "KLMNOPQRST");
    // A route is stored per row at its own offset.
    assert_eq!((r.ev.route_off[0], r.ev.route_off[1]), (0, 10));
}

// REQ: OA-FIXM-015
#[test]
fn route_text_is_bounded_and_entities_are_decoded() {
    let route = |t: &str| format!(r#"<agreed><route nasRouteText="{t}"/></agreed>"#);
    let limits = Limits {
        max_route_len: 8,
        ..Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz())
    };
    let (r, res) = run_with(&doc(&[message("FH", &route("12345678"))]), 4, 64, limits);
    res.unwrap();
    assert_eq!(route_text(&r.ev, 0), "12345678");
    let (r, res) = run_with(&doc(&[message("FH", &route("123456789"))]), 4, 64, limits);
    res.unwrap();
    assert!(field_error(&r.ev, 0, Field::Route));
    assert_eq!(route_text(&r.ev, 0), "");
    assert_eq!(r.ev.strings_used, 0, "a refused route takes no table space");
    // Decoded length is what counts, and it is decoded.
    let (r, res) = run_with(
        &doc(&[message("FH", &route("A&amp;B&#x2F;C"))]),
        4,
        64,
        limits,
    );
    res.unwrap();
    assert_eq!(route_text(&r.ev, 0), "A&B/C");
    let (r, res) = run_with(
        &doc(&[message(
            "FH",
            &route("&amp;&amp;&amp;&amp;&amp;&amp;&amp;&amp;&amp;"),
        )]),
        4,
        64,
        limits,
    );
    res.unwrap();
    assert!(field_error(&r.ev, 0, Field::Route));
    let (r, res) = run_with(&doc(&[message("FH", &route(""))]), 4, 64, limits);
    res.unwrap();
    assert!(field_error(&r.ev, 0, Field::Route));
}

// REQ: OA-FIXM-009
#[test]
fn unknown_elements_and_attributes_are_counted_per_path() {
    let body = r#"<flightStatus fdpsFlightStatus="ACTIVE" newAttr="x"/>
<newThing><inner/><inner/></newThing>
<enRoute><newLeaf>1</newLeaf></enRoute>
<enRoute><newLeaf>2</newLeaf></enRoute>
<supplementalData><additionalFlightInformation><nameValue name="BRAND_NEW" value="1"/><nameValue name="BRAND_NEW" value="2"/><nameValue name="MSG_SEQ_NO" value="7"/></additionalFlightInformation></supplementalData>"#;
    let r = run(&doc(&[message("TH", body), message("TH", body)]));
    assert_eq!(r.ev.len, 2, "unknown content never refuses a message");
    assert_eq!(r.sum.field_errors, 0);
    let st = r.dec.stats();
    let get = |t: &str| {
        st.unknown
            .iter()
            .find(|u| unknown_text(u) == t)
            .map(|u| (u.count, u.is_attr))
    };
    assert_eq!(get("message/flight/newThing"), Some((2, false)));
    // Only the topmost element of an unknown subtree is counted.
    assert_eq!(get("message/flight/newThing/inner"), None);
    assert_eq!(get("message/flight/enRoute/newLeaf"), Some((4, false)));
    assert_eq!(get("message/flight/flightStatus@newAttr"), Some((2, true)));
    assert_eq!(
        get("message/flight/supplementalData/additionalFlightInformation/nameValue[BRAND_NEW]"),
        Some((4, false))
    );
    assert_eq!(st.unknown_attributes, 2);
    assert_eq!(st.unknown_elements, 2 + 4 + 4);
    assert_eq!(r.sum.unknown_attributes, 2);
    assert_eq!(r.sum.unknown_elements, 10);
    assert_eq!(st.unknown.len(), 4);
    assert_eq!(st.unknown_overflow, 0);
    assert_eq!(r.ev.msg_seq[0], 7);
}

// REQ: OA-FIXM-009
#[test]
fn unknown_path_table_is_bounded() {
    let body = r#"<a1/><a2/><a3/><a4/><a1/>"#;
    let limits = Limits {
        max_unknown_paths: 2,
        ..Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz())
    };
    let (r, res) = run_with(&doc(&[message("TH", body)]), 4, 64, limits);
    res.unwrap();
    let st = r.dec.stats();
    assert_eq!(st.unknown.len(), 2);
    assert_eq!(st.unknown_overflow, 2);
    assert_eq!(st.unknown_elements, 5);
    // A path already in the table keeps counting past the bound.
    assert_eq!(st.unknown[0].count, 2);
}

// REQ: OA-FIXM-009
#[test]
fn namespaced_and_foreign_elements_are_unknown_not_decoded() {
    // `ns2:gufi` is not the FAA `gufi`; a default namespace changes names too.
    let msg = format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight source="TH" timestamp="{TS}">{}<ns2:flightStatus fdpsFlightStatus="ACTIVE"/><x xmlns="urn:made:up"><flightStatus fdpsFlightStatus="ACTIVE"/></x></flight><other/></message>"#,
        gufi_el(GUFI)
    );
    let r = run(&doc(&[msg]));
    assert_eq!(r.ev.len, 1);
    assert!(!has_field(&r.ev, 0, Field::Status));
    let st = r.dec.stats();
    let texts: Vec<_> = st
        .unknown
        .iter()
        .map(|u| unknown_text(u).to_string())
        .collect();
    assert!(texts.contains(&"message/flight/flightStatus".to_string()));
    assert!(texts.contains(&"message/flight/x".to_string()));
    assert!(texts.contains(&"message/other".to_string()));
    assert_eq!(st.unknown_elements, 3);
    // An attribute in an unrecognised namespace is unknown; the XML Schema instance ones are not.
    let msg = format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:q="urn:made:up" xsi:type="ns5:FlightMessageType"><flight q:extra="1" xsi:nil="false" source="TH" timestamp="{TS}">{}</flight></message>"#,
        gufi_el(GUFI)
    );
    let r = run(&doc(&[msg]));
    assert_eq!(r.dec.stats().unknown_attributes, 1);
    assert_eq!(r.dec.stats().unknown_elements, 0);
}

// REQ: OA-FIXM-010
#[test]
fn documents_that_are_not_sfdps_message_collections_are_refused() {
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let mut dec = Decoder::new(Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ));
    let mut ev = new_events(8, 64);
    for xml in [
        r#"<FDPSMsg xmlns="us:gov:dot:faa:atm:enroute:entities:flightdata"><center>ZXX</center></FDPSMsg>"#,
        r#"<ns7:AIXMBasicMessage xmlns:ns7="http://www.aixm.aero/schema/5.1/message"/>"#,
        r#"<ns5:MessageCollection xmlns:ns5="http://example.invalid/other"/>"#,
        r#"<MessageCollection/>"#,
        r#"<ns5:Other xmlns:ns5="http://www.faa.aero/nas/3.0"/>"#,
    ] {
        let d = parser.parse(xml.as_bytes()).unwrap();
        assert_eq!(
            dec.decode(&d, xml.as_bytes(), &mut ev),
            Err(FixmError::UnsupportedRoot),
            "{xml}"
        );
    }
    assert_eq!(dec.stats().unsupported_roots, 5);
    assert_eq!(ev.len, 0);
    // An empty collection is valid and yields nothing.
    let xml = doc(&[]);
    let d = parser.parse(xml.as_bytes()).unwrap();
    assert_eq!(
        dec.decode(&d, xml.as_bytes(), &mut ev).unwrap(),
        Summary::default()
    );
}

// REQ: OA-FIXM-010
#[test]
fn element_limit_and_input_mismatch_are_errors() {
    let xml = doc(&[message("TH", "")]);
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let d = parser.parse(xml.as_bytes()).unwrap();
    let n = d.elem_name.len();
    let mut ev = new_events(4, 64);
    let mut dec = Decoder::new(Limits {
        max_elements: n - 1,
        ..Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz())
    });
    assert_eq!(
        dec.decode(&d, xml.as_bytes(), &mut ev),
        Err(FixmError::TooManyElements {
            elements: n,
            limit: n - 1
        })
    );
    let mut dec = Decoder::new(Limits {
        max_elements: n,
        ..Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz())
    });
    assert!(dec.decode(&d, xml.as_bytes(), &mut ev).is_ok());
    let copy = xml.clone();
    assert_eq!(
        dec.decode(&d, copy.as_bytes(), &mut ev),
        Err(FixmError::InputMismatch)
    );
    assert_eq!(ev.len, 1);
}

// REQ: OA-FIXM-011
#[test]
fn duplicated_single_valued_fields_are_errors() {
    for (dup, f) in [
        (
            r#"<flightIdentification computerId="1" aircraftIdentification="A1"/><flightIdentification computerId="1" aircraftIdentification="A1"/>"#,
            Field::Callsign,
        ),
        (
            r#"<departure departurePoint="KAAA"/><departure departurePoint="KAAA"/>"#,
            Field::Departure,
        ),
        (
            r#"<assignedAltitude><simple uom="FEET">100</simple></assignedAltitude><assignedAltitude><simple uom="FEET">100</simple></assignedAltitude>"#,
            Field::AssignedAlt,
        ),
        (
            r#"<enRoute><position><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>1 2</pos></location></position></position><position><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>1 2</pos></location></position></position></enRoute>"#,
            Field::Position,
        ),
        (
            r#"<agreed><route nasRouteText="AAA"/><route nasRouteText="AAA"/></agreed>"#,
            Field::Route,
        ),
        (
            r#"<enRoute><position><trackVelocity><x uom="KNOTS">1</x><x uom="KNOTS">1</x><y uom="KNOTS">1</y></trackVelocity></position></enRoute>"#,
            Field::Velocity,
        ),
    ] {
        let r = one("TH", dup);
        err_field(&r, f);
    }
}

// REQ: OA-FIXM-012
#[test]
fn mutated_fixtures_never_panic() {
    let seed = doc(&[
        message_with(
            &format!(
                r#"centre="ZXX" source="TH" system="SLC" timestamp="{TS}" flightType="SCHEDULED""#
            ),
            &format!("{}{TH_BODY}", gufi_el(GUFI)),
        ),
        message(
            "FH",
            r#"<agreed><route nasRouteText="KAAA.A1..KBBB"/></agreed><assignedAltitude><vfrPlus uom="FEET">5500.0</vfrPlus></assignedAltitude>"#,
        ),
    ]);
    let bytes = seed.as_bytes();
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let mut dec = Decoder::new(Limits {
        max_unknown_paths: 8,
        ..Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz())
    });
    let mut ev = new_events(4, 64);
    let mut docs = 0usize;
    let mut decoded_rows = 0usize;
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let fills: [&[u8]; 9] = [b"<", b">", b"\"", b"&", b"0", b"-", b"9", b" ", b"/"];
    let mut try_one = |m: &[u8], dec: &mut Decoder, ev: &mut Events| {
        let Ok(d) = parser.parse(m) else { return };
        docs += 1;
        if let Ok(s) = dec.decode(&d, m, ev) {
            assert!(s.rows as usize <= s.messages as usize);
            assert!(ev.len <= ev.capacity);
            assert!(ev.strings_used <= ev.strings.len());
            for row in 0..ev.len {
                // Every present column is within its model range.
                if has_field(ev, row, Field::Position) {
                    assert!(ev.lat_e7[row].abs() <= 900_000_000);
                    assert!(ev.lon_e7[row].abs() <= 1_800_000_000);
                }
                if has_field(ev, row, Field::AssignedAlt) {
                    assert!((-2000..=100_000).contains(&ev.assigned_ft[row]));
                }
                let _ = route_text(ev, row);
                let _ = track_ddeg(ev, row);
                assert_eq!(ev.present[row] & ev.errors[row], 0);
            }
            decoded_rows += ev.len;
        }
        clear_events(ev);
    };
    for pos in 0..bytes.len() {
        // Deletion, duplication, truncation, and a replacement by each filler.
        let mut m = bytes.to_vec();
        m.remove(pos);
        try_one(&m, &mut dec, &mut ev);
        let mut m = bytes.to_vec();
        m.insert(pos, bytes[pos]);
        try_one(&m, &mut dec, &mut ev);
        try_one(&bytes[..pos], &mut dec, &mut ev);
        for f in fills {
            let mut m = bytes.to_vec();
            m[pos] = f[0];
            try_one(&m, &mut dec, &mut ev);
        }
    }
    // Random multi-byte damage.
    for _ in 0..4000 {
        let mut m = bytes.to_vec();
        for _ in 0..1 + next() % 4 {
            let p = (next() as usize) % m.len();
            m[p] = (next() & 0x7f) as u8;
        }
        try_one(&m, &mut dec, &mut ev);
    }
    assert!(docs > 5000, "most mutants still parse as XML ({docs})");
    assert!(decoded_rows > 1000);
    let st = dec.stats();
    // Messages of a document refused part-way for capacity are counted as seen, not as rows.
    assert!(st.rows + st.rejected.iter().sum::<u64>() <= st.messages);
}

// REQ: OA-FIXM-013
#[test]
fn decoder_is_reusable_and_does_not_grow() {
    let big = doc(&[message("TH", TH_BODY), message("FH", TH_BODY)]);
    let small = doc(&[message("BA", "")]);
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let mut dec = Decoder::new(Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ));
    let mut ev = new_events(8, 4096);
    let caps = |e: &Events| {
        (
            e.present.capacity(),
            e.gufi.capacity(),
            e.operator.capacity(),
            e.strings.capacity(),
        )
    };
    let before = caps(&ev);
    let ucap = dec.stats().unknown.capacity();
    let d = parser.parse(big.as_bytes()).unwrap();
    dec.decode(&d, big.as_bytes(), &mut ev).unwrap();
    assert_eq!(ev.len, 2);
    clear_events(&mut ev);
    // Nothing from the larger document leaks into the smaller one.
    let d = parser.parse(small.as_bytes()).unwrap();
    let s = dec.decode(&d, small.as_bytes(), &mut ev).unwrap();
    assert_eq!(s.rows, 1);
    assert_eq!(
        ev.present[0],
        (1 << Field::Source as u32) | (1 << Field::Centre as u32)
    );
    assert_eq!(ev.errors[0], 0);
    assert!(!has_field(&ev, 0, Field::Position));
    assert_eq!(caps(&ev), before);
    assert_eq!(dec.stats().unknown.capacity(), ucap);
    // After a failed decode the decoder still works.
    let mut full = new_events(1, 64);
    let d = parser.parse(big.as_bytes()).unwrap();
    assert!(dec.decode(&d, big.as_bytes(), &mut full).is_err());
    let d = parser.parse(small.as_bytes()).unwrap();
    assert_eq!(dec.decode(&d, small.as_bytes(), &mut full).unwrap().rows, 1);
    assert_eq!(dec.stats().docs, 4);
}

// REQ: OA-FIXM-016
#[test]
fn field_tables_agree() {
    assert_eq!(ALL_FIELDS.len(), FIELD_COUNT);
    assert_eq!(FIELD_NAMES.len(), FIELD_COUNT);
    for (i, f) in ALL_FIELDS.iter().enumerate() {
        assert_eq!(*f as usize, i);
    }
    let mut names: Vec<_> = FIELD_NAMES.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), FIELD_COUNT);
    assert_eq!(code_str(b"AB\0\0"), "AB");
    assert_eq!(code_str(b"ABCD"), "ABCD");
    assert_eq!(code_str(b"\0\0"), "");
}

// REQ: OA-FIXM-015
#[test]
fn route_attribute_whitespace_is_normalised() {
    let route = |t: &str| format!(r#"<agreed><route nasRouteText="{t}"/></agreed>"#);
    // A literal tab or line end in an attribute value reads as one space.
    let (r, res) = run_with(
        &doc(&[message("FH", &route("AA\tBB\r\nCC"))]),
        4,
        64,
        Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    res.unwrap();
    assert_eq!(route_text(&r.ev, 0), "AA BB CC");
}

fn decode_prefix_run(xml: &str, cap: usize) -> (Summary, usize) {
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let d = parser.parse(xml.as_bytes()).expect("fixture parses");
    let mut dec = Decoder::new(Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ));
    let mut ev = new_events(cap, 1 << 16);
    let s = dec.decode_prefix(&d, xml.as_bytes(), &mut ev).unwrap();
    (s, ev.len)
}

// REQ: OA-FIXM-019
#[test]
fn xsi_type_is_resolved_to_the_fixm_namespace() {
    let typed = |decl: &str, ty: &str| {
        format!(
            r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" {decl} xsi:type="{ty}"><flight source="TH" timestamp="{TS}">{}</flight></message>"#,
            gufi_el(GUFI)
        )
    };
    let rejected = |m: String| {
        let r = run(&doc(&[m]));
        assert_eq!((r.sum.rejected, r.sum.rows), (1, 0));
        assert_eq!(r.dec.stats().rejected[Reject::NotFlightMessage as usize], 1);
    };
    let accepted = |m: String| {
        let r = run(&doc(&[m]));
        assert_eq!((r.sum.rejected, r.sum.rows), (0, 1));
    };
    // A foreign namespace under the right local name.
    rejected(typed(r#"xmlns:evil="urn:other""#, "evil:FlightMessageType"));
    // A prefix that is not declared.
    rejected(typed("", "nope:FlightMessageType"));
    // No prefix and no default namespace.
    rejected(typed("", "FlightMessageType"));
    // Another prefix bound to the right namespace, plain or escaped.
    accepted(typed(
        r#"xmlns:zz="http://www.faa.aero/nas/3.0""#,
        "zz:FlightMessageType",
    ));
    accepted(typed(
        r#"xmlns:zz="http://www.faa.aero/nas/3&#46;0""#,
        "zz:FlightMessageType",
    ));
}

// REQ: OA-FIXM-019
#[test]
fn escaped_namespace_uris_decode_like_plain_ones() {
    let plain = doc(&[message("TH", "")]);
    let escaped = plain
        .replace("faa.aero/nas", "faa&#46;aero/nas")
        .replace("fixm.aero", "fixm&#46;aero")
        .replace("XMLSchema-instance", "XMLSchema&#45;instance");
    assert_ne!(plain, escaped);
    let a = run(&plain);
    let b = run(&escaped);
    assert_eq!(a.sum, b.sum);
    assert_eq!(b.sum.rows, 1);
    assert_eq!(b.sum.unknown_elements, 0);
}

// REQ: OA-FIXM-020
#[test]
fn foreign_siblings_do_not_count_as_messages() {
    let foreign = r#"<other xmlns="urn:other"><x/></other>"#.to_string();
    let xml = doc(&[message("TH", ""), foreign.clone()]);
    // Capacity for exactly one row: the foreign sibling needs none.
    let (r, res) = run_with(
        &xml,
        1,
        1 << 16,
        Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    res.expect("one message fits one row");
    assert_eq!((r.sum.messages, r.sum.rows), (1, 1));
    // Prefix mode with no capacity: the one real message is undecoded, the sibling is not counted.
    let (s, len) = decode_prefix_run(&xml, 0);
    assert_eq!((s.not_decoded, s.rows, len), (1, 0, 0));
    // Only foreign children: nothing to hold, nothing undecoded.
    let (s, _) = decode_prefix_run(&doc(&[foreign]), 0);
    assert_eq!(s.not_decoded, 0);
}

// REQ: OA-FIXM-022
#[test]
fn assigned_altitude_forms_are_states_not_numbers() {
    let simple = one(
        "TH",
        r#"<assignedAltitude><simple uom="FEET">36000.0</simple></assignedAltitude>"#,
    );
    assert_eq!(simple.ev.assigned_form[0], ALT_FORM_SIMPLE);
    assert_eq!(simple.ev.assigned_ft[0], 36_000);

    let plus = one(
        "TH",
        r#"<assignedAltitude><vfrPlus uom="FEET">5500.0</vfrPlus></assignedAltitude>"#,
    );
    assert_eq!(plus.ev.assigned_form[0], ALT_FORM_VFR_PLUS);
    assert_eq!(plus.ev.assigned_ft[0], 5500);

    let vfr = one("TH", r#"<assignedAltitude><vfr/></assignedAltitude>"#);
    assert_eq!(vfr.ev.assigned_form[0], ALT_FORM_VFR);
    assert!(has_field(&vfr.ev, 0, Field::AssignedForm));
    assert!(
        !has_field(&vfr.ev, 0, Field::AssignedAlt),
        "VFR is not a number"
    );

    let block = one(
        "TH",
        r#"<assignedAltitude><block><above uom="FEET">45000.0</above><below uom="FEET">47000.0</below></block></assignedAltitude>"#,
    );
    assert_eq!(block.ev.assigned_form[0], ALT_FORM_BLOCK);
    assert_eq!(
        (block.ev.block_above_ft[0], block.ev.block_below_ft[0]),
        (45_000, 47_000)
    );
    assert!(!has_field(&block.ev, 0, Field::AssignedAlt));
    assert_eq!(block.sum.unknown_elements, 0);
    assert_eq!(block.sum.unknown_attributes, 0);

    let fix = one(
        "TH",
        r#"<assignedAltitude><altFixAlt><point xsi:type="ns2:FixPointType" fix="ABCDE"/><post uom="FEET">7000.0</post><pre uom="FEET">22000.0</pre></altFixAlt></assignedAltitude>"#,
    );
    assert_eq!(fix.ev.assigned_form[0], ALT_FORM_AT_FIX);
    assert_eq!(
        (fix.ev.fixalt_pre_ft[0], fix.ev.fixalt_post_ft[0]),
        (22_000, 7000)
    );
    assert_eq!(code_str(&fix.ev.fixalt_point[0]), "ABCDE");
    assert_eq!(fix.sum.unknown_elements, 0);
}

// REQ: OA-FIXM-023
#[test]
fn a_bad_or_doubled_altitude_form_is_a_field_error() {
    // Two forms of one altitude: the single one meant cannot be known.
    let two = one(
        "TH",
        r#"<assignedAltitude><simple uom="FEET">100</simple><vfr/></assignedAltitude>"#,
    );
    err_field_n(&two, Field::AssignedForm);
    // A block whose limits are the wrong way round, and one with a limit missing.
    let swapped = one(
        "TH",
        r#"<assignedAltitude><block><above uom="FEET">47000.0</above><below uom="FEET">45000.0</below></block></assignedAltitude>"#,
    );
    err_field_n(&swapped, Field::AssignedForm);
    let half = one(
        "TH",
        r#"<assignedAltitude><block><above uom="FEET">45000.0</above></block></assignedAltitude>"#,
    );
    err_field_n(&half, Field::AssignedForm);
    // An altitude at a fix with no fix, and one with an unreadable value.
    let nofix = one(
        "TH",
        r#"<assignedAltitude><altFixAlt><post uom="FEET">7000.0</post><pre uom="FEET">22000.0</pre></altFixAlt></assignedAltitude>"#,
    );
    err_field_n(&nofix, Field::AssignedForm);
    let junk = one(
        "TH",
        r#"<assignedAltitude><altFixAlt><point fix="ABCDE"/><post uom="FEET">x</post><pre uom="FEET">22000.0</pre></altFixAlt></assignedAltitude>"#,
    );
    err_field_n(&junk, Field::AssignedForm);
    // A simple altitude that does not parse has no form either.
    let bad = one(
        "TH",
        r#"<assignedAltitude><simple uom="FEET">999999</simple></assignedAltitude>"#,
    );
    err_field_n(&bad, Field::AssignedForm);
    err_field_n(&bad, Field::AssignedAlt);
}

fn err_field_n(r: &Run, f: Field) {
    assert!(
        field_error(&r.ev, 0, f),
        "{} should be an error",
        FIELD_NAMES[f as usize]
    );
    assert!(
        !has_field(&r.ev, 0, f),
        "{} should be absent",
        FIELD_NAMES[f as usize]
    );
}

// REQ: OA-FIXM-024
#[test]
fn interim_altitude_is_a_value_or_an_explicit_nil() {
    let v = one(
        "LH",
        r#"<interimAltitude uom="FEET">23000.0</interimAltitude>"#,
    );
    assert!(has_field(&v.ev, 0, Field::InterimAlt));
    assert_eq!(v.ev.interim_ft[0], 23_000);
    assert_eq!(v.ev.flags[0] & FLAG_INTERIM_NIL, 0);
    assert_eq!(v.sum.unknown_attributes, 0);

    let nil = one("LH", r#"<interimAltitude uom="FEET" xsi:nil="true"/>"#);
    assert!(has_field(&nil.ev, 0, Field::InterimAlt));
    assert_ne!(nil.ev.flags[0] & FLAG_INTERIM_NIL, 0);
    assert_eq!(nil.sum.unknown_attributes, 0);
    assert_eq!(nil.sum.field_errors, 0);

    // Not stated is not nil: nothing is present.
    let none = one("TH", r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#);
    assert!(!has_field(&none.ev, 0, Field::InterimAlt));
    assert!(!field_error(&none.ev, 0, Field::InterimAlt));

    // An empty element with no nil, a nil that is not a boolean, and a doubled element.
    err_field_n(
        &one("LH", r#"<interimAltitude uom="FEET"/>"#),
        Field::InterimAlt,
    );
    err_field_n(
        &one("LH", r#"<interimAltitude uom="FEET" xsi:nil="maybe"/>"#),
        Field::InterimAlt,
    );
    err_field_n(
        &one(
            "LH",
            r#"<interimAltitude uom="FEET">1000</interimAltitude><interimAltitude uom="FEET" xsi:nil="true"/>"#,
        ),
        Field::InterimAlt,
    );
}

// REQ: OA-FIXM-025
#[test]
fn requested_altitude_forms() {
    let s = one(
        "FH",
        r#"<requestedAltitude><simple uom="FEET">35000.0</simple></requestedAltitude>"#,
    );
    assert_eq!(s.ev.requested_form[0], ALT_FORM_SIMPLE);
    assert_eq!(s.ev.requested_ft[0], 35_000);
    let p = one(
        "FH",
        r#"<requestedAltitude><vfrPlus uom="FEET">5500.0</vfrPlus></requestedAltitude>"#,
    );
    assert_eq!(p.ev.requested_form[0], ALT_FORM_VFR_PLUS);
    assert_eq!(p.ev.requested_ft[0], 5500);
    let v = one("FH", r#"<requestedAltitude><vfr/></requestedAltitude>"#);
    assert_eq!(v.ev.requested_form[0], ALT_FORM_VFR);
    assert_eq!(v.sum.unknown_elements, 0, "the vfr form is a known path");
    err_field_n(
        &one(
            "FH",
            r#"<requestedAltitude><simple uom="FEET">100</simple><vfr/></requestedAltitude>"#,
        ),
        Field::RequestedForm,
    );
    err_field_n(
        &one(
            "FH",
            r#"<requestedAltitude><simple uom="FEET">x</simple></requestedAltitude>"#,
        ),
        Field::RequestedForm,
    );
    // The requested altitude does not disturb the assigned one.
    let both = one(
        "FH",
        r#"<assignedAltitude><simple uom="FEET">10000</simple></assignedAltitude><requestedAltitude><simple uom="FEET">35000.0</simple></requestedAltitude>"#,
    );
    assert_eq!(both.ev.assigned_ft[0], 10_000);
    assert_eq!(both.ev.requested_ft[0], 35_000);
}

// REQ: OA-FIXM-026
#[test]
fn handoff_event_and_accepting_unit() {
    let units = r#"<receivingUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZYY" sectorIdentifier="1E"/><transferringUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZXX" sectorIdentifier="12"/><acceptingUnit xsi:type="ns2:IdentifiedUnitReferenceType" unitIdentifier="ZYY" sectorIdentifier="1E"/>"#;
    for (name, code) in [
        ("INITIATION", oa_fixm::HANDOFF_INITIATION),
        ("ACCEPTANCE", oa_fixm::HANDOFF_ACCEPTANCE),
        ("UPDATE", oa_fixm::HANDOFF_UPDATE),
        ("RETRACTION", oa_fixm::HANDOFF_RETRACTION),
        ("FAILURE", oa_fixm::HANDOFF_FAILURE),
        ("TAKE_CONTROL", oa_fixm::HANDOFF_TAKE_CONTROL),
    ] {
        let body = format!(
            r#"<enRoute><boundaryCrossings><handoff xsi:type="ns5:NasHandoffType" event="{name}">{units}</handoff></boundaryCrossings></enRoute>"#
        );
        let r = one("OH", &body);
        assert_eq!(r.ev.handoff_event[0], code, "{name}");
        assert_eq!(unit_code(&r.ev.acpt[0]), "ZYY");
        assert_eq!(sector_code(&r.ev.acpt[0]), "1E");
        assert_eq!(
            r.sum.unknown_elements + r.sum.unknown_attributes,
            0,
            "{name}"
        );
    }
    // A handoff with no event (the track-message form) has no event and no accepting unit.
    let r = one(
        "TH",
        r#"<enRoute><boundaryCrossings><handoff><receivingUnit unitIdentifier="ZYY" sectorIdentifier="03"/></handoff></boundaryCrossings></enRoute>"#,
    );
    assert!(!has_field(&r.ev, 0, Field::HandoffEvent));
    assert!(!field_error(&r.ev, 0, Field::HandoffEvent));
    assert!(!has_field(&r.ev, 0, Field::HandoffAccepting));
    assert!(has_field(&r.ev, 0, Field::HandoffReceiving));
    // An event this decoder does not know is a field error, not a default.
    let r = one(
        "OH",
        r#"<enRoute><boundaryCrossings><handoff event="OFFER"><receivingUnit unitIdentifier="ZYY"/></handoff></boundaryCrossings></enRoute>"#,
    );
    err_field_n(&r, Field::HandoffEvent);
    assert!(
        has_field(&r.ev, 0, Field::HandoffReceiving),
        "the rest of the row is kept"
    );
}

// REQ: OA-FIXM-027
#[test]
fn aircraft_address_is_six_hex_digits() {
    let desc = |a: &str| {
        format!(
            r#"<aircraftDescription aircraftAddress="{a}" wakeTurbulence="MEDIUM"><aircraftType><icaoModelIdentifier>B738</icaoModelIdentifier></aircraftType></aircraftDescription>"#
        )
    };
    let r = one("FH", &desc("A1B2C3"));
    assert_eq!(r.ev.address[0], 0xA1_B2_C3);
    let r = one("FH", &desc("a1b2c3"));
    assert_eq!(r.ev.address[0], 0xA1_B2_C3);
    let r = one("FH", &desc("000042"));
    assert_eq!(r.ev.address[0], 0x42);
    for bad in ["", "A1B2C", "A1B2C3D", "G1B2C3", "000000", "A1 2C3"] {
        err_field_n(&one("FH", &desc(bad)), Field::AircraftAddress);
    }
    // No address attribute, no field and no error.
    let r = one("FH", r#"<aircraftDescription wakeTurbulence="MEDIUM"/>"#);
    assert!(!has_field(&r.ev, 0, Field::AircraftAddress));
    assert!(!field_error(&r.ev, 0, Field::AircraftAddress));
}

/// 2026-11-20T00:00:00Z and 2026-11-23T00:17:00Z in Unix milliseconds: the sealed window.
const DROP: (i64, i64) = (1_795_132_800_000, 1_795_393_020_000);

fn at(ts: &str, body: &str) -> String {
    message_with(
        &format!(r#"centre="ZXX" source="TH" system="SLC" timestamp="{ts}""#),
        &format!("{}{body}", gufi_el(GUFI)),
    )
}

/// A decoder's stats as they are before it has seen anything, with the drop count set.
fn untouched(limits: Limits, dropped: u64) -> oa_fixm::Stats {
    let mut s = Decoder::new(limits).stats().clone();
    s.sealed_window_dropped = dropped;
    s
}

fn windowed() -> Limits {
    Limits::new(oa_sealed::DropWindow::new(DROP.0, DROP.1))
}

// REQ: OA-SEAL-014
#[test]
fn a_document_with_a_message_in_the_window_is_dropped_whole_and_every_counter_is_as_for_an_empty_run(
) {
    let unknown = r#"<flightStatus fdpsFlightStatus="ACTIVE" newAttr="x"/><newThing><inner/></newThing><supplementalData><additionalFlightInformation><nameValue name="BRAND_NEW" value="1"/></additionalFlightInformation></supplementalData>"#;
    let in_pos =
        format!(r#"{unknown}<enRoute><position positionTime="2026-11-22T00:00:00Z"/></enRoute>"#);
    let nogufi = message_with(
        r#"centre="ZXX" source="TH" system="SLC" timestamp="2026-11-21T00:00:00Z""#,
        unknown,
    );
    // Kept messages around: one timed in the window by its timestamp, one by its position time,
    // one refused (no GUFI) and in the window; the rest, with unknown paths, are outside it.
    let all = doc(&[
        at("2030-01-02T03:04:05Z", unknown),
        at("2026-11-21T01:00:00Z", unknown),
        at("2026-11-19T23:59:59Z", &in_pos),
        nogufi,
        at("2026-11-23T00:17:00Z", unknown),
    ]);
    let (r, res) = run_with(&all, 64, 1 << 16, windowed());
    res.unwrap();
    // Dropped whole: the count is the five messages of the document, and nothing else moved.
    assert_eq!(
        r.sum,
        Summary {
            sealed_window_dropped: 5,
            ..Summary::default()
        }
    );
    assert_eq!(r.dec.stats(), &untouched(windowed(), 5));
    assert_eq!((r.ev.len, r.ev.strings_used), (0, 0));
    // Without a message in the window the same kept messages are all decoded as usual.
    let kept = doc(&[
        at("2030-01-02T03:04:05Z", unknown),
        at("2026-11-23T00:17:00Z", unknown),
    ]);
    let (r, res) = run_with(&kept, 64, 1 << 16, windowed());
    res.unwrap();
    assert_eq!(
        (r.sum.sealed_window_dropped, r.sum.messages, r.ev.len),
        (0, 2, 2)
    );
}

// REQ: OA-SEAL-014
#[test]
fn an_in_window_message_after_a_full_table_or_a_prefix_cut_is_still_found() {
    let m = |ts: &str| at(ts, "");
    // Three messages, room for one row: a plain decode would be refused for capacity, and a
    // prefix decode would stop at the second. The in-window third message is read first.
    let xml = doc(&[
        m("2030-01-02T03:04:05Z"),
        m("2030-01-02T03:04:06Z"),
        m("2026-11-21T00:00:00Z"),
    ]);
    for prefix in [false, true] {
        let mut parser = Parser::new(XmlLimits::default()).unwrap();
        let d = parser.parse(xml.as_bytes()).unwrap();
        let mut dec = Decoder::new(windowed());
        let mut ev = new_events(1, 1 << 12);
        let r = if prefix {
            dec.decode_prefix(&d, xml.as_bytes(), &mut ev)
        } else {
            dec.decode(&d, xml.as_bytes(), &mut ev)
        };
        assert_eq!(
            r.unwrap(),
            Summary {
                sealed_window_dropped: 3,
                ..Summary::default()
            },
            "prefix {prefix}"
        );
        assert_eq!(dec.stats(), &untouched(windowed(), 3));
        assert_eq!(ev.len, 0);
    }
}

// REQ: OA-SEAL-014
#[test]
fn a_message_with_an_invalid_field_before_its_time_is_still_dropped_with_its_document() {
    // An invalid GUFI and a bad attribute come before the timestamp in document order.
    let bad = message_with(
        r#"centre="ZXX" source="TH" system="SLC" bogus="1" timestamp="2026-11-21T00:00:00Z""#,
        "<gufi>not-a-uuid</gufi>",
    );
    let xml = doc(&[at("2030-01-02T03:04:05Z", "<x/>"), bad]);
    let (r, res) = run_with(&xml, 64, 1 << 16, windowed());
    res.unwrap();
    assert_eq!(r.dec.stats(), &untouched(windowed(), 2));
    assert_eq!(r.sum.sealed_window_dropped, 2);
}

// REQ: OA-SEAL-014
#[test]
fn a_window_that_holds_all_time_drops_every_document_and_the_production_list_drops_the_window() {
    let xml = doc(&[at("2030-01-02T03:04:05Z", "")]);
    // All time (the closed failure of the list).
    let all = Limits::new(oa_sealed::DropWindow::all_time());
    let (r, res) = run_with(&xml, 64, 1 << 16, all);
    res.unwrap();
    assert_eq!(
        r.sum,
        Summary {
            sealed_window_dropped: 1,
            ..Summary::default()
        }
    );
    assert_eq!(r.dec.stats(), &untouched(all, 1));
    // The production list.
    let prod = Limits::new(oa_sealed::DropWindow::sealed());
    let (r, res) = run_with(&doc(&[at("2026-11-21T00:00:00Z", "")]), 64, 1 << 16, prod);
    res.unwrap();
    assert_eq!(r.sum.sealed_window_dropped, 1);
    let (r, res) = run_with(&xml, 64, 1 << 16, prod);
    res.unwrap();
    assert_eq!((r.sum.sealed_window_dropped, r.ev.len), (0, 1));
}

// REQ: OA-SEAL-014
#[test]
fn message_and_position_times_are_window_keys_in_any_encoding_and_event_times_are_not() {
    let dropped = |xml: &str| {
        let (r, res) = run_with(xml, 64, 1 << 16, windowed());
        res.unwrap();
        r.sum.sealed_window_dropped
    };
    // Entity-encoded (the final Z as a character reference), and padded with whitespace.
    for ts in ["2026-11-21T00:00:00&#90;", " 2026-11-21T00:00:00Z "] {
        let x = doc(&[at(ts, "")]);
        // The decoder's own parser reads a padded value as no time and refuses the message; then
        // there is no row to protect, and the document is kept as refused.
        let n = dropped(&x);
        let (r, res) = run_with(&x, 64, 1 << 16, windowed());
        res.unwrap();
        assert!(n == 1 || r.ev.len == 0, "{ts:?}");
    }
    assert_eq!(dropped(&doc(&[at("2026-11-21T00:00:00&#90;", "")])), 1);
    // A repeated attribute is not well-formed XML: the document is refused, not read.
    let dup = message_with(
        r#"centre="ZXX" source="TH" system="SLC" timestamp="2030-01-02T03:04:05Z" timestamp="2026-11-21T00:00:00Z""#,
        &gufi_el(GUFI),
    );
    let mut p = oa_xml::Parser::new(oa_xml::Limits::default()).unwrap();
    assert!(p.parse(doc(&[dup]).as_bytes()).is_err());
    // A position time in the window, the message time outside it.
    let pos = at(
        "2030-01-02T03:04:05Z",
        r#"<enRoute><position positionTime="2026-11-21T00:00:00Z"/></enRoute>"#,
    );
    assert_eq!(dropped(&doc(&[pos])), 1);
    // The actual departure is a key: a message stamped after the window can report it inside.
    let dep = |ts: &str, actual: &str| {
        at(
            ts,
            &format!(
                r#"<departure xsi:type="ns5:NasDepartureType" departurePoint="KDEP"><runwayPositionAndTime><runwayTime><actual time="{actual}"/></runwayTime></runwayPositionAndTime></departure>"#
            ),
        )
    };
    assert_eq!(
        dropped(&doc(&[dep("2026-11-23T01:00:00Z", "2026-11-22T23:00:00Z")])),
        1
    );
    // Outside the window both, and an estimated arrival in the window (a reference time), are kept.
    assert_eq!(
        dropped(&doc(&[dep("2026-11-23T01:00:00Z", "2026-11-23T00:17:00Z")])),
        0
    );
    let est = at(
        "2026-11-23T01:00:00Z",
        r#"<arrival xsi:type="ns5:NasArrivalType" arrivalPoint="KTST"><runwayPositionAndTime><runwayTime><estimated time="2026-11-22T23:00:00Z"/></runwayTime></runwayPositionAndTime></arrival>"#,
    );
    assert_eq!(dropped(&doc(&[est])), 0);
}

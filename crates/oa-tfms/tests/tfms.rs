//! Handwritten-fixture tests for oa-tfms. Every identifier here is made up; none comes from the
//! real feed. Only the element structure follows the survey (docs/TFMS_SURVEY.md).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_tfms::rows::{
    accat, divert, fflag, fkind, flag, kind, prog, scope, spd_unit, status, trig, ttype, ucat,
};
use oa_tfms::scalar::{
    contains_word, dms_e7, parse_altitude, parse_bool, parse_decimal_e7, parse_digits,
    parse_letter, parse_name, parse_vocab, parse_words,
};
use oa_tfms::schema::{BODIES, FLIGHT_DATA, FLOW_BODIES, MAX_DEPTH};
use oa_tfms::{
    clear_rows, decode_bytes, default_limits, error_kind_name, field_name, flag_get, flight_at,
    flights_len, flow_at, flows_len, name_str, new_rows, new_stats, observe, parse_iso_ms,
    push_flight, push_flow, text, truncate_rows, CapKind, DocError, DocKind, Field, FieldError,
    FieldErrorKind, FlightRow, FlowRow, Outcome, Rows, TimeError, BLANK_FLIGHT, BLANK_FLOW,
    FIELD_COUNT, NO_COORD, NO_NUM, NO_STR, NO_TIME,
};
use oa_xml::Parser;

const SVC: &str = "urn:us:gov:dot:faa:atm:tfm:tfmdataservice";
const FD: &str = "urn:us:gov:dot:faa:atm:tfm:flightdata";
const FI: &str = "urn:us:gov:dot:faa:atm:tfm:flowinformation";
const FCM: &str = "urn:us:gov:dot:faa:atm:tfm:flightdatacommonmessages";
const CORE: &str = "urn:us:gov:dot:faa:atm:tfm:tfmdatacoreelements";
const FIC: &str = "urn:us:gov:dot:faa:atm:tfm:ficommonmessages";
const FIC2: &str = "urn:us:gov:dot:faa:atm:tfm:ficommonmessages2";
const FIDT: &str = "urn:us:gov:dot:faa:atm:tfm:ficommondatatypes";

/// 2030-01-02T00:00:00Z in Unix ms.
const DAY: i64 = 1_893_542_400_000;

fn t(h: i64, m: i64, s: i64) -> i64 {
    DAY + ((h * 60 + m) * 60 + s) * 1000
}

// ---- fixture builders ----------------------------------------------------------------------

/// A flight data document: `ds:` prefix and default namespace for the outputs.
fn wrap_flight(msgs: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:tfmDataService xmlns:ds="{SVC}" xmlns="{SVC}" xmlns:fdm="{FD}" xmlns:nxce="{CORE}" xmlns:nxcm="{FCM}" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="{SVC} TFMData_Service.xsd"><fltdOutput>{msgs}</fltdOutput></ds:tfmDataService>"#
    )
}

/// A flow information document: `ns5:` style prefixes.
fn wrap_fi(msgs: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:tfmDataService xmlns:ns5="{SVC}" xmlns:ns12="{FI}" xmlns:ns11="{FIC}" xmlns:ns9="{FIC2}" xmlns:ns7="{CORE}" xmlns:ns4="{FIDT}"><ns5:fiOutput>{msgs}</ns5:fiOutput></ns5:tfmDataService>"#
    )
}

const FLTD_ATTRS: &str = r#"acid="TST123" airline="TST" arrArpt="KTBB" cdmPart="false" depArpt="KTAA" fdTrigger="HCS_TRACK_MSG" flightRef="100000001" major="TST" sensitivity="A" sourceFacility="KZXX" sourceTimeStamp="2030-01-02T03:04:05Z""#;

fn fm(msg_type: &str, body: &str) -> String {
    format!(r#"<fdm:fltdMessage {FLTD_ATTRS} msgType="{msg_type}">{body}</fdm:fltdMessage>"#)
}

const QAID: &str = r#"<nxcm:qualifiedAircraftId aircraftCategory="JET" userCategory="COMMERCIAL"><nxce:aircraftId>TST123</nxce:aircraftId><nxce:computerId><nxce:facilityIdentifier>KZXX</nxce:facilityIdentifier><nxce:idNumber>123</nxce:idNumber></nxce:computerId><nxce:gufi>ZZ000001aa</nxce:gufi><nxce:igtd>2030-01-02T01:00:00Z</nxce:igtd><nxce:departurePoint><nxce:airport>KTAA</nxce:airport></nxce:departurePoint><nxce:arrivalPoint><nxce:airport>KTBB</nxce:airport></nxce:arrivalPoint></nxcm:qualifiedAircraftId>"#;

const RVSM: &str =
    r#"<nxcm:rvsmData currentCompliance="true" equipped="true" futureCompliance="false"/>"#;

fn track() -> String {
    fm(
        "trackInformation",
        &format!(
            r#"<fdm:trackInformation>{QAID}<nxcm:speed>276</nxcm:speed><nxcm:reportedAltitude><nxce:assignedAltitude><nxce:simpleAltitude>350C</nxce:simpleAltitude></nxce:assignedAltitude></nxcm:reportedAltitude><nxcm:position><nxce:latitude><nxce:latitudeDMS degrees="41" direction="NORTH" minutes="46" seconds="24"/></nxce:latitude><nxce:longitude><nxce:longitudeDMS degrees="084" direction="WEST" minutes="08" seconds="48"/></nxce:longitude></nxcm:position><nxcm:timeAtPosition>2030-01-02T03:04:00Z</nxcm:timeAtPosition><nxcm:ncsmTrackData><nxcm:arrivalFixAndTime arrTime="2030-01-02T04:00:00Z" fixName="ARRFX"/><nxcm:departureFixAndTime arrTime="2030-01-02T02:00:00Z" fixName="DEPFX"/><nxcm:eta etaType="ESTIMATED" timeValue="2030-01-02T04:30:00Z"/><nxcm:nextEvent latitudeDecimal="41.9" longitudeDecimal="-84.5"/>{RVSM}</nxcm:ncsmTrackData></fdm:trackInformation>"#
        ),
    )
}

const ROUTE_DATA: &str = r#"<nxcm:ncsmRouteData><nxcm:arrivalFixAndTime arrTime="2030-01-02T04:00:00Z" fixName="ARRFX"/><nxcm:departureFixAndTime arrTime="2030-01-02T02:00:00Z" fixName="DEPFX"/><nxcm:diversionIndicator>NO_DIVERSION</nxcm:diversionIndicator><nxcm:dp routeName="DPTST1" routeType="DIRECT"/><nxcm:dpTransitionFix>TRFIX</nxcm:dpTransitionFix><nxcm:eta etaType="ESTIMATED" timeValue="2030-01-02T04:30:00Z"/><nxcm:etd etdType="ACTUAL" timeValue="2030-01-02T02:05:00Z"/><nxcm:flightTraversalData2><nxce:fix sequenceNumber="1">AAAAA</nxce:fix><nxce:waypoint latitudeDecimal="41.1" longitudeDecimal="-84.1" sequenceNumber="1" elapsedTime="10"/></nxcm:flightTraversalData2><nxcm:nextPosition latitudeDecimal="41.123456789" longitudeDecimal="-84.1"/><nxcm:remarksKeywords>NRP</nxcm:remarksKeywords><nxcm:routeOfFlight>KTAA./.ABC..DEF.STAR1.KTBB/0100</nxcm:routeOfFlight><nxcm:rvsmData currentCompliance="true" equipped="true" futureCompliance="true"/><nxcm:star routeName="STAR1" routeType="STAR"/><nxcm:starTransitionFix>XYZ</nxcm:starTransitionFix></nxcm:ncsmRouteData>"#;

fn track_route() -> String {
    fm(
        "trackInformation",
        &format!(
            r#"<fdm:trackInformation>{QAID}<nxcm:speed>100</nxcm:speed><nxcm:reportedAltitude><nxce:assignedAltitude><nxce:simpleAltitude>80</nxce:simpleAltitude></nxce:assignedAltitude></nxcm:reportedAltitude><nxcm:position><nxce:latitude><nxce:latitudeDMS degrees="01" direction="SOUTH" minutes="30"/></nxce:latitude><nxce:longitude><nxce:longitudeDMS degrees="010" direction="EAST" minutes="00"/></nxce:longitude></nxcm:position><nxcm:timeAtPosition>2030-01-02T03:04:00Z</nxcm:timeAtPosition>{ROUTE_DATA}</fdm:trackInformation>"#
        ),
    )
}

const SPEC_ATTRS: &str = r#"aircraftEngineClass="JET" equipmentQualifier="L" numberOfAircraft="1" specialAircraftQualifier="HEAVY JET""#;

fn modify() -> String {
    fm(
        "FlightModify",
        &format!(
            r#"<fdm:ncsmFlightModify><nxcm:airlineData><nxcm:arrivalFixAndTime arrTime="2030-01-02T04:00:00Z" fixName="ARRFX"/><nxcm:diversionIndicator>NO_DIVERSION</nxcm:diversionIndicator><nxcm:eta etaType="SCHEDULED" timeValue="2030-01-02T04:30:00Z"/><nxcm:etd etdType="PROPOSED" timeValue="2030-01-02T02:05:00Z"/><nxcm:flightStatusAndSpec><nxcm:aircraftModel>B738</nxcm:aircraftModel><nxcm:aircraftSpecification {SPEC_ATTRS}>B738</nxcm:aircraftSpecification><nxcm:flightStatus>ACTIVE</nxcm:flightStatus></nxcm:flightStatusAndSpec><nxcm:flightTimeData airlineInTime="2030-01-02T04:10:00Z" airlineOffTime="2030-01-02T02:10:00Z" airlineOnTime="2030-01-02T04:05:00Z" airlineOutTime="2030-01-02T02:00:00Z" flightCreation="2030-01-01T20:00:00Z" gateArrival="2030-01-02T04:11:00Z" gateDeparture="2030-01-02T01:59:00Z" originalArrival="2030-01-02T04:20:00Z" originalDeparture="2030-01-02T01:50:00Z" runwayArrival="2030-01-02T04:06:00Z" runwayDeparture="2030-01-02T02:11:00Z"/><nxcm:remarksKeywords>NRP WXRTE</nxcm:remarksKeywords>{RVSM}</nxcm:airlineData>{QAID}</fdm:ncsmFlightModify>"#
        ),
    )
}

fn plan() -> String {
    fm(
        "flightPlanInformation",
        &format!(
            r#"<fdm:flightPlanInformation><nxcm:altitude><nxce:assignedAltitude><nxce:simpleAltitude>330</nxce:simpleAltitude></nxce:assignedAltitude><nxce:requestedAltitude><nxce:simpleAltitude>370</nxce:simpleAltitude></nxce:requestedAltitude></nxcm:altitude><nxcm:coordinationPoint><nxce:namedFix>COORD</nxce:namedFix></nxcm:coordinationPoint><nxcm:coordinationTime type="PROPOSED">2030-01-02T03:30:00Z</nxcm:coordinationTime><nxcm:flightAircraftSpecs equipmentQualifier="W" specialAircraftQualifier="TCAS" numberOfAircraft="1">A320</nxcm:flightAircraftSpecs>{ROUTE_DATA}{QAID}<nxcm:routeOfFlight legacyFormat="KTAA.SID1.FIXA..FIXB.STAR2.KTBB/0200"/><nxcm:speed><nxce:filedTrueAirSpeed>450</nxce:filedTrueAirSpeed><nxce:mach>082</nxce:mach></nxcm:speed></fdm:flightPlanInformation>"#
        ),
    )
}

fn departure() -> String {
    fm(
        "departureInformation",
        &format!(
            r#"<fdm:departureInformation><nxcm:flightAircraftSpecs equipmentQualifier="L">C172</nxcm:flightAircraftSpecs><nxcm:ncsmFlightTimeData><nxcm:eta etaType="ESTIMATED" timeValue="2030-01-02T04:30:00Z"/><nxcm:etd etdType="ACTUAL" timeValue="2030-01-02T02:05:00Z"/>{RVSM}</nxcm:ncsmFlightTimeData>{QAID}<nxcm:timeOfArrival estimated="true">2030-01-02T04:31:00Z</nxcm:timeOfArrival><nxcm:timeOfDeparture estimated="false">2030-01-02T02:06:00Z</nxcm:timeOfDeparture></fdm:departureInformation>"#
        ),
    )
}

fn boundary() -> String {
    fm(
        "boundaryCrossingUpdate",
        &format!(
            r#"<fdm:boundaryCrossingUpdate><nxcm:boundaryPosition boundaryCrossingTime="2030-01-02T03:20:00Z"><nxce:fixRadialDistance distance="005" radial="120">BNDFX</nxce:fixRadialDistance></nxcm:boundaryPosition><nxcm:flightAircraftSpecs equipmentQualifier="L">B738</nxcm:flightAircraftSpecs>{QAID}<nxcm:reportedAltitude><nxce:assignedAltitude><nxce:simpleAltitude>240</nxce:simpleAltitude></nxce:assignedAltitude></nxcm:reportedAltitude><nxcm:routeOfFlight legacyFormat="KTAA.FIXA..FIXB.KTBB"/><nxcm:speed><nxce:filedTrueAirSpeed>430</nxce:filedTrueAirSpeed></nxcm:speed></fdm:boundaryCrossingUpdate>"#
        ),
    )
}

fn oceanic() -> String {
    fm(
        "oceanicReport",
        &format!(
            r#"<fdm:oceanicReport><nxcm:plannedPositionData planNumber="1"><nxce:altitude>350</nxce:altitude></nxcm:plannedPositionData>{QAID}<nxcm:reportedPositionData><nxce:altitude>350</nxce:altitude><nxce:position><nxce:latitude><nxce:latitudeDMS degrees="50" direction="NORTH" minutes="00"/></nxce:latitude><nxce:longitude><nxce:longitudeDMS degrees="030" direction="WEST" minutes="30"/></nxce:longitude></nxce:position><nxce:time>2030-01-02T03:00:00Z</nxce:time></nxcm:reportedPositionData><nxcm:speed>480</nxcm:speed></fdm:oceanicReport>"#
        ),
    )
}

fn fi(msg_type: &str, body: &str) -> String {
    format!(
        r#"<ns12:fiMessage sensitivity="A" visDomain="SHARED" destinationCodes="ALL" sourceFacility="DCCX" sourceTimeStamp="2030-01-02T03:04:05Z" msgType="{msg_type}">{body}</ns12:fiMessage>"#
    )
}

const FXA_FLIGHT: &str = r#"<ns9:fxaFlight><ns9:bentryTm>2030-01-02T03:00:00Z</ns9:bentryTm><ns9:fxaId><ns11:fcaId>fca.tst.lxstn01.20300101000000</ns11:fcaId></ns9:fxaId></ns9:fxaFlight>"#;

fn tmi_entry(acid: &str, flight_ref: &str, extra: &str) -> String {
    format!(
        r#"<ns12:flightData><ns9:flight><ns7:aircraftId>{acid}</ns7:aircraftId><ns7:arrivalPoint><ns7:airport>KTBB</ns7:airport></ns7:arrivalPoint><ns7:departurePoint><ns7:airport>KTAA</ns7:airport></ns7:departurePoint><ns7:gufi>ZZ000002bb</ns7:gufi><ns7:igtd>2030-01-02T01:30:00Z</ns7:igtd></ns9:flight><ns9:flightReference>{flight_ref}</ns9:flightReference><ns9:status>PLANNED</ns9:status><ns9:tmiFlightInfoList><ns9:fxaFlightData>{FXA_FLIGHT}{FXA_FLIGHT}</ns9:fxaFlightData><ns9:tmi updateType="NEW" lastUpdateTime="2030-01-01T12:00:00Z"><ns9:fcaId>fca.tst.lxstn01.20300101000000</ns9:fcaId></ns9:tmi></ns9:tmiFlightInfoList>{extra}</ns12:flightData>"#
    )
}

fn tmi_list() -> String {
    fi(
        "TMI_FLIGHT_LIST",
        &format!(
            "<ns12:tmiFlightDataList>{}{}</ns12:tmiFlightDataList>",
            tmi_entry("TST1", "100000010", ""),
            tmi_entry("TST2", "100000011", "")
        ),
    )
}

fn rstr() -> String {
    fi(
        "RSTR",
        r#"<ns12:restrictionMessage><ns11:action>4</ns11:action><ns11:aircraftType>ALL</ns11:aircraftType><ns11:airports>KTBB</ns11:airports><ns11:approvalTime>2030-01-02T03:00:00Z</ns11:approvalTime><ns11:entryTime>2030-01-02T03:01:00Z</ns11:entryTime><ns11:eventTime>2030-01-02T03:02:00Z</ns11:eventTime><ns11:facility>DCCX</ns11:facility><ns11:mitNumber>278529</ns11:mitNumber><ns11:mitValue>15</ns11:mitValue><ns11:passback>N</ns11:passback><ns11:providerStatus>X/1/Y</ns11:providerStatus><ns11:qualifier>PER FACILITY</ns11:qualifier><ns11:reasonText>VOL:Volume</ns11:reasonText><ns11:referenceRestrictionId>7</ns11:referenceRestrictionId><ns11:remarks>TEST REMARK</ns11:remarks><ns11:restrictedNasElements>FIXQ</ns11:restrictedNasElements><ns11:restrictionCategory>MIT</ns11:restrictionCategory><ns11:restrictionId>20300101</ns11:restrictionId><ns11:restrictionType>ARRIVAL</ns11:restrictionType><ns11:startTime>2030-01-02T04:00:00Z</ns11:startTime><ns11:stopTime>2030-01-02T06:00:00Z</ns11:stopTime></ns12:restrictionMessage>"#,
    )
}

fn gadv_with(title: &str, text: &str) -> String {
    fi(
        "GADV",
        &format!(
            r#"<ns12:generalAdvisory><ns11:advisoryNumber>0010</ns11:advisoryNumber><ns11:advisoryText>{text}</ns11:advisoryText><ns11:advisoryTitle>{title}</ns11:advisoryTitle><ns11:dateSent>2030-01-02T03:00:00Z</ns11:dateSent><ns11:effectivePeriod><ns4:endTime>2030-01-02T09:00:00Z</ns4:endTime><ns4:startTime>2030-01-02T04:00:00Z</ns4:startTime></ns11:effectivePeriod><ns11:facilities>ALL</ns11:facilities><ns11:origin>NOC OPS</ns11:origin></ns12:generalAdvisory>"#
        ),
    )
}

fn gadv() -> String {
    gadv_with(
        "ATCSCC ADVZY 010 TST/ZXX 01/02/2030 CDM GROUND DELAY PROGRAM",
        "LINE ONE\nLINE TWO &amp; MORE",
    )
}

fn fxa() -> String {
    fi(
        "FXA",
        r#"<ns12:feaFca><ns11:colorId>3</ns11:colorId><ns11:endTime>2030-01-03T00:00:00Z</ns11:endTime><ns11:fcaId>fca.tst.lxstn01.20300101000000</ns11:fcaId><ns11:fcaName>TEST_AREA</ns11:fcaName><ns11:fcaReason>VOLUME</ns11:fcaReason><ns11:feaFcaType>FEA</ns11:feaFcaType><ns11:lastUpdate>2030-01-02T03:00:00Z</ns11:lastUpdate><ns11:line><ns11:points><ns11:point><ns4:latitude>1</ns4:latitude></ns11:point></ns11:points></ns11:line><ns11:nas><ns11:airportName>KTBB</ns11:airportName></ns11:nas><ns11:primaryFilter><ns11:conditions/></ns11:primaryFilter><ns11:startTime>2030-01-02T00:00:00Z</ns11:startTime><ns11:tmiStatus>UPDATED</ns11:tmiStatus></ns12:feaFca>"#,
    )
}

fn aptc() -> String {
    fi(
        "APTC",
        r#"<ns12:airportConfigMessage><ns11:aarAdjust/><ns11:airport>KTBB</ns11:airport><ns11:arrRate>60</ns11:arrRate><ns11:arrRunwayConf>09/27</ns11:arrRunwayConf><ns11:depRate>0</ns11:depRate><ns11:depRunwayConf/><ns11:eventTime>2030-01-02T03:00:00Z</ns11:eventTime><ns11:facility>TST</ns11:facility><ns11:remarks>RWY CLSD.</ns11:remarks><ns11:stratAar>58</ns11:stratAar><ns11:weather>VMC</ns11:weather></ns12:airportConfigMessage>"#,
    )
}

// ---- running the decoder -------------------------------------------------------------------

fn run_with(
    xml: &str,
    flights: usize,
    flows: usize,
    strings: usize,
) -> (Result<Outcome, DocError>, Rows) {
    let mut rows = new_rows(
        flights,
        flows,
        strings,
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    );
    let mut p = Parser::new(default_limits()).unwrap();
    let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
    (r, rows)
}

fn run(xml: &str) -> (Result<Outcome, DocError>, Rows) {
    run_with(xml, 16, 8, 1 << 16)
}

fn ok(xml: &str) -> (Outcome, Rows) {
    let (r, rows) = run(xml);
    (r.unwrap(), rows)
}

/// Decodes a document expected to be refused: either the document itself (capacity, structure)
/// or its one message (the field error). Nothing of it may be left in the rows or the table.
fn err(xml: &str) -> DocError {
    let (r, rows) = run(xml);
    assert_eq!(
        flights_len(&rows.flights),
        0,
        "a refused document left flight rows"
    );
    assert_eq!(
        flows_len(&rows.flows),
        0,
        "a refused document left flow rows"
    );
    assert_eq!(rows.strings_used, 0, "a refused document left text");
    match r {
        Ok(o) => {
            assert_eq!(o.rejected, 1, "accepted: {o:?}");
            DocError::Field(o.first_reject.unwrap())
        }
        Err(e) => e,
    }
}

fn one_flight(msg: &str) -> (Outcome, Rows, FlightRow) {
    let (o, rows) = ok(&wrap_flight(msg));
    assert_eq!(flights_len(&rows.flights), 1);
    let r = flight_at(&rows.flights, 0).unwrap();
    (o, rows, r)
}

fn one_flow(msg: &str) -> (Outcome, Rows, FlowRow) {
    let (o, rows) = ok(&wrap_fi(msg));
    assert_eq!(flows_len(&rows.flows), 1);
    let r = flow_at(&rows.flows, 0).unwrap();
    (o, rows, r)
}

fn field_err(e: DocError) -> FieldError {
    match e {
        DocError::Field(f) => f,
        other => panic!("expected a field error, got {other:?}"),
    }
}

fn nm(n: &[u8]) -> &str {
    name_str(n)
}

// ---- OA-TFMS-001 .. 003: parsers and layout ------------------------------------------------

// REQ: OA-TFMS-001
#[test]
fn times_are_strict_iso_utc_to_unix_ms() {
    assert_eq!(parse_iso_ms(b"2030-01-02T03:04:05Z"), Ok(t(3, 4, 5)));
    assert_eq!(parse_iso_ms(b"1970-01-01T00:00:00Z"), Ok(0));
    assert_eq!(
        parse_iso_ms(b"2030-01-02T03:04:05.5Z"),
        Ok(t(3, 4, 5) + 500)
    );
    assert_eq!(
        parse_iso_ms(b"2030-01-02T03:04:05.05Z"),
        Ok(t(3, 4, 5) + 50)
    );
    assert_eq!(
        parse_iso_ms(b"2030-01-02T03:04:05.123Z"),
        Ok(t(3, 4, 5) + 123)
    );
    // Leap years are honoured.
    assert!(parse_iso_ms(b"2028-02-29T00:00:00Z").is_ok());
    assert_eq!(parse_iso_ms(b"2029-02-29T00:00:00Z"), Err(TimeError::Range));
    assert_eq!(parse_iso_ms(b"2100-02-29T00:00:00Z"), Err(TimeError::Range));
    for (bad, why) in [
        (&b"2030-01-02T03:04:05+00:00"[..], TimeError::Shape),
        (b"2030-01-02T03:04:05", TimeError::Shape),
        (b"2030-01-02t03:04:05Z", TimeError::Shape),
        (b"2030-01-02T03:04:05z", TimeError::Shape),
        (b"2030-01-02 03:04:05Z", TimeError::Shape),
        (b"2030-01-02", TimeError::Shape),
        (b"20300102T030405Z", TimeError::Shape),
        (b"2030-01-02T03:04:05.Z", TimeError::Shape),
        (b"2030-01-02T03:04:05.1234Z", TimeError::Fraction),
        (b"2030-01-02T03:04:6xZ", TimeError::Digit),
        (b"2030-01-02T03:04:60Z", TimeError::Range),
        (b"2030-01-02T24:00:00Z", TimeError::Range),
        (b"2030-13-02T03:04:05Z", TimeError::Range),
        (b"2030-00-02T03:04:05Z", TimeError::Range),
        (b"2030-01-32T03:04:05Z", TimeError::Range),
        (b"1969-12-31T23:59:59Z", TimeError::Range),
        (b"", TimeError::Shape),
    ] {
        assert_eq!(
            parse_iso_ms(bad),
            Err(why),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

// REQ: OA-TFMS-002
#[test]
fn scalar_parsers_are_strict() {
    // names
    assert_eq!(nm(&parse_name::<8>(b"TST123").unwrap()), "TST123");
    assert_eq!(parse_name::<4>(b"ABCDE"), Err(FieldErrorKind::TooLong));
    assert_eq!(parse_name::<8>(b"A B"), Err(FieldErrorKind::BadChar));
    assert_eq!(parse_name::<8>(b"A\x01"), Err(FieldErrorKind::BadChar));
    assert_eq!(
        parse_name::<8>("A\u{e9}".as_bytes()),
        Err(FieldErrorKind::BadChar)
    );
    assert_eq!(parse_name::<8>(b""), Ok([0; 8]));
    // words
    assert_eq!(nm(&parse_words::<16>(b"HEAVY JET").unwrap()), "HEAVY JET");
    for bad in [&b" A"[..], b"A ", b"A  B", b"A\tB"] {
        assert_eq!(parse_words::<16>(bad), Err(FieldErrorKind::BadChar));
    }
    assert_eq!(parse_words::<4>(b"HEAVY"), Err(FieldErrorKind::TooLong));
    // digits
    assert_eq!(parse_digits(b"0010", 4), Ok(10));
    assert_eq!(parse_digits(b"", 4), Err(FieldErrorKind::Format));
    assert_eq!(parse_digits(b"12345", 4), Err(FieldErrorKind::Format));
    assert_eq!(parse_digits(b"1a", 4), Err(FieldErrorKind::Format));
    assert_eq!(parse_digits(b"-1", 4), Err(FieldErrorKind::Format));
    assert_eq!(
        parse_digits(b"99999999999", 11),
        Err(FieldErrorKind::Format)
    );
    // altitude: hundreds of feet, optional capital letter
    assert_eq!(parse_altitude(b"350"), Ok((35_000, 0)));
    assert_eq!(parse_altitude(b"80"), Ok((8_000, 0)));
    assert_eq!(parse_altitude(b"350C"), Ok((35_000, b'C')));
    assert_eq!(parse_altitude(b"001"), Ok((100, 0)));
    for bad in [&b"3500"[..], b"35c", b"C", b"", b"3C0", b"35CC", b"-35"] {
        assert!(
            parse_altitude(bad).is_err(),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
    // decimal degrees to 1e-7
    assert_eq!(parse_decimal_e7(b"41.5", 90), Ok(415_000_000));
    assert_eq!(parse_decimal_e7(b"-84.12345678", 180), Ok(-841_234_568));
    assert_eq!(parse_decimal_e7(b"-84.12345674999", 180), Ok(-841_234_567));
    assert_eq!(parse_decimal_e7(b"0.0000000", 90), Ok(0));
    assert_eq!(parse_decimal_e7(b"90.0", 90), Ok(900_000_000));
    assert_eq!(parse_decimal_e7(b"90.00000004", 90), Ok(900_000_000));
    assert_eq!(
        parse_decimal_e7(b"90.00000005", 90),
        Err(FieldErrorKind::Range)
    );
    assert_eq!(parse_decimal_e7(b"181.0", 180), Err(FieldErrorKind::Range));
    for bad in [
        &b"41"[..],
        b"41.",
        b".5",
        b"4a.5",
        b"41.5x",
        b"1.1234567890123456789",
        b"-",
        b"1234.5",
    ] {
        assert_eq!(
            parse_decimal_e7(bad, 180),
            Err(FieldErrorKind::Format),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
    // degrees, minutes, seconds: nearest 1e-7, hemisphere by sign, ranges
    assert_eq!(dms_e7(41, 46, 24, false, 90), Ok(417_733_333));
    assert_eq!(dms_e7(41, 46, 24, true, 90), Ok(-417_733_333));
    assert_eq!(dms_e7(0, 0, 1, false, 90), Ok(2_778));
    assert_eq!(dms_e7(90, 0, 0, false, 90), Ok(900_000_000));
    assert_eq!(dms_e7(180, 0, 0, true, 180), Ok(-1_800_000_000));
    assert_eq!(dms_e7(0, 0, 0, true, 90), Ok(0));
    for (d, m, s, max) in [
        (91, 0, 0, 90),
        (90, 0, 1, 90),
        (90, 1, 0, 90),
        (10, 60, 0, 90),
        (10, 0, 60, 90),
        (181, 0, 0, 180),
    ] {
        assert_eq!(dms_e7(d, m, s, false, max), Err(FieldErrorKind::Range));
    }
    // flags, vocabularies, letters
    assert_eq!(parse_bool(b"true"), Ok(2));
    assert_eq!(parse_bool(b"false"), Ok(1));
    for bad in [&b"TRUE"[..], b"True", b"1", b"", b"yes"] {
        assert_eq!(parse_bool(bad), Err(FieldErrorKind::Vocabulary));
    }
    assert_eq!(parse_vocab(b"ACTIVE", &status::NAMES), Ok(status::ACTIVE));
    assert_eq!(
        parse_vocab(b"CANCELED", &status::NAMES),
        Ok(status::CANCELED)
    );
    assert_eq!(
        parse_vocab(b"-", &status::NAMES),
        Err(FieldErrorKind::Vocabulary)
    );
    assert_eq!(
        parse_vocab(b"active", &status::NAMES),
        Err(FieldErrorKind::Vocabulary)
    );
    assert_eq!(
        parse_vocab(b"GENERAL AVIATION", &ucat::NAMES),
        Ok(ucat::GENERAL)
    );
    assert_eq!(parse_letter(b"Q"), Ok(b'Q'));
    assert_eq!(parse_letter(b"q"), Err(FieldErrorKind::BadChar));
    assert_eq!(parse_letter(b"QQ"), Err(FieldErrorKind::Format));
    assert_eq!(parse_letter(b""), Err(FieldErrorKind::Format));
    // whole-word search
    assert!(contains_word(b"A GS CNX B", b"GS"));
    assert!(contains_word(b"GROUND STOP", b"GROUND STOP"));
    assert!(!contains_word(b"GSA", b"GS"));
    assert!(!contains_word(b"AGS", b"GS"));
    assert!(!contains_word(b"", b"GS"));
    assert!(!contains_word(b"GS", b""));
}

// REQ: OA-TFMS-003
#[test]
fn rows_are_plain_struct_of_arrays_with_absent_values() {
    assert_eq!(std::mem::size_of::<FlightRow>(), 408);
    assert_eq!(std::mem::size_of::<FlowRow>(), 144);
    assert_eq!(std::mem::size_of::<oa_tfms::StrRef>(), 8);
    // The blank rows hold only absent values.
    let b = BLANK_FLIGHT;
    for v in [
        b.src_ms,
        b.igtd_ms,
        b.eta_ms,
        b.etd_ms,
        b.pos_ms,
        b.out_ms,
        b.off_ms,
        b.on_ms,
        b.in_ms,
        b.rwy_dep_ms,
        b.rwy_arr_ms,
        b.gate_dep_ms,
        b.gate_arr_ms,
        b.created_ms,
        b.orig_dep_ms,
        b.orig_arr_ms,
        b.tod_ms,
        b.toa_ms,
        b.coord_ms,
        b.bnd_ms,
        b.arrfix_ms,
        b.depfix_ms,
        b.tmi_ms,
    ] {
        assert_eq!(v, NO_TIME);
    }
    for v in [b.lat_e7, b.lon_e7, b.next_lat_e7, b.next_lon_e7] {
        assert_eq!(v, NO_COORD);
    }
    for v in [b.alt_ft, b.asg_ft, b.req_ft, b.speed_milli, b.tas_milli] {
        assert_eq!(v, NO_NUM);
    }
    assert_eq!((b.route, b.remarks, b.fca), (NO_STR, NO_STR, NO_STR));
    assert_eq!((b.kind, b.flags, b.flight_ref), (kind::OTHER, 0, 0));
    let w = BLANK_FLOW;
    for v in [w.src_ms, w.start_ms, w.end_ms, w.event_ms] {
        assert_eq!(v, NO_TIME);
    }
    for v in [w.rate, w.rate_dep, w.rate_strat] {
        assert_eq!(v, NO_NUM);
    }
    // Push, rebuild, truncate, capacity: one row at a time, columns sharing an index.
    let mut rows = new_rows(2, 1, 64, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    assert_eq!(rows.flights.cap, 2);
    let mut a = BLANK_FLIGHT;
    a.kind = kind::TRACK;
    a.acid = *b"TST1\0\0\0\0";
    a.alt_ft = 12_300;
    let mut c = a;
    c.acid = *b"TST2\0\0\0\0";
    assert!(push_flight(&mut rows.flights, &a));
    assert!(push_flight(&mut rows.flights, &c));
    assert!(
        !push_flight(&mut rows.flights, &a),
        "a full table refuses a push"
    );
    assert_eq!(flights_len(&rows.flights), 2);
    assert_eq!(flight_at(&rows.flights, 0), Some(a));
    assert_eq!(flight_at(&rows.flights, 1), Some(c));
    assert_eq!(flight_at(&rows.flights, 2), None);
    assert!(push_flow(&mut rows.flows, &BLANK_FLOW_KIND));
    assert!(!push_flow(&mut rows.flows, &BLANK_FLOW_KIND));
    truncate_rows(&mut rows, 1, 0, 0);
    assert_eq!(flights_len(&rows.flights), 1);
    assert_eq!(flows_len(&rows.flows), 0);
    assert_eq!(flight_at(&rows.flights, 1), None);
    clear_rows(&mut rows);
    assert_eq!(flights_len(&rows.flights), 0);
    // Text: a reference outside the used table, or past the end, reads as empty.
    rows.strings[..3].copy_from_slice(b"abc");
    rows.strings_used = 3;
    assert_eq!(text(&rows, oa_tfms::StrRef { off: 1, len: 2 }), "bc");
    assert_eq!(text(&rows, oa_tfms::StrRef { off: 1, len: 3 }), "");
    assert_eq!(
        text(
            &rows,
            oa_tfms::StrRef {
                off: u32::MAX,
                len: u32::MAX
            }
        ),
        ""
    );
    assert_eq!(text(&rows, NO_STR), "");
    // Tri-state flags
    assert_eq!(flag_get(0b10_01, flag::CDM_PART), 1);
    assert_eq!(flag_get(0b10_01, flag::TOD_EST), 2);
}

const BLANK_FLOW_KIND: FlowRow = {
    let mut w = BLANK_FLOW;
    w.kind = fkind::RSTR;
    w
};

// ---- OA-TFMS-004 .. 007: flight data messages ----------------------------------------------

// REQ: OA-TFMS-004, OA-TFMS-005
#[test]
fn track_information_carries_identity_position_altitude_and_speed() {
    let (o, rows, r) = one_flight(&track());
    assert_eq!(o.kind, DocKind::Flight);
    assert_eq!((o.messages, o.flight_rows, o.flow_rows), (1, 1, 0));
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 1));
    assert_eq!(r.kind, kind::TRACK);
    assert_eq!(trig::NAMES[usize::from(r.trigger)], "HCS_TRACK_MSG");
    assert_eq!(nm(&r.acid), "TST123");
    assert_eq!(nm(&r.gufi), "ZZ000001aa");
    assert_eq!(r.flight_ref, 100_000_001);
    assert_eq!((nm(&r.airline), nm(&r.major)), ("TST", "TST"));
    assert_eq!((nm(&r.dep), nm(&r.arr)), ("KTAA", "KTBB"));
    assert_eq!(nm(&r.src_fac), "KZXX");
    assert_eq!((nm(&r.cid_fac), nm(&r.cid_num)), ("KZXX", "123"));
    assert_eq!(r.src_ms, t(3, 4, 5));
    assert_eq!(r.igtd_ms, t(1, 0, 0));
    assert_eq!(r.ac_cat, accat::JET);
    assert_eq!(r.user_cat, ucat::COMMERCIAL);
    // position: 41 deg 46' 24" N and 84 deg 08' 48" W, in 1e-7 degree
    assert_eq!((r.lat_e7, r.lon_e7), (417_733_333, -841_466_667));
    assert_eq!(r.pos_ms, t(3, 4, 0));
    assert_eq!((r.alt_ft, r.alt_sfx), (35_000, b'C'));
    assert_eq!((r.speed_milli, r.speed_unit), (276_000, spd_unit::KNOTS));
    assert_eq!((r.next_lat_e7, r.next_lon_e7), (419_000_000, -845_000_000));
    assert_eq!((nm(&r.arr_fix), r.arrfix_ms), ("ARRFX", t(4, 0, 0)));
    assert_eq!((nm(&r.dep_fix), r.depfix_ms), ("DEPFX", t(2, 0, 0)));
    assert_eq!((r.eta_ms, r.eta_type), (t(4, 30, 0), ttype::ESTIMATED));
    assert_eq!((r.etd_ms, r.etd_type), (NO_TIME, 0));
    assert_eq!(flag_get(r.flags, flag::CDM_PART), 1);
    assert_eq!(flag_get(r.flags, flag::RVSM_EQ), 2);
    assert_eq!(flag_get(r.flags, flag::RVSM_CUR), 2);
    assert_eq!(flag_get(r.flags, flag::RVSM_FUT), 1);
    assert_eq!(text(&rows, r.route), "");
    // Absent values stay absent.
    assert_eq!((r.asg_ft, r.req_ft, r.tas_milli), (NO_NUM, NO_NUM, NO_NUM));
    assert_eq!((r.status, r.diversion, r.eng_class, r.equip), (0, 0, 0, 0));
}

// REQ: OA-TFMS-005, OA-TFMS-006, OA-TFMS-007
#[test]
fn track_with_route_data_has_route_text_remarks_and_the_departure_actual() {
    let (o, rows, r) = one_flight(&track_route());
    // the traversal list and the elements known but not stored count as ignored, not unknown
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 6));
    assert_eq!(text(&rows, r.route), "KTAA./.ABC..DEF.STAR1.KTBB/0100");
    assert_eq!(r.flags & flag::ROUTE_LEGACY, 0);
    assert_eq!(text(&rows, r.remarks), "NRP");
    assert_eq!((nm(&r.dp), nm(&r.star)), ("DPTST1", "STAR1"));
    assert_eq!(r.diversion, divert::NONE);
    assert_eq!((r.etd_ms, r.etd_type), (t(2, 5, 0), ttype::ACTUAL));
    assert_eq!((r.eta_ms, r.eta_type), (t(4, 30, 0), ttype::ESTIMATED));
    // 41.123456789 rounds at the eighth digit; the seconds attribute is optional (absent is 0)
    assert_eq!((r.next_lat_e7, r.next_lon_e7), (411_234_568, -841_000_000));
    assert_eq!((r.lat_e7, r.lon_e7), (-15_000_000, 100_000_000));
    // an altitude without a letter
    assert_eq!((r.alt_ft, r.alt_sfx), (8_000, 0));
    assert_eq!(flag_get(r.flags, flag::RVSM_FUT), 2);
}

// REQ: OA-TFMS-004, OA-TFMS-006
#[test]
fn flight_modify_carries_status_aircraft_and_the_airline_times() {
    let (o, _, r) = one_flight(&modify());
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 2));
    assert_eq!(r.kind, kind::MODIFY);
    assert_eq!(r.status, status::ACTIVE);
    assert_eq!(nm(&r.ac_type), "B738");
    assert_eq!(r.eng_class, accat::JET);
    assert_eq!(r.equip, b'L');
    assert_eq!(nm(&r.special), "HEAVY JET");
    assert_eq!((r.eta_ms, r.eta_type), (t(4, 30, 0), ttype::SCHEDULED));
    assert_eq!((r.etd_ms, r.etd_type), (t(2, 5, 0), ttype::PROPOSED));
    assert_eq!(r.out_ms, t(2, 0, 0));
    assert_eq!(r.off_ms, t(2, 10, 0));
    assert_eq!(r.on_ms, t(4, 5, 0));
    assert_eq!(r.in_ms, t(4, 10, 0));
    assert_eq!(r.rwy_dep_ms, t(2, 11, 0));
    assert_eq!(r.rwy_arr_ms, t(4, 6, 0));
    assert_eq!(r.gate_dep_ms, t(1, 59, 0));
    assert_eq!(r.gate_arr_ms, t(4, 11, 0));
    assert_eq!(r.created_ms, t(1, 0, 0) - 5 * 3600 * 1000);
    // the original times let a delay be derived: this flight left 15 minutes after its original
    assert_eq!(r.orig_dep_ms, t(1, 50, 0));
    assert_eq!(r.orig_arr_ms, t(4, 20, 0));
    assert_eq!(r.out_ms - r.orig_dep_ms, 10 * 60 * 1000);
}

// REQ: OA-TFMS-004, OA-TFMS-005, OA-TFMS-006, OA-TFMS-007
#[test]
fn flight_plan_has_altitudes_speed_coordination_and_the_legacy_route() {
    let (o, rows, r) = one_flight(&plan());
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 8));
    assert_eq!(r.kind, kind::FLIGHT_PLAN);
    assert_eq!((r.asg_ft, r.req_ft), (33_000, 37_000));
    assert_eq!((r.tas_milli, r.tas_unit), (450_000, spd_unit::KNOTS));
    assert_eq!(nm(&r.ac_type), "A320");
    assert_eq!((r.equip, nm(&r.special)), (b'W', "TCAS"));
    assert_eq!(
        (nm(&r.coord_fix), r.coord_ms, r.coord_type),
        ("COORD", t(3, 30, 0), ttype::PROPOSED)
    );
    // The legacy format wins over the element text, whichever comes first.
    assert_eq!(text(&rows, r.route), "KTAA.SID1.FIXA..FIXB.STAR2.KTBB/0200");
    assert_ne!(r.flags & flag::ROUTE_LEGACY, 0);
    // The route text of ncsmRouteData still reads, and the legacy text is what the row keeps
    // when it comes first too.
    let swapped = plan().replace(
        r#"<nxcm:routeOfFlight legacyFormat="KTAA.SID1.FIXA..FIXB.STAR2.KTBB/0200"/>"#,
        "",
    );
    let swapped = swapped.replace(
        "<nxcm:ncsmRouteData>",
        r#"<nxcm:routeOfFlight legacyFormat="KTAA.SID1.FIXA..FIXB.STAR2.KTBB/0200"/><nxcm:ncsmRouteData>"#,
    );
    let (_, rows2, r2) = one_flight(&swapped);
    assert_eq!(
        text(&rows2, r2.route),
        "KTAA.SID1.FIXA..FIXB.STAR2.KTBB/0200"
    );
}

// REQ: OA-TFMS-004, OA-TFMS-006
#[test]
fn amendment_departure_arrival_and_cancellation() {
    let amend = fm(
        "flightPlanAmendmentInformation",
        &format!(
            r#"<fdm:flightPlanAmendmentInformation><nxcm:amendmentData><nxcm:newAltitude><nxce:assignedAltitude><nxce:simpleAltitude>290</nxce:simpleAltitude></nxce:assignedAltitude></nxcm:newAltitude><nxcm:newCoordinationPoint><nxce:namedFix>NEWFX</nxce:namedFix></nxcm:newCoordinationPoint><nxcm:newCoordinationTime type="ESTIMATED">2030-01-02T03:40:00Z</nxcm:newCoordinationTime><nxcm:newFlightAircraftSpecs equipmentQualifier="Q">B737</nxcm:newFlightAircraftSpecs><nxcm:newRouteOfFlight legacyFormat="KTAA.NEWRT.KTBB"/><nxcm:newSpeed><nxce:filedTrueAirSpeed>420</nxce:filedTrueAirSpeed></nxcm:newSpeed></nxcm:amendmentData>{QAID}</fdm:flightPlanAmendmentInformation>"#
        ),
    );
    let (_, rows, r) = one_flight(&amend);
    assert_eq!(r.kind, kind::AMENDMENT);
    assert_eq!(r.asg_ft, 29_000);
    assert_eq!(nm(&r.coord_fix), "NEWFX");
    assert_eq!((r.coord_ms, r.coord_type), (t(3, 40, 0), ttype::ESTIMATED));
    assert_eq!((nm(&r.ac_type), r.equip), ("B737", b'Q'));
    assert_eq!(text(&rows, r.route), "KTAA.NEWRT.KTBB");
    assert_eq!((r.tas_milli, r.tas_unit), (420_000, spd_unit::KNOTS));

    let (_, _, r) = one_flight(&departure());
    assert_eq!(r.kind, kind::DEPARTURE);
    assert_eq!(nm(&r.ac_type), "C172");
    assert_eq!(
        (r.tod_ms, flag_get(r.flags, flag::TOD_EST)),
        (t(2, 6, 0), 1)
    );
    assert_eq!(
        (r.toa_ms, flag_get(r.flags, flag::TOA_EST)),
        (t(4, 31, 0), 2)
    );
    assert_eq!((r.etd_ms, r.etd_type), (t(2, 5, 0), ttype::ACTUAL));

    let arrival = fm(
        "arrivalInformation",
        &format!(
            r#"<fdm:arrivalInformation><nxcm:ncsmFlightTimeData><nxcm:eta etaType="ACTUAL" timeValue="2030-01-02T04:29:00Z"/><nxcm:etd etdType="ACTUAL" timeValue="2030-01-02T02:05:00Z"/></nxcm:ncsmFlightTimeData>{QAID}<nxcm:timeOfArrival estimated="false">2030-01-02T04:29:00Z</nxcm:timeOfArrival></fdm:arrivalInformation>"#
        ),
    );
    let (_, _, r) = one_flight(&arrival);
    assert_eq!(r.kind, kind::ARRIVAL);
    assert_eq!(
        (r.toa_ms, flag_get(r.flags, flag::TOA_EST)),
        (t(4, 29, 0), 1)
    );
    assert_eq!((r.eta_ms, r.eta_type), (t(4, 29, 0), ttype::ACTUAL));

    let cancel = fm(
        "flightPlanCancellation",
        &format!("<fdm:flightPlanCancellation>{QAID}</fdm:flightPlanCancellation>"),
    );
    let (_, _, r) = one_flight(&cancel);
    assert_eq!(r.kind, kind::CANCELLATION);
    assert_eq!(nm(&r.acid), "TST123");
}

// REQ: OA-TFMS-004, OA-TFMS-005, OA-TFMS-006
#[test]
fn boundary_crossing_and_oceanic_report() {
    let (o, rows, r) = one_flight(&boundary());
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 2));
    assert_eq!(r.kind, kind::BOUNDARY);
    assert_eq!(r.bnd_ms, t(3, 20, 0));
    assert_eq!(r.alt_ft, 24_000);
    assert_eq!((r.tas_milli, r.tas_unit), (430_000, spd_unit::KNOTS));
    assert_eq!(text(&rows, r.route), "KTAA.FIXA..FIXB.KTBB");
    assert_eq!(nm(&r.ac_type), "B738");

    let (o, _, r) = one_flight(&oceanic());
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 2));
    assert_eq!(r.kind, kind::OCEANIC);
    // the reported position, altitude and time; the planned positions are ignored
    assert_eq!((r.lat_e7, r.lon_e7), (500_000_000, -305_000_000));
    assert_eq!(r.alt_ft, 35_000);
    assert_eq!(r.pos_ms, t(3, 0, 0));
    assert_eq!((r.speed_milli, r.speed_unit), (480_000, spd_unit::KNOTS));
}

// REQ: OA-TFMS-004, OA-TFMS-006
#[test]
fn create_times_route_sectors_and_activate() {
    let airline = r#"<nxcm:airlineData><nxcm:diversionIndicator>GROUND_NOCTL</nxcm:diversionIndicator><nxcm:eta etaType="SCHEDULED" timeValue="2030-01-02T04:30:00Z"/><nxcm:etd etdType="SCHEDULED" timeValue="2030-01-02T02:05:00Z"/><nxcm:flightStatusAndSpec><nxcm:aircraftModel>E75L</nxcm:aircraftModel><nxcm:aircraftSpecification aircraftEngineClass="JET" numberOfAircraft="1">E75L</nxcm:aircraftSpecification><nxcm:flightStatus>PLANNED</nxcm:flightStatus></nxcm:flightStatusAndSpec><nxcm:flightTimeData flightCreation="2030-01-01T00:00:00Z" originalArrival="2030-01-02T04:30:00Z" originalDeparture="2030-01-02T02:05:00Z" airlineInTime="2030-01-02T04:30:00Z" airlineOffTime="2030-01-02T02:05:00Z" airlineOnTime="2030-01-02T04:30:00Z" airlineOutTime="2030-01-02T02:05:00Z"/>"#;
    let create = fm(
        "FlightCreate",
        &format!(
            r#"<fdm:ncsmFlightCreate>{airline}</nxcm:airlineData><nxcm:diversionCancelData><nxcm:canceledFlightReference>100000002</nxcm:canceledFlightReference></nxcm:diversionCancelData>{QAID}</fdm:ncsmFlightCreate>"#
        ),
    );
    let (o, _, r) = one_flight(&create);
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 3));
    assert_eq!(r.kind, kind::CREATE);
    assert_eq!((r.status, r.diversion), (status::PLANNED, divert::GROUND));
    assert_eq!(nm(&r.ac_type), "E75L");
    assert_eq!(r.created_ms, t(0, 0, 0) - 24 * 3600 * 1000);

    let times = fm(
        "FlightTimes",
        &format!(
            r#"<fdm:ncsmFlightTimes><nxcm:eta etaType="ESTIMATED" timeValue="2030-01-02T04:30:00Z"/><nxcm:etd etdType="METERED" timeValue="2030-01-02T02:05:00Z"/><nxcm:flightStatusAndSpec><nxcm:aircraftModel>E75L</nxcm:aircraftModel><nxcm:flightStatus>COMPLETED</nxcm:flightStatus></nxcm:flightStatusAndSpec>{QAID}</fdm:ncsmFlightTimes>"#
        ),
    );
    let (_, _, r) = one_flight(&times);
    assert_eq!(r.kind, kind::TIMES);
    assert_eq!((r.etd_type, r.status), (ttype::METERED, status::COMPLETED));

    let route = fm(
        "FlightRoute",
        &format!(
            r#"<fdm:ncsmFlightRoute><nxcm:altitude><nxce:assignedAltitude><nxce:simpleAltitude>310</nxce:simpleAltitude></nxce:assignedAltitude></nxcm:altitude>{ROUTE_DATA}{QAID}<nxcm:speed><nxce:groundSpeed>440</nxce:groundSpeed></nxcm:speed></fdm:ncsmFlightRoute>"#
        ),
    );
    let (o, rows, r) = one_flight(&route);
    // the traversal list and the ground speed are known but not stored
    assert_eq!(o.ignored_paths, 7);
    assert_eq!(r.kind, kind::ROUTE);
    assert_eq!(r.asg_ft, 31_000);
    assert_eq!(text(&rows, r.route), "KTAA./.ABC..DEF.STAR1.KTBB/0100");

    let sectors = fm(
        "FlightSectors",
        &format!(
            r#"<fdm:ncsmFlightSectors><nxcm:flightTraversalData2><nxce:sector sequenceNumber="1" elapsedEntryTime="0">ZXX01</nxce:sector></nxcm:flightTraversalData2>{QAID}</fdm:ncsmFlightSectors>"#
        ),
    );
    let (o, _, r) = one_flight(&sectors);
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 2));
    assert_eq!(r.kind, kind::SECTORS);

    let activate = fm(
        "FlightScheduleActivate",
        &format!(
            r#"<fdm:ncsmFlightScheduleActivate><nxcm:altitude><nxce:assignedAltitude><nxce:simpleAltitude>350</nxce:simpleAltitude></nxce:assignedAltitude></nxcm:altitude><nxcm:flightStatusAndSpec><nxcm:aircraftModel>B738</nxcm:aircraftModel><nxcm:flightStatus>PLANNED</nxcm:flightStatus></nxcm:flightStatusAndSpec>{ROUTE_DATA}{QAID}<nxcm:speed><nxce:filedTrueAirSpeed>460</nxce:filedTrueAirSpeed></nxcm:speed></fdm:ncsmFlightScheduleActivate>"#
        ),
    );
    let (_, _, r) = one_flight(&activate);
    assert_eq!(r.kind, kind::ACTIVATE);
    assert_eq!((r.asg_ft, r.tas_milli), (35_000, 460_000));
}

// REQ: OA-TFMS-004
#[test]
fn a_document_of_several_messages_gives_one_row_each_in_order() {
    let msgs = [track(), modify(), plan(), departure(), boundary()].concat();
    let (o, rows) = ok(&wrap_flight(&msgs));
    assert_eq!((o.messages, o.flight_rows), (5, 5));
    let kinds: Vec<u8> = rows.flights.kind.clone();
    assert_eq!(
        kinds,
        [
            kind::TRACK,
            kind::MODIFY,
            kind::FLIGHT_PLAN,
            kind::DEPARTURE,
            kind::BOUNDARY
        ]
    );
    // a second document appends after the first
    let mut rows = rows;
    let mut p = Parser::new(default_limits()).unwrap();
    let doc = wrap_flight(&track());
    decode_bytes(&mut p, doc.as_bytes(), &mut rows).unwrap();
    assert_eq!(flights_len(&rows.flights), 6);
}

// ---- OA-TFMS-008 .. 010: flow information, TMI lists, identity -----------------------------

// REQ: OA-TFMS-008
#[test]
fn tmi_flight_list_gives_a_flight_row_per_entry() {
    let (o, rows) = ok(&wrap_fi(&tmi_list()));
    assert_eq!(o.kind, DocKind::FlowInfo);
    assert_eq!((o.messages, o.flight_rows, o.flow_rows), (1, 2, 0));
    assert_eq!(o.unknown_paths, 0);
    // the fxaFlight elements and their children are known but not stored
    assert_eq!(o.ignored_paths, 7);
    let a = flight_at(&rows.flights, 0).unwrap();
    let b = flight_at(&rows.flights, 1).unwrap();
    assert_eq!((a.kind, b.kind), (kind::TMI_LIST, kind::TMI_LIST));
    assert_eq!((nm(&a.acid), a.flight_ref), ("TST1", 100_000_010));
    assert_eq!((nm(&b.acid), b.flight_ref), ("TST2", 100_000_011));
    assert_eq!((nm(&a.dep), nm(&a.arr)), ("KTAA", "KTBB"));
    assert_eq!(nm(&a.gufi), "ZZ000002bb");
    assert_eq!(a.igtd_ms, t(1, 30, 0));
    assert_eq!(a.status, status::PLANNED);
    assert_eq!(
        (a.tmi_update, a.tmi_ms),
        (oa_tfms::rows::tmiu::NEW, t(0, 0, 0) - 12 * 3600 * 1000)
    );
    assert_eq!(text(&rows, a.fca), "fca.tst.lxstn01.20300101000000");
    assert_eq!(a.n_fxa, 2);
    // the message's own time and facility go to every entry
    assert_eq!((a.src_ms, nm(&a.src_fac)), (t(3, 4, 5), "DCCX"));
    assert_eq!((b.src_ms, nm(&b.src_fac)), (t(3, 4, 5), "DCCX"));
    // the entries do not share state: an entry without a tmi has no fca
    let bare = fi(
        "TMI_FLIGHT_LIST",
        &format!(
            "<ns12:tmiFlightDataList>{}<ns12:flightData><ns9:flight><ns7:aircraftId>TST3</ns7:aircraftId></ns9:flight><ns9:flightReference>100000012</ns9:flightReference><ns9:status>CANCELED</ns9:status></ns12:flightData></ns12:tmiFlightDataList>",
            tmi_entry("TST1", "100000010", "")
        ),
    );
    let (_, rows) = ok(&wrap_fi(&bare));
    let c = flight_at(&rows.flights, 1).unwrap();
    assert_eq!(
        (c.status, c.n_fxa, c.fca, c.tmi_ms),
        (status::CANCELED, 0, NO_STR, NO_TIME)
    );
    assert_eq!(nm(&c.dep), "");
}

// REQ: OA-TFMS-009
#[test]
fn restriction_rows_carry_program_scope_rate_times_and_text() {
    let (o, rows, r) = one_flow(&rstr());
    assert_eq!((o.messages, o.flow_rows, o.flight_rows), (1, 1, 0));
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 9));
    assert_eq!(r.kind, fkind::RSTR);
    assert_eq!(r.program, prog::MIT);
    assert_eq!(r.scope, scope::ARRIVAL);
    assert_eq!((r.rate, r.rate_dep, r.rate_strat), (15, NO_NUM, NO_NUM));
    assert_eq!(
        (r.id, r.ref_id, r.number, r.action),
        (20_300_101, 7, 278_529, 4)
    );
    assert_eq!(r.src_ms, t(3, 4, 5));
    assert_eq!(r.event_ms, t(3, 2, 0));
    assert_eq!((r.start_ms, r.end_ms), (t(4, 0, 0), t(6, 0, 0)));
    assert_eq!(nm(&r.facility), "DCCX");
    assert_eq!(text(&rows, r.text), "VOL:Volume");
    assert_eq!(text(&rows, r.title), "PER FACILITY");
    assert_eq!(text(&rows, r.element), "FIXQ");
    assert_eq!(text(&rows, r.airports), "KTBB");
    assert_eq!(text(&rows, r.remarks), "TEST REMARK");
    for (cat, want) in [
        ("MINIT", prog::MINIT),
        ("STOP", prog::STOP),
        ("APREQ", prog::APREQ),
        ("TBM", prog::TBM),
    ] {
        let x = rstr().replace(
            "<ns11:restrictionCategory>MIT<",
            &format!("<ns11:restrictionCategory>{cat}<"),
        );
        assert_eq!(one_flow(&x).2.program, want, "{cat}");
    }
    for (ty, want) in [("DEPARTURE", scope::DEPARTURE), ("ENROUTE", scope::ENROUTE)] {
        let x = rstr().replace("ARRIVAL", ty);
        assert_eq!(one_flow(&x).2.scope, want, "{ty}");
    }
}

// REQ: OA-TFMS-009
#[test]
fn advisory_program_comes_from_the_words_of_its_title() {
    let (o, rows, r) = one_flow(&gadv());
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 3));
    assert_eq!(r.kind, fkind::GADV);
    assert_eq!(r.program, prog::GROUND_DELAY);
    assert_eq!(r.flags & fflag::CANCEL, 0);
    assert_eq!(r.number, 10);
    assert_eq!(
        (r.event_ms, r.start_ms, r.end_ms),
        (t(3, 0, 0), t(4, 0, 0), t(9, 0, 0))
    );
    assert_eq!(nm(&r.origin), "NOC OPS");
    assert_eq!(text(&rows, r.element), "ALL");
    // an entity is decoded, the line feed kept
    assert_eq!(text(&rows, r.text), "LINE ONE\nLINE TWO & MORE");
    for (title, want, cancel) in [
        (
            "ATCSCC ADVZY 020 TST/ZXX 01/02/2030 CDM GROUND STOP",
            prog::GROUND_STOP,
            false,
        ),
        (
            "ATCSCC ADVZY 021 TST/ZXX 01/02/2030 CDM GS CNX",
            prog::GROUND_STOP,
            true,
        ),
        (
            "ATCSCC ADVZY 022 TST/ZXX CDM GROUND DELAY PROGRAM CNX",
            prog::GROUND_DELAY,
            true,
        ),
        (
            "ATCSCC ADVZY 030 DCC 01/02/30 REROUTE CANCELLATION",
            prog::REROUTE,
            true,
        ),
        (
            "ATCSCC ADVZY 031 DCC 01/02/30 REROUTE",
            prog::REROUTE,
            false,
        ),
        (
            "ATCSCC ADVZY 040 AIRSPACE FLOW PROGRAM",
            prog::AIRSPACE_FLOW,
            false,
        ),
        ("ATCSCC ADVZY 041 DCC AFP CNX", prog::AIRSPACE_FLOW, true),
        (
            "ATCSCC ADVZY 050 DCC 01/02/2030 OPERATIONS PLAN",
            prog::ADVISORY,
            false,
        ),
        ("ADVZY SGS AGSX", prog::ADVISORY, false),
    ] {
        let (_, rows, r) = one_flow(&gadv_with(title, "TEXT"));
        assert_eq!(r.program, want, "{title}");
        assert_eq!(r.flags & fflag::CANCEL != 0, cancel, "{title}");
        assert_eq!(text(&rows, r.title), title);
    }
}

// REQ: OA-TFMS-009
#[test]
fn fca_and_airport_configuration_rows() {
    let (o, rows, r) = one_flow(&fxa());
    assert_eq!(o.unknown_paths, 0);
    // colorId, line, primaryFilter and their children are known but not stored
    assert_eq!(o.ignored_paths, 6);
    assert_eq!(r.kind, fkind::FXA);
    assert_eq!(r.program, prog::FEA);
    assert_eq!(r.status, oa_tfms::rows::fstatus::UPDATED);
    assert_eq!(
        (r.start_ms, r.end_ms, r.event_ms),
        (t(0, 0, 0), t(0, 0, 0) + 86_400_000, t(3, 0, 0))
    );
    assert_eq!(text(&rows, r.title), "fca.tst.lxstn01.20300101000000");
    assert_eq!(text(&rows, r.element), "TEST_AREA");
    assert_eq!(text(&rows, r.text), "VOLUME");
    assert_eq!(text(&rows, r.airports), "KTBB");
    let (_, _, r) = one_flow(
        &fxa()
            .replace(">FEA<", ">FCA<")
            .replace(">UPDATED<", ">NEW<"),
    );
    assert_eq!(
        (r.program, r.status),
        (prog::FCA, oa_tfms::rows::fstatus::NEW)
    );

    let (o, rows, r) = one_flow(&aptc());
    // aarAdjust, depRunwayConf, facility and their children are known but not stored
    assert_eq!((o.unknown_paths, o.ignored_paths), (0, 6));
    assert_eq!(r.kind, fkind::APTC);
    assert_eq!(r.program, prog::AIRPORT_CONFIG);
    assert_eq!((r.rate, r.rate_dep, r.rate_strat), (60, 0, 58));
    assert_eq!(r.event_ms, t(3, 0, 0));
    assert_eq!(text(&rows, r.element), "KTBB");
    assert_eq!(text(&rows, r.title), "09/27");
    assert_eq!(text(&rows, r.text), "VMC");
    assert_eq!(text(&rows, r.remarks), "RWY CLSD.");
}

// REQ: OA-TFMS-010
#[test]
fn identity_comes_from_the_attributes_and_the_body_must_agree() {
    // attributes only: a message without the body copies still has its identity
    let bare = fm("flightPlanCancellation", "<fdm:flightPlanCancellation/>");
    let (_, _, r) = one_flight(&bare);
    assert_eq!(
        (nm(&r.acid), nm(&r.dep), nm(&r.arr)),
        ("TST123", "KTAA", "KTBB")
    );
    // an absent departure or arrival attribute is an absent column, not an error
    let nodep = fm("flightPlanCancellation", "<fdm:flightPlanCancellation/>")
        .replace(r#" depArpt="KTAA""#, "")
        .replace(r#" arrArpt="KTBB""#, "");
    let (_, _, r) = one_flight(&nodep);
    assert_eq!((nm(&r.dep), nm(&r.arr)), ("", ""));
    // the body supplies what the attribute leaves out
    let from_body = fm(
        "flightPlanCancellation",
        &format!("<fdm:flightPlanCancellation>{QAID}</fdm:flightPlanCancellation>"),
    )
    .replace(r#" depArpt="KTAA""#, "");
    let (_, _, r) = one_flight(&from_body);
    assert_eq!(nm(&r.dep), "KTAA");
    // disagreement between the attribute and the body
    for (from, to, field) in [
        (r#"acid="TST123""#, r#"acid="OTHER1""#, Field::Acid),
        (r#"depArpt="KTAA""#, r#"depArpt="KZZZ""#, Field::Dep),
        (r#"arrArpt="KTBB""#, r#"arrArpt="KZZZ""#, Field::Arr),
    ] {
        let x = fm(
            "flightPlanCancellation",
            &format!("<fdm:flightPlanCancellation>{QAID}</fdm:flightPlanCancellation>"),
        )
        .replace(from, to);
        let e = field_err(err(&wrap_flight(&x)));
        assert_eq!((e.field, e.kind), (field, FieldErrorKind::Inconsistent));
    }
    // identity every row needs
    for (from, field) in [
        (r#"acid="TST123" "#, Field::Acid),
        (r#"flightRef="100000001" "#, Field::FlightRef),
        (r#" sourceTimeStamp="2030-01-02T03:04:05Z""#, Field::SrcTime),
    ] {
        let x = fm("flightPlanCancellation", "<fdm:flightPlanCancellation/>").replace(from, "");
        let e = field_err(err(&wrap_flight(&x)));
        assert_eq!((e.field, e.kind), (field, FieldErrorKind::Missing));
    }
    let nomsg = fm("flightPlanCancellation", "<fdm:flightPlanCancellation/>")
        .replace(r#" msgType="flightPlanCancellation""#, "");
    let e = field_err(err(&wrap_flight(&nomsg)));
    assert_eq!((e.field, e.kind), (Field::MsgType, FieldErrorKind::Missing));
}

// ---- OA-TFMS-011 .. 016: refusing, capacity, unknown paths, mutation -----------------------

/// Applies `from -> to` to the track fixture's document and returns the refused field.
fn track_field_err(from: &str, to: &str) -> FieldError {
    let doc = wrap_flight(&track());
    assert!(doc.contains(from), "fixture lacks {from}");
    let (r, rows) = run(&doc.replacen(from, to, 1));
    assert_eq!(flights_len(&rows.flights), 0);
    match r {
        Ok(o) if o.rejected == 1 => o.first_reject.unwrap(),
        other => panic!("not refused: {from} -> {to}: {other:?}"),
    }
}

// REQ: OA-TFMS-011
#[test]
fn every_field_error_names_the_field_and_the_kind() {
    use FieldErrorKind as K;
    let cases: &[(&str, &str, Field, K)] = &[
        // identity
        (
            "<nxce:aircraftId>TST123<",
            "<nxce:aircraftId>tst 123<",
            Field::Acid,
            K::BadChar,
        ),
        (
            "<nxce:aircraftId>TST123<",
            "<nxce:aircraftId>ABCDEFGHIJ<",
            Field::Acid,
            K::TooLong,
        ),
        (
            "<nxce:airport>KTAA<",
            "<nxce:airport>KTA-A<",
            Field::Dep,
            K::Inconsistent,
        ),
        (
            "flightRef=\"100000001\"",
            "flightRef=\"1x\"",
            Field::FlightRef,
            K::Format,
        ),
        (
            "flightRef=\"100000001\"",
            "flightRef=\"99999999999\"",
            Field::FlightRef,
            K::Format,
        ),
        // times
        (
            "sourceTimeStamp=\"2030-01-02T03:04:05Z\"",
            "sourceTimeStamp=\"2030-01-02 03:04:05Z\"",
            Field::SrcTime,
            K::Time(TimeError::Shape),
        ),
        (
            "<nxce:igtd>2030-01-02T01:00:00Z<",
            "<nxce:igtd>2030-13-02T01:00:00Z<",
            Field::Igtd,
            K::Time(TimeError::Range),
        ),
        (
            "<nxcm:timeAtPosition>2030-01-02T03:04:00Z<",
            "<nxcm:timeAtPosition>2030-01-02T03:04:00<",
            Field::PosTime,
            K::Time(TimeError::Shape),
        ),
        (
            "arrTime=\"2030-01-02T04:00:00Z\"",
            "arrTime=\"2030-01-02T0a:00:00Z\"",
            Field::ArrFixTime,
            K::Time(TimeError::Digit),
        ),
        (
            "timeValue=\"2030-01-02T04:30:00Z\"",
            "timeValue=\"2030-01-02T04:30:00.Z\"",
            Field::Eta,
            K::Time(TimeError::Shape),
        ),
        // position, altitude, speed
        (
            "degrees=\"41\"",
            "degrees=\"91\"",
            Field::Position,
            K::Range,
        ),
        (
            "minutes=\"46\"",
            "minutes=\"60\"",
            Field::Position,
            K::Range,
        ),
        ("seconds=\"24\"", "seconds=\"x\"", Field::LatSec, K::Format),
        (
            "direction=\"NORTH\"",
            "direction=\"UP\"",
            Field::LatDir,
            K::Vocabulary,
        ),
        (
            "direction=\"WEST\"",
            "direction=\"NORTH\"",
            Field::LonDir,
            K::Vocabulary,
        ),
        (
            "degrees=\"084\"",
            "degrees=\"181\"",
            Field::Position,
            K::Range,
        ),
        (
            "<nxcm:speed>276<",
            "<nxcm:speed>2x6<",
            Field::Speed,
            K::Format,
        ),
        (
            "<nxce:simpleAltitude>350C<",
            "<nxce:simpleAltitude>35Cx<",
            Field::Alt,
            K::Format,
        ),
        (
            "<nxce:simpleAltitude>350C<",
            "<nxce:simpleAltitude>99999<",
            Field::Alt,
            K::Format,
        ),
        (
            "latitudeDecimal=\"41.9\"",
            "latitudeDecimal=\"4x.9\"",
            Field::NextLat,
            K::Format,
        ),
        (
            "longitudeDecimal=\"-84.5\"",
            "longitudeDecimal=\"-184.5\"",
            Field::NextLon,
            K::Range,
        ),
        (
            "fixName=\"ARRFX\"",
            "fixName=\"arr fix\"",
            Field::ArrFix,
            K::BadChar,
        ),
        // flags
    ];
    let mut wrong = Vec::new();
    for (from, to, field, kind) in cases {
        let doc = wrap_flight(&track());
        assert!(doc.contains(from), "fixture lacks {from}");
        match decode_bytes(
            &mut Parser::new(default_limits()).unwrap(),
            doc.replacen(from, to, 1).as_bytes(),
            &mut new_rows(
                4,
                4,
                1 << 12,
                oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
            ),
        ) {
            Ok(Outcome {
                rejected: 1,
                first_reject: Some(e),
                flight_rows: 0,
                ..
            }) if (e.field, e.kind) == (*field, *kind) => {
                assert!(!field_name(e.field).is_empty());
            }
            other => wrong.push(format!("{from} -> {to}: {other:?}")),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    // a missing latitude without a longitude is a position error
    let e = track_field_err(
        "<nxce:latitude><nxce:latitudeDMS degrees=\"41\" direction=\"NORTH\" minutes=\"46\" seconds=\"24\"/></nxce:latitude>",
        "",
    );
    assert_eq!(
        (e.field, e.kind),
        (Field::Position, FieldErrorKind::Missing)
    );
    // a field error carries the offset of the offending value
    let doc = wrap_flight(&track());
    let at = doc.find("100000001").unwrap() as u32;
    let e = field_err(err(&doc.replace("100000001", "1x")));
    assert_eq!(e.at, at);
    for k in [
        FieldErrorKind::Missing,
        FieldErrorKind::TooLong,
        FieldErrorKind::BadChar,
        FieldErrorKind::Format,
        FieldErrorKind::Time(TimeError::Shape),
        FieldErrorKind::Vocabulary,
        FieldErrorKind::Range,
        FieldErrorKind::Duplicate,
        FieldErrorKind::Inconsistent,
        FieldErrorKind::NotLeaf,
        FieldErrorKind::Text,
    ] {
        assert!(!error_kind_name(k).is_empty());
    }
}

// REQ: OA-TFMS-011
#[test]
fn duplicates_nesting_and_text_errors_are_refused() {
    use FieldErrorKind as K;
    // a value given twice
    let e = track_field_err(
        "<nxcm:speed>276</nxcm:speed>",
        "<nxcm:speed>276</nxcm:speed><nxcm:speed>277</nxcm:speed>",
    );
    assert_eq!((e.field, e.kind), (Field::Speed, K::Duplicate));
    let e = track_field_err(
        "<nxce:gufi>ZZ000001aa</nxce:gufi>",
        "<nxce:gufi>ZZ000001aa</nxce:gufi><nxce:gufi>ZZ000001ab</nxce:gufi>",
    );
    assert_eq!((e.field, e.kind), (Field::Gufi, K::Duplicate));
    // a leaf with children
    let e = track_field_err(
        "<nxcm:speed>276</nxcm:speed>",
        "<nxcm:speed>2<b/>76</nxcm:speed>",
    );
    assert_eq!((e.field, e.kind), (Field::Speed, K::NotLeaf));
    // a leaf with an unknown entity or with a CDATA of the wrong shape is a text error
    let e = track_field_err(
        "<nxcm:speed>276</nxcm:speed>",
        "<nxcm:speed>27&#x7f;</nxcm:speed>",
    );
    assert_eq!(e.field, Field::Speed);
    assert!(matches!(e.kind, K::Text | K::Format | K::BadChar));
    // an empty or whitespace-only leaf is an absent value, not a zero
    for empty in ["<nxcm:speed> </nxcm:speed>", "<nxcm:speed/>"] {
        let x = wrap_flight(&track()).replace("<nxcm:speed>276</nxcm:speed>", empty);
        let (_, rows) = ok(&x);
        assert_eq!(rows.flights.speed_milli[0], NO_NUM);
    }
}

// REQ: OA-TFMS-012
#[test]
fn one_bad_message_is_rolled_back_and_its_siblings_are_kept() {
    use oa_tfms::error_kind_index;
    let dup = track().replace(
        "<nxcm:speed>276</nxcm:speed>",
        "<nxcm:speed>276</nxcm:speed><nxcm:speed>277</nxcm:speed><nxcm:unseen/>",
    );
    let bad_text = track_route().replace(
        "<nxcm:remarksKeywords>NRP</nxcm:remarksKeywords>",
        "<nxcm:remarksKeywords>NRP</nxcm:remarksKeywords><nxcm:remarksKeywords>X</nxcm:remarksKeywords>",
    );
    // good, bad, bad (after its text was stored), good: only the two good ones are kept
    let (o, rows) = ok(&wrap_flight(&[track(), dup, bad_text, modify()].concat()));
    let (clean, clean_rows) = ok(&wrap_flight(&[track(), modify()].concat()));
    assert_eq!((o.messages, o.flight_rows, o.rejected), (4, 2, 2));
    assert_eq!(rows.flights.kind, [kind::TRACK, kind::MODIFY]);
    assert_eq!(rows.flights.acid, clean_rows.flights.acid);
    // the text table and the path counters are back to what the good messages alone give
    assert_eq!(rows.strings_used, clean_rows.strings_used);
    assert_eq!(
        (o.unknown_paths, o.ignored_paths),
        (clean.unknown_paths, clean.ignored_paths)
    );
    let first = o.first_reject.unwrap();
    assert_eq!(
        (first.field, first.kind),
        (Field::Speed, FieldErrorKind::Duplicate)
    );
    assert_eq!(o.reject_fields[Field::Speed as usize], 1);
    assert_eq!(o.reject_fields[Field::Remarks as usize], 1);
    assert_eq!(o.reject_fields.iter().sum::<u32>(), 2);
    assert_eq!(
        o.reject_kinds[error_kind_index(FieldErrorKind::Duplicate)],
        2
    );
    assert_eq!(o.reject_kinds.iter().sum::<u32>(), 2);
    assert_eq!(clean.rejected, 0);
    assert_eq!(clean.first_reject, None);
    // an unreadable message element (no message type) is one refused message too
    let nomsg = fm("trackInformation", "<fdm:trackInformation/>")
        .replace(r#" msgType="trackInformation""#, "");
    let (o, rows) = ok(&wrap_flight(&[nomsg, track()].concat()));
    assert_eq!((o.flight_rows, o.rejected), (1, 1));
    assert_eq!(flights_len(&rows.flights), 1);
    // a text-table overflow is a capacity error of the document, which undoes earlier rows too
    let (r, rows) = run_with(&wrap_flight(&[track_route(), plan()].concat()), 16, 8, 40);
    assert_eq!(r.unwrap_err(), DocError::Capacity(CapKind::Strings));
    assert_eq!(flights_len(&rows.flights), 0);
    assert_eq!(rows.strings_used, 0);
    // rows already in the caller's table from earlier documents survive a document whose only
    // message is refused
    let mut rows = new_rows(
        16,
        8,
        1 << 16,
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    );
    let mut p = Parser::new(default_limits()).unwrap();
    decode_bytes(&mut p, wrap_flight(&track_route()).as_bytes(), &mut rows).unwrap();
    let (f, w, s) = (
        flights_len(&rows.flights),
        flows_len(&rows.flows),
        rows.strings_used,
    );
    assert_eq!((f, w), (1, 0));
    let bad = track().replace("100000001", "1x");
    let o = decode_bytes(&mut p, wrap_flight(&bad).as_bytes(), &mut rows).unwrap();
    assert_eq!((o.rejected, o.flight_rows), (1, 0));
    assert_eq!(
        (
            flights_len(&rows.flights),
            flows_len(&rows.flows),
            rows.strings_used
        ),
        (f, w, s)
    );
    assert_eq!(
        text(&rows, flight_at(&rows.flights, 0).unwrap().route),
        "KTAA./.ABC..DEF.STAR1.KTBB/0100"
    );
    // the same holds for a flow document: the refused message goes, the other stays
    let bad_flow = rstr().replace(
        r#"sourceTimeStamp="2030-01-02T03:04:05Z""#,
        r#"sourceTimeStamp="soon""#,
    );
    let (o, rows) = ok(&wrap_fi(&[rstr(), bad_flow].concat()));
    assert_eq!((o.flow_rows, o.rejected), (1, 1));
    assert_eq!(flows_len(&rows.flows), 1);
    let first = o.first_reject.unwrap();
    assert_eq!(first.field, Field::SrcTime);
}

// REQ: OA-TFMS-013
#[test]
fn documents_that_are_not_tfms_or_not_xml_are_refused_by_reason() {
    use oa_tfms::{
        reject_reason, REJECT_CAP_FLIGHTS, REJECT_CAP_FLOWS, REJECT_CAP_STRINGS, REJECT_FIELD,
        REJECT_NAMES, REJECT_NOT_TFMS, REJECT_XML,
    };
    let xml = err("<ds:tfmDataService xmlns:ds=\"x\"><ds:fltdOutput></ds:tfmDataService>");
    assert!(matches!(xml, DocError::Xml(_)));
    assert_eq!(reject_reason(&xml), REJECT_XML);
    assert!(matches!(err(""), DocError::Xml(_)));
    assert!(matches!(err("not xml at all"), DocError::Xml(_)));
    for doc in [
        // wrong root name
        format!(r#"<ds:other xmlns:ds="{SVC}"/>"#),
        // right name, wrong namespace
        r#"<ds:tfmDataService xmlns:ds="urn:example:other"/>"#.to_string(),
        // no namespace at all
        "<tfmDataService/>".to_string(),
    ] {
        let e = err(&doc);
        assert!(matches!(e, DocError::NotTfms { .. }), "{doc}");
        assert_eq!(reject_reason(&e), REJECT_NOT_TFMS);
    }
    let e = err(&wrap_flight(&track()).replace("100000001", "1x"));
    assert_eq!(reject_reason(&e), REJECT_FIELD);
    assert_eq!(
        [
            reject_reason(&DocError::Capacity(CapKind::Flights)),
            reject_reason(&DocError::Capacity(CapKind::Flows)),
            reject_reason(&DocError::Capacity(CapKind::Strings)),
        ],
        [REJECT_CAP_FLIGHTS, REJECT_CAP_FLOWS, REJECT_CAP_STRINGS]
    );
    assert_eq!(REJECT_NAMES.len(), 6);
    // the kinds of an accepted document
    let status = format!(
        r#"<ds:tfmDataService xmlns:ds="{SVC}"><ds:tfmsStatusOutput><ds:status text="up"/></ds:tfmsStatusOutput></ds:tfmDataService>"#
    );
    let (o, rows) = ok(&status);
    assert_eq!(o.kind, DocKind::Status);
    assert_eq!((o.flight_rows, o.flow_rows), (0, 0));
    assert_eq!(flights_len(&rows.flights), 0);
    let empty = format!(r#"<ds:tfmDataService xmlns:ds="{SVC}"/>"#);
    assert_eq!(ok(&empty).0.kind, DocKind::Empty);
    let both = wrap_flight(&track()).replace(
        "</ds:tfmDataService>",
        &format!(r#"<ds:fiOutput xmlns:ns12="{FI}" xmlns:ns9="{FIC2}" xmlns:ns11="{FIC}" xmlns:ns7="{CORE}">{}</ds:fiOutput></ds:tfmDataService>"#, rstr()),
    );
    let (o, _) = ok(&both);
    assert_eq!((o.kind, o.flight_rows, o.flow_rows), (DocKind::Mixed, 1, 1));
}

// REQ: OA-TFMS-014
#[test]
fn capacity_is_stated_and_never_exceeded() {
    let two = wrap_flight(&[track(), modify()].concat());
    let (r, rows) = run_with(&two, 1, 4, 1 << 16);
    assert_eq!(r.unwrap_err(), DocError::Capacity(CapKind::Flights));
    assert_eq!(flights_len(&rows.flights), 0);
    let (r, rows) = run_with(&two, 2, 4, 1 << 16);
    assert_eq!(r.unwrap().flight_rows, 2);
    assert_eq!(flights_len(&rows.flights), 2);
    let (r, _) = run_with(&wrap_fi(&[rstr(), rstr()].concat()), 4, 1, 1 << 16);
    assert_eq!(r.unwrap_err(), DocError::Capacity(CapKind::Flows));
    let (r, _) = run_with(&wrap_fi(&rstr()), 0, 0, 1 << 16);
    assert_eq!(r.unwrap_err(), DocError::Capacity(CapKind::Flows));
    // the string table: refused at the byte that does not fit, accepted at the exact size
    let doc = wrap_flight(&track_route());
    let (r, rows) = run_with(&doc, 4, 4, 1 << 16);
    r.unwrap();
    let need = rows.strings_used;
    assert!(need > 0);
    let (r, _) = run_with(&doc, 4, 4, need - 1);
    assert_eq!(r.unwrap_err(), DocError::Capacity(CapKind::Strings));
    let (r, rows) = run_with(&doc, 4, 4, need);
    r.unwrap();
    assert_eq!(rows.strings_used, need);
    // pushes past capacity are refused, not grown
    let mut rows = new_rows(1, 1, 8, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    assert!(push_flight(&mut rows.flights, &BLANK_FLIGHT));
    assert!(!push_flight(&mut rows.flights, &BLANK_FLIGHT));
    assert!(push_flow(&mut rows.flows, &BLANK_FLOW));
    assert!(!push_flow(&mut rows.flows, &BLANK_FLOW));
    // the table does not move or grow while decoding: no allocation after init
    let mut rows = new_rows(
        8,
        4,
        1 << 12,
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    );
    let before = (
        rows.flights.acid.as_ptr(),
        rows.flights.acid.capacity(),
        rows.flows.text.as_ptr(),
        rows.flows.text.capacity(),
        rows.strings.as_ptr(),
        rows.strings.len(),
    );
    let mut p = Parser::new(default_limits()).unwrap();
    for _ in 0..3 {
        decode_bytes(&mut p, doc.as_bytes(), &mut rows).unwrap();
        decode_bytes(&mut p, wrap_fi(&rstr()).as_bytes(), &mut rows).unwrap();
        clear_rows(&mut rows);
    }
    let after = (
        rows.flights.acid.as_ptr(),
        rows.flights.acid.capacity(),
        rows.flows.text.as_ptr(),
        rows.flows.text.capacity(),
        rows.strings.as_ptr(),
        rows.strings.len(),
    );
    assert_eq!(before, after);
    assert_eq!(
        (
            flights_len(&rows.flights),
            flows_len(&rows.flows),
            rows.strings_used
        ),
        (0, 0, 0)
    );
    // truncate_rows rolls back to a mark
    decode_bytes(&mut p, doc.as_bytes(), &mut rows).unwrap();
    let mark = (
        flights_len(&rows.flights),
        flows_len(&rows.flows),
        rows.strings_used,
    );
    decode_bytes(&mut p, doc.as_bytes(), &mut rows).unwrap();
    truncate_rows(&mut rows, mark.0, mark.1, mark.2);
    assert_eq!(
        (
            flights_len(&rows.flights),
            flows_len(&rows.flows),
            rows.strings_used
        ),
        mark
    );
}

// REQ: OA-TFMS-015
#[test]
fn unknown_paths_are_counted_and_do_not_refuse_the_document() {
    let base = wrap_flight(&track());
    let (o0, rows0) = ok(&base);
    // an unknown element in a known place, with its own children
    let x = base.replace(
        "<nxcm:speed>",
        "<nxcm:surprise a=\"1\"><nxcm:inner/></nxcm:surprise><nxcm:speed>",
    );
    let (o, rows) = ok(&x);
    assert!(o.unknown_paths > o0.unknown_paths);
    assert_eq!(o.flight_rows, 1);
    assert_eq!(rows.flights.speed_milli, rows0.flights.speed_milli);
    // an unknown attribute on a known element
    let x = base.replace("sensitivity=\"A\"", "sensitivity=\"A\" novel=\"1\"");
    let (o, _) = ok(&x);
    assert_eq!(o.unknown_paths, o0.unknown_paths + 1);
    // an unknown message type: an identity-only row of kind OTHER, the body counted unknown
    let unknown = wrap_flight(&fm("brandNewMessage", "<fdm:brandNewMessage/>"));
    let (o, rows) = ok(&unknown);
    assert_eq!((o.messages, o.flight_rows), (1, 1));
    assert!(o.unknown_paths >= 1);
    assert_eq!(rows.flights.kind[0], kind::OTHER);
    assert_eq!(nm(&rows.flights.acid[0]), "TST123");
    assert_eq!(rows.flights.speed_milli[0], NO_NUM);
    // an unknown element in a foreign namespace is not mistaken for a known name
    let x = base.replace(
        "<nxcm:speed>276</nxcm:speed>",
        "<nxce:speed>999</nxce:speed><nxcm:speed>276</nxcm:speed>",
    );
    let (o, rows) = ok(&x);
    assert!(o.unknown_paths > o0.unknown_paths);
    assert_eq!(rows.flights.speed_milli, rows0.flights.speed_milli);
    // the same element in another prefix is the same element: names are matched by namespace
    let x = base
        .replace("xmlns:nxcm=", "xmlns:zz=")
        .replace("<nxcm:", "<zz:")
        .replace("</nxcm:", "</zz:");
    let (o, rows) = ok(&x);
    assert_eq!(
        (o.unknown_paths, o.ignored_paths),
        (o0.unknown_paths, o0.ignored_paths)
    );
    assert_eq!(rows.flights.alt_ft, rows0.flights.alt_ft);
    assert_eq!(rows.flights.speed_milli, rows0.flights.speed_milli);
}

// REQ: OA-TFMS-016
#[test]
fn mutating_documents_never_panics() {
    let seeds = [
        wrap_flight(
            &[
                track_route(),
                modify(),
                plan(),
                departure(),
                boundary(),
                oceanic(),
            ]
            .concat(),
        ),
        wrap_fi(&[rstr(), gadv(), fxa(), aptc(), tmi_list()].concat()),
    ];
    let mut rows = new_rows(
        64,
        32,
        1 << 16,
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    );
    let mut p = Parser::new(default_limits()).unwrap();
    let mut accepted = 0u32;
    let mut refused = 0u32;
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for seed in &seeds {
        let bytes = seed.as_bytes();
        // every truncation
        for n in 0..bytes.len() {
            let r = decode_bytes(&mut p, &bytes[..n], &mut rows);
            if r.is_ok() {
                accepted += 1;
            } else {
                refused += 1;
            }
            clear_rows(&mut rows);
        }
        // single byte replacements at every position, a few values each
        for i in 0..bytes.len() {
            for v in [0u8, b'<', b'"', b'9'] {
                let mut m = bytes.to_vec();
                m[i] = v;
                match decode_bytes(&mut p, &m, &mut rows) {
                    Ok(_) => accepted += 1,
                    Err(_) => refused += 1,
                }
                clear_rows(&mut rows);
            }
        }
        // random multi-byte damage and deletions
        for _ in 0..1500 {
            let mut m = bytes.to_vec();
            for _ in 0..1 + next() % 4 {
                let i = (next() as usize) % m.len();
                match next() % 3 {
                    0 => m[i] = next() as u8,
                    1 => {
                        m.remove(i);
                    }
                    _ => m.insert(i, next() as u8),
                }
            }
            match decode_bytes(&mut p, &m, &mut rows) {
                Ok(_) => accepted += 1,
                Err(_) => refused += 1,
            }
            // whatever happened, the columns stay the same length
            let n = flights_len(&rows.flights);
            assert_eq!(rows.flights.acid.len(), n);
            assert_eq!(rows.flights.route.len(), n);
            let w = flows_len(&rows.flows);
            assert_eq!(rows.flows.text.len(), w);
            assert!(rows.strings_used <= rows.strings.len());
            clear_rows(&mut rows);
        }
    }
    assert!(
        accepted > 0 && refused > 0,
        "{accepted} accepted, {refused} refused"
    );
}

// ---- OA-TFMS-019 .. 022: counters, linkage, layout -----------------------------------------

// REQ: OA-TFMS-019
#[test]
fn stats_fold_outcomes_rejects_and_field_errors() {
    let mut s = new_stats();
    let mut rows = new_rows(
        16,
        8,
        1 << 16,
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    );
    let mut p = Parser::new(default_limits()).unwrap();
    let docs = [
        wrap_flight(&[track(), modify()].concat()),
        wrap_fi(&rstr()),
        wrap_flight(&track().replace("100000001", "1x")),
        "<broken".to_string(),
        format!(r#"<a:other xmlns:a="{SVC}"/>"#),
    ];
    for d in &docs {
        let f0 = flights_len(&rows.flights);
        let w0 = flows_len(&rows.flows);
        let r = decode_bytes(&mut p, d.as_bytes(), &mut rows);
        observe(&mut s, &r, &rows.flights.kind[f0..], &rows.flows.kind[w0..]);
    }
    // the document with the one refused message is accepted (it has no rows)
    assert_eq!((s.documents, s.accepted), (5, 3));
    assert_eq!((s.messages, s.flight_rows, s.flow_rows), (4, 2, 1));
    assert_eq!(s.rejected_messages, 1);
    assert_eq!(s.flight_kinds[usize::from(kind::TRACK)], 1);
    assert_eq!(s.flight_kinds[usize::from(kind::MODIFY)], 1);
    assert_eq!(s.flow_kinds[usize::from(fkind::RSTR)], 1);
    assert_eq!(s.doc_kinds[DocKind::Flight as usize], 2);
    assert_eq!(s.doc_kinds[DocKind::FlowInfo as usize], 1);
    assert_eq!(s.rejects.iter().sum::<u64>(), 2);
    assert_eq!(s.rejects[oa_tfms::REJECT_XML], 1);
    assert_eq!(s.rejects[oa_tfms::REJECT_NOT_TFMS], 1);
    assert_eq!(s.rejects[oa_tfms::REJECT_FIELD], 0);
    assert_eq!(s.field_errors[Field::FlightRef as usize], 1);
    assert_eq!(s.field_errors.iter().sum::<u64>(), 1);
    assert_eq!(
        s.reject_kinds[oa_tfms::error_kind_index(FieldErrorKind::Format)],
        1
    );
    assert_eq!(s.unknown_words, 0);
    assert_eq!(FIELD_COUNT, s.field_errors.len());
}

/// One SFDPS row with just the columns the linkage reads.
fn sfdps(ev: &mut oa_fixm::Events, gufi: u128, plan: &str, acid: &str, dep: &str, arr: &str) {
    fn pad<const N: usize>(s: &str) -> [u8; N] {
        let mut a = [0u8; N];
        a[..s.len()].copy_from_slice(s.as_bytes());
        a
    }
    let mut present = 0;
    if !plan.is_empty() {
        present |= oa_fixm::field_bit(oa_fixm::Field::FlightPlanId);
    }
    if !acid.is_empty() {
        present |= oa_fixm::field_bit(oa_fixm::Field::Callsign);
    }
    if !dep.is_empty() {
        present |= oa_fixm::field_bit(oa_fixm::Field::Departure);
    }
    if !arr.is_empty() {
        present |= oa_fixm::field_bit(oa_fixm::Field::Arrival);
    }
    ev.present.push(present);
    ev.gufi.push(gufi);
    ev.plan_id.push(pad(plan));
    ev.callsign.push(pad(acid));
    ev.departure.push(pad(dep));
    ev.arrival.push(pad(arr));
    ev.len += 1;
}

/// A TFMS flight row for the linkage.
fn tfms(rows: &mut Rows, flight_ref: u64, gufi: &str, acid: &str, dep: &str, arr: &str) {
    let mut r = BLANK_FLIGHT;
    r.flight_ref = flight_ref;
    r.flags |= flag::FLIGHT_REF;
    for (dst, src) in [
        (&mut r.gufi[..], gufi),
        (&mut r.acid[..], acid),
        (&mut r.dep[..], dep),
        (&mut r.arr[..], arr),
    ] {
        dst[..src.len()].copy_from_slice(src.as_bytes());
    }
    assert!(push_flight(&mut rows.flights, &r));
}

// REQ: OA-TFMS-020
#[test]
fn linkage_matches_by_gufi_then_by_acid_departure_arrival() {
    use oa_tfms::{format_link_report, link_flights};
    let mut ev = oa_fixm::new_events(16, 64);
    // plan identifiers and keys
    sfdps(&mut ev, 1, "AB00000001", "TST1", "KTAA", "KTBB");
    sfdps(&mut ev, 1, "AB00000001", "TST1", "KTAA", "KTBB"); // a second row of the same flight
    sfdps(&mut ev, 2, "AB00000002", "TST2", "KTAA", "KTCC");
    sfdps(&mut ev, 3, "", "TST3", "KTDD", "KTEE"); // no plan identifier, has a key
    sfdps(&mut ev, 4, "AB00000004", "", "", ""); // plan identifier only
    let mut rows = new_rows(16, 1, 16, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    tfms(&mut rows, 10, "AB00000001", "TST1", "KTAA", "KTBB"); // by gufi, agrees by key
    tfms(&mut rows, 10, "AB00000001", "TST1", "KTAA", "KTBB"); // same flight, second message
    tfms(&mut rows, 11, "ZZ99999999", "TST2", "KTAA", "KTCC"); // gufi unknown, key matches
    tfms(&mut rows, 12, "", "TST3", "KTDD", "KTEE"); // no gufi: by key
    tfms(&mut rows, 13, "", "TST9", "KTDD", "KTEE"); // no gufi: no hit
    tfms(&mut rows, 14, "", "TST4", "", "KTEE"); // no gufi and no key
    tfms(&mut rows, 15, "AB00000004", "TST4", "KTAA", "KTBB"); // by gufi only
    tfms(&mut rows, 16, "AB00000002", "TST1", "KTAA", "KTBB"); // plan id says flight 2, key says 1
    let rep = link_flights(&ev, &rows);
    assert_eq!(
        (rep.sfdps_rows, rep.sfdps_flights, rep.sfdps_with_plan_id),
        (5, 4, 3)
    );
    assert_eq!((rep.tfms_rows, rep.tfms_flights), (8, 7));
    assert_eq!((rep.tfms_with_gufi, rep.tfms_without_gufi), (4, 3));
    assert_eq!(rep.by_gufi, 3);
    assert_eq!((rep.by_acid_no_gufi, rep.no_gufi_no_key), (1, 1));
    assert_eq!(rep.gufi_flights_also_by_acid, 2);
    assert_eq!((rep.agree, rep.disagree), (1, 1));
    assert_eq!(rep.acid_only_among_gufi, 1);
    assert_eq!((rep.tfms_uuid_shaped, rep.uuid_matched), (0, 0));
    assert_eq!(rep.ambiguous_acid_keys, 0);
    let s = format_link_report(&rep);
    assert!(s.contains("matched by gufi = SFDPS plan identifier: 3 of 4"));
    assert!(s.contains("no gufi, matched by acid+departure+arrival: 1 of 3"));
    assert!(s.contains("total matched: 4 of 7"));
}

// REQ: OA-TFMS-021
#[test]
fn linkage_reports_uuid_shaped_gufis_and_ambiguous_keys_as_found() {
    use oa_tfms::link_flights;
    let mut ev = oa_fixm::new_events(8, 64);
    sfdps(&mut ev, 1, "AB00000001", "TST1", "KTAA", "KTBB");
    sfdps(&mut ev, 2, "AB00000002", "TST1", "KTAA", "KTBB"); // the same key, another flight
    let mut rows = new_rows(4, 1, 16, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    tfms(&mut rows, 1, "", "TST1", "KTAA", "KTBB");
    // a TFMS gufi longer than the 16-byte name is refused by the decoder, so it never reaches the
    // linkage: a UUID-shaped gufi would be a field error, not a silent non-match
    let long = "00000000-0000-4000-8000-000000000001";
    let e = field_err(err(&wrap_flight(&track()).replace("ZZ000001aa", long)));
    assert_eq!((e.field, e.kind), (Field::Gufi, FieldErrorKind::TooLong));
    // an empty feed pair reports zeros, not an error
    let rep = link_flights(&ev, &rows);
    assert_eq!(rep.ambiguous_acid_keys, 1);
    assert_eq!(
        (
            rep.tfms_without_gufi,
            rep.tfms_uuid_shaped,
            rep.uuid_matched
        ),
        (1, 0, 0)
    );
    assert_eq!(rep.by_acid_no_gufi, 1);
    assert_eq!(rep.by_gufi, 0);
    let ev2 = oa_fixm::new_events(1, 8);
    let rows2 = new_rows(1, 1, 8, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let rep = link_flights(&ev2, &rows2);
    assert_eq!(rep, oa_tfms::LinkReport::default());
    // decoded rows link too: the flight of a handwritten message finds its plan identifier
    let (_, rows3) = ok(&wrap_flight(&track()));
    let mut ev3 = oa_fixm::new_events(2, 64);
    sfdps(&mut ev3, 9, "ZZ000001aa", "TST123", "KTAA", "KTBB");
    let rep = link_flights(&ev3, &rows3);
    assert_eq!((rep.by_gufi, rep.agree, rep.disagree), (1, 1, 0));
}

// REQ: OA-TFMS-022
#[test]
fn the_default_capacities_and_the_vocabularies_are_consistent() {
    use oa_tfms::rows::{DEFAULT_FLIGHT_CAPACITY, DEFAULT_FLOW_CAPACITY, DEFAULT_STRING_BYTES};
    let rows = new_rows(
        DEFAULT_FLIGHT_CAPACITY,
        DEFAULT_FLOW_CAPACITY,
        DEFAULT_STRING_BYTES,
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    );
    assert_eq!(rows.flights.acid.capacity(), DEFAULT_FLIGHT_CAPACITY);
    assert_eq!(rows.flows.text.capacity(), DEFAULT_FLOW_CAPACITY);
    assert_eq!(rows.strings.len(), DEFAULT_STRING_BYTES);
    // codes are dense: every name table has the count the codes allow
    assert_eq!(kind::NAMES.len(), kind::COUNT);
    assert_eq!(fkind::NAMES.len(), fkind::COUNT);
    assert!(prog::NAMES.len() > usize::from(prog::AIRPORT_CONFIG));
    assert!(trig::NAMES.len() > 1);
    // a blank row is all absent
    let b = BLANK_FLIGHT;
    assert_eq!(
        (b.src_ms, b.eta_ms, b.lat_e7, b.alt_ft),
        (NO_TIME, NO_TIME, NO_COORD, NO_NUM)
    );
    assert_eq!(BLANK_FLOW.text, NO_STR);
    assert_eq!(field_name(Field::FStatus), "tmiStatus");
    assert_eq!(field_name(Field::MsgType), "msgType");
    // a few walker constants
    const { assert!(MAX_DEPTH >= 8) };
    assert!(!BODIES.is_empty() && !FLOW_BODIES.is_empty());
    let _ = FLIGHT_DATA;
}

// ---- OA-TFMS-023 .. 026: messages refused alone, unknown words, wide ids, split text -------

// REQ: OA-TFMS-023
#[test]
fn a_tmi_flight_list_keeps_its_valid_entries_when_one_is_refused() {
    let list = |entries: &[String]| {
        fi(
            "TMI_FLIGHT_LIST",
            &format!(
                "<ns12:tmiFlightDataList>{}</ns12:tmiFlightDataList>",
                entries.concat()
            ),
        )
    };
    let bad = tmi_entry(
        "TST2",
        "100000011",
        "<ns9:flightReference>5</ns9:flightReference>",
    );
    let (o, rows) = ok(&wrap_fi(&list(&[
        tmi_entry("TST1", "100000010", ""),
        bad,
        tmi_entry("TST3", "100000012", ""),
    ])));
    assert_eq!((o.messages, o.flight_rows, o.rejected), (1, 2, 1));
    assert_eq!(rows.flights.flight_ref, [100_000_010, 100_000_012]);
    assert_eq!(
        o.first_reject.map(|e| (e.field, e.kind)),
        Some((Field::FlightRef, FieldErrorKind::Duplicate))
    );
    // an entry without a flight reference is refused alone too
    let bare = "<ns12:flightData><ns9:flight><ns7:aircraftId>TST4</ns7:aircraftId></ns9:flight></ns12:flightData>".to_string();
    let (o, rows) = ok(&wrap_fi(&list(&[bare, tmi_entry("TST1", "100000010", "")])));
    assert_eq!((o.flight_rows, o.rejected), (1, 1));
    assert_eq!(rows.flights.flight_ref, [100_000_010]);
}

// REQ: OA-TFMS-024
#[test]
fn a_word_outside_a_vocabulary_is_kept_as_unknown_and_counted() {
    use oa_tfms::rows::UNKNOWN_WORD;
    let doc = wrap_flight(&track())
        .replace("HCS_TRACK_MSG", "BRAND_NEW_TRIGGER")
        .replace("aircraftCategory=\"JET\"", "aircraftCategory=\"ROCKET\"")
        .replace("userCategory=\"COMMERCIAL\"", "userCategory=\"PIRATE\"")
        .replace("cdmPart=\"false\"", "cdmPart=\"maybe\"")
        .replace("etaType=\"ESTIMATED\"", "etaType=\"GUESSED\"")
        .replace("equipped=\"true\"", "equipped=\"yes\"");
    let (o, rows) = ok(&doc);
    assert_eq!((o.flight_rows, o.rejected, o.unknown_words), (1, 0, 6));
    let r = flight_at(&rows.flights, 0).unwrap();
    assert_eq!(r.trigger, UNKNOWN_WORD);
    assert_eq!(
        (r.ac_cat, r.user_cat, r.eta_type),
        (UNKNOWN_WORD, UNKNOWN_WORD, UNKNOWN_WORD)
    );
    assert_eq!(flag_get(r.flags, flag::CDM_PART), flag::UNKNOWN);
    assert_eq!(flag_get(r.flags, flag::RVSM_EQ), flag::UNKNOWN);
    // everything else in the message is read as usual
    assert_eq!(nm(&r.acid), "TST123");
    assert_eq!((r.speed_milli, r.speed_unit), (276_000, spd_unit::KNOTS));
    assert_eq!(r.lat_e7, 417_733_333);
    // the other vocabulary fields: a restriction category and scope, a general flow status
    let flow = wrap_fi(&rstr())
        .replace(
            "<ns11:restrictionCategory>MIT<",
            "<ns11:restrictionCategory>XYZ<",
        )
        .replace(
            "<ns11:restrictionType>ARRIVAL<",
            "<ns11:restrictionType>SIDEWAYS<",
        );
    let (o, rows) = ok(&flow);
    assert_eq!((o.flow_rows, o.rejected, o.unknown_words), (1, 0, 2));
    assert_eq!(rows.flows.program[0], UNKNOWN_WORD);
    assert_eq!(rows.flows.scope[0], UNKNOWN_WORD);
    // the direction of a position cannot be unknown: the message is refused
    let e = field_err(err(
        &wrap_flight(&track()).replace("direction=\"NORTH\"", "direction=\"UP\"")
    ));
    assert_eq!(
        (e.field, e.kind),
        (Field::LatDir, FieldErrorKind::Vocabulary)
    );
    // a known word is still stored as itself, and the count adds up in the statistics
    let mut s = new_stats();
    let r = decode_bytes(
        &mut Parser::new(default_limits()).unwrap(),
        doc.as_bytes(),
        &mut new_rows(
            4,
            4,
            1 << 12,
            oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
        ),
    );
    observe(&mut s, &r, &[], &[]);
    assert_eq!(s.unknown_words, 6);
    assert_eq!(s.rejected_messages, 0);
}

// REQ: OA-TFMS-025
#[test]
fn ten_digit_identifiers_do_not_overflow() {
    let (_, _, r) = one_flight(&track().replace("100000001", "9999999999"));
    assert_eq!(r.flight_ref, 9_999_999_999);
    let (_, _, r) = one_flight(&track().replace("100000001", "4294967296"));
    assert_eq!(r.flight_ref, 4_294_967_296);
    let big = rstr()
        .replace(
            "<ns11:restrictionId>20300101<",
            "<ns11:restrictionId>5000000000<",
        )
        .replace("<ns11:mitNumber>278529<", "<ns11:mitNumber>9999999999<")
        .replace(
            "<ns11:referenceRestrictionId>7<",
            "<ns11:referenceRestrictionId>4294967295<",
        );
    let (_, _, w) = one_flow(&big);
    assert_eq!(
        (w.id, w.number, w.ref_id),
        (5_000_000_000, 9_999_999_999, 4_294_967_295)
    );
    // eleven digits is still too many
    let e = field_err(err(&wrap_flight(
        &track().replace("100000001", "10000000000"),
    )));
    assert_eq!(
        (e.field, e.kind),
        (Field::FlightRef, FieldErrorKind::Format)
    );
    assert_eq!(
        oa_tfms::scalar::parse_digits64(b"18446744073709551615", 20),
        Ok(u64::MAX)
    );
    assert_eq!(
        oa_tfms::scalar::parse_digits64(b"18446744073709551616", 20),
        Err(FieldErrorKind::Format)
    );
    assert_eq!(
        oa_tfms::scalar::parse_digits64(b"", 10),
        Err(FieldErrorKind::Format)
    );
}

// REQ: OA-TFMS-026
#[test]
fn text_split_by_a_comment_or_cdata_is_joined() {
    let (_, _, r) = one_flight(&track().replace(
        "<nxcm:speed>276</nxcm:speed>",
        "<nxcm:speed>2<!--note-->7<![CDATA[6]]></nxcm:speed>",
    ));
    assert_eq!((r.speed_milli, r.speed_unit), (276_000, spd_unit::KNOTS));
    // a name, and free text that goes into the text table
    let route = track_route().replace(
        "<nxcm:routeOfFlight>KTAA./.ABC..DEF.STAR1.KTBB/0100</nxcm:routeOfFlight>",
        "<nxcm:routeOfFlight>KTAA./.ABC<!--x-->..DEF.<![CDATA[STAR1]]>.KTBB/0100</nxcm:routeOfFlight>",
    );
    let (_, rows, r) = one_flight(&route);
    assert_eq!(text(&rows, r.route), "KTAA./.ABC..DEF.STAR1.KTBB/0100");
    let a = track().replace(
        "<nxce:aircraftId>TST123</nxce:aircraftId>",
        "<nxce:aircraftId>TST<!--x-->123</nxce:aircraftId>",
    );
    let (_, _, r) = one_flight(&a);
    assert_eq!(nm(&r.acid), "TST123");
    // the bound still applies to the joined value: 40 + 40 bytes do not fit a name
    let long = format!("{0}<!--x-->{0}", "A".repeat(40));
    let x = track().replace(
        "<nxce:aircraftId>TST123</nxce:aircraftId>",
        &format!("<nxce:aircraftId>{long}</nxce:aircraftId>"),
    );
    let e = field_err(err(&wrap_flight(&x)));
    assert_eq!((e.field, e.kind), (Field::Acid, FieldErrorKind::TooLong));
    // and a string table that is too small for joined free text is the capacity reject
    let (r, _) = run_with(&wrap_flight(&route), 4, 4, 8);
    assert_eq!(r.unwrap_err(), DocError::Capacity(CapKind::Strings));
}

// REQ: OA-TFMS-027
#[test]
fn a_mach_or_decimal_speed_keeps_the_message_with_its_unit() {
    let with = |v: &str| track().replace("<nxcm:speed>276<", &format!("<nxcm:speed>{v}<"));
    // (text, value in thousandths, unit, counted as unknown)
    for (v, milli, unit, unk) in [
        ("276", 276_000, spd_unit::KNOTS, 0),
        ("276.5", 276_500, spd_unit::KNOTS, 0),
        ("0.82", 820, spd_unit::MACH, 0),
        ("M082", 820, spd_unit::MACH, 0),
        ("N0450", 450_000, spd_unit::KNOTS, 0),
        ("1.2.3", NO_NUM, spd_unit::UNKNOWN, 1),
        ("12345.5", 12_345_500, spd_unit::UNKNOWN, 1),
        ("M08", NO_NUM, spd_unit::UNKNOWN, 1),
    ] {
        let (o, _, r) = one_flight(&with(v));
        assert_eq!((o.rejected, o.unknown_units), (0, unk), "{v}");
        assert_eq!((r.speed_milli, r.speed_unit), (milli, unit), "{v}");
        assert_eq!((r.tas_milli, r.tas_unit), (NO_NUM, spd_unit::NONE), "{v}");
    }
    // The same for the filed true airspeed, and the counter folds into the totals.
    let plan_tas = plan().replace(">450<", ">0.80<");
    let (o, _, r) = one_flight(&plan_tas);
    assert_eq!((o.rejected, o.unknown_units), (0, 0));
    assert_eq!((r.tas_milli, r.tas_unit), (800, spd_unit::MACH));
    let mut s = new_stats();
    let (r, rows) = run(&wrap_flight(&with("1.2.3")));
    let kinds = rows.flights.kind.clone();
    observe(&mut s, &r, &kinds, &[]);
    assert_eq!((s.unknown_units, s.rejected_messages), (1, 0));
    // A byte that is no part of a speed still refuses the message, and rolls the count back.
    let e = field_err(err(&wrap_flight(&with("2x6"))));
    assert_eq!((e.field, e.kind), (Field::Speed, FieldErrorKind::Format));
    let (o, _) = ok(&wrap_flight(&format!("{}{}", with("1.2.3"), with("2x6"))));
    assert_eq!((o.rejected, o.unknown_units), (1, 1));
}

// REQ: OA-TFMS-028
#[test]
fn a_flight_reference_of_zero_is_told_from_no_reference() {
    let zero = track().replace("flightRef=\"100000001\"", "flightRef=\"0000000000\"");
    // The decoder refuses a message with no reference, so a decoded row's reference is always
    // carried, and 0 is a valid one; only a row nobody filled in is without one.
    let (_, _, a) = one_flight(&zero);
    assert_eq!(a.flight_ref, 0);
    assert_eq!(a.flags & flag::FLIGHT_REF, flag::FLIGHT_REF);
    assert_eq!(BLANK_FLIGHT.flags & flag::FLIGHT_REF, 0);
    let none = track().replace("flightRef=\"100000001\" ", "");
    let e = field_err(err(&wrap_flight(&none)));
    assert_eq!(
        (e.field, e.kind),
        (Field::FlightRef, FieldErrorKind::Missing)
    );
    // In the linkage: two rows with no reference are two flights, and a carried 0 is one flight
    // of its own that does not absorb them.
    use oa_tfms::link_flights;
    let ev = oa_fixm::new_events(1, 8);
    let mut rows = new_rows(8, 1, 16, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    tfms(&mut rows, 0, "AB00000001", "TST1", "KTAA", "KTBB"); // reference 0, carried
    tfms(&mut rows, 0, "AB00000001", "TST1", "KTAA", "KTBB"); // the same flight again
    for (acid, dep) in [("TST2", "KTAA"), ("TST3", "KTDD")] {
        tfms(&mut rows, 0, "", acid, dep, "KTBB");
        let i = flights_len(&rows.flights) - 1;
        rows.flights.flags[i] &= !flag::FLIGHT_REF;
    }
    let rep = link_flights(&ev, &rows);
    assert_eq!((rep.tfms_rows, rep.tfms_flights), (4, 3));
    assert_eq!(rep.tfms_rows_without_ref, 2);
}

// REQ: OA-TFMS-029
#[test]
fn a_namespace_written_with_a_character_reference_is_the_same_namespace() {
    let plain = wrap_flight(&track());
    let escaped = plain.replacen(
        &format!("xmlns:fdm=\"{FD}\""),
        &format!("xmlns:fdm=\"{}\"", FD.replacen('u', "&#117;", 1)),
        1,
    );
    assert_ne!(plain, escaped);
    let (o, rows) = ok(&plain);
    let (o2, rows2) = ok(&escaped);
    assert_eq!(o, o2);
    assert_eq!(flights_len(&rows2.flights), flights_len(&rows.flights));
    assert_eq!(rows2.flights.flight_ref[0], rows.flights.flight_ref[0]);
}

/// 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z in Unix ms: the sealed window.
const SEAL: (i64, i64) = (1_795_132_800_000, 1_795_393_020_000);

/// `xml` with every message time set to `ts`.
fn stamped(xml: &str, ts: &str) -> String {
    xml.replace(
        r#"sourceTimeStamp="2030-01-02T03:04:05Z""#,
        &format!(r#"sourceTimeStamp="{ts}""#),
    )
}

/// The outcome of a document dropped whole: only the count.
fn sealed(n: u32) -> Outcome {
    Outcome {
        kind: DocKind::Empty,
        messages: 0,
        flight_rows: 0,
        flow_rows: 0,
        unknown_paths: 0,
        ignored_paths: 0,
        dropped: 0,
        rejected: 0,
        first_reject: None,
        reject_fields: [0; FIELD_COUNT],
        reject_kinds: [0; oa_tfms::ERROR_KIND_COUNT],
        unknown_words: 0,
        unknown_units: 0,
        sealed_dropped: n,
    }
}

/// Decodes `xml` (prefix and plain) with the sealed window into tables of the given sizes, and
/// checks that the outcome is the sealed one, the tables are untouched and the stats are those
/// of an empty run with the drop count set.
fn assert_dropped_whole(xml: &str, flights: usize, flows: usize, n: u32) {
    let w = oa_sealed::DropWindow::new(SEAL.0, SEAL.1);
    let mut p = Parser::new(default_limits()).unwrap();
    for prefix in [false, true] {
        let mut rows = new_rows(flights, flows, 1 << 12, w);
        let doc = p.parse(xml.as_bytes()).unwrap();
        let r = if prefix {
            oa_tfms::decode_doc_prefix(&doc, &mut rows)
        } else {
            oa_tfms::decode_doc(&doc, &mut rows)
        };
        assert_eq!(r, Ok(sealed(n)), "prefix {prefix}");
        assert_eq!(flights_len(&rows.flights), 0);
        assert_eq!(flows_len(&rows.flows), 0);
        assert_eq!(rows.strings_used, 0);
        let mut s = new_stats();
        observe(&mut s, &r, &[], &[]);
        assert_eq!(
            s,
            oa_tfms::Stats {
                sealed_window_dropped: u64::from(n),
                ..new_stats()
            }
        );
    }
}

// REQ: OA-SEAL-014
#[test]
fn a_document_with_a_message_in_the_window_is_dropped_whole_and_every_counter_is_as_for_an_empty_run(
) {
    let inside = |x: String| stamped(&x, "2026-11-21T00:00:00Z");
    let outside = |x: String| x;
    // One in-window message among others: the document is dropped and n is its messages.
    let mixed = format!(
        "{}{}{}",
        outside(track()),
        inside(track()),
        outside(track())
    );
    assert_dropped_whole(&wrap_flight(&mixed), 16, 8, 3);
    // A full table: three flights, room for one, the in-window one last.
    assert_dropped_whole(&wrap_flight(&mixed), 1, 1, 3);
    // A flow message, and a TMI flight list (its entries have no time of their own).
    assert_dropped_whole(&wrap_fi(&inside(gadv())), 16, 8, 1);
    assert_dropped_whole(&wrap_fi(&inside(tmi_list())), 16, 8, 1);
    assert_dropped_whole(
        &wrap_fi(&format!("{}{}", gadv(), inside(tmi_list()))),
        1,
        1,
        2,
    );
    // An invalid attribute before the time, and an invalid field after it, in the window.
    let early = track().replace(r#"cdmPart="false""#, r#"cdmPart="maybe""#);
    assert_dropped_whole(&wrap_flight(&inside(early)), 16, 8, 1);
    let late = track().replace(
        "<nxcm:speed>276</nxcm:speed>",
        "<nxcm:speed>zzz</nxcm:speed>",
    );
    assert_dropped_whole(&wrap_flight(&inside(late)), 16, 8, 1);
    // Without a message in the window (the end is excluded) the document decodes as usual.
    let w = oa_sealed::DropWindow::new(SEAL.0, SEAL.1);
    let mut rows = new_rows(16, 8, 1 << 16, w);
    let mut p = Parser::new(default_limits()).unwrap();
    let xml = stamped(&wrap_flight(&track()), "2026-11-23T00:17:00Z");
    let o = decode_bytes(&mut p, xml.as_bytes(), &mut rows).unwrap();
    assert_eq!((o.sealed_dropped, flights_len(&rows.flights)), (0, 1));
}

/// Decodes `xml` with `w` into roomy tables, returning the result and the stats.
fn decode_with_window(
    xml: &str,
    w: oa_sealed::DropWindow,
) -> (Result<Outcome, DocError>, usize, oa_tfms::Stats) {
    let mut rows = new_rows(16, 8, 1 << 14, w);
    let mut p = Parser::new(default_limits()).unwrap();
    let doc = p.parse(xml.as_bytes()).unwrap();
    let r = oa_tfms::decode_doc(&doc, &mut rows);
    let mut s = new_stats();
    observe(&mut s, &r, &[], &[]);
    (r, flights_len(&rows.flights) + flows_len(&rows.flows), s)
}

// REQ: OA-SEAL-014
#[test]
fn a_window_that_holds_all_time_drops_every_document_and_the_production_list_drops_the_window() {
    let doc = wrap_flight(&track());
    // All time (the closed failure of the list): the document is dropped whole.
    let (r, rows, s) = decode_with_window(&doc, oa_sealed::DropWindow::all_time());
    assert_eq!((r, rows), (Ok(sealed(1)), 0));
    assert_eq!(
        s,
        oa_tfms::Stats {
            sealed_window_dropped: 1,
            ..new_stats()
        }
    );
    // The production list: a message in the window is dropped, one outside it is kept.
    let w = oa_sealed::DropWindow::sealed();
    let (r, rows, _) = decode_with_window(&stamped(&doc, "2026-11-21T00:00:00Z"), w);
    assert_eq!((r, rows), (Ok(sealed(1)), 0));
    let (r, rows, s) = decode_with_window(&doc, w);
    assert_eq!(
        (r.unwrap().sealed_dropped, rows, s.sealed_window_dropped),
        (0, 1, 0)
    );
}

// REQ: OA-SEAL-014
#[test]
fn every_observation_time_is_a_window_key_in_any_encoding_and_a_reference_time_is_not() {
    let w = oa_sealed::DropWindow::new(SEAL.0, SEAL.1);
    let inw = "2026-11-21T00:00:00Z";
    // The position time (`timeAtPosition`) in the window, the message time outside it.
    let pos = wrap_flight(&track().replace(
        "2030-01-02T03:04:00Z</nxcm:timeAtPosition>",
        &format!("{inw}</nxcm:timeAtPosition>"),
    ));
    let (r, rows, _) = decode_with_window(&pos, w);
    assert_eq!((r, rows), (Ok(sealed(1)), 0));
    // The oceanic position's `time`.
    let oc = pos.replace("timeAtPosition", "time");
    assert!(decode_with_window(&oc, w)
        .0
        .map_or(true, |o| o.sealed_dropped == 1 || o.flight_rows == 0));
    // Entity-encoded (the final Z as a character reference), in the message time and the
    // position time, and split by a comment.
    let enc = wrap_flight(&track()).replace(
        r#"sourceTimeStamp="2030-01-02T03:04:05Z""#,
        r#"sourceTimeStamp="2026-11-21T00:00:00&#90;""#,
    );
    assert_eq!(decode_with_window(&enc, w).0, Ok(sealed(1)));
    let split = pos.replace(inw, "2026-11-21T00:00:<!--x-->00Z");
    assert_eq!(decode_with_window(&split, w).0, Ok(sealed(1)));
    // Padded with whitespace: if the decoder would read the value as a time in the window the
    // document is dropped; if it refuses the value there is no row holding it.
    let pad = wrap_flight(&track()).replace(
        r#"sourceTimeStamp="2030-01-02T03:04:05Z""#,
        r#"sourceTimeStamp=" 2026-11-21T00:00:00Z ""#,
    );
    let (r, rows, _) = decode_with_window(&pad, w);
    assert!(
        matches!(r, Ok(o) if o.sealed_dropped == 1) || rows == 0,
        "{r:?} {rows}"
    );
    // A repeated position time element: both are read, so the one in the window is found.
    let dup = wrap_flight(&track().replace(
        "<nxcm:timeAtPosition>2030-01-02T03:04:00Z</nxcm:timeAtPosition>",
        &format!("<nxcm:timeAtPosition>2030-01-02T03:04:00Z</nxcm:timeAtPosition><nxcm:timeAtPosition>{inw}</nxcm:timeAtPosition>"),
    ));
    assert_eq!(decode_with_window(&dup, w).0, Ok(sealed(1)));
    // Reference times (here an ETA and the arrival fix time) in the window are not keys.
    let refs = wrap_flight(
        &track()
            .replace("2030-01-02T04:30:00Z", inw)
            .replace("2030-01-02T04:00:00Z", inw),
    );
    let (r, rows, _) = decode_with_window(&refs, w);
    assert_eq!((r.unwrap().sealed_dropped, rows), (0, 1));
    // A status output counts as a message of a dropped document.
    let status = wrap_flight(&format!("{}<tfmsStatusOutput/>", stamped(&track(), inw)));
    assert_eq!(decode_with_window(&status, w).0, Ok(sealed(2)));
}

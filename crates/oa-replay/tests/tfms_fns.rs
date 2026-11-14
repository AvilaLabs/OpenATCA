//! Tests of the TFMS and FNS feeds: the flight, programme and NOTAM tables, their bounds and
//! expiry, malformed payloads, the state hash and a mixed replay built from hand-made fixtures.
//! Every identifier, time and position is made up; only the element structure follows the surveys
//! (`docs/TFMS_SURVEY.md`, `docs/FNS_SURVEY.md`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_feed::Kind;
use oa_notam::{new_decoder, new_notams, new_stats, Limits};
use oa_replay::tfms::{tcol, TFMS_FLIGHT_MAX_AGE_NS};
use oa_replay::{
    apply_fns, apply_tfms_flight, errors_total, flow_live, flow_sweep, fns_in_force, fns_live,
    fns_slot_of, fns_sweep, new_flow_table, new_fns_table, new_pipeline, new_table, new_tfms_table,
    process_frame, replay_hash, run_streams, snapshot, spec_from_bytes, state_hash, tfms_linked,
    tfms_live, tfms_slot_of, tfms_sweep, ErrKind, FeedKind, FnsApplied, Pipeline, PipelineConfig,
    TableConfig, TfmsApplied, ERR_NAMES, LINK_ACID, LINK_PLAN,
};
use oa_tfms::{decode_bytes as tfms_decode, default_limits, new_rows as new_tfms_rows, NO_TIME};
use oa_xml::Parser;

const SVC: &str = "urn:us:gov:dot:faa:atm:tfm:tfmdataservice";
const FD: &str = "urn:us:gov:dot:faa:atm:tfm:flightdata";
const FI: &str = "urn:us:gov:dot:faa:atm:tfm:flowinformation";
const FCM: &str = "urn:us:gov:dot:faa:atm:tfm:flightdatacommonmessages";
const CORE: &str = "urn:us:gov:dot:faa:atm:tfm:tfmdatacoreelements";
const FIC: &str = "urn:us:gov:dot:faa:atm:tfm:ficommonmessages";
const FIC2: &str = "urn:us:gov:dot:faa:atm:tfm:ficommonmessages2";
const FIDT: &str = "urn:us:gov:dot:faa:atm:tfm:ficommondatatypes";

/// 2030-01-02T00:00:00Z, Unix ms.
const D30: i64 = 1_893_542_400_000;
const MS_NS: i64 = 1_000_000;
/// Replay time (ns) of `h:m:s` on 2030-01-02.
fn at(h: i64, m: i64, s: i64) -> i64 {
    (D30 + ((h * 60 + m) * 60 + s) * 1000) * MS_NS
}
/// ISO text of `h:m:s` on 2030-01-02.
fn ts(h: i64, m: i64, s: i64) -> String {
    format!("2030-01-02T{h:02}:{m:02}:{s:02}Z")
}

// ---- TFMS fixtures ----------------------------------------------------------------------------

fn wrap_flight(msgs: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:tfmDataService xmlns:ds="{SVC}" xmlns="{SVC}" xmlns:fdm="{FD}" xmlns:nxce="{CORE}" xmlns:nxcm="{FCM}"><fltdOutput>{msgs}</fltdOutput></ds:tfmDataService>"#
    )
}

fn wrap_fi(msgs: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:tfmDataService xmlns:ns5="{SVC}" xmlns:ns12="{FI}" xmlns:ns11="{FIC}" xmlns:ns9="{FIC2}" xmlns:ns7="{CORE}" xmlns:ns4="{FIDT}"><ns5:fiOutput>{msgs}</ns5:fiOutput></ns5:tfmDataService>"#
    )
}

/// One departure message for flight reference `fref`, aircraft `acid`, from KTAA to KTBB, with an
/// optional 10-character `gufi`, sent at `src` and giving an estimated arrival `eta` and an actual
/// departure `etd`.
fn departure(
    fref: u32,
    acid: &str,
    gufi: Option<&str>,
    src: (i64, i64, i64),
    eta: (i64, i64, i64),
    etd: (i64, i64, i64),
) -> String {
    let g = gufi.map_or(String::new(), |g| format!("<nxce:gufi>{g}</nxce:gufi>"));
    let q = format!(
        r#"<nxcm:qualifiedAircraftId aircraftCategory="JET" userCategory="COMMERCIAL"><nxce:aircraftId>{acid}</nxce:aircraftId><nxce:computerId><nxce:facilityIdentifier>KZXX</nxce:facilityIdentifier><nxce:idNumber>123</nxce:idNumber></nxce:computerId>{g}<nxce:departurePoint><nxce:airport>KTAA</nxce:airport></nxce:departurePoint><nxce:arrivalPoint><nxce:airport>KTBB</nxce:airport></nxce:arrivalPoint></nxcm:qualifiedAircraftId>"#
    );
    format!(
        r#"<fdm:fltdMessage acid="{acid}" airline="TST" arrArpt="KTBB" cdmPart="false" depArpt="KTAA" fdTrigger="HCS_TRACK_MSG" flightRef="{fref}" major="TST" sensitivity="A" sourceFacility="KZXX" sourceTimeStamp="{}" msgType="departureInformation"><fdm:departureInformation><nxcm:flightAircraftSpecs equipmentQualifier="L">C172</nxcm:flightAircraftSpecs><nxcm:ncsmFlightTimeData><nxcm:eta etaType="ESTIMATED" timeValue="{}"/><nxcm:etd etdType="ACTUAL" timeValue="{}"/></nxcm:ncsmFlightTimeData>{q}<nxcm:timeOfArrival estimated="true">{}</nxcm:timeOfArrival><nxcm:timeOfDeparture estimated="false">{}</nxcm:timeOfDeparture></fdm:departureInformation></fdm:fltdMessage>"#,
        ts(src.0, src.1, src.2),
        ts(eta.0, eta.1, eta.2),
        ts(etd.0, etd.1, etd.2),
        ts(eta.0, eta.1, eta.2 + 30),
        ts(etd.0, etd.1, etd.2 + 30)
    )
}

fn fi(msg_type: &str, src: (i64, i64, i64), body: &str) -> String {
    format!(
        r#"<ns12:fiMessage sensitivity="A" visDomain="SHARED" destinationCodes="ALL" sourceFacility="DCCX" sourceTimeStamp="{}" msgType="{msg_type}">{body}</ns12:fiMessage>"#,
        ts(src.0, src.1, src.2)
    )
}

/// A restriction with id `id`, in force from 04:00 to `stop_h`:00.
fn rstr(src: (i64, i64, i64), id: u32, mit: u32, stop_h: i64) -> String {
    fi(
        "RSTR",
        src,
        &format!(
            r#"<ns12:restrictionMessage><ns11:action>4</ns11:action><ns11:airports>KTBB</ns11:airports><ns11:eventTime>2030-01-02T03:02:00Z</ns11:eventTime><ns11:facility>DCCX</ns11:facility><ns11:mitNumber>278529</ns11:mitNumber><ns11:mitValue>{mit}</ns11:mitValue><ns11:qualifier>PER FACILITY</ns11:qualifier><ns11:reasonText>VOL:Volume</ns11:reasonText><ns11:restrictedNasElements>FIXQ</ns11:restrictedNasElements><ns11:restrictionCategory>MIT</ns11:restrictionCategory><ns11:restrictionId>{id}</ns11:restrictionId><ns11:restrictionType>ARRIVAL</ns11:restrictionType><ns11:startTime>2030-01-02T04:00:00Z</ns11:startTime><ns11:stopTime>2030-01-02T{stop_h:02}:00:00Z</ns11:stopTime></ns12:restrictionMessage>"#
        ),
    )
}

fn aptc(src: (i64, i64, i64), airport: &str, rate: u32) -> String {
    fi(
        "APTC",
        src,
        &format!(
            r#"<ns12:airportConfigMessage><ns11:aarAdjust/><ns11:airport>{airport}</ns11:airport><ns11:arrRate>{rate}</ns11:arrRate><ns11:arrRunwayConf>09/27</ns11:arrRunwayConf><ns11:depRate>0</ns11:depRate><ns11:depRunwayConf/><ns11:eventTime>2030-01-02T03:00:00Z</ns11:eventTime><ns11:facility>TST</ns11:facility><ns11:stratAar>58</ns11:stratAar><ns11:weather>VMC</ns11:weather></ns12:airportConfigMessage>"#
        ),
    )
}

fn tmi_list(src: (i64, i64, i64), fref: u32) -> String {
    let fxa = r#"<ns9:fxaFlight><ns9:bentryTm>2030-01-02T03:00:00Z</ns9:bentryTm><ns9:fxaId><ns11:fcaId>fca.tst.lxstn01.20300101000000</ns11:fcaId></ns9:fxaId></ns9:fxaFlight>"#;
    fi(
        "TMI_FLIGHT_LIST",
        src,
        &format!(
            r#"<ns12:tmiFlightDataList><ns12:flightData><ns9:flight><ns7:aircraftId>TST9</ns7:aircraftId><ns7:arrivalPoint><ns7:airport>KTBB</ns7:airport></ns7:arrivalPoint><ns7:departurePoint><ns7:airport>KTAA</ns7:airport></ns7:departurePoint><ns7:gufi>ZZ000002bb</ns7:gufi><ns7:igtd>2030-01-02T01:30:00Z</ns7:igtd></ns9:flight><ns9:flightReference>{fref}</ns9:flightReference><ns9:status>PLANNED</ns9:status><ns9:tmiFlightInfoList><ns9:fxaFlightData>{fxa}{fxa}</ns9:fxaFlightData><ns9:tmi updateType="NEW" lastUpdateTime="2030-01-01T12:00:00Z"><ns9:fcaId>fca.tst.lxstn01.20300101000000</ns9:fcaId></ns9:tmi></ns9:tmiFlightInfoList></ns12:flightData></ns12:tmiFlightDataList>"#
        ),
    )
}

// ---- FNS fixtures -----------------------------------------------------------------------------

const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><message:AIXMBasicMessage xmlns:message="http://www.aixm.aero/schema/5.1/message" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:aixm="http://www.aixm.aero/schema/5.1" xmlns:event="http://www.aixm.aero/schema/5.1/event" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:fns="urn:us.gov.dot.faa.aim.fns" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:html="http://www.w3.org/1999/xhtml" xmlns:fnse="http://www.aixm.aero/schema/5.1/extensions/FAA/FNSE" gml:id="M1">"#;

/// What varies between the test NOTAMs.
#[derive(Clone)]
struct Nt {
    series: &'static str,
    number: u32,
    ntype: &'static str,
    /// Referred (series, number); year is always 2030.
    refd: Option<(&'static str, u32)>,
    issued: &'static str,
    /// Start and end as `YYMMDDhhmm` text with the century, e.g. `203003051000`.
    start: &'static str,
    end: Option<&'static str>,
    /// Extra `fnse` elements and extra members (a polygon feature).
    ext: &'static str,
    members: String,
}

fn nt(series: &'static str, number: u32) -> Nt {
    Nt {
        series,
        number,
        ntype: "N",
        refd: None,
        issued: "2030-03-05T09:00:00.000Z",
        start: "203003051000",
        end: Some("203003061830"),
        ext: "",
        members: String::new(),
    }
}

fn notam(n: &Nt) -> String {
    let refd = n.refd.map_or(String::new(), |(s, k)| {
        format!("<event:referredSeries>{s}</event:referredSeries><event:referredNumber>{k}</event:referredNumber><event:referredYear>2030</event:referredYear>")
    });
    let end = n.end.map_or(String::new(), |e| {
        format!("<event:effectiveEnd>{e}</event:effectiveEnd>")
    });
    let period_end = if n.end.is_some() {
        "<gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition>"
    } else {
        ""
    };
    let uuid = format!(
        "0a1b2c3d-0000-4000-8000-{:012x}",
        u64::from(n.number) + 4096 * n.series.len() as u64
    );
    format!(
        r#"{HEAD}<message:hasMember><event:Event gml:id="E1"><gml:identifier codeSpace="urn:uuid:">{uuid}</gml:identifier><gml:boundedBy xsi:nil="true"/><event:timeSlice><event:EventTimeSlice gml:id="ET1"><gml:validTime><gml:TimePeriod gml:id="EP1"><gml:beginPosition>2030-03-05T10:00:00.000Z</gml:beginPosition>{period_end}</gml:TimePeriod></gml:validTime><aixm:interpretation>BASELINE</aixm:interpretation><aixm:sequenceNumber>1</aixm:sequenceNumber><aixm:correctionNumber>0</aixm:correctionNumber><event:scenario>123</event:scenario><event:textNOTAM><event:NOTAM gml:id="N1"><event:series>{}</event:series><event:number>{}</event:number><event:year>2030</event:year><event:type>{}</event:type>{refd}<event:issued>{}</event:issued><event:affectedFIR>ZQQ</event:affectedFIR><event:selectionCode>QXXXX</event:selectionCode><event:minimumFL>000</event:minimumFL><event:maximumFL>999</event:maximumFL><event:coordinates>3012N09045W</event:coordinates><event:radius>5</event:radius><event:location>QQA</event:location><event:effectiveStart>{}</event:effectiveStart>{end}<event:text>RWY 09/27 CLSD</event:text></event:NOTAM></event:textNOTAM><event:extension><fnse:EventExtension gml:id="X1"><fnse:classification>DOM</fnse:classification><fnse:accountId>QQA</fnse:accountId>{}<fnse:icaoLocation>KQQA</fnse:icaoLocation></fnse:EventExtension></event:extension></event:EventTimeSlice></event:timeSlice></event:Event></message:hasMember>{}</message:AIXMBasicMessage>"#,
        n.series, n.number, n.ntype, n.issued, n.start, n.ext, n.members
    )
}

/// A runway-element polygon member, vertices as a `posList`.
fn polygon(ring: &str) -> String {
    format!(
        r##"<message:hasMember><aixm:RunwayElement gml:id="F"><gml:identifier codeSpace="urn:uuid:">0a1b2c3d-0000-4000-8000-00000000f001</gml:identifier><aixm:timeSlice><aixm:RunwayElementTimeSlice gml:id="S"><gml:validTime><gml:TimeInstant gml:id="I"><gml:timePosition>2030-03-05T09:00:00.000Z</gml:timePosition></gml:TimeInstant></gml:validTime><aixm:interpretation>SNAPSHOT</aixm:interpretation><aixm:associatedRunway xlink:href="#R"/><aixm:extent><aixm:ElevatedSurface gml:id="S" srsDimension="2" srsName="urn:ogc:def:crs:EPSG::4326"><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>{ring}</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></aixm:ElevatedSurface></aixm:extent></aixm:RunwayElementTimeSlice></aixm:timeSlice></aixm:RunwayElement></message:hasMember>"##
    )
}

/// A NOTAM identity: series, number, year 2030.
fn key(series: &str, number: u32) -> ([u8; 12], u32, u16) {
    let mut s = [0u8; 12];
    s[..series.len()].copy_from_slice(series.as_bytes());
    (s, number, 2030)
}

/// 2030-03-05T00:00:00Z in replay ns, plus `h` hours.
fn mar5(h: i64) -> i64 {
    (1_898_899_200_000 + h * 3_600_000) * MS_NS
}

// ---- pipeline helpers -------------------------------------------------------------------------

fn pl() -> Pipeline {
    new_pipeline(PipelineConfig::default()).unwrap()
}

fn feed(p: &mut Pipeline, f: FeedKind, t: i64, xml: &str) {
    process_frame(p, f, Kind::Message, t, xml.as_bytes());
}

fn err(p: &Pipeline, k: ErrKind) -> u64 {
    p.errors[k as usize]
}

/// An SFDPS flight with GUFI number `n`, flight-plan identifier `plan`, KTAA to KTBB.
fn sfdps_flight(p: &mut Pipeline, n: u32, call: &str, plan: &str, t: i64) {
    let m = sf(n, 1000).call(call).plan(plan).dep("KTAA").arr("KTBB");
    feed(p, FeedKind::Sfdps, t, &sf_doc(&[m]));
}

// ---- TFMS: flights ----------------------------------------------------------------------------

// REQ: OA-RPL-041
#[test]
fn a_tfms_flight_row_keeps_the_latest_estimates_per_flight() {
    let mut p = pl();
    let a = wrap_flight(&departure(
        7001,
        "TST1",
        Some("KA000001AB"),
        (3, 0, 0),
        (5, 0, 0),
        (2, 0, 0),
    ));
    feed(&mut p, FeedKind::Tfms, at(3, 0, 1), &a);
    let s = tfms_slot_of(&p.tfms, 7001).unwrap();
    assert_eq!(p.tfms.times[s][tcol::ETA], D30 + 5 * 3_600_000);
    assert_eq!(p.tfms.times[s][tcol::ETD], D30 + 2 * 3_600_000);
    assert_eq!(p.tfms.times[s][tcol::TOA], D30 + 5 * 3_600_000 + 30_000);
    assert_eq!((p.tfms.eta_type[s], p.tfms.etd_type[s]), (1, 2));
    assert_eq!(&p.tfms.acid[s][..5], b"TST1\0");
    assert_eq!(&p.tfms.gufi[s][..11], b"KA000001AB\0");
    // A later message moves the estimate; a time the row lacks is left alone.
    let b = wrap_flight(&departure(
        7001,
        "TST1",
        Some("KA000001AB"),
        (3, 10, 0),
        (5, 20, 0),
        (2, 0, 0),
    ));
    feed(&mut p, FeedKind::Tfms, at(3, 10, 1), &b);
    assert_eq!(tfms_live(&p.tfms), 1);
    assert_eq!(p.tfms.times[s][tcol::ETA], D30 + (5 * 60 + 20) * 60_000);
    assert_eq!(p.tfms.times[s][tcol::ORIG_DEP], NO_TIME);
    assert_eq!(p.tfms.rows[s], 2);
    assert_eq!(p.rows_applied[FeedKind::Tfms as usize], 2);
    assert_eq!(p.rows_seen[FeedKind::Tfms as usize], 2);
    assert_eq!(p.not_applied, [0; 5]);
    assert_eq!(errors_total(&p.errors), 0);
    // A different flight is another slot.
    let c = wrap_flight(&departure(
        7002,
        "TST2",
        None,
        (3, 11, 0),
        (6, 0, 0),
        (2, 5, 0),
    ));
    feed(&mut p, FeedKind::Tfms, at(3, 11, 1), &c);
    assert_eq!(tfms_live(&p.tfms), 2);
}

// REQ: OA-RPL-042
#[test]
fn a_tfms_row_older_than_the_flights_latest_is_refused_whole_and_counted() {
    let mut p = pl();
    let late = wrap_flight(&departure(
        7001,
        "TST1",
        None,
        (3, 30, 0),
        (5, 30, 0),
        (2, 0, 0),
    ));
    let early = wrap_flight(&departure(
        7001,
        "TST1",
        None,
        (3, 0, 0),
        (9, 0, 0),
        (2, 0, 0),
    ));
    feed(&mut p, FeedKind::Tfms, at(3, 30, 1), &late);
    feed(&mut p, FeedKind::Tfms, at(3, 30, 2), &early);
    let s = tfms_slot_of(&p.tfms, 7001).unwrap();
    assert_eq!(p.tfms.times[s][tcol::ETA], D30 + (5 * 60 + 30) * 60_000);
    assert_eq!(p.tfms.stats.out_of_order, 1);
    assert_eq!(p.tfms.out_of_order[s], 1);
    assert_eq!(p.rows_out_of_order[FeedKind::Tfms as usize], 1);
    assert_eq!(p.rows_applied[FeedKind::Tfms as usize], 1);
    // The same source time applies.
    feed(&mut p, FeedKind::Tfms, at(3, 30, 3), &late);
    assert_eq!(p.tfms.stats.out_of_order, 1);
    assert_eq!(p.rows_applied[FeedKind::Tfms as usize], 2);
}

// REQ: OA-RPL-043
#[test]
fn a_tfms_flight_links_to_sfdps_by_plan_identifier_or_by_acid_and_airports() {
    let mut p = pl();
    sfdps_flight(&mut p, 1, "TST1", "KA000001AB", at(3, 0, 0));
    sfdps_flight(&mut p, 2, "TST2", "KA000002AB", at(3, 0, 1));
    // Two SFDPS flights share acid, departure and arrival: that key is ambiguous.
    sfdps_flight(&mut p, 3, "TST3", "KA000003AB", at(3, 0, 2));
    sfdps_flight(&mut p, 4, "TST3", "KA000004AB", at(3, 0, 3));
    let t = |fref, acid: &str, gufi: Option<&str>, s| {
        wrap_flight(&departure(
            fref,
            acid,
            gufi,
            (3, 1, s),
            (5, 0, 0),
            (2, 0, 0),
        ))
    };
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 1, 1),
        &t(7001, "TST1", Some("KA000001AB"), 1),
    );
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 1, 2),
        &t(7002, "TST2", None, 2),
    );
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 1, 3),
        &t(7003, "TST3", None, 3),
    );
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 1, 4),
        &t(7004, "TST9", None, 4),
    );
    // A gufi that matches no plan identifier does not fall back to the acid key.
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 1, 5),
        &t(7005, "TST2", Some("KZ999999ZZ"), 5),
    );
    let s1 = tfms_slot_of(&p.tfms, 7001).unwrap();
    let s2 = tfms_slot_of(&p.tfms, 7002).unwrap();
    assert_eq!(
        (p.tfms.link_kind[s1], p.tfms.link[s1]),
        (LINK_PLAN, gufi(1))
    );
    assert_eq!(
        (p.tfms.link_kind[s2], p.tfms.link[s2]),
        (LINK_ACID, gufi(2))
    );
    for r in [7003, 7004, 7005] {
        let s = tfms_slot_of(&p.tfms, r).unwrap();
        assert_eq!(p.tfms.link_kind[s], 0, "{r}");
    }
    assert_eq!(tfms_linked(&p.tfms), 2);
    let st = p.tfms.stats;
    assert_eq!((st.linked_plan, st.linked_acid, st.ambiguous), (1, 1, 1));
    // The link is retried: the plan identifier arrives from SFDPS after the TFMS row.
    sfdps_flight(&mut p, 5, "TST5", "KA000005AB", at(3, 2, 0));
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 2, 1),
        &t(7006, "TST5", Some("KA000005AB"), 6),
    );
    let s6 = tfms_slot_of(&p.tfms, 7006).unwrap();
    assert_eq!(p.tfms.link[s6], gufi(5));
}

// REQ: OA-RPL-044
#[test]
fn tfms_programmes_are_kept_one_per_key_with_the_latest_row_and_expire_at_their_end() {
    let mut p = pl();
    let doc = |m: &str| wrap_fi(m);
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 0, 0),
        &doc(&rstr((3, 0, 0), 20300101, 15, 6)),
    );
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 1, 0),
        &doc(&aptc((3, 1, 0), "KTBB", 60)),
    );
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 1, 1),
        &doc(&aptc((3, 1, 1), "KTAA", 40)),
    );
    assert_eq!(flow_live(&p.flows), 3);
    // The latest row of a programme replaces the held one.
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 5, 0),
        &doc(&rstr((3, 5, 0), 20300101, 20, 6)),
    );
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 6, 0),
        &doc(&aptc((3, 6, 0), "KTBB", 45)),
    );
    assert_eq!(flow_live(&p.flows), 3);
    assert_eq!(p.flows.stats.created, 3);
    assert_eq!(p.flows.stats.applied, 5);
    let s = (0..p.flows.slots.capacity)
        .find(|&i| p.flows.slots.occupied[i] && p.flows.key_kind[i] == 1)
        .unwrap();
    assert_eq!(p.flows.rates[s][0], 20);
    assert_eq!(p.flows.end_ms[s], D30 + 6 * 3_600_000);
    assert_eq!(p.flows.start_ms[s], D30 + 4 * 3_600_000);
    // An older row is refused whole.
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 7, 0),
        &doc(&rstr((3, 0, 0), 20300101, 99, 6)),
    );
    assert_eq!(p.flows.stats.out_of_order, 1);
    assert_eq!(p.flows.rates[s][0], 20);
    // Counted by program type: restriction MIT, airport configuration.
    use oa_tfms::rows::prog;
    assert_eq!(p.flows.stats.by_program[prog::MIT as usize], 2);
    assert_eq!(p.flows.stats.by_program[prog::AIRPORT_CONFIG as usize], 3);
    // The restriction ends at 06:00; airport configurations have no end and stay for a day.
    feed(
        &mut p,
        FeedKind::Tfms,
        at(5, 59, 30),
        &doc(&aptc((5, 59, 0), "KTBB", 50)),
    );
    assert_eq!(flow_live(&p.flows), 3);
    feed(
        &mut p,
        FeedKind::Tfms,
        at(6, 0, 30),
        &doc(&aptc((6, 0, 0), "KTBB", 50)),
    );
    assert_eq!(flow_live(&p.flows), 2);
    assert_eq!(p.flows.stats.evicted, 1);
    let mut t = new_flow_table(4);
    assert_eq!(flow_sweep(&mut t, at(9, 0, 0)), 0);
}

// REQ: OA-RPL-045
#[test]
fn tmi_flight_list_entries_set_the_flights_programme_fields() {
    let mut p = pl();
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 0, 5),
        &wrap_fi(&tmi_list((3, 0, 0), 7100)),
    );
    let s = tfms_slot_of(&p.tfms, 7100).unwrap();
    assert_eq!(p.tfms.tmi_update[s], 2);
    assert_eq!(p.tfms.n_fxa[s], 2);
    assert_eq!(&p.tfms.fca[s][..16], b"fca.tst.lxstn01.");
    assert_eq!(p.tfms.status[s], 1);
    assert_eq!(p.tfms.times[s][tcol::TMI], D30 - 12 * 3_600_000);
    assert_eq!(p.rows_applied[FeedKind::Tfms as usize], 1);
}

// REQ: OA-RPL-046
#[test]
fn the_tfms_tables_are_bounded_refuse_when_full_and_expire_by_replay_time() {
    let mut t = new_tfms_table(3);
    let flights = new_table(TableConfig::default()).unwrap();
    let mut parser = Parser::new(default_limits()).unwrap();
    let mut rows = new_tfms_rows(16, 4, 1 << 16);
    let mut r = Vec::new();
    for n in 1..=5u32 {
        let x = wrap_flight(&departure(
            8000 + n,
            "TST1",
            None,
            (3, 0, n as i64),
            (5, 0, 0),
            (2, 0, 0),
        ));
        oa_tfms::clear_rows(&mut rows);
        tfms_decode(&mut parser, x.as_bytes(), &mut rows).unwrap();
        r.push(apply_tfms_flight(
            &mut t,
            &flights,
            &rows,
            0,
            at(3, 0, n as i64),
        ));
    }
    assert_eq!(&r[..3], [TfmsApplied::New; 3]);
    assert_eq!(&r[3..], [TfmsApplied::TableFull; 2]);
    assert_eq!((t.stats.created, t.stats.refused_full), (3, 2));
    assert_eq!(tfms_live(&t), 3);
    // A held flight still updates when the table is full.
    assert_eq!(
        apply_tfms_flight(&mut t, &flights, &rows, 0, at(3, 1, 0)),
        TfmsApplied::TableFull
    );
    oa_tfms::clear_rows(&mut rows);
    let x = wrap_flight(&departure(
        8001,
        "TST1",
        None,
        (3, 5, 0),
        (7, 0, 0),
        (2, 0, 0),
    ));
    tfms_decode(&mut parser, x.as_bytes(), &mut rows).unwrap();
    assert_eq!(
        apply_tfms_flight(&mut t, &flights, &rows, 0, at(3, 5, 0)),
        TfmsApplied::Updated
    );
    // Six hours of silence evicts; the updated flight is newer and stays.
    let now = at(3, 0, 3) + TFMS_FLIGHT_MAX_AGE_NS + 1;
    assert_eq!(tfms_sweep(&mut t, now), 2);
    assert_eq!(tfms_live(&t), 1);
    assert!(tfms_slot_of(&t, 8001).is_some());
    assert_eq!(t.stats.evicted, 2);
    // Evicted slots are reused.
    oa_tfms::clear_rows(&mut rows);
    let x = wrap_flight(&departure(
        8009,
        "TST1",
        None,
        (9, 5, 0),
        (7, 0, 0),
        (2, 0, 0),
    ));
    tfms_decode(&mut parser, x.as_bytes(), &mut rows).unwrap();
    assert_eq!(
        apply_tfms_flight(&mut t, &flights, &rows, 0, now),
        TfmsApplied::New
    );
}

// REQ: OA-RPL-047
#[test]
fn malformed_tfms_payloads_are_counted_by_kind_and_never_applied() {
    let mut p = pl();
    let good = wrap_flight(&departure(
        7001,
        "TST1",
        None,
        (3, 0, 0),
        (5, 0, 0),
        (2, 0, 0),
    ));
    feed(&mut p, FeedKind::Tfms, at(3, 0, 1), "<not-xml");
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 0, 2),
        "<other xmlns=\"urn:x\"/>",
    );
    // A bad timestamp in one field refuses the whole message.
    feed(
        &mut p,
        FeedKind::Tfms,
        at(3, 0, 3),
        &good.replace("2030-01-02T05:00:00Z", "yesterday"),
    );
    // A document with more flights than the row table holds.
    let many: String = (0..300)
        .map(|n| departure(9000 + n, "TST1", None, (3, 0, 0), (5, 0, 0), (2, 0, 0)))
        .collect();
    feed(&mut p, FeedKind::Tfms, at(3, 0, 4), &wrap_flight(&many));
    assert_eq!(err(&p, ErrKind::TfmsXml), 1);
    assert_eq!(err(&p, ErrKind::TfmsNotTfms), 1);
    assert_eq!(err(&p, ErrKind::TfmsField), 1);
    assert_eq!(err(&p, ErrKind::TfmsCapacity), 1);
    assert_eq!(errors_total(&p.errors), 4);
    assert_eq!(tfms_live(&p.tfms), 0);
    assert_eq!(p.rows_applied[FeedKind::Tfms as usize], 0);
    assert_eq!(p.messages[FeedKind::Tfms as usize], 4);
    // The pipeline goes on: a good document after the bad ones applies.
    feed(&mut p, FeedKind::Tfms, at(3, 0, 5), &good);
    assert_eq!(tfms_live(&p.tfms), 1);
    assert_eq!(ERR_NAMES[ErrKind::TfmsCapacity as usize], "tfms-capacity");
    assert_eq!(ERR_NAMES[ErrKind::FnsNoKey as usize], "fns-no-key");
}

// REQ: OA-RPL-048
#[test]
fn a_tfms_flight_row_without_a_flight_reference_is_not_applied() {
    // The decoder refuses a message with no `flightRef`, so a row without one cannot come from a
    // payload; the apply step still refuses it and counts it, as the pipeline does.
    let mut t = new_tfms_table(4);
    let flights = new_table(TableConfig::default()).unwrap();
    let mut rows = new_tfms_rows(4, 4, 1 << 10);
    oa_tfms::push_flight(&mut rows.flights, &oa_tfms::BLANK_FLIGHT);
    assert_eq!(
        apply_tfms_flight(&mut t, &flights, &rows, 0, at(3, 0, 0)),
        TfmsApplied::NoRef
    );
    assert_eq!(
        apply_tfms_flight(&mut t, &flights, &rows, 5, at(3, 0, 0)),
        TfmsApplied::BadRow
    );
    assert_eq!((tfms_live(&t), t.stats.no_ref), (0, 1));
    // And a payload with no flight reference is a refused document.
    let mut p = pl();
    let x = wrap_flight(&departure(
        7001,
        "TST1",
        None,
        (3, 0, 0),
        (5, 0, 0),
        (2, 0, 0),
    ))
    .replace(r#"flightRef="7001" "#, "");
    feed(&mut p, FeedKind::Tfms, at(3, 0, 1), &x);
    assert_eq!(tfms_live(&p.tfms), 0);
    assert_eq!(err(&p, ErrKind::TfmsField), 1);
}

// ---- FNS --------------------------------------------------------------------------------------

// REQ: OA-RPL-049
#[test]
fn new_replacement_and_cancellation_notams_keep_the_notams_in_force() {
    let mut p = pl();
    let t = |n: &Nt| notam(n);
    feed(&mut p, FeedKind::Fns, mar5(9), &t(&nt("Z", 1)));
    feed(&mut p, FeedKind::Fns, mar5(9), &t(&nt("Z", 2)));
    feed(&mut p, FeedKind::Fns, mar5(9), &t(&nt("Y", 3)));
    assert_eq!(fns_live(&p.fns), 3);
    let s = fns_slot_of(&p.fns, &key("Z", 1)).unwrap();
    assert_eq!(p.fns.ntype[s], 1);
    assert_eq!(&p.fns.location[s], b"QQA\0");
    assert_eq!(&p.fns.icao[s], b"KQQA");
    assert_eq!(&p.fns.selection[s], b"QXXXX");
    assert_eq!(p.fns.codes[s][0], 2);
    assert_eq!(p.fns.times[s][1], 1_898_935_200_000);
    assert_eq!(p.fns.times[s][2], 1_899_052_200_000);
    // Z0004 replaces Z0001.
    let mut r = nt("Z", 4);
    r.ntype = "R";
    r.refd = Some(("Z", 1));
    feed(&mut p, FeedKind::Fns, mar5(9), &t(&r));
    assert_eq!(fns_live(&p.fns), 3);
    assert!(fns_slot_of(&p.fns, &key("Z", 1)).is_none());
    assert!(fns_slot_of(&p.fns, &key("Z", 4)).is_some());
    // Z0005 cancels Y0003; the cancelling NOTAM is not kept.
    let mut c = nt("Z", 5);
    c.ntype = "C";
    c.refd = Some(("Y", 3));
    feed(&mut p, FeedKind::Fns, mar5(9), &t(&c));
    assert_eq!(fns_live(&p.fns), 2);
    assert!(fns_slot_of(&p.fns, &key("Z", 5)).is_none());
    // A cancellation of a NOTAM not held, and a replacement of one not held.
    let mut c2 = nt("Z", 6);
    c2.ntype = "C";
    c2.refd = Some(("Y", 99));
    feed(&mut p, FeedKind::Fns, mar5(9), &t(&c2));
    let mut r2 = nt("Z", 7);
    r2.ntype = "R";
    r2.refd = Some(("Y", 98));
    feed(&mut p, FeedKind::Fns, mar5(9), &t(&r2));
    assert_eq!(fns_live(&p.fns), 3);
    let st = p.fns.stats;
    assert_eq!(
        (
            st.created,
            st.replaced,
            st.cancelled,
            st.cancel_unmatched,
            st.replace_unmatched
        ),
        (5, 1, 1, 1, 1)
    );
    assert_eq!(p.rows_seen[FeedKind::Fns as usize], 7);
    assert_eq!(p.rows_applied[FeedKind::Fns as usize], 6);
    assert_eq!(p.rows_unlinked, 1);
    assert_eq!(errors_total(&p.errors), 0);
}

// REQ: OA-RPL-050
#[test]
fn a_reissued_notam_replaces_the_held_one_unless_it_was_issued_earlier() {
    let mut p = pl();
    feed(&mut p, FeedKind::Fns, mar5(9), &notam(&nt("Z", 1)));
    let mut later = nt("Z", 1);
    later.issued = "2030-03-05T09:30:00.000Z";
    later.ext = "<fnse:canceled>2030-03-09T00:00:00.000Z</fnse:canceled>";
    feed(&mut p, FeedKind::Fns, mar5(10), &notam(&later));
    let mut earlier = nt("Z", 1);
    earlier.issued = "2030-03-05T08:00:00.000Z";
    feed(&mut p, FeedKind::Fns, mar5(11), &notam(&earlier));
    assert_eq!(fns_live(&p.fns), 1);
    let s = fns_slot_of(&p.fns, &key("Z", 1)).unwrap();
    assert_eq!(p.fns.times[s][3], 1_899_244_800_000);
    assert_eq!(
        (
            p.fns.stats.created,
            p.fns.stats.reissued,
            p.fns.stats.out_of_order
        ),
        (1, 1, 1)
    );
    assert_eq!(p.rows_out_of_order[FeedKind::Fns as usize], 1);
}

// REQ: OA-RPL-051
#[test]
fn notams_expire_by_replay_time_and_open_ended_ones_after_thirty_days() {
    let mut p = pl();
    feed(&mut p, FeedKind::Fns, mar5(9), &notam(&nt("Z", 1)));
    let mut open = nt("Z", 2);
    open.end = None;
    feed(&mut p, FeedKind::Fns, mar5(9), &notam(&open));
    // In force from the start time (10:00 on the 5th) to the end (18:30 on the 6th).
    let in_force = |p: &Pipeline, ms: i64| fns_in_force(&p.fns, ms);
    assert_eq!(in_force(&p, 1_898_935_200_000 - 1), 0);
    assert_eq!(in_force(&p, 1_898_935_200_000), 2);
    assert_eq!(in_force(&p, 1_899_052_200_001), 1);
    // A frame of any feed after the end moves the clock and the sweep runs.
    feed(
        &mut p,
        FeedKind::Fns,
        mar5(9) + 3 * 86_400 * 1_000_000_000,
        &notam(&nt("Z", 3)),
    );
    assert!(fns_slot_of(&p.fns, &key("Z", 1)).is_none());
    assert!(fns_slot_of(&p.fns, &key("Z", 2)).is_some());
    assert_eq!(p.fns.stats.expired, 1);
    let mut t = new_fns_table(4);
    assert_eq!(fns_sweep(&mut t, mar5(0)), 0);
    // Thirty days with no update.
    feed(
        &mut p,
        FeedKind::Fns,
        mar5(9) + 31 * 86_400 * 1_000_000_000,
        &notam(&nt("Z", 8)),
    );
    assert!(fns_slot_of(&p.fns, &key("Z", 2)).is_none());
    assert_eq!(p.fns.stats.expired, 3);
    // A cancelled time in the past ends a NOTAM at the next sweep.
    let mut c = nt("Z", 9);
    c.ext = "<fnse:canceled>2030-03-06T00:00:00.000Z</fnse:canceled>";
    feed(
        &mut p,
        FeedKind::Fns,
        mar5(9) + 40 * 86_400 * 1_000_000_000,
        &notam(&c),
    );
    assert_eq!(fns_live(&p.fns), 1);
    process_frame(
        &mut p,
        FeedKind::Fns,
        Kind::Status,
        mar5(9) + 40 * 86_400 * 1_000_000_000 + 120 * NS,
        b"x",
    );
    assert_eq!(fns_live(&p.fns), 0);
}

// REQ: OA-RPL-052
#[test]
fn a_notam_keeps_its_circle_altitudes_and_the_bounding_box_of_its_polygons() {
    let mut p = pl();
    let mut n = nt("Z", 1);
    n.members = polygon("30.10 -90.10 30.10 -90.20 30.20 -90.20 30.20 -90.10 30.10 -90.10");
    feed(&mut p, FeedKind::Fns, mar5(9), &notam(&n));
    let s = fns_slot_of(&p.fns, &key("Z", 1)).unwrap();
    // 30 degrees 12 minutes north, 90 degrees 45 minutes west, 5 NM = 9,260 m.
    assert_eq!(p.fns.circle[s], [302_000_000, -907_500_000, 9260]);
    assert_eq!(p.fns.polys[s], [1, 4]);
    assert_eq!(
        p.fns.bbox[s],
        [301_000_000, -902_000_000, 302_000_000, -901_000_000]
    );
    assert_eq!(p.fns.alt_ft[s], [0, 99_900]);
    assert_eq!(p.fns.feat_kind[s], oa_notam::FEAT_RUNWAY_ELEMENT);
    // A NOTAM with no polygon has no box.
    feed(&mut p, FeedKind::Fns, mar5(9), &notam(&nt("Z", 2)));
    let s2 = fns_slot_of(&p.fns, &key("Z", 2)).unwrap();
    assert_eq!((p.fns.polys[s2], p.fns.bbox[s2]), ([0, 0], [i32::MIN; 4]));
}

// REQ: OA-RPL-053
#[test]
fn malformed_fns_payloads_are_counted_by_kind_and_never_applied() {
    let mut p = pl();
    feed(&mut p, FeedKind::Fns, mar5(9), "<not-xml");
    feed(&mut p, FeedKind::Fns, mar5(9), "<other/>");
    // No identity: a NOTAM with neither a series nor a location is not applied.
    let x = notam(&nt("Z", 1))
        .replace("<event:series>Z</event:series>", "")
        .replace("<event:location>QQA</event:location>", "");
    feed(&mut p, FeedKind::Fns, mar5(9), &x);
    // A malformed field is counted and treated as absent: the NOTAM still applies.
    let y = notam(&nt("Z", 2)).replace(
        "<event:radius>5</event:radius>",
        "<event:radius>x</event:radius>",
    );
    feed(&mut p, FeedKind::Fns, mar5(9), &y);
    assert_eq!(err(&p, ErrKind::FnsXml), 1);
    assert_eq!(err(&p, ErrKind::FnsRejected), 1);
    assert_eq!(err(&p, ErrKind::FnsNoKey), 1);
    assert_eq!(err(&p, ErrKind::FnsField), 1);
    assert_eq!(fns_live(&p.fns), 1);
    let s = fns_slot_of(&p.fns, &key("Z", 2)).unwrap();
    assert_eq!(p.fns.circle[s], [i32::MIN, i32::MIN, 0]);
    assert_eq!(p.fns.stats.no_key, 1);
    assert_eq!(p.rows_applied[FeedKind::Fns as usize], 1);
    assert_eq!(p.rows_seen[FeedKind::Fns as usize], 2);
}

// REQ: OA-RPL-049
#[test]
fn a_local_format_notam_with_no_series_is_keyed_by_its_location() {
    let mut p = pl();
    let local = |loc: &str, n: u32| {
        notam(&nt("Z", n))
            .replace("<event:series>Z</event:series>", "")
            .replace(
                "<event:location>QQA</event:location>",
                &format!("<event:location>{loc}</event:location>"),
            )
    };
    // The same number at two locations is two NOTAMs; at one location it is one.
    feed(&mut p, FeedKind::Fns, mar5(9), &local("QQA", 42));
    feed(&mut p, FeedKind::Fns, mar5(9), &local("QQB", 42));
    feed(&mut p, FeedKind::Fns, mar5(9), &local("QQA", 42));
    assert_eq!(fns_live(&p.fns), 2);
    assert!(fns_slot_of(&p.fns, &key("!QQA", 42)).is_some());
    assert!(fns_slot_of(&p.fns, &key("!QQB", 42)).is_some());
    assert_eq!(p.fns.stats.reissued, 1);
    assert_eq!(p.fns.stats.no_key, 0);
}

// REQ: OA-RPL-054
#[test]
fn the_notam_table_is_bounded_and_refuses_when_full() {
    let mut t = new_fns_table(2);
    let mut parser = Parser::new(oa_xml::Limits::default()).unwrap();
    let mut dec = new_decoder(Limits::default());
    let mut stats = new_stats(0);
    let mut out = Vec::new();
    for n in 1..=3 {
        let mut notams = new_notams(4, 1 << 16, 16, 1 << 12);
        let x = notam(&nt("Z", n));
        let doc = parser.parse(x.as_bytes()).unwrap();
        oa_notam::decode_doc(&mut dec, &doc, &mut notams, &mut stats).unwrap();
        out.push(apply_fns(&mut t, &notams, 0, mar5(9)));
    }
    assert_eq!(
        out,
        [FnsApplied::New, FnsApplied::New, FnsApplied::TableFull]
    );
    assert_eq!((t.stats.created, t.stats.refused_full), (2, 1));
    assert_eq!(fns_live(&t), 2);
}

// ---- the state hash ---------------------------------------------------------------------------

fn swim_only() -> Pipeline {
    let mut p = pl();
    sfdps_flight(&mut p, 1, "TST1", "KA000001AB", at(3, 0, 0));
    sfdps_flight(&mut p, 2, "TST2", "KA000002AB", at(3, 0, 1));
    let airs = vec![tb(2000, Some("KA000001AB"), "T00001")
        .mrp("MFXA1", "09L", "NEW")
        .eta("MFXA1", 60_000)];
    feed(
        &mut p,
        FeedKind::Tbfm,
        at(3, 0, 2),
        &tb_doc(2000, "ZTT", &airs),
    );
    p
}

// REQ: OA-RPL-055
#[test]
fn replays_without_tfms_or_fns_hash_exactly_as_the_flight_table_does() {
    let mut p = swim_only();
    assert_eq!(replay_hash(&p), state_hash(&p.table));
    assert_eq!(snapshot(&p).hash, state_hash(&p.table));
    // Status frames of a feed count as a frame of that feed; so do empty-table replays: the
    // tables join the hash as soon as a frame of the feed has been fed.
    let mut q = swim_only();
    process_frame(
        &mut q,
        FeedKind::Tfms,
        Kind::Status,
        at(3, 1, 0),
        b"state=connected",
    );
    assert_ne!(replay_hash(&q), state_hash(&q.table));
    let mut r = swim_only();
    process_frame(
        &mut r,
        FeedKind::Fns,
        Kind::Status,
        at(3, 1, 0),
        b"state=connected",
    );
    assert_ne!(replay_hash(&r), state_hash(&r.table));
    assert_ne!(replay_hash(&r), replay_hash(&q));
    // The SFDPS and TBFM parts are unchanged by the new feeds being wired in: the flight table
    // hash of the replay with TFMS and FNS frames is the one without them.
    feed(
        &mut q,
        FeedKind::Tfms,
        at(3, 2, 0),
        &wrap_flight(&departure(
            7001,
            "TST1",
            Some("KA000001AB"),
            (3, 0, 0),
            (5, 0, 0),
            (2, 0, 0),
        )),
    );
    // (`p` sees the same replay times through status frames, which move the flight table's
    // clock and nothing else.)
    process_frame(&mut p, FeedKind::Sfdps, Kind::Status, at(3, 1, 0), b"x");
    process_frame(&mut p, FeedKind::Sfdps, Kind::Status, at(3, 2, 0), b"x");
    assert_eq!(state_hash(&q.table), state_hash(&p.table));
}

// REQ: OA-RPL-056
#[test]
fn the_tfms_and_fns_tables_change_the_hash_and_a_replay_hashes_the_same_every_time() {
    let build = || {
        let mut p = swim_only();
        feed(
            &mut p,
            FeedKind::Tfms,
            at(3, 1, 0),
            &wrap_flight(&departure(
                7001,
                "TST1",
                Some("KA000001AB"),
                (3, 0, 0),
                (5, 0, 0),
                (2, 0, 0),
            )),
        );
        feed(
            &mut p,
            FeedKind::Tfms,
            at(3, 1, 1),
            &wrap_fi(&rstr((3, 0, 0), 1, 15, 6)),
        );
        feed(&mut p, FeedKind::Fns, at(3, 1, 2), &notam(&nt("Z", 1)));
        p
    };
    let (a, b) = (build(), build());
    assert_eq!(replay_hash(&a), replay_hash(&b));
    assert_ne!(replay_hash(&a), state_hash(&a.table));
    // Each table moves the hash.
    let mut c = build();
    feed(&mut c, FeedKind::Fns, at(3, 1, 3), &notam(&nt("Z", 2)));
    assert_ne!(replay_hash(&c), replay_hash(&a));
    let mut d = build();
    feed(
        &mut d,
        FeedKind::Tfms,
        at(3, 1, 3),
        &wrap_flight(&departure(
            7001,
            "TST1",
            Some("KA000001AB"),
            (3, 5, 0),
            (5, 30, 0),
            (2, 0, 0),
        )),
    );
    assert_ne!(replay_hash(&d), replay_hash(&a));
    let mut e = build();
    feed(
        &mut e,
        FeedKind::Tfms,
        at(3, 1, 3),
        &wrap_fi(&rstr((3, 5, 0), 1, 30, 6)),
    );
    assert_ne!(replay_hash(&e), replay_hash(&a));
    // The order flights were first seen in does not matter.
    let mut f = swim_only();
    for (n, s) in [(7002u32, 1i64), (7001, 2)] {
        feed(
            &mut f,
            FeedKind::Tfms,
            at(3, 1, s),
            &wrap_flight(&departure(n, "TST1", None, (3, 0, 0), (5, 0, 0), (2, 0, 0))),
        );
    }
    let mut g = swim_only();
    for (n, s) in [(7001u32, 1i64), (7002, 2)] {
        feed(
            &mut g,
            FeedKind::Tfms,
            at(3, 1, s),
            &wrap_flight(&departure(n, "TST1", None, (3, 0, 0), (5, 0, 0), (2, 0, 0))),
        );
    }
    // Different slots, different times applied: the hashes differ only through update times.
    assert_eq!(f.tfms.stats.created, g.tfms.stats.created);
}

// REQ: OA-RPL-057
#[test]
fn a_mixed_swim_replay_from_recordings_is_the_same_however_it_is_read() {
    let mut s = Vec::new();
    let mut t = Vec::new();
    let mut n = Vec::new();
    let mut b = Vec::new();
    for k in 0..40i64 {
        s.extend(frame(
            1,
            k as u64,
            at(3, 0, 0) + k * NS,
            sfdps_doc(k).as_bytes(),
        ));
        t.extend(frame(
            1,
            k as u64,
            at(3, 0, 0) + k * NS + NS / 2,
            wrap_flight(&departure(
                7000 + (k % 5) as u32,
                "TST1",
                Some(&format!("KA{:06}AB", k % 5 + 1)),
                (3, 0, k),
                (5, 0, 0),
                (2, 0, 0),
            ))
            .as_bytes(),
        ));
        if k % 10 == 0 {
            n.extend(frame(
                1,
                k as u64,
                at(3, 0, 0) + k * NS + NS / 4,
                notam(&nt("Z", 1 + k as u32)).as_bytes(),
            ));
        }
        b.extend(frame(
            1,
            k as u64,
            at(3, 0, 0) + k * NS + 3 * NS / 4,
            wrap_fi(&aptc((3, 0, k), "KTBB", 40 + k as u32)).as_bytes(),
        ));
    }
    let run = |order: &[usize], chunk: usize| {
        let mut specs = Vec::new();
        for &i in order {
            specs.push(match i {
                0 => spec_from_bytes(FeedKind::Sfdps, vec![s.clone()]),
                1 => spec_from_bytes(FeedKind::Tfms, vec![t.clone(), b.clone()]),
                _ => spec_from_bytes(FeedKind::Fns, vec![n.clone()]),
            });
        }
        let mut p = pl();
        run_streams(&mut p, specs, chunk, &mut |_| {}).unwrap();
        p
    };
    let p = run(&[0, 1, 2], 64 * 1024);
    // The TFMS stream is two recordings in sequence, so the second one is a time regression;
    // use one stream with both for the equality check below.
    assert!(p.rows_applied[FeedKind::Tfms as usize] > 0);
    assert_eq!(tfms_live(&p.tfms), 5);
    assert_eq!(fns_live(&p.fns), 4);
    assert_eq!(p.frames[FeedKind::Fns as usize], 4);
    let want = snapshot(&p);
    for (order, chunk) in [([2usize, 1, 0], 1), ([1, 0, 2], 37), ([0, 2, 1], 4097)] {
        let q = run(&order, chunk);
        assert_eq!(snapshot(&q), want, "{order:?} {chunk}");
        assert_eq!(q.checkpoints, p.checkpoints);
        assert_eq!(q.errors, p.errors);
    }
}

fn sfdps_doc(k: i64) -> String {
    let n = (k % 5 + 1) as u32;
    sf_doc(&[sf(n, 1000 + k)
        .call("TST1")
        .plan(&format!("KA{n:06}AB"))
        .dep("KTAA")
        .arr("KTBB")])
}

// REQ: OA-RPL-058
#[test]
fn the_pipeline_still_hashes_and_counts_status_and_reject_frames_of_the_new_feeds() {
    let mut p = pl();
    process_frame(
        &mut p,
        FeedKind::Tfms,
        Kind::Status,
        at(3, 0, 0),
        b"state=connected",
    );
    process_frame(&mut p, FeedKind::Fns, Kind::Reject, at(3, 0, 1), b"refused");
    assert_eq!(p.frames[FeedKind::Tfms as usize], 1);
    assert_eq!(p.other_frames[FeedKind::Fns as usize], 1);
    assert_eq!(p.not_applied, [0; 5]);
    assert_eq!(errors_total(&p.errors), 0);
    assert_eq!(
        tfms_live(&p.tfms) + fns_live(&p.fns) + flow_live(&p.flows),
        0
    );
}

// REQ: OA-RPL-060
#[test]
fn the_tfms_and_fns_tables_never_move_or_grow_after_init() {
    fn f<T>(v: &Vec<T>) -> (usize, usize) {
        (v.as_ptr() as usize, v.capacity())
    }
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    let fp = |p: &Pipeline| {
        let (t, w, n) = (&p.tfms, &p.flows, &p.fns);
        vec![
            f(&t.slots.free),
            f(&t.slots.hash),
            f(&t.slots.index.cells),
            f(&t.flight_ref),
            f(&t.times),
            f(&t.link),
            f(&w.slots.free),
            f(&w.slots.index.cells),
            f(&w.title),
            f(&w.rates),
            f(&n.slots.free),
            f(&n.slots.index.cells),
            f(&n.series),
            f(&n.times),
            f(&n.bbox),
        ]
    };
    let start = fp(&p);
    for k in 0..60i64 {
        let x = wrap_flight(&departure(
            7000 + k as u32,
            "TST1",
            None,
            (3, 0, k),
            (5, 0, 0),
            (2, 0, 0),
        ));
        feed(&mut p, FeedKind::Tfms, at(3, 0, k), &x);
        feed(
            &mut p,
            FeedKind::Tfms,
            at(3, 0, k),
            &wrap_fi(&rstr((3, 0, k), 100 + k as u32, 15, 4)),
        );
        feed(
            &mut p,
            FeedKind::Fns,
            mar5(9) + k * NS,
            &notam(&nt("Z", 1 + k as u32)),
        );
    }
    // Hours later everything has expired and the slots go back on the free stacks.
    feed(
        &mut p,
        FeedKind::Tfms,
        at(23, 0, 0),
        &wrap_fi(&aptc((23, 0, 0), "KTBB", 1)),
    );
    assert_eq!(fp(&p), start);
    assert!(p.tfms.stats.evicted > 0 && p.flows.stats.evicted > 0);
    assert!(fns_live(&p.fns) > 0);
}

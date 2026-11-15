//! The benchmark's legacy extraction and the replay pipeline accept the same population from the
//! same documents: the same parser limits, the same decode mode, the same table sizes. Hand-made
//! fixtures; every identifier is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::ingest::{ingest_sfdps, ingest_tbfm, ingest_tfms, new_reader};
use oa_bench::legacy::{new_legacy, NO_TIME};
use oa_replay::{new_pipeline, process_frame, FeedKind, PipelineConfig};

const RECV_NS: i64 = 1_893_553_300_000_000_000;

fn replay_of(feed: FeedKind, doc: &[u8]) -> oa_replay::Pipeline {
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    process_frame(&mut p, feed, oa_feed::Kind::Message, RECV_NS, doc);
    p
}

fn gufi(i: usize) -> String {
    format!("{i:08x}-2222-4333-8444-555555555555")
}

/// A short track message with a position, one per `i`, with `pad` bytes of ignored remarks text.
fn th_min(i: usize, pad: usize) -> String {
    let remarks = if pad > 0 {
        format!("<remarks>{}</remarks>", "x".repeat(pad))
    } else {
        String::new()
    };
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="2030-01-02T03:00:05.250Z"><enRoute xsi:type="ns5:NasEnRouteType"><position xsi:type="ns5:NasAircraftPositionType" positionTime="2030-01-02T03:00:04Z"><position xsi:type="ns2:LocationPointType"><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>36.5 -95.25</pos></location></position></position></enRoute><gufi codeSpace="urn:uuid">{}</gufi>{remarks}</flight></message>"#,
        gufi(i)
    )
}

fn sfdps(messages: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">{messages}</ns5:MessageCollection>"#
    )
}

// REQ: OA-BENCH-055
#[test]
fn sfdps_document_of_two_mib_is_accepted_by_both_paths() {
    let doc = sfdps(&th_min(1, 2 << 20));
    assert!(doc.len() > 2 << 20 && doc.len() < 3 << 20);
    let p = replay_of(FeedKind::Sfdps, doc.as_bytes());
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_sfdps(&mut leg, &mut r, doc.as_bytes(), NO_TIME);
    assert_eq!(p.rows_seen[0], 1);
    assert_eq!(
        leg.counts.doc_errors[0], 0,
        "extraction refused the document"
    );
    assert_eq!(leg.th.flight.len() as u64, p.rows_applied[0]);
}

// REQ: OA-BENCH-055
#[test]
fn sfdps_document_of_4097_messages_keeps_the_same_prefix_in_both_paths() {
    let msgs: String = (0..4097).map(|i| th_min(i, 0)).collect();
    let doc = sfdps(&msgs);
    let p = replay_of(FeedKind::Sfdps, doc.as_bytes());
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_sfdps(&mut leg, &mut r, doc.as_bytes(), NO_TIME);
    assert_eq!(p.rows_seen[0], 4096);
    assert_eq!(
        leg.counts.doc_errors[0], 0,
        "extraction refused the whole document"
    );
    assert_eq!(leg.th.flight.len() as u64, p.rows_applied[0]);
    assert_eq!(leg.th.flight.len(), 4096);
    assert_eq!(leg.counts.rows_not_decoded, 1);
}

fn tbfm_air(i: usize) -> String {
    format!(
        r#"<air gufi="KR{i:07}" tmaId="G00001" apt="KTBB" dap="KTAA" aid="TST123" airType="AMD"><eta><mfx>MFXAA</mfx><eta_mfx>2030-01-02T03:30:00Z</eta_mfx><eta_dfx>2030-01-02T03:30:00Z</eta_dfx><eta_sfx>2030-01-02T03:30:00Z</eta_sfx><eta_rwy>2030-01-02T03:50:00Z</eta_rwy></eta><sta><mfx>MFXAA</mfx><sta_sfx>2030-01-02T03:31:00Z</sta_sfx><sta_rwy>2030-01-02T03:51:00Z</sta_rwy></sta></air>"#
    )
}

// REQ: OA-BENCH-055
#[test]
fn tbfm_document_of_4097_flights_keeps_the_same_prefix_in_both_paths() {
    let airs: String = (0..4097).map(tbfm_air).collect();
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><env xmlns="urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0" targetNamespace="urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0" envTime="2030-01-02T03:00:00.100Z" envSrce="TMA.ZXX.FAA.GOV"><tma msgTime="2030-01-02T03:00:00.100Z" msgId="1">{airs}</tma></env>"#
    );
    assert!(doc.len() > 1 << 20, "past the default parser limit");
    let p = replay_of(FeedKind::Tbfm, doc.as_bytes());
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_tbfm(&mut leg, &mut r, doc.as_bytes());
    assert_eq!(p.rows_seen[1], 4096);
    assert_eq!(
        leg.counts.doc_errors[1], 0,
        "extraction refused the whole document"
    );
    // A flight is an ETA row and a schedule row (two entries each): 8,194 rows, 4,096 kept.
    assert_eq!(leg.tbfm.kind.len() as u64, 2 * p.rows_applied[1]);
    assert_eq!(leg.counts.rows_not_decoded, 8194 - 4096);
}

fn tfms_msg(i: usize) -> String {
    format!(
        r#"<fdm:fltdMessage acid="TST123" airline="TST" arrArpt="KTBB" cdmPart="false" depArpt="KTAA" fdTrigger="HCS_TRACK_MSG" flightRef="{}" major="TST" sensitivity="A" sourceFacility="KZXX" sourceTimeStamp="2030-01-02T03:04:05Z" msgType="flightPlanInformation"><fdm:flightPlanInformation><nxcm:qualifiedAircraftId aircraftCategory="JET" userCategory="COMMERCIAL"><nxce:aircraftId>TST123</nxce:aircraftId><nxce:computerId><nxce:facilityIdentifier>KZXX</nxce:facilityIdentifier><nxce:idNumber>123</nxce:idNumber></nxce:computerId><nxce:gufi>KR{i:07}</nxce:gufi><nxce:departurePoint><nxce:airport>KTAA</nxce:airport></nxce:departurePoint><nxce:arrivalPoint><nxce:airport>KTBB</nxce:airport></nxce:arrivalPoint></nxcm:qualifiedAircraftId><nxcm:ncsmRouteData><nxcm:eta etaType="ESTIMATED" timeValue="2030-01-02T04:30:00Z"/><nxcm:etd etdType="ESTIMATED" timeValue="2030-01-02T02:05:00Z"/><nxcm:routeOfFlight>KTAA./.ABC..DEF.STAR1.KTBB/0100</nxcm:routeOfFlight></nxcm:ncsmRouteData></fdm:flightPlanInformation></fdm:fltdMessage>"#,
        100_000_000 + i
    )
}

// REQ: OA-BENCH-055
#[test]
fn tfms_document_past_the_flight_table_keeps_the_same_prefix_in_both_paths() {
    let n = oa_tfms::DEFAULT_FLIGHT_CAPACITY + 1;
    let msgs: String = (0..n).map(tfms_msg).collect();
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:tfmDataService xmlns:ds="urn:us:gov:dot:faa:atm:tfm:tfmdataservice" xmlns="urn:us:gov:dot:faa:atm:tfm:tfmdataservice" xmlns:fdm="urn:us:gov:dot:faa:atm:tfm:flightdata" xmlns:nxce="urn:us:gov:dot:faa:atm:tfm:tfmdatacoreelements" xmlns:nxcm="urn:us:gov:dot:faa:atm:tfm:flightdatacommonmessages"><fltdOutput>{msgs}</fltdOutput></ds:tfmDataService>"#
    );
    let p = replay_of(FeedKind::Tfms, doc.as_bytes());
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    ingest_tfms(&mut leg, &mut r, doc.as_bytes());
    assert_eq!(p.rows_seen[2] as usize, n - 1);
    assert_eq!(
        leg.counts.doc_errors[2], 0,
        "extraction refused the whole document"
    );
    // Two entries (ETA and ETD) per accepted flight row.
    assert_eq!(leg.tfms.kind.len() as u64, 2 * p.rows_applied[2]);
    assert_eq!(leg.counts.rows_not_decoded, 1);
}

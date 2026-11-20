//! The TFMS actual-arrival table is a diagnostic: the report text and counts of the existing
//! tables do not change with it populated.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use oa_bench::ingest::{ingest_tfms, new_reader};
use oa_bench::legacy::new_legacy;
use oa_bench::report::extraction_counts;

const FLIGHT_ATTRS: &str = r#"acid="TST123" airline="TST" arrArpt="KTBB" cdmPart="false" depArpt="KTAA" fdTrigger="HCS_TRACK_MSG" flightRef="100000001" major="TST" sensitivity="A" sourceFacility="KZXX" sourceTimeStamp="2030-01-02T03:04:05Z""#;
const QAID: &str = r#"<nxcm:qualifiedAircraftId aircraftCategory="JET" userCategory="COMMERCIAL"><nxce:aircraftId>TST123</nxce:aircraftId><nxce:computerId><nxce:facilityIdentifier>KZXX</nxce:facilityIdentifier><nxce:idNumber>123</nxce:idNumber></nxce:computerId><nxce:gufi>KR000001Aa</nxce:gufi><nxce:departurePoint><nxce:airport>KTAA</nxce:airport></nxce:departurePoint><nxce:arrivalPoint><nxce:airport>KTBB</nxce:airport></nxce:arrivalPoint></nxcm:qualifiedAircraftId>"#;

/// A TFMS document whose arrival time is of `eta_type` (`ESTIMATED` or `ACTUAL`) at `time`.
fn doc(eta_type: &str, time: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:tfmDataService xmlns:ds="urn:us:gov:dot:faa:atm:tfm:tfmdataservice" xmlns="urn:us:gov:dot:faa:atm:tfm:tfmdataservice" xmlns:fdm="urn:us:gov:dot:faa:atm:tfm:flightdata" xmlns:nxce="urn:us:gov:dot:faa:atm:tfm:tfmdatacoreelements" xmlns:nxcm="urn:us:gov:dot:faa:atm:tfm:flightdatacommonmessages"><fltdOutput><fdm:fltdMessage {FLIGHT_ATTRS} msgType="flightPlanInformation"><fdm:flightPlanInformation>{QAID}<nxcm:ncsmRouteData><nxcm:eta etaType="{eta_type}" timeValue="{time}"/><nxcm:routeOfFlight>KTAA./.ABC..DEF.STAR1.KTBB/0100</nxcm:routeOfFlight></nxcm:ncsmRouteData></fdm:flightPlanInformation></fdm:fltdMessage></fltdOutput></ds:tfmDataService>"#
    )
}

// REQ: OA-BENCH-136
#[test]
fn the_report_text_does_not_read_the_actual_arrival_table() {
    let mut r = new_reader().unwrap();
    let mut b = new_legacy();
    ingest_tfms(
        &mut b,
        &mut r,
        doc("ESTIMATED", "2030-01-02T04:30:00Z").as_bytes(),
    );
    for t in ["04:31:00", "04:32:00", "04:33:00"] {
        ingest_tfms(
            &mut b,
            &mut r,
            doc("ACTUAL", &format!("2030-01-02T{t}Z")).as_bytes(),
        );
    }
    // Actual arrivals go to their own table, through the reader, and the estimate table keeps
    // its one row.
    assert_eq!(b.tfms_actual.kind.len(), 3);
    assert_eq!(b.tfms.kind.len(), 1);
    // The counts of the report are the same with the table populated and emptied.
    let with = format!("{:?}", extraction_counts(&b));
    b.tfms_actual = Default::default();
    assert_eq!(with, format!("{:?}", extraction_counts(&b)));
    // The report still counts the four documents and the three times that were not estimates.
    assert!(with.contains("(\"TFMS documents\", 4)"), "{with}");
    assert!(
        with.contains("(\"TFMS times dropped, reason type is not ESTIMATED\", 3)"),
        "{with}"
    );
    // A full table counts in its own counter and in none the report shows.
    let mut c = new_legacy();
    c.limits.tfms = 1;
    for t in ["04:31:00", "04:32:00"] {
        ingest_tfms(
            &mut c,
            &mut r,
            doc("ACTUAL", &format!("2030-01-02T{t}Z")).as_bytes(),
        );
    }
    assert_eq!(
        (c.counts.overflow_tfms_actual, c.counts.overflow_tfms),
        (1, 0)
    );
}

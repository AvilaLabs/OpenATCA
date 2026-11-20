//! The flight table keeps the last transponder code an SFDPS message gave. Made-up identifiers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::ingest::{ingest_sfdps, new_reader};
use oa_bench::legacy::{new_legacy, NO_BEACON, NO_TIME};

fn doc(source: &str, gufi: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0"><message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="{source}" system="SLC" timestamp="2030-01-02T03:00:05.250Z">{body}<flightIdentification xsi:type="ns5:NasFlightIdentificationType" computerId="946" siteSpecificPlanId="237" aircraftIdentification="TST123"/><gufi codeSpace="urn:uuid">{gufi}</gufi></flight></message></ns5:MessageCollection>"#
    )
}

fn ba(code: &str) -> String {
    format!(
        r#"<enRoute xsi:type="ns5:NasEnRouteType"><beaconCodeAssignment><currentBeaconCode>{code}</currentBeaconCode></beaconCodeAssignment></enRoute>"#
    )
}

// REQ: OA-BENCH-133
#[test]
fn the_flight_keeps_its_last_beacon_code_and_none_when_never_given() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    let a = "11111111-2222-4333-8444-555555555555";
    let b = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    ingest_sfdps(
        &mut leg,
        &mut r,
        doc("BA", a, &ba("0123")).as_bytes(),
        NO_TIME,
    );
    assert_eq!(leg.flights.beacon[0], 123);
    ingest_sfdps(
        &mut leg,
        &mut r,
        doc("BA", a, &ba("7700")).as_bytes(),
        NO_TIME,
    );
    assert_eq!(leg.flights.beacon[0], 7700);
    // A message without a code leaves it; a malformed one (digit 8) is not a code.
    ingest_sfdps(
        &mut leg,
        &mut r,
        doc("BA", a, &ba("1289")).as_bytes(),
        NO_TIME,
    );
    ingest_sfdps(&mut leg, &mut r, doc("CL", a, "").as_bytes(), NO_TIME);
    assert_eq!(leg.flights.beacon[0], 7700);
    ingest_sfdps(&mut leg, &mut r, doc("CL", b, "").as_bytes(), NO_TIME);
    assert_eq!(leg.flights.beacon[1], NO_BEACON);
}

// REQ: OA-BENCH-134
#[test]
fn beacon_changes_are_kept_with_their_message_time() {
    let mut leg = new_legacy();
    let mut r = new_reader().unwrap();
    let a = "11111111-2222-4333-8444-555555555555";
    for code in ["0123", "0123", "7700"] {
        ingest_sfdps(
            &mut leg,
            &mut r,
            doc("BA", a, &ba(code)).as_bytes(),
            NO_TIME,
        );
    }
    // The repeated code adds no row; the change does. The time is the message's own.
    assert_eq!(leg.beacons.code, vec![123, 7700]);
    assert_eq!(leg.beacons.flight, vec![0, 0]);
    assert_eq!(leg.beacons.ts_ms[0], leg.beacons.ts_ms[1]);
    assert!(leg.beacons.ts_ms[0] > 0);
}

// REQ: OA-BENCH-135
#[test]
fn beacon_changes_over_the_cap_are_counted_not_stored() {
    use oa_bench::ingest::DROP_SFDPS_BEACON_FULL;
    let mut leg = new_legacy();
    leg.limits.beacons = 1;
    let mut r = new_reader().unwrap();
    let a = "11111111-2222-4333-8444-555555555555";
    for code in ["0123", "7700", "7600"] {
        ingest_sfdps(
            &mut leg,
            &mut r,
            doc("BA", a, &ba(code)).as_bytes(),
            NO_TIME,
        );
    }
    assert_eq!(leg.beacons.code, vec![123]);
    assert_eq!(leg.counts.reasons.get(DROP_SFDPS_BEACON_FULL), Some(&2));
    // The flight still holds the last code.
    assert_eq!(leg.flights.beacon[0], 7600);
}

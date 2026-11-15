//! Tests of the SFDPS flight table's memory of order: the position time, and the message source
//! time that orders status, controlling unit and flight-plan ownership. Every identifier is made
//! up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_replay::table::{CLASS_ACTIVE, CLASS_MASK, HAS_POS};
use oa_replay::{
    apply_sfdps, apply_tbfm, new_table, slot_of_gufi, state_hash, Applied, FlightTable, TableConfig,
};

fn table() -> FlightTable {
    new_table(TableConfig {
        capacity: 16,
        ..TableConfig::default()
    })
    .unwrap()
}

fn sfa(t: &mut FlightTable, m: Sf, now: i64) -> Applied {
    sfa_doc(t, &sf_doc(&[m]), now)
}

fn sfa_doc(t: &mut FlightTable, xml: &str, now: i64) -> Applied {
    let ev = events(xml);
    apply_sfdps(t, &ev, 0, now)
}

fn slot(t: &FlightTable, n: u32) -> usize {
    slot_of_gufi(t, gufi(n)).expect("flight present")
}

// REQ: OA-RPL-200
#[test]
fn a_malformed_position_does_not_advance_the_position_time() {
    let mut t = table();
    sfa(&mut t, sf(1, 1000).pos(10.0, 20.0, 100), NS);
    // PositionTime 200 but coordinates that do not parse: the decoder drops the position.
    let bad = sf_doc(&[sf(1, 2000).pos(11.0, 21.0, 200)]).replace("11.000000 21.000000", "abc def");
    let ev = events(&bad);
    assert!(!oa_fixm::has_field(&ev, 0, oa_fixm::Field::Position));
    assert!(oa_fixm::field_error(&ev, 0, oa_fixm::Field::Position));
    assert_eq!(apply_sfdps(&mut t, &ev, 0, 2 * NS), Applied::Applied);
    let s = slot(&t, 1);
    assert_eq!(t.pos_time_ms[s], DAY + 100);
    // A valid position older than the bad row's time but newer than the held one is applied.
    assert_eq!(
        sfa(&mut t, sf(1, 3000).pos(12.0, 22.0, 150), 3 * NS),
        Applied::Applied
    );
    assert_eq!((t.lat_e7[s], t.lon_e7[s]), (120_000_000, 220_000_000));
    assert_eq!(t.pos_time_ms[s], DAY + 150);
    assert_eq!(t.stats.out_of_order, 0);
}

// REQ: OA-RPL-200
#[test]
fn a_position_time_without_a_position_element_does_not_advance_it() {
    let mut t = table();
    sfa(&mut t, sf(1, 1000).pos(10.0, 20.0, 100), NS);
    // Remove the location, keeping the positionTime attribute on the enclosing element.
    let xml = sf_doc(&[sf(1, 2000).pos(11.0, 21.0, 200)]);
    let a = xml
        .find("<position xsi:type=\"ns2:LocationPointType\">")
        .unwrap();
    let b = xml.find("</position></position>").unwrap() + "</position>".len();
    let xml = format!("{}{}", &xml[..a], &xml[b..]);
    let ev = events(&xml);
    assert!(!oa_fixm::has_field(&ev, 0, oa_fixm::Field::Position));
    assert!(oa_fixm::has_field(&ev, 0, oa_fixm::Field::PositionTime));
    assert_eq!(apply_sfdps(&mut t, &ev, 0, 2 * NS), Applied::Applied);
    let s = slot(&t, 1);
    assert_eq!(t.pos_time_ms[s], DAY + 100);
    assert_eq!(
        sfa(&mut t, sf(1, 3000).pos(12.0, 22.0, 150), 3 * NS),
        Applied::Applied
    );
    assert_eq!(t.pos_time_ms[s], DAY + 150);
    assert_ne!(t.flags[s] & HAS_POS, 0);
}

// REQ: OA-RPL-201
#[test]
fn an_older_sfdps_message_does_not_roll_back_status_unit_or_computer_id() {
    let mut t = table();
    sfa(
        &mut t,
        sf(1, 5000).status("ACTIVE").unit("ZAA", "01").cid("AAA"),
        NS,
    );
    let s = slot(&t, 1);
    assert_eq!(t.flags[s] & CLASS_MASK, CLASS_ACTIVE);
    // Delivered later, but issued earlier: finished, another unit, another computer id.
    sfa(
        &mut t,
        sf(1, 4000)
            .status("COMPLETED")
            .unit("ZBB", "02")
            .cid("BBB")
            .call("TST1"),
        2 * NS,
    );
    assert_eq!(t.flags[s] & CLASS_MASK, CLASS_ACTIVE);
    assert_eq!(&t.ctrl[s][..3], b"ZAA");
    assert_eq!(&t.computer_id[s][..3], b"AAA");
    assert_eq!(t.stats.stale_amendments, 1);
    // Identity that fills an empty column still applies.
    assert_eq!(&t.callsign[s][..4], b"TST1");
    // The flight stays active for two hours of silence, not ten minutes.
    oa_replay::advance_time(&mut t, 2 * NS + 11 * 60 * NS);
    assert!(slot_of_gufi(&t, gufi(1)).is_some());
    // The same source time (a redelivery) and a newer one apply.
    sfa(&mut t, sf(1, 5000).unit("ZCC", "03"), 3 * NS);
    assert_eq!(&t.ctrl[s][..3], b"ZCC");
    sfa(&mut t, sf(1, 6000).status("COMPLETED"), 4 * NS);
    assert_ne!(t.flags[s] & CLASS_MASK, CLASS_ACTIVE);
    assert_eq!(t.stats.stale_amendments, 1);
}

// REQ: OA-RPL-202
#[test]
fn an_old_message_does_not_take_a_reused_flight_plan_identifier_back() {
    let mut t = table();
    sfa(&mut t, sf(1, 1000).plan("KA000001AB"), NS);
    // Flight 2 takes the reused identifier with a newer message.
    sfa(&mut t, sf(2, 2000).plan("KA000001AB"), 2 * NS);
    // An old message of flight 1 (earlier than flight 2's) arrives late.
    sfa(&mut t, sf(1, 1500).plan("KA000001AB"), 3 * NS);
    assert_eq!(t.stats.plan_refused_old, 1);
    assert_eq!(t.stats.key_reassigned, 1);
    // A TBFM update for the identifier links to flight 2, the owner.
    let r = rows(&tb_doc(
        4000,
        "ZTT",
        &[tb(4000, Some("KA000001AB"), "T00001").mrp("MFXA1", "09L", "NEW")],
    ));
    assert_eq!(apply_tbfm(&mut t, &r, 0, 4 * NS), Applied::Applied);
    assert_eq!(&t.meter_fix[slot(&t, 2)][..5], b"MFXA1");
    assert_eq!(t.meter_fix[slot(&t, 1)], [0; 8]);
    assert_eq!(t.stats.tbfm_linked_gufi, 1);
    assert_eq!(t.stats.tbfm_created, 0);
    // A newer message of flight 1 does take it back.
    sfa(&mut t, sf(1, 7000).plan("KA000001AB"), 5 * NS);
    assert_eq!(t.stats.plan_refused_old, 1);
    assert_eq!(t.stats.key_reassigned, 2);
}

// REQ: OA-RPL-201
#[test]
fn the_source_time_and_its_counters_are_part_of_the_state_hash() {
    let mut a = table();
    let mut b = table();
    sfa(&mut a, sf(1, 5000), NS);
    sfa(&mut b, sf(1, 6000), NS);
    assert_ne!(state_hash(&a), state_hash(&b));
    let h = state_hash(&a);
    sfa(&mut a, sf(1, 4000), 2 * NS);
    assert_ne!(state_hash(&a), h);
}

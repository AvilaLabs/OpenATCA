//! Caller-owned production-window policy; the reusable decoder takes a plain window.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use oa_fixm::{new_events, Decoder, Events, FixmError, Limits, Summary};
use oa_xml::{Limits as XmlLimits, Parser};
const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">"#;

const TAIL: &str = "</ns5:MessageCollection>";

const GUFI: &str = "11111111-2222-3333-4444-555555555555";

struct Run {
    ev: Events,
    sum: Summary,
    dec: Decoder,
}

fn gufi_el(g: &str) -> String {
    format!(r#"<gufi codeSpace="urn:uuid">{g}</gufi>"#)
}

fn message_with(flight_attrs: &str, body: &str) -> String {
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" {flight_attrs}>{body}</flight></message>"#
    )
}

fn doc(msgs: &[String]) -> String {
    format!("{HEAD}{}{TAIL}", msgs.concat())
}

fn at(ts: &str, body: &str) -> String {
    message_with(
        &format!(r#"centre="ZXX" source="TH" system="SLC" timestamp="{ts}""#),
        &format!("{}{body}", gufi_el(GUFI)),
    )
}

fn untouched(limits: Limits, dropped: u64) -> oa_fixm::Stats {
    let mut s = Decoder::new(limits).stats().clone();
    s.sealed_window_dropped = dropped;
    s
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
    let prod = Limits::new(oa_sealed::sealed_drop_window());
    let (r, res) = run_with(&doc(&[at("2026-11-21T00:00:00Z", "")]), 64, 1 << 16, prod);
    res.unwrap();
    assert_eq!(r.sum.sealed_window_dropped, 1);
    let (r, res) = run_with(&xml, 64, 1 << 16, prod);
    res.unwrap();
    assert_eq!((r.sum.sealed_window_dropped, r.ev.len), (0, 1));
}

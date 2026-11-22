//! The SFDPS adapter and the replay, over hand-written messages. Identifiers are made up; only the
//! element structure follows real SFDPS messages.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use oa_feed::encode_frame;
use oa_fixm::{clear_events, new_events, parse_iso_ms, Decoder, Events, Limits};
use oa_flight::{
    flight_by_address, FlightConfig, Op, ALT_NIL, ALT_SIMPLE, ALT_VFR, OP_ADDRESS, OP_ASSIGN,
    OP_DEPART, OP_HO_ACCEPT, OP_HO_INITIATE, OP_HO_RETRACT, OP_IDENTITY, OP_INTERIM, OP_OWNER,
    OP_REQUEST, OP_SET_PLAN,
};
use oa_flight_sfdps::{
    lag_quantile_s, recordings_in, replay_sfdps, replay_sfdps_timed_counted, row_ops, AdapterStats,
    ReplayConfig,
};
use oa_xml::{Limits as XmlLimits, Parser};

const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">"#;
const GUFI: &str = "11111111-2222-3333-4444-555555555555";

fn msg(source: &str, ts: &str, body: &str) -> String {
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="{source}" system="SLC" timestamp="{ts}"><gufi codeSpace="urn:uuid">{GUFI}</gufi>{body}</flight></message>"#
    )
}

fn doc(m: &str) -> String {
    format!("{HEAD}{m}</ns5:MessageCollection>")
}

fn ops_of(xml: &str, seen_ms: i64, st: &mut AdapterStats) -> Vec<Op> {
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let d = parser.parse(xml.as_bytes()).unwrap();
    let mut dec = Decoder::new(Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ));
    let mut ev: Events = new_events(8, 4096);
    clear_events(&mut ev);
    dec.decode(&d, xml.as_bytes(), &mut ev).unwrap();
    let mut out = Vec::new();
    for row in 0..ev.len {
        row_ops(&ev, row, seen_ms, row as u64, st, &mut out);
    }
    out
}

const FULL: &str = r#"<flightStatus fdpsFlightStatus="ACTIVE"/><flightIdentification aircraftIdentification="ABC123" computerId="123"/><departure departurePoint="KAAA"><runwayPositionAndTime><runwayTime><actual time="2030-01-02T03:00:00Z"/></runwayTime></runwayPositionAndTime></departure><arrival arrivalPoint="KBBB"/><aircraftDescription aircraftAddress="A1B2C3"/><assignedAltitude><simple uom="FEET">36000.0</simple></assignedAltitude><interimAltitude xsi:nil="true"/><requestedAltitude><vfr/></requestedAltitude><controllingUnit unitIdentifier="ZXX" sectorIdentifier="12"/>"#;

// REQ: OA-FLT-017
#[test]
fn a_full_message_makes_the_plan_altitude_owner_address_and_identity_operations() {
    let mut st = AdapterStats::default();
    let ops = ops_of(
        &doc(&msg("TH", "2030-01-02T03:04:05.678Z", FULL)),
        1_000,
        &mut st,
    );
    let kinds: Vec<u8> = ops.iter().map(|o| o.kind).collect();
    for k in [
        OP_SET_PLAN,
        OP_DEPART,
        OP_ASSIGN,
        OP_INTERIM,
        OP_REQUEST,
        OP_OWNER,
        OP_ADDRESS,
        OP_IDENTITY,
    ] {
        assert!(kinds.contains(&k), "operation {k} is made: {kinds:?}");
    }
    let by = |k: u8| ops.iter().find(|o| o.kind == k).unwrap();
    assert_eq!(by(OP_ASSIGN).alt.form, ALT_SIMPLE);
    assert_eq!(by(OP_ASSIGN).alt.a_ft, 36_000);
    assert_eq!(
        by(OP_INTERIM).alt.form,
        ALT_NIL,
        "an explicit clear is a state, not a number"
    );
    assert_eq!(by(OP_REQUEST).alt.form, ALT_VFR);
    assert_eq!(by(OP_ADDRESS).address, 0x00A1_B2C3);
    assert_eq!(by(OP_ASSIGN).seen_ms, 1_000);
    assert_eq!(
        by(OP_ASSIGN).ts_ms,
        parse_iso_ms(b"2030-01-02T03:04:05.678Z").unwrap(),
        "every operation carries the message's own time"
    );
    assert_eq!(st.rows, 1);
    assert_eq!(st.ops.iter().sum::<u64>(), ops.len() as u64);
}

// REQ: OA-FLT-018
#[test]
fn the_unknown_owner_and_the_receiving_unit_without_an_event_make_no_operation_and_are_counted() {
    let mut st = AdapterStats::default();
    let body = r#"<flightStatus fdpsFlightStatus="ACTIVE"/><controllingUnit unitIdentifier="???" sectorIdentifier="12"/><enRoute><boundaryCrossings><handoff><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#;
    let ops = ops_of(
        &doc(&msg("TH", "2030-01-02T03:04:05.678Z", body)),
        1_000,
        &mut st,
    );
    assert!(ops
        .iter()
        .all(|o| o.kind != OP_OWNER && o.kind != OP_HO_INITIATE));
    assert_eq!(st.owner_unknown, 1);
    assert_eq!(st.handoff_receiving_only, 1);
}

// REQ: OA-FLT-018
#[test]
fn handoff_events_make_their_operations_and_an_event_missing_a_unit_makes_none() {
    let mut st = AdapterStats::default();
    let init = r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/><transferringUnit unitIdentifier="ZXX" sectorIdentifier="12"/></handoff></boundaryCrossings></enRoute>"#;
    let acc = r#"<enRoute><boundaryCrossings><handoff event="ACCEPTANCE"><acceptingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#;
    let ret = r#"<enRoute><boundaryCrossings><handoff event="RETRACTION"/></boundaryCrossings></enRoute>"#;
    let half = r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#;
    let t = "2030-01-02T03:04:05.678Z";
    let a = ops_of(&doc(&msg("OH", t, init)), 1, &mut st);
    assert_eq!(
        a.iter().map(|o| o.kind).collect::<Vec<_>>(),
        [OP_HO_INITIATE]
    );
    let a = ops_of(&doc(&msg("OH", t, acc)), 1, &mut st);
    assert_eq!(a.iter().map(|o| o.kind).collect::<Vec<_>>(), [OP_HO_ACCEPT]);
    let a = ops_of(&doc(&msg("OH", t, ret)), 1, &mut st);
    assert_eq!(
        a.iter().map(|o| o.kind).collect::<Vec<_>>(),
        [OP_HO_RETRACT]
    );
    let a = ops_of(&doc(&msg("OH", t, half)), 1, &mut st);
    assert!(a.is_empty());
    assert_eq!(st.handoff_incomplete, 1);
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-flight-sfdps-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn frame(seq: u64, recv_ms: i64, xml: &str) -> Vec<u8> {
    frame_kind(oa_feed::Kind::Message as u8, seq, recv_ms, xml)
}

fn frame_kind(kind: u8, seq: u64, recv_ms: i64, payload: &str) -> Vec<u8> {
    let mut buf = vec![0u8; payload.len() + 64];
    let n = encode_frame(
        &mut buf,
        kind,
        0,
        seq,
        recv_ms * 1_000_000,
        0,
        payload.as_bytes(),
    )
    .unwrap();
    buf.truncate(n);
    buf
}

fn cfg(from_ns: i64, to_ns: i64) -> ReplayConfig {
    ReplayConfig {
        flight: FlightConfig {
            capacity: 64,
            lag_floor_ms: 58_000,
            max_future_ms: 5_000,
            refusal_log: 16,
            conflict_log: 16,
            removal_log: 16,
        },
        from_ns,
        to_ns,
        sweep_every_ms: 60_000,
        retain_ms: 3_600_000,
        silence_ms: 6 * 3_600_000,
    }
}

// REQ: OA-FLT-019
#[test]
fn a_replay_applies_the_lag_and_a_question_sees_only_what_had_aged_in() {
    let dir = temp_dir("lag");
    let ts = parse_iso_ms(b"2030-01-02T03:04:05.000Z").unwrap();
    // Received 60 s after its own time, so it is available at ts + 60 s.
    let recv = ts + 60_000;
    let body = r#"<flightStatus fdpsFlightStatus="ACTIVE"/><aircraftDescription aircraftAddress="A1B2C3"/><assignedAltitude><simple uom="FEET">24000.0</simple></assignedAltitude>"#;
    let x = doc(&msg("TH", "2030-01-02T03:04:05.000Z", body));
    std::fs::write(dir.join("a.oaf"), frame(1, recv, &x)).unwrap();
    let paths = recordings_in(&dir).unwrap();
    assert_eq!(paths.len(), 1);
    let mut asked: Vec<(i64, bool)> = Vec::new();
    let rep = replay_sfdps(paths, &cfg(i64::MIN, i64::MAX), &mut |t, _, avail| {
        if avail == i64::MAX {
            asked.push((avail, flight_by_address(t, 0x00A1_B2C3).is_some()));
        }
    })
    .unwrap();
    assert_eq!(rep.frames, 1);
    assert_eq!(rep.lag.rows, 1);
    assert_eq!(rep.lag.min_ms, 60_000);
    assert_eq!(lag_quantile_s(&rep.lag, 50, 100), Some(61));
    assert_eq!(
        asked,
        [(i64::MAX, true)],
        "after the last operation the flight is held"
    );
    assert_eq!(rep.table.stats.refused.iter().sum::<u64>(), 0);
    assert_eq!(rep.assigned_forms[1], 1);
    let f = flight_by_address(&rep.table, 0x00A1_B2C3).unwrap();
    assert_eq!(f.assigned.a_ft, 24_000);
    assert_eq!(
        f.stamps[oa_flight::F_ASSIGNED].avail_ms,
        recv.max(ts + 58_000)
    );

    // The first operation is announced at its availability time, before it is applied.
    let mut before = Vec::new();
    replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |t, _, avail| {
            before.push((avail, flight_by_address(t, 0x00A1_B2C3).is_some()));
        },
    )
    .unwrap();
    assert_eq!(
        before.first(),
        Some(&(recv, false)),
        "the table is empty before the first operation"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// REQ: OA-FLT-019
#[test]
fn frames_outside_the_window_are_skipped_and_the_run_ends_at_its_end() {
    let dir = temp_dir("window");
    let body = r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#;
    let x = doc(&msg("TH", "2030-01-02T03:04:05.000Z", body));
    let base = parse_iso_ms(b"2030-01-02T03:05:05.000Z").unwrap();
    let mut bytes = Vec::new();
    for k in 0..6 {
        let time = base + k as i64 * 1000;
        if k == 2 {
            let mut corrupt = frame(99, time, &x);
            *corrupt.last_mut().expect("encoded frame has a CRC") ^= 1;
            bytes.extend(corrupt);
        }
        bytes.extend(frame(k, time, &x));
    }
    std::fs::write(dir.join("a.oaf"), bytes).unwrap();
    let from = (base + 1000) * 1_000_000;
    let to = (base + 3000) * 1_000_000;
    let rep = replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(from, to),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(rep.frames_skipped_before, 1);
    assert_eq!(rep.frames, 3, "frames at 1, 2 and 3 s");
    assert_eq!(
        rep.frames_after_end, 1,
        "the run ends at the first frame past the window"
    );
    let scan = rep
        .merge
        .expect("window-limited run retains reader diagnostics");
    assert_eq!(scan.errors[oa_replay::ErrKind::FrameCrc as usize], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// REQ: OA-FLT-022
#[test]
fn a_flight_first_seen_active_with_an_actual_departure_departs() {
    let dir = temp_dir("depart");
    let ts = parse_iso_ms(b"2030-01-02T03:04:05.000Z").unwrap();
    let body = r#"<flightStatus fdpsFlightStatus="ACTIVE"/><departure departurePoint="KAAA"><runwayPositionAndTime><runwayTime><actual time="2030-01-02T03:00:00Z"/></runwayTime></runwayPositionAndTime></departure><aircraftDescription aircraftAddress="A1B2C3"/><controllingUnit unitIdentifier="ZXX" sectorIdentifier="12"/>"#;
    let x = doc(&msg("TH", "2030-01-02T03:04:05.000Z", body));
    std::fs::write(dir.join("a.oaf"), frame(1, ts + 60_000, &x)).unwrap();
    let rep = replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(
        rep.table.stats.refused.iter().sum::<u64>(),
        0,
        "{:?}",
        rep.refusals
    );
    let f = flight_by_address(&rep.table, 0x00A1_B2C3).unwrap();
    assert_eq!(
        oa_flight::view_of(&f, b"ZXX\0").unwrap().state,
        oa_flight::ST_DEPARTED
    );
    assert_eq!(rep.view_states[oa_flight::ST_DEPARTED as usize], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// REQ: OA-FLT-018
#[test]
fn a_handoff_unit_the_faa_gives_as_unknown_makes_no_operation_and_is_counted() {
    let mut st = AdapterStats::default();
    let init = r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/><transferringUnit unitIdentifier="???" sectorIdentifier="12"/></handoff></boundaryCrossings></enRoute>"#;
    let a = ops_of(
        &doc(&msg("OH", "2030-01-02T03:04:05.678Z", init)),
        1,
        &mut st,
    );
    assert!(a.is_empty());
    assert_eq!(st.handoff_unknown_unit, 1);
    assert_eq!(st.handoff_incomplete, 0);
}

// REQ: OA-SEAL-014
#[test]
fn the_replay_drops_sealed_window_frames_and_rows_and_counts_them() {
    // The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes), Unix nanoseconds.
    const FROM: i64 = 1_795_131_900_000_000_000;
    let dir = temp_dir("sealed");
    let body = r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#;
    let ok = doc(&msg("TH", "2026-11-19T23:00:00.000Z", body));
    // Received at from - 1 ns but timestamped at from: the document is dropped whole by the
    // decoder, so its frame is in no count but the drop.
    let late = doc(&msg("TH", "2026-11-20T00:00:00.000Z", body));
    let at = |seq: u64, t: i64, xml: &str| {
        let mut buf = vec![0u8; xml.len() + 64];
        let n = encode_frame(&mut buf, 1, 0, seq, t, 0, xml.as_bytes()).unwrap();
        buf[..n].to_vec()
    };
    let mut bytes = at(0, FROM - 1_000_000_000, &ok);
    bytes.extend(at(1, FROM - 1, &late));
    // A frame received at from is dropped by receive time.
    bytes.extend(at(2, FROM, &ok));
    std::fs::write(dir.join("a.oaf"), bytes).unwrap();
    let rep = replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(rep.frames, 1);
    assert_eq!(rep.sealed_window_dropped, 2);
    assert_eq!(rep.fixm.docs, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// REQ: OA-FLT-029
#[test]
fn a_callsign_is_made_upper_case_and_trimmed_and_a_position_is_kept_as_it_ages_in() {
    assert_eq!(
        oa_flight_sfdps::norm_callsign(*b"abc12 \0\0"),
        *b"ABC12\0\0\0"
    );
    assert_eq!(oa_flight_sfdps::norm_callsign(*b" n123ab "), *b"N123AB\0\0");
    let dir = temp_dir("pos");
    let ts = parse_iso_ms(b"2030-01-02T03:04:05.000Z").unwrap();
    let body = r#"<flightStatus fdpsFlightStatus="ACTIVE"/><flightIdentification aircraftIdentification="abc12" computerId="1"/><enRoute><position positionTime="2030-01-02T03:04:00Z"><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>41.9 -87.9</pos></location></position></position></enRoute>"#;
    let x = doc(&msg("TH", "2030-01-02T03:04:05.000Z", body));
    std::fs::write(dir.join("a.oaf"), frame(1, ts + 10_000, &x)).unwrap();
    let mut seen: Vec<(i64, bool)> = Vec::new();
    let g = 0x1111_1111_2222_3333_4444_5555_5555_5555u128;
    let rep = replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, p, avail| {
            seen.push((avail, p.get(g).is_some()));
        },
    )
    .unwrap();
    assert_eq!(
        seen.first(),
        Some(&(ts + 58_000, false)),
        "not there before it ages in"
    );
    assert_eq!(seen.last(), Some(&(i64::MAX, true)));
    let p = rep.positions.get(g).unwrap();
    assert_eq!((p.lat_e7, p.lon_e7), (419_000_000, -879_000_000));
    assert_eq!(p.avail_ms, ts + 58_000);
    let f = flight_by_callsign_via_table(&rep);
    assert_eq!(f, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

fn flight_by_callsign_via_table(rep: &oa_flight_sfdps::ReplayReport) -> usize {
    oa_flight::flights_flying_by_callsign(&rep.table, b"ABC12\0\0\0").count
}

// REQ: OA-FLT-031
#[test]
fn a_plan_filed_again_over_an_ended_one_is_remembered_with_its_time() {
    let dir = temp_dir("refile");
    let ts = parse_iso_ms(b"2030-01-02T03:04:05.000Z").unwrap();
    let plan = |id: &str, status: &str| {
        format!(r#"<flightStatus fdpsFlightStatus="{status}"/><flightPlan identifier="{id}"/>"#)
    };
    let mut bytes = Vec::new();
    for (k, (t, status, id)) in [
        ("03:04:05", "ACTIVE", "P1"),
        ("03:04:06", "COMPLETED", "P1"),
        ("03:04:07", "ACTIVE", "P2"),
    ]
    .into_iter()
    .enumerate()
    {
        let x = doc(&msg(
            "TH",
            &format!("2030-01-02T{t}.000Z"),
            &plan(id, status),
        ));
        bytes.extend(frame(k as u64 + 1, ts + 10_000 + k as i64 * 1_000, &x));
    }
    std::fs::write(dir.join("a.oaf"), bytes).unwrap();
    let rep = replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(
        rep.table.stats.conflicts[oa_flight::C_PLAN_REFILED as usize],
        1
    );
    let g = 0x1111_1111_2222_3333_4444_5555_5555_5555u128;
    assert_eq!(rep.positions.refiled_at(g), Some(ts + 2_000));
    assert_eq!(rep.positions.refiled_at(g + 1), None);
    let _ = std::fs::remove_dir_all(&dir);
}

// REQ: OA-FLT-040
#[test]
fn the_position_of_a_flight_the_table_removes_is_forgotten_with_it() {
    let dir = temp_dir("forget");
    let ts = parse_iso_ms(b"2030-01-02T03:04:05.000Z").unwrap();
    let other = "99999999-2222-3333-4444-555555555555";
    let pos = r#"<enRoute><position positionTime="2030-01-02T03:04:00Z"><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>41.9 -87.9</pos></location></position></position></enRoute>"#;
    let active = format!(r#"<flightStatus fdpsFlightStatus="ACTIVE"/>{pos}"#);
    let done = r#"<flightStatus fdpsFlightStatus="COMPLETED"/>"#;
    let at = |s: &str| format!("2030-01-02T03:{s}.000Z");
    let m = |g: &str, t: &str, b: &str| msg("TH", t, b).replace(GUFI, g);
    let frames = [
        frame(1, ts + 10_000, &doc(&m(GUFI, &at("04:05"), &active))),
        frame(2, ts + 11_000, &doc(&m(GUFI, &at("04:06"), done))),
        frame(3, ts + 600_000, &doc(&m(other, &at("14:05"), &active))),
        frame(4, ts + 700_000, &doc(&m(other, &at("15:45"), &active))),
    ];
    std::fs::write(dir.join("a.oaf"), frames.concat()).unwrap();
    let mut c = cfg(i64::MIN, i64::MAX);
    c.retain_ms = 60_000;
    let g = 0x1111_1111_2222_3333_4444_5555_5555_5555u128;
    let mut held_before_sweep = false;
    let rep = replay_sfdps(recordings_in(&dir).unwrap(), &c, &mut |_, p, avail| {
        if avail < ts + 300_000 && p.get(g).is_some() {
            held_before_sweep = true;
        }
    })
    .unwrap();
    assert!(
        held_before_sweep,
        "the position was held while the flight was"
    );
    assert_eq!(rep.removed_by_reason.iter().sum::<u64>(), 1);
    assert!(rep.positions.get(g).is_none());
    assert_eq!(rep.positions.refiled_at(g), None);
    assert_eq!(rep.positions.refiled_len(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

// REQ: OA-SEAL-014
#[test]
fn the_replay_decoder_drops_the_sealed_window() {
    let w = oa_flight_sfdps::decoder_limits().drop_window;
    assert_eq!(w, oa_sealed::DropWindow::sealed());
    assert_eq!(w.span(), oa_sealed::sealed_window_ms());
    assert!(w.span().is_some());
}

#[test]
fn opt_in_timing_counts_messages_but_leaves_a_status_only_second_empty() {
    let dir = temp_dir("timing-refused");
    let recv_ns = 1_000_000_000;
    let end_ns = 3_000_000_000;
    let frames = [
        frame(1, 1_000, "<MessageCollection/>"),
        frame_kind(oa_feed::Kind::Status as u8, 2, 2_000, "state=connected\n"),
        frame(3, 3_000, "<MessageCollection/>"),
    ];
    std::fs::write(dir.join("a.oaf"), frames.concat()).unwrap();
    let mut sealed = 0;
    let (rep, timing) = replay_sfdps_timed_counted(
        recordings_in(&dir).unwrap(),
        &cfg(recv_ns, end_ns),
        &mut |_, _, _| {},
        &mut sealed,
        recv_ns,
    )
    .unwrap();

    assert_eq!(rep.docs_unparsed, 0);
    assert_eq!(rep.docs_refused, 2);
    assert_eq!(rep.refused_by[0], 2);
    assert_eq!(rep.frames, 2);
    assert_eq!(timing.measure_from_ns, recv_ns);
    assert_eq!(timing.measure_to_ns, end_ns);
    assert_eq!(timing.expected_seconds, 3);
    assert_eq!(timing.observed_seconds, 2);
    assert_eq!(timing.empty_seconds, 1);
    assert_eq!(timing.timed_frames, 2);
    assert_eq!(timing.first_received_ns, Some(recv_ns));
    assert_eq!(timing.last_received_ns, Some(end_ns));
    assert_eq!(sealed, 0);
}

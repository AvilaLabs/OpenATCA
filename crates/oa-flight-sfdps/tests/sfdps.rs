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

// REQ: OA-FLT-040, OA-FLT-041
#[test]
fn future_position_time_does_not_replace_usable_evidence_before_a_valid_followup() {
    let dir = temp_dir("future-position-time");
    let position = |time: &str, coords: &str| {
        format!(
            r#"<flightStatus fdpsFlightStatus="ACTIVE"/><enRoute><position positionTime="{time}"><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>{coords}</pos></location></position></position></enRoute>"#
        )
    };
    let frames = [
        (
            1,
            "2030-01-02T03:04:05.000Z",
            "2030-01-02T03:05:05.000Z",
            position("2030-01-02T03:04:55Z", "41.9 -87.9"),
        ),
        (
            2,
            "2030-01-02T03:04:10.000Z",
            "2030-01-02T03:05:10.000Z",
            position("2030-01-02T03:05:11Z", "42.0 -88.0"),
        ),
        (
            3,
            "2030-01-02T03:05:20.000Z",
            "2030-01-02T03:05:20.000Z",
            position("2030-01-02T03:05:26Z", "42.0 -88.0"),
        ),
        (
            4,
            "2030-01-02T03:05:21.000Z",
            "2030-01-02T03:06:30.000Z",
            position("2030-01-02T03:06:15Z", "42.1 -88.1"),
        ),
    ];
    let mut bytes = Vec::new();
    for (seq, message_time, received_time, body) in frames {
        let xml = doc(&msg("TH", message_time, &body));
        let received_ms = parse_iso_ms(received_time.as_bytes()).unwrap();
        bytes.extend(frame(seq, received_ms, &xml));
    }
    std::fs::write(dir.join("a.oaf"), bytes).unwrap();

    let g = 0x1111_1111_2222_3333_4444_5555_5555_5555u128;
    // The third position has lagged publication at 03:06:18. By both of these
    // later callbacks the preceding rejected position has already been processed.
    let third_avail = parse_iso_ms(b"2030-01-02T03:06:18Z").unwrap();
    let fourth_avail = parse_iso_ms(b"2030-01-02T03:06:30Z").unwrap();
    let mut before_next_position = Vec::new();
    let rep = replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, positions, avail| {
            if avail == third_avail || avail == fourth_avail {
                before_next_position.push((avail, positions.get(g).map(|p| (p.ts_ms, p.lat_e7))));
            }
        },
    )
    .unwrap();

    assert_eq!(rep.positions.bad_time, 0, "the parent timestamps are valid");
    assert_eq!(rep.positions.bad_position_time, 2);
    let old = Some((parse_iso_ms(b"2030-01-02T03:04:55Z").unwrap(), 419_000_000));
    assert!(before_next_position.contains(&(third_avail, old)));
    assert!(before_next_position.contains(&(fourth_avail, old)));
    assert!(before_next_position
        .iter()
        .all(|&(_, position)| position == old));
    let final_position = rep.positions.get(g).unwrap();
    assert_eq!(
        final_position.ts_ms,
        parse_iso_ms(b"2030-01-02T03:06:15Z").unwrap()
    );
    assert_eq!(final_position.lat_e7, 421_000_000);
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
    assert_eq!(w, oa_sealed::sealed_drop_window());
    assert_eq!(oa_time_window::span(w), oa_sealed::sealed_window_ms());
    assert!(oa_time_window::span(w).is_some());
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

// REQ: OA-FLT-043, OA-FLT-044
#[test]
fn audited_replay_observes_handoff_semantics_without_calling_it_prediction() {
    use oa_flight::{HO_ACCEPTED, HO_NONE, HO_OFFERED, OP_HO_TAKE};
    use oa_flight_sfdps::audit::{
        observation_text, AuditLimits, AuditStats, Observation, ReplayAudit,
    };
    let dir = temp_dir("audit-handoff");
    let base = parse_iso_ms(b"2030-01-02T03:04:00Z").unwrap();
    let bodies = [
        r#"<flightStatus fdpsFlightStatus="ACTIVE"/><controllingUnit unitIdentifier="ZXX" sectorIdentifier="12"/><enRoute><position positionTime="2030-01-02T03:03:59Z"><altitude uom="FEET">38000.0</altitude><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>41.9 -87.9</pos></location></position></position></enRoute>"#,
        r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><transferringUnit unitIdentifier="ZXX" sectorIdentifier="12"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#,
        r#"<enRoute><boundaryCrossings><handoff event="ACCEPTANCE"><acceptingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#,
        r#"<enRoute><boundaryCrossings><handoff event="TAKE_CONTROL"><acceptingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#,
        r#"<flightStatus fdpsFlightStatus="ACTIVE"/><controllingUnit unitIdentifier="???" sectorIdentifier="12"/><enRoute><boundaryCrossings><handoff><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#,
        r#"<flightStatus fdpsFlightStatus="PROPOSED"/><departure departurePoint="KAAA"><runwayPositionAndTime><runwayTime><actual time="2030-01-02T03:03:00Z"/></runwayTime></runwayPositionAndTime></departure>"#,
        r#"<enRoute><boundaryCrossings><handoff event="INITIATION"/></boundaryCrossings></enRoute>"#,
        r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><transferringUnit unitIdentifier="???" sectorIdentifier="12"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="1E"/></handoff></boundaryCrossings></enRoute>"#,
    ];
    let mut bytes = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        let ts = format!("2030-01-02T03:04:0{i}Z");
        // Sequence deliberately repeats; decoded offsets, not sequence/time, identify frames.
        bytes.extend(frame(
            7,
            base + 60_000 + i as i64 * 1000,
            &doc(&msg("OH", &ts, body)),
        ));
    }
    std::fs::write(dir.join("a.oaf"), bytes).unwrap();
    let run = || {
        let mut observations = Vec::new();
        let (rep, stats) = {
            let mut sink = |record: &Observation| {
                observations.push(*record);
                Ok(())
            };
            let mut audit = ReplayAudit {
                limits: AuditLimits {
                    frames: 8,
                    rows: 8,
                    events: 64,
                },
                stats: AuditStats::default(),
                inputs: Vec::new(),
                sink: &mut sink,
            };
            let rep = oa_flight_sfdps::replay_sfdps_audited_counted(
                recordings_in(&dir).unwrap(),
                &cfg(i64::MIN, i64::MAX),
                &mut |_, _, _| {},
                &mut 0,
                &mut audit,
            )
            .unwrap();
            (rep, audit.stats)
        };
        (rep, stats, observations)
    };
    let (rep, stats, records) = run();
    let (_, again, same_records) = run();
    assert_eq!(stats, again);
    assert_eq!(records, same_records);
    let encoded: Vec<String> = records.iter().map(observation_text).collect();
    assert_eq!(encoded.len(), records.len());
    for (record, line) in records.iter().zip(&encoded) {
        assert!(line.is_ascii());
        assert!(line.ends_with('\n'));
        assert!(line.len() <= 8192);
        match record {
            Observation::Frame { .. } => assert!(line.starts_with("FRAME\t")),
            Observation::Row(_) => assert!(line.starts_with("ROW\t")),
            Observation::Operation(operation) => {
                assert!(line.starts_with("OP\t"));
                let clocks = format!(
                    "source_ms={}\treceived_ms={}",
                    operation.operation.ts_ms, operation.operation.seen_ms
                );
                assert!(line.contains(clocks.as_str()));
                assert!(line.contains("\toutcome="));
                assert!(line.contains("\tflight="));
            }
        }
    }
    assert_eq!(stats.frames_visited, 8);
    assert_eq!(stats.rows_delivered, 8);
    assert_eq!(rep.lag.rows, stats.rows_delivered);
    assert_eq!(stats.rows_missing_key_or_time, 0);
    assert_eq!(stats.rows_without_operation, 2);
    let mut applied = [0u64; oa_flight::OP_COUNT];
    let mut unchanged = [0u64; oa_flight::OP_COUNT];
    let mut refused = [[0u64; oa_flight::R_COUNT]; oa_flight::OP_COUNT];
    for record in &records {
        if let Observation::Operation(operation) = record {
            let kind = usize::from(operation.operation.kind);
            match operation.outcome {
                oa_flight::Outcome::Applied => applied[kind] += 1,
                oa_flight::Outcome::Unchanged => unchanged[kind] += 1,
                oa_flight::Outcome::Refused(reason) => refused[kind][usize::from(reason)] += 1,
            }
        }
        if let Observation::Row(row) = record {
            let observed_ops = records
                .iter()
                .filter(|candidate| {
                    matches!(candidate,
                    Observation::Operation(operation) if operation.identity == row.identity)
                })
                .count();
            assert_eq!(row.operations, observed_ops);
        }
    }
    assert_eq!(stats.operations_applied, applied);
    assert_eq!(stats.operations_unchanged, unchanged);
    assert_eq!(stats.operations_refused, refused);
    let ordinary = replay_sfdps(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(
        oa_flight::flights_digest(&rep.table),
        oa_flight::flights_digest(&ordinary.table)
    );
    assert_eq!(rep.adapter, ordinary.adapter);
    assert_eq!(rep.fixm.rows, ordinary.fixm.rows);
    let first_row = records
        .iter()
        .find_map(|record| match record {
            Observation::Row(row) if row.identity.row == 0 => Some(row),
            _ => None,
        })
        .unwrap();
    assert_eq!(first_row.source_time_ms, Some(base));
    assert_eq!(first_row.received_time_ms, base + 60_000);
    assert_eq!(first_row.centre.unwrap().get(..3), Some(b"ZXX".as_slice()));
    assert_eq!(
        first_row.reported_owner.unwrap().get(..3),
        Some(b"ZXX".as_slice())
    );
    assert_eq!(first_row.position_time_ms, Some(base - 1_000));
    assert_eq!(first_row.latitude_e7, Some(419_000_000));
    assert_eq!(first_row.longitude_e7, Some(-879_000_000));
    assert_eq!(first_row.reported_altitude_ft, Some(38_000));
    let mut stages = Vec::new();
    let mut locations = Vec::new();
    let mut row_flags = Vec::new();
    for record in records {
        match record {
            Observation::Row(row) => {
                assert_eq!(row.received_time_ms - row.source_time_ms.unwrap(), 60_000);
                locations.push(row.identity.decoded_offset);
                row_flags.push((
                    row.no_status,
                    row.owner_unknown,
                    row.handoff_incomplete,
                    row.handoff_unknown_unit,
                    row.handoff_receiving_only,
                    row.departure_not_active,
                ));
            }
            Observation::Operation(op)
                if matches!(
                    op.operation.kind,
                    OP_HO_INITIATE | OP_HO_ACCEPT | OP_HO_TAKE
                ) =>
            {
                assert_eq!(op.outcome, oa_flight::Outcome::Applied);
                let state = op.state.unwrap();
                stages.push((op.operation.kind, state.handoff_state, state.owner));
            }
            _ => {}
        }
    }
    assert!(locations.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(stages.len(), 3);
    assert_eq!((stages[0].0, stages[0].1), (OP_HO_INITIATE, HO_OFFERED));
    assert_eq!((stages[1].0, stages[1].1), (OP_HO_ACCEPT, HO_ACCEPTED));
    assert_eq!(
        stages[0].2, stages[1].2,
        "acceptance alone never transfers ownership"
    );
    assert_eq!((stages[2].0, stages[2].1), (OP_HO_TAKE, HO_NONE));
    assert_ne!(stages[1].2, stages[2].2, "take-control transfers ownership");
    assert_eq!(rep.adapter.no_status, 5);
    assert_eq!(rep.adapter.owner_unknown, 1);
    assert_eq!(rep.adapter.handoff_incomplete, 1);
    assert_eq!(rep.adapter.handoff_unknown_unit, 1);
    assert_eq!(rep.adapter.handoff_receiving_only, 1);
    assert_eq!(rep.adapter.departure_not_active, 1);
    assert_eq!(row_flags[4], (false, true, false, false, true, false));
    assert_eq!(row_flags[5], (false, false, false, false, false, true));
    assert_eq!(row_flags[6], (true, false, true, false, false, false));
    assert_eq!(row_flags[7], (true, false, false, true, false, false));
    let _ = std::fs::remove_dir_all(dir);
}

// REQ: OA-FLT-044
#[test]
fn audited_replay_stops_on_evidence_caps_and_sink_errors_and_keeps_counters() {
    use oa_flight_sfdps::audit::{AuditLimits, AuditStats, ReplayAudit};
    let dir = temp_dir("audit-caps");
    let xml = doc(&msg("TH", "2030-01-02T03:04:05Z", FULL));
    let received = parse_iso_ms(b"2030-01-02T03:05:05Z").unwrap();
    std::fs::write(
        dir.join("a.oaf"),
        [frame(1, received, &xml), frame(2, received + 1000, &xml)].concat(),
    )
    .unwrap();

    let mut sink = |_: &oa_flight_sfdps::audit::Observation| Ok(());
    let mut audit = ReplayAudit {
        limits: AuditLimits {
            frames: 4,
            rows: 1,
            events: 64,
        },
        stats: AuditStats::default(),
        inputs: Vec::new(),
        sink: &mut sink,
    };
    assert!(oa_flight_sfdps::replay_sfdps_audited_counted(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
        &mut 0,
        &mut audit
    )
    .is_err());
    assert!(audit.stats.cap_refused);
    assert_eq!(audit.stats.frames_visited, 2);
    assert_eq!(audit.stats.rows_delivered, 1);

    let mut sink = |_: &oa_flight_sfdps::audit::Observation| Ok(());
    let mut audit = ReplayAudit {
        limits: AuditLimits {
            frames: 1,
            rows: 4,
            events: 64,
        },
        stats: AuditStats::default(),
        inputs: Vec::new(),
        sink: &mut sink,
    };
    assert!(oa_flight_sfdps::replay_sfdps_audited_counted(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
        &mut 0,
        &mut audit
    )
    .is_err());
    assert!(audit.stats.cap_refused);
    assert_eq!(audit.stats.frames_visited, 1);
    assert_eq!(audit.stats.rows_delivered, 1);

    let mut sink = |_: &oa_flight_sfdps::audit::Observation| Ok(());
    let mut audit = ReplayAudit {
        limits: AuditLimits {
            frames: 1,
            rows: 1,
            events: 1,
        },
        stats: AuditStats::default(),
        inputs: Vec::new(),
        sink: &mut sink,
    };
    assert!(oa_flight_sfdps::replay_sfdps_audited_counted(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
        &mut 0,
        &mut audit
    )
    .is_err());
    assert!(audit.stats.cap_refused);
    assert_eq!(audit.stats.events_delivered, 1);
    assert_eq!(audit.stats.rows_delivered, 0);
    let mut sink = |_: &oa_flight_sfdps::audit::Observation| {
        Err(std::io::Error::other("injected writer failure"))
    };
    let mut audit = ReplayAudit {
        limits: AuditLimits {
            frames: 2,
            rows: 2,
            events: 32,
        },
        stats: AuditStats::default(),
        inputs: Vec::new(),
        sink: &mut sink,
    };
    assert!(oa_flight_sfdps::replay_sfdps_audited_counted(
        recordings_in(&dir).unwrap(),
        &cfg(i64::MIN, i64::MAX),
        &mut |_, _, _| {},
        &mut 0,
        &mut audit
    )
    .is_err());
    assert!(audit.stats.sink_failed);
    assert_eq!(audit.stats.frames_visited, 1);
    assert_eq!(audit.stats.events_delivered, 0);
    let _ = std::fs::remove_dir_all(dir);
}

// REQ: OA-FLT-044
#[test]
fn shared_audit_replay_entry_refuses_backwards_window_before_inputs_or_sink() {
    use oa_flight_sfdps::audit::{AuditLimits, AuditStats, Observation, ReplayAudit};
    let mut emitted = 0usize;
    let mut sink = |_: &Observation| {
        emitted += 1;
        Ok(())
    };
    let mut audit = ReplayAudit {
        limits: AuditLimits {
            frames: 1,
            rows: 1,
            events: 2,
        },
        stats: AuditStats::default(),
        inputs: Vec::new(),
        sink: &mut sink,
    };
    let result = oa_flight_sfdps::replay_sfdps_audited_counted(
        Vec::new(),
        &cfg(20, 10),
        &mut |_, _, _| {},
        &mut 0,
        &mut audit,
    );
    assert!(result
        .as_ref()
        .err()
        .is_some_and(|error| error.to_string().contains("from_ns exceeds to_ns")));
    assert_eq!(audit.stats, AuditStats::default());
    assert!(audit.inputs.is_empty());
    assert_eq!(emitted, 0);
}

// REQ: OA-FLT-043, OA-FLT-044
#[test]
fn audit_record_encoding_is_explicit_and_preserves_optional_state() {
    use oa_flight_sfdps::audit::{observation_text, Observation, RowIdentity, RowObservation};
    let event = Observation::Row(RowObservation {
        identity: RowIdentity {
            stream: 1,
            decoded_offset: 7,
            recorder_sequence: 3,
            row: 2,
        },
        source_time_ms: None,
        received_time_ms: 42,
        gufi: Some(1),
        present: Some(0),
        field_errors: None,
        operations: 0,
        centre: None,
        reported_owner: None,
        position_time_ms: None,
        latitude_e7: None,
        longitude_e7: None,
        reported_altitude_ft: None,
        no_status: false,
        owner_unknown: false,
        handoff_incomplete: false,
        handoff_unknown_unit: false,
        handoff_receiving_only: false,
        departure_not_active: false,
    });
    assert_eq!(observation_text(&event),"ROW\tstream=1\toffset=7\tseq=3\trow=2\tsource_ms=none\treceived_ms=42\tgufi=00000000000000000000000000000001\tpresent=0\terrors=none\tops=0\tcentre=none\treported_owner=none\tposition_ms=none\tlat_e7=none\tlon_e7=none\treported_alt_ft=none\tno_status=0\towner_unknown=0\thandoff_incomplete=0\thandoff_unknown_unit=0\thandoff_receiving_only=0\tdeparture_not_active=0\n");
}

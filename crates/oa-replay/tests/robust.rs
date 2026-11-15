//! Tests of the replay clock's step rules, the ADS-B table's age eviction and time check, the
//! SFDPS state-dependent eviction ages and the decoding of documents over the row limits. Every
//! identifier, address and time is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_adsb::{encode_payload, reports_len, reports_push, Reports, Row, ROW_EMPTY};
use oa_feed::Kind;
use oa_replay::{
    adsb_slot_of, is_stale, pipeline_stale_count, process_frame, replay_hash, slot_of_gufi,
    ErrKind, FeedKind, Pipeline,
};
use oa_sha256::to_hex;

const T0: i64 = DAY * 1_000_000 + 3_500_000_000;
const MIN: i64 = 60 * NS;
const HOUR: i64 = 60 * MIN;

/// A frame that carries no feed data: it moves the replay clock and nothing else.
fn tick(p: &mut Pipeline, t: i64) {
    process_frame(p, FeedKind::Sfdps, Kind::Status, t, &[]);
}

fn flight_at(p: &mut Pipeline, n: u32, t: i64, status: Option<&str>) {
    let mut m = sf(n, (t - DAY * 1_000_000) / 1_000_000).call("TST1");
    if let Some(s) = status {
        m = m.status(s);
    }
    feed(p, FeedKind::Sfdps, t, &sf_doc(&[m]));
}

fn alive(p: &Pipeline, n: u32) -> bool {
    slot_of_gufi(&p.table, gufi(n)).is_some()
}

fn adsb_row(icao: u32, t_ns: i64) -> Row {
    Row {
        icao,
        time_ns: t_ns,
        lat_e7: 420_000_000,
        lon_e7: -880_000_000,
        baro_alt_ft: 35_000,
        gs_dkt: 4500,
        track_ddeg: 900,
        ..ROW_EMPTY
    }
}

fn adsb_frame(p: &mut Pipeline, t: i64, rows: &[Row]) {
    let mut r = Reports::default();
    for x in rows {
        reports_push(&mut r, x);
    }
    let mut out = Vec::new();
    encode_payload(&mut out, &r, 0, reports_len(&r)).unwrap();
    process_frame(p, FeedKind::Adsb, Kind::Message, t, &out);
}

// REQ: OA-RPL-061
#[test]
fn one_frame_far_in_the_future_does_not_move_the_replay_clock_or_evict_anything() {
    let mut p = small_pipeline(64);
    for k in 0..10 {
        tick(&mut p, T0 + k * NS);
    }
    flight_at(&mut p, 1, T0 + 9 * NS, None);
    tick(&mut p, T0 + 400 * 24 * HOUR);
    assert!(
        p.replay_ns < T0 + HOUR,
        "the clock moved to {}",
        p.replay_ns - T0
    );
    for k in 10..20 {
        tick(&mut p, T0 + k * NS);
    }
    assert_eq!(p.replay_ns, T0 + 19 * NS);
    assert!(alive(&p, 1));
}

// REQ: OA-RPL-062
#[test]
fn a_confirmed_forward_step_of_the_recorder_clock_keeps_the_flights_and_time_runs_on() {
    let mut p = small_pipeline(64);
    for k in 0..10 {
        tick(&mut p, T0 + k * NS);
    }
    flight_at(&mut p, 1, T0 + 9 * NS, None);
    let step = 2 * HOUR;
    for k in 10..30 {
        tick(&mut p, T0 + step + k * NS);
    }
    assert_eq!(p.replay_ns, T0 + step + 29 * NS);
    assert!(alive(&p, 1), "the step evicted the flight");
    // Time is not frozen: 35 minutes of silence after the step evicts it by age.
    for k in 1..=35 {
        tick(&mut p, T0 + step + 29 * NS + k * MIN);
    }
    assert!(!alive(&p, 1));
}

// REQ: OA-RPL-063
#[test]
fn a_confirmed_backward_step_of_the_recorder_clock_keeps_the_flights_and_time_runs_on() {
    let mut p = small_pipeline(64);
    for k in 0..10 {
        tick(&mut p, T0 + k * NS);
    }
    flight_at(&mut p, 1, T0 + 9 * NS, None);
    let base = T0 - 3 * HOUR;
    for k in 0..20 {
        tick(&mut p, base + k * NS);
    }
    assert_eq!(p.replay_ns, base + 19 * NS);
    assert!(alive(&p, 1));
    for k in 1..=35 {
        tick(&mut p, base + 19 * NS + k * MIN);
    }
    assert!(!alive(&p, 1));
}

/// An ordinary replay: SFDPS, TBFM and ADS-B frames, small time regressions, a quiet spell.
fn ordinary(p: &mut Pipeline) {
    for k in 0..400i64 {
        let t = T0 + k * 1_500_000_000;
        let n = (k % 40) as u32 + 1;
        let m = sf(n, k * 1500)
            .call(&format!("TST{n}"))
            .plan(&format!("KA{n:06}AB"))
            .pos(30.0 + f64::from(n) / 10.0, -80.0, k * 1500);
        feed(p, FeedKind::Sfdps, t, &sf_doc(&[m]));
        let a = tb(k * 1500, Some(&format!("KA{n:06}AB")), &format!("T{n:05}"))
            .mrp("MFXA1", "09L", "NEW")
            .eta("MFXA1", 600_000 + k * 1500);
        feed(p, FeedKind::Tbfm, t + 1, &tb_doc(k * 1500, "ZTT", &[a]));
        if k % 4 == 0 {
            let rows: Vec<Row> = (1..=8u32).map(|a| adsb_row(0xA0_0000 + a, t)).collect();
            adsb_frame(p, t + 2, &rows);
        }
        if k % 50 == 7 {
            // A frame a little earlier than one already seen.
            tick(p, t - 3 * NS);
        }
    }
    // A hundred quiet seconds: inside the tolerance.
    tick(p, T0 + 400 * 1_500_000_000 + 100 * NS);
}

// REQ: OA-RPL-064
#[test]
fn an_ordinary_replay_hashes_as_it_always_did() {
    let mut p = small_pipeline(256);
    ordinary(&mut p);
    assert_eq!(
        p.errors[ErrKind::TimeRegression as usize],
        8,
        "regressions counted as before"
    );
    assert_eq!(
        to_hex(&replay_hash(&p)),
        // Equal to what the code before the 2-minute tolerance gave for the same frames; the pin
        // moved only because the replay now ends on a 100 s quiet spell, not a 9 minute one.
        "39cdf512d34a34fea093eb27c040b4bc6e7a3f992534f01b623dae345f0fd0a5"
    );
}

// REQ: OA-RPL-065
#[test]
fn the_aircraft_table_frees_slots_of_aircraft_not_heard_for_half_an_hour() {
    let mut p = small_pipeline(8);
    p.adsb = oa_replay::new_adsb_table(64);
    // One new aircraft a minute for three hours: 180 addresses through a table of 64.
    for k in 0..180i64 {
        let t = T0 + k * MIN;
        adsb_frame(&mut p, t, &[adsb_row(0xB0_0000 + k as u32, t)]);
    }
    let f = FeedKind::Adsb as usize;
    assert_eq!(p.rows_refused_full[f], 0);
    assert_eq!(p.rows_applied[f], 180);
    assert!(p.adsb.live <= 33, "{} aircraft held", p.adsb.live);
    assert!(adsb_slot_of(&p.adsb, 0xB0_0000).is_none());
    assert!(adsb_slot_of(&p.adsb, 0xB0_0000 + 179).is_some());
}

// REQ: OA-RPL-066
#[test]
fn a_report_stamped_far_ahead_of_the_clock_is_refused_and_cannot_block_the_real_ones() {
    let mut p = small_pipeline(8);
    for k in 0..5 {
        tick(&mut p, T0 + k * NS);
    }
    let bad = T0 + 400 * 24 * HOUR;
    adsb_frame(&mut p, T0 + 5 * NS, &[adsb_row(0xC0_0001, bad)]);
    assert!(adsb_slot_of(&p.adsb, 0xC0_0001).is_none());
    adsb_frame(&mut p, T0 + 6 * NS, &[adsb_row(0xC0_0001, T0 + 6 * NS)]);
    let s = adsb_slot_of(&p.adsb, 0xC0_0001).unwrap();
    assert_eq!(p.adsb.time_ns[s], T0 + 6 * NS);
    assert_eq!(p.adsb.out_of_order[s], 0);
}

// REQ: OA-RPL-067
#[test]
fn a_flight_is_kept_as_long_as_its_state_says_it_may_be_silent() {
    let mut p = small_pipeline(64);
    flight_at(&mut p, 1, T0, Some("ACTIVE"));
    flight_at(&mut p, 2, T0, Some("PROPOSED"));
    flight_at(&mut p, 3, T0, Some("COMPLETED"));
    flight_at(&mut p, 4, T0, Some("DROPPED"));
    flight_at(&mut p, 5, T0, None);
    // Replay on with frames that carry no feed data, a minute apart.
    let mut k = 1;
    let mut run_to = |p: &mut Pipeline, mins: i64| {
        while k <= mins {
            tick(p, T0 + k * MIN);
            k += 1;
        }
    };
    run_to(&mut p, 12);
    assert!(
        alive(&p, 1) && alive(&p, 2) && alive(&p, 5) && !alive(&p, 3) && !alive(&p, 4),
        "after 12 min only the finished flights are gone"
    );
    run_to(&mut p, 31);
    assert!(
        alive(&p, 1) && alive(&p, 2) && !alive(&p, 5),
        "after 31 min"
    );
    run_to(&mut p, 125);
    assert!(!alive(&p, 1) && alive(&p, 2), "after 125 min");
    run_to(&mut p, 245);
    assert!(!alive(&p, 2), "after 245 min");
}

/// An SFDPS document of `n` messages, each with a route of `route_len` bytes.
fn big_doc(n: u32, route_len: usize) -> String {
    let route = "A".repeat(route_len);
    let mut body = String::new();
    for i in 0..n {
        body += &format!(
            r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="{}"><gufi codeSpace="urn:uuid">{}</gufi><flightIdentification xsi:type="ns5:NasFlightIdentificationType" aircraftIdentification="TST{}"/><agreed><route xsi:type="ns5:NasRouteType" nasRouteText="{route}"/></agreed></flight></message>"#,
            iso(1000),
            uuid(i + 1),
            i % 100
        );
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">{body}</ns5:MessageCollection>"#
    )
}

// REQ: OA-RPL-068
#[test]
fn an_sfdps_document_over_the_row_limit_loses_only_the_messages_past_it() {
    let mut p = small_pipeline(8192);
    let rows = oa_replay::pipeline::DOC_ROW_CAPACITY as u32;
    feed(&mut p, FeedKind::Sfdps, T0, &big_doc(rows + 10, 0));
    let f = FeedKind::Sfdps as usize;
    assert_eq!(p.rows_applied[f], u64::from(rows));
    assert!(alive(&p, 1) && alive(&p, rows) && !alive(&p, rows + 1));
    assert_eq!(p.errors[ErrKind::SfdpsRowCapacity as usize], 10);
}

// REQ: OA-RPL-069
#[test]
fn an_sfdps_document_over_the_route_text_limit_loses_only_the_messages_past_it() {
    let mut p = small_pipeline(8192);
    // 1 MiB of route text holds 512 routes of 2,000 bytes.
    feed(&mut p, FeedKind::Sfdps, T0, &big_doc(600, 2000));
    let f = FeedKind::Sfdps as usize;
    assert_eq!(p.rows_applied[f], 524);
    assert_eq!(p.errors[ErrKind::SfdpsStringCapacity as usize], 600 - 524);
}

// REQ: OA-RPL-070
#[test]
fn a_tbfm_document_over_the_row_limit_loses_only_the_rows_past_it() {
    let mut p = small_pipeline(8192);
    let rows = oa_replay::pipeline::DOC_ROW_CAPACITY;
    let airs: Vec<Tb> = (0..rows + 10)
        .map(|i| tb(1000, Some(&format!("KA{i:06}AB")), &format!("T{i:05}")).flt("B738"))
        .collect();
    feed(&mut p, FeedKind::Tbfm, T0, &tb_doc(1000, "ZTT", &airs));
    let f = FeedKind::Tbfm as usize;
    assert_eq!(p.rows_applied[f], rows as u64);
    assert_eq!(p.errors[ErrKind::TbfmCapacity as usize], 10);
}

// REQ: OA-RPL-071
#[test]
fn scattered_wild_frames_are_held_and_counted_and_a_step_needs_three_in_a_row() {
    let mut p = small_pipeline(64);
    for k in 0..40i64 {
        tick(&mut p, T0 + k * NS);
        // Every fifth frame is followed by a wild one, alternately a day ahead and a day behind.
        if k % 5 == 4 {
            let off = if k % 10 == 4 { 24 * HOUR } else { -24 * HOUR };
            tick(&mut p, T0 + k * NS + off);
        }
    }
    assert_eq!(p.replay_ns, T0 + 39 * NS);
    assert_eq!(p.clock.held, 8);
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 0));
    // Two wild frames that agree are not enough, a third that is earlier restarts the count, and
    // so does one in the other direction.
    let far = T0 + 30 * 24 * HOUR;
    for t in [far, far + NS, far + 3 * HOUR, far + 3 * HOUR + 2 * NS] {
        tick(&mut p, t);
        assert_eq!(p.replay_ns, T0 + 39 * NS);
    }
    tick(&mut p, T0 - 30 * 24 * HOUR);
    assert_eq!(p.replay_ns, T0 + 39 * NS);
    assert_eq!(p.clock.held, 8 + 5);
    // Three in a row, non-decreasing: the step is confirmed on the third and that frame counts at
    // the new time.
    let base = T0 + 40 * 24 * HOUR;
    tick(&mut p, base);
    tick(&mut p, base + NS);
    assert_eq!(p.replay_ns, T0 + 39 * NS);
    tick(&mut p, base + NS);
    assert_eq!(p.replay_ns, base + NS);
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (1, 0));
    // A frame inside the tolerance after a step is ordinary again.
    tick(&mut p, base + 2 * NS);
    assert_eq!(p.replay_ns, base + 2 * NS);
}

// REQ: OA-RPL-071
#[test]
fn a_step_back_is_counted_and_the_regressions_before_it_stay_counted() {
    let mut p = small_pipeline(64);
    tick(&mut p, T0);
    for k in 1..=3 {
        tick(&mut p, T0 - 2 * HOUR + k * NS);
    }
    assert_eq!(p.replay_ns, T0 - 2 * HOUR + 3 * NS);
    assert_eq!(p.clock.backward_steps, 1);
    // The two held frames were behind the clock, so they are regressions as before.
    assert_eq!(p.errors[ErrKind::TimeRegression as usize], 2);
}

// REQ: OA-RPL-150
#[test]
fn a_step_is_confirmed_by_candidates_that_jitter_and_not_by_candidates_far_apart() {
    // A step back whose three frames arrive out of order by up to a second (two decoder
    // threads): confirmed on the third.
    let mut p = small_pipeline(64);
    tick(&mut p, T0);
    let base = T0 - 2 * HOUR;
    tick(&mut p, base + NS);
    tick(&mut p, base);
    assert_eq!(p.replay_ns, T0);
    tick(&mut p, base + NS / 2);
    assert_eq!(p.clock.backward_steps, 1);
    assert_eq!(p.replay_ns, base + NS / 2);
    // Candidates a day apart each start a new run: none confirms.
    let mut p = small_pipeline(64);
    tick(&mut p, T0);
    for k in 1..=3 {
        tick(&mut p, T0 + k * 24 * HOUR);
        assert_eq!(p.replay_ns, T0);
    }
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 0));
    assert_eq!(p.clock.held, 3);
}

/// A frame that carries no feed data, from the feed `feed`.
fn tick_from(p: &mut Pipeline, feed: FeedKind, t: i64) {
    process_frame(p, feed, Kind::Status, t, &[]);
}

// REQ: OA-RPL-078
#[test]
fn a_frame_minutes_ahead_does_not_move_the_clock_without_confirmation() {
    let mut p = small_pipeline(64);
    for k in 0..10 {
        tick(&mut p, T0 + k * NS);
    }
    // 9.9 minutes ahead, as the 10-minute rule used to let through.
    tick(&mut p, T0 + 9 * NS + 594 * NS);
    assert_eq!(p.replay_ns, T0 + 9 * NS);
    assert_eq!(p.clock.held, 1);
    // Repeated every few frames, with honest frames between: the clock follows the honest ones,
    // and they are never counted as regressions.
    for k in 10..60 {
        tick(&mut p, T0 + k * NS);
        if k % 5 == 0 {
            tick(&mut p, T0 + k * NS + 594 * NS);
        }
        assert_eq!(p.replay_ns, T0 + k * NS);
    }
    assert_eq!(p.errors[ErrKind::TimeRegression as usize], 0);
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 0));
    // A frame inside the tolerance (two minutes) is ordinary.
    tick(&mut p, T0 + 59 * NS + 100 * NS);
    assert_eq!(p.replay_ns, T0 + 159 * NS);
}

// REQ: OA-RPL-081
#[test]
fn a_frame_time_equal_to_the_no_clock_value_is_refused() {
    // i64::MIN is the clock's "no frame yet" value. Three frames of one feed stamped with it
    // used to confirm a step back to it, after which the next frame of any time was taken as a
    // first frame, with no confirmation.
    let mut p = small_pipeline(64);
    for k in 0..10 {
        tick(&mut p, T0 + k * NS);
    }
    for _ in 0..5 {
        tick(&mut p, i64::MIN);
    }
    assert_eq!(p.replay_ns, T0 + 9 * NS);
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 0));
    assert_eq!(p.clock.held, 5);
    // A frame a day ahead is still held, not taken as a first frame.
    tick(&mut p, T0 + 86_400 * NS);
    assert_eq!(p.replay_ns, T0 + 9 * NS);
    // Before the first frame too: the value never becomes the clock.
    let mut q = small_pipeline(64);
    tick(&mut q, i64::MIN);
    assert_eq!(q.clock.held, 1);
    tick(&mut q, T0);
    assert_eq!(q.replay_ns, T0);
}

// REQ: OA-RPL-079
#[test]
fn feeds_with_a_constant_skew_under_the_tolerance_do_not_regress_each_other() {
    let mut p = small_pipeline(64);
    // The feeds' receive times are 100 s apart all the way through, interleaved.
    for k in 0..60 {
        tick_from(&mut p, FeedKind::Sfdps, T0 + k * NS);
        tick_from(&mut p, FeedKind::Tbfm, T0 + 100 * NS + k * NS);
    }
    assert_eq!(p.errors[ErrKind::TimeRegression as usize], 0);
    assert_eq!(p.clock.held, 0);
    // The clock is the latest time any feed has had.
    assert_eq!(p.replay_ns, T0 + 159 * NS);
    // A frame of the slow feed that goes back in its own time is still a regression.
    tick_from(&mut p, FeedKind::Sfdps, T0 + 10 * NS);
    assert_eq!(p.errors[ErrKind::TimeRegression as usize], 1);
    // A feed that is five minutes behind the others from its first frame is not the clock's to
    // follow: its frames are held, and the clock stays with the others.
    for k in 60..70 {
        tick_from(&mut p, FeedKind::Sfdps, T0 + k * NS);
        tick_from(&mut p, FeedKind::Tbfm, T0 + 100 * NS + k * NS);
        tick_from(&mut p, FeedKind::Adsb, T0 + k * NS - 300 * NS);
    }
    assert_eq!(p.replay_ns, T0 + 100 * NS + 69 * NS);
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 0));
}

// REQ: OA-RPL-080
#[test]
fn flights_last_heard_before_a_confirmed_step_are_stale_until_they_are_heard_again() {
    let mut p = small_pipeline(64);
    for k in 0..10 {
        tick(&mut p, T0 + k * NS);
    }
    flight_at(&mut p, 1, T0 + 9 * NS, None);
    flight_at(&mut p, 2, T0 + 9 * NS, None);
    assert_eq!((pipeline_stale_count(&p), p.stale_marked), (0, 0));
    // Not stale through a step that is only a candidate.
    tick(&mut p, T0 + 2 * HOUR);
    assert_eq!(pipeline_stale_count(&p), 0);
    let step = 2 * HOUR;
    for k in 10..30 {
        tick(&mut p, T0 + step + k * NS);
    }
    assert_eq!(p.clock.forward_steps, 1);
    // Both flights survived the step with their ages shifted, and both are marked.
    assert!(alive(&p, 1) && alive(&p, 2));
    let stale = |p: &Pipeline, n| is_stale(&p.table, slot_of_gufi(&p.table, gufi(n)).unwrap());
    assert!(stale(&p, 1) && stale(&p, 2));
    assert_eq!((pipeline_stale_count(&p), p.stale_marked), (2, 2));
    // A fresh update of one clears its mark and only its mark.
    flight_at(&mut p, 1, T0 + step + 30 * NS, None);
    assert!(!stale(&p, 1) && stale(&p, 2));
    assert_eq!((pipeline_stale_count(&p), p.stale_marked), (1, 2));
    // A flight first heard after the step is never stale; an evicted one is no longer counted.
    flight_at(&mut p, 3, T0 + step + 31 * NS, None);
    assert_eq!(pipeline_stale_count(&p), 1);
    for k in 1..=35 {
        tick(&mut p, T0 + step + 31 * NS + k * MIN);
    }
    assert!(!alive(&p, 2));
    assert_eq!(pipeline_stale_count(&p), 0);
}

/// The ADS-B table after the same reports in two different orders and slot histories.
fn aircraft_hash(order: &[u32]) -> String {
    let mut p = small_pipeline(8);
    p.adsb = oa_replay::new_adsb_table(16);
    // An aircraft that is evicted before the others arrive, so slot 0 is free for the rest.
    adsb_frame(&mut p, T0, &[adsb_row(0xD0_0000, T0)]);
    for &n in order {
        let t = T0 + HOUR + i64::from(n) * NS;
        adsb_frame(&mut p, t, &[adsb_row(0xD1_0000 + n, t)]);
    }
    to_hex(&replay_hash(&p))
}

// REQ: OA-RPL-072
#[test]
fn aircraft_eviction_and_future_refusals_are_counted_and_hashed_deterministically() {
    let mut p = small_pipeline(8);
    p.adsb = oa_replay::new_adsb_table(16);
    let before = {
        adsb_frame(&mut p, T0, &[adsb_row(1, T0), adsb_row(2, T0)]);
        to_hex(&replay_hash(&p))
    };
    // One aircraft is heard again, the other is not; 50 minutes on, the silent one is freed.
    for k in 1..=50 {
        let t = T0 + k * MIN;
        if k % 5 == 0 {
            adsb_frame(&mut p, t, &[adsb_row(2, t)]);
        } else {
            tick(&mut p, t);
        }
    }
    for k in 51..=55 {
        tick(&mut p, T0 + k * MIN);
    }
    assert_eq!(p.adsb.stats.evicted, 1);
    assert_eq!((p.adsb.live, p.adsb.free.len()), (1, 15));
    assert!(adsb_slot_of(&p.adsb, 1).is_none() && adsb_slot_of(&p.adsb, 2).is_some());
    // A report 3 minutes ahead of the clock is refused and counted; 1 minute ahead is accepted.
    adsb_frame(&mut p, T0 + 56 * MIN, &[adsb_row(3, T0 + 59 * MIN)]);
    adsb_frame(&mut p, T0 + 56 * MIN, &[adsb_row(4, T0 + 57 * MIN)]);
    assert_eq!(p.adsb.stats.future_refused, 1);
    assert!(adsb_slot_of(&p.adsb, 3).is_none() && adsb_slot_of(&p.adsb, 4).is_some());
    let f = FeedKind::Adsb as usize;
    assert_eq!((p.rows_seen[f], p.rows_applied[f]), (14, 13));
    // The counts are part of the hash from then on; the tag changed, so it is not the old one.
    assert_ne!(to_hex(&replay_hash(&p)), before);
    // The same history gives the same hash, and the hash does not depend on the slot an aircraft
    // landed in: two orders of arrival, one aircraft evicted first, give one hash.
    assert_eq!(aircraft_hash(&[3, 1, 2]), aircraft_hash(&[3, 1, 2]));
    assert_ne!(aircraft_hash(&[3, 1, 2]), aircraft_hash(&[1, 2]));
}

// REQ: OA-RPL-074
#[test]
fn the_state_ages_are_settings_that_must_be_positive_and_have_command_line_flags() {
    use oa_replay::{new_table, ConfigError, TableConfig};
    let ok = TableConfig::default();
    assert_eq!(ok.active_age_ns, 2 * HOUR);
    assert_eq!(ok.proposed_age_ns, 4 * HOUR);
    assert_eq!(ok.terminal_age_ns, 10 * MIN);
    for bad in [
        TableConfig {
            active_age_ns: 0,
            ..ok
        },
        TableConfig {
            proposed_age_ns: -1,
            ..ok
        },
        TableConfig {
            terminal_age_ns: 0,
            ..ok
        },
    ] {
        assert!(matches!(
            new_table(bad).err(),
            Some(ConfigError::StateAge(_))
        ));
    }
    let args: Vec<String> = [
        "--sfdps",
        "x",
        "--active-age-secs",
        "60",
        "--proposed-age-secs",
        "120",
        "--terminal-age-secs",
        "5",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let c = oa_replay::cli::parse_args(&args).unwrap();
    assert_eq!(c.cfg.table.active_age_ns, 60 * NS);
    assert_eq!(c.cfg.table.proposed_age_ns, 120 * NS);
    assert_eq!(c.cfg.table.terminal_age_ns, 5 * NS);
}

// REQ: OA-RPL-075
#[test]
fn a_flight_whose_status_changes_takes_the_age_of_its_new_state() {
    let mut p = small_pipeline(16);
    flight_at(&mut p, 1, T0, Some("PROPOSED"));
    flight_at(&mut p, 2, T0, Some("PROPOSED"));
    // Flight 1 goes active, flight 2 stays proposed; a later message with no status leaves the
    // state alone.
    flight_at(&mut p, 1, T0 + MIN, Some("ACTIVE"));
    flight_at(&mut p, 1, T0 + 2 * MIN, None);
    for k in 3..=(3 * 60) {
        tick(&mut p, T0 + k * MIN);
    }
    // 3 hours on: flight 1 (active, silent 178 minutes) is gone, flight 2 (proposed) is kept.
    assert!(!alive(&p, 1) && alive(&p, 2));
    // Completing a flight shortens it.
    flight_at(&mut p, 2, T0 + 181 * MIN, Some("COMPLETED"));
    for k in 182..=195 {
        tick(&mut p, T0 + k * MIN);
    }
    assert!(!alive(&p, 2));
}

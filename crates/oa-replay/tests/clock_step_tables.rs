//! Tests of what a confirmed step of the replay clock does to the ADS-B table, of what counts as
//! hearing a stale row again, and of a step whose size does not fit. Every identifier, address and
//! time is made up.
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
    adsb_slot_of, adsb_stale_count, is_stale, pipeline_stale_count, process_frame, replay_hash,
    slot_of_gufi, AdsbApplied, ErrKind, FeedKind, Pipeline,
};

const T0: i64 = DAY * 1_000_000 + 3_500_000_000;
const MIN: i64 = 60 * NS;
const HOUR: i64 = 60 * MIN;

fn tick(p: &mut Pipeline, t: i64) {
    process_frame(p, FeedKind::Sfdps, Kind::Status, t, &[]);
}

fn ms(t: i64) -> i64 {
    (t - DAY * 1_000_000) / 1_000_000
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

/// A pipeline that has seen ten seconds of clock.
fn running() -> Pipeline {
    let mut p = small_pipeline(64);
    for k in 0..10 {
        tick(&mut p, T0 + k * NS);
    }
    p
}

/// Three frames in a row at `to + k` seconds: a confirmed step of the clock.
fn step_to(p: &mut Pipeline, to: i64) {
    for k in 0..3 {
        tick(p, to + k * NS);
    }
}

fn flight_stale(p: &Pipeline, n: u32) -> bool {
    is_stale(&p.table, slot_of_gufi(&p.table, gufi(n)).unwrap())
}

// REQ: OA-RPL-170
#[test]
fn a_forward_step_keeps_the_aircraft_that_were_live_and_marks_them_stale() {
    let mut p = running();
    adsb_frame(
        &mut p,
        T0 + 9 * NS,
        &[adsb_row(1, T0 + 9 * NS), adsb_row(2, T0 + 9 * NS)],
    );
    assert_eq!((p.adsb.live, adsb_stale_count(&p.adsb)), (2, 0));
    step_to(&mut p, T0 + HOUR);
    assert_eq!(p.clock.forward_steps, 1);
    assert_eq!(p.adsb.live, 2, "the step wiped the ADS-B table");
    assert_eq!((adsb_stale_count(&p.adsb), p.stale_marked), (2, 2));
    let s = adsb_slot_of(&p.adsb, 1).unwrap();
    // The step is pending: no report time moves until a report says which clock it follows.
    assert_eq!(p.adsb.time_ns[s], T0 + 9 * NS);
    assert_eq!(p.adsb.pending_ns, T0 + HOUR + 2 * NS - (T0 + 9 * NS));
}

// REQ: OA-RPL-170
#[test]
fn a_backward_step_accepts_the_next_genuine_aircraft_reports() {
    let mut p = running();
    adsb_frame(&mut p, T0 + 9 * NS, &[adsb_row(1, T0 + 9 * NS)]);
    let back = T0 - 10 * MIN;
    step_to(&mut p, back);
    assert_eq!(p.clock.backward_steps, 1);
    assert_eq!(p.adsb.live, 1);
    let t = back + 20 * NS;
    adsb_frame(&mut p, t, &[adsb_row(1, t), adsb_row(3, t)]);
    assert_eq!(p.adsb.stats.future_refused, 0);
    assert_eq!(p.adsb.stats.out_of_order, 0);
    assert_eq!((p.adsb.stats.applied, p.adsb.live), (3, 2));
    // The aircraft heard again is no longer stale.
    assert_eq!(adsb_stale_count(&p.adsb), 0);
    let _ = AdsbApplied::New;
}

// REQ: OA-RPL-171
#[test]
fn a_refused_older_aircraft_report_leaves_the_row_stale_and_an_accepted_one_clears_it() {
    let mut p = running();
    adsb_frame(&mut p, T0 + 9 * NS, &[adsb_row(1, T0 + 9 * NS)]);
    let after = T0 + HOUR;
    step_to(&mut p, after);
    assert_eq!(adsb_stale_count(&p.adsb), 1);
    // A report from before the one held (the held time moved with the step).
    let old = after - 10 * MIN;
    adsb_frame(&mut p, after + 30 * NS, &[adsb_row(1, old)]);
    assert_eq!(p.adsb.stats.out_of_order, 1);
    assert_eq!(adsb_stale_count(&p.adsb), 1);
    adsb_frame(&mut p, after + 40 * NS, &[adsb_row(1, after + 40 * NS)]);
    assert_eq!(adsb_stale_count(&p.adsb), 0);
}

// REQ: OA-RPL-171
#[test]
fn a_refused_older_position_leaves_the_flight_stale_and_an_accepted_one_clears_it() {
    let mut p = running();
    let t = T0 + 9 * NS;
    feed(
        &mut p,
        FeedKind::Sfdps,
        t,
        &sf_doc(&[sf(1, ms(t)).call("TST1").pos(41.0, -88.0, ms(t))]),
    );
    let after = T0 + 2 * HOUR;
    step_to(&mut p, after);
    assert!(flight_stale(&p, 1));
    // Position older than the one held: refused, the flight is not heard again.
    let t1 = after + 30 * NS;
    feed(
        &mut p,
        FeedKind::Sfdps,
        t1,
        &sf_doc(&[sf(1, ms(t1)).pos(41.1, -88.1, ms(t) - 5000)]),
    );
    assert_eq!(p.table.stats.out_of_order, 1);
    assert!(flight_stale(&p, 1));
    // A message with no position or state is not hearing the flight either.
    let t2 = after + 35 * NS;
    feed(
        &mut p,
        FeedKind::Sfdps,
        t2,
        &sf_doc(&[sf(1, ms(t2)).arr("KORD")]),
    );
    assert!(flight_stale(&p, 1));
    // An accepted position clears it.
    let t3 = after + 40 * NS;
    feed(
        &mut p,
        FeedKind::Sfdps,
        t3,
        &sf_doc(&[sf(1, ms(t3)).pos(41.2, -88.2, ms(t3))]),
    );
    assert!(!flight_stale(&p, 1));
}

// REQ: OA-RPL-172
#[test]
fn a_tbfm_row_clears_the_stale_mark_only_when_it_carries_metering_state() {
    let mut p = running();
    let t = T0 + 9 * NS;
    let doc = |t: i64, row: Tb| tb_doc(ms(t), "ZAU", &[row]);
    feed(
        &mut p,
        FeedKind::Tbfm,
        t,
        &doc(t, tb(ms(t), Some("ABCDE12345"), "T1").flt("A320")),
    );
    let after = T0 + 2 * HOUR;
    step_to(&mut p, after);
    let stale = |p: &Pipeline| is_stale(&p.table, 0);
    assert!(stale(&p));
    // An identity-only row does not.
    let t1 = after + 30 * NS;
    feed(
        &mut p,
        FeedKind::Tbfm,
        t1,
        &doc(t1, tb(ms(t1), Some("ABCDE12345"), "T1").flt("A320")),
    );
    assert!(stale(&p));
    // A row with a time does.
    let t2 = after + 40 * NS;
    feed(
        &mut p,
        FeedKind::Tbfm,
        t2,
        &doc(
            t2,
            tb(ms(t2), Some("ABCDE12345"), "T1").eta("MFX1", ms(t2) + 600_000),
        ),
    );
    assert_eq!(pipeline_stale_count(&p), 0);
}

// REQ: OA-RPL-173
#[test]
fn the_stale_mark_is_part_of_the_state_hash() {
    let build = |hear: bool| {
        let mut p = running();
        let t = T0 + 9 * NS;
        feed(
            &mut p,
            FeedKind::Sfdps,
            t,
            &sf_doc(&[sf(1, ms(t)).call("TST1").pos(41.0, -88.0, ms(t))]),
        );
        step_to(&mut p, T0 + 2 * HOUR);
        let t1 = T0 + 2 * HOUR + 30 * NS;
        // Both leave the same columns except for the mark: a refused older position.
        let pos_ms = if hear { ms(t1) } else { ms(t) - 5000 };
        feed(
            &mut p,
            FeedKind::Sfdps,
            t1,
            &sf_doc(&[sf(1, ms(t1)).pos(41.0, -88.0, pos_ms)]),
        );
        p
    };
    let (a, b) = (build(false), build(true));
    assert!(flight_stale(&a, 1) && !flight_stale(&b, 1));
    // Unmarking the stale one by hand changes nothing else, so the hash alone tells them apart.
    let mut c = build(false);
    let s = slot_of_gufi(&c.table, gufi(1)).unwrap();
    let before = replay_hash(&c);
    c.table.stale[s] = false;
    assert_ne!(replay_hash(&c), before);
}

// REQ: OA-RPL-174
#[test]
fn a_step_too_large_to_measure_is_refused_counted_and_leaves_the_clock() {
    let mut p = small_pipeline(64);
    // The latest report time the ADS-B decoder accepts is just under 4.1e18 ns.
    let now = 4_000_000_000_000_000_000;
    for k in 0..10 {
        tick(&mut p, now + k);
    }
    adsb_frame(&mut p, now + 9, &[adsb_row(1, now + 9)]);
    let clock = p.replay_ns;
    let far = i64::MIN + 100;
    for k in 0..3 {
        tick(&mut p, far + k);
    }
    assert_eq!(p.replay_ns, clock);
    assert_eq!(p.errors[ErrKind::ClockStepOverflow as usize], 1);
    assert_eq!(p.stale_marked, 0);
    assert_eq!(p.adsb.live, 1);
    assert_eq!(adsb_stale_count(&p.adsb), 0);
    // The refused step is not remembered as a step.
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 0));
    // The clock goes on from where it was.
    tick(&mut p, now + 20);
    assert_eq!(p.replay_ns, now + 20);
}

/// Aircraft 1 and 2 heard at `T0 + 9 s`, then a confirmed step of the recorder's clock by
/// `to - (T0 + 9 s)`. Returns the replay clock after it.
fn heard_then_step(p: &mut Pipeline, to: i64) -> i64 {
    adsb_frame(
        p,
        T0 + 9 * NS,
        &[adsb_row(1, T0 + 9 * NS), adsb_row(2, T0 + 9 * NS)],
    );
    step_to(p, to);
    p.replay_ns
}

// REQ: OA-RPL-250
#[test]
fn a_forward_step_the_report_times_did_not_follow_keeps_the_aircraft_and_takes_new_reports() {
    let mut p = running();
    let now = heard_then_step(&mut p, T0 + HOUR);
    assert_eq!(p.clock.forward_steps, 1);
    let s = adsb_slot_of(&p.adsb, 2).unwrap();
    // The report times are in their own domain: nothing was moved.
    assert_eq!(p.adsb.time_ns[s], T0 + 9 * NS);
    // Twenty seconds later by the recorder; the reports' own clock says the same.
    let t = now + 20 * NS;
    adsb_frame(
        &mut p,
        t,
        &[adsb_row(1, T0 + 29 * NS), adsb_row(3, T0 + 29 * NS)],
    );
    assert_eq!(p.adsb.stats.out_of_order, 0);
    assert_eq!(p.adsb.stats.future_refused, 0);
    assert_eq!((p.adsb.stats.applied, p.adsb.live), (4, 3));
    assert_eq!(adsb_stale_count(&p.adsb), 1);
    assert_eq!(p.adsb.time_ns[s], T0 + 9 * NS);
    assert_eq!(p.adsb.stats.domain_kept_own, 1);
    assert_eq!(p.adsb.stats.domain_follows_clock, 0);
    assert_eq!(
        (p.adsb.pending_ns, p.adsb.offset_ns),
        (0, now - (T0 + 9 * NS))
    );
}

// REQ: OA-RPL-250
#[test]
fn a_backward_step_the_report_times_did_not_follow_takes_new_reports() {
    let mut p = running();
    let now = heard_then_step(&mut p, T0 - 10 * MIN);
    assert_eq!(p.clock.backward_steps, 1);
    let t = now + 20 * NS;
    adsb_frame(
        &mut p,
        t,
        &[adsb_row(1, T0 + 29 * NS), adsb_row(3, T0 + 29 * NS)],
    );
    assert_eq!(p.adsb.stats.future_refused, 0);
    assert_eq!(p.adsb.stats.out_of_order, 0);
    assert_eq!((p.adsb.stats.applied, p.adsb.live), (4, 3));
    assert_eq!(p.adsb.stats.domain_kept_own, 1);
    assert_eq!(
        (p.adsb.pending_ns, p.adsb.offset_ns),
        (0, now - (T0 + 9 * NS))
    );
}

// REQ: OA-RPL-250
#[test]
fn report_times_that_step_with_the_recorder_shift_the_held_ones() {
    let mut p = running();
    let now = heard_then_step(&mut p, T0 + HOUR);
    let t = now + 20 * NS;
    adsb_frame(&mut p, t, &[adsb_row(1, t)]);
    assert_eq!(p.adsb.stats.out_of_order, 0);
    assert_eq!(p.adsb.stats.domain_follows_clock, 1);
    assert_eq!(p.adsb.stats.domain_kept_own, 0);
    assert_eq!((p.adsb.pending_ns, p.adsb.offset_ns), (0, 0));
    // The aircraft not heard again moved with the step.
    let s = adsb_slot_of(&p.adsb, 2).unwrap();
    assert_eq!(p.adsb.time_ns[s], now);
}

// REQ: OA-RPL-251
#[test]
fn a_sweep_while_a_step_is_unattributed_evicts_nobody() {
    let mut p = running();
    heard_then_step(&mut p, T0 + HOUR);
    let mut t = p.replay_ns;
    for _ in 0..40 {
        t += 30 * NS;
        tick(&mut p, t);
    }
    assert_eq!(p.adsb.stats.evicted, 0);
    assert_eq!(p.adsb.live, 2);
    // The reports' own clock is now T0 + 9 s + 20 min: the next report resolves it.
    let own = T0 + 9 * NS + 20 * MIN;
    adsb_frame(&mut p, t, &[adsb_row(1, own)]);
    assert_eq!(p.adsb.stats.out_of_order, 0);
    assert_eq!(p.adsb.stats.domain_kept_own, 1);
    assert_eq!(p.adsb.live, 2);
}

// REQ: OA-RPL-252
#[test]
fn a_report_that_fits_neither_reading_is_refused_and_the_step_stays_pending() {
    let mut p = running();
    let now = heard_then_step(&mut p, T0 + HOUR);
    let t = now + 20 * NS;
    // Twenty minutes into the reports' own time: forty minutes old by one reading and twenty
    // minutes ahead by the other.
    adsb_frame(&mut p, t, &[adsb_row(1, T0 + 20 * MIN)]);
    assert_eq!(p.adsb.stats.future_refused, 1);
    assert_eq!(p.adsb.stats.applied, 2);
    assert_ne!(p.adsb.pending_ns, 0);
    assert_eq!(
        (
            p.adsb.stats.domain_kept_own,
            p.adsb.stats.domain_follows_clock
        ),
        (0, 0)
    );
    adsb_frame(&mut p, t + NS, &[adsb_row(1, T0 + 30 * NS)]);
    assert_eq!(p.adsb.stats.applied, 3);
    assert_eq!(p.adsb.pending_ns, 0);
    assert_eq!(p.adsb.stats.domain_kept_own, 1);
}

// REQ: OA-RPL-253
#[test]
fn two_steps_before_any_report_resolve_by_their_sum() {
    let mut p = running();
    let now1 = heard_then_step(&mut p, T0 + HOUR);
    step_to(&mut p, now1 + 30 * MIN);
    let now = p.replay_ns;
    assert_eq!(p.clock.forward_steps, 2);
    let t = now + 20 * NS;
    adsb_frame(&mut p, t, &[adsb_row(1, T0 + 29 * NS)]);
    assert_eq!(p.adsb.stats.out_of_order, 0);
    assert_eq!(p.adsb.stats.domain_kept_own, 1);
    assert_eq!(
        (p.adsb.pending_ns, p.adsb.offset_ns),
        (0, now - (T0 + 9 * NS))
    );
}

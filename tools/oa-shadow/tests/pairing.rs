//! Pairing, unpaired tracks, presence spans, start and drop timing, identity.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_shadow::compare::{compare_all, segments, Class};
use oa_shadow::model::EramFlight;
use oa_shadow::pairing::{pair, Rule};
use oa_shadow::thresholds::THRESHOLDS;

fn track(addr: Option<u32>, from_s: i64, to_s: i64) -> EramFlight {
    let samples = (from_s..=to_s)
        .step_by(12)
        .map(|s| eram_at(s, s as f64, 100.0, 0.0, 0.0))
        .collect();
    flight(addr, samples)
}

// REQ: OA-SHD-001
#[test]
fn an_address_pairs_with_the_aircraft_of_that_address() {
    let oa = oa_map(vec![
        oa_north(0xA, 0, 100, 100.0),
        oa_north(0xB, 0, 100, 100.0),
    ]);
    let eram = vec![track(Some(0xB), 0, 96), track(Some(0xC), 0, 96)];
    let p = pair(&eram, &oa);
    assert_eq!(p.groups.len(), 1);
    assert_eq!(p.groups[0].oa_addr, 0xB);
    assert_eq!(p.groups[0].eram, vec![(0, Rule::Address)]);
    assert_eq!(p.unpaired_eram, vec![1]);
    assert_eq!(p.unpaired_oa, vec![0xA]);
}

// REQ: OA-SHD-001
#[test]
fn several_eram_flights_of_one_address_make_one_group() {
    let oa = oa_map(vec![oa_north(0xA, 0, 100, 100.0)]);
    let p = pair(&[track(Some(0xA), 0, 48), track(Some(0xA), 60, 96)], &oa);
    assert_eq!(p.groups.len(), 1);
    assert_eq!(p.groups[0].eram.len(), 2);
}

fn with_squawk(mut a: oa_shadow::model::OaAircraft, sq: u16) -> oa_shadow::model::OaAircraft {
    a.idents = vec![ident(0, Some(sq), None)];
    a
}

// REQ: OA-SHD-001
#[test]
fn a_flight_without_an_address_pairs_by_beacon_and_proximity_within_five_miles() {
    let mut f = track(None, 0, 96);
    f.beacons = vec![(T0, 1234)];
    // The right squawk and near: paired.
    let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 100, 100.0), 1234)]);
    let p = pair(&[f.clone()], &oa);
    assert_eq!(p.groups[0].eram, vec![(0, Rule::BeaconProximity)]);
    // The wrong squawk: not paired.
    let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 100, 100.0), 1235)]);
    assert_eq!(pair(&[f.clone()], &oa).groups.len(), 0);
    // No squawk offered at all: not paired.
    let oa = oa_map(vec![oa_north(0x30, 0, 100, 100.0)]);
    assert_eq!(pair(&[f.clone()], &oa).unpaired_eram, vec![0]);
    // Radius edge: 5 NM = 9260 m. Move ERAM's first report east.
    let near = |east: f64| {
        let mut g = f.clone();
        g.samples[0] = eram_at(0, 0.0, 100.0, east, 0.0);
        let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 100, 100.0), 1234)]);
        pair(&[g], &oa).groups.len()
    };
    assert_eq!(near(9255.0), 1);
    assert_eq!(near(9265.0), 0);
    // No beacon either: unpaired.
    let mut g = f;
    g.beacons.clear();
    let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 100, 100.0), 1234)]);
    assert_eq!(pair(&[g], &oa).unpaired_eram, vec![0]);
}

// REQ: OA-SHD-001
#[test]
fn beacon_pairing_takes_the_nearest_then_the_lowest_address_and_skips_address_paired_aircraft() {
    let mut f = track(None, 0, 96);
    f.beacons = vec![(T0, 7)];
    let mut near = with_squawk(oa_north(0x50, 0, 100, 100.0), 7);
    let mut far = with_squawk(oa_north(0x40, 0, 100, 100.0), 7);
    for s in &mut far.samples {
        s.lat_e7 += 20_000; // about 222 m north
    }
    let oa = oa_map(vec![near.clone(), far.clone()]);
    assert_eq!(pair(&[f.clone()], &oa).groups[0].oa_addr, 0x50);
    // Same distance: the lowest address.
    near.addr = 0x60;
    far.addr = 0x40;
    far.samples = near.samples.clone();
    let oa = oa_map(vec![near, far]);
    assert_eq!(pair(&[f.clone()], &oa).groups[0].oa_addr, 0x40);
    // An aircraft already paired by address is not a candidate.
    let a = with_squawk(oa_north(0x40, 0, 100, 100.0), 7);
    let other = track(Some(0x40), 0, 96);
    let p = pair(&[other, f], &oa_map(vec![a]));
    assert_eq!(p.groups.len(), 1);
    assert_eq!(p.unpaired_eram, vec![1]);
}

// REQ: OA-SHD-002
#[test]
fn unpaired_tracks_are_events_with_their_duration() {
    let oa = oa_map(vec![oa_north(0xA, 10, 70, 100.0)]);
    let eram = vec![track(Some(0xC), 0, 96)];
    let c = compare_all(&eram, &oa, None);
    let e = of(&c, Class::UnpairedEram);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].value, 108.0); // 96 s plus half of 12 s at each end
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0, T0 + 96_000));
    let o = of(&c, Class::UnpairedOa);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].value, 61.0); // 60 s plus half of 1 s at each end
                                  // Each has its own flight row and index.
    assert_eq!(c.flights.len(), 2);
    assert_eq!(c.flights[0].kind, "eram_only");
    assert_eq!(c.flights[1].kind, "oa_only");
    assert_ne!(e[0].flight, o[0].flight);
}

// REQ: OA-SHD-009
#[test]
fn eram_reports_without_an_openatca_sample_are_one_span_with_a_padded_duration() {
    // OpenATCA has rows 0..=40 s and 70..=120 s; ERAM reports every 12 s from 12 to 108 s.
    let mut oa = oa_north(1, 0, 120, 100.0);
    oa.samples
        .retain(|s| s.t_ms - T0 <= 40_000 || s.t_ms - T0 >= 70_000);
    let samples = (1..=9)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
        .collect();
    let c = compare_all(&[flight(Some(1), samples)], &oa_map(vec![oa]), None);
    let e = of(&c, Class::EramOnlySpan);
    // Uncovered: the reports at 48 and 60 s (72 s has a row). 12 s between them plus half the
    // 12 s interval at each end.
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0 + 48_000, T0 + 60_000));
    assert_eq!(e[0].value, 24.0);
    assert_eq!(c.flights[0].eram_only_s, 24.0);
}

#[test]
fn a_lone_report_gets_one_interval_and_the_interval_is_capped_at_the_segment_gap() {
    use oa_shadow::compare::padded_ms;
    assert_eq!(padded_ms(&[0, 12_000, 24_000], 1, 1), 12_000);
    assert_eq!(
        padded_ms(&[0, 12_000, 24_000], 0, 2),
        24_000 + 6_000 + 6_000
    );
    assert_eq!(padded_ms(&[0, 12_000, 24_000], 0, 0), 12_000);
    assert_eq!(padded_ms(&[5], 0, 0), 0);
    // A neighbour 10 minutes away counts as one segment gap (30 s), half of it at the end.
    assert_eq!(padded_ms(&[0, 600_000], 1, 1), 30_000);
}

// REQ: OA-SHD-009
#[test]
fn openatca_rows_outside_every_eram_flight_span_are_an_openatca_only_span() {
    // ERAM reports 24..=60 s (12 s apart); OpenATCA rows 0..=100 s. A span is widened by 12 s
    // plus the 12 s interval either side: [0, 84]. Rows 85..=100 are OpenATCA only.
    let samples = (2..=5)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
        .collect();
    let c = compare_all(
        &[flight(Some(1), samples)],
        &oa_map(vec![oa_north(1, 0, 100, 100.0)]),
        None,
    );
    let e = of(&c, Class::OaOnlySpan);
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0 + 85_000, T0 + 100_000));
    assert_eq!(e[0].value, 16.0);
    assert_eq!(c.flights[0].oa_only_s, 16.0);
}

// REQ: OA-SHD-019
#[test]
fn a_gap_inside_one_eram_flight_is_a_report_gap_not_an_openatca_only_span() {
    // Reports 12, 24, then 24 + 31 = 55 s and 67 s; OpenATCA flies through the gap.
    let times = [12, 24, 55, 67];
    let samples = times
        .iter()
        .map(|&t| eram_at(t, t as f64, 100.0, 0.0, 0.0))
        .collect();
    let c = compare_all(
        &[flight(Some(1), samples)],
        &oa_map(vec![oa_north(1, 0, 80, 100.0)]),
        None,
    );
    let g = of(&c, Class::EramReportGap);
    assert_eq!(g.len(), 1);
    assert_eq!(
        (g[0].t_ms, g[0].end_ms, g[0].value),
        (T0 + 24_000, T0 + 55_000, 31.0)
    );
    assert_eq!(count(&c, Class::OaOnlySpan), 0);
    // Exactly 30 s is not a gap.
    let samples = [12, 42]
        .iter()
        .map(|&t| eram_at(t, t as f64, 100.0, 0.0, 0.0))
        .collect();
    let c = compare_all(
        &[flight(Some(1), samples)],
        &oa_map(vec![oa_north(1, 0, 80, 100.0)]),
        None,
    );
    assert_eq!(count(&c, Class::EramReportGap), 0);
}

/// An aircraft northbound at 100 m/s with rows every second from `start_ms` to `end_ms` after
/// T0 (the last row exactly at `end_ms`).
fn oa_between(addr: u32, start_ms: i64, end_ms: i64) -> oa_shadow::model::OaAircraft {
    let mut a = oa_north(addr, 0, 0, 100.0);
    a.samples.clear();
    let mut t = start_ms;
    loop {
        let (lat, lon) = displaced(0.0, 100.0 * t as f64 / 1000.0);
        let mut s = oa_north(addr, 0, 0, 100.0).samples[0];
        s.t_ms = T0 + t;
        s.lat_e7 = lat;
        s.lon_e7 = lon;
        a.samples.push(s);
        if t >= end_ms {
            break;
        }
        t = (t + 1000).min(end_ms);
    }
    a
}

fn eram_24_to_60() -> EramFlight {
    track(Some(1), 24, 60)
}

// REQ: OA-SHD-008
#[test]
fn start_and_drop_differences_must_exceed_twelve_seconds_plus_the_eram_report_interval() {
    assert!(!THRESHOLDS.timing_differs(12_000, 0));
    assert!(THRESHOLDS.timing_differs(12_001, 0));
    assert!(!THRESHOLDS.timing_differs(-24_000, 12_000));
    assert!(THRESHOLDS.timing_differs(-24_001, 12_000));
    // ERAM 24..=60 s, a report every 12 s, so the threshold is 24 s at either end.
    for (d, e, starts, drops) in [
        (0i64, 0i64, 0usize, 0usize),
        (24_000, 0, 0, 0),
        (24_001, 0, 1, 0),
        (-24_000, 0, 0, 0),
        (-24_001, 0, 1, 0),
        (0, 24_000, 0, 0),
        (0, 24_001, 0, 1),
        (0, -24_000, 0, 0),
        (0, -24_001, 0, 1),
    ] {
        let oa = oa_between(1, 24_000 + d, 60_000 + e);
        let c = compare_all(&[eram_24_to_60()], &oa_map(vec![oa]), None);
        assert_eq!(
            (count(&c, Class::StartTiming), count(&c, Class::DropTiming)),
            (starts, drops),
            "d {d} e {e}"
        );
    }
    // The median ERAM report interval is recorded for the summary.
    let c = compare_all(
        &[eram_24_to_60()],
        &oa_map(vec![oa_between(1, 0, 100_000)]),
        None,
    );
    assert_eq!(c.totals.eram_intervals_ms, vec![12_000, 12_000, 12_000]);
}

// REQ: OA-SHD-008
#[test]
fn timing_events_name_the_direction_and_the_two_times() {
    let c = compare_all(
        &[eram_24_to_60()],
        &oa_map(vec![oa_between(1, 55_000, 100_000)]),
        None,
    );
    let s = of(&c, Class::StartTiming);
    assert_eq!(s.len(), 1);
    assert_eq!(
        (s[0].t_ms, s[0].end_ms, s[0].value),
        (T0 + 24_000, T0 + 55_000, 31.0)
    );
    assert_eq!(s[0].detail, "oa_later");
    let c = compare_all(
        &[eram_24_to_60()],
        &oa_map(vec![oa_between(1, 0, 30_000)]),
        None,
    );
    let d = of(&c, Class::DropTiming);
    assert_eq!(d.len(), 1);
    assert_eq!(
        (d[0].t_ms, d[0].end_ms, d[0].value),
        (T0 + 60_000, T0 + 30_000, 30.0)
    );
    assert_eq!(d[0].detail, "oa_earlier");
}

#[test]
fn segments_split_on_gaps_over_the_limit_only() {
    assert_eq!(segments(&[0, 10, 40, 80], 30), vec![(0, 40), (80, 80)]);
    assert_eq!(segments(&[], 30), vec![]);
}

fn ident_ms(t_ms: i64, squawk: u16) -> oa_shadow::model::IdentPoint {
    oa_shadow::model::IdentPoint {
        t_ms: T0 + t_ms,
        squawk: Some(squawk),
        callsign: None,
    }
}

fn beacon_run(change_ms: i64, reports: &[i64]) -> usize {
    // OpenATCA was offered 1200 from the start and 2000 from `change_ms`; ERAM says 2000.
    let mut oa = oa_north(1, 0, 200, 100.0);
    oa.idents = vec![ident_ms(0, 1200), ident_ms(change_ms, 2000)];
    let mut f = flight(
        Some(1),
        reports
            .iter()
            .map(|&t| eram_at(t, t as f64, 100.0, 0.0, 0.0))
            .collect(),
    );
    f.beacons = vec![(T0, 2000)];
    let c = compare_all(&[f], &oa_map(vec![oa]), None);
    count(&c, Class::IdBeacon)
}

// REQ: OA-SHD-007
#[test]
fn a_beacon_disagrees_only_beyond_the_60_second_window_for_three_reports() {
    // Reports at 12, 24, 36 s. 2000 is offered within +-60 s of a report when the change is at
    // or before report + 60 s. A change at 96.000 s is exactly 60 s after the 36 s report: that
    // report agrees, leaving two disagreeing reports (12, 24) which are not a run.
    assert_eq!(beacon_run(96_000, &[12, 24, 36]), 0);
    // One millisecond later all three disagree: one event.
    assert_eq!(beacon_run(96_001, &[12, 24, 36]), 1);
    // Two reports are never enough, however far off.
    assert_eq!(beacon_run(150_000, &[12, 24]), 0);
    // A change much later: three reports of one flight, one event not three.
    assert_eq!(beacon_run(190_000, &[12, 24, 36, 48]), 1);
}

// REQ: OA-SHD-007
#[test]
fn a_beacon_run_is_one_event_with_its_span_and_the_codes() {
    let mut oa = oa_north(1, 0, 200, 100.0);
    oa.idents = vec![ident_ms(0, 1200)];
    let mut f = track(Some(1), 12, 108);
    f.beacons = vec![(T0, 7000)];
    let c = compare_all(&[f.clone()], &oa_map(vec![oa.clone()]), None);
    let e = of(&c, Class::IdBeacon);
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0 + 12_000, T0 + 108_000));
    assert_eq!(e[0].value, 108.0); // 96 s plus half of 12 s each end
    assert_eq!(e[0].detail, "eram=7000 oa=1200 reports=9");
    assert_eq!(c.totals.beacon_compared, 9);
    // Agreeing, no ERAM code, or no OpenATCA squawk: nothing.
    f.beacons = vec![(T0, 1200)];
    assert_eq!(
        count(
            &compare_all(&[f.clone()], &oa_map(vec![oa.clone()]), None),
            Class::IdBeacon
        ),
        0
    );
    f.beacons.clear();
    assert_eq!(
        count(
            &compare_all(&[f.clone()], &oa_map(vec![oa.clone()]), None),
            Class::IdBeacon
        ),
        0
    );
    f.beacons = vec![(T0, 7000)];
    oa.idents.clear();
    assert_eq!(
        count(&compare_all(&[f], &oa_map(vec![oa]), None), Class::IdBeacon),
        0
    );
}

// REQ: OA-SHD-007
#[test]
fn a_callsign_disagrees_by_the_same_window_and_run_rule() {
    let mk = |n: usize, cs: &str| {
        let mut oa = oa_north(1, 0, 200, 100.0);
        oa.idents = vec![ident(0, None, Some("N123AB"))];
        let mut f = flight(
            Some(1),
            (1..=n as i64)
                .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
                .collect(),
        );
        f.callsign = Some(cs8(cs));
        compare_all(&[f], &oa_map(vec![oa]), None)
    };
    assert_eq!(count(&mk(2, "DAL9"), Class::IdCallsign), 0);
    let c = mk(3, "DAL9");
    assert_eq!(count(&c, Class::IdCallsign), 1);
    assert_eq!(
        of(&c, Class::IdCallsign)[0].detail,
        "eram=DAL9 oa=N123AB reports=3"
    );
    // Case and padding are ignored.
    assert_eq!(count(&mk(5, "n123ab"), Class::IdCallsign), 0);
    // A callsign offered by a point within 60 s counts as offered.
    let mut oa = oa_north(1, 0, 200, 100.0);
    oa.idents = vec![
        ident(0, None, Some("N123AB")),
        ident(96, None, Some("DAL9")),
    ];
    let mut f = flight(
        Some(1),
        (1..=3)
            .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
            .collect(),
    );
    f.callsign = Some(cs8("DAL9"));
    // The 36 s report sees the 96 s point (exactly 60 s): 2 disagreeing reports, no run.
    assert_eq!(
        count(
            &compare_all(&[f], &oa_map(vec![oa]), None),
            Class::IdCallsign
        ),
        0
    );
}

// REQ: OA-SHD-007
#[test]
fn the_squawk_in_force_is_the_last_point_at_or_before_the_time() {
    let mut oa = oa_north(1, 0, 10, 100.0);
    oa.idents = vec![
        ident(0, Some(1111), None),
        ident(100, Some(2222), None),
        ident(300, None, None),
    ];
    assert_eq!(oa.squawk_at(T0 - 1), None);
    assert_eq!(oa.squawk_at(T0), Some(1111));
    assert_eq!(oa.squawk_at(T0 + 99_999), Some(1111));
    assert_eq!(oa.squawk_at(T0 + 100_000), Some(2222));
    assert_eq!(oa.squawk_at(T0 + 300_000), None);
}

// REQ: OA-SHD-020
#[test]
fn two_overlapping_eram_flights_of_one_aircraft_are_compared_once() {
    // Flight A reports 12..=108 s on the path; flight B (a duplicate track) reports 48..=120 s
    // 1000 m east of it. Inside the overlap only A is compared.
    let a = track(Some(0xA), 12, 108);
    let mut b = flight(
        Some(0xA),
        (48..=120)
            .step_by(12)
            .map(|s| eram_at(s, s as f64, 100.0, 1000.0, 0.0))
            .collect(),
    );
    b.gufi = 2;
    let c = compare_all(
        &[a.clone(), b.clone()],
        &oa_map(vec![oa_north(0xA, 0, 130, 100.0)]),
        None,
    );
    let d = of(&c, Class::DupEramTrack);
    assert_eq!(d.len(), 1);
    assert_eq!(
        (d[0].t_ms, d[0].end_ms, d[0].value),
        (T0 + 48_000, T0 + 108_000, 60.0)
    );
    // A's 9 reports and B's one report after the overlap (120 s): each time once.
    assert_eq!(c.totals.compared, 10);
    assert_eq!(c.flights.len(), 1);
    // B's reports inside the overlap are not compared, the one after is.
    assert_eq!(count(&c, Class::PosCross05), 1);
    assert_eq!(of(&c, Class::PosCross05)[0].t_ms, T0 + 120_000);
    // The flight with the earliest first report is the primary whatever the input order.
    let c2 = compare_all(
        &[b.clone(), a.clone()],
        &oa_map(vec![oa_north(0xA, 0, 130, 100.0)]),
        None,
    );
    assert_eq!(c2.totals.compared, 10);
    assert_eq!(count(&c2, Class::PosCross05), 1);
    // Equal first reports: the lower flight index is the primary.
    let mut b0 = b.clone();
    b0.samples = (12..=108)
        .step_by(12)
        .map(|s| eram_at(s, s as f64, 100.0, 1000.0, 0.0))
        .collect();
    let c3 = compare_all(
        &[a.clone(), b0.clone()],
        &oa_map(vec![oa_north(0xA, 0, 130, 100.0)]),
        None,
    );
    assert_eq!((c3.totals.compared, count(&c3, Class::PosCross05)), (9, 0));
    let c4 = compare_all(&[b0, a], &oa_map(vec![oa_north(0xA, 0, 130, 100.0)]), None);
    assert_eq!((c4.totals.compared, count(&c4, Class::PosCross05)), (9, 9));
    // Disjoint spans of one aircraft are not duplicates.
    let c5 = compare_all(
        &[track(Some(0xA), 12, 48), track(Some(0xA), 72, 108)],
        &oa_map(vec![oa_north(0xA, 0, 130, 100.0)]),
        None,
    );
    assert_eq!(count(&c5, Class::DupEramTrack), 0);
    assert_eq!(c5.totals.compared, 4 + 4);
}

// REQ: OA-SHD-021
#[test]
fn an_address_far_from_its_aircraft_for_three_reports_is_contradicted_and_left_out_of_position() {
    // 20 NM = 37.04 km. Reports 12..=108 s: 48..=72 s (three) are 50 km north of the aircraft.
    let far = |t: i64| {
        if (48..=72).contains(&t) {
            50_000.0
        } else {
            0.0
        }
    };
    let samples: Vec<_> = (1..=9)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, far(k * 12)))
        .collect();
    let c = compare_all(
        &[flight(Some(1), samples.clone())],
        &oa_map(vec![oa_north(1, 0, 120, 100.0)]),
        None,
    );
    let e = of(&c, Class::AddrContradictsTrack);
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0 + 48_000, T0 + 72_000));
    assert_eq!(e[0].value, 24.0 + 12.0); // 24 s between the reports plus half of 12 s each end
    assert_eq!(e[0].detail, "reports=3");
    assert_eq!(
        count(&c, Class::PosAlong10) + count(&c, Class::PosCross10),
        0
    );
    assert!(c.flights[0].worst_along_m < 1.0);
    // Two far reports are not enough: they stay position events.
    let two: Vec<_> = (1..=9)
        .map(|k| {
            eram_at(
                k * 12,
                (k * 12) as f64,
                100.0,
                0.0,
                if (48..=60).contains(&(k * 12)) {
                    50_000.0
                } else {
                    0.0
                },
            )
        })
        .collect();
    let c = compare_all(
        &[flight(Some(1), two)],
        &oa_map(vec![oa_north(1, 0, 120, 100.0)]),
        None,
    );
    assert_eq!(count(&c, Class::AddrContradictsTrack), 0);
    assert_eq!(count(&c, Class::PosAlong10), 2);
    // A beacon-paired flight is not judged by its address.
    let mut f = flight(None, samples);
    f.beacons = vec![(T0, 5)];
    let mut oa = oa_north(1, 0, 120, 100.0);
    oa.idents = vec![ident(0, Some(5), None)];
    let c = compare_all(&[f], &oa_map(vec![oa]), None);
    assert_eq!(count(&c, Class::AddrContradictsTrack), 0);
}

// REQ: OA-SHD-001
#[test]
fn an_address_openatca_has_no_aircraft_for_falls_through_to_the_beacon_rule() {
    let mut f = track(Some(0xDEAD), 0, 96);
    f.beacons = vec![(T0, 1234)];
    let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 100, 100.0), 1234)]);
    let p = pair(&[f.clone()], &oa);
    assert_eq!(p.groups.len(), 1);
    assert_eq!(p.groups[0].oa_addr, 0x30);
    assert_eq!(p.groups[0].eram, vec![(0, Rule::BeaconProximity)]);
    // Without a beacon it stays unpaired.
    f.beacons.clear();
    assert_eq!(pair(&[f], &oa).unpaired_eram, vec![0]);
}

// REQ: OA-SHD-001
#[test]
fn beacon_claims_are_exclusive_only_within_overlapping_spans() {
    let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 400, 100.0), 7)]);
    let mk = |from: i64, to: i64, addr: Option<u32>| {
        let mut f = track(addr, from, to);
        f.beacons = vec![(T0, 7)];
        f
    };
    // Paired by address over 0..=96 s; a beacon flight over 60..=156 s overlaps it: refused.
    let p = pair(&[mk(0, 96, Some(0x30)), mk(60, 156, None)], &oa);
    assert_eq!(p.unpaired_eram, vec![1]);
    // One over 120..=200 s does not overlap: it pairs.
    let p = pair(&[mk(0, 96, Some(0x30)), mk(120, 204, None)], &oa);
    assert_eq!(
        p.groups[0].eram,
        vec![(0, Rule::Address), (1, Rule::BeaconProximity)]
    );
    // Two beacon flights at once: the first claims, the second is refused.
    let p = pair(&[mk(0, 96, None), mk(24, 120, None)], &oa);
    assert_eq!(
        (p.groups[0].eram.len(), p.unpaired_eram.clone()),
        (1, vec![1])
    );
    // One after the other: both pair.
    let p = pair(&[mk(0, 48, None), mk(96, 144, None)], &oa);
    assert_eq!(p.groups[0].eram.len(), 2);
}

// REQ: OA-SHD-001
#[test]
fn the_beacon_in_force_at_the_first_report_pairs_the_flight() {
    let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 100, 100.0), 9)]);
    let mut f = track(None, 12, 96);
    // 5 from the start, 9 only from 40 s: the code in force at the first report (5) is used.
    f.beacons = vec![(T0, 5), (T0 + 40_000, 9)];
    assert_eq!(pair(&[f.clone()], &oa).unpaired_eram, vec![0]);
    // A first message after the first report: the earliest code is used.
    f.beacons = vec![(T0 + 40_000, 9)];
    assert_eq!(pair(&[f], &oa).groups.len(), 1);
}

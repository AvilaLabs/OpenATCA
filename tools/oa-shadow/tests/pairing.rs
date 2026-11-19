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
    f.beacon = Some(1234);
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
    g.beacon = None;
    let oa = oa_map(vec![with_squawk(oa_north(0x30, 0, 100, 100.0), 1234)]);
    assert_eq!(pair(&[g], &oa).unpaired_eram, vec![0]);
}

// REQ: OA-SHD-001
#[test]
fn beacon_pairing_takes_the_nearest_then_the_lowest_address_and_skips_address_paired_aircraft() {
    let mut f = track(None, 0, 96);
    f.beacon = Some(7);
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
    assert_eq!(e[0].value, 96.0);
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0, T0 + 96_000));
    let o = of(&c, Class::UnpairedOa);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].value, 60.0);
    // Each has its own flight row and index.
    assert_eq!(c.flights.len(), 2);
    assert_eq!(c.flights[0].kind, "eram_only");
    assert_eq!(c.flights[1].kind, "oa_only");
    assert_ne!(e[0].flight, o[0].flight);
}

// REQ: OA-SHD-009
#[test]
fn eram_reports_without_an_openatca_sample_are_one_span_with_a_duration() {
    // OpenATCA has rows 0..=40 s and 70..=120 s; ERAM reports every 12 s from 12 to 108 s.
    let mut oa = oa_north(1, 0, 120, 100.0);
    oa.samples
        .retain(|s| s.t_ms - T0 <= 40_000 || s.t_ms - T0 >= 70_000);
    let samples = (1..=9)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
        .collect();
    let c = compare_all(&[flight(Some(1), samples)], &oa_map(vec![oa]), None);
    let e = of(&c, Class::EramOnlySpan);
    // Reports at 48, 60 and 72?? 72 is covered (row at 72); uncovered: 48 and 60.
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0 + 48_000, T0 + 60_000));
    assert_eq!(e[0].value, 12.0);
    assert_eq!(c.flights[0].eram_only_s, 12.0);
}

// REQ: OA-SHD-009
#[test]
fn openatca_rows_beyond_the_timing_threshold_of_eram_are_an_openatca_only_span() {
    // ERAM reports 24..=60 s; OpenATCA rows 0..=100 s. Rows more than 12 s before 24 s (t < 12)
    // and after 72 s are OpenATCA only; those at exactly 12 s and 72 s are not.
    let samples = (2..=5)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
        .collect();
    let c = compare_all(
        &[flight(Some(1), samples)],
        &oa_map(vec![oa_north(1, 0, 100, 100.0)]),
        None,
    );
    let e = of(&c, Class::OaOnlySpan);
    assert_eq!(e.len(), 2);
    assert_eq!((e[0].t_ms, e[0].end_ms), (T0, T0 + 11_000));
    assert_eq!((e[1].t_ms, e[1].end_ms), (T0 + 73_000, T0 + 100_000));
    assert_eq!(c.flights[0].oa_only_s, 11.0 + 27.0);
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
fn start_and_drop_timing_differences_are_over_twelve_seconds_either_sign() {
    assert!(!THRESHOLDS.timing_differs(12_000));
    assert!(!THRESHOLDS.timing_differs(-12_000));
    assert!(THRESHOLDS.timing_differs(12_001));
    assert!(THRESHOLDS.timing_differs(-12_001));
    // ERAM 24..=60 s; OpenATCA starts at 24 s + d and drops at 60 s + e (milliseconds).
    for (d, e, starts, drops) in [
        (0i64, 0i64, 0usize, 0usize),
        (12_000, -12_000, 0, 0),
        (12_001, -12_001, 1, 1),
        (-12_000, 12_000, 0, 0),
        (-12_001, 12_001, 1, 1),
        (30_000, 0, 1, 0),
        (0, 30_000, 0, 1),
    ] {
        let oa = oa_between(1, 24_000 + d, 60_000 + e);
        let c = compare_all(&[eram_24_to_60()], &oa_map(vec![oa]), None);
        assert_eq!(
            (count(&c, Class::StartTiming), count(&c, Class::DropTiming)),
            (starts, drops),
            "d {d} e {e}"
        );
    }
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

// REQ: OA-SHD-007
#[test]
fn identity_differences_are_beacon_and_callsign_not_offered_near_the_flight() {
    let mut oa = oa_north(1, 0, 120, 100.0);
    oa.idents = vec![
        ident(0, Some(1200), Some("N123AB")),
        ident(60, Some(2000), None),
    ];
    let mk = |beacon: Option<u16>, cs: Option<&str>| {
        let mut f = track(Some(1), 12, 108);
        f.beacon = beacon;
        f.callsign = cs.map(cs8);
        compare_all(&[f], &oa_map(vec![oa.clone()]), None)
    };
    // Agreeing on either of the squawks offered, case and padding ignored.
    let c = mk(Some(2000), Some("n123ab"));
    assert_eq!(
        (count(&c, Class::IdBeacon), count(&c, Class::IdCallsign)),
        (0, 0)
    );
    let c = mk(Some(1200), Some("N123AB  "));
    assert_eq!(
        (count(&c, Class::IdBeacon), count(&c, Class::IdCallsign)),
        (0, 0)
    );
    // Disagreeing.
    let c = mk(Some(7000), Some("DAL9"));
    assert_eq!(count(&c, Class::IdBeacon), 1);
    assert_eq!(count(&c, Class::IdCallsign), 1);
    assert_eq!(of(&c, Class::IdBeacon)[0].detail, "eram=7000 oa=1200,2000");
    assert_eq!(of(&c, Class::IdCallsign)[0].detail, "eram=DAL9 oa=N123AB");
    // Where either side has none, there is nothing to compare.
    let c = mk(None, None);
    assert_eq!(
        (count(&c, Class::IdBeacon), count(&c, Class::IdCallsign)),
        (0, 0)
    );
    let mut bare = oa.clone();
    bare.idents.clear();
    let mut f = track(Some(1), 12, 108);
    f.beacon = Some(7000);
    f.callsign = Some(cs8("DAL9"));
    let c = compare_all(&[f], &oa_map(vec![bare]), None);
    assert_eq!(
        (count(&c, Class::IdBeacon), count(&c, Class::IdCallsign)),
        (0, 0)
    );
}

// REQ: OA-SHD-007
#[test]
fn identity_points_outside_the_flights_span_do_not_count_except_the_one_in_force() {
    let mut oa = oa_north(1, 0, 400, 100.0);
    oa.idents = vec![
        ident(0, Some(1111), None),
        ident(100, Some(2222), None),
        ident(300, Some(3333), None),
    ];
    // The flight is at 150..=200 s: the point in force at the start is 2222 (from 100 s); 3333
    // starts 100 s after the span plus one segment gap (30 s), and 1111 is superseded.
    let mut f = track(Some(1), 150, 198);
    f.beacon = Some(2222);
    let c = compare_all(&[f.clone()], &oa_map(vec![oa.clone()]), None);
    assert_eq!(count(&c, Class::IdBeacon), 0);
    f.beacon = Some(1111);
    let c = compare_all(&[f.clone()], &oa_map(vec![oa.clone()]), None);
    assert_eq!(count(&c, Class::IdBeacon), 1);
    f.beacon = Some(3333);
    let c = compare_all(&[f], &oa_map(vec![oa]), None);
    assert_eq!(count(&c, Class::IdBeacon), 1);
}

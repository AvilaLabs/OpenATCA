//! The crossing search, the landing time, the bins, the statistics, the join rule and the report
//! text, over made-up paths and flights.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_legacy_traj::core::bins::{bin_of, score, stats, BIN_COUNT, BIN_NAMES};
use oa_legacy_traj::core::cross::{approach, choose_landing, choose_passage, landings};
use oa_legacy_traj::core::join::{index_legs, join, Leg, Outcome, Unit};
use oa_legacy_traj::core::report::{
    closest_add, kind_add, render_counts_csv, render_csv, render_text, system_new,
};
use oa_legacy_traj::core::types::{AirState, Config, Exclusion, PathPoint, Place, NO_ADDR};

const M_PER_DEG: f64 = 111_194.926_644_558_74;
const NM: f64 = 1852.0;

fn cfg() -> Config {
    Config {
        cut_m: 2.0 * NM,
        runway_radius_m: 5.0 * NM,
        landing_gap_max_ms: 120_000,
        max_look_ahead_ms: 120 * 60_000,
    }
}

const ORIGIN: Place = Place {
    lat_deg: 41.0,
    lon_deg: -88.0,
};

/// A path due north along the meridian of the origin, 100 m/s, a sample every `step_s`, from
/// `south_m` south of the origin to `north_m` north of it, starting at `t0_ms`.
fn northbound(t0_ms: i64, south_m: f64, north_m: f64, step_s: i64, east_m: f64) -> Vec<PathPoint> {
    let cos_lat = (ORIGIN.lat_deg * std::f64::consts::PI / 180.0).cos();
    let mut out = Vec::new();
    let mut k = 0i64;
    loop {
        let y = -south_m + 100.0 * (k * step_s) as f64;
        if y > north_m {
            break;
        }
        out.push(PathPoint {
            t_ms: t0_ms + k * step_s * 1000,
            lat_deg: ORIGIN.lat_deg + y / M_PER_DEG,
            lon_deg: ORIGIN.lon_deg + east_m / (M_PER_DEG * cos_lat),
            gap_before: false,
        });
        k += 1;
    }
    out
}

// REQ: OA-LTJ-001
#[test]
fn the_crossing_is_the_closest_approach_interpolated_between_samples() {
    // 100 m/s from 10 km south: at the origin 100 s after the start. Samples every 30 s, so no
    // sample is at the origin (the nearest are at 90 s and 120 s).
    let path = northbound(1_000_000, 10_000.0, 10_000.0, 30, 0.0);
    let a = approach(&path, ORIGIN, 2.0 * NM);
    assert_eq!(a.passages.len(), 1);
    let p = a.passages[0];
    assert!((p.t_ms - 1_100_000).abs() <= 2, "{}", p.t_ms);
    assert!(p.dist_m < 1.0, "{}", p.dist_m);
    assert!(!p.at_edge);
    assert!(a.min_dist_m < 1.0);
    assert_eq!(choose_passage(&a, 1_000_000, &cfg()), Ok(p.t_ms));
}

// REQ: OA-LTJ-002
#[test]
fn a_closest_approach_beyond_the_cut_is_not_a_crossing_and_is_counted_by_distance() {
    let cut = 2.0 * NM;
    let near = approach(
        &northbound(0, 10_000.0, 10_000.0, 10, 0.9 * cut),
        ORIGIN,
        cut,
    );
    let far = approach(
        &northbound(0, 10_000.0, 10_000.0, 10, 1.1 * cut),
        ORIGIN,
        cut,
    );
    assert_eq!(near.passages.len(), 1);
    assert!((near.min_dist_m - 0.9 * cut).abs() < 5.0);
    assert!(far.passages.is_empty());
    assert!((far.min_dist_m - 1.1 * cut).abs() < 5.0);
    assert_eq!(choose_passage(&far, 0, &cfg()), Err(Exclusion::BeyondCut));
    let mut s = system_new("X", &["k"]);
    closest_add(&mut s, near.min_dist_m);
    closest_add(&mut s, far.min_dist_m);
    closest_add(&mut s, 20.0 * NM);
    closest_add(&mut s, f64::INFINITY);
    // 0.9 * 2 NM = 1.8 NM (bucket over 1 up to 2), 2.2 NM (over 2 up to 3), 20 NM (over 10 up to
    // 20); the infinite one is no search.
    assert_eq!(s.closest.iter().sum::<u64>(), 3);
    assert_eq!(s.closest[2], 1);
    assert_eq!(s.closest[3], 1);
    assert_eq!(s.closest[6], 1);
}

// REQ: OA-LTJ-003
#[test]
fn a_passage_at_a_path_end_or_a_hole_is_not_scored() {
    // The path begins at the origin: its closest approach is at the first sample.
    let start = northbound(0, 0.0, 10_000.0, 10, 0.0);
    let a = approach(&start, ORIGIN, 2.0 * NM);
    assert!(a.passages[0].at_edge);
    assert_eq!(choose_passage(&a, 0, &cfg()), Err(Exclusion::PassageAtEdge));
    // A hole over the origin: truth has no answer for 60 s around it.
    let mut hole = northbound(0, 10_000.0, 10_000.0, 10, 0.0);
    hole.retain(|p| p.t_ms < 80_000 || p.t_ms > 130_000);
    if let Some(p) = hole.iter_mut().find(|p| p.t_ms > 130_000) {
        p.gap_before = true;
    }
    let a = approach(&hole, ORIGIN, 2.0 * NM);
    assert!(a.passages.iter().all(|p| p.at_edge));
    assert_eq!(choose_passage(&a, 0, &cfg()), Err(Exclusion::PassageAtEdge));
    // A path that never gets near, and an empty path.
    let a = approach(&[], ORIGIN, 2.0 * NM);
    assert_eq!(choose_passage(&a, 0, &cfg()), Err(Exclusion::NoTruthPath));
}

// REQ: OA-LTJ-004
#[test]
fn the_estimate_is_scored_against_the_first_passage_not_before_its_issue() {
    // North through the origin and back south through it: two passages, at 100 s and 400 s.
    let mut path = northbound(0, 10_000.0, 10_000.0, 10, 0.0);
    let turn = path.last().unwrap().t_ms;
    let back: Vec<PathPoint> = northbound(0, 10_000.0, 10_000.0, 10, 0.0)
        .iter()
        .rev()
        .enumerate()
        .map(|(k, p)| PathPoint {
            t_ms: turn + 10_000 * (k as i64 + 1),
            ..*p
        })
        .collect();
    path.extend(back);
    let a = approach(&path, ORIGIN, 2.0 * NM);
    assert_eq!(a.passages.len(), 2);
    let (first, second) = (a.passages[0].t_ms, a.passages[1].t_ms);
    assert!(second > first + 100_000);
    assert_eq!(choose_passage(&a, 0, &cfg()), Ok(first));
    assert_eq!(choose_passage(&a, first + 1_000, &cfg()), Ok(second));
    assert_eq!(
        choose_passage(&a, second + 1_000, &cfg()),
        Err(Exclusion::PassedBeforeIssue)
    );
}

fn st(t_s: i64, north_m: f64, ground: bool) -> AirState {
    AirState {
        t_ms: t_s * 1000,
        lat_deg: ORIGIN.lat_deg + north_m / M_PER_DEG,
        lon_deg: ORIGIN.lon_deg,
        ground,
    }
}

// REQ: OA-LTJ-005
#[test]
fn the_landing_is_the_last_airborne_report_before_the_ground_within_the_radius() {
    let s = [
        st(0, 20_000.0, false),
        st(60, 3_000.0, false),
        st(100, 1_000.0, false),
        st(110, 500.0, true),
        st(500, 500.0, true),
        st(900, 500.0, false),
        st(1000, 30_000.0, false),
        st(1100, 30_000.0, true),
    ];
    let l = landings(&s, ORIGIN, &cfg());
    // Two airborne-to-ground changes: the first at the airport, the second 30 km away.
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].t_ms, 100_000);
    assert_eq!(l[0].gap_ms, 10_000);
    assert!(l[0].at_airport);
    assert!(!l[1].at_airport);
    assert_eq!(choose_landing(&l, 0, &cfg()), Ok(l[0]));
    // Issued after the landing: nothing at the airport is left.
    assert_eq!(
        choose_landing(&l, 200_000, &cfg()),
        Err(Exclusion::PassedBeforeIssue)
    );
    // A change far from the airport only.
    assert_eq!(
        choose_landing(&l[1..], 0, &cfg()),
        Err(Exclusion::NoLanding)
    );
    // A gap of five minutes to the ground report: not timed.
    let slow = [st(0, 1_000.0, false), st(300, 500.0, true)];
    let l = landings(&slow, ORIGIN, &cfg());
    assert_eq!(choose_landing(&l, 0, &cfg()), Err(Exclusion::LandingGap));
}

// REQ: OA-LTJ-006
#[test]
fn look_ahead_bins_are_half_open_and_out_of_range_is_excluded() {
    let m = |min: i64| min * 60_000;
    assert_eq!(BIN_COUNT, 6);
    assert_eq!(bin_of(0), Some(0));
    assert_eq!(bin_of(m(5) - 1), Some(0));
    assert_eq!(bin_of(m(5)), Some(1));
    assert_eq!(bin_of(m(10)), Some(2));
    assert_eq!(bin_of(m(20) - 1), Some(2));
    assert_eq!(bin_of(m(20)), Some(3));
    assert_eq!(bin_of(m(40)), Some(4));
    assert_eq!(bin_of(m(60)), Some(5));
    assert_eq!(bin_of(m(120) - 1), Some(5));
    assert_eq!(bin_of(m(120)), None);
    assert_eq!(bin_of(-1), None);
    assert_eq!(BIN_NAMES[3], "20 to 40 min");
    let c = cfg();
    assert_eq!(
        score(0, m(120), 0, &c),
        Err(Exclusion::LookAheadBeyondRange)
    );
    assert_eq!(score(0, 1000, 2000, &c), Err(Exclusion::PassedBeforeIssue));
    let s = score(0, m(15), 0, &c).unwrap();
    assert_eq!(s.bin, 2);
    assert_eq!(s.look_ahead_s, 900.0);
}

// REQ: OA-LTJ-007
#[test]
fn the_error_is_the_estimate_minus_the_actual_time() {
    let c = cfg();
    // Estimate 40 s after the actual crossing: positive. 25 s before it: negative.
    let late = score(1_040_000, 1_000_000, 700_000, &c).unwrap();
    assert_eq!(late.error_s, 40.0);
    assert_eq!(late.look_ahead_s, 300.0);
    let early = score(975_000, 1_000_000, 700_000, &c).unwrap();
    assert_eq!(early.error_s, -25.0);
    // The same sign through a crossing found on a path.
    let path = northbound(0, 10_000.0, 10_000.0, 10, 0.0);
    let a = approach(&path, ORIGIN, 2.0 * NM);
    let actual = choose_passage(&a, 0, &c).unwrap();
    assert!(score(actual + 30_000, actual, 0, &c).unwrap().error_s > 0.0);
    assert!(score(actual - 30_000, actual, 0, &c).unwrap().error_s < 0.0);
}

// REQ: OA-LTJ-008
// REQ: OA-LTJ-025
#[test]
fn statistics_have_n_median_mean_percentiles_and_rms_in_any_input_order() {
    assert!(stats(&[]).is_none());
    let v: Vec<f64> = (1..=101).map(f64::from).collect();
    let s = stats(&v).unwrap();
    assert_eq!(s.n, 101);
    assert_eq!(s.median, 51.0);
    assert_eq!(s.mean, 51.0);
    assert_eq!(s.p5, 6.0);
    assert_eq!(s.p95, 96.0);
    let mut rev = v.clone();
    rev.reverse();
    assert_eq!(stats(&rev), Some(s));
    // Signed values: the rms is of the values, not of their deviation from the mean.
    let s = stats(&[-3.0, 4.0]).unwrap();
    assert_eq!(s.n, 2);
    assert_eq!(s.mean, 0.5);
    // Nearest rank, as oa-bench and p002 define it: rank ceil(p n / 100), no interpolation.
    assert_eq!(s.median, -3.0);
    assert_eq!(s.p95, 4.0);
    assert!((s.rms - 12.5f64.sqrt()).abs() < 1e-12);
    let one = stats(&[7.0]).unwrap();
    assert_eq!((one.median, one.p5, one.p95, one.rms), (7.0, 7.0, 7.0, 7.0));
}

fn leg(addr: u32, start: i64, end: i64, cs: &[u8]) -> Leg {
    let mut callsign = [0u8; 8];
    callsign[..cs.len()].copy_from_slice(cs);
    Leg {
        addr,
        start_ms: start,
        end_ms: end,
        callsign,
    }
}

fn unit(addr: u32, cs: &[u8], from: i64, to: i64) -> Unit {
    let mut callsign = [0u8; 8];
    callsign[..cs.len()].copy_from_slice(cs);
    Unit {
        addr,
        addr_conflict: false,
        callsign,
        from_ms: from,
        to_ms: to,
    }
}

// REQ: OA-LTJ-009
#[test]
fn a_flight_with_an_address_joins_by_it_where_a_leg_overlaps() {
    let ix = index_legs(&[
        leg(7, 0, 100, b"AAA1"),
        leg(7, 500, 600, b"AAA1"),
        leg(8, 0, 1000, b"BBB2"),
    ]);
    assert_eq!(join(&ix, &unit(7, b"", 50, 60)), Outcome::ByAddress(7));
    assert_eq!(join(&ix, &unit(7, b"", 550, 900)), Outcome::ByAddress(7));
    // The address names an aircraft with no leg in the interval: no truth, and the callsign of
    // another aircraft is not tried.
    assert_eq!(join(&ix, &unit(7, b"BBB2", 200, 300)), Outcome::NoTruth);
    assert_eq!(join(&ix, &unit(99, b"", 0, 1000)), Outcome::NoTruth);
}

// REQ: OA-LTJ-010
#[test]
fn a_flight_without_an_address_joins_by_callsign_when_one_aircraft_overlaps() {
    let ix = index_legs(&[
        leg(1, 0, 100, b"UAL12"),
        leg(2, 500, 600, b"UAL12"),
        leg(3, 0, 1000, b"DAL9"),
        leg(4, 0, 1000, b"DAL9"),
    ]);
    // Two aircraft use the callsign, but only one is flying in the interval.
    assert_eq!(
        join(&ix, &unit(NO_ADDR, b"UAL12", 0, 200)),
        Outcome::ByCallsign(1)
    );
    assert_eq!(
        join(&ix, &unit(NO_ADDR, b"UAL12", 450, 700)),
        Outcome::ByCallsign(2)
    );
    // Both overlap: ambiguous. Neither overlaps: no truth. Nobody has the callsign: no truth.
    assert_eq!(
        join(&ix, &unit(NO_ADDR, b"UAL12", 0, 700)),
        Outcome::Ambiguous
    );
    assert_eq!(
        join(&ix, &unit(NO_ADDR, b"DAL9", 0, 700)),
        Outcome::Ambiguous
    );
    assert_eq!(
        join(&ix, &unit(NO_ADDR, b"UAL12", 200, 400)),
        Outcome::NoTruth
    );
    assert_eq!(join(&ix, &unit(NO_ADDR, b"SWA1", 0, 700)), Outcome::NoTruth);
}

// REQ: OA-LTJ-011
#[test]
fn a_flight_with_no_identity_or_conflicting_addresses_has_its_own_outcome() {
    let ix = index_legs(&[leg(1, 0, 100, b"UAL12")]);
    assert_eq!(join(&ix, &unit(NO_ADDR, b"", 0, 100)), Outcome::NoAddress);
    let mut u = unit(NO_ADDR, b"UAL12", 0, 100);
    u.addr_conflict = true;
    assert_eq!(join(&ix, &u), Outcome::Ambiguous);
}

// REQ: OA-LTJ-013
#[test]
fn the_report_is_deterministic_and_every_number_carries_its_n() {
    let c = cfg();
    let mut s = system_new("SYS", &["kind a"]);
    s.outcomes[0] = 2;
    s.outcomes[3] = 1;
    for (unit, err) in [(1u32, 10), (1, 20), (2, -30)] {
        let est = 1_000_000 + err * 1000;
        let r = score(est, 1_000_000, 1_000_000 - 7 * 60_000, &c);
        kind_add(&mut s.kinds[0], unit, r);
    }
    kind_add(&mut s.kinds[0], 3, Err(Exclusion::NotJoined));
    let header = vec![("input".to_string(), "x".to_string())];
    let text = render_text(&header, &c, &[], &[s.clone()]);
    assert_eq!(text, render_text(&header, &c, &[], &[s.clone()]));
    assert!(
        text.contains("5 to 10 min: n=3 flights=2 median=10.0 mean=0.0"),
        "{text}"
    );
    assert!(text.contains("flight not joined to an ADS-B aircraft: n=1"));
    assert!(text.contains("0 to 5 min: n=0"));
    assert!(text.contains("excluded in all: n=1; scored: n=3; offered: n=4"));
    assert!(text.contains("joined by address: n=2"));
    assert!(text.contains("all flights: n=3"));
    let csv = render_csv(&s);
    assert!(
        csv.starts_with("system,kind,bin,n,flights,median_s,mean_s,p5_s,p95_s,rms_s,over_1800s\n")
    );
    assert!(
        csv.contains("SYS,kind a,5 to 10 min,3,2,10.0,0.0,"),
        "{csv}"
    );
    assert!(csv.contains("SYS,kind a,0 to 5 min,0,0,,,,,,\n"));
    let counts = render_counts_csv(&s);
    assert!(counts.contains("SYS,estimates,kind a,4\n"));
    assert!(counts.contains("SYS,excluded kind a,flight not joined to an ADS-B aircraft,1\n"));
}

// REQ: OA-LTJ-022
#[test]
fn a_name_with_several_places_takes_the_one_nearest_the_route_and_far_apart_ones_are_refused() {
    use oa_legacy_traj::core::places::{classify, resolve, Resolution};
    let at = |lat: f64, lon: f64| Place {
        lat_deg: lat,
        lon_deg: lon,
    };
    let (east, west) = (at(41.0, -75.0), at(41.0, -80.0));
    // About 227 NM apart: the one nearer the route is taken, whichever is listed first.
    let route = [at(41.2, -79.0)];
    assert_eq!(resolve(&[east, west], &route), Resolution::Picked(west));
    assert_eq!(resolve(&[west, east], &route), Resolution::Picked(west));
    assert_eq!(classify(&[east, west]), 2);
    // No reference: none is taken.
    assert_eq!(resolve(&[east, west], &[]), Resolution::Refused);
    assert_eq!(resolve(&[], &route), Resolution::NotFound);
    assert_eq!(resolve(&[east], &[]), Resolution::Unique(east));
    // Over 500 NM apart (Pennsylvania and Oregon): refused however near the route.
    let far = at(45.0, -122.0);
    assert_eq!(resolve(&[east, far], &[far]), Resolution::Refused);
    assert_eq!(classify(&[east, far]), 3);
    assert_eq!(classify(&[]), 0);
    assert_eq!(classify(&[east]), 1);
}

// REQ: OA-LTJ-023
#[test]
fn the_error_report_splits_by_estimated_look_ahead_join_type_and_large_errors() {
    let c = cfg();
    let mut s = system_new("SYS", &["kind a"]);
    // Estimated 8 min after issue, actual 2 min after: actual look-ahead bin 0 to 5, estimated
    // bin 5 to 10, error +360 s.
    let issue = 10_000_000;
    let r = score(issue + 480_000, issue + 120_000, issue, &c);
    let x = oa_legacy_traj::core::report::Extra {
        gap_s: Some(4.0),
        alt: Some((300.0, 60.0)),
        by_callsign: Some(true),
    };
    oa_legacy_traj::core::report::kind_add_with(&mut s.kinds[0], 1, r, x);
    let r = score(issue + 100_000, issue + 130_000 + 2_000_000, issue, &c);
    let x = oa_legacy_traj::core::report::Extra {
        by_callsign: Some(false),
        ..Default::default()
    };
    oa_legacy_traj::core::report::kind_add_with(&mut s.kinds[0], 2, r, x);
    let k = &s.kinds[0];
    assert_eq!(k.errors[0], vec![360.0]);
    assert_eq!(k.errors_est[1], vec![360.0]);
    assert_eq!(k.errors_join[1], vec![360.0]);
    assert_eq!(k.errors_join[0].len(), 1);
    assert_eq!(k.errors_alt[0], vec![300.0]);
    assert_eq!(k.landing_minus_actual_s, vec![60.0]);
    assert_eq!(k.gaps_s, vec![4.0]);
    let text = render_text(&[], &c, &[], &[s]);
    assert!(text.contains("error by estimated look-ahead"), "{text}");
    assert!(text.contains("joined by callsign: n=1"), "{text}");
    assert!(text.contains("over 1800s: n=1 (100.0%)"), "{text}");
}

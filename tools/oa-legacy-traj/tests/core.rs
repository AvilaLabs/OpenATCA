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
use oa_legacy_traj::core::join::{
    all_legs_of, bind_leg, index_legs, join, leg_in_window, Leg, Outcome, Unit,
};
use oa_legacy_traj::core::report::{
    closest_add, group_of, kind_record, render_counts_csv, render_csv, render_text, system_new,
    Extra,
};
use oa_legacy_traj::core::types::{
    AirState, Config, Exclusion, LandingSet, PathPoint, Place, NO_ADDR, NO_ALT, NO_SPEED,
};

const M_PER_DEG: f64 = 111_194.926_644_558_74;
const NM: f64 = 1852.0;

fn cfg() -> Config {
    Config::new(2.0 * NM, 5.0 * NM, 30_000, 120 * 60_000)
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
    // Every passage before the issue: the last is kept (the event happened before the message).
    assert_eq!(choose_passage(&a, second + 1_000, &cfg()), Ok(second));
}

// REQ: OA-LTJ-026
#[test]
fn a_passage_in_progress_at_the_issue_is_chosen_by_its_closest_approach() {
    // One passage: the path is inside the 2 NM cut from about 63 s to 137 s, closest at 100 s.
    let path = northbound(0, 10_000.0, 10_000.0, 10, 0.0);
    let a = approach(&path, ORIGIN, 2.0 * NM);
    assert_eq!(a.passages.len(), 1);
    let t = a.passages[0].t_ms;
    // Issued while the aircraft is inside the cut and before the closest approach: that passage.
    assert_eq!(choose_passage(&a, t - 20_000, &cfg()), Ok(t));
    // Issued inside the cut after the closest approach: the event is before the issue, kept.
    assert_eq!(choose_passage(&a, t + 20_000, &cfg()), Ok(t));
    let kept = score(t + 60_000, t, t + 20_000, &cfg()).unwrap();
    assert!(kept.before_issue);
    assert_eq!(kept.error_s, 60.0);
}

fn st(t_s: i64, north_m: f64, ground: bool) -> AirState {
    st_with(t_s, north_m, ground, if ground { 0 } else { 300 }, NO_SPEED)
}

fn st_with(t_s: i64, north_m: f64, ground: bool, alt_ft: i32, gs_dkt: u16) -> AirState {
    AirState {
        t_ms: t_s * 1000,
        lat_deg: ORIGIN.lat_deg + north_m / M_PER_DEG,
        lon_deg: ORIGIN.lon_deg,
        ground,
        alt_ft,
        gs_dkt,
    }
}

// REQ: OA-LTJ-005
#[test]
fn the_landing_is_the_middle_of_the_gap_and_held_on_the_ground_within_the_radius() {
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
    let set = landings(&s, ORIGIN, Some(0.0), &cfg());
    // Two airborne-to-ground changes: the first at the airport, the second 30 km away.
    let l = &set.landings;
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].t_ms, 105_000);
    assert_eq!((l[0].t_air_ms, l[0].t_gnd_ms), (100_000, 110_000));
    assert_eq!(l[0].gap_ms, 10_000);
    assert!(l[0].at_airport);
    assert!(!l[1].at_airport);
    assert_eq!(choose_landing(&set, 0, &cfg()), Ok(l[0]));
    // Issued after the landing: the landing is before the issue and is kept.
    assert_eq!(choose_landing(&set, 200_000, &cfg()), Ok(l[0]));
    // A change far from the airport only: a landing elsewhere.
    let far = LandingSet {
        landings: l[1..].to_vec(),
        ..LandingSet::default()
    };
    assert_eq!(choose_landing(&far, 0, &cfg()), Err(Exclusion::Diversion));
    assert_eq!(
        choose_landing(&LandingSet::default(), 0, &cfg()),
        Err(Exclusion::NoLanding)
    );
    // A gap of five minutes to the ground report: not timed.
    let slow = [st(0, 1_000.0, false), st(300, 500.0, true)];
    let set = landings(&slow, ORIGIN, Some(0.0), &cfg());
    assert_eq!(choose_landing(&set, 0, &cfg()), Err(Exclusion::LandingGap));
}

// REQ: OA-LTJ-027
#[test]
fn a_touch_and_go_a_blip_and_a_high_fast_change_are_not_landings_and_are_counted() {
    // Ground at 10 s, airborne again at 20 s (touch and go); then a landing that holds.
    let s = [
        st(0, 1_000.0, false),
        st(10, 500.0, true),
        st(20, 800.0, false),
        st(30, 1_500.0, false),
        st(40, 600.0, true),
        st(100, 500.0, true),
    ];
    let set = landings(&s, ORIGIN, Some(0.0), &cfg());
    assert_eq!(set.refused_persist, 1);
    assert_eq!(set.landings.len(), 1);
    assert_eq!(set.landings[0].t_ms, 35_000);
    // Only the touch and go: not a landing.
    let tg = landings(&s[..4], ORIGIN, Some(0.0), &cfg());
    assert!(tg.landings.is_empty());
    assert_eq!(choose_landing(&tg, 0, &cfg()), Err(Exclusion::TouchAndGo));
    // The last airborne report of a change: 5,000 ft at 300 kt is neither low nor slow.
    let high = [
        st_with(0, 1_000.0, false, 5_000, 3_000),
        st_with(10, 500.0, true, 0, NO_SPEED),
    ];
    let h = landings(&high, ORIGIN, Some(0.0), &cfg());
    assert!(h.landings.is_empty());
    assert_eq!(h.refused_altitude, 1);
    // Slow (100 kt is 51 m/s): a landing though the altitude is high.
    let slow = [
        st_with(0, 1_000.0, false, 5_000, 1_000),
        st_with(10, 500.0, true, 0, NO_SPEED),
    ];
    assert_eq!(landings(&slow, ORIGIN, Some(0.0), &cfg()).landings.len(), 1);
    // Low above a high airport: 1,500 ft is under 1,000 ft above a 1,000 ft elevation.
    let low = [
        st_with(0, 1_000.0, false, 1_500, NO_SPEED),
        st_with(10, 500.0, true, 0, NO_SPEED),
    ];
    assert_eq!(
        landings(&low, ORIGIN, Some(1_000.0), &cfg()).landings.len(),
        1
    );
    assert!(landings(&low, ORIGIN, Some(0.0), &cfg())
        .landings
        .is_empty());
    // No altitude and no speed, or no elevation and no speed: the change is not trusted.
    let unknown = [
        st_with(0, 1_000.0, false, NO_ALT, NO_SPEED),
        st_with(10, 500.0, true, 0, NO_SPEED),
    ];
    assert!(landings(&unknown, ORIGIN, Some(0.0), &cfg())
        .landings
        .is_empty());
    assert!(landings(&low, ORIGIN, None, &cfg()).landings.is_empty());
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
    // The bin is that of the stated lead, the estimate minus the issue time.
    assert_eq!(
        score(m(120), 0, 0, &c),
        Err(Exclusion::LookAheadBeyondRange)
    );
    assert_eq!(
        score(1000, 0, 2000, &c),
        Err(Exclusion::EstimateBeforeIssue)
    );
    let s = score(m(15), m(14), 0, &c).unwrap();
    assert_eq!(s.bin, 2);
    assert_eq!(s.lead_s, 900.0);
    assert_eq!(s.act_bin, Some(2));
    assert!(!s.before_issue);
}

// REQ: OA-LTJ-028
#[test]
fn a_row_whose_event_was_before_the_issue_is_kept_and_counted_per_bin() {
    let c = cfg();
    let issue = 10_000_000;
    // Estimate 3 min after the issue, the event 1 min before it: error +240 s, kept, bin 0 to 5.
    let s = score(issue + 180_000, issue - 60_000, issue, &c).unwrap();
    assert_eq!(
        (s.error_s, s.bin, s.act_bin, s.before_issue),
        (240.0, 0, None, true)
    );
    let mut sys = system_new("SYS", &["k"]);
    kind_record(&mut sys.kinds[0], 1, 0, Ok(s), Extra::default());
    let g = &sys.kinds[0].groups[0];
    assert_eq!(g.errors[0], vec![240.0]);
    assert_eq!(g.before_issue[0], 1);
    // Not in the sensitivity by actual lead (no bin for a negative one).
    assert!(g.errors_act.iter().all(Vec::is_empty));
    assert_eq!(sys.kinds[0].exclusions.iter().sum::<u64>(), 0);
}

// REQ: OA-LTJ-007
#[test]
fn the_error_is_the_estimate_minus_the_actual_time() {
    let c = cfg();
    // Estimate 40 s after the actual crossing: positive. 25 s before it: negative.
    let late = score(1_040_000, 1_000_000, 700_000, &c).unwrap();
    assert_eq!(late.error_s, 40.0);
    assert_eq!(late.lead_s, 340.0);
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
        kind_record(&mut s.kinds[0], unit, 0, r, Extra::default());
    }
    kind_record(
        &mut s.kinds[0],
        3,
        0,
        Err(Exclusion::NotJoined),
        Extra::default(),
    );
    let header = vec![("input".to_string(), "x".to_string())];
    let text = render_text(&header, &c, &[], &[s.clone()]);
    assert_eq!(text, render_text(&header, &c, &[], &[s.clone()]));
    assert!(
        text.contains("5 to 10 min: n=3 flights=2 |e| median=20.0 p95=30.0 p99=30.0; bias median=10.0 mean=0.0"),
        "{text}"
    );
    assert!(text.contains("HEADLINE: address-joined, airborne at issue"));
    assert!(text.contains(
        "PRE-DEPARTURE: address-joined, not airborne at issue (includes departure delay)"
    ));
    assert!(text.contains("flight not joined to an ADS-B aircraft: n=1"));
    assert!(text.contains("0 to 5 min: n=0"));
    assert!(text.contains("excluded in all: n=1; scored: n=3; offered: n=4"));
    assert!(text.contains("joined by address: n=2"));
    assert!(text.contains("all flights: n=3"));
    let csv = render_csv(&s);
    assert!(csv.starts_with(
        "system,kind,block,bin,n,flights,abs_median_s,abs_p95_s,abs_p99_s,bias_median_s,bias_mean_s,p5_s,p95_s,rms_s,over_1800s\n"
    ));
    assert!(
        csv.contains("SYS,kind a,0,5 to 10 min,3,2,20.0,30.0,30.0,10.0,0.0,"),
        "{csv}"
    );
    assert!(csv.contains("SYS,kind a,0,0 to 5 min,0,0,,,,,,,,,\n"));
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
fn rows_are_split_by_join_and_airborne_state_and_the_headline_is_the_first_block() {
    let c = cfg();
    assert_eq!(group_of(false, true), 0);
    assert_eq!(group_of(false, false), 1);
    assert_eq!(group_of(true, true), 2);
    assert_eq!(group_of(true, false), 3);
    let mut s = system_new("SYS", &["kind a"]);
    let issue = 10_000_000;
    // Stated lead 8 min, actual 2 min after the issue: bin 5 to 10 by lead, 0 to 5 by actual.
    let r = score(issue + 480_000, issue + 120_000, issue, &c);
    let x = Extra {
        gap_s: Some(4.0),
        alt: Some((300.0, 60.0)),
        td: Some((365.0, 355.0)),
        issue_ms: issue,
    };
    kind_record(&mut s.kinds[0], 1, 0, r, x);
    // The same estimate in the other three blocks.
    for g in 1..4 {
        kind_record(&mut s.kinds[0], 1, g, r, Extra::default());
    }
    // A 2,000 s error in the callsign block.
    let r = score(issue + 100_000, issue + 2_100_000, issue, &c);
    kind_record(&mut s.kinds[0], 2, 2, r, Extra::default());
    let k = &s.kinds[0];
    assert_eq!(k.groups[0].errors[1], vec![360.0]);
    assert_eq!(k.groups[0].errors_act[0], vec![360.0]);
    assert_eq!(k.groups[1].errors[1], vec![360.0]);
    assert_eq!(k.groups[2].errors[0], vec![-2000.0]);
    // Diagnostics are of the headline block only.
    assert_eq!(k.errors_alt[1], vec![300.0]);
    assert_eq!(k.landing_minus_actual_s, vec![60.0]);
    assert_eq!(k.td_air[1], vec![365.0]);
    assert_eq!(k.gaps_s, vec![4.0]);
    let text = render_text(&[], &c, &[], &[s]);
    assert!(
        text.contains("sensitivity of the headline: binned by the actual time"),
        "{text}"
    );
    assert!(
        text.contains("CALLSIGN-JOINED, airborne at issue"),
        "{text}"
    );
    assert!(text.contains("over 1800s: n=1 (100.0%)"), "{text}");
}

// REQ: OA-LTJ-029
#[test]
fn a_message_binds_one_leg_and_a_turnaround_is_never_scored() {
    // One aircraft: leg 0 flies 0 to 1,000 s; leg 1 (the turnaround) flies 5,000 to 6,000 s; leg 2
    // flies 20,000 s on, past the window.
    let a = 0xa1;
    let legs = [
        leg(a, 0, 1_000_000, b"AAA1"),
        leg(a, 5_000_000, 6_000_000, b"AAA2"),
        leg(a, 20_000_000, 21_000_000, b"AAA3"),
    ];
    let ix = index_legs(&legs);
    let cands = all_legs_of(&ix, a);
    assert_eq!(cands, vec![0, 1, 2]);
    let w = 3 * 3_600_000;
    // In progress at the issue: that leg, whatever the airports.
    assert_eq!(bind_leg(&ix, &cands, &[false; 3], 500_000, w), Some(0));
    // Between legs: the first starting after, if its take-off airport is the departure airport.
    assert_eq!(bind_leg(&ix, &cands, &[true; 3], 2_000_000, w), Some(1));
    assert_eq!(
        bind_leg(&ix, &cands, &[true, false, true], 2_000_000, w),
        None
    );
    // Past the window (leg 2 starts 5 h later): no leg.
    assert_eq!(bind_leg(&ix, &cands, &[true; 3], 6_500_000, w), None);
    // After every leg: no leg.
    assert_eq!(bind_leg(&ix, &cands, &[true; 3], 30_000_000, w), None);
    // A flight whose messages are all before leg 0 and whose window reaches leg 1 only through
    // the rule never binds the turnaround leg 1 for messages inside leg 0: one leg per message.
    assert_eq!(bind_leg(&ix, &cands, &[true; 3], 900_000, w), Some(0));
    // The callsign window: a leg starting over 3 h after the first issue is refused.
    assert!(leg_in_window(&legs[1], 0, 6_000_000_000));
    assert!(!leg_in_window(&legs[2], 0, w));
    assert!(leg_in_window(&legs[0], 500_000, w));
    assert!(!leg_in_window(&legs[0], 2_000_000, w));
}

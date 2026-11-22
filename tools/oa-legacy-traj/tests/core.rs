//! The crossing search, the landing time, the bins, the statistics, the join rule and the report
//! text, over made-up paths and flights.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_legacy_traj::core::bins::{bin_of, nearest_rank, score, stats, BIN_COUNT, BIN_NAMES};
use oa_legacy_traj::core::cross::{
    airborne_span, approach, choose_landing, choose_passage, landings, near_box_edge,
};
use oa_legacy_traj::core::join::{
    all_legs_of, bind_leg, index_legs, join, leg_in_window, Known, Leg, LegFacts, Miss, Outcome,
    Refused, Unit,
};
use oa_legacy_traj::core::report::{
    abeam_add, airport_offer, airport_scored, closest_add, group_of, kind_record,
    render_counts_csv, render_csv, render_text, system_new, Extra,
};
use oa_legacy_traj::core::types::{
    AirState, Approach, Config, Exclusion, LandingSet, PathPoint, Place, NO_ADDR, NO_ALT, NO_SPEED,
};

const M_PER_DEG: f64 = 111_194.926_644_558_74;
const NM: f64 = 1852.0;

/// The time of the passage chosen.
fn chosen(a: &Approach, issue_ms: i64, cfg: &Config) -> Result<i64, Exclusion> {
    choose_passage(a, issue_ms, cfg).map(|p| p.t_ms)
}

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
    assert_eq!(chosen(&a, 1_000_000, &cfg()), Ok(p.t_ms));
}

// REQ: OA-LTJ-057
#[test]
fn a_dateline_segment_crosses_the_dateline_and_stays_far_from_greenwich() {
    for direction in [1.0, -1.0] {
        let path = [
            PathPoint {
                t_ms: 10_000,
                lat_deg: 0.0,
                lon_deg: direction * 179.98,
                gap_before: false,
            },
            PathPoint {
                t_ms: 30_000,
                lat_deg: 0.0,
                lon_deg: -direction * 179.98,
                gap_before: false,
            },
        ];
        let dateline = approach(
            &path,
            Place {
                lat_deg: 0.0,
                lon_deg: 180.0,
            },
            NM,
        );
        assert_eq!(dateline.passages.len(), 1);
        assert_eq!(dateline.passages[0].t_ms, 20_000);
        assert!(dateline.min_dist_m < 0.01);

        let greenwich = approach(
            &path,
            Place {
                lat_deg: 0.0,
                lon_deg: 0.0,
            },
            NM,
        );
        assert!(greenwich.passages.is_empty());
        assert!(greenwich.min_dist_m > 19_000_000.0);
    }
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
    assert_eq!(chosen(&far, 0, &cfg()), Err(Exclusion::BeyondCut));
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
    assert_eq!(chosen(&a, 0, &cfg()), Err(Exclusion::PassageAtEdge));
    // A hole over the origin: truth has no answer for 60 s around it.
    let mut hole = northbound(0, 10_000.0, 10_000.0, 10, 0.0);
    hole.retain(|p| p.t_ms < 80_000 || p.t_ms > 130_000);
    if let Some(p) = hole.iter_mut().find(|p| p.t_ms > 130_000) {
        p.gap_before = true;
    }
    let a = approach(&hole, ORIGIN, 2.0 * NM);
    assert!(a.passages.iter().all(|p| p.at_edge));
    assert_eq!(chosen(&a, 0, &cfg()), Err(Exclusion::PassageAtEdge));
    // A path that never gets near, and an empty path.
    let a = approach(&[], ORIGIN, 2.0 * NM);
    assert_eq!(chosen(&a, 0, &cfg()), Err(Exclusion::NoTruthPath));
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
    assert_eq!(chosen(&a, 0, &cfg()), Ok(first));
    assert_eq!(chosen(&a, first + 1_000, &cfg()), Ok(second));
    // Every passage before the issue: the last is kept (the event happened before the message).
    assert_eq!(chosen(&a, second + 1_000, &cfg()), Ok(second));
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
    assert_eq!(chosen(&a, t - 20_000, &cfg()), Ok(t));
    // Issued inside the cut after the closest approach: the event is before the issue, kept.
    assert_eq!(chosen(&a, t + 20_000, &cfg()), Ok(t));
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
    let actual = chosen(&a, 0, &c).unwrap();
    assert!(score(actual + 30_000, actual, 0, &c).unwrap().error_s > 0.0);
    assert!(score(actual - 30_000, actual, 0, &c).unwrap().error_s < 0.0);
}

// REQ: OA-LTJ-008, OA-LTJ-025
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
    // The rank rule is oa-bench's own, called directly: the same value for every length and
    // percentile.
    for n in 1..=250usize {
        let v: Vec<f64> = (0..n).map(|i| i as f64 * 1.5).collect();
        for p in [1u64, 5, 25, 50, 75, 95, 99, 100] {
            assert_eq!(
                nearest_rank(&v, p),
                oa_bench::stats::nearest_rank(&v, p),
                "n={n} p={p}"
            );
        }
    }
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
        text.contains("5 to 10 min: n=3 flights=2 |e| median=20.0 p95=n<100 p99=n<100; bias median=10.0 mean=0.0"),
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
        csv.contains("SYS,kind a,0,5 to 10 min,3,2,20.0,,,10.0,0.0,"),
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
        ..Extra::default()
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

/// Facts of a leg that flies from `from` to `to` (seconds).
fn facts(dep_ok: bool, arr_ok: bool, cs_ok: bool, from_s: i64, to_s: i64) -> LegFacts {
    LegFacts {
        dep_ok,
        arr_ok,
        dep_edge: false,
        arr_edge: false,
        cs_ok,
        air_from_ms: from_s * 1000,
        air_to_ms: to_s * 1000,
    }
}

const KNOWN: Known = Known {
    dep: true,
    arr: true,
};
const NONE_KNOWN: Known = Known {
    dep: false,
    arr: false,
};
const W: i64 = 3 * 3_600_000;

// REQ: OA-LTJ-029
#[test]
fn a_message_binds_one_leg_and_a_turnaround_is_never_scored() {
    // One aircraft: leg 0 flies 0 to 1,000 s (to its own flight's airports); leg 1 (the turnaround)
    // flies 5,000 to 6,000 s; leg 2 flies 20,000 s on, past the window.
    let a = 0xa1;
    let legs = [
        leg(a, 0, 1_000_000, b"AAA1"),
        leg(a, 5_000_000, 6_000_000, b"AAA2"),
        leg(a, 20_000_000, 21_000_000, b"AAA3"),
    ];
    let ix = index_legs(&legs);
    let cands = all_legs_of(&ix, a);
    assert_eq!(cands, vec![0, 1, 2]);
    // The flight departs where leg 1 took off and lands where leg 0 landed: it is leg 1's.
    let f = [
        facts(false, true, false, 10, 990),
        facts(true, false, false, 5_010, 5_990),
        facts(true, false, false, 20_010, 20_990),
    ];
    // Between the legs: the first of its own starting after, within the window.
    let b = bind_leg(&ix, &cands, &f, KNOWN, (2_000_000, W), false);
    assert_eq!((b.leg, b.refused), (Ok(1), None));
    // Past the window (leg 2 starts 5 h later, leg 1 has gone): too far, and counted as such.
    let b = bind_leg(&ix, &cands, &f, KNOWN, (6_500_000, W), false);
    assert_eq!(b.leg, Err(Miss::TooFar));
    // After every leg: no leg.
    let b = bind_leg(&ix, &cands, &f, KNOWN, (30_000_000, W), false);
    assert_eq!(b.leg, Err(Miss::NoLeg));
    // Leg 0 in progress and airborne: ADS-B says it lands at the flight's arrival airport, so it
    // is the flight's.
    let b = bind_leg(&ix, &cands, &f, KNOWN, (500_000, W), false);
    assert_eq!(b.leg, Ok(0));
    // The callsign window: a leg starting over 3 h after the first issue is refused.
    assert!(leg_in_window(&legs[1], 0, 6_000_000_000));
    assert!(!leg_in_window(&legs[2], 0, W));
    assert!(leg_in_window(&legs[0], 500_000, W));
    assert!(!leg_in_window(&legs[0], 2_000_000, W));
}

// REQ: OA-LTJ-035, OA-LTJ-051
#[test]
fn an_in_progress_leg_that_is_another_flights_is_refused_and_counted() {
    // The turnaround: the aircraft flew flight A into the airport (leg 0, 0 to 1,000 s) and flies
    // flight B out of it (leg 1, 5,000 to 6,000 s); one fix lies on both legs. A message of flight
    // B issued while the aircraft still flies A's leg must not bind it.
    let a = 0xa1;
    let legs = [
        leg(a, 0, 1_000_000, b"AAA1"),
        leg(a, 5_000_000, 6_000_000, b"AAA1"),
    ];
    let ix = index_legs(&legs);
    let cands = all_legs_of(&ix, a);
    // Flight B departs the airport leg 1 took off from; leg 0 took off elsewhere and landed there.
    let f = [
        facts(false, false, true, 10, 990),
        facts(true, false, true, 5_010, 5_990),
    ];
    let known = Known {
        dep: true,
        arr: false,
    };
    // Issued during leg 0: refused as not its own (it neither took off at B's departure airport
    // nor lands at an arrival airport of B), and the next leg binds.
    let b = bind_leg(&ix, &cands, &f, known, (500_000, W), false);
    assert_eq!((b.leg, b.refused), (Ok(1), Some(Refused::NotOwn)));
    // The same message when leg 1 is not B's either: no leg of its own.
    let g = [f[0], facts(false, false, true, 5_010, 5_990)];
    let b = bind_leg(&ix, &cands, &g, known, (500_000, W), false);
    assert_eq!(b.leg, Err(Miss::NotOwn));
    assert_eq!(b.refused, Some(Refused::NotOwn));
    // Arriving at an airport the flight lands at is also its own, even when it did not take off
    // at the departure airport (a flight that departed outside the clip).
    let known_both = Known {
        dep: true,
        arr: true,
    };
    let h = [facts(false, true, false, 10, 990), f[1]];
    let b = bind_leg(&ix, &cands, &h, known_both, (500_000, W), false);
    assert_eq!((b.leg, b.refused), (Ok(0), None));
    // Strictly, with both airports known, the departure must match too, or be out of the leg's
    // sight: the return leg of a shuttle (took off at the flight's arrival airport) is refused;
    // a leg that began at the clip box edge is not.
    let b = bind_leg(&ix, &cands, &h, known_both, (500_000, W), true);
    assert_eq!(b.refused, Some(Refused::NotOwn));
    let edge = [
        LegFacts {
            dep_edge: true,
            ..h[0]
        },
        h[1],
    ];
    let b = bind_leg(&ix, &cands, &edge, known_both, (500_000, W), true);
    assert_eq!((b.leg, b.refused), (Ok(0), None));
    // Both match: owned. Neither matches though both are out of sight: refused (nothing matches).
    let both = [facts(true, true, false, 10, 990), f[1]];
    let b = bind_leg(&ix, &cands, &both, known_both, (500_000, W), true);
    assert_eq!(b.leg, Ok(0));
    let blind = [
        LegFacts {
            dep_edge: true,
            arr_edge: true,
            ..facts(false, false, false, 10, 990)
        },
        f[1],
    ];
    let b = bind_leg(&ix, &cands, &blind, known_both, (500_000, W), true);
    assert_eq!(b.refused, Some(Refused::NotOwn));
    // With neither airport known, the callsign decides; without a match no leg binds, and the
    // next-leg branch needs the callsign too.
    let c = [
        facts(false, false, true, 10, 990),
        facts(false, false, false, 5_010, 5_990),
    ];
    let b = bind_leg(&ix, &cands, &c, NONE_KNOWN, (500_000, W), false);
    assert_eq!(b.leg, Ok(0));
    let d = [
        facts(false, false, false, 10, 990),
        facts(false, false, false, 5_010, 5_990),
    ];
    let b = bind_leg(&ix, &cands, &d, NONE_KNOWN, (500_000, W), false);
    assert_eq!(
        (b.leg, b.refused),
        (Err(Miss::NotOwn), Some(Refused::NotOwn))
    );
    // A departure airport that is not in NASR needs the callsign for the next leg.
    let e = [
        facts(true, false, false, 10, 990),
        facts(true, false, true, 5_010, 5_990),
    ];
    let unknown_dep = Known {
        dep: false,
        arr: false,
    };
    let b = bind_leg(&ix, &cands, &e, unknown_dep, (2_000_000, W), false);
    assert_eq!(b.leg, Ok(1));
    let e2 = [e[0], facts(true, false, false, 5_010, 5_990)];
    let b = bind_leg(&ix, &cands, &e2, unknown_dep, (2_000_000, W), false);
    assert_eq!(b.leg, Err(Miss::NotOwn));
}

// REQ: OA-LTJ-036
#[test]
fn a_leg_that_landed_before_the_issue_never_binds_as_in_progress() {
    // Taxi-in: the aircraft landed at 900 s and its leg runs to 1,000 s (reports on the ground).
    // A message at 950 s of the flight leaving next must not bind the leg just flown.
    let a = 0xa1;
    let legs = [
        leg(a, 0, 1_000_000, b"AAA1"),
        leg(a, 5_000_000, 6_000_000, b"AAA1"),
    ];
    let ix = index_legs(&legs);
    let cands = all_legs_of(&ix, a);
    let f = [
        facts(true, true, true, 10, 900),
        facts(true, false, true, 5_010, 5_990),
    ];
    let b = bind_leg(&ix, &cands, &f, KNOWN, (950_000, W), false);
    assert_eq!((b.leg, b.refused), (Ok(1), Some(Refused::Landed)));
    // With no next leg: no leg, not a binding of the taxi-in.
    let b = bind_leg(&ix, &cands[..1], &f[..1], KNOWN, (950_000, W), false);
    assert_eq!(
        (b.leg, b.refused),
        (Err(Miss::NoLeg), Some(Refused::Landed))
    );
    // Before the first airborne report the leg is not yet airborne: the flight's own next leg.
    let b = bind_leg(&ix, &cands[..1], &f[..1], KNOWN, (5_000, W), false);
    assert_eq!(b.leg, Ok(0));
    assert!(!f[0].airborne_at(5_000));
    assert!(f[0].airborne_at(10_000) && f[0].airborne_at(900_000));
    assert!(!f[0].airborne_at(901_000));
}

// REQ: OA-LTJ-037
#[test]
fn the_airborne_span_runs_from_the_first_airborne_report_to_the_landing_or_the_last_report() {
    let c = cfg();
    // Ground, ground, airborne to 100 s, ground from 104 s on: the span ends at the first ground
    // report of the landing.
    let mut v = vec![st(0, 0.0, true), st(10, 0.0, true)];
    v.extend((20..=100).step_by(10).map(|t| st(t, 1000.0, false)));
    v.push(st(104, 1500.0, true));
    v.push(st(120, 1500.0, true));
    assert_eq!(airborne_span(&v, &c), Some((20_000, 104_000)));
    // Persistence is measured from the last airborne report: an airborne report within 30 s of
    // it makes the change a blip, so the span runs on to the real landing.
    let mut w: Vec<AirState> = v[..11].to_vec();
    w.extend([
        st(104, 1500.0, true),
        st(110, 1500.0, false),
        st(200, 0.0, true),
    ]);
    assert_eq!(airborne_span(&w, &c), Some((20_000, 200_000)));
    // No landing: to the last report. No airborne report: none.
    let up: Vec<AirState> = (0..=5).map(|t| st(t * 10, 1000.0, false)).collect();
    assert_eq!(airborne_span(&up, &c), Some((0, 50_000)));
    assert_eq!(airborne_span(&[st(0, 0.0, true)], &c), None);
    assert_eq!(airborne_span(&[], &c), None);
}

// REQ: OA-LTJ-038
#[test]
fn the_landing_persistence_is_measured_from_the_last_airborne_report() {
    let c = cfg();
    let arp = ORIGIN;
    // Last airborne at 100 s, first ground at 110 s: an airborne report at 125 s is within 30 s
    // of the last airborne report (130 s), so it is not a landing; at 135 s it is not within it.
    let base = |t_back: i64| {
        vec![
            st_with(90, 0.0, false, 300, 500),
            st_with(100, 0.0, false, 300, 500),
            st_with(110, 0.0, true, 0, 500),
            st_with(t_back, 0.0, false, 300, 500),
            st_with(t_back + 5, 0.0, true, 0, 500),
        ]
    };
    let blip = landings(&base(125), arp, Some(0.0), &c);
    assert_eq!(blip.refused_persist, 1);
    assert!(blip.landings.iter().all(|l| l.t_air_ms != 100_000));
    let held = landings(&base(135), arp, Some(0.0), &c);
    assert!(
        held.landings.iter().any(|l| l.t_air_ms == 100_000),
        "{held:?}"
    );
}

// REQ: OA-LTJ-039
#[test]
fn a_passage_is_at_an_edge_only_when_its_closest_segment_or_a_neighbour_touches_one() {
    // A path from 4,000 m south to 4,000 m north in 5 s steps at 100 m/s: the point is at 0 m,
    // the cut 2,000 m: the closest approach is in the middle, far from the ends.
    let c = cfg();
    let place = ORIGIN;
    let mid = northbound(0, 4_000.0, 4_000.0, 5, 0.0);
    let a = approach(&mid, place, 2_000.0);
    assert!(!a.passages[0].at_edge);
    // A path that starts within the cut and ends in it too, but whose closest approach is in
    // the middle: not at an edge (the ends are not next to the closest segment).
    let short = northbound(0, 1_500.0, 1_500.0, 5, 0.0);
    let a = approach(&short, place, 2_000.0);
    assert!(!a.passages[0].at_edge, "{a:?}");
    // The closest approach is at the last segment: at an edge.
    let ends = northbound(0, 1_500.0, 100.0, 5, 0.0);
    let a = approach(&ends, place, 2_000.0);
    assert!(a.passages[0].at_edge);
    assert_eq!(chosen(&a, 0, &c), Err(Exclusion::PassageAtEdge));
}

// REQ: OA-LTJ-040
#[test]
fn the_report_counts_offered_no_leg_and_scored_per_bin_and_the_landing_exclusions_by_leg_end() {
    let c = cfg();
    let mut s = system_new("SYS", &["eta"]);
    let k = &mut s.kinds[0];
    let issue = 10_000_000;
    // Offered in bin 2 (10 to 20 min): one scored, two with no leg (one the flight's own absent,
    // one the departure past the window), one landing exclusion at the box edge, one inside.
    let ok = score(issue + 900_000, issue + 960_000, issue, &c);
    kind_record(k, 1, 0, ok, Extra::default());
    let at = |lead_bin, end_at_edge| Extra {
        lead_bin,
        end_at_edge,
        ..Extra::default()
    };
    kind_record(k, 1, 0, Err(Exclusion::LegNotOwn), at(Some(2), false));
    kind_record(k, 1, 0, Err(Exclusion::DepartureTooFar), at(Some(2), false));
    kind_record(k, 1, 0, Err(Exclusion::NoLanding), at(Some(2), true));
    kind_record(k, 1, 0, Err(Exclusion::Diversion), at(Some(2), false));
    kind_record(k, 1, 0, Err(Exclusion::NoLeg), at(None, false));
    assert_eq!(k.offered_bin[2], 5);
    assert_eq!(k.offered_bin[BIN_COUNT], 1);
    assert_eq!(
        (k.not_own_bin[2], k.too_far_bin[2], k.no_leg_bin[BIN_COUNT]),
        (1, 1, 1)
    );
    airport_offer(k, "KORD");
    airport_offer(k, "KORD");
    airport_scored(k, "KORD");
    let text = render_text(&[], &c, &[], &[s]);
    assert!(
        text.contains("10 to 20 min: offered=5 no leg=2 (no leg of the flight's own=1; departure past the window=1) scored=1"),
        "{text}"
    );
    assert!(
        text.contains("no stated lead in range: offered=1 no leg=1"),
        "{text}"
    );
    assert!(
        text.contains("(bound leg ends inside the box: 0; at its edge: 1)"),
        "{text}"
    );
    assert!(
        text.contains("(bound leg ends inside the box: 1; at its edge: 0)"),
        "{text}"
    );
    assert!(
        text.contains("KORD: offered=2 scored=1 share=50.0%"),
        "{text}"
    );
    assert!(
        text.contains("bound leg has no ground report") || !text.contains("no ground coverage"),
        "{text}"
    );
}

// REQ: OA-LTJ-041, OA-LTJ-046
#[test]
fn tail_percentiles_are_printed_only_from_one_hundred_values_and_every_kind_has_a_last_message_row()
{
    let c = cfg();
    let mut s = system_new("SYS", &["eta", "sta_sfx"]);
    let issue = 10_000_000;
    // 99 rows: no p95 or p99. Two flights; flight 1 has two messages in the bin: only the last
    // counts in the one-row-per-flight table.
    for i in 0..99 {
        let r = score(issue + 480_000, issue + 480_000 - i * 1000, issue, &c);
        kind_record(
            &mut s.kinds[0],
            i as u32 % 2,
            0,
            r,
            Extra {
                issue_ms: issue,
                ..Extra::default()
            },
        );
    }
    let r = score(issue + 480_000, issue + 400_000, issue, &c);
    kind_record(
        &mut s.kinds[0],
        1,
        0,
        r,
        Extra {
            issue_ms: issue + 5_000,
            ..Extra::default()
        },
    );
    let k = &s.kinds[0];
    // Flight 0's last message is the issue one (49 or 50 rows), flight 1's is the later one.
    let last = &k.last_in_bin[&(1, 1)];
    assert_eq!(last.0, issue + 5_000);
    assert_eq!(last.1, vec![80.0]);
    let text = render_text(&[], &c, &[], &[s.clone()]);
    assert!(
        text.contains("5 to 10 min: n=100 flights=2 |e| median="),
        "{text}"
    );
    assert!(
        text.contains("one message per flight and stated-lead bin"),
        "{text}"
    );
    assert!(
        text.contains("schedule target (metering), not a forecast"),
        "{text}"
    );
    // 100 rows: tails printed; 99: not.
    let mut few = system_new("SYS", &["eta"]);
    for i in 0..99 {
        let r = score(issue + 480_000, issue + 480_000 - i * 1000, issue, &c);
        kind_record(&mut few.kinds[0], 0, 0, r, Extra::default());
    }
    let t99 = render_text(&[], &c, &[], &[few.clone()]);
    assert!(
        t99.contains("5 to 10 min: n=99 flights=1 |e| median=49.0 p95=n<100 p99=n<100"),
        "{t99}"
    );
    assert!(
        render_csv(&few).contains(",5 to 10 min,99,1,49.0,,,"),
        "{}",
        render_csv(&few)
    );
    let r = score(issue + 480_000, issue + 380_000, issue, &c);
    kind_record(&mut few.kinds[0], 0, 0, r, Extra::default());
    let t100 = render_text(&[], &c, &[], &[few]);
    assert!(!t100.contains("n<100"), "{t100}");
    // The abeam table.
    let mut ab = system_new("SYS", &["eta"]);
    let r = score(issue + 480_000, issue + 400_000, issue, &c).unwrap();
    abeam_add(&mut ab.kinds[0], &r);
    let ta = render_text(&[], &c, &[], &[ab]);
    assert!(ta.contains("crossings abeam"), "{ta}");
}

// REQ: OA-LTJ-042
#[test]
fn the_box_edge_is_within_the_margin_of_any_side() {
    let b = [38.5, 46.5, -93.5, -83.5];
    let m = 10.0 * NM;
    let at = |lat, lon| {
        near_box_edge(
            Place {
                lat_deg: lat,
                lon_deg: lon,
            },
            b,
            m,
        )
    };
    assert!(!at(42.0, -88.0));
    // 5 NM from the east edge: at the edge; 20 NM: not.
    assert!(at(42.0, -83.5 - 5.0 / 60.0 / 0.74));
    assert!(!at(42.0, -83.5 - 20.0 / 60.0 / 0.74));
    assert!(at(38.5 + 5.0 / 60.0, -88.0));
    assert!(at(46.5, -88.0));
    assert!(at(42.0, -93.6));
}

/// A path through the given (seconds, metres east, metres north of the origin) points, all joined.
fn path_through(pts: &[(i64, f64, f64)]) -> Vec<PathPoint> {
    let cos_lat = (ORIGIN.lat_deg * std::f64::consts::PI / 180.0).cos();
    pts.iter()
        .map(|&(t, e, n)| PathPoint {
            t_ms: t * 1000,
            lat_deg: ORIGIN.lat_deg + n / M_PER_DEG,
            lon_deg: ORIGIN.lon_deg + e / (M_PER_DEG * cos_lat),
            gap_before: false,
        })
        .collect()
}

// REQ: OA-LTJ-048
#[test]
fn the_closest_passage_is_chosen_once_and_every_cut_scores_that_passage_or_none() {
    // An early passage 4 NM away that starts the path (at an edge), then a later one 0.8 NM away.
    let p = path_through(&[
        (0, 4.0 * NM, 0.0),
        (10, 6.0 * NM, 0.0),
        (200, 1500.0, -20_000.0),
        (300, 1500.0, 0.0),
        (400, 1500.0, 20_000.0),
        (500, 1500.0, 40_000.0),
    ]);
    let widest = approach(&p, ORIGIN, 5.0 * NM);
    assert_eq!(widest.passages.len(), 2);
    let cuts = [0.5, 1.0, 2.0, 5.0];
    let at = |issue_s: i64| -> Vec<Result<i64, Exclusion>> {
        cuts.iter()
            .map(|c| {
                let cfg = Config {
                    cut_m: c * NM,
                    ..cfg()
                };
                choose_passage(&widest, issue_s * 1000, &cfg).map(|x| x.t_ms)
            })
            .collect()
    };
    // The closest passage not before the issue is chosen: the 0.8 NM one, whether the issue is at
    // the start or after the early one. From 1 NM up every cut scores it, at the same time; no cut
    // scores the early one.
    for issue in [0, 100] {
        let r = at(issue);
        assert_eq!(r[0], Err(Exclusion::BeyondCut), "issue {issue}");
        assert!(
            r[1..].iter().all(|x| *x == Ok(300_000)),
            "issue {issue}: {r:?}"
        );
    }
    // Issued after both passages in time (the later one at 300 s): the last passage before it.
    assert_eq!(at(350)[3], Ok(300_000));
    // Nesting: a cut that scores scores at every larger cut, for every issue time.
    for issue in (0..=400).step_by(20) {
        let r = at(issue);
        for w in r.windows(2) {
            assert!(w[0].is_err() || w[0] == w[1], "issue {issue}: {r:?}");
        }
    }
}

/// A racetrack: one-minute legs about 4 by 2 NM, the fix overflown on each northbound leg at the
/// given offsets east of it (metres), entered and left by two far segments each.
fn racetrack(offsets_m: &[f64]) -> Vec<PathPoint> {
    let (half, wide) = (3_704.0, 3_704.0);
    let mut pts: Vec<(i64, f64, f64)> = vec![(-120, 0.0, -40_000.0), (-60, 0.0, -20_000.0)];
    let mut t = 0;
    for (i, &e) in offsets_m.iter().enumerate() {
        pts.push((t, e, -half));
        pts.push((t + 60, e, half));
        pts.push((t + 90, wide, half));
        pts.push((t + 150, wide, -half));
        t += 180;
        if i + 1 == offsets_m.len() {
            pts.push((t, 0.0, -20_000.0));
            pts.push((t + 60, 0.0, -40_000.0));
            pts.push((t + 120, 0.0, -60_000.0));
        }
    }
    path_through(&pts)
}

// REQ: OA-LTJ-048, OA-LTJ-049
#[test]
fn a_racetrack_hold_is_one_passage_per_loop_and_the_first_crossing_is_chosen() {
    // Three loops inside the 5 NM cut, overflying the fix at 0.2, 0.1 and 0.3 NM.
    let p = racetrack(&[370.0, 185.0, 556.0]);
    let widest = approach(&p, ORIGIN, 5.0 * NM);
    assert_eq!(widest.passages.len(), 3, "{:?}", widest.passages);
    let times: Vec<i64> = widest.passages.iter().map(|x| x.t_ms).collect();
    assert_eq!(times, vec![30_000, 210_000, 390_000]);
    assert!(widest.passages.iter().all(|x| !x.at_edge));
    for cut in [0.5, 1.0, 2.0, 5.0] {
        let cfg = Config {
            cut_m: cut * NM,
            ..cfg()
        };
        // The first loop is chosen, though the second is closer.
        let c = choose_passage(&widest, 0, &cfg).unwrap();
        assert_eq!(c.t_ms, 30_000, "cut {cut}");
        assert!((c.dist_m - 370.0).abs() < 2.0);
    }
    // Issued after the first loop's pass: the second, the first within 0.5 NM that remains.
    assert_eq!(
        choose_passage(&widest, 100_000, &cfg()).unwrap().t_ms,
        210_000
    );
    // Issued after every pass: the last.
    assert_eq!(
        choose_passage(&widest, 1_000_000, &cfg()).unwrap().t_ms,
        390_000
    );
    // None within 0.5 NM: the closest is chosen, the earliest on a tie.
    let far = racetrack(&[1_500.0, 1_200.0, 1_500.0]);
    let a = approach(&far, ORIGIN, 5.0 * NM);
    assert_eq!(a.passages.len(), 3);
    assert_eq!(choose_passage(&a, 0, &cfg()).unwrap().t_ms, 210_000);
    let tie = racetrack(&[1_500.0, 1_500.0, 1_500.0]);
    let a = approach(&tie, ORIGIN, 5.0 * NM);
    assert_eq!(choose_passage(&a, 0, &cfg()).unwrap().t_ms, 30_000);
}

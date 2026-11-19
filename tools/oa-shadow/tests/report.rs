//! Adjudication, the report's bytes and digest, the thresholds table, the loader's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_adsb::{reports_push, Reports, ROW_EMPTY};
use oa_sha256::{sha256, to_hex};
use oa_shadow::compare::{compare_all, Class, Comparison, TruthAt};
use oa_shadow::geo::{deg, Closer};
use oa_shadow::model::EramFlight;
use oa_shadow::report::{largest_divergences, render};
use oa_shadow::run::{ident_points, in_box};
use oa_shadow::thresholds::THRESHOLDS;

fn world() -> (
    Vec<EramFlight>,
    std::collections::BTreeMap<u32, oa_shadow::model::OaAircraft>,
) {
    // 1: east 1000 m (cross over 0.5 NM) on four of nine reports; 2: clean; 3: unpaired both.
    let mut a: Vec<_> = (1..=9)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
        .collect();
    for s in a.iter_mut().take(4) {
        *s = eram_at(
            (s.t_ms - T0) / 1000,
            ((s.t_ms - T0) / 1000) as f64,
            100.0,
            1000.0,
            0.0,
        );
    }
    let b: Vec<_> = (1..=9)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, 0.0, 0.0))
        .collect();
    let mut f1 = flight(Some(1), a);
    f1.gufi = 5;
    f1.callsign = Some(cs8("ABC1"));
    let mut f2 = flight(Some(2), b);
    f2.gufi = 3;
    let f3 = flight(Some(9), vec![eram_at(12, 12.0, 100.0, 0.0, 0.0)]);
    (
        vec![f1, f2, f3],
        oa_map(vec![
            oa_north(1, 0, 120, 100.0),
            oa_north(2, 0, 120, 100.0),
            oa_north(4, 0, 50, 100.0),
        ]),
    )
}

// REQ: OA-SHD-011
#[test]
fn adjudication_names_the_closer_system_and_never_changes_a_class() {
    let (eram, oa) = world();
    // Truth on the OpenATCA path: OpenATCA is closer to it than ERAM's 1000 m offset.
    let on_oa = |_: u32, t_ms: i64| {
        let (lat, lon) = displaced(0.0, 100.0 * (t_ms - T0) as f64 / 1000.0);
        Some((deg(lat), deg(lon)))
    };
    // Truth 1000 m east: ERAM is closer.
    let on_eram = |_: u32, t_ms: i64| {
        let (lat, lon) = displaced(1000.0, 100.0 * (t_ms - T0) as f64 / 1000.0);
        Some((deg(lat), deg(lon)))
    };
    // Truth halfway: a tie.
    let half = |_: u32, t_ms: i64| {
        let (lat, lon) = displaced(500.0, 100.0 * (t_ms - T0) as f64 / 1000.0);
        Some((deg(lat), deg(lon)))
    };
    let none = |_: u32, _: i64| None;
    let plain = compare_all(&eram, &oa, None);
    // Compared as text: absent values are NaN, which is not equal to itself.
    let strip = |c: &Comparison| -> Vec<String> {
        c.events
            .iter()
            .map(|e| {
                let mut e = e.clone();
                e.closer = Closer::NoTruth;
                format!("{e:?}")
            })
            .collect()
    };
    for (truth, want) in [
        (&on_oa as TruthAt<'_>, Closer::OpenAtca),
        (&on_eram, Closer::Eram),
        (&half, Closer::Tie),
        (&none, Closer::NoTruth),
    ] {
        let c = compare_all(&eram, &oa, Some(truth));
        // Same events, same order, same values; only the adjudication column differs.
        assert_eq!(strip(&c), strip(&plain));
        assert_eq!(format!("{:?}", c.flights), format!("{:?}", plain.flights));
        let pos: Vec<_> = c.events.iter().filter(|e| e.class.is_position()).collect();
        assert_eq!(pos.len(), 4);
        assert!(pos.iter().all(|e| e.closer == want), "{want:?}");
        // Events that are not positions carry no adjudication.
        assert!(c
            .events
            .iter()
            .filter(|e| !e.class.is_position())
            .all(|e| e.closer == Closer::NoTruth));
    }
    assert!(plain.events.iter().all(|e| e.closer == Closer::NoTruth));
}

// REQ: OA-SHD-011
#[test]
fn both_distances_are_to_the_truth_at_the_eram_time_with_no_time_credit_for_eram() {
    // Truth is the OpenATCA path. ERAM sits 1000 m east and 100 m ahead (one second at 100 m/s).
    // Judged at the report time ERAM is 1005 m from the truth and OpenATCA 0 m.
    let oa = oa_map(vec![oa_north(1, 0, 40, 100.0)]);
    let truth = |_: u32, t_ms: i64| {
        let (lat, lon) = displaced(0.0, 100.0 * (t_ms - T0) as f64 / 1000.0);
        Some((deg(lat), deg(lon)))
    };
    let s = eram_at(24, 24.0, 100.0, 1000.0, 100.0);
    let c = compare_all(&[flight(Some(1), vec![s])], &oa, Some(&truth));
    assert_eq!(of(&c, Class::PosCross05)[0].closer, Closer::OpenAtca);
    // ERAM 100 m ahead of the truth path with OpenATCA 150 m east of it: along-track credit does
    // not apply to the adjudication, so ERAM (100 m) is closer than OpenATCA (150 m) only by raw
    // distance, and with OpenATCA on the path and ERAM ahead OpenATCA is closer.
    let c = compare_all(
        &[flight(Some(1), vec![eram_at(24, 24.0, 100.0, 1000.0, 0.0)])],
        &oa,
        Some(&truth),
    );
    assert_eq!(of(&c, Class::PosCross05)[0].closer, Closer::OpenAtca);
}

// REQ: OA-SHD-011
#[test]
fn a_truth_query_with_no_answer_is_counted_in_the_summary() {
    let (eram, oa) = world();
    let none = |_: u32, _: i64| None;
    let c = compare_all(&eram, &oa, Some(&none));
    let r = render(&c, &[]);
    assert!(
        r.summary
            .contains("position events 4; truth query gave no answer for 4"),
        "{}",
        r.summary
    );
    assert!(r.summary.contains("Adjudication: distance of each system to the ADS-B truth at the ERAM report time. The truth is built from ADS-B reports (set B) of the same aircraft whose other reports (set A) feed OpenATCA, so their errors are correlated and this column favours OpenATCA; the count of events where ERAM is closer is the more informative one."));
}

// REQ: OA-SHD-012
#[test]
fn a_value_that_rounds_to_zero_is_written_without_a_sign() {
    let (eram, oa) = world();
    let mut c = compare_all(&eram, &oa, None);
    c.events[0].cross_m = -0.04;
    c.events[0].along_m = -0.0;
    let csv = oa_shadow::report::events_csv(&c);
    let row = csv.lines().nth(1).unwrap();
    assert!(!row.contains("-0.0"), "{row}");
    assert!(row.contains(",0.0,"), "{row}");
}

// REQ: OA-SHD-012
#[test]
fn two_runs_give_the_same_bytes_and_the_digest_line_matches_them() {
    let (eram, oa) = world();
    let header = vec!["input sha256 00".to_string()];
    let a = render(&compare_all(&eram, &oa, None), &header);
    let b = render(&compare_all(&eram, &oa, None), &header);
    assert_eq!(a, b);
    assert_eq!(a.events_csv.as_bytes(), b.events_csv.as_bytes());
    // The digest covers the three texts as written.
    let body = a.summary.rsplit_once("digest sha256:").unwrap().0;
    let mut d = Vec::new();
    d.extend_from_slice(a.events_csv.as_bytes());
    d.extend_from_slice(a.flights_csv.as_bytes());
    d.extend_from_slice(body.as_bytes());
    assert_eq!(a.digest, to_hex(&sha256(&d)));
    assert!(a
        .summary
        .lines()
        .last()
        .unwrap()
        .starts_with(&format!("digest sha256:{}", a.digest)));
    // A change in the data changes the digest.
    let (mut eram2, oa2) = world();
    eram2[1].samples[0].lat_e7 += 100_000;
    assert_ne!(
        a.digest,
        render(&compare_all(&eram2, &oa2, None), &header).digest
    );
    // The input order of the OpenATCA map is by address, whatever order it was built in.
    let (eram3, _) = world();
    let oa3 = oa_map(vec![
        oa_north(4, 0, 50, 100.0),
        oa_north(2, 0, 120, 100.0),
        oa_north(1, 0, 120, 100.0),
    ]);
    assert_eq!(
        a.digest,
        render(&compare_all(&eram3, &oa3, None), &header).digest
    );
}

// REQ: OA-SHD-012
#[test]
fn the_summary_counts_each_class_and_names_no_aircraft_or_path() {
    let (eram, oa) = world();
    let r = render(
        &compare_all(&eram, &oa, None),
        &["clip sha256 ab".to_string()],
    );
    assert!(r.summary.contains("clip sha256 ab"));
    for class in oa_shadow::compare::CLASSES {
        assert!(r.summary.contains(class.name()), "{}", class.name());
    }
    let line = |name: &str| {
        r.summary
            .lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap()
            .to_string()
    };
    assert!(
        line("POS_CROSS_0P5_TO_1NM").contains("events        4"),
        "{}",
        line("POS_CROSS_0P5_TO_1NM")
    );
    assert!(line("UNPAIRED_ERAM").contains("events        1"));
    assert!(line("UNPAIRED_OA").contains("events        1"));
    // No aircraft address and no callsign in the summary; the events file has the address.
    assert!(r.summary.contains("Coast flags are not the same concept: ERAM's is a message indicator in SFDPS, OpenATCA's is the tracker's own state when no measurement arrived. Compared only at OpenATCA rows at the exact ERAM report time."));
    assert!(r.summary.contains("median ERAM report interval"));
    assert!(!r.summary.contains("ABC1") && !r.summary.contains(",000001,"));
    assert!(r.events_csv.contains(",000001,"));
    assert!(
        !r.summary.contains("/home") && !r.summary.contains("/tmp") && !r.summary.contains(".oaf"),
        "no paths"
    );
    // The largest divergences are listed by flight index.
    let big = largest_divergences(&compare_all(&eram, &oa, None).events, 10);
    assert_eq!(big.len(), 4);
    assert!(big.windows(2).all(|w| w[0].0 >= w[1].0));
}

// REQ: OA-SHD-013
#[test]
fn every_threshold_is_in_the_table_and_printed_in_the_summary() {
    let t = THRESHOLDS;
    assert_eq!(t.pos_m, [926.0, 1852.0]);
    assert_eq!(
        (t.alt_ft, t.timing_ms, t.segment_gap_ms),
        (300.0, 12_000, 30_000)
    );
    let r = render(&compare_all(&[], &oa_map(vec![]), None), &[]);
    for l in t.lines() {
        assert!(r.summary.contains(&l), "{l}");
    }
    assert!(r.summary.contains("0.5 to 1.0 NM and > 1.0 NM"));
    assert!(r.summary.contains("> 300 ft"));
    assert!(r.summary.contains("> 12 s plus the ERAM report interval"));
}

// REQ: OA-SHD-014
#[test]
fn per_flight_rows_hold_the_worst_cases_and_a_flight_index() {
    let (eram, oa) = world();
    let c = compare_all(&eram, &oa, None);
    let r = render(&c, &[]);
    assert_eq!(c.flights.len(), 4); // two groups, one unpaired each side
    let f0 = &c.flights[0];
    assert_eq!(
        (f0.kind, f0.addr, f0.eram_samples, f0.compared),
        ("paired", Some(1), 9, 9)
    );
    assert!((f0.worst_cross_m - 1000.0).abs() < 2.0);
    assert!(f0.worst_along_m < 1.0);
    assert_eq!(f0.events, 4);
    assert_eq!(r.flights_csv.lines().count(), 5);
    assert!(r
        .flights_csv
        .lines()
        .nth(1)
        .unwrap()
        .starts_with("0,000001,paired,address,9,9,99"));
}

// REQ: OA-SHD-015
#[test]
fn both_systems_are_restricted_to_the_clip_box_and_ident_points_carry_values_forward() {
    assert!(in_box(420_000_000, -880_000_000));
    assert!(in_box(385_000_000, -935_000_000));
    assert!(in_box(465_000_000, -835_000_000));
    assert!(!in_box(384_999_999, -880_000_000));
    assert!(!in_box(420_000_000, -935_000_001));
    assert!(!in_box(465_000_001, -880_000_000));
    assert!(!in_box(420_000_000, -834_999_999));
    let mut r = Reports::default();
    let row = |t: i64, icao: u32, sq: u16, cs: &[u8; 8]| {
        let mut x = ROW_EMPTY;
        x.time_ns = t * 1_000_000;
        x.icao = icao;
        x.squawk = sq;
        x.callsign = *cs;
        x
    };
    let none = oa_adsb::NONE_U16;
    // Out of time order on purpose; address 2 first.
    for x in [
        row(3000, 1, none, b"        "),
        row(1000, 1, 1200, b"        "),
        row(2000, 1, none, b"ABC     "),
        row(4000, 1, 1200, b"ABC     "),
        row(5000, 1, 7700, b"ABC     "),
        row(1000, 2, none, &[0; 8]),
    ] {
        reports_push(&mut r, &x);
    }
    let p = ident_points(&r);
    let one = &p[&1];
    // (1000: sq 1200), (2000: callsign joins), then 7700 at 5000; 3000 and 4000 change nothing.
    assert_eq!(one.len(), 3);
    assert_eq!(
        (one[0].t_ms, one[0].squawk, one[0].callsign),
        (1000, Some(1200), None)
    );
    assert_eq!((one[1].t_ms, one[1].squawk), (2000, Some(1200)));
    assert_eq!(one[1].callsign.unwrap(), *b"ABC     ");
    assert_eq!((one[2].t_ms, one[2].squawk), (5000, Some(7700)));
    // An address that never gave either has one empty point.
    assert_eq!(p[&2].len(), 1);
    assert_eq!((p[&2][0].squawk, p[&2][0].callsign), (None, None));
}

// REQ: OA-SHD-018
#[test]
fn both_sides_are_cut_to_the_overlap_of_the_two_recordings() {
    use oa_shadow::window::{overlap, restrict_eram, restrict_oa, utc_text};
    assert_eq!(overlap((0, 10), (5, 20)), Some((5, 10)));
    assert_eq!(overlap((0, 10), (10, 20)), Some((10, 10)));
    assert_eq!(overlap((0, 10), (11, 20)), None);
    assert_eq!(overlap((5, 6), (0, 100)), Some((5, 6)));
    let mut eram = vec![
        flight(
            Some(1),
            (0..=10)
                .map(|k| eram_at(k * 12, 0.0, 100.0, 0.0, 0.0))
                .collect(),
        ),
        flight(Some(2), vec![eram_at(500, 0.0, 100.0, 0.0, 0.0)]),
    ];
    let mut oa = oa_map(vec![
        oa_north(1, 0, 100, 100.0),
        oa_north(2, 300, 400, 100.0),
    ]);
    let (lo, hi) = (T0 + 24_000, T0 + 60_000);
    restrict_eram(&mut eram, lo, hi);
    restrict_oa(&mut oa, lo, hi);
    // Edges are inclusive; a flight or aircraft left with nothing is gone.
    assert_eq!(eram.len(), 1);
    assert_eq!(eram[0].samples.first().unwrap().t_ms, lo);
    assert_eq!(eram[0].samples.last().unwrap().t_ms, hi);
    assert_eq!(oa.len(), 1);
    assert_eq!(oa[&1].samples.len(), 37);
    // The same hours of a system outside the other's recording are not "missing".
    let c = compare_all(&eram, &oa, None);
    assert_eq!(
        count(&c, Class::UnpairedEram) + count(&c, Class::UnpairedOa),
        0
    );
    assert_eq!(utc_text(0), "1970-01-01 00:00:00");
    assert_eq!(
        utc_text(1_794_700_800_000 + 3_723_000),
        "2026-11-15 01:02:03"
    );
    assert_eq!(utc_text(951_782_400_000), "2000-02-29 00:00:00");
}

// REQ: OA-SHD-006
#[test]
fn altitude_events_name_the_band_of_eram_altitude_and_the_summary_counts_both() {
    let mut low = eram_at(12, 12.0, 100.0, 0.0, 0.0);
    low.alt_ft = Some(17_999 - 2000);
    let mut high = eram_at(24, 24.0, 100.0, 0.0, 0.0);
    high.alt_ft = Some(18_000 + 1000);
    let mut oa = oa_north(1, 0, 40, 100.0);
    oa.samples
        .iter_mut()
        .for_each(|s| s.alt_ft = Some(18_000.0));
    let c = compare_all(&[flight(Some(1), vec![low, high])], &oa_map(vec![oa]), None);
    let alt = of(&c, Class::Alt);
    assert_eq!(alt.len(), 2);
    assert_eq!(alt[0].detail, "eram_below_fl180");
    assert_eq!(alt[1].detail, "eram_at_or_above_fl180");
    let r = render(&c, &[]);
    assert!(
        r.summary.contains("below 18,000 ft 1, at or above 1"),
        "{}",
        r.summary
    );
}

// REQ: OA-SHD-018
#[test]
fn the_extent_of_a_set_of_times_is_its_first_and_last() {
    use oa_shadow::window::extent;
    assert_eq!(extent([5, 1, 9, 3].into_iter()), Some((1, 9)));
    assert_eq!(extent(std::iter::empty()), None);
    assert_eq!(extent([4].into_iter()), Some((4, 4)));
}

// REQ: OA-SHD-029
#[test]
fn nonzero_load_counts_are_listed_for_the_summary_header() {
    let mut c = oa_bench::legacy::Counts::default();
    assert!(oa_shadow::run::load_notes(&c).is_empty());
    c.sfdps_rejected = 3;
    c.note(
        "SFDPS beacon changes dropped, reason no message time".into(),
        2,
    );
    let v = oa_shadow::run::load_notes(&c);
    assert!(v
        .iter()
        .any(|l| l.contains("rejected") && l.ends_with(" 3")));
    assert!(v
        .iter()
        .any(|l| l.contains("no message time") && l.ends_with(" 2")));
}

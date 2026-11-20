//! Scoring tests over synthetic STDDS records, trackers and truths. Every position, address and
//! time is made up. No real recording is read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_stdds::{new_stats, parse_message, Limits, Message};
use oa_stdds_ref::engine::{analyse, evaluate, fit_offset, pair_err, FitMiss, FIT_MIN_SAMPLES};
use oa_stdds_ref::load::{load_reader, rec_of};
use oa_stdds_ref::model::{classify_record, Class, Fix, Miss, Rec, Reference, Unpaired};
use oa_stdds_ref::report::build_report;
use oa_stdds_ref::sources::{interpolate, Row, TrackerSource};
use oa_xml::{Limits as XmlLimits, Parser};

const T0: i64 = 1_900_000_000_000;
const LAT: f64 = 41.5;
const LON: f64 = -88.0;
const ADDR: u32 = 0xA1B2C3;

/// An aircraft flying a straight line at constant velocity, known for `t0..=t1`. The position at
/// time `t` is the line's at `t - shift_ms` (a shifted line is a clock that is late).
#[derive(Clone, Copy)]
struct Line {
    addr: u32,
    lat: f64,
    lon: f64,
    ve: f64,
    vn: f64,
    alt: f64,
    t0: i64,
    t1: i64,
    shift_ms: i64,
}

fn line() -> Line {
    Line {
        addr: ADDR,
        lat: LAT,
        lon: LON,
        ve: 200.0,
        vn: 0.0,
        alt: 10_000.0,
        t0: T0,
        t1: T0 + 200_000,
        shift_ms: 0,
    }
}

impl Reference for Line {
    fn at(&self, addr: u32, t_ms: i64) -> Result<Fix, Miss> {
        if addr != self.addr {
            return Err(Miss::NoAircraft);
        }
        let t = t_ms - self.shift_ms;
        if t < self.t0 || t > self.t1 {
            return Err(Miss::OutsideSpan);
        }
        let dt = (t - self.t0) as f64 / 1000.0;
        let f = oa_geo::aeqd::frame_new(self.lat, self.lon);
        let (lat, lon) = oa_geo::aeqd::unproject(&f, self.ve * dt, self.vn * dt).unwrap();
        Ok(Fix {
            lat,
            lon,
            alt_ft: Some(self.alt),
            vel: Some((self.ve, self.vn)),
            extrapolated: false,
        })
    }
}

/// The one-second rows of `l` as a tracker source.
fn tracker_of(l: &Line) -> TrackerSource {
    let rows = (l.t0..=l.t1 + l.shift_ms).step_by(1000).filter_map(|t| {
        let f = l.at(l.addr, t).ok()?;
        Some((
            l.addr,
            Row {
                t_ms: t,
                lat: f.lat,
                lon: f.lon,
                alt_ft: f.alt_ft,
                vel: f.vel,
            },
        ))
    });
    TrackerSource::from_rows(rows)
}

/// A STDDS record stamped `stamp` whose position is the generator line's at that stamp.
fn rec(gen: &Line, addr: Option<u32>, stamp: i64, index: u32, class: Class) -> Rec {
    let f = gen.at(gen.addr, stamp).unwrap();
    Rec {
        file: 0,
        index,
        class,
        addr,
        src: *b"ZZA\0\0\0\0\0",
        track_num: Some(1234),
        t_ms: Some(stamp),
        pos: Some((f.lat, f.lon)),
        alt_ft: f.alt_ft,
        vel: f.vel,
    }
}

/// 81 records, 2 s apart, 20 s to 180 s into the line, from a generator whose clock is `late_ms`
/// late: the record stamped `s` has the position of the aircraft at `s - late_ms`.
fn track(late_ms: i64, class: Class) -> Vec<Rec> {
    let gen = Line {
        shift_ms: late_ms,
        ..line()
    };
    (0..81)
        .map(|k| rec(&gen, Some(ADDR), T0 + 20_000 + 2_000 * k, k as u32, class))
        .collect()
}

fn section<'a>(summary: &'a str, class: &str) -> &'a str {
    let key = format!("== independence class: {class} ==");
    let from = summary.find(&key).unwrap();
    let rest = &summary[from + key.len()..];
    let end = rest.find("== independence class:").unwrap_or(rest.len());
    &rest[..end]
}

// REQ: OA-SRF-001
#[test]
fn records_are_paired_by_mode_s_address_only() {
    let l = line();
    let other = Line {
        addr: 0x123456,
        lat: LAT + 0.5,
        ..line()
    };
    // The record's position is on `l`, but its address is the other aircraft's: it must not be
    // paired with `l`, whatever the position says.
    let mut r = rec(&l, Some(0x123456), T0 + 50_000, 0, Class::Unknown);
    let both = TrackerSource::from_rows(
        [&l, &other]
            .iter()
            .flat_map(|x| (x.t0..=x.t1).step_by(1000).map(move |t| (x, t)))
            .map(|(x, t)| {
                let f = x.at(x.addr, t).unwrap();
                (
                    x.addr,
                    Row {
                        t_ms: t,
                        lat: f.lat,
                        lon: f.lon,
                        alt_ft: None,
                        vel: None,
                    },
                )
            }),
    );
    struct Two(Line, Line);
    impl Reference for Two {
        fn at(&self, a: u32, t: i64) -> Result<Fix, Miss> {
            if a == self.0.addr {
                self.0.at(a, t)
            } else {
                self.1.at(a, t)
            }
        }
    }
    let truth = Two(l, other);
    let s = evaluate(&r, T0 + 50_000, &both, &truth).unwrap();
    // Paired with the other aircraft, 0.5 degrees (about 55 km) away.
    assert!(s.errs[0].h_m > 50_000.0, "{}", s.errs[0].h_m);
    r.addr = Some(ADDR);
    let s = evaluate(&r, T0 + 50_000, &both, &truth).unwrap();
    assert!(s.errs[0].h_m < 1.0, "{}", s.errs[0].h_m);
}

// REQ: OA-SRF-001
#[test]
fn a_record_without_an_address_is_counted_and_not_paired() {
    let l = line();
    let mut recs = track(0, Class::Unknown);
    recs.push(Rec {
        addr: None,
        ..recs[0]
    });
    let a = analyse(&recs, &tracker_of(&l), &l);
    let last = a.rows.last().unwrap();
    assert_eq!(last.plain, Err(Unpaired::NoAddress));
    assert_eq!(last.fitted, Err(Unpaired::NoAddress));
    let r = build_report(&[], &a);
    assert!(r.summary.contains("without address 1 "), "{}", r.summary);
    assert!(r.summary.contains("unpaired, no Mode S address: 1"));
    // An address no reference holds is a counted reason of its own.
    let stranger = Rec {
        addr: Some(0x999999),
        ..recs[0]
    };
    let a = analyse(&[stranger], &tracker_of(&l), &l);
    assert_eq!(a.rows[0].plain, Err(Unpaired::NoTracker(Miss::NoAircraft)));
}

// REQ: OA-SRF-012
#[test]
fn a_record_without_time_or_position_is_counted_and_not_paired() {
    let l = line();
    let base = track(0, Class::Unknown)[10];
    let recs = [Rec { t_ms: None, ..base }, Rec { pos: None, ..base }, base];
    let a = analyse(&recs, &tracker_of(&l), &l);
    assert_eq!(a.rows[0].plain, Err(Unpaired::NoTimeOrPosition));
    assert_eq!(a.rows[1].plain, Err(Unpaired::NoTimeOrPosition));
    assert_eq!(a.rows[1].fitted, Err(Unpaired::NoTimeOrPosition));
    assert!(a.rows[2].plain.is_ok());
    let r = build_report(&[], &a);
    assert!(r.summary.contains("unpaired, no time or position: 2"));
}

fn tais_xml(records: &[(Option<&str>, &str, &str, &str)]) -> Vec<u8> {
    let mut s = String::from(
        "<?xml version=\"1.0\"?>\n<ns2:TATrackAndFlightPlan xmlns:ns2=\"urn:us:gov:dot:faa:atm:terminal:entities:v4-0:tais:terminalautomationinformation\"><src>ZZA</src>",
    );
    for (addr, time, lat, lon) in records {
        s.push_str("<record><track><trackNum>77</trackNum>");
        s.push_str(&format!(
            "<mrtTime>{time}</mrtTime><lat>{lat}</lat><lon>{lon}</lon>"
        ));
        s.push_str("<reportedAltitude>5000</reportedAltitude><vx>10.5</vx><vy>-20</vy>");
        if let Some(a) = addr {
            s.push_str(&format!("<acAddress>{a}</acAddress>"));
        }
        s.push_str("</track></record>");
    }
    s.push_str("</ns2:TATrackAndFlightPlan>");
    s.into_bytes()
}

fn parse(xml: &[u8]) -> oa_stdds::TaisMessage {
    let mut p = Parser::new(XmlLimits::default()).unwrap();
    match parse_message(&mut p, xml, &Limits::default(), &mut new_stats()).unwrap() {
        Message::Tais(m) => m,
        other => panic!("not TAIS: {other:?}"),
    }
}

// REQ: OA-SRF-002
#[test]
fn every_record_is_class_unknown_for_now() {
    let m = parse(&tais_xml(&[
        (Some("A1B2C3"), "2030-01-02T03:04:05.000Z", "40.5", "-75.5"),
        (None, "2030-01-02T03:04:06.000Z", "40.6", "-75.6"),
    ]));
    assert_eq!(m.records.len(), 2);
    for r in &m.records {
        assert_eq!(classify_record(r, &m), Class::Unknown);
        assert_eq!(rec_of(0, 0, &m, r).class, Class::Unknown);
    }
    let a = rec_of(0, 0, &m, &m.records[0]);
    assert_eq!(a.addr, Some(0xA1B2C3));
    assert_eq!(a.alt_ft, Some(5000.0));
    let v = a.vel.unwrap();
    assert!((v.0 - 10.5).abs() < 1e-9 && (v.1 + 20.0).abs() < 1e-9);
    assert_eq!(rec_of(0, 1, &m, &m.records[1]).addr, None);
}

fn oaf(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, p) in payloads.iter().enumerate() {
        let mut buf = vec![0u8; p.len() + 256];
        let n = oa_feed::encode_frame(&mut buf, 1, 0, i as u64, 0, 0, p).unwrap();
        out.extend_from_slice(&buf[..n]);
    }
    out
}

// REQ: OA-SRF-011
#[test]
fn a_recording_is_read_into_records_with_counts_and_a_digest() {
    let m1 = tais_xml(&[
        (Some("A1B2C3"), "2030-01-02T03:04:05.000Z", "40.5", "-75.5"),
        (None, "2030-01-02T03:04:06.000Z", "40.6", "-75.6"),
    ]);
    let m2 = tais_xml(&[(Some("00ABCD"), "2030-01-02T03:04:07.000Z", "40.7", "-75.7")]);
    let other = b"<x xmlns=\"urn:none\"/>".to_vec();
    let bytes = oaf(&[m1, other, m2]);
    let mut out = Vec::new();
    let info = load_reader(&bytes[..], "mem", 3, &mut out).unwrap();
    assert_eq!(info.frames_message, 3);
    assert_eq!(info.messages_not_tais, 1);
    assert_eq!(info.records, 3);
    assert_eq!(info.frames_damaged, 0);
    assert_eq!(out.len(), 3);
    assert_eq!(
        out.iter().map(|r| (r.file, r.index)).collect::<Vec<_>>(),
        vec![(3, 0), (3, 1), (3, 2)]
    );
    assert_eq!(out[2].addr, Some(0xABCD));
    assert_eq!(info.sha256.len(), 64);
    // The digest is of the bytes, so it is the same for the same bytes and not for others.
    let mut again = Vec::new();
    assert_eq!(
        load_reader(&bytes[..], "mem", 3, &mut again)
            .unwrap()
            .sha256,
        info.sha256
    );
    let mut damaged = bytes.clone();
    let k = damaged.len() / 2;
    damaged[k] ^= 0xff;
    let mut o2 = Vec::new();
    let i2 = load_reader(&damaged[..], "mem", 3, &mut o2).unwrap();
    assert_ne!(i2.sha256, info.sha256);
    assert!(i2.frames_damaged > 0);
}

// REQ: OA-SRF-005
#[test]
fn a_stamp_late_by_2_3_s_recovers_an_offset_of_2_3_s() {
    let l = line();
    let recs = track(2_300, Class::Unknown);
    let refs: Vec<&Rec> = recs.iter().collect();
    let f = fit_offset(&refs, &l).unwrap();
    assert_eq!(f.offset_ms, 2_300);
    assert!(!f.at_edge);
    assert_eq!(f.samples, 81);
    // Early stamps recover a negative offset.
    let early = track(-4_700, Class::Unknown);
    let refs: Vec<&Rec> = early.iter().collect();
    assert_eq!(fit_offset(&refs, &l).unwrap().offset_ms, -4_700);
    // Through the whole analysis: published error is the 460 m the clock costs, the fitted one
    // is zero.
    let a = analyse(&recs, &tracker_of(&l), &l);
    assert_eq!(a.fits.len(), 1);
    let mid = &a.rows[40];
    assert_eq!(mid.offset_ms, Some(2_300));
    let plain = mid.plain.unwrap().errs[1];
    let fitted = mid.fitted.unwrap().errs[1];
    assert!((plain.along_m.unwrap() - 460.0).abs() < 0.5, "{plain:?}");
    assert!(fitted.h_m < 0.5, "{fitted:?}");
}

// REQ: OA-SRF-005
#[test]
fn the_offset_is_fitted_against_the_truth_never_the_tracker() {
    let l = line();
    let recs = track(2_300, Class::Unknown);
    // A tracker whose clock is 6 s wrong must change neither the fit nor the truth set.
    let bad = Line {
        shift_ms: 6_000,
        ..l
    };
    let a = analyse(&recs, &tracker_of(&bad), &l);
    assert_eq!(a.rows[40].offset_ms, Some(2_300));
    let good = analyse(&recs, &tracker_of(&l), &l);
    assert_eq!(
        a.rows[40].fitted.unwrap().errs[1],
        good.rows[40].fitted.unwrap().errs[1]
    );
    assert_ne!(
        a.rows[40].fitted.unwrap().errs[0],
        good.rows[40].fitted.unwrap().errs[0]
    );
}

// REQ: OA-SRF-006
#[test]
fn offset_search_edges_are_flagged_and_ties_go_toward_zero() {
    let l = line();
    // Exactly at the edge is found and flagged.
    let r = track(15_000, Class::Unknown);
    let refs: Vec<&Rec> = r.iter().collect();
    let f = fit_offset(&refs, &l).unwrap();
    assert_eq!((f.offset_ms, f.at_edge), (15_000, true));
    let r = track(-15_000, Class::Unknown);
    let refs: Vec<&Rec> = r.iter().collect();
    let f = fit_offset(&refs, &l).unwrap();
    assert_eq!((f.offset_ms, f.at_edge), (-15_000, true));
    // Beyond the range the best in range is the edge, flagged: the true offset may lie past it.
    let r = track(20_000, Class::Unknown);
    let refs: Vec<&Rec> = r.iter().collect();
    let f = fit_offset(&refs, &l).unwrap();
    assert_eq!((f.offset_ms, f.at_edge), (15_000, true));
    // Too few samples: not fitted, and no offset is made up.
    let few = track(2_300, Class::Unknown);
    let refs: Vec<&Rec> = few.iter().take(FIT_MIN_SAMPLES - 1).collect();
    assert_eq!(fit_offset(&refs, &l), Err(FitMiss::TooFewSamples));
    // A truth that is the same at every offset ties every offset: zero wins.
    struct Still;
    impl Reference for Still {
        fn at(&self, _: u32, _: i64) -> Result<Fix, Miss> {
            Ok(Fix {
                lat: LAT,
                lon: LON,
                alt_ft: None,
                vel: Some((10.0, 0.0)),
                extrapolated: false,
            })
        }
    }
    let r = track(2_300, Class::Unknown);
    let refs: Vec<&Rec> = r.iter().collect();
    let f = fit_offset(&refs, &Still).unwrap();
    assert_eq!((f.offset_ms, f.at_edge), (0, false));
    // A track whose truth is missing at the far offsets (it ends too soon) is not fitted.
    let short = Line {
        t1: T0 + 40_000,
        ..l
    };
    let r = track(0, Class::Unknown);
    let refs: Vec<&Rec> = r.iter().collect();
    assert_eq!(fit_offset(&refs, &short), Err(FitMiss::TooFewSamples));
}

// REQ: OA-SRF-005
#[test]
fn a_track_that_is_not_fitted_has_no_fitted_samples() {
    let l = line();
    let recs: Vec<Rec> = track(2_300, Class::Unknown).into_iter().take(3).collect();
    let a = analyse(&recs, &tracker_of(&l), &l);
    assert!(a.rows[0].plain.is_ok());
    assert_eq!(a.rows[0].offset_ms, None);
    assert_eq!(a.rows[0].fitted, Err(Unpaired::NotFitted));
    let r = build_report(&[], &a);
    assert!(r
        .summary
        .contains("not fitted (fewer than 8 usable samples) 1"));
    assert!(r
        .summary
        .contains("unpaired, track has no fitted time offset: 3"));
}

fn rows_every_second(n: i64, vel: Option<(f64, f64)>) -> Vec<Row> {
    (0..n)
        .map(|k| Row {
            t_ms: T0 + k * 1000,
            lat: LAT,
            lon: LON + k as f64 * 0.001,
            alt_ft: Some(1000.0 + k as f64),
            vel,
        })
        .collect()
}

// REQ: OA-SRF-007
#[test]
fn interpolation_at_segment_ends_extrapolates_at_most_one_second() {
    let rows = rows_every_second(11, Some((100.0, 0.0)));
    // Inside: linear.
    let f = interpolate(&rows, T0 + 2_500).unwrap();
    assert!((f.lon - (LON + 0.0025)).abs() < 1e-12);
    assert_eq!(f.alt_ft, Some(1002.5));
    assert!(!f.extrapolated);
    // Exactly on a row and at both ends: the row, not extrapolated.
    assert!(!interpolate(&rows, T0).unwrap().extrapolated);
    assert!(!interpolate(&rows, T0 + 10_000).unwrap().extrapolated);
    // Up to 1 s past either end: advanced by the end row's velocity, flagged.
    let f = interpolate(&rows, T0 + 10_500).unwrap();
    assert!(f.extrapolated);
    let eastward = oa_geo::aeqd::project(
        &oa_geo::aeqd::frame_new(rows[10].lat, rows[10].lon),
        f.lat,
        f.lon,
    );
    assert!(
        (eastward.0 - 50.0).abs() < 0.01 && eastward.1.abs() < 0.01,
        "{eastward:?}"
    );
    let f = interpolate(&rows, T0 - 1_000).unwrap();
    assert!(f.extrapolated);
    let west = oa_geo::aeqd::project(
        &oa_geo::aeqd::frame_new(rows[0].lat, rows[0].lon),
        f.lat,
        f.lon,
    );
    assert!((west.0 + 100.0).abs() < 0.01, "{west:?}");
    // Past 1 s: nothing.
    assert_eq!(interpolate(&rows, T0 + 11_001), Err(Miss::OutsideSpan));
    assert_eq!(interpolate(&rows, T0 - 1_001), Err(Miss::OutsideSpan));
    // An end row without a velocity is not extrapolated at all.
    let bare = rows_every_second(11, None);
    assert_eq!(interpolate(&bare, T0 + 10_500), Err(Miss::OutsideSpan));
    assert_eq!(interpolate(&[], T0), Err(Miss::NoAircraft));
    // A gap between rows is never bridged.
    let mut gap = rows_every_second(11, None);
    gap.drain(2..7);
    assert_eq!(interpolate(&gap, T0 + 3_000), Err(Miss::Gap));
    assert!(interpolate(&gap, T0 + 500).is_ok());
}

// REQ: OA-SRF-007
#[test]
fn extrapolated_tracker_fixes_are_counted_in_the_report() {
    let l = line();
    // The tracker's rows end 0.5 s before the last record.
    let short = Line {
        t1: l.t1 - 500,
        ..l
    };
    let gen = line();
    let recs = vec![
        rec(&gen, Some(ADDR), l.t1 - 5_000, 0, Class::Unknown),
        rec(&gen, Some(ADDR), l.t1 - 250, 1, Class::Unknown),
        rec(&gen, Some(ADDR), l.t1, 2, Class::Unknown),
    ];
    let mut tracker_rows = tracker_of(&short);
    let a = analyse(&recs, &tracker_rows, &l);
    assert!(!a.rows[0].plain.unwrap().tracker_extrapolated);
    assert!(a.rows[1].plain.unwrap().tracker_extrapolated);
    assert!(a.rows[2].plain.is_err() || a.rows[2].plain.unwrap().tracker_extrapolated);
    let r = build_report(&[], &a);
    assert!(
        r.summary.contains("tracker fix extrapolated within 1 s: 2")
            || r.summary.contains("tracker fix extrapolated within 1 s: 1")
    );
    tracker_rows = TrackerSource::default();
    let a = analyse(&recs, &tracker_rows, &l);
    assert_eq!(a.rows[0].plain, Err(Unpaired::NoTracker(Miss::NoAircraft)));
}

// REQ: OA-SRF-004
#[test]
fn the_three_error_sets_are_on_the_same_sample() {
    // Truth on the line, tracker 20 m north of it, STDDS 50 m north of it, all at the same time.
    let l = line();
    let north = |m: f64| {
        let f = oa_geo::aeqd::frame_new(LAT, LON);
        let (la, lo) = oa_geo::aeqd::unproject(&f, 0.0, m).unwrap();
        (la - LAT, lo - LON)
    };
    let (dla20, _) = north(20.0);
    let (dla50, _) = north(50.0);
    let rows = (l.t0..=l.t1).step_by(1000).map(|t| {
        let f = l.at(ADDR, t).unwrap();
        (
            ADDR,
            Row {
                t_ms: t,
                lat: f.lat + dla20,
                lon: f.lon,
                alt_ft: Some(10_030.0),
                vel: Some((205.0, 0.0)),
            },
        )
    });
    let tracker = TrackerSource::from_rows(rows);
    let f = l.at(ADDR, T0 + 60_000).unwrap();
    let r = Rec {
        pos: Some((f.lat + dla50, f.lon)),
        alt_ft: Some(10_100.0),
        vel: Some((190.0, 0.0)),
        ..track(0, Class::Unknown)[0]
    };
    let s = evaluate(&r, T0 + 60_000, &tracker, &l).unwrap();
    let [ts, us, tu] = s.errs;
    // Tracker minus STDDS: 30 m south of it (cross is positive to the right, south for an
    // eastbound aircraft), no along error.
    assert!((ts.h_m - 30.0).abs() < 0.05, "{ts:?}");
    assert!(ts.along_m.unwrap().abs() < 0.05);
    assert!((ts.cross_m.unwrap() - 30.0).abs() < 0.05);
    assert!((ts.dalt_ft.unwrap() + 70.0).abs() < 1e-9);
    assert!((ts.dvel_mps.unwrap() - 15.0).abs() < 1e-9);
    // Truth minus STDDS: 50 m south, 100 ft below, velocity 10 m/s over.
    assert!((us.h_m - 50.0).abs() < 0.05);
    assert!((us.cross_m.unwrap() - 50.0).abs() < 0.05);
    assert!((us.dalt_ft.unwrap() + 100.0).abs() < 1e-9);
    assert!((us.dvel_mps.unwrap() - 10.0).abs() < 1e-9);
    // Tracker minus truth: 20 m north (cross -20), 30 ft above, 5 m/s.
    assert!((tu.h_m - 20.0).abs() < 0.05);
    assert!((tu.cross_m.unwrap() + 20.0).abs() < 0.05);
    assert!((tu.dalt_ft.unwrap() - 30.0).abs() < 1e-9);
    assert!((tu.dvel_mps.unwrap() - 5.0).abs() < 1e-9);
}

// REQ: OA-SRF-008
#[test]
fn metrics_have_n_rms_percentiles_and_absent_values_are_not_zero() {
    let slow = Fix {
        lat: LAT,
        lon: LON,
        alt_ft: None,
        vel: Some((0.5, 0.0)),
        extrapolated: false,
    };
    let other = Fix {
        lat: LAT + 0.0001,
        lon: LON,
        alt_ft: None,
        vel: None,
        extrapolated: false,
    };
    // A slow truth gives no direction: the horizontal error stays, along and cross are absent.
    let e = pair_err(&other, &slow, None).unwrap();
    assert!(e.h_m > 10.0);
    assert_eq!(
        (e.along_m, e.cross_m, e.dalt_ft, e.dvel_mps),
        (None, None, None, None)
    );
    // Report over ten samples at 1..=10 m: rms, nearest-rank percentiles.
    let l = line();
    let recs: Vec<Rec> = (1..=10)
        .map(|k| {
            let f = l.at(ADDR, T0 + 30_000 + 2_000 * k).unwrap();
            let dla =
                oa_geo::aeqd::unproject(&oa_geo::aeqd::frame_new(f.lat, f.lon), 0.0, k as f64)
                    .unwrap()
                    .0
                    - f.lat;
            Rec {
                pos: Some((f.lat + dla, f.lon)),
                t_ms: Some(T0 + 30_000 + 2_000 * k),
                index: k as u32,
                ..track(0, Class::Unknown)[0]
            }
        })
        .collect();
    let a = analyse(&recs, &tracker_of(&l), &l);
    let r = build_report(&[], &a);
    let sec = section(&r.summary, "unknown");
    let p = sec.find("stamp as published (offset 0)").unwrap();
    let block = &sec[p..];
    let tracker_set = &block[block.find("set: tracker - stdds").unwrap()..];
    let horiz = tracker_set
        .lines()
        .find(|x| x.contains("horizontal (m)"))
        .unwrap();
    // rms of 1..=10 is sqrt(38.5) = 6.20; mean 5.5; p50 5; p95 10; p99 10.
    assert!(horiz.contains("n      10"), "{horiz}");
    assert!(horiz.contains("rms      6.20"), "{horiz}");
    assert!(horiz.contains("mean      5.50"), "{horiz}");
    assert!(horiz.contains("p50      5.00"), "{horiz}");
    assert!(horiz.contains("p95     10.00"), "{horiz}");
    assert!(horiz.contains("p99     10.00"), "{horiz}");
    for m in [
        "along track (m)",
        "cross track (m)",
        "altitude (ft)",
        "velocity (m/s)",
    ] {
        assert!(tracker_set.contains(m), "{m}");
    }
    // Nothing paired says "no data", never 0.
    let none = [Rec {
        addr: None,
        ..recs[0]
    }];
    let r = build_report(&[], &analyse(&none, &tracker_of(&l), &l));
    assert!(
        r.summary.contains("horizontal (m)   no data"),
        "{}",
        r.summary
    );
}

// REQ: OA-SRF-003
#[test]
fn every_metric_is_reported_per_class_and_never_blended() {
    let l = line();
    let mut recs = track(0, Class::Unknown);
    // The second class's records sit 100 m north of the line: a different error entirely.
    let shifted: Vec<Rec> = track(0, Class::FusedAdsb)
        .into_iter()
        .map(|mut r| {
            let (la, lo) = r.pos.unwrap();
            let dla = oa_geo::aeqd::unproject(&oa_geo::aeqd::frame_new(la, lo), 0.0, 100.0)
                .unwrap()
                .0
                - la;
            r.pos = Some((la + dla, lo));
            r.track_num = Some(9);
            r
        })
        .collect();
    recs.extend(shifted);
    let a = analyse(&recs, &tracker_of(&l), &l);
    let r = build_report(&[], &a);
    let unknown = section(&r.summary, "unknown");
    let fused = section(&r.summary, "fused-adsb");
    let line_of = |s: &str| {
        let b = &s[s.find("stamp as published (offset 0)").unwrap()..];
        b.lines()
            .find(|x| x.contains("horizontal (m)"))
            .unwrap()
            .to_string()
    };
    assert!(
        line_of(unknown).contains("rms      0.0"),
        "{}",
        line_of(unknown)
    );
    assert!(
        line_of(fused).contains("rms    100.0"),
        "{}",
        line_of(fused)
    );
    assert!(unknown.contains("records 81,") && fused.contains("records 81,"));
    // A class with no records has no section, and there is no all-class section or line.
    assert!(!r.summary.contains("independence class: radar-only"));
    assert!(!r.summary.contains("independence class: all"));
    assert_eq!(r.summary.matches("== independence class:").count(), 2);
    // Tracks are fitted per class too.
    assert!(unknown.contains("time offsets: tracks 1,"));
    assert!(fused.contains("time offsets: tracks 1,"));
    assert!(r.csv.lines().nth(1).unwrap().contains(",unknown,"));
    assert!(r.csv.lines().last().unwrap().contains(",fused-adsb,"));
}

// REQ: OA-SRF-009
#[test]
fn the_report_is_deterministic_and_carries_digests() {
    let l = line();
    let recs = track(2_300, Class::Unknown);
    let header = vec!["STDDS file 0: x (digest abc)".to_string()];
    let a = analyse(&recs, &tracker_of(&l), &l);
    let r1 = build_report(&header, &a);
    let r2 = build_report(&header, &analyse(&recs, &tracker_of(&l), &l));
    assert_eq!(r1, r2);
    assert!(r1.summary.contains("STDDS file 0: x (digest abc)"));
    let last = r1.summary.lines().last().unwrap();
    let hex = last
        .strip_prefix("digest (summary above and the csv): ")
        .unwrap();
    assert_eq!(hex.len(), 64);
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(r1.csv.lines().count() == 82);
    // Another input gives another digest.
    let mut other = recs.clone();
    other[5].t_ms = Some(T0 + 31_000);
    let r3 = build_report(&header, &analyse(&other, &tracker_of(&l), &l));
    assert_ne!(r3.summary.lines().last(), r1.summary.lines().last());
    // No wall-clock or run-specific text: nothing but the inputs reaches the report.
    assert!(!r1.summary.contains("seconds elapsed"));
}

// REQ: OA-SRF-014
#[test]
fn the_text_output_holds_none_of_the_fixture_identifiers() {
    let l = line();
    let mut recs = track(2_300, Class::Unknown);
    for (k, r) in recs.iter_mut().enumerate() {
        r.src = *b"QQZ\0\0\0\0\0";
        r.track_num = Some(31_337);
        r.index = k as u32;
    }
    let a = analyse(&recs, &tracker_of(&l), &l);
    let r = build_report(&["STDDS file 0: input.oaf (digest abc)".to_string()], &a);
    let addr_hex = format!("{ADDR:06x}");
    let samples = [
        addr_hex.clone(),
        addr_hex.to_uppercase(),
        ADDR.to_string(),
        "31337".to_string(),
        "QQZ".to_string(),
        format!("{LAT}"),
        format!("{LON}"),
        T0.to_string(),
        (T0 + 20_000).to_string(),
    ];
    for out in [&r.summary, &r.csv] {
        for id in &samples {
            assert!(!out.contains(id.as_str()), "{id} in the output");
        }
    }
    // The record is still counted and scored.
    assert!(r.summary.contains("records 81,"));
    assert_eq!(r.csv.lines().count(), 82);
}

// REQ: OA-SRF-010, OA-SRF-013
#[test]
fn a_sealed_path_exits_2_naming_it_before_anything_is_opened() {
    use std::os::unix::fs::symlink;
    let base = std::env::temp_dir().join(format!("oa-stdds-ref-seal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let sealed = base.join("scds").join("rec-20261119").join("day2");
    let holdout = base.join("scds").join("stdds-20261119-holdout");
    let day = base.join("raw").join("adsblol").join("2026.11.21");
    for d in [&sealed, &holdout, &day] {
        std::fs::create_dir_all(d).unwrap();
    }
    let file = sealed.join("day.oaf");
    std::fs::write(&file, b"synthetic").unwrap();
    let to_file = base.join("link-to-file");
    let to_dir = base.join("link-to-dir");
    let to_link = base.join("link-to-link");
    symlink(&file, &to_file).unwrap();
    symlink(&sealed, &to_dir).unwrap();
    symlink(&to_dir, &to_link).unwrap();
    let fine = base.join("fine.oaf");
    std::fs::write(&fine, b"synthetic").unwrap();
    let out_dir = base.join("out");

    let exe = env!("CARGO_BIN_EXE_oa-stdds-ref");
    let run = |clip: &std::path::Path, out: &std::path::Path, file: &std::path::Path| {
        std::process::Command::new(exe)
            .arg("--adsb")
            .arg(clip)
            .arg("--out")
            .arg(out)
            .arg(file)
            .output()
            .unwrap()
    };
    let missing = base.join("no-such-clip.oaf");
    // A STDDS file: a sealed directory, a file inside it, and links to the file, the folder and a link.
    for p in [
        holdout.clone(),
        file.clone(),
        to_file.clone(),
        to_dir.join("day.oaf"),
        to_link.join("day.oaf"),
    ] {
        let o = run(&missing, &out_dir, &p);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(2), "{}: {err}", p.display());
        assert!(
            err.contains("refused") && err.contains(&*p.to_string_lossy()),
            "{err}"
        );
        assert!(!out_dir.exists());
    }
    // A sealed ADS-B day, or a link to sealed data, as the clip.
    for p in [day.join("clip.oaf"), to_file.clone(), to_link.join("c.oaf")] {
        let o = run(&p, &out_dir, &fine);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(2), "{}: {err}", p.display());
        assert!(err.contains(&*p.to_string_lossy()), "{err}");
        assert!(!out_dir.exists());
    }
    // A sealed output folder is refused too, and not created.
    let o = run(&missing, &holdout.join("out"), &fine);
    assert_eq!(o.status.code(), Some(2));
    assert!(!holdout.join("out").exists());
    let _ = std::fs::remove_dir_all(&base);
}

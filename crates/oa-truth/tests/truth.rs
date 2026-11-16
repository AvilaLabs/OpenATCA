//! Truth construction on synthetic traffic with known errors, the query rules, determinism and
//! the real-data run (skipped when the clip is absent).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{
    reports_len, reports_push, Reports, Row, F_NEW_LEG, F_STALE, NONE_I32, NONE_U16, NONE_U8,
    NON_ICAO, ROW_EMPTY,
};
use oa_sha256::Sha256;
use oa_truth::*;

const LAT0: f64 = 41.0;
const LON0: f64 = -88.0;
const T0: i64 = 1_700_000_000_000_000_000;
const M_PER_DEG: f64 = 111_195.0;
const ADDR: u32 = 0xa12345;

/// Deterministic standard normal numbers (LCG plus Box-Muller).
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let (a, b) = (self.unit(), self.unit());
        (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
    }
}

/// East/north metres about the origin to degrees (flat; the tests compare in the same terms).
fn to_deg(e: f64, n: f64) -> (f64, f64) {
    let lat = LAT0 + n / M_PER_DEG;
    (lat, LON0 + e / (M_PER_DEG * LAT0.to_radians().cos()))
}

/// Distance in metres between two degree positions near the origin.
fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dn = (a.0 - b.0) * M_PER_DEG;
    let de = (a.1 - b.1) * M_PER_DEG * LAT0.to_radians().cos();
    (dn * dn + de * de).sqrt()
}

fn good(addr: u32, t_s: f64, lat: f64, lon: f64) -> Row {
    let mut r = ROW_EMPTY;
    r.icao = addr;
    r.time_ns = T0 + (t_s * 1e9).round() as i64;
    r.lat_e7 = (lat * 1e7).round() as i32;
    r.lon_e7 = (lon * 1e7).round() as i32;
    r.nic = 8;
    r.nac_p = 10;
    r.sil = 3;
    r.version = 2;
    r.source = 0;
    r
}

fn ms(t_s: f64) -> i64 {
    T0 / 1_000_000 + (t_s * 1000.0).round() as i64
}

fn ns(t_s: f64) -> i64 {
    T0 + (t_s * 1e9).round() as i64
}

/// A straight track at 200 m/s toward 045 degrees, one report every `dt` s for `dur` s, with
/// Gaussian noise of `sigma` m per axis.
fn straight(addr: u32, dt: f64, dur: f64, sigma: f64, seed: u64) -> Reports {
    let mut rng = Rng(seed);
    let mut r = Reports::default();
    let v = 200.0 / 2f64.sqrt();
    let mut t = 0.0;
    while t <= dur + 1e-9 {
        let (lat, lon) = to_deg(v * t + sigma * rng.normal(), v * t + sigma * rng.normal());
        reports_push(&mut r, &good(addr, t, lat, lon));
        t += dt;
    }
    r
}

fn straight_pos(t: f64) -> (f64, f64) {
    let v = 200.0 / 2f64.sqrt();
    to_deg(v * t, v * t)
}

fn build(r: &Reports) -> (Truth, TruthStats) {
    build_truth(r, &DEFAULT_CONFIG).unwrap()
}

fn sample(q: TruthQuery) -> TruthSample {
    match q {
        TruthQuery::Ok(s) => s,
        other => panic!("expected a sample, got {other:?}"),
    }
}

fn digest(t: &Truth) -> [u8; 32] {
    let mut h = Sha256::new();
    truth_hash_into(&mut h, t);
    h.finalize()
}

fn removed(s: &TruthStats, kind: usize) -> u64 {
    s.removed[kind]
}

fn balanced(s: &TruthStats) -> bool {
    s.rows_in == s.rows_kept + s.removed.iter().sum::<u64>()
}

// REQ: OA-TRUTH-001
#[test]
fn quality_gates_remove_and_count_each_reason() {
    let mut r = Reports::default();
    let mut t = 0.0;
    let mut push = |r: &mut Reports, f: &dyn Fn(&mut Row)| {
        let (la, lo) = straight_pos(t);
        let mut row = good(ADDR, t, la, lo);
        f(&mut row);
        reports_push(r, &row);
        t += 2.0;
    };
    for _ in 0..30 {
        push(&mut r, &|_| {});
    }
    push(&mut r, &|w| w.nac_p = 7);
    push(&mut r, &|w| w.nac_p = 12);
    push(&mut r, &|w| w.nic = 6);
    push(&mut r, &|w| w.sil = 2);
    push(&mut r, &|w| w.version = 1);
    push(&mut r, &|w| w.flags |= F_STALE);
    push(&mut r, &|w| w.source = 5); // mlat
    push(&mut r, &|w| w.source = 10); // tisb_trackfile
    for _ in 0..30 {
        push(&mut r, &|_| {});
    }
    let (truth, s) = build(&r);
    assert_eq!(removed(&s, REM_NACP), 2);
    assert_eq!(removed(&s, REM_NIC), 1);
    assert_eq!(removed(&s, REM_SIL), 1);
    assert_eq!(removed(&s, REM_VERSION), 1);
    assert_eq!(removed(&s, REM_STALE), 1);
    assert_eq!(removed(&s, REM_SOURCE), 2);
    assert_eq!(s.rows_kept, 60);
    assert!(balanced(&s));
    // The 16 s hole is one leg (under the 120 s gap), not two.
    assert_eq!(s.legs, 1);
    assert_eq!(leg_count(&truth), 1);
    // A version that does not exist in the row is not a reason to refuse it.
    let mut r2 = Reports::default();
    for k in 0..10 {
        let (la, lo) = straight_pos(f64::from(k));
        let mut row = good(ADDR, f64::from(k), la, lo);
        row.version = NONE_U8;
        row.source = NONE_U8;
        reports_push(&mut r2, &row);
    }
    assert_eq!(build(&r2).1.rows_kept, 10);
}

// REQ: OA-TRUTH-002
#[test]
fn quality_is_carried_forward_for_at_most_sixty_seconds() {
    let mut r = Reports::default();
    for k in 0..200 {
        let t = f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        let mut row = good(ADDR, t, la, lo);
        // The aircraft object appears on report 0 and report 100 only.
        if k != 0 && k != 100 {
            row.nic = NONE_U8;
            row.nac_p = NONE_U8;
            row.sil = NONE_U8;
            row.version = NONE_U8;
        }
        reports_push(&mut r, &row);
    }
    let (_, s) = build(&r);
    // Rows at 0..=60 s (31) and 200..=260 s (31) are kept; the 138 rows between and after
    // have no quality value younger than 60 s. The 140 s between the kept groups splits the leg.
    assert_eq!(s.rows_kept, 62);
    assert_eq!(removed(&s, REM_NO_QUALITY), 138);
    assert_eq!(s.quality_carried, 60);
    assert_eq!((s.legs, s.splits[SPLIT_GAP]), (2, 1));
    assert!(balanced(&s));
}

// REQ: OA-TRUTH-003
#[test]
fn jumps_are_removed_and_counted() {
    let mut r = Reports::default();
    for k in 0..60 {
        let t = f64::from(k) * 2.0;
        let (mut la, mut lo) = straight_pos(t);
        if k == 30 {
            (la, lo) = (la + 0.5, lo - 0.5); // about 70 km away
        }
        reports_push(&mut r, &good(ADDR, t, la, lo));
    }
    let (truth, s) = build(&r);
    assert_eq!(removed(&s, REM_JUMP), 1);
    assert_eq!(s.rows_kept, 59);
    assert_eq!(s.legs, 1);
    let q = sample(truth_at(&truth, ADDR, ms(60.0)));
    assert!(dist((q.lat_deg, q.lon_deg), straight_pos(60.0)) < 5.0);
    // A real turn-around of the data (the jump persists) starts a new leg after the
    // resync count instead of rejecting the rest of the flight.
    let mut r = Reports::default();
    for k in 0..60 {
        let t = f64::from(k) * 2.0;
        let (mut la, mut lo) = straight_pos(t);
        if k >= 30 {
            (la, lo) = (la + 0.5, lo - 0.5);
        }
        reports_push(&mut r, &good(ADDR, t, la, lo));
    }
    let (_, s) = build(&r);
    assert_eq!(removed(&s, REM_JUMP), u64::from(JUMP_RESYNC) - 1);
    assert_eq!(s.splits[SPLIT_JUMP], 1);
    assert_eq!(s.legs, 2);
    assert!(balanced(&s));
}

// REQ: OA-TRUTH-017
#[test]
fn a_900_kt_aircraft_is_one_continuous_leg() {
    // 900 kt is 463 m/s: a real fast aircraft in a jet stream or a military jet. Reports every
    // 3 s (some were lost) are 1389 m apart, over what a 700 kt limit allows (1080 m + 200 m).
    // The aircraft's own ground speed says it is going that fast, so no report is a jump.
    for step in [1.0, 3.0] {
        let mut r = Reports::default();
        let mut t = 0.0;
        let mut n = 0u64;
        while t <= 600.0 + 1e-9 {
            let (la, lo) = to_deg(0.0, 463.0 * t);
            let mut row = good(ADDR, t, la, lo);
            row.gs_dkt = 9000;
            row.category = 0xa3;
            reports_push(&mut r, &row);
            t += step;
            n += 1;
        }
        let (_, s) = build(&r);
        assert_eq!(removed(&s, REM_JUMP), 0, "step {step}");
        assert_eq!((s.legs, s.rows_kept, s.splits[SPLIT_JUMP]), (1, n, 0));
        assert!(balanced(&s));
    }
}

// REQ: OA-TRUTH-018
#[test]
fn a_position_jump_is_removed_whether_or_not_ground_speed_is_known() {
    // A 450 kt aircraft whose 31st report is 2862 m off in 2 s: 2782 kt implied, which its own
    // ground speed (and any aircraft) contradicts.
    for gs in [4500u16, NONE_U16] {
        let mut r = Reports::default();
        for k in 0..60 {
            let t = f64::from(k) * 2.0;
            let (mut la, lo) = to_deg(0.0, 231.5 * t);
            if k == 30 {
                la += 2862.0 / M_PER_DEG;
            }
            let mut row = good(ADDR, t, la, lo);
            row.gs_dkt = gs;
            row.category = 0xa3;
            reports_push(&mut r, &row);
        }
        let (_, s) = build(&r);
        assert_eq!(
            (removed(&s, REM_JUMP), s.rows_kept, s.legs),
            (1, 59, 1),
            "gs {gs}"
        );
    }
}

/// A 450 kt aircraft reporting every 2 s for 2 minutes; report `spoof` is `dist_m` off along
/// the track and every report prints the category `cat(k)`.
fn with_spoof(spoof: usize, dist_m: f64, cat: impl Fn(usize) -> u8) -> Reports {
    let mut r = Reports::default();
    for k in 0..60 {
        let t = k as f64 * 2.0;
        let (mut la, lo) = to_deg(0.0, 231.5 * t);
        if k == spoof {
            la += dist_m / M_PER_DEG;
        }
        let mut row = good(ADDR, t, la, lo);
        row.category = cat(k);
        reports_push(&mut r, &row);
    }
    r
}

// REQ: OA-TRUTH-020
#[test]
fn one_report_claiming_a_fast_category_does_not_raise_the_jump_cap() {
    // 2862 m in 2 s is 2782 kt, a jump for an ordinary class (1,000 kt) and plausible without
    // a ground speed for a space vehicle (4,088 kt). Reserved codes never raise the cap.
    for spoof_cat in [0xb7u8, 0xa8, 0xb5] {
        let r = with_spoof(30, 2862.0, |k| if k == 30 { spoof_cat } else { 0xa3 });
        let (_, s) = build(&r);
        assert_eq!(
            (removed(&s, REM_JUMP), s.rows_kept),
            (1, 59),
            "category {spoof_cat:x}"
        );
        assert!(balanced(&s));
    }
    // 1543 m in 2 s is 1500 kt: over the ordinary cap, under the high-performance one.
    let r = with_spoof(30, 1543.0, |k| if k == 30 { 0xa8 } else { 0xa3 });
    assert_eq!(removed(&build(&r).1, REM_JUMP), 1);
}

// REQ: OA-TRUTH-020
#[test]
fn a_report_that_failed_quality_does_not_set_the_category() {
    // The spoofed report has no usable accuracy (NACp 0), so the quality rules remove it; its
    // category must not carry on to the next report, which is the jump.
    let mut r = Reports::default();
    for k in 0..60 {
        let t = k as f64 * 2.0;
        let (mut la, lo) = to_deg(0.0, 231.5 * t);
        let mut row = good(ADDR, t, la, lo);
        row.category = 0xa3;
        if k == 29 {
            row.category = 0xb7;
            row.nac_p = 0;
        }
        if k == 30 {
            la += 2862.0 / M_PER_DEG;
            row = good(ADDR, t, la, lo);
        }
        reports_push(&mut r, &row);
    }
    let (_, s) = build(&r);
    assert_eq!(removed(&s, REM_JUMP), 1);
}

// REQ: OA-TRUTH-020
#[test]
fn a_fast_category_that_persists_does_raise_the_cap() {
    // Every report prints B7: after three accepted reports the class counts, and the same
    // 2,782 kt displacement is possible for it.
    let r = with_spoof(30, 2862.0, |_| 0xb7);
    let (_, s) = build(&r);
    assert_eq!((removed(&s, REM_JUMP), s.rows_kept), (0, 60));
    // A different category in the middle of the run resets the count.
    let r = with_spoof(30, 2862.0, |k| if k == 29 { 0xa3 } else { 0xb7 });
    assert_eq!(removed(&build(&r).1, REM_JUMP), 1);
}

// REQ: OA-TRUTH-021
#[test]
fn a_row_from_another_source_does_not_vouch_for_the_adsb_rows_after_it() {
    // An MLAT row (source 5) carries good quality; ten seconds later an ADS-B row has none.
    let mut r = Reports::default();
    let mut mlat = good(ADDR, 0.0, LAT0, LON0);
    mlat.source = 5;
    reports_push(&mut r, &mlat);
    let mut adsb = good(ADDR, 10.0, LAT0, LON0 + 0.001);
    (adsb.nac_p, adsb.nic, adsb.sil, adsb.version) = (NONE_U8, NONE_U8, NONE_U8, NONE_U8);
    reports_push(&mut r, &adsb);
    let (_, s) = build(&r);
    assert_eq!(removed(&s, REM_SOURCE), 1);
    assert_eq!(removed(&s, REM_NO_QUALITY), 1);
    assert_eq!(s.rows_kept, 0);
    // A good ADS-B row first does carry its quality to the next.
    let mut r = Reports::default();
    reports_push(&mut r, &good(ADDR, 0.0, LAT0, LON0));
    reports_push(&mut r, &adsb);
    assert_eq!(removed(&build(&r).1, REM_NO_QUALITY), 0);
}

// REQ: OA-TRUTH-022
#[test]
fn rows_equal_in_time_give_the_same_truth_in_any_input_order() {
    // Two rows of one instant 20 m apart (one position measured twice): either input order
    // keeps the same one.
    let rows = |swap: bool| {
        let mut r = Reports::default();
        for k in 0..30 {
            let t = f64::from(k) * 2.0;
            let (la, lo) = to_deg(0.0, 231.5 * t);
            let a = good(ADDR, t, la, lo);
            if k == 10 {
                let (la2, lo2) = to_deg(20.0, 231.5 * t);
                let b = good(ADDR, t, la2, lo2);
                let pair = if swap { [b, a] } else { [a, b] };
                for x in pair {
                    reports_push(&mut r, &x);
                }
            } else {
                reports_push(&mut r, &a);
            }
        }
        r
    };
    let (t1, s1) = build(&rows(false));
    let (t2, s2) = build(&rows(true));
    assert_eq!(digest(&t1), digest(&t2));
    assert_eq!(s1.removed, s2.removed);
    assert_eq!(removed(&s1, REM_DUPLICATE), 1);
    // Two rows of one instant 600 m apart contradict each other: both are removed, in any order.
    let far = |swap: bool| {
        let mut r = Reports::default();
        for k in 0..30 {
            let t = f64::from(k) * 2.0;
            let (la, lo) = to_deg(0.0, 231.5 * t);
            let a = good(ADDR, t, la, lo);
            if k == 10 {
                let (la2, lo2) = to_deg(600.0, 231.5 * t);
                let b = good(ADDR, t, la2, lo2);
                let pair = if swap { [b, a] } else { [a, b] };
                for x in pair {
                    reports_push(&mut r, &x);
                }
            } else {
                reports_push(&mut r, &a);
            }
        }
        r
    };
    let (t1, s1) = build(&far(false));
    let (t2, s2) = build(&far(true));
    assert_eq!(digest(&t1), digest(&t2));
    assert_eq!((removed(&s1, REM_DUPLICATE), s1.rows_kept), (2, 29));
    assert_eq!(s1.removed, s2.removed);
    assert!(balanced(&s1));
}

/// A 60 s track at 2 s spacing, north at 231.5 m/s; `extra(k, t, row_of_k)` returns the rows to
/// add at report `k` (its own row is pushed first).
fn track_with(extra: &dyn Fn(usize, f64) -> Vec<Row>) -> Reports {
    let mut r = Reports::default();
    for k in 0..30usize {
        let t = k as f64 * 2.0;
        let (la, lo) = to_deg(0.0, 231.5 * t);
        reports_push(&mut r, &good(ADDR, t, la, lo));
        for x in extra(k, t) {
            reports_push(&mut r, &x);
        }
    }
    r
}

fn pair_row(t: f64, east: f64, nacp: u8) -> Row {
    let (la, lo) = to_deg(east, 231.5 * t);
    let mut x = good(ADDR, t, la, lo);
    x.nac_p = nacp;
    x
}

// REQ: OA-TRUTH-024
#[test]
fn an_unusable_same_time_row_does_not_veto_a_usable_one() {
    for kind in 0..2 {
        let r = track_with(&|k, t| {
            if k != 10 {
                return vec![];
            }
            let mut x = pair_row(t, 100.0, 10);
            if kind == 0 {
                x.nac_p = 7;
            } else {
                x.lat_e7 = 910_000_000;
            }
            vec![x]
        });
        let (_, s) = build(&r);
        let reason = if kind == 0 { REM_NACP } else { REM_INVALID };
        assert_eq!(removed(&s, reason), 1, "kind {kind}: {s:?}");
        assert_eq!(removed(&s, REM_DUPLICATE), 0, "kind {kind}: {s:?}");
        assert_eq!(s.rows_kept, 30, "kind {kind}");
        assert!(balanced(&s));
    }
}

// REQ: OA-TRUTH-025
#[test]
fn same_time_eligibility_ignores_group_order() {
    // The unusable row carries a high NACp of its own; the usable one has none and would
    // inherit it. Both input orders give the same truth and counts.
    let run = |swap: bool| {
        let r = track_with(&|k, t| {
            if k != 10 {
                return vec![];
            }
            let mut bad = pair_row(t, 100.0, 7);
            bad.nic = 3;
            vec![bad]
        });
        let mut rows = Reports::default();
        let n = reports_len(&r);
        let order: Vec<usize> = if swap {
            (0..n).rev().collect()
        } else {
            (0..n).collect()
        };
        for i in order {
            reports_push(&mut rows, &oa_adsb::reports_row(&r, i).unwrap());
        }
        build(&rows)
    };
    let (t1, s1) = run(false);
    let (t2, s2) = run(true);
    assert_eq!(digest(&t1), digest(&t2));
    assert_eq!(s1.removed, s2.removed);
}

// REQ: OA-TRUTH-026
#[test]
fn identical_same_time_rows_are_processed_in_linear_work() {
    let r = track_with(&|k, t| {
        if k == 10 {
            vec![pair_row(t, 0.0, 10); 999]
        } else {
            vec![]
        }
    });
    let before = pair_evals_this_thread();
    let (_, s) = build(&r);
    let work = pair_evals_this_thread() - before;
    assert!(work <= reports_len(&r) as u64, "pair evaluations {work}");
    assert_eq!(removed(&s, REM_DUPLICATE), 999);
    assert_eq!(s.rows_kept, 30);
}

// REQ: OA-TRUTH-027
#[test]
fn a_group_of_more_than_16_distinct_same_time_rows_is_refused_and_counted() {
    let group = |n: usize| {
        track_with(&|k, t| {
            if k == 10 {
                (1..n).map(|j| pair_row(t, j as f64 * 0.5, 10)).collect()
            } else {
                vec![]
            }
        })
    };
    let (_, s) = build(&group(16));
    assert_eq!(removed(&s, REM_OVERSIZED), 0, "{s:?}");
    assert_eq!(removed(&s, REM_DUPLICATE), 15);
    let (_, s) = build(&group(17));
    assert_eq!(removed(&s, REM_OVERSIZED), 17, "{s:?}");
    assert_eq!(removed(&s, REM_DUPLICATE), 0);
    assert_eq!(s.rows_kept, 29);
    assert!(balanced(&s));
}

/// One pair of same-time rows at report 10: the track's row (NACp `na`) and one `d` m east
/// (NACp `nb`).
fn pair_at(d: f64, na: u8, nb: u8) -> TruthStats {
    let r = track_with(&|k, t| {
        if k == 10 {
            vec![pair_row(t, d, nb)]
        } else {
            vec![]
        }
    });
    // Rewrite the track row's NACp for the pair.
    let mut r2 = Reports::default();
    for i in 0..reports_len(&r) {
        let mut row = oa_adsb::reports_row(&r, i).unwrap();
        if i == 10 {
            row.nac_p = na;
        }
        reports_push(&mut r2, &row);
    }
    build(&r2).1
}

// REQ: OA-TRUTH-028
#[test]
fn two_nacp_8_reports_70_m_apart_are_one_position_measured_twice() {
    let s = pair_at(70.0, 8, 8);
    assert_eq!(removed(&s, REM_DUPLICATE), 1, "{s:?}");
    assert_eq!(s.rows_kept, 30);
}

// REQ: OA-TRUTH-029
#[test]
fn a_pair_far_beyond_the_admitted_uncertainty_is_quarantined() {
    let s = pair_at(400.0, 8, 8);
    assert_eq!((removed(&s, REM_DUPLICATE), s.rows_kept), (2, 29), "{s:?}");
}

// REQ: OA-TRUTH-030
#[test]
fn a_mixed_nacp_pair_is_judged_at_its_own_bound() {
    // NACp 8 with NACp 10: the bound is sqrt(13.82 (37.83^2 + 4.085^2)) = 141.5 m.
    let inside = pair_at(139.0, 8, 10);
    assert_eq!(removed(&inside, REM_DUPLICATE), 1, "{inside:?}");
    let outside = pair_at(144.0, 8, 10);
    assert_eq!(removed(&outside, REM_DUPLICATE), 2, "{outside:?}");
}

/// A 60 s track whose report 10 (t = 20 s) is two rows at one position and NACp, with barometric
/// altitudes `a` and `b` (`NONE_I32` for missing), pushed in the order `swap` selects.
fn alt_pair_track(a: i32, b: i32, swap: bool) -> Reports {
    let mut r = Reports::default();
    for k in 0..30usize {
        let t = k as f64 * 2.0;
        let (la, lo) = to_deg(0.0, 231.5 * t);
        let mut x = good(ADDR, t, la, lo);
        if k == 10 {
            let mut y = x;
            x.baro_alt_ft = a;
            y.baro_alt_ft = b;
            let pair = if swap { [y, x] } else { [x, y] };
            for z in pair {
                reports_push(&mut r, &z);
            }
        } else {
            reports_push(&mut r, &x);
        }
    }
    r
}

/// The altitude of the truth point at t = 20 s, if there is one.
fn alt_at_20s(t: &Truth) -> Option<i32> {
    let i = t.pt_t_ns.iter().position(|&x| x == ns(20.0))?;
    t.pt_alt_ft.get(i).copied()
}

// REQ: OA-TRUTH-047
#[test]
fn a_missing_altitude_never_beats_a_valid_one_at_the_same_instant() {
    for (a, b) in [(NONE_I32, 35_000), (35_000, NONE_I32)] {
        for swap in [false, true] {
            let (t, s) = build(&alt_pair_track(a, b, swap));
            assert_eq!(alt_at_20s(&t), Some(35_000), "a {a} b {b} swap {swap}");
            assert_eq!(removed(&s, REM_DUPLICATE), 1, "{s:?}");
            assert_eq!(removed(&s, REM_AMBIGUOUS), 0, "{s:?}");
            assert!(balanced(&s));
        }
    }
}

// REQ: OA-TRUTH-048
#[test]
fn conflicting_valid_altitudes_at_one_instant_drop_the_instant_and_count_it() {
    for swap in [false, true] {
        let (t, s) = build(&alt_pair_track(35_000, 34_000, swap));
        assert_eq!(alt_at_20s(&t), None, "swap {swap}");
        assert_eq!(removed(&s, REM_AMBIGUOUS), 2, "{s:?}");
        assert_eq!(s.rows_kept, 29);
        assert!(balanced(&s));
    }
    // Within the 25 ft quantum the two are one altitude measured twice.
    let (t, s) = build(&alt_pair_track(35_000, 35_025, false));
    assert!(alt_at_20s(&t).is_some());
    assert_eq!(removed(&s, REM_AMBIGUOUS), 0, "{s:?}");
    let (t2, _) = build(&alt_pair_track(35_025, 35_000, true));
    assert_eq!(digest(&t), digest(&t2));
}

/// A 40-report track at 2 s: the first and the 31st report print quality (NACp 10 and 8), the
/// others none; with an optional far-away row at 60.5 s that prints `jump_nacp`.
fn carry_track(jump_nacp: Option<u8>) -> Reports {
    let mut r = Reports::default();
    for k in 0..40usize {
        let t = k as f64 * 2.0;
        let (la, lo) = to_deg(0.0, 231.5 * t);
        let mut row = good(ADDR, t, la, lo);
        if k > 0 {
            (row.nac_p, row.nic, row.sil, row.version) = (NONE_U8, NONE_U8, NONE_U8, NONE_U8);
        }
        if k == 30 {
            // Renewed here, so the reports after it depend on what the row at 60.5 s leaves.
            row.nac_p = 8;
            row.nic = 8;
            row.sil = 3;
        }
        reports_push(&mut r, &row);
        if k == 30 {
            if let Some(nacp) = jump_nacp {
                let (la, lo) = to_deg(0.0, 231.5 * (t + 0.5) + 60_000.0);
                let mut j = good(ADDR, t + 0.5, la, lo);
                j.nac_p = nacp;
                reports_push(&mut r, &j);
            }
        }
    }
    r
}

// REQ: OA-TRUTH-031
#[test]
fn a_rejected_jump_row_does_not_change_the_carried_quality() {
    let (t0, s0) = build(&carry_track(None));
    let (t1, s1) = build(&carry_track(Some(11)));
    assert_eq!(removed(&s1, REM_JUMP), 1, "{s1:?}");
    assert_eq!(s0.rows_kept, s1.rows_kept);
    assert_eq!(digest(&t0), digest(&t1), "weights differ");
}

// REQ: OA-TRUTH-032
#[test]
fn a_low_quality_jump_row_does_not_remove_later_usable_reports() {
    let (_, s0) = build(&carry_track(None));
    let (_, s1) = build(&carry_track(Some(7)));
    assert_eq!(removed(&s1, REM_NACP), 1, "{s1:?}");
    assert_eq!(s1.rows_kept, s0.rows_kept, "{s1:?}");
    // A low-quality row on the track does still make the reports after it fail.
    let mut r = carry_track(None);
    let (la, lo) = to_deg(0.0, 231.5 * 60.5);
    let mut j = good(ADDR, 60.5, la, lo);
    j.nac_p = 7;
    reports_push(&mut r, &j);
    let (_, s2) = build(&r);
    assert!(removed(&s2, REM_NACP) >= 1, "{s2:?}");
}

// REQ: OA-TRUTH-019
#[test]
fn reserved_addresses_are_never_an_aircraft() {
    // Two aircraft's worth of reports under 0x000000 and 0xFFFFFF, plus a real one.
    let mut r = straight(ADDR, 2.0, 100.0, 5.0, 3);
    let real = reports_len(&r) as u64;
    for addr in [0u32, 0x00ff_ffff] {
        let mut other = straight(addr, 2.0, 100.0, 5.0, 9);
        oa_adsb::reports_extend(&mut r, &std::mem::take(&mut other));
    }
    let (truth, s) = build(&r);
    assert_eq!((s.rows_kept, s.legs, s.aircraft_out), (real, 1, 1));
    assert_eq!(removed(&s, REM_INVALID), 2 * real);
    assert!(balanced(&s));
    for addr in [0u32, 0x00ff_ffff] {
        assert_eq!(
            truth_at(&truth, addr, ms(10.0)),
            TruthQuery::UnknownAircraft
        );
    }
    // A non-ICAO pseudo-address with the same digits is a trackfile identifier, kept.
    let r = straight(NON_ICAO, 2.0, 100.0, 5.0, 4);
    assert_eq!(build(&r).1.legs, 1);
}

// REQ: OA-TRUTH-004
#[test]
fn legs_split_at_the_new_leg_flag_and_at_gaps_over_120_seconds() {
    let mut r = Reports::default();
    // 0..=100 s, a 130 s gap, 230..=330 s, then a new-leg flag at 340 s with no gap.
    for k in 0..51 {
        let t = f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        reports_push(&mut r, &good(ADDR, t, la, lo));
    }
    for k in 0..51 {
        let t = 230.0 + f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        reports_push(&mut r, &good(ADDR, t, la, lo));
    }
    for k in 0..30 {
        let t = 332.0 + f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        let mut row = good(ADDR, t, la, lo);
        if k == 4 {
            row.flags |= F_NEW_LEG;
        }
        reports_push(&mut r, &row);
    }
    let (truth, s) = build(&r);
    assert_eq!(s.legs, 3);
    assert_eq!((s.splits[SPLIT_GAP], s.splits[SPLIT_NEW_LEG]), (1, 1));
    let lr = legs_of(&truth, ADDR);
    assert_eq!((lr.addr, lr.count), (ADDR, 3));
    let l = |i: u32| LegIx(lr.first.0 + i);
    assert_eq!(leg_addr(&truth, l(0)), Some(ADDR));
    assert_eq!(leg_start_ms(&truth, l(0)), Some(ms(0.0)));
    assert_eq!(leg_end_ms(&truth, l(0)), Some(ms(100.0)));
    assert_eq!(leg_start_ms(&truth, l(1)), Some(ms(230.0)));
    assert_eq!(leg_reports(&truth, l(1)), Some(55));
    assert_eq!(leg_end_ms(&truth, l(1)), Some(ms(338.0)));
    assert_eq!(leg_reports(&truth, l(2)), Some(30 - 4));
    assert_eq!(leg_start_ms(&truth, l(2)), Some(ms(340.0)));
    // A gap of exactly 120 s is not a split.
    let mut r = Reports::default();
    for t in [0.0, 2.0, 4.0, 124.0, 126.0, 128.0] {
        let (la, lo) = straight_pos(t);
        reports_push(&mut r, &good(ADDR, t, la, lo));
    }
    assert_eq!(build(&r).1.legs, 1);
    // A new-leg flag on a row that is itself removed still splits at the next kept row.
    let mut r = Reports::default();
    for k in 0..10 {
        let t = f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        let mut row = good(ADDR, t, la, lo);
        if k == 5 {
            row.flags |= F_NEW_LEG | F_STALE;
        }
        reports_push(&mut r, &row);
    }
    let (_, s) = build(&r);
    assert_eq!(
        (s.legs, s.splits[SPLIT_NEW_LEG], removed(&s, REM_STALE)),
        (2, 1, 1)
    );
    assert_eq!(s.rows_kept, 9);
}

// REQ: OA-TRUTH-005
#[test]
fn smoother_recovers_a_straight_track_within_the_stated_tolerance() {
    // 10 minutes, one report every 2 s, 5 m noise per axis (NACp 10 claims 4.1 m).
    let sigma = 5.0;
    let r = straight(ADDR, 2.0, 600.0, sigma, 7);
    let (truth, s) = build(&r);
    assert_eq!(s.legs, 1);
    let (mut sum2, mut vsum2, mut n) = (0.0, 0.0, 0.0);
    let v = 200.0 / 2f64.sqrt();
    // Query between reports (odd seconds) away from the leg ends.
    let mut t = 11.0;
    while t < 590.0 {
        let q = sample(truth_at(&truth, ADDR, ms(t)));
        let e = dist((q.lat_deg, q.lon_deg), straight_pos(t));
        sum2 += e * e;
        let dv = ((q.vel_east_mps - v).powi(2) + (q.vel_north_mps - v).powi(2)).sqrt();
        vsum2 += dv * dv;
        n += 1.0;
        t += 6.0;
    }
    let rms = (sum2 / n).sqrt();
    let vrms = (vsum2 / n).sqrt();
    // The raw reports have RMS horizontal error sigma * sqrt(2) = 7.1 m. Tolerance: the
    // smoothed position under 4 m RMS (the model must stay free to manoeuvre, so it does not
    // average the noise away) and velocity under 1.5 m/s RMS (differencing raw reports gives 3.5).
    assert!(rms < 4.0, "position RMS {rms}");
    assert!(vrms < 1.5, "velocity RMS {vrms}");
    // The model's own 95 % radius is believable: above the actual error scale, below the gate.
    let q = sample(truth_at(&truth, ADDR, ms(301.0)));
    assert!(
        q.sigma95_h_m > rms && q.sigma95_h_m < MAX_SIGMA95_H_M,
        "{}",
        q.sigma95_h_m
    );
    // Heading 045: east and north speeds equal.
    assert!((q.vel_east_mps - q.vel_north_mps).abs() < 1.0);
}

// REQ: OA-TRUTH-006
#[test]
fn smoother_follows_a_constant_rate_turn() {
    // 200 m/s on an 8 km radius circle (1.43 deg/s, 5 m/s^2 lateral), about one lap, 5 m noise.
    let sigma = 5.0;
    let (radius, speed) = (8000.0, 200.0);
    let w = speed / radius;
    let pos = |t: f64| to_deg(radius * (w * t).sin(), radius * (1.0 - (w * t).cos()));
    let mut rng = Rng(11);
    let mut r = Reports::default();
    let mut t = 0.0;
    while t <= 250.0 {
        let (e, n) = (radius * (w * t).sin(), radius * (1.0 - (w * t).cos()));
        let (la, lo) = to_deg(e + sigma * rng.normal(), n + sigma * rng.normal());
        reports_push(&mut r, &good(ADDR, t, la, lo));
        t += 2.0;
    }
    let (truth, _) = build(&r);
    let (mut sum2, mut n) = (0.0, 0.0);
    let mut worst = 0.0f64;
    let mut vworst = 0.0f64;
    let mut t = 11.0;
    while t < 240.0 {
        let q = sample(truth_at(&truth, ADDR, ms(t)));
        let e = dist((q.lat_deg, q.lon_deg), pos(t));
        sum2 += e * e;
        worst = worst.max(e);
        let (ve, vn) = (speed * (w * t).cos(), speed * (w * t).sin());
        vworst =
            vworst.max(((q.vel_east_mps - ve).powi(2) + (q.vel_north_mps - vn).powi(2)).sqrt());
        n += 1.0;
        t += 6.0;
    }
    let rms = (sum2 / n).sqrt();
    // Tolerances: RMS under 6 m (raw noise is 7.1 m), worst under 20 m, velocity under 5 m/s.
    assert!(rms < 6.0, "turn RMS {rms}");
    assert!(worst < 20.0, "turn worst {worst}");
    assert!(vworst < 5.0, "turn velocity worst {vworst}");
}

// REQ: OA-TRUTH-007
#[test]
fn a_long_leg_stays_accurate_in_the_local_frame() {
    // 400 km due north along a meridian at 250 m/s, 3 m noise, a report every 4 s. Positions
    // are placed on a 6,371 km sphere (111,195 m per degree), the units `dist` measures in.
    let sigma = 3.0;
    let mut rng = Rng(23);
    let mut r = Reports::default();
    let path = |t: f64| (LAT0 - 1.0 + 250.0 * t / M_PER_DEG, LON0);
    let mut t = 0.0;
    while t <= 1600.0 {
        let (la, lo) = path(t);
        let lat = la + sigma * rng.normal() / M_PER_DEG;
        let lon = lo + sigma * rng.normal() / (M_PER_DEG * la.to_radians().cos());
        reports_push(&mut r, &good(ADDR, t, lat, lon));
        t += 4.0;
    }
    let (truth, s) = build(&r);
    assert_eq!((s.legs, s.splits[SPLIT_FRAME]), (1, 0));
    let (mut sum2, mut n) = (0.0, 0.0);
    let mut t = 50.0;
    while t < 1550.0 {
        let q = sample(truth_at(&truth, ADDR, ms(t)));
        let e = dist((q.lat_deg, q.lon_deg), path(t));
        sum2 += e * e;
        n += 1.0;
        // Speed is measured on the ellipsoid, the track was laid on a sphere: 0.1 % apart.
        assert!((q.vel_north_mps - 250.0).abs() < 2.0, "{}", q.vel_north_mps);
        assert!(q.vel_east_mps.abs() < 1.0, "{}", q.vel_east_mps);
        t += 17.0;
    }
    let rms = (sum2 / n).sqrt();
    assert!(rms < 4.0, "long-leg RMS {rms}");
}

// REQ: OA-TRUTH-008
#[test]
fn altitude_is_passed_through_and_interpolated_only_between_the_bracketing_reports() {
    // A climb at 1,500 fpm sampled every 2 s, altitude quantised to 25 ft.
    let mut r = Reports::default();
    for k in 0..100 {
        let t = f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        let mut row = good(ADDR, t, la, lo);
        row.baro_alt_ft = (((10_000.0 + 25.0 * t) / 25.0).round() * 25.0) as i32;
        row.vrate_fpm = 1500;
        // Reports 50 and 51 have no barometric altitude (a geometric-only row).
        if k == 50 || k == 51 {
            row.baro_alt_ft = NONE_I32;
            row.vrate_fpm = NONE_I32;
        }
        reports_push(&mut r, &row);
    }
    let (truth, _) = build(&r);
    // At a report time the altitude is exactly the reported value (no smoothing).
    let q = sample(truth_at(&truth, ADDR, ms(40.0)));
    assert_eq!(q.baro_alt_ft, Some(11_000.0));
    // Between two reports it is their linear interpolation.
    let q = sample(truth_at(&truth, ADDR, ms(41.0)));
    assert_eq!(q.baro_alt_ft, Some(11_025.0));
    assert_eq!(q.vert_rate_fpm, Some(1500.0));
    // Next to reports without altitude (100 s and 102 s) there is none, and none is invented.
    let q = sample(truth_at(&truth, ADDR, ms(99.0)));
    assert_eq!(q.baro_alt_ft, None);
    assert_eq!(q.vert_rate_fpm, None);
    let q = sample(truth_at(&truth, ADDR, ms(101.0)));
    assert_eq!(q.baro_alt_ft, None);
    // Without a reported rate the altitude difference over a bracket of 2 s or more gives it.
    let mut r2 = Reports::default();
    for k in 0..20 {
        let t = f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        let mut row = good(ADDR, t, la, lo);
        row.baro_alt_ft = 10_000 + 50 * k;
        reports_push(&mut r2, &row);
    }
    let (t2, _) = build(&r2);
    let q = sample(truth_at(&t2, ADDR, ms(11.0)));
    assert!((q.vert_rate_fpm.unwrap() - 1500.0).abs() < 1e-6);
}

// REQ: OA-TRUTH-009
#[test]
fn there_is_no_extrapolation_and_every_refusal_has_a_reason() {
    // Reports every 2 s from 0 to 100 s, a 30 s hole, then 130 to 200 s: one leg.
    let mut r = Reports::default();
    for t in (0..=50).chain(65..=100).map(|k| f64::from(k) * 2.0) {
        let (la, lo) = straight_pos(t);
        reports_push(&mut r, &good(ADDR, t, la, lo));
    }
    let (truth, s) = build(&r);
    assert_eq!(s.legs, 1);
    // Inside, between reports and exactly on one: truth.
    assert!(matches!(
        truth_at(&truth, ADDR, ms(41.0)),
        TruthQuery::Ok(_)
    ));
    assert!(matches!(
        truth_at(&truth, ADDR, ms(100.0)),
        TruthQuery::Ok(_)
    ));
    // Edges: nothing before the first or after the last report, even by a millisecond.
    assert_eq!(truth_at(&truth, ADDR, ms(0.0) - 1), TruthQuery::OutsideLegs);
    assert_eq!(
        truth_at(&truth, ADDR, ms(200.0) + 1),
        TruthQuery::OutsideLegs
    );
    assert!(matches!(truth_at(&truth, ADDR, ms(0.0)), TruthQuery::Ok(_)));
    assert!(matches!(
        truth_at(&truth, ADDR, ms(200.0)),
        TruthQuery::Ok(_)
    ));
    // The hole (100 to 130 s): a real report within 10 s on both sides is needed.
    assert_eq!(
        truth_at(&truth, ADDR, ms(115.0)),
        TruthQuery::NoNearbyReport
    );
    assert_eq!(
        truth_at(&truth, ADDR, ms(105.0)),
        TruthQuery::NoNearbyReport
    ); // 5 s from 100, 25 s from 130
    assert_eq!(
        truth_at(&truth, ADDR, ms(125.0)),
        TruthQuery::NoNearbyReport
    );
    assert_eq!(
        truth_at(&truth, ADDR, ms(100.0) + 1),
        TruthQuery::NoNearbyReport
    );
    // A bracket of exactly 20 s (10 s each side) is enough.
    let mut r = Reports::default();
    for t in [
        0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 30.0, 32.0, 34.0, 36.0, 38.0, 40.0,
    ] {
        let (la, lo) = straight_pos(t);
        reports_push(&mut r, &good(ADDR, t, la, lo));
    }
    let (t20, _) = build(&r);
    assert!(matches!(truth_at(&t20, ADDR, ms(20.0)), TruthQuery::Ok(_)));
    assert_eq!(truth_at(&t20, ADDR, ms(19.0)), TruthQuery::NoNearbyReport);
    assert_eq!(truth_at(&t20, ADDR, ms(21.0)), TruthQuery::NoNearbyReport);
    // Unknown aircraft, and an address that differs only by the non-ICAO bit.
    assert_eq!(
        truth_at(&truth, 0xabcdef, ms(50.0)),
        TruthQuery::UnknownAircraft
    );
    assert_eq!(
        truth_at(&truth, ADDR | NON_ICAO, ms(50.0)),
        TruthQuery::UnknownAircraft
    );
    assert_eq!(legs_of(&truth, 0xabcdef).count, 0);
    // Between legs, and before and after all of them: no bracket (not extrapolated).
    let mut r = Reports::default();
    for t in (0..30).chain(200..230) {
        let (la, lo) = straight_pos(f64::from(t));
        reports_push(&mut r, &good(ADDR, f64::from(t), la, lo));
    }
    let (two, s) = build(&r);
    assert_eq!(s.legs, 2);
    assert_eq!(truth_at(&two, ADDR, ms(100.0)), TruthQuery::OutsideLegs);
    assert_eq!(truth_at(&two, ADDR, ms(35.0)), TruthQuery::OutsideLegs);
    assert_eq!(truth_at(&two, ADDR, ms(-50.0)), TruthQuery::OutsideLegs);
    assert_eq!(truth_at(&two, ADDR, ms(500.0)), TruthQuery::OutsideLegs);
    assert_eq!(truth_at(&two, ADDR, i64::MAX), TruthQuery::OutsideLegs);
    let q = sample(truth_at(&two, ADDR, ms(210.0)));
    assert_eq!(q.leg, LegIx(legs_of(&two, ADDR).first.0 + 1));
}

// REQ: OA-TRUTH-010
#[test]
fn truth_that_is_too_uncertain_is_refused() {
    // NACp 8 reports whose noise is taken at five times its bound (sigma_scale 5), 10 s apart:
    // the smoothed position is not good enough for 50 m.
    let make = |nacp: u8| {
        let mut r = Reports::default();
        for k in 0..6 {
            let t = f64::from(k) * 10.0;
            let (la, lo) = straight_pos(t);
            let mut row = good(ADDR, t, la, lo);
            row.nac_p = nacp;
            reports_push(&mut r, &row);
        }
        r
    };
    let cfg = DEFAULT_CONFIG;
    let mut loose = cfg;
    loose.sigma_scale = 5.0;
    let (truth, _) = build_truth(&make(8), &loose).unwrap();
    assert_eq!(truth_at(&truth, ADDR, ms(25.0)), TruthQuery::TooUncertain);
    // The same reports with NACp 11 are fine.
    let (truth, _) = build_truth(&make(11), &cfg).unwrap();
    let q = sample(truth_at(&truth, ADDR, ms(25.0)));
    assert!(q.sigma95_h_m < MAX_SIGMA95_H_M);
    // And a stricter gate turns that into a refusal.
    let mut tight = cfg;
    tight.max_sigma95_h_m = 0.5;
    let (truth, _) = build_truth(&make(11), &tight).unwrap();
    assert_eq!(truth_at(&truth, ADDR, ms(25.0)), TruthQuery::TooUncertain);
}

// REQ: OA-TRUTH-011
#[test]
fn leave_one_out_measures_the_residuals_on_known_noise() {
    // A straight track with 5 m noise per axis, a report every 2 s: every fifth is hidden.
    let sigma = 5.0;
    let r = straight(ADDR, 2.0, 1000.0, sigma, 99);
    let (_, s) = build(&r);
    let l = s.loo;
    assert_eq!(l.stride, 5);
    assert_eq!(l.hidden, 100);
    assert!(l.horizontal.n >= 95, "{}", l.horizontal.n);
    // The hidden report's own noise (7.1 m RMS) plus the prediction's: RMS between 6 and 11 m.
    let d = l.horizontal;
    assert!(d.rms > 6.0 && d.rms < 11.0, "RMS {}", d.rms);
    assert!(d.median <= d.p95 && d.p95 <= d.p99 && d.p99 <= d.max);
    // Cross and along track carry the same noise per axis, about 5.4 m each, and no bias.
    for c in [l.cross, l.along] {
        assert!(c.rms > 4.0 && c.rms < 8.0, "{c:?}");
        assert!(c.mean.abs() < 2.0, "{c:?}");
    }
    // No altitude was given.
    assert_eq!(l.alt.n, 0);
    // The model's 95 % radius is honest: the hidden report falls inside it about 95 % of the
    // time (the NACp 10 claim of 4.1 m is a little optimistic for 5 m noise).
    let cov = l.coverage95.hit as f64 / l.coverage95.n as f64;
    assert!(cov > 0.85 && cov <= 1.0, "coverage {cov}");
    assert_eq!(l.under_gate.hit, l.under_gate.n);
    assert_eq!(s.unc_at_reports.hit, s.unc_at_reports.n);
    assert_eq!(s.unc_at_midpoints.n, 500);
    // Altitude residual: a linear climb in whole feet is recovered exactly.
    let mut r = Reports::default();
    for k in 0..100 {
        let t = f64::from(k) * 2.0;
        let (la, lo) = straight_pos(t);
        let mut row = good(ADDR, t, la, lo);
        row.baro_alt_ft = 10_000 + 50 * k;
        reports_push(&mut r, &row);
    }
    let (_, s) = build(&r);
    assert_eq!(s.loo.alt.n, 20);
    assert!(s.loo.alt.max < 1e-6 && s.loo.alt.mean.abs() < 1e-6);
    // Switching it off leaves zeros.
    let mut cfg = DEFAULT_CONFIG;
    cfg.loo = false;
    let (_, s) = build_truth(&r, &cfg).unwrap();
    assert_eq!(s.loo, LooStats::default());
}

// REQ: OA-TRUTH-012
#[test]
fn the_same_input_gives_the_same_bytes_in_any_input_order() {
    let mut r = straight(ADDR, 2.0, 300.0, 5.0, 5);
    oa_adsb::reports_extend(&mut r, &straight(0xa00001, 3.0, 300.0, 5.0, 6));
    oa_adsb::reports_extend(&mut r, &straight(0xa00001 | NON_ICAO, 3.0, 300.0, 5.0, 8));
    let (t1, s1) = build(&r);
    let (t2, s2) = build(&r);
    assert_eq!(digest(&t1), digest(&t2));
    assert_eq!(t1, t2);
    assert_eq!(format!("{s1:?}"), format!("{s2:?}"));
    // Reversed input gives the same model: reports are put in a total order before use.
    let mut rev = Reports::default();
    for i in (0..reports_len(&r)).rev() {
        reports_push(&mut rev, &oa_adsb::reports_row(&r, i).unwrap());
    }
    let (t3, s3) = build(&rev);
    assert_eq!(digest(&t1), digest(&t3));
    assert_eq!(format!("{s1:?}"), format!("{s3:?}"));
    // A change of one unit in one report changes the digest.
    r.lat_e7[10] += 1;
    let (t4, _) = build(&r);
    assert_ne!(digest(&t1), digest(&t4));
}

// REQ: OA-TRUTH-013
#[test]
fn non_icao_addresses_are_separate_aircraft() {
    let mut r = straight(0xa00001, 2.0, 100.0, 3.0, 1);
    oa_adsb::reports_extend(&mut r, &straight(0xa00001 | NON_ICAO, 2.0, 100.0, 3.0, 2));
    let (truth, s) = build(&r);
    assert_eq!((s.aircraft_in, s.aircraft_out, s.legs), (2, 2, 2));
    assert_eq!(aircraft_addrs(&truth), &[0xa00001, 0xa00001 | NON_ICAO]);
    assert_eq!(legs_of(&truth, 0xa00001).count, 1);
    assert_eq!(legs_of(&truth, 0xa00001 | NON_ICAO).count, 1);
    assert_eq!(leg_count(&truth), 2);
}

// REQ: OA-TRUTH-014
#[test]
fn bad_input_is_refused_with_a_typed_error() {
    let mut r = straight(ADDR, 2.0, 20.0, 3.0, 1);
    r.sda.pop();
    assert_eq!(
        build_truth(&r, &DEFAULT_CONFIG).unwrap_err(),
        TruthError::ColumnLength
    );
    let r = straight(ADDR, 2.0, 20.0, 3.0, 1);
    let bad = |f: &dyn Fn(&mut TruthConfig)| {
        let mut c = DEFAULT_CONFIG;
        f(&mut c);
        build_truth(&r, &c).unwrap_err()
    };
    assert!(matches!(bad(&|c| c.min_nacp = 0), TruthError::BadConfig(_)));
    assert!(matches!(
        bad(&|c| c.accel_q = f64::NAN),
        TruthError::BadConfig(_)
    ));
    assert!(matches!(
        bad(&|c| c.sigma_scale = -1.0),
        TruthError::BadConfig(_)
    ));
    assert!(matches!(
        bad(&|c| c.bracket_ms = 0),
        TruthError::BadConfig(_)
    ));
    assert!(matches!(
        bad(&|c| c.leg_gap_ms = i64::MAX),
        TruthError::BadConfig(_)
    ));
    assert!(matches!(
        bad(&|c| c.min_leg_points = 2),
        TruthError::BadConfig(_)
    ));
    assert!(matches!(
        bad(&|c| c.loo_stride = 1),
        TruthError::BadConfig(_)
    ));
    // Out-of-range coordinates are removed and counted, not passed on.
    let mut r = straight(ADDR, 2.0, 20.0, 3.0, 1);
    r.lat_e7[3] = 950_000_000;
    let (_, s) = build(&r);
    assert_eq!(removed(&s, REM_INVALID), 1);
    assert!(balanced(&s));
    // Empty input is an empty truth.
    let (truth, s) = build(&Reports::default());
    assert_eq!((s.rows_in, s.legs, leg_count(&truth)), (0, 0, 0));
    assert_eq!(truth_at(&truth, ADDR, 0), TruthQuery::UnknownAircraft);
    // The gate values are the design's.
    assert_eq!(
        (
            MIN_NACP,
            MIN_NIC,
            REQUIRED_SIL,
            MIN_VERSION,
            QUALITY_CARRY_MS,
            LEG_GAP_MS,
            BRACKET_MS
        ),
        (8, 7, 3, 2, 60_000, 120_000, 10_000)
    );
    assert_eq!(MAX_SIGMA95_H_M, 50.0);
    assert_eq!(ns(0.0), T0);
}

/// The ZAU clip, if the environment names it and it exists.
fn clip() -> Option<Reports> {
    use oa_adsb::decode_payload;
    use oa_feed::{FrameReader, Kind, MAX_FRAME_LEN};
    let path = std::env::var("OPENATCA_ADSB_CLIP").ok()?;
    let bytes = std::fs::read(&path).ok()?;
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut rd = FrameReader::new(&mut buf).unwrap();
    let mut out = Reports::default();
    let mut at = 0;
    while at < bytes.len() {
        let end = (at + (1 << 20)).min(bytes.len());
        let took = rd.push(&bytes[at..end]);
        at += took;
        while let Some(f) = rd.next() {
            let f = f.unwrap();
            if f.kind == Kind::Message {
                decode_payload(f.payload, &mut out).unwrap();
            }
        }
        assert!(took > 0);
    }
    Some(out)
}

// REQ: OA-TRUTH-015
#[test]
fn real_clip_builds_a_balanced_deterministic_calibrated_truth() {
    let Some(r) = clip() else {
        eprintln!("skipped: OPENATCA_ADSB_CLIP not set or unreadable");
        return;
    };
    let (truth, s) = build(&r);
    eprintln!("{s:#?}");
    assert!(balanced(&s));
    assert!(s.legs > 100 && s.rows_kept > s.rows_in / 2);
    assert!(s.loo.hidden > 0);
    // The model's uncertainty is honest to within a few points of 95 %.
    let cov = s.loo.coverage95.hit as f64 / s.loo.coverage95.n as f64;
    assert!(cov > 0.85 && cov < 0.995, "coverage {cov}");
    // Truth is far better than the 50 m gate on the real data.
    assert!(s.loo.horizontal.p95 < 50.0, "{:?}", s.loo.horizontal);
    // No query outside a leg is answered: one millisecond before each leg's first report is
    // never truth (it is before the first report of the leg, whatever precedes it).
    let mut asked = 0;
    for &a in aircraft_addrs(&truth) {
        let lr = legs_of(&truth, a);
        for i in 0..lr.count {
            let l = LegIx(lr.first.0 + i);
            let st = leg_start_ns(&truth, l).unwrap();
            if i == 0 {
                assert_eq!(
                    truth_at(&truth, a, (st - 1) / 1_000_000 - 1),
                    TruthQuery::OutsideLegs
                );
                asked += 1;
            }
        }
    }
    assert!(asked > 100);
    let (t2, s2) = build(&r);
    assert_eq!(digest(&truth), digest(&t2));
    assert_eq!(format!("{s:?}"), format!("{s2:?}"));
}

// REQ: OA-TRUTH-033
#[test]
fn a_setting_that_can_make_the_covariance_non_finite_is_refused() {
    let mut r = Reports::default();
    for k in 0..3 {
        let mut row = good(ADDR, f64::from(k), LAT0, LON0);
        row.nac_p = 8;
        reports_push(&mut r, &row);
    }
    for (name, f) in [
        (
            "sigma_scale",
            (|c: &mut TruthConfig| c.sigma_scale = 1e153) as fn(&mut TruthConfig),
        ),
        ("accel_q", |c| c.accel_q = 1e300),
    ] {
        let mut c = DEFAULT_CONFIG;
        c.loo = false;
        f(&mut c);
        match build_truth(&r, &c) {
            Err(TruthError::BadConfig(w)) => assert_eq!(w, name),
            Err(e) => panic!("{name}: wrong refusal {e:?}"),
            Ok(_) => panic!("{name}: the setting was accepted"),
        }
    }
    // The largest accepted values still build.
    let mut c = DEFAULT_CONFIG;
    c.sigma_scale = MAX_SIGMA_SCALE;
    c.accel_q = MAX_ACCEL_Q;
    assert!(build_truth(&r, &c).is_ok());
}

// REQ: OA-TRUTH-034
#[test]
fn a_non_finite_variance_is_never_a_radius() {
    assert_eq!(sigma95_h_m(f64::NAN), None);
    assert_eq!(sigma95_h_m(f64::INFINITY), None);
    assert_eq!(sigma95_h_m(f64::NEG_INFINITY), None);
    assert_eq!(sigma95_h_m(-1e-12), Some(0.0));
    let s = sigma95_h_m(4.0).unwrap();
    assert!((s - 2.0 * SIGMA95).abs() < 1e-12, "{s}");
}

/// A two-knot truth about (0, 0) with the given smoothed state at both knots, 1 s apart from
/// 1e9 ms.
fn two_knots(x: [f64; 6], p: [f64; 6]) -> Truth {
    let mut t = Truth {
        cfg: TruthConfig {
            loo: false,
            ..DEFAULT_CONFIG
        },
        ..Truth::default()
    };
    t.ac_addr.push(ADDR);
    t.ac_first_leg = vec![0, 1];
    t.leg_addr.push(ADDR);
    t.leg_first_pt = vec![0, 2];
    t.leg_lat0.push(0.0);
    t.leg_lon0.push(0.0);
    for k in 0..2i64 {
        t.pt_t_ns.push(T0 + k * 1_000_000_000);
        t.pt_alt_ft.push(10_000);
        t.pt_vrate_fpm.push(0);
        t.pt_xf.push(x);
        t.pt_xs.push(x);
        t.pt_pf.push(p);
        t.pt_ps.push(p);
    }
    t
}

// REQ: OA-TRUTH-041
#[test]
fn a_state_that_converts_to_a_non_finite_position_is_never_returned_as_truth() {
    let p = [1.0e-4, 0.0, 0.0, 1.0e-6, 0.0, 1.0e-6];
    let t_ms = T0 / 1_000_000;
    for x in [
        [1.0e308, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, -1.0e308, 0.0, 0.0],
        [1.0e7, 1.0e308, 0.0, 0.0, 0.0, 0.0],
        [1.0e7, 0.0, 0.0, 1.0e7, 1.0e308, 0.0],
    ] {
        let t = two_knots(x, p);
        let q = truth_at(&t, ADDR, t_ms);
        assert!(!matches!(q, TruthQuery::Ok(_)), "{x:?} gave {q:?}");
    }
    // An ordinary state is still truth.
    let ok = two_knots([1.0e4, 100.0, 0.0, 2.0e4, 50.0, 0.0], p);
    assert!(matches!(truth_at(&ok, ADDR, t_ms), TruthQuery::Ok(_)));
}

// REQ: OA-TRUTH-049
#[test]
fn a_state_at_the_antipode_of_the_leg_frame_is_a_refusal_and_not_a_nan_sample() {
    let p = [1.0e-4, 0.0, 0.0, 1.0e-6, 0.0, 1.0e-6];
    let t_ms = T0 / 1_000_000;
    // Half the sphere's circumference east of the leg origin, and a velocity that is finite.
    let t = two_knots([2.0015e7, 100.0, 0.0, 0.0, 50.0, 0.0], p);
    assert_eq!(truth_at(&t, ADDR, t_ms), TruthQuery::TooUncertain);
}

fn at_ns(t: i64) -> Row {
    let mut r = good(ADDR, 0.0, LAT0, LON0);
    r.time_ns = t;
    r
}

// REQ: OA-TRUTH-043
#[test]
fn reports_whose_times_differ_by_more_than_an_i64_do_not_overflow_the_builder() {
    let mut r = Reports::default();
    reports_push(&mut r, &at_ns(-4_700_000_000_000_000_000));
    reports_push(&mut r, &at_ns(4_700_000_000_000_000_000));
    let (t, s) = build(&r);
    assert!(balanced(&s));
    assert_eq!(s.rows_in, 2);
    // The two reports are far apart in time: they are two short legs, not one.
    assert_eq!(leg_count(&t), 0);
    // Queries at the extremes of the clock refuse and do not overflow.
    for t_ms in [i64::MIN, i64::MAX, 4_700_000_000_000] {
        assert!(!matches!(truth_at(&t, ADDR, t_ms), TruthQuery::Ok(_)));
    }
}

// REQ: OA-TRUTH-043
#[test]
fn a_leg_of_reports_at_the_extremes_of_the_clock_is_built_without_overflow() {
    let mut r = Reports::default();
    let mut cfg = DEFAULT_CONFIG;
    cfg.min_leg_points = 3;
    for k in 0..4 {
        reports_push(&mut r, &at_ns(i64::MAX - 4_000_000_000 + k * 1_000_000_000));
    }
    for k in 0..4 {
        reports_push(&mut r, &at_ns(i64::MIN + k * 1_000_000_000));
    }
    let (t, s) = build_truth(&r, &cfg).unwrap();
    assert!(balanced(&s));
    assert_eq!(leg_count(&t), 2);
    for t_ms in [i64::MIN, i64::MAX, 0] {
        let _ = truth_at(&t, ADDR, t_ms);
    }
}

// REQ: OA-TRUTH-044
#[test]
fn a_report_time_far_before_the_held_time_does_not_carry_quality() {
    let cfg = DEFAULT_CONFIG;
    let mut st = QualityState::default();
    let first = at_ns(1);
    let v = quality_check(&mut st, &first, 1, &cfg);
    assert_eq!(v.removal, None);
    // A row with no quality values of its own, a time whose difference from the held one does not
    // fit an i64: not within the carry age, so nothing is carried.
    let mut bare = at_ns(i64::MIN);
    bare.nic = NONE_U8;
    bare.nac_p = NONE_U8;
    bare.sil = NONE_U8;
    bare.version = NONE_U8;
    let v = quality_check(&mut st, &bare, i64::MIN, &cfg);
    assert!(!v.carried, "{v:?}");
    assert_eq!(v.removal, Some(REM_NO_QUALITY));
}

// REQ: OA-TRUTH-044
#[test]
fn a_query_whose_distance_to_a_report_does_not_fit_an_i64_is_not_near_one() {
    let mut r = Reports::default();
    for k in 0..4 {
        reports_push(&mut r, &at_ns(T0 + k * 1_000_000_000));
    }
    let (mut t, _) = build(&r);
    assert_eq!(leg_count(&t), 1);
    // A leg that spans more than an i64 of nanoseconds cannot be built (legs split at gaps), so
    // the columns are set by hand to reach the query's time arithmetic.
    t.pt_t_ns = vec![i64::MIN + 1, i64::MIN + 2, i64::MAX - 1, i64::MAX];
    assert_eq!(
        truth_at(&t, ADDR, 1_000_000_000_000),
        TruthQuery::NoNearbyReport
    );
}

// REQ: OA-TRUTH-044
#[test]
fn no_leg_has_a_step_longer_than_the_leg_gap() {
    // The smoother and the queries subtract report times of one leg without a check; this is the
    // invariant that makes that safe.
    let mut r = Reports::default();
    let gap = DEFAULT_CONFIG.leg_gap_ms * 1_000_000;
    for base in [T0, i64::MIN + 5_000_000_000, i64::MAX - 1_000_000_000_000] {
        let mut t = base;
        for k in 0..8i64 {
            reports_push(&mut r, &at_ns(t));
            // Steps of exactly the gap, then one just over it.
            t = t.saturating_add(if k == 4 { gap + 1 } else { gap });
        }
    }
    let mut cfg = DEFAULT_CONFIG;
    cfg.min_leg_points = 3;
    let (t, s) = build_truth(&r, &cfg).unwrap();
    assert!(balanced(&s));
    assert!(leg_count(&t) > 0);
    for l in 0..leg_count(&t) {
        let (Some(&lo), Some(&hi)) = (t.leg_first_pt.get(l), t.leg_first_pt.get(l + 1)) else {
            panic!("leg offsets");
        };
        let pts = &t.pt_t_ns[lo as usize..hi as usize];
        for w in pts.windows(2) {
            let d = w[1].checked_sub(w[0]).unwrap();
            assert!(d > 0 && d <= gap, "{d}");
        }
    }
}

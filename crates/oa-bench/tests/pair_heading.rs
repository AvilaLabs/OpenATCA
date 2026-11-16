//! A pair whose truth is too slow for a heading is excluded from the heading statistics with a
//! counted reason. Every identifier and position is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::Assoc;
use oa_bench::legacy::{flight_ix, new_legacy, push_th};
use oa_bench::pair::{build_pairs, PairConfig};
use oa_bench::thindex::build_th_index;
use oa_bench::truth::{NoTruth, Truth, TruthLeg, TruthSample};

const ADDR: u32 = 0xA1_0004;
const KT_MPS: f64 = 0.514_444;

type Excl = Vec<((&'static str, &'static str), u64)>;

/// One ERAM sample with velocity against a truth moving east at `truth_kt`.
fn exclusions(truth_kt: f64) -> (usize, Excl) {
    let q = |_: u32, _: i64| -> Result<TruthSample, NoTruth> {
        Ok(TruthSample {
            lat: 41.0,
            lon: -88.0,
            baro_alt_ft: Some(300),
            vel_east_mps: truth_kt * KT_MPS,
            vel_north_mps: 0.0,
            vert_rate_fpm: Some(0),
            sigma95_h_m: 5.0,
            leg: 0,
        })
    };
    let legs = [TruthLeg {
        addr: ADDR,
        start_ms: 0,
        end_ms: 100_000,
        callsign: None,
        start: Default::default(),
    }];
    let truth = Truth {
        query: &q,
        legs: &legs,
    };
    let mut leg = new_legacy();
    let f = flight_ix(&mut leg, 1).unwrap();
    push_th(
        &mut leg,
        f,
        10_000,
        10_000,
        410_000_000,
        -880_000_000,
        300,
        Some((10 * truth_kt as i16, 0)),
        false,
    );
    let ix = build_th_index(&leg.th, 1);
    let assoc = Assoc {
        addr: vec![ADDR],
        method: vec![0],
        status: vec![0],
        ..Assoc::default()
    };
    let (p, ex) = build_pairs(&leg, &ix, &assoc, &truth, &PairConfig::default());
    (
        p.hdg_err_deg.iter().filter(|h| h.is_finite()).count(),
        ex.into_iter().collect(),
    )
}

// REQ: OA-BENCH-127
#[test]
fn a_pair_with_truth_below_30_kt_is_excluded_from_heading_with_its_reason() {
    let (with_heading, ex) = exclusions(25.0);
    assert_eq!(with_heading, 0);
    assert!(
        ex.iter().any(|((scope, why), n)| *scope == "T3-heading"
            && *why == "truth speed below 30 kt"
            && *n == 1),
        "{ex:?}"
    );
}

// REQ: OA-BENCH-127
#[test]
fn a_pair_at_or_above_30_kt_keeps_its_heading_and_no_exclusion() {
    let (with_heading, ex) = exclusions(60.0);
    assert_eq!(with_heading, 1);
    assert!(!ex.iter().any(|((s, _), _)| *s == "T3-heading"), "{ex:?}");
}

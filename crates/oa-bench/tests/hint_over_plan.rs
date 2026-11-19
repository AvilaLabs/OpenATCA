//! A plan address that fails the plausibility check, or that truth does not know, gives way to a
//! different track address hint that passes it; every other case is unchanged.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::assoc::{associate, astatus, index_truth, AssocConfig};
use oa_bench::legacy::{flight_ix, new_legacy, push_th, Legacy, NONE, NO_TIME};
use oa_bench::pair::{build_pairs, PairConfig};
use oa_bench::run::{find, run_benchmark};
use oa_bench::stats::method;
use oa_bench::synth::{env, self_test_config, T0_MS};
use oa_bench::thindex::build_th_index;
use oa_bench::truth::{NoTruth, Truth, TruthLeg, TruthSample};

const NEAR: u32 = 0xB00001;
const FAR: u32 = 0xA00001;
/// Far from the flights, with a callsign that contradicts theirs.
const OTHER: u32 = 0xC00001;
const UNKNOWN: u32 = 0xDEAD00;

fn sample(lat: f64, t_ms: i64) -> TruthSample {
    let s = (t_ms - T0_MS) as f64 / 1000.0;
    TruthSample {
        lat,
        lon: -88.0 + 200.0 * s / 82_000.0,
        baro_alt_ft: Some(30_000),
        vel_east_mps: 200.0,
        vel_north_mps: 0.0,
        vert_rate_fpm: Some(0),
        sigma95_h_m: 5.0,
        leg: 0,
    }
}

fn legs() -> Vec<TruthLeg> {
    let mk = |addr, callsign| TruthLeg {
        addr,
        start_ms: T0_MS,
        end_ms: T0_MS + 1_000_000,
        callsign,
        start: Default::default(),
    };
    vec![
        mk(NEAR, None),
        mk(FAR, None),
        mk(OTHER, Some(*b"OTHER1\0\0")),
    ]
}

/// Flights: (plan address, hint address, samples). All are flown over `NEAR`'s track.
fn world(flights: &[(u32, u32, i64)]) -> Legacy {
    let mut leg = new_legacy();
    for (k, &(plan, hint, n)) in flights.iter().enumerate() {
        let f = flight_ix(&mut leg, k as u128 + 1).unwrap();
        leg.flights.addr[f as usize] = plan;
        leg.flights.addr_hint[f as usize] = hint;
        leg.flights.callsign[f as usize][..6].copy_from_slice(b"HINT01");
        for i in 0..n {
            let t = sample(41.0, T0_MS + 12_000 * i);
            push_th(
                &mut leg,
                f,
                T0_MS + 12_000 * i,
                NO_TIME,
                (t.lat * 1e7) as i32,
                (t.lon * 1e7) as i32,
                30_000,
                None,
                false,
            );
        }
    }
    leg
}

fn query(a: u32, t: i64) -> Result<TruthSample, NoTruth> {
    match a {
        NEAR => Ok(sample(41.0, t)),
        FAR | OTHER => Ok(sample(46.0, t)),
        _ => Err(NoTruth::UnknownAircraft),
    }
}

fn flights() -> Vec<(u32, u32, i64)> {
    vec![
        (FAR, NEAR, 6),     // 0: plan far, hint plausible: hint used
        (OTHER, NEAR, 3),   // 1: plan contradicts, hint unchecked: flagged
        (NEAR, FAR, 6),     // 2: plan plausible, other hint: plan used
        (UNKNOWN, NEAR, 6), // 3: plan unknown to truth, hint plausible: hint used
        (NEAR, NEAR, 6),    // 4: hint equal to plan: unchanged
        (FAR, UNKNOWN, 6),  // 5: plan far, hint unknown to truth: flagged
        (OTHER, NEAR, 6),   // 6: plan callsign contradicts, hint plausible: hint used
        (FAR, NONE, 6),     // 7: plan far, no hint: flagged
        (UNKNOWN, NONE, 6), // 8: plan unknown, no hint: no truth
    ]
}

// REQ: OA-BENCH-132
#[test]
fn a_plan_address_that_fails_gives_way_to_a_plausible_hint_and_nothing_else_changes() {
    let legs = legs();
    let truth = Truth {
        query: &query,
        legs: &legs,
    };
    let tix = index_truth(&legs);
    let mut fl = flights();
    // Flight 1: the hint is plausible by position but has too few shared times to decide, so it
    // stays unchecked and does not replace the contradicted plan address.
    fl[1] = (OTHER, NEAR, 3);
    let leg = world(&fl);
    let ix = build_th_index(&leg.th, fl.len());
    let a = associate(&leg, &ix, &truth, &tix, &AssocConfig::default());
    assert_eq!(a.status[0], astatus::HINT_OVER_PLAN);
    assert_eq!((a.addr[0], a.method[0]), (NEAR, method::HINT_OVER_PLAN));
    assert_eq!(a.flag_addr[0], NONE);
    assert_eq!(a.plan_fail[0], 1);
    // Unchecked hint: flagged as without the hint.
    assert_eq!(a.status[1], astatus::ADDRESS_IMPLAUSIBLE);
    assert_eq!((a.addr[1], a.flag_addr[1]), (NONE, OTHER));
    assert_eq!(a.plan_fail[1], 0);
    // Plausible plan address wins over a different hint.
    assert_eq!(a.status[2], astatus::ADDRESS);
    assert_eq!((a.addr[2], a.method[2]), (NEAR, method::ADDRESS));
    // Plan unknown to truth.
    assert_eq!(a.status[3], astatus::HINT_OVER_PLAN);
    assert_eq!(a.addr[3], NEAR);
    assert_eq!(a.plan_fail[3], 3);
    // Hint equal to plan.
    assert_eq!(a.status[4], astatus::ADDRESS);
    assert_eq!(a.addr[4], NEAR);
    // Hint unknown to truth, or absent: as before.
    assert_eq!(a.status[5], astatus::ADDRESS_IMPLAUSIBLE);
    assert_eq!(a.status[6], astatus::HINT_OVER_PLAN);
    assert_eq!(a.plan_fail[6], 2);
    assert_eq!(a.status[7], astatus::ADDRESS_IMPLAUSIBLE);
    assert_eq!(a.status[8], astatus::ADDRESS_NO_TRUTH);
    assert_eq!(a.plan_fail[8], 0);
    // Counts add up.
    assert_eq!(a.counts[usize::from(astatus::HINT_OVER_PLAN)], 3);
    assert_eq!(a.counts[usize::from(astatus::ADDRESS)], 2);
    assert_eq!(a.counts[usize::from(astatus::ADDRESS_IMPLAUSIBLE)], 3);
    assert_eq!(a.counts.iter().sum::<u64>(), fl.len() as u64);
    // The pairs of the three flights are scored in the primary rows as their own method.
    let (p, _) = build_pairs(&leg, &ix, &a, &truth, &PairConfig::default());
    let n = p
        .method
        .iter()
        .filter(|&&m| m == method::HINT_OVER_PLAN)
        .count();
    assert_eq!(n, 18);
}

// REQ: OA-BENCH-132
#[test]
fn the_report_splits_the_hint_over_plan_method_and_lists_each_flight_with_both_addresses() {
    let legs = legs();
    let truth = Truth {
        query: &query,
        legs: &legs,
    };
    let leg = world(&flights());
    let out = run_benchmark(&leg, &truth, &env(), &self_test_config());
    let t1 = find(&out, "T1").unwrap();
    let label = "method=address hint (plan address implausible or unknown to truth)";
    let row = t1.rows.iter().find(|r| r.label == label).unwrap();
    assert!(row.stats.n > 0);
    let asc = find(&out, "ASSOC").unwrap();
    let c = asc
        .scalars
        .iter()
        .find(|(k, _)| k.starts_with("plan address implausible or unknown to truth; track"))
        .unwrap()
        .1;
    assert_eq!(c, 3.0);
    let listed: Vec<&String> = asc
        .cases
        .iter()
        .filter(|l| l.contains("track address hint"))
        .collect();
    assert_eq!(listed.len(), 3);
    assert!(listed.iter().any(
        |l| l.contains("plan address A00001, track address hint B00001")
            && l.contains("ERAM positions far from the plan-address aircraft")
    ));
    assert!(listed
        .iter()
        .any(|l| l.contains("plan address DEAD00") && l.contains("plan address unknown to truth")));
    assert!(listed
        .iter()
        .any(|l| l.contains("plan address C00001") && l.contains("callsigns disagree")));
}

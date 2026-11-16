//! The diagnostic that tests whether a scored velocity is grid-relative: the heading error
//! regressed on the meridian convergence term.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_bench::metrics_th::t3_heading_grid;
use oa_bench::pair::PairTable;
use oa_bench::run::find;
use oa_bench::synth::{run_world, self_test_config, Injected};

fn scalar(m: &oa_bench::stats::MetricResult, k: &str) -> f64 {
    m.scalars.iter().find(|(n, _)| n == k).unwrap().1
}

/// A grid of positions over 40 to 46 N and 92 to 84 W; the heading error of each is `f(c)`.
fn pairs(f: impl Fn(usize, f64) -> f64) -> (PairTable, f64) {
    let mut p = PairTable::default();
    let mut pts = Vec::new();
    for i in 0..40 {
        for j in 0..60 {
            pts.push((
                40.0 + 6.0 * f64::from(i) / 39.0 + 0.001 * f64::from(j),
                -92.0 + 8.0 * f64::from(j) / 59.0 + 0.0007 * f64::from(i),
            ));
        }
    }
    // The longitudes are stored in 1e-7 degrees, so the reference is the mean of the stored ones.
    let q = |x: f64| (x * 1e7).round() as i32;
    let lon_ref = pts
        .iter()
        .map(|&(_, lo)| f64::from(q(lo)) * 1e-7)
        .sum::<f64>()
        / pts.len() as f64;
    for (k, &(la, lo)) in pts.iter().enumerate() {
        let c = (f64::from(q(lo)) * 1e-7 - lon_ref) * (f64::from(q(la)) * 1e-7).to_radians().sin();
        p.t_ms.push(k as i64);
        p.lat_e7.push(q(la));
        p.lon_e7.push(q(lo));
        p.hdg_err_deg.push(f(k, c) as f32);
    }
    (p, lon_ref)
}

// REQ: OA-BENCH-115
#[test]
fn a_heading_error_equal_to_the_convergence_term_has_slope_one() {
    let (p, lon_ref) = pairs(|_, c| c);
    let m = t3_heading_grid(&p);
    assert!((scalar(&m, "slope_deg_per_deg") - 1.0).abs() < 1e-4);
    assert!(scalar(&m, "slope_se") < 1e-3);
    assert_eq!(scalar(&m, "n"), 2400.0);
    assert!((scalar(&m, "lon_ref_deg") - lon_ref).abs() < 1e-6);
}

// REQ: OA-BENCH-115
#[test]
fn no_injected_heading_error_has_slope_zero() {
    let (p, _) = pairs(|_, _| 0.0);
    let m = t3_heading_grid(&p);
    assert!(scalar(&m, "slope_deg_per_deg").abs() < 1e-9);
    // A constant bias is not grid-relative either, and noise gives a slope with a real error.
    let (p, _) = pairs(|k, _| if k % 2 == 0 { 2.5 } else { 1.5 });
    let m = t3_heading_grid(&p);
    assert!(scalar(&m, "slope_deg_per_deg").abs() < 3.0 * scalar(&m, "slope_se") + 1e-3);
    assert!(scalar(&m, "slope_se") > 0.0);
}

// REQ: OA-BENCH-115
#[test]
fn samples_without_a_heading_or_a_spread_give_no_slope() {
    let (mut p, _) = pairs(|_, c| c);
    p.hdg_err_deg.fill(f32::NAN);
    let m = t3_heading_grid(&p);
    assert!(m.scalars.iter().all(|(n, _)| n != "slope_deg_per_deg"));
    assert_eq!(scalar(&m, "n"), 0.0);
    // One longitude and one latitude: no spread in c.
    let mut q = PairTable::default();
    for k in 0..10 {
        q.t_ms.push(k);
        q.lat_e7.push(450_000_000);
        q.lon_e7.push(-880_000_000);
        q.hdg_err_deg.push(1.0);
    }
    let m = t3_heading_grid(&q);
    assert!(m.scalars.iter().all(|(n, _)| n != "slope_deg_per_deg"));
    assert!(!m.notes.is_empty());
}

// REQ: OA-BENCH-115
#[test]
fn the_run_publishes_the_diagnostic_and_the_synthetic_world_has_no_grid_heading() {
    let (_, out) = run_world(&Injected::default(), &self_test_config());
    let m = find(&out, "T3-heading-grid").unwrap();
    let (slope, se) = (scalar(m, "slope_deg_per_deg"), scalar(m, "slope_se"));
    assert!(scalar(m, "n") > 100.0);
    assert!(slope.abs() < 4.0 * se + 0.05, "slope {slope} se {se}");
}

fn table(rows: &[(f64, f64, f32)]) -> PairTable {
    let mut p = PairTable::default();
    for (k, &(la, lo, h)) in rows.iter().enumerate() {
        p.t_ms.push(k as i64);
        p.lat_e7.push((la * 1e7).round() as i32);
        p.lon_e7.push((lo * 1e7).round() as i32);
        p.hdg_err_deg.push(h);
    }
    p
}

fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![vec![]];
    }
    let mut out = Vec::new();
    for p in permutations(n - 1) {
        for at in 0..n {
            let mut q = p.clone();
            q.insert(at, n - 1);
            out.push(q);
        }
    }
    out
}

// REQ: OA-BENCH-123
#[test]
fn the_result_does_not_depend_on_the_order_of_the_pairs() {
    // Three longitudes a third of the way round at 60 N, the error being the convergence term of
    // each from longitude 0: in no order may the answer differ, and a span this wide is refused.
    let e = (120.0 * 60f64.to_radians().sin()) as f32;
    let rows = [(60.0, 0.0, 0.0), (60.0, 120.0, e), (60.0, -120.0, -e)];
    let mut first = None;
    for perm in permutations(3) {
        let m = t3_heading_grid(&table(&perm.iter().map(|&i| rows[i]).collect::<Vec<_>>()));
        assert!(m
            .scalars
            .iter()
            .any(|(n, v)| n == "refused_span" && *v == 1.0));
        assert!(m.scalars.iter().all(|(n, _)| n != "slope_deg_per_deg"));
        assert_eq!(scalar(&m, "n"), 3.0);
        match &first {
            None => first = Some(m.scalars.clone()),
            Some(f) => assert_eq!(f, &m.scalars),
        }
    }
    // A cohort across the antimeridian is one block, and the order of its rows changes no bit.
    let rows = [
        (59.0, 179.9, 0.1),
        (60.0, -179.95, 0.4),
        (61.0, -179.8, 0.2),
        (62.0, 179.85, 0.9),
        (63.0, -179.7, 0.3),
    ];
    let mut first = None;
    for perm in permutations(5) {
        let m = t3_heading_grid(&table(&perm.iter().map(|&i| rows[i]).collect::<Vec<_>>()));
        assert!(m.scalars.iter().any(|(n, _)| n == "slope_deg_per_deg"));
        match &first {
            None => first = Some(m.scalars.clone()),
            Some(f) => assert_eq!(f, &m.scalars),
        }
    }
}

//! Independent C1 oracle for nearest-sample tie and tolerance-boundary accounting.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#![forbid(unsafe_code)]

use oa_bench::common_time::{common_time_compare, CommonTimeConfig};
use oa_bench::pair::PairTable;
use oa_bench::stats::MetricResult;

fn table(rows: &[(i64, f32)]) -> PairTable {
    let mut p = PairTable::default();
    for &(t_ms, cross_m) in rows {
        p.th.push(0);
        p.flight.push(0);
        p.addr.push(0xA11CE);
        p.t_ms.push(t_ms);
        p.cross_m.push(cross_m);
        p.along_m.push(0.0);
    }
    p
}

fn row<'a>(m: &'a MetricResult, label: &str) -> &'a oa_bench::stats::Stats {
    &m.rows.iter().find(|r| r.label == label).unwrap().stats
}

fn scalar(m: &MetricResult, label: &str) -> f64 {
    m.scalars.iter().find(|(k, _)| k == label).unwrap().1
}

// REQ: OA-BENCH-054
#[test]
fn common_time_uses_earlier_sample_on_a_tie_and_includes_the_tolerance_boundary() {
    // Hand oracle: the candidate grid times are 10,000 and 20,000 ms. A's reports at 9,000 and
    // 11,000 ms are equally distant; the earlier report (3 m) must win. B's 7 m
    // sample is exactly 1,000 ms away, the inclusive tolerance, so the pair scores.
    // At t=20,000 ms both systems span the grid time, but A's reports at 18,999 and
    // 21,001 ms are each 1,001 ms away and outside tolerance, so this grid point is
    // counted without both reports.
    let a = table(&[
        (9_000, 3.0),
        (11_000, 9.0),
        (18_999, 100.0),
        (21_001, 200.0),
    ]);
    let b = table(&[(9_000, 7.0), (20_000, 8.0)]);
    let m = common_time_compare(
        &a,
        &b,
        &CommonTimeConfig {
            grid_ms: 10_000,
            tol_ms: 1_000,
        },
    );

    assert_eq!(row(&m, "horizontal, system A").n, 1);
    assert_eq!(row(&m, "horizontal, system A").rms, 3.0);
    assert_eq!(row(&m, "horizontal, system B").n, 1);
    assert_eq!(row(&m, "horizontal, system B").rms, 7.0);
    assert_eq!(scalar(&m, "grid times scored"), 1.0);
    assert_eq!(
        scalar(&m, "grid times without a report of both systems"),
        1.0
    );
}

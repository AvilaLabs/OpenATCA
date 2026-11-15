//! The chunked benchmark's exact histogram: the statistics of whole-millisecond values agree
//! with `stats_of` over the same values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_bench::chunked::MsHist;
use oa_bench::stats::stats_of;

fn lcg(s: &mut u64) -> u64 {
    *s = s
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *s >> 33
}

// REQ: OA-TRK-043
#[test]
fn the_millisecond_histogram_gives_the_statistics_of_the_values() {
    let mut s = 7u64;
    for (n, whole_seconds) in [(1usize, true), (2, false), (1000, false), (50_000, true)] {
        let ms: Vec<i64> = (0..n)
            .map(|_| {
                let v = (lcg(&mut s) % 1_200_000) as i64 - 100_000;
                if whole_seconds {
                    (v / 1000) * 1000
                } else {
                    v
                }
            })
            .collect();
        let vals: Vec<f64> = ms.iter().map(|&m| m as f64 / 1000.0).collect();
        let mut h = MsHist::default();
        for &m in &ms {
            h.add(m);
        }
        let (a, b) = (h.stats(), stats_of(&vals));
        // Order statistics are exact; the sums are exact integers here against a running floating
        // point sum there, so the mean and RMS agree to rounding.
        assert_eq!(
            (a.n, a.median, a.p95, a.p99, a.max),
            (b.n, b.median, b.p95, b.p99, b.max)
        );
        let close = |x: f64, y: f64| (x - y).abs() <= 1e-12 * y.abs().max(1.0);
        assert!(close(a.mean, b.mean) && close(a.rms, b.rms), "{a:?} {b:?}");
        if whole_seconds {
            // Whole seconds add exactly in floating point: bit for bit.
            assert_eq!(
                (a.mean.to_bits(), a.rms.to_bits()),
                (b.mean.to_bits(), b.rms.to_bits())
            );
        }
        // Adding two halves gives the whole.
        let (mut h1, mut h2) = (MsHist::default(), MsHist::default());
        for (i, &m) in ms.iter().enumerate() {
            if i % 2 == 0 {
                h1.add(m);
            } else {
                h2.add(m);
            }
        }
        h1.merge(&h2);
        assert_eq!(h1, h);
    }
    assert_eq!(MsHist::default().stats(), oa_bench::stats::Stats::default());
}

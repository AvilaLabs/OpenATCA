//! Distribution summaries of signed or unsigned differences: count, median, 10th, 90th and 99th
//! percentile (nearest rank) and a 20-bin histogram over a stated range. Counts and
//! percentiles only; no individual value is kept after [`summarize`].

/// Bins of a histogram.
pub const HIST_BINS: usize = 20;

// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub n: u64,
    pub p10: Option<f64>,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p99: Option<f64>,
    /// The histogram range `[lo, hi)`, the counts below and at or above it, and the 20 bins.
    pub lo: f64,
    pub hi: f64,
    pub below: u64,
    pub above: u64,
    pub bins: [u64; HIST_BINS],
}

impl Summary {
    pub fn empty(lo: f64, hi: f64) -> Summary {
        Summary {
            n: 0,
            p10: None,
            p50: None,
            p90: None,
            p99: None,
            lo,
            hi,
            below: 0,
            above: 0,
            bins: [0; HIST_BINS],
        }
    }
}

/// Nearest-rank percentile `pct` of the sorted values.
fn rank(sorted: &[f32], pct: u64) -> Option<f64> {
    let n = sorted.len() as u64;
    if n == 0 {
        return None;
    }
    let k = (n * pct).div_ceil(100).max(1) - 1;
    sorted.get(k as usize).map(|&v| f64::from(v))
}

/// Summarises `values` (NaN is never in them) over the histogram range `[lo, hi)`.
pub fn summarize(values: &mut [f32], lo: f64, hi: f64) -> Summary {
    values.sort_by(|a, b| oa_math::cmp_canon_f32(*a, *b));
    let mut s = Summary::empty(lo, hi);
    s.n = values.len() as u64;
    s.p10 = rank(values, 10);
    s.p50 = rank(values, 50);
    s.p90 = rank(values, 90);
    s.p99 = rank(values, 99);
    let w = (hi - lo) / HIST_BINS as f64;
    for &v in values.iter() {
        let v = f64::from(v);
        if v < lo {
            s.below += 1;
        } else if v >= hi {
            s.above += 1;
        } else if let Some(b) = s.bins.get_mut(((v - lo) / w) as usize) {
            *b += 1;
        }
    }
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // REQ: OA-CEN-010
    #[test]
    fn percentiles_and_histogram() {
        let mut v: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let s = summarize(&mut v, 0.0, 100.0);
        assert_eq!(s.n, 100);
        assert_eq!(s.p10, Some(10.0));
        assert_eq!(s.p50, Some(50.0));
        assert_eq!(s.p90, Some(90.0));
        assert_eq!(s.p99, Some(99.0));
        // 1..=99 fall in bins of width 5; 100 is at the edge and counts as above.
        assert_eq!(s.above, 1);
        assert_eq!(s.below, 0);
        assert_eq!(s.bins[0], 4);
        assert_eq!(s.bins[19], 5);
        assert_eq!(s.bins.iter().sum::<u64>(), 99);
    }

    // REQ: OA-CEN-010
    #[test]
    fn signed_values_and_empty() {
        let mut v = vec![-30.0f32, -10.0, 0.0, 10.0, 500.0];
        let s = summarize(&mut v, -100.0, 100.0);
        assert_eq!(s.p50, Some(0.0));
        assert_eq!(s.above, 1);
        assert_eq!(s.bins[7], 1);
        let e = summarize(&mut [], 0.0, 1.0);
        assert_eq!((e.n, e.p50), (0, None));
    }
}

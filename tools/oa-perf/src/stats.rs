//! Percentiles, a fixed-bin histogram and a key=value writer for the harness. Pure functions of
//! their arguments.

use std::fmt::Display;

/// The `q` quantile (0 to 1) of an ascending slice by the nearest-rank rule: the smallest value
/// with at least a fraction `q` of the values at or below it. NaN for an empty slice or a `q`
/// outside 0 to 1.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 0 || !(0.0..=1.0).contains(&q) {
        return f64::NAN;
    }
    // The small offset keeps q = 0.999 of 1000 values at rank 999, not 1000.
    let rank = (q * n as f64 - 1.0e-9).ceil().max(1.0) as usize;
    sorted.get(rank.min(n) - 1).copied().unwrap_or(f64::NAN)
}

/// The summary of a set of values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dist {
    pub n: usize,
    pub min: f64,
    pub mean: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
}

impl Dist {
    pub const EMPTY: Dist = Dist {
        n: 0,
        min: f64::NAN,
        mean: f64::NAN,
        p50: f64::NAN,
        p90: f64::NAN,
        p99: f64::NAN,
        p999: f64::NAN,
        max: f64::NAN,
    };
}

/// Summary of `v` (any order; NaN values are left out).
pub fn dist_of(v: &[f64]) -> Dist {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| !x.is_nan()).collect();
    if s.is_empty() {
        return Dist::EMPTY;
    }
    s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let n = s.len();
    Dist {
        n,
        min: s.first().copied().unwrap_or(f64::NAN),
        mean: s.iter().sum::<f64>() / n as f64,
        p50: percentile(&s, 0.5),
        p90: percentile(&s, 0.9),
        p99: percentile(&s, 0.99),
        p999: percentile(&s, 0.999),
        max: s.last().copied().unwrap_or(f64::NAN),
    }
}

/// Summary of `(value, weight)` pairs: each value counts `weight` times (a call that processed
/// `n` inputs in one go gives `n` inputs the same per-input time).
pub fn dist_weighted(items: &[(f64, u64)]) -> Dist {
    let mut s: Vec<(f64, u64)> = items
        .iter()
        .copied()
        .filter(|(v, w)| !v.is_nan() && *w > 0)
        .collect();
    if s.is_empty() {
        return Dist::EMPTY;
    }
    s.sort_by(|a, b| oa_math::cmp_canon_f64(a.0, b.0));
    let total: u64 = s.iter().map(|x| x.1).sum();
    let at = |q: f64| -> f64 {
        let want = ((q * total as f64 - 1.0e-9).ceil().max(1.0)) as u64;
        let mut acc = 0u64;
        for &(v, w) in &s {
            acc += w;
            if acc >= want {
                return v;
            }
        }
        s.last().map_or(f64::NAN, |x| x.0)
    };
    Dist {
        n: total as usize,
        min: s.first().map_or(f64::NAN, |x| x.0),
        mean: s.iter().map(|&(v, w)| v * w as f64).sum::<f64>() / total as f64,
        p50: at(0.5),
        p90: at(0.9),
        p99: at(0.99),
        p999: at(0.999),
        max: s.last().map_or(f64::NAN, |x| x.0),
    }
}

/// A histogram of non-negative values in equal bins; the memory is fixed however many values go
/// in. A percentile is the lower edge of the bin holding that rank; the maximum, mean and count
/// are exact.
#[derive(Debug, Clone)]
pub struct Hist {
    width: f64,
    bins: Vec<u64>,
    over: u64,
    n: u64,
    sum: f64,
    max: f64,
}

impl Hist {
    pub fn new(width: f64, nbins: usize) -> Hist {
        Hist {
            width,
            bins: vec![0; nbins],
            over: 0,
            n: 0,
            sum: 0.0,
            max: f64::NAN,
        }
    }

    pub fn add(&mut self, v: f64) {
        if v.is_nan() {
            return;
        }
        self.n += 1;
        self.sum += v;
        self.max = if self.max.is_nan() {
            v
        } else {
            self.max.max(v)
        };
        let ix = (v.max(0.0) / self.width) as usize;
        match self.bins.get_mut(ix) {
            Some(b) => *b += 1,
            None => self.over += 1,
        }
    }

    pub fn count(&self) -> u64 {
        self.n
    }

    /// Lower edge of the bin holding the `q` rank; the exact maximum if it is past the last bin.
    pub fn percentile(&self, q: f64) -> f64 {
        if self.n == 0 || !(0.0..=1.0).contains(&q) {
            return f64::NAN;
        }
        let want = ((q * self.n as f64 - 1.0e-9).ceil().max(1.0)) as u64;
        let mut acc = 0u64;
        for (i, &b) in self.bins.iter().enumerate() {
            acc += b;
            if acc >= want {
                return i as f64 * self.width;
            }
        }
        self.max
    }

    pub fn summary(&self) -> Dist {
        if self.n == 0 {
            return Dist::EMPTY;
        }
        Dist {
            n: self.n as usize,
            min: f64::NAN,
            mean: self.sum / self.n as f64,
            p50: self.percentile(0.5),
            p90: self.percentile(0.9),
            p99: self.percentile(0.99),
            p999: self.percentile(0.999),
            max: self.max,
        }
    }
}

/// Ordered key=value lines, the machine-readable result.
#[derive(Debug, Default, Clone)]
pub struct Kv {
    pub rows: Vec<(String, String)>,
}

impl Kv {
    pub fn put(&mut self, k: &str, v: impl Display) {
        self.rows.push((k.to_string(), v.to_string()));
    }

    /// The keys of a [`Dist`] under `prefix`, each with `unit` in its name.
    pub fn put_dist(&mut self, prefix: &str, unit: &str, d: &Dist) {
        self.put(&format!("{prefix}.n"), d.n);
        for (name, v) in [
            ("min", d.min),
            ("mean", d.mean),
            ("p50", d.p50),
            ("p90", d.p90),
            ("p99", d.p99),
            ("p999", d.p999),
            ("max", d.max),
        ] {
            self.put(&format!("{prefix}.{name}_{unit}"), fmt_f(v));
        }
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.rows {
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push('\n');
        }
        s
    }

    pub fn parse(text: &str) -> Kv {
        let mut kv = Kv::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                kv.rows.push((k.to_string(), v.to_string()));
            }
        }
        kv
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
    }

    pub fn get_f64(&self, k: &str) -> Option<f64> {
        let value: f64 = self.get(k)?.parse().ok()?;
        (value.is_finite() || value.is_nan()).then_some(value)
    }
}

/// A float for a table: 3 significant decimals, `nan` for none.
pub fn fmt_f(v: f64) -> String {
    if v.is_nan() {
        "nan".to_string()
    } else if v.abs() >= 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.3}")
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // REQ: OA-PRF-001
    #[test]
    fn percentiles_are_nearest_rank() {
        let v: Vec<f64> = (1..=1000).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.5), 500.0);
        assert_eq!(percentile(&v, 0.99), 990.0);
        assert_eq!(percentile(&v, 0.999), 999.0);
        assert_eq!(percentile(&v, 1.0), 1000.0);
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&[7.0], 0.999), 7.0);
        let small = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&small, 0.5), 2.0);
        assert_eq!(percentile(&small, 0.75), 3.0);
        assert_eq!(percentile(&small, 0.76), 4.0);
        assert!(percentile(&[], 0.5).is_nan());
        assert!(percentile(&small, 1.5).is_nan());
    }

    // REQ: OA-PRF-001
    #[test]
    fn a_summary_sorts_and_skips_nan() {
        let d = dist_of(&[5.0, f64::NAN, 1.0, 3.0, 2.0, 4.0]);
        assert_eq!((d.n, d.min, d.max, d.p50, d.mean), (5, 1.0, 5.0, 3.0, 3.0));
        assert_eq!(dist_of(&[]).n, 0);
    }

    // REQ: OA-PRF-001
    #[test]
    fn weights_count_a_value_that_many_times() {
        // 99 inputs at 1 us and one at 100 us: the median is 1, the maximum 100.
        let d = dist_weighted(&[(100.0, 1), (1.0, 99)]);
        assert_eq!((d.n, d.p50, d.p99, d.max), (100, 1.0, 1.0, 100.0));
        assert_eq!(dist_weighted(&[(1.0, 99), (100.0, 2)]).p99, 100.0);
        let same = dist_weighted(&[(3.0, 1), (1.0, 1), (2.0, 1)]);
        assert_eq!(same.p50, dist_of(&[3.0, 1.0, 2.0]).p50);
    }

    // REQ: OA-PRF-001
    #[test]
    fn a_histogram_gives_bin_edges_and_exact_extremes() {
        let mut h = Hist::new(1.0, 10);
        for v in [0.0, 0.5, 1.2, 2.9, 3.0, 50.0] {
            h.add(v);
        }
        let s = h.summary();
        assert_eq!(s.n, 6);
        assert_eq!(s.max, 50.0);
        assert_eq!(h.percentile(0.5), 1.0);
        assert_eq!(h.percentile(1.0), 50.0);
        assert!((s.mean - 57.6 / 6.0).abs() < 1e-12);
    }

    // REQ: OA-PRF-001
    #[test]
    fn key_value_text_round_trips() {
        let mut kv = Kv::default();
        kv.put("a.b", 3);
        kv.put("c", "x y");
        let back = Kv::parse(&kv.render());
        assert_eq!(back.get("a.b"), Some("3"));
        assert_eq!(back.get_f64("a.b"), Some(3.0));
        assert_eq!(back.get("c"), Some("x y"));
        assert_eq!(back.get("none"), None);
    }

    #[test]
    fn float_reader_preserves_nan_sentinel_but_rejects_infinity() {
        let kv = Kv::parse("missing=nan\noverflow=inf\nnegative_overflow=-inf\nfinite=1.25\n");
        assert!(kv.get_f64("missing").unwrap().is_nan());
        assert_eq!(kv.get_f64("overflow"), None);
        assert_eq!(kv.get_f64("negative_overflow"), None);
        assert_eq!(kv.get_f64("finite"), Some(1.25));
    }
}

//! A fixed-resolution histogram of update intervals: 10 ms bins up to 120 s, from which the
//! quantiles and the 0.5 s display bins (to 15 s) are read. Counts only.

/// Milliseconds per tick (the stored resolution of an interval).
pub const TICK_MS: i64 = 10;
/// Largest interval kept, in ticks (120 s); a longer gap is a reset, counted elsewhere.
pub const MAX_TICKS: usize = 12_000;
/// Number of 0.5 s display bins (0 to 15 s).
pub const COARSE_BINS: usize = 30;
/// Ticks per display bin (0.5 s).
const COARSE_TICKS: usize = 50;

// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Hist {
    bins: Vec<u64>,
    pub n: u64,
}

impl Default for Hist {
    fn default() -> Hist {
        Hist::new()
    }
}

impl Hist {
    pub fn new() -> Hist {
        Hist {
            bins: vec![0; MAX_TICKS + 1],
            n: 0,
        }
    }

    /// Adds one interval of `ticks` (values above [`MAX_TICKS`] are clamped to the last bin; the
    /// caller keeps those out).
    pub fn add(&mut self, ticks: usize) {
        let i = ticks.min(MAX_TICKS);
        if let Some(b) = self.bins.get_mut(i) {
            *b += 1;
            self.n += 1;
        }
    }

    /// The `num`/`den` quantile in seconds (bin centre), or `None` when empty: the smallest bin
    /// whose cumulative count reaches `ceil(n * num / den)`.
    pub fn quantile(&self, num: u64, den: u64) -> Option<f64> {
        if self.n == 0 || den == 0 {
            return None;
        }
        let target = (self.n * num).div_ceil(den).max(1);
        let mut cum = 0u64;
        for (i, &c) in self.bins.iter().enumerate() {
            cum += c;
            if cum >= target {
                return Some((i as f64 + 0.5) * TICK_MS as f64 / 1000.0);
            }
        }
        None
    }

    /// Counts in the 30 bins of 0.5 s from 0 to 15 s, and the count at 15 s and over.
    pub fn coarse(&self) -> ([u64; COARSE_BINS], u64) {
        let mut out = [0u64; COARSE_BINS];
        let mut over = 0u64;
        for (i, &c) in self.bins.iter().enumerate() {
            match out.get_mut(i / COARSE_TICKS) {
                Some(slot) => *slot += c,
                None => over += c,
            }
        }
        (out, over)
    }

    /// Count of intervals of exactly zero (below 10 ms).
    pub fn zeros(&self) -> u64 {
        self.bins.first().copied().unwrap_or(0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // REQ: OA-CEN-001
    #[test]
    fn quantiles_and_coarse_bins() {
        let mut h = Hist::new();
        // 10 intervals: 4.60 s x4, 4.70 s x4, 9.00 s, 14.99 s.
        for t in [460, 460, 460, 460, 470, 470, 470, 470, 900, 1499] {
            h.add(t);
        }
        assert_eq!(h.n, 10);
        assert!((h.quantile(1, 2).unwrap() - 4.705).abs() < 1e-9);
        assert!((h.quantile(1, 10).unwrap() - 4.605).abs() < 1e-9);
        assert!((h.quantile(9, 10).unwrap() - 9.005).abs() < 1e-9);
        let (c, over) = h.coarse();
        assert_eq!(c[9], 8);
        assert_eq!(c[18], 1);
        assert_eq!(c[29], 1);
        assert_eq!(over, 0);
        h.add(1500);
        h.add(0);
        let (c, over) = h.coarse();
        assert_eq!(over, 1);
        assert_eq!(c[0], 1);
        assert_eq!(h.zeros(), 1);
    }

    // REQ: OA-CEN-001
    #[test]
    fn empty_has_no_quantile() {
        assert!(Hist::new().quantile(1, 2).is_none());
    }
}

//! Core (pure, B1): the interval distribution of the rows of each track, from a histogram. The
//! shell keeps the counts (value, how many); this turns them into the numbers the README states.

/// Nearest-rank percentile `num/den` of a histogram sorted by value: the smallest value such that
/// at least `num/den` of all counts are at or below it. `None` for an empty histogram or `den` 0.
pub fn percentile(hist: &[(u32, u64)], num: u64, den: u64) -> Option<u32> {
    if den == 0 {
        return None;
    }
    let total: u64 = hist.iter().map(|&(_, c)| c).sum();
    if total == 0 {
        return None;
    }
    // Rank, 1-based, rounded up: ceil(total * num / den), at least 1. 128-bit: cannot overflow.
    let rank = (u128::from(total) * u128::from(num))
        .div_ceil(u128::from(den))
        .max(1);
    let mut seen = 0u128;
    for &(v, c) in hist {
        seen += u128::from(c);
        if seen >= rank {
            return Some(v);
        }
    }
    hist.last().map(|&(v, _)| v)
}

/// The interval between consecutive rows of one track, over all tracks of a file, in 1/128 s.
/// A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntervalSummary {
    /// Pairs of consecutive rows of one track.
    pub pairs: u64,
    /// Pairs with the same time (conflicting rows at one time).
    pub zero: u64,
    pub median: u32,
    pub p90: u32,
    pub max: u32,
}

/// Summary of a histogram sorted by value.
pub fn summarize(hist: &[(u32, u64)]) -> IntervalSummary {
    IntervalSummary {
        pairs: hist.iter().map(|&(_, c)| c).sum(),
        zero: hist.iter().find(|&&(v, _)| v == 0).map_or(0, |&(_, c)| c),
        median: percentile(hist, 1, 2).unwrap_or(0),
        p90: percentile(hist, 9, 10).unwrap_or(0),
        max: hist.last().map_or(0, |&(v, _)| v),
    }
}

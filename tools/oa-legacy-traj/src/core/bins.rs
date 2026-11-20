//! Look-ahead bins and the statistics of an error sample. Core (pure).

use crate::core::types::{Config, Exclusion, Sample};

/// Bin edges, minutes of look-ahead: bin `k` is from `EDGES_MIN[k]` up to (excluding)
/// `EDGES_MIN[k + 1]`.
pub const EDGES_MIN: [i64; 7] = [0, 5, 10, 20, 40, 60, 120];
/// Number of bins.
pub const BIN_COUNT: usize = 6;
/// Display names of the bins.
pub const BIN_NAMES: [&str; BIN_COUNT] = [
    "0 to 5 min",
    "5 to 10 min",
    "10 to 20 min",
    "20 to 40 min",
    "40 to 60 min",
    "60 to 120 min",
];

const MS_PER_MIN: i64 = 60_000;

/// The bin of a look-ahead in ms; `None` for a negative one or one at or past the last edge.
pub fn bin_of(look_ahead_ms: i64) -> Option<usize> {
    if look_ahead_ms < 0 {
        return None;
    }
    (0..BIN_COUNT).find(|&k| {
        let hi = EDGES_MIN.get(k + 1).copied().unwrap_or(0) * MS_PER_MIN;
        let lo = EDGES_MIN.get(k).copied().unwrap_or(0) * MS_PER_MIN;
        look_ahead_ms >= lo && look_ahead_ms < hi
    })
}

/// Scores one estimate against the actual time: the error is the estimate minus the actual time,
/// in seconds (positive when the estimate is later than what happened), and the look-ahead is the
/// actual time minus the issue time.
pub fn score(
    est_ms: i64,
    actual_ms: i64,
    issue_ms: i64,
    cfg: &Config,
) -> Result<Sample, Exclusion> {
    let look_ahead_ms = actual_ms - issue_ms;
    if look_ahead_ms < 0 {
        return Err(Exclusion::PassedBeforeIssue);
    }
    if look_ahead_ms >= cfg.max_look_ahead_ms {
        return Err(Exclusion::LookAheadBeyondRange);
    }
    let Some(bin) = bin_of(look_ahead_ms) else {
        return Err(Exclusion::LookAheadBeyondRange);
    };
    Ok(Sample {
        error_s: (est_ms - actual_ms) as f64 / 1000.0,
        look_ahead_s: look_ahead_ms as f64 / 1000.0,
        bin,
    })
}

/// The statistics of a list of values (here, errors in seconds).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub n: u64,
    pub median: f64,
    pub mean: f64,
    pub p5: f64,
    pub p95: f64,
    pub rms: f64,
}

/// The value at fraction `q` of the sorted values, by linear interpolation between the two
/// nearest ranks (rank `q * (n - 1)`). `sorted` is not empty.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    let rank = q * (n - 1) as f64;
    let lo = oa_ieee::floor(rank) as usize;
    let frac = rank - lo as f64;
    let a = sorted.get(lo).copied().unwrap_or(f64::NAN);
    let b = sorted.get(lo + 1).copied().unwrap_or(a);
    a + (b - a) * frac
}

/// The statistics of `values`; `None` for no value. Values are put in order first, so the sums
/// are taken in one fixed order whatever order they arrived in.
pub fn stats(values: &[f64]) -> Option<Stats> {
    if values.is_empty() {
        return None;
    }
    let mut v: Vec<f64> = values.to_vec();
    v.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let n = v.len() as f64;
    let mut sum = 0.0;
    let mut sq = 0.0;
    for x in &v {
        sum += *x;
        sq += *x * *x;
    }
    Some(Stats {
        n: v.len() as u64,
        median: quantile(&v, 0.5),
        mean: sum / n,
        p5: quantile(&v, 0.05),
        p95: quantile(&v, 0.95),
        rms: oa_ieee::sqrt(sq / n),
    })
}

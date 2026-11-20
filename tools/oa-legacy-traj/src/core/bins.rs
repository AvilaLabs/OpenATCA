//! Look-ahead bins and the statistics of an error sample. Core (pure).

use crate::core::types::{Config, Exclusion, Sample, NO_TIME};

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

/// The bin of a stated lead, when both times are known and the lead is in range.
pub fn lead_bin(est_ms: i64, issue_ms: i64) -> Option<usize> {
    if est_ms == NO_TIME || issue_ms == NO_TIME {
        return None;
    }
    bin_of(est_ms.saturating_sub(issue_ms))
}

/// Whether an estimate's stated lead is in range: not before the issue and short of the last bin.
pub fn lead_in_range(est_ms: i64, issue_ms: i64, cfg: &Config) -> Result<(), Exclusion> {
    let lead_ms = est_ms - issue_ms;
    if lead_ms < 0 {
        return Err(Exclusion::EstimateBeforeIssue);
    }
    if lead_ms >= cfg.max_look_ahead_ms || bin_of(lead_ms).is_none() {
        return Err(Exclusion::LookAheadBeyondRange);
    }
    Ok(())
}

/// Scores one estimate against the actual time: the error is the estimate minus the actual time,
/// in seconds (positive when the estimate is later than what happened). The bin is that of the
/// stated lead, the estimate minus the issue time. An event before the issue time is kept (it is
/// an error like any other) and flagged.
pub fn score(
    est_ms: i64,
    actual_ms: i64,
    issue_ms: i64,
    cfg: &Config,
) -> Result<Sample, Exclusion> {
    let lead_ms = est_ms - issue_ms;
    if lead_ms < 0 {
        return Err(Exclusion::EstimateBeforeIssue);
    }
    if lead_ms >= cfg.max_look_ahead_ms {
        return Err(Exclusion::LookAheadBeyondRange);
    }
    let Some(bin) = bin_of(lead_ms) else {
        return Err(Exclusion::LookAheadBeyondRange);
    };
    Ok(Sample {
        error_s: (est_ms - actual_ms) as f64 / 1000.0,
        lead_s: lead_ms as f64 / 1000.0,
        bin,
        act_bin: bin_of(actual_ms - issue_ms),
        before_issue: actual_ms < issue_ms,
        dist_m: 0.0,
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
    /// Nearest-rank percentiles of the absolute values, as oa-bench defines them.
    pub abs_median: f64,
    pub abs_p95: f64,
    pub abs_p99: f64,
}

/// The nearest-rank percentile `p` of an ascending slice, as `oa-bench` and `p002` define it:
/// the value at 1-based rank `ceil(p * n / 100)`, no interpolation. `sorted` is not empty. (They
/// apply it to absolute values; here it is applied to the signed errors.)
pub fn nearest_rank(sorted: &[f64], p: u64) -> f64 {
    let n = sorted.len() as u64;
    let k = (p * n).div_ceil(100).clamp(1, n);
    sorted.get((k - 1) as usize).copied().unwrap_or(f64::NAN)
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
    let mut abs: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    abs.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    for x in &v {
        sum += *x;
        sq += *x * *x;
    }
    Some(Stats {
        n: v.len() as u64,
        median: nearest_rank(&v, 50),
        mean: sum / n,
        p5: nearest_rank(&v, 5),
        p95: nearest_rank(&v, 95),
        rms: oa_ieee::sqrt(sq / n),
        abs_median: nearest_rank(&abs, 50),
        abs_p95: nearest_rank(&abs, 95),
        abs_p99: nearest_rank(&abs, 99),
    })
}

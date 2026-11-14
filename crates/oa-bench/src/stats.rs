//! Error statistics and the shape of a metric result. Core (pure).
//!
//! **Definitions** (stated in every report): `mean` is the signed mean. `rms` is the root mean
//! square. `median`, `p95`, `p99` and `max` are taken over the absolute values, with the
//! nearest-rank method: sort the `n` absolute values ascending and take the value at 1-based
//! rank `ceil(p * n / 100)`, clamped to `1..=n`; the median is `p = 50`. The method has no
//! interpolation, so the same values always give the same result, whatever their order.

/// Phase codes of a paired sample, from the truth vertical rate.
pub mod phase {
    pub const CLIMB: u8 = 0;
    pub const CRUISE: u8 = 1;
    pub const DESCENT: u8 = 2;
    pub const UNKNOWN: u8 = 3;
    pub const COUNT: usize = 4;
    pub const NAMES: [&str; COUNT] = ["climb", "cruise", "descent", "unknown"];
    /// Vertical rate beyond which a sample is climbing or descending, feet per minute.
    pub const RATE_FPM: i32 = 500;
}

/// Altitude band codes, from the ERAM reported altitude.
pub mod band {
    pub const COUNT: usize = 6;
    pub const NAMES: [&str; COUNT] = [
        "below 10000 ft",
        "10000 to 17999 ft",
        "18000 to 28999 ft",
        "29000 to 40999 ft",
        "41000 ft and above",
        "no altitude",
    ];
    /// Band of an altitude in feet; `None` is the last band.
    pub fn of(alt_ft: Option<i32>) -> u8 {
        match alt_ft {
            None => 5,
            Some(a) if a < 10_000 => 0,
            Some(a) if a < 18_000 => 1,
            Some(a) if a < 29_000 => 2,
            Some(a) if a < 41_000 => 3,
            Some(_) => 4,
        }
    }
}

/// Coast split names, indexed by the coast flag.
pub const COAST_NAMES: [&str; 2] = ["not coasting", "coasting"];
/// Association method codes stored per pair.
pub mod method {
    pub const ADDRESS: u8 = 1;
    pub const CALLSIGN: u8 = 2;
    /// The address came from the `ADSB_02M` name-value pair, not `aircraftAddress`.
    pub const ADDRESS_HINT: u8 = 3;
    pub const NAMES: [&str; 4] = ["none", "address", "callsign", "address hint"];
}

/// The statistics of one set of values.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub n: u64,
    pub mean: f64,
    pub rms: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// The nearest-rank percentile of an ascending slice of absolute values.
pub fn nearest_rank(sorted: &[f64], p: u64) -> f64 {
    let n = sorted.len() as u64;
    if n == 0 {
        return 0.0;
    }
    let k = (p * n).div_ceil(100).clamp(1, n);
    sorted.get((k - 1) as usize).copied().unwrap_or(0.0)
}

/// The statistics of `values`; non-finite values are skipped (the caller counts them).
pub fn stats_of(values: &[f64]) -> Stats {
    let mut abs: Vec<f64> = Vec::with_capacity(values.len());
    let mut sum = 0.0;
    let mut sumsq = 0.0;
    for &v in values {
        if v.is_finite() {
            sum += v;
            sumsq += v * v;
            abs.push(v.abs());
        }
    }
    if abs.is_empty() {
        return Stats::default();
    }
    abs.sort_by(f64::total_cmp);
    let n = abs.len() as f64;
    Stats {
        n: abs.len() as u64,
        mean: sum / n,
        rms: (sumsq / n).sqrt(),
        median: nearest_rank(&abs, 50),
        p95: nearest_rank(&abs, 95),
        p99: nearest_rank(&abs, 99),
        max: abs.last().copied().unwrap_or(0.0),
    }
}

/// One labelled row of a metric result.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub stats: Stats,
}

/// One metric's result: statistics rows plus named scalars, cases and notes.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct MetricResult {
    pub id: &'static str,
    pub name: &'static str,
    pub unit: &'static str,
    pub rows: Vec<Row>,
    pub scalars: Vec<(String, f64)>,
    /// Set when the numbers are withheld, with the reason (the rows are then empty).
    pub withheld: Option<String>,
    /// Listed cases (association errors, off-route flights), bounded by [`MAX_CASES`].
    pub cases: Vec<String>,
    pub notes: Vec<String>,
}

/// Most cases listed per metric; the rest are counted in a scalar.
pub const MAX_CASES: usize = 200;

/// An empty result for a metric.
pub fn new_result(id: &'static str, name: &'static str, unit: &'static str) -> MetricResult {
    MetricResult {
        id,
        name,
        unit,
        rows: Vec::new(),
        scalars: Vec::new(),
        withheld: None,
        cases: Vec::new(),
        notes: Vec::new(),
    }
}

/// A result that is withheld, with the reason.
pub fn withheld_result(
    id: &'static str,
    name: &'static str,
    unit: &'static str,
    why: &str,
) -> MetricResult {
    let mut r = new_result(id, name, unit);
    r.withheld = Some(why.to_string());
    r
}

/// The columns a value set is split by: one code per value for each split.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct Splits<'a> {
    pub phase: &'a [u8],
    pub band: &'a [u8],
    pub coast: &'a [u8],
    pub method: &'a [u8],
}

/// One split: a key, a code per value and the names the codes index.
pub type Group<'a> = (&'a str, &'a [u8], &'a [&'a str]);

/// Statistics of `values` overall and for each group's codes. Empty groups are left out; the
/// `all` row is always present.
pub fn group_rows(values: &[f64], groups: &[Group<'_>]) -> Vec<Row> {
    let mut rows = vec![Row {
        label: "all".to_string(),
        stats: stats_of(values),
    }];
    for (key, codes, names) in groups {
        for (code, name) in names.iter().enumerate() {
            let sel: Vec<f64> = values
                .iter()
                .zip(codes.iter())
                .filter(|(_, &c)| usize::from(c) == code)
                .map(|(&v, _)| v)
                .collect();
            if !sel.is_empty() {
                rows.push(Row {
                    label: format!("{key}={name}"),
                    stats: stats_of(&sel),
                });
            }
        }
    }
    rows
}

/// Statistics of `values` overall and split by phase, altitude band, coast flag and association
/// method.
pub fn split_rows(values: &[f64], sp: &Splits<'_>) -> Vec<Row> {
    group_rows(
        values,
        &[
            ("phase", sp.phase, &phase::NAMES),
            ("altitude", sp.band, &band::NAMES),
            ("coast", sp.coast, &COAST_NAMES),
            ("method", sp.method, &method::NAMES),
        ],
    )
}

/// A result with the split rows of `values`.
pub fn split_result(
    id: &'static str,
    name: &'static str,
    unit: &'static str,
    values: &[f64],
    sp: &Splits<'_>,
) -> MetricResult {
    let mut r = new_result(id, name, unit);
    r.rows = split_rows(values, sp);
    r
}

/// Exclusions: (scope, reason) to a count, so nothing is dropped silently. Ordered by key.
pub type Exclusions = std::collections::BTreeMap<(&'static str, &'static str), u64>;

/// Counts one exclusion.
pub fn exclude(ex: &mut Exclusions, scope: &'static str, reason: &'static str) {
    let c = ex.entry((scope, reason)).or_insert(0);
    *c = c.saturating_add(1);
}

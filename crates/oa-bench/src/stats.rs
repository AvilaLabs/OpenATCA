//! Error statistics and the shape of a metric result. Core (pure).
//!
//! **Definitions** (stated in every report): `mean` is the signed mean. `rms` is the root mean
//! square. `median`, `p95`, `p99` and `max` are taken over the absolute values, with the
//! nearest-rank method: sort the `n` absolute values ascending and take the value at 1-based
//! rank `ceil(p * n / 100)`, clamped to `1..=n`; the median is `p = 50`. The method has no
//! interpolation, so the same values always give the same result, whatever their order.
//!
//! **Nothing is not zero.** The statistics of an empty set are `n = 0` and NaN in every other
//! field, never 0.0, which would read as a perfect result. The report prints such a row as
//! "no data" and the CSV as `NA`. The same holds for a value that cannot be computed: the sums
//! are scaled by the largest magnitude, so finite input never overflows to a falsely perfect
//! (or infinite) RMS, and a result that is still not finite is NaN, never 0.

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
    /// Associated by callsign but the ERAM positions fail the 3 NM / 1,000 ft check: kept (the
    /// selection must not depend on the error being measured) and scored under its own method.
    pub const CALLSIGN_GATE_FAILED: u8 = 4;
    pub const NAMES: [&str; 5] = [
        "none",
        "address",
        "callsign",
        "address hint",
        "callsign (failed 3 NM / 1000 ft check)",
    ];
}

/// The statistics of one set of values.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub n: u64,
    pub mean: f64,
    pub rms: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Stats {
    /// The statistics of no values: `n = 0`, everything else NaN.
    pub const EMPTY: Stats = Stats {
        n: 0,
        mean: f64::NAN,
        rms: f64::NAN,
        median: f64::NAN,
        p95: f64::NAN,
        p99: f64::NAN,
        max: f64::NAN,
    };
}

impl Default for Stats {
    fn default() -> Stats {
        Stats::EMPTY
    }
}

/// Bitwise equality, so two empty results (NaN fields) are equal.
impl PartialEq for Stats {
    fn eq(&self, o: &Stats) -> bool {
        self.n == o.n
            && [
                (self.mean, o.mean),
                (self.rms, o.rms),
                (self.median, o.median),
                (self.p95, o.p95),
                (self.p99, o.p99),
                (self.max, o.max),
            ]
            .iter()
            .all(|(a, b)| a.to_bits() == b.to_bits())
    }
}

/// A floating point sum that is exactly rounded, so it does not depend on the order the values
/// are added in (Shewchuk's expansion sum, the method of Python's `math.fsum`): the same values
/// in any order give the same bits. That is what lets a report built from address ranges equal
/// the one built in one piece. Cold record (A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExactSum {
    /// Non-overlapping partial sums, smallest first.
    partials: Vec<f64>,
}

impl ExactSum {
    /// Adds one finite value; a non-finite value is ignored (callers count those).
    pub fn add(&mut self, mut x: f64) {
        if !x.is_finite() {
            return;
        }
        let mut k = 0usize;
        for i in 0..self.partials.len() {
            let Some(mut y) = self.partials.get(i).copied() else {
                break;
            };
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                if let Some(p) = self.partials.get_mut(k) {
                    *p = lo;
                }
                k += 1;
            }
            x = hi;
        }
        self.partials.truncate(k);
        self.partials.push(x);
    }

    /// The sum, correctly rounded.
    pub fn value(&self) -> f64 {
        let p = &self.partials;
        let mut n = p.len();
        let Some(&top) = p.last() else { return 0.0 };
        let mut hi = top;
        let mut lo = 0.0;
        n -= 1;
        while n > 0 {
            let x = hi;
            n -= 1;
            let y = p.get(n).copied().unwrap_or(0.0);
            hi = x + y;
            lo = y - (hi - x);
            if lo != 0.0 {
                break;
            }
        }
        // Round half to even across the discarded partials.
        let below = n
            .checked_sub(1)
            .and_then(|i| p.get(i))
            .copied()
            .unwrap_or(0.0);
        if n > 0 && ((lo < 0.0 && below < 0.0) || (lo > 0.0 && below > 0.0)) {
            let y = lo * 2.0;
            let x = hi + y;
            if y == x - hi {
                hi = x;
            }
        }
        hi
    }
}

/// The nearest-rank percentile of an ascending slice of absolute values.
pub fn nearest_rank(sorted: &[f64], p: u64) -> f64 {
    let n = sorted.len() as u64;
    if n == 0 {
        return f64::NAN;
    }
    let k = (p * n).div_ceil(100).clamp(1, n);
    sorted.get((k - 1) as usize).copied().unwrap_or(f64::NAN)
}

/// The largest power of two not above `max`, a finite positive number. Dividing by it is exact
/// (except for digits below the subnormal range) and brings the largest magnitude into `[1, 2)`.
fn scale_of(max: f64) -> f64 {
    let bits = max.to_bits();
    if bits & 0x7ff0_0000_0000_0000 != 0 {
        f64::from_bits(bits & 0x7ff0_0000_0000_0000)
    } else {
        // Subnormal: the mantissa is the value in units of the smallest subnormal.
        f64::from_bits(1u64 << (63 - bits.leading_zeros()))
    }
}

/// The statistics of `values`; non-finite values are skipped (the caller counts them). The mean
/// and the root mean square are summed on the values divided by a power of two near the largest
/// magnitude, so no sum or square leaves the range of a double for finite input: `[1e200]` has
/// RMS `1e200`, and a value near the smallest subnormal does not square to zero. A statistic that
/// is still not finite is not computable: see [`stats_checked`]. The mean keeps what the scaling
/// rounds away from values too small for the scaled range, so `[1e308, -1e308, 1e-300]` has mean
/// `3.33e-301`.
pub fn stats_of(values: &[f64]) -> Stats {
    match stats_checked(values) {
        Ok(s) => s,
        Err(_) => Stats {
            n: values.iter().filter(|v| v.is_finite()).count() as u64,
            ..Stats::EMPTY
        },
    }
}

/// [`stats_of`], or the reason the mean or the RMS is not computable (`n` is then still the count
/// of finite values, in [`stats_of`]'s result). The scaled sums stay within the range of a double
/// for any finite input, so the reason is a guard, not an expected outcome.
pub fn stats_checked(values: &[f64]) -> Result<Stats, &'static str> {
    let mut abs: Vec<f64> = Vec::with_capacity(values.len());
    for &v in values {
        if v.is_finite() {
            abs.push(v.abs());
        }
    }
    let Some(max) = abs
        .iter()
        .copied()
        .max_by(|a, b| oa_math::cmp_canon_f64(*a, *b))
    else {
        return Ok(Stats::EMPTY);
    };
    let n = abs.len() as f64;
    let (mean, rms) = if max == 0.0 {
        (0.0, 0.0)
    } else {
        let scale = scale_of(max);
        let mut sum = ExactSum::default();
        // What dividing by the scale rounds away (a value too small for the scaled range), summed
        // exactly on its own: it is below 8 in magnitude, so it cannot overflow, and a mean
        // that cancels to nothing in the scaled sum keeps it.
        let mut lost = ExactSum::default();
        let mut sumsq = ExactSum::default();
        for &v in values.iter().filter(|v| v.is_finite()) {
            let u = v / scale;
            sum.add(u);
            lost.add(v - u * scale);
            sumsq.add(u * u);
        }
        // Unscaling is exact unless the sum leaves the range of a double; then the mean is as
        // large as the sum is, and what the scaling lost is far below it.
        let total = sum.value() * scale;
        let mean = if total.is_finite() {
            (total + lost.value()) / n
        } else {
            sum.value() / n * scale
        };
        (mean, (sumsq.value() / n).sqrt() * scale)
    };
    if !mean.is_finite() || !rms.is_finite() {
        return Err("statistic not finite");
    }
    abs.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    Ok(Stats {
        n: abs.len() as u64,
        mean,
        rms,
        median: nearest_rank(&abs, 50),
        p95: nearest_rank(&abs, 95),
        p99: nearest_rank(&abs, 99),
        max: abs.last().copied().unwrap_or(f64::NAN),
    })
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

/// The order of a metric's case list: severity (0 is the worst), truth address, time in Unix ms.
/// The same cases in any order of discovery list the same way, so a run in address ranges lists
/// what a run in one piece lists.
pub type CaseKey = (u8, u32, i64);

/// A case line with its place in the list.
pub type KeyedCase = (CaseKey, String);

/// Sorts `cases` by key (then by text, so equal keys are ordered too) and keeps the first
/// [`MAX_CASES`].
pub fn sort_cases(cases: &mut Vec<KeyedCase>) {
    cases.sort();
    cases.truncate(MAX_CASES);
}

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

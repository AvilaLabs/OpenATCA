//! The ADS-B persistent-error screen: how many aircraft broadcast a position with a constant
//! offset or a latency, judged against ERAM's published positions. Pure: plain values in, plain
//! values and text out; `run::run_screen` does the loading.
//!
//! Per sample (one ERAM report of an address-paired flight): ADS-B is evaluated at the ERAM
//! report time by oa-truth's smoother (the traces are thinned, so raw interpolation would be
//! refused for most reports), and the offset ADS-B minus ERAM is resolved along and across the
//! ADS-B velocity direction. A sample is kept only where the smoother is sure (95 % radius at
//! most [`MAX_SIGMA_M`]) and the aircraft is not turning or climbing (interpolation error in a
//! turn would pass as lag). Along offset `= c - L * speed`: L is the apparent latency, positive
//! when ADS-B is late (a late report is behind, so the along offset gets more negative with
//! speed). Two stages, because one aircraft's speed barely varies and its `c` and `L` are
//! collinear: (a) the fleet fit of `c` and `L` over all screened samples (least squares, with
//! Theil-Sen beside it, a bootstrap over aircraft for the intervals); (b) per aircraft the one
//! parameter `L_i` with `c` held at the fleet value, a seeded block bootstrap of consecutive
//! samples for its interval (samples are correlated). A constant along-track avionics offset and
//! a latency cannot be told apart at constant speed, so a LAG is an along-track excess
//! equivalent to a latency of 0.1 s or more. The cross offset is the mean `b`. The two-parameter
//! per-aircraft fit is kept as a cross-check for aircraft whose speed spans [`MIN_SPEED_RANGE_MPS`].

use std::collections::BTreeMap;
use std::fmt::Write as _;

use oa_math::cmp_canon_f64;
use oa_sha256::{sha256, to_hex};
use oa_truth::nacp_epu_m;

use crate::geo::{deg, enu_m};
use crate::model::EramFlight;
use crate::thresholds::THRESHOLDS;

/// Block bootstrap: consecutive samples per block (circular starts), the fewest blocks an
/// aircraft needs, resamples for the fleet fit and for an aircraft, and the base seed.
pub const BLOCK: usize = 10;
pub const MIN_BLOCKS: usize = 10;
pub const RESAMPLES: usize = 200;
pub const AIRCRAFT_RESAMPLES: usize = 1000;
/// Fewest samples (ten blocks) an aircraft needs to be flagged, fewest for the fleet fit, and
/// the shortest span (ms) for either.
pub const MIN_SAMPLES: usize = BLOCK * MIN_BLOCKS;
pub const FIT_MIN_SAMPLES: usize = 30;
pub const MIN_SPAN_MS: i64 = 600_000;
pub const BOOT_SEED: u64 = 0x5eed_adb5_c4ee_0001;
/// LAG: the lower bound of the 95 % interval of the latency excess must exceed this, seconds.
pub const LAG_LOWER_S: f64 = 0.1;
/// BIAS: the offset must exceed this many NACp bounds, with its interval clear of it.
pub const BIAS_FACTOR: f64 = 3.0;
/// Theil-Sen looks at this many samples of each aircraft (evenly spaced within it), all pairs of
/// the pooled points, for the point fit and for every bootstrap resample alike.
pub const TS_PER_AIRCRAFT: usize = 12;
/// A sample whose ADS-B position is farther than this from ERAM's is a gross error of one of the
/// two (or a pairing), not an offset: left out and counted, metres.
pub const MAX_OFFSET_M: f64 = 2000.0;
/// The altitude band of a sample whose ERAM report has no altitude.
pub const ALT_UNKNOWN: i8 = i8::MIN;
/// A sample needs the smoother's 95 % horizontal radius at most this, metres ...
pub const MAX_SIGMA_M: f64 = 15.0;
/// ... a turn rate at most this, degrees per second, and a vertical rate at most this, ft/min.
pub const MAX_TURN_DPS: f64 = 0.5;
pub const MAX_VERT_FPM: f64 = 500.0;
/// The turn rate is the heading change of the smoother over this span either side, ms.
pub const TURN_SPAN_MS: i64 = 2_000;
/// The two-parameter cross-check needs the 5th to 95th percentile of speed to span at least
/// this, m/s (50 kt).
pub const MIN_SPEED_RANGE_MPS: f64 = 50.0 * 0.514_444_444_444;

/// The verbatim caveat the summary carries.
pub const CAVEAT: &str = "ERAM also uses ADS-B in its own tracking, so an ADS-B error shared with ERAM is not visible here; these counts are a lower bound. Radar error grows with range and is not modelled per sample.";

/// One raw ADS-B report of one aircraft, kept for the NACp in force (A7.2: cold).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawReport {
    pub t_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// East and north velocity, m/s, when the report has a ground speed and track.
    pub vel: Option<(f32, f32)>,
    /// NACp in force (carried forward); 0 is unknown.
    pub nacp: u8,
}

/// The NACp in force at `t_ms`: that of the last raw report at or before it (0 when none).
pub fn nacp_at(raw: &[RawReport], t_ms: i64) -> u8 {
    let i = raw.partition_point(|r| r.t_ms <= t_ms);
    i.checked_sub(1)
        .and_then(|k| raw.get(k))
        .map_or(0, |r| r.nacp)
}

/// The distance in time from `t_ms` to the nearest raw report, ms (`i64::MAX` with none).
pub fn nearest_gap_ms(raw: &[RawReport], t_ms: i64) -> i64 {
    let i = raw.partition_point(|r| r.t_ms < t_ms);
    let after = raw.get(i).map(|r| r.t_ms - t_ms);
    let before = i
        .checked_sub(1)
        .and_then(|k| raw.get(k))
        .map(|r| t_ms - r.t_ms);
    after.into_iter().chain(before).min().unwrap_or(i64::MAX)
}

/// The smoother's answer at one time (A7.2: cold).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TruthPoint {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub vel_east_mps: f64,
    pub vel_north_mps: f64,
    /// 95 % horizontal radius, metres.
    pub sigma95_m: f64,
    /// `None` when the smoother has none.
    pub vert_fpm: Option<f64>,
    /// Heading change per second, absolute, over [`TURN_SPAN_MS`] either side; `None` when the
    /// smoother has no state at either end.
    pub turn_dps: Option<f64>,
}

/// One sample of one aircraft (A7.2: cold).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleRow {
    pub t_ms: i64,
    /// ADS-B minus ERAM along the ADS-B velocity direction, metres.
    pub along_m: f64,
    /// ADS-B minus ERAM across it, positive to the right, metres.
    pub cross_m: f64,
    /// ADS-B ground speed, m/s.
    pub speed_mps: f64,
    /// NACp in force; 0 unknown.
    pub nacp: u8,
    /// ERAM altitude band in units of 10,000 ft (floor, at least -1); [`ALT_UNKNOWN`] when ERAM
    /// gave no altitude.
    pub alt_band: i8,
    /// Distance to the nearest raw ADS-B report in time, ms (`i64::MAX` with none).
    pub gap_ms: i64,
}

/// What happened to the ERAM reports of one aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub eram_reports: u64,
    pub dup_overlap: u64,
    pub contradicts: u64,
    pub coast: u64,
    /// The smoother has no state at the ERAM time (outside its legs, no report near, or over its
    /// own 50 m gate).
    pub no_truth: u64,
    /// ADS-B and ERAM more than [`MAX_OFFSET_M`] apart.
    pub offset_gross: u64,
    /// No direction of travel (under 2 kt).
    pub no_velocity: u64,
    /// The along or cross offset is not a finite number.
    pub non_finite: u64,
    pub sigma: u64,
    pub turn_over: u64,
    pub turn_unknown: u64,
    pub vert_over: u64,
    pub vert_unknown: u64,
    pub used: u64,
}

impl Tally {
    pub fn add(&mut self, o: &Tally) {
        self.eram_reports += o.eram_reports;
        self.dup_overlap += o.dup_overlap;
        self.contradicts += o.contradicts;
        self.coast += o.coast;
        self.no_truth += o.no_truth;
        self.offset_gross += o.offset_gross;
        self.non_finite += o.non_finite;
        self.no_velocity += o.no_velocity;
        self.sigma += o.sigma;
        self.turn_over += o.turn_over;
        self.turn_unknown += o.turn_unknown;
        self.vert_over += o.vert_over;
        self.vert_unknown += o.vert_unknown;
        self.used += o.used;
    }

    /// Everything accounted for: the reasons plus `used` equal the ERAM reports.
    pub fn accounted(&self) -> u64 {
        self.dup_overlap
            + self.contradicts
            + self.coast
            + self.no_truth
            + self.offset_gross
            + self.non_finite
            + self.no_velocity
            + self.sigma
            + self.turn_over
            + self.turn_unknown
            + self.vert_over
            + self.vert_unknown
            + self.used
    }
}

/// The samples of one aircraft from its address-paired ERAM flights, the smoother `truth` (the
/// aircraft's state at a time) and its raw reports (for the NACp and the report gap). Inside the
/// overlap of two flights only the one with the earliest first report (ties: the lower position)
/// counts, and a flight lends its span to that rule only over the reports that survive the
/// contradiction rule; a run of `contradict_run` or more consecutive reports of a flight farther
/// than `contradict_m` from the smoother is a contradicted address and its span is left out;
/// coasted ERAM reports are left out. Each remaining report is left out for the first reason that
/// applies: no smoother state, a gross offset, no direction of travel, sigma, turn rate, vertical
/// rate, a non-finite offset. Samples come out in time order.
pub fn build_samples(
    raw: &[RawReport],
    flights: &[&EramFlight],
    truth: &dyn Fn(i64) -> Option<TruthPoint>,
) -> (Vec<SampleRow>, Tally) {
    let th = &THRESHOLDS;
    let mut tally = Tally::default();
    let spans: Vec<Option<(i64, i64)>> = flights.iter().map(|f| f.span()).collect();
    let mut order: Vec<usize> = (0..flights.len()).collect();
    order.sort_by_key(|&k| (spans.get(k).copied().flatten().map_or(i64::MAX, |s| s.0), k));
    let mut rows: Vec<SampleRow> = Vec::new();
    // The spans of the flights already handled, over their reports that survive the
    // contradiction rule.
    let mut earlier: Vec<(i64, i64)> = Vec::new();
    for &k in &order {
        let Some(f) = flights.get(k) else { continue };
        let mine: Vec<&crate::model::EramSample> = f
            .samples
            .iter()
            .filter(|s| {
                tally.eram_reports += 1;
                let dup = earlier.iter().any(|&(c, d)| s.t_ms >= c && s.t_ms <= d);
                tally.dup_overlap += u64::from(dup);
                !dup
            })
            .collect();
        let at: Vec<Option<TruthPoint>> = mine.iter().map(|s| truth(s.t_ms)).collect();
        let far: Vec<bool> = mine
            .iter()
            .zip(&at)
            .map(|(s, p)| {
                p.is_some_and(|p| {
                    let (e, n) = enu_m(p.lat_deg, p.lon_deg, deg(s.lat_e7), deg(s.lon_e7));
                    e.hypot(n) > th.contradict_m
                })
            })
            .collect();
        let mut skip = vec![false; mine.len()];
        let mut m = 0;
        while m < far.len() {
            if !far.get(m).copied().unwrap_or(false) {
                m += 1;
                continue;
            }
            let mut e = m;
            while far.get(e + 1).copied().unwrap_or(false) {
                e += 1;
            }
            if e - m + 1 >= th.contradict_run {
                for x in skip.iter_mut().take(e + 1).skip(m) {
                    *x = true;
                }
            }
            m = e + 1;
        }
        let kept: Vec<i64> = mine
            .iter()
            .zip(&skip)
            .filter(|(_, sk)| !**sk)
            .map(|(s, _)| s.t_ms)
            .collect();
        if let (Some(&a), Some(&b)) = (kept.first(), kept.last()) {
            earlier.push((a, b));
        }
        for ((s, p), skipped) in mine.iter().zip(&at).zip(&skip) {
            if *skipped {
                tally.contradicts += 1;
                continue;
            }
            if s.coast {
                tally.coast += 1;
                continue;
            }
            let Some(p) = p else {
                tally.no_truth += 1;
                continue;
            };
            let (e, n) = enu_m(deg(s.lat_e7), deg(s.lon_e7), p.lat_deg, p.lon_deg);
            if e.hypot(n) > MAX_OFFSET_M {
                tally.offset_gross += 1;
                continue;
            }
            let speed = p.vel_east_mps.hypot(p.vel_north_mps);
            if speed.is_nan() || speed < th.low_speed_mps {
                tally.no_velocity += 1;
                continue;
            }
            if p.sigma95_m.is_nan() || p.sigma95_m > MAX_SIGMA_M {
                tally.sigma += 1;
                continue;
            }
            match p.turn_dps {
                None => {
                    tally.turn_unknown += 1;
                    continue;
                }
                Some(t) if t.is_nan() || t > MAX_TURN_DPS => {
                    tally.turn_over += 1;
                    continue;
                }
                Some(_) => {}
            }
            match p.vert_fpm {
                None => {
                    tally.vert_unknown += 1;
                    continue;
                }
                Some(v) if v.is_nan() || v.abs() > MAX_VERT_FPM => {
                    tally.vert_over += 1;
                    continue;
                }
                Some(_) => {}
            }
            let (ux, uy) = (p.vel_east_mps / speed, p.vel_north_mps / speed);
            let (along_m, cross_m) = (e * ux + n * uy, e * uy - n * ux);
            if !along_m.is_finite() || !cross_m.is_finite() {
                tally.non_finite += 1;
                continue;
            }
            rows.push(SampleRow {
                t_ms: s.t_ms,
                along_m,
                cross_m,
                speed_mps: speed,
                nacp: nacp_at(raw, s.t_ms),
                alt_band: s.alt_ft.map_or(ALT_UNKNOWN, |a| {
                    (f64::from(a) / 10_000.0).floor().clamp(-1.0, 100.0) as i8
                }),
                gap_ms: nearest_gap_ms(raw, s.t_ms),
            });
            tally.used += 1;
        }
    }
    rows.sort_by_key(|r| r.t_ms);
    (rows, tally)
}

/// A small deterministic generator (splitmix64) for the bootstrap.
pub struct SplitMix(pub u64);

impl SplitMix {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0; the modulo bias is far below the bootstrap's noise).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// Median (mean of the two middle values for an even count) of the finite values.
pub fn median(v: &[f64]) -> Option<f64> {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(|a, b| cmp_canon_f64(*a, *b));
    let n = s.len();
    let hi = *s.get(n / 2)?;
    if n % 2 == 1 {
        Some(hi)
    } else {
        Some(0.5 * (hi + *s.get(n / 2 - 1)?))
    }
}

/// Linear-interpolated quantile `q` in [0, 1] of the finite values.
pub fn quantile(v: &[f64], q: f64) -> Option<f64> {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(|a, b| cmp_canon_f64(*a, *b));
    let last = s.len().checked_sub(1)?;
    let pos = q.clamp(0.0, 1.0) * last as f64;
    let i = pos.floor() as usize;
    let a = *s.get(i)?;
    let b = *s.get((i + 1).min(last))?;
    Some(a + (b - a) * (pos - i as f64))
}

/// Least squares of `along` on `speed`: `(intercept, slope, residual standard error of the slope)`.
/// `None` when the speeds do not vary.
pub fn ols(rows: &[&SampleRow]) -> Option<(f64, f64, f64)> {
    let n = rows.len() as f64;
    if rows.len() < 3 {
        return None;
    }
    let mx = rows.iter().map(|r| r.speed_mps).sum::<f64>() / n;
    let my = rows.iter().map(|r| r.along_m).sum::<f64>() / n;
    let sxx: f64 = rows.iter().map(|r| (r.speed_mps - mx).powi(2)).sum();
    let sxy: f64 = rows
        .iter()
        .map(|r| (r.speed_mps - mx) * (r.along_m - my))
        .sum();
    // A speed that does not vary (here: under 0.1 m/s rms) leaves the slope undefined.
    if sxx / n < 0.01 {
        return None;
    }
    let slope = sxy / sxx;
    let icpt = my - slope * mx;
    let ssr: f64 = rows
        .iter()
        .map(|r| (r.along_m - icpt - slope * r.speed_mps).powi(2))
        .sum();
    let se = (ssr / (n - 2.0) / sxx).sqrt();
    Some((icpt, slope, se))
}

fn ci95(v: &[f64]) -> Option<(f64, f64)> {
    Some((quantile(v, 0.025)?, quantile(v, 0.975)?))
}

/// Sufficient statistics of a pooled least squares of `along` on `speed`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Suff {
    pub n: f64,
    pub sx: f64,
    pub sy: f64,
    pub sxx: f64,
    pub sxy: f64,
}

impl Suff {
    pub fn of(rows: &[SampleRow]) -> Suff {
        let mut s = Suff::default();
        for r in rows {
            s.n += 1.0;
            s.sx += r.speed_mps;
            s.sy += r.along_m;
            s.sxx += r.speed_mps * r.speed_mps;
            s.sxy += r.speed_mps * r.along_m;
        }
        s
    }

    pub fn add(&mut self, o: &Suff) {
        self.n += o.n;
        self.sx += o.sx;
        self.sy += o.sy;
        self.sxx += o.sxx;
        self.sxy += o.sxy;
    }

    /// `(c, L)` of `along = c - L * speed`; `None` when the speeds do not vary.
    pub fn fit(&self) -> Option<(f64, f64)> {
        if self.n < 3.0 {
            return None;
        }
        let (mx, my) = (self.sx / self.n, self.sy / self.n);
        let vxx = self.sxx - self.n * mx * mx;
        if vxx / self.n < 0.01 {
            return None;
        }
        let slope = (self.sxy - self.n * mx * my) / vxx;
        Some((my - slope * mx, -slope))
    }
}

/// The two-parameter fit of one aircraft, the cross-check.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct TwoParam {
    pub c_m: f64,
    pub l_s: f64,
    pub l_ci: (f64, f64),
}

/// The fleet references stage (b) works against: the along intercept `c` and the cross offset.
#[derive(Clone, Copy, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Reference {
    pub c_m: f64,
    pub b_m: f64,
}

/// The statistics of one screened aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct AircraftStat {
    pub addr: u32,
    pub n: usize,
    pub span_s: f64,
    pub mean_speed_mps: f64,
    /// Median ERAM altitude band over the samples that have one, [`ALT_UNKNOWN`] with none.
    pub alt_band: i8,
    /// Stage (b): `L_i`, the median over samples of `(c_fleet - along) / speed`, and its
    /// block-bootstrap interval; `boot_n` is the number of valid replicates behind it.
    pub l_s: f64,
    pub l_ci: (f64, f64),
    pub boot_n: usize,
    /// Cross offset: the median, its excess over the fleet reference `b - b_fleet`, and the
    /// interval of the magnitude of that excess.
    pub b_m: f64,
    pub b_rel_m: f64,
    pub absb_ci: (f64, f64),
    /// Median NACp in force and its 95 % bound (infinite when unknown).
    pub nacp_median: u8,
    pub bound_m: f64,
    /// Present only when the aircraft's speed spans [`MIN_SPEED_RANGE_MPS`].
    pub two: Option<TwoParam>,
}

/// Whether an aircraft's samples reach `min` samples and [`MIN_SPAN_MS`], and if not the first
/// reason.
pub fn eligibility_for(rows: &[SampleRow], min: usize) -> Result<(), &'static str> {
    if rows.len() < min {
        return Err("too few samples");
    }
    match (rows.first(), rows.last()) {
        (Some(f), Some(l)) if l.t_ms - f.t_ms >= MIN_SPAN_MS => Ok(()),
        _ => Err("too short a span"),
    }
}

/// Whether an aircraft's samples are enough to be flagged (ten blocks over 10 minutes).
pub fn eligible(rows: &[SampleRow]) -> bool {
    eligibility_for(rows, MIN_SAMPLES).is_ok()
}

/// The median of `(c - along) / speed` of the samples: the one-parameter latency at `c`.
fn l_median(rows: &[&SampleRow], c: f64) -> Option<f64> {
    median(
        &rows
            .iter()
            .map(|r| (c - r.along_m) / r.speed_mps)
            .collect::<Vec<_>>(),
    )
}

fn cross_median(rows: &[&SampleRow]) -> Option<f64> {
    median(&rows.iter().map(|r| r.cross_m).collect::<Vec<_>>())
}

/// The bootstrap replicates of one aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Boot {
    pub l: Vec<f64>,
    pub absb: Vec<f64>,
    pub l2: Vec<f64>,
}

/// Block bootstrap of one aircraft: [`AIRCRAFT_RESAMPLES`] resamples of `n / BLOCK` blocks of
/// [`BLOCK`] consecutive samples, each block starting anywhere (circularly). Returns, per
/// resample, the median latency at `c_fleet`, `|median cross - b_fleet|` and (when `two`) the
/// two-parameter `L`.
pub fn bootstrap(rows: &[SampleRow], seed: u64, r: Reference, two: bool) -> Boot {
    let mut rng = SplitMix(seed);
    let mut out = Boot::default();
    let n = rows.len();
    let blocks = (n / BLOCK).max(1);
    for _ in 0..AIRCRAFT_RESAMPLES {
        let mut pick: Vec<&SampleRow> = Vec::with_capacity(blocks * BLOCK);
        for _ in 0..blocks {
            let s = rng.below(n);
            for j in 0..BLOCK {
                pick.extend(rows.get((s + j) % n));
            }
        }
        if let (Some(l), Some(b)) = (l_median(&pick, r.c_m), cross_median(&pick)) {
            out.l.push(l);
            out.absb.push((b - r.b_m).abs());
        }
        if two {
            if let Some((_, slope, _)) = ols(&pick) {
                out.l2.push(-slope);
            }
        }
    }
    out
}

/// Stage (b) for one aircraft against the fleet reference `r`: `None` when the aircraft is not
/// eligible or a statistic is undefined. Deterministic: the bootstrap seed is a function of the
/// address.
pub fn screen_aircraft(addr: u32, rows: &[SampleRow], r: Reference) -> Option<AircraftStat> {
    if !eligible(rows) {
        return None;
    }
    let refs: Vec<&SampleRow> = rows.iter().collect();
    let n = rows.len() as f64;
    let l = l_median(&refs, r.c_m)?;
    let b = cross_median(&refs)?;
    let mean_speed = rows.iter().map(|r| r.speed_mps).sum::<f64>() / n;
    let mut speeds: Vec<f64> = rows.iter().map(|r| r.speed_mps).collect();
    speeds.sort_by(|a, b| cmp_canon_f64(*a, *b));
    let wide = quantile(&speeds, 0.95)
        .zip(quantile(&speeds, 0.05))
        .is_some_and(|(hi, lo)| hi - lo >= MIN_SPEED_RANGE_MPS);
    let boot = bootstrap(
        rows,
        BOOT_SEED ^ u64::from(addr).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        r,
        wide,
    );
    let two = if wide {
        ols(&refs)
            .zip(ci95(&boot.l2))
            .map(|((c, slope, _), ci)| TwoParam {
                c_m: c,
                l_s: -slope,
                l_ci: ci,
            })
    } else {
        None
    };
    let nacps: Vec<f64> = rows
        .iter()
        .filter(|r| r.nacp > 0)
        .map(|r| f64::from(r.nacp))
        .collect();
    let nacp_median = median(&nacps).map_or(0, |m| m.floor() as u8);
    let mut bands: Vec<i8> = rows
        .iter()
        .map(|r| r.alt_band)
        .filter(|&b| b != ALT_UNKNOWN)
        .collect();
    bands.sort_unstable();
    let (first, last) = (rows.first()?, rows.last()?);
    Some(AircraftStat {
        addr,
        n: rows.len(),
        span_s: (last.t_ms - first.t_ms) as f64 / 1000.0,
        mean_speed_mps: mean_speed,
        alt_band: bands.get(bands.len() / 2).copied().unwrap_or(ALT_UNKNOWN),
        l_s: l,
        l_ci: ci95(&boot.l)?,
        boot_n: boot.l.len(),
        b_m: b,
        b_rel_m: b - r.b_m,
        absb_ci: ci95(&boot.absb)?,
        nacp_median,
        bound_m: nacp_epu_m(nacp_median),
        two,
    })
}

/// One aircraft with the fleet reference applied and its flags.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Screened {
    pub stat: AircraftStat,
    /// `L_i` minus the fleet `L`, and its interval (the interval of `L_i` shifted by the
    /// reference).
    pub excess_s: f64,
    pub excess_ci: (f64, f64),
    /// The flags after the influence check: a flag stays only if it survives dropping the one
    /// sample that most supports it.
    pub lag: bool,
    pub bias: bool,
    /// The flags before it.
    pub lag_pre: bool,
    pub bias_pre: bool,
    /// The whole interval of the excess is under -0.1 s: ADS-B early, reported, not a LAG.
    pub early: bool,
    /// |b - fleet b| over the NACp bound; `None` when the bound is infinite.
    pub bias_ratio: Option<f64>,
}

/// LAG: the lower bound of the excess interval is strictly over [`LAG_LOWER_S`].
pub fn is_lag(excess_ci_lo: f64) -> bool {
    excess_ci_lo > LAG_LOWER_S
}

/// EARLY: the upper bound of the excess interval is strictly under `-LAG_LOWER_S`.
pub fn is_early(excess_ci_hi: f64) -> bool {
    excess_ci_hi < -LAG_LOWER_S
}

/// The BIAS threshold of an aircraft: [`BIAS_FACTOR`] NACp bounds, but not under the fleet's
/// own p95 spread of the cross offset (`fleet_p95_m`); infinite for an unknown NACp.
pub fn bias_threshold(bound_m: f64, fleet_p95_m: f64) -> f64 {
    if bound_m.is_finite() {
        (BIAS_FACTOR * bound_m).max(fleet_p95_m)
    } else {
        f64::INFINITY
    }
}

/// BIAS: the offset from the fleet reference strictly over the threshold, and the lower bound of
/// its interval too. A non-finite threshold never flags.
pub fn is_bias(abs_b: f64, absb_ci_lo: f64, bound_m: f64, fleet_p95_m: f64) -> bool {
    let t = bias_threshold(bound_m, fleet_p95_m);
    t.is_finite() && abs_b > t && absb_ci_lo > t
}

/// What the flags of one aircraft are, from its statistics.
fn flags(s: &AircraftStat, l_fleet: f64, p95: f64) -> (bool, bool, bool) {
    let (lo, hi) = (s.l_ci.0 - l_fleet, s.l_ci.1 - l_fleet);
    (
        is_lag(lo),
        is_bias(s.b_rel_m.abs(), s.absb_ci.0, s.bound_m, p95),
        is_early(hi),
    )
}

/// The aircraft's samples without index `j`.
fn without(rows: &[SampleRow], j: usize) -> Vec<SampleRow> {
    rows.iter()
        .enumerate()
        .filter(|(i, _)| *i != j)
        .map(|(_, r)| *r)
        .collect()
}

/// The index of the sample whose removal moves `stat` (a statistic of the samples) lowest: the
/// sample that most supports a high value of it.
pub fn most_supporting(
    rows: &[SampleRow],
    stat: &dyn Fn(&[&SampleRow]) -> Option<f64>,
) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for j in 0..rows.len() {
        let rest: Vec<&SampleRow> = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != j)
            .map(|(_, r)| r)
            .collect();
        let v = stat(&rest)?;
        if best.is_none_or(|(b, _)| v < b) {
            best = Some((v, j));
        }
    }
    best.map(|(_, j)| j)
}

/// Stage (b) for every eligible aircraft against the fleet reference `r` and fleet latency
/// `l_fleet`, with the flags checked for influence: a LAG (BIAS) stays only if the aircraft,
/// recomputed without the one sample that most supports it, is still one.
fn against(
    ok: &[&(u32, Vec<SampleRow>)],
    r: Reference,
    l_fleet: f64,
    p95: f64,
) -> Result<Vec<Screened>, String> {
    let mut out = Vec::new();
    for (addr, rows) in ok {
        let s = screen_aircraft(*addr, rows, r)
            .ok_or_else(|| format!("an eligible aircraft could not be screened ({addr:06x})"))?;
        let (lag_pre, bias_pre, early) = flags(&s, l_fleet, p95);
        let survives = |stat: &dyn Fn(&[&SampleRow]) -> Option<f64>,
                        pick: fn(bool, bool, bool) -> bool| {
            most_supporting(rows, stat)
                .and_then(|j| screen_aircraft(*addr, &without(rows, j), r))
                .is_some_and(|t| {
                    let (a, b, c) = flags(&t, l_fleet, p95);
                    pick(a, b, c)
                })
        };
        let lag = lag_pre && survives(&|p| l_median(p, r.c_m), |a, _, _| a);
        // The sample that most supports a large |b - b_fleet| is the one whose removal lowers
        // it most; the median is monotone in each sample, so search on the signed side of b.
        let bias = bias_pre
            && survives(
                &|p| cross_median(p).map(|b| (b - r.b_m) * oa_math::signum_canon_f64(s.b_rel_m)),
                |_, b, _| b,
            );
        let excess_ci = (s.l_ci.0 - l_fleet, s.l_ci.1 - l_fleet);
        out.push(Screened {
            excess_s: s.l_s - l_fleet,
            excess_ci,
            lag,
            bias,
            lag_pre,
            bias_pre,
            early,
            bias_ratio: s.bound_m.is_finite().then(|| s.b_rel_m.abs() / s.bound_m),
            stat: s,
        });
    }
    Ok(out)
}

/// The fleet fit of stage (a): `along = c - L * speed` over every sample of the eligible
/// aircraft, by least squares and by Theil-Sen (the reference: lagged aircraft pull the least
/// squares).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct FleetFit {
    pub aircraft: usize,
    pub samples: usize,
    /// Least squares, with the 95 % bootstrap interval over aircraft and its valid replicates.
    pub c_m: f64,
    pub l_s: f64,
    pub c_ci: (f64, f64),
    pub l_ci: (f64, f64),
    pub ols_boot_n: usize,
    /// Theil-Sen: `L` and the median of `along + L * speed`, with the bootstrap interval over
    /// aircraft (and its valid replicates) when it was run.
    pub l_ts_s: f64,
    pub c_ts_m: f64,
    pub l_ts_ci: Option<(f64, f64)>,
    pub c_ts_ci: Option<(f64, f64)>,
    pub ts_boot_n: usize,
}

/// One row of the sensitivity table: the LAG count with the fleet intercept (and latency) set
/// as named.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub label: &'static str,
    pub c_m: f64,
    pub l_s: f64,
    pub lag: usize,
    pub early: usize,
}

/// The fleet result.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Fleet {
    pub fit: FleetFit,
    /// Aircraft against the Theil-Sen reference (the result) and against the least-squares one.
    pub aircraft: Vec<Screened>,
    pub aircraft_ols_ref: Vec<Screened>,
    /// The sensitivity table: Theil-Sen c, its interval's ends, least squares.
    pub variants: Vec<Variant>,
    /// The fleet cross reference (median of the aircraft medians) and the fleet p95 of
    /// `|b - reference|`, the floor of the BIAS threshold.
    pub b_ref_m: f64,
    pub bias_p95_m: f64,
    /// Aircraft with samples, those in the fleet fit (30 samples over 10 minutes), those screened
    /// for flags (ten blocks over 10 minutes), and the reasons the rest are out of the fit.
    pub with_samples: usize,
    pub in_fit: usize,
    pub eligible: usize,
    pub few_samples: usize,
    pub short_span: usize,
    /// Distance in time of every used sample to the nearest raw ADS-B report, seconds.
    pub gaps_s: Vec<f64>,
}

/// Theil-Sen over the aircraft `picks` (aircraft may repeat, as in a bootstrap resample):
/// [`TS_PER_AIRCRAFT`] evenly spaced samples of each, every pair of the pooled points; `c` is the
/// median of `along + L * speed` over all samples. Returns `(c, L)`.
pub fn ts_fit(picks: &[&[SampleRow]]) -> Option<(f64, f64)> {
    let pts: Vec<&SampleRow> = picks
        .iter()
        .flat_map(|r| {
            let n = r.len();
            (0..TS_PER_AIRCRAFT.min(n)).filter_map(move |i| r.get(i * n / TS_PER_AIRCRAFT.min(n)))
        })
        .collect();
    let mut slopes: Vec<f64> = Vec::new();
    for (i, a) in pts.iter().enumerate() {
        for b in pts.iter().skip(i + 1) {
            let dx = b.speed_mps - a.speed_mps;
            if dx.abs() > 1e-9 {
                slopes.push((b.along_m - a.along_m) / dx);
            }
        }
    }
    let l = -median(&slopes)?;
    let c = median(
        &picks
            .iter()
            .flat_map(|r| r.iter())
            .map(|r| r.along_m + l * r.speed_mps)
            .collect::<Vec<_>>(),
    )?;
    Some((c, l))
}

/// Stage (a): the fleet fit with bootstraps over aircraft (whole aircraft resampled with
/// replacement, seeded); `ts_ci` also bootstraps the Theil-Sen fit.
pub fn fleet_fit(eligible_rows: &[&[SampleRow]], ts_ci: bool) -> Option<FleetFit> {
    let suffs: Vec<Suff> = eligible_rows.iter().map(|r| Suff::of(r)).collect();
    let mut all = Suff::default();
    for s in &suffs {
        all.add(s);
    }
    let (c, l) = all.fit()?;
    let mut rng = SplitMix(BOOT_SEED ^ 0xf1ee7);
    let (mut cs, mut ls) = (Vec::new(), Vec::new());
    let (mut cts, mut lts) = (Vec::new(), Vec::new());
    for _ in 0..RESAMPLES {
        let mut t = Suff::default();
        let mut pick: Vec<&[SampleRow]> = Vec::new();
        for _ in 0..suffs.len() {
            let k = rng.below(suffs.len());
            if let Some(s) = suffs.get(k) {
                t.add(s);
            }
            if ts_ci {
                pick.extend(eligible_rows.get(k).copied());
            }
        }
        if let Some((c, l)) = t.fit() {
            cs.push(c);
            ls.push(l);
        }
        if ts_ci {
            if let Some((c, l)) = ts_fit(&pick) {
                cts.push(c);
                lts.push(l);
            }
        }
    }
    let (c_ts, l_ts) = ts_fit(eligible_rows)?;
    Some(FleetFit {
        aircraft: eligible_rows.len(),
        samples: eligible_rows.iter().map(|r| r.len()).sum(),
        c_m: c,
        l_s: l,
        c_ci: ci95(&cs)?,
        l_ci: ci95(&ls)?,
        ols_boot_n: cs.len(),
        l_ts_s: l_ts,
        c_ts_m: c_ts,
        l_ts_ci: if ts_ci { ci95(&lts) } else { None },
        c_ts_ci: if ts_ci { ci95(&cts) } else { None },
        ts_boot_n: lts.len(),
    })
}

/// Both stages over `inputs` (address, samples in time order). Fails when no aircraft is
/// eligible, the fleet's speeds do not vary, or an eligible aircraft cannot be screened (so
/// screened equals eligible). The Theil-Sen fleet fit is the reference; `ts_ci` runs its
/// bootstrap (and, with it, the sensitivity at the ends of its intercept interval).
pub fn screen_fleet(inputs: &[(u32, Vec<SampleRow>)], ts_ci: bool) -> Result<Fleet, String> {
    let mut fit_set: Vec<&(u32, Vec<SampleRow>)> = Vec::new();
    let (mut few_samples, mut short_span) = (0, 0);
    for i in inputs {
        match eligibility_for(&i.1, FIT_MIN_SAMPLES) {
            Ok(()) => fit_set.push(i),
            Err("too few samples") => few_samples += 1,
            Err(_) => short_span += 1,
        }
    }
    fit_set.sort_by_key(|i| i.0);
    // The fleet fit and the cross reference use every aircraft with enough for them; only the
    // aircraft with ten blocks are flagged.
    let ok: Vec<&(u32, Vec<SampleRow>)> =
        fit_set.iter().copied().filter(|i| eligible(&i.1)).collect();
    let rows: Vec<&[SampleRow]> = fit_set.iter().map(|i| i.1.as_slice()).collect();
    let fit = fleet_fit(&rows, ts_ci)
        .ok_or("no fleet fit: no aircraft with enough samples or no speed variation")?;
    // The fleet cross reference: the median of the aircraft's median cross offsets.
    let medians: Vec<f64> = rows
        .iter()
        .filter_map(|r| cross_median(&r.iter().collect::<Vec<_>>()))
        .collect();
    let b_ref_m = median(&medians).ok_or("no cross reference")?;
    let abs_rel: Vec<f64> = medians.iter().map(|b| (b - b_ref_m).abs()).collect();
    let bias_p95_m = quantile(&abs_rel, 0.95).ok_or("no cross spread")?;
    let at = |c: f64| Reference {
        c_m: c,
        b_m: b_ref_m,
    };
    let aircraft = against(&ok, at(fit.c_ts_m), fit.l_ts_s, bias_p95_m)?;
    let aircraft_ols_ref = against(&ok, at(fit.c_m), fit.l_s, bias_p95_m)?;
    let mut variants = vec![Variant {
        label: "Theil-Sen c",
        c_m: fit.c_ts_m,
        l_s: fit.l_ts_s,
        lag: aircraft.iter().filter(|s| s.lag).count(),
        early: aircraft.iter().filter(|s| s.early).count(),
    }];
    if let Some((lo, hi)) = fit.c_ts_ci {
        for (label, c) in [
            ("Theil-Sen c, interval low", lo),
            ("Theil-Sen c, interval high", hi),
        ] {
            let v = against(&ok, at(c), fit.l_ts_s, bias_p95_m)?;
            variants.push(Variant {
                label,
                c_m: c,
                l_s: fit.l_ts_s,
                lag: v.iter().filter(|s| s.lag).count(),
                early: v.iter().filter(|s| s.early).count(),
            });
        }
    }
    variants.push(Variant {
        label: "least squares",
        c_m: fit.c_m,
        l_s: fit.l_s,
        lag: aircraft_ols_ref.iter().filter(|s| s.lag).count(),
        early: aircraft_ols_ref.iter().filter(|s| s.early).count(),
    });
    let gaps_s = inputs
        .iter()
        .flat_map(|i| i.1.iter())
        .map(|r| r.gap_ms as f64 / 1000.0)
        .collect();
    Ok(Fleet {
        eligible: ok.len(),
        in_fit: fit_set.len(),
        with_samples: inputs.len(),
        few_samples,
        short_span,
        fit,
        aircraft,
        aircraft_ols_ref,
        variants,
        b_ref_m,
        bias_p95_m,
        gaps_s,
    })
}

/// The rendered screen.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRendered {
    pub aircraft_csv: String,
    /// Ends with the digest line.
    pub summary: String,
    pub digest: String,
}

fn num(v: f64, dec: usize) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let s = format!("{v:.dec$}");
    if s.starts_with('-') && s.chars().all(|c| matches!(c, '-' | '0' | '.')) {
        s.trim_start_matches('-').to_string()
    } else {
        s
    }
}

/// The per-aircraft CSV; the address is the first column (the summary never names one). `L_s`
/// is stage (b); `lag`/`bias` are after the influence check, `*_pre` before it; `L2_*` and
/// `c2_m` are the two-parameter cross-check, empty when the aircraft's speed range is under
/// 50 kt; the altitude band is empty when ERAM gave no altitude.
pub fn aircraft_csv(f: &Fleet) -> String {
    let mut o = String::from(
        "addr,n,span_s,mean_speed_mps,alt_band_10kft,L_s,L_lo_s,L_hi_s,L_excess_s,L_excess_lo_s,L_excess_hi_s,lag,lag_pre,early,b_median_m,b_rel_m,abs_b_rel_lo_m,abs_b_rel_hi_m,nacp_median,nacp_bound_m,bias_ratio,bias,bias_pre,boot_n,c2_m,L2_s,L2_lo_s,L2_hi_s,L_excess_ols_ref_s,lag_ols_ref\n",
    );
    for (s, alt) in f.aircraft.iter().zip(&f.aircraft_ols_ref) {
        let t = &s.stat;
        let two = |g: fn(&TwoParam) -> f64, dec: usize| {
            t.two.as_ref().map_or(String::new(), |w| num(g(w), dec))
        };
        let band = if t.alt_band == ALT_UNKNOWN {
            String::new()
        } else {
            t.alt_band.to_string()
        };
        let _ = writeln!(
            o,
            "{:06x},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            t.addr,
            t.n,
            num(t.span_s, 0),
            num(t.mean_speed_mps, 1),
            band,
            num(t.l_s, 3),
            num(t.l_ci.0, 3),
            num(t.l_ci.1, 3),
            num(s.excess_s, 3),
            num(s.excess_ci.0, 3),
            num(s.excess_ci.1, 3),
            u8::from(s.lag),
            u8::from(s.lag_pre),
            u8::from(s.early),
            num(t.b_m, 2),
            num(t.b_rel_m, 2),
            num(t.absb_ci.0, 2),
            num(t.absb_ci.1, 2),
            t.nacp_median,
            num(t.bound_m, 1),
            s.bias_ratio.map_or(String::new(), |r| num(r, 3)),
            u8::from(s.bias),
            u8::from(s.bias_pre),
            t.boot_n,
            two(|w| w.c_m, 2),
            two(|w| w.l_s, 3),
            two(|w| w.l_ci.0, 3),
            two(|w| w.l_ci.1, 3),
            num(alt.excess_s, 3),
            u8::from(alt.lag),
        );
    }
    o
}

fn dist_line(label: &str, v: &[f64], dec: usize) -> String {
    let q = |p: f64| quantile(v, p).map_or("n/a".to_string(), |x| num(x, dec));
    format!(
        "  {label}: p05 {}  p50 {}  p95 {}  (n={})\n",
        q(0.05),
        q(0.5),
        q(0.95),
        v.iter().filter(|x| x.is_finite()).count()
    )
}

/// Renders the summary and the CSV. `header` lines (inputs, window) come first;
/// `flights_paired` is the number of address-paired ERAM flights, `tally` the sample accounting.
pub fn render_screen(
    f: &Fleet,
    header: &[String],
    flights_paired: u64,
    tally: &Tally,
) -> ScreenRendered {
    let csv = aircraft_csv(f);
    let n = f.aircraft.len();
    let frac = |k: usize| {
        if n == 0 {
            "n/a".to_string()
        } else {
            format!("{:.2} %", 100.0 * k as f64 / n as f64)
        }
    };
    let count = |g: fn(&Screened) -> bool| f.aircraft.iter().filter(|s| g(s)).count();
    let (lag, bias, early) = (count(|s| s.lag), count(|s| s.bias), count(|s| s.early));
    let (lag_pre, bias_pre) = (count(|s| s.lag_pre), count(|s| s.bias_pre));
    let both = count(|s| s.lag && s.bias);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "ADS-B persistent-error screen against ERAM (SFDPS), offline"
    );
    for h in header {
        let _ = writeln!(s, "  {h}");
    }
    let _ = writeln!(
        s,
        "\nMethod: ADS-B evaluated at each ERAM report time by oa-truth's smoother (default settings); address-paired flights only; DUP_ERAM_TRACK overlaps (a flight lends its span over the reports that survive the contradiction rule), ADDR_CONTRADICTS_TRACK spans and coasted ERAM reports left out; a sample needs |ADS-B - ERAM| at most {MAX_OFFSET_M} m, smoother sigma95 at most {MAX_SIGMA_M} m, turn rate at most {MAX_TURN_DPS} deg/s over +-{} s, |vertical rate| at most {MAX_VERT_FPM} ft/min, a direction of travel (over 2 kt) and finite offsets. Along offset = ADS-B minus ERAM along the ADS-B velocity = c - L x speed (L is the apparent latency, positive when ADS-B is late); cross offset = across it, positive to the right.",
        TURN_SPAN_MS / 1000
    );
    let _ = writeln!(
        s,
        "Stage (a): fleet fit of c and L over all samples of the screened aircraft: Theil-Sen over all pairs of {TS_PER_AIRCRAFT} evenly spaced samples of each aircraft (the reference; c is the median of along + L x speed over all samples), and least squares; bootstrap over aircraft, {RESAMPLES} resamples. Stage (b): per aircraft, c held at the fleet value, L_i = the median of (c - along) / speed (an aircraft's speed barely varies, so c and L are collinear within one aircraft); b = the median cross offset. 95 % intervals by block bootstrap, {AIRCRAFT_RESAMPLES} resamples of n / {BLOCK} blocks of {BLOCK} consecutive samples (circular starts), seeded by the address. Screened: {MIN_SAMPLES} or more samples ({MIN_BLOCKS} blocks of {BLOCK}) over {} s or more. A constant along-track avionics offset and a latency are indistinguishable at constant speed, so LAG means an along-track excess equivalent to a latency of {LAG_LOWER_S} s or more.",
        MIN_SPAN_MS / 1000
    );
    let _ = writeln!(
        s,
        "Flags: LAG when the lower 95 % bound of (L_i - fleet L) is over {LAG_LOWER_S} s; EARLY when the upper bound is under -{LAG_LOWER_S} s (reported, not a LAG); BIAS when |b - fleet cross reference| is over the larger of {BIAS_FACTOR} x the 95 % bound of the aircraft's median NACp and the fleet's p95 of |b - reference|, and the lower 95 % bound of it is too. A LAG or BIAS must also survive dropping the one sample that most supports it (the aircraft recomputed without it). The two-parameter per-aircraft fit (columns c2_m, L2_*) is a cross-check for the aircraft whose speed (5th to 95th percentile) spans 50 kt or more."
    );
    let _ = writeln!(
        s,
        "\nSamples: {} ERAM reports of {} address-paired flights. Left out: {} in duplicate overlaps, {} in contradicted-address spans, {} coasted, {} with no smoother state, {} with |ADS-B - ERAM| over {MAX_OFFSET_M} m, {} with no direction of travel, {} with sigma95 over {MAX_SIGMA_M} m, {} turning over {MAX_TURN_DPS} deg/s (and {} with no turn rate), {} with |vertical rate| over {MAX_VERT_FPM} ft/min (and {} with none), {} with a non-finite offset; used {}.",
        tally.eram_reports,
        flights_paired,
        tally.dup_overlap,
        tally.contradicts,
        tally.coast,
        tally.no_truth,
        tally.offset_gross,
        tally.no_velocity,
        tally.sigma,
        tally.turn_over,
        tally.turn_unknown,
        tally.vert_over,
        tally.vert_unknown,
        tally.non_finite,
        tally.used
    );
    s.push_str(&dist_line(
        "distance of a used sample to the nearest ADS-B report, s",
        &f.gaps_s,
        1,
    ));
    let _ = writeln!(
        s,
        "Aircraft: {} with samples; {} in the fleet fit ({FIT_MIN_SAMPLES} or more samples over {} s or more; {} left out with too few samples, {} with too short a span); screened for flags: {n} of {} in the fleet fit ({MIN_SAMPLES} or more samples, {MIN_BLOCKS} blocks of {BLOCK}; equal to the eligible {}, checked). Every prevalence line below is over the {n} screened for flags.",
        f.with_samples,
        f.in_fit,
        MIN_SPAN_MS / 1000,
        f.few_samples,
        f.short_span,
        f.in_fit,
        f.eligible
    );
    if n < 30 {
        let _ = writeln!(
            s,
            "This day has too few long, gated tracks for a prevalence estimate ({n} aircraft screened for flags)."
        );
    }
    let ft = &f.fit;
    let two = f.aircraft.iter().filter(|a| a.stat.two.is_some()).count();
    let ci = |c: Option<(f64, f64)>, dec: usize| {
        c.map_or("not run".to_string(), |c| {
            format!("[{}, {}]", num(c.0, dec), num(c.1, dec))
        })
    };
    let min_boot = f.aircraft.iter().map(|a| a.stat.boot_n).min().unwrap_or(0);
    let _ = writeln!(
        s,
        "Fleet fit ({} aircraft, {} samples), reference = Theil-Sen: L {} s {}, c {} m {} ({} valid of {RESAMPLES} aircraft-bootstrap resamples). Least squares (pulled by lagged aircraft): L {} s [{}, {}], c {} m [{}, {}] ({} valid). Aircraft intervals: at least {min_boot} valid of {AIRCRAFT_RESAMPLES} resamples each. Two-parameter cross-check available for {two} aircraft.",
        ft.aircraft,
        ft.samples,
        num(ft.l_ts_s, 3),
        ci(ft.l_ts_ci, 3),
        num(ft.c_ts_m, 2),
        ci(ft.c_ts_ci, 2),
        ft.ts_boot_n,
        num(ft.l_s, 3),
        num(ft.l_ci.0, 3),
        num(ft.l_ci.1, 3),
        num(ft.c_m, 2),
        num(ft.c_ci.0, 2),
        num(ft.c_ci.1, 2),
        ft.ols_boot_n,
    );
    let tmax = f
        .aircraft
        .iter()
        .filter_map(|a| {
            a.stat
                .bound_m
                .is_finite()
                .then_some(BIAS_FACTOR * a.stat.bound_m)
        })
        .collect::<Vec<_>>();
    let _ = writeln!(
        s,
        "Cross reference (median of the aircraft's median cross offsets): {} m. BIAS threshold = max({BIAS_FACTOR} x NACp bound, fleet p95 of |b - reference|): the second term is {} m; the first is p05 {} m, p50 {} m, p95 {} m over the aircraft.",
        num(f.b_ref_m, 2),
        num(f.bias_p95_m, 2),
        quantile(&tmax, 0.05).map_or("n/a".into(), |x| num(x, 1)),
        quantile(&tmax, 0.5).map_or("n/a".into(), |x| num(x, 1)),
        quantile(&tmax, 0.95).map_or("n/a".into(), |x| num(x, 1)),
    );
    let _ = writeln!(
        s,
        "LAG:  {lag} of {n} screened ({}); {lag_pre} before the influence check, {} removed by it",
        frac(lag),
        lag_pre - lag
    );
    let _ = writeln!(s, "BIAS: {bias} of {n} screened ({}); {bias_pre} before the influence check, {} removed by it. Threshold rule: |b - reference| over max({BIAS_FACTOR} x NACp bound, {} m), lower bound too.", frac(bias), bias_pre - bias, num(f.bias_p95_m, 2));
    let _ = writeln!(
        s,
        "EARLY (upper bound of the excess under -{LAG_LOWER_S} s, not a LAG): {early} of {n} ({})",
        frac(early)
    );
    let _ = writeln!(
        s,
        "Sensitivity of LAG to the fleet reference (BIAS does not depend on it; c varied alone, fleet L as stated):\n  {:<28} {:>11}  {:>11}  {:>4}  {:>5}",
        "reference", "fleet L (s)", "fleet c (m)", "LAG", "EARLY"
    );
    for v in &f.variants {
        let _ = writeln!(
            s,
            "  {:<28} {:>11}  {:>11}  {:>4}  {:>5}",
            v.label,
            num(v.l_s, 3),
            num(v.c_m, 2),
            v.lag,
            v.early
        );
    }
    let _ = writeln!(s, "both: {both}");
    let col = |g: fn(&Screened) -> f64| f.aircraft.iter().map(g).collect::<Vec<_>>();
    s.push_str(&dist_line("L_i - fleet L, s", &col(|a| a.excess_s), 3));
    s.push_str(&dist_line(
        "|b - reference| / NACp bound",
        &col(|a| a.bias_ratio.unwrap_or(f64::NAN)),
        3,
    ));
    s.push_str(&dist_line(
        "L_i 95 % interval width, s",
        &col(|a| a.stat.l_ci.1 - a.stat.l_ci.0),
        3,
    ));
    s.push_str(&dist_line(
        "|b - reference|, m",
        &col(|a| a.stat.b_rel_m.abs()),
        2,
    ));
    s.push_str(&dist_line(
        "L2 - L_i, s (cross-check)",
        &col(|a| a.stat.two.as_ref().map_or(f64::NAN, |w| w.l_s - a.stat.l_s)),
        3,
    ));
    let _ = writeln!(s, "\n{CAVEAT}");
    let _ = writeln!(s, "\nPer-aircraft values are in aircraft.csv.");
    let mut d = Vec::new();
    d.extend_from_slice(csv.as_bytes());
    d.extend_from_slice(s.as_bytes());
    let digest = to_hex(&sha256(&d));
    let _ = writeln!(
        s,
        "digest sha256:{digest} (aircraft.csv and this summary above)"
    );
    ScreenRendered {
        aircraft_csv: csv,
        summary: s,
        digest,
    }
}

/// Groups `(address, flight)` pairs into the flights of each address (a helper for the run).
pub fn by_address<'a>(
    flights: impl Iterator<Item = (u32, &'a EramFlight)>,
) -> BTreeMap<u32, Vec<&'a EramFlight>> {
    let mut m: BTreeMap<u32, Vec<&EramFlight>> = BTreeMap::new();
    for (a, f) in flights {
        m.entry(a).or_default().push(f);
    }
    m
}

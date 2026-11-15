//! The benchmark of a tracker's output, one address range at a time. Core (pure).
//!
//! [`crate::run::run_benchmark`] holds every sample of the run at once, which for a tracker's
//! one-second output grows with the run (about 900 bytes per output row once the legacy table,
//! its index, the pair table and the extraction columns are counted). Here the rows are taken in
//! address ranges (an aircraft is never split), each range is scored with the same functions
//! (`associate`, `build_pairs`, T9, F, R, H), and the pieces that are sums or sample lists are
//! added up in a [`ChunkAcc`]. Memory is the largest range plus what the result itself needs:
//!
//! * the paired samples of T1 to T4 are kept (about 60 bytes per paired sample, a quarter of the
//!   output rows): their statistics need every value, and the percentiles are exact, from the
//!   same sort over the same values in the same order as the one-shot run;
//! * T5 (the publication interval) is kept as a histogram of whole milliseconds (the position
//!   times are whole milliseconds), at most 600,001 bins per split, and its percentiles are read
//!   exactly from the counts; the mean and RMS come from exact integer sums, which equal the
//!   one-shot floating point sums when the values are whole seconds and differ only in the last
//!   bits otherwise;
//! * T7 and T8 are counts and one delay per truth leg;
//! * T6 is withheld for a tracker's output (no receive time); a chunk that has one is refused;
//! * F1 to F3, R1 and H1 have no data in a tracker's legacy table; a chunk where one is not empty
//!   is refused.
//!
//! A range must hold every flight associated with a truth address in it (true for a tracker's
//! output, which is associated by address). That is checked, not assumed: a chunk with a flight
//! keyed or associated outside its range is refused, two ranges that overlap are refused, and a
//! run whose ranges do not together cover the whole key space (every `u32`, so the radar-only
//! tracks keyed from 0x4000_0000 are in some range) is refused when it is finished, so no flight
//! is counted twice or dropped.
//!
//! The report of a run in ranges is byte for byte the report of a run in one piece, whatever
//! order the ranges are added in: case lists are sorted by [`crate::stats::CaseKey`] before they
//! are cut, sums are exactly rounded ([`crate::stats::ExactSum`]), and the per-flight
//! association is put together in range order.

use std::collections::BTreeMap;

use crate::assoc::{associate, astatus, index_truth, Assoc, TruthIndex};
use crate::legacy::{new_legacy, plan_index, Legacy, NONE, NO_ALT, NO_TIME};
use crate::metrics_event::{f1_f2_tbfm, f3_tfms, h1_handoffs, r1_route_conformance, EventCtx};
use crate::metrics_flow::{
    presence_add, presence_parts, presence_results, t9_keyed, PresenceParts, MAX_INTERVAL_S,
};
use crate::metrics_th::{
    t1_cross_track, t2_along_corrected, t2_along_raw, t3_heading, t3_speed, t4_altitude,
};
use crate::pair::{build_pairs, PairTable};
use crate::run::{
    assoc_result, association_case_lines, flagged_assoc, flagged_metrics, flagged_views, RunConfig,
    RunOutput,
};
use crate::stats::{
    band, new_result, sort_cases, Exclusions, KeyedCase, MetricResult, Row, Stats, COAST_NAMES,
};
use crate::thindex::{build_th_index, samples_of};
use crate::truth::{Env, Truth};

/// Why a chunk or the merge was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkError(pub String);

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ChunkError {}

/// A multiset of whole-millisecond values with exact sums, for statistics in seconds. Cold
/// record (A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MsHist {
    pub n: u64,
    pub sum_ms: i128,
    pub sumsq_ms: u128,
    /// Absolute value in ms to count.
    pub by_abs: BTreeMap<u64, u64>,
}

impl MsHist {
    /// Counts one value.
    pub fn add(&mut self, ms: i64) {
        self.n += 1;
        self.sum_ms += i128::from(ms);
        self.sumsq_ms += (i128::from(ms) * i128::from(ms)) as u128;
        *self.by_abs.entry(ms.unsigned_abs()).or_insert(0) += 1;
    }

    /// Adds `o` to this histogram.
    pub fn merge(&mut self, o: &MsHist) {
        self.n += o.n;
        self.sum_ms += o.sum_ms;
        self.sumsq_ms += o.sumsq_ms;
        for (k, c) in &o.by_abs {
            *self.by_abs.entry(*k).or_insert(0) += c;
        }
    }

    /// Nearest-rank percentile of the absolute values, seconds.
    fn rank(&self, p: u64) -> f64 {
        let k = (p * self.n).div_ceil(100).clamp(1, self.n.max(1));
        let mut seen = 0u64;
        for (v, c) in &self.by_abs {
            seen += c;
            if seen >= k {
                return *v as f64 / 1000.0;
            }
        }
        0.0
    }

    /// The statistics in seconds, as `stats_of` gives them for the same values.
    pub fn stats(&self) -> Stats {
        if self.n == 0 {
            return Stats::default();
        }
        let n = self.n as f64;
        Stats {
            n: self.n,
            mean: self.sum_ms as f64 / 1000.0 / n,
            rms: (self.sumsq_ms as f64 / 1.0e6 / n).sqrt(),
            median: self.rank(50),
            p95: self.rank(95),
            p99: self.rank(99),
            max: self
                .by_abs
                .keys()
                .next_back()
                .map_or(0.0, |&v| v as f64 / 1000.0),
        }
    }
}

/// T5's values split as `group_rows` splits them: all, by altitude band, by coast flag.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct T5Hists {
    pub all: MsHist,
    pub band: [MsHist; band::COUNT],
    pub coast: [MsHist; 2],
    pub repeats: u64,
    pub long: u64,
}

pub fn t5_add(h: &mut T5Hists, leg: &Legacy, ix: &crate::thindex::ThIndex) {
    for f in 0..leg.flights.gufi.len() {
        let s = samples_of(ix, f as u32);
        for w in s.windows(2) {
            let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else {
                continue;
            };
            let (ta, tb) = (
                leg.th.pos_ms.get(a as usize).copied().unwrap_or(0),
                leg.th.pos_ms.get(b as usize).copied().unwrap_or(0),
            );
            if tb == ta {
                h.repeats += 1;
                continue;
            }
            if (tb - ta) as f64 / 1000.0 > MAX_INTERVAL_S {
                h.long += 1;
                continue;
            }
            let dt = tb - ta;
            let coast = usize::from(leg.th.coast.get(b as usize).copied().unwrap_or(0));
            let bd = usize::from(band::of(
                leg.th
                    .alt_ft
                    .get(b as usize)
                    .copied()
                    .filter(|&a| a != NO_ALT),
            ));
            h.all.add(dt);
            if let Some(x) = h.band.get_mut(bd) {
                x.add(dt);
            }
            if let Some(x) = h.coast.get_mut(coast) {
                x.add(dt);
            }
        }
    }
}

pub fn t5_result(h: &T5Hists) -> MetricResult {
    let mut r = new_result("T5", "publication interval per flight", "s");
    let mut rows = vec![Row {
        label: "all".to_string(),
        stats: h.all.stats(),
    }];
    for (name, x) in band::NAMES.iter().zip(h.band.iter()) {
        if x.n > 0 {
            rows.push(Row {
                label: format!("altitude={name}"),
                stats: x.stats(),
            });
        }
    }
    for (name, x) in COAST_NAMES.iter().zip(h.coast.iter()) {
        if x.n > 0 {
            rows.push(Row {
                label: format!("coast={name}"),
                stats: x.stats(),
            });
        }
    }
    r.rows = rows;
    r.scalars
        .push(("repeated_position_times".into(), h.repeats as f64));
    r.scalars
        .push(("intervals_over_limit_excluded".into(), h.long as f64));
    r.notes.push(format!(
        "intervals above {MAX_INTERVAL_S} s are excluded and counted"
    ));
    r
}

/// What one address range gave. Cold record (A7.2).
// A7.2: cold struct, size guard waived
pub struct ChunkOut {
    pub pairs: PairTable,
    /// Pairs of the flagged flights (the flagged tables).
    pub flagged_pairs: PairTable,
    pub exclusions: Exclusions,
    pub assoc_counts: [u64; astatus::COUNT],
    /// Flights per [`astatus::REASON_NAMES`] entry.
    pub flag_reasons: [u64; astatus::REASON_NAMES.len()],
    pub unchecked: u64,
    /// The address range the chunk was scored for, inclusive.
    pub range: (u32, u32),
    /// The association of the chunk's flights (one entry per flight).
    pub assoc: Assoc,
    /// Flights keyed or associated with an address outside `range` (refused by [`chunk_add`]).
    pub outside: u64,
    /// The association's case lines with their places in the list, at most [`MAX_CASES`].
    pub assoc_cases: Vec<KeyedCase>,
    /// T9's case lines with their places in the list.
    pub t9_cases: Vec<KeyedCase>,
    pub t5: T5Hists,
    pub t6_none: u64,
    pub t6_with: u64,
    pub presence: PresenceParts,
    pub t9: MetricResult,
    /// F1, F2, F3 arrival, F3 departure, R1, H1.
    pub flows: Vec<MetricResult>,
    /// The same six results for a table with no flights at all: what "no plan data" looks like,
    /// NaN and all. [`chunk_add`] refuses a chunk whose results are not exactly these.
    pub flows_no_data: Vec<MetricResult>,
}

/// Whether two results are the same, a NaN equal to a NaN (every float bit for bit).
fn same_result(a: &MetricResult, b: &MetricResult) -> bool {
    let bits = |x: f64, y: f64| x.to_bits() == y.to_bits();
    a.id == b.id
        && a.name == b.name
        && a.unit == b.unit
        && a.rows == b.rows
        && a.withheld == b.withheld
        && a.cases == b.cases
        && a.notes == b.notes
        && a.scalars.len() == b.scalars.len()
        && a.scalars
            .iter()
            .zip(&b.scalars)
            .all(|(x, y)| x.0 == y.0 && bits(x.1, y.1))
}

/// The six flow results of a table with no flights, for `ctx`'s truth and settings.
fn flows_of_nothing(
    truth: &Truth<'_>,
    tix: &TruthIndex,
    env: &Env<'_>,
    cfg: &RunConfig,
) -> Vec<MetricResult> {
    let leg = new_legacy();
    let assoc = Assoc::default();
    let plan = plan_index(&leg);
    let ctx = EventCtx {
        assoc: &assoc,
        truth,
        tix,
        env,
        cfg: &cfg.event,
    };
    let mut ex = Exclusions::new();
    let (f1, f2) = f1_f2_tbfm(&leg, &plan, &ctx, &mut ex);
    let (f3a, f3d) = f3_tfms(&leg, &plan, &ctx, &mut ex);
    let r1 = r1_route_conformance(&leg, &PairTable::default(), truth, env, &cfg.event, &mut ex);
    let h1 = h1_handoffs(&leg, &assoc, truth, env, &cfg.event, &mut ex);
    vec![f1, f2, f3a, f3d, r1, h1]
}

/// Scores the rows of `leg` (the flights of the address range `range`, inclusive) with the
/// benchmark's own functions. `tix` is `index_truth(truth.legs)`.
pub fn benchmark_chunk(
    leg: &Legacy,
    truth: &Truth<'_>,
    tix: &TruthIndex,
    env: &Env<'_>,
    cfg: &RunConfig,
    range: (u32, u32),
) -> ChunkOut {
    let ix = build_th_index(&leg.th, leg.flights.gufi.len());
    let assoc = associate(leg, &ix, truth, tix, &cfg.assoc);
    let (pairs, mut ex) = build_pairs(leg, &ix, &assoc, truth, &cfg.pair);
    let flagged = flagged_assoc(&assoc);
    let (flagged_pairs, _) = build_pairs(leg, &ix, &flagged, truth, &cfg.pair);
    let mut flag_reasons = [0u64; astatus::REASON_NAMES.len()];
    for &r in &assoc.flag_reason {
        if let Some(c) = flag_reasons.get_mut(usize::from(r)) {
            *c += 1;
        }
    }
    let mut assoc_cases = association_case_lines(leg, &ix, &assoc);
    sort_cases(&mut assoc_cases);
    let in_range = |a: u32| a == NONE || (a >= range.0 && a <= range.1);
    let outside = (0..leg.flights.gufi.len())
        .filter(|&f| {
            [
                leg.flights.addr.get(f),
                assoc.addr.get(f),
                assoc.flag_addr.get(f),
            ]
            .iter()
            .any(|a| a.is_some_and(|&a| !in_range(a)))
        })
        .count() as u64;
    let plan = plan_index(leg);
    let ctx = EventCtx {
        assoc: &assoc,
        truth,
        tix,
        env,
        cfg: &cfg.event,
    };
    let (t9, t9_cases) = t9_keyed(leg, &pairs, truth, &cfg.swap);
    let presence = presence_parts(leg, &ix, &assoc, truth, env, &cfg.presence, range);
    let (f1, f2) = f1_f2_tbfm(leg, &plan, &ctx, &mut ex);
    let (f3a, f3d) = f3_tfms(leg, &plan, &ctx, &mut ex);
    let r1 = r1_route_conformance(leg, &pairs, truth, env, &cfg.event, &mut ex);
    let h1 = h1_handoffs(leg, &assoc, truth, env, &cfg.event, &mut ex);
    let mut t5 = T5Hists::default();
    t5_add(&mut t5, leg, &ix);
    let with = leg.th.recv_ms.iter().filter(|&&r| r != NO_TIME).count() as u64;
    ChunkOut {
        pairs,
        flagged_pairs,
        exclusions: ex,
        assoc_counts: assoc.counts,
        flag_reasons,
        unchecked: assoc.unchecked,
        range,
        assoc,
        outside,
        assoc_cases,
        t9_cases,
        t5,
        t6_none: leg.th.pos_ms.len() as u64 - with,
        t6_with: with,
        presence,
        t9,
        flows: vec![f1, f2, f3a, f3d, r1, h1],
        flows_no_data: flows_of_nothing(truth, tix, env, cfg),
    }
}

/// The sums of the chunks so far.
// A7.2: cold struct, size guard waived
#[derive(Default)]
pub struct ChunkAcc {
    pairs: PairTable,
    flagged_pairs: PairTable,
    flag_reasons: [u64; astatus::REASON_NAMES.len()],
    unchecked: u64,
    assoc_cases: Vec<KeyedCase>,
    t9_cases: Vec<KeyedCase>,
    exclusions: Exclusions,
    assoc_counts: [u64; astatus::COUNT],
    /// The range and association of each chunk, put together in range order at the end.
    assocs: Vec<((u32, u32), Assoc)>,
    t5: T5Hists,
    t6_none: u64,
    presence: PresenceParts,
    t9: Option<MetricResult>,
    flows: Option<Vec<MetricResult>>,
}

/// Appends the rows of `b` to `a`.
fn pairs_extend(a: &mut PairTable, b: &PairTable) {
    a.addr.extend_from_slice(&b.addr);
    a.t_ms.extend_from_slice(&b.t_ms);
    a.cross_m.extend_from_slice(&b.cross_m);
    a.along_m.extend_from_slice(&b.along_m);
    a.speed_err_kt.extend_from_slice(&b.speed_err_kt);
    a.hdg_err_deg.extend_from_slice(&b.hdg_err_deg);
    a.alt_err_ft.extend_from_slice(&b.alt_err_ft);
    a.truth_speed_mps.extend_from_slice(&b.truth_speed_mps);
    a.method.extend_from_slice(&b.method);
    a.phase.extend_from_slice(&b.phase);
    a.band.extend_from_slice(&b.band);
    a.coast.extend_from_slice(&b.coast);
}

/// Adds one chunk to the sums. A chunk is refused when its range is empty or overlaps a range
/// already added, when one of its flights is keyed or associated outside its range, when it has
/// receive times, or when a plan-data result is not exactly what no data gives.
pub fn chunk_add(acc: &mut ChunkAcc, c: ChunkOut) -> Result<(), ChunkError> {
    let (lo, hi) = c.range;
    if lo > hi {
        return Err(ChunkError(format!(
            "the address range {lo:#x}..={hi:#x} is empty"
        )));
    }
    if let Some((a, b)) = acc
        .assocs
        .iter()
        .map(|(r, _)| *r)
        .find(|&(a, b)| lo <= b && a <= hi)
    {
        return Err(ChunkError(format!(
            "the address range {lo:#x}..={hi:#x} overlaps {a:#x}..={b:#x}, which was already scored: its flights and legs would be counted twice"
        )));
    }
    if c.outside > 0 {
        return Err(ChunkError(format!(
            "{} flight(s) of the chunk are keyed or associated with an address outside its range {lo:#x}..={hi:#x}",
            c.outside
        )));
    }
    if c.t6_with > 0 {
        return Err(ChunkError(
            "chunked scoring is for a tracker's output, which has no receive time".to_string(),
        ));
    }
    if c.flows.len() != c.flows_no_data.len() {
        return Err(ChunkError(
            "the plan-data results are incomplete".to_string(),
        ));
    }
    if let Some(r) = c
        .flows
        .iter()
        .zip(&c.flows_no_data)
        .find(|(r, none)| !same_result(r, none))
    {
        return Err(ChunkError(format!(
            "chunked scoring is for a tracker's output, which has no plan data, but {} is not what no data gives",
            r.0.id
        )));
    }
    pairs_extend(&mut acc.pairs, &c.pairs);
    pairs_extend(&mut acc.flagged_pairs, &c.flagged_pairs);
    for (k, v) in c.exclusions {
        *acc.exclusions.entry(k).or_insert(0) += v;
    }
    for (a, b) in acc.assoc_counts.iter_mut().zip(c.assoc_counts.iter()) {
        *a += b;
    }
    for (a, b) in acc.flag_reasons.iter_mut().zip(c.flag_reasons.iter()) {
        *a += b;
    }
    acc.unchecked += c.unchecked;
    acc.assoc_cases.extend(c.assoc_cases);
    sort_cases(&mut acc.assoc_cases);
    acc.t9_cases.extend(c.t9_cases);
    sort_cases(&mut acc.t9_cases);
    acc.assocs.push((c.range, c.assoc));
    acc.t5.all.merge(&c.t5.all);
    for (a, b) in acc.t5.band.iter_mut().zip(c.t5.band.iter()) {
        a.merge(b);
    }
    for (a, b) in acc.t5.coast.iter_mut().zip(c.t5.coast.iter()) {
        a.merge(b);
    }
    acc.t5.repeats += c.t5.repeats;
    acc.t5.long += c.t5.long;
    acc.t6_none += c.t6_none;
    presence_add(&mut acc.presence, c.presence);
    match &mut acc.t9 {
        None => acc.t9 = Some(c.t9),
        Some(t) => {
            for (a, b) in t.scalars.iter_mut().zip(c.t9.scalars.iter()) {
                a.1 += b.1;
            }
        }
    }
    if acc.flows.is_none() {
        acc.flows = Some(c.flows);
    }
    Ok(())
}

/// Puts the per-flight associations of the chunks together in range order.
fn assoc_of(
    mut parts: Vec<((u32, u32), Assoc)>,
    counts: [u64; astatus::COUNT],
    unchecked: u64,
) -> Assoc {
    parts.sort_by_key(|p| p.0);
    let mut a = Assoc {
        counts,
        unchecked,
        ..Assoc::default()
    };
    for (_, p) in parts {
        a.addr.extend(p.addr);
        a.method.extend(p.method);
        a.status.extend(p.status);
        a.flag_addr.extend(p.flag_addr);
        a.flag_method.extend(p.flag_method);
        a.flag_reason.extend(p.flag_reason);
        a.gate_share.extend(p.gate_share);
        a.shared.extend(p.shared);
    }
    a
}

/// The finished benchmark and the paired samples it was made from (for the ESASSP-style split).
/// `presence` is the T7 and T8 configuration of `cfg`. A run with no chunk is refused, and so is
/// one whose ranges do not cover every key from 0 to `u32::MAX` (a gap would drop the flights
/// keyed in it). `RunOutput::assoc` is the chunks' associations one after the other in range
/// order: its flight `i` is the `i`-th flight of the run in that order, not a flight of any one
/// table.
pub fn chunk_finish(acc: ChunkAcc, cfg: &RunConfig) -> Result<(RunOutput, PairTable), ChunkError> {
    let (Some(mut t9), Some(flows)) = (acc.t9, acc.flows) else {
        return Err(ChunkError("no chunk was scored".to_string()));
    };
    let mut ranges: Vec<(u32, u32)> = acc.assocs.iter().map(|(r, _)| *r).collect();
    ranges.sort_unstable();
    let mut next = Some(0u32);
    for &(lo, hi) in &ranges {
        if next != Some(lo) {
            return Err(ChunkError(format!(
                "the address ranges do not cover the whole key space: a gap before {lo:#x} (every key from 0 to 0xffffffff must be in some range, the radar-only tracks keyed from 0x40000000 included)"
            )));
        }
        next = hi.checked_add(1);
    }
    if next.is_some() {
        return Err(ChunkError(format!(
            "the address ranges do not cover the whole key space: nothing from {:#x} to 0xffffffff",
            next.unwrap_or(0)
        )));
    }
    t9.cases = acc.t9_cases.into_iter().map(|c| c.1).collect();
    let pairs = acc.pairs;
    let mut m = vec![
        t1_cross_track(&pairs),
        t2_along_raw(&pairs),
        t2_along_corrected(&pairs, &cfg.offset),
        t3_speed(&pairs),
        t3_heading(&pairs),
        t4_altitude(&pairs),
        t5_result(&acc.t5),
    ];
    flagged_views(&mut m, &pairs, &acc.flagged_pairs, &acc.assoc_counts);
    let mut t6 = new_result("T6", "staleness (receive time minus position time)", "s");
    t6.withheld =
        Some("the recordings carry no receive time (raw .msg files); use .oaf recordings".into());
    t6.scalars
        .push(("samples_without_receive_time".into(), acc.t6_none as f64));
    m.push(t6);
    let (t7, t8) = presence_results(acc.presence, &cfg.presence);
    m.push(t7);
    m.push(t8);
    m.push(t9);
    m.extend(flows);
    m.extend(flagged_metrics(&acc.flagged_pairs));
    let mut a = assoc_result(
        &acc.assoc_counts,
        &acc.flag_reasons,
        acc.unchecked,
        &cfg.assoc,
    );
    a.cases = acc.assoc_cases.into_iter().map(|c| c.1).collect();
    m.push(a);
    let n_pairs = pairs.t_ms.len();
    Ok((
        RunOutput {
            metrics: m,
            exclusions: acc.exclusions,
            assoc: assoc_of(acc.assocs, acc.assoc_counts, acc.unchecked),
            pairs: n_pairs,
        },
        pairs,
    ))
}

/// `index_truth`, so a caller needs only this module.
pub fn truth_index(truth: &Truth<'_>) -> TruthIndex {
    index_truth(truth.legs)
}

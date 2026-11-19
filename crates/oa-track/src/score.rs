//! The scoring path (`docs/M3_DESIGN.md` section 2): split reports, run the tracker on one half,
//! build truth from the other, score the tracker's one-second output with `oa-bench`'s metric
//! functions. Core (pure) except where noted; the clock and the files are the binary's.
//!
//! # The split, defined exactly
//!
//! For each aircraft (the 24-bit address with `oa_adsb::NON_ICAO`, the same key as `oa-truth`),
//! its decoded reports are put in the total order (time, input index). Copies are first removed:
//! a report with the address, time, position, altitudes, velocity and quality fields of one
//! already kept is the same observation (counted in [`SplitStats::duplicates_removed`]). The
//! distinct observations are then grouped by (address, time) and the groups numbered from 0; a
//! group of reports with different content at one time (a conflict, counted) stays whole. The
//! even-numbered groups are **set A** (the tracker's input) and the odd-numbered ones **set B**
//! (the truth's input). The two sets share no observation, together they are every distinct
//! decoded report, and
//! the quality fields (NACp, NIC, SIL, version, NACv) are carried forward over the whole
//! aircraft sequence before the split, by the same 60 s rule and code as the truth gates
//! (`oa_truth::quality_check`), and written into the rows of both sets. readsb prints them on a
//! minority of rows, so without this half the aircraft would have none in age after the split;
//! they describe the aircraft's navigation state, not a measurement, so sharing them does not
//! share any position noise. GNSS errors that are correlated over seconds are shared
//! between the sets. The score measures disagreement with a correlated reference, not a bound
//! on the tracker's absolute error.
//!
//! # What is scored
//!
//! The tracker's fixed one-second rows (confirmed tracks only, coasting included and flagged)
//! become the "legacy" stream of `oa-bench` through [`oa_bench::tracker_input`], and
//! `run_benchmark` runs unchanged: T1 cross-track, T2 along-track, T3 speed and heading, T4
//! altitude, T5 interval, T7 presence. T5 of a one-second output is one second by construction.
//! The zone of T7 and T8 is oa-adsb's ZAU box (no airspace model is loaded), stated in the
//! report.
//!
//! The ESASSP-style position and speed RMS split into straight and turning follow ESASSP
//! Volume 1 section 4.2.14 (read 2026-11-14): a turn is a stretch where the reconstructed
//! trajectory's transversal acceleration exceeds 1.5 m/s^2, here the truth speed times the
//! truth heading rate over [`TURN_SPAN_MS`] around the sample. A sample whose truth cannot be
//! asked on both sides of that span is counted as unclassified. These numbers are on the
//! thinned input and are not an ESASSP exit result.

use std::collections::BTreeMap;
use std::path::Path;

use oa_adsb::{reports_len, reports_push, reports_row, Reports, NONE_U8, ZAU_BOX};
use oa_bench::adsb_truth::{self, BuiltTruth};
use oa_bench::chunked::{benchmark_chunk, chunk_add, chunk_finish, truth_index, ChunkAcc};
use oa_bench::legacy::NO_ALT;
use oa_bench::pair::PairTable;
use oa_bench::run::{RunConfig, RunOutput};
use oa_bench::split_check::observation_key;
use oa_bench::stats::{stats_of, Stats};
use oa_bench::tracker_input::{legacy_from_track_samples_by_leg, TrackSamples};
use oa_bench::truth::{Env, Truth};
use oa_sha256::{to_hex, Sha256};
use oa_truth::{quality_check, QualityState, TruthConfig, DEFAULT_CONFIG};

use crate::params::NS_PER_S;
use crate::spill::{spill_append, spill_counts, spill_finish, spill_new, spill_rows, SpilledRun};
use crate::tracker::{
    tracker_advance, tracker_flush, tracker_log, tracker_new, tracker_stats, tracker_update,
};
use crate::types::{
    output_clear, output_hash_into, output_len, output_new, Measurement, TrackError, TrackLog,
    TrackOutput, TrackerConfig, TrackerStats, O_COAST, O_HAS_ALT, O_TICK, O_UNCERTAIN,
};

/// The split, in words, for every report.
pub const SPLIT_TEXT: &str = "per aircraft, copies of one observation removed, then distinct (time, input index) groups numbered from 0: even to set A (tracker), odd to set B (truth); reports of different content at one time stay on one side";
/// The basis every scored number carries.
pub const BASIS_TEXT: &str = "on ADS-B input split into disjoint report groups; not an ESASSP exit result; GNSS error correlated over seconds is shared between the sets; disagreement does not bound absolute error";
/// A track is dropped after this long without an update in the scoring run, ns: the gap at which
/// `oa-truth` starts a new leg (`oa_truth::LEG_GAP_MS`), so tracker and truth end an aircraft's
/// pass at the same silence.
pub const SCORE_DROP_AFTER_NS: i64 = oa_truth::LEG_GAP_MS * 1_000_000;
/// The scoring run gives the tracker an output table this large (rows) and drains it often, so
/// no row is lost: 1 Mi rows.
pub const SCORE_OUT_CAPACITY: usize = 1 << 20;
/// Drain the output table into the accumulator when it holds more than this many rows.
pub const DRAIN_ROWS: usize = 1 << 16;
/// Longest gap between two reports counted in the measured interval distribution, ns.
const MAX_INTERVAL_NS: i64 = 120 * NS_PER_S;
/// Turn classification: half-width of the span over which the truth heading rate is measured, ms.
pub const TURN_SPAN_MS: i64 = 3_000;
/// ESASSP section 4.2.14: transversal acceleration above this is a turn, m/s^2.
pub const TURN_ACCEL_MPS2: f64 = 1.5;
/// ESASSP R4: per-flight position RMS the Recommended 3 NM column must stay under, metres.
pub const R4_PER_FLIGHT_M: f64 = 230.0;
/// A flight needs at least this many paired samples to count in the per-flight position RMS.
pub const PER_FLIGHT_MIN_N: usize = 10;
const MPS_PER_KT: f64 = 0.514_444_444_444;

/// What the split did. Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SplitStats {
    pub rows_in: u64,
    pub aircraft: u64,
    pub set_a: u64,
    pub set_b: u64,
    /// Copies removed: a report with the address, time and content of one already kept.
    pub duplicates_removed: u64,
    /// (address, time) groups holding reports of different content; all of one group are kept
    /// on one side.
    pub conflict_groups: u64,
    /// Reports in those groups.
    pub conflict_rows: u64,
    /// Rows that could not be read back and were left out.
    pub rows_invalid: u64,
}

/// Splits `r` into the two disjoint sets (see the module documentation). Rows keep every field.
pub fn split_reports(r: &Reports) -> (Reports, Reports, SplitStats) {
    let n = reports_len(r);
    let mut order: Vec<(u32, i64, u32)> = Vec::with_capacity(n);
    for (i, (&a, &t)) in r.icao.iter().zip(r.time_ns.iter()).enumerate() {
        order.push((a, t, i as u32));
    }
    order.sort_unstable();
    let (mut a, mut b) = (Reports::default(), Reports::default());
    let mut st = SplitStats {
        rows_in: n as u64,
        ..SplitStats::default()
    };
    let mut cur = None;
    let mut k = 0u64;
    let mut q = QualityState::default();
    let mut g = 0;
    while let Some(&(addr, t, _)) = order.get(g) {
        let end = order.get(g..).map_or(g, |s| {
            g + s.iter().take_while(|x| x.0 == addr && x.1 == t).count()
        });
        if cur != Some(addr) {
            cur = Some(addr);
            k = 0;
            q = QualityState::default();
            st.aircraft += 1;
        }
        // The distinct observations of this (address, time), in input order.
        let mut group: Vec<oa_adsb::Row> = Vec::new();
        for &(_, _, i) in order.get(g..end).unwrap_or(&[]) {
            let Some(row) = reports_row(r, i as usize) else {
                st.rows_invalid += 1;
                continue;
            };
            if group
                .iter()
                .any(|x| observation_key(x) == observation_key(&row))
            {
                st.duplicates_removed += 1;
            } else {
                group.push(row);
            }
        }
        g = end;
        // Siblings go through quality handling in one fixed order, not the input order.
        group.sort_by_key(observation_key);
        if group.len() > 1 {
            st.conflict_groups += 1;
            st.conflict_rows += group.len() as u64;
        }
        if group.is_empty() {
            continue;
        }
        // The whole group goes to one side: no physical observation is in both sets.
        let to_a = k.is_multiple_of(2);
        for mut row in group {
            let v = quality_check(&mut q, &row, t, &DEFAULT_CONFIG);
            row.nac_p = v.nacp.unwrap_or(NONE_U8);
            row.nic = v.nic.unwrap_or(NONE_U8);
            row.sil = v.sil.unwrap_or(NONE_U8);
            row.version = v.version.unwrap_or(NONE_U8);
            row.nac_v = v.nacv.unwrap_or(NONE_U8);
            if to_a {
                reports_push(&mut a, &row);
                st.set_a += 1;
            } else {
                reports_push(&mut b, &row);
                st.set_b += 1;
            }
        }
        k += 1;
    }
    (a, b, st)
}

/// Distribution of the intervals between consecutive measurements of one aircraft (gaps over
/// two minutes excluded). Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntervalStats {
    pub n: u64,
    pub p50_ns: i64,
    pub p90_ns: i64,
    pub p99_ns: i64,
}

/// The measured interval distribution of `meas` (sorted by aircraft internally); `None` if no
/// aircraft has two measurements within two minutes.
pub fn interval_stats(meas: &[Measurement]) -> Option<IntervalStats> {
    let mut by: Vec<(u32, i64)> = meas.iter().map(|m| (m.addr, m.toa_ns)).collect();
    by.sort_unstable();
    let mut d: Vec<i64> = by
        .windows(2)
        .filter_map(|w| match (w.first(), w.get(1)) {
            (Some(a), Some(b)) if a.0 == b.0 => Some(b.1.saturating_sub(a.1)),
            _ => None,
        })
        .filter(|&x| x > 0 && x <= MAX_INTERVAL_NS)
        .collect();
    if d.is_empty() {
        return None;
    }
    d.sort_unstable();
    let at = |p: usize| {
        let k = (p * d.len()).div_ceil(100).clamp(1, d.len());
        d.get(k - 1).copied().unwrap_or(0)
    };
    Some(IntervalStats {
        n: d.len() as u64,
        p50_ns: at(50),
        p90_ns: at(90),
        p99_ns: at(99),
    })
}

/// The tracker configuration of a scoring run: the base with the expected interval set from the
/// input's measured median (what coasting is judged against), the initiation interval from its
/// 90th percentile (so the slow tail can be confirmed), the drop time of
/// [`SCORE_DROP_AFTER_NS`] and a large output table.
pub fn scoring_config(base: &TrackerConfig, iv: &IntervalStats) -> TrackerConfig {
    TrackerConfig {
        expected_interval_ns: iv.p50_ns.max(NS_PER_S),
        init_interval_ns: iv.p90_ns.max(NS_PER_S),
        drop_after_ns: SCORE_DROP_AFTER_NS,
        out_capacity: SCORE_OUT_CAPACITY,
        ..*base
    }
}

/// A finished tracker run. A7.2: cold struct.
pub struct TrackRun {
    /// Every row the tracker produced, accumulated (unbounded; the tracker's own table was
    /// drained into it).
    pub out: TrackOutput,
    pub stats: TrackerStats,
    pub log: TrackLog,
    pub offered: u64,
    /// Digest of the accumulated output.
    pub digest: String,
    /// Rows the tracker's bounded table lost (must be 0 in a scored run).
    pub overflow: u64,
}

fn append(dst: &mut TrackOutput, src: &TrackOutput) {
    dst.t_ns.extend_from_slice(&src.t_ns);
    dst.addr.extend_from_slice(&src.addr);
    dst.lat_e7.extend_from_slice(&src.lat_e7);
    dst.lon_e7.extend_from_slice(&src.lon_e7);
    dst.vel_east_mps.extend_from_slice(&src.vel_east_mps);
    dst.vel_north_mps.extend_from_slice(&src.vel_north_mps);
    dst.pos_cov_m2.extend_from_slice(&src.pos_cov_m2);
    dst.vel_cov_m2s2.extend_from_slice(&src.vel_cov_m2s2);
    dst.turn_rate_rps.extend_from_slice(&src.turn_rate_rps);
    dst.alt_ft.extend_from_slice(&src.alt_ft);
    dst.alt_var_ft2.extend_from_slice(&src.alt_var_ft2);
    dst.vrate_fpm.extend_from_slice(&src.vrate_fpm);
    dst.mu_h.extend_from_slice(&src.mu_h);
    dst.mu_v.extend_from_slice(&src.mu_v);
    dst.flags.extend_from_slice(&src.flags);
}

/// Runs the tracker over measurements already in time order, draining its output into the run.
/// Pure: the same measurements and configuration give the same run.
pub fn run_tracker(meas: &[Measurement], cfg: &TrackerConfig) -> Result<TrackRun, TrackError> {
    let mut t = tracker_new(cfg)?;
    let mut scratch = output_new(cfg.out_capacity);
    let mut acc = TrackOutput {
        capacity: usize::MAX,
        ..TrackOutput::default()
    };
    for m in meas {
        tracker_update(&mut t, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            append(&mut acc, &scratch);
            output_clear(&mut scratch);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut scratch);
    tracker_flush(&mut t, end, &mut scratch);
    append(&mut acc, &scratch);
    let overflow = scratch.overflow;
    let mut h = Sha256::new();
    output_hash_into(&mut h, &acc);
    Ok(TrackRun {
        out: acc,
        stats: *tracker_stats(&t),
        log: tracker_log(&t).clone(),
        offered: meas.len() as u64,
        digest: to_hex(&h.finalize()),
        overflow,
    })
}

/// Initiation delays in seconds (first report to confirmation) of the confirmed tracks, and the
/// number of tracks that never were.
pub fn initiation_delays(log: &TrackLog) -> (Vec<f64>, u64) {
    let mut d = Vec::new();
    let mut never = 0;
    for (f, c) in log.first_ns.iter().zip(log.confirmed_ns.iter()) {
        if *c == i64::MIN {
            never += 1;
        } else {
            d.push((c - f) as f64 / 1.0e9);
        }
    }
    (d, never)
}

/// Heading in radians clockwise from north.
fn heading(e: f64, n: f64) -> f64 {
    oa_math::atan2(e, n)
}

/// Transversal acceleration, m/s^2, from two truth velocities `span_s` apart: the speed times the
/// heading change over the span (a turn rate in rad/s).
pub fn transversal_accel(v1: (f64, f64), v2: (f64, f64), span_s: f64) -> f64 {
    let mut dh = heading(v2.0, v2.1) - heading(v1.0, v1.1);
    while dh > std::f64::consts::PI {
        dh -= 2.0 * std::f64::consts::PI;
    }
    while dh < -std::f64::consts::PI {
        dh += 2.0 * std::f64::consts::PI;
    }
    let speed = (oa_math::hypot(v1.0, v1.1) + oa_math::hypot(v2.0, v2.1)) / 2.0;
    speed * dh.abs() / span_s
}

/// Index of the all, straight and turning classes in [`Esassp`].
pub const CLASS_ALL: usize = 0;
pub const CLASS_STRAIGHT: usize = 1;
pub const CLASS_TURN: usize = 2;
/// Class names.
pub const CLASS_NAMES: [&str; 3] = ["all", "straight", "turning"];

/// Position, speed and heading error statistics split straight and turning. A7.2: cold struct.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Esassp {
    /// Position error (horizontal, metres) by class.
    pub position_m: [Stats; 3],
    /// Speed error, m/s, by class.
    pub speed_mps: [Stats; 3],
    /// Heading error, degrees, by class.
    pub heading_deg: [Stats; 3],
    /// Samples whose truth could not be asked on both sides of the turn span.
    pub unclassified: u64,
    /// Coasting samples, left out of this table.
    pub coasting: u64,
    /// Samples with no cross/along (truth nearly stationary), left out of position.
    pub no_position: u64,
    /// Flights with at least [`PER_FLIGHT_MIN_N`] position samples, and those whose position RMS
    /// exceeds [`R4_PER_FLIGHT_M`]; the largest per-flight RMS.
    pub flights: u64,
    pub flights_over_r4: u64,
    pub flight_rms_max_m: f64,
    /// 95th percentile (nearest rank) of the per-flight position RMS.
    pub flight_rms_p95_m: f64,
}

/// Per-truth-leg errors from the exact paired rows used by the ESASSP-style summary.
/// A7.2: cold diagnostic output; this is not tracker state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlightErrorSummary {
    pub address: u32,
    pub truth_leg_index: u32,
    pub leg_start_ms: i64,
    pub leg_end_ms: i64,
    pub paired_samples: u64,
    pub noncoast_samples: u64,
    pub position_samples: u64,
    pub position_rms_m: Option<f64>,
    pub position_max_m: Option<f64>,
    pub speed_samples: u64,
    pub speed_rms_mps: Option<f64>,
    pub heading_samples: u64,
    pub heading_rms_deg: Option<f64>,
    pub window_start_ms: Option<i64>,
    pub window_end_ms: Option<i64>,
}

/// Builds address/truth-leg summaries from the same paired errors used by `esassp_of`.
/// Every truth leg is retained, including legs with no paired rows or too few position samples.
pub fn flight_error_summaries(
    p: &PairTable,
    truth: &oa_truth::Truth,
    legs: &[oa_bench::truth::TruthLeg],
) -> Result<Vec<FlightErrorSummary>, ScoreError> {
    #[derive(Default)]
    struct Acc {
        paired: u64,
        noncoast: u64,
        pos_n: u64,
        pos_sq: f64,
        pos_max: Option<f64>,
        speed_n: u64,
        speed_sq: f64,
        heading_n: u64,
        heading_sq: f64,
        start: Option<i64>,
        end: Option<i64>,
    }
    let n = p.t_ms.len();
    for (name, len) in [
        ("addr", p.addr.len()),
        ("coast", p.coast.len()),
        ("cross_m", p.cross_m.len()),
        ("along_m", p.along_m.len()),
        ("speed_err_kt", p.speed_err_kt.len()),
        ("hdg_err_deg", p.hdg_err_deg.len()),
    ] {
        if len != n {
            return Err(ScoreError(format!(
                "flight error summaries: PairTable column {name} has {len} rows; expected {n}"
            )));
        }
    }
    let truth_leg_count = oa_truth::leg_count(truth);
    if legs.len() != truth_leg_count {
        return Err(ScoreError(format!(
            "flight error summaries: adapted truth has {} legs; oa-truth has {truth_leg_count}",
            legs.len()
        )));
    }
    for (i, leg) in legs.iter().enumerate() {
        let ix = u32::try_from(i).map_err(|_| {
            ScoreError("flight error summaries: truth leg index exceeds u32".to_string())
        })?;
        let leg_ix = oa_truth::LegIx(ix);
        let source = (
            oa_truth::leg_addr(truth, leg_ix),
            oa_truth::leg_start_ms(truth, leg_ix),
            oa_truth::leg_end_ms(truth, leg_ix),
        );
        if source != (Some(leg.addr), Some(leg.start_ms), Some(leg.end_ms)) {
            return Err(ScoreError(format!(
                "flight error summaries: adapted truth leg {ix} does not match oa-truth identity/bounds"
            )));
        }
    }

    let mut acc: BTreeMap<(u32, u32), Acc> = BTreeMap::new();
    for (i, leg) in legs.iter().enumerate() {
        let ix = u32::try_from(i).map_err(|_| {
            ScoreError("flight error summaries: truth leg index exceeds u32".to_string())
        })?;
        acc.insert((leg.addr, ix), Acc::default());
    }
    for i in 0..p.t_ms.len() {
        let addr = p.addr.get(i).copied().ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: PairTable address missing at row {i}"
            ))
        })?;
        let t = p.t_ms.get(i).copied().ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: PairTable time missing at row {i}"
            ))
        })?;
        let trip = trip_of(truth, addr, t);
        let leg = legs.get(trip as usize).ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: pair row {i} names unknown truth leg {trip}"
            ))
        })?;
        if leg.addr != addr
            || oa_truth::leg_addr(truth, oa_truth::LegIx(trip)) != Some(addr)
            || !(leg.start_ms..=leg.end_ms).contains(&t)
        {
            return Err(ScoreError(format!(
                "flight error summaries: pair row {i} identity/time does not match truth leg {trip}"
            )));
        }
        let a = acc.get_mut(&(addr, trip)).ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: pair row {i} has no summary slot for address {addr:08X}, leg {trip}"
            ))
        })?;
        a.paired += 1;
        a.start = Some(a.start.map_or(t, |v| v.min(t)));
        a.end = Some(a.end.map_or(t, |v| v.max(t)));
        let coast = p.coast.get(i).copied().ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: PairTable coast flag missing at row {i}"
            ))
        })?;
        if coast != 0 {
            continue;
        }
        a.noncoast += 1;
        let cross = f64::from(p.cross_m.get(i).copied().ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: PairTable cross-track error missing at row {i}"
            ))
        })?);
        let along = f64::from(p.along_m.get(i).copied().ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: PairTable along-track error missing at row {i}"
            ))
        })?);
        let pos = oa_math::hypot(cross, along);
        if pos.is_finite() {
            a.pos_n += 1;
            a.pos_sq += pos * pos;
            a.pos_max = Some(a.pos_max.map_or(pos, |v| v.max(pos)));
        }
        let speed = f64::from(p.speed_err_kt.get(i).copied().ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: PairTable speed error missing at row {i}"
            ))
        })?) * MPS_PER_KT;
        if speed.is_finite() {
            a.speed_n += 1;
            a.speed_sq += speed * speed;
        }
        let heading = f64::from(p.hdg_err_deg.get(i).copied().ok_or_else(|| {
            ScoreError(format!(
                "flight error summaries: PairTable heading error missing at row {i}"
            ))
        })?);
        if heading.is_finite() {
            a.heading_n += 1;
            a.heading_sq += heading * heading;
        }
    }
    acc.into_iter()
        .map(|((address, truth_leg_index), a)| {
            let leg = legs.get(truth_leg_index as usize).ok_or_else(|| {
                ScoreError(format!(
                    "flight error summaries: unknown truth leg {truth_leg_index}"
                ))
            })?;
            Ok(FlightErrorSummary {
                address,
                truth_leg_index,
                leg_start_ms: leg.start_ms,
                leg_end_ms: leg.end_ms,
                paired_samples: a.paired,
                noncoast_samples: a.noncoast,
                position_samples: a.pos_n,
                position_rms_m: (a.pos_n > 0).then(|| (a.pos_sq / a.pos_n as f64).sqrt()),
                position_max_m: a.pos_max,
                speed_samples: a.speed_n,
                speed_rms_mps: (a.speed_n > 0).then(|| (a.speed_sq / a.speed_n as f64).sqrt()),
                heading_samples: a.heading_n,
                heading_rms_deg: (a.heading_n > 0)
                    .then(|| (a.heading_sq / a.heading_n as f64).sqrt()),
                window_start_ms: a.start,
                window_end_ms: a.end,
            })
        })
        .collect()
}

fn push_class(v: &mut [Vec<f64>; 3], class: Option<usize>, x: f64) {
    if let Some(a) = v.get_mut(CLASS_ALL) {
        a.push(x);
    }
    if let Some(c) = class.and_then(|c| v.get_mut(c)) {
        c.push(x);
    }
}

/// The trip a sample belongs to: the index of the truth leg of `addr` with the latest start at
/// or before `t_ms` (the leg `truth_at` answers from), or `u32::MAX` when none.
fn trip_of(truth: &oa_truth::Truth, addr: u32, t_ms: i64) -> u32 {
    let range = oa_truth::legs_of(truth, addr);
    let t_ns = t_ms.saturating_mul(1_000_000);
    let mut k = range.first.0;
    let mut found = u32::MAX;
    while k < range.first.0.saturating_add(range.count) {
        match oa_truth::leg_start_ns(truth, oa_truth::LegIx(k)) {
            Some(s) if s <= t_ns => found = k,
            _ => break,
        }
        k += 1;
    }
    found
}

/// The ESASSP-style split of a paired-sample table (see the module documentation).
pub fn esassp_of(p: &PairTable, truth: &oa_truth::Truth) -> Esassp {
    let mut coasting = 0u64;
    let mut pos: [Vec<f64>; 3] = Default::default();
    let mut spd: [Vec<f64>; 3] = Default::default();
    let mut hdg: [Vec<f64>; 3] = Default::default();
    let mut per_flight: BTreeMap<(u32, u32), Vec<f64>> = BTreeMap::new();
    let (mut unclassified, mut no_position) = (0u64, 0u64);
    for i in 0..p.t_ms.len() {
        let (Some(&addr), Some(&t)) = (p.addr.get(i), p.t_ms.get(i)) else {
            continue;
        };
        // A coasting row is a prediction, not a report: counted, scored by oa-bench's T1 to T4
        // under their coast split, and left out of the ESASSP-style table.
        if p.coast.get(i).copied().unwrap_or(0) != 0 {
            coasting += 1;
            continue;
        }
        let q = |dt: i64| match adsb_truth::query(truth, addr, t + dt) {
            Ok(s) => Some((s.vel_east_mps, s.vel_north_mps)),
            Err(_) => None,
        };
        let class = match (q(-TURN_SPAN_MS), q(TURN_SPAN_MS)) {
            (Some(a), Some(b)) => {
                let accel = transversal_accel(a, b, 2.0 * TURN_SPAN_MS as f64 / 1000.0);
                Some(if accel > TURN_ACCEL_MPS2 {
                    CLASS_TURN
                } else {
                    CLASS_STRAIGHT
                })
            }
            _ => None,
        };
        if class.is_none() {
            unclassified += 1;
        }
        let (cross, along) = (
            p.cross_m.get(i).copied().unwrap_or(f32::NAN),
            p.along_m.get(i).copied().unwrap_or(f32::NAN),
        );
        let e = oa_math::hypot(f64::from(cross), f64::from(along));
        let sp = f64::from(p.speed_err_kt.get(i).copied().unwrap_or(f32::NAN)) * MPS_PER_KT;
        let hd = f64::from(p.hdg_err_deg.get(i).copied().unwrap_or(f32::NAN));
        if e.is_finite() {
            push_class(&mut pos, class, e);
            per_flight
                .entry((addr, trip_of(truth, addr, t)))
                .or_default()
                .push(e);
        } else {
            no_position += 1;
        }
        if sp.is_finite() {
            push_class(&mut spd, class, sp);
        }
        if hd.is_finite() {
            push_class(&mut hdg, class, hd);
        }
    }
    let mut rms: Vec<f64> = per_flight
        .values()
        .filter(|v| v.len() >= PER_FLIGHT_MIN_N)
        .map(|v| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt())
        .collect();
    rms.sort_by(f64::total_cmp);
    let p95 = if rms.is_empty() {
        0.0
    } else {
        let k = (95 * rms.len()).div_ceil(100).clamp(1, rms.len());
        rms.get(k - 1).copied().unwrap_or(0.0)
    };
    Esassp {
        position_m: pos.each_ref().map(|v| stats_of(v)),
        speed_mps: spd.each_ref().map(|v| stats_of(v)),
        heading_deg: hdg.each_ref().map(|v| stats_of(v)),
        unclassified,
        coasting,
        no_position,
        flights: rms.len() as u64,
        flights_over_r4: rms.iter().filter(|&&r| r > R4_PER_FLIGHT_M).count() as u64,
        flight_rms_max_m: rms.last().copied().unwrap_or(0.0),
        flight_rms_p95_m: p95,
    }
}

/// The scored result of one tracker run. A7.2: cold struct.
pub struct Scored {
    pub bench: RunOutput,
    pub esassp: Esassp,
    /// Read-only per-truth-leg errors from the same final paired rows.
    pub flight_errors: Vec<FlightErrorSummary>,
    /// Confirmed-track one-second rows offered to the scorer, before pair exclusions.
    pub tick_rows: usize,
    pub pairs: usize,
}

/// Why scoring failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreError(pub String);

impl std::fmt::Display for ScoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ScoreError {}

/// The altitude a row is scored with, feet: its rounded value, or [`NO_ALT`] when the row has
/// none (flag clear, or a column that is not a number), so an absent altitude is never scored
/// as 0 ft.
pub fn scored_altitude_ft(flags: u8, alt_ft: f32) -> i32 {
    // A value past the range of an i32 is no altitude, not a saturated one.
    let r = alt_ft.round();
    if flags & O_HAS_ALT != 0 && r.is_finite() && r.abs() < 2.0e9 {
        r as i32
    } else {
        NO_ALT
    }
}

/// Which rows of a run are scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSet {
    /// The fixed one-second output (confirmed tracks, coasting flagged): what a consumer sees.
    Ticks,
    /// The track state at each applied report, at the report's own time: what ESASSP's
    /// per-report errors are closest to.
    Updates,
}

/// Most output rows scored at once: an address range is closed when adding the next aircraft
/// would pass this (an aircraft is never split, so one with more rows is a range of its own).
/// Peak memory of scoring is a few hundred bytes per row of the range, so about 100 MB at the
/// most whatever the length of the run, plus the paired samples ([`oa_bench::chunked`]).
pub const SCORE_CHUNK_ROWS: usize = 1 << 17;

/// The columns of the rows of one address range in one [`RowSet`], in time order, as
/// `oa-bench`'s adapter takes them. Plain columns (A3); A7.2: transient.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScoreRows {
    pub addr: Vec<u32>,
    pub t_ns: Vec<i64>,
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    pub alt_ft: Vec<i32>,
    pub vel_east_mps: Vec<f32>,
    pub vel_north_mps: Vec<f32>,
    pub coast: Vec<bool>,
    /// The row carries `O_UNCERTAIN`.
    pub uncertain: Vec<bool>,
}

/// Where scoring reads a run's output rows from: memory ([`TrackRun`]) or disk
/// ([`SpilledRun`]).
pub trait RowSource {
    /// Rows per address in `rows`.
    fn row_counts(&self, rows: RowSet) -> Result<BTreeMap<u32, u64>, ScoreError>;
    /// The rows in `rows` with an address in `lo..=hi`.
    fn rows_in(&self, rows: RowSet, lo: u32, hi: u32) -> Result<ScoreRows, ScoreError>;
}

fn alt_of(flags: u8, alt: f32) -> i32 {
    scored_altitude_ft(flags, alt)
}

impl RowSource for TrackRun {
    fn row_counts(&self, rows: RowSet) -> Result<BTreeMap<u32, u64>, ScoreError> {
        let mut m = BTreeMap::new();
        for (a, f) in self.out.addr.iter().zip(self.out.flags.iter()) {
            if (f & O_TICK != 0) == (rows == RowSet::Ticks) {
                *m.entry(*a).or_insert(0u64) += 1;
            }
        }
        Ok(m)
    }

    fn rows_in(&self, rows: RowSet, lo: u32, hi: u32) -> Result<ScoreRows, ScoreError> {
        let o = &self.out;
        let mut r = ScoreRows::default();
        for i in 0..o.t_ns.len() {
            let (Some(&a), Some(&f)) = (o.addr.get(i), o.flags.get(i)) else {
                continue;
            };
            if (f & O_TICK != 0) != (rows == RowSet::Ticks) || a < lo || a > hi {
                continue;
            }
            r.addr.push(a);
            r.t_ns.push(o.t_ns.get(i).copied().unwrap_or(0));
            r.lat_e7.push(o.lat_e7.get(i).copied().unwrap_or(0));
            r.lon_e7.push(o.lon_e7.get(i).copied().unwrap_or(0));
            r.alt_ft
                .push(alt_of(f, o.alt_ft.get(i).copied().unwrap_or(f32::NAN)));
            r.vel_east_mps
                .push(o.vel_east_mps.get(i).copied().unwrap_or(f32::NAN));
            r.vel_north_mps
                .push(o.vel_north_mps.get(i).copied().unwrap_or(f32::NAN));
            r.coast.push(f & O_COAST != 0);
            r.uncertain.push(f & O_UNCERTAIN != 0);
        }
        Ok(r)
    }
}

fn io_err(e: std::io::Error) -> ScoreError {
    ScoreError(format!("spilled output: {e}"))
}

impl RowSource for SpilledRun {
    fn row_counts(&self, rows: RowSet) -> Result<BTreeMap<u32, u64>, ScoreError> {
        spill_counts(self, rows == RowSet::Ticks).map_err(io_err)
    }

    fn rows_in(&self, rows: RowSet, lo: u32, hi: u32) -> Result<ScoreRows, ScoreError> {
        let (addr, t_ns, lat_e7, lon_e7, alt, ve, vn, flags) =
            spill_rows(self, rows == RowSet::Ticks, lo, hi).map_err(io_err)?;
        Ok(ScoreRows {
            alt_ft: alt
                .iter()
                .zip(flags.iter())
                .map(|(&a, &f)| alt_of(f, a))
                .collect(),
            coast: flags.iter().map(|&f| f & O_COAST != 0).collect(),
            uncertain: flags.iter().map(|&f| f & O_UNCERTAIN != 0).collect(),
            addr,
            t_ns,
            lat_e7,
            lon_e7,
            vel_east_mps: ve,
            vel_north_mps: vn,
        })
    }
}

/// Splits the address space into the ranges scored one at a time: consecutive addresses of
/// `counts` grouped while the rows stay within `chunk_rows`, the first range starting at 0 and
/// the last ending at `u32::MAX`, so every truth leg falls in exactly one range.
pub fn score_ranges(counts: &BTreeMap<u32, u64>, chunk_rows: usize) -> Vec<(u32, u32)> {
    let mut starts: Vec<u32> = vec![0];
    let mut acc = 0u64;
    for (&a, &c) in counts {
        if acc > 0 && acc + c > chunk_rows as u64 {
            starts.push(a);
            acc = 0;
        }
        acc += c;
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, starts.get(i + 1).map_or(u32::MAX, |&n| n - 1)))
        .collect()
}

/// Scores the rows of `run` in `rows` against `built` (truth built from set B) with the
/// unchanged `oa-bench` metrics. `tcfg` is the truth configuration `built` was made with: its
/// uncertainty gate is also the pairing gate, as in the M2 run. Rows are scored in address
/// ranges of at most [`SCORE_CHUNK_ROWS`] (see [`score_source`]).
pub fn score(
    run: &TrackRun,
    built: &BuiltTruth,
    tcfg: &TruthConfig,
    rows: RowSet,
) -> Result<Scored, ScoreError> {
    score_source(run, built, tcfg, rows, SCORE_CHUNK_ROWS)
}

/// [`score`] over any row source with the range size given. The numbers do not depend on it:
/// flights are independent in every metric, the ranges are taken in ascending address order (the
/// order of the flights of a one-shot run), and the sample lists the percentiles are read from
/// are rebuilt in that order. See [`oa_bench::chunked`] for the exact statement per metric.
pub fn score_source(
    src: &dyn RowSource,
    built: &BuiltTruth,
    tcfg: &TruthConfig,
    rows: RowSet,
    chunk_rows: usize,
) -> Result<Scored, ScoreError> {
    let counts = src.row_counts(rows)?;
    let tick_rows: u64 = counts.values().sum();
    let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
    let truth = Truth {
        query: &q,
        legs: &built.legs,
    };
    // The zone of the presence metrics is the ZAU box of the clip (no airspace model loaded).
    let in_zone = |la: f64, lo: f64| {
        la >= f64::from(ZAU_BOX.lat_min_e7) * 1e-7
            && la <= f64::from(ZAU_BOX.lat_max_e7) * 1e-7
            && lo >= f64::from(ZAU_BOX.lon_min_e7) * 1e-7
            && lo <= f64::from(ZAU_BOX.lon_max_e7) * 1e-7
    };
    let mut env: Env<'_> = oa_bench::truth::empty_env();
    env.in_zone = &in_zone;
    let mut rcfg = RunConfig::default();
    rcfg.pair.max_sigma_m = tcfg.max_sigma95_h_m;
    let tix = truth_index(&truth);
    let mut acc = ChunkAcc::default();
    for range in score_ranges(&counts, chunk_rows) {
        let r = src.rows_in(rows, range.0, range.1)?;
        // One flight per truth leg (T5 per trip, not pooled across trips of one address). The
        // ranges are by address, so all legs of an address are in the one chunk.
        let legacy = legacy_from_track_samples_by_leg(
            &TrackSamples {
                addr: &r.addr,
                t_ns: &r.t_ns,
                lat_e7: &r.lat_e7,
                lon_e7: &r.lon_e7,
                alt_ft: &r.alt_ft,
                vel_east_mps: &r.vel_east_mps,
                vel_north_mps: &r.vel_north_mps,
                coast: &r.coast,
            },
            truth.legs,
        )
        .map_err(|e| ScoreError(format!("track samples: {e:?}")))?;
        drop(r);
        let c = benchmark_chunk(&legacy, &truth, &tix, &env, &rcfg, range);
        chunk_add(&mut acc, c).map_err(|e| ScoreError(e.0))?;
    }
    let (bench, pairs) = chunk_finish(acc, &rcfg).map_err(|e| ScoreError(e.0))?;
    let esassp = esassp_of(&pairs, &built.truth);
    let flight_errors = flight_error_summaries(&pairs, &built.truth, &built.legs)?;
    Ok(Scored {
        bench,
        esassp,
        flight_errors,
        tick_rows: tick_rows as usize,
        pairs: pairs.t_ms.len(),
    })
}

/// Runs the tracker like [`run_tracker`] but writes its output to files under `dir` as it goes,
/// so the run's memory is the tracker's tables and one drain buffer whatever its length. The
/// digest is the same as [`run_tracker`]'s for the same input.
pub fn run_tracker_spilled(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    dir: &Path,
) -> Result<SpilledRun, ScoreError> {
    run_tracker_spilled_observed(meas, cfg, dir, &mut |_, _, _| {})
}

/// [`run_tracker_spilled`], calling `observe(index, stats before, stats after)` around every
/// offered measurement (a diagnostic: the counter differences say what became of it).
pub fn run_tracker_spilled_observed(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    dir: &Path,
    observe: &mut dyn FnMut(usize, &TrackerStats, &TrackerStats),
) -> Result<SpilledRun, ScoreError> {
    let io = |e: std::io::Error| ScoreError(format!("spilled output: {e}"));
    let mut t = tracker_new(cfg).map_err(|e| ScoreError(e.to_string()))?;
    let mut scratch = output_new(cfg.out_capacity);
    let mut w = spill_new(dir).map_err(io)?;
    for (i, m) in meas.iter().enumerate() {
        let before = *tracker_stats(&t);
        tracker_update(&mut t, m, &mut scratch);
        observe(i, &before, tracker_stats(&t));
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            spill_append(&mut w, &scratch).map_err(io)?;
            output_clear(&mut scratch);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut scratch);
    tracker_flush(&mut t, end, &mut scratch);
    spill_append(&mut w, &scratch).map_err(io)?;
    let overflow = scratch.overflow;
    spill_finish(
        w,
        overflow,
        *tracker_stats(&t),
        tracker_log(&t).clone(),
        meas.len() as u64,
    )
    .map_err(io)
}

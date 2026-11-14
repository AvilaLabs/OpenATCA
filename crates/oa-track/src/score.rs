//! The scoring path (`docs/M3_DESIGN.md` section 2): split reports, run the tracker on one half,
//! build truth from the other, score the tracker's one-second output with `oa-bench`'s metric
//! functions. Core (pure) except where noted; the clock and the files are the binary's.
//!
//! # The split, defined exactly
//!
//! For each aircraft (the 24-bit address with `oa_adsb::NON_ICAO`, the same key as `oa-truth`),
//! its decoded reports are put in the total order (time, input index) and numbered from 0. The
//! even-numbered reports are **set A** (the tracker's input) and the odd-numbered ones **set B**
//! (the truth's input). The two sets are disjoint, together they are every decoded report, and
//! the quality fields (NACp, NIC, SIL, version, NACv) are carried forward over the whole
//! aircraft sequence before the split, by the same 60 s rule and code as the truth gates
//! (`oa_truth::quality_check`), and written into the rows of both sets. readsb prints them on a
//! minority of rows, so without this half the aircraft would have none in age after the split;
//! they describe the aircraft's navigation state, not a measurement, so sharing them does not
//! share any position noise. GNSS errors that are correlated over seconds are shared
//! between the sets, so the scored error is a lower bound; every report says so.
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

use oa_adsb::{reports_len, reports_push, reports_row, Reports, NONE_U8, ZAU_BOX};
use oa_bench::adsb_truth::{self, BuiltTruth};
use oa_bench::assoc::{associate, index_truth};
use oa_bench::legacy::NO_ALT;
use oa_bench::pair::{build_pairs, PairTable};
use oa_bench::run::{run_benchmark, RunConfig, RunOutput};
use oa_bench::stats::{stats_of, Stats};
use oa_bench::thindex::build_th_index;
use oa_bench::tracker_input::{legacy_from_track_samples, TrackSamples};
use oa_bench::truth::{Env, Truth};
use oa_sha256::{to_hex, Sha256};
use oa_truth::{quality_check, QualityState, TruthConfig, DEFAULT_CONFIG};

use crate::params::NS_PER_S;
use crate::tracker::{
    tracker_advance, tracker_flush, tracker_log, tracker_new, tracker_stats, tracker_update,
};
use crate::types::{
    output_clear, output_hash_into, output_len, output_new, Measurement, TrackError, TrackLog,
    TrackOutput, TrackerConfig, TrackerStats, O_COAST, O_HAS_ALT, O_TICK,
};

/// The split, in words, for every report.
pub const SPLIT_TEXT: &str = "per aircraft, reports in (time, input index) order numbered from 0: even to set A (tracker), odd to set B (truth)";
/// The basis every scored number carries.
pub const BASIS_TEXT: &str = "on thinned ~17 s input split in two; not an ESASSP exit result; GNSS error correlated over seconds is shared between the sets";
/// A track is dropped after this long without an update in the scoring run, ns: the gap at which
/// `oa-truth` starts a new leg (`oa_truth::LEG_GAP_MS`), so tracker and truth end an aircraft's
/// pass at the same silence.
pub const SCORE_DROP_AFTER_NS: i64 = oa_truth::LEG_GAP_MS * 1_000_000;
/// The scoring run gives the tracker an output table this large (rows) and drains it often, so
/// no row is lost: 1 Mi rows.
pub const SCORE_OUT_CAPACITY: usize = 1 << 20;
/// Drain the output table into the accumulator when it holds more than this many rows.
const DRAIN_ROWS: usize = 1 << 16;
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
    for &(addr, t, i) in &order {
        if cur != Some(addr) {
            cur = Some(addr);
            k = 0;
            q = QualityState::default();
            st.aircraft += 1;
        }
        if let Some(mut row) = reports_row(r, i as usize) {
            let v = quality_check(&mut q, &row, t, &DEFAULT_CONFIG);
            row.nac_p = v.nacp.unwrap_or(NONE_U8);
            row.nic = v.nic.unwrap_or(NONE_U8);
            row.sil = v.sil.unwrap_or(NONE_U8);
            row.version = v.version.unwrap_or(NONE_U8);
            row.nac_v = v.nacv.unwrap_or(NONE_U8);
            if k.is_multiple_of(2) {
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
            (Some(a), Some(b)) if a.0 == b.0 => Some(b.1 - a.1),
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
    e.atan2(n)
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
    let speed = (v1.0.hypot(v1.1) + v2.0.hypot(v2.1)) / 2.0;
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

fn push_class(v: &mut [Vec<f64>; 3], class: Option<usize>, x: f64) {
    if let Some(a) = v.get_mut(CLASS_ALL) {
        a.push(x);
    }
    if let Some(c) = class.and_then(|c| v.get_mut(c)) {
        c.push(x);
    }
}

/// The ESASSP-style split of a paired-sample table (see the module documentation).
pub fn esassp_of(p: &PairTable, truth: &oa_truth::Truth) -> Esassp {
    let mut coasting = 0u64;
    let mut pos: [Vec<f64>; 3] = Default::default();
    let mut spd: [Vec<f64>; 3] = Default::default();
    let mut hdg: [Vec<f64>; 3] = Default::default();
    let mut per_flight: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
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
        let e = f64::from(cross).hypot(f64::from(along));
        let sp = f64::from(p.speed_err_kt.get(i).copied().unwrap_or(f32::NAN)) * MPS_PER_KT;
        let hd = f64::from(p.hdg_err_deg.get(i).copied().unwrap_or(f32::NAN));
        if e.is_finite() {
            push_class(&mut pos, class, e);
            per_flight.entry(addr).or_default().push(e);
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
    /// One-second rows scored (confirmed tracks).
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

/// Which rows of a run are scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSet {
    /// The fixed one-second output (confirmed tracks, coasting flagged): what a consumer sees.
    Ticks,
    /// The track state at each applied report, at the report's own time: what ESASSP's
    /// per-report errors are closest to.
    Updates,
}

/// Scores the rows of `run` in `rows` against `built` (truth built from set B) with the
/// unchanged `oa-bench` metrics. `tcfg` is the truth configuration `built` was made with: its
/// uncertainty gate is also the pairing gate, as in the M2 run.
pub fn score(
    run: &TrackRun,
    built: &BuiltTruth,
    tcfg: &TruthConfig,
    rows: RowSet,
) -> Result<Scored, ScoreError> {
    let o = &run.out;
    let ticks: Vec<usize> = (0..o.t_ns.len())
        .filter(|&i| {
            o.flags
                .get(i)
                .is_some_and(|f| (f & O_TICK != 0) == (rows == RowSet::Ticks))
        })
        .collect();
    let pick = |f: &dyn Fn(usize) -> Option<i64>| -> Vec<i64> {
        ticks.iter().map(|&i| f(i).unwrap_or(0)).collect()
    };
    let t_ns = pick(&|i| o.t_ns.get(i).copied());
    let addr: Vec<u32> = ticks
        .iter()
        .filter_map(|&i| o.addr.get(i).copied())
        .collect();
    let lat: Vec<i32> = ticks
        .iter()
        .filter_map(|&i| o.lat_e7.get(i).copied())
        .collect();
    let lon: Vec<i32> = ticks
        .iter()
        .filter_map(|&i| o.lon_e7.get(i).copied())
        .collect();
    let alt: Vec<i32> = ticks
        .iter()
        .map(|&i| {
            if o.flags.get(i).is_some_and(|f| f & O_HAS_ALT != 0) {
                o.alt_ft.get(i).map_or(NO_ALT, |&a| a.round() as i32)
            } else {
                NO_ALT
            }
        })
        .collect();
    let ve: Vec<f32> = ticks
        .iter()
        .filter_map(|&i| o.vel_east_mps.get(i).copied())
        .collect();
    let vn: Vec<f32> = ticks
        .iter()
        .filter_map(|&i| o.vel_north_mps.get(i).copied())
        .collect();
    let coast: Vec<bool> = ticks
        .iter()
        .map(|&i| o.flags.get(i).is_some_and(|f| f & O_COAST != 0))
        .collect();
    let legacy = legacy_from_track_samples(&TrackSamples {
        addr: &addr,
        t_ns: &t_ns,
        lat_e7: &lat,
        lon_e7: &lon,
        alt_ft: &alt,
        vel_east_mps: &ve,
        vel_north_mps: &vn,
        coast: &coast,
    })
    .map_err(|e| ScoreError(format!("track samples: {e:?}")))?;
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
    let bench = run_benchmark(&legacy, &truth, &env, &rcfg);
    // The paired samples again, for the ESASSP split (run_benchmark does not return them).
    let ix = build_th_index(&legacy.th, legacy.flights.gufi.len());
    let tix = index_truth(truth.legs);
    let assoc = associate(&legacy, &ix, &truth, &tix, &rcfg.assoc);
    let (pairs, _) = build_pairs(&legacy, &ix, &assoc, &truth, &rcfg.pair);
    let esassp = esassp_of(&pairs, &built.truth);
    Ok(Scored {
        bench,
        esassp,
        tick_rows: ticks.len(),
        pairs: pairs.t_ms.len(),
    })
}

//! The exact-truth scoring harness of the radar fusion (docs/M3_DESIGN.md sections 2 and 3):
//! simulate plots (and ADS-B) from a truth, run a tracker variant on identical inputs, score its
//! output against **the same truth the simulator used**. Shell-side code (it builds the world,
//! reads the clock for timing and allocates freely, RULESET C6); the tracker it drives is core.
//!
//! # Exact truth
//!
//! Plots come from `oa_sensorsim::radar::simulate_radar` over a [`oa_truth::Truth`]; the scorer
//! asks `oa_truth::truth_at` for the same aircraft at the track row's time, through
//! `oa_bench::adsb_truth` and the unchanged `oa-bench` pairing (`score::score`). The simulator
//! treats the truth's trajectory as the real flight path, so the truth's own error is zero.
//! What remains between the number reported and the tracker's error is only the output's
//! representation: positions are 1e-7 degree (1.1 cm), the pairing queries truth in whole
//! milliseconds (at most 0.25 m of along-track error at 250 m/s, and plot and ADS-B times are
//! rounded to the millisecond here so the two agree), and velocity is carried in 0.1 knot
//! (0.05 m/s). All are far under the 70 m plot noise. Two things are *not* exact and are
//! stated wherever a number is: ADS-B position noise is white at the NACp bound (real GNSS error
//! is correlated over seconds), and pressure altitude is taken as ellipsoidal height.
//!
//! # Identity
//!
//! Radar-only tracks carry no address, so scoring needs to know which aircraft a track follows.
//! That comes from the plots' simulator labels, which the tracker records but never reads
//! (`RadarPlot::label`; a test shows labels do not change any decision): a track is the aircraft
//! its applied true plots mostly came from. A swap is a change of that label inside one track; a
//! plot of another aircraft applied to a track is a wrong plot; a track mostly of false plots is
//! a false track; a second track on an aircraft that already has one is a duplicate and is not
//! scored (counted).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use oa_adsb::Reports;
use oa_bench::adsb_truth::{self, BuiltTruth};
use oa_bench::geo::enu;
use oa_bench::stats::{stats_of, Stats};
use oa_sensorsim::plots::{plots_len, plots_row, KIND_FALSE, KIND_SPLIT};
use oa_sensorsim::prng::Rng;
use oa_sensorsim::profile::{RadarProfile, EARTH_K_STANDARD, SCAN_EN_ROUTE_S, SCAN_TERMINAL_S};
use oa_sensorsim::radar::{simulate_radar, SimStats};
use oa_truth::{
    aircraft_addrs, leg_end_ns, leg_start_ns, legs_of, nacp_epu_m, nacv_epu_mps, truth_at, LegIx,
    Truth, TruthQuery, SIGMA95,
};

use crate::fusion::{
    fusion_adsb, fusion_advance, fusion_assign_log, fusion_config, fusion_finish, fusion_new,
    fusion_plots, fusion_radar_tracks, fusion_registration, fusion_stats, fusion_tracker,
    fusion_tracker_config, fusion_tracker_stats, AssignLog, Fusion, FusionError, FusionStats,
    RadarTrackRec, ASSOC_GNN, ASSOC_NEAREST,
};
use crate::fusion_params::KEY_RADAR_BASE;
use crate::radar::{plot_convert, radar_geom, RadarPlot, RadarSite, FT_M, NONE_ALT, NO_BIAS};
use crate::registration::{RegSample, RegState};
use crate::score::{
    score, transversal_accel, Esassp, RowSet, TrackRun, TURN_ACCEL_MPS2, TURN_SPAN_MS,
};
use crate::tracker::tracker_log;
use crate::types::{
    output_clear, output_new, output_push, Measurement, OutRow, TrackLog, TrackOutput,
    TrackerConfig, TrackerStats, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_IDENT,
};

/// The clock of the synthetic worlds starts here, Unix ns.
pub const EPOCH_NS: i64 = 1_700_000_000_000_000_000;

/// The two radars of the study. A radar's frame and the scan period are fixed by the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// O'Hare reference point (NASR 2026-10-29: 41.97694027 N, 87.90814972 W, 680 ft), 4.8 s,
    /// 60 NM coverage (a stated study value).
    Terminal,
    /// Peoria reference point (40.66419444 N, 89.69325 W, 660.7 ft), 12 s, 150 NM coverage.
    EnRoute,
}

/// Where a radar is and how far it reaches. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteDef {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub height_m: f64,
    pub period_s: f64,
    pub coverage_m: f64,
}

/// The site of a kind (the same reference points `oa-sensorsim`'s binary uses).
pub fn site_def(k: Kind) -> SiteDef {
    match k {
        Kind::Terminal => SiteDef {
            lat_deg: 41.976_940_27,
            lon_deg: -87.908_149_72,
            height_m: 680.0 * FT_M,
            period_s: SCAN_TERMINAL_S,
            coverage_m: 60.0 * 1852.0,
        },
        Kind::EnRoute => SiteDef {
            lat_deg: 40.664_194_44,
            lon_deg: -89.693_25,
            height_m: 660.7 * FT_M,
            period_s: SCAN_EN_ROUTE_S,
            coverage_m: 150.0 * 1852.0,
        },
    }
}

/// Split-plot geometry used by the studies: range 140 m (two plot sigmas), azimuth 0.16
/// degrees. No sourced value exists (docs/BENCHMARKS.md 3.1 "Gaps"); a stress choice.
pub const SPLIT_DR_M: f64 = 140.0;
pub const SPLIT_DAZ_DEG: f64 = 0.16;

/// A radar for the study: the simulator's site and the tracker's. A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudyRadar {
    pub sim: oa_sensorsim::radar::RadarSite,
    pub trk: RadarSite,
}

/// A radar of `kind` with `profile`, range bias `bias_range_m` and azimuth bias `bias_az_deg`.
/// The tracker is told only the profile's random errors, never the bias.
pub fn make_radar(
    kind: Kind,
    profile: RadarProfile,
    bias_range_m: f64,
    bias_az_deg: f64,
) -> StudyRadar {
    let d = site_def(kind);
    let sim = oa_sensorsim::radar::RadarSite {
        lat_deg: d.lat_deg,
        lon_deg: d.lon_deg,
        height_m: d.height_m,
        rotation_period_s: d.period_s,
        initial_az_deg: 0.0,
        epoch_ns: EPOCH_NS,
        coverage_range_m: d.coverage_m,
        earth_k: EARTH_K_STANDARD,
        profile,
        bias_range_m,
        bias_az_deg,
        split_dr_m: SPLIT_DR_M,
        split_daz_deg: SPLIT_DAZ_DEG,
        sac: 0,
        sic: 1,
    };
    let trk = RadarSite {
        lat_deg: d.lat_deg,
        lon_deg: d.lon_deg,
        height_m: d.height_m,
        sigma_range_m: profile.range_sigma_m,
        sigma_az_deg: profile.az_sigma_deg,
        range_quantum_m: profile.range_quantum_m,
        alt_quantum_ft: profile.alt_quantum_ft as u16,
        period_ns: (d.period_s * 1.0e9).round() as i64,
        epoch_ns: EPOCH_NS,
        sensor: match kind {
            Kind::Terminal => 1,
            Kind::EnRoute => 2,
        },
    };
    StudyRadar { sim, trk }
}

/// A truth as the scorer wants it: the model plus its legs (no callsigns). Exact: nothing is
/// added to the model.
pub fn built_of(truth: Truth) -> BuiltTruth {
    let legs = adsb_truth::legs(&truth, &Reports::default());
    BuiltTruth {
        truth,
        stats: oa_truth::TruthStats::default(),
        legs,
        digest: String::new(),
    }
}

fn round_ms(t_ns: i64) -> i64 {
    (t_ns + 500_000).div_euclid(1_000_000) * 1_000_000
}

/// Plots of one radar over a truth, as the tracker's input, with the simulator's counts and the
/// kind of each plot (`KIND_*`). Times are rounded to the millisecond.
pub fn simulate_plots(
    truth: &Truth,
    radar: &StudyRadar,
    seed: u64,
    from_ns: i64,
    to_ns: i64,
) -> Result<(Vec<RadarPlot>, Vec<u8>, SimStats), String> {
    let (plots, st) =
        simulate_radar(truth, &radar.sim, seed, from_ns, to_ns).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(plots_len(&plots));
    let mut kinds = Vec::with_capacity(plots_len(&plots));
    for i in 0..plots_len(&plots) {
        let Some(p) = plots_row(&plots, i) else { break };
        out.push(RadarPlot {
            time_ns: round_ms(p.time_ns),
            range_m: p.range_m,
            az_deg: p.az_deg,
            alt_ft: p.alt_ft,
            addr: p.addr,
            label: p.truth_addr,
            mode_a: p.mode_a,
        });
        kinds.push(p.kind);
    }
    Ok((out, kinds, st))
}

/// ADS-B report settings of the study. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsbSim {
    /// NACp of every report (10: 95 % bound 10 m).
    pub nacp: u8,
    /// NACv of every report (2: 95 % bound 3 m/s).
    pub nacv: u8,
    /// Reporting interval, ns.
    pub period_ns: i64,
}

/// The study's ADS-B: NACp 10, NACv 2, one report per second per aircraft.
pub const ADSB_STUDY: AdsbSim = AdsbSim {
    nacp: 10,
    nacv: 2,
    period_ns: 1_000_000_000,
};

/// Most samples [`simulate_adsb`] will produce: 2^25. The study's one-hour clip at one report
/// per second gives 657,824, so the limit is 51 times that.
pub const MAX_ADSB_SAMPLES: u64 = 1 << 25;

/// ADS-B measurements from the truth for the aircraft `equip` accepts: position and velocity
/// with white Gaussian noise at the NACp and NACv 95 % bounds over 2.448 per axis (an assumption:
/// real receivers beat the bound and their error is correlated), altitude quantised to 25 ft,
/// vertical rate in 64 fpm steps. Each aircraft's phase within the second is random. A sample's
/// noise is keyed by the seed, the address and the sample time, so it is the same whichever leg or
/// subrange produces the sample. Refuses, before allocating, a period that is not positive, a
/// NACp or NACv without a bound and more than [`MAX_ADSB_SAMPLES`] samples.
pub fn simulate_adsb(
    truth: &Truth,
    cfg: &AdsbSim,
    seed: u64,
    from_ns: i64,
    to_ns: i64,
    equip: &dyn Fn(u32) -> bool,
) -> Result<Vec<Measurement>, String> {
    let sp = nacp_epu_m(cfg.nacp) / SIGMA95;
    let sv = nacv_epu_mps(cfg.nacv) / SIGMA95;
    if cfg.period_ns <= 0 {
        return Err(format!("ADS-B period {} ns is not positive", cfg.period_ns));
    }
    if !(sp.is_finite() && sv.is_finite()) {
        return Err("NACp or NACv has no bound".to_string());
    }
    // The count first, from the legs' covered spans, so a small period or a long range is
    // refused before anything is allocated.
    let mut count = 0u64;
    for &addr in aircraft_addrs(truth) {
        if !equip(addr) {
            continue;
        }
        let range = legs_of(truth, addr);
        for l in 0..range.count {
            let leg = LegIx(range.first.0 + l);
            let (Some(ls), Some(le)) = (leg_start_ns(truth, leg), leg_end_ns(truth, leg)) else {
                continue;
            };
            let (lo, hi) = (ls.max(from_ns), le.min(to_ns));
            if lo <= hi {
                let span = (i128::from(hi) - i128::from(lo)) / i128::from(cfg.period_ns) + 1;
                count = count.saturating_add(u64::try_from(span).unwrap_or(u64::MAX));
            }
        }
    }
    if count > MAX_ADSB_SAMPLES {
        return Err(format!(
            "ADS-B period {} ns would give {count} samples, over {MAX_ADSB_SAMPLES}",
            cfg.period_ns
        ));
    }
    let mut out = Vec::with_capacity(count as usize);
    for &addr in aircraft_addrs(truth) {
        if !equip(addr) {
            continue;
        }
        let range = legs_of(truth, addr);
        let mut rng0 = Rng::from_stream(seed, u64::from(addr), u64::MAX);
        let phase = (rng0.unit() * cfg.period_ns as f64) as i64;
        for l in 0..range.count {
            let leg = LegIx(range.first.0 + l);
            let (Some(ls), Some(le)) = (leg_start_ns(truth, leg), leg_end_ns(truth, leg)) else {
                continue;
            };
            let (lo, hi) = (ls.max(from_ns), le.min(to_ns));
            let mut t = round_ms(lo + (phase - lo).rem_euclid(cfg.period_ns));
            while t <= hi {
                if let TruthQuery::Ok(s) = truth_at(truth, addr, t.div_euclid(1_000_000)) {
                    let mut rng = Rng::from_stream(seed, u64::from(addr), t as u64);
                    let (ne, nn, nve, nvn) = (
                        rng.normal() * sp,
                        rng.normal() * sp,
                        rng.normal() * sv,
                        rng.normal() * sv,
                    );
                    let fr = oa_geo::aeqd::frame_new(s.lat_deg, s.lon_deg);
                    // A noise draw past the chart gives no sample.
                    if let Some((lat, lon)) = oa_geo::aeqd::unproject(&fr, ne, nn) {
                        let var = (sp * sp) as f32;
                        let vvar = (sv * sv) as f32;
                        let q = |a: f64| (a / 25.0).round() * 25.0;
                        out.push(Measurement {
                            toa_ns: t,
                            pos_cov_m2: [var, 0.0, var],
                            vel_cov_m2s2: [vvar, 0.0, vvar],
                            vel_east_mps: (s.vel_east_mps + nve) as f32,
                            vel_north_mps: (s.vel_north_mps + nvn) as f32,
                            lat_e7: (lat * 1.0e7).round() as i32,
                            lon_e7: (lon * 1.0e7).round() as i32,
                            alt_ft: s.baro_alt_ft.map_or(NONE_I32, |a| q(a) as i32),
                            vrate_fpm: s
                                .vert_rate_fpm
                                .map_or(NONE_I32, |r| ((r / 64.0).round() * 64.0) as i32),
                            addr,
                            alt_quant_ft: 25,
                            sensor: 0,
                            ident: NO_IDENT,
                            flags: M_HAS_VEL,
                            category: 0,
                        });
                    }
                }
                let Some(next) = t.checked_add(cfg.period_ns) else {
                    break;
                };
                t = next;
            }
        }
    }
    out.sort_by_key(|m| (m.toa_ns, m.addr));
    Ok(out)
}

/// A tracker variant under test. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Variant {
    pub name: &'static str,
    /// Horizontal filter mode: 0 the interacting pair, 1 to 3 a single straight-model filter.
    pub h_mode: u8,
    /// `ASSOC_GNN` or `ASSOC_NEAREST`.
    pub assoc: u8,
    pub hold_ambiguous: bool,
    pub registration: bool,
}

/// Baseline (a): one nearly-constant-velocity Kalman filter (process noise 1 m^2/s^3), nearest
/// neighbour association, no registration.
pub const V_BASE_Q1: Variant = Variant {
    name: "a1 CV q=1 + NN",
    h_mode: 1,
    assoc: ASSOC_NEAREST,
    hold_ambiguous: false,
    registration: false,
};
/// Baseline (a) tuned for turns (process noise 5 m^2/s^3), the same association.
pub const V_BASE_Q5: Variant = Variant {
    name: "a3 CV q=5 + NN",
    h_mode: 3,
    assoc: ASSOC_NEAREST,
    hold_ambiguous: false,
    registration: false,
};
/// Our tracker without registration (b).
pub const V_OURS_NOREG: Variant = Variant {
    name: "b  ours, no reg",
    h_mode: 0,
    assoc: ASSOC_GNN,
    hold_ambiguous: true,
    registration: false,
};
/// Our tracker with registration (c).
pub const V_OURS_REG: Variant = Variant {
    name: "c  ours + reg",
    h_mode: 0,
    assoc: ASSOC_GNN,
    hold_ambiguous: true,
    registration: true,
};

/// Wall-clock cost of the processing, per input. A7.2: cold struct.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timing {
    /// Inputs processed.
    pub events: u64,
    pub total_s: f64,
    /// `(inputs, microseconds per input)` for every call that processed any.
    pub samples: Vec<(u32, f64)>,
}

/// Mean, median, 99th percentile and worst microseconds per input (input-weighted).
pub fn timing_summary(t: &Timing) -> (f64, f64, f64, f64) {
    if t.events == 0 || t.samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut s = t.samples.clone();
    s.sort_by(|a, b| a.1.total_cmp(&b.1));
    let total: u64 = s.iter().map(|x| u64::from(x.0)).sum();
    let at = |p: f64| {
        let want = (p * total as f64).ceil().max(1.0) as u64;
        let mut acc = 0u64;
        for (n, us) in &s {
            acc += u64::from(*n);
            if acc >= want {
                return *us;
            }
        }
        s.last().map_or(0.0, |x| x.1)
    };
    (
        t.total_s * 1.0e6 / t.events as f64,
        at(0.5),
        at(0.99),
        s.last().map_or(0.0, |x| x.1),
    )
}

/// A finished tracker run. A7.2: cold struct.
pub struct RunOut {
    pub out: TrackOutput,
    pub assign: AssignLog,
    pub radar_tracks: Vec<RadarTrackRec>,
    pub fstats: FusionStats,
    pub tstats: TrackerStats,
    pub log: TrackLog,
    pub reg: Vec<(RegState, Vec<RegSample>)>,
    pub timing: Timing,
    pub overflow: u64,
}

fn append_out(dst: &mut TrackOutput, src: &TrackOutput) {
    for i in 0..src.t_ns.len() {
        copy_row(src, i, dst, None);
    }
}

/// Copies row `i` of `src` to `dst`, with a different address if given.
fn copy_row(src: &TrackOutput, i: usize, dst: &mut TrackOutput, addr: Option<u32>) {
    let g = |v: &[f32]| v.get(i).copied().unwrap_or(0.0);
    let row = OutRow {
        t_ns: src.t_ns.get(i).copied().unwrap_or(0),
        addr: addr.unwrap_or_else(|| src.addr.get(i).copied().unwrap_or(0)),
        lat_e7: src.lat_e7.get(i).copied().unwrap_or(0),
        lon_e7: src.lon_e7.get(i).copied().unwrap_or(0),
        vel_east_mps: g(&src.vel_east_mps),
        vel_north_mps: g(&src.vel_north_mps),
        pos_cov_m2: src.pos_cov_m2.get(i).copied().unwrap_or([0.0; 3]),
        vel_cov_m2s2: src.vel_cov_m2s2.get(i).copied().unwrap_or([0.0; 3]),
        turn_rate_rps: g(&src.turn_rate_rps),
        alt_ft: g(&src.alt_ft),
        alt_var_ft2: g(&src.alt_var_ft2),
        vrate_fpm: g(&src.vrate_fpm),
        mu_h: src.mu_h.get(i).copied().unwrap_or([0.0; 2]),
        mu_v: src.mu_v.get(i).copied().unwrap_or([0.0; 2]),
        flags: src.flags.get(i).copied().unwrap_or(0),
    };
    output_push(dst, &row);
}

fn unbounded() -> TrackOutput {
    TrackOutput {
        capacity: usize::MAX,
        ..TrackOutput::default()
    }
}

/// Runs `variant` over the plots of every radar (`plots[r]` for `sites[r]`) and the ADS-B
/// measurements, fed in time order, and returns the output and everything logged.
pub fn run_variant(
    variant: &Variant,
    sites: &[RadarSite],
    plots: &[&[RadarPlot]],
    adsb: &[Measurement],
) -> Result<RunOut, FusionError> {
    let base = TrackerConfig {
        h_mode: variant.h_mode,
        out_capacity: 1 << 17,
        log_capacity: 1 << 16,
        ..DEFAULT_CONFIG
    };
    let mut cfg = fusion_config(fusion_tracker_config(&base, sites));
    cfg.registration = variant.registration;
    cfg.assoc = variant.assoc;
    cfg.hold_ambiguous = variant.hold_ambiguous;
    cfg.log_assign = true;
    let mut f: Fusion = fusion_new(&cfg, sites)?;
    // Merge the inputs by time: (time, source, radar, index); ADS-B first on a tie.
    let mut order: Vec<(i64, u8, usize, usize)> = Vec::new();
    for (i, m) in adsb.iter().enumerate() {
        order.push((m.toa_ns, 0, 0, i));
    }
    for (r, ps) in plots.iter().enumerate() {
        for (i, p) in ps.iter().enumerate() {
            order.push((p.time_ns, 1, r, i));
        }
    }
    order.sort_unstable();
    let mut scratch = output_new(1 << 17);
    let mut acc = unbounded();
    let mut timing = Timing::default();
    let started = Instant::now();
    let mut last = 0i64;
    for &(t, src, r, i) in &order {
        let before = fusion_stats(&f).processed;
        let t0 = Instant::now();
        if src == 0 {
            if let Some(m) = adsb.get(i) {
                fusion_adsb(&mut f, m);
            }
        } else if let Some(p) = plots.get(r).and_then(|v| v.get(i)) {
            fusion_plots(&mut f, r, std::slice::from_ref(p));
        }
        fusion_advance(&mut f, t, &mut scratch);
        let el = t0.elapsed().as_secs_f64();
        let n = fusion_stats(&f).processed - before;
        if n > 0 {
            timing.events += n;
            timing.samples.push((n as u32, el * 1.0e6 / n as f64));
        }
        last = t;
        if scratch.t_ns.len() > (1 << 16) {
            append_out(&mut acc, &scratch);
            output_clear(&mut scratch);
        }
    }
    let t0 = Instant::now();
    let before = fusion_stats(&f).processed;
    fusion_finish(&mut f, last, &mut scratch);
    let n = fusion_stats(&f).processed - before;
    if n > 0 {
        timing.events += n;
        timing
            .samples
            .push((n as u32, t0.elapsed().as_secs_f64() * 1.0e6 / n as f64));
    }
    timing.total_s = started.elapsed().as_secs_f64();
    append_out(&mut acc, &scratch);
    let overflow = scratch.overflow;
    let reg = (0..sites.len())
        .filter_map(|r| fusion_registration(&f, r).map(|(s, h)| (s.clone(), h.to_vec())))
        .collect();
    Ok(RunOut {
        out: acc,
        assign: fusion_assign_log(&f).clone(),
        radar_tracks: fusion_radar_tracks(&f).to_vec(),
        fstats: *fusion_stats(&f),
        tstats: *fusion_tracker_stats(&f),
        log: tracker_log(fusion_tracker(&f)).clone(),
        reg,
        timing,
        overflow,
    })
}

// ---- identity and scoring --------------------------------------------------------------

/// Which aircraft each track is, and the identity errors. A7.2: cold struct.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ident {
    /// Track key to truth address, for the primary track of each aircraft.
    pub truth_of: BTreeMap<u32, u32>,
    /// Tracks that are a second track on an aircraft that has one.
    pub duplicates: u64,
    /// Tracks mostly made of false plots (or with no true plot).
    pub false_tracks: u64,
    /// Changes of the aircraft behind a track's applied true plots.
    pub swaps: u64,
    /// True plots applied to a track that is another aircraft.
    pub wrong_plots: u64,
    /// False plots applied to a track of a real aircraft.
    pub false_applied: u64,
    /// Tracks with at least one applied plot, and keys seen only in the output.
    pub tracks: u64,
}

/// Decides the identity of every track of `run` from the plots it applied (see the module
/// documentation). Address keys are their address; radar keys are the aircraft most of their
/// true plots came from.
pub fn identify(run: &RunOut) -> Ident {
    let mut per_key: BTreeMap<u32, Vec<(i64, u32)>> = BTreeMap::new();
    for i in 0..run.assign.t_ns.len() {
        if let (Some(&t), Some(&k), Some(&l)) = (
            run.assign.t_ns.get(i),
            run.assign.key.get(i),
            run.assign.label.get(i),
        ) {
            per_key.entry(k).or_default().push((t, l));
        }
    }
    for &k in &run.out.addr {
        per_key.entry(k).or_default();
    }
    let mut id = Ident::default();
    // Candidates per truth address: (is address key, true plots, key).
    let mut claims: BTreeMap<u32, Vec<(bool, usize, u32)>> = BTreeMap::new();
    for (&key, rows) in per_key.iter_mut() {
        rows.sort_by_key(|r| r.0);
        id.tracks += 1;
        let is_addr = key < KEY_RADAR_BASE;
        let mut hist: BTreeMap<u32, usize> = BTreeMap::new();
        let mut false_n = 0usize;
        for &(_, l) in rows.iter() {
            if l == u32::MAX {
                false_n += 1;
            } else {
                *hist.entry(l).or_insert(0) += 1;
            }
        }
        let true_n: usize = hist.values().sum();
        let identity = if is_addr {
            Some(key)
        } else {
            hist.iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(l, _)| *l)
        };
        let Some(ident) = identity else {
            id.false_tracks += 1;
            continue;
        };
        if !is_addr && false_n >= true_n {
            id.false_tracks += 1;
            continue;
        }
        // Swaps: label changes in time order (an address track starts as its address).
        let mut prev = is_addr.then_some(key);
        for &(_, l) in rows.iter() {
            if l == u32::MAX {
                id.false_applied += 1;
                continue;
            }
            if let Some(p) = prev {
                if p != l {
                    id.swaps += 1;
                }
            }
            prev = Some(l);
            if l != ident {
                id.wrong_plots += 1;
            }
        }
        claims.entry(ident).or_default().push((
            is_addr,
            hist.get(&ident).copied().unwrap_or(0),
            key,
        ));
    }
    for (truth, mut v) in claims {
        // The address track first, then the track with the most plots of the aircraft.
        v.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        if let Some(&(_, _, key)) = v.first() {
            id.truth_of.insert(key, truth);
        }
        id.duplicates += v.len().saturating_sub(1) as u64;
    }
    id
}

/// The rows of `run` that belong to primary tracks whose aircraft `keep` accepts, addressed by
/// the aircraft (so `oa-bench` pairs them with the truth leg).
pub fn scored_rows(run: &RunOut, id: &Ident, keep: &dyn Fn(u32) -> bool) -> TrackRun {
    let mut out = unbounded();
    for i in 0..run.out.t_ns.len() {
        let Some(&k) = run.out.addr.get(i) else {
            continue;
        };
        if let Some(&truth) = id.truth_of.get(&k) {
            if keep(truth) {
                copy_row(&run.out, i, &mut out, Some(truth));
            }
        }
    }
    TrackRun {
        out,
        stats: run.tstats,
        log: run.log.clone(),
        offered: run.fstats.plots_in + run.fstats.adsb_in,
        digest: String::new(),
        overflow: run.overflow,
    }
}

/// The ESASSP-style table (position, speed and heading by class) of the primary tracks of the
/// aircraft `keep` accepts, on update rows and on the one-second ticks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub updates: Esassp,
    pub ticks: Esassp,
    pub update_rows: usize,
}

/// Scores `run` against `built` (exact truth) for the aircraft `keep` accepts.
pub fn score_run(
    run: &RunOut,
    id: &Ident,
    built: &BuiltTruth,
    keep: &dyn Fn(u32) -> bool,
) -> Result<Table, String> {
    let tr = scored_rows(run, id, keep);
    let tcfg = built.truth.cfg;
    let u = score(&tr, built, &tcfg, RowSet::Updates).map_err(|e| e.to_string())?;
    let k = score(&tr, built, &tcfg, RowSet::Ticks).map_err(|e| e.to_string())?;
    Ok(Table {
        update_rows: u.tick_rows,
        updates: u.esassp,
        ticks: k.esassp,
    })
}

/// Truth aircraft that had at least one true plot, and those with a primary track.
pub fn coverage_counts(id: &Ident, plot_sets: &[&[RadarPlot]]) -> (usize, usize) {
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    for ps in plot_sets {
        for p in ps.iter() {
            if p.label != u32::MAX {
                seen.insert(p.label);
            }
        }
    }
    let tracked: BTreeSet<u32> = id.truth_of.values().copied().collect();
    (seen.len(), seen.intersection(&tracked).count())
}

/// Initiation delays, seconds, of the aircraft whose primary track was made by radar-only
/// initiation: from the aircraft's first true plot (in `plot_sets`) to the confirming plot.
pub fn initiation_delays_s(run: &RunOut, id: &Ident, plot_sets: &[&[RadarPlot]]) -> Vec<f64> {
    let mut first: BTreeMap<u32, i64> = BTreeMap::new();
    for ps in plot_sets {
        for p in ps.iter() {
            if p.label != u32::MAX {
                let e = first.entry(p.label).or_insert(p.time_ns);
                *e = (*e).min(p.time_ns);
            }
        }
    }
    let mut v = Vec::new();
    for rec in &run.radar_tracks {
        if let Some(truth) = id.truth_of.get(&rec.key) {
            if let Some(f) = first.get(truth) {
                v.push((rec.confirm_ns - f) as f64 / 1.0e9);
            }
        }
    }
    v
}

/// Position error of the raw plots (no filtering, no registration) against the exact truth at
/// the plot time, split straight and turning as in `score::esassp_of`: horizontal error from the
/// plot's polar position converted at its own Mode C altitude.
pub fn raw_plot_errors(truth: &Truth, radar: &StudyRadar, plots: &[RadarPlot]) -> [Stats; 3] {
    let g = radar_geom(&radar.trk);
    let mut cls: [Vec<f64>; 3] = Default::default();
    for p in plots {
        if p.label == u32::MAX || p.alt_ft == NONE_ALT {
            continue;
        }
        let Ok(c) = plot_convert(&g, &radar.trk, p, &NO_BIAS, None, 1) else {
            continue;
        };
        let t_ms = p.time_ns.div_euclid(1_000_000);
        let TruthQuery::Ok(s) = truth_at(truth, p.label, t_ms) else {
            continue;
        };
        let (e, n) = enu(s.lat_deg, s.lon_deg, c.geo.lat_deg, c.geo.lon_deg);
        let err = oa_math::hypot(e, n);
        let q = |dt: i64| match truth_at(truth, p.label, t_ms + dt) {
            TruthQuery::Ok(s) => Some((s.vel_east_mps, s.vel_north_mps)),
            _ => None,
        };
        let class = match (q(-TURN_SPAN_MS), q(TURN_SPAN_MS)) {
            (Some(a), Some(b)) => {
                let acc = transversal_accel(a, b, 2.0 * TURN_SPAN_MS as f64 / 1000.0);
                Some(if acc > TURN_ACCEL_MPS2 { 2 } else { 1 })
            }
            _ => None,
        };
        cls[0].push(err);
        if let Some(k) = class {
            if let Some(v) = cls.get_mut(k) {
                v.push(err);
            }
        }
    }
    cls.each_ref().map(|v| stats_of(v))
}

/// Number of simulator plots of a kind.
pub fn count_kind(kinds: &[u8], kind: u8) -> usize {
    kinds.iter().filter(|&&k| k == kind).count()
}

/// Plots that are false or split, for the report.
pub fn false_and_split(kinds: &[u8]) -> (usize, usize) {
    (count_kind(kinds, KIND_FALSE), count_kind(kinds, KIND_SPLIT))
}

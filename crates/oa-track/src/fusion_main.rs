#![forbid(unsafe_code)]
// The study indexes fixed three-element class arrays (all, straight, turning) with c in 0..3.
#![allow(clippy::indexing_slicing)]
//! `oa-track-fusion`: the exact-truth study of radar and ADS-B fusion with registration
//! (docs/M3_DESIGN.md build order step 2). Shell (B3): arguments, files, the clock and stdout.
//!
//! ```text
//! oa-track-fusion --out <dir> [--clip <clip.oaf>] [--worlds N] [--pair-seeds N] [--quick|--smoke]
//!                 [--only base|sweep|pairs|false|clip]
//! ```
//!
//! Simulates plots (and ADS-B) from exact truths, runs the baselines and our tracker on identical
//! inputs, scores every run against the same truth the simulator used, and writes `report.txt`
//! and `tables.csv`. Everything it prints is labelled with the truth it is scored against, the
//! truth's own error (zero: the simulator treats it as exact) and the baseline beside it.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use oa_bench::adsb_truth::BuiltTruth;
use oa_bench::stats::Stats;
use oa_sensorsim::profile::{RadarProfile, MSSR_EUROCONTROL_1997};
use oa_track::fusion_score::{
    coverage_counts, false_and_split, identify, initiation_delays_s, make_radar, raw_plot_errors,
    run_variant, score_run, simulate_adsb, simulate_plots, timing_summary, Ident, Kind, RunOut,
    StudyRadar, Table, Variant, ADSB_STUDY, SPLIT_DAZ_DEG, SPLIT_DR_M, V_BASE_Q1, V_BASE_Q5,
    V_OURS_NOREG, V_OURS_REG,
};
use oa_track::fusion_world::{base_world, clip_world, pair_world, PAIR_GEOMS};
use oa_track::radar::RadarPlot;
use oa_track::score::{CLASS_NAMES, CLASS_STRAIGHT, CLASS_TURN};

const USAGE: &str =
    "usage: oa-track-fusion --out <dir> [--clip <clip.oaf>] [--worlds N] [--pair-seeds N] [--quick|--smoke] [--only base|sweep|pairs|false|clip]";

/// ESASSP Volume 1 Table 4 (3 NM), Recommended column, as in docs/BENCHMARKS.md 3.1.
const R4_RMS_M: f64 = 210.0;
const R17_STRAIGHT_MPS: f64 = 4.0;
const R17_TURN_MPS: f64 = 8.0;
const R18_STRAIGHT_DEG: f64 = 10.0;
const R18_TURN_DEG: f64 = 25.0;

/// Bias values of the sweep: stated range, since no sourced value exists.
const SWEEP_RANGE_M: [f64; 3] = [0.0, 50.0, 150.0];
const SWEEP_AZ_DEG: [f64; 3] = [0.0, 0.05, 0.15];

/// Pooled sums of squares of one class of one error.
#[derive(Debug, Clone, Copy, Default)]
struct Pool {
    n: u64,
    ss: f64,
}

impl Pool {
    fn add(&mut self, s: &Stats) {
        self.n += s.n;
        self.ss += s.rms * s.rms * s.n as f64;
    }
    fn rms(&self) -> f64 {
        if self.n == 0 {
            f64::NAN
        } else {
            (self.ss / self.n as f64).sqrt()
        }
    }
}

/// Position, speed and heading pools by class (all, straight, turning). A7.2: cold struct.
#[derive(Debug, Clone, Copy, Default)]
struct Pools {
    pos: [Pool; 3],
    spd: [Pool; 3],
    hdg: [Pool; 3],
}

fn add_table(p: &mut Pools, t: &Table, ticks: bool) {
    let e = if ticks { &t.ticks } else { &t.updates };
    for c in 0..3 {
        p.pos[c].add(&e.position_m[c]);
        p.spd[c].add(&e.speed_mps[c]);
        p.hdg[c].add(&e.heading_deg[c]);
    }
}

/// Everything accumulated for one variant over several worlds.
#[derive(Debug, Clone, Default)]
struct Acc {
    pools: Pools,
    swaps: u64,
    wrong: u64,
    false_tracks: u64,
    dups: u64,
    false_applied: u64,
    aircraft: u64,
    tracked: u64,
    delays: Vec<f64>,
    us_mean: Vec<f64>,
    us_p50: Vec<f64>,
    us_p99: Vec<f64>,
    us_max: f64,
    events: u64,
    false_plots: u64,
    split_plots: u64,
    plots: u64,
}

struct Report {
    txt: String,
    csv: String,
}

fn fmt_rms(v: f64) -> String {
    if v.is_nan() {
        "   n/a".to_string()
    } else {
        format!("{v:6.1}")
    }
}

fn csv_field(value: &str) -> String {
    if value
        .bytes()
        .any(|b| matches!(b, b',' | b'"' | b'\r' | b'\n'))
    {
        let mut escaped = String::with_capacity(value.len() + 2);
        escaped.push('"');
        for c in value.chars() {
            if c == '"' {
                escaped.push('"');
            }
            escaped.push(c);
        }
        escaped.push('"');
        escaped
    } else {
        value.to_string()
    }
}

fn pass(v: f64, bound: f64) -> &'static str {
    if v.is_nan() {
        " "
    } else if v <= bound {
        "+"
    } else {
        "-"
    }
}

/// A table row: position (all, straight, turning), speed and heading RMS by class, with the
/// ESASSP pass mark (+ meets the 3 NM Recommended value, - does not).
fn esassp_row(label: &str, p: &Pools, rep: &mut Report, tag: &str) {
    let (pa, ps, pt) = (p.pos[0].rms(), p.pos[1].rms(), p.pos[2].rms());
    let (ss, st) = (p.spd[1].rms(), p.spd[2].rms());
    let (hs, ht) = (p.hdg[1].rms(), p.hdg[2].rms());
    let _ = writeln!(
        rep.txt,
        "  {label:<18} {:>6}  pos {}{} {} {}  | spd {}{} {}{}  | hdg {}{} {}{}",
        p.pos[0].n,
        fmt_rms(pa),
        pass(pa, R4_RMS_M),
        fmt_rms(ps),
        fmt_rms(pt),
        fmt_rms(ss),
        pass(ss, R17_STRAIGHT_MPS),
        fmt_rms(st),
        pass(st, R17_TURN_MPS),
        fmt_rms(hs),
        pass(hs, R18_STRAIGHT_DEG),
        fmt_rms(ht),
        pass(ht, R18_TURN_DEG),
    );
    let _ = writeln!(
        rep.csv,
        "{},{},{},{:.2},{:.2},{:.2},{:.3},{:.3},{:.3},{:.3}",
        csv_field(tag),
        csv_field(label),
        p.pos[0].n,
        pa,
        ps,
        pt,
        ss,
        st,
        hs,
        ht
    );
}

fn header(rep: &mut Report, title: &str) {
    let _ = writeln!(rep.txt, "\n{title}");
    let _ = writeln!(
        rep.txt,
        "  {:<18} {:>6}  {:<28}  | {:<14}  | {:<14}",
        "variant", "n", "position RMS m: all, straight, turn", "speed RMS m/s", "heading RMS deg"
    );
    let _ = writeln!(
        rep.txt,
        "  {:<18} {:>6}  {:<28}  | {:<14}  | {:<14}",
        "", "", "(R4 <= 210 all)", "(R17 <=4 / <=8)", "(R18 <=10 / <=25)"
    );
    let _ = writeln!(
        rep.txt,
        "  {:<18} {:>6}  {:<28}  | {:<14}  | {:<14}",
        "", "", "", "straight  turn", "straight  turn"
    );
}

/// Whether the aircraft is ADS-B equipped in mixed runs: even index of the synthetic base set
/// and even address (low bit clear) in a clip.
fn equipped(addr: u32) -> bool {
    addr & 1 == 0
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sensors {
    RadarOnly,
    Mixed,
}

/// One scored run's by-products.
struct Cell {
    run: RunOut,
    id: Ident,
    all: Table,
    eq: Option<Table>,
    rad: Option<Table>,
    plots: Vec<RadarPlot>,
    kinds: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn run_cell(
    built: &BuiltTruth,
    radar: &StudyRadar,
    sensors: Sensors,
    variant: &Variant,
    seed: u64,
    plots: &[RadarPlot],
    kinds: &[u8],
    adsb: &[oa_track::Measurement],
) -> Result<Cell, String> {
    let run = run_variant(variant, &[radar.trk], &[plots], adsb).map_err(|e| e.to_string())?;
    let id = identify(&run);
    let all = score_run(&run, &id, built, &|_| true)?;
    let (eq, rad) = if sensors == Sensors::Mixed {
        (
            Some(score_run(&run, &id, built, &|a| equipped(a))?),
            Some(score_run(&run, &id, built, &|a| !equipped(a))?),
        )
    } else {
        (None, None)
    };
    let _ = seed;
    Ok(Cell {
        run,
        id,
        all,
        eq,
        rad,
        plots: plots.to_vec(),
        kinds: kinds.to_vec(),
    })
}

fn absorb(acc: &mut Acc, c: &Cell, sel: u8) {
    let t = match sel {
        1 => c.eq.as_ref(),
        2 => c.rad.as_ref(),
        _ => Some(&c.all),
    };
    if let Some(t) = t {
        add_table(&mut acc.pools, t, false);
    }
    acc.swaps += c.id.swaps;
    acc.wrong += c.id.wrong_plots;
    acc.false_tracks += c.id.false_tracks;
    acc.dups += c.id.duplicates;
    acc.false_applied += c.id.false_applied;
    let (seen, tracked) = coverage_counts(&c.id, &[&c.plots]);
    acc.aircraft += seen as u64;
    acc.tracked += tracked as u64;
    acc.delays
        .extend(initiation_delays_s(&c.run, &c.id, &[&c.plots]));
    let (mean, p50, p99, max) = timing_summary(&c.run.timing);
    acc.us_mean.push(mean);
    acc.us_p50.push(p50);
    acc.us_p99.push(p99);
    acc.us_max = acc.us_max.max(max);
    acc.events += c.run.timing.events;
    let (f, s) = false_and_split(&c.kinds);
    acc.false_plots += f as u64;
    acc.split_plots += s as u64;
    acc.plots += c.plots.len() as u64;
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        f64::NAN
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let k = ((p * s.len() as f64).ceil() as usize).clamp(1, s.len());
    s.get(k - 1).copied().unwrap_or(f64::NAN)
}

struct Opts {
    worlds: u64,
    pair_seeds: u64,
    quick: bool,
    /// Run only this section (`base`, `sweep`, `pairs`, `false`, `clip`).
    only: Option<String>,
    /// Terminal radar only (the smoke test of the binary).
    terminal_only: bool,
}

fn wants(o: &Opts, section: &str) -> bool {
    o.only.as_deref().is_none_or(|s| s == section)
}

fn profile_with_false(p: RadarProfile, rate: f64) -> RadarProfile {
    RadarProfile {
        false_rate: rate,
        ..p
    }
}

/// Section 1: the base worlds, radar-only and mixed, for every variant, at one bias.
fn section_base(
    rep: &mut Report,
    kind: Kind,
    sensors: Sensors,
    bias: (f64, f64),
    opts: &Opts,
    tag: &str,
) -> Result<(), String> {
    let (radar_name, period) = match kind {
        Kind::Terminal => ("terminal 4.8 s", 4.8),
        Kind::EnRoute => ("en route 12 s", 12.0),
    };
    let variants: Vec<Variant> = if sensors == Sensors::Mixed {
        vec![V_BASE_Q1, V_BASE_Q5, V_OURS_NOREG, V_OURS_REG]
    } else {
        vec![V_BASE_Q1, V_BASE_Q5, V_OURS_NOREG]
    };
    let subsets: &[(u8, &str)] = if sensors == Sensors::Mixed {
        &[(0, "all"), (1, "ADS-B equipped"), (2, "radar-only")]
    } else {
        &[(0, "all")]
    };
    let mut accs: Vec<Vec<Acc>> = vec![vec![Acc::default(); subsets.len()]; variants.len()];
    let mut raw = Pools::default();
    let mut reg_lines: Vec<String> = Vec::new();
    let mut note_plots = (0u64, 0u64, 0u64);
    for w in 0..opts.worlds {
        let built = base_world(kind, 1 + w)?;
        let radar = make_radar(kind, MSSR_EUROCONTROL_1997, bias.0, bias.1);
        let first = oa_truth::leg_start_ns(&built.truth, oa_truth::LegIx(0)).unwrap_or(0);
        let (plots, kinds, _st) = simulate_plots(
            &built.truth,
            &radar,
            100 + w,
            first - 1_000_000_000,
            first + 1_000_000_000_000,
        )?;
        let adsb = if sensors == Sensors::Mixed {
            simulate_adsb(
                &built.truth,
                &ADSB_STUDY,
                200 + w,
                first,
                first + 1_000_000_000_000,
                &equipped,
            )?
        } else {
            Vec::new()
        };
        let rp = raw_plot_errors(&built.truth, &radar, &plots);
        for (c, s) in rp.iter().enumerate() {
            raw.pos[c].add(s);
        }
        for (vi, v) in variants.iter().enumerate() {
            let cell = run_cell(&built, &radar, sensors, v, w, &plots, &kinds, &adsb)?;
            for (si, (sel, _)) in subsets.iter().enumerate() {
                if let Some(a) = accs.get_mut(vi).and_then(|r| r.get_mut(si)) {
                    absorb(a, &cell, *sel);
                }
            }
            if sensors == Sensors::Mixed && v.registration {
                if let Some((st, hist)) = cell.run.reg.first() {
                    reg_lines.push(format!(
                        "    world {}: est range {:+.1} m (sigma {:.1}), az {:+.4} deg (sigma {:.4}); true {:+.1} m, {:+.4} deg; used {} of {} eligible-or-rejected residuals ({} awaiting references, {} disagreeing with the other references, {} ignored for want of a reference slot, {} restarts on a shift); {} published estimates",
                        1 + w,
                        st.range_m,
                        st.var_range_m2.sqrt(),
                        st.az_deg,
                        st.var_az_deg2.sqrt(),
                        bias.0,
                        bias.1,
                        st.used,
                        st.used + st.rejected,
                        st.building,
                        st.inconsistent,
                        st.ignored,
                        st.resets,
                        hist.len()
                    ));
                    let fs = &cell.run.fstats;
                    let flagged = cell
                        .run
                        .out
                        .flags
                        .iter()
                        .filter(|f| **f & oa_track::O_RADAR_DISAGREES != 0)
                        .count();
                    reg_lines.push(format!(
                        "    world {}: radar contradicts an ADS-B track: {} Mode S plots outside the wide gate of their address's track ({} marked a live track), {} output rows carry the flag",
                        1 + w,
                        fs.hard_mismatch,
                        fs.radar_disagree,
                        flagged
                    ));
                }
            }
        }
        note_plots = (
            note_plots.0 + plots.len() as u64,
            note_plots.1 + count_true(&plots),
            note_plots.2 + built.truth.ac_addr.len() as u64,
        );
    }
    let sens = if sensors == Sensors::Mixed {
        "radar + ADS-B (NACp 10, 1 Hz, half the aircraft)"
    } else {
        "radar only"
    };
    for (si, (_, sname)) in subsets.iter().enumerate() {
        header(
            rep,
            &format!(
                "{tag}  {radar_name}, {sens}, bias {:+.0} m / {:+.2} deg, aircraft: {sname}  [{} worlds, {} plots, {} aircraft]",
                bias.0, bias.1, opts.worlds, note_plots.0, note_plots.2
            ),
        );
        for (vi, v) in variants.iter().enumerate() {
            if let Some(a) = accs.get(vi).and_then(|r| r.get(si)) {
                esassp_row(v.name, &a.pools, rep, &format!("{tag}|{sname}"));
            }
        }
        if si == 0 {
            esassp_row("raw plots (no tracker)", &raw, rep, &format!("{tag}|raw"));
        }
        // No RMS table stands alone: the run's duplicate and false tracks go beside it.
        let counts: Vec<String> = variants
            .iter()
            .enumerate()
            .filter_map(|(vi, v)| {
                let a = accs.get(vi).and_then(|r| r.first())?;
                Some(format!("{} {}/{}", &v.name[..2], a.dups, a.false_tracks))
            })
            .collect();
        let _ = writeln!(
            rep.txt,
            "  duplicate / false tracks of the same runs (dupT/falseT, all aircraft, not only those scored above): {}",
            counts.join(", ")
        );
    }
    let _ = writeln!(
        rep.txt,
        "  per-run identity and cost (all aircraft, summed over worlds):"
    );
    let _ = writeln!(
        rep.txt,
        "    {:<18} {:>5} {:>6} {:>6} {:>6} {:>5} {:>7} | init delay s (mean/p95) | us/input mean/p50/p99/max",
        "variant", "swaps", "wrongP", "falseT", "dupT", "missd", "falseP"
    );
    for (vi, v) in variants.iter().enumerate() {
        if let Some(a) = accs.get(vi).and_then(|r| r.first()) {
            let _ = writeln!(
                rep.txt,
                "    {:<18} {:>5} {:>6} {:>6} {:>6} {:>5} {:>7} | {:>7.1} / {:>7.1}        | {:.1} / {:.1} / {:.1} / {:.1}",
                v.name,
                a.swaps,
                a.wrong,
                a.false_tracks,
                a.dups,
                a.aircraft - a.tracked,
                a.false_plots,
                mean(&a.delays),
                pct(&a.delays, 0.95),
                mean(&a.us_mean),
                mean(&a.us_p50),
                mean(&a.us_p99),
                a.us_max,
            );
            let _ = writeln!(
                rep.csv,
                "{},{},{},{},{},{},{},{:.2},{:.2},{:.2},{:.2}",
                csv_field(&format!("{tag}|id")),
                csv_field(v.name),
                a.swaps,
                a.wrong,
                a.false_tracks,
                a.dups,
                a.aircraft - a.tracked,
                mean(&a.delays),
                pct(&a.delays, 0.95),
                mean(&a.us_mean),
                a.us_max
            );
        }
    }
    let _ = writeln!(
        rep.txt,
        "    ({} true-plot aircraft; plot period {period} s; false plots here are the profile's 0.1 %, so none is expected to start a track)",
        note_plots.2
    );
    for l in reg_lines {
        let _ = writeln!(rep.txt, "{l}");
    }
    Ok(())
}

fn count_true(plots: &[RadarPlot]) -> u64 {
    plots.iter().filter(|p| p.label != u32::MAX).count() as u64
}

/// Section 2: the bias sweep for one scenario (one world), position RMS per variant and the
/// registration estimate error.
fn section_sweep(
    rep: &mut Report,
    kind: Kind,
    sensors: Sensors,
    opts: &Opts,
) -> Result<(), String> {
    let name = match (kind, sensors) {
        (Kind::Terminal, Sensors::RadarOnly) => "R-T radar-only terminal",
        (Kind::EnRoute, Sensors::RadarOnly) => "R-E radar-only en route",
        (Kind::Terminal, Sensors::Mixed) => "M-T mixed terminal",
        (Kind::EnRoute, Sensors::Mixed) => "M-E mixed en route",
    };
    let variants: Vec<Variant> = if sensors == Sensors::Mixed {
        vec![V_BASE_Q1, V_OURS_NOREG, V_OURS_REG]
    } else {
        vec![V_BASE_Q1, V_OURS_NOREG]
    };
    let _ = writeln!(
        rep.txt,
        "\nBIAS SWEEP  {name}  (1 world, position RMS m over all aircraft, each with its duplicate / false track counts dupT/falseT{}; truth exact)",
        if sensors == Sensors::Mixed {
            "; then radar-only aircraft only"
        } else {
            ""
        }
    );
    let mut line = format!("  {:>8} {:>8}", "rng bias", "az bias");
    for v in &variants {
        let _ = write!(
            line,
            " | {:>22}",
            format!("{} (RMS dupT/falseT)", &v.name[..2])
        );
    }
    if sensors == Sensors::Mixed {
        for v in &variants {
            let _ = write!(line, " | rad-only {:>5}", &v.name[..2]);
        }
        let _ = write!(line, " | est err range m / az deg (sigma)");
    }
    let _ = writeln!(rep.txt, "{line}");
    let built = base_world(kind, 1)?;
    let first = oa_truth::leg_start_ns(&built.truth, oa_truth::LegIx(0)).unwrap_or(0);
    let _ = opts;
    for &br in &SWEEP_RANGE_M {
        for &ba in &SWEEP_AZ_DEG {
            let radar = make_radar(kind, MSSR_EUROCONTROL_1997, br, ba);
            let (plots, kinds, _) = simulate_plots(
                &built.truth,
                &radar,
                300,
                first - 1_000_000_000,
                first + 1_000_000_000_000,
            )?;
            let adsb = if sensors == Sensors::Mixed {
                simulate_adsb(
                    &built.truth,
                    &ADSB_STUDY,
                    301,
                    first,
                    first + 1_000_000_000_000,
                    &equipped,
                )?
            } else {
                Vec::new()
            };
            let mut line = format!("  {br:>8.0} {ba:>8.2}");
            let mut rad_cols = String::new();
            let mut est = String::new();
            for v in &variants {
                let c = run_cell(&built, &radar, sensors, v, 0, &plots, &kinds, &adsb)?;
                let mut p = Pools::default();
                add_table(&mut p, &c.all, false);
                let _ = write!(
                    line,
                    " | {:>22}",
                    format!(
                        "{} {}/{}",
                        fmt_rms(p.pos[0].rms()),
                        c.id.duplicates,
                        c.id.false_tracks
                    )
                );
                if let Some(r) = &c.rad {
                    let mut pr = Pools::default();
                    add_table(&mut pr, r, false);
                    let _ = write!(rad_cols, " | {:>14}", fmt_rms(pr.pos[0].rms()));
                }
                if v.registration {
                    if let Some((st, _)) = c.run.reg.first() {
                        let _ = write!(
                            est,
                            " | {:+6.1} ({:.1}) / {:+.4} ({:.4})",
                            st.range_m - br,
                            st.var_range_m2.sqrt(),
                            st.az_deg - ba,
                            st.var_az_deg2.sqrt()
                        );
                    }
                }
                let _ = writeln!(
                    rep.csv,
                    "{},{},{br},{ba},{:.2},{},{}",
                    csv_field(&format!("sweep|{name}")),
                    csv_field(v.name),
                    p.pos[0].rms(),
                    c.id.duplicates,
                    c.id.false_tracks
                );
            }
            let _ = writeln!(rep.txt, "{line}{rad_cols}{est}");
        }
    }
    Ok(())
}

/// Section 3: pairs. Totals over seeds, per geometry and variant.
fn section_pairs(rep: &mut Report, kind: Kind, opts: &Opts) -> Result<(), String> {
    let (kname, period) = match kind {
        Kind::Terminal => ("terminal 4.8 s", 4.8),
        Kind::EnRoute => ("en route 12 s", 12.0),
    };
    let _ = writeln!(
        rep.txt,
        "\nPAIRS  {kname}, plot noise per seed, {} seeds per geometry, MSSR profile, bias 0, radar-only anonymous plots; counts are totals over the seeds",
        opts.pair_seeds
    );
    let _ = writeln!(
        rep.txt,
        "  swaps = label changes inside a track; wrongP = plots of the other aircraft applied; lost = aircraft with no primary track; dupT = second tracks; falseT = tracks of false plots"
    );
    let variants = [V_BASE_Q1, V_BASE_Q5, V_OURS_NOREG];
    let _ = writeln!(
        rep.txt,
        "  {:<40} {:<16} {:>6} {:>6} {:>5} {:>5} {:>6}",
        "geometry", "variant", "swaps", "wrongP", "lost", "dupT", "falseT"
    );
    let radar = make_radar(kind, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let _ = period;
    for g in &PAIR_GEOMS {
        let built = pair_world(kind, g)?;
        let first = oa_truth::leg_start_ns(&built.truth, oa_truth::LegIx(0)).unwrap_or(0);
        let mut tot = vec![(0u64, 0u64, 0u64, 0u64, 0u64); variants.len()];
        for seed in 0..opts.pair_seeds {
            let (plots, _k, _) = simulate_plots(
                &built.truth,
                &radar,
                1_000 + seed,
                first - 1_000_000_000,
                first + 1_000_000_000_000,
            )?;
            for (vi, v) in variants.iter().enumerate() {
                let run =
                    run_variant(v, &[radar.trk], &[&plots], &[]).map_err(|e| e.to_string())?;
                let id = identify(&run);
                let (seen, tracked) = coverage_counts(&id, &[&plots]);
                if let Some(t) = tot.get_mut(vi) {
                    t.0 += id.swaps;
                    t.1 += id.wrong_plots;
                    t.2 += (seen - tracked) as u64;
                    t.3 += id.duplicates;
                    t.4 += id.false_tracks;
                }
            }
        }
        for (vi, v) in variants.iter().enumerate() {
            if let Some(t) = tot.get(vi) {
                let _ = writeln!(
                    rep.txt,
                    "  {:<40} {:<16} {:>6} {:>6} {:>5} {:>5} {:>6}",
                    if vi == 0 { g.name } else { "" },
                    v.name,
                    t.0,
                    t.1,
                    t.2,
                    t.3,
                    t.4
                );
                let _ = writeln!(
                    rep.csv,
                    "{},{},{},{},{},{},{},{}",
                    csv_field(&format!("pairs|{kname}")),
                    csv_field(g.name),
                    csv_field(v.name),
                    t.0,
                    t.1,
                    t.2,
                    t.3,
                    t.4
                );
            }
        }
    }
    // One aircraft with ADS-B, the other radar-only, for the pairs with a separation of at least
    // 3 NM or 1,000 ft: the radar-only aircraft must not be taken by the ADS-B track.
    let _ = writeln!(
        rep.txt,
        "\nPAIRS  {kname}, mixed: aircraft 1 ADS-B equipped (NACp 10), aircraft 2 radar-only, same seeds"
    );
    let _ = writeln!(
        rep.txt,
        "  {:<40} {:<16} {:>6} {:>6} {:>5} {:>5} {:>6}",
        "geometry", "variant", "swaps", "wrongP", "lost", "dupT", "falseT"
    );
    let mixed_variants = [V_BASE_Q1, V_OURS_NOREG, V_OURS_REG];
    for g in PAIR_GEOMS.iter().filter(|g| !g.name.starts_with("STRESS")) {
        let built = pair_world(kind, g)?;
        let first = oa_truth::leg_start_ns(&built.truth, oa_truth::LegIx(0)).unwrap_or(0);
        let mut tot = vec![(0u64, 0u64, 0u64, 0u64, 0u64); mixed_variants.len()];
        for seed in 0..opts.pair_seeds {
            let (plots, _k, _) = simulate_plots(
                &built.truth,
                &radar,
                2_000 + seed,
                first - 1_000_000_000,
                first + 1_000_000_000_000,
            )?;
            let adsb = simulate_adsb(
                &built.truth,
                &ADSB_STUDY,
                3_000 + seed,
                first,
                first + 1_000_000_000_000,
                &|a| a == 0xA0_0001,
            )?;
            for (vi, v) in mixed_variants.iter().enumerate() {
                let run =
                    run_variant(v, &[radar.trk], &[&plots], &adsb).map_err(|e| e.to_string())?;
                let id = identify(&run);
                let (seen, tracked) = coverage_counts(&id, &[&plots]);
                if let Some(t) = tot.get_mut(vi) {
                    t.0 += id.swaps;
                    t.1 += id.wrong_plots;
                    t.2 += (seen - tracked) as u64;
                    t.3 += id.duplicates;
                    t.4 += id.false_tracks;
                }
            }
        }
        for (vi, v) in mixed_variants.iter().enumerate() {
            if let Some(t) = tot.get(vi) {
                let _ = writeln!(
                    rep.txt,
                    "  {:<40} {:<16} {:>6} {:>6} {:>5} {:>5} {:>6}",
                    if vi == 0 { g.name } else { "" },
                    v.name,
                    t.0,
                    t.1,
                    t.2,
                    t.3,
                    t.4
                );
                let _ = writeln!(
                    rep.csv,
                    "{},{},{},{},{},{},{},{}",
                    csv_field(&format!("pairs-mixed|{kname}")),
                    csv_field(g.name),
                    csv_field(v.name),
                    t.0,
                    t.1,
                    t.2,
                    t.3,
                    t.4
                );
            }
        }
    }
    Ok(())
}

/// Section 4: false plots. The nominal rate and a stress rate (5 % of true plots, a stress value,
/// not a sourced one), radar-only base world.
fn section_false(rep: &mut Report, kind: Kind, opts: &Opts) -> Result<(), String> {
    let kname = match kind {
        Kind::Terminal => "terminal",
        Kind::EnRoute => "en route",
    };
    let _ = writeln!(
        rep.txt,
        "\nFALSE TRACKS  {kname}, radar-only base world, bias 0: false plots uniform over the coverage disc, one per scan per true plot at the stated rate"
    );
    let _ = writeln!(
        rep.txt,
        "  {:<26} {:<16} {:>8} {:>7} {:>9} {:>8}",
        "false-plot rate", "variant", "falseP", "falseT", "confirmed", "swaps"
    );
    for (rate, label) in [(0.001, "0.1 % (SURICG/6 bound)"), (0.05, "5 % (stress)")] {
        let prof = profile_with_false(MSSR_EUROCONTROL_1997, rate);
        let built = base_world(kind, 1)?;
        let first = oa_truth::leg_start_ns(&built.truth, oa_truth::LegIx(0)).unwrap_or(0);
        let radar = make_radar(kind, prof, 0.0, 0.0);
        let _ = opts;
        let (plots, kinds, _) = simulate_plots(
            &built.truth,
            &radar,
            400,
            first - 1_000_000_000,
            first + 1_000_000_000_000,
        )?;
        for v in [V_BASE_Q1, V_OURS_NOREG] {
            let run = run_variant(&v, &[radar.trk], &[&plots], &[]).map_err(|e| e.to_string())?;
            let id = identify(&run);
            let (f, _) = false_and_split(&kinds);
            let _ = writeln!(
                rep.txt,
                "  {:<26} {:<16} {:>8} {:>7} {:>9} {:>8}",
                label, v.name, f, id.false_tracks, run.fstats.cand_confirmed, id.swaps
            );
            let _ = writeln!(
                rep.csv,
                "{},{},{},{f},{},{}",
                csv_field(&format!("false|{kname}")),
                csv_field(label),
                csv_field(v.name),
                id.false_tracks,
                run.fstats.cand_confirmed
            );
        }
    }
    Ok(())
}

/// Section 5: the clip's shapes.
fn section_clip(rep: &mut Report, path: &str) -> Result<(), String> {
    let t0 = Instant::now();
    let built = clip_world(path)?;
    let aircraft = built.truth.ac_addr.len();
    let _ = writeln!(
        rep.txt,
        "\nREAL-CLIP SHAPES  {path}: {aircraft} aircraft, {} legs built by oa-truth in {:.1} s; truth = oa-truth's trajectories, used only as shapes and treated as exact by the simulator and the scorer (not an accuracy claim about the real flights, and not a field validation of the radar model)",
        built.legs.len(),
        t0.elapsed().as_secs_f64()
    );
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for l in 0..built.legs.len() {
        if let (Some(a), Some(b)) = (
            oa_truth::leg_start_ns(&built.truth, oa_truth::LegIx(l as u32)),
            oa_truth::leg_end_ns(&built.truth, oa_truth::LegIx(l as u32)),
        ) {
            lo = lo.min(a);
            hi = hi.max(b);
        }
    }
    for (kind, sensors, bias) in [
        (Kind::Terminal, Sensors::RadarOnly, (0.0, 0.0)),
        (Kind::EnRoute, Sensors::RadarOnly, (0.0, 0.0)),
        (Kind::Terminal, Sensors::Mixed, (50.0, 0.05)),
        (Kind::EnRoute, Sensors::Mixed, (50.0, 0.05)),
    ] {
        let radar = make_radar(kind, MSSR_EUROCONTROL_1997, bias.0, bias.1);
        let t1 = Instant::now();
        let (plots, kinds, st) = simulate_plots(&built.truth, &radar, 500, lo, hi + 1)?;
        let adsb = if sensors == Sensors::Mixed {
            simulate_adsb(&built.truth, &ADSB_STUDY, 501, lo, hi, &equipped)?
        } else {
            Vec::new()
        };
        let variants: Vec<Variant> = if sensors == Sensors::Mixed {
            vec![V_BASE_Q1, V_BASE_Q5, V_OURS_NOREG, V_OURS_REG]
        } else {
            vec![V_BASE_Q1, V_BASE_Q5, V_OURS_NOREG]
        };
        let tag = format!(
            "CLIP {} {}",
            if kind == Kind::Terminal {
                "terminal"
            } else {
                "en route"
            },
            if sensors == Sensors::Mixed {
                "mixed"
            } else {
                "radar-only"
            }
        );
        let subsets: &[(u8, &str)] = if sensors == Sensors::Mixed {
            &[(0, "all"), (1, "ADS-B equipped"), (2, "radar-only")]
        } else {
            &[(0, "all")]
        };
        let mut raw = Pools::default();
        for (c, s) in raw_plot_errors(&built.truth, &radar, &plots)
            .iter()
            .enumerate()
        {
            raw.pos[c].add(s);
        }
        let mut cells = Vec::new();
        for v in &variants {
            cells.push(run_cell(
                &built, &radar, sensors, v, 0, &plots, &kinds, &adsb,
            )?);
        }
        for (si, (sel, sname)) in subsets.iter().enumerate() {
            header(
                rep,
                &format!(
                    "{tag}, bias {:+.0} m / {:+.2} deg, aircraft: {sname}  [{} plots, {} detected of {} in coverage, simulated in {:.1} s]",
                    bias.0, bias.1, plots.len(), st.detected, st.in_coverage, t1.elapsed().as_secs_f64()
                ),
            );
            for (v, c) in variants.iter().zip(cells.iter()) {
                let mut acc = Acc::default();
                absorb(&mut acc, c, *sel);
                esassp_row(v.name, &acc.pools, rep, &format!("{tag}|{sname}"));
            }
            if si == 0 {
                esassp_row("raw plots (no tracker)", &raw, rep, &format!("{tag}|raw"));
            }
            let counts: Vec<String> = variants
                .iter()
                .zip(cells.iter())
                .map(|(v, c)| format!("{} {}/{}", &v.name[..2], c.id.duplicates, c.id.false_tracks))
                .collect();
            let _ = writeln!(
                rep.txt,
                "  duplicate / false tracks of the same runs (dupT/falseT, all aircraft, not only those scored above): {}",
                counts.join(", ")
            );
        }
        let _ = writeln!(
            rep.txt,
            "    {:<18} {:>5} {:>6} {:>6} {:>6} {:>5} | us/input mean/p50/p99/max",
            "variant", "swaps", "wrongP", "falseT", "dupT", "missd"
        );
        for (v, c) in variants.iter().zip(cells.iter()) {
            let (seen, tracked) = coverage_counts(&c.id, &[&c.plots]);
            let (mean_us, p50, p99, max) = timing_summary(&c.run.timing);
            let _ = writeln!(
                rep.txt,
                "    {:<18} {:>5} {:>6} {:>6} {:>6} {:>5} | {mean_us:.1} / {p50:.1} / {p99:.1} / {max:.1}  ({} inputs)",
                v.name,
                c.id.swaps,
                c.id.wrong_plots,
                c.id.false_tracks,
                c.id.duplicates,
                seen - tracked,
                c.run.timing.events
            );
        }
    }
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let mut out: Option<PathBuf> = None;
    let mut clip: Option<String> = None;
    let (mut worlds, mut pair_seeds): (Option<u64>, Option<u64>) = (None, None);
    let mut opts = Opts {
        worlds: 3,
        pair_seeds: 20,
        quick: false,
        only: None,
        terminal_only: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--quick" => opts.quick = true,
            "--smoke" => {
                opts.quick = true;
                opts.terminal_only = true;
            }
            "--only" => opts.only = it.next().cloned(),
            "--out" => out = it.next().map(PathBuf::from),
            "--clip" => clip = it.next().cloned(),
            "--worlds" => {
                worlds = Some(
                    it.next()
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| USAGE.to_string())?,
                )
            }
            "--pair-seeds" => {
                pair_seeds = Some(
                    it.next()
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| USAGE.to_string())?,
                )
            }
            _ => return Err(USAGE.to_string()),
        }
    }
    let out = out.ok_or_else(|| USAGE.to_string())?;
    if opts.quick {
        opts.worlds = 1;
        opts.pair_seeds = 3;
    }
    opts.worlds = worlds.unwrap_or(opts.worlds);
    opts.pair_seeds = pair_seeds.unwrap_or(opts.pair_seeds);
    let started = Instant::now();
    let mut rep = Report {
        txt: String::new(),
        csv: String::from("section,label,a,b,c,d,e,f,g,h,i,j\n"),
    };
    let _ = writeln!(
        rep.txt,
        "OpenATCA M3 step 2: radar and ADS-B fusion with registration, scored against EXACT truth\n\
\n\
Truth: every number below is scored against the very trajectories the simulator made the plots (and ADS-B)\n\
from; oa_truth::truth_at is the single source for both, so the truth's own error is zero by construction.\n\
Synthetic worlds: smooth analytic paths (straight, 1.5/3/6 deg/s turns, +-1,500 fpm climbs), interpolation\n\
off the path by <1 cm (<0.5 m across the knot where a turn starts or stops). Clip worlds: oa-truth\n\
trajectories used as shapes only. Output representation limits: 1e-7 deg positions (1.1 cm), whole-ms\n\
pairing (plot and ADS-B times rounded to ms), 0.1 kt velocity (0.05 m/s).\n\
Sensor model (inputs, not results): MSSR 70 m / 0.08 deg, Mode C 100 ft, Pd 97 %, false 0.1 %, split 0.3 %\n\
(NLR-CR-2004-472; ICAO SURICG/6 IP/04); scans 4.8 s terminal (O'Hare ARP), 12 s en route (Peoria ARP);\n\
split offsets {SPLIT_DR_M} m / {SPLIT_DAZ_DEG} deg are a stress choice. ADS-B: NACp 10 (95 % bound 10 m), NACv 2, 1 Hz, white noise\n\
at the bound (real error is correlated over seconds), half the aircraft equipped (even address).\n\
No sourced radar bias exists: biases are swept, never picked. Registration prior is an assumption (250 m / 0.25 deg).\n\
Baselines on identical plots: a1 = one nearly-constant-velocity Kalman filter (q = 1 m^2/s^3) with plain nearest-\n\
neighbour association, no registration; a3 = same with q = 5 (tuned for turns); b = our tracker (IMM, exact GNN,\n\
ambiguity hold, address hard constraint) without registration; c = b with registration.\n\
ESASSP bars (EUROCONTROL-SPEC-0147 Ed 1.3 Vol 1 Table 4, 3 NM Recommended): R4 position RMS <= 210 m, R17 speed\n\
<= 4 m/s straight / 8 turning, R18 heading <= 10 deg straight / 25 turning. Straight/turning by ESASSP 4.2.14\n\
(transversal acceleration 1.5 m/s^2). '+' marks a value meeting its bar, '-' missing it. Scoring rows are the\n\
track state at each applied report (the closest to ESASSP's per-report errors), coasting rows excluded.\n\
These synthetic runs are far below ESASSP's 36,000-interval sample; they are an accuracy comparison, not an\n\
ESASSP exit result.\n\
Radar-only keys are source-local tracking hypotheses with unconfirmed identity; several keys may\n\
describe one aircraft. Duplicate hypothesis counts remain in the tables."
    );
    let kinds: &[Kind] = if opts.terminal_only {
        &[Kind::Terminal]
    } else {
        &[Kind::Terminal, Kind::EnRoute]
    };
    if wants(&opts, "base") {
        for &kind in kinds {
            eprintln!("base {kind:?} ({:.0} s)", started.elapsed().as_secs_f64());
            section_base(&mut rep, kind, Sensors::RadarOnly, (0.0, 0.0), &opts, "R")?;
            section_base(&mut rep, kind, Sensors::RadarOnly, (50.0, 0.05), &opts, "R")?;
            section_base(&mut rep, kind, Sensors::Mixed, (0.0, 0.0), &opts, "M")?;
            section_base(&mut rep, kind, Sensors::Mixed, (50.0, 0.05), &opts, "M")?;
            section_base(&mut rep, kind, Sensors::Mixed, (150.0, 0.15), &opts, "M")?;
        }
    }
    if wants(&opts, "sweep") {
        for &kind in kinds {
            for s in [Sensors::RadarOnly, Sensors::Mixed] {
                eprintln!("sweep {kind:?} ({:.0} s)", started.elapsed().as_secs_f64());
                section_sweep(&mut rep, kind, s, &opts)?;
            }
        }
    }
    if wants(&opts, "pairs") || wants(&opts, "false") {
        for &kind in kinds {
            if wants(&opts, "pairs") {
                eprintln!("pairs {kind:?} ({:.0} s)", started.elapsed().as_secs_f64());
                section_pairs(&mut rep, kind, &opts)?;
            }
            if wants(&opts, "false") {
                section_false(&mut rep, kind, &opts)?;
            }
        }
    }
    if let (Some(c), true) = (clip, wants(&opts, "clip")) {
        eprintln!("clip ({:.0} s)", started.elapsed().as_secs_f64());
        section_clip(&mut rep, &c)?;
    } else {
        let _ = writeln!(rep.txt, "\n(real-clip shapes not run)");
    }
    let _ = writeln!(
        rep.txt,
        "\nTotal wall time {:.1} s (simulation and scoring included).",
        started.elapsed().as_secs_f64()
    );
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    fs::write(out.join("report.txt"), &rep.txt).map_err(|e| e.to_string())?;
    fs::write(out.join("tables.csv"), &rep.csv).map_err(|e| e.to_string())?;
    print!("{}", rep.txt);
    let _ = (CLASS_NAMES, CLASS_STRAIGHT, CLASS_TURN);
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e == USAGE => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("oa-track-fusion: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod csv_tests {
    use super::{esassp_row, Pools, Report};

    // REQ: OA-TRK-065
    #[test]
    fn esassp_csv_quotes_text_fields_with_commas_quotes_and_newlines() {
        let mut report = Report {
            txt: String::new(),
            csv: String::new(),
        };
        esassp_row(
            "raw, \"plots\"\nsecond line",
            &Pools::default(),
            &mut report,
            "tag, quoted\nsection",
        );
        assert_eq!(
            report.csv,
            "\"tag, quoted\nsection\",\"raw, \"\"plots\"\"\nsecond line\",0,NaN,NaN,NaN,NaN,NaN,NaN,NaN\n"
        );
    }
}

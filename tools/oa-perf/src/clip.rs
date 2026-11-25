//! Per-update latency on the real clip: the clip's trajectories are the truth, ADS-B and radar
//! plots are simulated from them as `oa-track`'s own studies do, and every input is offered and
//! the layer advanced to its time, each such call timed. Shell code (reads the clip and the
//! clock).
//!
//! The fusion layer batches by one-second bucket (`fusion_params::BUCKET_NS`): an input is
//! applied when the advance that completes its bucket runs, so most calls only queue and the
//! call that crosses a bucket boundary does the work of the whole bucket. The run therefore
//! reports three views for fusion (the time of every call; the time of the calls that processed
//! inputs; and that time shared over the inputs processed, the way `oa-track`'s own timing
//! does) and, apart from fusion, the tracker alone with ADS-B only, where `tracker_update`
//! applies the input before it returns, so the time of the call is the whole latency.

use std::time::Instant;

use oa_sensorsim::profile::MSSR_EUROCONTROL_1997;
use oa_track::fusion::{
    fusion_adsb, fusion_advance, fusion_config, fusion_finish, fusion_new, fusion_plots,
    fusion_stats, fusion_tracker_config,
};
use oa_track::radar::{RadarPlot, RadarSite};
use oa_track::{
    output_clear, output_new, tracker_new, tracker_update, Measurement, TrackerConfig,
    DEFAULT_CONFIG,
};
use oa_track_tools::fusion_score::{make_radar, simulate_adsb, simulate_plots, Kind, ADSB_STUDY};
use oa_track_tools::fusion_world::clip_world_tallied;

use crate::stats::{dist_of, dist_weighted, Dist, Kv};
use crate::sys::{loadavg3, status_kb};

/// One clip latency run's settings.
#[derive(Debug, Clone)]
pub struct ClipCfg {
    pub path: String,
    /// Seconds of the clip, from its first leg.
    pub secs: i64,
    pub seed: u64,
}

/// The inputs of a run: sites and their plots, and the ADS-B.
pub struct ClipInputs {
    pub sites: Vec<RadarSite>,
    pub plots: Vec<Vec<RadarPlot>>,
    pub adsb: Vec<Measurement>,
    pub aircraft: usize,
}

/// The distributions of one pass over the inputs.
#[derive(Debug, Clone)]
pub struct PassResult {
    pub name: String,
    pub inputs: u64,
    pub calls: u64,
    pub calls_that_processed: u64,
    /// Every offer-and-advance call, microseconds.
    pub all_calls_us: Dist,
    /// The calls that processed inputs, microseconds (the whole batch).
    pub busy_calls_us: Dist,
    /// A processing call's time shared over the inputs it processed, microseconds, weighted by
    /// the inputs.
    pub per_input_us: Dist,
}

/// All passes of the clip run.
pub struct ClipResult {
    pub aircraft: usize,
    pub adsb_reports: usize,
    pub plots: Vec<usize>,
    pub clip_s: f64,
    pub passes: Vec<PassResult>,
    /// `tracker_update` per ADS-B report (no fusion), microseconds.
    pub tracker_update_us: Dist,
    pub hwm_kb: u64,
    pub load_start: String,
    pub load_end: String,
}

/// Builds the inputs from the clip. The rows and frames the clip reader dropped for their time
/// (a sealed window, OA-SEAL-014) are put in `sealed` as soon as the clip is read, so the caller
/// has the count on an error exit too.
pub fn clip_inputs(cfg: &ClipCfg, sealed: &mut u64) -> Result<ClipInputs, String> {
    let built = clip_world_tallied(&cfg.path, sealed)?;
    let (mut lo, mut hi) = (i64::MAX, i64::MIN);
    for l in 0..built.legs.len() {
        let leg = oa_truth::LegIx(l as u32);
        if let (Some(s), Some(e)) = (
            oa_truth::leg_start_ns(&built.truth, leg),
            oa_truth::leg_end_ns(&built.truth, leg),
        ) {
            lo = lo.min(s);
            hi = hi.max(e);
        }
    }
    if lo > hi {
        return Err("the clip has no legs".to_string());
    }
    hi = hi.min(lo.saturating_add(cfg.secs.saturating_mul(1_000_000_000)));
    let ra = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 50.0, 0.05);
    let rb = make_radar(Kind::EnRoute, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let (pa, _, _) = simulate_plots(&built.truth, &ra, cfg.seed, lo, hi + 1)?;
    let (pb, _, _) = simulate_plots(&built.truth, &rb, cfg.seed + 2, lo, hi + 1)?;
    // Half the aircraft send ADS-B (the even addresses), as the study's mixed runs do.
    let adsb = simulate_adsb(&built.truth, &ADSB_STUDY, cfg.seed + 1, lo, hi, &|a| {
        a & 1 == 0
    })?;
    Ok(ClipInputs {
        sites: vec![ra.trk, rb.trk],
        plots: vec![pa, pb],
        adsb,
        aircraft: oa_truth::aircraft_addrs(&built.truth).len(),
    })
}

/// Feeds a pass: the inputs of `adsb` and `plots` in time order (ADS-B first on a tie), each
/// offered and the layer advanced to its time, the call timed.
pub fn run_pass(
    name: &str,
    sites: &[RadarSite],
    plots: &[&[RadarPlot]],
    adsb: &[Measurement],
) -> Result<PassResult, String> {
    let base = TrackerConfig {
        out_capacity: 1 << 17,
        log_capacity: 1 << 16,
        ..DEFAULT_CONFIG
    };
    let cfg = fusion_config(fusion_tracker_config(&base, sites));
    let mut f = fusion_new(&cfg, sites).map_err(|e| e.to_string())?;
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
    let mut out = output_new(1 << 17);
    let (mut all, mut busy) = (Vec::with_capacity(order.len()), Vec::new());
    let mut per_input: Vec<(f64, u64)> = Vec::new();
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
        fusion_advance(&mut f, t, &mut out);
        let us = t0.elapsed().as_secs_f64() * 1.0e6;
        let n = fusion_stats(&f).processed - before;
        all.push(us);
        if n > 0 {
            busy.push(us);
            per_input.push((us / n as f64, n));
        }
        last = t;
        if out.t_ns.len() > (1 << 16) {
            output_clear(&mut out);
        }
    }
    let t0 = Instant::now();
    let before = fusion_stats(&f).processed;
    fusion_finish(&mut f, last, &mut out);
    let n = fusion_stats(&f).processed - before;
    if n > 0 {
        let us = t0.elapsed().as_secs_f64() * 1.0e6;
        busy.push(us);
        per_input.push((us / n as f64, n));
    }
    Ok(PassResult {
        name: name.to_string(),
        inputs: fusion_stats(&f).processed,
        calls: order.len() as u64,
        calls_that_processed: busy.len() as u64,
        all_calls_us: dist_of(&all),
        busy_calls_us: dist_of(&busy),
        per_input_us: dist_weighted(&per_input),
    })
}

/// The tracker alone, ADS-B only: every report through `tracker_update`, timed per call. The
/// call emits the fixed-rate rows that fall before the report's time, so its time includes them;
/// `tracker_advance` is not timed here because the reports themselves keep the clock moving.
pub fn run_tracker_only(adsb: &[Measurement]) -> Result<Dist, String> {
    let cfg = TrackerConfig {
        out_capacity: 1 << 17,
        log_capacity: 1 << 16,
        max_tracks: 8192,
        ..DEFAULT_CONFIG
    };
    let mut t = tracker_new(&cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(1 << 17);
    let mut upd = Vec::with_capacity(adsb.len());
    for m in adsb {
        let t0 = Instant::now();
        tracker_update(&mut t, m, &mut out);
        upd.push(t0.elapsed().as_secs_f64() * 1.0e6);
        if out.t_ns.len() > (1 << 16) {
            output_clear(&mut out);
        }
    }
    Ok(dist_of(&upd))
}

/// The whole clip run: ADS-B only, radar only, both, and the tracker alone. The sealed-window
/// count of the clip read is put in `sealed` (see [`clip_inputs`]).
pub fn run_clip(cfg: &ClipCfg, sealed: &mut u64) -> Result<ClipResult, String> {
    let load_start = loadavg3();
    let inp = clip_inputs(cfg, sealed)?;
    let none: Vec<&[RadarPlot]> = inp.plots.iter().map(|_| &[][..]).collect();
    let all: Vec<&[RadarPlot]> = inp.plots.iter().map(Vec::as_slice).collect();
    let passes = vec![
        run_pass("fusion, ADS-B only", &inp.sites, &none, &inp.adsb)?,
        run_pass("fusion, radar plots only", &inp.sites, &all, &[])?,
        run_pass("fusion, ADS-B and plots", &inp.sites, &all, &inp.adsb)?,
    ];
    let tracker_update_us = run_tracker_only(&inp.adsb)?;
    Ok(ClipResult {
        aircraft: inp.aircraft,
        adsb_reports: inp.adsb.len(),
        plots: inp.plots.iter().map(Vec::len).collect(),
        clip_s: cfg.secs as f64,
        passes,
        tracker_update_us,
        hwm_kb: status_kb("VmHWM").unwrap_or(0),
        load_start,
        load_end: loadavg3(),
    })
}

pub fn clip_kv(r: &ClipResult) -> Kv {
    let mut kv = Kv::default();
    kv.put("label", "loaded machine, preliminary");
    kv.put("aircraft", r.aircraft);
    kv.put("adsb_reports", r.adsb_reports);
    for (i, n) in r.plots.iter().enumerate() {
        kv.put(&format!("plots_radar{i}"), n);
    }
    kv.put("clip_s", r.clip_s);
    for (i, p) in r.passes.iter().enumerate() {
        let k = format!("pass{i}");
        kv.put(&format!("{k}.name"), &p.name);
        kv.put(&format!("{k}.inputs"), p.inputs);
        kv.put(&format!("{k}.calls"), p.calls);
        kv.put(&format!("{k}.calls_that_processed"), p.calls_that_processed);
        kv.put_dist(&format!("{k}.all_calls"), "us", &p.all_calls_us);
        kv.put_dist(&format!("{k}.busy_calls"), "us", &p.busy_calls_us);
        kv.put_dist(&format!("{k}.per_input"), "us", &p.per_input_us);
    }
    kv.put_dist("tracker_update", "us", &r.tracker_update_us);
    kv.put("vmhwm_kb", r.hwm_kb);
    kv.put("load_start", &r.load_start);
    kv.put("load_end", &r.load_end);
    kv
}

fn row(name: &str, d: &Dist) -> String {
    use crate::stats::fmt_f;
    format!(
        "    {name:<34} n={:<9} p50 {:>9} p99 {:>9} p99.9 {:>9} max {:>10} us\n",
        d.n,
        fmt_f(d.p50),
        fmt_f(d.p99),
        fmt_f(d.p999),
        fmt_f(d.max)
    )
}

pub fn clip_text(r: &ClipResult) -> String {
    let mut s = format!(
        "== per-update latency, real clip [loaded machine, preliminary; load {} -> {}] ==\n  {} aircraft in the first {:.0} s; {} ADS-B reports, plots per radar {:?}\n",
        r.load_start, r.load_end, r.aircraft, r.clip_s, r.adsb_reports, r.plots
    );
    for p in &r.passes {
        s.push_str(&format!(
            "  {}: {} inputs processed in {} calls ({} of them did the work)\n",
            p.name, p.inputs, p.calls, p.calls_that_processed
        ));
        s.push_str(&row("every offer+advance call", &p.all_calls_us));
        s.push_str(&row("calls that processed (whole batch)", &p.busy_calls_us));
        s.push_str(&row("per input processed (batch / n)", &p.per_input_us));
    }
    s.push_str("  tracker alone, ADS-B only (an update is applied before the call returns)\n");
    s.push_str(&row("tracker_update per report", &r.tracker_update_us));
    s.push_str(&format!(
        "  peak VmHWM {:.0} MB\n",
        r.hwm_kb as f64 / 1024.0
    ));
    s
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::gen::{Layout, Traffic, TrafficCfg};
    use oa_track_tools::fusion_score::EPOCH_NS;

    // REQ: OA-PRF-006
    #[test]
    fn a_pass_over_synthetic_inputs_times_every_call() {
        let cfg = TrafficCfg {
            seed: 2,
            n: 30,
            layout: Layout::Stress,
            adsb_frac: 0.5,
            radar_only: false,
            radars: vec![Kind::Terminal],
            horizon_s: 100.0,
        };
        let mut t = Traffic::new(&cfg).unwrap();
        let (mut adsb, mut w, mut plots) = (Vec::new(), Vec::new(), Vec::<RadarPlot>::new());
        for k in 0..40 {
            t.adsb_window(k, &mut adsb);
            t.plot_windows(k, &mut w);
            plots.extend_from_slice(&w[0]);
        }
        adsb.sort_by_key(|m| m.toa_ns);
        let sites = t.sites();
        let p = run_pass("x", &sites, &[&plots], &adsb).unwrap();
        assert_eq!(p.calls as usize, adsb.len() + plots.len());
        assert_eq!(p.inputs as usize, adsb.len() + plots.len());
        assert_eq!(p.all_calls_us.n as u64, p.calls);
        assert!(p.calls_that_processed > 0 && p.calls_that_processed < p.calls);
        assert_eq!(p.per_input_us.n as u64, p.inputs);
        assert!(p.per_input_us.p50 <= p.per_input_us.max);
        let u = run_tracker_only(&adsb).unwrap();
        assert_eq!(u.n, adsb.len());
        assert!(adsb.first().unwrap().toa_ns >= EPOCH_NS);
    }
}

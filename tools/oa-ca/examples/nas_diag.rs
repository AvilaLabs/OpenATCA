//! `nas_diag`: `nas_hour` (the M9 national-hour measurement) with the diagnosis of the grid's
//! latitude limit added to `ca`: per cycle the largest radius and its terms, the limit, the
//! radii and the latitudes of the rows above it (to `--diag <csv>`), and the alerts as hashes
//! (to `--alerts <file>`), for comparing two runs without addresses. Otherwise as follows.
//!
//! `nas_hour`: the M9 national-hour measurement. It replays a recorded ADS-B clip through the
//! tracker, and optionally through conflict alert, and times it; it scores nothing. Shell (B3):
//! reads the clock, `/proc` and the clip.
//!
//! ```text
//! nas_hour census <clip.oaf>
//! nas_hour track  <clip.oaf> --start <time> [--max-tracks <n>]
//! nas_hour ca     <clip.oaf> --start <time> --config <file.cfg> [--max-tracks <n>]
//! ```
//!
//! `census` prints, from the clip, the distinct airborne addresses of each UTC hour and, for the
//! hour with the most, of each of its minutes (counts only, no address). `track` and `ca` feed
//! every measurement of the clip in time order; the ones before `--start` are warm-up and are not
//! measured, those from `--start` to `--start` + 3600 s are. A cycle is one second of
//! measurement time: its work is the sum of the timed calls of that second's measurements (`track`
//! and `ca`) and, for `ca`, the conflict-alert cycle of each one-second tick of rows. Times are
//! this machine's, with the load beside them left to the caller.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

use oa_adsb::{decode_payload_lenient, reports_len, Reports, F_GROUND, NONE_I32};
use oa_bench::input::{file_opener, for_each_adsb_frame, load_adsb_counted};
use oa_ca::{
    alert_log_clear, alert_log_new, ca_check, ca_cycle, ca_new, ca_totals, event_log_clear,
    event_log_new, CaTrack, F_COAST, F_IDENTITY_UNCONFIRMED, F_RADAR_ONLY, F_UNCERTAIN,
};
use oa_ca_run::config::parse_config;
use oa_ca_run::guard::refuse_sealed;
use oa_ca_run::report::state_text;
use oa_ca_run::run::r95_of;
use oa_encounter_run::run::{ground_at, ground_table};
use oa_safety::inhibit::inhibits_new;
use oa_safety::net::{net_status, state_table_new, NetId, NetState};
use oa_sealed::Sealed;
use oa_track::adsb::{adsb_measurements, check_adsb_config, AdsbConfig};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_counts, tracker_flush,
    tracker_new, tracker_stats, tracker_update, TrackOutput, DEFAULT_CONFIG, O_COAST, O_HAS_ALT,
    O_IDENTITY_UNCONFIRMED, O_TICK, O_UNCERTAIN,
};
use oa_track_tools::score::{interval_stats, scoring_config, IntervalStats};

const HOUR_S: i64 = 3600;
const NS: i64 = 1_000_000_000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("nas_hour: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, clip) = match args {
        [c, p, ..] => (c.as_str(), Path::new(p)),
        _ => return Err("usage: nas_hour <census|track|ca> <clip.oaf> [flags]".to_string()),
    };
    refuse_sealed(Sealed::production(), clip)?;
    let (mut start, mut cfg_path, mut max_tracks) = (None, None, 65_536usize);
    let (mut diag_path, mut alerts_path) = (None, None);
    let mut it = args.iter().skip(2);
    while let Some(f) = it.next() {
        let v = it.next().ok_or_else(|| format!("{f} needs a value"))?;
        match f.as_str() {
            "--start" => start = Some(oa_adsb::cli::parse_time(v)?),
            "--config" => cfg_path = Some(v.clone()),
            "--diag" => diag_path = Some(v.clone()),
            "--alerts" => alerts_path = Some(v.clone()),
            "--max-tracks" => max_tracks = v.parse().map_err(|_| "bad --max-tracks".to_string())?,
            _ => return Err(format!("unknown flag {f}")),
        }
    }
    match cmd {
        "census" => census(clip),
        "track" => replay(
            clip,
            start.ok_or("--start is required")?,
            max_tracks,
            None,
            (None, None),
        ),
        "ca" => {
            let p = cfg_path.ok_or("--config is required")?;
            replay(
                clip,
                start.ok_or("--start is required")?,
                max_tracks,
                Some(p),
                (diag_path, alerts_path),
            )
        }
        _ => Err(format!("unknown command {cmd}")),
    }
}

/// Distinct airborne addresses per UTC hour, and per minute of the busiest hour. Airborne: a
/// report with a barometric altitude and not on the ground (the rule of `ground_table`).
fn census(clip: &Path) -> Result<(), String> {
    let mut hours: BTreeMap<i64, HashSet<u32>> = BTreeMap::new();
    let mut minutes: BTreeMap<i64, HashSet<u32>> = BTreeMap::new();
    let mut rows = 0u64;
    let mut fr = Reports::default();
    let damaged = for_each_adsb_frame(clip, &mut |_, payload| {
        oa_adsb::reports_clear(&mut fr);
        if decode_payload_lenient(payload, &mut fr).is_err() {
            return;
        }
        for i in 0..reports_len(&fr) {
            let (Some(&a), Some(&t), Some(&fl), Some(&alt)) = (
                fr.icao.get(i),
                fr.time_ns.get(i),
                fr.flags.get(i),
                fr.baro_alt_ft.get(i),
            ) else {
                continue;
            };
            rows += 1;
            if fl & F_GROUND == 0 && alt != NONE_I32 {
                let s = t.div_euclid(NS);
                hours.entry(s.div_euclid(HOUR_S)).or_default().insert(a);
                minutes.entry(s.div_euclid(60)).or_default().insert(a);
            }
        }
    })
    .map_err(|e| e.to_string())?;
    println!("rows {rows}, damaged frames {damaged}");
    let mut best = None;
    for (h, s) in &hours {
        println!("hour_unix_s {} distinct_airborne {}", h * HOUR_S, s.len());
        if best.is_none_or(|(_, n)| s.len() > n) {
            best = Some((*h, s.len()));
        }
    }
    if let Some((h, n)) = best {
        println!("busiest_hour_unix_s {} distinct_airborne {n}", h * HOUR_S);
        let mut peak = (0i64, 0usize);
        let mut all: Vec<usize> = Vec::new();
        for m in (h * 60)..(h * 60 + 60) {
            let c = minutes.get(&m).map_or(0, HashSet::len);
            println!("minute_unix_s {} distinct_airborne {c}", m * 60);
            all.push(c);
            if c > peak.1 {
                peak = (m * 60, c);
            }
        }
        println!("peak_minute_unix_s {} distinct_airborne {}", peak.0, peak.1);
    }
    Ok(())
}

/// Reads `/proc/self/stat` user and system CPU, seconds.
fn cpu_s() -> (f64, f64) {
    // seal-allow: /proc, not an input
    let s = fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = s.rsplit_once(')').map_or("", |x| x.1);
    let f: Vec<&str> = rest.split_whitespace().collect();
    let g = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 100.0;
    (g(11), g(12))
}

/// `VmHWM` of this process, kB.
fn hwm_kb() -> u64 {
    // seal-allow: /proc, not an input
    let s = fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn load_text() -> String {
    // seal-allow: /proc, not an input
    fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// A sorted sample's percentile (nearest rank), 0 if empty.
fn pct(v: &[u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let k = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v.get(k.clamp(1, v.len()) - 1).copied().unwrap_or(0)
}

fn dist(name: &str, v: &mut [u64], unit: &str, div: f64) {
    v.sort_unstable();
    let f = |x: u64| x as f64 / div;
    let mean = v.iter().map(|&x| x as f64).sum::<f64>() / v.len().max(1) as f64 / div;
    println!(
        "{name} n {} mean {mean:.3} p50 {:.3} p99 {:.3} p99.9 {:.3} max {:.3} {unit}",
        v.len(),
        f(pct(v, 50.0)),
        f(pct(v, 99.0)),
        f(pct(v, 99.9)),
        f(v.last().copied().unwrap_or(0)),
    );
}

fn flags_of(o: u8) -> u8 {
    let mut f = 0;
    if o & O_COAST != 0 {
        f |= F_COAST;
    }
    if o & O_UNCERTAIN != 0 {
        f |= F_UNCERTAIN;
    }
    if o & O_IDENTITY_UNCONFIRMED != 0 {
        f |= F_RADAR_ONLY | F_IDENTITY_UNCONFIRMED;
    }
    f
}

/// Conflict alert's side of a run: the state, and what each measured cycle did.
struct CaSide {
    st: oa_ca::CaState,
    alerts: oa_ca::AlertLog,
    events: oa_ca::EventLog,
    net: oa_safety::net::StateTable,
    inh: Box<oa_safety::inhibit::InhibitTable>,
    ground: oa_encounter_run::run::GroundTable,
    batch: Vec<CaTrack>,
    cur: Option<i64>,
    /// Cycle times, ns, of the measured cycles.
    cycle_ns: Vec<u64>,
    evaluated: Vec<u64>,
    candidates: Vec<u64>,
    tracks: Vec<u64>,
    total_ns: u64,
    cycles_work_short: u64,
    cycles_not_degraded_but_unevaluated: u64,
    cycles_not_running: u64,
    /// Rows above the flat-test latitude and rows whose radius was held to the cap, per cycle.
    high_lat: Vec<u64>,
    r95_clamped: Vec<u64>,
    cycles_input_short: u64,
    states: BTreeMap<String, u64>,
    cycles_unscanned: u64,
    alerts_n: u64,
    last_overflow: u64,
    cfg: oa_ca::CaConfig,
    diag: Diag,
    last_fresh: std::collections::HashMap<u32, i64>,
    /// One line per closed alert: hash, start, end, midpoint, flags (no addresses).
    alert_lines: Vec<String>,
}

/// What the diagnosis keeps over the measured cycles.
#[derive(Default)]
struct Diag {
    csv: String,
    /// Raw 95 % radii above 10 km, metres, of every measured row.
    big_r: Vec<u64>,
    /// Per row above 20 km: coasting, uncertain, and seconds since the address last had a row that
    /// was not coasting (None: none in the clip so far).
    over20: Vec<(bool, bool, Option<u64>)>,
    /// Latitude of every row above the limit, 5-degree bins from 0.
    lat_hist: [u64; 19],
    lat_above_min: f64,
    lat_above_max: f64,
    /// Largest-radius terms, per cycle.
    r_max: Vec<u64>,
    limit_deg_x1000: Vec<u64>,
    rows_above: Vec<u64>,
    mine_above: Vec<u64>,
    capped_rows: Vec<u64>,
    /// Rows held to the cap, by cause: radius not finite or negative, and above the cap.
    capped_nan: Vec<u64>,
    capped_over: Vec<u64>,
    cycles_rmax_is_cap: u64,
    cycles_vmax_term_biggest: u64,
    radii_all: Vec<u64>,
    speeds_all: Vec<u64>,
    /// The limit the cycle would have with the radii of all rows but the capped ones, and with the
    /// 99th percentile radius and speed.
    limit_nocap_x1000: Vec<u64>,
    limit_p99_x1000: Vec<u64>,
}

fn limit_deg(radius_m: f64) -> f64 {
    let dphi = radius_m / 6_356_752.0;
    if dphi > 0.0 {
        (0.02 / dphi).atan().to_degrees()
    } else {
        90.0
    }
}

fn fnv(addr_a: u32, addr_b: u32, t: i64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ 0x6361_2d65_6366_u64;
    for b in addr_a
        .to_le_bytes()
        .into_iter()
        .chain(addr_b.to_le_bytes())
        .chain(t.to_le_bytes())
    {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl CaSide {
    fn flush(&mut self, measured_from_ns: i64) {
        let Some(t) = self.cur.take() else { return };
        let diag_now = t >= measured_from_ns;
        let pre = if diag_now { Some(self.pre(t)) } else { None };
        let a = Instant::now();
        let out = ca_cycle(
            &mut self.st,
            t,
            &self.batch,
            &mut self.alerts,
            &mut self.events,
            &mut self.net,
        );
        let ns = a.elapsed().as_nanos() as u64;
        self.batch.clear();
        self.alerts_n += self.alerts.items.len() as u64;
        for a in &self.alerts.items {
            self.alert_lines.push(format!(
                "{:016x} {} {} {} {} {} {} {} {} {:016x}",
                fnv(a.addr_a, a.addr_b, a.start_ns),
                a.start_ns,
                a.end_ns,
                a.lat_e7,
                a.lon_e7,
                a.flags,
                a.ticks_conflict,
                a.ticks_current,
                a.h_eff_m,
                fnv(a.addr_a, a.addr_b, 0)
            ));
        }
        alert_log_clear(&mut self.alerts);
        event_log_clear(&mut self.events);
        if t < measured_from_ns {
            return;
        }
        let s = net_status(&self.net, &self.inh, NetId::ConflictAlert, t);
        self.cycle_ns.push(ns);
        self.total_ns += ns;
        self.evaluated.push(out.stats.evaluated);
        self.candidates.push(out.stats.candidates);
        self.tracks.push(out.stats.tracks);
        self.cycles_work_short += u64::from(out.work_short > 0);
        self.cycles_unscanned += u64::from(out.stats.tracks_unscanned > 0);
        self.cycles_not_degraded_but_unevaluated +=
            u64::from(out.stats.not_evaluated > 0 && s.state == NetState::Running);
        self.cycles_not_running += u64::from(s.state != NetState::Running);
        if let Some(p) = pre {
            self.diag.mine_above.push(p);
            self.diag.rows_above.push(out.stats.rows_high_lat);
        }
        self.high_lat.push(out.stats.rows_high_lat);
        self.r95_clamped.push(out.stats.r95_clamped);
        self.cycles_input_short += u64::from(out.input_short > 0);
        *self.states.entry(state_text(&s.state)).or_default() += 1;
    }

    /// The diagnosis of this cycle's batch, before the cycle runs: the terms of the largest radius
    /// as the grid forms it, the limit that follows, the radii and the latitudes above the limit.
    /// Returns the count of rows above the limit by this calculation.
    fn pre(&mut self, t: i64) -> u64 {
        let cap = self.cfg.r95_cap_m;
        for o in &self.batch {
            let fresh = o.flags & F_COAST == 0;
            if fresh {
                self.last_fresh.insert(o.addr, t);
            }
            let r = f64::from(o.r95_m);
            if r.is_finite() && r > 10_000.0 {
                self.diag.big_r.push(r as u64);
                if r > 20_000.0 {
                    let age = self.last_fresh.get(&o.addr).map(|&l| ((t - l) / NS) as u64);
                    self.diag.over20.push((
                        o.flags & F_COAST != 0,
                        o.flags & F_UNCERTAIN != 0,
                        age,
                    ));
                }
            }
        }
        let big_t = self.cfg.look_ahead_ns as f64 / 1e9;
        let (h_max, _) = oa_ca::volume_maxima(&self.cfg);
        let rows: Vec<(f64, f64, f64)> = self
            .batch
            .iter()
            .map(|o| {
                let r = f64::from(o.r95_m);
                let r = if r.is_finite() && (0.0..=cap).contains(&r) {
                    r
                } else {
                    cap
                };
                let (e, n) = (f64::from(o.vel_east_mps), f64::from(o.vel_north_mps));
                (r, (e * e + n * n).sqrt(), o.lat_deg)
            })
            .collect();
        let vmax = rows.iter().map(|x| x.1).fold(0.0, f64::max);
        let rmax = rows.iter().map(|x| x.0).fold(0.0, f64::max);
        let radius = big_t * 2.0 * vmax + h_max + 2.0 * rmax;
        let lim = limit_deg(radius);
        let mut rs: Vec<f64> = rows.iter().map(|x| x.0).collect();
        let mut vs: Vec<f64> = rows.iter().map(|x| x.1).collect();
        rs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        vs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let q = |v: &[f64], p: f64| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            let k = ((p / 100.0) * v.len() as f64).ceil() as usize;
            v.get(k.clamp(1, v.len()) - 1).copied().unwrap_or(0.0)
        };
        let uncapped = rows
            .iter()
            .filter(|x| x.0 < cap)
            .map(|x| x.0)
            .fold(0.0, f64::max);
        let lim_nocap = limit_deg(big_t * 2.0 * vmax + h_max + 2.0 * uncapped);
        let lim_p99 = limit_deg(big_t * 2.0 * q(&vs, 99.0) + h_max + 2.0 * q(&rs, 99.0));
        let capped = rows.iter().filter(|x| x.0 >= cap).count() as u64;
        let nan = self
            .batch
            .iter()
            .filter(|o| !f64::from(o.r95_m).is_finite() || o.r95_m < 0.0)
            .count() as u64;
        let over = self
            .batch
            .iter()
            .filter(|o| f64::from(o.r95_m).is_finite() && f64::from(o.r95_m) > cap)
            .count() as u64;
        self.diag.capped_nan.push(nan);
        self.diag.capped_over.push(over);
        let mut above = 0u64;
        for x in &rows {
            if x.2.abs() > lim {
                above += 1;
                let b = ((x.2.abs() / 5.0) as usize).min(18);
                if let Some(c) = self.diag.lat_hist.get_mut(b) {
                    *c += 1;
                }
                let la = x.2.abs();
                if self.diag.lat_above_min == 0.0 || la < self.diag.lat_above_min {
                    self.diag.lat_above_min = la;
                }
                if la > self.diag.lat_above_max {
                    self.diag.lat_above_max = la;
                }
            }
        }
        let (st, rt, ht) = (big_t * 2.0 * vmax, 2.0 * rmax, h_max);
        self.diag.cycles_rmax_is_cap += u64::from(rmax >= cap);
        self.diag.cycles_vmax_term_biggest += u64::from(st > rt && st > ht);
        self.diag.r_max.push(radius as u64);
        self.diag.limit_deg_x1000.push((lim * 1000.0) as u64);
        self.diag
            .limit_nocap_x1000
            .push((lim_nocap * 1000.0) as u64);
        self.diag.limit_p99_x1000.push((lim_p99 * 1000.0) as u64);
        self.diag.capped_rows.push(capped);
        self.diag.radii_all.extend(rs.iter().map(|&x| x as u64));
        self.diag.speeds_all.extend(vs.iter().map(|&x| x as u64));
        self.diag.csv.push_str(&format!(
            "{},{},{:.1},{:.1},{:.0},{:.0},{:.0},{:.0},{:.3},{:.3},{:.3},{},{:.0},{:.0},{:.0},{:.0},{:.0},{}\n",
            t / NS,
            rows.len(),
            vmax,
            rmax,
            ht,
            st,
            rt,
            radius,
            lim,
            lim_nocap,
            lim_p99,
            capped,
            q(&rs, 50.0),
            q(&rs, 90.0),
            q(&rs, 99.0),
            q(&vs, 50.0),
            q(&vs, 99.0),
            above
        ));
        above
    }

    fn consume(&mut self, o: &TrackOutput, from_ns: i64) {
        oa_ca::ca_note_lost_input(&mut self.st, o.overflow.saturating_sub(self.last_overflow));
        self.last_overflow = o.overflow;
        for k in 0..output_len(o) {
            let (
                Some(&fl),
                Some(&t),
                Some(&addr),
                Some(&la),
                Some(&lo),
                Some(&alt),
                Some(&vr),
                Some(&ve),
                Some(&vn),
                Some(&cov),
            ) = (
                o.flags.get(k),
                o.t_ns.get(k),
                o.addr.get(k),
                o.lat_e7.get(k),
                o.lon_e7.get(k),
                o.alt_ft.get(k),
                o.vrate_fpm.get(k),
                o.vel_east_mps.get(k),
                o.vel_north_mps.get(k),
                o.pos_cov_m2.get(k),
            )
            else {
                oa_ca::ca_note_lost_input(&mut self.st, 1);
                continue;
            };
            if fl & O_TICK == 0 {
                continue;
            }
            if self.cur != Some(t) {
                self.flush(from_ns);
                self.cur = Some(t);
            }
            self.batch.push(CaTrack {
                lat_deg: f64::from(la) / 1e7,
                lon_deg: f64::from(lo) / 1e7,
                addr,
                vel_east_mps: ve,
                vel_north_mps: vn,
                r95_m: r95_of(cov),
                alt_ft: if fl & O_HAS_ALT != 0 { alt } else { f32::NAN },
                vrate_fpm: if fl & O_HAS_ALT != 0 { vr } else { f32::NAN },
                flags: flags_of(fl),
                ground: ground_at(&self.ground, addr, t),
            });
        }
    }
}

fn replay(
    clip: &Path,
    start_s: i64,
    max_tracks: usize,
    ca_cfg: Option<String>,
    outs: (Option<String>, Option<String>),
) -> Result<(), String> {
    let t_open = Instant::now();
    let mut sealed = 0u64;
    let loaded = load_adsb_counted(clip, &file_opener(), &mut sealed).map_err(|e| e.to_string())?;
    let ground = ground_table(&loaded.reports);
    let acfg = AdsbConfig::default();
    check_adsb_config(&acfg)?;
    let (meas, astats) = adsb_measurements(&loaded.reports, &acfg);
    println!(
        "clip rows {} bad_frames {} dropped_records {} sealed_window_dropped {sealed} measurements {} quality_removed {:?}",
        astats.rows_in, loaded.bad_frames, loaded.dropped_records, meas.len(), astats.removed
    );
    drop(loaded);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: NS,
        p90_ns: NS,
        p99_ns: NS,
    });
    let base = oa_track::TrackerConfig {
        max_tracks,
        ..DEFAULT_CONFIG
    };
    let cfg = scoring_config(&base, &iv);
    let mut t = tracker_new(&cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(cfg.out_capacity);
    let drain = cfg.out_capacity.min(1 << 16) / 2;
    let mut ca = match ca_cfg {
        None => None,
        Some(p) => {
            refuse_sealed(Sealed::production(), Path::new(&p))?;
            // seal-allow: the configuration file, checked by the guard on the line above
            let text = fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?;
            let set = parse_config(&text).map_err(|e| format!("{p}: {e}"))?;
            ca_check(&set.ca).map_err(|e| format!("conflict alert configuration: {e}"))?;
            Some(CaSide {
                st: ca_new(&set.ca)?,
                alerts: alert_log_new(set.alert_log_capacity)?,
                events: event_log_new(set.event_log_capacity)?,
                net: state_table_new(set.net_stale_limit_ns, set.net_future_horizon_ns)
                    .map_err(|e| format!("{e:?}"))?,
                inh: Box::new(inhibits_new()),
                ground,
                batch: Vec::new(),
                cur: None,
                cycle_ns: Vec::new(),
                evaluated: Vec::new(),
                candidates: Vec::new(),
                tracks: Vec::new(),
                total_ns: 0,
                cycles_work_short: 0,
                cycles_not_degraded_but_unevaluated: 0,
                cycles_not_running: 0,
                high_lat: Vec::new(),
                r95_clamped: Vec::new(),
                cycles_input_short: 0,
                states: BTreeMap::new(),
                cycles_unscanned: 0,
                alerts_n: 0,
                last_overflow: 0,
                cfg: set.ca,
                diag: Diag::default(),
                alert_lines: Vec::new(),
                last_fresh: std::collections::HashMap::new(),
            })
        }
    };
    let from_ns = start_s * NS;
    let to_ns = (start_s + HOUR_S) * NS;
    println!(
        "setup_s {:.1} tracker max_tracks {} out_capacity {} expected_interval_ms {} init_interval_ms {} load {}",
        t_open.elapsed().as_secs_f64(),
        cfg.max_tracks,
        cfg.out_capacity,
        cfg.expected_interval_ns / 1_000_000,
        cfg.init_interval_ns / 1_000_000,
        load_text()
    );
    // The replay. `call_ns` is each measured update call; `sec_*` are per second of
    // measurement time.
    let mut call_ns: Vec<u32> = Vec::new();
    let (mut sec_ns, mut sec_reports) = (Vec::<u64>::new(), Vec::<u64>::new());
    let (mut conf, mut tent) = (Vec::<u64>::new(), Vec::<u64>::new());
    let (mut cur_sec, mut cur_work, mut cur_n) = (i64::MIN, 0u64, 0u64);
    let (c0, w0) = (cpu_s(), Instant::now());
    let mut ca_wall_ns = 0u64;
    let mut measured_from: Option<(f64, Instant)> = None;
    for m in &meas {
        if m.toa_ns >= to_ns {
            break;
        }
        let measured = m.toa_ns >= from_ns;
        if measured && measured_from.is_none() {
            let c = cpu_s();
            measured_from = Some((c.0 + c.1, Instant::now()));
        }
        let sec = m.toa_ns.div_euclid(NS);
        if measured && sec != cur_sec {
            if cur_sec != i64::MIN {
                sec_ns.push(cur_work);
                sec_reports.push(cur_n);
                let (tn, c) = tracker_counts(&t);
                conf.push(c as u64);
                tent.push(tn as u64);
            }
            cur_sec = sec;
            cur_work = 0;
            cur_n = 0;
        }
        let a = Instant::now();
        tracker_update(&mut t, m, &mut out);
        let mut ns = a.elapsed().as_nanos() as u64;
        if measured {
            call_ns.push(u32::try_from(ns).unwrap_or(u32::MAX));
        }
        if output_len(&out) > drain {
            let a = Instant::now();
            if let Some(c) = ca.as_mut() {
                let b = Instant::now();
                c.consume(&out, from_ns);
                ca_wall_ns += b.elapsed().as_nanos() as u64;
            }
            output_clear(&mut out);
            // The drain is the consumer's, charged to the second it fell in.
            ns += if ca.is_some() {
                0
            } else {
                a.elapsed().as_nanos() as u64
            };
        }
        if measured {
            cur_work += ns;
            cur_n += 1;
        }
    }
    let end = to_ns;
    tracker_advance(&mut t, end, &mut out);
    tracker_flush(&mut t, end, &mut out);
    if let Some(c) = ca.as_mut() {
        c.consume(&out, from_ns);
        c.flush(from_ns);
    }
    let (c1, w1) = (cpu_s(), Instant::now());
    if cur_sec != i64::MIN {
        sec_ns.push(cur_work);
        sec_reports.push(cur_n);
        let (tn, c) = tracker_counts(&t);
        conf.push(c as u64);
        tent.push(tn as u64);
    }
    let (mc, mw) = measured_from.ok_or("no measurement at or after --start")?;
    let replay_cpu = c1.0 + c1.1 - (c0.0 + c0.1);
    let measured_cpu = c1.0 + c1.1 - mc;
    let measured_wall = w1.duration_since(mw).as_secs_f64();
    println!("load_end {}", load_text());
    println!(
        "replay_wall_s {:.2} replay_cpu_s {:.2} (user {:.2} sys {:.2}) measured_window_wall_s {measured_wall:.2} measured_window_cpu_s {measured_cpu:.2} (warm-up included in replay_*)",
        w1.duration_since(w0).as_secs_f64(),
        replay_cpu,
        c1.0 - c0.0,
        c1.1 - c0.1,
    );
    println!(
        "real_time_factor_measured_window {:.5} peak_rss_kb {}",
        measured_cpu / HOUR_S as f64,
        hwm_kb()
    );
    let total: u64 = sec_reports.iter().sum();
    println!(
        "measured_reports {total} reports_per_s_mean {:.1} reports_per_s_max {} seconds {}",
        total as f64 / sec_reports.len().max(1) as f64,
        sec_reports.iter().max().copied().unwrap_or(0),
        sec_reports.len()
    );
    println!(
        "confirmed_max {} confirmed_mean {:.1} tentative_max {} tentative_mean {:.1}",
        conf.iter().max().copied().unwrap_or(0),
        conf.iter().sum::<u64>() as f64 / conf.len().max(1) as f64,
        tent.iter().max().copied().unwrap_or(0),
        tent.iter().sum::<u64>() as f64 / tent.len().max(1) as f64,
    );
    let mut calls: Vec<u64> = call_ns.iter().map(|&x| u64::from(x)).collect();
    dist("tracker_update_call", &mut calls, "us", 1e3);
    let over = sec_ns.iter().filter(|&&x| x > NS as u64).count();
    dist("tracker_cycle_1s", &mut sec_ns.clone(), "ms", 1e6);
    println!("tracker_cycles_over_1s {over}");
    let st = tracker_stats(&t);
    println!(
        "tracker_counters offered {} applied {} refused {:?} capacity_refused {} duplicates {} late_window {} late_ring {} numerical_refused {} time_held {} out_overflow {} rows_unrepresentable {} nan_covariance {} started {} confirmed {}",
        st.offered, st.applied, st.refused, st.capacity_refused, st.duplicates, st.late_window,
        st.late_ring, st.numerical_refused, st.time_held, out.overflow, st.rows_unrepresentable,
        st.nan_covariance, st.started, st.confirmed
    );
    if let Some(mut c) = ca {
        let tot = ca_totals(&c.st);
        println!(
            "ca_cycle_total_s {:.2} ca_consume_incl_cycle_s {:.2} cycles_measured {} alerts_closed {}",
            c.total_ns as f64 / 1e9,
            ca_wall_ns as f64 / 1e9,
            c.cycle_ns.len(),
            c.alerts_n
        );
        dist("ca_cycle", &mut c.cycle_ns, "ms", 1e6);
        let over_1s = c.cycle_ns.iter().filter(|&&x| x > NS as u64).count();
        println!("ca_cycles_over_1s {over_1s}");
        let mean = |v: &[u64]| v.iter().sum::<u64>() as f64 / v.len().max(1) as f64;
        println!(
            "ca_tracks_per_cycle max {} mean {:.1}",
            c.tracks.iter().max().copied().unwrap_or(0),
            mean(&c.tracks)
        );
        println!(
            "ca_pairs_evaluated_per_cycle max {} mean {:.1}",
            c.evaluated.iter().max().copied().unwrap_or(0),
            mean(&c.evaluated)
        );
        println!(
            "ca_candidates_per_cycle max {} mean {:.1}",
            c.candidates.iter().max().copied().unwrap_or(0),
            mean(&c.candidates)
        );
        println!(
            "ca_cycles_work_short {} cycles_tracks_unscanned {} cycles_unevaluated_but_running {} cycles_net_not_running {}",
            c.cycles_work_short,
            c.cycles_unscanned,
            c.cycles_not_degraded_but_unevaluated,
            c.cycles_not_running
        );
        println!(
            "ca_rows_above_flat_test_latitude_per_cycle max {} mean {:.1} cycles_with_any {}",
            c.high_lat.iter().max().copied().unwrap_or(0),
            mean(&c.high_lat),
            c.high_lat.iter().filter(|&&x| x > 0).count()
        );
        println!(
            "ca_r95_clamped_per_cycle max {} mean {:.1} cycles_input_short {}",
            c.r95_clamped.iter().max().copied().unwrap_or(0),
            mean(&c.r95_clamped),
            c.cycles_input_short
        );
        println!("ca_net_states_measured_cycles {:?}", c.states);
        println!("ca_totals_all_cycles {tot:?}");
        diag_report(&mut c, &outs)?;
    }
    Ok(())
}

fn diag_report(c: &mut CaSide, outs: &(Option<String>, Option<String>)) -> Result<(), String> {
    let d = &mut c.diag;
    if let Some(p) = &outs.0 {
        let head = "t_s,tracks,vmax_mps,rmax_m,term_hmax_m,term_speed_m,term_r95_m,radius_m,limit_deg,limit_without_capped_deg,limit_at_p99_deg,rows_capped,r_p50,r_p90,r_p99,v_p50,v_p99,rows_above_limit\n";
        fs::write(p, format!("{head}{}", d.csv)).map_err(|e| format!("{p}: {e}"))?;
    }
    println!(
        "diag cycles {} cycles_rmax_is_cap {} cycles_speed_term_largest {}",
        d.r_max.len(),
        d.cycles_rmax_is_cap,
        d.cycles_vmax_term_biggest
    );
    dist("diag_radius", &mut d.r_max, "km", 1e3);
    dist("diag_limit_deg", &mut d.limit_deg_x1000, "deg", 1e3);
    dist(
        "diag_limit_without_capped_deg",
        &mut d.limit_nocap_x1000,
        "deg",
        1e3,
    );
    dist("diag_limit_at_p99_deg", &mut d.limit_p99_x1000, "deg", 1e3);
    dist("diag_rows_capped", &mut d.capped_rows, "rows", 1.0);
    dist(
        "diag_rows_capped_unusable_radius",
        &mut d.capped_nan,
        "rows",
        1.0,
    );
    dist(
        "diag_rows_capped_radius_above_cap",
        &mut d.capped_over,
        "rows",
        1.0,
    );
    dist("diag_rows_above_by_calc", &mut d.mine_above, "rows", 1.0);
    dist("diag_rows_above_by_core", &mut d.rows_above, "rows", 1.0);
    dist("diag_row_radius_all_rows", &mut d.radii_all, "m", 1.0);
    dist("diag_row_speed_all_rows", &mut d.speeds_all, "mps", 1.0);
    println!(
        "diag_rows_above_limit latitude min {:.2} max {:.2} deg; histogram by 5 deg of |lat| (from 0): {:?}",
        d.lat_above_min, d.lat_above_max, d.lat_hist
    );
    d.big_r.sort_unstable();
    println!(
        "diag_r95_above_10km_all_rows count {} p50 {} p99 {} max {} m",
        d.big_r.len(),
        pctv(&d.big_r, 50.0),
        pctv(&d.big_r, 99.0),
        d.big_r.last().copied().unwrap_or(0)
    );
    let n20 = d.over20.len();
    let coast = d.over20.iter().filter(|x| x.0).count();
    let unc = d.over20.iter().filter(|x| x.1).count();
    let none = d.over20.iter().filter(|x| x.2.is_none()).count();
    let mut ages: Vec<u64> = d.over20.iter().filter_map(|x| x.2).collect();
    ages.sort_unstable();
    println!("diag_rows_above_20km count {n20} coasting {coast} uncertain {unc} neither {} no_fresh_row_in_clip {none} age_s_since_last_non_coasting_row p50 {} p90 {} p99 {} max {} ; ages_over_30s {} over_60s {}",
        d.over20.iter().filter(|x| !x.0 && !x.1).count(), pctv(&ages, 50.0), pctv(&ages, 90.0), pctv(&ages, 99.0), ages.last().copied().unwrap_or(0),
        ages.iter().filter(|&&a| a > 30).count(), ages.iter().filter(|&&a| a > 60).count());
    if let Some(p) = &outs.1 {
        c.alert_lines.sort();
        fs::write(p, c.alert_lines.join("\n") + "\n").map_err(|e| format!("{p}: {e}"))?;
    }
    Ok(())
}

fn pctv(v: &[u64], p: f64) -> u64 {
    pct(v, p)
}

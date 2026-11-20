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
use oa_track::score::{interval_stats, scoring_config, IntervalStats};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_counts, tracker_flush,
    tracker_new, tracker_stats, tracker_update, TrackOutput, DEFAULT_CONFIG, O_COAST, O_HAS_ALT,
    O_IDENTITY_UNCONFIRMED, O_TICK, O_UNCERTAIN,
};

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
    let mut it = args.iter().skip(2);
    while let Some(f) = it.next() {
        let v = it.next().ok_or_else(|| format!("{f} needs a value"))?;
        match f.as_str() {
            "--start" => start = Some(oa_adsb::cli::parse_time(v)?),
            "--config" => cfg_path = Some(v.clone()),
            "--max-tracks" => max_tracks = v.parse().map_err(|_| "bad --max-tracks".to_string())?,
            _ => return Err(format!("unknown flag {f}")),
        }
    }
    match cmd {
        "census" => census(clip),
        "track" => replay(clip, start.ok_or("--start is required")?, max_tracks, None),
        "ca" => {
            let p = cfg_path.ok_or("--config is required")?;
            replay(
                clip,
                start.ok_or("--start is required")?,
                max_tracks,
                Some(p),
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
}

impl CaSide {
    fn flush(&mut self, measured_from_ns: i64) {
        let Some(t) = self.cur.take() else { return };
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
        self.high_lat.push(out.stats.rows_high_lat);
        self.r95_clamped.push(out.stats.r95_clamped);
        self.cycles_input_short += u64::from(out.input_short > 0);
        *self.states.entry(state_text(&s.state)).or_default() += 1;
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
    }
    Ok(())
}

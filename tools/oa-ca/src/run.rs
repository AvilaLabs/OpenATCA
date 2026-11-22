//! The run: clip, tracker, conflict alert, files. Shell (B3).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use oa_bench::input::{file_opener, load_adsb_counted, Opener};
use oa_ca::{
    alert_log_clear, alert_log_new, ca_check, ca_cycle, ca_finish, ca_new, ca_note_lost_input,
    ca_totals, event_log_clear, event_log_new, Alert, AlertLog, AltLost, CaState, CaTrack,
    CycleStats, EventLog, F_COAST, F_IDENTITY_UNCONFIRMED, F_RADAR_ONLY, F_UNCERTAIN,
};
use oa_encounter_run::run::{ground_at, ground_table, GroundTable};
use oa_safety::inhibit::{inhibits_new, InhibitTable};
use oa_safety::net::{
    net_status, state_report, state_table_new, Area, NetId, NetReport, Reason, StateTable,
};
use oa_sealed::Sealed;
use oa_sha256::{sha256, to_hex};
use oa_track::adsb::{adsb_measurements, check_adsb_config, AdsbConfig};
use oa_track::score::{interval_stats, scoring_config, split_reports, IntervalStats};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_stats, tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG,
    O_COAST, O_HAS_ALT, O_IDENTITY_UNCONFIRMED, O_TICK, O_UNCERTAIN,
};

use crate::args::Args;
use crate::config::{parse_config, Settings};
use crate::guard::{open_guarded, refuse_sealed, OpenFail};
use crate::report::{alerts_csv, events_csv, state_text, summary_text, SummaryInput};

/// The chi-square quantile of 2 degrees of freedom at 95 %: the 95 % radius of a circular
/// Gaussian is `sqrt(CHI2_2_95 * variance)`; for the covariance of a track the larger eigenvalue
/// takes the variance, so the radius holds in every direction.
pub const CHI2_2_95: f64 = 5.991_464_547_107_98;

/// The 95 % horizontal radius, metres, of a position covariance `[ee, en, nn]` (m^2); NaN if the
/// covariance is not usable (the core then holds it to the cap and counts it).
pub fn r95_of(cov: [f32; 3]) -> f32 {
    let (ee, en, nn) = (f64::from(cov[0]), f64::from(cov[1]), f64::from(cov[2]));
    if !(ee.is_finite() && en.is_finite() && nn.is_finite()) || ee < 0.0 || nn < 0.0 {
        return f32::NAN;
    }
    let half = (ee - nn) / 2.0;
    let lam = (ee + nn) / 2.0 + (half * half + en * en).sqrt();
    (CHI2_2_95 * lam).sqrt() as f32
}

/// What a run of the tracker and conflict alert produced. A7.2: cold struct, size guard waived.
pub struct Record {
    pub alerts: Vec<Alert>,
    pub events: Vec<AltLost>,
    pub totals: CycleStats,
    /// Cycles by the text of the net state after each.
    pub net_cycles: BTreeMap<String, u64>,
    pub log_overflow: u64,
    pub event_overflow: u64,
    pub tracker_overflow: u64,
    /// Valid measurements refused because the track table had no free slot.
    pub tracker_capacity_refused: u64,
    pub tick_rows: u64,
    /// Rows of the tracker's output left out for a missing column (also in `totals`).
    pub rows_skipped: u64,
    /// Cycles that left work undone and also had input missing.
    pub cycles_both: u64,
    /// The net state after the last report, the end of the run included.
    pub final_net: String,
}

struct Feed<'a> {
    st: CaState,
    alerts: AlertLog,
    events: EventLog,
    net: StateTable,
    inh: Box<InhibitTable>,
    batch: Vec<CaTrack>,
    cur: Option<i64>,
    ground: &'a GroundTable,
    all: Vec<Alert>,
    all_events: Vec<AltLost>,
    net_cycles: BTreeMap<String, u64>,
    tick_rows: u64,
    rows_skipped: u64,
    cycles_both: u64,
    last_overflow: u64,
    tracker_capacity_refused: u64,
    capacity_refused_seen: u64,
    capacity_refused_by_time: BTreeMap<i64, u64>,
    last_t: i64,
}

fn flush(f: &mut Feed) {
    if let Some(t) = f.cur.take() {
        let ready: Vec<(i64, u64)> = f
            .capacity_refused_by_time
            .range(..=t)
            .map(|(&at, &count)| (at, count))
            .collect();
        for (at, count) in ready {
            f.capacity_refused_by_time.remove(&at);
            // A refused observation at `at` cannot affect earlier tick cycles. Charge it to
            // the first cycle at or after the observation via the existing upstream-loss path.
            ca_note_lost_input(&mut f.st, count);
        }
        let out = ca_cycle(
            &mut f.st,
            t,
            &f.batch,
            &mut f.alerts,
            &mut f.events,
            &mut f.net,
        );
        f.last_t = t;
        if out.work_short > 0 && out.input_short > 0 {
            f.cycles_both += 1;
        }
        f.batch.clear();
        f.all.extend_from_slice(&f.alerts.items);
        alert_log_clear(&mut f.alerts);
        f.all_events.extend_from_slice(&f.events.items);
        event_log_clear(&mut f.events);
        let s = net_status(&f.net, &f.inh, NetId::ConflictAlert, t);
        *f.net_cycles.entry(state_text(&s.state)).or_default() += 1;
    }
}

/// The core's flag byte of the tracker's. The tracker's one identity flag is the radar-handle
/// key of a track with no ADS-B address, so it sets both radar-only and identity-unconfirmed.
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

/// Takes the one-second rows (every live track at a tick, one time after another) of a drained
/// table; a change of time ends the cycle.
fn consume(f: &mut Feed, o: &TrackOutput) {
    // Rows the tracker's output table dropped for want of room since the last look were never
    // seen by conflict alert: the next cycle must not read as running.
    ca_note_lost_input(&mut f.st, o.overflow.saturating_sub(f.last_overflow));
    f.last_overflow = o.overflow;
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
            // A row with a column missing cannot be read: counted, and told to the core.
            f.rows_skipped += 1;
            ca_note_lost_input(&mut f.st, 1);
            continue;
        };
        if fl & O_TICK == 0 {
            continue;
        }
        if f.cur != Some(t) {
            flush(f);
            f.cur = Some(t);
        }
        f.tick_rows += 1;
        f.batch.push(CaTrack {
            lat_deg: f64::from(la) / 1e7,
            lon_deg: f64::from(lo) / 1e7,
            addr,
            vel_east_mps: ve,
            vel_north_mps: vn,
            r95_m: r95_of(cov),
            alt_ft: if fl & O_HAS_ALT != 0 { alt } else { f32::NAN },
            vrate_fpm: if fl & O_HAS_ALT != 0 { vr } else { f32::NAN },
            flags: flags_of(fl),
            ground: ground_at(f.ground, addr, t),
        });
    }
}

/// Runs the tracker over `meas` (in time order) as the scoring run does and gives its one-second
/// rows to conflict alert, one cycle per tick.
pub fn track_and_alert(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    ground: &GroundTable,
    set: &Settings,
) -> Result<Record, String> {
    ca_check(&set.ca).map_err(|e| format!("conflict alert configuration: {e}"))?;
    let mut t = tracker_new(cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(cfg.out_capacity);
    let mut f = Feed {
        st: ca_new(&set.ca)?,
        alerts: alert_log_new(set.alert_log_capacity)?,
        events: event_log_new(set.event_log_capacity)?,
        net: state_table_new(set.net_stale_limit_ns, set.net_future_horizon_ns)
            .map_err(|e| format!("net state table: {e:?}"))?,
        inh: Box::new(inhibits_new()),
        batch: Vec::new(),
        cur: None,
        ground,
        all: Vec::new(),
        all_events: Vec::new(),
        net_cycles: BTreeMap::new(),
        tick_rows: 0,
        rows_skipped: 0,
        cycles_both: 0,
        last_overflow: 0,
        tracker_capacity_refused: 0,
        capacity_refused_seen: 0,
        capacity_refused_by_time: BTreeMap::new(),
        last_t: 0,
    };
    let drain = cfg.out_capacity.min(1 << 16) / 2;
    for m in meas {
        tracker_update(&mut t, m, &mut out);
        let refused = tracker_stats(&t).capacity_refused;
        let newly_refused = refused.saturating_sub(f.capacity_refused_seen);
        if newly_refused > 0 {
            f.capacity_refused_seen = refused;
            f.tracker_capacity_refused = f.tracker_capacity_refused.saturating_add(newly_refused);
            let count = f.capacity_refused_by_time.entry(m.toa_ns).or_default();
            *count = count.saturating_add(newly_refused);
        }
        if output_len(&out) > drain {
            consume(&mut f, &out);
            output_clear(&mut out);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut out);
    tracker_flush(&mut t, end, &mut out);
    consume(&mut f, &out);
    flush(&mut f);
    ca_finish(&mut f.st, &mut f.alerts, &mut f.net);
    f.all.extend_from_slice(&f.alerts.items);
    let mut final_t = f.last_t;
    if let Some((&last_fault_ns, _)) = f.capacity_refused_by_time.last_key_value() {
        // No tracker tick followed the final refusal. Report the run-level shortfall at its
        // observation time so the final status does not remain RUNNING.
        final_t = final_t.max(last_fault_ns);
        let pending = f
            .capacity_refused_by_time
            .values()
            .copied()
            .fold(0u64, u64::saturating_add);
        let _ = state_report(
            &mut f.net,
            NetId::ConflictAlert,
            NetReport::Degraded {
                reason: Reason::InputMissing,
                area: Area::Whole,
                count: u32::try_from(pending).unwrap_or(u32::MAX),
            },
            last_fault_ns,
            last_fault_ns,
        );
    }
    let final_net = state_text(&net_status(&f.net, &f.inh, NetId::ConflictAlert, final_t).state);
    Ok(Record {
        alerts: f.all,
        events: f.all_events,
        totals: ca_totals(&f.st),
        net_cycles: f.net_cycles,
        log_overflow: f.alerts.overflow,
        event_overflow: f.events.overflow,
        tracker_overflow: out.overflow,
        tracker_capacity_refused: f.tracker_capacity_refused,
        tick_rows: f.tick_rows,
        rows_skipped: f.rows_skipped,
        cycles_both: f.cycles_both,
        final_net,
    })
}

/// The whole run: reads the clip and the configuration, writes `alerts.csv`,
/// `altitude_lost.csv` and `summary.txt` under `args.out`, and returns the summary text.
pub fn run(args: &Args) -> Result<String, String> {
    run_counted(args, &mut 0)
}

/// [`run`], also giving `sealed` the count of clip rows left out for a sealed window (set as soon
/// as the frames are read, so an error after that, or in reading the rest, still carries it).
pub fn run_counted(args: &Args, sealed: &mut u64) -> Result<String, String> {
    run_counted_with(args, &file_opener(), sealed)
}

/// The names of the output files, the summary last: it is the mark of a complete run.
const OUTPUTS: [&str; 3] = ["alerts.csv", "altitude_lost.csv", "summary.txt"];

/// Removes the output files of an earlier run, each path checked by the guard first, so that a
/// run that fails never leaves files that look like its own result.
fn remove_stale_outputs(out: &Path) -> Result<(), String> {
    for name in OUTPUTS {
        let p = out.join(name);
        refuse_sealed(Sealed::production(), &p)?;
        match fs::remove_file(&p) {
            Ok(()) => {}
            // No such file, or no such directory (the run then fails on creating it, with its
            // own message, after the clip is read).
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
    }
    Ok(())
}

/// Writes `text` to `dir/name` whole or not at all: to a temporary name in the same directory,
/// flushed to disk, then renamed over the final name.
fn write_atomic(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let fin = dir.join(name);
    let written = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &fin)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("{}: {e}", fin.display()));
    }
    Ok(())
}

/// [`run_counted`] with the clip opened by `open` (a test hands it a clip that fails).
pub fn run_counted_with(args: &Args, open: &Opener, sealed: &mut u64) -> Result<String, String> {
    // Every path is checked before anything is opened or created, for a caller of the library too.
    for p in [&args.adsb, &args.config, &args.out] {
        refuse_sealed(Sealed::production(), p)?;
    }
    remove_stale_outputs(&args.out)?;
    let mut cfg_bytes = Vec::new();
    open_guarded(Sealed::production(), &args.config)
        .map_err(|e| match e {
            OpenFail::Refused(m) | OpenFail::Io(m) => m,
        })?
        .read_to_end(&mut cfg_bytes)
        .map_err(|e| format!("{}: {e}", args.config.display()))?;
    let cfg_text = String::from_utf8(cfg_bytes.clone())
        .map_err(|_| format!("{}: not UTF-8 text", args.config.display()))?;
    let set = parse_config(&cfg_text).map_err(|e| format!("{}: {e}", args.config.display()))?;
    // The count goes straight into `sealed`, so a clip that fails after rows were dropped still
    // reports them.
    let clip = load_adsb_counted(&args.adsb, open, sealed).map_err(|e| e.to_string())?;
    let clip_sha = clip.hash.sha256.clone();
    let (reports, scope) = if args.scorer_half {
        let (a, b, _) = split_reports(&clip.reports);
        oa_bench::split_check::check_split(&a, &b)
            .map_err(|v| format!("the split of the clip is not valid, run refused: {v:?}"))?;
        (
            a,
            "the scorer's half A of the clip (as oa-track-run)".to_string(),
        )
    } else {
        (clip.reports, "every report of the clip".to_string())
    };
    let adsb_cfg = AdsbConfig::default();
    check_adsb_config(&adsb_cfg).map_err(|e| format!("adapter configuration: {e}"))?;
    let ground = ground_table(&reports);
    let (meas, _) = adsb_measurements(&reports, &adsb_cfg);
    drop(reports);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let rec = track_and_alert(&meas, &cfg, &ground, &set)?;
    fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let csv = alerts_csv(&rec.alerts);
    let csv_sha = to_hex(&sha256(csv.as_bytes()));
    let name_of = |p: &std::path::Path| {
        p.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    };
    let summary = summary_text(&SummaryInput {
        cfg: &set.ca,
        alerts: &rec.alerts,
        events: &rec.events,
        totals: rec.totals,
        net_cycles: &rec.net_cycles,
        log_overflow: rec.log_overflow,
        event_overflow: rec.event_overflow,
        tracker_overflow: rec.tracker_overflow,
        tracker_capacity_refused: rec.tracker_capacity_refused,
        measurements: meas.len() as u64,
        tick_rows: rec.tick_rows,
        rows_skipped: rec.rows_skipped,
        cycles_both: rec.cycles_both,
        final_net: &rec.final_net,
        clip_name: &name_of(&args.adsb),
        clip_sha256: &clip_sha,
        sealed_window_dropped: *sealed,
        config_name: &name_of(&args.config),
        config_sha256: &to_hex(&sha256(&cfg_bytes)),
        scope: &scope,
        alerts_sha256: &csv_sha,
    });
    let events = events_csv(&rec.events);
    for (file, text) in OUTPUTS.into_iter().zip([&csv, &events, &summary]) {
        write_atomic(&args.out, file, text)?;
    }
    // The renames reach the disk with the directory.
    // seal-allow: opens the output directory, which this run wrote, to sync the renames
    fs::File::open(&args.out)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("{}: {e}", args.out.display()))?;
    Ok(summary)
}

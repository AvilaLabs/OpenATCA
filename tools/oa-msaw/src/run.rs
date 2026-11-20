//! The run: clip, tracker, MSAW, files. Shell (B3).

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use oa_bench::input::{file_opener, load_adsb_counted, Opener};
use oa_encounter_run::run::{ground_at, ground_table, GroundTable};
use oa_msaw::{
    alert_log_clear, alert_log_new, msaw_check, msaw_evaluate, msaw_finish, msaw_new,
    msaw_note_lost_input, msaw_plan, msaw_points, msaw_totals, skip_log_clear, skip_log_new, Alert,
    AlertLog, CycleStats, FloorSample, MsawState, MsawTrack, SkipLog, SkipWhy, FLOOR_NONE, F_COAST,
    F_IDENTITY_UNCONFIRMED, F_RADAR_ONLY, F_UNCERTAIN, MAX_PROCS,
};
use oa_safety::geom::Vertex;
use oa_safety::inhibit::{
    events_take, inhibit_create, inhibit_expire, inhibits_new, suppressed_clear, suppressed_log,
    suppressed_overflow, Event, EventKind, InhibitRequest, InhibitTable,
};
use oa_safety::net::{net_status, state_table_new, NetId, StateTable};
use oa_sealed::Sealed;
use oa_sha256::{sha256, to_hex};
use oa_track::adsb::{adsb_measurements, check_adsb_config, AdsbConfig};
use oa_track::score::{interval_stats, scoring_config, split_reports, IntervalStats};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG, O_COAST, O_HAS_ALT,
    O_IDENTITY_UNCONFIRMED, O_TICK, O_UNCERTAIN,
};

use crate::args::Args;
use crate::cifp::{areas_from_cifp, CifpArea};
use crate::config::{parse_config, InhibitSpec, Settings};
use crate::floors::{obstacle_index, FloorStats, Floors, ObstacleIndex};
use crate::guard::{open_guarded, refuse_sealed, OpenFail};
use crate::report::{
    alerts_csv, inhibitions_csv, skipped_csv, state_text, summary_text, DataUsed, SummaryInput,
};

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

/// A listing of a track the net left out, kept once per address and reason.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkipAgg {
    pub first_ns: i64,
    pub last_ns: i64,
    pub cycles: u64,
    pub lat_e7: i32,
    pub lon_e7: i32,
}

/// What an inhibition hid, kept once per inhibition.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiddenAgg {
    pub count: u64,
    pub first_ns: i64,
    pub last_ns: i64,
}

/// An inhibition as it was made: its id and its spec, with the absolute times.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct MadeInhibit {
    pub id: u32,
    pub start_ns: i64,
    pub end_ns: i64,
    pub spec: InhibitSpec,
}

/// What a run of the tracker and the net produced. A7.2: cold struct, size guard waived.
pub struct Record {
    pub alerts: Vec<Alert>,
    pub skips: BTreeMap<(u32, u8), SkipAgg>,
    pub totals: CycleStats,
    /// Cycles by the text of the net state after each.
    pub net_cycles: BTreeMap<String, u64>,
    pub alert_overflow: u64,
    pub skip_overflow: u64,
    pub tracker_overflow: u64,
    pub tick_rows: u64,
    /// Rows of the tracker's output left out for a missing column.
    pub rows_skipped: u64,
    /// The net state after the last report, the end of the run included.
    pub final_net: String,
    pub made: Vec<MadeInhibit>,
    pub hidden: BTreeMap<u32, HiddenAgg>,
    /// When each inhibition expired (the review event), by id.
    pub expired_ns: BTreeMap<u32, i64>,
    pub suppressed_overflow: u64,
    pub floor_stats: FloorStats,
}

struct Feed<'a> {
    st: MsawState,
    alerts: AlertLog,
    skips: SkipLog,
    net: StateTable,
    inh: Box<InhibitTable>,
    floors: Floors,
    floor_buf: Vec<FloorSample>,
    batch: Vec<MsawTrack>,
    cur: Option<i64>,
    ground: &'a GroundTable,
    all: Vec<Alert>,
    skip_agg: BTreeMap<(u32, u8), SkipAgg>,
    hidden: BTreeMap<u32, HiddenAgg>,
    expired_ns: BTreeMap<u32, i64>,
    net_cycles: BTreeMap<String, u64>,
    tick_rows: u64,
    rows_skipped: u64,
    last_overflow: u64,
    last_t: i64,
    /// The first terrain file that could not be read: the run is then refused.
    error: Option<String>,
}

fn why_code(w: SkipWhy) -> u8 {
    w as u8
}

fn flush(f: &mut Feed) {
    let Some(t) = f.cur.take() else {
        return;
    };
    if f.error.is_some() {
        f.batch.clear();
        return;
    }
    inhibit_expire(&mut f.inh, t);
    msaw_plan(&mut f.st, t, &f.batch, &mut f.skips);
    f.floor_buf.clear();
    for p in msaw_points(&f.st) {
        match f.floors.floor_at(p) {
            Ok(s) => f.floor_buf.push(s),
            Err(e) => {
                f.error = Some(e);
                f.floor_buf.push(FloorSample {
                    terrain_ft: FLOOR_NONE,
                    obstacle_ft: FLOOR_NONE,
                    mva_ft: FLOOR_NONE,
                });
            }
        }
    }
    msaw_evaluate(
        &mut f.st,
        &f.floor_buf,
        &mut f.alerts,
        &mut f.skips,
        &mut f.net,
        &mut f.inh,
    );
    f.last_t = t;
    f.batch.clear();
    f.all.extend_from_slice(&f.alerts.items);
    alert_log_clear(&mut f.alerts);
    for s in &f.skips.items {
        let e = f
            .skip_agg
            .entry((s.addr, why_code(s.why)))
            .or_insert(SkipAgg {
                first_ns: s.t_ns,
                last_ns: s.t_ns,
                cycles: 0,
                lat_e7: s.lat_e7,
                lon_e7: s.lon_e7,
            });
        e.last_ns = s.t_ns;
        e.cycles += 1;
    }
    skip_log_clear(&mut f.skips);
    for h in suppressed_log(&f.inh) {
        let e = f.hidden.entry(h.inhibition_id).or_insert(HiddenAgg {
            count: 0,
            first_ns: h.time_ns,
            last_ns: h.time_ns,
        });
        e.count += 1;
        e.last_ns = h.time_ns;
    }
    suppressed_clear(&mut f.inh);
    let mut ev = [Event {
        time_ns: 0,
        id: 0,
        kind: EventKind::Created,
        net: NetId::Msaw,
        by: [0; oa_safety::inhibit::AUTHOR_BYTES],
    }; 16];
    loop {
        let n = events_take(&mut f.inh, &mut ev);
        for e in ev.iter().take(n) {
            if e.kind == EventKind::Expired {
                f.expired_ns.insert(e.id, e.time_ns);
            }
        }
        if n < ev.len() {
            break;
        }
    }
    let s = net_status(&f.net, &f.inh, NetId::Msaw, t);
    *f.net_cycles.entry(state_text(&s.state)).or_default() += 1;
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
    // seen by the net: the next cycle must not read as running.
    msaw_note_lost_input(&mut f.st, o.overflow.saturating_sub(f.last_overflow));
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
            // A row with a column missing cannot be read: counted, and told to the net.
            f.rows_skipped += 1;
            msaw_note_lost_input(&mut f.st, 1);
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
        f.batch.push(MsawTrack {
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

/// Makes the inhibitions of the configuration at `t0_ns`; their times are seconds after it.
fn make_inhibits(
    inh: &mut InhibitTable,
    set: &Settings,
    t0_ns: i64,
) -> Result<Vec<MadeInhibit>, String> {
    let mut made = Vec::new();
    for (i, spec) in set.inhibits.iter().enumerate() {
        let (start_ns, end_ns) = (
            t0_ns.saturating_add((spec.start_s * 1e9).round() as i64),
            t0_ns.saturating_add((spec.end_s * 1e9).round() as i64),
        );
        let verts: Vec<Vertex> = spec.vertices.clone();
        let req = InhibitRequest {
            net: NetId::Msaw,
            vertices: &verts,
            alt_low_ft: spec.alt_lo_ft,
            alt_high_ft: spec.alt_hi_ft,
            start_ns,
            end_ns,
            reason: &spec.reason,
            author: &spec.author,
        };
        let id = inhibit_create(inh, &set.inhibit_cfg, &req, t0_ns)
            .map_err(|e| format!("inhibit line {} refused: {e:?}", i + 1))?;
        made.push(MadeInhibit {
            id,
            start_ns,
            end_ns,
            spec: spec.clone(),
        });
    }
    Ok(made)
}

/// Runs the tracker over `meas` (in time order) as the scoring run does and gives its one-second
/// rows to the net, one cycle per tick, with the floors from `floors`.
pub fn track_and_alert(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    ground: &GroundTable,
    set: &Settings,
    floors: Floors,
    t0_ns: i64,
) -> Result<Record, String> {
    msaw_check(&set.msaw).map_err(|e| format!("MSAW configuration: {e}"))?;
    let mut t = tracker_new(cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(cfg.out_capacity);
    let mut inh = Box::new(inhibits_new());
    let made = make_inhibits(&mut inh, set, t0_ns)?;
    let mut f = Feed {
        st: msaw_new(&set.msaw)?,
        alerts: alert_log_new(set.alert_log_capacity)?,
        skips: skip_log_new(set.skip_log_capacity)?,
        net: state_table_new(set.net_stale_limit_ns, set.net_future_horizon_ns)
            .map_err(|e| format!("net state table: {e:?}"))?,
        inh,
        floors,
        floor_buf: Vec::new(),
        batch: Vec::new(),
        cur: None,
        ground,
        all: Vec::new(),
        skip_agg: BTreeMap::new(),
        hidden: BTreeMap::new(),
        expired_ns: BTreeMap::new(),
        net_cycles: BTreeMap::new(),
        tick_rows: 0,
        rows_skipped: 0,
        last_overflow: 0,
        last_t: 0,
        error: None,
    };
    let drain = cfg.out_capacity.min(1 << 16) / 2;
    for m in meas {
        tracker_update(&mut t, m, &mut out);
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
    if let Some(e) = f.error.take() {
        return Err(e);
    }
    msaw_finish(&mut f.st, &mut f.alerts);
    f.all.extend_from_slice(&f.alerts.items);
    let final_net = state_text(&net_status(&f.net, &f.inh, NetId::Msaw, f.last_t).state);
    Ok(Record {
        alerts: f.all,
        skips: f.skip_agg,
        totals: msaw_totals(&f.st),
        net_cycles: f.net_cycles,
        alert_overflow: f.alerts.overflow,
        skip_overflow: f.skips.overflow,
        tracker_overflow: out.overflow,
        tick_rows: f.tick_rows,
        rows_skipped: f.rows_skipped,
        final_net,
        made,
        hidden: f.hidden,
        expired_ns: f.expired_ns,
        suppressed_overflow: suppressed_overflow(&f.inh),
        floor_stats: f.floors.stats,
    })
}

/// The whole run: reads the clip, the configuration and the data, writes `alerts.csv`,
/// `skipped.csv`, `inhibitions.csv` and `summary.txt` under `args.out`, and returns the summary
/// text.
pub fn run(args: &Args) -> Result<String, String> {
    run_counted(args, &mut 0)
}

/// [`run`], also giving `sealed` the count of clip rows left out for a sealed window (set as soon
/// as the frames are read, so an error after that, or in reading the rest, still carries it).
pub fn run_counted(args: &Args, sealed: &mut u64) -> Result<String, String> {
    run_counted_with(args, &file_opener(), sealed)
}

/// The names of the output files, the summary last: it is the mark of a complete run.
const OUTPUTS: [&str; 4] = [
    "alerts.csv",
    "skipped.csv",
    "inhibitions.csv",
    "summary.txt",
];

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

fn opened(g: &Sealed, p: &Path) -> Result<fs::File, String> {
    open_guarded(g, p).map_err(|e| match e {
        OpenFail::Refused(m) | OpenFail::Io(m) => m,
    })
}

/// The obstacles of a DOF file: the zip that holds `DOF.DAT`, or the file itself.
fn read_dof(p: &Path) -> Result<ObstacleIndex, String> {
    let is_zip = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let bytes = if is_zip {
        let z = oa_zip::read_archive_file(p).map_err(|e| format!("{}: {e}", p.display()))?;
        oa_zip::Archive::parse(&z)
            .and_then(|a| a.extract("DOF.DAT"))
            .map_err(|e| format!("{}: {e}", p.display()))?
    } else {
        oa_dof::read_file(p).map_err(|e| format!("{}: {e}", p.display()))?
    };
    let d = oa_dof::parse_dof(&bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(obstacle_index(&d))
}

/// [`run_counted`] with the clip opened by `open` (a test hands it a clip that fails).
pub fn run_counted_with(args: &Args, open: &Opener, sealed: &mut u64) -> Result<String, String> {
    let g = Sealed::production();
    // Every path is checked before anything is opened or created, for a caller of the library too.
    let optional = [&args.terrain, &args.dof, &args.mva, &args.cifp];
    for p in [&args.adsb, &args.config, &args.out]
        .into_iter()
        .chain(optional.into_iter().flatten())
    {
        refuse_sealed(g, p)?;
    }
    remove_stale_outputs(&args.out)?;
    let mut cfg_bytes = Vec::new();
    opened(g, &args.config)?
        .read_to_end(&mut cfg_bytes)
        .map_err(|e| format!("{}: {e}", args.config.display()))?;
    let cfg_text = String::from_utf8(cfg_bytes.clone())
        .map_err(|_| format!("{}: not UTF-8 text", args.config.display()))?;
    let mut set = parse_config(&cfg_text).map_err(|e| format!("{}: {e}", args.config.display()))?;
    // The data first: a file that cannot be read is an error before the clip is read.
    let mut used = DataUsed::default();
    let obstacles = match &args.dof {
        Some(p) => {
            let ix = read_dof(p)?;
            used.obstacles = Some(ix.count);
            Some(ix)
        }
        None => None,
    };
    let chart = match &args.mva {
        Some(p) => {
            let l = oa_mva::aixm::load_directory(p).map_err(|e| format!("{}: {e}", p.display()))?;
            used.chart_files = Some((
                l.files.iter().filter(|(_, r)| r.is_ok()).count(),
                l.files.iter().filter(|(_, r)| r.is_err()).count(),
                oa_mva::sector_count(&l.model),
            ));
            Some(l.model)
        }
        None => None,
    };
    if let Some(d) = &args.terrain {
        if !d.is_dir() {
            return Err(format!("{}: not a directory", d.display()));
        }
    }
    let mut cifp_areas: Vec<CifpArea> = Vec::new();
    if !set.cifp_airports.is_empty() {
        let p = args
            .cifp
            .as_ref()
            .ok_or("the configuration has cifp_airport lines and there is no --cifp")?;
        let f = opened(g, p)?;
        let c =
            oa_arinc424::parse_reader(BufReader::with_capacity(1 << 16, f), &p.to_string_lossy())
                .map_err(|e| format!("{}: {e}", p.display()))?;
        for a in &set.cifp_airports {
            cifp_areas.extend(areas_from_cifp(&c, a, set.cifp_radius_nm)?);
        }
    }
    if set.msaw.n_procs + cifp_areas.len() > MAX_PROCS {
        return Err(format!(
            "n_procs: {} procedure areas and {} from the CIFP are more than {MAX_PROCS}",
            set.msaw.n_procs,
            cifp_areas.len()
        ));
    }
    for a in &cifp_areas {
        let n = set.msaw.n_procs;
        if let Some(slot) = set.msaw.procs.get_mut(n) {
            *slot = a.area;
            set.msaw.n_procs += 1;
        }
    }
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
    let (mut meas, _) = adsb_measurements(&reports, &adsb_cfg);
    drop(reports);
    let t0_ns = meas.first().map_or(0, |m| m.toa_ns);
    let mut scope = scope;
    if let Some((a, b)) = args.window_s {
        let (lo, hi) = (
            t0_ns.saturating_add((a * 1e9) as i64),
            t0_ns.saturating_add((b * 1e9) as i64),
        );
        meas.retain(|m| m.toa_ns >= lo && m.toa_ns <= hi);
        scope = format!("{scope}, measurements from {a} s to {b} s after the first");
    }
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let floors = Floors::new(args.terrain.clone(), obstacles, chart);
    used.terrain = floors.has_terrain();
    used.chart = floors.has_chart();
    used.cifp_areas = cifp_areas
        .iter()
        .map(|a| (a.label.clone(), a.area.floor_ft, a.approaches))
        .collect();
    let rec = track_and_alert(&meas, &cfg, &ground, &set, floors, t0_ns)?;
    fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let csv = alerts_csv(&rec.alerts);
    let csv_sha = to_hex(&sha256(csv.as_bytes()));
    let name_of = |p: &Path| {
        p.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    };
    let skipped = skipped_csv(&rec.skips);
    let inhibitions = inhibitions_csv(&rec.made, &rec.hidden, &rec.expired_ns);
    let summary = summary_text(&SummaryInput {
        cfg: &set.msaw,
        alerts: &rec.alerts,
        totals: rec.totals,
        net_cycles: &rec.net_cycles,
        alert_overflow: rec.alert_overflow,
        skip_overflow: rec.skip_overflow,
        tracker_overflow: rec.tracker_overflow,
        measurements: meas.len() as u64,
        tick_rows: rec.tick_rows,
        rows_skipped: rec.rows_skipped,
        final_net: &rec.final_net,
        made: &rec.made,
        hidden: &rec.hidden,
        expired_ns: &rec.expired_ns,
        suppressed_overflow: rec.suppressed_overflow,
        floor_stats: rec.floor_stats,
        data: &used,
        skipped_listed: rec.skips.len(),
        clip_name: &name_of(&args.adsb),
        clip_sha256: &clip_sha,
        sealed_window_dropped: *sealed,
        config_name: &name_of(&args.config),
        config_sha256: &to_hex(&sha256(&cfg_bytes)),
        scope: &scope,
        alerts_sha256: &csv_sha,
        t0_ns,
    });
    for (file, text) in OUTPUTS
        .into_iter()
        .zip([&csv, &skipped, &inhibitions, &summary])
    {
        write_atomic(&args.out, file, text)?;
    }
    // The renames reach the disk with the directory.
    // seal-allow: opens the output directory, which this run wrote, to sync the renames
    fs::File::open(&args.out)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("{}: {e}", args.out.display()))?;
    Ok(summary)
}

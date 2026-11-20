//! The run: clip, tracker, record, files. Shell (B3).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use oa_adsb::{reports_len, Reports, F_GROUND, NONE_I32};
use oa_bench::input::load_adsb_with;
use oa_encounter::{
    encounter_check, encounter_cycle, encounter_finish, encounter_lat_limit_deg,
    encounter_log_clear, encounter_log_new, encounter_new, encounter_totals, ClassLimit,
    CycleStats, Encounter, EncounterConfig, EncounterLog, EncounterState, Obs, AIRBORNE,
    GROUND_UNKNOWN, MAX_CLASSES, ON_GROUND,
};
use oa_sha256::{sha256, to_hex};
use oa_track::adsb::{adsb_measurements, check_adsb_config, AdsbConfig};
use oa_track::score::{interval_stats, scoring_config, split_reports, IntervalStats};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG, O_HAS_ALT, O_TICK,
};

use crate::args::Args;
use crate::join::{answer_queries, join_text, Sfdps};
use crate::report::{band_of, encounters_csv, summary_text, Airport, SummaryInput};

const NM: f64 = 1852.0;
const FT: f64 = 0.3048;
const S_NS: i64 = 1_000_000_000;
/// A ground or airborne report older than this at a tick says nothing about that tick, ns.
pub const GROUND_AGE_NS: i64 = 30 * S_NS;

/// The record's settings, stated by the tool (the core has none). The record volume and the
/// first two classes are the bands the NTSB quoted from ASIAS for DCA (AIR-26-02): within 1,500
/// ft horizontally and 200 ft vertically, and within 1 NM and 400 ft. The hysteresis of 10 s
/// joins the two halves of a pass that a dropped report or a coast splits. Capacities: a
/// facility's peak is a few thousand tracks (the tracker's own limit is 4,096), pairs inside 5
/// NM are a small multiple of that; the budgets are about ten times what the busiest cycle of
/// the development clip needs.
pub fn tool_config() -> EncounterConfig {
    let mut classes = [ClassLimit {
        h_m: 1.0,
        v_ft: 1.0,
    }; MAX_CLASSES];
    for (k, (h_m, v_ft)) in [
        (1500.0 * FT, 200.0),
        (NM, 400.0),
        (3.0 * NM, 1000.0),
        (5.0 * NM, 1000.0),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(c) = classes.get_mut(k) {
            *c = ClassLimit { h_m, v_ft };
        }
    }
    EncounterConfig {
        max_tracks: 8192,
        max_open: 16_384,
        max_candidates: 1 << 17,
        volume_h_m: 5.0 * NM,
        volume_v_ft: 1000.0,
        classes,
        n_classes: 4,
        hysteresis_ns: 10 * S_NS,
        work_budget: 4_000_000,
        eval_budget: 100_000,
    }
}

/// Per aircraft, the times of the reports that say airborne or on the ground, sorted.
pub type GroundTable = BTreeMap<u32, Vec<(i64, u8)>>;

/// The ground state each report states: a barometric altitude is airborne, the altitude
/// `"ground"` (flag `F_GROUND`) is on the ground, anything else says nothing.
pub fn ground_table(r: &Reports) -> GroundTable {
    let mut t: GroundTable = BTreeMap::new();
    for i in 0..reports_len(r) {
        let (Some(&a), Some(&ns), Some(&fl), Some(&alt)) = (
            r.icao.get(i),
            r.time_ns.get(i),
            r.flags.get(i),
            r.baro_alt_ft.get(i),
        ) else {
            continue;
        };
        if fl & F_GROUND != 0 {
            t.entry(a).or_default().push((ns, ON_GROUND));
        } else if alt != NONE_I32 {
            t.entry(a).or_default().push((ns, AIRBORNE));
        }
    }
    for v in t.values_mut() {
        v.sort_unstable();
    }
    t
}

/// The ground state of `addr` at `t_ns` from its latest report not after it and not older than
/// [`GROUND_AGE_NS`]; [`GROUND_UNKNOWN`] if there is none.
pub fn ground_at(t: &GroundTable, addr: u32, t_ns: i64) -> u8 {
    let Some(v) = t.get(&addr) else {
        return GROUND_UNKNOWN;
    };
    let k = v.partition_point(|e| e.0 <= t_ns);
    match k.checked_sub(1).and_then(|k| v.get(k)) {
        Some(&(ns, g)) if t_ns - ns <= GROUND_AGE_NS => g,
        _ => GROUND_UNKNOWN,
    }
}

/// What a run of the tracker and the record produced. A7.2: cold struct, size guard waived.
pub struct Record {
    pub encounters: Vec<Encounter>,
    pub totals: CycleStats,
    pub lat_limit_deg: f64,
    pub log_overflow: u64,
    pub tracker_overflow: u64,
    pub tick_rows: u64,
}

struct Feed<'a> {
    st: EncounterState,
    log: EncounterLog,
    batch: Vec<Obs>,
    cur: Option<i64>,
    ground: &'a GroundTable,
    all: Vec<Encounter>,
    tick_rows: u64,
}

fn flush(f: &mut Feed) {
    if let Some(t) = f.cur.take() {
        encounter_cycle(&mut f.st, t, &f.batch, &mut f.log);
        f.batch.clear();
        f.all.extend_from_slice(&f.log.items);
        encounter_log_clear(&mut f.log);
    }
}

/// Takes the one-second rows (every live track at a tick, one time after another) of a drained
/// table; a change of time ends the cycle.
fn consume(f: &mut Feed, o: &TrackOutput) {
    for k in 0..output_len(o) {
        let (Some(&fl), Some(&t), Some(&addr), Some(&la), Some(&lo), Some(&alt), Some(&vr)) = (
            o.flags.get(k),
            o.t_ns.get(k),
            o.addr.get(k),
            o.lat_e7.get(k),
            o.lon_e7.get(k),
            o.alt_ft.get(k),
            o.vrate_fpm.get(k),
        ) else {
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
        f.batch.push(Obs {
            lat_deg: f64::from(la) / 1e7,
            lon_deg: f64::from(lo) / 1e7,
            addr,
            alt_ft: if fl & O_HAS_ALT != 0 { alt } else { f32::NAN },
            vrate_fpm: if fl & O_HAS_ALT != 0 { vr } else { f32::NAN },
            flags: fl,
            ground: ground_at(f.ground, addr, t),
        });
    }
}

/// Runs the tracker over `meas` (in time order) as the scoring run does and gives its one-second
/// rows to the record.
pub fn track_and_record(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    ground: &GroundTable,
    ecfg: &EncounterConfig,
) -> Result<Record, String> {
    encounter_check(ecfg).map_err(|e| format!("record configuration: {e}"))?;
    let mut t = tracker_new(cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(cfg.out_capacity);
    let mut f = Feed {
        st: encounter_new(ecfg)?,
        log: encounter_log_new(1 << 16),
        batch: Vec::new(),
        cur: None,
        ground,
        all: Vec::new(),
        tick_rows: 0,
    };
    let drain = cfg.out_capacity.min(1 << 16) / 2;
    let mut log_overflow = 0u64;
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
    encounter_finish(&mut f.st, &mut f.log);
    f.all.extend_from_slice(&f.log.items);
    log_overflow += f.log.overflow;
    Ok(Record {
        encounters: f.all,
        totals: encounter_totals(&f.st),
        lat_limit_deg: encounter_lat_limit_deg(&f.st),
        log_overflow,
        tracker_overflow: out.overflow,
        tick_rows: f.tick_rows,
    })
}

/// The public airports of an NASR CSV archive (`APT_BASE.csv`: site type airport, public use).
pub fn load_airports(path: &std::path::Path) -> Result<Vec<Airport>, String> {
    let bytes = oa_zip::read_archive_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let zip = oa_zip::Archive::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let csv = zip
        .extract("APT_BASE.csv")
        .map_err(|e| format!("{}: APT_BASE.csv: {e}", path.display()))?;
    let parsed = oa_nasr::apt::parse_airports("APT_BASE.csv", &csv[..])
        .map_err(|e| format!("{}: APT_BASE.csv: {e}", path.display()))?;
    Ok(parsed
        .records
        .iter()
        .filter(|a| a.site_type_code == "A" && a.facility_use_code == "PU")
        .map(|a| Airport {
            lat_deg: a.position.lat,
            lon_deg: a.position.lon,
            elev_ft: a.elev,
        })
        .collect())
}

/// For each encounter at or above 18,000 ft pressure altitude, the smallest separation between
/// reports of its two aircraft, metres: each report of one against the other's position at that
/// time (its own report, or the line between its two reports around it, not over 30 s apart),
/// over the encounter's time with 5 s each side, in both directions. Keyed by address pair.
pub fn report_separations(
    meas: &[Measurement],
    encounters: &[Encounter],
) -> BTreeMap<(u32, u32), f32> {
    let hi: Vec<&Encounter> = encounters
        .iter()
        .filter(|e| band_of(e.alt_ft) == 3)
        .collect();
    let wanted: BTreeSet<u32> = hi.iter().flat_map(|e| [e.addr_a, e.addr_b]).collect();
    let mut by: BTreeMap<u32, Vec<(i64, f64, f64)>> = BTreeMap::new();
    for m in meas.iter().filter(|m| wanted.contains(&m.addr)) {
        by.entry(m.addr).or_default().push((
            m.toa_ns,
            f64::from(m.lat_e7) / 1e7,
            f64::from(m.lon_e7) / 1e7,
        ));
    }
    let mut out = BTreeMap::new();
    let none = Vec::new();
    for e in hi {
        let (ra, rb) = (
            by.get(&e.addr_a).unwrap_or(&none),
            by.get(&e.addr_b).unwrap_or(&none),
        );
        let (lo, hi_t) = (e.start_ns - 5 * S_NS, e.end_ns + 5 * S_NS);
        let mut best = f64::INFINITY;
        for (a, b) in [(ra, rb), (rb, ra)] {
            for &(t, la, lo_a) in a.iter().filter(|r| r.0 >= lo && r.0 <= hi_t) {
                // The other aircraft's position at this time: its own report, or the straight line
                // between the two reports around it when they are not more than 30 s apart.
                let k = b.partition_point(|r| r.0 < t);
                let (Some(&(t1, la1, lo1)), prev) =
                    (b.get(k), k.checked_sub(1).and_then(|p| b.get(p)))
                else {
                    continue;
                };
                let (lb, lo_b) = if t1 == t {
                    (la1, lo1)
                } else {
                    let Some(&(t0, la0, lo0)) = prev else {
                        continue;
                    };
                    if t1 - t0 > 30 * S_NS {
                        continue;
                    }
                    let w = (t - t0) as f64 / (t1 - t0) as f64;
                    let mut d = lo1 - lo0;
                    if d > 180.0 {
                        d -= 360.0;
                    } else if d < -180.0 {
                        d += 360.0;
                    }
                    (la0 + w * (la1 - la0), lo0 + w * d)
                };
                let mut dl = lo_b - lo_a;
                if dl > 180.0 {
                    dl -= 360.0;
                } else if dl < -180.0 {
                    dl += 360.0;
                }
                let dy = (lb - la) * 111_200.0;
                let dx = dl * 111_200.0 * la.to_radians().cos();
                best = best.min(dx.hypot(dy));
            }
        }
        if best.is_finite() {
            out.insert((e.addr_a, e.addr_b), best as f32);
        }
    }
    out
}

/// The reports of a clip by UTC hour (Unix hour number and count), one per line.
pub fn hour_counts(r: &Reports) -> String {
    let mut by: BTreeMap<i64, u64> = BTreeMap::new();
    for i in 0..reports_len(r) {
        if let Some(&ns) = r.time_ns.get(i) {
            *by.entry(ns.div_euclid(3600 * S_NS)).or_default() += 1;
        }
    }
    let mut o = String::new();
    for (h, n) in by {
        o.push_str(&format!("hour_start_unix_s {} reports {n}\n", h * 3600));
    }
    o
}

/// The whole run: reads the clip, writes `encounters.csv` and `summary.txt` under `args.out`, and
/// returns the summary text.
pub fn run(args: &Args) -> Result<String, String> {
    // The seconds the arguments give become ns and ms before anything is read.
    let window_ns = args
        .window
        .map(|(from, to)| -> Result<(i64, i64), String> {
            let ns = |s: i64, what: &str| {
                s.checked_mul(S_NS)
                    .ok_or_else(|| format!("{what} {s} s is out of range"))
            };
            Ok((ns(from, "--from")?, ns(to, "--to")?))
        })
        .transpose()?;
    let lag_floor_ms = args
        .lag_floor_s
        .checked_mul(1000)
        .ok_or_else(|| format!("--lag-floor-s {} s is out of range", args.lag_floor_s))?;
    let clip =
        load_adsb_with(&args.adsb, &crate::guard::guarded_opener()).map_err(|e| e.to_string())?;
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
    if args.hours {
        return Ok(hour_counts(&reports));
    }
    let adsb_cfg = AdsbConfig::default();
    check_adsb_config(&adsb_cfg).map_err(|e| format!("adapter configuration: {e}"))?;
    let ground = ground_table(&reports);
    let (mut meas, _) = adsb_measurements(&reports, &adsb_cfg);
    drop(reports);
    if let Some((from_ns, to_ns)) = window_ns {
        meas.retain(|m| m.toa_ns >= from_ns && m.toa_ns < to_ns);
    }
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: S_NS,
        p90_ns: S_NS,
        p99_ns: S_NS,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let ecfg = tool_config();
    let rec = track_and_record(&meas, &cfg, &ground, &ecfg)?;
    fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let (airports, airports_from) = match &args.nasr {
        Some(p) => {
            let a = load_airports(p)?;
            let n = a.len();
            (
                a,
                format!(
                    "{n} public airports of {}",
                    p.file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                ),
            )
        }
        None => (
            Vec::new(),
            "none (no --nasr: the surface rule uses the ground state only)".to_string(),
        ),
    };
    let report_sep = report_separations(&meas, &rec.encounters);
    let mut join = String::new();
    if let Some(dir) = &args.sfdps {
        let queries: Vec<(u32, i64)> = rec
            .encounters
            .iter()
            .flat_map(|e| [(e.addr_a, e.closest_ns), (e.addr_b, e.closest_ns)])
            .collect();
        let sf = Sfdps {
            dir: dir.clone(),
            lag_floor_ms,
        };
        let (answers, st) = answer_queries(&sf, &queries)?;
        let window = args.window.map_or_else(
            || "the whole clip".to_string(),
            |(f, t)| format!("Unix seconds {f} up to {t}"),
        );
        join = join_text(
            &rec.encounters,
            &answers,
            &airports,
            ecfg.volume_v_ft as f32,
            &st,
            &window,
        );
    }
    let csv = encounters_csv(&rec.encounters, &airports, ecfg.volume_v_ft as f32);
    let csv_sha = to_hex(&sha256(csv.as_bytes()));
    let name = args
        .adsb
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut summary = summary_text(&SummaryInput {
        cfg: &ecfg,
        encounters: &rec.encounters,
        airports: &airports,
        airports_from: &airports_from,
        report_sep_m: &report_sep,
        totals: rec.totals,
        lat_limit_deg: rec.lat_limit_deg,
        log_overflow: rec.log_overflow,
        tracker_overflow: rec.tracker_overflow,
        measurements: meas.len() as u64,
        tick_rows: rec.tick_rows,
        clip_name: &name,
        clip_sha256: &clip_sha,
        scope: &scope,
        csv_sha256: &csv_sha,
    });
    summary.push_str(&join);
    for (file, text) in [("encounters.csv", &csv), ("summary.txt", &summary)] {
        let p = args.out.join(file);
        fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(summary)
}

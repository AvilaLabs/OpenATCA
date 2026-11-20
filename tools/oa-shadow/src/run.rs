//! The run: loads the two systems' recorded outputs and compares them. Shell (B3): files, the
//! tracker run and the table loads. OpenATCA's side is run as `examples/vs_eram.rs` runs it:
//! set A of the scorer's split is the tracker's input and set B alone builds the truth that is
//! used only for the adjudication column; ERAM's side is the SFDPS `TH` track positions the
//! benchmark reads, with the flight's address, callsign and last beacon code.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use oa_adsb::{Reports, F_GROUND, F_STALE, NONE_U16, ZAU_BOX};
use oa_bench::adsb_truth;

use oa_bench::input::{file_opener, load_adsb_with, load_with, FeedInput, Opener};
use oa_bench::legacy::{NONE, NO_ALT};
use oa_replay::FeedKind;
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    interval_stats, scoring_config, split_reports, IntervalStats, DRAIN_ROWS, SPLIT_TEXT,
};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_update, TrackOutput, DEFAULT_CONFIG, O_COAST, O_HAS_ALT, O_TICK, RADAR_KEY_BASE,
};
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

use crate::args::Args;
use crate::compare::compare_all;
use crate::guard::refuse_sealed;
use crate::model::{EramFlight, EramSample, IdentPoint, OaAircraft, OaSample};
use crate::report::render;
use crate::screen::{
    build_samples, by_address, quantile, render_screen, screen_fleet, RawReport, SampleRow, Tally,
    TruthPoint, TURN_SPAN_MS,
};
use crate::window::{extent, overlap, restrict_eram, restrict_oa, utc_text};

/// Whether a position (1e-7 degree) is inside the clip box both systems are restricted to.
pub fn in_box(lat_e7: i32, lon_e7: i32) -> bool {
    (ZAU_BOX.lat_min_e7..=ZAU_BOX.lat_max_e7).contains(&lat_e7)
        && (ZAU_BOX.lon_min_e7..=ZAU_BOX.lon_max_e7).contains(&lon_e7)
}

fn text8(c: [u8; 8]) -> Option<[u8; 8]> {
    c.iter().any(|&b| b != 0 && b != b' ').then_some(c)
}

/// The identity change points of each address in `reports`: the squawk and callsign carried
/// forward from the last report that gave one, a point only when the pair changes.
pub fn ident_points(reports: &Reports) -> BTreeMap<u32, Vec<IdentPoint>> {
    let mut order: Vec<(u32, i64, usize)> = reports
        .icao
        .iter()
        .zip(&reports.time_ns)
        .enumerate()
        .map(|(i, (&a, &t))| (a, t, i))
        .collect();
    order.sort_unstable();
    let mut out: BTreeMap<u32, Vec<IdentPoint>> = BTreeMap::new();
    let mut cur: Option<(u32, Option<u16>, Option<[u8; 8]>)> = None;
    for (addr, t_ns, i) in order {
        let sq = reports.squawk.get(i).copied().filter(|&s| s != NONE_U16);
        let cs = reports.callsign.get(i).copied().and_then(text8);
        let (mut psq, mut pcs) = match cur {
            Some((a, s, c)) if a == addr => (s, c),
            _ => (None, None),
        };
        psq = sq.or(psq);
        pcs = cs.or(pcs);
        let list = out.entry(addr).or_default();
        if list.last().map(|p| (p.squawk, p.callsign)) != Some((psq, pcs)) {
            list.push(IdentPoint {
                t_ms: t_ns.div_euclid(1_000_000),
                squawk: psq,
                callsign: pcs,
            });
        }
        cur = Some((addr, psq, pcs));
    }
    out
}

fn collect_ticks(out: &TrackOutput, rows: &mut BTreeMap<u32, Vec<OaSample>>) {
    for i in 0..output_len(out) {
        let (Some(&flags), Some(&addr), Some(&t_ns), Some(&lat), Some(&lon)) = (
            out.flags.get(i),
            out.addr.get(i),
            out.t_ns.get(i),
            out.lat_e7.get(i),
            out.lon_e7.get(i),
        ) else {
            continue;
        };
        if flags & O_TICK == 0 || addr >= RADAR_KEY_BASE || !in_box(lat, lon) {
            continue;
        }
        let alt = out
            .alt_ft
            .get(i)
            .copied()
            .filter(|_| flags & O_HAS_ALT != 0);
        rows.entry(addr).or_default().push(OaSample {
            t_ms: t_ns.div_euclid(1_000_000),
            lat_e7: lat,
            lon_e7: lon,
            alt_ft: alt,
            vel_east_mps: out.vel_east_mps.get(i).copied().unwrap_or(0.0),
            vel_north_mps: out.vel_north_mps.get(i).copied().unwrap_or(0.0),
            coast: flags & O_COAST != 0,
        });
    }
}

/// The nonzero counts of an ingest, as header lines for the summary.
pub fn load_notes(c: &oa_bench::legacy::Counts) -> Vec<String> {
    let mut v = Vec::new();
    let mut add = |label: &str, n: u64| {
        if n > 0 {
            v.push(format!("load: {label} {n}"));
        }
    };
    add(
        "SFDPS documents refused",
        c.doc_errors.first().copied().unwrap_or(0),
    );
    add("SFDPS messages rejected", c.sfdps_rejected);
    add("SFDPS field errors", c.sfdps_field_errors);
    add("rows not decoded", c.rows_not_decoded);
    add("flights dropped (table full)", c.overflow_flights);
    add("track samples dropped (table full)", c.overflow_th);
    for (label, n) in &c.reasons {
        add(label, *n);
    }
    v
}

/// The opener every input file goes through: the file opener of `oa-bench` is itself guarded
/// (`oa-sealed`: checked before the open and again on the opened file), so a link inside an
/// unsealed directory is caught too.
pub fn guarded_opener() -> Opener {
    file_opener()
}

/// The ERAM flights of the SFDPS recording in the clip box, sorted by GUFI, with the input digest
/// lines added to `header`.
pub fn load_eram(
    args: &Args,
    open: &Opener,
    header: &mut Vec<String>,
    sealed: &mut u64,
) -> Result<Vec<EramFlight>, String> {
    let loaded = load_with(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: args.sfdps_dir.clone(),
        }],
        open,
    )
    .map_err(|e| format!("{}: {e}", args.sfdps_dir.display()))?;
    // SFDPS frames (by receive time) and records (by their own time) left out for a sealed
    // window; the caller prints the one line after its last read.
    *sealed += loaded.sealed_window_dropped;
    for i in &loaded.inputs {
        header.push(format!(
            "SFDPS input: {} files, {} bytes, sha256 {}",
            i.files, i.bytes, i.sha256
        ));
    }
    let leg = &loaded.legacy;
    // Every nonzero load count is part of the report; a beacon table that overflowed means the
    // ERAM side is incomplete, which is an error.
    header.extend(load_notes(&leg.counts));
    if leg
        .counts
        .reasons
        .get(oa_bench::ingest::DROP_SFDPS_BEACON_FULL)
        .is_some_and(|&n| n > 0)
    {
        return Err("the SFDPS beacon table overflowed; the ERAM identity is incomplete".into());
    }
    let mut by_flight: BTreeMap<u32, Vec<EramSample>> = BTreeMap::new();
    for i in 0..leg.th.flight.len() {
        let (Some(&f), Some(&t), Some(&lat), Some(&lon), Some(&alt), Some(&coast)) = (
            leg.th.flight.get(i),
            leg.th.pos_ms.get(i),
            leg.th.lat_e7.get(i),
            leg.th.lon_e7.get(i),
            leg.th.alt_ft.get(i),
            leg.th.coast.get(i),
        ) else {
            continue;
        };
        if in_box(lat, lon) {
            by_flight.entry(f).or_default().push(EramSample {
                t_ms: t,
                lat_e7: lat,
                lon_e7: lon,
                alt_ft: (alt != NO_ALT).then_some(alt),
                coast: coast != 0,
            });
        }
    }
    let mut beacons: BTreeMap<u32, Vec<(i64, u16)>> = BTreeMap::new();
    for i in 0..leg.beacons.flight.len() {
        if let (Some(&f), Some(&t), Some(&c)) = (
            leg.beacons.flight.get(i),
            leg.beacons.ts_ms.get(i),
            leg.beacons.code.get(i),
        ) {
            beacons.entry(f).or_default().push((t, c));
        }
    }
    for v in beacons.values_mut() {
        v.sort_by_key(|b| b.0);
    }
    let mut eram: Vec<EramFlight> = Vec::new();
    for (f, mut samples) in by_flight {
        let fi = f as usize;
        let Some(&gufi) = leg.flights.gufi.get(fi) else {
            continue;
        };
        samples.sort_by_key(|s| s.t_ms);
        eram.push(EramFlight {
            gufi,
            addr: leg.flights.addr.get(fi).copied().filter(|&a| a != NONE),
            beacons: beacons.remove(&f).unwrap_or_default(),
            callsign: leg.flights.callsign.get(fi).copied().and_then(text8),
            samples,
        });
    }
    eram.sort_by_key(|f| f.gufi);
    Ok(eram)
}

/// Runs the whole comparison and writes `events.csv`, `flights.csv` and `summary.txt` to the
/// output directory; returns the summary text.
pub fn run(args: &Args) -> Result<String, String> {
    for p in [&args.adsb, &args.sfdps_dir, &args.out] {
        refuse_sealed(p)?;
    }
    let open = guarded_opener();
    // The files of the SFDPS folder are guarded as they are listed, before anything is created.
    oa_bench::input::list_inputs(&args.sfdps_dir)
        .map_err(|e| format!("{}: {e}", args.sfdps_dir.display()))?;
    fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let mut header: Vec<String> = Vec::new();

    // OpenATCA: the tracker on set A.
    let clip =
        load_adsb_with(&args.adsb, &open).map_err(|e| format!("{}: {e}", args.adsb.display()))?;
    header.push(format!("ADS-B clip sha256 {}", clip.hash.sha256));
    let mut sealed = clip.sealed_window_dropped;
    let (set_a, set_b, split) = split_reports(&clip.reports);
    oa_bench::split_check::check_split(&set_a, &set_b)
        .map_err(|v| format!("split refused: {v:?}"))?;
    drop(clip);
    header.push(format!("split: {SPLIT_TEXT}"));
    let mut idents = ident_points(&set_a);
    let (meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    drop(set_a);
    header.push(format!(
        "set A {} reports -> {} tracker measurements; set B {} reports -> truth (adjudication only)",
        split.set_a,
        meas.len(),
        split.set_b
    ));
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let mut tracker = tracker_new(&cfg).map_err(|e| format!("tracker: {e}"))?;
    let mut scratch = output_new(cfg.out_capacity);
    let mut rows: BTreeMap<u32, Vec<OaSample>> = BTreeMap::new();
    for m in &meas {
        tracker_update(&mut tracker, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            collect_ticks(&scratch, &mut rows);
            output_clear(&mut scratch);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut tracker, end, &mut scratch);
    tracker_flush(&mut tracker, end, &mut scratch);
    collect_ticks(&scratch, &mut rows);
    if scratch.overflow > 0 {
        return Err(format!(
            "the tracker lost {} output rows to a full table; the run is not complete",
            scratch.overflow
        ));
    }
    drop((tracker, meas));
    eprintln!("tracker done");
    let mut oa: BTreeMap<u32, OaAircraft> = rows
        .into_iter()
        .map(|(addr, mut samples)| {
            samples.sort_by_key(|s| s.t_ms);
            samples.dedup_by_key(|s| s.t_ms);
            let idents = idents.remove(&addr).unwrap_or_default();
            (
                addr,
                OaAircraft {
                    addr,
                    samples,
                    idents,
                },
            )
        })
        .collect();

    // ERAM: the SFDPS track positions in the box.
    let mut eram = load_eram(args, &open, &mut header, &mut sealed)?;
    // After the last read: the clip's count and the SFDPS loader's, each input once; the line is
    // in the summary header and on standard error.
    header.push(oa_sealed::drop_line(sealed));
    header.push(
        "  (the line above counts ADS-B rows and SFDPS frames by receive time and records by their own time)"
            .to_string(),
    );
    eprintln!("{}", oa_sealed::drop_line(sealed));
    // The recorded extents inside the box (the clip is boxed, so only those are comparable).
    let eram_extent = extent(eram.iter().flat_map(|f| f.samples.iter().map(|s| s.t_ms)));
    let oa_extent = extent(
        oa.values()
            .flat_map(|a| a.samples.first().zip(a.samples.last()))
            .flat_map(|(f, l)| [f.t_ms, l.t_ms]),
    );
    let (lo, hi) = eram_extent
        .zip(oa_extent)
        .and_then(|(a, b)| overlap(a, b))
        .ok_or("the two recordings do not overlap in time")?;
    header.push(format!(
        "comparison window (overlap of the two in-box recorded extents, both cut to it): {} to {} UTC",
        utc_text(lo),
        utc_text(hi)
    ));
    restrict_eram(&mut eram, lo, hi);
    restrict_oa(&mut oa, lo, hi);
    eprintln!("ERAM flights in the box: {}", eram.len());

    // Truth for the adjudication column, from set B only.
    let built = adsb_truth::build(&set_b, &TRUTH_DEFAULT).map_err(|e| format!("truth: {e}"))?;
    drop(set_b);
    let truth_at = |addr: u32, t_ms: i64| {
        adsb_truth::query(&built.truth, addr, t_ms)
            .ok()
            .map(|s| (s.lat, s.lon))
    };
    let comparison = compare_all(&eram, &oa, Some(&truth_at));
    let rendered = render(&comparison, &header);
    for (name, body) in [
        ("events.csv", &rendered.events_csv),
        ("flights.csv", &rendered.flights_csv),
        ("summary.txt", &rendered.summary),
    ] {
        fs::write(args.out.join(name), body)
            .map_err(|e| format!("{}: {e}", args.out.join(name).display()))?;
    }
    Ok(rendered.summary)
}

/// Whether a report's source type is ADS-B (or the clip has no source column).
fn is_adsb_source(source: u8) -> bool {
    // adsb_icao, adsb_icao_nt, adsb_other; NONE_U8 is a file without the column.
    matches!(source, 0 | 1 | 8) || source == oa_adsb::NONE_U8
}

/// The raw ADS-B reports of the wanted addresses, in strict time order per address: a usable
/// position (not stale, not on the ground, ADS-B source, ICAO address), the velocity when the
/// report has a ground speed and a track, and the NACp carried forward from the last known one
/// within the truth configuration's carry age.
pub fn raw_reports(reports: &Reports, wanted: &BTreeSet<u32>) -> BTreeMap<u32, Vec<RawReport>> {
    let mut order: Vec<(u32, i64, usize)> = Vec::new();
    for (i, (&a, &t)) in reports.icao.iter().zip(&reports.time_ns).enumerate() {
        let flags = reports.flags.get(i).copied().unwrap_or(0);
        let source = reports.source.get(i).copied().unwrap_or(oa_adsb::NONE_U8);
        if wanted.contains(&a)
            && flags & (F_STALE | F_GROUND) == 0
            && is_adsb_source(source)
            && reports.lat_e7.get(i).is_some_and(|&v| v != 0)
        {
            order.push((a, t, i));
        }
    }
    order.sort_unstable();
    let carry_ns = TRUTH_DEFAULT.quality_carry_ms * 1_000_000;
    let mut out: BTreeMap<u32, Vec<RawReport>> = BTreeMap::new();
    let mut held: Option<(u32, u8, i64)> = None;
    for (addr, t_ns, i) in order {
        let (Some(&lat), Some(&lon)) = (reports.lat_e7.get(i), reports.lon_e7.get(i)) else {
            continue;
        };
        // 0 is "unknown" and an absent value is NONE_U8: neither updates the carried NACp.
        let own = reports.nac_p.get(i).copied().unwrap_or(0);
        if (1..=15).contains(&own) {
            held = Some((addr, own, t_ns));
        }
        let nacp = match held {
            Some((a, v, at)) if a == addr && t_ns - at <= carry_ns => v,
            _ => 0,
        };
        let vel = match (reports.gs_dkt.get(i), reports.track_ddeg.get(i)) {
            (Some(&g), Some(&k)) if g != NONE_U16 && k != NONE_U16 => {
                let v = f64::from(g) * 0.1 * 0.514_444_444_444;
                let a = (f64::from(k) * 0.1).to_radians();
                Some(((v * a.sin()) as f32, (v * a.cos()) as f32))
            }
            _ => None,
        };
        let t_ms = t_ns.div_euclid(1_000_000);
        let list = out.entry(addr).or_default();
        // One report per millisecond: a repeat of the same time is dropped.
        if list.last().is_none_or(|p| p.t_ms < t_ms) {
            list.push(RawReport {
                t_ms,
                lat_e7: lat,
                lon_e7: lon,
                vel,
                nacp,
            });
        }
    }
    out
}

/// The smoother's state of `addr` at `t_ms` with its turn rate (the heading change over
/// [`TURN_SPAN_MS`] either side); `None` when it has no state there.
pub fn truth_point(truth: &oa_truth::Truth, addr: u32, t_ms: i64) -> Option<TruthPoint> {
    let q = adsb_truth::query(truth, addr, t_ms).ok()?;
    let heading = |t: i64| {
        adsb_truth::query(truth, addr, t)
            .ok()
            .map(|s| s.vel_east_mps.atan2(s.vel_north_mps))
    };
    let turn_dps = heading(t_ms - TURN_SPAN_MS)
        .zip(heading(t_ms + TURN_SPAN_MS))
        .map(|(a, b)| {
            let d = (b - a + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI)
                - std::f64::consts::PI;
            d.to_degrees().abs() / (2.0 * TURN_SPAN_MS as f64 / 1000.0)
        });
    Some(TruthPoint {
        lat_deg: q.lat,
        lon_deg: q.lon,
        vel_east_mps: q.vel_east_mps,
        vel_north_mps: q.vel_north_mps,
        sigma95_m: q.sigma95_h_m,
        vert_fpm: q.vert_rate_fpm.map(f64::from),
        turn_dps,
    })
}

/// Runs the ADS-B persistent-error screen and writes `aircraft.csv` and `summary.txt` to the
/// output directory; returns the summary text.
pub fn run_screen(args: &Args) -> Result<String, String> {
    for p in [&args.adsb, &args.sfdps_dir, &args.out] {
        refuse_sealed(p)?;
    }
    let open = guarded_opener();
    // The files of the SFDPS folder are guarded as they are listed, before anything is created.
    oa_bench::input::list_inputs(&args.sfdps_dir)
        .map_err(|e| format!("{}: {e}", args.sfdps_dir.display()))?;
    fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let mut header: Vec<String> = Vec::new();
    let mut sealed = 0u64;
    let mut eram = load_eram(args, &open, &mut header, &mut sealed)?;
    let wanted: BTreeSet<u32> = eram.iter().filter_map(|f| f.addr).collect();
    let clip =
        load_adsb_with(&args.adsb, &open).map_err(|e| format!("{}: {e}", args.adsb.display()))?;
    header.push(format!("ADS-B clip sha256 {}", clip.hash.sha256));
    // After the last read: the SFDPS loader's count and the clip's, each input once; the line is
    // in the summary header and on standard error.
    sealed += clip.sealed_window_dropped;
    header.push(oa_sealed::drop_line(sealed));
    header.push(
        "  (the line above counts ADS-B rows and SFDPS frames by receive time and records by their own time)"
            .to_string(),
    );
    eprintln!("{}", oa_sealed::drop_line(sealed));
    let raw = raw_reports(&clip.reports, &wanted);
    // The smoother, as the scorer builds it (default settings), over every report of the clip.
    let built =
        adsb_truth::build(&clip.reports, &TRUTH_DEFAULT).map_err(|e| format!("truth: {e}"))?;
    header.push(format!("truth digest {}", built.digest));
    drop(clip);
    let gaps: Vec<f64> = raw
        .values()
        .flat_map(|v| {
            v.windows(2).filter_map(|w| match w {
                [a, b] => Some((b.t_ms - a.t_ms) as f64 / 1000.0),
                _ => None,
            })
        })
        .collect();
    let within = gaps.iter().filter(|&&g| g <= 2.0).count();
    header.push(format!(
        "ADS-B report spacing of the addressed aircraft (consecutive reports): p50 {:.1} s, p90 {:.1} s, {:.1} % within 2 s",
        quantile(&gaps, 0.5).unwrap_or(f64::NAN),
        quantile(&gaps, 0.9).unwrap_or(f64::NAN),
        100.0 * within as f64 / gaps.len().max(1) as f64,
    ));
    let eram_extent = extent(eram.iter().flat_map(|f| f.samples.iter().map(|s| s.t_ms)));
    let adsb_extent = extent(raw.values().flat_map(|v| {
        v.first()
            .zip(v.last())
            .into_iter()
            .flat_map(|(f, l)| [f.t_ms, l.t_ms])
    }));
    let (lo, hi) = eram_extent
        .zip(adsb_extent)
        .and_then(|(a, b)| overlap(a, b))
        .ok_or("the two recordings do not overlap in time")?;
    header.push(format!(
        "comparison window (overlap of ERAM's in-box extent and the ADS-B reports of its addressed flights): {} to {} UTC",
        utc_text(lo),
        utc_text(hi)
    ));
    restrict_eram(&mut eram, lo, hi);
    let paired = eram
        .iter()
        .filter_map(|f| f.addr.filter(|a| raw.contains_key(a)).map(|a| (a, f)));
    let groups = by_address(paired);
    let flights_paired: u64 = groups.values().map(|v| v.len() as u64).sum();
    let mut tally = Tally::default();
    let mut inputs: Vec<(u32, Vec<SampleRow>)> = Vec::new();
    for (addr, flights) in &groups {
        let Some(r) = raw.get(addr) else { continue };
        let at = |t_ms: i64| truth_point(&built.truth, *addr, t_ms);
        let (rows, t) = build_samples(r, flights, &at);
        tally.add(&t);
        if !rows.is_empty() {
            inputs.push((*addr, rows));
        }
    }
    let t_fleet = std::time::Instant::now();
    let fleet = screen_fleet(&inputs, true)?;
    eprintln!(
        "fleet fits and stage (b): {:.1} s",
        t_fleet.elapsed().as_secs_f64()
    );
    let rendered = render_screen(&fleet, &header, flights_paired, &tally);
    for (name, body) in [
        ("aircraft.csv", &rendered.aircraft_csv),
        ("summary.txt", &rendered.summary),
    ] {
        fs::write(args.out.join(name), body)
            .map_err(|e| format!("{}: {e}", args.out.join(name).display()))?;
    }
    Ok(rendered.summary)
}

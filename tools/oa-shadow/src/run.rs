//! The run: loads the two systems' recorded outputs and compares them. Shell (B3): files, the
//! tracker run and the table loads. OpenATCA's side is run as `examples/vs_eram.rs` runs it:
//! set A of the scorer's split is the tracker's input and set B alone builds the truth that is
//! used only for the adjudication column; ERAM's side is the SFDPS `TH` track positions the
//! benchmark reads, with the flight's address, callsign and last beacon code.

use std::collections::BTreeMap;
use std::fs;

use oa_adsb::{Reports, NONE_U16, ZAU_BOX};
use oa_bench::adsb_truth;
use std::io;
use std::rc::Rc;

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

/// The opener every input file goes through: the path is canonicalised and a sealed target is
/// refused before the file is opened, so a link inside an unsealed directory is caught too.
pub fn guarded_opener() -> Opener {
    let inner = file_opener();
    Rc::new(move |p: &std::path::Path| {
        refuse_sealed(p).map_err(io::Error::other)?;
        inner(p)
    })
}

/// Runs the whole comparison and writes `events.csv`, `flights.csv` and `summary.txt` to the
/// output directory; returns the summary text.
pub fn run(args: &Args) -> Result<String, String> {
    for p in [&args.adsb, &args.sfdps_dir, &args.out] {
        refuse_sealed(p)?;
    }
    let open = guarded_opener();
    fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let mut header: Vec<String> = Vec::new();

    // OpenATCA: the tracker on set A.
    let clip =
        load_adsb_with(&args.adsb, &open).map_err(|e| format!("{}: {e}", args.adsb.display()))?;
    header.push(format!("ADS-B clip sha256 {}", clip.hash.sha256));
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
    let loaded = load_with(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: args.sfdps_dir.clone(),
        }],
        &open,
    )
    .map_err(|e| format!("{}: {e}", args.sfdps_dir.display()))?;
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
    // The recorded extents inside the box (the clip is boxed, so only those are comparable).
    let eram_extent = extent(by_flight.values().flat_map(|v| v.iter().map(|s| s.t_ms)));
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
    restrict_eram(&mut eram, lo, hi);
    restrict_oa(&mut oa, lo, hi);
    eprintln!("ERAM flights in the box: {}", eram.len());
    drop(loaded);

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

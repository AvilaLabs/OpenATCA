//! Shell (B1): feeds the rows of the two systems into their exports. OpenATCA: the tracker is
//! run on the ADS-B clip exactly as the scorer (`examples/p002.rs`) runs it, set A of the split,
//! the scoring configuration, and its one-second tick rows are drained into the export as they are
//! produced (nothing is spilled or held). ERAM: the legacy tables the scorer reads (`TH` track
//! samples with the flight's address and callsign) are sorted by position time and written.

use oa_bench::legacy::{Counts, Legacy, NONE, NO_ALT, NO_VEL};
use oa_track::score::{
    interval_stats, scoring_config, split_reports_with, IntervalStats, SplitStats, DRAIN_ROWS,
};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG, O_COAST, O_HAS_ALT,
    O_IDENTITY_UNCONFIRMED, O_RADAR_DISAGREES, O_REACQUIRED, O_TICK, O_UNCERTAIN, RADAR_KEY_BASE,
};

use std::collections::{BTreeMap, HashMap, HashSet};

use super::adsb_export::AdsbExport;
use super::error::ExportError;
use super::export::SystemExport;
use crate::core::addrcheck::{check_flight, RefPoint, Verdict};
use crate::core::adsb::{adsb_record, AdsbRow};
use crate::core::convert::{eram_record, track_record, EramRow, TrackRow};

/// What a tracker run did besides the rows exported. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackRunInfo {
    pub measurements: u64,
    /// Tick rows exported (`--openatca-rows ticks`).
    pub tick_rows: u64,
    /// Update rows (the state at each applied report) exported (`--openatca-rows updates`).
    pub update_rows: u64,
    /// Update rows not exported (in `ticks` mode, all of them).
    pub update_rows_not_exported: u64,
    /// Tick rows not exported (in `updates` mode, all of them).
    pub tick_rows_not_exported: u64,
    /// Update rows left out because an earlier row was already written for a later time (the
    /// file is in time order); `updates` mode only.
    pub updates_out_of_order: u64,
    /// Time of the last update row written, Unix ns.
    pub last_update_ns: Option<i64>,
    /// Exported rows carrying `O_IDENTITY_UNCONFIRMED`.
    pub unconfirmed_identity: u64,
    /// Rows the tracker lost to a full output table; must be 0.
    pub overflow: u64,
    /// Exported rows carrying `O_UNCERTAIN`, `O_REACQUIRED`, `O_COAST`.
    pub uncertain: u64,
    pub reacquired: u64,
    pub coasting: u64,
}

/// The row at index `i` of `out` and the key of its track.
fn track_row(out: &TrackOutput, i: usize, mono: bool) -> Option<(TrackRow, u32)> {
    let flags = *out.flags.get(i)?;
    let addr = *out.addr.get(i)?;
    let confirmed = flags & O_IDENTITY_UNCONFIRMED == 0 && addr < RADAR_KEY_BASE;
    let row = TrackRow {
        t_ns: *out.t_ns.get(i)?,
        lat_e7: *out.lat_e7.get(i)?,
        lon_e7: *out.lon_e7.get(i)?,
        vel_east_mps: *out.vel_east_mps.get(i)?,
        vel_north_mps: *out.vel_north_mps.get(i)?,
        pos_cov_m2: *out.pos_cov_m2.get(i)?,
        alt_ft: *out.alt_ft.get(i)?,
        has_alt: flags & O_HAS_ALT != 0,
        vrate_fpm: *out.vrate_fpm.get(i)?,
        coast: flags & O_COAST != 0,
        radar_disagrees: flags & O_RADAR_DISAGREES != 0,
        uncertain: flags & O_UNCERTAIN != 0,
        reacquired: flags & O_REACQUIRED != 0,
        identity_unconfirmed: flags & O_IDENTITY_UNCONFIRMED != 0,
        address: confirmed.then_some(addr),
        monosensor: mono,
    };
    Some((row, addr))
}

/// Which rows of the tracker are exported (`--openatca-rows`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenatcaRows {
    /// One row per second per track (the fixed-rate output). The default.
    Ticks,
    /// The tracker's update rows: the state at each applied report, at the report's time.
    Updates,
}

/// Offers the rows of `out` that `rows` selects to `ex`; the others are counted.
pub fn export_output(
    out: &TrackOutput,
    ex: &mut SystemExport,
    info: &mut TrackRunInfo,
    mono: bool,
    rows: OpenatcaRows,
) -> Result<(), ExportError> {
    for i in 0..output_len(out) {
        let Some((row, key)) = track_row(out, i, mono) else {
            return Err(ExportError::Input(format!(
                "tracker output row {i} has a column shorter than the table"
            )));
        };
        let is_tick = out.flags.get(i).is_some_and(|f| f & O_TICK != 0);
        let wanted = is_tick == (rows == OpenatcaRows::Ticks);
        if !wanted {
            if is_tick {
                info.tick_rows_not_exported = info.tick_rows_not_exported.saturating_add(1);
            } else {
                info.update_rows_not_exported = info.update_rows_not_exported.saturating_add(1);
            }
            continue;
        }
        if rows == OpenatcaRows::Updates {
            // Update rows come in the order the reports were applied; a late report can give a
            // row earlier than one already written, and the file is in time order.
            if info.last_update_ns.is_some_and(|t| row.t_ns < t) {
                info.updates_out_of_order = info.updates_out_of_order.saturating_add(1);
                continue;
            }
            info.last_update_ns = Some(row.t_ns);
            info.update_rows = info.update_rows.saturating_add(1);
        } else {
            info.tick_rows = info.tick_rows.saturating_add(1);
        }
        info.uncertain = info.uncertain.saturating_add(u64::from(row.uncertain));
        info.reacquired = info.reacquired.saturating_add(u64::from(row.reacquired));
        info.coasting = info.coasting.saturating_add(u64::from(row.coast));
        info.unconfirmed_identity = info
            .unconfirmed_identity
            .saturating_add(u64::from(row.identity_unconfirmed));
        match track_record(&row) {
            Ok(c) => ex.push(&c, key, (row.lat_e7, row.lon_e7))?,
            Err(r) => ex.refuse(r),
        }
    }
    Ok(())
}

/// Runs the tracker over `meas` and exports its rows (`rows`). `mono` is true when `meas` came from
/// one kind of sensor (I062/080 MON).
pub fn run_tracker(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    ex: &mut SystemExport,
    mono: bool,
    rows: OpenatcaRows,
) -> Result<TrackRunInfo, ExportError> {
    let mut t = tracker_new(cfg).map_err(|e| ExportError::Input(format!("tracker: {e}")))?;
    let mut scratch = output_new(cfg.out_capacity);
    let mut info = TrackRunInfo {
        measurements: meas.len() as u64,
        ..TrackRunInfo::default()
    };
    for m in meas {
        tracker_update(&mut t, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            export_output(&scratch, ex, &mut info, mono, rows)?;
            output_clear(&mut scratch);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut scratch);
    tracker_flush(&mut t, end, &mut scratch);
    export_output(&scratch, ex, &mut info, mono, rows)?;
    info.overflow = scratch.overflow;
    if info.overflow > 0 {
        return Err(ExportError::Input(format!(
            "the tracker lost {} output rows to a full table; the run is not complete",
            info.overflow
        )));
    }
    Ok(info)
}

/// The scoring configuration for `meas`, as the scorer builds it.
pub fn scorer_config(meas: &[Measurement]) -> TrackerConfig {
    let iv = interval_stats(meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    scoring_config(&DEFAULT_CONFIG, &iv)
}

/// Which ADS-B reports the tracker's adapter is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputsMode {
    /// Set A of the scorer's split only (half of the reports; set B is the independent
    /// reference), the scorer's own run.
    Split,
    /// Every report of the clip, as ERAM gets all its inputs.
    All,
}

/// What the clip's split and the ADS-B file did. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipInfo {
    /// Reports offered to the tracker's adapter.
    pub offered: u64,
    /// Reports not offered (set B of the split, empty in `all` mode).
    pub not_offered: u64,
    /// What the shared split code removed and kept.
    pub split: SplitStats,
    /// The ADS-B file, when one was asked for.
    pub adsb: Option<AdsbInfo>,
    /// What the tracker's own adapter (oa-truth's quality gates and the rest) removed from the
    /// reports it was offered.
    pub gate: oa_track::adsb::AdsbStats,
}

/// What became of the reports offered to the ADS-B file. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdsbInfo {
    pub rows_in: u64,
    /// Position marked stale by the source: not a current position, left out.
    pub stale: u64,
    /// Source types that are not ADS-B (`oa_truth::ADSB_SOURCES`), by name.
    pub other_sources: Vec<(String, u64)>,
    pub converted: u64,
    /// The reports written, ICAO addresses only, sorted by (address, time): what the ERAM
    /// address check compares the flights with.
    pub positions: Vec<RefPoint>,
}

/// The ADS-B reports as CAT021 rows, in time order (time, address, input order).
pub fn export_adsb_reports(
    reports: &oa_adsb::Reports,
    ax: &mut AdsbExport,
) -> Result<AdsbInfo, ExportError> {
    let n = oa_adsb::reports_len(reports);
    let mut order: Vec<(i64, u32, u32)> = Vec::with_capacity(n);
    for (i, (&t, &a)) in reports.time_ns.iter().zip(reports.icao.iter()).enumerate() {
        let Ok(ix) = u32::try_from(i) else {
            return Err(ExportError::Input("more than 2^32 ADS-B reports".into()));
        };
        order.push((t, a, ix));
    }
    order.sort_unstable();
    let mut info = AdsbInfo {
        rows_in: n as u64,
        ..AdsbInfo::default()
    };
    let mut other = [0u64; 256];
    for &(_, _, i) in &order {
        let Some(row) = oa_adsb::reports_row(reports, i as usize) else {
            return Err(ExportError::Input(format!(
                "ADS-B report {i} cannot be read back"
            )));
        };
        if row.flags & oa_adsb::F_STALE != 0 {
            info.stale = info.stale.saturating_add(1);
            continue;
        }
        if row.source != oa_adsb::NONE_U8 && !oa_truth::ADSB_SOURCES.contains(&row.source) {
            if let Some(c) = other.get_mut(usize::from(row.source)) {
                *c = c.saturating_add(1);
            }
            continue;
        }
        match adsb_record(&adsb_row(&row)) {
            Ok(c) => {
                ax.push(&c, (row.lat_e7, row.lon_e7))?;
                info.converted = info.converted.saturating_add(1);
                if row.icao & oa_adsb::NON_ICAO == 0 {
                    info.positions.push(RefPoint {
                        address: row.icao & 0xFF_FFFF,
                        t_ns: row.time_ns,
                        lat_e7: row.lat_e7,
                        lon_e7: row.lon_e7,
                    });
                }
            }
            Err(r) => ax.refuse(r),
        }
    }
    for (s, &c) in other.iter().enumerate() {
        if c > 0 {
            let name = oa_adsb::SOURCE_NAMES
                .get(s)
                .map_or_else(|| format!("source {s}"), |n| (*n).to_string());
            info.other_sources.push((name, c));
        }
    }
    info.positions.sort_unstable();
    Ok(info)
}

/// The conversion row of a report.
fn adsb_row(r: &oa_adsb::Row) -> AdsbRow {
    use oa_adsb::{NONE_I32, NONE_U16, NONE_U8, NON_ICAO, SIL_PER_HOUR, SIL_PER_SAMPLE};
    let u8_of = |v: u8| (v != NONE_U8).then_some(v);
    AdsbRow {
        time_ns: r.time_ns,
        address: r.icao & 0xFF_FFFF,
        non_icao: r.icao & NON_ICAO != 0,
        lat_e7: r.lat_e7,
        lon_e7: r.lon_e7,
        baro_alt_ft: (r.baro_alt_ft != NONE_I32).then_some(r.baro_alt_ft),
        geom_alt_ft: (r.geom_alt_ft != NONE_I32).then_some(r.geom_alt_ft),
        ground: r.flags & oa_adsb::F_GROUND != 0,
        vrate_fpm: (r.vrate_fpm != NONE_I32).then_some(r.vrate_fpm),
        rate_geometric: r.flags & oa_adsb::F_RATE_GEOM != 0,
        gs_dkt: (r.gs_dkt != NONE_U16).then_some(r.gs_dkt),
        track_ddeg: (r.track_ddeg != NONE_U16).then_some(r.track_ddeg),
        callsign: r.callsign,
        category: r.category,
        nic: u8_of(r.nic),
        nac_p: u8_of(r.nac_p),
        nac_v: u8_of(r.nac_v),
        sil: u8_of(r.sil),
        sil_per_sample: match r.sil_type {
            SIL_PER_SAMPLE => Some(true),
            SIL_PER_HOUR => Some(false),
            _ => None,
        },
        version: u8_of(r.version),
        gva: u8_of(r.gva),
        sda: u8_of(r.sda),
    }
}

/// Loads the ADS-B clip at `path`, removes the copies of reports through the scorer's own split
/// code (`split_reports_with`: the same function the scorer calls, with the halving on for
/// `Split` and off for `All`), runs the tracker on the reports `mode` selects and exports its
/// tick rows. The reports offered to the tracker's adapter are also written to `adsb`, if given. The tracker is
/// fed ADS-B only, so the tracks are monosensor.
///
/// Conflict rule: reports of one address and time with different content are all kept, in one
/// fixed order (the tracker sees them as the scorer's split gives them); only exact copies are
/// removed. This holds in both modes.
pub fn export_adsb_clip(
    path: &std::path::Path,
    ex: &mut SystemExport,
    adsb: Option<&mut AdsbExport>,
    mode: InputsMode,
    rows: OpenatcaRows,
) -> Result<(TrackRunInfo, ClipInfo), ExportError> {
    let clip = oa_bench::input::load_adsb(path)
        .map_err(|e| ExportError::Input(format!("{}: {e}", path.display())))?;
    let (fed_set, other, split) = split_reports_with(&clip.reports, mode == InputsMode::Split);
    drop(clip);
    let mut info = ClipInfo {
        offered: oa_adsb::reports_len(&fed_set) as u64,
        not_offered: oa_adsb::reports_len(&other) as u64,
        split,
        adsb: None,
        gate: oa_track::adsb::AdsbStats::default(),
    };
    drop(other);
    if let Some(ax) = adsb {
        info.adsb = Some(export_adsb_reports(&fed_set, ax)?);
    }
    let (meas, gate) =
        oa_track::adsb::adsb_measurements(&fed_set, &oa_track::adsb::AdsbConfig::default());
    info.gate = gate;
    drop(fed_set);
    let cfg = scorer_config(&meas);
    let run = run_tracker(&meas, &cfg, ex, true, rows)?;
    if run.overflow > 0 {
        return Err(ExportError::Input(format!(
            "the tracker lost {} output rows to a full table; the run is not complete",
            run.overflow
        )));
    }
    Ok((run, info))
}

/// A flight whose track contradicts the address it carries. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContradictedFlight {
    pub flight: u32,
    pub address: u32,
    /// The flight's callsign, NUL padding removed; empty if none.
    pub callsign: String,
    /// Track samples of the flight, and those paired with an ADS-B report of the address.
    pub samples: u64,
    pub pairs: u64,
    /// Median distance to the ADS-B reports of the address, metres.
    pub median_m: u64,
}

/// ERAM rows of one input: how many, and what was dropped or kept as a conflict. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EramInfo {
    pub rows_in: u64,
    /// Same flight, position time and content as an earlier row: dropped.
    pub exact_duplicates: u64,
    /// Same flight and position time, different content: both kept.
    pub conflicting_same_time: u64,
    /// Rows whose flight index is not in the flight table: refused.
    pub unknown_flight: u64,
    /// Rows written whose position time is on a whole second.
    pub whole_second: u64,
    /// Flights with at least one row, and how many carry an address.
    pub flights: u64,
    pub flights_with_address: u64,
    /// Whether the address check ran (ADS-B reports of the day were given), and its result by
    /// flight: the track agrees with the address, too little to compare, contradicts it.
    pub checked: bool,
    pub flights_agree: u64,
    pub flights_not_checked: u64,
    pub flights_contradicted: u64,
    /// The largest median distance among the flights that agree, metres: how far the rule's
    /// threshold is from what the agreeing flights do in this run.
    pub agree_max_median_m: u64,
    /// Rows written without I380 address because their flight contradicts it.
    pub rows_address_withheld: u64,
    pub contradicted: Vec<ContradictedFlight>,
    /// Addresses carried by more than one flight (flights with rows), and the first of them.
    pub addresses_on_several_flights: u64,
    pub several: Vec<(u32, u64)>,
}

/// Most earlier rows of one (flight, position time) group a row is compared with. A group with
/// more distinct rows than this keeps the rest as they come (no row is dropped on a comparison
/// that was not made).
const GROUP_WINDOW: usize = 16;

/// Every count `oa-bench` keeps of what the SFDPS ingest saw and dropped before and while filling
/// the TH table, as report lines (E6), and a refusal if the tables overflowed: a full flight or
/// track table loses rows, and an export that lost rows is not complete (E5).
pub fn eram_input_counts(c: &Counts) -> Result<Vec<(String, u64)>, ExportError> {
    if c.overflow_flights > 0 || c.overflow_th > 0 {
        return Err(ExportError::Input(format!(
            "the SFDPS ingest lost rows to a full table (flights {}, track samples {}); the export would be incomplete",
            c.overflow_flights, c.overflow_th
        )));
    }
    let mut v: Vec<(String, u64)> = vec![
        ("ingest: SFDPS documents offered".into(), c.docs[0]),
        (
            "ingest: SFDPS documents the parser or decoder refused".into(),
            c.doc_errors[0],
        ),
        (
            "ingest: SFDPS messages rejected by the decoder".into(),
            c.sfdps_rejected,
        ),
        (
            "ingest: rows of accepted documents past the per-document table size, not decoded"
                .into(),
            c.rows_not_decoded,
        ),
        (
            "ingest: SFDPS field errors in rows kept (field treated as absent)".into(),
            c.sfdps_field_errors,
        ),
        (
            "ingest: flight table overflow (must be 0)".into(),
            c.overflow_flights,
        ),
        (
            "ingest: track sample table overflow (must be 0)".into(),
            c.overflow_th,
        ),
        (
            "ingest: track samples with a coast indicator".into(),
            c.th_coast,
        ),
        (
            "ingest: track samples without a reported altitude".into(),
            c.th_no_alt,
        ),
    ];
    for (k, n) in &c.reasons {
        v.push((format!("ingest: {k}"), *n));
    }
    Ok(v)
}

/// Longest list of addresses on several flights written to the README.
const SEVERAL_LISTED: usize = 50;

/// Per flight: its rows and, for the address check, its positions.
struct FlightRows {
    rows: u64,
    samples: Vec<(i64, i32, i32)>,
}

/// The flights that contradict their address, and the counts by outcome, into `info`. Returns
/// the set of the contradicting flights.
fn address_check(
    leg: &Legacy,
    flights: &HashMap<u32, FlightRows>,
    refs: &[RefPoint],
    info: &mut EramInfo,
) -> HashSet<u32> {
    let mut withheld = HashSet::new();
    let mut ids: Vec<u32> = flights.keys().copied().collect();
    ids.sort_unstable();
    for f in ids {
        let Some(fr) = flights.get(&f) else { continue };
        let addr = leg.flights.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE || addr > 0x00FF_FFFF {
            continue;
        }
        match check_flight(addr, &fr.samples, refs) {
            Verdict::NotChecked { .. } => {
                info.flights_not_checked = info.flights_not_checked.saturating_add(1);
            }
            Verdict::Agrees { median_km, .. } => {
                info.flights_agree = info.flights_agree.saturating_add(1);
                info.agree_max_median_m = info.agree_max_median_m.max((median_km * 1000.0) as u64);
            }
            Verdict::Contradicts { pairs, median_km } => {
                info.flights_contradicted = info.flights_contradicted.saturating_add(1);
                let callsign = leg
                    .flights
                    .callsign
                    .get(f as usize)
                    .map(|c| {
                        c.iter()
                            .take_while(|&&b| b != 0)
                            .map(|&b| char::from(b))
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                info.contradicted.push(ContradictedFlight {
                    flight: f,
                    address: addr,
                    callsign,
                    samples: fr.samples.len() as u64,
                    pairs: pairs as u64,
                    median_m: (median_km * 1000.0) as u64,
                });
                withheld.insert(f);
            }
        }
    }
    withheld
}

/// Sorts the ERAM track samples by (position time, flight, input order) and exports them. With
/// `refs` (the ADS-B reports of the day, sorted by address and time) every flight is checked
/// against the reports of its own address, and a flight whose track contradicts its address is
/// written without the address.
pub fn export_eram(
    leg: &Legacy,
    ex: &mut SystemExport,
    refs: Option<&[RefPoint]>,
) -> Result<EramInfo, ExportError> {
    let ingest = eram_input_counts(&leg.counts)?;
    ex.counts.extend(ingest);
    let th = &leg.th;
    let n = th.flight.len();
    let mut order: Vec<(i64, u32, u32)> = Vec::with_capacity(n);
    let mut flights: HashMap<u32, FlightRows> = HashMap::new();
    for (i, (&t, &f)) in th.pos_ms.iter().zip(th.flight.iter()).enumerate() {
        let Ok(ix) = u32::try_from(i) else {
            return Err(ExportError::Input(
                "more than 2^32 ERAM track samples".into(),
            ));
        };
        order.push((t, f, ix));
        let fr = flights.entry(f).or_insert_with(|| FlightRows {
            rows: 0,
            samples: Vec::new(),
        });
        fr.rows = fr.rows.saturating_add(1);
        if refs.is_some() {
            if let (Some(&la), Some(&lo)) = (th.lat_e7.get(i), th.lon_e7.get(i)) {
                if let Some(ns) = t.checked_mul(1_000_000) {
                    fr.samples.push((ns, la, lo));
                }
            }
        }
    }
    order.sort_unstable();
    let mut info = EramInfo {
        rows_in: n as u64,
        checked: refs.is_some(),
        ..EramInfo::default()
    };
    // Flights with rows that the flight table knows, with an address.
    let mut by_address: BTreeMap<u32, u64> = BTreeMap::new();
    for &f in flights.keys() {
        if let Some(&a) = leg.flights.addr.get(f as usize) {
            info.flights = info.flights.saturating_add(1);
            if a != NONE {
                info.flights_with_address = info.flights_with_address.saturating_add(1);
                let c = by_address.entry(a).or_insert(0);
                *c = c.saturating_add(1);
            }
        }
    }
    for (&a, &c) in &by_address {
        if c > 1 {
            info.addresses_on_several_flights = info.addresses_on_several_flights.saturating_add(1);
            if info.several.len() < SEVERAL_LISTED {
                info.several.push((a, c));
            }
        }
    }
    let withheld = match refs {
        Some(r) => address_check(leg, &flights, r, &mut info),
        None => HashSet::new(),
    };
    drop(flights);
    let mut group: (i64, u32) = (i64::MIN, NONE);
    let mut seen: Vec<[i64; 6]> = Vec::with_capacity(GROUP_WINDOW);
    for &(t, f, i) in &order {
        let i = i as usize; // u32 to usize: lossless on the 64-bit targets this tool runs on
        let (Some(&lat), Some(&lon), Some(&alt), Some(&vx), Some(&vy), Some(&coast)) = (
            th.lat_e7.get(i),
            th.lon_e7.get(i),
            th.alt_ft.get(i),
            th.vx_dkt.get(i),
            th.vy_dkt.get(i),
            th.coast.get(i),
        ) else {
            return Err(ExportError::Input(format!(
                "ERAM sample {i} has a column shorter than the table"
            )));
        };
        let fi = f as usize;
        let (Some(&addr), Some(callsign)) =
            (leg.flights.addr.get(fi), leg.flights.callsign.get(fi))
        else {
            info.unknown_flight = info.unknown_flight.saturating_add(1);
            continue;
        };
        if group != (t, f) {
            group = (t, f);
            seen.clear();
        }
        let content = [
            i64::from(lat),
            i64::from(lon),
            i64::from(alt),
            i64::from(vx),
            i64::from(vy),
            i64::from(coast),
        ];
        if seen.contains(&content) {
            info.exact_duplicates = info.exact_duplicates.saturating_add(1);
            continue;
        }
        if !seen.is_empty() {
            info.conflicting_same_time = info.conflicting_same_time.saturating_add(1);
        }
        if seen.len() < GROUP_WINDOW {
            seen.push(content);
        }
        let is_withheld = withheld.contains(&f);
        let row = EramRow {
            pos_ms: t,
            lat_e7: lat,
            lon_e7: lon,
            alt_ft: (alt != NO_ALT).then_some(alt),
            vel_dkt: (vx != NO_VEL && vy != NO_VEL).then_some((vx, vy)),
            coast: coast != 0,
            address: (addr != NONE).then_some(addr),
            address_withheld: is_withheld,
            callsign: (*callsign != [0u8; 8]).then_some(*callsign),
        };
        match eram_record(&row) {
            Ok(c) => {
                ex.push(&c, f, (lat, lon))?;
                info.whole_second = info
                    .whole_second
                    .saturating_add(u64::from(t.rem_euclid(1000) == 0));
                info.rows_address_withheld = info
                    .rows_address_withheld
                    .saturating_add(u64::from(is_withheld && addr != NONE));
            }
            Err(r) => ex.refuse(r),
        }
    }
    Ok(info)
}

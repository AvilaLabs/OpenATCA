//! Shell (B1): feeds the rows of the two systems into their exports. OpenATCA: the tracker is
//! run on the ADS-B clip exactly as the scorer (`examples/p002.rs`) runs it, set A of the split,
//! the scoring configuration, and its one-second tick rows are drained into the export as they are
//! produced (nothing is spilled or held). ERAM: the legacy tables the scorer reads (`TH` track
//! samples with the flight's address and callsign) are sorted by position time and written.

use oa_bench::legacy::{Counts, Legacy, NONE, NO_ALT, NO_VEL};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG, O_COAST, O_HAS_ALT,
    O_IDENTITY_UNCONFIRMED, O_RADAR_DISAGREES, O_REACQUIRED, O_TICK, O_UNCERTAIN, RADAR_KEY_BASE,
};
use oa_track_tools::score::{
    interval_stats, scoring_config, split_reports_keyed, split_reports_with, IntervalStats,
    SplitStats, DRAIN_ROWS,
};

use std::collections::{BTreeMap, HashMap, HashSet};

use super::adsb_export::AdsbExport;
use super::error::ExportError;
use super::export::SystemExport;
use super::guard::guarded_opener;
use crate::core::addrcheck::{check_flight, RefPoint, Verdict};
use crate::core::adsb::{adsb_record, AdsbRow, LinkSource};
use crate::core::convert::{eram_record, eram_refusal, track_record, EramRow, TrackRow};

/// What a tracker run did besides the rows exported. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackRunInfo {
    pub measurements: u64,
    /// Tick rows written (`--openatca-rows ticks`); a row refused by the conversion is not one.
    pub tick_rows: u64,
    /// Update rows (the state at each applied report) written (`--openatca-rows updates`).
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
    /// Per address, the last time (Unix ns) at which each track instance the tracker dropped
    /// could still have a row: its last applied update plus the drop age, in time order. A row of
    /// the address later than the k-th of these is of an instance after the k-th: that is how a
    /// restart is told apart, from the tracker's own end events and not from gaps between rows.
    /// Valid only when the tracker never stepped its clock back: a step back drops every track
    /// updated after the new clock at once, at the step and not at `last + drop age`, so a run
    /// with `time_steps_back` or `time_future_dropped` above zero fails (`note_ends`) before
    /// these are used.
    /// An instance's rows are all at or before its own entry (a tick row is on the table through
    /// the tick at which the age equals the drop age) and the next instance of the address starts
    /// only after the sweep that dropped the old one, which is after it (age over the drop age).
    pub ends: BTreeMap<u32, Vec<i64>>,
}

impl TrackRunInfo {
    /// The instance (0, 1, ...) of the address `key` that a row at `t_ns` belongs to: how many
    /// dropped instances of the address ended before it.
    pub fn instance_of(&self, key: u32, t_ns: i64) -> u32 {
        self.ends.get(&key).map_or(0, |v| {
            u32::try_from(v.partition_point(|&end| end < t_ns)).unwrap_or(u32::MAX)
        })
    }
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

/// Offers the rows of `out` that `rows` selects to `ex`; the others are counted. The counters of
/// the exported rows (ticks, updates, coasting, uncertain, reacquired, unconfirmed identity) and
/// the time of the last update row change only for a row that was written; a row that is refused
/// is counted under the export's refusals and nowhere else. In `updates` mode the rows of one
/// call are taken in time order (stable for equal times), so that fewer fall behind a row already
/// written.
pub fn export_output(
    out: &TrackOutput,
    ex: &mut SystemExport,
    info: &mut TrackRunInfo,
    mono: bool,
    rows: OpenatcaRows,
) -> Result<(), ExportError> {
    let mut wanted: Vec<usize> = Vec::new();
    for i in 0..output_len(out) {
        let is_tick = out.flags.get(i).is_some_and(|f| f & O_TICK != 0);
        if is_tick == (rows == OpenatcaRows::Ticks) {
            wanted.push(i);
        } else if is_tick {
            info.tick_rows_not_exported = info.tick_rows_not_exported.saturating_add(1);
        } else {
            info.update_rows_not_exported = info.update_rows_not_exported.saturating_add(1);
        }
    }
    if rows == OpenatcaRows::Updates {
        wanted.sort_by_key(|&i| (out.t_ns.get(i).copied().unwrap_or(0), i));
    }
    for i in wanted {
        let Some((row, key)) = track_row(out, i, mono) else {
            return Err(ExportError::Input(format!(
                "tracker output row {i} has a column shorter than the table"
            )));
        };
        // Update rows come in the order the reports were applied; a late report can give a row
        // earlier than one already written, and the file is in time order.
        if rows == OpenatcaRows::Updates && info.last_update_ns.is_some_and(|t| row.t_ns < t) {
            info.updates_out_of_order = info.updates_out_of_order.saturating_add(1);
            continue;
        }
        match track_record(&row) {
            Ok(c) => {
                let segment = info.instance_of(key, row.t_ns);
                ex.push_segment(&c, key, segment, (row.lat_e7, row.lon_e7))?;
                if rows == OpenatcaRows::Updates {
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
            }
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
    let mut logged = 0usize;
    for m in meas {
        tracker_update(&mut t, m, &mut scratch);
        note_ends(&t, cfg.drop_after_ns, &mut logged, &mut info)?;
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            export_output(&scratch, ex, &mut info, mono, rows)?;
            output_clear(&mut scratch);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut scratch);
    tracker_flush(&mut t, end, &mut scratch);
    note_ends(&t, cfg.drop_after_ns, &mut logged, &mut info)?;
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

/// Reads the tracks the tracker has dropped since the last call from its log into `info.ends`.
/// A dropped track that was never confirmed has no rows and is not an instance. A full log would
/// lose events, so it fails the run.
pub(crate) fn note_ends(
    t: &oa_track::Tracker,
    drop_after_ns: i64,
    logged: &mut usize,
    info: &mut TrackRunInfo,
) -> Result<(), ExportError> {
    let log = oa_track::tracker_log(t);
    if log.overflow > 0 {
        return Err(ExportError::Input(format!(
            "the tracker's track log lost {} ended tracks to a full table; the track numbers would be wrong",
            log.overflow
        )));
    }
    // A step back drops every track updated after the new clock at once: the end of each is the
    // moment of the step, not `last + drop_after`, and the tracker's log does not say when that
    // was. The numbers would be wrong, so the run fails.
    let st = oa_track::tracker_stats(t);
    if st.time_steps_back > 0 || st.time_future_dropped > 0 {
        return Err(ExportError::Input(format!(
            "the tracker moved its clock back {} time(s) and dropped {} track(s), confirmed or not, built on the wrong clock; \
             track restarts cannot be placed in time, so the run is not complete",
            st.time_steps_back, st.time_future_dropped
        )));
    }
    for i in *logged..log.addr.len() {
        let (Some(&addr), Some(&last), Some(&conf), Some(&why)) = (
            log.addr.get(i),
            log.last_ns.get(i),
            log.confirmed_ns.get(i),
            log.reason.get(i),
        ) else {
            continue;
        };
        if conf != i64::MIN && why == oa_track::END_DROPPED {
            info.ends
                .entry(addr)
                .or_default()
                .push(last.saturating_add(drop_after_ns));
        }
    }
    *logged = log.addr.len();
    Ok(())
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
    /// Clip rows left out because their time is in a sealed window (OA-SEAL-014).
    pub sealed_window_dropped: u64,
    /// What the shared split code removed and kept.
    pub split: SplitStats,
    /// The ADS-B file, when one was asked for.
    pub adsb: Option<AdsbInfo>,
    /// Reports left for the file by its own copy removal ([`export_key`]); never fewer than
    /// `offered`, because its key is finer than the scorer's.
    pub file_offered: u64,
    /// What the tracker's own adapter (oa-truth's quality gates and the rest) removed from the
    /// reports it was offered.
    pub gate: oa_track::adsb::AdsbStats,
    /// Every ADS-B report of the whole clip (before the split and the duplicate removal), ICAO
    /// addresses only, not stale and of an ADS-B source type, sorted by (address, time): what the
    /// ERAM address check compares the flights with. Under `--inputs split` this includes set B.
    pub positions: Vec<RefPoint>,
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
    /// Reports whose position or time cannot be written (refused, counted by the file).
    pub refused: u64,
}

impl AdsbInfo {
    /// The reconciliation of what was offered: offered = stale + not ADS-B + written + refused,
    /// and the two counts the file itself gives: `in_file`, the records found in the written
    /// file by the format walker, equal to `written`, and `refused_listed`, the refusals the
    /// file listed, equal to `refused`. Returns the README line and whether everything agrees.
    pub fn reconciliation(&self, in_file: u64, refused_listed: u64) -> (String, bool) {
        let other: u64 = self.other_sources.iter().map(|(_, c)| *c).sum();
        let sum = self
            .stale
            .saturating_add(other)
            .saturating_add(self.converted)
            .saturating_add(self.refused);
        let ok = sum == self.rows_in && in_file == self.converted && refused_listed == self.refused;
        let verdict = if ok {
            "reconciles"
        } else {
            "DOES NOT RECONCILE"
        };
        let line = format!(
            "offered {} = stale {} + not ADS-B {} + written {} + refused {} = {}; records counted in the written file {} (written {}); refusals listed by the file {} (refused {}): {}",
            self.rows_in,
            self.stale,
            other,
            self.converted,
            self.refused,
            sum,
            in_file,
            self.converted,
            refused_listed,
            self.refused,
            verdict
        );
        (line, ok)
    }
}

/// The reference points of the address check: every report of `reports` that is not stale, of an
/// ADS-B source type and under an ICAO address, sorted by (address, time).
pub fn ref_points(reports: &oa_adsb::Reports) -> Vec<RefPoint> {
    let mut v = Vec::new();
    for i in 0..oa_adsb::reports_len(reports) {
        let Some(row) = oa_adsb::reports_row(reports, i) else {
            continue;
        };
        if row.flags & oa_adsb::F_STALE != 0
            || row.icao & oa_adsb::NON_ICAO != 0
            || (row.source != oa_adsb::NONE_U8 && !oa_truth::ADSB_SOURCES.contains(&row.source))
        {
            continue;
        }
        v.push(RefPoint {
            address: row.icao & 0xFF_FFFF,
            t_ns: row.time_ns,
            lat_e7: row.lat_e7,
            lon_e7: row.lon_e7,
        });
    }
    v.sort_unstable();
    v
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
            }
            Err(r) => {
                ax.refuse(r);
                info.refused = info.refused.saturating_add(1);
            }
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
        nic_baro: u8_of(r.nic_baro),
        link: match r.source {
            NONE_U8 => LinkSource::NoColumn,
            0 | 1 => LinkSource::Es1090,
            _ => LinkSource::NotSaid,
        },
    }
}

/// The identity of an observation for the ADS-B file: the scorer's key and, in addition, the
/// fields the file writes that the scorer does not read, NIC_baro, SDA and GVA. Two reports that
/// agree on the scorer's key and differ in one of these are not copies here.
pub fn export_key(r: &oa_adsb::Row) -> (oa_bench::split_check::Key, [u8; 3]) {
    (
        oa_bench::split_check::observation_key(r),
        [r.nic_baro, r.sda, r.gva],
    )
}

/// Loads the ADS-B clip at `path`, removes the copies of reports through the scorer's own split
/// code (`split_reports_with`: the same function the scorer calls, with the halving on for
/// `Split` and off for `All`), runs the tracker on the reports `mode` selects and exports its
/// tick rows. The reports offered to the tracker's adapter are also written to `adsb`, if given. The tracker is
/// fed ADS-B only, so the tracks are monosensor.
///
/// Conflict rule: reports of one address and time with different content are all kept, in one
/// fixed order (the tracker sees them as the scorer's split gives them). What counts as a copy
/// differs by path: the tracker's feed removes copies by the scorer's key (`observation_key`,
/// which does not hold NIC_baro, SDA, GVA or the callsign), and the file by [`export_key`], which
/// adds NIC_baro, SDA and GVA (the callsign is still not part of either). This holds in both modes.
pub fn export_adsb_clip(
    path: &std::path::Path,
    ex: &mut SystemExport,
    adsb: Option<&mut AdsbExport>,
    mode: InputsMode,
    rows: OpenatcaRows,
) -> Result<(TrackRunInfo, ClipInfo), ExportError> {
    let clip = oa_bench::input::load_adsb_with(path, &guarded_opener())
        .map_err(|e| ExportError::Input(format!("{}: {e}", path.display())))?;
    export_adsb_loaded(clip, ex, adsb, mode, rows)
}

/// [`export_adsb_clip`] over a clip that is already loaded, so that what a caller inspected
/// first is what is exported.
pub fn export_adsb_loaded(
    clip: oa_bench::input::AdsbClip,
    ex: &mut SystemExport,
    adsb: Option<&mut AdsbExport>,
    mode: InputsMode,
    rows: OpenatcaRows,
) -> Result<(TrackRunInfo, ClipInfo), ExportError> {
    let sealed_window_dropped = clip.sealed_window_dropped;
    let positions = ref_points(&clip.reports);
    let halve = mode == InputsMode::Split;
    let (fed_set, other, split) = split_reports_with(&clip.reports, halve);
    // The file's own copy removal, with a key that also holds NIC_baro, SDA and GVA (the scorer's
    // does not read them, but the file writes them): the groups are the same, so the sets are the
    // same observations, and two reports that differ only in those are both written.
    let file_set = if adsb.is_some() {
        Some(split_reports_keyed(&clip.reports, halve, export_key).0)
    } else {
        None
    };
    drop(clip);
    let mut info = ClipInfo {
        offered: oa_adsb::reports_len(&fed_set) as u64,
        not_offered: oa_adsb::reports_len(&other) as u64,
        sealed_window_dropped,
        split,
        adsb: None,
        file_offered: 0,
        gate: oa_track::adsb::AdsbStats::default(),
        positions,
    };
    drop(other);
    if let (Some(ax), Some(file_set)) = (adsb, file_set) {
        info.file_offered = oa_adsb::reports_len(&file_set) as u64;
        info.adsb = Some(export_adsb_reports(&file_set, ax)?);
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
    /// Flights whose address is not an ICAO address (above 0xFFFFFF): their own outcome.
    pub flights_non_icao: u64,
    /// What was done with the contradicting flights.
    pub address: EramAddress,
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

/// What the export does with the address of a flight whose track contradicts it
/// (`--eram-contradicted-address`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EramAddress {
    /// Write the address ERAM published. The default: a wrong address is ERAM's own
    /// identification error and stays visible to the evaluator; the check only reports it.
    #[default]
    Keep,
    /// Write the flight without I380 address. The file is altered; the README says so and lists
    /// the flights.
    Drop,
}

/// Per flight: its rows and, for the address check, its positions.
struct FlightRows {
    samples: Vec<(i64, i32, i32)>,
}

/// The check of every flight with an address, counts by outcome into `info`; returns the flights
/// that contradict their address. The four outcomes add up to the flights with an address, or the
/// run is refused.
fn address_check(
    leg: &Legacy,
    flights: &HashMap<u32, FlightRows>,
    refs: &[RefPoint],
    info: &mut EramInfo,
) -> Result<HashSet<u32>, ExportError> {
    let mut contradicting = HashSet::new();
    let mut ids: Vec<u32> = flights.keys().copied().collect();
    ids.sort_unstable();
    for f in ids {
        let Some(fr) = flights.get(&f) else { continue };
        let addr = leg.flights.addr.get(f as usize).copied().unwrap_or(NONE);
        if addr == NONE {
            continue;
        }
        match check_flight(addr, &fr.samples, refs) {
            Verdict::NonIcao => {
                info.flights_non_icao = info.flights_non_icao.saturating_add(1);
            }
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
                contradicting.insert(f);
            }
        }
    }
    let sum = info
        .flights_agree
        .saturating_add(info.flights_not_checked)
        .saturating_add(info.flights_contradicted)
        .saturating_add(info.flights_non_icao);
    if sum != info.flights_with_address {
        return Err(ExportError::Verify(format!(
            "address check: {sum} flights in the four outcomes, {} flights with an address",
            info.flights_with_address
        )));
    }
    Ok(contradicting)
}

/// Sorts the ERAM track samples by (position time, flight, input order) and exports them. With
/// `refs` (every ADS-B report of the clip, sorted by address and time) every flight with an
/// address is checked against the reports of its own address; the samples used are those that
/// are exported (not refused for position or time), one per flight and position time (the first
/// of any copies or same-time conflicts). A flight that contradicts its address is listed, and
/// with `address` = [`EramAddress::Drop`] written without it.
pub fn export_eram(
    leg: &Legacy,
    ex: &mut SystemExport,
    refs: Option<&[RefPoint]>,
    address: EramAddress,
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
        flights.entry(f).or_insert_with(|| FlightRows {
            samples: Vec::new(),
        });
    }
    order.sort_unstable();
    let mut info = EramInfo {
        rows_in: n as u64,
        checked: refs.is_some(),
        address,
        ..EramInfo::default()
    };
    // The samples of the check: exported ones, one per flight and position time.
    if refs.is_some() {
        let mut last: (i64, u32) = (i64::MIN, NONE);
        for &(t, f, i) in &order {
            let i = i as usize; // u32 to usize: lossless on the 64-bit targets this tool runs on
            let (Some(&la), Some(&lo)) = (th.lat_e7.get(i), th.lon_e7.get(i)) else {
                continue;
            };
            if eram_refusal(t, la, lo).is_some() || last == (t, f) {
                continue;
            }
            last = (t, f);
            if let (Some(fr), Some(ns)) = (flights.get_mut(&f), t.checked_mul(1_000_000)) {
                fr.samples.push((ns, la, lo));
            }
        }
    }
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
    let contradicting = match refs {
        Some(r) => address_check(leg, &flights, r, &mut info)?,
        None => HashSet::new(),
    };
    let withheld = if address == EramAddress::Drop {
        contradicting
    } else {
        HashSet::new()
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

//! Shell (B1): feeds the rows of the two systems into their exports. OpenATCA: the tracker is
//! run on the ADS-B clip exactly as the scorer (`examples/p002.rs`) runs it, set A of the split,
//! the scoring configuration, and its one-second tick rows are drained into the export as they are
//! produced (nothing is spilled or held). ERAM: the legacy tables the scorer reads (`TH` track
//! samples with the flight's address and callsign) are sorted by position time and written.

use oa_bench::legacy::{Legacy, NONE, NO_ALT, NO_VEL};
use oa_track::score::{interval_stats, scoring_config, split_reports, IntervalStats, DRAIN_ROWS};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG,
    O_COAST, O_HAS_ALT, O_IDENTITY_UNCONFIRMED, O_RADAR_DISAGREES, O_REACQUIRED, O_TICK,
    O_UNCERTAIN, RADAR_KEY_BASE,
};

use super::error::ExportError;
use super::export::SystemExport;
use crate::core::convert::{eram_record, track_record, EramRow, TrackRow};

/// What a tracker run did besides the rows exported. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackRunInfo {
    pub measurements: u64,
    /// Tick rows offered to the export.
    pub tick_rows: u64,
    /// Update rows (the state at each applied report): not exported, one tick row per second per
    /// track is.
    pub update_rows_not_exported: u64,
    /// Rows the tracker lost to a full output table; must be 0.
    pub overflow: u64,
    /// Tick rows carrying `O_UNCERTAIN`, `O_REACQUIRED`, `O_COAST`.
    pub uncertain: u64,
    pub reacquired: u64,
    pub coasting: u64,
}

/// The row at index `i` of `out` and the key of its track.
fn track_row(out: &TrackOutput, i: usize) -> Option<(TrackRow, u32)> {
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
        address: confirmed.then_some(addr),
    };
    Some((row, addr))
}

/// Offers every tick row of `out` to `ex`.
pub fn export_output(
    out: &TrackOutput,
    ex: &mut SystemExport,
    info: &mut TrackRunInfo,
) -> Result<(), ExportError> {
    for i in 0..output_len(out) {
        let Some((row, key)) = track_row(out, i) else {
            return Err(ExportError::Input(format!(
                "tracker output row {i} has a column shorter than the table"
            )));
        };
        let is_tick = out.flags.get(i).is_some_and(|f| f & O_TICK != 0);
        if !is_tick {
            info.update_rows_not_exported += 1;
            continue;
        }
        info.tick_rows += 1;
        info.uncertain += u64::from(row.uncertain);
        info.reacquired += u64::from(row.reacquired);
        info.coasting += u64::from(row.coast);
        match track_record(&row) {
            Ok(c) => ex.push(&c, key, (row.lat_e7, row.lon_e7))?,
            Err(r) => ex.refuse(r),
        }
    }
    Ok(())
}

/// Runs the tracker over `meas` and exports its tick rows.
pub fn run_tracker(
    meas: &[Measurement],
    cfg: &TrackerConfig,
    ex: &mut SystemExport,
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
            export_output(&scratch, ex, &mut info)?;
            output_clear(&mut scratch);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut scratch);
    tracker_flush(&mut t, end, &mut scratch);
    export_output(&scratch, ex, &mut info)?;
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

/// Loads the ADS-B clip at `path`, takes set A of the scorer's split, runs the tracker and exports
/// its tick rows. Returns the run's counts and the split's report counts.
pub fn export_adsb_clip(
    path: &std::path::Path,
    ex: &mut SystemExport,
) -> Result<(TrackRunInfo, u64, u64), ExportError> {
    let clip = oa_bench::input::load_adsb(path)
        .map_err(|e| ExportError::Input(format!("{}: {e}", path.display())))?;
    let (set_a, set_b, _) = split_reports(&clip.reports);
    let (n_a, n_b) = (
        oa_adsb::reports_len(&set_a) as u64,
        oa_adsb::reports_len(&set_b) as u64,
    );
    drop(clip);
    drop(set_b);
    let (meas, _) = oa_track::adsb::adsb_measurements(&set_a, &oa_track::adsb::AdsbConfig::default());
    drop(set_a);
    let cfg = scorer_config(&meas);
    let info = run_tracker(&meas, &cfg, ex)?;
    Ok((info, n_a, n_b))
}

/// ERAM rows of one input: how many, and what was dropped or kept as a conflict. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EramInfo {
    pub rows_in: u64,
    /// Same flight, position time and content as an earlier row: dropped.
    pub exact_duplicates: u64,
    /// Same flight and position time, different content: both kept.
    pub conflicting_same_time: u64,
    /// Rows whose flight index is not in the flight table: refused.
    pub unknown_flight: u64,
}

/// Most earlier rows of one (flight, position time) group a row is compared with. A group with
/// more distinct rows than this keeps the rest as they come (no row is dropped on a comparison
/// that was not made).
const GROUP_WINDOW: usize = 16;

/// Sorts the ERAM track samples by (position time, flight, input order) and exports them.
pub fn export_eram(leg: &Legacy, ex: &mut SystemExport) -> Result<EramInfo, ExportError> {
    let th = &leg.th;
    let n = th.flight.len();
    let mut order: Vec<(i64, u32, u32)> = Vec::with_capacity(n);
    for (i, (&t, &f)) in th.pos_ms.iter().zip(th.flight.iter()).enumerate() {
        order.push((t, f, i as u32));
    }
    order.sort_unstable();
    let mut info = EramInfo {
        rows_in: n as u64,
        ..EramInfo::default()
    };
    let mut group: (i64, u32) = (i64::MIN, NONE);
    let mut seen: Vec<[i64; 6]> = Vec::with_capacity(GROUP_WINDOW);
    for &(t, f, i) in &order {
        let i = i as usize;
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
        let (Some(&addr), Some(callsign)) = (leg.flights.addr.get(fi), leg.flights.callsign.get(fi))
        else {
            info.unknown_flight += 1;
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
            info.exact_duplicates += 1;
            continue;
        }
        if !seen.is_empty() {
            info.conflicting_same_time += 1;
        }
        if seen.len() < GROUP_WINDOW {
            seen.push(content);
        }
        let row = EramRow {
            pos_ms: t,
            lat_e7: lat,
            lon_e7: lon,
            alt_ft: (alt != NO_ALT).then_some(alt),
            vel_dkt: (vx != NO_VEL && vy != NO_VEL).then_some((vx, vy)),
            coast: coast != 0,
            address: (addr != NONE).then_some(addr),
            callsign: (*callsign != [0u8; 8]).then_some(*callsign),
        };
        match eram_record(&row) {
            Ok(c) => ex.push(&c, f, (lat, lon))?,
            Err(r) => ex.refuse(r),
        }
    }
    Ok(info)
}

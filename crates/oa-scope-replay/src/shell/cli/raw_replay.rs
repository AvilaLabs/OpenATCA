//! Offline raw replay shell. Retains source receipt evidence and prepares observations for the
//! independent display without constructing a tracker.

use super::{
    create_new, failed, remove_ours, stamp_of, unix_text, CliError, RawReceiptMode, ReplayArgs,
};
use crate::frame::{replay_build_raw_with_source_stale, replay_new, replay_raster, ReplayConfig};
use crate::raw::{RawObservationColumns, MAX_RAW_OBSERVATIONS, RAW_STALE_CUTOFF_NS};
use crate::rows::NS_PER_S;
use oa_adsb::{decode_payload_lenient, reports_clear, reports_len, Reports};
use oa_display::raster::frame_digest;
use oa_display::traffic::traffic_loss;
use oa_sha256::to_hex;
use std::io::{Read, Write};
use std::path::Path;

/// Maximum total report visits across all frames in the opt-in raw-only CLI path.
pub const MAX_RAW_FRAME_VISITS: usize = 32_000_000;
/// Upper bound on bytes scanned by this opt-in whole-history raw path.
pub const MAX_RAW_CLIP_BYTES: u64 = 512 * 1024 * 1024;

/// One row of cold raw history, reduced to fields used by the raw display.
struct ReceivedObservation {
    received_ns: i64,
    time_ns: i64,
    addr: u32,
    lat: i32,
    lon: i32,
    alt: i32,
    speed: u16,
    track: u16,
    source_stale: bool,
}

impl ReceivedObservation {
    fn new(row: oa_adsb::Row, received_ns: i64) -> Self {
        let alt = if row.flags & oa_adsb::F_GROUND != 0 {
            oa_display::traffic::NO_ALT
        } else if row.flags & oa_adsb::F_ALT_GEOM != 0 {
            row.geom_alt_ft
        } else {
            row.baro_alt_ft
        };
        Self {
            received_ns,
            time_ns: row.time_ns,
            addr: row.icao,
            lat: row.lat_e7,
            lon: row.lon_e7,
            alt,
            speed: row.gs_dkt,
            track: row.track_ddeg,
            source_stale: row.flags & oa_adsb::F_STALE != 0,
        }
    }
}

// A7.2: cold per-run source history and counts, bounded by MAX_RAW_OBSERVATIONS.
#[derive(Default)]
struct RawHistory {
    rows: Vec<ReceivedObservation>,
    sealed_frames: u64,
    refused_frames: u64,
    undecodable: u64,
    status_frames: u64,
    outside: u64,
    damaged_frames: u64,
    outside_rows: u64,
    decode_records: u64,
    decode_kept: u64,
    decode_dropped: u64,
    decode_bad_time: u64,
    decode_unknown_source: u64,
    decode_bad_measurement: u64,
    decode_reserved: u64,
}

/// Reads a bounded raw history while retaining each frame's availability time. Archive-reconstructed
/// mode assigns the maximum decoded observation timestamp in a MESSAGE frame to every row in that
/// frame; this is an explicit proxy, not recorded receipt evidence.
fn raw_history(
    path: &Path,
    start_ns: i64,
    end_ns: i64,
    mode: RawReceiptMode,
) -> Result<RawHistory, CliError> {
    let earliest = start_ns
        .checked_sub(RAW_STALE_CUTOFF_NS)
        .ok_or_else(|| CliError::Failed("raw history start is outside clock range".into()))?;
    let mut history = RawHistory::default();
    let mut capacity_refused = 0u64;
    let (file, size) = oa_sealed::open_regular(path)
        .map_err(|e| failed(&format!("read {}", path.display()), e))?;
    if size > MAX_RAW_CLIP_BYTES {
        return Err(CliError::Failed(format!(
            "{} exceeds the raw-only input limit of {MAX_RAW_CLIP_BYTES} bytes",
            path.display()
        )));
    }
    // One extra byte distinguishes an exact-limit file from one that grew past the bound.
    let mut input = file.take(MAX_RAW_CLIP_BYTES + 1);
    let mut scratch = Reports::default();
    let mut latest_receipt = None;
    let mut receipt_regressions = 0u64;
    let damaged = oa_feed::read_frames(&mut input, &mut |frame| {
        if oa_sealed::in_sealed_window(frame.recv_unix_ns) {
            history.sealed_frames = history.sealed_frames.saturating_add(1);
            oa_sealed::note_window_drops(1);
            return;
        }
        if mode == RawReceiptMode::Recorded {
            if latest_receipt.is_some_and(|latest| frame.recv_unix_ns < latest) {
                receipt_regressions = receipt_regressions.saturating_add(1);
                return;
            }
            latest_receipt = Some(frame.recv_unix_ns);
        }
        let message = frame.kind == oa_feed::Kind::Message;
        let frame_outside = if message {
            mode == RawReceiptMode::Recorded && frame.recv_unix_ns > end_ns
        } else {
            !(earliest..=end_ns).contains(&frame.recv_unix_ns)
        };
        if frame_outside {
            history.outside = history.outside.saturating_add(1);
            return;
        }
        match frame.kind {
            oa_feed::Kind::Reject => {
                history.refused_frames = history.refused_frames.saturating_add(1);
                return;
            }
            oa_feed::Kind::Status => {
                history.status_frames = history.status_frames.saturating_add(1);
                return;
            }
            oa_feed::Kind::Message => {}
        }
        reports_clear(&mut scratch);
        let decoded = match decode_payload_lenient(frame.payload, &mut scratch) {
            Ok(c) => c,
            Err(_) => {
                history.undecodable = history.undecodable.saturating_add(1);
                return;
            }
        };
        // Match the tracker loader's frame-level row seal: a sealed row drops the whole payload.
        if (0..reports_len(&scratch)).any(|i| {
            oa_adsb::reports_row(&scratch, i)
                .is_some_and(|row| oa_sealed::in_sealed_window(row.time_ns))
        }) {
            history.sealed_frames = history.sealed_frames.saturating_add(1);
            oa_sealed::note_window_drops(reports_len(&scratch).max(1) as u64);
            return;
        }
        history.decode_records += decoded.records as u64;
        history.decode_kept += decoded.kept as u64;
        history.decode_dropped += decoded.dropped as u64;
        history.decode_bad_time += decoded.bad_time as u64;
        history.decode_unknown_source += decoded.unknown_source as u64;
        history.decode_bad_measurement += decoded.bad_measurement as u64;
        history.decode_reserved += decoded.reserved_set as u64;
        let mut received_ns = frame.recv_unix_ns;
        if mode == RawReceiptMode::ArchiveReconstructed {
            let mut latest = None;
            for i in 0..reports_len(&scratch) {
                if let Some(row) = oa_adsb::reports_row(&scratch, i) {
                    latest = Some(latest.map_or(row.time_ns, |v: i64| v.max(row.time_ns)));
                }
            }
            let Some(proxy) = latest else {
                return;
            };
            received_ns = proxy;
        }
        if received_ns > end_ns {
            history.outside = history.outside.saturating_add(1);
            history.outside_rows += reports_len(&scratch) as u64;
            return;
        }
        let eligible = (0..reports_len(&scratch))
            .filter_map(|i| oa_adsb::reports_row(&scratch, i))
            .filter(|row| (earliest..=end_ns).contains(&row.time_ns))
            .count();
        history.outside_rows += reports_len(&scratch).saturating_sub(eligible) as u64;
        if history.rows.len().saturating_add(eligible) > MAX_RAW_OBSERVATIONS {
            capacity_refused = capacity_refused.saturating_add(1);
            return;
        }
        for i in 0..reports_len(&scratch) {
            if let Some(row) = oa_adsb::reports_row(&scratch, i) {
                if (earliest..=end_ns).contains(&row.time_ns) {
                    history
                        .rows
                        .push(ReceivedObservation::new(row, received_ns));
                }
            }
        }
    })
    .map_err(|e| failed(&format!("read {}", path.display()), e))?;
    if input.limit() == 0 {
        return Err(CliError::Failed(format!(
            "{} exceeded the raw-only input limit of {MAX_RAW_CLIP_BYTES} bytes while reading",
            path.display()
        )));
    }
    if receipt_regressions > 0 {
        return Err(CliError::Failed(format!(
            "raw clip has {receipt_regressions} recorded receipt clock regressions"
        )));
    }
    if capacity_refused > 0 {
        return Err(CliError::Failed(format!(
            "raw history refused {capacity_refused} frame(s) at the {MAX_RAW_OBSERVATIONS}-observation limit; narrow the window"
        )));
    }
    history.damaged_frames = damaged;
    if history.rows.is_empty() {
        return Err(CliError::Failed(format!(
            "{} has no eligible raw observations ({} outside the display window, {} sealed frames, {} reject frames, {} undecodable payloads, {} invalid observations, {} damaged frames)",
            path.display(),
            history.outside_rows,
            history.sealed_frames,
            history.refused_frames,
            history.undecodable,
            history.decode_dropped,
            history.damaged_frames
        )));
    }
    history
        .rows
        .sort_unstable_by_key(|row| (row.addr, row.time_ns));
    Ok(history)
}

// A7.2: cold scratch columns reused for each display frame.
#[derive(Default)]
struct RawFrameColumns {
    time: Vec<i64>,
    addr: Vec<u32>,
    lat: Vec<i32>,
    lon: Vec<i32>,
    alt: Vec<i32>,
    speed: Vec<u16>,
    track: Vec<u16>,
    source_stale: Vec<bool>,
}

impl RawFrameColumns {
    fn with_capacity(n: usize) -> Self {
        Self {
            time: Vec::with_capacity(n),
            addr: Vec::with_capacity(n),
            lat: Vec::with_capacity(n),
            lon: Vec::with_capacity(n),
            alt: Vec::with_capacity(n),
            speed: Vec::with_capacity(n),
            track: Vec::with_capacity(n),
            source_stale: Vec::with_capacity(n),
        }
    }
    fn fill(&mut self, history: &RawHistory, as_of_ns: i64) -> u64 {
        self.time.clear();
        self.addr.clear();
        self.lat.clear();
        self.lon.clear();
        self.alt.clear();
        self.speed.clear();
        self.track.clear();
        self.source_stale.clear();
        let mut later_receipts = 0;
        for row in &history.rows {
            if row.received_ns > as_of_ns {
                later_receipts += 1;
                continue;
            }
            self.time.push(row.time_ns);
            self.addr.push(row.addr);
            self.lat.push(row.lat);
            self.lon.push(row.lon);
            self.alt.push(row.alt);
            self.speed.push(row.speed);
            self.track.push(row.track);
            self.source_stale.push(row.source_stale);
        }
        later_receipts
    }
    fn columns(&self) -> RawObservationColumns<'_> {
        RawObservationColumns {
            t_ns: &self.time,
            addr: &self.addr,
            lat_e7: &self.lat,
            lon_e7: &self.lon,
            alt_ft: &self.alt,
            gs_dkt: &self.speed,
            track_ddeg: &self.track,
        }
    }
    fn source_stale(&self) -> &[bool] {
        &self.source_stale
    }
}

/// Renders the bounded surveillance history and writes aggregate receipt and loss counts.
pub(super) fn run_raw_only(
    a: &ReplayArgs,
    mode: RawReceiptMode,
    model: oa_airspace::model::Model,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let span = i64::try_from(a.frames.saturating_sub(1))
        .ok()
        .and_then(|n| n.checked_mul(a.step_ns))
        .ok_or_else(|| CliError::Failed("raw run span is outside clock range".into()))?;
    let end_ns = a
        .start_ns
        .checked_add(span)
        .ok_or_else(|| CliError::Failed("raw run end is outside clock range".into()))?;
    let history = raw_history(&a.adsb, a.start_ns, end_ns, mode)?;
    if history.rows.len().saturating_mul(a.frames as usize) > MAX_RAW_FRAME_VISITS {
        return Err(CliError::Failed(format!(
            "raw history × frames exceeds {MAX_RAW_FRAME_VISITS} report visits; narrow the window or reduce frames"
        )));
    }
    let cfg = ReplayConfig {
        centre_lat_e7: a.centre_lat_e7,
        centre_lon_e7: a.centre_lon_e7,
        range_nm: a.range_nm,
        width: a.width,
        height: a.height,
        max_targets: a.max_targets,
        style: a.style,
        convert: a.convert,
    };
    let mut replay = replay_new(model, &cfg)?;
    std::fs::create_dir_all(&a.out_dir)
        .map_err(|e| failed(&format!("create {}", a.out_dir.display()), e))?;
    if a.overwrite {
        remove_ours(&a.out_dir)?;
    }
    let mut columns = RawFrameColumns::with_capacity(history.rows.len());
    let mut hashes = String::new();
    let mut total = RawRunStats::default();
    for i in 0..a.frames {
        let t_ns = i64::try_from(i)
            .ok()
            .and_then(|k| k.checked_mul(a.step_ns))
            .and_then(|d| a.start_ns.checked_add(d))
            .ok_or_else(|| CliError::Failed("raw frame time is outside clock range".into()))?;
        total.later_receipts += columns.fill(&history, t_ns);
        let stamp = stamp_of(t_ns);
        let (info, stats) = replay_build_raw_with_source_stale(
            &mut replay,
            &columns.columns(),
            Some(columns.source_stale()),
            t_ns,
            &stamp,
        )?;
        replay_raster(&mut replay)?;
        let digest = frame_digest(&replay.spec, &replay.buf).map_err(|e| failed("hash", e))?;
        hashes.push_str(&format!("{i:04} {} {}\n", unix_text(t_ns), to_hex(&digest)));
        if !a.hashes_only {
            let path = a.out_dir.join(format!("frame-{i:04}.ppm"));
            let file = create_new(&path)?;
            let mut writer = std::io::BufWriter::new(file);
            oa_display::ppm::write_ppm(&mut writer, &replay.spec, &replay.buf)
                .and_then(|()| writer.flush())
                .map_err(|e| failed(&format!("write {}", path.display()), e))?;
        }
        total.frames += 1;
        total.frames_without_fresh_positions += u64::from(stats.fresh_valid_positions == 0);
        total.observations_seen += stats.observations_in;
        total.targets_admitted += stats.targets_admitted;
        total.targets_drawn += u64::from(info.traffic.drawn);
        // The traffic builder copies the target set refusal counts into its traffic stats.
        total.draw_loss += traffic_loss(&info.traffic) + u64::from(info.stamp_clipped);
        total.future_observations += stats.future_observations;
        total.future_only_addresses += stats.future_only_addresses;
        total.source_stale_addresses += stats.source_stale_addresses;
        total.source_stale_conflicts += stats.source_stale_conflicts;
        total.stale_addresses += stats.stale_addresses;
        total.conflicts += stats.conflicting_timestamps;
        total.refused += stats.refused_capacity + stats.refused_malformed;
        total.invalid_positions += stats.invalid_positions;
        total.unprojectable_positions += stats.unprojectable_positions;
        total.off_map_positions += stats.off_map_positions;
    }
    let hashes_path = a.out_dir.join("hashes.txt");
    create_new(&hashes_path)?
        .write_all(hashes.as_bytes())
        .map_err(|e| failed(&format!("write {}", hashes_path.display()), e))?;
    let loss = write_raw_report(out, mode, &history, &total, &hashes_path, a.allow_loss)
        .map_err(|error| failed("write raw replay summary", error))?;
    if loss > 0 && !a.allow_loss {
        return Err(CliError::Loss(format!(
            "raw-only run recorded {loss} refused or unavailable work items"
        )));
    }
    Ok(())
}

fn write_raw_report(
    out: &mut dyn Write,
    mode: RawReceiptMode,
    history: &RawHistory,
    total: &RawRunStats,
    hashes_path: &Path,
    allow_loss: bool,
) -> std::io::Result<u64> {
    let provenance = match mode {
        RawReceiptMode::Recorded => "caller-declared recorded OAF frame receipt timestamp",
        RawReceiptMode::ArchiveReconstructed => {
            "reconstructed per-frame availability: maximum row observation time"
        }
    };
    writeln!(out, "RAW SURVEILLANCE ONLY")?;
    writeln!(out, "availability {}", provenance)?;
    writeln!(out, "raw      {} source observations retained; {} frames; {} source-frame sealed drops; {} reject frames; {} status frames; {} undecodable payloads; {} frames outside the receipt window; {} decoded observations outside the display window", history.rows.len(), total.frames, history.sealed_frames, history.refused_frames, history.status_frames, history.undecodable, history.outside, history.outside_rows)?;
    writeln!(out, "decode   {} records, {} retained, {} dropped (bad time {}, unknown source {}, bad measurement {}, reserved {}); {} damaged frames", history.decode_records, history.decode_kept, history.decode_dropped, history.decode_bad_time, history.decode_unknown_source, history.decode_bad_measurement, history.decode_reserved, history.damaged_frames)?;
    writeln!(out, "display  totals over {} frames: {} observations considered ({} future at frame time); {} observation visits withheld for later receipt; {} target admissions; {} targets drawn; {} draw losses (including target refusal); {} future-only addresses; {} source-stale address groups excluded; {} source-stale same-time conflicts (included in contradictory total); {} age-stale address groups (age > {} s); {} contradictory same-time address groups; {} refused targets; invalid positions {}; unprojectable {}; off-map {}; {} frames without fresh raw positions", total.frames, total.observations_seen, total.future_observations, total.later_receipts, total.targets_admitted, total.targets_drawn, total.draw_loss, total.future_only_addresses, total.source_stale_addresses, total.source_stale_conflicts, total.stale_addresses, RAW_STALE_CUTOFF_NS / NS_PER_S, total.conflicts, total.refused, total.invalid_positions, total.unprojectable_positions, total.off_map_positions, total.frames_without_fresh_positions)?;
    writeln!(out, "hashes   {}", hashes_path.display())?;
    let loss = history.refused_frames
        + history.undecodable
        + history.decode_dropped
        + history.damaged_frames
        + total.draw_loss
        + total.conflicts
        + total.invalid_positions
        + total.unprojectable_positions;
    if loss == 0 {
        writeln!(out, "loss     none")?;
    } else if allow_loss {
        writeln!(
            out,
            "loss     {loss} refused or unavailable raw-only work items; allowed by --allow-loss"
        )?;
    } else {
        writeln!(
            out,
            "loss     {loss} refused or unavailable raw-only work items"
        )?;
    }
    out.flush()?;
    Ok(loss)
}

// A7.2: cold run-level raw-path counters.
#[derive(Default)]
struct RawRunStats {
    frames: u64,
    frames_without_fresh_positions: u64,
    observations_seen: u64,
    later_receipts: u64,
    targets_admitted: u64,
    targets_drawn: u64,
    draw_loss: u64,
    future_observations: u64,
    future_only_addresses: u64,
    source_stale_addresses: u64,
    source_stale_conflicts: u64,
    stale_addresses: u64,
    conflicts: u64,
    refused: u64,
    invalid_positions: u64,
    unprojectable_positions: u64,
    off_map_positions: u64,
}

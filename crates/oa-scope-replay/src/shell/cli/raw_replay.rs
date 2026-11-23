//! Offline raw replay shell. Retains source receipt evidence and prepares observations for the
//! independent display without constructing a tracker.

use super::{
    create_new, failed, remove_ours, stamp_of, unix_text, CliError, RawReceiptMode,
    RawWitnessAction, ReplayArgs,
};
use crate::frame::{replay_build_raw_with_source_stale, replay_new, replay_raster, ReplayConfig};
use crate::raw::{RawObservationColumns, MAX_RAW_OBSERVATIONS, RAW_STALE_CUTOFF_NS};
use crate::rows::NS_PER_S;
use oa_adsb::{decode_payload_lenient, reports_clear, reports_len, Reports};
use oa_display::raster::frame_digest;
use oa_display::traffic::traffic_loss;
use oa_sha256::{sha256, to_hex, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

/// Maximum total report visits across all frames in the opt-in raw-only CLI path.
pub const MAX_RAW_FRAME_VISITS: usize = 32_000_000;
/// Upper bound on bytes scanned by this opt-in whole-history raw path.
pub const MAX_RAW_CLIP_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum encoded recovery witness size (10,000 frames plus fixed metadata).
pub const MAX_RAW_WITNESS_BYTES: usize = 1_000_000;
// C7: bounds the component walks and ancestor probes used by collision preflight.
const MAX_RAW_WITNESS_PATH_BYTES: usize = 4096;

// A7.2: cold decoded witness, bounded by MAX_RAW_WITNESS_BYTES and MAX_FRAMES.
struct RawWitness {
    snapshot: [u8; 32],
    clip: [u8; 32],
    receipt: String,
    settings: [u8; 32],
    frames: Vec<(i64, [u8; 32])>,
}

fn witness_error(message: impl Into<String>) -> CliError {
    CliError::Failed(format!("raw witness: {}", message.into()))
}

fn parse_digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let high = *pair.first()?;
        let low = *pair.get(1)?;
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        let slot = out.get_mut(i)?;
        *slot = digit(high)?.checked_mul(16)?.checked_add(digit(low)?)?;
    }
    Some(out)
}

fn witness_metadata<'a>(key: &str, line: Option<&'a str>) -> Result<&'a str, CliError> {
    line.and_then(|value| value.strip_prefix(&format!("{key} ")))
        .filter(|value| !value.contains(' '))
        .ok_or_else(|| witness_error(format!("missing or malformed {key} metadata")))
}

fn settings_digest(a: &ReplayArgs, mode: RawReceiptMode) -> Result<[u8; 32], CliError> {
    let mut bytes = Vec::with_capacity(128);
    bytes.extend_from_slice(b"oa-scope-raw-settings-v1\0");
    bytes.extend_from_slice(&a.centre_lat_e7.to_le_bytes());
    bytes.extend_from_slice(&a.centre_lon_e7.to_le_bytes());
    bytes.extend_from_slice(&oa_math::canon_nan(a.range_nm).to_bits().to_le_bytes());
    bytes.extend_from_slice(&a.width.to_le_bytes());
    bytes.extend_from_slice(&a.height.to_le_bytes());
    bytes.extend_from_slice(&(a.max_targets as u64).to_le_bytes());
    bytes.extend_from_slice(
        &oa_math::canon_nan(a.style.leader_min)
            .to_bits()
            .to_le_bytes(),
    );
    bytes.push(a.style.trail_n);
    bytes.extend_from_slice(&a.style.reacquired_s.to_le_bytes());
    bytes.extend_from_slice(&a.convert.trail_step_ns.to_le_bytes());
    bytes.extend_from_slice(&a.convert.stale_ns.to_le_bytes());
    bytes.extend_from_slice(&a.convert.reacq_window_s.to_le_bytes());
    bytes.extend_from_slice(&a.start_ns.to_le_bytes());
    bytes.extend_from_slice(&a.step_ns.to_le_bytes());
    bytes.extend_from_slice(&a.frames.to_le_bytes());
    bytes.push(u8::from(a.hashes_only));
    bytes.push(u8::from(a.allow_loss));
    bytes.push(match mode {
        RawReceiptMode::Recorded => 0,
        RawReceiptMode::ArchiveReconstructed => 1,
    });
    Ok(sha256(&bytes))
}

// A7.2: cold shell adapter around one bounded raw clip read.
struct HashingReader {
    inner: std::io::Take<std::fs::File>,
    hasher: Option<Sha256>,
}

impl Read for HashingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buffer)?;
        if n > 0 {
            if let Some(hasher) = &mut self.hasher {
                let chunk = buffer.get(..n).ok_or_else(|| {
                    std::io::Error::other("reader returned more bytes than the output buffer holds")
                })?;
                hasher.update(chunk).map_err(std::io::Error::other)?;
            }
        }
        Ok(n)
    }
}

fn read_witness(path: &Path) -> Result<RawWitness, CliError> {
    let (file, size) = oa_sealed::open_regular(path)
        .map_err(|e| failed(&format!("read {}", path.display()), e))?;
    if size > MAX_RAW_WITNESS_BYTES as u64 {
        return Err(witness_error(format!(
            "{} exceeds the {MAX_RAW_WITNESS_BYTES}-byte limit",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_RAW_WITNESS_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| failed(&format!("read {}", path.display()), e))?;
    if bytes.len() > MAX_RAW_WITNESS_BYTES {
        return Err(witness_error("file grew past its byte limit while reading"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| witness_error("file is not UTF-8"))?;
    let checksum_at = text
        .rfind("sha256 ")
        .ok_or_else(|| witness_error("missing checksum"))?;
    let body = bytes
        .get(..checksum_at)
        .ok_or_else(|| witness_error("checksum offset is outside the file"))?;
    if !body.ends_with(b"\n") {
        return Err(witness_error("checksum is not on a new line"));
    }
    let tail = text
        .get(checksum_at..)
        .ok_or_else(|| witness_error("checksum offset is not a text boundary"))?;
    let checksum_line = tail
        .strip_suffix('\n')
        .ok_or_else(|| witness_error("missing final newline"))?;
    let checksum = checksum_line
        .strip_prefix("sha256 ")
        .and_then(parse_digest)
        .ok_or_else(|| witness_error("malformed checksum"))?;
    if sha256(body) != checksum {
        return Err(witness_error("checksum mismatch"));
    }
    let body_text = std::str::from_utf8(body).map_err(|_| witness_error("body is not UTF-8"))?;
    let mut lines = body_text.lines();
    if lines.next() != Some("OARW001") {
        return Err(witness_error("unsupported or missing header"));
    }
    // Metadata is validated by the caller after parsing. Keeping a fixed header here rejects
    // unknown fields and prevents permissive parsing of a future format.
    let snapshot = parse_digest(witness_metadata("snapshot", lines.next())?)
        .ok_or_else(|| witness_error("malformed snapshot digest"))?;
    let clip = parse_digest(witness_metadata("clip", lines.next())?)
        .ok_or_else(|| witness_error("malformed clip digest"))?;
    let receipt = witness_metadata("receipt", lines.next())?.to_owned();
    if receipt != "recorded" && receipt != "archive-reconstructed" {
        return Err(witness_error("unknown receipt basis"));
    }
    let settings = parse_digest(witness_metadata("settings", lines.next())?)
        .ok_or_else(|| witness_error("malformed settings digest"))?;
    let count = lines
        .next()
        .and_then(|line| line.strip_prefix("frames "))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&n| n > 0 && n <= super::MAX_FRAMES as usize)
        .ok_or_else(|| witness_error("invalid frame count"))?;
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        let line = lines
            .next()
            .ok_or_else(|| witness_error("frame list is truncated"))?;
        let mut fields = line.split(' ');
        if fields.next() != Some("frame")
            || fields.next().and_then(|n| n.parse::<usize>().ok()) != Some(index)
        {
            return Err(witness_error(
                "frame indexes are not an exact ordered prefix",
            ));
        }
        let time_ns = fields
            .next()
            .and_then(|n| n.parse::<i64>().ok())
            .ok_or_else(|| witness_error("invalid frame timestamp"))?;
        let digest = fields
            .next()
            .and_then(parse_digest)
            .filter(|_| fields.next().is_none())
            .ok_or_else(|| witness_error("invalid frame digest"))?;
        frames.push((time_ns, digest));
    }
    if lines.next().is_some() {
        return Err(witness_error("unexpected body fields"));
    }
    Ok(RawWitness {
        snapshot,
        clip,
        receipt,
        settings,
        frames,
    })
}

fn validate_witness_metadata(
    witness: &RawWitness,
    a: &ReplayArgs,
    mode: RawReceiptMode,
    snapshot_sha256: [u8; 32],
    clip_sha256: [u8; 32],
) -> Result<(), CliError> {
    if witness.snapshot != snapshot_sha256
        || witness.clip != clip_sha256
        || witness.receipt != receipt_name(mode)
        || witness.settings != settings_digest(a, mode)?
    {
        return Err(witness_error(
            "snapshot, source, receipt basis, or settings differ",
        ));
    }
    if witness.frames.len() != a.frames as usize {
        return Err(witness_error("recorded frame count differs from this run"));
    }
    for (i, (time_ns, _)) in witness.frames.iter().enumerate() {
        let expected_time = i64::try_from(i)
            .ok()
            .and_then(|n| n.checked_mul(a.step_ns))
            .and_then(|d| a.start_ns.checked_add(d))
            .ok_or_else(|| witness_error("recorded frame time overflows"))?;
        if *time_ns != expected_time {
            return Err(witness_error("frame timestamps differ from this run"));
        }
    }
    Ok(())
}

fn receipt_name(mode: RawReceiptMode) -> &'static str {
    match mode {
        RawReceiptMode::Recorded => "recorded",
        RawReceiptMode::ArchiveReconstructed => "archive-reconstructed",
    }
}

fn witness_bytes(
    a: &ReplayArgs,
    mode: RawReceiptMode,
    snapshot_sha256: [u8; 32],
    clip_sha256: [u8; 32],
    frame_hashes: &[(i64, [u8; 32])],
) -> Result<Vec<u8>, CliError> {
    let mut body = format!(
        "OARW001\nsnapshot {}\nclip {}\nreceipt {}\nsettings {}\nframes {}\n",
        to_hex(&snapshot_sha256),
        to_hex(&clip_sha256),
        receipt_name(mode),
        to_hex(&settings_digest(a, mode)?),
        frame_hashes.len()
    )
    .into_bytes();
    for (index, (time_ns, digest)) in frame_hashes.iter().enumerate() {
        body.extend_from_slice(format!("frame {index} {time_ns} {}\n", to_hex(digest)).as_bytes());
    }
    let checksum = sha256(&body);
    body.extend_from_slice(format!("sha256 {}\n", to_hex(&checksum)).as_bytes());
    if body.len() > MAX_RAW_WITNESS_BYTES {
        return Err(witness_error("encoded witness exceeds its size limit"));
    }
    Ok(body)
}

fn check_witness_output(path: &Path) -> Result<(), CliError> {
    oa_sealed::refuse(path).map_err(CliError::Failed)?;
    let partial = partial_witness_path(path)?;
    oa_sealed::refuse(&partial).map_err(CliError::Failed)?;
    for candidate in [path, partial.as_path()] {
        // seal-allow: both output names passed the shared guard above; existence metadata only, no content read
        match std::fs::symlink_metadata(candidate) {
            Ok(_) => {
                return Err(witness_error(format!(
                    "{} already exists; witness output never overwrites",
                    candidate.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(failed(&format!("stat {}", candidate.display()), error)),
        }
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, CliError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| failed("read current directory", e))?
            .join(path)
    };
    if absolute.as_os_str().len() > MAX_RAW_WITNESS_PATH_BYTES {
        return Err(witness_error(
            "path exceeds the 4096-byte collision-preflight limit",
        ));
    }
    Ok(absolute)
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, CliError> {
    let absolute = absolute_path(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

/// Canonical path for an existing candidate, or its nearest existing ancestor plus the missing
/// suffix. The path guard runs first so dangling or unresolved symlinks are refused separately.
fn canonical_candidate(path: &Path) -> Result<PathBuf, CliError> {
    // Preserve symlink/.. traversal until the filesystem resolves it. Lexically collapsing ..
    // first can name a different parent and miss a collision with a generated output.
    let mut probe = absolute_path(path)?;
    let mut suffix = Vec::new();
    // Each failed probe removes a nonempty component, so the named byte limit bounds this loop.
    loop {
        match std::fs::canonicalize(&probe) {
            Ok(mut resolved) => {
                for component in suffix.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = probe.file_name() else {
                    return Err(failed(&format!("resolve {}", path.display()), error));
                };
                suffix.push(name.to_os_string());
                if !probe.pop() {
                    return Err(failed(&format!("resolve {}", path.display()), error));
                }
            }
            Err(error) => return Err(failed(&format!("resolve {}", path.display()), error)),
        }
    }
}

fn check_witness_output_collisions(a: &ReplayArgs) -> Result<(), CliError> {
    let Some(action) = &a.raw_witness else {
        return Ok(());
    };
    let witness_path = match action {
        RawWitnessAction::Write(path) | RawWitnessAction::Verify(path) => path,
    };
    let partial_path = partial_witness_path(witness_path)?;
    let protected = [witness_path.as_path(), partial_path.as_path()]
        .into_iter()
        .map(|path| Ok((absolute_lexical(path)?, canonical_candidate(path)?)))
        .collect::<Result<Vec<_>, CliError>>()?;
    let output_dir_lexical = absolute_lexical(&a.out_dir)?;
    let output_names = std::iter::once("hashes.txt".to_owned())
        .chain((0..a.frames).map(|i| format!("frame-{i:04}.ppm")));
    for name in output_names {
        let generated = a.out_dir.join(&name);
        let generated_lexical = output_dir_lexical.join(&name);
        let generated_canonical = canonical_candidate(&generated)?;
        if protected.iter().any(|(lexical, canonical)| {
            lexical == &generated_lexical || canonical == &generated_canonical
        }) {
            return Err(witness_error(format!(
                "witness path or partial sibling collides with generated output {}",
                generated.display()
            )));
        }
    }
    Ok(())
}

fn partial_witness_path(path: &Path) -> Result<std::path::PathBuf, CliError> {
    let mut partial_name = path
        .file_name()
        .ok_or_else(|| witness_error("output path has no file name"))?
        .to_os_string();
    partial_name.push(".oarw-partial");
    Ok(path.with_file_name(partial_name))
}

fn sync_parent(path: &Path) -> Result<(), CliError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    oa_sealed::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| failed(&format!("sync {}", parent.display()), e))
}

fn publish_witness(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    check_witness_output(path)?;
    let partial_path = partial_witness_path(path)?;
    let mut partial = create_new(&partial_path)?;
    if let Err(error) = partial.write_all(bytes).and_then(|()| partial.sync_all()) {
        return Err(failed(
            &format!(
                "write {}; it may be incomplete and was left untouched",
                partial_path.display()
            ),
            error,
        ));
    }
    sync_parent(&partial_path).map_err(|error| {
        failed(
            &format!(
                "sync directory; synced recovery copy remains at {}",
                partial_path.display()
            ),
            error,
        )
    })?;

    // The final copy has its own inode, so both it and the recovery copy satisfy OA-SEAL-002.
    // A crash during this copy can leave an invalid final file; the synced partial remains the
    // explicit recovery input and the strict checksum parser refuses the incomplete final.
    let mut final_file = create_new(path).map_err(|error| {
        failed(
            &format!(
                "create {}; synced recovery copy remains at {}",
                path.display(),
                partial_path.display()
            ),
            error,
        )
    })?;
    if let Err(error) = final_file
        .write_all(bytes)
        .and_then(|()| final_file.sync_all())
    {
        return Err(failed(
            &format!(
                "write {}; it may be incomplete, synced recovery copy remains at {}",
                path.display(),
                partial_path.display()
            ),
            error,
        ));
    }
    // Make the complete final directory entry durable while the recovery copy still exists.
    sync_parent(path).map_err(|error| {
        failed(
            &format!(
                "sync final directory entry; synced recovery copy remains at {}",
                partial_path.display()
            ),
            error,
        )
    })?;

    // Retain the bounded, complete recovery copy even after success. Removing it by pathname
    // after an identity check would still have a check/unlink race with another filesystem actor.
    Ok(())
}

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
    hash_input: bool,
) -> Result<(RawHistory, Option<[u8; 32]>), CliError> {
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
    let mut input = HashingReader {
        inner: file.take(MAX_RAW_CLIP_BYTES + 1),
        hasher: hash_input.then(Sha256::new),
    };
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
    if input.inner.limit() == 0 {
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
    let digest = input.hasher.map(|hasher| hasher.finalize());
    Ok((history, digest))
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
    snapshot_sha256: [u8; 32],
    out: &mut dyn Write,
) -> Result<(), CliError> {
    check_witness_output_collisions(a)?;
    let span = i64::try_from(a.frames.saturating_sub(1))
        .ok()
        .and_then(|n| n.checked_mul(a.step_ns))
        .ok_or_else(|| CliError::Failed("raw run span is outside clock range".into()))?;
    let end_ns = a
        .start_ns
        .checked_add(span)
        .ok_or_else(|| CliError::Failed("raw run end is outside clock range".into()))?;
    let verify_witness = match &a.raw_witness {
        Some(RawWitnessAction::Verify(path)) => {
            let witness = read_witness(path)?;
            Some(witness)
        }
        Some(RawWitnessAction::Write(path)) => {
            check_witness_output(path)?;
            None
        }
        None => None,
    };
    let (history, clip_sha256) =
        raw_history(&a.adsb, a.start_ns, end_ns, mode, a.raw_witness.is_some())?;
    let clip_sha256 = match (a.raw_witness.is_some(), clip_sha256) {
        (true, Some(digest)) => Some(digest),
        (true, None) => return Err(witness_error("missing raw input fingerprint")),
        (false, None) => None,
        (false, Some(_)) => return Err(witness_error("unexpected raw input fingerprint")),
    };
    if let Some(witness) = verify_witness.as_ref() {
        validate_witness_metadata(
            witness,
            a,
            mode,
            snapshot_sha256,
            clip_sha256.ok_or_else(|| witness_error("missing raw input fingerprint"))?,
        )?;
    }
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
    let mut frame_hashes = Vec::with_capacity(a.frames as usize);
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
        if let Some(witness) = verify_witness.as_ref() {
            if witness.frames.get(i as usize) != Some(&(t_ns, digest)) {
                return Err(witness_error(format!(
                    "replay prefix differs at frame {i}; earlier matching frames may already be published"
                )));
            }
        }
        hashes.push_str(&format!("{i:04} {} {}\n", unix_text(t_ns), to_hex(&digest)));
        frame_hashes.push((t_ns, digest));
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
    if loss > 0 && a.raw_witness.is_some() {
        return Err(witness_error(
            "withheld because the raw replay contains refused or unavailable work",
        ));
    }
    if let Some(RawWitnessAction::Write(path)) = &a.raw_witness {
        let encoded = witness_bytes(
            a,
            mode,
            snapshot_sha256,
            clip_sha256.ok_or_else(|| witness_error("missing raw input fingerprint"))?,
            &frame_hashes,
        )?;
        publish_witness(path, &encoded)?;
        writeln!(
            out,
            "witness  {} (integrity checksum; not authenticated)",
            path.display()
        )
        .map_err(|e| failed("write raw replay summary", e))?;
    }
    if verify_witness.is_some() {
        writeln!(out, "witness  verified exact ordered frame hashes")
            .map_err(|e| failed("write raw replay summary", e))?;
    }
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

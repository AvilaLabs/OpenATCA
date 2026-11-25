//! Replaying a recorded SFDPS stream into the flight record. Shell (RULESET B3): it reads the
//! recordings and drives the core table with the operations the adapter makes. Replay time is each
//! frame's receive time; the lag is applied by the table (`oa-flight`), so a question asked "as of" a
//! time sees only what had arrived and aged by the lag floor.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use oa_feed::Kind;
use oa_fixm::{Decoder, FIELD_COUNT};
use oa_flight::{
    flights_apply, flights_new, flights_op_avail, flights_sweep, FlightConfig, FlightTable, Op,
    Outcome, C_COUNT, OP_COUNT, REM_COUNT, R_COUNT, ST_COUNT,
};
use oa_replay::{
    doc_fixm_limits, doc_xml_limits, list_recordings, merge_counted, new_doc_events,
    spec_from_paths, FeedKind, MergeReport,
};
use oa_xml::Parser;

use crate::adapter::{row_ops, AdapterStats};
use crate::audit::{
    audit_check_limits, audit_emit, audit_frame, FrameDisposition, Observation,
    OperationObservation, ReplayAudit, RowIdentity, RowObservation, StateProjection,
};
use crate::timing::{ReplayTimingReport, TimingBins};

/// Lag histogram bounds: one-second buckets from `LAG_MIN_S` up to (excluding) `LAG_MAX_S`.
pub const LAG_MIN_S: i64 = -10;
pub const LAG_MAX_S: i64 = 300;

/// The lag of a message: receive time minus the message's own timestamp (UK-004). A7.2: cold
/// struct, size guard waived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LagHist {
    pub rows: u64,
    pub min_ms: i64,
    pub max_ms: i64,
    pub under: u64,
    pub over: u64,
    /// Rows by whole second of lag, `LAG_MIN_S` upward.
    pub buckets: Vec<u64>,
}

fn lag_new() -> LagHist {
    LagHist {
        rows: 0,
        min_ms: i64::MAX,
        max_ms: i64::MIN,
        under: 0,
        over: 0,
        buckets: vec![0; (LAG_MAX_S - LAG_MIN_S) as usize],
    }
}

/// The lag in seconds at or under which `num / den` of the rows fall (whole-second resolution,
/// rounded up to the bucket edge); `None` when there are no rows or the quantile is out of range.
pub fn lag_quantile_s(h: &LagHist, num: u64, den: u64) -> Option<i64> {
    if h.rows == 0 || den == 0 {
        return None;
    }
    let want = (u128::from(h.rows) * u128::from(num)).div_ceil(u128::from(den)) as u64;
    let mut seen = h.under;
    if seen >= want {
        return None;
    }
    for (k, c) in h.buckets.iter().enumerate() {
        seen += c;
        if seen >= want {
            return Some(LAG_MIN_S + k as i64 + 1);
        }
    }
    None
}

/// What a replay is asked to do. A7.2: cold struct, size guard waived.
#[derive(Clone, Copy, Debug)]
pub struct ReplayConfig {
    pub flight: FlightConfig,
    /// Frames received before this are skipped without being decoded; frames after `to_ns` end
    /// the run (Unix ns).
    pub from_ns: i64,
    pub to_ns: i64,
    /// How often, in replay time, the table is swept, and the retention and silence limits.
    pub sweep_every_ms: i64,
    pub retain_ms: i64,
    pub silence_ms: i64,
}

/// Reject a backwards replay window before input or output work begins.
pub fn validate_replay_window(from_ns: i64, to_ns: i64) -> io::Result<()> {
    if from_ns > to_ns {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "replay from_ns exceeds to_ns",
        ));
    }
    Ok(())
}

fn raw_input_budget_exceeded(reserved_bytes: u64) -> bool {
    reserved_bytes > crate::audit::MAX_AUDIT_RAW_INPUT_BYTES
}

/// What a replay found. A7.2: cold struct, size guard waived.
pub struct ReplayReport {
    pub frames: u64,
    pub frames_skipped_before: u64,
    pub frames_after_end: u64,
    /// Documents the XML parser refused, and documents the decoder refused whole.
    pub docs_unparsed: u64,
    pub docs_refused: u64,
    /// Refused documents by reason: unsupported root, too many elements, input mismatch, row
    /// capacity, string capacity.
    pub refused_by: [u64; 5],
    pub first_recv_ns: i64,
    pub last_recv_ns: i64,
    pub fixm: oa_fixm::Stats,
    pub adapter: AdapterStats,
    pub lag: LagHist,
    /// Rows that have each decoder field, and the forms of the altitudes and handoff events.
    pub present: [u64; FIELD_COUNT],
    pub assigned_forms: [u64; 6],
    pub requested_forms: [u64; 6],
    pub handoff_events: [u64; 7],
    pub interim_nil: u64,
    pub interim_value: u64,
    /// Refusals by (operation, reason, state of the plan view when refused).
    pub refusals: BTreeMap<(u8, u8, u8), u64>,
    /// Plan view states at the end of the run, over every view of every flight.
    pub view_states: [u64; ST_COUNT],
    pub removed_by_reason: [u64; REM_COUNT],
    /// Reader diagnostics through the stopping point, including a normal bounded-window stop.
    /// Decoded byte counts may include buffered data beyond the stopping frame.
    pub merge: Option<MergeReport>,
    /// SFDPS frames (by receive time) and rows (by their own time) left out because they are in a
    /// sealed window (OA-SEAL-014). A bounded run counts only drops encountered before stopping.
    pub sealed_window_dropped: u64,
    pub positions: Positions,
    pub table: FlightTable,
}

/// Every recording (`.oaf`, `.oaf.gz`) of a directory, sorted by name.
pub fn recordings_in(dir: &std::path::Path) -> io::Result<Vec<PathBuf>> {
    list_recordings(dir)
}

/// H2 bounded inventory enumeration, before allocation/sort. Non-recording entries also
/// consume the 512-entry budget; no audit can hide unlimited directory traversal.
pub fn recordings_for_audit(dir: &std::path::Path) -> io::Result<Vec<PathBuf>> {
    oa_sealed::refuse_io(dir)?;
    let mut paths = Vec::new();
    // seal-allow: parent and every entry are checked; opened descriptors are guarded later.
    for (entries, entry) in std::fs::read_dir(dir)?.enumerate() {
        if entries >= 512 {
            return Err(io::Error::other("H2 audit directory-entry cap"));
        }
        let path = entry?.path();
        oa_sealed::refuse_io(&path)?;
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".oaf") || name.ends_with(".oaf.gz"))
        {
            if paths.len() >= 128 {
                return Err(io::Error::other("H2 audit recording count cap"));
            }
            if path.as_os_str().as_encoded_bytes().len() > 4096 {
                return Err(io::Error::other("H2 audit pathname byte cap"));
            }
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(io::Error::other("H2 audit has no recordings"));
    }
    Ok(paths)
}

const WINDOW_END: &str = "end of the replay window";

/// The latest position a flight reported, as the record had it. A7.2: cold struct, size guard
/// waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// The position's own time (`positionTime`), Unix ms.
    pub ts_ms: i64,
    /// When it became available: the clock after the operations of its message.
    pub avail_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
}

/// The latest position of each flight, keyed by GUFI, bounded: a flight that would take the table
/// past its capacity is refused and counted, none is evicted by a newcomer; the entries of a flight
/// go when the table removes the flight ([`Positions::forget`]). A position older than the one
/// held is counted and not kept. A7.2: cold struct, size guard waived.
#[derive(Clone, Debug, Default)]
pub struct Positions {
    map: BTreeMap<u128, Position>,
    /// The time of the latest listed refile conflict (a plan filed again over an ended one) of
    /// each flight, by GUFI, bounded like the positions.
    refiled: BTreeMap<u128, i64>,
    pub refiled_refused: u64,
    /// Positions left out because their parent message is stamped too far ahead of receipt.
    pub bad_time: u64,
    /// Positions whose own timestamp is beyond availability or too far ahead of receipt.
    pub bad_position_time: u64,
    capacity: usize,
    pub updated: u64,
    pub refused_full: u64,
    pub out_of_order: u64,
}

impl Positions {
    pub fn new(capacity: usize) -> Positions {
        Positions {
            capacity,
            ..Positions::default()
        }
    }

    /// The latest position the flight with this GUFI had made available.
    pub fn get(&self, gufi: u128) -> Option<&Position> {
        self.map.get(&gufi)
    }

    /// The stamped time of the latest refile conflict listed for the flight with this GUFI.
    pub fn refiled_at(&self, gufi: u128) -> Option<i64> {
        self.refiled.get(&gufi).copied()
    }

    fn note_refile(&mut self, gufi: u128, ts_ms: i64) {
        let full = self.refiled.len() >= self.capacity;
        match self.refiled.get_mut(&gufi) {
            Some(h) => *h = (*h).max(ts_ms),
            None if full => self.refiled_refused += 1,
            None => {
                self.refiled.insert(gufi, ts_ms);
            }
        }
    }

    /// Drops what is held for the flight with this GUFI: the position and the refile time.
    pub fn forget(&mut self, gufi: u128) {
        self.map.remove(&gufi);
        self.refiled.remove(&gufi);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// How many flights have a refile time held.
    pub fn refiled_len(&self) -> usize {
        self.refiled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn put(&mut self, gufi: u128, p: Position) {
        let full = self.map.len() >= self.capacity;
        match self.map.get_mut(&gufi) {
            Some(h) if p.ts_ms < h.ts_ms => self.out_of_order += 1,
            Some(h) => {
                *h = p;
                self.updated += 1;
            }
            None if full => self.refused_full += 1,
            None => {
                self.map.insert(gufi, p);
                self.updated += 1;
            }
        }
    }
}

/// Replays the SFDPS recordings `paths` into a new table. Before each operation is applied,
/// `before_op(table, positions, avail_ms)` is called with the time the operation will become available, so
/// the caller can answer questions as of any earlier time; a last call with `i64::MAX` follows the
/// final operation.
pub fn replay_sfdps(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    before_op: &mut dyn FnMut(&FlightTable, &Positions, i64),
) -> io::Result<ReplayReport> {
    replay_sfdps_counted(paths, cfg, before_op, &mut 0)
}

/// Counts one frame that reached the decoder's report.
fn note_frame(rep: &mut ReplayReport, recv_ns: i64) {
    rep.frames += 1;
    rep.first_recv_ns = rep.first_recv_ns.min(recv_ns);
    rep.last_recv_ns = rep.last_recv_ns.max(recv_ns);
}

/// The decoder limits of every replay here: the replay pipeline's own, with the sealed window as
/// the drop window (OA-SEAL-014).
pub fn decoder_limits() -> oa_fixm::Limits {
    doc_fixm_limits()
}

/// [`replay_sfdps`], with the frames (by receive time, from the merge, also when the run stops
/// at `to_ns` or on an error) and the messages (by their own time) dropped in a sealed window put
/// in `sealed` on every exit, so a caller can print the count whatever the result.
pub fn replay_sfdps_counted(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    before_op: &mut dyn FnMut(&FlightTable, &Positions, i64),
    sealed: &mut u64,
) -> io::Result<ReplayReport> {
    replay_sfdps_counted_inner(paths, cfg, before_op, sealed, None, None, None)
        .map(|(report, _)| report)
}

/// Replays as [`replay_sfdps_counted`] and adds bounded per-received-second timing for
/// SFDPS MESSAGE frames with receive times in [measure_from_ns, cfg.to_ns]. The existing
/// inclusive `to_ns` gate is retained. Document-sealed removals are excluded from timing bins
/// and observed receive extrema, but their existing sealed counters remain visible. Timing
/// excludes file reads, decompression, merge work, and frames skipped before `cfg.from_ns`; it
/// includes decode, adapter, flight-table, sweep, and any caller `before_op` hook work, including
/// refusals. This shell diagnostic uses `Instant`; independent replay loops are not synchronized
/// by simulated time.
pub fn replay_sfdps_timed_counted(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    before_op: &mut dyn FnMut(&FlightTable, &Positions, i64),
    sealed: &mut u64,
    measure_from_ns: i64,
) -> io::Result<(ReplayReport, ReplayTimingReport)> {
    let timing = TimingBins::new(cfg.from_ns, measure_from_ns, cfg.to_ns).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid timing bounds: {e:?}"),
        )
    })?;
    let (report, timing) =
        replay_sfdps_counted_inner(paths, cfg, before_op, sealed, Some(timing), None, None)?;
    let timing = timing.ok_or_else(|| io::Error::other("timing report was not produced"))?;
    Ok((report, timing))
}

/// H2 observed-state trace. Uses the same decoder, adapter and engine as ordinary replay.
/// No independent controller behavior agreement is implied. A failed sink/cap returns an error;
/// the caller retains audit counters and must label any partial output INCOMPLETE.
pub fn replay_sfdps_audited_counted(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    before_op: &mut dyn FnMut(&FlightTable, &Positions, i64),
    sealed: &mut u64,
    audit: &mut ReplayAudit<'_>,
) -> io::Result<ReplayReport> {
    validate_replay_window(cfg.from_ns, cfg.to_ns)?;
    audit_check_limits(audit.limits)?;
    if paths.is_empty() || paths.len() > 128 || !audit.inputs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "H2 audit path cap or reused input receipts",
        ));
    }
    if audit.stats != crate::audit::AuditStats::default() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "H2 audit is single-use; counters must start empty",
        ));
    }
    replay_sfdps_counted_inner(paths, cfg, before_op, sealed, None, Some(audit), None)
        .map(|(report, _)| report)
}

/// H2 compatibility sidecar collector. The aggregate is independently derived from decoded
/// source fields and actual adapter operations; it does not create an independent controller
/// prediction. The existing observed-state v1 stream is not modified.
pub fn replay_sfdps_compat_counted(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    before_op: &mut dyn FnMut(&FlightTable, &Positions, i64),
    sealed: &mut u64,
    audit: &mut ReplayAudit<'_>,
    compatibility: &mut crate::compat::CompatibilityAudit,
) -> io::Result<ReplayReport> {
    validate_replay_window(cfg.from_ns, cfg.to_ns)?;
    audit_check_limits(audit.limits)?;
    if paths.is_empty() || paths.len() > 128 || !audit.inputs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "H2 audit path cap or reused input receipts",
        ));
    }
    if audit.stats != crate::audit::AuditStats::default()
        || *compatibility != crate::compat::CompatibilityAudit::default()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "H2 audit is single-use; counters must start empty",
        ));
    }
    replay_sfdps_counted_inner(
        paths,
        cfg,
        before_op,
        sealed,
        None,
        Some(audit),
        Some(compatibility),
    )
    .map(|(report, _)| report)
}

fn finish_timing(
    timing: &mut Option<TimingBins>,
    bin: Option<usize>,
    recv_ns: i64,
    started: Option<Instant>,
) {
    if let (Some(timing), Some(bin), Some(started)) = (timing.as_mut(), bin, started) {
        timing.observe(bin, recv_ns, started.elapsed().as_nanos());
    }
}

fn replay_sfdps_counted_inner(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    before_op: &mut dyn FnMut(&FlightTable, &Positions, i64),
    sealed: &mut u64,
    mut timing: Option<TimingBins>,
    mut audit: Option<&mut ReplayAudit<'_>>,
    mut compatibility: Option<&mut crate::compat::CompatibilityAudit>,
) -> io::Result<(ReplayReport, Option<ReplayTimingReport>)> {
    validate_replay_window(cfg.from_ns, cfg.to_ns)?;
    let mut table = flights_new(&cfg.flight).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut parser =
        Parser::new(doc_xml_limits()).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut decoder = Decoder::new(decoder_limits());
    let mut events = new_doc_events();
    let mut rep = ReplayReport {
        frames: 0,
        frames_skipped_before: 0,
        frames_after_end: 0,
        docs_unparsed: 0,
        docs_refused: 0,
        refused_by: [0; 5],
        first_recv_ns: i64::MAX,
        last_recv_ns: i64::MIN,
        fixm: decoder.stats().clone(),
        adapter: AdapterStats::default(),
        lag: lag_new(),
        present: [0; FIELD_COUNT],
        assigned_forms: [0; 6],
        requested_forms: [0; 6],
        handoff_events: [0; 7],
        interim_nil: 0,
        interim_value: 0,
        refusals: BTreeMap::new(),
        view_states: [0; ST_COUNT],
        removed_by_reason: [0; REM_COUNT],
        merge: None,
        sealed_window_dropped: 0,
        positions: Positions::default(),
        table: flights_new(&FlightConfig {
            capacity: 1,
            ..cfg.flight
        })
        .map_err(|e| io::Error::other(format!("{e:?}")))?,
    };
    let mut positions = Positions::new(cfg.flight.capacity.saturating_mul(2));
    let mut ops: Vec<Op> = Vec::with_capacity(crate::adapter::MAX_OPS_PER_MESSAGE);
    let mut next_sweep = i64::MIN;
    let spec = if let Some(audit) = audit.as_deref_mut() {
        let (spec, inputs, reserved_bytes) =
            oa_replay::source::spec_from_paths_observed(FeedKind::Sfdps, paths)?;
        audit.inputs = inputs;
        audit.stats.raw_input_bytes_reserved = reserved_bytes;
        if raw_input_budget_exceeded(reserved_bytes) {
            audit.stats.raw_input_cap_refused = true;
            audit.stats.cap_refused = true;
            return Err(io::Error::other(
                "H2 aggregate raw/compressed input byte budget exhausted; INCOMPLETE",
            ));
        }
        spec
    } else {
        spec_from_paths(FeedKind::Sfdps, paths)
    };
    let mut ended = false;
    let auditing = audit.is_some();
    let mut sink = |m: &oa_replay::Merged<'_>| {
        if let Some(audit) = audit.as_deref_mut() {
            audit_frame(audit)?;
        }
        let identity = RowIdentity {
            stream: m.stream,
            decoded_offset: m.offset,
            recorder_sequence: m.seq,
            row: 0,
        };
        let mut frame_event = |disposition| -> io::Result<()> {
            if let Some(audit) = audit.as_deref_mut() {
                audit_emit(
                    audit,
                    Observation::Frame {
                        identity,
                        received_ns: m.recv_ns,
                        disposition,
                    },
                )?;
            }
            Ok(())
        };
        if m.feed != FeedKind::Sfdps || m.kind != Kind::Message {
            frame_event(FrameDisposition::OtherFrame)?;
            return Ok(());
        }
        if m.recv_ns < cfg.from_ns {
            frame_event(FrameDisposition::BeforeWindow)?;
            rep.frames_skipped_before += 1;
            return Ok(());
        }
        if m.recv_ns > cfg.to_ns {
            frame_event(FrameDisposition::AfterWindow)?;
            rep.frames_after_end += 1;
            ended = true;
            return Err(io::Error::other(WINDOW_END));
        }
        let timing_bin = timing.as_ref().and_then(|t| t.bin(m.recv_ns));
        let timing_started = timing_bin.map(|_| Instant::now());
        let seen_ms = m.recv_ns.div_euclid(1_000_000);
        let doc = match parser.parse(m.payload) {
            Ok(d) => d,
            Err(_) => {
                note_frame(&mut rep, m.recv_ns);
                frame_event(FrameDisposition::XmlRefused)?;
                rep.docs_unparsed += 1;
                finish_timing(&mut timing, timing_bin, m.recv_ns, timing_started);
                return Ok(());
            }
        };
        oa_fixm::clear_events(&mut events);
        let decoded = decoder.decode_prefix(&doc, m.payload, &mut events);
        if let Ok(s) = &decoded {
            if s.sealed_window_dropped > 0 {
                // The decoder dropped the whole document for a message timed in the sealed
                // window: the frame is in no count here but the drop (OA-SEAL-014).
                *sealed += u64::from(s.sealed_window_dropped);
                oa_sealed::note_window_drops(u64::from(s.sealed_window_dropped));
                frame_event(FrameDisposition::SealedDocument)?;
                return Ok(());
            }
        }
        note_frame(&mut rep, m.recv_ns);
        if let Err(e) = decoded {
            rep.docs_refused += 1;
            let k = match e {
                oa_fixm::FixmError::UnsupportedRoot => 0,
                oa_fixm::FixmError::TooManyElements { .. } => 1,
                oa_fixm::FixmError::InputMismatch => 2,
                oa_fixm::FixmError::RowCapacity { .. } => 3,
                oa_fixm::FixmError::StringCapacity { .. } => 4,
            };
            frame_event(FrameDisposition::DecoderRefused(
                u8::try_from(k).map_err(|_| io::Error::other("decoder reason overflow"))?,
            ))?;
            if let Some(c) = rep.refused_by.get_mut(k) {
                *c += 1;
            }
            finish_timing(&mut timing, timing_bin, m.recv_ns, timing_started);
            return Ok(());
        }
        frame_event(FrameDisposition::Decoded)?;
        for row in 0..events.len {
            if let Some(audit) = audit.as_deref_mut() {
                if audit.stats.rows_delivered >= audit.limits.rows {
                    audit.stats.cap_refused = true;
                    return Err(io::Error::other(
                        "H2 audit row budget exhausted before adaptation; INCOMPLETE",
                    ));
                }
            }
            count_row(&mut rep, &events, row, seen_ms);
            ops.clear();
            let msg = m.seq << 16 | (row as u64 & 0xFFFF);
            let adapter_before = audit.as_ref().map(|_| rep.adapter.clone());
            row_ops(&events, row, seen_ms, msg, &mut rep.adapter, &mut ops);
            if let Some(compatibility) = compatibility.as_deref_mut() {
                compatibility.observe_row(&events, row, &ops)?;
            }
            let row_identity = RowIdentity { row, ..identity };
            if let (Some(audit), Some(previous)) = (audit.as_deref_mut(), adapter_before) {
                let valid = |field| oa_fixm::has_field(&events, row, field);
                audit_emit(
                    audit,
                    Observation::Row(RowObservation {
                        identity: row_identity,
                        source_time_ms: events.ts_ms.get(row).copied(),
                        received_time_ms: seen_ms,
                        gufi: events.gufi.get(row).copied(),
                        present: events.present.get(row).copied(),
                        field_errors: events.errors.get(row).copied(),
                        operations: ops.len(),
                        centre: valid(oa_fixm::Field::Centre)
                            .then(|| events.centre.get(row).copied())
                            .flatten(),
                        reported_owner: valid(oa_fixm::Field::ControllingUnit)
                            .then(|| events.ctrl.get(row).copied())
                            .flatten(),
                        position_time_ms: valid(oa_fixm::Field::PositionTime)
                            .then(|| events.pos_time_ms.get(row).copied())
                            .flatten(),
                        latitude_e7: valid(oa_fixm::Field::Position)
                            .then(|| events.lat_e7.get(row).copied())
                            .flatten(),
                        longitude_e7: valid(oa_fixm::Field::Position)
                            .then(|| events.lon_e7.get(row).copied())
                            .flatten(),
                        reported_altitude_ft: valid(oa_fixm::Field::ReportedAlt)
                            .then(|| events.reported_ft.get(row).copied())
                            .flatten(),
                        no_status: rep.adapter.no_status != previous.no_status,
                        owner_unknown: rep.adapter.owner_unknown != previous.owner_unknown,
                        handoff_incomplete: rep.adapter.handoff_incomplete
                            != previous.handoff_incomplete,
                        handoff_unknown_unit: rep.adapter.handoff_unknown_unit
                            != previous.handoff_unknown_unit,
                        handoff_receiving_only: rep.adapter.handoff_receiving_only
                            != previous.handoff_receiving_only,
                        departure_not_active: rep.adapter.departure_not_active
                            != previous.departure_not_active,
                    }),
                )?;
            }
            for (operation_index, o) in ops.iter().enumerate() {
                let avail = flights_op_avail(&table, o);
                before_op(&table, &positions, avail);
                let out = flights_apply(&mut table, o);
                if let Some(compatibility) = compatibility.as_deref_mut() {
                    compatibility.observe_operation_outcome(&events, row, o.kind, out)?;
                }
                debug_assert!(matches!(
                    out,
                    Outcome::Applied | Outcome::Unchanged | Outcome::Refused(_)
                ));
                if let Some(audit) = audit.as_deref_mut() {
                    let state =
                        oa_flight::flight_get(&table, &o.key).map(|flight| StateProjection {
                            owner: flight.owner,
                            handoff_state: flight.ho_state,
                            handoff_from: flight.ho_from,
                            handoff_to: flight.ho_to,
                            accepted_by: flight.ho_accepted_by,
                            assigned: flight.assigned,
                            interim: flight.interim,
                            requested: flight.requested,
                            stamps: flight.stamps,
                            plan_state: oa_flight::view_of(&flight, &o.centre)
                                .map(|view| view.state),
                        });
                    audit_emit(
                        audit,
                        Observation::Operation(OperationObservation {
                            identity: row_identity,
                            operation_index,
                            operation: *o,
                            outcome: out,
                            state,
                        }),
                    )?;
                }
            }
            // A position is as available as the operations of its message (the clock after
            // them); a message with no operation is placed as an operation of its time would be,
            // and the question is announced first.
            match position_avail(&events, row, seen_ms, &table) {
                PositionAvail::NoPosition => {}
                PositionAvail::BadMessageTime => positions.bad_time += 1,
                PositionAvail::BadPositionTime => positions.bad_position_time += 1,
                PositionAvail::At(avail) => {
                    if avail > table.clock_ms {
                        before_op(&table, &positions, avail);
                    }
                    note_position(&events, row, avail, &mut positions);
                }
            }
        }
        drain_refusals(&mut table, &mut rep, &mut positions);
        if table.clock_ms >= next_sweep {
            if next_sweep != i64::MIN {
                let now = table.clock_ms;
                flights_sweep(&mut table, now, cfg.retain_ms, cfg.silence_ms);
                drain_removals(&mut table, &mut rep, &mut positions);
            }
            next_sweep = table.clock_ms.saturating_add(cfg.sweep_every_ms);
        }
        finish_timing(&mut timing, timing_bin, m.recv_ns, timing_started);
        Ok(())
    };
    // C7: 20M finite reader steps and 64GiB decoded input cover invalid bytes too. A separate
    // 64GiB reservation over full opened-file lengths bounds raw/compressed input before reads;
    // these audit-only caps cannot be raised by a recording or configuration.
    const AUDIT_READER_STEPS: u64 = 20_000_000;
    const AUDIT_DECODED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
    let mut budget = oa_replay::source::MergeBudget {
        steps_remaining: AUDIT_READER_STEPS,
        decoded_bytes_remaining: AUDIT_DECODED_BYTES,
        refused: false,
    };
    let (report, res) = if auditing {
        oa_replay::source::merge_counted_bounded(vec![spec], 1 << 16, &mut sink, &mut budget)
    } else {
        merge_counted(vec![spec], 1 << 16, &mut sink)
    };
    if let Some(audit) = audit.as_mut() {
        audit.stats.cap_refused |= budget.refused;
        audit.stats.merge_files = report.files;
        audit.stats.merge_bytes = report.bytes;
        audit.stats.merge_errors = report.errors;
        audit.stats.reader_steps_used = AUDIT_READER_STEPS - budget.steps_remaining;
        audit.stats.decoded_bytes_charged = AUDIT_DECODED_BYTES - budget.decoded_bytes_remaining;
    }
    // The merge's own count of frames dropped by receive time is added on every way out, a stop at
    // `to_ns` and an error included.
    *sealed += report.sealed_window_dropped;
    rep.sealed_window_dropped = *sealed;
    if let Some(audit) = audit.as_mut() {
        audit.stats.sealed_drops = *sealed;
    }
    match res {
        Ok(()) => {}
        Err(e) if ended && e.to_string() == WINDOW_END => {}
        Err(e) => return Err(e),
    }
    rep.merge = Some(report);
    before_op(&table, &positions, i64::MAX);
    drain_refusals(&mut table, &mut rep, &mut positions);
    drain_removals(&mut table, &mut rep, &mut positions);
    for f in table
        .flights
        .iter()
        .filter(|f| f.key.kind != oa_flight::KEY_FREE)
    {
        for v in f.views.iter().take(usize::from(f.n_views)) {
            if let Some(c) = rep.view_states.get_mut(usize::from(v.state)) {
                *c += 1;
            }
        }
    }
    rep.fixm = decoder.stats().clone();
    rep.positions = positions;
    rep.table = table;
    let timing = timing.map(|bins| bins.report());
    Ok((rep, timing))
}

#[cfg(test)]
mod audit_window_tests {
    use super::*;

    #[test]
    fn audit_window_rejects_only_backwards_bounds() {
        assert!(validate_replay_window(2, 1).is_err());
        assert!(validate_replay_window(1, 1).is_ok());
        assert!(validate_replay_window(i64::MIN, i64::MAX).is_ok());
    }

    #[test]
    fn aggregate_input_budget_accepts_the_limit_and_refuses_above_it() {
        let limit = crate::audit::MAX_AUDIT_RAW_INPUT_BYTES;
        assert!(!raw_input_budget_exceeded(limit));
        assert!(raw_input_budget_exceeded(limit + 1));
    }
}

/// When the position of a row becomes available.
enum PositionAvail {
    /// The row has no position.
    NoPosition,
    /// Its parent message is stamped too far ahead of receipt.
    BadMessageTime,
    /// Its own position timestamp is too far ahead of receipt or availability.
    BadPositionTime,
    At(i64),
}

fn position_avail(
    ev: &oa_fixm::Events,
    row: usize,
    seen_ms: i64,
    table: &FlightTable,
) -> PositionAvail {
    use oa_fixm::{has_field, Field};
    if !has_field(ev, row, Field::Position) || !has_field(ev, row, Field::PositionTime) {
        return PositionAvail::NoPosition;
    }
    let Some(&ts) = ev.ts_ms.get(row) else {
        return PositionAvail::NoPosition;
    };
    if ts.saturating_sub(seen_ms) > table.cfg.max_future_ms {
        return PositionAvail::BadMessageTime;
    }
    let avail = seen_ms
        .max(ts.saturating_add(table.cfg.lag_floor_ms))
        .max(table.clock_ms);
    let Some(&position_ts) = ev.pos_time_ms.get(row) else {
        return PositionAvail::NoPosition;
    };
    if position_ts.saturating_sub(seen_ms) > table.cfg.max_future_ms || position_ts > avail {
        return PositionAvail::BadPositionTime;
    }
    PositionAvail::At(avail)
}

fn note_position(ev: &oa_fixm::Events, row: usize, avail_ms: i64, positions: &mut Positions) {
    let (Some(&g), Some(&pt), Some(&la), Some(&lo)) = (
        ev.gufi.get(row),
        ev.pos_time_ms.get(row),
        ev.lat_e7.get(row),
        ev.lon_e7.get(row),
    ) else {
        return;
    };
    positions.put(
        g,
        Position {
            ts_ms: pt,
            avail_ms,
            lat_e7: la,
            lon_e7: lo,
        },
    );
}

fn count_row(rep: &mut ReplayReport, ev: &oa_fixm::Events, row: usize, seen_ms: i64) {
    use oa_fixm::{has_field, Field, ALL_FIELDS};
    for (k, f) in ALL_FIELDS.iter().enumerate() {
        if has_field(ev, row, *f) {
            if let Some(c) = rep.present.get_mut(k) {
                *c += 1;
            }
        }
    }
    let bump = |a: &mut [u64], i: u8| {
        if let Some(c) = a.get_mut(usize::from(i)) {
            *c += 1;
        }
    };
    if has_field(ev, row, Field::AssignedForm) {
        bump(
            &mut rep.assigned_forms,
            ev.assigned_form.get(row).copied().unwrap_or(0),
        );
    }
    if has_field(ev, row, Field::RequestedForm) {
        bump(
            &mut rep.requested_forms,
            ev.requested_form.get(row).copied().unwrap_or(0),
        );
    }
    if has_field(ev, row, Field::HandoffEvent) {
        bump(
            &mut rep.handoff_events,
            ev.handoff_event.get(row).copied().unwrap_or(0),
        );
    }
    if has_field(ev, row, Field::InterimAlt) {
        if ev
            .flags
            .get(row)
            .is_some_and(|f| f & oa_fixm::FLAG_INTERIM_NIL != 0)
        {
            rep.interim_nil += 1;
        } else {
            rep.interim_value += 1;
        }
    }
    if let Some(&ts) = ev.ts_ms.get(row) {
        let lag = seen_ms - ts;
        let h = &mut rep.lag;
        h.rows += 1;
        h.min_ms = h.min_ms.min(lag);
        h.max_ms = h.max_ms.max(lag);
        let s = lag.div_euclid(1000);
        if s < LAG_MIN_S {
            h.under += 1;
        } else if s >= LAG_MAX_S {
            h.over += 1;
        } else if let Some(c) = h.buckets.get_mut((s - LAG_MIN_S) as usize) {
            *c += 1;
        }
    }
}

fn drain_refusals(t: &mut FlightTable, rep: &mut ReplayReport, positions: &mut Positions) {
    for r in &t.refusals {
        *rep.refusals.entry((r.op, r.reason, r.state)).or_default() += 1;
    }
    t.refusals.clear();
    for c in &t.conflicts {
        if c.kind == oa_flight::C_PLAN_REFILED && c.a.kind == oa_flight::KEY_GUFI {
            positions.note_refile(c.a.gufi, c.ts_ms);
        }
    }
    t.conflicts.clear();
}

/// Counts the removals the table listed and drops what the replay held for those flights.
fn drain_removals(t: &mut FlightTable, rep: &mut ReplayReport, positions: &mut Positions) {
    for r in &t.removals {
        if r.key.kind == oa_flight::KEY_GUFI {
            positions.forget(r.key.gufi);
        }
        if let Some(c) = rep.removed_by_reason.get_mut(usize::from(r.reason)) {
            *c += 1;
        }
    }
    t.removals.clear();
}

/// The number of refusal reasons, for tables the report prints.
pub const REASONS: usize = R_COUNT;
/// The number of operation codes.
pub const OPS: usize = OP_COUNT;
/// The number of conflict kinds.
pub const CONFLICTS: usize = C_COUNT;

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ts_ms: i64) -> Position {
        Position {
            ts_ms,
            avail_ms: ts_ms,
            lat_e7: 1,
            lon_e7: 2,
        }
    }

    // REQ: OA-FLT-040
    #[test]
    fn a_forgotten_flight_frees_its_place_so_the_table_fills_to_capacity_again() {
        let mut p = Positions::new(3);
        for g in 1..=3u128 {
            p.put(g, at(100));
            p.note_refile(g, 100);
        }
        // Full: a newcomer is refused and counted.
        p.put(4, at(100));
        p.note_refile(4, 100);
        assert_eq!((p.len(), p.refused_full, p.refiled_refused), (3, 1, 1));
        // The flight is removed: its entries go, and the table fills to capacity again.
        p.forget(2);
        assert!(p.get(2).is_none() && p.refiled_at(2).is_none());
        assert_eq!((p.len(), p.refiled_len()), (2, 2));
        p.put(4, at(200));
        p.note_refile(4, 200);
        assert_eq!((p.len(), p.refiled_len()), (3, 3));
        assert!(p.get(4).is_some());
        p.put(5, at(300));
        p.note_refile(5, 300);
        assert_eq!((p.refused_full, p.refiled_refused), (2, 2));
        // Forgetting a flight never held is harmless.
        p.forget(99);
        assert_eq!(p.len(), 3);
    }
}

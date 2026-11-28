//! Guarded offline session execution and evidence publication (shell, B1/B3/C6).
//! Each successful run is a new directory; COMPLETE is written last. Prefix reconstruction
//! recomputes state and does not claim a live or distributed process checkpoint.

use crate::input::NormalizedEvent;
use crate::runtime::{
    regional_acknowledge_quarantine_before_commit, regional_adaptation_sha256,
    regional_new_from_issued_package_with_audit_checkpoint,
    regional_new_with_profile_and_quarantine_audit_checkpoint, regional_pixels,
    regional_quarantine_stats, regional_step, QuarantineMaintenanceRequest, RegionalRunProfile,
    DEFAULT_REGIONAL_RUN_PROFILE,
};
use crate::surveillance::{SurveillanceConfig, FRAME_HEIGHT, FRAME_WIDTH, MAX_FRAMES};
use crate::{
    flush_receipt_group, parse_event_line, push_event, session_cursor_new_with_profile_limits,
    session_total_events, ReceiptBatch, SessionCursor, MAX_EVENTS_PER_RECEIPT,
    RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
};
use oa_airspace::core::model::{self, Model};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
// Debug executables include symbols; hash them incrementally rather than retaining the image.
const MAX_EXECUTABLE_BYTES: usize = 256 * 1024 * 1024;
const MAX_FRAME_REPORT_BYTES: usize = 512 * 1024;
const RECORDED_HOUR_FRAME_COLUMNS: &str =
    "sequence\treceipt_ns\tsource_min_ns\tsource_max_ns\tinput_events\tsurveillance_events\tflight_events\troute_events\tweather_events\tadaptation_events\ttracks\ttracked_flight_rows\tcorrelated_flights\tknown_flight_views\tca_open\tca_closed\tmsaw_open\tmsaw_closed\tmsaw_episode_onsets\tstep_ns\trender_ns\tframe_sha256\tmsaw_markers\tavailable_flight_views_for_runtime_centre\tpositive_address_claims_for_runtime_centre\tmatched_observed_tracks\tunmatched_observed_tracks\tambiguous_observed_tracks\trefused_observed_track_lookups\n";
pub const RECORDED_HOUR_FRAMES: usize = 3_600;
const RECORDED_HOUR_NS: i64 = RECORDED_HOUR_FRAMES as i64 * crate::surveillance::SECOND_NS;
const WARMUP_MAX_NS: i64 = crate::recordings_cli::RECORDED_HOUR_MAX_WARMUP_NS;
const RECORDED_HOUR_MAX_FRAMES: usize =
    RECORDED_HOUR_FRAMES + (WARMUP_MAX_NS / crate::surveillance::SECOND_NS) as usize;

// A7.2: cold CLI choice; synthetic adaptation is always explicit.
pub enum ModelSource {
    Snapshot(PathBuf),
    Synthetic,
    IssuedPackage {
        root: PathBuf,
        expected_manifest_sha256: [u8; 32],
        processor_id: u32,
    },
}

impl ModelSource {
    fn evidence_label(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => "SUPPLIED_UNPINNED_SNAPSHOT",
            Self::Synthetic => "PUBLIC_SYNTHETIC",
            Self::IssuedPackage { .. } => "ISSUED_PACKAGE_PIN_VERIFIED",
        }
    }
}

fn recorded_hour_centre(artcc: &str) -> Result<[u8; 4], String> {
    if artcc.is_empty()
        || artcc.len() > 4
        || !artcc.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err("recorded-hour model centre is missing or invalid".into());
    }
    let mut centre = [0u8; 4];
    centre
        .get_mut(..artcc.len())
        .ok_or("recorded-hour model centre is missing or invalid")?
        .copy_from_slice(artcc.as_bytes());
    if !crate::input::is_valid_centre_code(&centre) {
        return Err("recorded-hour model centre is missing or invalid".into());
    }
    Ok(centre)
}

pub enum MsawSource {
    Disabled,
    Synthetic,
    Manifest(PathBuf),
}

/// Explicit caller-supplied inputs for one in-process manual quarantine handoff. The pinned
/// registry is local role configuration, not authenticated identity.
#[derive(Clone, Debug)]
pub struct QuarantineReleaseOptions {
    pub source: oa_quarantine::InputSource,
    pub identity: oa_quarantine::InputIdentity,
    pub manual_handoff_ref: String,
    pub audit_log: PathBuf,
    pub audit_checkpoint: oa_audit_core::Checkpoint,
    pub principal_registry: oa_audit_core::PrincipalRegistry,
    pub principal: u64,
    pub time_ns: u64,
}

/// Persisted CLI profile selection. The absent/default option remains the historical 32-track
/// rendered run; the recorded profile is an explicit bounded high-volume ADS-B replay variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionalCliProfile {
    Default,
    RecordedPeakRendered,
    RecordedHourRendered,
}

/// Half-open source interval for one whole-second replay window up to 3,600 frames. The importer
/// ceiling-rounds source receipts, so corresponding render cutoffs are `(start_ns, end_ns]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedHourWindow {
    pub start_ns: i64,
    pub end_ns: i64,
    pub warmup_start_ns: Option<i64>,
}

impl RecordedHourWindow {
    pub fn new(start_ns: i64, end_ns: i64) -> Result<Self, String> {
        Self::new_with_warmup(start_ns, end_ns, None)
    }

    pub fn new_with_warmup(
        start_ns: i64,
        end_ns: i64,
        warmup_start_ns: Option<i64>,
    ) -> Result<Self, String> {
        let duration_ns = end_ns.checked_sub(start_ns);
        if start_ns < 0
            || end_ns <= start_ns
            || start_ns % crate::surveillance::SECOND_NS != 0
            || end_ns % crate::surveillance::SECOND_NS != 0
            || duration_ns.is_none_or(|duration| duration > RECORDED_HOUR_NS)
        {
            return Err(
                "recorded-hour window must be a whole-second interval up to one hour".into(),
            );
        }
        if let Some(warmup_start) = warmup_start_ns {
            let warmup_duration = start_ns.checked_sub(warmup_start);
            if warmup_start < 0
                || warmup_start % crate::surveillance::SECOND_NS != 0
                || warmup_duration.is_none_or(|duration| !(0..=WARMUP_MAX_NS).contains(&duration))
            {
                return Err(
                    "recorded-hour warmup must be whole-second and within ten minutes".into(),
                );
            }
        }
        Ok(Self {
            start_ns,
            end_ns,
            warmup_start_ns,
        })
    }

    pub fn frame_count(self) -> usize {
        ((self.end_ns - self.start_ns) / crate::surveillance::SECOND_NS) as usize
    }

    fn warmup_frame_count(self) -> usize {
        self.warmup_start_ns.map_or(0, |warmup_start| {
            ((self.start_ns - warmup_start) / crate::surveillance::SECOND_NS) as usize
        })
    }

    fn replay_start_ns(self) -> i64 {
        self.warmup_start_ns.unwrap_or(self.start_ns)
    }

    fn replay_frame_count(self) -> usize {
        self.warmup_frame_count() + self.frame_count()
    }

    fn is_warmup_tick(self, receipt_ns: i64) -> bool {
        self.warmup_start_ns.is_some() && receipt_ns <= self.start_ns
    }
}

/// Bounded shell selections for one run. The hour window is mandatory only for its profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalSessionOptions {
    pub profile: RegionalCliProfile,
    pub recorded_hour: Option<RecordedHourWindow>,
}

pub const fn regional_cli_profile_id(profile: RegionalCliProfile) -> &'static str {
    match profile {
        RegionalCliProfile::Default => "DEFAULT_V1",
        RegionalCliProfile::RecordedPeakRendered => "RECORDED_PEAK_RENDERED_V1",
        RegionalCliProfile::RecordedHourRendered => "RECORDED_HOUR_RENDERED_V2",
    }
}

pub fn regional_cli_runtime_profile(profile: RegionalCliProfile) -> RegionalRunProfile {
    match profile {
        RegionalCliProfile::Default => DEFAULT_REGIONAL_RUN_PROFILE,
        RegionalCliProfile::RecordedPeakRendered => RegionalRunProfile::recorded_peak_rendered(),
        RegionalCliProfile::RecordedHourRendered => {
            let mut profile = RegionalRunProfile::recorded_hour_rendered();
            profile.surveillance.tracker_timing =
                crate::surveillance::TrackerTimingProfile::ArchivedFixed10_30_120V1;
            profile
        }
    }
}

pub fn read_guarded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let (file, len) = oa_sealed::open_regular(path).map_err(|e| format!("guarded input: {e}"))?;
    let cap = u64::try_from(max).map_err(|_| "byte limit width")?;
    if len > cap {
        return Err("input exceeds byte limit".into());
    }
    let mut bytes = Vec::new();
    file.take(cap.checked_add(1).ok_or("byte limit overflow")?)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("input grew beyond byte limit".into());
    }
    Ok(bytes)
}

fn hash_guarded(path: &Path, max: usize) -> Result<[u8; 32], String> {
    let (file, len) = oa_sealed::open_regular(path).map_err(|e| format!("guarded input: {e}"))?;
    let cap = u64::try_from(max).map_err(|_| "byte limit width")?;
    if len > cap {
        return Err("executable exceeds byte limit".into());
    }
    let limit = cap.checked_add(1).ok_or("byte limit overflow")?;
    let mut reader = file.take(limit);
    let digest = oa_sha256::sha256_reader(&mut reader).map_err(|e| e.to_string())?;
    let read = limit
        .checked_sub(reader.limit())
        .ok_or("hash read count overflow")?;
    if read != len || read > cap {
        return Err("executable length changed while hashing".into());
    }
    Ok(digest)
}

#[cfg(test)]
fn fill_recorded_hour_ticks(
    batches: Vec<ReceiptBatch>,
    window: RecordedHourWindow,
) -> Result<Vec<ReceiptBatch>, String> {
    let mut pending = batches.into_iter().peekable();
    let frame_count = window.frame_count();
    let mut ticks = Vec::with_capacity(frame_count);
    for second in 0..frame_count {
        let receipt_ns = window.start_ns + (second as i64 + 1) * crate::surveillance::SECOND_NS;
        match pending.peek() {
            Some(batch) if batch.receipt_ns < receipt_ns => {
                return Err("recorded-hour input contains an unordered or off-grid cutoff".into());
            }
            Some(batch) if batch.receipt_ns == receipt_ns => {
                ticks.push(pending.next().ok_or("recorded-hour batch disappeared")?);
            }
            _ => ticks.push(ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events: Vec::new(),
            }),
        }
    }
    if pending.next().is_some() {
        return Err(
            "recorded-hour input contains a cutoff outside its right-closed frame grid".into(),
        );
    }
    Ok(ticks)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RecordedHourInputCounts {
    warmup_events: u64,
    display_events: u64,
}

fn validate_recorded_hour_input(
    text: &str,
    window: RecordedHourWindow,
    max_events: usize,
    max_events_per_receipt: usize,
) -> Result<RecordedHourInputCounts, String> {
    let mut cursor = session_cursor_new_with_profile_limits(max_events, max_events_per_receipt)
        .map_err(|error| format!("session profile event capacity: {error:?}"))?;
    let mut counts = RecordedHourInputCounts::default();
    for row in text.lines().skip(1).filter(|line| !line.is_empty()) {
        let event = parse_event_line(row).map_err(|error| format!("input: {error:?}"))?;
        if !recorded_hour_cutoff_admitted(event.receipt_ns, window) {
            continue;
        }
        let offset = event.receipt_ns - window.start_ns;
        if offset % crate::surveillance::SECOND_NS != 0 {
            return Err("recorded-hour input receipt is not on a one-second frame boundary".into());
        }
        let count = if window.is_warmup_tick(event.receipt_ns) {
            &mut counts.warmup_events
        } else {
            &mut counts.display_events
        };
        *count = count
            .checked_add(1)
            .ok_or("recorded-hour event count overflow")?;
        let _completed_group =
            push_event(&mut cursor, event).map_err(|error| format!("input order: {error:?}"))?;
    }
    let _final_group =
        flush_receipt_group(&mut cursor).map_err(|error| format!("input order: {error:?}"))?;
    let admitted_events = counts
        .warmup_events
        .checked_add(counts.display_events)
        .ok_or("recorded-hour event count overflow")?;
    if counts.display_events == 0 {
        return Err("recorded-hour input has no admitted events".into());
    }
    if session_total_events(&cursor) != admitted_events {
        return Err("recorded-hour input accounting is inconsistent".into());
    }
    Ok(counts)
}

struct RecordedHourTickStream<'a> {
    rows: std::iter::Skip<std::str::Lines<'a>>,
    cursor: SessionCursor,
    window: RecordedHourWindow,
    pending: Option<ReceiptBatch>,
    next_frame: usize,
    exhausted: bool,
}

impl<'a> RecordedHourTickStream<'a> {
    fn new(
        text: &'a str,
        window: RecordedHourWindow,
        max_events: usize,
        max_events_per_receipt: usize,
    ) -> Result<Self, String> {
        let cursor = session_cursor_new_with_profile_limits(max_events, max_events_per_receipt)
            .map_err(|error| format!("session profile event capacity: {error:?}"))?;
        Ok(Self {
            rows: text.lines().skip(1),
            cursor,
            window,
            pending: None,
            next_frame: 0,
            exhausted: false,
        })
    }

    fn next_batch(&mut self) -> Result<Option<ReceiptBatch>, String> {
        if self.next_frame >= self.window.replay_frame_count() {
            return Ok(None);
        }
        let tick_ns = self.window.replay_start_ns()
            + (self.next_frame as i64 + 1) * crate::surveillance::SECOND_NS;
        self.next_frame += 1;
        loop {
            if let Some(batch) = self.pending.take() {
                if batch.receipt_ns < tick_ns {
                    return Err("recorded-hour stream missed a receipt frame".into());
                }
                if batch.receipt_ns == tick_ns {
                    return Ok(Some(batch));
                }
                self.pending = Some(batch);
                return Ok(Some(empty_receipt_batch(tick_ns)));
            }
            if self.exhausted {
                return Ok(Some(empty_receipt_batch(tick_ns)));
            }
            match self.rows.next() {
                Some("") => continue,
                Some(row) => {
                    let event =
                        parse_event_line(row).map_err(|error| format!("input: {error:?}"))?;
                    if !recorded_hour_cutoff_admitted(event.receipt_ns, self.window) {
                        continue;
                    }
                    let offset = event.receipt_ns - self.window.start_ns;
                    if offset % crate::surveillance::SECOND_NS != 0 {
                        return Err(
                            "recorded-hour input receipt is not on a one-second frame boundary"
                                .into(),
                        );
                    }
                    self.pending = push_event(&mut self.cursor, event)
                        .map_err(|error| format!("input order: {error:?}"))?;
                }
                None => {
                    self.exhausted = true;
                    self.pending = flush_receipt_group(&mut self.cursor)
                        .map_err(|error| format!("input order: {error:?}"))?;
                }
            }
        }
    }
}

fn empty_receipt_batch(receipt_ns: i64) -> ReceiptBatch {
    ReceiptBatch {
        receipt_ns,
        source_clock_regressions: 0,
        events: Vec::new(),
    }
}

fn recorded_hour_cutoff_admitted(receipt_ns: i64, window: RecordedHourWindow) -> bool {
    receipt_ns > window.replay_start_ns() && receipt_ns <= window.end_ns
}

fn recorded_hour_batch_is_display(window: RecordedHourWindow, batch: &ReceiptBatch) -> bool {
    !window.is_warmup_tick(batch.receipt_ns)
}

#[derive(Default)]
struct RecordedHourSummary {
    tracks_total: u64,
    tracks_peak: u32,
    ca_episode_onsets: u64,
    msaw_episode_onsets: u64,
    msaw_caution_onsets: u64,
    msaw_warning_onsets: u64,
}

fn event_source_bounds(events: &[NormalizedEvent]) -> (Option<i64>, Option<i64>) {
    let mut minimum = None;
    let mut maximum = None;
    for event in events {
        minimum = Some(minimum.map_or(event.source_ns, |value: i64| value.min(event.source_ns)));
        maximum = Some(maximum.map_or(event.source_ns, |value: i64| value.max(event.source_ns)));
    }
    (minimum, maximum)
}

fn recorded_hour_frame_row(
    frame: &crate::runtime::RegionalFrame,
    batch: &ReceiptBatch,
    step_ns: u64,
    render_ns: u64,
    image_sha: &[u8; 32],
) -> (String, RecordedHourSummary) {
    let (source_min, source_max) = event_source_bounds(&batch.events);
    let msaw_open = frame
        .surveillance
        .msaw
        .as_ref()
        .map_or(0, |msaw| msaw.open.len());
    let msaw_closed = frame
        .surveillance
        .msaw
        .as_ref()
        .map_or(0, |msaw| msaw.closed.len());
    let msaw_episode_onsets = frame
        .surveillance
        .msaw
        .as_ref()
        .map(|msaw| {
            msaw.open
                .iter()
                .chain(msaw.closed.iter())
                .filter(|alert| alert.start_ns == batch.receipt_ns)
                .count()
        })
        .unwrap_or(0);
    let ca_episode_onsets = frame
        .surveillance
        .ca_open
        .iter()
        .filter(|alert| alert.start_ns == batch.receipt_ns)
        .count()
        .saturating_add(
            frame
                .surveillance
                .ca_closed
                .iter()
                .filter(|alert| alert.start_ns == batch.receipt_ns)
                .count(),
        );
    let (msaw_caution_onsets, msaw_warning_onsets) = frame
        .surveillance
        .msaw
        .as_ref()
        .map(|msaw| {
            let alerts = msaw.open.iter().chain(msaw.closed.iter());
            let caution = alerts
                .clone()
                .filter(|alert| alert.caution_ns == Some(batch.receipt_ns))
                .count();
            let warning = alerts
                .filter(|alert| alert.warning_ns == Some(batch.receipt_ns))
                .count();
            (caution, warning)
        })
        .unwrap_or((0, 0));
    let tracks = u32::try_from(frame.surveillance.tracks.len()).unwrap_or(u32::MAX);
    let correlated = frame
        .flights
        .iter()
        .filter(|row| row.view.is_some())
        .count();
    let [surveillance_events, flight_events, route_events, weather_events, adaptation_events] =
        frame.input.source_events;
    let source_min = source_min.map_or_else(|| "NONE".to_owned(), |value| value.to_string());
    let source_max = source_max.map_or_else(|| "NONE".to_owned(), |value| value.to_string());
    let row = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        frame.sequence,
        batch.receipt_ns,
        source_min,
        source_max,
        batch.events.len(),
        surveillance_events,
        flight_events,
        route_events,
        weather_events,
        adaptation_events,
        tracks,
        frame.flights.len(),
        correlated,
        frame.flight_data.len(),
        frame.surveillance.ca_open.len(),
        frame.surveillance.ca_closed.len(),
        msaw_open,
        msaw_closed,
        msaw_episode_onsets,
        step_ns,
        render_ns,
        oa_sha256::to_hex(image_sha),
        frame.surveillance.msaw_markers.map_or(0, |stats| stats.warning + stats.caution),
        frame
            .flight_correlation
            .available_flight_views_for_runtime_centre,
        frame
            .flight_correlation
            .positive_address_claims_for_runtime_centre,
        frame.flight_correlation.matched_observed_tracks,
        frame.flight_correlation.unmatched_observed_tracks,
        frame.flight_correlation.ambiguous_observed_tracks,
        frame.flight_correlation.refused_observed_track_lookups,
    );
    (
        row,
        RecordedHourSummary {
            tracks_total: tracks as u64,
            tracks_peak: tracks,
            ca_episode_onsets: ca_episode_onsets as u64,
            msaw_episode_onsets: msaw_episode_onsets as u64,
            msaw_caution_onsets: msaw_caution_onsets as u64,
            msaw_warning_onsets: msaw_warning_onsets as u64,
        },
    )
}

fn recorded_hour_compact_frame_report(row: &str) -> Result<String, String> {
    const FIELDS: [(&str, usize); 27] = [
        ("sequence", 0),
        ("receipt_ns", 1),
        ("source_min_ns", 2),
        ("source_max_ns", 3),
        ("input_events", 4),
        ("surveillance_events", 5),
        ("flight_events", 6),
        ("route_events", 7),
        ("weather_events", 8),
        ("adaptation_events", 9),
        ("tracks", 10),
        ("tracked_flight_rows", 11),
        ("correlated_flights", 12),
        ("known_flight_views", 13),
        ("ca_open", 14),
        ("ca_closed", 15),
        ("msaw_open", 16),
        ("msaw_closed", 17),
        ("msaw_episode_onsets", 18),
        ("frame_sha256", 21),
        ("msaw_markers", 22),
        ("available_flight_views_for_runtime_centre", 23),
        ("positive_address_claims_for_runtime_centre", 24),
        ("matched_observed_tracks", 25),
        ("unmatched_observed_tracks", 26),
        ("ambiguous_observed_tracks", 27),
        ("refused_observed_track_lookups", 28),
    ];
    let values = row.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
    let mut report = String::from("OA_REGIONAL_FRAME_COMPACT\t1\n");
    for (name, index) in FIELDS {
        let value = values
            .get(index)
            .ok_or("recorded-hour frame row is missing a compact field")?;
        report.push_str(name);
        report.push('\t');
        report.push_str(value);
        report.push('\n');
    }
    Ok(report)
}

fn recorded_hour_window_settings(
    window: RecordedHourWindow,
    counts: RecordedHourInputCounts,
) -> String {
    let warmup_start = window
        .warmup_start_ns
        .map_or_else(|| "NONE".to_owned(), |value| value.to_string());
    let warmup_end = window
        .warmup_start_ns
        .map_or_else(|| "NONE".to_owned(), |_| window.start_ns.to_string());
    format!(
        "replay_receipt_start_ns_exclusive\t{}\nreplay_receipt_end_ns_inclusive\t{}\ndisplay_receipt_start_ns_exclusive\t{}\ndisplay_receipt_end_ns_inclusive\t{}\ndisplay_ticks\t{}\ndisplay_admitted_events\t{}\nwarmup_enabled\t{}\nwarmup_receipt_start_ns_exclusive\t{}\nwarmup_receipt_end_ns_inclusive\t{}\nwarmup_ticks\t{}\nwarmup_admitted_events\t{}\n",
        window.replay_start_ns(),
        window.end_ns,
        window.start_ns,
        window.end_ns,
        window.frame_count(),
        counts.display_events,
        window.warmup_start_ns.is_some(),
        warmup_start,
        warmup_end,
        window.warmup_frame_count(),
        counts.warmup_events,
    )
}

fn recorded_hour_tracker_timing_settings(
    timing: crate::surveillance::TrackerTimingProfile,
) -> Result<String, String> {
    match timing {
        crate::surveillance::TrackerTimingProfile::ArchivedFixed10_30_120V1 => Ok(format!(
            "tracker_timing_profile\tARCHIVED_FIXED_10_30_120_V1\ntracker_expected_interval_ns\t{}\ntracker_init_interval_ns\t{}\ntracker_drop_after_ns\t{}\n",
            crate::surveillance::RECORDED_HOUR_EXPECTED_INTERVAL_NS,
            crate::surveillance::RECORDED_HOUR_INIT_INTERVAL_NS,
            crate::surveillance::RECORDED_HOUR_DROP_AFTER_NS,
        )),
        crate::surveillance::TrackerTimingProfile::DefaultV1 => {
            Err("recorded-hour tracker timing profile selection is inconsistent".into())
        }
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Public fixed synthetic fixture, containing no operational adaptation/data. The two fixes
/// support an explicit normalized direct route for integration drills.
pub fn synthetic_model() -> Result<Model, String> {
    let mut model = Model {
        meta: model::Meta {
            nasr_effective: 20261101,
            cifp_cycle: 2611,
            artcc: "TEST".into(),
            buffer_m: 0,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        },
        strings: model::StrTable::default(),
        points: model::Points::default(),
        vertices: Vec::new(),
        boundaries: model::Boundaries::default(),
        segments: model::Segments::default(),
        airways: model::Airways::default(),
        airway_legs: model::AirwayLegs::default(),
        procs: model::Procs::default(),
        routes: model::Routes::default(),
        legs: model::Legs::default(),
        preferred_route_state: model::preferred_route_state::UNAVAILABLE,
        preferred_routes: model::PreferredRoutes::default(),
        preferred_route_segments: model::PreferredRouteSegments::default(),
        mora: model::Mora::default(),
        unresolved: model::Unresolved::default(),
        disagreements: model::Disagreements::default(),
        cycle_override: None,
    };
    for (name, lat, lon) in [
        ("FIXA", 410_000_000, -870_000_000),
        ("FIXB", 410_000_000, -850_000_000),
    ] {
        let mut intern = |text: &str| -> Result<model::StrIx, String> {
            let raw = model
                .strings
                .offsets
                .len()
                .checked_sub(1)
                .ok_or("invalid string table")?;
            let index = model::StrIx::new(u32::try_from(raw).map_err(|_| "string index overflow")?);
            model.strings.text.push_str(text);
            model.strings.offsets.push(
                u32::try_from(model.strings.text.len()).map_err(|_| "string length overflow")?,
            );
            Ok(index)
        };
        let ident = intern(name)?;
        let region = intern("K1")?;
        model.points.kind.push(model::point_kind::FIX);
        model.points.lat.push(lat);
        model.points.lon.push(lon);
        model.points.elev_ft.push(model::ELEV_NONE);
        model.points.ident.push(ident);
        model.points.alt_ident.push(model::StrIx::NONE);
        model.points.region.push(region);
        model.points.scope.push(model::StrIx::NONE);
        model.points.name.push(model::StrIx::NONE);
        model::bit_push(&mut model.points.src_nasr, false);
        model::bit_push(&mut model.points.src_cifp, false);
        model::bit_push(&mut model.points.in_area, true);
        model::bit_push(&mut model.points.cls_b, false);
        model::bit_push(&mut model.points.cls_c, false);
        model::bit_push(&mut model.points.cls_d, false);
        model::bit_push(&mut model.points.cls_e, false);
    }
    Ok(model)
}

pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    oa_sealed::refuse(path)?;
    // seal-allow: create-new evidence output after path refusal; write only, never reads an input
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    oa_sealed::Sealed::production()
        .verify_open(path, &file)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

pub fn run_session(bytes: &[u8], output: &Path, source: &ModelSource) -> Result<(), String> {
    run_session_with_safety(bytes, output, source, &MsawSource::Disabled)
}

pub fn run_session_with_safety(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
) -> Result<(), String> {
    run_session_with_options(bytes, output, source, safety, None)
}

pub fn run_session_with_options(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
) -> Result<(), String> {
    run_session_with_profile_options(
        bytes,
        output,
        source,
        safety,
        performance,
        RegionalCliProfile::Default,
    )
}

pub fn run_session_with_profile_options(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
    selected_profile: RegionalCliProfile,
) -> Result<(), String> {
    run_session_with_profile_options_and_quarantine_release(
        bytes,
        output,
        source,
        safety,
        performance,
        selected_profile,
        None,
    )
}

/// Run a bounded replay and optionally append before acknowledging one explicitly selected
/// pending queue front as a caller-claimed manual-responsibility handoff.
pub fn run_session_with_profile_options_and_quarantine_release(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
    selected_profile: RegionalCliProfile,
    quarantine_release: Option<&QuarantineReleaseOptions>,
) -> Result<(), String> {
    run_session_with_options_and_quarantine_release(
        bytes,
        output,
        source,
        safety,
        performance,
        RegionalSessionOptions {
            profile: selected_profile,
            recorded_hour: None,
        },
        quarantine_release,
    )
}

/// Executes one bounded run with optional exact-window schedule metadata.
pub fn run_session_with_options_and_quarantine_release(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
    options: RegionalSessionOptions,
    quarantine_release: Option<&QuarantineReleaseOptions>,
) -> Result<(), String> {
    let selected_profile = options.profile;
    let input_limit = if selected_profile == RegionalCliProfile::RecordedHourRendered {
        crate::recordings_cli::MAX_RECORDED_HOUR_BYTES
    } else {
        16 * 1024 * 1024
    };
    if bytes.len() > input_limit {
        return Err("normalized input exceeds byte limit".into());
    }
    match (selected_profile, options.recorded_hour) {
        (RegionalCliProfile::RecordedHourRendered, Some(_))
        | (RegionalCliProfile::Default | RegionalCliProfile::RecordedPeakRendered, None) => {}
        _ => return Err("recorded-hour window/profile selection is inconsistent".into()),
    }
    if let Some(window) = options.recorded_hour {
        RecordedHourWindow::new_with_warmup(
            window.start_ns,
            window.end_ns,
            window.warmup_start_ns,
        )?;
        for second in 0..window.replay_frame_count() {
            let tick =
                window.replay_start_ns() + (second as i64 + 1) * crate::surveillance::SECOND_NS;
            if oa_sealed::in_sealed_window(tick) {
                return Err("recorded-hour window crosses a sealed source-time interval".into());
            }
        }
    }
    oa_sealed::refuse(output)?;
    // Explicit synthetic-development settings, not FAA separation minima or facility sizing.
    let cfg = SurveillanceConfig {
        centre_lat_e7: 410_000_000,
        centre_lon_e7: -870_000_000,
        range_nm: 180.0,
    };
    let mut profile = regional_cli_runtime_profile(selected_profile);
    if let Some(window) = options.recorded_hour {
        profile.surveillance.max_frames = if window.warmup_start_ns.is_some() {
            RECORDED_HOUR_MAX_FRAMES
        } else {
            window.frame_count()
        };
    }
    let audit_checkpoint =
        quarantine_release.map_or_else(oa_audit_core::genesis, |request| request.audit_checkpoint);
    let mut activation = None;
    let mut startup_processor_report = None;
    let mut settings_centre = "ZAU1".to_owned();
    let mut runtime = match source {
        ModelSource::Snapshot(path) => {
            let model_bytes = read_guarded(path, MAX_SNAPSHOT_BYTES)?;
            let model = oa_airspace::core::snapshot::decode(&model_bytes)
                .map_err(|e| format!("snapshot: {e:?}"))?;
            let snapshot_sha = oa_sha256::sha256(&model_bytes);
            let centre = if selected_profile == RegionalCliProfile::RecordedHourRendered {
                settings_centre = model.meta.artcc.clone();
                recorded_hour_centre(&settings_centre)?
            } else {
                *b"ZAU1"
            };
            regional_new_with_profile_and_quarantine_audit_checkpoint(
                model,
                snapshot_sha,
                centre,
                cfg,
                profile,
                audit_checkpoint,
            )?
        }
        ModelSource::Synthetic => {
            let model_bytes = oa_airspace::core::snapshot::encode(&synthetic_model()?)
                .map_err(|e| format!("synthetic snapshot: {e:?}"))?;
            let model = oa_airspace::core::snapshot::decode(&model_bytes)
                .map_err(|e| format!("synthetic snapshot: {e:?}"))?;
            let snapshot_sha = oa_sha256::sha256(&model_bytes);
            regional_new_with_profile_and_quarantine_audit_checkpoint(
                model,
                snapshot_sha,
                *b"ZAU1",
                cfg,
                profile,
                audit_checkpoint,
            )?
        }
        ModelSource::IssuedPackage {
            root,
            expected_manifest_sha256,
            processor_id,
        } => {
            let (package, local_activation) = match oa_adapt::local_inventory::verify_local_startup(
                root,
                *expected_manifest_sha256,
                *processor_id,
            ) {
                Ok(verified) => verified,
                Err(_) => {
                    fs::create_dir(output)
                        .map_err(|error| format!("create startup refusal output: {error}"))?;
                    let report = crate::processor_inventory::startup_refusal_report(
                        *processor_id,
                        *expected_manifest_sha256,
                        crate::processor_inventory::observed_unix_time_ns(),
                    );
                    write_new(&output.join("processor-report.tsv"), report.as_bytes())?;
                    return Err(
                        "issued adaptation startup verification refused; no model activated".into(),
                    );
                }
            };
            let centre = if selected_profile == RegionalCliProfile::RecordedHourRendered {
                let facility = package.identity().facility.clone();
                settings_centre = facility.clone();
                recorded_hour_centre(&facility)?
            } else {
                *b"ZAU1"
            };
            let runtime = regional_new_from_issued_package_with_audit_checkpoint(
                package,
                centre,
                cfg,
                profile,
                audit_checkpoint,
            )?;
            let report = crate::processor_inventory::check_active_runtime_adaptation(
                &runtime,
                &local_activation,
                crate::processor_inventory::observed_unix_time_ns(),
            )
            .map_err(str::to_owned)?;
            if report.status != oa_adapt::local_inventory::LocalInventoryStatus::Match {
                return Err(format!(
                    "issued adaptation startup disk recheck refused: {:?}\n{}",
                    report.status,
                    oa_adapt::local_inventory::render_processor_report_tsv("STARTUP", &report)
                ));
            }
            activation = Some(local_activation);
            startup_processor_report = Some(report);
            runtime
        }
    };
    let snapshot_sha = regional_adaptation_sha256(&runtime);
    let map_evidence = if options.recorded_hour.is_some() {
        let evidence = crate::runtime::regional_map_evidence(&runtime)?;
        if evidence.len() > 256 * 1024 {
            return Err("recorded-hour map evidence exceeds its fixed bound".into());
        }
        Some(evidence)
    } else {
        None
    };
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let executable_sha = hash_guarded(&executable, MAX_EXECUTABLE_BYTES)?;
    let text = std::str::from_utf8(bytes).map_err(|_| "input not UTF-8")?;
    if text.lines().next() != Some("OA-RS\t1") {
        return Err("expected OA-RS header".into());
    }
    let receipt_group_limit = if options.recorded_hour.is_some() {
        RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
    } else {
        MAX_EVENTS_PER_RECEIPT
    };
    let (batches, admitted_event_total, warmup_event_total) = if let Some(window) =
        options.recorded_hour
    {
        let counts = validate_recorded_hour_input(
            text,
            window,
            profile.max_total_events,
            receipt_group_limit,
        )?;
        (Vec::new(), counts.display_events, counts.warmup_events)
    } else {
        let mut cursor =
            session_cursor_new_with_profile_limits(profile.max_total_events, receipt_group_limit)
                .map_err(|error| format!("session profile event capacity: {error:?}"))?;
        let mut batches = Vec::new();
        for row in text.lines().skip(1).filter(|line| !line.is_empty()) {
            let event = parse_event_line(row).map_err(|error| format!("input: {error:?}"))?;
            if let Some(batch) =
                push_event(&mut cursor, event).map_err(|error| format!("input order: {error:?}"))?
            {
                if batches.len() >= MAX_FRAMES {
                    return Err("frame capacity exhausted".into());
                }
                batches.push(batch);
            }
        }
        if let Some(batch) =
            flush_receipt_group(&mut cursor).map_err(|error| format!("input order: {error:?}"))?
        {
            if batches.len() >= MAX_FRAMES {
                return Err("frame capacity exhausted".into());
            }
            batches.push(batch);
        }
        if batches.is_empty() {
            return Err("session contains no events".into());
        }
        (batches, session_total_events(&cursor), 0)
    };
    let frame_count = match options.recorded_hour {
        Some(window) => window.frame_count(),
        None => batches.len(),
    };
    let mut recorded_stream = options
        .recorded_hour
        .map(|window| {
            RecordedHourTickStream::new(text, window, profile.max_total_events, receipt_group_limit)
        })
        .transpose()?;
    let first = match options.recorded_hour {
        Some(window) => window.replay_start_ns() + crate::surveillance::SECOND_NS,
        None => batches.first().ok_or("session has no cutoff")?.receipt_ns,
    };
    let mut legacy_batches = batches.into_iter();
    let performance_data = performance
        .map(crate::performance::load_performance)
        .transpose()?;
    let mut safety_data = None;
    let safety_manifest_sha = match safety {
        MsawSource::Disabled => None,
        MsawSource::Synthetic => {
            if !matches!(source, ModelSource::Synthetic) {
                return Err(
                    "synthetic MSAW requires the explicit synthetic adaptation model".into(),
                );
            }
            crate::runtime::regional_enable_synthetic_msaw(&mut runtime)?;
            None
        }
        MsawSource::Manifest(path) => {
            let (data, manifest_sha) =
                crate::msaw_bridge::load_msaw_manifest(path, first / 1_000_000)?;
            let evidence = format!(
                "OA_REGIONAL_MSAW_DATA\t1\nprovenance\n{:#?}\nsettings\n{:#?}\nlimits\n{:#?}\n",
                data.provenance,
                data.settings,
                oa_msaw_run::regional::RegionalDataLimits::default()
            );
            if evidence.len() > 2 * 1024 * 1024 {
                return Err("MSAW startup evidence exceeds 2 MiB".into());
            }
            safety_data = Some(evidence);
            crate::runtime::regional_enable_msaw(&mut runtime, data)?;
            Some(manifest_sha)
        }
    };
    // Runtime failures precede final output publication. Temporary evidence is identifiable by
    // the absence of COMPLETE and is never accepted as a closed result.
    fs::create_dir(output).map_err(|e| format!("create new output: {e}"))?;
    let mut processor_report = match &startup_processor_report {
        Some(report) => oa_adapt::local_inventory::render_processor_report_tsv("STARTUP", report),
        None => crate::processor_inventory::unpinned_source_report(),
    };
    let mut settings = match selected_profile {
        // Preserve the prior default settings bytes and behavior exactly.
        RegionalCliProfile::Default => format!("OA_REGIONAL_SETTINGS\t1\ncentre\tZAU1\nconfiguration\t{cfg:?}\ntracks\t32\nframes\t512\nframe_gap_max_ns\t60000000000\nCA_h_m\t1300\nCA_v_ft\t300\nCA_lookahead_ns\t120000000000\ntrajectory_track_age_max_ms\t5000\nnormalization\tpositive-address airborne positions\nmodel\t{}\n", source.evidence_label()),
        RegionalCliProfile::RecordedPeakRendered => format!("OA_REGIONAL_SETTINGS\t1\ncentre\tZAU1\nprofile\t{}\nconfiguration\t{cfg:?}\ntracks\t{}\nflights\t{}\nframes\t512\nsurveillance_max_frames\t{}\nmax_total_events\t{}\nmax_drain_rows\t{}\nmax_baro_evidence\t{}\nmax_open_alerts\t{}\nmax_candidate_pairs\t{}\nmax_ca_work_per_frame\t{}\nmax_flight_log_entries\t{}\nmax_presentation_updates\t{}\nrender_scope\t{}\nframe_gap_max_ns\t60000000000\nCA_h_m\t1300\nCA_v_ft\t300\nCA_lookahead_ns\t120000000000\ntrajectory_track_age_max_ms\t5000\nnormalization\tpositive-address airborne positions\nmodel\t{}\n", regional_cli_profile_id(selected_profile), profile.surveillance.max_tracks, profile.max_flights, profile.surveillance.max_frames, profile.max_total_events, profile.surveillance.max_drain_rows, profile.surveillance.max_baro_evidence, profile.surveillance.max_open_alerts, profile.surveillance.max_candidate_pairs, profile.surveillance.max_ca_work_per_frame, profile.max_flight_log_entries, profile.surveillance.max_presentation_updates, profile.surveillance.render_scope, source.evidence_label()),
        RegionalCliProfile::RecordedHourRendered => format!("OA_REGIONAL_SETTINGS\t1\ncentre\t{}\nprofile\t{}\nconfiguration\t{cfg:?}\ntracks\t{}\nflights\t{}\nframes\t{}\nframe_cadence_ns\t{}\nsource_window_clock_basis\tOA-RS-v1-unqualified-source-ns\nsource_window_start_ns_inclusive\t{}\nsource_window_end_ns_exclusive\t{}\nrender_cutoff_clock_basis\tOA-RS-v1-ceiling-receipt-cutoff-ns\nrender_cutoff_start_ns_exclusive\t{}\nrender_cutoff_end_ns_inclusive\t{}\nsurveillance_max_frames\t{}\nmax_total_events\t{}\nmax_events_per_receipt\t{}\nmax_drain_rows\t{}\nmax_baro_evidence\t{}\nmax_open_alerts\t{}\nmax_candidate_pairs\t{}\nmax_ca_work_per_frame\t{}\nmax_flight_log_entries\t{}\nmax_presentation_updates\t{}\nrender_scope\t{}\nframe_gap_max_ns\t60000000000\nCA_h_m\t1300\nCA_v_ft\t300\nCA_lookahead_ns\t120000000000\ntrajectory_track_age_max_ms\t5000\nnormalization\tpositive-address airborne positions\nmodel\t{}\n", settings_centre, regional_cli_profile_id(selected_profile), profile.surveillance.max_tracks, profile.max_flights, options.recorded_hour.ok_or("recorded-hour settings lack a window")?.frame_count(), crate::surveillance::SECOND_NS, options.recorded_hour.ok_or("recorded-hour settings lack a window")?.start_ns, options.recorded_hour.ok_or("recorded-hour settings lack a window")?.end_ns, options.recorded_hour.ok_or("recorded-hour settings lack a window")?.start_ns, options.recorded_hour.ok_or("recorded-hour settings lack a window")?.end_ns, profile.surveillance.max_frames, profile.max_total_events, receipt_group_limit, profile.surveillance.max_drain_rows, profile.surveillance.max_baro_evidence, profile.surveillance.max_open_alerts, profile.surveillance.max_candidate_pairs, profile.surveillance.max_ca_work_per_frame, profile.max_flight_log_entries, profile.surveillance.max_presentation_updates, profile.surveillance.render_scope, source.evidence_label()),
    };
    if let Some(window) = options.recorded_hour {
        settings.push_str(&recorded_hour_window_settings(
            window,
            RecordedHourInputCounts {
                warmup_events: warmup_event_total,
                display_events: admitted_event_total,
            },
        ));
        settings.push_str(&recorded_hour_tracker_timing_settings(
            profile.surveillance.tracker_timing,
        )?);
    }
    settings.push_str(match safety {
        MsawSource::Disabled => "msaw\tUNAVAILABLE_NOT_CONFIGURED\n",
        MsawSource::Synthetic => "msaw\tPUBLIC_SYNTHETIC_FLOOR_WEATHER\n",
        MsawSource::Manifest(_) => "msaw\tSUPPLIED_DATA_MANIFEST\n",
    });
    if let Some(sha) = safety_manifest_sha {
        settings.push_str(&format!(
            "msaw_input_manifest_sha256\t{}\n",
            oa_sha256::to_hex(&sha)
        ));
    }
    if let Some(data) = &performance_data {
        settings.push_str(&format!(
            "vertical_profile\tEXPLICIT_PERFORMANCE\nperformance_sha256\t{}\n",
            oa_sha256::to_hex(&data.sha256)
        ));
    } else {
        settings.push_str("vertical_profile\tDISABLED\n");
    }
    if let Some(evidence) = &map_evidence {
        settings.push_str(&format!(
            "map_evidence_sha256\t{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(evidence.as_bytes()))
        ));
    }
    write_new(&output.join("settings.tsv"), settings.as_bytes())?;
    let manifest_header = if options.recorded_hour.is_some() {
        "OA_REGIONAL_HOUR_MANIFEST\t1"
    } else {
        "OA_REGIONAL_MANIFEST\t1"
    };
    let mut manifest = format!("{manifest_header}\ninput_sha256\t{}\nsnapshot_sha256\t{}\nexecutable_sha256\t{}\nsettings_sha256\t{}\n", oa_sha256::to_hex(&oa_sha256::sha256(bytes)), oa_sha256::to_hex(&snapshot_sha), oa_sha256::to_hex(&executable_sha), oa_sha256::to_hex(&oa_sha256::sha256(settings.as_bytes())));
    if let Some(data) = safety_data {
        write_new(&output.join("msaw-data.txt"), data.as_bytes())?;
        manifest.push_str(&format!(
            "msaw_data_sha256\t{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(data.as_bytes()))
        ));
    }
    if let Some(data) = &performance_data {
        write_new(&output.join("performance-inputs.tsv"), &data.bytes)?;
        manifest.push_str(&format!(
            "performance_sha256\t{}\n",
            oa_sha256::to_hex(&data.sha256)
        ));
    }
    if let Some(evidence) = &map_evidence {
        write_new(&output.join("map-evidence.tsv"), evidence.as_bytes())?;
        manifest.push_str(&format!(
            "map_evidence\tmap-evidence.tsv\t{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(evidence.as_bytes()))
        ));
    }
    let mut hour_rows = options
        .recorded_hour
        .map(|_| format!("OA_REGIONAL_FRAME_INDEX\t1\n{RECORDED_HOUR_FRAME_COLUMNS}"));
    let mut hour_summary = RecordedHourSummary::default();
    let mut processed_frames = 0usize;
    let mut processed_warmup_frames = 0usize;
    loop {
        let batch = match &mut recorded_stream {
            Some(stream) => stream.next_batch()?,
            None => legacy_batches.next(),
        };
        let Some(batch) = batch else {
            break;
        };
        let step_started = Instant::now();
        let frame = match &performance_data {
            Some(data) => crate::runtime::regional_step_with_performance(
                &mut runtime,
                &batch,
                &data.envelopes,
            )?,
            None => regional_step(&mut runtime, &batch)?,
        };
        if options
            .recorded_hour
            .is_some_and(|window| !recorded_hour_batch_is_display(window, &batch))
        {
            processed_warmup_frames = processed_warmup_frames
                .checked_add(1)
                .ok_or("warmup frame count overflow")?;
            continue;
        }
        let step_ns = elapsed_ns(step_started);
        let name = format!("frame-{:04}.txt", frame.sequence);
        let legacy_evidence = if hour_rows.is_none() {
            let evidence = format!("OA_REGIONAL_FRAME\t1\n{frame:#?}\n");
            if evidence.len() > MAX_FRAME_REPORT_BYTES {
                return Err("frame report byte budget exceeded".into());
            }
            write_new(&output.join(&name), evidence.as_bytes())?;
            Some(evidence)
        } else {
            None
        };
        let render_started = Instant::now();
        let pixels = regional_pixels(&runtime)?;
        let mut ppm = Vec::new();
        let spec = oa_display::raster::FrameSpec {
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            stride: FRAME_WIDTH * 4,
        };
        oa_display::ppm::write_ppm(&mut ppm, &spec, pixels)
            .map_err(|e| format!("frame encoding: {e}"))?;
        let image = format!("frame-{:04}.ppm", frame.sequence);
        write_new(&output.join(&image), &ppm)?;
        let render_ns = elapsed_ns(render_started);
        let image_sha = oa_sha256::sha256(&ppm);
        let compact_report = if let Some(rows) = &mut hour_rows {
            let (row, current) =
                recorded_hour_frame_row(&frame, &batch, step_ns, render_ns, &image_sha);
            rows.push_str(&row);
            hour_summary.tracks_total = hour_summary
                .tracks_total
                .saturating_add(current.tracks_total);
            hour_summary.tracks_peak = hour_summary.tracks_peak.max(current.tracks_peak);
            hour_summary.ca_episode_onsets = hour_summary
                .ca_episode_onsets
                .saturating_add(current.ca_episode_onsets);
            hour_summary.msaw_episode_onsets = hour_summary
                .msaw_episode_onsets
                .saturating_add(current.msaw_episode_onsets);
            hour_summary.msaw_caution_onsets = hour_summary
                .msaw_caution_onsets
                .saturating_add(current.msaw_caution_onsets);
            hour_summary.msaw_warning_onsets = hour_summary
                .msaw_warning_onsets
                .saturating_add(current.msaw_warning_onsets);
            Some(format!(
                "{}tracker_stats\t{:?}\nmeasurement_rejections\t{}\nduplicate_rows_replaced\t{}\n",
                recorded_hour_compact_frame_report(&row)?,
                frame.surveillance.tracker_stats,
                frame.surveillance.measurement_rejections,
                frame.surveillance.duplicate_rows_replaced,
            ))
        } else {
            None
        };
        let evidence = compact_report
            .or(legacy_evidence)
            .ok_or("frame report selection is inconsistent")?;
        if evidence.len() > MAX_FRAME_REPORT_BYTES {
            return Err("frame report byte budget exceeded".into());
        }
        if options.recorded_hour.is_some() {
            write_new(&output.join(&name), evidence.as_bytes())?;
        }
        let report_sha = oa_sha256::to_hex(&oa_sha256::sha256(evidence.as_bytes()));
        if options.recorded_hour.is_some() {
            manifest.push_str(&format!(
                "frame\t{}\t{}\t{}\t{}\t{}\t{}\n",
                frame.sequence,
                frame.input.receipt_ns,
                name,
                report_sha,
                image,
                oa_sha256::to_hex(&image_sha)
            ));
        } else {
            manifest.push_str(&format!(
                "frame\t{}\t{}\t{}\t{}\t{}\n",
                frame.sequence,
                frame.input.receipt_ns,
                name,
                report_sha,
                oa_sha256::to_hex(&image_sha)
            ));
        }
        processed_frames = processed_frames
            .checked_add(1)
            .ok_or("rendered frame count overflow")?;
    }
    if let Some(rows) = hour_rows {
        let expected_warmup_frames = options
            .recorded_hour
            .ok_or("hour report lacks its window")?
            .warmup_frame_count();
        if processed_frames != frame_count || processed_warmup_frames != expected_warmup_frames {
            return Err("recorded-hour output did not contain its declared frame steps".into());
        }
        let summary = format!(
            "OA_REGIONAL_HOUR_SUMMARY\t1\nframes\t{}\nadmitted_input_events\t{}\ntracks_mean_milli\t{}\ntracks_peak\t{}\nca_episode_onsets\t{}\nmsaw_episode_onsets\t{}\nmsaw_caution_transition_onsets\t{}\nmsaw_warning_transition_onsets\t{}\nsource_time_domain\tOA-RS-v1-unqualified-source-ns\nsource_window_start_ns_inclusive\t{}\nsource_window_end_ns_exclusive\t{}\nrender_cutoff_clock_basis\tOA-RS-v1-ceiling-receipt-cutoff-ns\nrender_cutoff_start_ns_exclusive\t{}\nrender_cutoff_end_ns_inclusive\t{}\n",
            frame_count,
            admitted_event_total,
            hour_summary.tracks_total.saturating_mul(1000) / frame_count as u64,
            hour_summary.tracks_peak,
            hour_summary.ca_episode_onsets,
            hour_summary.msaw_episode_onsets,
            hour_summary.msaw_caution_onsets,
            hour_summary.msaw_warning_onsets,
            options.recorded_hour.ok_or("hour report lacks its window")?.start_ns,
            options.recorded_hour.ok_or("hour report lacks its window")?.end_ns,
            options.recorded_hour.ok_or("hour report lacks its window")?.start_ns,
            options.recorded_hour.ok_or("hour report lacks its window")?.end_ns,
        );
        write_new(&output.join("frame-index.tsv"), rows.as_bytes())?;
        write_new(&output.join("hour-summary.tsv"), summary.as_bytes())?;
        manifest.push_str(&format!(
            "frame_index\tframe-index.tsv\t{}\nhour_summary\thour-summary.tsv\t{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(rows.as_bytes())),
            oa_sha256::to_hex(&oa_sha256::sha256(summary.as_bytes())),
        ));
    }
    if let Some(request) = quarantine_release {
        let receipt = regional_acknowledge_quarantine_before_commit(
            &mut runtime,
            QuarantineMaintenanceRequest {
                source: request.source,
                expected_identity: request.identity,
                audit_log: &request.audit_log,
                expected_checkpoint: request.audit_checkpoint,
                principal_registry: &request.principal_registry,
                principal: request.principal,
                time_ns: request.time_ns,
                manual_handoff_ref: &request.manual_handoff_ref,
            },
        )?;
        let stats = regional_quarantine_stats(&runtime)?;
        let report = format!(
            "OA_REGIONAL_QUARANTINE_MAINTENANCE\t1\nsource\t{:?}\nsource_instance\t{}\nitem_id\t{}\nreasons\t{}\nclock_domain_id\t{}\ncompleted_at_ns\t{}\nmanual_handoff_ref\t{}\naudit_count\t{}\naudit_tail\t{}\nsource_stats\t{stats:?}\n",
            receipt.event.identity.source,
            receipt.event.identity.source_instance,
            receipt.event.identity.item_id,
            receipt.event.reasons.0,
            receipt.event.clock_domain_id,
            receipt.event.completed_at_ns,
            request.manual_handoff_ref,
            receipt.audit_checkpoint.count,
            oa_sha256::to_hex(&receipt.audit_checkpoint.tail),
        );
        if report.len() > 4096 {
            return Err("quarantine maintenance report exceeds its fixed bound".into());
        }
        write_new(
            &output.join("quarantine-maintenance.tsv"),
            report.as_bytes(),
        )?;
        manifest.push_str(&format!(
            "quarantine_maintenance\tquarantine-maintenance.tsv\t{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(report.as_bytes()))
        ));
    }
    if let Some(local_activation) = activation.as_ref() {
        let report = crate::processor_inventory::check_active_runtime_adaptation(
            &runtime,
            local_activation,
            crate::processor_inventory::observed_unix_time_ns(),
        )
        .map_err(str::to_owned)?;
        processor_report.push_str(&oa_adapt::local_inventory::render_processor_report_tsv(
            "ON_DEMAND",
            &report,
        ));
        if report.status != oa_adapt::local_inventory::LocalInventoryStatus::Match {
            write_new(
                &output.join("processor-report.tsv"),
                processor_report.as_bytes(),
            )?;
            return Err(format!(
                "on-demand issued adaptation check reported {:?}; loaded model retained and disk model not adopted",
                report.status
            ));
        }
    }
    write_new(
        &output.join("processor-report.tsv"),
        processor_report.as_bytes(),
    )?;
    manifest.push_str(&format!(
        "processor_report_sha256\t{}\n",
        oa_sha256::to_hex(&oa_sha256::sha256(processor_report.as_bytes()))
    ));

    write_new(&output.join("manifest.tsv"), manifest.as_bytes())?;
    let directory = oa_sealed::open(output).map_err(|e| e.to_string())?;
    directory.sync_all().map_err(|e| e.to_string())?;
    let complete = output.join("COMPLETE");
    if let Err(error) = write_new(
        &complete,
        format!(
            "{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(manifest.as_bytes()))
        )
        .as_bytes(),
    ) {
        // Never leave an apparently valid commit marker after a write or durability failure.
        let cleanup = std::fs::remove_file(&complete);
        return Err(format!(
            "completion marker failed: {error}; cleanup={cleanup:?}"
        ));
    }
    if let Err(error) = directory.sync_all() {
        let cleanup = std::fs::remove_file(&complete);
        return Err(format!(
            "completion directory sync failed: {error}; cleanup={cleanup:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod recorded_hour_tests {
    use super::*;
    use crate::input::{EventKind, FlightOperation as EventFlightOperation, Identity, SourceKind};
    use std::sync::atomic::{AtomicU64, Ordering};

    static WARMUP_OUTPUT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn synthetic_hour_runtime() -> crate::runtime::RegionalRuntime {
        let model = crate::cli::synthetic_model().expect("public synthetic adaptation");
        let snapshot = oa_airspace::core::snapshot::encode(&model).expect("snapshot encoding");
        regional_new_with_profile_and_quarantine_audit_checkpoint(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            regional_cli_runtime_profile(RegionalCliProfile::RecordedHourRendered),
            oa_audit_core::genesis(),
        )
        .expect("synthetic runtime")
    }

    fn synthetic_warmup_runtime(window: RecordedHourWindow) -> crate::runtime::RegionalRuntime {
        let model = crate::cli::synthetic_model().expect("public synthetic adaptation");
        let snapshot = oa_airspace::core::snapshot::encode(&model).expect("snapshot encoding");
        let mut profile = DEFAULT_REGIONAL_RUN_PROFILE;
        profile.surveillance.max_frames = window.replay_frame_count();
        profile.max_events_per_receipt = RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT;
        regional_new_with_profile_and_quarantine_audit_checkpoint(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            profile,
            oa_audit_core::genesis(),
        )
        .expect("small warmup runtime")
    }

    fn event(receipt_ns: i64, sequence: u64) -> NormalizedEvent {
        NormalizedEvent {
            receipt_ns,
            source_ns: receipt_ns,
            sequence,
            source_message_id: None,
            source: SourceKind::Surveillance,
            identity: Identity {
                flight_gufi: 0,
                aircraft_address: 0xabc123,
            },
            centre: *b"ZAU1",
            kind: EventKind::Position {
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
                altitude_ft: Some(10_000),
                east_mmps: 0,
                north_mmps: 0,
                vertical_rate_fpm: None,
            },
        }
    }

    // Mutation killed: left-closed cutoff admission drops the importer’s final ceiling bucket;
    // omitting empty ticks loses one-second steps, and replaying a batch duplicates events.
    #[test]
    fn hour_schedule_runs_every_tick_and_places_events_once() {
        let start = 1_800_000_000_000_000_000i64;
        let window =
            RecordedHourWindow::new(start, start + RECORDED_HOUR_NS).expect("valid hour window");
        let batches = vec![
            ReceiptBatch {
                receipt_ns: start + crate::surveillance::SECOND_NS,
                source_clock_regressions: 0,
                events: vec![event(start + crate::surveillance::SECOND_NS, 1)],
            },
            ReceiptBatch {
                receipt_ns: start + 3 * crate::surveillance::SECOND_NS,
                source_clock_regressions: 0,
                events: vec![event(start + 3 * crate::surveillance::SECOND_NS, 2)],
            },
            ReceiptBatch {
                receipt_ns: start + RECORDED_HOUR_NS,
                source_clock_regressions: 0,
                events: vec![event(start + RECORDED_HOUR_NS, 3)],
            },
        ];
        let input_event_count = batches
            .iter()
            .map(|batch| batch.events.len())
            .sum::<usize>();
        let ticks = fill_recorded_hour_ticks(batches, window).expect("full tick schedule");
        assert_eq!(ticks.len(), RECORDED_HOUR_FRAMES);
        assert_eq!(
            ticks.first().map(|batch| batch.receipt_ns),
            Some(window.start_ns + crate::surveillance::SECOND_NS)
        );
        assert_eq!(
            ticks.last().map(|batch| batch.receipt_ns),
            Some(window.end_ns)
        );
        assert!(ticks.first().is_some_and(|batch| !batch.events.is_empty()));
        assert!(ticks.get(1).is_some_and(|batch| batch.events.is_empty()));
        assert_eq!(
            ticks.iter().map(|batch| batch.events.len()).sum::<usize>(),
            input_event_count
        );
        assert!(ticks.get(2).is_some_and(|batch| !batch.events.is_empty()));
        assert!(ticks
            .last()
            .is_some_and(|batch| { batch.receipt_ns == window.end_ns && batch.events.len() == 1 }));
    }

    #[test]
    fn source_hour_maps_to_right_closed_ceiling_cutoffs() {
        let start = 1_800_000_000_000_000_000i64;
        let window =
            RecordedHourWindow::new(start, start + RECORDED_HOUR_NS).expect("valid hour window");
        let second = crate::surveillance::SECOND_NS;
        assert!(!recorded_hour_cutoff_admitted(start, window));
        assert!(recorded_hour_cutoff_admitted(start + second, window));
        assert!(recorded_hour_cutoff_admitted(window.end_ns, window));
        assert!(!recorded_hour_cutoff_admitted(
            window.end_ns + second,
            window
        ));
    }

    #[test]
    fn recorded_window_requires_whole_seconds_and_at_most_one_hour() {
        let start = 1_800_000_000_000_000_000i64;
        assert_eq!(
            regional_cli_runtime_profile(RegionalCliProfile::Default).max_events_per_receipt,
            MAX_EVENTS_PER_RECEIPT
        );
        assert_eq!(
            regional_cli_runtime_profile(RegionalCliProfile::RecordedPeakRendered)
                .max_events_per_receipt,
            MAX_EVENTS_PER_RECEIPT
        );
        assert_eq!(
            regional_cli_runtime_profile(RegionalCliProfile::RecordedHourRendered)
                .max_events_per_receipt,
            RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
        );
        assert_eq!(
            regional_cli_runtime_profile(RegionalCliProfile::RecordedHourRendered)
                .surveillance
                .tracker_timing,
            crate::surveillance::TrackerTimingProfile::ArchivedFixed10_30_120V1
        );
        assert_eq!(
            regional_cli_runtime_profile(RegionalCliProfile::Default)
                .surveillance
                .tracker_timing,
            crate::surveillance::TrackerTimingProfile::DefaultV1
        );
        assert_eq!(
            regional_cli_runtime_profile(RegionalCliProfile::RecordedPeakRendered)
                .surveillance
                .tracker_timing,
            crate::surveillance::TrackerTimingProfile::DefaultV1
        );
        let no_warmup =
            RecordedHourWindow::new(start, start + RECORDED_HOUR_NS).expect("valid hour window");
        assert!(no_warmup.warmup_start_ns.is_none());
        assert_eq!(no_warmup.replay_frame_count(), no_warmup.frame_count());
        let short = RecordedHourWindow::new(start, start + 60 * crate::surveillance::SECOND_NS)
            .expect("short render window for bounded inspection");
        assert_eq!(short.frame_count(), 60);
        assert!(RecordedHourWindow::new(start, start + RECORDED_HOUR_NS - 1).is_err());
        assert!(RecordedHourWindow::new(
            start,
            start + RECORDED_HOUR_NS + crate::surveillance::SECOND_NS
        )
        .is_err());
        assert!(RecordedHourWindow::new(start + 1, start + 1 + RECORDED_HOUR_NS).is_err());
    }

    // Mutation killed: substituting a legacy hard-coded centre for model identity.
    // REQ: OA-REG-013
    #[test]
    fn recorded_hour_centre_preserves_native_artcc_and_refuses_bad_metadata() {
        let artcc = "ZAU";
        let centre = recorded_hour_centre(artcc).expect("native three-letter ARTCC");
        let prefix = centre
            .get(..artcc.len())
            .expect("validated centre prefix width");
        let padding = centre
            .get(artcc.len()..)
            .expect("validated centre padding width");
        assert_eq!(prefix, artcc.as_bytes());
        assert!(padding.iter().all(|byte| *byte == 0));
        assert!(crate::input::is_valid_centre_code(&centre));
        assert!(recorded_hour_centre("").is_err());
        assert!(recorded_hour_centre("ZA/U").is_err());
        assert!(recorded_hour_centre("ABCDE").is_err());
    }

    // Mutation killed: hard-coding the one-hour schedule ignores caller-selected preview windows.
    #[test]
    fn short_window_consumer_emits_only_its_declared_ticks() {
        let start = 1_800_000_000_000_000_000i64;
        let window = RecordedHourWindow::new(start, start + 60 * crate::surveillance::SECOND_NS)
            .expect("short whole-second preview");
        let ticks = fill_recorded_hour_ticks(Vec::new(), window).expect("empty preview ticks");
        assert_eq!(ticks.len(), window.frame_count());
        assert_eq!(
            ticks.last().map(|batch| batch.receipt_ns),
            Some(window.end_ns)
        );
    }

    // Mutation killed: writing the full RegionalFrame debug tree into each
    // hourly frame report instead of a compact, raster-bound summary.
    // REQ: OA-REG-019
    #[test]
    fn recorded_hour_report_keeps_synthetic_raster_and_aggregate_counts_compact() {
        let mut runtime = synthetic_hour_runtime();
        let receipt_ns = 1_800_000_000_000_000_000i64;
        let batch = ReceiptBatch {
            receipt_ns,
            source_clock_regressions: 0,
            events: vec![event(receipt_ns, 1)],
        };
        let frame = regional_step(&mut runtime, &batch).expect("synthetic frame");
        let pixels = regional_pixels(&runtime).expect("synthetic composed raster");
        let spec = oa_display::raster::FrameSpec {
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            stride: FRAME_WIDTH * 4,
        };
        let mut ppm = Vec::new();
        oa_display::ppm::write_ppm(&mut ppm, &spec, pixels).expect("synthetic PPM output");
        let raster_sha = oa_sha256::sha256(&ppm);
        let (row, _) = recorded_hour_frame_row(&frame, &batch, 0, 0, &raster_sha);
        let report = recorded_hour_compact_frame_report(&row).expect("compact frame evidence");
        let track_count = frame.surveillance.tracks.len().to_string();
        let correlated_count = frame
            .flights
            .iter()
            .filter(|flight| flight.view.is_some())
            .count()
            .to_string();
        let correlation = &frame.flight_correlation;

        assert!(report.len() <= MAX_FRAME_REPORT_BYTES);
        assert!(report.contains(&format!("tracks\t{track_count}\n")));
        assert!(report.contains(&format!("correlated_flights\t{correlated_count}\n")));
        for (name, count) in [
            (
                "available_flight_views_for_runtime_centre",
                correlation.available_flight_views_for_runtime_centre,
            ),
            (
                "positive_address_claims_for_runtime_centre",
                correlation.positive_address_claims_for_runtime_centre,
            ),
            (
                "matched_observed_tracks",
                correlation.matched_observed_tracks,
            ),
            (
                "unmatched_observed_tracks",
                correlation.unmatched_observed_tracks,
            ),
            (
                "ambiguous_observed_tracks",
                correlation.ambiguous_observed_tracks,
            ),
            (
                "refused_observed_track_lookups",
                correlation.refused_observed_track_lookups,
            ),
        ] {
            assert!(report.contains(&format!("{name}\t{count}\n")));
        }
        assert!(report.contains(&oa_sha256::to_hex(&raster_sha)));
        assert!(!report.contains("flight_operations"));
        assert!(!report.contains("vertical_refusal"));
        assert!(!report.contains("step_ns"));
        assert!(!report.contains("render_ns"));
        assert_eq!(
            report,
            recorded_hour_compact_frame_report(&row).expect("repeatable compact format")
        );
    }

    // Mutation killed: retaining all parsed receipt groups or shifting an
    // event across its receipt cutoff while streaming hour frames.
    // REQ: OA-REG-019
    #[test]
    fn recorded_hour_stream_matches_materialized_schedule_and_frames() {
        let start = 1_800_000_000_000_000_000i64;
        let window = RecordedHourWindow::new(start, start + 6 * crate::surveillance::SECOND_NS)
            .expect("short aligned replay");
        let events = [
            event(start + crate::surveillance::SECOND_NS, 1),
            event(start + 4 * crate::surveillance::SECOND_NS, 2),
            event(window.end_ns, 3),
        ];
        let input = crate::recordings::serialize_recorded_events(&events)
            .expect("synthetic normalized input");
        let groups = events
            .iter()
            .cloned()
            .map(|event| ReceiptBatch {
                receipt_ns: event.receipt_ns,
                source_clock_regressions: 0,
                events: vec![event],
            })
            .collect();
        let reference =
            fill_recorded_hour_ticks(groups, window).expect("materialized reference schedule");
        let mut stream = RecordedHourTickStream::new(
            &input,
            window,
            regional_cli_runtime_profile(RegionalCliProfile::RecordedHourRendered).max_total_events,
            RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
        )
        .expect("stream cursor");
        let mut streamed = Vec::new();
        while let Some(batch) = stream.next_batch().expect("stream next frame") {
            streamed.push(batch);
        }
        assert_eq!(streamed, reference);
        assert!(streamed.iter().any(|batch| batch.events.is_empty()));

        let mut expected_runtime = synthetic_hour_runtime();
        let mut streamed_runtime = synthetic_hour_runtime();
        let expected_frames = reference
            .iter()
            .map(|batch| regional_step(&mut expected_runtime, batch).expect("reference frame"))
            .collect::<Vec<_>>();
        let streamed_frames = streamed
            .iter()
            .map(|batch| regional_step(&mut streamed_runtime, batch).expect("streamed frame"))
            .collect::<Vec<_>>();
        assert_eq!(streamed_frames, expected_frames);
    }

    // Mutation killed: skipping warmup batches before flight state and the
    // later display join are processed, or exporting warmup ticks as display frames.
    // REQ: OA-REG-019
    #[test]
    fn explicit_warmup_plan_survives_to_display_join_without_exporting_warmup_ticks() {
        let display_start = 1_800_000_000_000_000_000i64;
        let second = crate::surveillance::SECOND_NS;
        let warmup_start = display_start - 2 * second;
        let window = RecordedHourWindow::new_with_warmup(
            display_start,
            display_start + 3 * second,
            Some(warmup_start),
        )
        .expect("synthetic warmup interval");
        let gufi = 0x123_u128;
        let address = 0xabc123;
        let plan_receipt = display_start - second;
        let plan_events = [
            NormalizedEvent {
                receipt_ns: plan_receipt,
                source_ns: plan_receipt,
                sequence: 1,
                source_message_id: Some(1),
                source: SourceKind::Flight,
                identity: Identity {
                    flight_gufi: gufi,
                    aircraft_address: address,
                },
                centre: *b"ZAU1",
                kind: EventKind::Flight(EventFlightOperation::SetPlan {
                    plan_id: *b"PLAN00000001",
                    status: oa_flight::ST_ACTIVE,
                }),
            },
            NormalizedEvent {
                receipt_ns: plan_receipt,
                source_ns: plan_receipt,
                sequence: 2,
                source_message_id: Some(1),
                source: SourceKind::Flight,
                identity: Identity {
                    flight_gufi: gufi,
                    aircraft_address: address,
                },
                centre: *b"ZAU1",
                kind: EventKind::Flight(EventFlightOperation::SetAddress),
            },
            NormalizedEvent {
                identity: Identity {
                    flight_gufi: 0,
                    aircraft_address: address,
                },
                ..event(display_start + second, 1)
            },
            NormalizedEvent {
                identity: Identity {
                    flight_gufi: 0,
                    aircraft_address: address,
                },
                ..event(display_start + 2 * second, 2)
            },
            NormalizedEvent {
                identity: Identity {
                    flight_gufi: 0,
                    aircraft_address: address,
                },
                ..event(display_start + 3 * second, 3)
            },
        ];
        let input = crate::recordings::serialize_recorded_events(&plan_events)
            .expect("synthetic warmup and display events");
        let counts = validate_recorded_hour_input(
            &input,
            window,
            regional_cli_runtime_profile(RegionalCliProfile::RecordedHourRendered).max_total_events,
            RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
        )
        .expect("full warmup and display preflight");
        let expected_warmup_events = plan_events
            .iter()
            .filter(|event| event.receipt_ns <= display_start)
            .count() as u64;
        let expected_display_events = plan_events
            .iter()
            .filter(|event| event.receipt_ns > display_start)
            .count() as u64;
        assert_eq!(counts.warmup_events, expected_warmup_events);
        assert_eq!(counts.display_events, expected_display_events);
        let settings = recorded_hour_window_settings(window, counts);
        assert!(settings.contains(&format!(
            "warmup_receipt_start_ns_exclusive\t{warmup_start}\n"
        )));
        assert!(settings.contains(&format!(
            "warmup_admitted_events\t{}\n",
            counts.warmup_events
        )));
        assert!(settings.contains(&format!(
            "display_admitted_events\t{}\n",
            counts.display_events
        )));

        let mut runtime = synthetic_warmup_runtime(window);
        let mut stream = RecordedHourTickStream::new(
            &input,
            window,
            DEFAULT_REGIONAL_RUN_PROFILE.max_total_events,
            RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
        )
        .expect("bounded warmup stream");
        let mut warmup_frames = 0usize;
        let mut display_frames = Vec::new();
        while let Some(batch) = stream.next_batch().expect("stream next frame") {
            let frame = regional_step(&mut runtime, &batch).expect("runtime warmup/display step");
            if recorded_hour_batch_is_display(window, &batch) {
                display_frames.push(frame);
            } else {
                warmup_frames = warmup_frames
                    .checked_add(1)
                    .expect("bounded warmup tick count");
            }
        }
        assert_eq!(warmup_frames, window.warmup_frame_count());
        assert_eq!(display_frames.len(), window.frame_count());
        assert_eq!(
            display_frames.len(),
            ((window.end_ns - window.start_ns) / second) as usize
        );
        assert!(display_frames
            .iter()
            .all(|frame| { frame.sequence > window.warmup_frame_count() as u64 }));
        assert!(display_frames
            .iter()
            .any(|frame| { frame.flight_correlation.matched_observed_tracks > 0 }));
    }

    // Mutation killed: trusting a caller-mutated warmup window at the public boundary.
    // REQ: OA-REG-019
    #[test]
    fn session_consumer_refuses_mutated_warmup_before_writing_output() {
        let start = 1_800_000_000_000_000_000i64;
        let mut window = RecordedHourWindow::new(start, start + crate::surveillance::SECOND_NS)
            .expect("valid initial display window");
        window.warmup_start_ns = Some(start - 601 * crate::surveillance::SECOND_NS);
        let output = std::env::temp_dir().join(format!(
            "oa-regional-invalid-window-{}-{}",
            std::process::id(),
            WARMUP_OUTPUT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let error = run_session_with_options_and_quarantine_release(
            b"OA-RS\t1\n",
            &output,
            &ModelSource::Synthetic,
            &MsawSource::Disabled,
            None,
            RegionalSessionOptions {
                profile: RegionalCliProfile::RecordedHourRendered,
                recorded_hour: Some(window),
            },
            None,
        )
        .expect_err("a modified window must be revalidated before any output");
        assert_eq!(
            error,
            "recorded-hour warmup must be whole-second and within ten minutes"
        );
        assert!(!output.exists());
    }

    // Mutation killed: rejecting the warmup frame capacity at runtime startup
    // or publishing warmup ticks as recorded-hour output.
    // REQ: OA-REG-019
    #[test]
    fn warmup_run_session_starts_with_expanded_capacity_and_exports_display_window_only() {
        let display_start = 1_800_000_000_000_000_000i64;
        let second = crate::surveillance::SECOND_NS;
        let warmup_start = display_start - 2 * second;
        let window = RecordedHourWindow::new_with_warmup(
            display_start,
            display_start + 3 * second,
            Some(warmup_start),
        )
        .expect("synthetic warmup interval");
        let gufi = 0x456_u128;
        let address = 0xdef456;
        let plan_receipt = display_start - second;
        let events = [
            NormalizedEvent {
                receipt_ns: plan_receipt,
                source_ns: plan_receipt,
                sequence: 1,
                source_message_id: Some(1),
                source: SourceKind::Flight,
                identity: Identity {
                    flight_gufi: gufi,
                    aircraft_address: address,
                },
                centre: *b"ZAU1",
                kind: EventKind::Flight(EventFlightOperation::SetPlan {
                    plan_id: *b"PLAN00000002",
                    status: oa_flight::ST_ACTIVE,
                }),
            },
            NormalizedEvent {
                receipt_ns: plan_receipt,
                source_ns: plan_receipt,
                sequence: 2,
                source_message_id: Some(1),
                source: SourceKind::Flight,
                identity: Identity {
                    flight_gufi: gufi,
                    aircraft_address: address,
                },
                centre: *b"ZAU1",
                kind: EventKind::Flight(EventFlightOperation::SetAddress),
            },
            NormalizedEvent {
                identity: Identity {
                    flight_gufi: 0,
                    aircraft_address: address,
                },
                ..event(display_start + second, 1)
            },
            NormalizedEvent {
                identity: Identity {
                    flight_gufi: 0,
                    aircraft_address: address,
                },
                ..event(display_start + 2 * second, 2)
            },
            NormalizedEvent {
                identity: Identity {
                    flight_gufi: 0,
                    aircraft_address: address,
                },
                ..event(display_start + 3 * second, 3)
            },
        ];
        let input = crate::recordings::serialize_recorded_events(&events)
            .expect("synthetic normalized warmup input");
        let output = std::env::temp_dir().join(format!(
            "oa-regional-warmup-output-{}-{}",
            std::process::id(),
            WARMUP_OUTPUT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&output);

        run_session_with_options_and_quarantine_release(
            input.as_bytes(),
            &output,
            &ModelSource::Synthetic,
            &MsawSource::Disabled,
            None,
            RegionalSessionOptions {
                profile: RegionalCliProfile::RecordedHourRendered,
                recorded_hour: Some(window),
            },
            None,
        )
        .expect("recorded-hour consumer accepts bounded warmup capacity");

        let index = std::fs::read_to_string(output.join("frame-index.tsv"))
            .expect("published display frame index");
        assert_eq!(index.lines().skip(2).count(), window.frame_count());
        for sequence in 1..=window.warmup_frame_count() {
            for extension in ["ppm", "txt"] {
                let name = format!("frame-{sequence:04}.{extension}");
                assert!(!output.join(name).exists());
            }
        }
        let settings = std::fs::read_to_string(output.join("settings.tsv"))
            .expect("published replay window settings");
        let warmup_events = events
            .iter()
            .filter(|event| event.receipt_ns <= window.start_ns)
            .count();
        let display_events = events
            .iter()
            .filter(|event| event.receipt_ns > window.start_ns)
            .count();
        assert!(settings.contains(&format!("warmup_ticks\t{}\n", window.warmup_frame_count())));
        assert!(settings.contains(&format!("display_ticks\t{}\n", window.frame_count())));
        assert!(settings.contains(&format!("warmup_admitted_events\t{warmup_events}\n")));
        assert!(settings.contains(&format!("display_admitted_events\t{display_events}\n")));
        assert!(settings.contains(&format!(
            "tracker_expected_interval_ns\t{}\n",
            crate::surveillance::RECORDED_HOUR_EXPECTED_INTERVAL_NS
        )));
        assert!(settings.contains(&format!(
            "tracker_init_interval_ns\t{}\n",
            crate::surveillance::RECORDED_HOUR_INIT_INTERVAL_NS
        )));
        assert!(settings.contains(&format!(
            "tracker_drop_after_ns\t{}\n",
            crate::surveillance::RECORDED_HOUR_DROP_AFTER_NS
        )));
        std::fs::remove_dir_all(output).expect("remove synthetic output tree");
    }
}

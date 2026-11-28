//! Offline regional surveillance composition (**shell**, B1/B3/C6). Real tracker output at
//! the issue tick feeds both conflict alert and scope; intended routes never suppress CA.

use crate::input::{EventKind, NormalizedEvent, SourceKind};
use oa_airspace::core::model::Model;
use oa_ca::{
    alert_log_clear, alert_log_new, ca_cycle_with_inhibits, ca_new, ca_open, event_log_clear,
    event_log_new, Area, CaConfig, CaTrack, Volume, AIRBORNE, F_COAST, F_IDENTITY_UNCONFIRMED,
    F_UNCERTAIN, MAX_VOLUMES,
};
use oa_display::core::font::{self, TextBounds};
use oa_display::core::list::DisplayList;
use oa_display::core::ownership::OwnershipStatus;
use oa_display::core::project::{project, Projection};
use oa_display::core::raster::frame_digest;
use oa_display::core::safety::{FeedState, FeedStatus, SafetyStats};
use oa_display::core::scene::LAYER_TEXT;
use oa_display::core::traffic::{targets_columns, DEFAULT_STYLE, LAYER_BLOCK};
use oa_safety::inhibit::{inhibits_new, InhibitTable};
use oa_safety::net::{state_table_new, StateTable};
use oa_scope_replay::adapt::{row_columns, ColumnBuf};
use oa_scope_replay::frame::{
    replay_build_with_overlays, replay_new_with_top_inset, replay_raster, Replay, ReplayConfig,
    ReplaySafety,
};
use oa_scope_replay::rows::{Callsigns, RowStore, DEFAULT_CONVERT};
use oa_scope_replay::store::{store_add, store_finish};
use oa_track::{
    output_clear, output_new, r95_m_of, tracker_advance, tracker_config, tracker_counts,
    tracker_live_iter, tracker_new, tracker_newest_update_ns, tracker_snapshot_at, tracker_stats,
    tracker_update, Measurement, OutRow, TrackOutput, Tracker, TrackerStats, DEFAULT_CONFIG,
    M_HAS_VEL, NONE_I32, NO_IDENT, O_COAST, O_HAS_ALT, O_IDENTITY_UNCONFIRMED, O_UNCERTAIN,
};
use std::collections::BTreeMap;

pub const SECOND_NS: i64 = 1_000_000_000;
/// Research envelope: 32 simultaneous aircraft and 512 frames, rather than a facility claim.
pub const MAX_TRACKS: usize = 32;
pub const MAX_FRAMES: usize = 512;
/// Hard offline diagnostic limit; unlike the default profile this counter does not retain frames.
pub const MAX_OFFLINE_FRAMES: usize = 20_000_000;
pub const MAX_OFFLINE_BARO_EVIDENCE: usize = 1_000_000;
pub const MAX_OFFLINE_CA_WORK_PER_FRAME: u64 = 8_388_608;
/// A 60-second gap emits at most 32 * 60 ticks plus 256 group updates, under 4096 rows.
pub const MAX_FRAME_GAP_NS: i64 = 60 * SECOND_NS;
pub const MAX_DRAIN_ROWS: usize = 4096;
/// Recorded-hour engineering timing values, mirrored from
/// `oa-track-tools/src/score.rs::SCORING_FIXED_10_30_120_*` (profile V1).
pub const RECORDED_HOUR_EXPECTED_INTERVAL_NS: i64 = 10 * SECOND_NS;
pub const RECORDED_HOUR_INIT_INTERVAL_NS: i64 = 30 * SECOND_NS;
pub const RECORDED_HOUR_DROP_AFTER_NS: i64 = 120 * SECOND_NS;
pub const FRAME_WIDTH: u32 = 800;
pub const FRAME_HEIGHT: u32 = 600;
pub const REVIEW_FRAME_WIDTH: u32 = 1_600;
pub const REVIEW_FRAME_HEIGHT: u32 = 1_200;
pub const REGIONAL_STATUS_HEADER_INSET_PX: u32 = 184;
const REGIONAL_MODE_HEADER_Y_PX: i32 = 106;
/// C7: control redraws over the entire offline session, including ordinary completed frames.
pub const MAX_PRESENTATION_UPDATES: usize = 8192;
const MAX_PAIRS: usize = MAX_TRACKS * (MAX_TRACKS - 1) / 2;
const CA_CONNECTOR_LAYER: u8 = 12;
const CA_LABEL_LAYER: u8 = 15;
const CA_LABEL_SIZE: u8 = 10;
// C2/C7: at most max_open=MAX_PAIRS episodes can close in one cycle, and each retained
// track can lose altitude once. Both logs are captured in frame evidence and drained every
// cycle; storage therefore depends on simultaneous traffic, never the session's frame count.
const MAX_CLOSED_ALERTS_PER_CYCLE: usize = MAX_PAIRS;
// C2/C6: at most one new address per admitted event, bounded across the offline session.
// Keeping the latest accepted barometric source time also preserves tentative-track evidence.

/// Validated per-run capacities for an offline replay. Existing constructors continue to use
/// [`DEFAULT_SURVEILLANCE_PROFILE`]; the recorded profile is separately selected and bounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurveillanceProfile {
    pub max_tracks: usize,
    pub max_frames: usize,
    pub max_drain_rows: usize,
    pub max_baro_evidence: usize,
    pub max_open_alerts: usize,
    pub max_candidate_pairs: usize,
    pub max_ca_work_per_frame: u64,
    pub max_presentation_updates: usize,
    pub render_scope: bool,
    pub tracker_timing: TrackerTimingProfile,
    pub frame_width: u32,
    pub frame_height: u32,
    /// Existing map layer switches supplied to the scope scene.
    pub map_layers: oa_display::core::scene::Layers,
    /// Restrict airport points to those with a coded procedure; non-IFR airports are otherwise
    /// shown only within the view's existing non-IFR distance threshold.
    pub ifr_airports_only: bool,
    /// Let the explicitly detailed review use fixes up to its current view range.
    pub fixes_to_view_range: bool,
}

/// Explicit timing configuration for report confirmation and track retention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackerTimingProfile {
    /// Historical one-second cadence used by live-style and recorded-peak profiles.
    DefaultV1,
    /// Fixed 10/30/120-second engineering profile for sparse archived reports.
    ArchivedFixed10_30_120V1,
}

pub const DEFAULT_SURVEILLANCE_PROFILE: SurveillanceProfile = SurveillanceProfile {
    max_tracks: MAX_TRACKS,
    max_frames: MAX_FRAMES,
    max_drain_rows: MAX_DRAIN_ROWS,
    max_baro_evidence: crate::session::MAX_SESSION_EVENTS,
    max_open_alerts: MAX_CLOSED_ALERTS_PER_CYCLE,
    max_candidate_pairs: MAX_PAIRS,
    max_ca_work_per_frame: 1_000_000,
    max_presentation_updates: MAX_PRESENTATION_UPDATES,
    render_scope: true,
    tracker_timing: TrackerTimingProfile::DefaultV1,
    frame_width: FRAME_WIDTH,
    frame_height: FRAME_HEIGHT,
    map_layers: oa_display::core::scene::layers_all(),
    ifr_airports_only: false,
    fixes_to_view_range: false,
};
const FEED_NAMES: [[u8; 8]; 5] = [
    *b"TRACK\0\0\0",
    *b"FLIGHT\0\0",
    *b"ROUTE\0\0\0",
    *b"WEATHER\0",
    *b"ADAPT\0\0\0",
];

/// A3 exception: at most 32 offline output records; every consumer reads the whole record.
/// Status flags are in a separate column (A6); every value here is fixed point (A9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ObservedTrack {
    pub state_ns: i64,
    pub newest_source_ns: i64,
    pub address: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub altitude_ft: i32,
    pub vertical_rate_fpm: i32,
    pub east_mmps: i32,
    pub north_mmps: i32,
    pub r95_mm: u32,
}
const _: () = assert!(core::mem::size_of::<ObservedTrack>() == 48);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurveillanceConfig {
    pub centre_lat_e7: i32,
    pub centre_lon_e7: i32,
    pub range_nm: f64,
}
// A7.2: cold startup configuration.

pub struct Surveillance {
    tracker: Tracker,
    drain: TrackOutput,
    frame_snapshot: TrackOutput,
    scope: Replay,
    ca: oa_ca::CaState,
    alerts: oa_ca::AlertLog,
    events: oa_ca::EventLog,
    states: StateTable,
    inhibits: Box<InhibitTable>,
    seen: [Option<i64>; 5],
    down: [bool; 5],
    track_down: bool,
    track_measurement_seen_ns: Option<i64>,
    measurement_rejections: u64,
    previous_ns: Option<i64>,
    frames: usize,
    completed: bool,
    failed: bool,
    overlay_base: Option<OverlayBase>,
    presentation_updates: usize,
    msaw: Option<crate::msaw_bridge::RegionalMsaw>,
    baro_seen: BTreeMap<u32, i64>,
    track_callsigns: BTreeMap<u32, TrackCallsignEvidence>,
    profile: SurveillanceProfile,
    tracker_drop_after_ns: i64,
}

/// Callsign carried by an accepted position observation. It may survive coast frames only while
/// the live tracker emits the same address/source epoch within its configured drop interval.
/// This is observation-level corroboration data, never an authentication claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrackCallsignEvidence {
    pub callsign: [u8; 8],
    pub source_ns: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurveillanceStageError {
    FailedSession,
    SealedTimestamp,
    WrongReceiptOrTimestamp,
    WrongSourceKind,
    InvalidPosition,
    BarometricEvidenceCapacity,
}

impl SurveillanceStageError {
    pub fn message(self) -> &'static str {
        match self {
            Self::FailedSession => {
                "surveillance failed; reconstruct from the admitted input prefix"
            }
            Self::SealedTimestamp => "sealed source or receipt timestamp refused",
            Self::WrongReceiptOrTimestamp => "event does not belong to the current receipt cutoff",
            Self::WrongSourceKind => "event kind/source category mismatch",
            Self::InvalidPosition => "position fields are outside the supported input bounds",
            Self::BarometricEvidenceCapacity => "barometric evidence capacity exhausted",
        }
    }
}

fn valid_track_callsign(callsign: [u8; 8]) -> bool {
    if !callsign.is_ascii() {
        return false;
    }
    let end = callsign
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(callsign.len());
    end > 0
        && callsign
            .get(..end)
            .is_some_and(|value| value.iter().all(u8::is_ascii_alphanumeric))
        && callsign
            .get(end..)
            .is_some_and(|tail| tail.iter().all(|byte| *byte == 0))
}
// A7.2: cold subsystem container; core tables allocate once in their constructors.

// A7.2: cold lengths in the current owned display list, never exported across a module.
#[derive(Clone, Copy)]
struct OverlayBase {
    texts: usize,
    bytes: usize,
    layers: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurveillanceFrame {
    pub time_ns: i64,
    /// False in the bounded analysis-only profile; `frame_sha256` then hashes tracker and alert
    /// state projection bytes, not a raster image.
    pub raster_rendered: bool,
    /// Aggregate traffic-detail policy actually applied to this frame's raster, when rendered.
    pub traffic_stats: Option<oa_display::core::traffic::TrafficStats>,
    pub tracks: Vec<ObservedTrack>,
    pub flags: Vec<u8>,
    pub frame_sha256: [u8; 32],
    pub ca_open: Vec<oa_ca::Alert>,
    /// OA-CA-035: current CA pair connectors submitted to the scope renderer.
    pub ca_overlay: Option<CaOverlayStats>,
    pub ca_closed: Vec<oa_ca::Alert>,
    pub ca_altitude_lost: Vec<oa_ca::AltLost>,
    /// Per-alert acknowledgement/audit rows retained from the inhibition subsystem.
    pub ca_suppressed: Vec<oa_safety::inhibit::Suppressed>,
    pub ca_suppressed_overflow: u64,
    pub msaw: Option<crate::msaw_bridge::RegionalMsawFrame>,
    pub msaw_markers: Option<oa_display::core::alert_markers::MarkerStats>,
    pub msaw_altitude_source_unknown: u32,
    pub ca_stats: oa_ca::CycleStats,
    pub ca_work_short: u64,
    pub ca_input_short: u64,
    pub tracker_stats: TrackerStats,
    pub track_transport_seen_ns: Option<i64>,
    pub track_measurement_seen_ns: Option<i64>,
    /// Source health shared by the display and regional automation mode selector.
    pub feeds: Vec<FeedStatus>,
    /// Safety-layer draw accounting when a scope frame was rendered; absent in analysis-only mode.
    pub safety_stats: Option<SafetyStats>,
    /// Product-specific status. A retained empty/None value is never a healthy no-alert result
    /// when the corresponding product was withheld by the selected/effective mode.
    pub track_product: ProductAvailability,
    pub ca_product: ProductAvailability,
    pub msaw_product: ProductAvailability,
    /// True when the selected/effective regional mode intentionally withheld surveillance products.
    pub surveillance_withheld: bool,
    pub withheld_tracks: u32,
    pub withheld_alerts: u32,
    pub withheld_ca_records: u32,
    pub withheld_msaw_records: u32,
    pub measurement_rejections: u64,
    pub duplicate_rows_replaced: u32,
    pub displayed: u32,
    pub off_map: u32,
    pub warnings: Vec<String>,
}

/// OA-CA-035 display accounting. No urgency tier is inferred from prediction values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaOverlayStats {
    pub pairs_examined: u32,
    pub pairs_drawn: u32,
    pub pairs_offscreen: u32,
    pub pairs_unlocated: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductUnavailableReason {
    OperatorSelectedFlightDataOnly,
    SurveillanceFeedUnavailable,
    FusionUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductAvailability {
    Available,
    NotConfigured,
    Withheld(ProductUnavailableReason),
}
// A7.2: cold owned frame evidence, retained at most MAX_FRAMES times.

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}

fn prune_track_callsigns(s: &mut Surveillance, tracks: &[ObservedTrack], now_ns: i64) {
    let max_age_ns = s.tracker_drop_after_ns;
    s.track_callsigns.retain(|address, evidence| {
        let Ok(index) = tracks.binary_search_by_key(address, |track| track.address) else {
            return false;
        };
        let Some(track) = tracks.get(index) else {
            return false;
        };
        evidence.source_ns == track.newest_source_ns
            && now_ns
                .checked_sub(evidence.source_ns)
                .is_some_and(|age| age >= 0 && age <= max_age_ns)
    });
}

pub fn surveillance_new(model: Model, cfg: SurveillanceConfig) -> Result<Surveillance, String> {
    surveillance_new_with_profile(model, cfg, DEFAULT_SURVEILLANCE_PROFILE)
}

/// Starts a replay with a fixed-capacity offline profile. Existing constructors keep the
/// historical small limits; this profile allocates live tables once and retains no frame history.
pub fn surveillance_new_with_profile(
    model: Model,
    cfg: SurveillanceConfig,
    profile: SurveillanceProfile,
) -> Result<Surveillance, String> {
    let frame_stride = profile
        .frame_width
        .checked_mul(4)
        .ok_or("surveillance frame stride overflow")?;
    if profile.frame_width == 0 || profile.frame_height == 0 {
        return Err("offline surveillance frame dimensions must be positive".into());
    }
    let frame_spec = oa_display::raster::FrameSpec {
        width: profile.frame_width,
        height: profile.frame_height,
        stride: frame_stride,
    };
    oa_display::raster::check_frame(&frame_spec, oa_display::raster::frame_len(&frame_spec))
        .map_err(err)?;
    let maximum_pairs = profile
        .max_tracks
        .checked_mul(profile.max_tracks.saturating_sub(1))
        .and_then(|pairs| pairs.checked_div(2))
        .ok_or("track pair capacity overflow")?;
    if profile.max_tracks == 0
        || profile.max_tracks > 4096
        || profile.max_frames == 0
        || profile.max_frames > MAX_OFFLINE_FRAMES
        || profile.max_drain_rows < profile.max_tracks
        || profile.max_drain_rows > 262_144
        || profile.max_baro_evidence == 0
        || profile.max_baro_evidence > MAX_OFFLINE_BARO_EVIDENCE
        || profile.max_open_alerts == 0
        || profile.max_open_alerts > (1 << 20)
        || profile.max_candidate_pairs == 0
        || profile.max_candidate_pairs > (1 << 24)
        || profile.max_candidate_pairs < maximum_pairs
        || profile.max_ca_work_per_frame == 0
        || profile.max_ca_work_per_frame > MAX_OFFLINE_CA_WORK_PER_FRAME
        || profile.max_presentation_updates > MAX_PRESENTATION_UPDATES
        || (profile.render_scope && profile.max_presentation_updates == 0)
    {
        return Err("offline surveillance profile is outside its bounded capacity contract".into());
    }
    let mut tracker_config = oa_track::TrackerConfig {
        max_tracks: profile.max_tracks,
        out_capacity: profile.max_drain_rows,
        log_capacity: profile.max_drain_rows,
        confirm_pool: profile.max_tracks,
        ..DEFAULT_CONFIG
    };
    if profile.tracker_timing == TrackerTimingProfile::ArchivedFixed10_30_120V1 {
        tracker_config.expected_interval_ns = RECORDED_HOUR_EXPECTED_INTERVAL_NS;
        tracker_config.init_interval_ns = RECORDED_HOUR_INIT_INTERVAL_NS;
        tracker_config.drop_after_ns = RECORDED_HOUR_DROP_AFTER_NS;
    }
    let tracker = tracker_new(&tracker_config).map_err(err)?;
    let mut scope = replay_new_with_top_inset(
        model,
        &ReplayConfig {
            centre_lat_e7: cfg.centre_lat_e7,
            centre_lon_e7: cfg.centre_lon_e7,
            range_nm: cfg.range_nm,
            width: profile.frame_width,
            height: profile.frame_height,
            max_targets: profile.max_tracks,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
        REGIONAL_STATUS_HEADER_INSET_PX,
    )
    .map_err(err)?;
    scope.view.layers = profile.map_layers;
    if profile.ifr_airports_only {
        scope.view.non_ifr_airport_max_nm = 0.0;
        scope.view.runway_max_nm = 0.0;
    }
    if profile.fixes_to_view_range {
        scope.view.fix_max_nm = scope.view.proj.range_nm;
    }
    let interaction_caps = oa_display::list::ListCaps {
        // One current conflict connector and one label per retained active alert.
        lines: 1 + profile.max_open_alerts,
        rects: 1,
        circles: profile.max_tracks * oa_display::core::alert_markers::ALERT_CIRCLES_PER_MARKER,
        polylines: 0,
        poly_vertices: 0,
        texts: 4
            + profile.max_tracks * oa_display::core::alert_markers::ALERT_TEXTS_PER_MARKER
            + oa_display::core::alert_markers::ALERT_HEADER_TEXTS
            + profile.max_open_alerts,
        text_bytes: 256
            + profile.max_tracks * oa_display::core::alert_markers::ALERT_TEXT_BYTES_PER_MARKER
            + oa_display::core::alert_markers::ALERT_HEADER_TEXT_BYTES
            + profile.max_open_alerts * 2,
    };
    let caps = oa_display::traffic::caps_add(&scope.list.caps, &interaction_caps).map_err(err)?;
    scope.list = oa_display::list::list_new(caps).map_err(err)?;
    // Synthetic development settings, retained in the run manifest. They are not FAA minima.
    let volume = Volume {
        area: Area::Box {
            lat_min_e7: -900_000_000,
            lat_max_e7: 900_000_000,
            lon_min_e7: -1_800_000_000,
            lon_max_e7: 1_800_000_000,
        },
        alt_lo_ft: -2000,
        alt_hi_ft: 60_000,
        h_m: 1300.0,
        v_ft: 300.0,
    };
    let ca = ca_new(&CaConfig {
        max_tracks: profile.max_tracks,
        max_open: profile.max_open_alerts,
        max_candidates: profile.max_candidate_pairs,
        look_ahead_ns: 120 * SECOND_NS,
        hysteresis_ns: 3 * SECOND_NS,
        work_budget: profile.max_ca_work_per_frame,
        eval_budget: profile.max_candidate_pairs,
        max_speed_mps: 1000.0,
        r95_cap_m: 20_000.0,
        grid_band_m: 10_000.0,
        default_volume: volume,
        volumes: [volume; MAX_VOLUMES],
        n_volumes: 0,
    })
    .map_err(err)?;
    Ok(Surveillance {
        tracker,
        drain: output_new(profile.max_drain_rows),
        frame_snapshot: output_new(profile.max_tracks),
        scope,
        ca,
        alerts: alert_log_new(profile.max_open_alerts).map_err(err)?,
        events: event_log_new(profile.max_tracks).map_err(err)?,
        states: state_table_new(10 * SECOND_NS, 0).map_err(err)?,
        inhibits: Box::new(inhibits_new()),
        seen: [None; 5],
        down: [false; 5],
        track_down: false,
        msaw: None,
        baro_seen: BTreeMap::new(),
        track_callsigns: BTreeMap::new(),
        track_measurement_seen_ns: None,
        measurement_rejections: 0,
        profile,
        tracker_drop_after_ns: tracker_config.drop_after_ns,
        previous_ns: None,
        frames: 0,
        completed: true,
        failed: false,
        overlay_base: None,
        presentation_updates: 0,
    })
}

pub(crate) fn tracker_drop_after_ns(s: &Surveillance) -> i64 {
    s.tracker_drop_after_ns
}

pub fn surveillance_enable_msaw(
    s: &mut Surveillance,
    service: crate::msaw_bridge::RegionalMsaw,
) -> Result<(), String> {
    if s.frames != 0 || s.previous_ns.is_some() || s.msaw.is_some() {
        return Err("MSAW must be configured exactly once before the first frame".into());
    }
    let (stale_ns, future_ns) = crate::msaw_bridge::regional_msaw_net_policy(&service);
    s.states = state_table_new(stale_ns, future_ns).map_err(err)?;
    s.msaw = Some(service);
    Ok(())
}

pub fn source_index(source: SourceKind) -> usize {
    match source {
        SourceKind::Surveillance => 0,
        SourceKind::Flight => 1,
        SourceKind::Route => 2,
        SourceKind::Weather => 3,
        SourceKind::Adaptation => 4,
    }
}

pub fn surveillance_begin(s: &mut Surveillance, time_ns: i64) -> Result<(), String> {
    surveillance_begin_with_resolution(s, time_ns, false)
}

/// Configured monitor path accepts exact millisecond scan times, which the downstream flight
/// snapshot API can represent. Existing research replay keeps its whole-second receipt contract.
pub fn surveillance_begin_for_monitor(s: &mut Surveillance, time_ns: i64) -> Result<(), String> {
    surveillance_begin_with_resolution(s, time_ns, true)
}

fn surveillance_begin_with_resolution(
    s: &mut Surveillance,
    time_ns: i64,
    allow_millisecond_scan: bool,
) -> Result<(), String> {
    if s.failed {
        return Err("surveillance failed; reconstruct from the admitted input prefix".into());
    }
    if !s.completed {
        return Err("previous frame has not been completed".into());
    }
    if time_ns <= 0 {
        return Err("frame receipt must be positive".into());
    }
    if (!allow_millisecond_scan && time_ns % SECOND_NS != 0)
        || (allow_millisecond_scan && time_ns % 1_000_000 != 0)
    {
        return Err("frame receipt does not match the selected time resolution".into());
    }
    if oa_sealed::in_sealed_window(time_ns) {
        return Err("sealed frame receipt timestamp refused".into());
    }
    if s.frames >= s.profile.max_frames {
        return Err("regional frame capacity exhausted before application".into());
    }
    if let Some(previous) = s.previous_ns {
        let gap = time_ns
            .checked_sub(previous)
            .ok_or("frame time difference overflow")?;
        if gap <= 0 || gap > MAX_FRAME_GAP_NS {
            return Err("frame gap must be positive and at most 60 seconds".into());
        }
    }
    output_clear(&mut s.drain);
    s.previous_ns = Some(time_ns);
    s.frames += 1; // bounded above before the operation
    s.completed = false;
    s.overlay_base = None;
    Ok(())
}

/// Read-only event checks, including the bounded evidence map, before tracker or health mutation.
pub fn surveillance_stage_event(
    s: &Surveillance,
    event: &NormalizedEvent,
) -> Result<(), SurveillanceStageError> {
    if s.failed {
        return Err(SurveillanceStageError::FailedSession);
    }
    if oa_sealed::in_sealed_window(event.source_ns) || oa_sealed::in_sealed_window(event.receipt_ns)
    {
        return Err(SurveillanceStageError::SealedTimestamp);
    }
    if s.completed
        || Some(event.receipt_ns) != s.previous_ns
        || event.receipt_ns < 0
        || event.source_ns < 0
        || event.source_ns > event.receipt_ns
    {
        return Err(SurveillanceStageError::WrongReceiptOrTimestamp);
    }
    let source_matches = match event.kind {
        EventKind::Position { .. } => event.source == SourceKind::Surveillance,
        EventKind::Flight(_) | EventKind::Operator(_) => event.source == SourceKind::Flight,
        EventKind::Route { .. } | EventKind::FiledRoute { .. } | EventKind::ActiveCursor { .. } => {
            event.source == SourceKind::Route
        }
        EventKind::FeedState { .. } => true,
    };
    if !source_matches {
        return Err(SurveillanceStageError::WrongSourceKind);
    }
    if let EventKind::Position {
        latitude_e7,
        longitude_e7,
        altitude_ft,
        track_callsign,
        ..
    } = event.kind
    {
        if event.identity.aircraft_address == 0
            || event.identity.aircraft_address > 0x00ff_ffff
            || !(-900_000_000..=900_000_000).contains(&latitude_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&longitude_e7)
            || altitude_ft.is_some_and(|alt| !(-2_000..=100_000).contains(&alt))
        {
            return Err(SurveillanceStageError::InvalidPosition);
        }
        if altitude_ft.is_some()
            && !s.baro_seen.contains_key(&event.identity.aircraft_address)
            && s.baro_seen.len() >= s.profile.max_baro_evidence
        {
            return Err(SurveillanceStageError::BarometricEvidenceCapacity);
        }
        if track_callsign.is_some_and(|callsign| !valid_track_callsign(callsign)) {
            return Err(SurveillanceStageError::InvalidPosition);
        }
    }
    Ok(())
}

pub fn surveillance_event(s: &mut Surveillance, event: &NormalizedEvent) -> Result<(), String> {
    surveillance_stage_event(s, event).map_err(|error| error.message().to_owned())?;
    let index = source_index(event.source);
    let seen = s.seen.get_mut(index).ok_or("source index")?;
    let down = s.down.get_mut(index).ok_or("source index")?;
    match event.kind {
        EventKind::FeedState { available, .. } => {
            *down = !available;
            *seen = Some(event.receipt_ns);
            if event.source == SourceKind::Surveillance {
                s.track_down = !available;
            }
        }
        EventKind::Position {
            latitude_e7,
            longitude_e7,
            altitude_ft,
            east_mmps,
            north_mmps,
            vertical_rate_fpm,
            track_callsign,
        } => {
            *seen = Some(event.receipt_ns);
            *down = false;
            let before = *tracker_stats(&s.tracker);
            tracker_update(
                &mut s.tracker,
                &Measurement {
                    toa_ns: event.source_ns,
                    pos_cov_m2: [100.0, 0.0, 100.0],
                    vel_cov_m2s2: [4.0, 0.0, 4.0],
                    vel_east_mps: east_mmps as f32 / 1000.0,
                    vel_north_mps: north_mmps as f32 / 1000.0,
                    lat_e7: latitude_e7,
                    lon_e7: longitude_e7,
                    alt_ft: altitude_ft.unwrap_or(NONE_I32),
                    vrate_fpm: vertical_rate_fpm.unwrap_or(NONE_I32),
                    addr: event.identity.aircraft_address,
                    alt_quant_ft: 25,
                    sensor: 1,
                    ident: NO_IDENT,
                    flags: M_HAS_VEL,
                    category: 0,
                    vel_lag_s: 0.0,
                },
                &mut s.drain,
            );
            let after = *tracker_stats(&s.tracker);
            // Clock epochs and filter/slot reconstruction invalidate a cached altitude's
            // identity. A source timestamp from the previous generation must not make a
            // restarted filter's older observation appear younger. Re-filtering is handled
            // conservatively: only this accepted altitude can seed new freshness evidence.
            if after.time_steps_back > before.time_steps_back {
                s.baro_seen.clear();
                s.track_callsigns.clear();
            } else if after.started > before.started
                || after.init_implausible > before.init_implausible
                || after.init_window_expired > before.init_window_expired
                || after.resyncs > before.resyncs
                || after.refiltered > before.refiltered
            {
                s.baro_seen.remove(&event.identity.aircraft_address);
                s.track_callsigns.remove(&event.identity.aircraft_address);
            }
            if measurement_rejected(before, after) {
                s.measurement_rejections = s.measurement_rejections.saturating_add(1);
            } else if after.applied > before.applied
                || after.started > before.started
                || after.refiltered > before.refiltered
            {
                // A time-gate-held report is transport evidence, not an applied observation.
                // Read actual accepted tracker time instead of assuming this offered event won.
                let accepted = tracker_newest_update_ns(&s.tracker);
                if accepted != i64::MIN {
                    s.track_measurement_seen_ns = Some(accepted);
                }
                s.track_down = false;
                if accepted == event.source_ns {
                    if let Some(callsign) = track_callsign {
                        let address = event.identity.aircraft_address;
                        let conflicts = s
                            .track_callsigns
                            .get(&address)
                            .is_some_and(|prior| prior.callsign != callsign);
                        if conflicts {
                            s.track_callsigns.remove(&address);
                        } else if s.track_callsigns.len() < s.profile.max_tracks
                            || s.track_callsigns.contains_key(&address)
                        {
                            s.track_callsigns.insert(
                                address,
                                TrackCallsignEvidence {
                                    callsign,
                                    source_ns: event.source_ns,
                                },
                            );
                        }
                    } else {
                        s.track_callsigns.remove(&event.identity.aircraft_address);
                    }
                }
                if altitude_ft.is_some() && after.alt_rejected == before.alt_rejected {
                    let address = event.identity.aircraft_address;
                    let known = s.baro_seen.entry(address).or_insert(event.source_ns);
                    if event.source_ns > *known {
                        *known = event.source_ns;
                    }
                }
            }
        }
        EventKind::Operator(_) => {}
        _ => {
            *seen = Some(event.receipt_ns);
            *down = false;
        }
    }
    Ok(())
}

fn measurement_rejected(before: TrackerStats, after: TrackerStats) -> bool {
    after.refused != before.refused
        || after.duplicates != before.duplicates
        || after.late_window != before.late_window
        || after.late_ring != before.late_ring
        || after.jump_rejected != before.jump_rejected
        || after.numerical_refused != before.numerical_refused
        || after.capacity_refused != before.capacity_refused
}

fn required<T: Copy>(v: &[T], index: usize) -> Result<T, String> {
    v.get(index)
        .copied()
        .ok_or_else(|| "inconsistent tracker output columns".into())
}

fn replace_latest_row(
    last: &mut BTreeMap<u32, usize>,
    address: u32,
    index: usize,
    max_tracks: usize,
) -> Result<bool, String> {
    let replaced = last.contains_key(&address);
    if !replaced && last.len() >= max_tracks {
        return Err("current track capacity exhausted".into());
    }
    last.insert(address, index);
    Ok(replaced)
}

fn output_row(o: &TrackOutput, i: usize) -> Result<OutRow, String> {
    Ok(OutRow {
        t_ns: required(&o.t_ns, i)?,
        addr: required(&o.addr, i)?,
        ident: required(&o.ident, i)?,
        ident_ns: required(&o.ident_ns, i)?,
        ident_conflict: required(&o.ident_conflict, i)?,
        lat_e7: required(&o.lat_e7, i)?,
        lon_e7: required(&o.lon_e7, i)?,
        vel_east_mps: required(&o.vel_east_mps, i)?,
        vel_north_mps: required(&o.vel_north_mps, i)?,
        pos_cov_m2: required(&o.pos_cov_m2, i)?,
        vel_cov_m2s2: required(&o.vel_cov_m2s2, i)?,
        turn_rate_rps: required(&o.turn_rate_rps, i)?,
        alt_ft: required(&o.alt_ft, i)?,
        alt_var_ft2: required(&o.alt_var_ft2, i)?,
        vrate_fpm: required(&o.vrate_fpm, i)?,
        mu_h: required(&o.mu_h, i)?,
        mu_v: required(&o.mu_v, i)?,
        flags: required(&o.flags, i)?,
    })
}

fn append(o: &mut TrackOutput, r: OutRow) -> Result<(), String> {
    if o.t_ns.len() >= o.capacity {
        return Err("current tracker snapshot capacity exhausted".into());
    }
    o.t_ns.push(r.t_ns);
    o.addr.push(r.addr);
    o.ident.push(r.ident);
    o.ident_ns.push(r.ident_ns);
    o.ident_conflict.push(r.ident_conflict);
    o.lat_e7.push(r.lat_e7);
    o.lon_e7.push(r.lon_e7);
    o.vel_east_mps.push(r.vel_east_mps);
    o.vel_north_mps.push(r.vel_north_mps);
    o.pos_cov_m2.push(r.pos_cov_m2);
    o.vel_cov_m2s2.push(r.vel_cov_m2s2);
    o.turn_rate_rps.push(r.turn_rate_rps);
    o.alt_ft.push(r.alt_ft);
    o.alt_var_ft2.push(r.alt_var_ft2);
    o.vrate_fpm.push(r.vrate_fpm);
    o.mu_h.push(r.mu_h);
    o.mu_v.push(r.mu_v);
    o.flags.push(r.flags);
    Ok(())
}

fn fixed_i32(value: f64) -> Result<i32, String> {
    let rounded = oa_ieee::round(value);
    if !rounded.is_finite() || rounded < f64::from(i32::MIN + 1) || rounded > f64::from(i32::MAX) {
        return Err("unrepresentable tracker evidence value".into());
    }
    Ok(rounded as i32)
}

fn capture_ca_logs(s: &mut Surveillance) -> (Vec<oa_ca::Alert>, Vec<oa_ca::AltLost>) {
    let closed = s.alerts.items.clone();
    let altitude_lost = s.events.items.clone();
    alert_log_clear(&mut s.alerts);
    event_log_clear(&mut s.events);
    (closed, altitude_lost)
}

fn offline_projection_digest(
    time_ns: i64,
    tracks: &[ObservedTrack],
    alerts: &[oa_ca::Alert],
) -> Result<[u8; 32], String> {
    let mut hash = oa_sha256::Sha256::new();
    hash.update(&time_ns.to_le_bytes()).map_err(err)?;
    for track in tracks {
        hash.update(&track.state_ns.to_le_bytes()).map_err(err)?;
        hash.update(&track.newest_source_ns.to_le_bytes())
            .map_err(err)?;
        hash.update(&track.address.to_le_bytes()).map_err(err)?;
        hash.update(&track.lat_e7.to_le_bytes()).map_err(err)?;
        hash.update(&track.lon_e7.to_le_bytes()).map_err(err)?;
        hash.update(&track.altitude_ft.to_le_bytes()).map_err(err)?;
        hash.update(&track.vertical_rate_fpm.to_le_bytes())
            .map_err(err)?;
        hash.update(&track.east_mmps.to_le_bytes()).map_err(err)?;
        hash.update(&track.north_mmps.to_le_bytes()).map_err(err)?;
        hash.update(&track.r95_mm.to_le_bytes()).map_err(err)?;
    }
    for alert in alerts {
        hash.update(&alert.addr_a.to_le_bytes()).map_err(err)?;
        hash.update(&alert.addr_b.to_le_bytes()).map_err(err)?;
        hash.update(&alert.start_ns.to_le_bytes()).map_err(err)?;
        hash.update(&alert.end_ns.to_le_bytes()).map_err(err)?;
    }
    Ok(hash.finalize())
}

fn orientation(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> i8 {
    let ab_x = i128::from(b.0) - i128::from(a.0);
    let ab_y = i128::from(b.1) - i128::from(a.1);
    let ac_x = i128::from(c.0) - i128::from(a.0);
    let ac_y = i128::from(c.1) - i128::from(a.1);
    match ab_x * ac_y - ab_y * ac_x {
        cross if cross < 0 => -1,
        cross if cross > 0 => 1,
        _ => 0,
    }
}

fn between(value: i32, a: i32, b: i32) -> bool {
    value >= a.min(b) && value <= a.max(b)
}

fn on_segment(a: (i32, i32), b: (i32, i32), p: (i32, i32)) -> bool {
    between(p.0, a.0, b.0) && between(p.1, a.1, b.1)
}

fn segments_intersect(a: (i32, i32), b: (i32, i32), c: (i32, i32), d: (i32, i32)) -> bool {
    let (ab_c, ab_d, cd_a, cd_b) = (
        orientation(a, b, c),
        orientation(a, b, d),
        orientation(c, d, a),
        orientation(c, d, b),
    );
    (ab_c == 0 && on_segment(a, b, c))
        || (ab_d == 0 && on_segment(a, b, d))
        || (cd_a == 0 && on_segment(c, d, a))
        || (cd_b == 0 && on_segment(c, d, b))
        || (ab_c != ab_d && cd_a != cd_b)
}

fn scope_rect(projection: &oa_display::core::project::Projection) -> Option<(i32, i32, i32, i32)> {
    let right = i64::from(projection.width)
        .checked_mul(16)?
        .checked_sub(1)?;
    let top = i64::from(projection.top_inset).checked_mul(16)?;
    let bottom = i64::from(projection.height)
        .checked_mul(16)?
        .checked_sub(1)?;
    let (right, top, bottom) = (
        i32::try_from(right).ok()?,
        i32::try_from(top).ok()?,
        i32::try_from(bottom).ok()?,
    );
    (top <= bottom).then_some((0, top, right, bottom))
}

fn segment_intersects_rect(a: (i32, i32), b: (i32, i32), rect: (i32, i32, i32, i32)) -> bool {
    let (left, top, right, bottom) = rect;
    let inside =
        |point: (i32, i32)| (left..=right).contains(&point.0) && (top..=bottom).contains(&point.1);
    if inside(a) || inside(b) {
        return true;
    }
    let top_left = (left, top);
    let top_right = (right, top);
    let bottom_left = (left, bottom);
    let bottom_right = (right, bottom);
    [
        (top_left, top_right),
        (top_right, bottom_right),
        (bottom_right, bottom_left),
        (bottom_left, top_left),
    ]
    .iter()
    .any(|(edge_a, edge_b)| segments_intersect(a, b, *edge_a, *edge_b))
}

fn increment_ca_overlay(count: &mut u32) -> Result<(), String> {
    *count = count
        .checked_add(1)
        .ok_or_else(|| "CA overlay counter overflow".to_owned())?;
    Ok(())
}

fn render_ca_overlay(
    list: &mut oa_display::core::list::DisplayList,
    projection: &oa_display::core::project::Projection,
    alerts: &[oa_ca::Alert],
    tracks: &[ObservedTrack],
) -> Result<CaOverlayStats, String> {
    let mut stats = CaOverlayStats {
        pairs_examined: u32::try_from(alerts.len()).map_err(|_| "CA alert count overflow")?,
        ..CaOverlayStats::default()
    };
    if alerts.is_empty() {
        return Ok(stats);
    }
    let rect = scope_rect(projection).ok_or("invalid scope bounds for CA overlay")?;
    let mut positions = Vec::with_capacity(tracks.len());
    for track in tracks {
        positions.push((
            track.address,
            oa_display::core::project::project(projection, track.lat_e7, track.lon_e7),
        ));
    }
    positions.sort_unstable_by_key(|(address, _)| *address);
    for alert in alerts {
        let locate = |address| {
            positions
                .binary_search_by_key(&address, |(key, _)| *key)
                .ok()
                .and_then(|index| positions.get(index))
                .and_then(|(_, point)| *point)
        };
        let (Some(point_a), Some(point_b)) = (locate(alert.addr_a), locate(alert.addr_b)) else {
            increment_ca_overlay(&mut stats.pairs_unlocated)?;
            continue;
        };
        if !segment_intersects_rect(point_a, point_b, rect) {
            increment_ca_overlay(&mut stats.pairs_offscreen)?;
            continue;
        }
        oa_display::core::list::push_line_w(
            list,
            point_a,
            point_b,
            oa_display::core::palette::ALERT,
            CA_CONNECTOR_LAYER,
            2,
        )
        .map_err(err)?;
        let mid_x = ((i64::from(point_a.0) + i64::from(point_b.0)) / 2) as i32;
        let mid_y = ((i64::from(point_a.1) + i64::from(point_b.1)) / 2) as i32;
        let label_x = mid_x.clamp(rect.0, rect.2);
        let label_y = mid_y
            .saturating_sub(i32::from(CA_LABEL_SIZE / 2) * 16)
            .clamp(rect.1, rect.3);
        oa_display::core::list::push_text(
            list,
            (label_x, label_y),
            CA_LABEL_SIZE,
            oa_display::core::list::ALIGN_CENTRE,
            oa_display::core::palette::ALERT,
            CA_LABEL_LAYER,
            "CA",
        )
        .map_err(err)?;
        increment_ca_overlay(&mut stats.pairs_drawn)?;
    }
    let classified = stats
        .pairs_drawn
        .checked_add(stats.pairs_offscreen)
        .and_then(|count| count.checked_add(stats.pairs_unlocated))
        .ok_or("CA overlay classification overflow")?;
    if classified != stats.pairs_examined {
        return Err("CA overlay classification is incomplete".into());
    }
    Ok(stats)
}

pub fn surveillance_finish(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
) -> Result<SurveillanceFrame, String> {
    surveillance_finish_with_visibility(s, ownership, true, None)
}

pub(crate) fn surveillance_finish_with_visibility(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
    include_surveillance: bool,
    unavailable_reason: Option<ProductUnavailableReason>,
) -> Result<SurveillanceFrame, String> {
    surveillance_finish_with_policy(
        s,
        ownership,
        include_surveillance,
        unavailable_reason,
        None,
        SECOND_NS,
    )
}

type TrafficPolicySelector<'a> = dyn FnMut(
        &ObservedTrack,
        Option<TrackCallsignEvidence>,
    ) -> Result<oa_display::core::traffic::TrafficBlockPolicy, String>
    + 'a;

pub(crate) fn surveillance_finish_with_traffic_policy_and_track_period(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
    include_surveillance: bool,
    unavailable_reason: Option<ProductUnavailableReason>,
    policy_for_track: &mut TrafficPolicySelector<'_>,
    track_health_period_ns: i64,
) -> Result<SurveillanceFrame, String> {
    surveillance_finish_with_policy(
        s,
        ownership,
        include_surveillance,
        unavailable_reason,
        Some(policy_for_track),
        track_health_period_ns,
    )
}

fn capture_frame_snapshot(s: &mut Surveillance, time_ns: i64) -> Result<bool, String> {
    output_clear(&mut s.frame_snapshot);
    let tick_ns = tracker_config(&s.tracker).tick_ns;
    if time_ns.rem_euclid(tick_ns) == 0 {
        return Ok(false);
    }
    if tracker_counts(&s.tracker).1 == 0 {
        return Ok(true);
    }
    if !tracker_snapshot_at(&mut s.tracker, time_ns, &mut s.frame_snapshot) {
        return Err("tracker refused a snapshot outside its advanced non-grid clock".into());
    }
    if s.frame_snapshot.overflow != 0 || s.frame_snapshot.t_ns.len() > s.profile.max_tracks {
        return Err("current-frame tracker snapshot lost work".into());
    }
    Ok(true)
}

fn surveillance_finish_with_policy(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
    include_surveillance: bool,
    unavailable_reason: Option<ProductUnavailableReason>,
    policy_for_track: Option<&mut TrafficPolicySelector<'_>>,
    track_health_period_ns: i64,
) -> Result<SurveillanceFrame, String> {
    if !include_surveillance && unavailable_reason.is_none() {
        return Err("withheld surveillance products require a named reason".into());
    }
    if s.failed {
        return Err("surveillance failed; reconstruct from the admitted input prefix".into());
    }
    if s.completed {
        return Err("frame already completed or not begun".into());
    }
    let time_ns = s.previous_ns.ok_or("no admitted receipt group")?;
    // From this point the tracker, CA and scope are mutated. Any later error makes this object
    // terminal so callers cannot append events to or retry the partially completed frame.
    s.failed = true;
    tracker_advance(&mut s.tracker, time_ns, &mut s.drain);
    let has_frame_snapshot = capture_frame_snapshot(s, time_ns)?;
    let frame_output = if has_frame_snapshot {
        &s.frame_snapshot
    } else {
        &s.drain
    };
    if s.drain.overflow != 0
        || s.drain.t_ns.len() > s.profile.max_drain_rows
        || frame_output.overflow != 0
    {
        return Err("tracker output lost work".into());
    }
    let mut last = BTreeMap::new();
    let mut duplicate_rows_replaced = 0u32;
    // C7: no more than MAX_DRAIN_ROWS records are scanned; only the current tick is admitted.
    for (i, &time) in frame_output.t_ns.iter().enumerate() {
        if time == time_ns {
            let address = required(&frame_output.addr, i)?;
            if replace_latest_row(&mut last, address, i, s.profile.max_tracks)? {
                duplicate_rows_replaced = duplicate_rows_replaced.saturating_add(1);
            }
        }
    }
    let newest: BTreeMap<_, _> = tracker_live_iter(&s.tracker)
        .take(s.profile.max_tracks + 1)
        .collect();
    if newest.len() > s.profile.max_tracks {
        return Err("tracker live identity budget exceeded".into());
    }
    let mut snapshot = output_new(s.profile.max_tracks);
    let mut ca_rows = Vec::with_capacity(s.profile.max_tracks);
    let mut tracks = Vec::with_capacity(s.profile.max_tracks);
    let mut flags = Vec::with_capacity(s.profile.max_tracks);
    let mut msaw_rows = Vec::with_capacity(s.profile.max_tracks);
    let mut msaw_altitude_source_unknown = 0u32;
    for (&address, &index) in &last {
        let row = output_row(frame_output, index)?;
        let source_ns = *newest
            .get(&address)
            .ok_or("emitted row has no live track")?;
        if source_ns > time_ns {
            return Err("tracker snapshot contains future source evidence".into());
        }
        let radius = r95_m_of(&row.pos_cov_m2);
        let radius_mm = oa_ieee::round(radius * 1000.0);
        if !radius_mm.is_finite() || radius_mm < 0.0 || radius_mm > f64::from(u32::MAX) {
            return Err("invalid tracker radius".into());
        }
        let altitude = if row.flags & O_HAS_ALT != 0 {
            fixed_i32(f64::from(row.alt_ft))?
        } else {
            NONE_I32
        };
        let rate = if row.flags & O_HAS_ALT != 0 {
            fixed_i32(f64::from(row.vrate_fpm))?
        } else {
            NONE_I32
        };
        tracks.push(ObservedTrack {
            state_ns: time_ns,
            newest_source_ns: source_ns,
            address,
            lat_e7: row.lat_e7,
            lon_e7: row.lon_e7,
            altitude_ft: altitude,
            vertical_rate_fpm: rate,
            east_mmps: fixed_i32(f64::from(row.vel_east_mps) * 1000.0)?,
            north_mmps: fixed_i32(f64::from(row.vel_north_mps) * 1000.0)?,
            r95_mm: radius_mm as u32,
        });
        flags.push(row.flags);
        let mut ca_flags = 0;
        if row.flags & O_COAST != 0 {
            ca_flags |= F_COAST;
        }
        if row.flags & O_UNCERTAIN != 0 {
            ca_flags |= F_UNCERTAIN;
        }
        if row.flags & O_IDENTITY_UNCONFIRMED != 0 {
            ca_flags |= F_IDENTITY_UNCONFIRMED;
        }
        ca_rows.push(CaTrack {
            lat_deg: f64::from(row.lat_e7) / 1e7,
            lon_deg: f64::from(row.lon_e7) / 1e7,
            addr: address,
            vel_east_mps: row.vel_east_mps,
            vel_north_mps: row.vel_north_mps,
            r95_m: radius as f32,
            alt_ft: if row.flags & O_HAS_ALT != 0 {
                row.alt_ft
            } else {
                f32::NAN
            },
            vrate_fpm: if row.flags & O_HAS_ALT != 0 {
                row.vrate_fpm
            } else {
                f32::NAN
            },
            flags: ca_flags,
            ground: AIRBORNE,
        });
        if s.msaw.is_some() {
            let mut msaw_flags = 0;
            if row.flags & O_COAST != 0 {
                msaw_flags |= oa_msaw::F_COAST;
            }
            if row.flags & O_UNCERTAIN != 0 {
                msaw_flags |= oa_msaw::F_UNCERTAIN;
            }
            if row.flags & O_IDENTITY_UNCONFIRMED != 0 {
                msaw_flags |= oa_msaw::F_IDENTITY_UNCONFIRMED;
            }
            let source = s.baro_seen.get(&address).copied();
            let age_s = match source {
                Some(at) if at <= time_ns => {
                    let age = time_ns
                        .checked_sub(at)
                        .ok_or("barometric source age overflow")?;
                    let exact = age as f64 / 1e9;
                    let rounded = exact as f32;
                    // Round measurement age upward, never making an old altitude younger.
                    if f64::from(rounded) < exact {
                        f32::from_bits(
                            rounded
                                .to_bits()
                                .checked_add(1)
                                .ok_or("altitude age rounding overflow")?,
                        )
                    } else {
                        rounded
                    }
                }
                Some(_) => return Err("barometric evidence is after the state cutoff".into()),
                None => {
                    msaw_altitude_source_unknown = msaw_altitude_source_unknown.saturating_add(1);
                    f32::NAN
                }
            };
            msaw_rows.push(oa_msaw::MsawTrack {
                lat_deg: f64::from(row.lat_e7) / 1e7,
                lon_deg: f64::from(row.lon_e7) / 1e7,
                addr: address,
                vel_east_mps: row.vel_east_mps,
                vel_north_mps: row.vel_north_mps,
                r95_m: radius as f32,
                alt_ft: if row.flags & O_HAS_ALT != 0 && source.is_some() {
                    row.alt_ft
                } else {
                    f32::NAN
                },
                vrate_fpm: if row.flags & O_HAS_ALT != 0 {
                    row.vrate_fpm
                } else {
                    f32::NAN
                },
                flags: msaw_flags,
                ground: oa_msaw::AIRBORNE,
                alt_age_s: age_s,
                alt_change_10s_ft: f32::NAN,
                ground_ft: f32::NAN,
            });
        }
        append(&mut snapshot, row)?;
    }
    prune_track_callsigns(s, &tracks, time_ns);
    let traffic_block_policies = if include_surveillance {
        match policy_for_track {
            Some(select) => {
                let mut policies = Vec::with_capacity(s.profile.max_tracks);
                for track in &tracks {
                    let evidence = s
                        .track_callsigns
                        .get(&track.address)
                        .copied()
                        .filter(|evidence| evidence.source_ns == track.newest_source_ns);
                    let policy = select(track, evidence)?;
                    if policy.address != track.address {
                        return Err("traffic policy returned a different track address".into());
                    }
                    policies.push(policy);
                }
                policies.sort_unstable_by_key(|policy| policy.address);
                Some(policies)
            }
            None => None,
        }
    } else {
        None
    };
    let msaw = match s.msaw.as_mut() {
        Some(service) => Some(crate::msaw_bridge::regional_msaw_cycle(
            service,
            time_ns,
            &msaw_rows,
            &mut s.states,
            &mut s.inhibits,
        )?),
        None => None,
    };
    let ca = ca_cycle_with_inhibits(
        &mut s.ca,
        time_ns,
        &ca_rows,
        &mut s.alerts,
        &mut s.events,
        &mut s.states,
        &mut s.inhibits,
    );
    let ca_presented: Vec<_> = ca_open(&s.ca)
        .iter()
        .filter(|alert| alert.flags & oa_ca::AF_SUPPRESSED == 0)
        .copied()
        .collect();
    let (ca_closed, ca_altitude_lost) = capture_ca_logs(s);
    let ca_suppressed = oa_safety::inhibit::suppressed_log(&s.inhibits).to_vec();
    let ca_suppressed_overflow = oa_safety::inhibit::suppressed_overflow(&s.inhibits);
    oa_safety::inhibit::suppressed_clear(&mut s.inhibits);
    let withheld_tracks = if include_surveillance {
        0
    } else {
        u32::try_from(tracks.len()).map_err(|_| "withheld track count overflow")?
    };
    let withheld_ca_records = if include_surveillance {
        0
    } else {
        let rows = ca_presented
            .len()
            .checked_add(ca_closed.len())
            .and_then(|count| count.checked_add(ca_altitude_lost.len()))
            .and_then(|count| count.checked_add(ca_suppressed.len()))
            .ok_or("withheld CA record count overflow")?;
        u32::try_from(rows).map_err(|_| "withheld CA record count overflow")?
    };
    let withheld_msaw_records = if include_surveillance {
        0
    } else {
        let rows = msaw.as_ref().map_or(Ok(0), |frame| {
            frame
                .open
                .len()
                .checked_add(frame.closed.len())
                .and_then(|count| count.checked_add(frame.skipped.len()))
                .ok_or("withheld MSAW record count overflow")
        })?;
        u32::try_from(rows).map_err(|_| "withheld MSAW record count overflow")?
    };
    let withheld_alerts = withheld_ca_records
        .checked_add(withheld_msaw_records)
        .ok_or("withheld alert count overflow")?;
    let msaw_configured = msaw.is_some();
    let feeds = feed_statuses_with_track_period(s, time_ns, track_health_period_ns)?;
    let mut ca_overlay = None;
    let mut safety_stats = None;
    let (msaw_markers, traffic_stats, digest, displayed, off_map, warnings) =
        if !s.profile.render_scope {
            let digest = if include_surveillance {
                offline_projection_digest(time_ns, &tracks, &ca_presented)?
            } else {
                offline_projection_digest(time_ns, &[], &[])?
            };
            (None, None, digest, 0, 0, Vec::new())
        } else {
            let mut store = RowStore::default();
            if include_surveillance {
                let stats = store_add(
                    &mut store,
                    &row_columns(&snapshot, &mut ColumnBuf::default()),
                    time_ns,
                )
                .map_err(err)?;
                if stats.kept != stats.rows_in || store_finish(&mut store) != 0 {
                    return Err("scope input snapshot lost rows".into());
                }
            }
            let newest_track = tracker_newest_update_ns(&s.tracker);
            let info = replay_build_with_overlays(
                &mut s.scope,
                &store,
                &Callsigns::default(),
                time_ns,
                "REGIONAL RESEARCH",
                Some(&ReplaySafety {
                    states: &s.states,
                    inhibits: &s.inhibits,
                    feeds: &feeds,
                    newest_track_ns: (newest_track != i64::MIN).then_some(newest_track),
                    frozen_limit_ns: 5 * SECOND_NS,
                    use_reserved_status_header: true,
                    traffic_block_policies: traffic_block_policies.as_deref(),
                }),
                ownership,
            )
            .map_err(err)?;
            let rendered_safety_stats = info
                .safety_stats
                .ok_or("regional scope omitted safety-layer accounting")?;
            if rendered_safety_stats.targets_under_banner != 0 {
                return Err("regional safety banner overlaps a displayed target".into());
            }
            safety_stats = Some(rendered_safety_stats);
            if info.refused != 0
                || info.malformed != 0
                || info.stamp_clipped
                || info.ownership_clipped
                || info.convert.invalid_position != 0
                || info.convert.unprojectable != 0
                || info.convert.columns_inconsistent != 0
            {
                return Err("scope lost or could not represent current evidence".into());
            }
            let displayed = targets_columns(&s.scope.set);
            // Off-map targets are a named presentation exclusion. Every displayed target must be one of
            // the exact same issue-time observations offered to CA; no independent fixture traffic.
            for (i, &address) in displayed.addr.iter().enumerate() {
                let track = tracks
                    .iter()
                    .find(|t| t.address == address)
                    .ok_or("scope/CA identity mismatch")?;
                if required(displayed.lat_e7, i)? != track.lat_e7
                    || required(displayed.lon_e7, i)? != track.lon_e7
                {
                    return Err("scope/CA position mismatch".into());
                }
            }
            declutter_scope_blocks(&mut s.scope.list, &s.scope.view.proj, &displayed)?;
            if include_surveillance {
                ca_overlay = Some(render_ca_overlay(
                    &mut s.scope.list,
                    &s.scope.view.proj,
                    &ca_presented,
                    &tracks,
                )?)
            }
            // Reserve rows 104..176 for alerts and console text. Layer 14 masks map primitives that
            // extend beyond their clipped centre, while the standard legend remains in rows 0..104.
            oa_display::list::push_rect(
                &mut s.scope.list,
                (0, 104 * 16),
                (
                    i32::try_from(s.scope.view.proj.width).map_err(|_| "panel width")? * 16,
                    i32::try_from(REGIONAL_STATUS_HEADER_INSET_PX)
                        .map_err(|_| "status header inset")?
                        * 16,
                ),
                oa_display::palette::BACKGROUND,
                14,
            )
            .map_err(err)?;
            oa_display::list::push_line(
                &mut s.scope.list,
                (
                    0,
                    i32::try_from(REGIONAL_STATUS_HEADER_INSET_PX)
                        .map_err(|_| "status header inset")?
                        .saturating_sub(1)
                        * 16,
                ),
                (
                    i32::try_from(s.scope.view.proj.width).map_err(|_| "panel width")? * 16,
                    i32::try_from(REGIONAL_STATUS_HEADER_INSET_PX)
                        .map_err(|_| "status header inset")?
                        .saturating_sub(1)
                        * 16,
                ),
                oa_display::palette::DIM_TEXT,
                15,
            )
            .map_err(err)?;
            let msaw_markers = if include_surveillance {
                if let Some(frame) = msaw.as_ref() {
                    use oa_display::core::alert_markers::{
                        append_alert_markers, AlertMarker, MarkerState, NO_POSITION_E7,
                    };
                    if frame.open.len() > s.profile.max_tracks {
                        return Err("MSAW alert marker capacity exceeded".into());
                    }
                    let mut markers = Vec::with_capacity(s.profile.max_tracks);
                    for alert in &frame.open {
                        let position = tracks.iter().find(|track| track.address == alert.addr);
                        let state =
                            if frame.stats.is_none() || alert.end_reason != oa_msaw::END_CLEAR {
                                MarkerState::ProtectionLost
                            } else {
                                match alert.peak {
                                    oa_msaw::Grade::Caution => MarkerState::Caution,
                                    oa_msaw::Grade::Warning => MarkerState::Warning,
                                }
                            };
                        markers.push(AlertMarker {
                            address: alert.addr,
                            lat_e7: position.map_or(NO_POSITION_E7, |track| track.lat_e7),
                            lon_e7: position.map_or(NO_POSITION_E7, |track| track.lon_e7),
                            state,
                            position_known: u8::from(position.is_some()),
                            reserved: [0; 2],
                        });
                    }
                    markers.sort_by_key(|marker| marker.address);
                    Some(
                        append_alert_markers(&mut s.scope.list, &s.scope.view.proj, &markers)
                            .map_err(err)?,
                    )
                } else {
                    None
                }
            } else {
                None
            };
            replay_raster(&mut s.scope).map_err(err)?;
            let mut warnings = Vec::new();
            if s.scope.list.texts.start.len() > 100_000 {
                return Err("scope text evidence budget exceeded".into());
            }
            for (start, len) in s.scope.list.texts.start.iter().zip(&s.scope.list.texts.len) {
                let start = usize::try_from(*start).map_err(|_| "text offset")?;
                let end = start
                    .checked_add(usize::try_from(*len).map_err(|_| "text length")?)
                    .ok_or("text span overflow")?;
                let text = s
                    .scope
                    .list
                    .text
                    .get(start..end)
                    .ok_or("invalid scope text span")?;
                if text.starts_with("FEED ")
                    || text.starts_with("PICTURE ")
                    || text.starts_with("MSAW ")
                    || text.starts_with("AREA ")
                    || text.starts_with("APPROACH ")
                    || text.starts_with("CONFLICT ")
                {
                    warnings.push(text.to_owned());
                }
            }
            let digest = frame_digest(&s.scope.spec, &s.scope.buf).map_err(err)?;
            (
                msaw_markers,
                Some(info.traffic),
                digest,
                info.convert.added,
                info.convert.off_map,
                warnings,
            )
        };
    let track_transport_seen_ns = s.seen.first().copied().flatten();
    s.completed = true;
    s.failed = false;
    if !include_surveillance {
        tracks.clear();
        flags.clear();
    }
    Ok(SurveillanceFrame {
        time_ns,
        raster_rendered: s.profile.render_scope,
        traffic_stats,
        tracks,
        flags,
        frame_sha256: digest,
        ca_open: ca_presented,
        ca_overlay,
        ca_closed,
        ca_altitude_lost,
        ca_suppressed,
        ca_suppressed_overflow,
        msaw,
        msaw_markers,
        msaw_altitude_source_unknown,
        ca_stats: ca.stats,
        ca_work_short: ca.work_short,
        ca_input_short: ca.input_short,
        tracker_stats: *tracker_stats(&s.tracker),
        track_transport_seen_ns,
        track_measurement_seen_ns: s.track_measurement_seen_ns,
        feeds,
        safety_stats,
        track_product: product_availability(include_surveillance, true, unavailable_reason)?,
        ca_product: product_availability(include_surveillance, true, unavailable_reason)?,
        msaw_product: product_availability(
            include_surveillance,
            msaw_configured,
            unavailable_reason,
        )?,
        surveillance_withheld: !include_surveillance,
        withheld_tracks,
        withheld_alerts,
        withheld_ca_records,
        withheld_msaw_records,
        measurement_rejections: s.measurement_rejections,
        duplicate_rows_replaced,
        displayed,
        off_map,
        warnings,
    })
}

fn product_availability(
    include_surveillance: bool,
    configured: bool,
    reason: Option<ProductUnavailableReason>,
) -> Result<ProductAvailability, String> {
    if include_surveillance && configured {
        return Ok(ProductAvailability::Available);
    }
    if include_surveillance {
        return Ok(ProductAvailability::NotConfigured);
    }
    reason
        .map(ProductAvailability::Withheld)
        .ok_or_else(|| "withheld surveillance products require a named reason".into())
}

#[derive(Clone, Copy)]
struct BlockGroup {
    address: u32,
    start: usize,
    end: usize,
    center: (i32, i32),
    original: TextBounds,
}

fn text_at(list: &DisplayList, row: usize) -> &str {
    list.texts
        .start
        .get(row)
        .zip(list.texts.len.get(row))
        .and_then(|(&start, &len)| {
            let begin = usize::try_from(start).ok()?;
            let end = begin.checked_add(usize::try_from(len).ok()?)?;
            list.text.get(begin..end)
        })
        .unwrap_or("")
}

fn block_bounds(list: &DisplayList, start: usize, end: usize) -> Result<TextBounds, String> {
    let x = *list
        .texts
        .x
        .get(start)
        .ok_or("missing block x coordinate")?;
    let y = *list
        .texts
        .y
        .get(start)
        .ok_or("missing block y coordinate")?;
    let size = *list
        .texts
        .size
        .get(start)
        .ok_or("missing block font size")?;
    let align = *list
        .texts
        .align
        .get(start)
        .ok_or("missing block alignment")?;
    let chars = (start..end)
        .map(|row| text_at(list, row).chars().count())
        .max()
        .unwrap_or(0);
    let lines = u32::try_from(end.saturating_sub(start)).map_err(|_| "block line count")?;
    Ok(font::text_block_bounds_px(
        (x, y),
        size,
        align,
        chars,
        lines,
        14,
    ))
}

fn overlap_area(a: TextBounds, b: TextBounds) -> i64 {
    i64::from((a.x1.min(b.x1) - a.x0.max(b.x0)).max(0))
        .saturating_mul(i64::from((a.y1.min(b.y1) - a.y0.max(b.y0)).max(0)))
}

fn symbol_bounds(center: (i32, i32)) -> TextBounds {
    let x = center.0 / 16;
    let y = center.1 / 16;
    TextBounds {
        x0: x - 9,
        y0: y - 9,
        x1: x + 10,
        y1: y + 10,
    }
}

fn choose_block_bounds(
    center: (i32, i32),
    width: i32,
    height: i32,
    placed: &[TextBounds],
    symbols: &[(i32, i32)],
    map_labels: &[(usize, TextBounds)],
    projection: &Projection,
) -> TextBounds {
    let (x, y) = (center.0 / 16, center.1 / 16);
    let view = TextBounds {
        x0: 2,
        y0: i32::try_from(projection.top_inset)
            .unwrap_or(i32::MAX)
            .saturating_add(2),
        x1: i32::try_from(projection.width)
            .unwrap_or(i32::MAX)
            .saturating_sub(2),
        y1: i32::try_from(projection.height)
            .unwrap_or(i32::MAX)
            .saturating_sub(2),
    };
    let mut best = TextBounds {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };
    let mut best_cost = i64::MAX;
    for (dx, dy) in [
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
    ] {
        for distance in [60, 160] {
            let x0 = x + dx * distance
                - if dx < 0 {
                    width
                } else if dx == 0 {
                    width / 2
                } else {
                    0
                };
            let y0 = y + dy * distance
                - if dy < 0 {
                    height
                } else if dy == 0 {
                    height / 2
                } else {
                    0
                };
            let candidate = TextBounds {
                x0,
                y0,
                x1: x0.saturating_add(width),
                y1: y0.saturating_add(height),
            };
            let outside = (i64::from(width) * i64::from(height))
                .saturating_sub(overlap_area(candidate, view));
            let cost = outside
                .saturating_mul(1000)
                .saturating_add(placed.iter().map(|&b| overlap_area(candidate, b)).sum())
                .saturating_add(
                    symbols
                        .iter()
                        .map(|&at| overlap_area(candidate, symbol_bounds(at)))
                        .sum(),
                )
                .saturating_add(
                    map_labels
                        .iter()
                        .map(|(_, b)| overlap_area(candidate, *b))
                        .sum(),
                );
            if cost < best_cost {
                best = candidate;
                best_cost = cost;
            }
        }
    }
    best
}

/// Repositions mandatory traffic blocks using the same measured-text, map-label-yielding
/// placement policy as the demonstration renderer. Flight data remains visible; only colliding
/// map text yields. Work is bounded by the regional profile's track cap and the display's
/// existing 100,000-text frame limit.
fn collect_block_groups(
    list: &DisplayList,
    projection: &Projection,
    targets: &oa_display::core::traffic::TargetColumns<'_>,
) -> Result<Vec<BlockGroup>, String> {
    let mut groups = Vec::with_capacity(targets.addr.len());
    let mut cursor = 0usize;
    for (source, &address) in targets.addr.iter().enumerate() {
        let lat = *targets
            .lat_e7
            .get(source)
            .ok_or("missing target latitude")?;
        let lon = *targets
            .lon_e7
            .get(source)
            .ok_or("missing target longitude")?;
        let Some(center) = project(projection, lat, lon) else {
            return Err("displayed target cannot be projected for block layout".into());
        };
        let old_x = center.0.saturating_add(14 * 16);
        let old_y = center.1.saturating_sub(30 * 16);
        let Some(start) = (cursor..list.texts.x.len()).find(|&row| {
            list.texts.layer.get(row) == Some(&LAYER_BLOCK)
                && list.texts.x.get(row) == Some(&old_x)
                && list.texts.y.get(row) == Some(&old_y)
        }) else {
            return Err("displayed target has no corresponding data block".into());
        };
        let mut end = start;
        while end < start.saturating_add(4)
            && list.texts.layer.get(end) == Some(&LAYER_BLOCK)
            && list.texts.x.get(end) == Some(&old_x)
            && list.texts.y.get(end).is_some_and(|&row_y| {
                row_y
                    == old_y
                        .saturating_add(i32::try_from(end - start).unwrap_or(i32::MAX) * 14 * 16)
            })
        {
            end += 1;
        }
        if end == start {
            return Err("displayed target has an empty data block".into());
        }
        cursor = end;
        groups.push(BlockGroup {
            address,
            start,
            end,
            center,
            original: block_bounds(list, start, end)?,
        });
    }
    groups.sort_unstable_by_key(|group| group.address);
    Ok(groups)
}

fn declutter_scope_blocks(
    list: &mut DisplayList,
    projection: &Projection,
    targets: &oa_display::core::traffic::TargetColumns<'_>,
) -> Result<(), String> {
    if list.texts.x.len() > 100_000 {
        return Err("scope text evidence budget exceeded".into());
    }
    let groups = collect_block_groups(list, projection, targets)?;
    let mut map_rows = Vec::new();
    for row in 0..list.texts.x.len() {
        let layer = list.texts.layer.get(row).copied().unwrap_or(u8::MAX);
        if !matches!(layer, 4 | LAYER_TEXT) || text_at(list, row).is_empty() {
            continue;
        }
        let x = *list.texts.x.get(row).ok_or("missing map label x")?;
        let y = *list.texts.y.get(row).ok_or("missing map label y")?;
        let size = *list.texts.size.get(row).ok_or("missing map label size")?;
        let align = *list
            .texts
            .align
            .get(row)
            .ok_or("missing map label alignment")?;
        let bounds = font::text_block_bounds_px(
            (x, y),
            size,
            align,
            text_at(list, row).chars().count(),
            1,
            u32::from(size).saturating_add(2),
        );
        map_rows.push((row, bounds));
    }
    let symbols: Vec<_> = groups.iter().map(|group| group.center).collect();
    let mut placed = Vec::with_capacity(groups.len());
    for group in groups {
        let width = group.original.x1 - group.original.x0;
        let height = group.original.y1 - group.original.y0;
        let extent_x = 160i32.saturating_add(width);
        let extent_y = 160i32.saturating_add(height);
        let center_x = group.center.0 / 16;
        let center_y = group.center.1 / 16;
        let labels: Vec<_> = map_rows
            .iter()
            .copied()
            .filter(|(_, bounds)| {
                bounds.x1 >= center_x.saturating_sub(extent_x)
                    && bounds.x0 <= center_x.saturating_add(extent_x)
                    && bounds.y1 >= center_y.saturating_sub(extent_y)
                    && bounds.y0 <= center_y.saturating_add(extent_y)
            })
            .collect();
        let first = choose_block_bounds(
            group.center,
            width,
            height,
            &placed,
            &symbols,
            &labels,
            projection,
        );
        let clear = choose_block_bounds(
            group.center,
            width,
            height,
            &placed,
            &symbols,
            &[],
            projection,
        );
        let should_yield = labels
            .iter()
            .any(|(_, bounds)| overlap_area(clear, *bounds) > 0);
        let chosen = if should_yield {
            for (row, bounds) in &mut map_rows {
                if overlap_area(clear, *bounds) > 0 {
                    if let Some(len) = list.texts.len.get_mut(*row) {
                        *len = 0;
                    }
                    *bounds = TextBounds {
                        x0: 0,
                        y0: 0,
                        x1: 0,
                        y1: 0,
                    };
                }
            }
            clear
        } else {
            first
        };
        let dx = chosen
            .x0
            .saturating_sub(group.original.x0)
            .saturating_mul(16);
        let dy = chosen
            .y0
            .saturating_sub(group.original.y0)
            .saturating_mul(16);
        for row in group.start..group.end {
            if let Some(x) = list.texts.x.get_mut(row) {
                *x = x.saturating_add(dx);
            }
            if let Some(y) = list.texts.y.get_mut(row) {
                *y = y.saturating_add(dy);
            }
        }
        placed.push(chosen);
    }
    Ok(())
}

pub(crate) fn feed_statuses_with_track_period(
    s: &Surveillance,
    time_ns: i64,
    track_health_period_ns: i64,
) -> Result<Vec<FeedStatus>, String> {
    if track_health_period_ns <= 0 {
        return Err("TRACK health period must be positive".into());
    }
    let slow_after_ns = track_health_period_ns
        .checked_mul(2)
        .ok_or("TRACK slow-health threshold overflow")?;
    let lost_after_ns = track_health_period_ns
        .checked_mul(5)
        .ok_or("TRACK lost-health threshold overflow")?;
    let mut feeds = Vec::with_capacity(FEED_NAMES.len());
    for (i, &name) in FEED_NAMES.iter().enumerate() {
        let (slow_threshold_ns, lost_threshold_ns) = if i == 0 {
            (slow_after_ns, lost_after_ns)
        } else {
            (2 * SECOND_NS, 5 * SECOND_NS)
        };
        let seen = if i == 0 {
            s.track_measurement_seen_ns.or_else(|| {
                if s.track_down {
                    s.seen.first().copied().flatten()
                } else {
                    None
                }
            })
        } else {
            required(&s.seen, i)?
        };
        let down = if i == 0 {
            s.track_down
        } else {
            required(&s.down, i)?
        };
        let state = match seen {
            None => FeedState::NoMessageSeen,
            Some(at) => {
                let age = time_ns.checked_sub(at).ok_or("feed age overflow")?;
                if age < 0 {
                    return Err("feed age is negative".into());
                }
                let age_ms = u32::try_from(age / 1_000_000)
                    .map_err(|_| "feed age exceeds research envelope")?;
                if down || age >= lost_threshold_ns {
                    FeedState::Lost { age_ms }
                } else if age >= slow_threshold_ns {
                    FeedState::Slow { age_ms }
                } else {
                    FeedState::Live
                }
            }
        };
        feeds.push(FeedStatus { name, state });
    }
    Ok(feeds)
}

/// Pixels are borrowed only while the completed immutable frame is being published by the shell.
pub fn surveillance_pixels(s: &Surveillance) -> Result<&[u8], String> {
    if !s.profile.render_scope || s.frames == 0 || s.failed || !s.completed {
        return Err("no publishable pixels for an incomplete or failed frame".into());
    }
    Ok(&s.scope.buf)
}

pub fn surveillance_frame_spec(s: &Surveillance) -> Result<oa_display::raster::FrameSpec, String> {
    if !s.profile.render_scope {
        return Err("frame geometry is unavailable when raster rendering is disabled".into());
    }
    let spec = s.scope.spec;
    if spec.width != s.profile.frame_width || spec.height != s.profile.frame_height {
        return Err("raster geometry differs from the selected profile".into());
    }
    Ok(spec)
}

/// Appends controller selection/outcome to one completed frame, then re-rasterizes that exact
/// list. A failure invalidates publication; the returned digest replaces the pre-overlay one.
pub fn surveillance_present_interaction(
    s: &mut Surveillance,
    interaction: &oa_display::interaction::InteractionState,
) -> Result<[u8; 32], String> {
    surveillance_present_controls(s, interaction, "")
}

/// Replace control primitives while retaining the exact observed/safety/map base. Input editing
/// cannot accumulate old labels or consume another surveillance epoch (offline shell, C6/C7).
pub fn surveillance_present_controls(
    s: &mut Surveillance,
    interaction: &oa_display::interaction::InteractionState,
    console: &str,
) -> Result<[u8; 32], String> {
    surveillance_present_controls_inner(s, interaction, console, None)
}

/// Render the configured final-monitor result on the same admitted scope frame and within the
/// ordinary single presentation update. Empty degraded results still draw their status header.
pub fn surveillance_present_controls_with_final_monitor(
    s: &mut Surveillance,
    interaction: &oa_display::interaction::InteractionState,
    console: &str,
    markers: &[oa_display::core::alert_markers::FinalMonitorMarker],
    degraded: bool,
) -> Result<[u8; 32], String> {
    surveillance_present_controls_inner(s, interaction, console, Some((markers, degraded)))
}

fn surveillance_present_controls_inner(
    s: &mut Surveillance,
    interaction: &oa_display::interaction::InteractionState,
    console: &str,
    final_monitor: Option<(&[oa_display::core::alert_markers::FinalMonitorMarker], bool)>,
) -> Result<[u8; 32], String> {
    if !s.profile.render_scope {
        return Err("analysis-only profile does not produce a rendered control frame".into());
    }
    if s.failed || !s.completed || s.frames == 0 {
        return Err("control overlay requires a completed frame".into());
    }
    if console.len() > 192 || !console.bytes().all(|byte| (32..=126).contains(&byte)) {
        return Err("console and mode status must be at most 192 printable ASCII bytes".into());
    }
    if s.presentation_updates >= s.profile.max_presentation_updates {
        return Err("regional presentation update budget exhausted".into());
    }
    let base = s.overlay_base.unwrap_or(OverlayBase {
        texts: s.scope.list.texts.x.len(),
        bytes: s.scope.list.text.len(),
        layers: s.scope.list.layers_used,
    });
    let list = &mut s.scope.list;
    if [
        list.texts.x.len(),
        list.texts.y.len(),
        list.texts.size.len(),
        list.texts.align.len(),
        list.texts.colour.len(),
        list.texts.layer.len(),
        list.texts.start.len(),
        list.texts.len.len(),
    ]
    .iter()
    .any(|len| *len < base.texts)
        || base.bytes > list.text.len()
        || !list.text.is_char_boundary(base.bytes)
    {
        return Err("control overlay base is inconsistent".into());
    }
    s.failed = true;
    s.presentation_updates += 1;
    list.texts.x.truncate(base.texts);
    list.texts.y.truncate(base.texts);
    list.texts.size.truncate(base.texts);
    list.texts.align.truncate(base.texts);
    list.texts.colour.truncate(base.texts);
    list.texts.layer.truncate(base.texts);
    list.texts.start.truncate(base.texts);
    list.texts.len.truncate(base.texts);
    list.text.truncate(base.bytes);
    list.layers_used = base.layers;
    s.overlay_base = Some(base);
    oa_display::interaction::present_interaction(
        interaction,
        &mut s.scope.list,
        &s.scope.view.proj,
    )
    .map_err(err)?;
    for (index, line) in console.as_bytes().chunks(64).enumerate() {
        let text = std::str::from_utf8(line).map_err(|_| "console encoding")?;
        let y = REGIONAL_MODE_HEADER_Y_PX + i32::try_from(index).map_err(|_| "console row")? * 12;
        oa_display::list::push_text(
            &mut s.scope.list,
            (8 * 16, y * 16),
            7,
            oa_display::list::ALIGN_LEFT,
            oa_display::palette::TEXT,
            15,
            text,
        )
        .map_err(err)?;
    }
    if let Some((markers, degraded)) = final_monitor {
        oa_display::core::alert_markers::append_final_monitor_frame(
            &mut s.scope.list,
            &s.scope.view.proj,
            markers,
            degraded,
        )
        .map_err(err)?;
    }
    replay_raster(&mut s.scope).map_err(err)?;
    let digest = frame_digest(&s.scope.spec, &s.scope.buf).map_err(err)?;
    s.failed = false;
    Ok(digest)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use oa_airspace::core::model::{self, Model};
    use oa_ca::ca_cycle;

    const SPARSE_TEST_ADDRESS: u32 = 0x00ab_cdef;

    fn test_model() -> Model {
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
            vertices: model::Vertices::default(),
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
            let mut intern = |text: &str| {
                let index = model::StrIx::new(
                    u32::try_from(model.strings.offsets.len() - 1).unwrap_or(u32::MAX),
                );
                model.strings.text.push_str(text);
                model
                    .strings
                    .offsets
                    .push(u32::try_from(model.strings.text.len()).unwrap_or(u32::MAX));
                index
            };
            let ident = intern(name);
            let region = intern("K1");
            model.points.kind.push(model::point_kind::FIX);
            model.points.lat.push(lat);
            model.points.lon.push(lon);
            model.points.elev_ft.push(model::ELEV_NONE);
            model.points.ident.push(ident);
            model.points.alt_ident.push(model::StrIx::NONE);
            model.points.region.push(region);
            model.points.scope.push(model::StrIx::NONE);
            model.points.name.push(model::StrIx::NONE);
        }
        model
    }

    fn review_map_model() -> Model {
        use oa_airspace::core::model::{
            boundary_source, edge_interp, level, point_kind, proc_kind, stratum, time_code,
            AirwayLegIx, PointIx, PosE7, RouteIx, SegIx, Span, VertexIx,
        };

        let mut model = crate::cli::synthetic_model().expect("public synthetic adaptation");
        model.meta.artcc = "ZAU".into();
        let intern = |model: &mut Model, text: &str| {
            let index = model::StrIx::new(
                u32::try_from(model.strings.offsets.len() - 1).expect("small fixture index"),
            );
            model.strings.text.push_str(text);
            model
                .strings
                .offsets
                .push(u32::try_from(model.strings.text.len()).expect("small fixture string"));
            index
        };

        for (ident, latitude) in [("KAAA", 410_100_000), ("1AAA", 410_200_000)] {
            let ident_ix = intern(&mut model, ident);
            let region_ix = intern(&mut model, "K5");
            model.points.kind.push(point_kind::AIRPORT);
            model.points.lat.push(latitude);
            model.points.lon.push(-870_000_000);
            model.points.elev_ft.push(model::ELEV_NONE);
            model.points.ident.push(ident_ix);
            model.points.alt_ident.push(model::StrIx::NONE);
            model.points.region.push(region_ix);
            model.points.scope.push(model::StrIx::NONE);
            model.points.name.push(model::StrIx::NONE);
            model::bit_push(&mut model.points.src_nasr, true);
            model::bit_push(&mut model.points.src_cifp, false);
            model::bit_push(&mut model.points.in_area, true);
            model::bit_push(&mut model.points.cls_b, false);
            model::bit_push(&mut model.points.cls_c, false);
            model::bit_push(&mut model.points.cls_d, false);
            model::bit_push(&mut model.points.cls_e, false);
        }

        model.procs.kind.push(proc_kind::APPROACH);
        model.procs.airport.push(PointIx::new(2));
        let procedure_ident = intern(&mut model, "I09");
        model.procs.ident.push(procedure_ident);
        model.procs.name.push(model::StrIx::NONE);
        model.procs.routes.push(Span {
            start: RouteIx::new(0),
            len: 0,
        });
        model.procs.src_nasr = model::bit_zeros(1);
        model.procs.src_cifp = model::bit_zeros(1);
        model.procs.coded = model::bit_zeros(1);
        model::bit_set(&mut model.procs.coded, 0, true);

        for (facility, lat, lon) in [
            ("ZAU", 410_000_000, -870_000_000),
            ("ZMP", 410_200_000, -870_000_000),
        ] {
            let facility_ix = intern(&mut model, facility);
            let start = u32::try_from(model.vertices.len()).expect("small vertex fixture");
            for (latitude, longitude) in [
                (lat + 1_000_000, lon - 1_000_000),
                (lat + 1_000_000, lon + 1_000_000),
                (lat - 1_000_000, lon + 1_000_000),
                (lat - 1_000_000, lon - 1_000_000),
                (lat + 1_000_000, lon - 1_000_000),
            ] {
                model.vertices.push(PosE7 {
                    lat: latitude,
                    lon: longitude,
                });
            }
            model.boundaries.source.push(boundary_source::NASR_ARB);
            model.boundaries.stratum.push(stratum::ARTCC_HIGH);
            model.boundaries.edge_interp.push(edge_interp::PER_SEGMENT);
            model.boundaries.multiple.push(0);
            model.boundaries.facility.push(facility_ix);
            model.boundaries.name.push(model::StrIx::NONE);
            model.boundaries.floor_ft.push(model::ALT_NONE);
            model.boundaries.ceiling_ft.push(model::ALT_NONE);
            model.boundaries.time_code.push(time_code::UNKNOWN);
            model.boundaries.level.push(level::UNKNOWN);
            model.boundaries.vertices.push(Span {
                start: VertexIx::new(start),
                len: 5,
            });
            model.boundaries.segs.push(Span {
                start: SegIx::new(0),
                len: 0,
            });
        }
        model.boundaries.floor_agl = model::bit_zeros(2);
        model.boundaries.ceiling_agl = model::bit_zeros(2);
        model.boundaries.incomplete = model::bit_zeros(2);

        let airway_ident = intern(&mut model, "J80");
        model.airways.designator.push(airway_ident);
        model.airways.location.push(b'J');
        model.airways.legs.push(Span {
            start: AirwayLegIx::new(0),
            len: 1,
        });
        model.airway_legs.from.push(PointIx::new(0));
        model.airway_legs.to.push(PointIx::new(1));
        model.airway_legs.mea_ft.push(model::ALT_NONE);
        model.airway_legs.mea_opp_ft.push(model::ALT_NONE);
        model.airway_legs.gps_mea_ft.push(model::ALT_NONE);
        model.airway_legs.mea_dir.push(0);
        model.airway_legs.gps_mea_dir.push(0);
        model.airway_legs.gap = model::bit_zeros(1);
        model.airway_legs.signal_gap = model::bit_zeros(1);
        model.airway_legs.dogleg = model::bit_zeros(1);
        model
    }

    fn new_surveillance() -> Surveillance {
        surveillance_new(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
        )
        .expect("synthetic surveillance setup is valid")
    }

    fn one_line_bounds(list: &DisplayList, row: usize) -> Option<TextBounds> {
        Some(font::text_block_bounds_px(
            (*list.texts.x.get(row)?, *list.texts.y.get(row)?),
            *list.texts.size.get(row)?,
            *list.texts.align.get(row)?,
            text_at(list, row).chars().count(),
            1,
            u32::from(*list.texts.size.get(row)?).saturating_add(2),
        ))
    }

    // Mutation killed: place mode, MSAW, final-monitor, or safety text on a shared status row.
    // REQ: OA-DSP-078, OA-REG-019
    #[test]
    fn composed_reserved_status_headers_have_disjoint_renderer_bounds() {
        let mut session = new_surveillance();
        surveillance_begin(&mut session, SECOND_NS).expect("begin status frame");
        let frame = surveillance_finish(&mut session, None).expect("finish status frame");
        assert!(
            frame
                .safety_stats
                .expect("reserved safety accounting")
                .banner_lines
                > 0
        );
        assert_eq!(
            session.scope.view.proj.top_inset,
            REGIONAL_STATUS_HEADER_INSET_PX
        );

        let msaw_marker = oa_display::core::alert_markers::AlertMarker {
            address: SPARSE_TEST_ADDRESS,
            lat_e7: oa_display::core::alert_markers::NO_POSITION_E7,
            lon_e7: oa_display::core::alert_markers::NO_POSITION_E7,
            state: oa_display::core::alert_markers::MarkerState::Warning,
            position_known: 0,
            reserved: [0; 2],
        };
        oa_display::core::alert_markers::append_alert_markers(
            &mut session.scope.list,
            &session.scope.view.proj,
            &[msaw_marker],
        )
        .expect("append synthetic MSAW presentation packet");
        let automation = crate::runtime::RegionalAutomationStatus {
            selected: crate::runtime::RegionalAutomationMode::Surveillance,
            effective: crate::runtime::EffectiveAutomationMode::Surveillance,
            degradation: crate::runtime::AutomationDegradation::None,
            fusion_off: true,
        };
        let mode_text = crate::runtime::automation_console_text("", automation);
        let final_marker = oa_display::core::alert_markers::FinalMonitorMarker {
            address: SPARSE_TEST_ADDRESS,
            lat_e7: oa_display::core::alert_markers::NO_POSITION_E7,
            lon_e7: oa_display::core::alert_markers::NO_POSITION_E7,
            severity: oa_display::core::alert_markers::FinalMonitorSeverity::ProtectionLost as u8,
            position_known: 0,
            reserved: [0; 2],
        };
        surveillance_present_controls_with_final_monitor(
            &mut session,
            &oa_display::interaction::interaction_new(),
            &mode_text,
            &[final_marker],
            true,
        )
        .expect("compose mode, both alerts, and compact safety headers");

        let list = &session.scope.list;
        let rows = 0..list.texts.start.len();
        let mode = rows
            .clone()
            .filter(|row| text_at(list, *row) == mode_text)
            .find_map(|row| one_line_bounds(list, row))
            .expect("mode header bounds");
        let final_row = rows
            .clone()
            .find(|row| text_at(list, *row).contains("PROTECTION LOST"))
            .expect("final-monitor protection-lost header remains present");
        let final_bounds = one_line_bounds(list, final_row).expect("final header bounds");
        let final_y = *list.texts.y.get(final_row).expect("final row y");
        let msaw = rows
            .clone()
            .filter(|row| {
                list.texts.y.get(*row).is_some_and(|y| *y < final_y)
                    && list.texts.colour.get(*row) == Some(&oa_display::palette::ALERT)
                    && list.texts.layer.get(*row) == Some(&15)
            })
            .find_map(|row| one_line_bounds(list, row))
            .expect("MSAW summary header bounds");
        let safety: Vec<_> = rows
            .filter(|row| list.texts.y.get(*row).is_some_and(|y| *y > final_y))
            .filter_map(|row| one_line_bounds(list, row))
            .filter(|bounds| {
                bounds.y1 < i32::try_from(REGIONAL_STATUS_HEADER_INSET_PX).unwrap_or(i32::MAX)
            })
            .collect();
        assert!(!safety.is_empty());
        let all_rows: Vec<_> = [mode, msaw, final_bounds]
            .into_iter()
            .chain(safety)
            .collect();
        for (index, first) in all_rows.iter().enumerate() {
            for second in all_rows.iter().skip(index + 1) {
                assert_eq!(
                    overlap_area(*first, *second),
                    0,
                    "status text bounds overlap: {first:?} / {second:?}"
                );
            }
        }
    }

    // Mutation killed: ignoring the selected review map layers when the runtime creates its
    // actual scope scene.
    // REQ: OA-REG-019
    #[test]
    fn review_layer_profiles_change_the_runtime_scene_and_keep_boundaries_and_ifr_airports() {
        let cfg = SurveillanceConfig {
            centre_lat_e7: 410_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 180.0,
        };
        let mut overview_profile = DEFAULT_SURVEILLANCE_PROFILE;
        overview_profile.map_layers = crate::cli::RegionalReviewView::ZauOverview.map_layers();
        overview_profile.ifr_airports_only = true;
        let mut overview = surveillance_new_with_profile(review_map_model(), cfg, overview_profile)
            .expect("synthetic overview runtime");
        let empty_rows = oa_scope_replay::rows::RowStore::default();
        let no_callsigns = oa_scope_replay::rows::Callsigns::default();
        let overview_info = oa_scope_replay::frame::replay_build(
            &mut overview.scope,
            &empty_rows,
            &no_callsigns,
            1_800_000_000_000_000_000,
            "overview",
        )
        .expect("overview display list");
        assert_eq!(overview_info.scene.own_boundaries, 1);
        assert_eq!(overview_info.scene.neighbour_boundaries, 1);
        assert_eq!(overview_info.scene.airports, 1);
        assert_eq!(overview_info.scene.fixes, 0);
        assert_eq!(overview_info.scene.airway_legs, 0);
        assert_eq!(overview_info.scene.rings, 0);

        let mut detailed_profile = DEFAULT_SURVEILLANCE_PROFILE;
        detailed_profile.map_layers = crate::cli::RegionalReviewView::ZauDetailed.map_layers();
        detailed_profile.fixes_to_view_range = true;
        let mut detailed = surveillance_new_with_profile(review_map_model(), cfg, detailed_profile)
            .expect("synthetic detailed runtime");
        let detailed_info = oa_scope_replay::frame::replay_build(
            &mut detailed.scope,
            &empty_rows,
            &no_callsigns,
            1_800_000_000_000_000_000,
            "detailed",
        )
        .expect("detailed display list");
        assert_eq!(detailed_info.scene.own_boundaries, 1);
        assert_eq!(detailed_info.scene.neighbour_boundaries, 1);
        // At 180 NM, the existing 40-NM non-IFR airport threshold still hides 1AAA.
        assert_eq!(detailed_info.scene.airports, 1);
        assert!(detailed_info.scene.fixes > 0);
        assert!(detailed_info.scene.airway_legs > 0);
        assert!(detailed_info.scene.rings > 0);
        assert_eq!(detailed.scope.view.fix_max_nm, 180.0);

        let zoomed_cfg = SurveillanceConfig {
            range_nm: 20.0,
            ..cfg
        };
        let mut zoomed_overview =
            surveillance_new_with_profile(review_map_model(), zoomed_cfg, overview_profile)
                .expect("synthetic zoomed overview runtime");
        let zoomed_overview_info = oa_scope_replay::frame::replay_build(
            &mut zoomed_overview.scope,
            &empty_rows,
            &no_callsigns,
            1_800_000_000_000_000_000,
            "zoomed overview",
        )
        .expect("zoomed overview display list");
        assert_eq!(zoomed_overview_info.scene.airports, 1);
        assert_eq!(zoomed_overview_info.scene.fixes, 0);
        assert_eq!(zoomed_overview_info.scene.airway_legs, 0);
        assert_eq!(zoomed_overview_info.scene.rings, 0);

        let mut zoomed_detailed =
            surveillance_new_with_profile(review_map_model(), zoomed_cfg, detailed_profile)
                .expect("synthetic zoomed detailed runtime");
        let zoomed_detailed_info = oa_scope_replay::frame::replay_build(
            &mut zoomed_detailed.scope,
            &empty_rows,
            &no_callsigns,
            1_800_000_000_000_000_000,
            "zoomed detailed",
        )
        .expect("zoomed detailed display list");
        assert_eq!(zoomed_detailed_info.scene.airports, 2);
        assert!(zoomed_detailed_info.scene.fixes > 0);
        assert!(zoomed_detailed_info.scene.airway_legs > 0);
        assert!(zoomed_detailed_info.scene.rings > 0);

        let mut ord_profile = DEFAULT_SURVEILLANCE_PROFILE;
        ord_profile.map_layers = crate::cli::RegionalReviewView::OrdCentered.map_layers();
        let ord_config = SurveillanceConfig {
            centre_lat_e7: 411_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 60.0,
        };
        let mut ord = surveillance_new_with_profile(review_map_model(), ord_config, ord_profile)
            .expect("synthetic ORD runtime");
        let ord_info = oa_scope_replay::frame::replay_build(
            &mut ord.scope,
            &empty_rows,
            &no_callsigns,
            1_800_000_000_000_000_000,
            "ord",
        )
        .expect("ORD display list");
        // ORD keeps the existing 60-NM view threshold, so only the IFR airport and airway draw.
        assert_eq!(ord_info.scene.airports, 1);
        assert_eq!(ord_info.scene.fixes, 0);
        assert!(ord_info.scene.airway_legs > 0);
        assert_eq!(
            oa_display::core::scene::layers_flags(&ord_profile.map_layers),
            oa_display::core::scene::layers_flags(&oa_display::core::scene::layers_all())
        );
        assert!(!crate::cli::RegionalReviewView::OrdCentered.ifr_airports_only());
    }

    // Mutation killed: apply the fixed one-second TRACK thresholds to an explicitly configured
    // 4.8-second sensor cadence, or silently treat a genuinely lost feed as live.
    // REQ: OA-RWY-015
    #[test]
    fn track_feed_health_scales_only_with_explicit_expected_period() {
        fn track_state(surveillance: &Surveillance, period_ns: i64, time_ns: i64) -> FeedState {
            feed_statuses_with_track_period(surveillance, time_ns, period_ns)
                .expect("bounded TRACK health thresholds")
                .iter()
                .find(|feed| feed.name == *b"TRACK\0\0\0")
                .map(|feed| feed.state)
                .expect("TRACK feed is present")
        }
        let mut surveillance = new_surveillance();
        surveillance.track_measurement_seen_ns = Some(0);
        if let Some(flight_seen) = surveillance.seen.get_mut(1) {
            *flight_seen = Some(0);
        }

        assert_eq!(
            track_state(&surveillance, SECOND_NS, 3 * SECOND_NS),
            FeedState::Slow { age_ms: 3_000 }
        );
        assert_eq!(
            track_state(&surveillance, 4_800_000_000, 3 * SECOND_NS),
            FeedState::Live
        );
        assert_eq!(
            track_state(&surveillance, 4_800_000_000, 10 * SECOND_NS),
            FeedState::Slow { age_ms: 10_000 }
        );
        assert_eq!(
            track_state(&surveillance, 4_800_000_000, 24 * SECOND_NS),
            FeedState::Lost { age_ms: 24_000 }
        );
        let feeds = feed_statuses_with_track_period(&surveillance, 7_800_000_000, 4_800_000_000)
            .expect("bounded feed statuses");
        assert_eq!(
            feeds.get(1).map(|feed| feed.state),
            Some(FeedState::Lost { age_ms: 7_800 })
        );

        surveillance.track_down = true;
        assert_eq!(
            track_state(&surveillance, 4_800_000_000, SECOND_NS),
            FeedState::Lost { age_ms: 1_000 }
        );
        assert!(feed_statuses_with_track_period(&surveillance, SECOND_NS, i64::MAX).is_err());
    }

    // Mutation killed: reuse the last whole-second tick as a non-grid frame, or make its source
    // time equal the frame time instead of preserving the last accepted measurement.
    // REQ: OA-TRK-013, OA-REG-004
    #[test]
    fn non_grid_frame_uses_separate_current_snapshot_and_preserves_source_time() {
        let mut surveillance = new_surveillance();
        let address = 0x00ab_cdef;
        let first_time = 1_000 * SECOND_NS;
        let second_time = first_time + SECOND_NS;
        let frame_time = second_time + SECOND_NS / 5;

        for (time_ns, sequence) in [(first_time, 1), (second_time, 2)] {
            surveillance_begin(&mut surveillance, time_ns).unwrap();
            surveillance_event(
                &mut surveillance,
                &position(time_ns, time_ns, address, sequence),
            )
            .unwrap();
            surveillance_finish(&mut surveillance, None).unwrap();
        }

        surveillance_begin_for_monitor(&mut surveillance, frame_time).unwrap();
        let frame = surveillance_finish(&mut surveillance, None).unwrap();
        let track = frame
            .tracks
            .iter()
            .find(|track| track.address == address)
            .unwrap();
        assert_eq!(track.state_ns, frame_time);
        assert_eq!(track.newest_source_ns, second_time);
        assert!(surveillance.frame_snapshot.t_ns.contains(&frame_time));
        assert!(!surveillance.drain.t_ns.contains(&frame_time));
    }

    // Mutation killed: using fixed legacy dimensions after a review profile is
    // selected, or accepting dimensions beyond the display pixel/stride bounds.
    // REQ: OA-REG-019
    #[test]
    fn review_profile_dimensions_flow_into_replay_frame_spec_and_invalid_sizes_refuse() {
        let mut profile = DEFAULT_SURVEILLANCE_PROFILE;
        profile.frame_width = REVIEW_FRAME_WIDTH;
        profile.frame_height = REVIEW_FRAME_HEIGHT;
        let surveillance = surveillance_new_with_profile(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
            profile,
        )
        .expect("bounded review raster profile");
        let spec = surveillance_frame_spec(&surveillance).expect("replay frame geometry");
        assert_eq!(
            (spec.width, spec.height),
            (REVIEW_FRAME_WIDTH, REVIEW_FRAME_HEIGHT)
        );

        let mut invalid_profile = DEFAULT_SURVEILLANCE_PROFILE;
        invalid_profile.frame_width = u32::MAX;
        assert!(surveillance_new_with_profile(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
            invalid_profile,
        )
        .is_err());
    }

    fn position(receipt_ns: i64, source_ns: i64, address: u32, sequence: u64) -> NormalizedEvent {
        NormalizedEvent {
            receipt_ns,
            source_ns,
            sequence,
            source_message_id: None,
            source: SourceKind::Surveillance,
            identity: crate::input::Identity {
                flight_gufi: 1,
                aircraft_address: address,
            },
            centre: *b"TEST",
            kind: EventKind::Position {
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
                altitude_ft: Some(20_000),
                east_mmps: 10_000,
                north_mmps: 0,
                vertical_rate_fpm: Some(0),
                track_callsign: None,
            },
        }
    }

    fn position_with_callsign(
        receipt_ns: i64,
        address: u32,
        sequence: u64,
        callsign: Option<[u8; 8]>,
    ) -> NormalizedEvent {
        let mut event = position(receipt_ns, receipt_ns, address, sequence);
        if let EventKind::Position { track_callsign, .. } = &mut event.kind {
            *track_callsign = callsign;
        }
        event
    }

    fn finish_with_call_sign_evidence(
        s: &mut Surveillance,
    ) -> Result<Vec<Option<TrackCallsignEvidence>>, String> {
        let mut evidence = Vec::with_capacity(s.profile.max_tracks);
        let mut selector = |track: &ObservedTrack, source: Option<TrackCallsignEvidence>| {
            evidence.push(source);
            Ok(oa_display::core::traffic::TrafficBlockPolicy {
                address: track.address,
                detail: oa_display::core::traffic::TrafficBlockDetail::AltitudeOnly,
                flight_callsign: None,
            })
        };
        surveillance_finish_with_traffic_policy_and_track_period(
            s,
            None,
            true,
            None,
            &mut selector,
            SECOND_NS,
        )?;
        Ok(evidence)
    }

    fn run_sparse_aircraft_profile(
        profile: SurveillanceProfile,
    ) -> Result<SurveillanceFrame, String> {
        let base_ns = 1_893_456_780_000_000_000i64;
        let mut surveillance = surveillance_new_with_profile(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
            profile,
        )?;
        let mut latest = None;
        for tick in 0..=42i64 {
            let cutoff = base_ns + tick * SECOND_NS;
            surveillance_begin(&mut surveillance, cutoff)?;
            if [0, 4, 8].contains(&tick) {
                let sequence =
                    u64::try_from(tick).map_err(|_| "fixture tick exceeds sequence width")?;
                let mut event = position(cutoff, cutoff, SPARSE_TEST_ADDRESS, sequence);
                if let EventKind::Position {
                    east_mmps,
                    north_mmps,
                    ..
                } = &mut event.kind
                {
                    *east_mmps = 0;
                    *north_mmps = 0;
                }
                surveillance_event(&mut surveillance, &event)?;
            }
            latest = Some(surveillance_finish(&mut surveillance, None)?);
        }
        latest.ok_or_else(|| "sparse profile produced no surveillance frame".to_owned())
    }

    // REQ: OA-REG-013
    #[test]
    fn archived_timing_confirms_and_coasts_sparse_reports() -> Result<(), String> {
        let live_style = run_sparse_aircraft_profile(DEFAULT_SURVEILLANCE_PROFILE)?;
        let mut archived_profile = DEFAULT_SURVEILLANCE_PROFILE;
        archived_profile.tracker_timing = TrackerTimingProfile::ArchivedFixed10_30_120V1;
        let archived = run_sparse_aircraft_profile(archived_profile)?;

        assert!(
            archived.tracker_stats.confirmed > live_style.tracker_stats.confirmed,
            "archived timing did not confirm the three sparse reports: archive={:?}, default={:?}",
            archived.tracker_stats,
            live_style.tracker_stats
        );
        assert!(archived
            .tracks
            .iter()
            .zip(&archived.flags)
            .any(|(track, flags)| {
                track.address == SPARSE_TEST_ADDRESS && flags & O_COAST == O_COAST
            }));
        Ok(())
    }

    // REQ: OA-REG-013
    #[test]
    fn archived_tracker_profile_uses_its_declared_drop_interval() -> Result<(), String> {
        let mut profile = DEFAULT_SURVEILLANCE_PROFILE;
        profile.tracker_timing = TrackerTimingProfile::ArchivedFixed10_30_120V1;
        let s = surveillance_new_with_profile(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
            profile,
        )?;
        assert_eq!(tracker_drop_after_ns(&s), RECORDED_HOUR_DROP_AFTER_NS);
        Ok(())
    }

    // Kills mutation: a report at the render cutoff defers the exact tracker tick, so the shell
    // exports only that report's row and omits other confirmed live tracks.
    // REQ: OA-REG-013
    #[test]
    fn archived_tick_cutoff_keeps_all_live_tracks_with_jittered_sources() -> Result<(), String> {
        let addresses = [0x123451, 0x123452, 0x123453];
        let base_ns = 1_893_456_780_000_000_000i64;
        let mut profile = DEFAULT_SURVEILLANCE_PROFILE;
        profile.max_tracks = addresses.len();
        profile.tracker_timing = TrackerTimingProfile::ArchivedFixed10_30_120V1;
        let mut s = surveillance_new_with_profile(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
            profile,
        )?;

        for tick in 0..=40i64 {
            let receipt_ns = base_ns + (tick + 1) * SECOND_NS;
            surveillance_begin(&mut s, receipt_ns)?;
            for (index, address) in addresses.iter().copied().enumerate() {
                let jitter_ns = i64::try_from(addresses.len() - index - 1)
                    .map_err(|_| "jitter index width")?
                    * 100_000_000;
                let sequence = u64::try_from(tick)
                    .map_err(|_| "tick sequence width")?
                    .checked_mul(u64::try_from(addresses.len()).map_err(|_| "address count width")?)
                    .and_then(|value| value.checked_add(u64::try_from(index).ok()?))
                    .ok_or("position sequence overflow")?;
                let event = position(receipt_ns, receipt_ns - jitter_ns, address, sequence);
                surveillance_event(&mut s, &event)?;
            }
            let frame = surveillance_finish(&mut s, None)?;
            if tick >= 32 {
                assert_eq!(frame.tracks.len(), addresses.len());
                for address in addresses {
                    assert!(frame.tracks.iter().any(|track| track.address == address));
                }
                assert!(frame.tracks.iter().all(|track| {
                    frame
                        .time_ns
                        .checked_sub(track.newest_source_ns)
                        .is_some_and(|age| (0..RECORDED_HOUR_DROP_AFTER_NS).contains(&age))
                }));
            }
        }
        Ok(())
    }

    // Kills per-frame callsign-proof clearing and verifies coast retention plus proof clearing
    // after a changed or missing accepted callsign.
    // REQ: OA-REG-002
    #[test]
    fn callsign_proof_survives_coast_and_missing_report_clears_it() -> Result<(), String> {
        let mut profile = DEFAULT_SURVEILLANCE_PROFILE;
        profile.max_tracks = 1;
        let mut s = surveillance_new_with_profile(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
            profile,
        )?;
        let address = 0x123456;
        let callsign = *b"N123AB\0\0";
        for receipt in [SECOND_NS, 2 * SECOND_NS] {
            surveillance_begin(&mut s, receipt)?;
            surveillance_event(
                &mut s,
                &position_with_callsign(
                    receipt,
                    address,
                    u64::try_from(receipt).unwrap(),
                    Some(callsign),
                ),
            )?;
            finish_with_call_sign_evidence(&mut s)?;
        }
        assert!(s.track_callsigns.contains_key(&address));

        let coast_receipt = 3 * SECOND_NS;
        surveillance_begin(&mut s, coast_receipt)?;
        let coast_evidence = finish_with_call_sign_evidence(&mut s)?;
        assert!(coast_evidence.iter().flatten().any(|evidence| {
            evidence.callsign == callsign && evidence.source_ns == 2 * SECOND_NS
        }));

        let missing_receipt = 4 * SECOND_NS;
        surveillance_begin(&mut s, missing_receipt)?;
        surveillance_event(
            &mut s,
            &position_with_callsign(missing_receipt, address, 4, Some(*b"N999ZZ\0\0")),
        )?;
        let changed_evidence = finish_with_call_sign_evidence(&mut s)?;
        assert!(changed_evidence.iter().all(Option::is_none));
        assert!(!s.track_callsigns.contains_key(&address));

        let reobserved_receipt = 5 * SECOND_NS;
        surveillance_begin(&mut s, reobserved_receipt)?;
        surveillance_event(
            &mut s,
            &position_with_callsign(reobserved_receipt, address, 5, Some(*b"N999ZZ\0\0")),
        )?;
        let reobserved = finish_with_call_sign_evidence(&mut s)?;
        assert!(reobserved.iter().flatten().any(|evidence| {
            evidence.callsign == *b"N999ZZ\0\0" && evidence.source_ns == reobserved_receipt
        }));

        let missing_receipt = 6 * SECOND_NS;
        surveillance_begin(&mut s, missing_receipt)?;
        surveillance_event(
            &mut s,
            &position_with_callsign(missing_receipt, address, 6, None),
        )?;
        let missing_evidence = finish_with_call_sign_evidence(&mut s)?;
        assert!(missing_evidence.iter().all(Option::is_none));
        assert!(!s.track_callsigns.contains_key(&address));
        Ok(())
    }

    // Kills a non-reclaiming fixed callsign table: a one-track profile expires the old entry,
    // frees its address, and admits fresh evidence for a replacement track.
    // REQ: OA-REG-002
    #[test]
    fn callsign_proof_expires_and_refills_at_selected_track_capacity() -> Result<(), String> {
        let mut profile = DEFAULT_SURVEILLANCE_PROFILE;
        profile.max_tracks = 1;
        let mut s = surveillance_new_with_profile(
            test_model(),
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 100.0,
            },
            profile,
        )?;
        let first_address = 0x123456;
        let replacement_address = 0x123457;
        let callsign = *b"N123AB\0\0";
        for receipt in [SECOND_NS, 2 * SECOND_NS] {
            surveillance_begin(&mut s, receipt)?;
            surveillance_event(
                &mut s,
                &position_with_callsign(
                    receipt,
                    first_address,
                    u64::try_from(receipt).unwrap(),
                    Some(callsign),
                ),
            )?;
            finish_with_call_sign_evidence(&mut s)?;
        }
        assert_eq!(s.track_callsigns.len(), profile.max_tracks);

        let last_source = 2 * SECOND_NS;
        let at_expiry = last_source + s.tracker_drop_after_ns;
        for receipt in [at_expiry, at_expiry + SECOND_NS] {
            surveillance_begin(&mut s, receipt)?;
            finish_with_call_sign_evidence(&mut s)?;
        }
        assert!(!s.track_callsigns.contains_key(&first_address));
        assert!(s.track_callsigns.is_empty());

        for receipt in [
            at_expiry + 2 * SECOND_NS,
            at_expiry + 3 * SECOND_NS,
            at_expiry + 4 * SECOND_NS,
        ] {
            surveillance_begin(&mut s, receipt)?;
            surveillance_event(
                &mut s,
                &position_with_callsign(
                    receipt,
                    replacement_address,
                    u64::try_from(receipt).unwrap(),
                    Some(callsign),
                ),
            )?;
            finish_with_call_sign_evidence(&mut s)?;
        }
        assert_eq!(s.track_callsigns.len(), profile.max_tracks);
        assert!(s.track_callsigns.contains_key(&replacement_address));
        Ok(())
    }

    // REQ: OA-REG-018
    #[test]
    fn barometric_evidence_capacity_refuses_before_tracker_or_health_mutation() {
        let mut s = new_surveillance();
        surveillance_begin(&mut s, SECOND_NS).expect("begin frame before capacity check");
        for address in 1..=u32::try_from(DEFAULT_SURVEILLANCE_PROFILE.max_baro_evidence).unwrap() {
            s.baro_seen.insert(address, SECOND_NS);
        }
        let event = position(SECOND_NS, SECOND_NS, 0x00ab_cdef, 1);
        let seen_before = s.seen;
        let down_before = s.down;
        let tracker_before = *tracker_stats(&s.tracker);
        let newest_before = tracker_newest_update_ns(&s.tracker);
        let map_len_before = s.baro_seen.len();

        assert_eq!(
            surveillance_stage_event(&s, &event),
            Err(SurveillanceStageError::BarometricEvidenceCapacity)
        );
        assert!(surveillance_event(&mut s, &event).is_err());
        assert_eq!(s.seen, seen_before);
        assert_eq!(s.down, down_before);
        assert_eq!(*tracker_stats(&s.tracker), tracker_before);
        assert_eq!(tracker_newest_update_ns(&s.tracker), newest_before);
        assert_eq!(s.baro_seen.len(), map_len_before);
    }

    #[test]
    fn wrong_source_and_negative_source_time_do_not_change_feed_health() {
        let mut s = new_surveillance();
        surveillance_begin(&mut s, SECOND_NS).expect("begin first frame");
        let mut wrong_source = position(SECOND_NS, SECOND_NS, 0x123456, 1);
        wrong_source.source = SourceKind::Route;
        assert!(surveillance_event(&mut s, &wrong_source).is_err());
        let negative_source = position(SECOND_NS, -1, 0x123456, 2);
        assert!(surveillance_event(&mut s, &negative_source).is_err());
        assert_eq!(s.seen, [None; 5]);
        assert_eq!(s.track_measurement_seen_ns, None);
    }

    #[test]
    fn invalid_position_does_not_refresh_feed_or_tracker_health() {
        let mut s = new_surveillance();
        surveillance_begin(&mut s, SECOND_NS).expect("begin first frame");
        let bad_address = position(SECOND_NS, SECOND_NS, u32::MAX, 1);
        assert_eq!(
            surveillance_stage_event(&s, &bad_address),
            Err(SurveillanceStageError::InvalidPosition)
        );
        assert!(surveillance_event(&mut s, &bad_address).is_err());
        assert_eq!(s.seen[0], None);
        assert_eq!(s.track_measurement_seen_ns, None);
        assert_eq!(s.measurement_rejections, 0);
    }

    #[test]
    fn failed_finish_poison_blocks_new_events_retries_and_pixels() {
        let mut s = new_surveillance();
        surveillance_begin(&mut s, SECOND_NS).expect("begin first frame");
        s.drain.overflow = 1;
        assert!(surveillance_finish(&mut s, None).is_err());
        assert!(surveillance_event(&mut s, &position(SECOND_NS, SECOND_NS, 0x123456, 1)).is_err());
        assert!(surveillance_finish(&mut s, None).is_err());
        assert!(surveillance_begin(&mut s, 2 * SECOND_NS).is_err());
        assert!(surveillance_pixels(&s).is_err());
    }

    #[test]
    fn duplicate_same_tick_rows_replace_by_last_index_and_are_countable() {
        let mut latest = BTreeMap::new();
        assert!(!replace_latest_row(&mut latest, 0x123456, 3, MAX_TRACKS).expect("first address"));
        assert!(replace_latest_row(&mut latest, 0x123456, 7, MAX_TRACKS).expect("replacement"));
        assert_eq!(latest.get(&0x123456), Some(&7));
    }

    // REQ: OA-REG-004
    #[test]
    fn finished_ca_episodes_and_altitude_loss_are_captured_once_per_cycle() {
        let mut s = new_surveillance();
        let track = |addr, alt_ft| CaTrack {
            lat_deg: 41.0,
            lon_deg: -87.0,
            addr,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            r95_m: 0.0,
            alt_ft,
            vrate_fpm: 0.0,
            flags: 0,
            ground: AIRBORNE,
        };
        let rows = [track(0x123456, 10_000.0), track(0x123457, 10_000.0)];
        ca_cycle(
            &mut s.ca,
            SECOND_NS,
            &rows,
            &mut s.alerts,
            &mut s.events,
            &mut s.states,
        );
        assert_eq!(
            ca_open(&s.ca).len(),
            1,
            "coincident level aircraft conflict"
        );
        assert_eq!(capture_ca_logs(&mut s), (vec![], vec![]));
        let rows = [track(0x123456, f32::NAN), track(0x123457, 10_000.0)];
        let cycle = ca_cycle(
            &mut s.ca,
            2 * SECOND_NS,
            &rows,
            &mut s.alerts,
            &mut s.events,
            &mut s.states,
        );
        assert_eq!(cycle.stats.alt_lost, 1);
        let (closed, lost) = capture_ca_logs(&mut s);
        assert!(closed.is_empty());
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].addr, 0x123456);
        assert_eq!(lost[0].t_ns, 2 * SECOND_NS);
        let cycle = ca_cycle(
            &mut s.ca,
            5 * SECOND_NS,
            &[],
            &mut s.alerts,
            &mut s.events,
            &mut s.states,
        );
        assert_eq!(cycle.stats.closed, 1);
        let (closed, lost) = capture_ca_logs(&mut s);
        assert_eq!(closed.len(), 1);
        assert_eq!((closed[0].addr_a, closed[0].addr_b), (0x123456, 0x123457));
        assert_eq!(closed[0].start_ns, SECOND_NS);
        assert!(lost.is_empty());
        ca_cycle(
            &mut s.ca,
            6 * SECOND_NS,
            &[],
            &mut s.alerts,
            &mut s.events,
            &mut s.states,
        );
        assert_eq!(capture_ca_logs(&mut s), (vec![], vec![]));
        assert_eq!(s.alerts.capacity, MAX_PAIRS);
        assert_eq!(s.events.capacity, MAX_TRACKS);
    }

    #[test]
    fn withheld_ca_product_retains_suppressed_open_or_closed_alert_state() {
        let mut s = new_surveillance();
        let addresses = [0x123456, 0x123457, 0x123458, 0x123459];
        surveillance_begin(&mut s, SECOND_NS).expect("begin first surveillance scan");
        for address in addresses {
            let mut event = position(SECOND_NS, SECOND_NS, address, 1);
            event.identity.flight_gufi = u128::from(address);
            surveillance_event(&mut s, &event).expect("first scan report is admitted");
        }
        surveillance_finish(&mut s, None).expect("first scan completes");

        let receipt_ns = 2 * SECOND_NS;
        surveillance_begin(&mut s, receipt_ns).expect("begin confirming surveillance scan");
        let mut policy = oa_safety::inhibit::inhibit_config_none();
        policy.max_alert_suppression_ns = Some(5 * SECOND_NS);
        oa_safety::inhibit::pair_suppress_create(
            &mut s.inhibits,
            &policy,
            &oa_safety::inhibit::PairSuppressRequest {
                addr_a: 0x123456,
                addr_b: 0x123457,
                alert_start_ns: receipt_ns,
                start_ns: receipt_ns,
                end_ns: 5 * SECOND_NS,
                text: oa_safety::inhibit::AlertControlText {
                    reason: "SECTOR COORDINATION",
                    author: "SUP A",
                },
            },
            receipt_ns,
        )
        .expect("explicit pair suppression is admitted before the scan");
        let control_id = oa_safety::inhibit::pair_suppressions(&s.inhibits)
            .last()
            .map(|control| control.id)
            .expect("accepted pair control is retained");
        for address in addresses {
            let mut event = position(receipt_ns, receipt_ns, address, 2);
            event.identity.flight_gufi = u128::from(address);
            surveillance_event(&mut s, &event).expect("confirming scan report is admitted");
        }
        let frame = surveillance_finish_with_visibility(
            &mut s,
            None,
            false,
            Some(ProductUnavailableReason::OperatorSelectedFlightDataOnly),
        )
        .expect("withheld product carries actual surveillance state");
        assert_eq!(
            frame.ca_product,
            ProductAvailability::Withheld(ProductUnavailableReason::OperatorSelectedFlightDataOnly)
        );
        let suppressed_pair = frame
            .ca_suppressed
            .iter()
            .find(|audit| {
                (audit.addr_a, audit.addr_b, audit.alert_start_ns)
                    == (0x123456, 0x123457, receipt_ns)
            })
            .expect("withheld frame retains the suppressed pair audit");
        assert_eq!(suppressed_pair.inhibition_id, control_id);
        assert!(frame
            .ca_open
            .iter()
            .all(|alert| (alert.addr_a, alert.addr_b)
                != (suppressed_pair.addr_a, suppressed_pair.addr_b)));
        assert!(frame.ca_open.iter().any(|alert| {
            (alert.addr_a, alert.addr_b) == (0x123458, 0x123459)
                && alert.flags & oa_ca::AF_SUPPRESSED == 0
        }));
        let accounted = frame
            .ca_open
            .len()
            .checked_add(frame.ca_closed.len())
            .and_then(|count| count.checked_add(frame.ca_altitude_lost.len()))
            .and_then(|count| count.checked_add(frame.ca_suppressed.len()))
            .expect("withheld alert count adds without overflow");
        assert_eq!(usize::try_from(frame.withheld_ca_records), Ok(accounted));
    }

    #[test]
    fn withheld_ca_frame_exports_closed_and_altitude_lost_alert_details() {
        let mut s = new_surveillance();
        let track = |addr, alt_ft| CaTrack {
            lat_deg: 41.0,
            lon_deg: -87.0,
            addr,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            r95_m: 0.0,
            alt_ft,
            vrate_fpm: 0.0,
            flags: 0,
            ground: AIRBORNE,
        };
        let initial = [track(0x123456, 10_000.0), track(0x123457, 10_000.0)];
        ca_cycle(
            &mut s.ca,
            SECOND_NS,
            &initial,
            &mut s.alerts,
            &mut s.events,
            &mut s.states,
        );
        let altitude_loss = [track(0x123456, f32::NAN), track(0x123457, 10_000.0)];
        let loss = ca_cycle(
            &mut s.ca,
            2 * SECOND_NS,
            &altitude_loss,
            &mut s.alerts,
            &mut s.events,
            &mut s.states,
        );
        assert_eq!(loss.stats.alt_lost, 1);

        surveillance_begin(&mut s, 5 * SECOND_NS).expect("begin outage frame");
        let frame = surveillance_finish_with_visibility(
            &mut s,
            None,
            false,
            Some(ProductUnavailableReason::SurveillanceFeedUnavailable),
        )
        .expect("outage frame retains closed and altitude-loss details");
        assert_eq!(
            frame.ca_product,
            ProductAvailability::Withheld(ProductUnavailableReason::SurveillanceFeedUnavailable)
        );
        assert!(frame
            .ca_closed
            .iter()
            .any(|alert| alert.start_ns == SECOND_NS));
        assert!(frame
            .ca_altitude_lost
            .iter()
            .any(|event| event.addr == 0x123456 && event.t_ns == 2 * SECOND_NS));
        assert!(frame.withheld_ca_records >= 2);
    }

    #[test]
    fn dense_airport_traffic_blocks_reposition_without_losing_targets() {
        use oa_display::core::list::{list_new, push_text, ListCaps};
        use oa_display::core::project::projection_new;
        use oa_display::core::raster::{frame_len, rasterize, FrameSpec};
        use oa_display::core::traffic::caps_add;
        use oa_display::core::traffic::{
            build_traffic, targets_new, targets_push, traffic_caps, Target, IDENT_UNKNOWN,
            NO_IDENT, NO_REACQ, NO_U16,
        };
        let projection = projection_new(410_000_000, -870_000_000, 180.0, 800, 600, 104)
            .expect("synthetic 180 NM view");
        let mut targets = targets_new(2).expect("two target slots");
        for (address, latitude, callsign) in [
            (0x123456, 410_000_000, *b"UAL001\0\0"),
            (0x123457, 410_000_100, *b"UAL002\0\0"),
        ] {
            targets_push(
                &mut targets,
                &Target {
                    lat_e7: latitude,
                    lon_e7: -870_000_000,
                    alt_ft: 20_000,
                    addr: address,
                    ident_age_s: u32::MAX,
                    r95_m: f32::NAN,
                    trail: [(0, 0); oa_display::core::traffic::MAX_TRAIL],
                    callsign,
                    track_ddeg: NO_U16,
                    gs_dkt: NO_U16,
                    reacq_age_s: NO_REACQ,
                    ident: NO_IDENT,
                    flags: 0,
                    trail_len: 0,
                    ident_state: IDENT_UNKNOWN,
                },
            )
            .expect("well-formed target");
        }
        let style = DEFAULT_STYLE;
        let caps = caps_add(
            &traffic_caps(2, &style).expect("traffic list capacity"),
            &ListCaps {
                lines: 0,
                rects: 0,
                circles: 0,
                polylines: 0,
                poly_vertices: 0,
                texts: 1,
                text_bytes: 4,
            },
        )
        .expect("map label capacity");
        let mut list = list_new(caps).expect("bounded display list");
        let ellipsoid = oa_geo::wgs84();
        build_traffic(&mut list, &projection, &ellipsoid, &targets, &style)
            .expect("initial traffic render");
        let centers = [
            project(&projection, 410_000_000, -870_000_000).expect("first target projects"),
            project(&projection, 410_000_100, -870_000_000).expect("second target projects"),
        ];
        let first_before = block_bounds(&list, 0, 2).expect("first measured block");
        let second_before = block_bounds(&list, 2, 4).expect("second measured block");
        assert!(overlap_area(first_before, second_before) > 0);
        let map_label_row = list.texts.x.len();
        let label_x = centers[0].0.saturating_add(60 * 16 + 3 * 16);
        let label_y = centers[0]
            .1
            .saturating_sub(60 * 16 + (first_before.y1 - first_before.y0) * 16);
        push_text(
            &mut list,
            (label_x, label_y),
            10,
            0,
            oa_display::core::palette::TEXT,
            LAYER_TEXT,
            "FIXA",
        )
        .expect("synthetic nearby airport-area fix label");
        let spec = FrameSpec {
            width: 800,
            height: 600,
            stride: 3200,
        };
        let text_rows_before = list.texts.x.len();
        let mut before = vec![0; frame_len(&spec)];
        rasterize(&list, &spec, &mut before).expect("baseline synthetic raster");

        declutter_scope_blocks(&mut list, &projection, &targets_columns(&targets))
            .expect("layout all visible traffic blocks");
        let first_after = block_bounds(&list, 0, 2).expect("first block remains");
        let second_after = block_bounds(&list, 2, 4).expect("second block remains");
        assert_eq!(overlap_area(first_after, second_after), 0);
        assert_eq!(
            list.texts.len.get(map_label_row),
            Some(&0),
            "mandatory traffic makes the conflicting map label yield"
        );
        assert_eq!(
            list.texts.x.len(),
            text_rows_before,
            "both targets and the map label remain accounted for"
        );
        let mut after = vec![0; frame_len(&spec)];
        rasterize(&list, &spec, &mut after).expect("decluttered synthetic raster");
        assert_ne!(
            before, after,
            "placement changes the raster while retaining traffic rows"
        );
    }

    fn ca_overlay_labels(s: &Surveillance) -> usize {
        s.scope
            .list
            .texts
            .start
            .iter()
            .zip(&s.scope.list.texts.len)
            .filter(|(start, len)| {
                let Ok(start) = usize::try_from(**start) else {
                    return false;
                };
                let Ok(len) = usize::try_from(**len) else {
                    return false;
                };
                start
                    .checked_add(len)
                    .and_then(|end| s.scope.list.text.get(start..end))
                    == Some("CA")
            })
            .count()
    }

    fn ca_overlay_connectors(s: &Surveillance) -> usize {
        s.scope
            .list
            .lines
            .layer
            .iter()
            .zip(&s.scope.list.lines.colour)
            .filter(|(layer, colour)| {
                **layer == CA_CONNECTOR_LAYER && **colour == oa_display::core::palette::ALERT
            })
            .count()
    }

    // REQ: OA-SAF-021, OA-SAF-023, OA-CA-035
    #[test]
    fn visible_scan_frame_filters_only_the_suppressed_alert_at_the_presenter() {
        let mut s = new_surveillance();
        surveillance_begin(&mut s, SECOND_NS).expect("begin visible scan frame");
        for address in [0x123456, 0x123457, 0x123458, 0x123459] {
            let mut event = position(SECOND_NS, SECOND_NS, address, 1);
            event.identity.flight_gufi = u128::from(address);
            surveillance_event(&mut s, &event).expect("first normal scan input is admitted");
        }
        let first = surveillance_finish(&mut s, None).expect("first visible frame is produced");
        assert!(!first.surveillance_withheld);

        surveillance_begin(&mut s, 2 * SECOND_NS).expect("begin confirming scan frame");
        let mut policy = oa_safety::inhibit::inhibit_config_none();
        policy.max_alert_suppression_ns = Some(5 * SECOND_NS);
        oa_safety::inhibit::pair_suppress_create(
            &mut s.inhibits,
            &policy,
            &oa_safety::inhibit::PairSuppressRequest {
                addr_a: 0x123456,
                addr_b: 0x123457,
                alert_start_ns: 2 * SECOND_NS,
                start_ns: 2 * SECOND_NS,
                end_ns: 5 * SECOND_NS,
                text: oa_safety::inhibit::AlertControlText {
                    reason: "SECTOR COORDINATION",
                    author: "SUP A",
                },
            },
            2 * SECOND_NS,
        )
        .expect("pair control is admitted before the scan");
        for address in [0x123456, 0x123457, 0x123458, 0x123459] {
            let mut event = position(2 * SECOND_NS, 2 * SECOND_NS, address, 2);
            event.identity.flight_gufi = u128::from(address);
            surveillance_event(&mut s, &event).expect("confirming scan input is admitted");
        }

        let frame = surveillance_finish(&mut s, None).expect("visible frame is produced");
        assert!(!frame.surveillance_withheld);
        let overlay = frame
            .ca_overlay
            .expect("rendered frame reports CA overlays");
        assert_eq!(overlay.pairs_examined as usize, frame.ca_open.len());
        assert_eq!(overlay.pairs_drawn as usize, frame.ca_open.len());
        assert_eq!(overlay.pairs_offscreen, 0);
        assert_eq!(overlay.pairs_unlocated, 0);
        assert_eq!(ca_overlay_connectors(&s), frame.ca_open.len());
        assert_eq!(ca_overlay_labels(&s), frame.ca_open.len());
        let alert_colour = oa_display::core::palette::bytes_of(oa_display::core::palette::ALERT)
            .expect("alert palette entry exists");
        assert!(s
            .scope
            .buf
            .chunks_exact(4)
            .any(|pixel| pixel == alert_colour.as_slice()));
        assert!(frame
            .ca_open
            .iter()
            .any(|alert| (alert.addr_a, alert.addr_b) == (0x123458, 0x123459)));
        assert!(frame.ca_open.iter().all(|alert| {
            (alert.addr_a, alert.addr_b) != (0x123456, 0x123457)
                && alert.flags & oa_ca::AF_SUPPRESSED == 0
        }));
        assert!(frame.ca_suppressed.iter().any(|receipt| {
            (receipt.addr_a, receipt.addr_b, receipt.alert_start_ns)
                == (0x123456, 0x123457, 2 * SECOND_NS)
        }));
        assert!(frame.tracks.iter().any(|track| track.address == 0x123456));
        assert!(frame.tracks.iter().any(|track| track.address == 0x123457));
        assert!(frame.displayed > 0);
        let target_colour = oa_display::core::palette::bytes_of(oa_display::core::palette::TARGET)
            .expect("target palette entry exists");
        assert!(s
            .scope
            .buf
            .chunks_exact(4)
            .any(|pixel| pixel == target_colour.as_slice()));

        let suppressed_start = ca_open(&s.ca)
            .iter()
            .find(|alert| (alert.addr_a, alert.addr_b) == (0x123456, 0x123457))
            .map(|alert| alert.start_ns)
            .expect("suppression retains the current CA episode");
        surveillance_begin(&mut s, 5 * SECOND_NS).expect("begin post-expiry scan");
        for address in [0x123456, 0x123457, 0x123458, 0x123459] {
            let mut event = position(5 * SECOND_NS, 5 * SECOND_NS, address, 3);
            event.identity.flight_gufi = u128::from(address);
            surveillance_event(&mut s, &event).expect("post-expiry position is admitted");
        }
        let revealed = surveillance_finish(&mut s, None).expect("expired alert is presented");
        let reopened = revealed
            .ca_open
            .iter()
            .find(|alert| (alert.addr_a, alert.addr_b) == (0x123456, 0x123457))
            .expect("the pair connector returns after control expiry");
        assert_eq!(reopened.start_ns, suppressed_start);
        assert_eq!(reopened.flags & oa_ca::AF_SUPPRESSED, 0);
        let overlay = revealed
            .ca_overlay
            .expect("post-expiry raster reports CA overlay");
        assert_eq!(overlay.pairs_drawn as usize, revealed.ca_open.len());
        assert_eq!(ca_overlay_connectors(&s), revealed.ca_open.len());
        assert_eq!(ca_overlay_labels(&s), revealed.ca_open.len());
    }

    // REQ: OA-CA-035
    #[test]
    fn ca_pair_overlay_counts_unlocated_offscreen_and_clipped_current_pairs() {
        let mut s = new_surveillance();
        let mut current_tracks = Vec::new();
        for time in [SECOND_NS, 2 * SECOND_NS] {
            surveillance_begin(&mut s, time).expect("begin normal pair scan");
            for address in [0x123456, 0x123457] {
                let mut event = position(time, time, address, u64::try_from(time).unwrap());
                event.identity.flight_gufi = u128::from(address);
                surveillance_event(&mut s, &event).expect("normal position is admitted");
            }
            let frame = surveillance_finish(&mut s, None).expect("normal pair frame is rendered");
            if time == 2 * SECOND_NS {
                assert!(!frame.ca_open.is_empty());
                current_tracks = frame.tracks;
            }
        }
        let alert = ca_open(&s.ca)
            .first()
            .copied()
            .expect("the current tracker pair raises CA");

        oa_display::list::list_clear(&mut s.scope.list);
        let unlocated = render_ca_overlay(
            &mut s.scope.list,
            &s.scope.view.proj,
            std::slice::from_ref(&alert),
            &[],
        )
        .expect("a missing current track is counted");
        assert_eq!(unlocated.pairs_unlocated, 1);
        assert_eq!(unlocated.pairs_drawn, 0);

        let mut endpoints: Vec<_> = current_tracks
            .iter()
            .copied()
            .filter(|track| track.address == alert.addr_a || track.address == alert.addr_b)
            .collect();
        assert_eq!(endpoints.len(), 2);
        for track in &mut endpoints {
            track.lon_e7 = -700_000_000;
        }
        oa_display::list::list_clear(&mut s.scope.list);
        let offscreen = render_ca_overlay(
            &mut s.scope.list,
            &s.scope.view.proj,
            std::slice::from_ref(&alert),
            &endpoints,
        )
        .expect("two projected endpoints outside the view are counted");
        assert_eq!(offscreen.pairs_offscreen, 1);
        assert_eq!(offscreen.pairs_drawn, 0);

        endpoints[1].lon_e7 = -1_040_000_000;
        oa_display::list::list_clear(&mut s.scope.list);
        let clipped = render_ca_overlay(
            &mut s.scope.list,
            &s.scope.view.proj,
            std::slice::from_ref(&alert),
            &endpoints,
        )
        .expect("a connector crossing the frame remains drawable");
        assert_eq!(clipped.pairs_drawn, 1);
        replay_raster(&mut s.scope).expect("the clipped connector reaches the raster");
        let alert_colour = oa_display::core::palette::bytes_of(oa_display::core::palette::ALERT)
            .expect("alert palette entry exists");
        assert!(s
            .scope
            .buf
            .chunks_exact(4)
            .any(|pixel| pixel == alert_colour.as_slice()));
    }
}

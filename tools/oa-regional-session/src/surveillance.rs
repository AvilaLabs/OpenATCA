//! Offline regional surveillance composition (**shell**, B1/B3/C6). Real tracker output at
//! the issue tick feeds both conflict alert and scope; intended routes never suppress CA.

use crate::input::{AdsbSourceEvidence, EventKind, NormalizedEvent, SourceKind};
use oa_airspace::core::model::Model;
use oa_ca::{
    alert_log_clear, alert_log_new, ca_cycle, ca_new, ca_open, event_log_clear, event_log_new,
    Area, CaConfig, CaTrack, Volume, AIRBORNE, F_COAST, F_IDENTITY_UNCONFIRMED, F_UNCERTAIN,
    MAX_VOLUMES,
};
use oa_display::core::ownership::OwnershipStatus;
use oa_display::core::raster::frame_digest;
use oa_display::core::safety::{FeedState, FeedStatus};
use oa_display::core::traffic::{targets_columns, DEFAULT_STYLE};
use oa_safety::inhibit::{inhibits_new, InhibitTable};
use oa_safety::net::{state_table_new, StateTable};
use oa_scope_replay::adapt::{row_columns, ColumnBuf};
use oa_scope_replay::frame::{
    replay_build_with_mode_overlays, replay_new_with_top_inset, replay_raster, Replay,
    ReplayConfig, ReplaySafety,
};
use oa_scope_replay::rows::{Callsigns, RowStore, DEFAULT_CONVERT};
use oa_scope_replay::store::{store_add, store_finish};
use oa_track::surveillance_mode::{
    surveillance_mode_result, IsrEvidence, ModeAdaptation, ModeResult, ReliabilityEvidence,
    TargetModeEvidence, TargetModeState, TargetSource,
};
use oa_track::{
    output_clear, output_new, r95_m_of, tracker_advance, tracker_live_iter, tracker_new,
    tracker_newest_update_ns, tracker_stats, tracker_update, Measurement, OutRow, TrackOutput,
    Tracker, TrackerStats, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_IDENT, O_COAST, O_HAS_ALT,
    O_IDENTITY_UNCONFIRMED, O_UNCERTAIN,
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
pub const FRAME_WIDTH: u32 = 800;
pub const FRAME_HEIGHT: u32 = 600;
/// C7: control redraws over the entire offline session, including ordinary completed frames.
pub const MAX_PRESENTATION_UPDATES: usize = 8192;
const MAX_PAIRS: usize = MAX_TRACKS * (MAX_TRACKS - 1) / 2;
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
    first_measurement_ns: Option<i64>,
    measurement_rejections: u64,
    previous_ns: Option<i64>,
    frames: usize,
    completed: bool,
    failed: bool,
    overlay_base: Option<OverlayBase>,
    presentation_updates: usize,
    msaw: Option<crate::msaw_bridge::RegionalMsaw>,
    baro_seen: BTreeMap<u32, i64>,
    adsb_source_seen: BTreeMap<u32, AdsbSourceObservation>,
    profile: SurveillanceProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AdsbSourceObservation {
    source_ns: i64,
    event_sequence: u64,
    evidence: AdsbSourceEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurveillanceStageError {
    FailedSession,
    SealedTimestamp,
    WrongReceiptOrTimestamp,
    WrongSourceKind,
    InvalidPosition,
    InvalidSourceEvidence,
    SourceEvidenceCapacity,
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
            Self::InvalidSourceEvidence => "ADS-B source evidence is outside field bounds",
            Self::SourceEvidenceCapacity => "ADS-B source evidence capacity exhausted",
            Self::BarometricEvidenceCapacity => "barometric evidence capacity exhausted",
        }
    }
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
    pub tracks: Vec<ObservedTrack>,
    /// Present only when the caller selected an explicit surveillance mode. One bounded 40-byte
    /// row per current track; `refusal == 0` means `result` is valid, otherwise decision fields
    /// are zero and the distance field is its unavailable sentinel.
    pub mode_decisions: Vec<ModeDecisionRecord>,
    pub flags: Vec<u8>,
    pub frame_sha256: [u8; 32],
    pub ca_open: Vec<oa_ca::Alert>,
    pub ca_closed: Vec<oa_ca::Alert>,
    pub ca_altitude_lost: Vec<oa_ca::AltLost>,
    pub msaw: Option<crate::msaw_bridge::RegionalMsawFrame>,
    pub msaw_markers: Option<oa_display::core::alert_markers::MarkerStats>,
    pub msaw_altitude_source_unknown: u32,
    pub ca_stats: oa_ca::CycleStats,
    pub ca_work_short: u64,
    pub ca_input_short: u64,
    pub tracker_stats: TrackerStats,
    pub track_transport_seen_ns: Option<i64>,
    pub track_measurement_seen_ns: Option<i64>,
    pub measurement_rejections: u64,
    pub duplicate_rows_replaced: u32,
    pub displayed: u32,
    pub off_map: u32,
    pub warnings: Vec<String>,
}
// A7.2: cold owned frame evidence, retained at most MAX_FRAMES times.

/// Caller-visible result of applying one explicit mode adaptation to one current track.
/// The supplied evidence fields are caller assertions, not authenticated claims. FLAT is a
/// separate result field; this runtime leaves it false because its flight-view join does not aid
/// the tracker. Absent rows are reported with `evidence_present == 0`, unknown source/reliability/
/// ISR, and the missing-FL sentinel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ModeDecisionRecord {
    pub address: u32,
    pub evidence_ns: i64,
    /// Source time of the raw ADS-B row that supplied `adsb_evidence`; zero means absent.
    pub adsb_source_ns: i64,
    /// Normalized event sequence that binds this observation to its EventProvenance row.
    pub adsb_event_sequence: u64,
    pub adsb_evidence_present: u8,
    pub adsb_evidence: AdsbSourceEvidence,
    pub flight_level_hundreds: i16,
    pub source: TargetSource,
    pub reliability: ReliabilityEvidence,
    pub isr: IsrEvidence,
    pub controller_initiated_coast: u8,
    pub flight_plan_aided: u8,
    pub evidence_present: u8,
    pub coasting: u8,
    /// Zero means selected; otherwise the numeric `oa_track::surveillance_mode::ModeRefusal`.
    pub refusal: u8,
    pub reserved: [u8; 2],
    pub result: ModeResult,
}
const _: () = assert!(core::mem::size_of::<ModeDecisionRecord>() == 80);

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
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
    let tracker = tracker_new(&oa_track::TrackerConfig {
        max_tracks: profile.max_tracks,
        out_capacity: profile.max_drain_rows,
        log_capacity: profile.max_drain_rows,
        confirm_pool: profile.max_tracks,
        ..DEFAULT_CONFIG
    })
    .map_err(err)?;
    let mut scope = replay_new_with_top_inset(
        model,
        &ReplayConfig {
            centre_lat_e7: cfg.centre_lat_e7,
            centre_lon_e7: cfg.centre_lon_e7,
            range_nm: cfg.range_nm,
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            max_targets: profile.max_tracks,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
        160,
    )
    .map_err(err)?;
    let interaction_caps = oa_display::list::ListCaps {
        lines: 1,
        rects: 1,
        circles: profile.max_tracks * oa_display::core::alert_markers::ALERT_CIRCLES_PER_MARKER,
        polylines: 0,
        poly_vertices: 0,
        texts: 4
            + profile.max_tracks * oa_display::core::alert_markers::ALERT_TEXTS_PER_MARKER
            + oa_display::core::alert_markers::ALERT_HEADER_TEXTS,
        text_bytes: 256
            + profile.max_tracks * oa_display::core::alert_markers::ALERT_TEXT_BYTES_PER_MARKER
            + oa_display::core::alert_markers::ALERT_HEADER_TEXT_BYTES,
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
        adsb_source_seen: BTreeMap::new(),
        track_measurement_seen_ns: None,
        first_measurement_ns: None,
        measurement_rejections: 0,
        profile,
        previous_ns: None,
        frames: 0,
        completed: true,
        failed: false,
        overlay_base: None,
        presentation_updates: 0,
    })
}

pub fn surveillance_enable_msaw(
    s: &mut Surveillance,
    service: crate::msaw_bridge::RegionalMsaw,
    start_ns: i64,
) -> Result<(), String> {
    if s.frames != 0 || s.previous_ns.is_some() || s.msaw.is_some() {
        return Err("MSAW must be configured exactly once before the first frame".into());
    }
    let (stale_ns, future_ns) = crate::msaw_bridge::regional_msaw_net_policy(&service);
    crate::msaw_bridge::regional_msaw_install_inhibits(&service, &mut s.inhibits, start_ns)?;
    s.states = state_table_new(stale_ns, future_ns).map_err(err)?;
    s.msaw = Some(service);
    Ok(())
}

pub fn surveillance_acknowledge_msaw_aural(
    s: &mut Surveillance,
    address: u32,
    alert_start_ns: i64,
    source_time_ns: i64,
) -> Result<(), String> {
    let Some(service) = s.msaw.as_mut() else {
        return Err("MSAW aural acknowledgement refused because MSAW is not configured".into());
    };
    crate::msaw_bridge::regional_msaw_acknowledge_aural(
        service,
        address,
        alert_start_ns,
        source_time_ns,
    )
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
    if s.failed {
        return Err("surveillance failed; reconstruct from the admitted input prefix".into());
    }
    if !s.completed {
        return Err("previous frame has not been completed".into());
    }
    if time_ns <= 0 || time_ns % SECOND_NS != 0 {
        return Err("regional research frame receipt must be a positive whole second".into());
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
        EventKind::Position { .. } | EventKind::PositionV2 { .. } => {
            event.source == SourceKind::Surveillance
        }
        EventKind::Flight(_) | EventKind::Operator(_) => event.source == SourceKind::Flight,
        EventKind::Route { .. } | EventKind::FiledRoute { .. } | EventKind::ActiveCursor { .. } => {
            event.source == SourceKind::Route
        }
        EventKind::FeedState { .. } => true,
    };
    if !source_matches {
        return Err(SurveillanceStageError::WrongSourceKind);
    }
    if let EventKind::PositionV2 { evidence, .. } = event.kind {
        if !valid_adsb_source_evidence(evidence) {
            return Err(SurveillanceStageError::InvalidSourceEvidence);
        }
        if !s
            .adsb_source_seen
            .contains_key(&event.identity.aircraft_address)
            && s.adsb_source_seen.len() >= s.profile.max_tracks
        {
            return Err(SurveillanceStageError::SourceEvidenceCapacity);
        }
    }
    if let EventKind::Position {
        latitude_e7,
        longitude_e7,
        altitude_ft,
        ..
    }
    | EventKind::PositionV2 {
        latitude_e7,
        longitude_e7,
        altitude_ft,
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
    }
    Ok(())
}

fn valid_adsb_source_evidence(evidence: AdsbSourceEvidence) -> bool {
    let in_range = |value: u8, max: u8| value == u8::MAX || value <= max;
    in_range(evidence.source_type, 13)
        && in_range(evidence.version, 7)
        && in_range(evidence.nic, 11)
        && in_range(evidence.nac_p, 15)
        && in_range(evidence.nac_v, 7)
        && in_range(evidence.sil, 3)
        && in_range(evidence.sil_type, 2)
        && in_range(evidence.sda, 3)
        && in_range(evidence.gva, 3)
        && in_range(evidence.nic_baro, 1)
        && (evidence.row_flags == u8::MAX || evidence.row_flags <= 31)
        && valid_squawk_decimal(evidence.squawk)
        && evidence
            .callsign
            .iter()
            .all(|byte| *byte == 0 || (0x20..=0x7e).contains(byte))
}

fn valid_squawk_decimal(squawk: u16) -> bool {
    if squawk == u16::MAX {
        return true;
    }
    let mut value = squawk;
    for _ in 0..4 {
        if value % 10 > 7 {
            return false;
        }
        value /= 10;
    }
    value == 0
}

const fn missing_adsb_source_evidence() -> AdsbSourceEvidence {
    AdsbSourceEvidence {
        source_type: u8::MAX,
        category: 0,
        version: u8::MAX,
        nic: u8::MAX,
        nac_p: u8::MAX,
        nac_v: u8::MAX,
        sil: u8::MAX,
        sil_type: u8::MAX,
        sda: u8::MAX,
        gva: u8::MAX,
        nic_baro: u8::MAX,
        row_flags: u8::MAX,
        squawk: u16::MAX,
        callsign: [0; 8],
    }
}

pub fn surveillance_event(s: &mut Surveillance, event: &NormalizedEvent) -> Result<(), String> {
    surveillance_stage_event(s, event).map_err(|error| error.message().to_owned())?;
    let source_evidence = match event.kind {
        EventKind::PositionV2 { evidence, .. } => Some(evidence),
        _ => None,
    };
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
        }
        | EventKind::PositionV2 {
            latitude_e7,
            longitude_e7,
            altitude_ft,
            east_mmps,
            north_mmps,
            vertical_rate_fpm,
            ..
        } => {
            s.first_measurement_ns = Some(
                s.first_measurement_ns
                    .map_or(event.source_ns, |first| first.min(event.source_ns)),
            );
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
                s.adsb_source_seen.clear();
            } else if after.started > before.started
                || after.init_implausible > before.init_implausible
                || after.init_window_expired > before.init_window_expired
                || after.resyncs > before.resyncs
                || after.refiltered > before.refiltered
            {
                s.baro_seen.remove(&event.identity.aircraft_address);
                s.adsb_source_seen.remove(&event.identity.aircraft_address);
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
                if altitude_ft.is_some() && after.alt_rejected == before.alt_rejected {
                    let address = event.identity.aircraft_address;
                    let known = s.baro_seen.entry(address).or_insert(event.source_ns);
                    if event.source_ns > *known {
                        *known = event.source_ns;
                    }
                }
                let address = event.identity.aircraft_address;
                match source_evidence {
                    Some(evidence) => {
                        let prior_source_ns = s
                            .adsb_source_seen
                            .get(&address)
                            .map(|prior| prior.source_ns);
                        if prior_source_ns.is_none_or(|prior| event.source_ns >= prior) {
                            s.adsb_source_seen.insert(
                                address,
                                AdsbSourceObservation {
                                    source_ns: event.source_ns,
                                    event_sequence: event.sequence,
                                    evidence,
                                },
                            );
                        }
                    }
                    None => {
                        s.adsb_source_seen.remove(&address);
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
    mode_decisions: &[ModeDecisionRecord],
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
    for decision in mode_decisions {
        hash.update(&decision.address.to_le_bytes()).map_err(err)?;
        hash.update(&decision.evidence_ns.to_le_bytes())
            .map_err(err)?;
        hash.update(&decision.adsb_source_ns.to_le_bytes())
            .map_err(err)?;
        hash.update(&decision.adsb_event_sequence.to_le_bytes())
            .map_err(err)?;
        hash.update(&[decision.adsb_evidence_present])
            .map_err(err)?;
        hash.update(&[
            decision.adsb_evidence.source_type,
            decision.adsb_evidence.category,
            decision.adsb_evidence.version,
            decision.adsb_evidence.nic,
            decision.adsb_evidence.nac_p,
            decision.adsb_evidence.nac_v,
            decision.adsb_evidence.sil,
            decision.adsb_evidence.sil_type,
            decision.adsb_evidence.sda,
            decision.adsb_evidence.gva,
            decision.adsb_evidence.nic_baro,
            decision.adsb_evidence.row_flags,
        ])
        .map_err(err)?;
        hash.update(&decision.adsb_evidence.squawk.to_le_bytes())
            .map_err(err)?;
        hash.update(&decision.adsb_evidence.callsign).map_err(err)?;
        hash.update(&decision.flight_level_hundreds.to_le_bytes())
            .map_err(err)?;
        hash.update(&[
            decision.source as u8,
            decision.reliability as u8,
            decision.isr as u8,
            decision.controller_initiated_coast,
            decision.flight_plan_aided,
            decision.evidence_present,
            decision.coasting,
        ])
        .map_err(err)?;
        hash.update(&[decision.refusal]).map_err(err)?;
        hash.update(&decision.result.minimum_nm.to_le_bytes())
            .map_err(err)?;
        hash.update(&[
            decision.result.three_nm_symbol,
            decision.result.separation_eligible,
            decision.result.flat_aided,
            decision.result.fusion_solid_circle,
        ])
        .map_err(err)?;
        hash.update(&decision.result.preferred_radar_distance_m.to_le_bytes())
            .map_err(err)?;
    }
    Ok(hash.finalize())
}

pub fn surveillance_finish(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
) -> Result<SurveillanceFrame, String> {
    surveillance_finish_with_mode(s, ownership, None, &[])
}

/// Complete a receipt and optionally apply caller-provided facility mode and per-target evidence.
/// Missing target rows remain unknown; this function never infers sensor source, reliability,
/// ISR, flight level, or flight-plan-aided tracking from the mixed-source tracker output.
pub fn surveillance_finish_with_mode(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
    mode: Option<ModeAdaptation>,
    mode_evidence: &[TargetModeEvidence],
) -> Result<SurveillanceFrame, String> {
    surveillance_finish_inner(s, ownership, mode, mode_evidence, None, *b"\0\0\0\0")
}

/// Complete the normal regional caller path with current flight-table snapshots for MSAW
/// eligibility. Unmatched flight rows remain NotJudged unless the table reports ambiguity.
pub fn surveillance_finish_with_live_flights(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
    mode: Option<ModeAdaptation>,
    mode_evidence: &[TargetModeEvidence],
    flights: &mut crate::flight::FlightDriver,
    centre: [u8; 4],
) -> Result<SurveillanceFrame, String> {
    surveillance_finish_inner(s, ownership, mode, mode_evidence, Some(flights), centre)
}

fn surveillance_finish_inner(
    s: &mut Surveillance,
    ownership: Option<&OwnershipStatus>,
    mode: Option<ModeAdaptation>,
    mode_evidence: &[TargetModeEvidence],
    mut flight_driver: Option<&mut crate::flight::FlightDriver>,
    centre: [u8; 4],
) -> Result<SurveillanceFrame, String> {
    if s.failed {
        return Err("surveillance failed; reconstruct from the admitted input prefix".into());
    }
    if s.completed {
        return Err("frame already completed or not begun".into());
    }
    let time_ns = s.previous_ns.ok_or("no admitted receipt group")?;
    if let (Some(service), Some(first_measurement_ns)) = (s.msaw.as_mut(), s.first_measurement_ns) {
        crate::msaw_bridge::regional_msaw_set_request_epoch(service, first_measurement_ns)?;
    }
    if mode_evidence.len() > s.profile.max_tracks
        || mode_evidence
            .windows(2)
            .any(|pair| pair[0].address >= pair[1].address)
        || mode_evidence
            .iter()
            .any(|item| item.state_ns != time_ns || item.controller_initiated_coast > 1)
        || (mode.is_none() && !mode_evidence.is_empty())
    {
        return Err("surveillance mode evidence is not bounded, ordered, and epoch-matched".into());
    }
    // From this point the tracker, CA and scope are mutated. Any later error makes this object
    // terminal so callers cannot append events to or retry the partially completed frame.
    s.failed = true;
    tracker_advance(&mut s.tracker, time_ns, &mut s.drain);
    if s.drain.overflow != 0 || s.drain.t_ns.len() > s.profile.max_drain_rows {
        return Err("tracker output lost work".into());
    }
    let mut last = BTreeMap::new();
    let mut duplicate_rows_replaced = 0u32;
    // C7: no more than MAX_DRAIN_ROWS records are scanned; only the current tick is admitted.
    for (i, &time) in s.drain.t_ns.iter().enumerate() {
        if time == time_ns {
            let address = required(&s.drain.addr, i)?;
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
    s.adsb_source_seen
        .retain(|address, _| newest.contains_key(address));
    if mode_evidence
        .iter()
        .any(|evidence| !last.contains_key(&evidence.address))
    {
        return Err("mode evidence names an address absent from this frame".into());
    }
    let mut snapshot = output_new(s.profile.max_tracks);
    let mut ca_rows = Vec::with_capacity(s.profile.max_tracks);
    let mut tracks = Vec::with_capacity(s.profile.max_tracks);
    let mut flags = Vec::with_capacity(s.profile.max_tracks);
    let mut msaw_rows = Vec::with_capacity(s.profile.max_tracks);
    let mut msaw_basis = Vec::with_capacity(if s.msaw.is_some() {
        s.profile.max_tracks
    } else {
        0
    });
    let flight_view_capacity = flight_driver
        .as_deref()
        .map(crate::flight::flight_driver_capacity)
        .unwrap_or(0);
    let mut flight_views = Vec::with_capacity(flight_view_capacity);
    let mut mode_decisions = Vec::with_capacity(if mode.is_some() {
        s.profile.max_tracks
    } else {
        0
    });
    let mut mode_flags = Vec::with_capacity(if mode.is_some() {
        s.profile.max_tracks
    } else {
        0
    });
    let mut msaw_altitude_source_unknown = 0u32;
    for (&address, &index) in &last {
        let row = output_row(&s.drain, index)?;
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
        let adsb_observation = s
            .adsb_source_seen
            .get(&address)
            .copied()
            .filter(|item| item.source_ns == source_ns);
        if s.msaw.is_some() {
            let (flight, ambiguous) = match flight_driver.as_deref_mut() {
                Some(driver) => {
                    let result = crate::flight::flight_views_for_address(
                        driver,
                        address,
                        centre,
                        time_ns / 1_000_000,
                        &mut flight_views,
                    );
                    let view = (result.status == crate::flight::FlightViewStatus::Found
                        && flight_views.len() == 1)
                        .then(|| flight_views.first().copied())
                        .flatten();
                    (
                        view,
                        result.status == crate::flight::FlightViewStatus::AmbiguousAddress,
                    )
                }
                None => (None, false),
            };
            let service = s
                .msaw
                .as_mut()
                .ok_or("MSAW service disappeared during eligibility")?;
            msaw_basis.push(crate::msaw_bridge::regional_msaw_track_basis(
                service,
                address,
                source_ns,
                adsb_observation.map(|item| item.evidence),
                flight,
                ambiguous,
                time_ns,
            ));
        }
        let actual_source = adsb_observation.map_or(TargetSource::Unknown, |item| {
            if matches!(item.evidence.source_type, 0 | 1 | 8) {
                TargetSource::AdsB
            } else {
                TargetSource::Unknown
            }
        });
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
        if let Some(adaptation) = mode {
            let evidence = mode_evidence
                .binary_search_by_key(&address, |item| item.address)
                .ok()
                .and_then(|index| mode_evidence.get(index));
            let evidence_row = evidence.copied().unwrap_or(TargetModeEvidence {
                address,
                state_ns: if adsb_observation.is_some() {
                    time_ns
                } else {
                    0
                },
                flight_level_hundreds: i16::MIN,
                source: actual_source,
                reliability: ReliabilityEvidence::Unknown,
                isr: IsrEvidence::Unknown,
                controller_initiated_coast: 0,
            });
            let mut target_mode = TargetModeState {
                lat_e7: row.lat_e7,
                lon_e7: row.lon_e7,
                // Track altitude is mixed-source evidence, not a validated flight-level field.
                // The mode caller must supply a qualified FL value explicitly.
                flight_level_hundreds: evidence_row.flight_level_hundreds,
                source: evidence_row.source,
                reliability: evidence_row.reliability,
                isr: evidence_row.isr,
                coasting: u8::from(row.flags & O_COAST != 0),
                controller_initiated_coast: evidence_row.controller_initiated_coast,
                // The current runtime's address-to-flight-view join only drives a separate
                // trajectory forecast; it does not aid the tracker. Never synthesize FLAT here.
                flight_plan_aided: 0,
            };
            if adsb_observation.is_some() {
                target_mode.source = actual_source;
            }
            let decision = surveillance_mode_result(adaptation, target_mode);
            let (refusal, result) = match decision {
                Ok(result) => (0, result),
                Err(refusal) => (
                    refusal as u8,
                    ModeResult {
                        minimum_nm: 0,
                        three_nm_symbol: 0,
                        separation_eligible: 0,
                        flat_aided: 0,
                        fusion_solid_circle: 0,
                        reserved: [0; 2],
                        preferred_radar_distance_m: u32::MAX,
                    },
                ),
            };
            mode_decisions.push(ModeDecisionRecord {
                address,
                evidence_ns: evidence_row.state_ns,
                adsb_source_ns: adsb_observation.map_or(0, |item| item.source_ns),
                adsb_event_sequence: adsb_observation.map_or(0, |item| item.event_sequence),
                adsb_evidence_present: u8::from(adsb_observation.is_some()),
                adsb_evidence: adsb_observation
                    .map_or_else(missing_adsb_source_evidence, |item| item.evidence),
                flight_level_hundreds: evidence_row.flight_level_hundreds,
                source: target_mode.source,
                reliability: evidence_row.reliability,
                isr: evidence_row.isr,
                controller_initiated_coast: evidence_row.controller_initiated_coast,
                flight_plan_aided: result.flat_aided,
                evidence_present: u8::from(evidence.is_some() || adsb_observation.is_some()),
                coasting: target_mode.coasting,
                refusal,
                reserved: [0; 2],
                result,
            });
            let display_flags =
                (if refusal == 0 && result.fusion_solid_circle != 0 {
                    oa_display::traffic::T_FUSION
                } else {
                    0
                }) | (if evidence.is_some_and(|item| item.isr == IsrEvidence::Required) {
                    oa_display::traffic::T_ISR
                } else {
                    0
                });
            mode_flags.push((address, display_flags));
        }
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
    let msaw = match s.msaw.as_mut() {
        Some(service) => Some(crate::msaw_bridge::regional_msaw_cycle_with_basis(
            service,
            time_ns,
            &msaw_rows,
            &msaw_basis,
            &mut s.states,
            &mut s.inhibits,
        )?),
        None => None,
    };
    let ca = ca_cycle(
        &mut s.ca,
        time_ns,
        &ca_rows,
        &mut s.alerts,
        &mut s.events,
        &mut s.states,
    );
    let (ca_closed, ca_altitude_lost) = capture_ca_logs(s);
    let (msaw_markers, digest, displayed, off_map, warnings) = if !s.profile.render_scope {
        let digest = offline_projection_digest(time_ns, &tracks, ca_open(&s.ca), &mode_decisions)?;
        (None, digest, 0, 0, Vec::new())
    } else {
        let mut store = RowStore::default();
        let stats = store_add(
            &mut store,
            &row_columns(&snapshot, &mut ColumnBuf::default()),
            time_ns,
        )
        .map_err(err)?;
        if stats.kept != stats.rows_in || store_finish(&mut store) != 0 {
            return Err("scope input snapshot lost rows".into());
        }
        let mut feeds = Vec::with_capacity(5);
        for (i, &name) in FEED_NAMES.iter().enumerate() {
            let seen = if i == 0 {
                s.track_measurement_seen_ns
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
                    if down || age >= 5 * SECOND_NS {
                        FeedState::Lost { age_ms }
                    } else if age >= 2 * SECOND_NS {
                        FeedState::Slow { age_ms }
                    } else {
                        FeedState::Live
                    }
                }
            };
            feeds.push(FeedStatus { name, state });
        }
        let newest_track = tracker_newest_update_ns(&s.tracker);
        let info = replay_build_with_mode_overlays(
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
            }),
            ownership,
            &mode_flags,
        )
        .map_err(err)?;
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
        // Reserve rows 104..160 for alerts and console text. Layer 14 masks map primitives that
        // extend beyond their clipped centre, while the standard legend remains in rows 0..104.
        oa_display::list::push_rect(
            &mut s.scope.list,
            (0, 104 * 16),
            (
                i32::try_from(FRAME_WIDTH).map_err(|_| "panel width")? * 16,
                160 * 16,
            ),
            oa_display::palette::BACKGROUND,
            14,
        )
        .map_err(err)?;
        oa_display::list::push_line(
            &mut s.scope.list,
            (0, 159 * 16),
            (
                i32::try_from(FRAME_WIDTH).map_err(|_| "panel width")? * 16,
                159 * 16,
            ),
            oa_display::palette::DIM_TEXT,
            15,
        )
        .map_err(err)?;
        let msaw_markers = if let Some(frame) = msaw.as_ref() {
            use oa_display::core::alert_markers::{
                append_alert_markers, AlertMarker, MarkerState, NO_POSITION_E7,
            };
            if frame.open.len() > s.profile.max_tracks {
                return Err("MSAW alert marker capacity exceeded".into());
            }
            let mut markers = Vec::with_capacity(s.profile.max_tracks);
            for alert in &frame.open {
                let position = tracks.iter().find(|track| track.address == alert.addr);
                let state = if frame.stats.is_none() || alert.end_reason != oa_msaw::END_CLEAR {
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
            digest,
            info.convert.added,
            info.convert.off_map,
            warnings,
        )
    };
    let track_transport_seen_ns = s.seen.first().copied().flatten();
    s.completed = true;
    s.failed = false;
    Ok(SurveillanceFrame {
        time_ns,
        raster_rendered: s.profile.render_scope,
        tracks,
        mode_decisions,
        flags,
        frame_sha256: digest,
        ca_open: ca_open(&s.ca).to_vec(),
        ca_closed,
        ca_altitude_lost,
        msaw,
        msaw_markers,
        msaw_altitude_source_unknown,
        ca_stats: ca.stats,
        ca_work_short: ca.work_short,
        ca_input_short: ca.input_short,
        tracker_stats: *tracker_stats(&s.tracker),
        track_transport_seen_ns,
        track_measurement_seen_ns: s.track_measurement_seen_ns,
        measurement_rejections: s.measurement_rejections,
        duplicate_rows_replaced,
        displayed,
        off_map,
        warnings,
    })
}

/// Pixels are borrowed only while the completed immutable frame is being published by the shell.
pub fn surveillance_pixels(s: &Surveillance) -> Result<&[u8], String> {
    if !s.profile.render_scope || s.frames == 0 || s.failed || !s.completed {
        return Err("no publishable pixels for an incomplete or failed frame".into());
    }
    Ok(&s.scope.buf)
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
    if !s.profile.render_scope {
        return Err("analysis-only profile does not produce a rendered control frame".into());
    }
    if s.failed || !s.completed || s.frames == 0 {
        return Err("control overlay requires a completed frame".into());
    }
    if console.len() > 128 || !console.bytes().all(|byte| (32..=126).contains(&byte)) {
        return Err("console text must be at most 128 printable ASCII bytes".into());
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
        let y = 132 + i32::try_from(index).map_err(|_| "console row")? * 12;
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

    fn position(receipt_ns: i64, source_ns: i64, address: u32, sequence: u64) -> NormalizedEvent {
        NormalizedEvent {
            receipt_ns,
            source_ns,
            sequence,
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
            },
        }
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
}

//! Offline composed regional session (shell, B1/B3/C6), with one common receipt cutoff.
//! This executes actual product cores; it is not deadline or allocation qualification.

use crate::commands;
use crate::flight::{
    self, FlightApplyOutcome, FlightDriver, FlightDriverConfig, FlightDriverReport,
    FlightViewResult, FlightViewStatus, SharedFlightView, MAX_DRIVER_FLIGHTS,
    MAX_DRIVER_LOG_ENTRIES,
};
use crate::nam_flight::{self, NamFlightContext, NamFlightResult, NamFlightRuntime};
use crate::route::{self, RouteStore};
use crate::session::{
    summarize_input_batch_with_limit, FrameEvidence, ReceiptBatch, MAX_EVENTS_PER_RECEIPT,
    RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
};
use crate::surveillance::{self, Surveillance, SurveillanceConfig, SurveillanceFrame};
use oa_adapt::manifest::VerifiedPackage;
use oa_airspace::core::model::Model;
use oa_legacy_traj::core::trajectory::{
    PerformanceEnvelope, TrajectoryIssueOutput, VerticalProfileOutput,
};
use oa_quarantine::{
    queue_acknowledge, queue_front, route_input, source_stats, Disposition, InputIdentity,
    InputOutcome, InputSource, QuarantineEvent, QuarantineQueue, ReasonFlags, SourceStats,
    REASON_INPUT_INVALID, REASON_RESOURCE_LIMIT, REASON_SURVEILLANCE_INVALID,
};
use oa_runway::final_monitor as final_monitor_core;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

const DEFAULT_SESSION_NAMESPACE: u16 = 1;
const QUARANTINE_CLOCK_DOMAIN_ID: u32 = 1;
/// C7 budget for one full-key search through the bounded (at most 8192-key) sorted view ledger.
const MAX_FLIGHT_KEY_SEARCH_STEPS: usize = 14;
/// C7 budget for the bounded in-place suffix move when a new key is registered.
const MAX_FLIGHT_KEY_INSERT_MOVES: usize = flight::MAX_OFFLINE_DRIVER_FLIGHTS;
/// C7 budget for terminal-key retirement when the view ledger reaches its configured capacity.
const MAX_FLIGHT_KEY_RECLAIM_WORK: usize = flight::MAX_OFFLINE_DRIVER_FLIGHTS;
/// C7 worst-case hash probes for that retirement pass: at most two exact-key lookups per ledger
/// key, each bounded by the driver's exact `by_key.cells.len()` limit.
const MAX_FLIGHT_KEY_RECLAIM_PROBE_WORK: usize =
    MAX_FLIGHT_KEY_RECLAIM_WORK * 2 * flight::MAX_OFFLINE_DRIVER_KEY_INDEX_CELLS;
/// C7 budget for exporting the current bounded full-key flight view list once per frame.
const MAX_FLIGHT_DATA_VIEW_WORK: usize = flight::MAX_OFFLINE_DRIVER_FLIGHTS;
const MAX_RUNTIME_ITEM_WORK: u32 =
    oa_airspace::core::route::MAX_ROUTE_WORK + crate::input::MAX_FILED_ROUTE_BYTES as u32 + 16;

/// OpenATCA functional selection. These names describe this runtime and do not claim a legacy
/// FSL/EFSL flag mapping.
/// Functional basis: FAA JO 7110.65BB §13-1-16 allows electronic flight data to support
/// situational awareness during surveillance/flight-data outages; JO 7210.3EE §3-6-7 addresses
/// Fusion-mode degradation and facility contingency procedures. These are functional references,
/// not evidence of an FSL/EFSL flag mapping or a multi-sensor source contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionalAutomationMode {
    FlightDataOnly,
    Surveillance,
    Fusion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveAutomationMode {
    FlightDataOnly,
    Surveillance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationDegradation {
    None,
    OperatorSelectedFlightDataOnly,
    SurveillanceUnavailable,
    FusionUnavailable,
}

/// Selected/effective mode and explicit fusion-off indication published on every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalAutomationStatus {
    pub selected: RegionalAutomationMode,
    pub effective: EffectiveAutomationMode,
    pub degradation: AutomationDegradation,
    /// Fusion is currently not implemented by this one-surveillance-source runtime.
    pub fusion_off: bool,
}

/// Caller-provided runtime configuration for one explicitly selected parallel runway pair.
/// These values are not implied by the adaptation snapshot or authenticated by its manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeFinalMonitorConfig {
    pub pair: final_monitor_core::FinalPair,
    pub assignments: Vec<FinalApproachAssignment>,
}

/// Full flight identity and selected runway end supplied by an authorized runtime caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FinalApproachAssignment {
    pub flight: oa_flight::FlightKey,
    pub runway_id: u32,
    pub end_name: [u8; 3],
}

const MAX_FINAL_APPROACH_ASSIGNMENTS: usize = oa_display::core::alert_markers::MAX_ALERT_MARKERS;
const _: () = assert!(MAX_FINAL_APPROACH_ASSIGNMENTS <= final_monitor_core::MAX_TRACKS);
const MAX_OPERATOR_CONSOLE_BYTES: usize = 128;
const OPERATOR_CONSOLE_SEPARATOR_BYTES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalMonitorStatus {
    NotConfigured,
    Available,
    Degraded(FinalMonitorDegradation),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalMonitorDegradation {
    MissingAssignedTrack,
    UnusableUncertainty,
    StalePrediction,
    InputCadence,
    StalePredictionAndInputCadence,
    InvalidPrediction,
    CoreRefusal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalFinalMonitorAlert {
    pub address: u32,
    pub pair_id: final_monitor_core::FinalPairId,
    pub assigned_runway_id: u32,
    pub severity: final_monitor_core::FinalSeverity,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub seconds_to_entry_milli: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalFinalMonitorTiming {
    pub observed_input_interval_ns: u64,
    pub observed_evaluation_interval_ns: u64,
    pub prediction_age_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionalFinalMonitorFrame {
    pub status: FinalMonitorStatus,
    pub alerts: Vec<RegionalFinalMonitorAlert>,
    pub timing: Option<RegionalFinalMonitorTiming>,
}

struct ConfiguredFinalMonitor {
    pair: final_monitor_core::FinalPair,
    assignments: Vec<FinalApproachAssignment>,
    state: final_monitor_core::FinalMonitorState,
    latest_alerts: Vec<RegionalFinalMonitorAlert>,
    loss_reason: Option<FinalMonitorDegradation>,
}

#[derive(Clone, Copy)]
struct FinalMonitorTrack {
    address: u32,
    assigned_runway_id: u32,
    east_m: f64,
    north_m: f64,
    velocity_east_mps: f64,
    velocity_north_mps: f64,
    r95_m: f64,
    lat_e7: i32,
    lon_e7: i32,
    newest_source_ns: i64,
}

struct FinalMonitorBuffers {
    addresses: Vec<u32>,
    pair_ids: Vec<final_monitor_core::FinalPairId>,
    severity: Vec<final_monitor_core::FinalSeverity>,
    lat_e7: Vec<i32>,
    lon_e7: Vec<i32>,
    seconds_to_entry_milli: Vec<u32>,
}

#[derive(Clone, Copy)]
struct CurrentFlightView {
    address: u32,
    result: FlightViewResult,
    view: Option<SharedFlightView>,
    identity_corroborated: bool,
}

#[derive(Clone, Copy)]
struct KnownFlight {
    key: oa_flight::FlightKey,
    last_consumed_receipt_ns: Option<i64>,
}

#[derive(Clone, Copy)]
struct FlightFreshness {
    last_receipt_ns: Option<i64>,
    now_ns: i64,
    drop_after_ns: i64,
    cutoff_ms: i64,
}

fn traffic_detail_for_current_view(
    track: &crate::surveillance::ObservedTrack,
    result: FlightViewResult,
    view: Option<SharedFlightView>,
    runtime_centre: [u8; 4],
    identity_corroborated: bool,
) -> oa_display::core::traffic::TrafficBlockDetail {
    use oa_display::core::traffic::TrafficBlockDetail;

    if result.status != FlightViewStatus::Found {
        return if track.altitude_ft == i32::MIN {
            TrafficBlockDetail::SymbolOnly
        } else {
            TrafficBlockDetail::AltitudeOnly
        };
    }
    if !identity_corroborated || view.is_none() {
        return if track.altitude_ft == i32::MIN {
            TrafficBlockDetail::SymbolOnly
        } else {
            TrafficBlockDetail::AltitudeOnly
        };
    }
    if runtime_centre == *b"ZAU\0"
        && view.is_some_and(|row| {
            matches!(
                row.plan_state,
                oa_flight::ST_ACTIVE | oa_flight::ST_DEPARTED
            )
        })
    {
        TrafficBlockDetail::Full
    } else if track.altitude_ft == i32::MIN {
        TrafficBlockDetail::SymbolOnly
    } else {
        TrafficBlockDetail::AltitudeOnly
    }
}

fn flight_identity_corroborates(
    track: &crate::surveillance::ObservedTrack,
    observation: Option<crate::surveillance::TrackCallsignEvidence>,
    view: &SharedFlightView,
    runtime_centre: [u8; 4],
    freshness: FlightFreshness,
) -> bool {
    let Some(observation) = observation else {
        return false;
    };
    // Callsign is tied to the exact accepted track observation. Flight-message receipt time is a
    // separate freshness witness; identity change stamps do not refresh when a message is
    // successfully consumed without changing the identity fields.
    observation.source_ns == track.newest_source_ns
        && callsigns_equal(&observation.callsign, &view.callsign)
        && view.gufi != 0
        && view.address == track.address
        && view.centre == runtime_centre
        && matches!(
            view.plan_state,
            oa_flight::ST_ACTIVE | oa_flight::ST_DEPARTED
        )
        && flight_message_is_fresh(
            freshness.last_receipt_ns,
            freshness.now_ns,
            freshness.drop_after_ns,
        )
        && flight::flight_view_as_of(view, freshness.cutoff_ms).is_ok()
}

fn flight_message_is_fresh(last_receipt_ns: Option<i64>, now_ns: i64, drop_after_ns: i64) -> bool {
    if now_ns < 0 || drop_after_ns < 0 {
        return false;
    }
    last_receipt_ns.is_some_and(|last| {
        last >= 0
            && last <= now_ns
            && now_ns
                .checked_sub(last)
                .is_some_and(|age| age <= drop_after_ns)
    })
}

fn known_flight_receipt(
    flights: &[KnownFlight],
    view: &SharedFlightView,
) -> Result<Option<i64>, String> {
    let Some(key) = flight_key_from_view(view) else {
        return Ok(None);
    };
    let Ok(index) = flight_key_search(flights, key)? else {
        return Ok(None);
    };
    flights
        .get(index)
        .map(|known| known.last_consumed_receipt_ns)
        .ok_or_else(|| "flight liveness ledger index invariant".into())
}

fn callsigns_equal(left: &[u8; 8], right: &[u8; 8]) -> bool {
    fn body(value: &[u8; 8]) -> Option<&[u8]> {
        if !value.is_ascii() {
            return None;
        }
        let end = value
            .iter()
            .position(|byte| *byte == 0 || *byte == b' ')
            .unwrap_or(value.len());
        let prefix = value.get(..end)?;
        if prefix.is_empty()
            || !prefix.iter().all(u8::is_ascii_alphanumeric)
            || value
                .get(end..)?
                .iter()
                .any(|byte| *byte != 0 && *byte != b' ')
        {
            return None;
        }
        Some(prefix)
    }
    body(left).zip(body(right)).is_some_and(|(a, b)| a == b)
}

// A7.2: cold subsystem composition. All core memory belongs to its originating subsystem.
pub struct RegionalRuntime {
    model: Model,
    adaptation_sha256: [u8; 32],
    issued_manifest_sha256: Option<[u8; 32]>,
    flights: FlightDriver,
    surveillance: Surveillance,
    routes: RouteStore,
    nam_flights: NamFlightRuntime,
    centre: [u8; 4],
    frame_sequence: u64,
    failed: bool,
    session_namespace: u16,
    quarantine_queue: QuarantineQueue,
    interaction: oa_display::interaction::InteractionState,
    views: Vec<oa_display::interaction::TargetView>,
    observed: Vec<crate::surveillance::ObservedTrack>,
    next_generation: u32,
    console_text: String,
    profile: RegionalRunProfile,
    selected_mode: RegionalAutomationMode,
    last_automation: RegionalAutomationStatus,
    known_flights: Vec<KnownFlight>,
    final_monitor: Option<ConfiguredFinalMonitor>,
    final_monitor_display: RegionalFinalMonitorFrame,
}

/// Versioned capacities for a single offline replay. Defaults reproduce the historic small
/// session; `recorded_peak` is an explicitly selected high-volume analysis profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalRunProfile {
    pub surveillance: surveillance::SurveillanceProfile,
    pub max_flights: usize,
    /// Current converted-route slots. The legacy selector history remains independently capped.
    pub max_route_revisions: usize,
    /// Replace a flight's current route in-place while recording explicit as-of expiry intervals.
    pub reclaim_current_route_by_key: bool,
    pub max_flight_log_entries: usize,
    pub max_total_events: usize,
    pub max_events_per_receipt: usize,
}

impl RegionalRunProfile {
    /// Offline capacity envelope for recorded replay. It supports 20M frame callbacks without
    /// retaining frames, 10M normalized events in a streaming cursor, 4096 live tracks, and 8192
    /// flight plans. At full track occupancy, the pair count is 8,386,560; the candidate table
    /// cap is 8,388,608 × 16 B (~128 MiB). The open-alert state and alert log are each capped at
    /// 1,048,576 × 96 B (~96 MiB). Those dominant CA tables total about 320 MiB, plus O(track)
    /// tables, before tracker/model/runtime storage; this leaves substantial headroom inside a
    /// 6 GiB run cap. Resource caps still cause explicit refusal; this does not promise a workload will fit.
    pub const fn recorded_peak() -> Self {
        Self {
            surveillance: surveillance::SurveillanceProfile {
                max_tracks: 4096,
                max_frames: surveillance::MAX_OFFLINE_FRAMES,
                max_drain_rows: 262_144,
                max_baro_evidence: surveillance::MAX_OFFLINE_BARO_EVIDENCE,
                max_open_alerts: 1 << 20,
                max_candidate_pairs: 8_388_608,
                max_ca_work_per_frame: 8_388_608,
                max_presentation_updates: 0,
                render_scope: false,
                tracker_timing: surveillance::TrackerTimingProfile::DefaultV1,
                frame_width: surveillance::FRAME_WIDTH,
                frame_height: surveillance::FRAME_HEIGHT,
                map_layers: oa_display::core::scene::layers_all(),
                ifr_airports_only: false,
                fixes_to_view_range: false,
            },
            max_flights: 8192,
            max_route_revisions: route::MAX_ROUTE_REVISIONS,
            reclaim_current_route_by_key: false,
            max_flight_log_entries: MAX_DRIVER_LOG_ENTRIES,
            max_total_events: crate::session::MAX_RECORDED_PROFILE_EVENTS,
            max_events_per_receipt: MAX_EVENTS_PER_RECEIPT,
        }
    }

    /// Bounded raster-producing variant for short, explicit recorded replay bundles such as J2.
    /// It keeps the recorded surveillance/CA capacities and raises flight/view storage to the
    /// same finite 4,096-track bound, then enables the existing bounded scope/presentation
    /// path. This profile is for ADS-B-heavy recorded replay; it is not the analysis-only
    /// full-day profile and makes no facility-capacity claim.
    pub const fn recorded_peak_rendered() -> Self {
        let mut profile = Self::recorded_peak();
        profile.max_flights = profile.surveillance.max_tracks;
        profile.surveillance.max_presentation_updates = surveillance::MAX_PRESENTATION_UPDATES;
        profile.surveillance.render_scope = true;
        profile
    }

    /// Bounded output profile for imported recorded-hour replay. Only this explicitly selected
    /// offline profile raises the normalized receipt group to accommodate dense SFDPS cutoffs.
    pub const fn recorded_hour_rendered() -> Self {
        let mut profile = Self::recorded_peak_rendered();
        profile.surveillance.max_frames = crate::cli::RECORDED_HOUR_FRAMES;
        profile.max_events_per_receipt = RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT;
        profile
    }

    /// Explicit 8,192-plan offline profile. It reserves a bounded current-route table and
    /// replaces route payloads by full flight key; older as-of intervals return HistoryExpired.
    /// The one-day horizon matches the full-day replay/query workflow; successful frames own
    /// copies and retain no store references. With 7,080 current plans, at most 1,112 additional
    /// identities fit in the same retained pool before an explicit capacity refusal or watermark
    /// reclamation. This is a finite occupancy envelope, not a guarantee of arbitrary turnover.
    /// The 256-entry cursor history and 32,640 pairwise cursor-work limit are unchanged.
    /// Inline converted payload is 32,004 B × 8,192 (~250 MiB); exact vector capacities are
    /// measured at startup and refused above 320 MiB. This is the route-store reservation only,
    /// not a whole-process RSS bound.
    pub const fn flight_plan_capacity() -> Self {
        let mut profile = Self::recorded_peak();
        profile.max_route_revisions = route::MAX_CURRENT_ROUTE_PLANS;
        profile.reclaim_current_route_by_key = true;
        profile
    }
}

pub const DEFAULT_REGIONAL_RUN_PROFILE: RegionalRunProfile = RegionalRunProfile {
    surveillance: surveillance::DEFAULT_SURVEILLANCE_PROFILE,
    max_flights: MAX_DRIVER_FLIGHTS,
    max_route_revisions: route::MAX_ROUTE_REVISIONS,
    reclaim_current_route_by_key: false,
    max_flight_log_entries: MAX_DRIVER_LOG_ENTRIES,
    max_total_events: crate::session::MAX_SESSION_EVENTS,
    max_events_per_receipt: MAX_EVENTS_PER_RECEIPT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationModeError {
    FusionUnavailable,
}

/// Set the requested OpenATCA mode. Fusion is refused until the runtime has multiple qualified
/// surveillance inputs; a single-source tracker cannot claim fusion. Rejection leaves selection
/// unchanged, and every published frame still reports fusion off.
pub fn regional_select_automation_mode(
    runtime: &mut RegionalRuntime,
    selected: RegionalAutomationMode,
) -> Result<(), AutomationModeError> {
    if selected == RegionalAutomationMode::Fusion {
        return Err(AutomationModeError::FusionUnavailable);
    }
    runtime.selected_mode = selected;
    Ok(())
}

fn automation_status(
    selected: RegionalAutomationMode,
    feeds: &[oa_display::core::safety::FeedStatus],
) -> RegionalAutomationStatus {
    use oa_display::core::safety::FeedState;
    let surveillance_live = feeds
        .iter()
        .find(|feed| feed.name == *b"TRACK\0\0\0")
        .is_some_and(|feed| feed.state == FeedState::Live);
    let (effective, degradation) = match selected {
        RegionalAutomationMode::FlightDataOnly => (
            EffectiveAutomationMode::FlightDataOnly,
            AutomationDegradation::OperatorSelectedFlightDataOnly,
        ),
        RegionalAutomationMode::Surveillance if surveillance_live => (
            EffectiveAutomationMode::Surveillance,
            AutomationDegradation::None,
        ),
        RegionalAutomationMode::Surveillance => (
            EffectiveAutomationMode::FlightDataOnly,
            AutomationDegradation::SurveillanceUnavailable,
        ),
        RegionalAutomationMode::Fusion => (
            EffectiveAutomationMode::FlightDataOnly,
            AutomationDegradation::FusionUnavailable,
        ),
    };
    RegionalAutomationStatus {
        selected,
        effective,
        degradation,
        fusion_off: true,
    }
}

fn automation_status_label(status: RegionalAutomationStatus) -> &'static str {
    match (status.effective, status.degradation) {
        (
            EffectiveAutomationMode::FlightDataOnly,
            AutomationDegradation::SurveillanceUnavailable,
        ) => "MODE FLIGHT DATA ONLY | SURVEILLANCE LOST | FUSION OFF",
        (EffectiveAutomationMode::FlightDataOnly, _) => "MODE FLIGHT DATA ONLY | FUSION OFF",
        (EffectiveAutomationMode::Surveillance, _) => "MODE SURVEILLANCE | FUSION OFF",
    }
}

pub(crate) fn automation_console_text(console: &str, status: RegionalAutomationStatus) -> String {
    let mut text = String::with_capacity(192);
    if !console.is_empty() {
        text.push_str(console);
        text.push_str(" | ");
    }
    text.push_str(automation_status_label(status));
    text
}

// A3/C6: cold offline evidence rows/frame are bounded by the selected flight profile capacity
// (32 by default, 8192 in recorded_peak); snapshots are copied, never aliased. Optional vertical
// outputs remain limited by MAX_PERFORMANCE_ENVELOPES and fixed point profile capacity.
#[derive(Clone, Debug, PartialEq)]
pub struct FlightEvidence {
    pub address: u32,
    pub claimants: usize,
    pub view: Option<SharedFlightView>,
    pub trajectory: Option<TrajectoryIssueOutput>,
    pub vertical_profile: Option<VerticalProfileOutput>,
    pub vertical_refusal: String,
    pub refusal: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct RegionalFrame {
    pub sequence: u64,
    pub input: FrameEvidence,
    pub surveillance: SurveillanceFrame,
    /// Parallel-runway monitor output; NotConfigured is distinct from an empty healthy result.
    pub final_monitor: RegionalFinalMonitorFrame,
    pub automation: RegionalAutomationStatus,
    pub flight_operations: Vec<FlightApplyOutcome>,
    pub flights: Vec<FlightEvidence>,
    /// Bounded full-key plan/owner/handoff views, including flights with no surveillance target.
    pub flight_data: Vec<flight::SharedFlightView>,
    /// Per-field as-of state aligned by index with `flight_data`; ages are observations, not
    /// expiry/freshness classifications.
    pub flight_data_as_of: Vec<flight::SharedFlightAsOf>,
    /// Known keys without a current view for this centre; never silently folded into the list.
    pub flight_data_unavailable: u32,
    /// Bounded address-join counts for the exact rendered frame; unavailable other-centre keys
    /// remain separately represented by `flight_data_unavailable`.
    pub flight_correlation: FlightCorrelationSummary,
    pub flight_report: FlightDriverReport,
    pub refused_legacy_route_events: u64,
    pub controller_views: Vec<oa_display::interaction::TargetView>,
    pub controller_dispatches: Vec<commands::Dispatch>,
    pub controller_outcome: oa_display::interaction::CommandOutcome,
    pub controller_selection_kind: oa_display::interaction::SelectionKind,
    pub controller_selected_address: Option<u32>,
    pub controller_selection: Option<oa_display::interaction::FlightIdentity>,
    pub console_text: String,
    pub quarantine: RuntimeQuarantineReport,
}

/// Aggregate of full-key flight views and co-timed callsign-corroborated track joins for this
/// runtime centre. It contains no per-flight identifiers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlightCorrelationSummary {
    pub available_flight_views_for_runtime_centre: u32,
    pub positive_address_claims_for_runtime_centre: u32,
    pub matched_observed_tracks: u32,
    pub unmatched_observed_tracks: u32,
    pub ambiguous_observed_tracks: u32,
    pub refused_observed_track_lookups: u32,
}
// A7.2: cold shell evidence, bounded by runtime capacities.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeQuarantineRecord {
    pub item_index: u16,
    pub input_source: InputSource,
    pub identity: InputIdentity,
    pub reasons: ReasonFlags,
    pub manual_handling_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeQuarantineReport {
    /// Current receipt batch only; raw event payload remains with the caller's batch.
    pub records: Vec<RuntimeQuarantineRecord>,
    /// Queue status is cumulative for this runtime instance.
    pub source_stats: [SourceStats; oa_quarantine::INPUT_SOURCE_COUNT],
}

/// Maximum caller-supplied performance envelopes in one opt-in receipt (at most 32 rows; matching
/// work is bounded by 32 flights × 32 envelopes).
pub const MAX_PERFORMANCE_ENVELOPES: usize = MAX_DRIVER_FLIGHTS;

fn transaction_item(event: &crate::input::NormalizedEvent) -> bool {
    match event.kind {
        crate::input::EventKind::Position { .. }
        | crate::input::EventKind::Flight(_)
        | crate::input::EventKind::FiledRoute { .. }
        | crate::input::EventKind::ActiveCursor { .. } => true,
        crate::input::EventKind::FeedState { .. } => matches!(
            event.source,
            crate::input::SourceKind::Surveillance
                | crate::input::SourceKind::Flight
                | crate::input::SourceKind::Route
        ),
        crate::input::EventKind::Operator(_) => false,
        crate::input::EventKind::Route { .. } => false,
    }
}

fn quarantine_source(event: &crate::input::NormalizedEvent) -> Result<InputSource, String> {
    match event.source {
        crate::input::SourceKind::Surveillance => Ok(InputSource::Surveillance),
        crate::input::SourceKind::Flight | crate::input::SourceKind::Route => {
            Ok(InputSource::FlightData)
        }
        crate::input::SourceKind::Weather | crate::input::SourceKind::Adaptation => {
            Err("this runtime transaction slice has no quarantine source mapping".into())
        }
    }
}

fn runtime_item_id(
    batch_ordinal: u64,
    item_index: usize,
    receipt_limit: usize,
) -> Result<u64, String> {
    let index = u64::try_from(item_index).map_err(|_| "quarantine item index overflow")?;
    if !matches!(
        receipt_limit,
        MAX_EVENTS_PER_RECEIPT | RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
    ) {
        return Err("quarantine batch/item identity overflow".into());
    }
    let stride = u64::try_from(receipt_limit).map_err(|_| "quarantine item index overflow")?;
    if index >= stride || batch_ordinal == 0 {
        return Err("quarantine batch/item identity overflow".into());
    }
    batch_ordinal
        .checked_mul(stride)
        .and_then(|base| base.checked_add(index))
        .filter(|value| *value != 0)
        .ok_or_else(|| "quarantine item identity overflow".into())
}

fn event_flight_key(event: &crate::input::NormalizedEvent) -> Option<oa_flight::FlightKey> {
    use crate::input::FlightOperation;
    match event.kind {
        crate::input::EventKind::Flight(
            FlightOperation::SetPlanByKey { key, .. }
            | FlightOperation::CancelPlanByKey { key, .. }
            | FlightOperation::SetOwnerByKey { key, .. }
            | FlightOperation::HandoffByKey { key, .. },
        ) => Some(key),
        crate::input::EventKind::Flight(_) if event.identity.flight_gufi != 0 => {
            Some(oa_flight::key_gufi(event.identity.flight_gufi))
        }
        _ => None,
    }
}

fn flight_key_search(
    keys: &[KnownFlight],
    key: oa_flight::FlightKey,
) -> Result<Result<usize, usize>, String> {
    if keys.len() > flight::MAX_OFFLINE_DRIVER_FLIGHTS {
        return Err("known flight key ledger exceeds its C7 scan bound".into());
    }
    let (mut low, mut high) = (0usize, keys.len());
    let mut steps = 0usize;
    while low < high {
        steps = steps
            .checked_add(1)
            .ok_or("flight-key search work counter overflow")?;
        if steps > MAX_FLIGHT_KEY_SEARCH_STEPS {
            return Err("flight-key binary-search budget exceeded".into());
        }
        let middle = low + (high - low) / 2;
        let current = keys
            .get(middle)
            .copied()
            .ok_or("flight-key binary-search index invariant")?;
        match (
            current.key.kind,
            current.key.gufi,
            current.key.callsign,
            current.key.dep,
            current.key.arr,
        )
            .cmp(&(key.kind, key.gufi, key.callsign, key.dep, key.arr))
        {
            std::cmp::Ordering::Equal => return Ok(Ok(middle)),
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
        }
    }
    Ok(Err(low))
}

fn reclaim_retired_flight_keys(runtime: &mut RegionalRuntime, now_ms: i64) -> Result<(), String> {
    let len = runtime.known_flights.len();
    if len > MAX_FLIGHT_KEY_RECLAIM_WORK {
        return Err("flight-key retirement scan exceeded its C7 bound".into());
    }
    let probes_per_key = flight::flight_driver_key_index_cells(&runtime.flights);
    if probes_per_key > flight::MAX_OFFLINE_DRIVER_KEY_INDEX_CELLS {
        return Err("flight-key index exceeds its declared C7 probe bound".into());
    }
    let maximum_probe_work = len
        .checked_mul(probes_per_key)
        .and_then(|work| work.checked_mul(2))
        .ok_or("flight-key retirement probe-work counter overflow")?;
    if maximum_probe_work > MAX_FLIGHT_KEY_RECLAIM_PROBE_WORK {
        return Err("flight-key retirement probe-work budget exceeded".into());
    }
    let mut write = 0usize;
    for read in 0..len {
        if read >= MAX_FLIGHT_KEY_RECLAIM_WORK {
            return Err("flight-key retirement work budget exceeded".into());
        }
        let known = runtime
            .known_flights
            .get(read)
            .copied()
            .ok_or("flight-key retirement index invariant")?;
        let retired =
            flight::flight_driver_remove_retired(&mut runtime.flights, &known.key, now_ms)
                .map_err(|error| format!("flight-key retirement receipt: {error}"))?;
        if retired.is_none() && flight::flight_driver_has_key(&runtime.flights, known.key) {
            if write != read {
                let dest = runtime
                    .known_flights
                    .get_mut(write)
                    .ok_or("flight-key retirement destination invariant")?;
                *dest = known;
            }
            write = write
                .checked_add(1)
                .ok_or("flight-key retirement count overflow")?;
        }
    }
    runtime.known_flights.truncate(write);
    Ok(())
}

/// Capacity and bounded lookup preflight, before the flight driver, quarantine queue, route store,
/// or surveillance subsystem commits this item. `Some` reserves a new exact key for commit.
fn preflight_flight_key(
    runtime: &mut RegionalRuntime,
    event: &crate::input::NormalizedEvent,
) -> Result<Option<(oa_flight::FlightKey, usize)>, String> {
    let Some(key) = event_flight_key(event) else {
        return Ok(None);
    };
    if runtime.known_flights.len() >= runtime.profile.max_flights {
        reclaim_retired_flight_keys(runtime, event.receipt_ns / 1_000_000)?;
    }
    match flight_key_search(&runtime.known_flights, key)? {
        Ok(_) => Ok(None),
        Err(insert_at) if runtime.known_flights.len() < runtime.profile.max_flights => {
            if runtime.known_flights.len() > MAX_FLIGHT_KEY_INSERT_MOVES {
                return Err("flight-key insertion move budget exceeded before item commit".into());
            }
            Ok(Some((key, insert_at)))
        }
        Err(_) => Err("known flight view capacity exhausted before item commit".into()),
    }
}

fn commit_flight_key(
    runtime: &mut RegionalRuntime,
    pending: Option<(oa_flight::FlightKey, usize)>,
    event: &crate::input::NormalizedEvent,
    outcome: flight::FlightApplyOutcome,
) -> Result<(), String> {
    let Some(key) = event_flight_key(event) else {
        return Ok(());
    };
    if outcome.adapter_refusal.is_some()
        || !matches!(outcome.operation, Some(oa_flight::Outcome::Applied))
    {
        return Ok(());
    }
    let source_current = flight_source_is_current(
        event.source_ns,
        event.input_receipt_ns,
        surveillance::tracker_drop_after_ns(&runtime.surveillance),
    );
    match pending {
        Some((pending_key, insert_at)) if pending_key == key => {
            runtime.known_flights.insert(
                insert_at,
                KnownFlight {
                    key,
                    last_consumed_receipt_ns: source_current.then_some(event.input_receipt_ns),
                },
            );
        }
        Some(_) => return Err("flight-key reservation does not match committed event".into()),
        None => {
            let index = match flight_key_search(&runtime.known_flights, key)? {
                Ok(index) => index,
                Err(_) => {
                    return Err("successful flight event missing from bounded key ledger".into());
                }
            };
            let known = runtime
                .known_flights
                .get_mut(index)
                .ok_or("successful flight key ledger index invariant")?;
            if source_current {
                known.last_consumed_receipt_ns = Some(event.input_receipt_ns);
            }
        }
    }
    Ok(())
}

fn flight_source_is_current(source_ns: i64, receipt_ns: i64, drop_after_ns: i64) -> bool {
    source_ns >= 0
        && receipt_ns >= 0
        && drop_after_ns >= 0
        && source_ns <= receipt_ns
        && receipt_ns
            .checked_sub(source_ns)
            .is_some_and(|age| age <= drop_after_ns)
}

fn runtime_item_outcome(
    runtime: &RegionalRuntime,
    event: &crate::input::NormalizedEvent,
    item_index: usize,
    batch_ordinal: u64,
    reasons: u16,
    clock_origin: Instant,
    item_started: Instant,
) -> Result<InputOutcome, String> {
    let input_source = quarantine_source(event)?;
    let started_at_ns = i64::try_from(
        item_started
            .checked_duration_since(clock_origin)
            .ok_or("runtime quarantine clock regressed")?
            .as_nanos(),
    )
    .map_err(|_| "runtime quarantine start clock overflow")?;
    let completed_at_ns = i64::try_from(clock_origin.elapsed().as_nanos())
        .map_err(|_| "runtime quarantine completion clock overflow")?;
    Ok(InputOutcome {
        identity: InputIdentity {
            source: input_source,
            source_instance: runtime.session_namespace,
            item_id: runtime_item_id(
                batch_ordinal,
                item_index,
                runtime.profile.max_events_per_receipt,
            )?,
        },
        refusal_reasons: ReasonFlags(reasons),
        clock_domain_id: QUARANTINE_CLOCK_DOMAIN_ID,
        started_at_ns,
        completed_at_ns,
        // This is offline processing. Duration is observed only; input work is bounded by the
        // route converter, fixed batch size, and bounded event fields, not interrupted by time.
        elapsed_limit_ns: u64::MAX,
        work_units: MAX_RUNTIME_ITEM_WORK,
        work_limit: MAX_RUNTIME_ITEM_WORK,
    })
}

fn quarantine_runtime_item(
    runtime: &mut RegionalRuntime,
    event: &crate::input::NormalizedEvent,
    item_index: usize,
    batch_ordinal: u64,
    reasons: u16,
    clock_origin: Instant,
    item_started: Instant,
) -> Result<RuntimeQuarantineRecord, String> {
    let input_source = quarantine_source(event)?;
    let outcome = runtime_item_outcome(
        runtime,
        event,
        item_index,
        batch_ordinal,
        reasons,
        clock_origin,
        item_started,
    )?;
    match route_input(&mut runtime.quarantine_queue, outcome)
        .map_err(|error| format!("quarantine core invariant: {error:?}"))?
    {
        Disposition::Queued(item) => Ok(RuntimeQuarantineRecord {
            item_index: u16::try_from(item_index).map_err(|_| "quarantine item index overflow")?,
            input_source,
            identity: item.identity,
            reasons: item.reasons,
            manual_handling_required: false,
        }),
        Disposition::ManualHandlingRequired(item) => Ok(RuntimeQuarantineRecord {
            item_index: u16::try_from(item_index).map_err(|_| "quarantine item index overflow")?,
            input_source,
            identity: item.identity,
            reasons: item.reasons,
            manual_handling_required: true,
        }),
        Disposition::Accepted => Err("refused runtime item unexpectedly accepted".into()),
    }
}

fn accept_runtime_item(
    runtime: &mut RegionalRuntime,
    event: &crate::input::NormalizedEvent,
    item_index: usize,
    batch_ordinal: u64,
    clock_origin: Instant,
    item_started: Instant,
) -> Result<(), String> {
    let outcome = runtime_item_outcome(
        runtime,
        event,
        item_index,
        batch_ordinal,
        0,
        clock_origin,
        item_started,
    )?;
    match route_input(&mut runtime.quarantine_queue, outcome)
        .map_err(|error| format!("quarantine core invariant: {error:?}"))?
    {
        Disposition::Accepted => Ok(()),
        Disposition::Queued(_) | Disposition::ManualHandlingRequired(_) => {
            Err("accepted runtime item was unexpectedly refused by quarantine core".into())
        }
    }
}

fn quarantine_stats(
    queue: &QuarantineQueue,
) -> Result<[SourceStats; oa_quarantine::INPUT_SOURCE_COUNT], String> {
    Ok([
        source_stats(queue, InputSource::FlightData)
            .map_err(|error| format!("quarantine flight stats: {error:?}"))?,
        source_stats(queue, InputSource::Surveillance)
            .map_err(|error| format!("quarantine surveillance stats: {error:?}"))?,
        source_stats(queue, InputSource::Adaptation)
            .map_err(|error| format!("quarantine adaptation stats: {error:?}"))?,
    ])
}

fn surveillance_refusal_reason(error: surveillance::SurveillanceStageError) -> Option<u16> {
    match error {
        surveillance::SurveillanceStageError::FailedSession => None,
        surveillance::SurveillanceStageError::BarometricEvidenceCapacity => {
            Some(REASON_RESOURCE_LIMIT)
        }
        surveillance::SurveillanceStageError::WrongSourceKind
        | surveillance::SurveillanceStageError::InvalidPosition => {
            Some(REASON_SURVEILLANCE_INVALID)
        }
        surveillance::SurveillanceStageError::SealedTimestamp
        | surveillance::SurveillanceStageError::WrongReceiptOrTimestamp => {
            Some(REASON_INPUT_INVALID)
        }
    }
}

fn performance_for_key(
    rows: &[PerformanceEnvelope],
    key: oa_legacy_traj::core::trajectory::FlightKey,
    cutoff_ms: i64,
) -> Result<Option<PerformanceEnvelope>, &'static str> {
    let mut selected = None;
    for envelope in rows {
        // Availability and source time are the outer causal gate. Future payload fields are never
        // inspected and cannot suppress an eligible exact-key row.
        if envelope.available_ms > cutoff_ms || envelope.source_ms > cutoff_ms {
            continue;
        }
        if envelope.key == key {
            if selected.is_some() {
                return Err("duplicate eligible performance envelopes for full flight key");
            }
            selected = Some(*envelope);
        }
    }
    Ok(selected)
}

pub fn regional_new(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
) -> Result<RegionalRuntime, String> {
    regional_new_with_profile_and_namespace(
        model,
        snapshot_sha256,
        centre,
        cfg,
        DEFAULT_REGIONAL_RUN_PROFILE,
        DEFAULT_SESSION_NAMESPACE,
    )
}

/// Construct the runtime from the exact model verified from an independently pinned issue.
/// The model and identity enter as one `VerifiedPackage`; callers cannot pair a caller-supplied
/// model with a claimed digest. The runtime recomputes and checks the model fingerprint before
/// accepting the package identity.
pub fn regional_new_from_issued_package(
    package: VerifiedPackage,
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
) -> Result<RegionalRuntime, String> {
    regional_new_from_issued_package_with_audit_checkpoint(
        package,
        centre,
        cfg,
        profile,
        oa_audit_core::genesis(),
    )
}

/// Construct from the independently verified adaptation model plus a separate runtime-supplied
/// final-monitor configuration. The monitor config is caller-provided and is not part of the
/// issued package manifest or its authentication boundary.
pub fn regional_new_from_issued_package_with_runtime_final_monitor(
    package: VerifiedPackage,
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    monitor: RuntimeFinalMonitorConfig,
) -> Result<RegionalRuntime, String> {
    let configured = configure_final_monitor(monitor, profile.surveillance)?;
    let mut runtime = regional_new_from_issued_package(package, centre, cfg, profile)?;
    runtime.final_monitor = Some(configured);
    Ok(runtime)
}

/// Construct an analysis runtime with caller-supplied final-monitor geometry and explicit
/// full-key runway assignments. The config is not claimed to be issued or facility-authenticated.
pub fn regional_new_with_runtime_final_monitor(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    monitor: RuntimeFinalMonitorConfig,
) -> Result<RegionalRuntime, String> {
    let configured = configure_final_monitor(monitor, profile.surveillance)?;
    let mut runtime = regional_new_with_profile(model, snapshot_sha256, centre, cfg, profile)?;
    runtime.final_monitor = Some(configured);
    Ok(runtime)
}

fn validate_alert_composition(
    profile: surveillance::SurveillanceProfile,
    final_rows: usize,
    msaw_rows: usize,
) -> Result<(), String> {
    let total_rows = final_rows
        .checked_add(msaw_rows)
        .ok_or("composed alert row count overflow")?;
    if total_rows > profile.max_tracks {
        return Err(format!(
            "composed MSAW/final alert rows {total_rows} exceed shared profile reservation {}",
            profile.max_tracks
        ));
    }
    if !profile.render_scope {
        return Ok(());
    }
    if profile.frame_width < 500 {
        return Err(format!(
            "final-monitor status requires rendered width at least 500 px; profile width is {}",
            profile.frame_width
        ));
    }
    if profile.frame_height <= surveillance::REGIONAL_STATUS_HEADER_INSET_PX {
        return Err(format!(
            "final-monitor status requires frame height greater than {} px; profile height is {}",
            surveillance::REGIONAL_STATUS_HEADER_INSET_PX,
            profile.frame_height
        ));
    }
    if final_rows > MAX_FINAL_APPROACH_ASSIGNMENTS || msaw_rows > MAX_FINAL_APPROACH_ASSIGNMENTS {
        return Err(format!(
            "composed alert family exceeds the {}-row marker presenter limit",
            MAX_FINAL_APPROACH_ASSIGNMENTS
        ));
    }

    let marker = oa_display::core::alert_markers::ALERT_CIRCLES_PER_MARKER;
    let circles_needed = total_rows
        .checked_mul(marker)
        .ok_or("composed alert circle requirement overflow")?;
    let circles_reserved = profile
        .max_tracks
        .checked_mul(marker)
        .ok_or("profile alert circle reservation overflow")?;
    if circles_needed > circles_reserved {
        return Err(
            "composed MSAW/final alert circles exceed the shared display reservation".into(),
        );
    }

    let header_count =
        (if final_rows > 0 { 1_usize } else { 0 }) + (if msaw_rows > 0 { 1_usize } else { 0 });
    let label_texts = total_rows
        .checked_mul(oa_display::core::alert_markers::ALERT_TEXTS_PER_MARKER)
        .ok_or("composed alert label count overflow")?;
    let mode_rows = maximum_operator_status_bytes()
        .checked_add(MAX_OPERATOR_CONSOLE_BYTES)
        .and_then(|bytes| bytes.checked_add(OPERATOR_CONSOLE_SEPARATOR_BYTES))
        .and_then(|bytes| bytes.checked_add(63))
        .and_then(|bytes| bytes.checked_div(64))
        .ok_or("operator status row requirement overflow")?;
    let texts_needed = label_texts
        .checked_add(header_count)
        .and_then(|count| count.checked_add(mode_rows))
        .ok_or("composed alert text row requirement overflow")?;
    let texts_reserved = profile
        .max_tracks
        .checked_mul(oa_display::core::alert_markers::ALERT_TEXTS_PER_MARKER)
        .and_then(|count| count.checked_add(oa_display::core::alert_markers::ALERT_HEADER_TEXTS))
        .and_then(|count| count.checked_add(surveillance::STATUS_AUX_TEXT_ROWS))
        .ok_or("profile alert text reservation overflow")?;
    if texts_needed > texts_reserved {
        return Err(
            "composed MSAW/final headers exceed the shared display text-row reservation".into(),
        );
    }

    let marker_bytes = total_rows
        .checked_mul(oa_display::core::alert_markers::ALERT_TEXT_BYTES_PER_MARKER)
        .ok_or("composed alert label byte requirement overflow")?;
    let final_header_bytes = if final_rows > 0 {
        oa_display::core::alert_markers::FINAL_MONITOR_HEADER_TEXT_BYTES
    } else {
        0
    };
    let msaw_header_bytes = if msaw_rows > 0 {
        oa_display::core::alert_markers::MSAW_HEADER_TEXT_BYTES
    } else {
        0
    };
    let header_bytes = final_header_bytes
        .checked_add(msaw_header_bytes)
        .ok_or("composed alert header byte requirement overflow")?;
    let bytes_needed = marker_bytes
        .checked_add(header_bytes)
        .and_then(|bytes| bytes.checked_add(MAX_OPERATOR_CONSOLE_BYTES))
        .and_then(|bytes| bytes.checked_add(OPERATOR_CONSOLE_SEPARATOR_BYTES))
        .and_then(|bytes| bytes.checked_add(maximum_operator_status_bytes()))
        .ok_or("composed alert text byte requirement overflow")?;
    let bytes_reserved = profile
        .max_tracks
        .checked_mul(oa_display::core::alert_markers::ALERT_TEXT_BYTES_PER_MARKER)
        .and_then(|bytes| {
            bytes.checked_add(oa_display::core::alert_markers::ALERT_HEADER_TEXT_BYTES)
        })
        .and_then(|bytes| bytes.checked_add(surveillance::STATUS_AUX_TEXT_BYTES))
        .ok_or("profile alert text byte reservation overflow")?;
    if bytes_needed > bytes_reserved {
        return Err(
            "composed MSAW/final headers exceed the shared display text-byte reservation".into(),
        );
    }
    Ok(())
}

fn maximum_operator_status_bytes() -> usize {
    [
        RegionalAutomationStatus {
            selected: RegionalAutomationMode::FlightDataOnly,
            effective: EffectiveAutomationMode::FlightDataOnly,
            degradation: AutomationDegradation::SurveillanceUnavailable,
            fusion_off: true,
        },
        RegionalAutomationStatus {
            selected: RegionalAutomationMode::Surveillance,
            effective: EffectiveAutomationMode::Surveillance,
            degradation: AutomationDegradation::SurveillanceUnavailable,
            fusion_off: true,
        },
        RegionalAutomationStatus {
            selected: RegionalAutomationMode::FlightDataOnly,
            effective: EffectiveAutomationMode::FlightDataOnly,
            degradation: AutomationDegradation::OperatorSelectedFlightDataOnly,
            fusion_off: true,
        },
    ]
    .iter()
    .map(|status| automation_status_label(*status).len())
    .fold(0, usize::max)
}

fn configure_final_monitor(
    monitor: RuntimeFinalMonitorConfig,
    profile: surveillance::SurveillanceProfile,
) -> Result<ConfiguredFinalMonitor, String> {
    if monitor.assignments.is_empty() {
        return Err("final-monitor requires at least one runway assignment".into());
    }
    if monitor.assignments.len() > MAX_FINAL_APPROACH_ASSIGNMENTS {
        return Err(format!(
            "final-monitor assignment count must be within the renderable bound of {MAX_FINAL_APPROACH_ASSIGNMENTS}"
        ));
    }
    if monitor.assignments.len() > profile.max_tracks {
        return Err(format!(
            "final-monitor assignment count {} exceeds selected profile track capacity {}",
            monitor.assignments.len(),
            profile.max_tracks
        ));
    }
    validate_alert_composition(profile, monitor.assignments.len(), 0)?;
    let mut validation_state = final_monitor_core::FinalMonitorState::new();
    let empty: [u32; 0] = [];
    let empty_f64: [f64; 0] = [];
    let empty_i32: [i32; 0] = [];
    let mut empty_pair_ids: [final_monitor_core::FinalPairId; 0] = [];
    let mut empty_severity: [final_monitor_core::FinalSeverity; 0] = [];
    let mut empty_u32: [u32; 0] = [];
    final_monitor_core::update(
        monitor.pair,
        &mut validation_state,
        1,
        1,
        final_monitor_core::PredictedTracks {
            addresses: &empty,
            east_m: &empty_f64,
            north_m: &empty_f64,
            velocity_east_mps: &empty_f64,
            velocity_north_mps: &empty_f64,
            r95_m: &empty_f64,
            lat_e7: &empty_i32,
            lon_e7: &empty_i32,
        },
        final_monitor_core::FinalAlertBuffer {
            addresses: &mut [],
            pair_ids: &mut empty_pair_ids,
            severity: &mut empty_severity,
            lat_e7: &mut [],
            lon_e7: &mut [],
            seconds_to_entry_milli: &mut empty_u32,
        },
    )
    .map_err(|error| format!("final-monitor pair refused: {error:?}"))?;
    let pair_id = monitor.pair.id;
    for (index, assignment) in monitor.assignments.iter().enumerate() {
        let valid_end = (assignment.runway_id == pair_id.first_runway_id
            && assignment.end_name == pair_id.first_end_name)
            || (assignment.runway_id == pair_id.second_runway_id
                && assignment.end_name == pair_id.second_end_name);
        if !valid_end || !valid_flight_key(assignment.flight) {
            return Err(
                "final-monitor assignment does not name a pair end and full flight key".into(),
            );
        }
        if monitor
            .assignments
            .get(..index)
            .is_some_and(|prior| prior.iter().any(|row| row.flight == assignment.flight))
        {
            return Err("duplicate full flight key in final-monitor assignments".into());
        }
    }
    Ok(ConfiguredFinalMonitor {
        pair: monitor.pair,
        assignments: monitor.assignments,
        state: final_monitor_core::FinalMonitorState::new(),
        latest_alerts: Vec::with_capacity(MAX_FINAL_APPROACH_ASSIGNMENTS),
        loss_reason: None,
    })
}

fn valid_flight_key(key: oa_flight::FlightKey) -> bool {
    match key.kind {
        oa_flight::KEY_GUFI => {
            key.gufi != 0 && key.callsign == [0; 8] && key.dep == [0; 12] && key.arr == [0; 12]
        }
        oa_flight::KEY_TFMS => {
            key.gufi == 0
                && key.callsign.iter().any(|byte| *byte != 0)
                && key.dep.iter().any(|byte| *byte != 0)
                && key.arr.iter().any(|byte| *byte != 0)
        }
        _ => false,
    }
}

fn flight_key_from_view(view: &SharedFlightView) -> Option<oa_flight::FlightKey> {
    if view.gufi != 0 {
        return Some(oa_flight::key_gufi(view.gufi));
    }
    let key = oa_flight::key_tfms(view.callsign, view.departure, view.arrival);
    valid_flight_key(key).then_some(key)
}

fn prepare_final_monitor_tracks(
    monitor: &ConfiguredFinalMonitor,
    frame: &SurveillanceFrame,
    flight_data: &[SharedFlightView],
    unavailable_flight_data: u32,
) -> Result<Vec<FinalMonitorTrack>, FinalMonitorDegradation> {
    if unavailable_flight_data != 0 {
        return Err(FinalMonitorDegradation::MissingAssignedTrack);
    }
    let origin = monitor
        .pair
        .projection_origin
        .ok_or(FinalMonitorDegradation::InvalidPrediction)?;
    let plane = oa_runway::plane::plane_new(origin.lat_deg, origin.lon_deg)
        .ok_or(FinalMonitorDegradation::InvalidPrediction)?;
    let mut tracks = Vec::with_capacity(monitor.assignments.len());
    for view in flight_data {
        if view.plan_state != oa_flight::ST_ACTIVE {
            continue;
        }
        let Some(key) = flight_key_from_view(view) else {
            continue;
        };
        let Some(assignment) = monitor.assignments.iter().find(|row| row.flight == key) else {
            continue;
        };
        if tracks.len() >= monitor.assignments.len() {
            return Err(FinalMonitorDegradation::InvalidPrediction);
        }
        let mut matching_tracks = frame
            .tracks
            .iter()
            .filter(|track| track.address == view.address);
        let Some(track) = matching_tracks.next() else {
            return Err(FinalMonitorDegradation::MissingAssignedTrack);
        };
        if matching_tracks.next().is_some() || track.address == 0 || track.newest_source_ns < 0 {
            return Err(FinalMonitorDegradation::InvalidPrediction);
        }
        let geo = oa_runway::plane::Geo {
            lat_deg: f64::from(track.lat_e7) / 10_000_000.0,
            lon_deg: f64::from(track.lon_e7) / 10_000_000.0,
        };
        let local = oa_runway::plane::to_local(&plane, geo)
            .ok_or(FinalMonitorDegradation::InvalidPrediction)?;
        let local_velocity = oa_runway::plane::true_velocity_to_local(
            &plane,
            geo,
            f64::from(track.east_mmps) / 1000.0,
            f64::from(track.north_mmps) / 1000.0,
        )
        .ok_or(FinalMonitorDegradation::InvalidPrediction)?;
        tracks.push(FinalMonitorTrack {
            address: track.address,
            assigned_runway_id: assignment.runway_id,
            east_m: local.east_m,
            north_m: local.north_m,
            velocity_east_mps: local_velocity.east_m,
            velocity_north_mps: local_velocity.north_m,
            r95_m: f64::from(track.r95_mm) / 1000.0,
            lat_e7: track.lat_e7,
            lon_e7: track.lon_e7,
            newest_source_ns: track.newest_source_ns,
        });
    }
    if tracks.len() != monitor.assignments.len() {
        return Err(FinalMonitorDegradation::MissingAssignedTrack);
    }
    tracks.sort_by_key(|track| track.address);
    if tracks.windows(2).any(|rows| {
        rows.first()
            .zip(rows.get(1))
            .is_some_and(|(left, right)| left.address == right.address)
    }) {
        return Err(FinalMonitorDegradation::InvalidPrediction);
    }
    Ok(tracks)
}

fn evaluate_final_monitor(
    monitor: &mut ConfiguredFinalMonitor,
    tracks: &[FinalMonitorTrack],
    now_ns: u64,
    prediction_report_ns: u64,
) -> Result<(final_monitor_core::UpdateResult, FinalMonitorBuffers), final_monitor_core::UpdateError>
{
    let addresses: Vec<u32> = tracks.iter().map(|track| track.address).collect();
    let east_m: Vec<f64> = tracks.iter().map(|track| track.east_m).collect();
    let north_m: Vec<f64> = tracks.iter().map(|track| track.north_m).collect();
    let velocity_east_mps: Vec<f64> = tracks.iter().map(|track| track.velocity_east_mps).collect();
    let velocity_north_mps: Vec<f64> = tracks
        .iter()
        .map(|track| track.velocity_north_mps)
        .collect();
    let r95_m: Vec<f64> = tracks.iter().map(|track| track.r95_m).collect();
    let lat_e7: Vec<i32> = tracks.iter().map(|track| track.lat_e7).collect();
    let lon_e7: Vec<i32> = tracks.iter().map(|track| track.lon_e7).collect();
    let mut output = FinalMonitorBuffers {
        addresses: vec![0; tracks.len()],
        pair_ids: vec![monitor.pair.id; tracks.len()],
        severity: vec![final_monitor_core::FinalSeverity::Caution; tracks.len()],
        lat_e7: vec![0; tracks.len()],
        lon_e7: vec![0; tracks.len()],
        seconds_to_entry_milli: vec![0; tracks.len()],
    };
    let result = final_monitor_core::update(
        monitor.pair,
        &mut monitor.state,
        now_ns,
        prediction_report_ns,
        final_monitor_core::PredictedTracks {
            addresses: &addresses,
            east_m: &east_m,
            north_m: &north_m,
            velocity_east_mps: &velocity_east_mps,
            velocity_north_mps: &velocity_north_mps,
            r95_m: &r95_m,
            lat_e7: &lat_e7,
            lon_e7: &lon_e7,
        },
        final_monitor_core::FinalAlertBuffer {
            addresses: &mut output.addresses,
            pair_ids: &mut output.pair_ids,
            severity: &mut output.severity,
            lat_e7: &mut output.lat_e7,
            lon_e7: &mut output.lon_e7,
            seconds_to_entry_milli: &mut output.seconds_to_entry_milli,
        },
    )?;
    Ok((result, output))
}

fn regional_alert_from_buffers(
    index: usize,
    tracks: &[FinalMonitorTrack],
    output: &FinalMonitorBuffers,
) -> Option<RegionalFinalMonitorAlert> {
    let address = output.addresses.get(index).copied()?;
    let pair_id = output.pair_ids.get(index).copied()?;
    let severity = output.severity.get(index).copied()?;
    let lat_e7 = output.lat_e7.get(index).copied()?;
    let lon_e7 = output.lon_e7.get(index).copied()?;
    let seconds_to_entry_milli = output.seconds_to_entry_milli.get(index).copied()?;
    let assigned_runway_id = tracks
        .iter()
        .find(|track| track.address == address)?
        .assigned_runway_id;
    Some(RegionalFinalMonitorAlert {
        address,
        pair_id,
        assigned_runway_id,
        severity,
        lat_e7,
        lon_e7,
        seconds_to_entry_milli,
    })
}

fn regional_final_monitor_frame(
    status: FinalMonitorStatus,
    alerts: Vec<RegionalFinalMonitorAlert>,
    timing: Option<RegionalFinalMonitorTiming>,
) -> RegionalFinalMonitorFrame {
    RegionalFinalMonitorFrame {
        status,
        alerts,
        timing,
    }
}

fn present_final_monitor_controls(
    surveillance: &mut Surveillance,
    interaction: &oa_display::interaction::InteractionState,
    console: &str,
    display: &RegionalFinalMonitorFrame,
) -> Result<[u8; 32], String> {
    if display.status == FinalMonitorStatus::NotConfigured {
        return surveillance::surveillance_present_controls(surveillance, interaction, console);
    }
    if display.alerts.len() > MAX_FINAL_APPROACH_ASSIGNMENTS {
        return Err("final-monitor display exceeds its admitted marker bound".into());
    }
    let mut markers = Vec::new();
    markers
        .try_reserve_exact(display.alerts.len())
        .map_err(|_| "final-monitor marker allocation refused")?;
    for alert in &display.alerts {
        markers.push(oa_display::core::alert_markers::FinalMonitorMarker {
            address: alert.address,
            lat_e7: alert.lat_e7,
            lon_e7: alert.lon_e7,
            severity: alert.severity as u8,
            position_known: 1,
            reserved: [0; 2],
        });
    }
    surveillance::surveillance_present_controls_with_final_monitor(
        surveillance,
        interaction,
        console,
        &markers,
        display.status != FinalMonitorStatus::Available,
    )
}

fn regional_final_monitor_timing(
    result: final_monitor_core::UpdateResult,
) -> RegionalFinalMonitorTiming {
    RegionalFinalMonitorTiming {
        observed_input_interval_ns: result.observed_input_interval_ns,
        observed_evaluation_interval_ns: result.observed_interval_ns,
        prediction_age_ns: result.prediction_age_ns,
    }
}

fn final_monitor_degradation(result: final_monitor_core::UpdateResult) -> FinalMonitorDegradation {
    match (result.input_cadence_degraded, result.prediction_stale) {
        (true, true) => FinalMonitorDegradation::StalePredictionAndInputCadence,
        (true, false) => FinalMonitorDegradation::InputCadence,
        (false, true) => FinalMonitorDegradation::StalePrediction,
        (false, false) => FinalMonitorDegradation::CoreRefusal,
    }
}

fn configured_final_monitor_frame(
    monitor: Option<&mut ConfiguredFinalMonitor>,
    frame: &SurveillanceFrame,
    flight_data: &[SharedFlightView],
    unavailable_flight_data: u32,
) -> RegionalFinalMonitorFrame {
    let Some(monitor) = monitor else {
        return regional_final_monitor_frame(FinalMonitorStatus::NotConfigured, Vec::new(), None);
    };
    let tracks =
        match prepare_final_monitor_tracks(monitor, frame, flight_data, unavailable_flight_data) {
            Ok(tracks) => tracks,
            Err(reason) => return degrade_final_monitor(monitor, reason, None),
        };
    let now_ns = match u64::try_from(frame.time_ns) {
        Ok(value) => value,
        Err(_) => {
            return degrade_final_monitor(
                monitor,
                FinalMonitorDegradation::InvalidPrediction,
                None,
            );
        }
    };
    let prediction_report_ns = tracks
        .iter()
        .map(|track| track.newest_source_ns)
        .min()
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(now_ns);
    let (result, output) =
        match evaluate_final_monitor(monitor, &tracks, now_ns, prediction_report_ns) {
            Ok(result) => result,
            Err(final_monitor_core::UpdateError::UnusableUncertainty) => {
                return degrade_final_monitor(
                    monitor,
                    FinalMonitorDegradation::UnusableUncertainty,
                    None,
                );
            }
            Err(_) => {
                return degrade_final_monitor(monitor, FinalMonitorDegradation::CoreRefusal, None);
            }
        };
    let timing = Some(regional_final_monitor_timing(result));
    if result.degraded {
        return degrade_final_monitor(monitor, final_monitor_degradation(result), timing);
    }
    if result.due {
        let mut latest_alerts = Vec::new();
        if latest_alerts
            .try_reserve_exact(usize::from(result.alerts))
            .is_err()
        {
            return degrade_final_monitor(monitor, FinalMonitorDegradation::CoreRefusal, timing);
        }
        for index in 0..usize::from(result.alerts) {
            let Some(alert) = regional_alert_from_buffers(index, &tracks, &output) else {
                return degrade_final_monitor(
                    monitor,
                    FinalMonitorDegradation::CoreRefusal,
                    timing,
                );
            };
            latest_alerts.push(alert);
        }
        monitor.latest_alerts = latest_alerts;
        monitor.loss_reason = None;
    } else if let Some(reason) = monitor.loss_reason {
        return regional_final_monitor_frame(
            FinalMonitorStatus::Degraded(reason),
            Vec::new(),
            timing,
        );
    }
    regional_final_monitor_frame(
        FinalMonitorStatus::Available,
        monitor.latest_alerts.clone(),
        timing,
    )
}

fn degrade_final_monitor(
    monitor: &mut ConfiguredFinalMonitor,
    reason: FinalMonitorDegradation,
    timing: Option<RegionalFinalMonitorTiming>,
) -> RegionalFinalMonitorFrame {
    monitor.latest_alerts.clear();
    monitor.loss_reason = Some(reason);
    regional_final_monitor_frame(FinalMonitorStatus::Degraded(reason), Vec::new(), timing)
}

/// Construct from a verified issued package while preserving the caller's durable audit-chain
/// checkpoint for subsequent quarantine maintenance.
pub fn regional_new_from_issued_package_with_audit_checkpoint(
    package: VerifiedPackage,
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    audit_checkpoint: oa_audit_core::Checkpoint,
) -> Result<RegionalRuntime, String> {
    let identity = package.identity();
    regional_new_with_source_identity_and_checkpoint(
        RegionalModelSource {
            model: package.model().clone(),
            expected_snapshot_sha256: identity.snapshot_sha256,
            issued_manifest_sha256: Some(identity.manifest_sha256),
        },
        centre,
        cfg,
        profile,
        DEFAULT_SESSION_NAMESPACE,
        audit_checkpoint,
    )
}

/// Construct a runtime with an explicit nonzero namespace for normalized-item quarantine IDs.
pub fn regional_new_with_namespace(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    session_namespace: u16,
) -> Result<RegionalRuntime, String> {
    regional_new_with_profile_and_namespace(
        model,
        snapshot_sha256,
        centre,
        cfg,
        DEFAULT_REGIONAL_RUN_PROFILE,
        session_namespace,
    )
}

/// Construct an offline runtime with an explicit fixed-capacity profile.
pub fn regional_new_with_profile(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
) -> Result<RegionalRuntime, String> {
    regional_new_with_profile_and_namespace(
        model,
        snapshot_sha256,
        centre,
        cfg,
        profile,
        DEFAULT_SESSION_NAMESPACE,
    )
}

/// Construct a regional replay with the caller's trusted durable quarantine-audit checkpoint.
/// The shell append verifies this value against the actual audit file before any release mutation.
pub fn regional_new_with_profile_and_quarantine_audit_checkpoint(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    checkpoint: oa_audit_core::Checkpoint,
) -> Result<RegionalRuntime, String> {
    regional_new_with_profile_and_namespace_and_audit_checkpoint(
        model,
        snapshot_sha256,
        centre,
        cfg,
        profile,
        DEFAULT_SESSION_NAMESPACE,
        checkpoint,
    )
}

/// Result of an explicit manual quarantine handoff, bound to the durable audit chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuarantineMaintenanceReceipt {
    pub event: QuarantineEvent,
    pub audit_checkpoint: oa_audit_core::Checkpoint,
}

/// Audit and acknowledge the selected front item as an explicit manual-responsibility handoff.
/// The principal, registry, timestamp and note are caller-supplied inputs. The registry role is a
/// local configuration claim, not authenticated identity, and this path does not imply that the
/// external manual workflow itself was performed.
pub struct QuarantineMaintenanceRequest<'a> {
    pub source: InputSource,
    pub expected_identity: InputIdentity,
    pub audit_log: &'a Path,
    pub expected_checkpoint: oa_audit_core::Checkpoint,
    pub principal_registry: &'a oa_audit_core::PrincipalRegistry,
    pub principal: u64,
    pub time_ns: u64,
    pub manual_handoff_ref: &'a str,
}

pub fn regional_acknowledge_quarantine_before_commit(
    runtime: &mut RegionalRuntime,
    request: QuarantineMaintenanceRequest<'_>,
) -> Result<QuarantineMaintenanceReceipt, String> {
    let QuarantineMaintenanceRequest {
        source,
        expected_identity,
        audit_log,
        expected_checkpoint,
        principal_registry,
        principal,
        time_ns,
        manual_handoff_ref,
    } = request;
    if runtime.failed {
        return Err("session failed; reconstruct before quarantine maintenance".into());
    }
    if principal == 0 || time_ns == 0 {
        return Err("audit principal and timestamp must be positive".into());
    }
    let role = oa_audit_core::authorize_principal(
        principal_registry,
        principal,
        oa_audit_core::Action::QuarantineRelease,
    )
    .map_err(|error| format!("quarantine principal registry refused: {error:?}"))?;
    if manual_handoff_ref.is_empty()
        || manual_handoff_ref.len() > 256
        || !manual_handoff_ref
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte) && byte != b'\t')
    {
        return Err("manual-handoff reference must be 1..=256 printable ASCII bytes".into());
    }
    let front = queue_front(&runtime.quarantine_queue, source)
        .map_err(|error| format!("quarantine front: {error:?}"))?
        .ok_or_else(|| "quarantine source has no pending front item".to_owned())?;
    if front.identity != expected_identity || expected_identity.source != source {
        return Err("requested identity does not match the pending quarantine front".into());
    }
    let mut details = String::new();
    details
        .try_reserve_exact(512)
        .map_err(|_| "quarantine audit details allocation refused")?;
    writeln!(
        details,
        "QUARANTINE-RELEASE-V1\nsource\t{:?}\nsource_instance\t{}\nitem_id\t{}\nreasons\t{}\nclock_domain_id\t{}\ncompleted_at_ns\t{}\nmanual_handoff_ref\t{}",
        front.identity.source,
        front.identity.source_instance,
        front.identity.item_id,
        front.reasons.0,
        front.clock_domain_id,
        front.completed_at_ns,
        manual_handoff_ref
    )
    .map_err(|_| "quarantine audit detail formatting failed")?;
    let (target, target_len) = oa_quarantine::release_audit_target(front);
    let (details, details_len) = oa_audit_core::details(details.as_bytes())
        .map_err(|error| format!("quarantine audit details: {error:?}"))?;
    let event = oa_audit_core::Event {
        principal,
        role,
        action: oa_audit_core::Action::QuarantineRelease,
        target,
        target_len,
        time_ns,
        details,
        details_len,
    };
    oa_audit_core::validate_event(&event)
        .map_err(|error| format!("quarantine audit event: {error:?}"))?;
    let (audit_checkpoint, receipt) =
        oa_audit_log::shell::append_receipt(audit_log, expected_checkpoint, event)
            .map_err(|error| format!("quarantine audit append: {error:?}"))?;
    let released = queue_acknowledge(&mut runtime.quarantine_queue, source, receipt)
        .map_err(|error| format!("quarantine release commit: {error:?}"))?;
    Ok(QuarantineMaintenanceReceipt {
        event: released,
        audit_checkpoint,
    })
}

/// Returns the immutable validated capacities selected at construction.
pub fn regional_profile_limits(runtime: &RegionalRuntime) -> RegionalRunProfile {
    runtime.profile
}

/// Actual startup-reserved route-store bytes, measured from the vectors' capacities.
pub fn regional_route_store_reserved_bytes(runtime: &RegionalRuntime) -> Result<usize, String> {
    route::route_store_reserved_bytes(&runtime.routes)
        .ok_or_else(|| "route reservation byte count overflow".to_owned())
}

#[cfg(test)]
pub(crate) fn regional_route_store_counts(runtime: &RegionalRuntime) -> (usize, usize) {
    (
        runtime.routes.revisions.len(),
        runtime.routes.history_expiries.len(),
    )
}

#[cfg(test)]
pub(crate) fn regional_flight_reclamation_counts(runtime: &RegionalRuntime) -> (usize, u64) {
    (
        flight::flight_driver_len(&runtime.flights),
        flight::flight_driver_report(&runtime.flights).retirements_consumed,
    )
}

/// Apply one framed NAM FPL/CHG/DLA/CNL through the admitted peer session and the composed
/// runtime's authoritative flight table and route store. The caller supplies the exact positive
/// flight identity and capture time context; this boundary does not join from callsign or header ID.
pub fn regional_receive_nam_flight(
    runtime: &mut RegionalRuntime,
    ingress: &mut crate::nam_handoff::NamSessionIngress,
    framed_bytes: &[u8],
    now_ms: u64,
    context: NamFlightContext,
    variations: &[oa_airspace::shell::route_parse::MagneticVariationSource],
    profile: oa_nam::Profile,
) -> Result<
    (
        crate::nam_handoff::NamIngressResult,
        Option<NamFlightResult>,
    ),
    crate::nam_handoff::NamHandoffError,
> {
    nam_flight::nam_flight_receive(
        &mut runtime.nam_flights,
        &mut runtime.flights,
        &mut runtime.routes,
        ingress,
        framed_bytes,
        now_ms,
        context,
        &runtime.model,
        variations,
        profile,
    )
}

fn regional_new_with_profile_and_namespace(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    session_namespace: u16,
) -> Result<RegionalRuntime, String> {
    regional_new_with_source_identity_and_checkpoint(
        RegionalModelSource {
            model,
            expected_snapshot_sha256: snapshot_sha256,
            issued_manifest_sha256: None,
        },
        centre,
        cfg,
        profile,
        session_namespace,
        oa_audit_core::genesis(),
    )
}

fn regional_new_with_profile_and_namespace_and_audit_checkpoint(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    session_namespace: u16,
    audit_checkpoint: oa_audit_core::Checkpoint,
) -> Result<RegionalRuntime, String> {
    regional_new_with_source_identity_and_checkpoint(
        RegionalModelSource {
            model,
            expected_snapshot_sha256: snapshot_sha256,
            issued_manifest_sha256: None,
        },
        centre,
        cfg,
        profile,
        session_namespace,
        audit_checkpoint,
    )
}

struct RegionalModelSource {
    model: Model,
    expected_snapshot_sha256: [u8; 32],
    issued_manifest_sha256: Option<[u8; 32]>,
}

fn regional_new_with_source_identity_and_checkpoint(
    source: RegionalModelSource,
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    session_namespace: u16,
    audit_checkpoint: oa_audit_core::Checkpoint,
) -> Result<RegionalRuntime, String> {
    let RegionalModelSource {
        model,
        expected_snapshot_sha256,
        issued_manifest_sha256,
    } = source;
    if !crate::input::is_valid_centre_code(&centre) {
        return Err(
            "centre must be an alphanumeric code with optional trailing zero padding".into(),
        );
    }
    if session_namespace == 0 {
        return Err("session namespace must be nonzero".into());
    }
    if profile.max_flights == 0
        || profile.max_flights > crate::flight::MAX_OFFLINE_DRIVER_FLIGHTS
        || profile.max_route_revisions == 0
        || profile.max_route_revisions > route::MAX_CURRENT_ROUTE_PLANS
        || profile.max_flight_log_entries == 0
        || profile.max_flight_log_entries > MAX_DRIVER_LOG_ENTRIES
        || profile.max_total_events == 0
        || profile.max_total_events > crate::session::MAX_RECORDED_PROFILE_EVENTS
        || !matches!(
            profile.max_events_per_receipt,
            MAX_EVENTS_PER_RECEIPT | RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
        )
        || (profile.surveillance.render_scope
            && profile.max_flights > profile.surveillance.max_tracks)
    {
        return Err("regional profile exceeds fixed runtime bounds".into());
    }
    let encoded_model = oa_airspace::core::snapshot::encode(&model)
        .map_err(|error| format!("runtime model fingerprint encode: {error:?}"))?;
    let adaptation_sha256 = oa_sha256::sha256(&encoded_model);
    if adaptation_sha256 != expected_snapshot_sha256 {
        return Err("runtime model fingerprint does not match its admitted source identity".into());
    }
    let nam_flights = nam_flight::nam_flight_runtime_new(profile.max_flights)
        .ok_or("NAM flight-plan runtime capacity is invalid")?;
    let routes = route::route_store_new_with_capacity(
        adaptation_sha256,
        profile.max_route_revisions,
        profile.reclaim_current_route_by_key,
    );
    let route_bytes = route::route_store_reserved_bytes(&routes)
        .ok_or("route startup reservation byte count overflow")?;
    if route_bytes > route::MAX_CURRENT_ROUTE_POOL_BYTES {
        return Err("route startup reservation exceeds fixed 320 MiB budget".into());
    }
    Ok(RegionalRuntime {
        surveillance: surveillance::surveillance_new_with_profile(
            model.clone(),
            cfg,
            profile.surveillance,
        )?,
        model,
        adaptation_sha256,
        issued_manifest_sha256,
        flights: flight::flight_driver_new_bounded(
            FlightDriverConfig {
                flight_capacity: profile.max_flights,
                refusal_log_capacity: profile.max_flight_log_entries,
                conflict_log_capacity: profile.max_flight_log_entries,
            },
            profile.max_flights,
        )
        .map_err(|e| format!("flight startup: {e:?}"))?,
        routes,
        nam_flights,
        centre,
        frame_sequence: 0,
        failed: false,
        session_namespace,
        quarantine_queue: oa_quarantine::queue_new_with_audit_checkpoint(audit_checkpoint),
        interaction: oa_display::interaction::interaction_new(),
        views: Vec::with_capacity(profile.max_flights),
        observed: Vec::with_capacity(profile.surveillance.max_tracks),
        next_generation: 0,
        console_text: String::with_capacity(MAX_OPERATOR_CONSOLE_BYTES),
        profile,
        selected_mode: RegionalAutomationMode::Surveillance,
        last_automation: RegionalAutomationStatus {
            selected: RegionalAutomationMode::Surveillance,
            effective: EffectiveAutomationMode::FlightDataOnly,
            degradation: AutomationDegradation::SurveillanceUnavailable,
            fusion_off: true,
        },
        known_flights: Vec::with_capacity(profile.max_flights),
        final_monitor: None,
        final_monitor_display: regional_final_monitor_frame(
            FinalMonitorStatus::NotConfigured,
            Vec::new(),
            None,
        ),
    })
}

/// Read the final quarantine counters after an optional on-demand release.
pub fn regional_quarantine_stats(
    runtime: &RegionalRuntime,
) -> Result<[SourceStats; oa_quarantine::INPUT_SOURCE_COUNT], String> {
    quarantine_stats(&runtime.quarantine_queue)
}

/// Fingerprint captured from the model accepted by the runtime constructor.
pub fn regional_adaptation_sha256(runtime: &RegionalRuntime) -> [u8; 32] {
    runtime.adaptation_sha256
}

/// Snapshot-bound inventory of the public geometry rendered from this runtime's adaptation.
/// The summary identifies element counts by source/type and contains no flight-level data.
pub fn regional_map_evidence(runtime: &RegionalRuntime) -> Result<String, String> {
    crate::map::render_inventory(&runtime.model, &runtime.adaptation_sha256)
}

/// Independent issued manifest pin retained with this runtime, if startup used an issued package.
pub fn regional_issued_manifest_sha256(runtime: &RegionalRuntime) -> Option<[u8; 32]> {
    runtime.issued_manifest_sha256
}

fn execute(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
    performance: Option<&[PerformanceEnvelope]>,
) -> Result<RegionalFrame, String> {
    if batch.events.len() > runtime.profile.max_events_per_receipt
        || batch.events.iter().any(|e| {
            e.receipt_ns != batch.receipt_ns
                || e.input_receipt_ns < 0
                || e.input_receipt_ns > e.receipt_ns
                || e.source_ns > e.input_receipt_ns
                || oa_sealed::in_sealed_window(e.input_receipt_ns)
        })
    {
        return Err("malformed receipt batch".into());
    }
    if !runtime.profile.surveillance.render_scope
        && batch
            .events
            .iter()
            .any(|event| matches!(event.kind, crate::input::EventKind::Operator(_)))
    {
        return Err("operator requests are unavailable in the analysis-only replay profile".into());
    }
    let input = summarize_input_batch_with_limit(batch, runtime.profile.max_events_per_receipt)
        .map_err(|e| format!("input accounting: {e:?}"))?;
    let performance_cutoff_ms = match performance {
        Some(_) => Some(route::receipt_cutoff_ms(batch.receipt_ns)?),
        None => None,
    };
    let next_frame_sequence = runtime
        .frame_sequence
        .checked_add(1)
        .ok_or("frame sequence overflow")?;
    let batch_ordinal = next_frame_sequence;
    let processing_started = Instant::now();
    let mut quarantine_records = Vec::new();
    quarantine_records
        .try_reserve_exact(batch.events.len())
        .map_err(|_| "runtime quarantine output allocation refused")?;
    if runtime.final_monitor.is_some() {
        surveillance::surveillance_begin_for_monitor(&mut runtime.surveillance, batch.receipt_ns)?;
    } else {
        surveillance::surveillance_begin(&mut runtime.surveillance, batch.receipt_ns)?;
    }
    let mut order_cursor = flight::flight_order_cursor(&runtime.flights);
    let mut flight_operations = Vec::with_capacity(batch.events.len());
    let mut controller_dispatches = Vec::with_capacity(batch.events.len());
    for (item_index, event) in batch.events.iter().enumerate() {
        let item_started = Instant::now();
        let eligible = transaction_item(event);
        let order_result = flight::flight_stage_order(&runtime.flights, &order_cursor, event);
        if !eligible {
            order_result.map_err(|error| format!("flight adapter preflight: {error:?}"))?;
            if let crate::input::EventKind::Operator(request) = event.kind {
                let dispatch = commands::dispatch(
                    &mut runtime.flights,
                    &mut runtime.interaction,
                    &runtime.views,
                    &runtime.observed,
                    u32::try_from(runtime.frame_sequence).map_err(|_| "view sequence exhausted")?,
                    event,
                    request,
                );
                if dispatch.terminal_failure {
                    return Err(format!(
                        "controller bridge invariant failure: {}",
                        dispatch.refusal
                    ));
                }
                controller_dispatches.push(dispatch);
                flight_operations.push(FlightApplyOutcome {
                    operation: None,
                    adapter_refusal: None,
                    ignored_non_flight_event: true,
                });
            } else {
                flight_operations.push(
                    flight::flight_apply_event(&mut runtime.flights, event)
                        .map_err(|error| format!("flight adapter: {error:?}"))?,
                );
            }
            route::route_event(&mut runtime.routes, &runtime.model, event)?;
            surveillance::surveillance_event(&mut runtime.surveillance, event)?;
            flight::flight_commit_order(&mut order_cursor, event);
            continue;
        }

        let route_stage = route::route_stage_event(&runtime.routes, &runtime.model, event)?;
        let surveillance_stage =
            surveillance::surveillance_stage_event(&runtime.surveillance, event);
        let adapter_refusal = flight::flight_stage_adapter_refusal(&runtime.flights, event);
        let mut reasons = match order_result {
            Ok(()) => 0,
            Err(
                crate::flight::FlightDriverError::InvalidTimestamp
                | crate::flight::FlightDriverError::SealedTimestamp
                | crate::flight::FlightDriverError::NonMillisecondAdapterTimestamp
                | crate::flight::FlightDriverError::WrongSourceKind
                | crate::flight::FlightDriverError::InvalidBatchOrder,
            ) => REASON_INPUT_INVALID,
            Err(error) => return Err(format!("flight stage invariant: {error:?}")),
        };
        if let route::RouteStage::Refusal { reason, .. } = &route_stage {
            reasons |= *reason;
        }
        if adapter_refusal.is_some() {
            reasons |= REASON_INPUT_INVALID;
        }
        match surveillance_stage {
            Ok(()) => {}
            Err(error) => match surveillance_refusal_reason(error) {
                Some(reason) => reasons |= reason,
                None => return Err(format!("surveillance stage invariant: {}", error.message())),
            },
        }
        let mut flight_capacity_refusal =
            flight::flight_table_capacity_refusal(&runtime.flights, event);
        let mut pending_flight_key = None;
        if reasons == 0 {
            if flight_capacity_refusal {
                // A full core table can contain terminal rows that are still eligible for
                // receipt-backed retirement. Reclaim only after all stages validate, then
                // recheck the real core capacity before classifying this item as refused.
                reclaim_retired_flight_keys(runtime, event.receipt_ns / 1_000_000)?;
                flight_capacity_refusal =
                    flight::flight_table_capacity_refusal(&runtime.flights, event);
            }
            if !flight_capacity_refusal {
                pending_flight_key = preflight_flight_key(runtime, event)?;
            }
        }
        if flight_capacity_refusal {
            reasons |= REASON_RESOURCE_LIMIT;
        }

        if reasons != 0 {
            let fence =
                route::route_rejection_fence(&runtime.routes, event, &route_stage, reasons)?;
            if matches!(event.kind, crate::input::EventKind::FiledRoute { .. }) && fence.is_none() {
                return Err("refused filed route lacks an availability fence".into());
            }
            let record = quarantine_runtime_item(
                runtime,
                event,
                item_index,
                batch_ordinal,
                reasons,
                processing_started,
                item_started,
            )?;
            quarantine_records.push(record);
            if let Some(fence) = fence {
                route::route_commit_refusal(&mut runtime.routes, fence)?;
            }
            match order_result {
                Err(error) => flight::flight_record_item_refusal(&mut runtime.flights, error),
                Ok(()) => match adapter_refusal {
                    Some(reason) => {
                        flight::flight_record_adapter_refusal(&mut runtime.flights, reason)
                    }
                    None if flight_capacity_refusal => {
                        flight::flight_record_capacity_refusal(&mut runtime.flights)
                    }
                    None => flight::flight_record_external_refusal(&mut runtime.flights),
                },
            }
            continue;
        }

        accept_runtime_item(
            runtime,
            event,
            item_index,
            batch_ordinal,
            processing_started,
            item_started,
        )?;
        let flight_outcome = flight::flight_apply_event(&mut runtime.flights, event)
            .map_err(|error| format!("accepted flight commit invariant: {error:?}"))?;
        commit_flight_key(runtime, pending_flight_key, event, flight_outcome)?;
        flight_operations.push(flight_outcome);
        route::route_commit_stage(&mut runtime.routes, route_stage);
        surveillance::surveillance_event(&mut runtime.surveillance, event)
            .map_err(|error| format!("accepted surveillance commit invariant: {error}"))?;
        flight::flight_commit_order(&mut order_cursor, event);
    }
    // Flight ownership presentation currently requires an explicitly selected flight. Per-flight
    // snapshots below preserve every owner; no arbitrary 'first' flight is promoted to selection.
    let configured_track_period = runtime
        .final_monitor
        .as_ref()
        .map_or(surveillance::SECOND_NS as u64, |monitor| {
            monitor.pair.update_ns
        });
    let track_health_period_ns = i64::try_from(configured_track_period)
        .map_err(|_| "configured track health period exceeds the supported time range")?;
    let current_feeds = surveillance::feed_statuses_with_track_period(
        &runtime.surveillance,
        batch.receipt_ns,
        track_health_period_ns,
    )?;
    let automation = automation_status(runtime.selected_mode, &current_feeds);
    runtime.last_automation = automation;
    let unavailable_reason = match automation.degradation {
        AutomationDegradation::OperatorSelectedFlightDataOnly => {
            Some(surveillance::ProductUnavailableReason::OperatorSelectedFlightDataOnly)
        }
        AutomationDegradation::SurveillanceUnavailable => {
            Some(surveillance::ProductUnavailableReason::SurveillanceFeedUnavailable)
        }
        AutomationDegradation::FusionUnavailable => {
            Some(surveillance::ProductUnavailableReason::FusionUnavailable)
        }
        AutomationDegradation::None => None,
    };
    let flight_cutoff_ms = route::receipt_cutoff_ms(batch.receipt_ns)?;
    let (mut surveillance, current_flight_views) = {
        let mut views = Vec::with_capacity(runtime.profile.max_flights);
        let mut current_flight_views = Vec::with_capacity(runtime.profile.surveillance.max_tracks);
        let runtime_centre = runtime.centre;
        let flight_driver = &mut runtime.flights;
        let known_flights = &runtime.known_flights;
        let flight_liveness_now_ns = batch.receipt_ns;
        let track_drop_after_ns = surveillance::tracker_drop_after_ns(&runtime.surveillance);
        let mut select_traffic_detail =
            |observed: &surveillance::ObservedTrack,
             observation: Option<surveillance::TrackCallsignEvidence>|
             -> Result<oa_display::core::traffic::TrafficBlockPolicy, String> {
                let result = flight::flight_views_for_address(
                    flight_driver,
                    observed.address,
                    runtime_centre,
                    flight_cutoff_ms,
                    &mut views,
                );
                let candidate = (result.status == FlightViewStatus::Found)
                    .then(|| views.first().copied())
                    .flatten();
                let last_flight_receipt = candidate
                    .map(|view| known_flight_receipt(known_flights, &view))
                    .transpose()?
                    .flatten();
                let identity_corroborated = candidate.is_some_and(|view| {
                    result.claimants == 1
                        && result.views_written == 1
                        && flight_identity_corroborates(
                            observed,
                            observation,
                            &view,
                            runtime_centre,
                            FlightFreshness {
                                last_receipt_ns: last_flight_receipt,
                                now_ns: flight_liveness_now_ns,
                                drop_after_ns: track_drop_after_ns,
                                cutoff_ms: flight_cutoff_ms,
                            },
                        )
                });
                let view = candidate.filter(|_| identity_corroborated);
                let detail = traffic_detail_for_current_view(
                    observed,
                    result,
                    candidate,
                    runtime_centre,
                    identity_corroborated,
                );
                current_flight_views.push(CurrentFlightView {
                    address: observed.address,
                    result,
                    view,
                    identity_corroborated,
                });
                Ok(oa_display::core::traffic::TrafficBlockPolicy {
                    address: observed.address,
                    detail,
                    flight_callsign: (detail
                        == oa_display::core::traffic::TrafficBlockDetail::Full)
                        .then(|| view.map(|row| row.callsign))
                        .flatten(),
                })
            };
        let surveillance = surveillance::surveillance_finish_with_traffic_policy_and_track_period(
            &mut runtime.surveillance,
            None,
            automation.effective == EffectiveAutomationMode::Surveillance,
            unavailable_reason,
            &mut select_traffic_detail,
            track_health_period_ns,
        )?;
        (surveillance, current_flight_views)
    };
    runtime.frame_sequence = next_frame_sequence;
    if surveillance.tracks.len() != current_flight_views.len() {
        return Err("exported tracks and cached flight views are not aligned".into());
    }
    let mut flights = Vec::with_capacity(runtime.profile.surveillance.max_tracks);
    let mut flight_correlation = FlightCorrelationSummary::default();
    for (index, observed) in surveillance.tracks.iter().enumerate() {
        let current = current_flight_views
            .get(index)
            .filter(|row| row.address == observed.address)
            .ok_or("traffic and flight-view rows are not aligned")?;
        let result = current.result;
        let mut row = FlightEvidence {
            address: observed.address,
            claimants: result.claimants,
            view: None,
            trajectory: None,
            vertical_profile: None,
            vertical_refusal: if performance.is_some() {
                "performance evidence missing for full flight key".into()
            } else {
                "vertical profile not requested".into()
            },
            refusal: String::new(),
        };
        match result.status {
            FlightViewStatus::Found if current.identity_corroborated => {
                flight_correlation.matched_observed_tracks =
                    flight_correlation.matched_observed_tracks.saturating_add(1);
            }
            FlightViewStatus::Found => {
                flight_correlation.unmatched_observed_tracks = flight_correlation
                    .unmatched_observed_tracks
                    .saturating_add(1);
            }
            FlightViewStatus::AmbiguousAddress => {
                flight_correlation.ambiguous_observed_tracks = flight_correlation
                    .ambiguous_observed_tracks
                    .saturating_add(1);
            }
            FlightViewStatus::UnknownFlight | FlightViewStatus::UnknownCentre => {
                flight_correlation.unmatched_observed_tracks = flight_correlation
                    .unmatched_observed_tracks
                    .saturating_add(1);
            }
            FlightViewStatus::FutureSnapshot | FlightViewStatus::OutputCapacity => {
                flight_correlation.refused_observed_track_lookups = flight_correlation
                    .refused_observed_track_lookups
                    .saturating_add(1);
            }
        }
        if result.status != FlightViewStatus::Found {
            row.refusal = format!("flight view: {:?}", result.status);
        } else if !current.identity_corroborated {
            row.refusal = "address candidate lacks co-timed callsign corroboration".into();
        } else if let Some(view) = current.view.as_ref() {
            row.view = Some(*view);
            if view.plan_state != oa_flight::ST_ACTIVE && view.plan_state != oa_flight::ST_DEPARTED
            {
                row.refusal = "flight plan is not active/departed".into();
            } else {
                let selected_performance = match (performance, performance_cutoff_ms) {
                    (Some(rows), Some(cutoff_ms)) => {
                        match performance_for_key(rows, route::flight_key(view), cutoff_ms) {
                            Ok(value) => (value, None),
                            Err(reason) => (None, Some(reason)),
                        }
                    }
                    _ => (None, None),
                };
                if let Some(reason) = selected_performance.1 {
                    row.vertical_refusal = reason.into();
                    match route::route_issue(
                        &runtime.routes,
                        view,
                        observed,
                        batch.receipt_ns,
                        runtime.frame_sequence,
                    ) {
                        Ok(trajectory) => row.trajectory = Some(trajectory),
                        Err(reason) => row.refusal = reason,
                    }
                } else if let Some(envelope) = selected_performance.0 {
                    match route::route_issue_with_performance(
                        &runtime.routes,
                        view,
                        observed,
                        batch.receipt_ns,
                        runtime.frame_sequence,
                        envelope,
                    ) {
                        Ok((trajectory, vertical_profile)) => {
                            row.trajectory = Some(trajectory);
                            row.vertical_refusal = if vertical_profile.refusal
                                == oa_legacy_traj::core::trajectory::Refusal::None
                            {
                                String::new()
                            } else {
                                format!("{:?}", vertical_profile.refusal)
                            };
                            row.vertical_profile = Some(vertical_profile);
                        }
                        Err(reason) => {
                            row.vertical_refusal = format!("route input unavailable: {reason}");
                            row.refusal = reason;
                        }
                    }
                } else {
                    match route::route_issue(
                        &runtime.routes,
                        view,
                        observed,
                        batch.receipt_ns,
                        runtime.frame_sequence,
                    ) {
                        Ok(trajectory) => row.trajectory = Some(trajectory),
                        Err(reason) => {
                            row.vertical_refusal = format!("route input unavailable: {reason}");
                            row.refusal = reason;
                        }
                    }
                }
            }
        } else {
            return Err("flight adapter reported a found view without a row".into());
        }
        flights.push(row);
    }
    runtime.views = commands::current_views(
        &runtime.views,
        &flights,
        &mut runtime.next_generation,
        runtime.profile.max_flights,
    )?;
    runtime.observed.clone_from(&surveillance.tracks);
    if runtime.known_flights.len() > MAX_FLIGHT_DATA_VIEW_WORK {
        return Err("flight-data view work budget exceeded before frame assembly".into());
    }
    let mut flight_data = Vec::with_capacity(runtime.known_flights.len());
    let mut flight_data_as_of = Vec::with_capacity(runtime.known_flights.len());
    let mut flight_data_unavailable = 0u32;
    for known in &runtime.known_flights {
        match flight::flight_view_for_key(
            &mut runtime.flights,
            known.key,
            runtime.centre,
            flight_cutoff_ms,
        ) {
            Ok(view) => {
                let as_of = flight::flight_view_as_of(&view, flight_cutoff_ms)
                    .map_err(|_| "flight-data as-of interpretation rejected the frame cutoff")?;
                flight_data.push(view);
                flight_data_as_of.push(as_of);
            }
            Err(
                flight::FlightViewStatus::UnknownFlight | flight::FlightViewStatus::UnknownCentre,
            ) => {
                flight_data_unavailable = flight_data_unavailable.saturating_add(1);
            }
            Err(flight::FlightViewStatus::FutureSnapshot) => {
                return Err("future flight snapshot refused in regional flight-data view".into())
            }
            Err(
                flight::FlightViewStatus::Found
                | flight::FlightViewStatus::AmbiguousAddress
                | flight::FlightViewStatus::OutputCapacity,
            ) => return Err("unexpected full-key flight view status".into()),
        }
    }
    flight_correlation.available_flight_views_for_runtime_centre =
        u32::try_from(flight_data.len()).unwrap_or(u32::MAX);
    flight_correlation.positive_address_claims_for_runtime_centre =
        u32::try_from(flight_data.iter().filter(|view| view.address > 0).count())
            .unwrap_or(u32::MAX);
    let monitor_flight_data: Vec<_> = flights
        .iter()
        .filter_map(|flight| flight.view)
        .filter(|view| {
            matches!(
                view.plan_state,
                oa_flight::ST_ACTIVE | oa_flight::ST_DEPARTED
            )
        })
        .collect();
    let final_monitor = configured_final_monitor_frame(
        runtime.final_monitor.as_mut(),
        &surveillance,
        &monitor_flight_data,
        flight_data_unavailable,
    );
    runtime.final_monitor_display = final_monitor.clone();
    if runtime.profile.surveillance.render_scope {
        let console = automation_console_text(&runtime.console_text, automation);
        surveillance.frame_sha256 = present_final_monitor_controls(
            &mut runtime.surveillance,
            &runtime.interaction,
            &console,
            &final_monitor,
        )?;
    }
    Ok(RegionalFrame {
        sequence: runtime.frame_sequence,
        input,
        surveillance,
        final_monitor,
        automation,
        flight_operations,
        flights,
        flight_data,
        flight_data_as_of,
        flight_data_unavailable,
        flight_correlation,
        flight_report: flight::flight_driver_report(&runtime.flights),
        refused_legacy_route_events: runtime.routes.rejected_legacy_routes,
        controller_views: runtime.views.clone(),
        controller_dispatches,
        controller_outcome: oa_display::interaction::command_outcome(&runtime.interaction),
        controller_selection_kind: oa_display::interaction::selection_kind(&runtime.interaction),
        controller_selected_address: oa_display::interaction::selected_address(
            &runtime.interaction,
        ),
        controller_selection: oa_display::interaction::selected_identity(&runtime.interaction),
        console_text: runtime.console_text.clone(),
        quarantine: RuntimeQuarantineReport {
            records: quarantine_records,
            source_stats: quarantine_stats(&runtime.quarantine_queue)?,
        },
    })
}

/// A failed partial application poisons this offline session; callers must reconstruct a fresh
/// runtime from the admitted prefix. They cannot continue from a partly changed frame.
pub fn regional_step(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
) -> Result<RegionalFrame, String> {
    regional_step_inner(runtime, batch, None)
}

/// Opt in to vertical reachability for this receipt using only the caller-supplied, full-key
/// performance envelopes. No performance defaults are created. Future rows are gated before any
/// identity or capability fields are inspected.
pub fn regional_step_with_performance(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
    performance: &[PerformanceEnvelope],
) -> Result<RegionalFrame, String> {
    if runtime.failed {
        return Err("session failed; reconstruct from the admitted input prefix".into());
    }
    if performance.len() > MAX_PERFORMANCE_ENVELOPES {
        return Err(format!(
            "performance envelope slice exceeds {} rows",
            MAX_PERFORMANCE_ENVELOPES
        ));
    }
    regional_step_inner(runtime, batch, Some(performance))
}

fn regional_step_inner(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
    performance: Option<&[PerformanceEnvelope]>,
) -> Result<RegionalFrame, String> {
    if runtime.failed {
        return Err("session failed; reconstruct from the admitted input prefix".into());
    }
    match execute(runtime, batch, performance) {
        Ok(frame) => {
            // `RegionalFrame` owns copied route/view values; no renderer or caller retains a
            // borrow into the route store. Advancing only after the complete frame exists means
            // failed work never reclaims history or frees current-plan storage.
            route::advance_route_history_watermark(
                &mut runtime.routes,
                batch.receipt_ns / 1_000_000,
            );
            Ok(frame)
        }
        Err(error) => {
            runtime.failed = true;
            Err(error)
        }
    }
}

pub fn regional_pixels(runtime: &RegionalRuntime) -> Result<&[u8], String> {
    if runtime.failed {
        return Err("no publishable frame after session failure".into());
    }
    surveillance::surveillance_pixels(&runtime.surveillance)
}

pub fn regional_frame_spec(
    runtime: &RegionalRuntime,
) -> Result<oa_display::raster::FrameSpec, String> {
    if runtime.failed {
        return Err("no publishable frame geometry after session failure".into());
    }
    surveillance::surveillance_frame_spec(&runtime.surveillance)
}

/// Explicit current-view selection. Ordinary refusal clears selection and is rendered; it
/// cannot manufacture a full flight binding, advance source time or transfer authority.
pub fn regional_select(
    runtime: &mut RegionalRuntime,
    address: u32,
    generation: u32,
    identity: Option<oa_display::interaction::FlightIdentity>,
) -> Result<[u8; 32], String> {
    if runtime.failed || runtime.frame_sequence == 0 {
        return Err("selection requires a completed regional frame".into());
    }
    let sequence = u32::try_from(runtime.frame_sequence).map_err(|_| "view sequence exhausted")?;
    let outcome = oa_display::interaction::select_target(
        &mut runtime.interaction,
        &runtime.views,
        sequence,
        address,
        generation,
        identity,
    );
    if !matches!(outcome, oa_display::interaction::InteractionResult::Applied) {
        oa_display::interaction::clear_selection(&mut runtime.interaction);
    }
    let final_monitor_display = runtime.final_monitor_display.clone();
    let digest = match present_final_monitor_controls(
        &mut runtime.surveillance,
        &runtime.interaction,
        &automation_console_text(&runtime.console_text, runtime.last_automation),
        &final_monitor_display,
    ) {
        Ok(digest) => digest,
        Err(reason) => {
            runtime.failed = true;
            return Err(reason);
        }
    };
    match outcome {
        oa_display::interaction::InteractionResult::Applied => Ok(digest),
        other => Err(format!("selection: {other:?}")),
    }
}

/// Bounded input/status text for operated replay. Rendering replaces prior control primitives,
/// keeps the observed frame unchanged, and records this text in the next frame's evidence.
pub fn regional_console_overlay(
    runtime: &mut RegionalRuntime,
    text: &str,
) -> Result<[u8; 32], String> {
    if runtime.failed || runtime.frame_sequence == 0 {
        return Err("console presentation requires a completed regional frame".into());
    }
    if text.len() > MAX_OPERATOR_CONSOLE_BYTES
        || !text.bytes().all(|byte| (32..=126).contains(&byte))
    {
        return Err("console text must be at most 128 printable ASCII bytes".into());
    }
    runtime.console_text.clear();
    runtime.console_text.push_str(text);
    let final_monitor_display = runtime.final_monitor_display.clone();
    match present_final_monitor_controls(
        &mut runtime.surveillance,
        &runtime.interaction,
        &automation_console_text(&runtime.console_text, runtime.last_automation),
        &final_monitor_display,
    ) {
        Ok(digest) => Ok(digest),
        Err(reason) => {
            runtime.failed = true;
            Err(reason)
        }
    }
}

/// Clear only operator selection; captured authority requests retain their own full identity.
pub fn regional_clear_selection(runtime: &mut RegionalRuntime) -> Result<[u8; 32], String> {
    if runtime.failed || runtime.frame_sequence == 0 {
        return Err("selection clearing requires a completed regional frame".into());
    }
    oa_display::interaction::clear_selection(&mut runtime.interaction);
    let final_monitor_display = runtime.final_monitor_display.clone();
    match present_final_monitor_controls(
        &mut runtime.surveillance,
        &runtime.interaction,
        &automation_console_text(&runtime.console_text, runtime.last_automation),
        &final_monitor_display,
    ) {
        Ok(digest) => Ok(digest),
        Err(reason) => {
            runtime.failed = true;
            Err(reason)
        }
    }
}

pub fn regional_enable_msaw(
    runtime: &mut RegionalRuntime,
    data: oa_msaw_run::regional::RegionalData,
) -> Result<(), String> {
    let service = crate::msaw_bridge::regional_msaw_new_for_profile(
        data,
        runtime.profile.surveillance.max_tracks,
        runtime.profile.surveillance.max_frames,
    )?;
    regional_attach_msaw(runtime, service)
}

pub fn regional_enable_synthetic_msaw(runtime: &mut RegionalRuntime) -> Result<(), String> {
    let service = crate::msaw_bridge::regional_msaw_synthetic_for_profile(
        runtime.profile.surveillance.max_tracks,
        runtime.profile.surveillance.max_frames,
    )?;
    regional_attach_msaw(runtime, service)
}

fn available_msaw_marker_rows(runtime: &RegionalRuntime) -> Result<usize, String> {
    let profile = runtime.profile.surveillance;
    let final_rows = runtime
        .final_monitor
        .as_ref()
        .map_or(0, |monitor| monitor.assignments.len());
    let remaining = profile
        .max_tracks
        .checked_sub(final_rows)
        .ok_or("final-monitor rows exceed the selected profile track capacity")?;
    let max_rows = if profile.render_scope {
        remaining.min(MAX_FINAL_APPROACH_ASSIGNMENTS)
    } else {
        remaining
    };
    if max_rows == 0 {
        return Err("MSAW has no remaining shared alert rows in this rendered profile".into());
    }
    validate_alert_composition(profile, final_rows, max_rows)?;
    Ok(max_rows)
}

pub fn regional_attach_msaw(
    runtime: &mut RegionalRuntime,
    service: crate::msaw_bridge::RegionalMsaw,
) -> Result<(), String> {
    let available_rows = available_msaw_marker_rows(runtime)?;
    let service_rows = crate::msaw_bridge::regional_msaw_row_limit(&service);
    if service_rows > runtime.profile.surveillance.max_tracks {
        return Err(format!(
            "MSAW service row capacity {service_rows} exceeds selected profile track capacity {}",
            runtime.profile.surveillance.max_tracks
        ));
    }
    let alert_limit = crate::msaw_bridge::regional_msaw_alert_limit(&service);
    if alert_limit > available_rows {
        return Err(format!(
            "MSAW open-alert capacity {alert_limit} exceeds composed profile allowance {available_rows}"
        ));
    }
    surveillance::surveillance_enable_msaw(&mut runtime.surveillance, service)
}

#[cfg(test)]
mod transaction_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::cli::synthetic_model;
    use crate::input::{
        parse_event_line, EventKind, FlightOperation, Identity, NormalizedEvent, SourceKind,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_QUARANTINE_AUDIT_TEST: AtomicU64 = AtomicU64::new(0);

    // REQ: OA-QUAR-004, OA-REG-013
    #[test]
    fn recorded_hour_quarantine_ids_cover_the_dense_batch_without_aliasing() {
        let limit = RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT;
        let first = runtime_item_id(1, 0, limit).unwrap();
        let last = runtime_item_id(1, limit - 1, limit).unwrap();
        let next = runtime_item_id(2, 0, limit).unwrap();
        assert_eq!(first, 8192);
        assert_eq!(last, 16383);
        assert_eq!(next, 16384);
        assert!(first < last && last < next);
        assert_eq!(
            runtime_item_id(1, 255, MAX_EVENTS_PER_RECEIPT).unwrap(),
            511
        );
        assert!(runtime_item_id(1, 256, MAX_EVENTS_PER_RECEIPT).is_err());
        assert!(runtime_item_id(1, limit, limit).is_err());
        assert!(runtime_item_id(u64::MAX, 0, limit).is_err());
        assert!(runtime_item_id(0, 0, limit).is_err());
    }

    fn intern(model: &mut Model, text: &str) -> oa_airspace::core::model::StrIx {
        let raw = model.strings.offsets.len().checked_sub(1).unwrap();
        let index = oa_airspace::core::model::StrIx::new(u32::try_from(raw).unwrap());
        model.strings.text.push_str(text);
        model
            .strings
            .offsets
            .push(u32::try_from(model.strings.text.len()).unwrap());
        index
    }

    // REQ: OA-QUAR-004, OA-AUDIT-002, OA-AUDIT-003, OA-AUDIT-005, OA-AUDIT-006
    #[test]
    fn quarantine_maintainer_appends_before_ack_and_refuses_audit_failure() {
        let model = synthetic_model().expect("public synthetic model");
        let snapshot = oa_airspace::core::snapshot::encode(&model).expect("encode model");
        let checkpoint = oa_audit_core::genesis();
        let registry = oa_audit_core::parse_principal_registry(
            b"OACERT-LOCAL-PRINCIPALS-V1\n51\toperate\n52\tmonitor\n",
        )
        .expect("synthetic principal registry");
        let mut runtime = regional_new_with_profile_and_quarantine_audit_checkpoint(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            DEFAULT_REGIONAL_RUN_PROFILE,
            checkpoint,
        )
        .expect("new synthetic runtime");
        let source = InputSource::Surveillance;
        let identity = InputIdentity {
            source,
            source_instance: 2,
            item_id: 77,
        };
        assert!(matches!(
            route_input(
                &mut runtime.quarantine_queue,
                InputOutcome {
                    identity,
                    refusal_reasons: ReasonFlags(REASON_SURVEILLANCE_INVALID),
                    clock_domain_id: 1,
                    started_at_ns: 10,
                    completed_at_ns: 20,
                    elapsed_limit_ns: 10,
                    work_units: 1,
                    work_limit: 1,
                },
            ),
            Ok(Disposition::Queued(_))
        ));
        let before = queue_front(&runtime.quarantine_queue, source)
            .expect("read synthetic queue")
            .expect("queued event");
        let serial = NEXT_QUARANTINE_AUDIT_TEST.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "oa-regional-quarantine-maintenance-{}-{serial}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).expect("create synthetic output directory");
        let protected_alias = root
            .join("scds")
            .join("rec-20261119")
            .join("day2-synthetic")
            .join("audit.log");
        assert!(regional_acknowledge_quarantine_before_commit(
            &mut runtime,
            QuarantineMaintenanceRequest {
                source,
                expected_identity: identity,
                audit_log: &protected_alias,
                expected_checkpoint: checkpoint,
                principal_registry: &registry,
                principal: 51,
                time_ns: 100,
                manual_handoff_ref: "manual-case-77",
            },
        )
        .is_err());
        assert_eq!(
            queue_front(&runtime.quarantine_queue, source).expect("queue still available"),
            Some(before)
        );
        assert!(!protected_alias.exists());

        let wrong_role_path = root.join("wrong-role.audit");
        assert!(regional_acknowledge_quarantine_before_commit(
            &mut runtime,
            QuarantineMaintenanceRequest {
                source,
                expected_identity: identity,
                audit_log: &wrong_role_path,
                expected_checkpoint: checkpoint,
                principal_registry: &registry,
                principal: 52,
                time_ns: 100,
                manual_handoff_ref: "manual-case-77",
            },
        )
        .is_err());
        assert!(!wrong_role_path.exists());
        assert_eq!(
            queue_front(&runtime.quarantine_queue, source).expect("queue still available"),
            Some(before)
        );

        let audit_path = root.join("quarantine.audit");
        let receipt = regional_acknowledge_quarantine_before_commit(
            &mut runtime,
            QuarantineMaintenanceRequest {
                source,
                expected_identity: identity,
                audit_log: &audit_path,
                expected_checkpoint: checkpoint,
                principal_registry: &registry,
                principal: 51,
                time_ns: 101,
                manual_handoff_ref: "manual-case-77",
            },
        )
        .expect("append and acknowledge selected queue front");
        assert_eq!(receipt.event, before);
        assert_eq!(receipt.audit_checkpoint.count, 1);
        assert_eq!(
            oa_audit_log::shell::verify(&audit_path, receipt.audit_checkpoint)
                .expect("verify durable release record"),
            receipt.audit_checkpoint
        );
        assert_eq!(
            queue_front(&runtime.quarantine_queue, source).expect("queue empty after release"),
            None
        );
        assert_eq!(
            source_stats(&runtime.quarantine_queue, source)
                .expect("release counter")
                .acknowledged,
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn insert_bit(bits: &mut oa_airspace::core::model::BitSet, index: usize, value: bool) {
        let old_len = usize::try_from(bits.len).unwrap();
        let old_values: Vec<_> = (0..old_len)
            .map(|row| {
                bits.words
                    .get(row / 64)
                    .is_some_and(|word| (word >> (row % 64)) & 1 != 0)
            })
            .collect();
        let mut rebuilt = oa_airspace::core::model::BitSet::default();
        for row in 0..=old_len {
            oa_airspace::core::model::bit_push(
                &mut rebuilt,
                if row == index {
                    value
                } else {
                    *old_values
                        .get(if row < index { row } else { row - 1 })
                        .unwrap()
                },
            );
        }
        *bits = rebuilt;
    }

    fn repeated_name_model() -> Model {
        let mut model = synthetic_model().unwrap();
        let ident = intern(&mut model, "FIXA");
        let region = intern(&mut model, "K2");
        let points = &mut model.points;
        points
            .kind
            .insert(1, oa_airspace::core::model::point_kind::FIX);
        points.lat.insert(1, 420_000_000);
        points.lon.insert(1, -860_000_000);
        points
            .elev_ft
            .insert(1, oa_airspace::core::model::ELEV_NONE);
        points.ident.insert(1, ident);
        points
            .alt_ident
            .insert(1, oa_airspace::core::model::StrIx::NONE);
        points.region.insert(1, region);
        points
            .scope
            .insert(1, oa_airspace::core::model::StrIx::NONE);
        points.name.insert(1, oa_airspace::core::model::StrIx::NONE);
        for (column, value) in [
            (&mut points.src_nasr, false),
            (&mut points.src_cifp, false),
            (&mut points.in_area, true),
            (&mut points.cls_b, false),
            (&mut points.cls_c, false),
            (&mut points.cls_d, false),
            (&mut points.cls_e, false),
        ] {
            insert_bit(column, 1, value);
        }
        model
    }

    fn runtime() -> RegionalRuntime {
        runtime_with_model(synthetic_model().unwrap())
    }

    fn runtime_with_model(model: Model) -> RegionalRuntime {
        runtime_with_model_and_centre(model, *b"ZAU1")
    }

    fn runtime_with_centre(centre: [u8; 4]) -> RegionalRuntime {
        runtime_with_model_and_centre(synthetic_model().unwrap(), centre)
    }

    fn runtime_with_profile_and_centre(
        profile: RegionalRunProfile,
        centre: [u8; 4],
    ) -> RegionalRuntime {
        let model = synthetic_model().unwrap();
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        regional_new_with_profile(
            model,
            oa_sha256::sha256(&snapshot),
            centre,
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            profile,
        )
        .unwrap()
    }

    fn runtime_with_model_and_centre(model: Model, centre: [u8; 4]) -> RegionalRuntime {
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        regional_new(
            model,
            oa_sha256::sha256(&snapshot),
            centre,
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
        )
        .unwrap()
    }

    fn runtime_with_flight_capacity(max_flights: usize) -> RegionalRuntime {
        let model = synthetic_model().unwrap();
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        let mut profile = DEFAULT_REGIONAL_RUN_PROFILE;
        profile.max_flights = max_flights;
        regional_new_with_profile(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            profile,
        )
        .unwrap()
    }

    fn step_event_line(
        engine: &mut RegionalRuntime,
        line: &str,
    ) -> (NormalizedEvent, RegionalFrame) {
        let event = parse_event_line(line).unwrap();
        let frame = regional_step(
            engine,
            &ReceiptBatch {
                receipt_ns: event.receipt_ns,
                source_clock_regressions: 0,
                events: vec![event],
            },
        )
        .unwrap();
        (event, frame)
    }

    fn seeded_claim_frame(
        engine: &mut RegionalRuntime,
        claims: &[(u128, u32, [u8; 4])],
        receipt_ns: i64,
    ) -> RegionalFrame {
        let mut events = Vec::with_capacity(claims.len().saturating_mul(2));
        for (index, (gufi, address, centre)) in claims.iter().enumerate() {
            let sequence = (index as u64).saturating_mul(2).saturating_add(1);
            for (offset, operation) in [
                FlightOperation::SetPlan {
                    plan_id: *b"PLAN00000001",
                    status: oa_flight::ST_ACTIVE,
                },
                FlightOperation::SetAddress,
            ]
            .into_iter()
            .enumerate()
            {
                events.push(NormalizedEvent {
                    receipt_ns,
                    input_receipt_ns: receipt_ns,
                    source_ns: receipt_ns,
                    sequence: sequence.saturating_add(offset as u64),
                    source_message_id: None,
                    source: SourceKind::Flight,
                    identity: Identity {
                        flight_gufi: *gufi,
                        aircraft_address: *address,
                    },
                    centre: *centre,
                    kind: EventKind::Flight(operation),
                });
            }
        }
        regional_step(
            engine,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events,
            },
        )
        .unwrap()
    }

    fn observed_address_frame(
        engine: &mut RegionalRuntime,
        address: u32,
        centre: [u8; 4],
        receipt_ns: i64,
    ) -> RegionalFrame {
        let events = [receipt_ns - surveillance::SECOND_NS, receipt_ns]
            .into_iter()
            .enumerate()
            .map(|(index, source_ns)| NormalizedEvent {
                receipt_ns,
                input_receipt_ns: receipt_ns,
                source_ns,
                sequence: u64::try_from(index + 1).unwrap(),
                source_message_id: None,
                source: SourceKind::Surveillance,
                identity: Identity {
                    flight_gufi: 0,
                    aircraft_address: address,
                },
                centre,
                kind: EventKind::Position {
                    latitude_e7: 410_000_000,
                    longitude_e7: -870_000_000,
                    altitude_ft: Some(10_000),
                    east_mmps: 0,
                    north_mmps: 0,
                    vertical_rate_fpm: None,
                    track_callsign: None,
                },
            })
            .collect();
        regional_step(
            engine,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events,
            },
        )
        .unwrap()
    }

    fn co_timed_identity_frame(
        engine: &mut RegionalRuntime,
        address: u32,
        receipt_ns: i64,
        track_callsign: Option<&str>,
        flight_identity: Option<(&str, i64)>,
    ) -> RegionalFrame {
        co_timed_frame_with_plan_refusal(
            engine,
            address,
            receipt_ns,
            track_callsign,
            flight_identity,
            None,
            false,
        )
    }

    fn co_timed_frame_with_plan_refusal(
        engine: &mut RegionalRuntime,
        address: u32,
        receipt_ns: i64,
        track_callsign: Option<&str>,
        flight_identity: Option<(&str, i64)>,
        refused_plan_source_ns: Option<i64>,
        complete_flight: bool,
    ) -> RegionalFrame {
        assert_eq!(
            engine.known_flights.len(),
            1,
            "identity fixture must seed exactly one full flight key"
        );
        let known_flight = engine
            .known_flights
            .first()
            .expect("identity fixture checked for one full flight key");
        let gufi = known_flight.key.gufi;
        assert_ne!(gufi, 0, "identity fixture requires a GUFI-backed full key");
        let mut events = Vec::with_capacity(5);
        for (sequence, source_ns) in [receipt_ns - surveillance::SECOND_NS, receipt_ns]
            .into_iter()
            .enumerate()
        {
            let line = if let Some(callsign) = track_callsign {
                format!(
                    "v1\t{receipt_ns}\t{source_ns}\t{}\tSURVEILLANCE\t00000000000000000000000000000000\t{address:06x}\tZAU\tPOS\t410000000\t-870000000\t10000\t0\t0\t0\tCALLSIGN\t{callsign}",
                    sequence + 1
                )
            } else {
                format!(
                    "v1\t{receipt_ns}\t{source_ns}\t{}\tSURVEILLANCE\t00000000000000000000000000000000\t{address:06x}\tZAU\tPOS\t410000000\t-870000000\t10000\t0\t0\t0",
                    sequence + 1
                )
            };
            events.push(parse_event_line(&line).expect("parse source callsign position"));
        }
        if let Some((flight_callsign, identity_source_ns)) = flight_identity {
            let identity = format!(
                "v1\t{receipt_ns}\t{identity_source_ns}\t3\tFLIGHT\t{gufi:032x}\t{address:06x}\tZAU\tFLIGHT\tIDENTITY_PARTIAL\t{flight_callsign}\t-\t-"
            );
            events.push(parse_event_line(&identity).expect("parse flight identity update"));
        }
        if let Some(source_ns) = refused_plan_source_ns {
            let refused_plan = format!(
                "v1\t{receipt_ns}\t{source_ns}\t4\tFLIGHT\t{gufi:032x}\t{address:06x}\tZAU\tFLIGHT\tPLAN\tPLAN00000001\t1"
            );
            events.push(parse_event_line(&refused_plan).expect("parse refused plan transition"));
        }
        if complete_flight {
            let complete = format!(
                "v1\t{receipt_ns}\t{receipt_ns}\t5\tFLIGHT\t{gufi:032x}\t{address:06x}\tZAU\tFLIGHT\tCOMPLETE"
            );
            events.push(parse_event_line(&complete).expect("parse flight completion"));
        }
        let route = format!(
            "v1\t{receipt_ns}\t{receipt_ns}\t1\tROUTE\t{gufi:032x}\t{address:06x}\tZAU\tFILED_ROUTE\tPLAN00000001\tFIXB\tFIX:FIXA:K1:1:- FIX:FIXB:K1:1:-"
        );
        events.push(parse_event_line(&route).expect("parse synthetic route for identity gate"));
        regional_step(
            engine,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events,
            },
        )
        .expect("run co-timed identity frame through presenter")
    }

    #[test]
    fn flight_view_key_capacity_refuses_before_a_second_plan_commit() {
        let mut engine = runtime_with_flight_capacity(1);
        let first = parse_event_line(
            "v1\t1000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2",
        )
        .unwrap();
        regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 1_000_000_000,
                source_clock_regressions: 0,
                events: vec![first],
            },
        )
        .unwrap();
        let second = parse_event_line(
            "v1\t2000000000\t2000000000\t2\tFLIGHT\t00000000000000000000000000000002\tABC102\tZAU1\tFLIGHT\tPLAN\tPLAN00000002\t2",
        )
        .unwrap();
        let frame = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 2_000_000_000,
                source_clock_regressions: 0,
                events: vec![second],
            },
        )
        .unwrap();
        assert_eq!(flight::flight_driver_report(&engine.flights).flights, 1);
        assert_eq!(engine.known_flights.len(), 1);
        assert_eq!(frame.quarantine.records.len(), 1);
        assert_eq!(
            frame
                .quarantine
                .records
                .first()
                .expect("one quarantine record was asserted above")
                .item_index,
            0
        );
        assert_eq!(
            flight::flight_driver_report(&engine.flights).operations,
            1,
            "capacity refusal must not apply the second plan"
        );
        assert!(engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == first.identity.flight_gufi));
        assert!(!engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == second.identity.flight_gufi));
        assert_eq!(
            flight::flight_driver_report(&engine.flights).retirements_consumed,
            0
        );
    }

    // Kills the preflight mutation that retained terminal core rows in the key ledger until
    // capacity refusal, despite the core's explicit retirement receipt path.
    #[test]
    fn terminal_full_key_is_retired_with_receipt_before_reusing_view_capacity() {
        let mut engine = runtime_with_flight_capacity(1);
        let (first, _) = step_event_line(
            &mut engine,
            "v1\t1000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2",
        );
        step_event_line(
            &mut engine,
            "v1\t2000000000\t2000000000\t2\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tCOMPLETE",
        );
        let (second, _) = step_event_line(
            &mut engine,
            "v1\t3000000000\t3000000000\t3\tFLIGHT\t00000000000000000000000000000002\tABC102\tZAU1\tFLIGHT\tPLAN\tPLAN00000002\t2",
        );

        let report = flight::flight_driver_report(&engine.flights);
        assert_eq!(report.flights, engine.known_flights.len());
        assert_eq!(report.retirements_consumed, 1);
        assert!(engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == second.identity.flight_gufi));
        assert!(!engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == first.identity.flight_gufi));
    }

    #[test]
    fn invalid_staged_candidate_does_not_retire_terminal_capacity() {
        let mut engine = runtime_with_flight_capacity(1);
        let (first, _) = step_event_line(
            &mut engine,
            "v1\t1000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2",
        );
        step_event_line(
            &mut engine,
            "v1\t2000000000\t2000000000\t2\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tCOMPLETE",
        );

        // The frame receipt remains a whole second, but the source timestamp is one
        // nanosecond off the flight adapter's millisecond grid. Parsing accepts this input;
        // flight staging rejects it before terminal cleanup can consume a receipt.
        let (invalid, rejected) = step_event_line(
            &mut engine,
            "v1\t3000000000\t2999999999\t3\tFLIGHT\t00000000000000000000000000000002\tABC102\tZAU1\tFLIGHT\tPLAN\tPLAN00000002\t2",
        );
        assert_eq!(rejected.quarantine.records.len(), 1);
        assert_ne!(invalid.source_ns % 1_000_000, 0);
        assert_eq!(
            flight::flight_driver_report(&engine.flights).retirements_consumed,
            0
        );
        assert!(engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == first.identity.flight_gufi));

        let (second, _) = step_event_line(
            &mut engine,
            "v1\t4000000000\t4000000000\t4\tFLIGHT\t00000000000000000000000000000002\tABC102\tZAU1\tFLIGHT\tPLAN\tPLAN00000002\t2",
        );
        let report = flight::flight_driver_report(&engine.flights);
        assert_eq!(report.flights, 1);
        assert_eq!(report.retirements_consumed, 1);
        assert!(engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == second.identity.flight_gufi));
    }

    #[test]
    fn cancelled_full_key_is_retired_before_capacity_refill() {
        let mut engine = runtime_with_flight_capacity(1);
        let first = parse_event_line(
            "v1\t1000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2",
        )
        .unwrap();
        regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: first.receipt_ns,
                source_clock_regressions: 0,
                events: vec![first],
            },
        )
        .unwrap();
        let cancel = parse_event_line(
            "v1\t2000000000\t2000000000\t2\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tCANCEL",
        )
        .unwrap();
        regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: cancel.receipt_ns,
                source_clock_regressions: 0,
                events: vec![cancel],
            },
        )
        .unwrap();
        let second = parse_event_line(
            "v1\t3000000000\t3000000000\t3\tFLIGHT\t00000000000000000000000000000002\tABC102\tZAU1\tFLIGHT\tPLAN\tPLAN00000002\t2",
        )
        .unwrap();
        regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: second.receipt_ns,
                source_clock_regressions: 0,
                events: vec![second],
            },
        )
        .unwrap();

        let report = flight::flight_driver_report(&engine.flights);
        assert_eq!(report.flights, 1);
        assert_eq!(report.retirements_consumed, 1);
        assert_eq!(engine.known_flights.len(), 1);
        assert!(engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == second.identity.flight_gufi));
        assert!(!engine
            .known_flights
            .iter()
            .any(|known| known.key.gufi == first.identity.flight_gufi));
    }

    // REQ: OA-REG-018
    #[test]
    fn nats2023_runtime_retains_resolved_full_region_coordinate_and_identity() {
        let mut engine = runtime_with_model(repeated_name_model());
        let event = parse_event_line(
            "v1\t1000000000\t1000000000\t1\tROUTE\t00000000000000000000000000000001\tABC101\tZAU1\tFILED_ROUTE\tPLAN00000001\tFIXB\tFIX:FIXA:K2:1:- FIX:FIXB:K1:1:-",
        )
        .unwrap();
        let frame = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 1_000_000_000,
                source_clock_regressions: 0,
                events: vec![event],
            },
        )
        .unwrap();

        assert_eq!(frame.quarantine.records.len(), 0);
        let resolved = engine.routes.revisions.last().unwrap();
        assert_eq!(resolved.evidence.key.plan_id, *b"PLAN00000001");
        assert_eq!(resolved.converted.len, 2);
        assert_eq!(
            resolved.converted.lat_e7.first().copied(),
            Some(420_000_000)
        );
        assert_eq!(
            resolved.converted.lon_e7.first().copied(),
            Some(-860_000_000)
        );
        let fix = resolved.converted.ident.first().unwrap();
        assert_eq!(fix.len, 4);
        assert_eq!(fix.bytes, *b"FIXA\0\0\0\0\0\0\0\0");
        let region = resolved.converted.region.first().unwrap();
        assert_eq!(region.len, 2);
        assert_eq!(region.bytes, *b"K2\0\0");
    }

    // REQ: OA-REG-018
    #[test]
    fn injected_precommit_failure_leaves_staged_item_state_unchanged() {
        let mut engine = runtime();
        let event = parse_event_line(
            "v1\t1000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\tABCDEF\tZAU1\tFLIGHT\tASSIGNED\t12000",
        )
        .unwrap();
        surveillance::surveillance_begin(&mut engine.surveillance, 1_000_000_000).unwrap();
        let mut order = flight::flight_order_cursor(&engine.flights);

        let flights_before = flight::flight_driver_report(&engine.flights);
        let revisions_before = engine.routes.revisions.len();
        let fences_before = engine.routes.route_refusals.len();
        let cursors_before = engine.routes.cursors.len();
        let quarantine_before = quarantine_stats(&engine.quarantine_queue).unwrap();

        assert!(flight::flight_stage_order(&engine.flights, &order, &event).is_ok());
        assert!(matches!(
            route::route_stage_event(&engine.routes, &engine.model, &event).unwrap(),
            route::RouteStage::Noop
        ));
        assert!(surveillance::surveillance_stage_event(&engine.surveillance, &event).is_ok());
        let injected: Result<(), &str> = Err("injected failure between stage and commit");
        assert_eq!(injected, Err("injected failure between stage and commit"));

        assert_eq!(
            flight::flight_driver_report(&engine.flights),
            flights_before
        );
        assert_eq!(engine.routes.revisions.len(), revisions_before);
        assert_eq!(engine.routes.route_refusals.len(), fences_before);
        assert_eq!(engine.routes.cursors.len(), cursors_before);
        assert_eq!(
            quarantine_stats(&engine.quarantine_queue).unwrap(),
            quarantine_before
        );
        assert_eq!(flight::flight_order_cursor(&engine.flights), order);
        assert!(
            !engine.failed,
            "the isolated test seam precedes runtime commit"
        );
        flight::flight_commit_order(&mut order, &event);
        assert_ne!(order, flight::flight_order_cursor(&engine.flights));
    }

    // REQ: OA-REG-018
    #[test]
    fn required_route_refusal_fence_capacity_failure_poisons_runtime() {
        let mut engine = runtime();
        let key = oa_legacy_traj::core::trajectory::FlightKey {
            gufi: 1_u128.to_be_bytes(),
            aircraft_address: 0x00ab_cdef,
            centre: *b"ZAU1",
            plan_id: *b"PLAN00000001",
        };
        engine.routes.route_refusals.resize(
            route::MAX_ROUTE_REVISIONS,
            route::RouteRefusalFence {
                key,
                source_ms: Some(1),
                available_ms: 1,
                message_id: 1,
                attempted_text_sha256: [0; 32],
                reason: REASON_INPUT_INVALID,
            },
        );
        let event = parse_event_line(
            "v1\t2000000000\t2000000000\t1\tROUTE\t00000000000000000000000000000001\tABCDEF\tZAU1\tFILED_ROUTE\tPLAN00000001\tFIXB\tNOT A ROUTE",
        )
        .unwrap();
        let batch = ReceiptBatch {
            receipt_ns: 2_000_000_000,
            source_clock_regressions: 0,
            events: vec![event],
        };

        assert!(regional_step(&mut engine, &batch)
            .unwrap_err()
            .contains("capacity"));
        assert!(engine.failed);
        assert!(regional_step(&mut engine, &batch)
            .unwrap_err()
            .contains("reconstruct"));
    }

    #[test]
    fn frame_consumer_reports_exact_address_and_centre_correlation_outcomes() {
        let receipt = 1_800_000_000_000_000_000i64;
        let next_receipt = receipt + surveillance::SECOND_NS;
        let address = 0x00ab_cdef;
        let gufi_a = 0x1000_0000_0000_0000_0000_0000_0000_0001;
        let gufi_b = 0x1000_0000_0000_0000_0000_0000_0000_0002;

        let mut matched = runtime();
        let claims = [(gufi_a, address, *b"ZAU1")];
        seeded_claim_frame(&mut matched, &claims, receipt);
        let matched_frame = observed_address_frame(&mut matched, address, *b"ZAU1", next_receipt);
        assert_eq!(
            matched_frame
                .flight_correlation
                .available_flight_views_for_runtime_centre,
            claims.len() as u32
        );
        assert_eq!(
            matched_frame
                .flight_correlation
                .positive_address_claims_for_runtime_centre,
            claims.len() as u32
        );
        assert_eq!(
            matched_frame.flight_correlation.matched_observed_tracks, 0,
            "address alone is not a co-timed identity corroboration"
        );
        assert_eq!(
            matched_frame.flight_correlation.unmatched_observed_tracks,
            matched_frame.surveillance.tracks.len() as u32
        );
        assert!(!matched_frame.surveillance.tracks.is_empty());
        let traffic = matched_frame
            .surveillance
            .traffic_stats
            .expect("scope frame exposes aggregate block-policy accounting");
        assert_eq!(
            traffic.full_blocks, 0,
            "legacy ZAU1 is not an alias for native ZAU"
        );
        assert!(traffic.altitude_only_blocks > 0);

        let mut wrong_address = runtime();
        let claims = [(gufi_a, address + 1, *b"ZAU1")];
        seeded_claim_frame(&mut wrong_address, &claims, receipt);
        let unmatched_frame =
            observed_address_frame(&mut wrong_address, address, *b"ZAU1", next_receipt);
        assert_eq!(
            unmatched_frame.flight_correlation.unmatched_observed_tracks,
            unmatched_frame.surveillance.tracks.len() as u32
        );
        assert_eq!(
            unmatched_frame.flight_correlation.matched_observed_tracks,
            0
        );
        assert_eq!(
            unmatched_frame
                .surveillance
                .traffic_stats
                .expect("scope frame exposes aggregate block-policy accounting")
                .full_blocks,
            0,
            "wrong-address views cannot authorize complete blocks"
        );

        let mut ambiguous = runtime();
        let claims = [(gufi_a, address, *b"ZAU1"), (gufi_b, address, *b"ZAU1")];
        seeded_claim_frame(&mut ambiguous, &claims, receipt);
        let ambiguous_frame =
            observed_address_frame(&mut ambiguous, address, *b"ZAU1", next_receipt);
        assert_eq!(
            ambiguous_frame
                .flight_correlation
                .positive_address_claims_for_runtime_centre,
            claims.len() as u32
        );
        assert_eq!(
            ambiguous_frame.flight_correlation.ambiguous_observed_tracks,
            ambiguous_frame.surveillance.tracks.len() as u32
        );
        assert_eq!(
            ambiguous_frame.flight_correlation.matched_observed_tracks,
            0
        );
        assert_eq!(
            ambiguous_frame
                .surveillance
                .traffic_stats
                .expect("scope frame exposes aggregate block-policy accounting")
                .full_blocks,
            0,
            "ambiguous address claims cannot authorize complete blocks"
        );

        let mut wrong_centre = runtime();
        let claims = [(gufi_a, address, *b"KXYZ")];
        seeded_claim_frame(&mut wrong_centre, &claims, receipt);
        let centre_frame =
            observed_address_frame(&mut wrong_centre, address, *b"ZAU1", next_receipt);
        assert_eq!(centre_frame.flight_data_unavailable, claims.len() as u32);
        assert_eq!(
            centre_frame
                .flight_correlation
                .available_flight_views_for_runtime_centre,
            0
        );
        assert_eq!(centre_frame.flight_correlation.unmatched_observed_tracks, 1);
        assert_eq!(
            centre_frame
                .surveillance
                .traffic_stats
                .expect("scope frame exposes aggregate block-policy accounting")
                .full_blocks,
            0,
            "wrong-centre views cannot authorize complete blocks"
        );
    }

    // Kills per-frame evidence clearing and verifies receipt freshness through a coasted track,
    // successful unchanged message, refused message, exact expiry boundary, and the next stale frame.
    // REQ: OA-REG-002
    #[test]
    fn flight_identity_liveness_survives_coast_and_expires_after_refusal() {
        let base = 1_800_000_000_000_000_000i64;
        let first_identity_receipt = base + surveillance::SECOND_NS;
        let address = 0x00ab_cdef;
        let claims = [(
            0x1000_0000_0000_0000_0000_0000_0000_0001,
            address,
            *b"ZAU\0",
        )];
        let mut positive = runtime_with_centre(*b"ZAU\0");
        seeded_claim_frame(&mut positive, &claims, base);
        let matched = co_timed_identity_frame(
            &mut positive,
            address,
            first_identity_receipt,
            Some("N123AB"),
            Some(("N123AB", first_identity_receipt)),
        );
        assert_eq!(matched.flight_correlation.matched_observed_tracks, 1);
        assert!(matched
            .flights
            .first()
            .is_some_and(|row| row.view.is_some() && row.trajectory.is_some()));
        assert!(matched
            .controller_views
            .first()
            .is_some_and(|row| row.identity_present == 1));
        assert!(matched
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks > 0));

        let coast_receipt = first_identity_receipt + surveillance::SECOND_NS;
        let coast = regional_step(
            &mut positive,
            &ReceiptBatch {
                receipt_ns: coast_receipt,
                source_clock_regressions: 0,
                events: Vec::new(),
            },
        )
        .expect("command-only frame completes with the retained accepted observation");
        assert_eq!(coast.flight_correlation.matched_observed_tracks, 1);
        assert!(coast
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks > 0));

        let unchanged_receipt = coast_receipt + surveillance::SECOND_NS;
        let unchanged_before = flight::flight_driver_report(&positive.flights).unchanged;
        let refreshed = co_timed_identity_frame(
            &mut positive,
            address,
            unchanged_receipt,
            Some("N123AB"),
            Some(("N123AB", unchanged_receipt)),
        );
        assert!(flight::flight_driver_report(&positive.flights).unchanged > unchanged_before);
        assert_eq!(
            positive
                .known_flights
                .first()
                .unwrap()
                .last_consumed_receipt_ns,
            Some(first_identity_receipt),
            "a successful no-op is not a new flight receipt"
        );
        assert_eq!(refreshed.flight_correlation.matched_observed_tracks, 1);
        assert!(refreshed
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks > 0));

        let boundary_receipt = first_identity_receipt + 60 * surveillance::SECOND_NS;
        let boundary = co_timed_frame_with_plan_refusal(
            &mut positive,
            address,
            boundary_receipt,
            Some("N123AB"),
            None,
            Some(first_identity_receipt),
            false,
        );
        assert!(boundary
            .flight_operations
            .iter()
            .any(|outcome| matches!(outcome.operation, Some(oa_flight::Outcome::Refused(_)))));
        assert_eq!(boundary.flight_correlation.matched_observed_tracks, 1);
        assert!(boundary
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks > 0));

        let stale_receipt = boundary_receipt + surveillance::SECOND_NS;
        let stale =
            co_timed_identity_frame(&mut positive, address, stale_receipt, Some("N123AB"), None);
        assert_eq!(stale.flight_correlation.matched_observed_tracks, 0);
        assert_eq!(stale.flight_correlation.unmatched_observed_tracks, 1);
        assert!(stale
            .flights
            .first()
            .is_some_and(|row| row.view.is_none() && row.trajectory.is_none()));
        assert!(stale
            .controller_views
            .first()
            .is_some_and(|row| row.identity_present == 0));
        assert!(stale
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks == 0));
    }

    // Kills the same-value stale restatement mutation that refreshed a flight's lease.
    // A current applied identity change refreshes the lease; an old-source no-op does not.
    // REQ: OA-REG-002
    #[test]
    fn stale_same_value_flight_restatement_cannot_refresh_identity_lease() {
        let base = 1_800_000_000_000_000_000i64;
        let first_receipt = base + surveillance::SECOND_NS;
        let changed_receipt = first_receipt + surveillance::SECOND_NS;
        let stale_restatement_receipt = changed_receipt + 61 * surveillance::SECOND_NS;
        let address = 0x00ab_cdef;
        let gufi = 0x1000_0000_0000_0000_0000_0000_0000_0001;
        let mut runtime = runtime_with_centre(*b"ZAU\0");
        seeded_claim_frame(&mut runtime, &[(gufi, address, *b"ZAU\0")], base);
        co_timed_identity_frame(
            &mut runtime,
            address,
            first_receipt,
            Some("N123AB"),
            Some(("N123AB", first_receipt)),
        );
        let changed = co_timed_identity_frame(
            &mut runtime,
            address,
            changed_receipt,
            Some("N123AC"),
            Some(("N123AC", changed_receipt)),
        );
        assert!(changed
            .flight_operations
            .iter()
            .any(|outcome| matches!(outcome.operation, Some(oa_flight::Outcome::Applied))));
        assert_eq!(
            runtime
                .known_flights
                .first()
                .unwrap()
                .last_consumed_receipt_ns,
            Some(changed_receipt)
        );

        co_timed_identity_frame(
            &mut runtime,
            address,
            changed_receipt + 30 * surveillance::SECOND_NS,
            Some("N123AC"),
            None,
        );

        let restatement = co_timed_identity_frame(
            &mut runtime,
            address,
            stale_restatement_receipt,
            Some("N123AC"),
            Some(("N123AC", changed_receipt)),
        );
        assert!(restatement
            .flight_operations
            .iter()
            .any(|outcome| matches!(outcome.operation, Some(oa_flight::Outcome::Unchanged))));
        assert_eq!(
            runtime
                .known_flights
                .first()
                .unwrap()
                .last_consumed_receipt_ns,
            Some(changed_receipt)
        );
        assert_eq!(restatement.flight_correlation.matched_observed_tracks, 0);
        assert!(restatement
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks == 0));

        let mut stale_applied_runtime = runtime_with_centre(*b"ZAU\0");
        seeded_claim_frame(
            &mut stale_applied_runtime,
            &[(gufi, address, *b"ZAU\0")],
            base,
        );
        let stale_source = base + surveillance::SECOND_NS;
        let stale_applied_receipt = stale_source + 61 * surveillance::SECOND_NS;
        co_timed_identity_frame(
            &mut stale_applied_runtime,
            address,
            stale_source + 30 * surveillance::SECOND_NS,
            Some("N123AB"),
            None,
        );
        let stale_applied = co_timed_identity_frame(
            &mut stale_applied_runtime,
            address,
            stale_applied_receipt,
            Some("N123AB"),
            Some(("N123AB", stale_source)),
        );
        assert!(stale_applied
            .flight_operations
            .iter()
            .any(|outcome| matches!(outcome.operation, Some(oa_flight::Outcome::Applied))));
        assert_eq!(
            stale_applied_runtime
                .known_flights
                .first()
                .unwrap()
                .last_consumed_receipt_ns,
            Some(base)
        );
        assert_eq!(stale_applied.flight_correlation.matched_observed_tracks, 0);
        assert!(stale_applied
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks == 0));
    }

    // Kills using the rounded issue cutoff instead of the raw receipt for lease expiry: an
    // archived receipt just over 120 s old can still look fresh on the rounded issue grid.
    // REQ: OA-REG-013
    #[test]
    fn flight_lease_uses_raw_receipt_at_the_120_second_freshness_boundary() {
        let base = 1_800_000_000_000_000_000i64;
        let now_ns = base + 150 * surveillance::SECOND_NS;
        let address = 0x00ab_cdef;
        let gufi = 0x1000_0000_0000_0000_0000_0000_0000_0001u128;
        let drop_ns = surveillance::RECORDED_HOUR_DROP_AFTER_NS;
        let mut profile = DEFAULT_REGIONAL_RUN_PROFILE;
        profile.surveillance.tracker_timing =
            surveillance::TrackerTimingProfile::ArchivedFixed10_30_120V1;

        for (offset_ns, should_be_fresh) in [(-1, false), (0, true), (1, true)] {
            let mut runtime = runtime_with_profile_and_centre(profile, *b"ZAU\0");
            seeded_claim_frame(&mut runtime, &[(gufi, address, *b"ZAU\0")], base);
            let raw_receipt_ns = now_ns - drop_ns + offset_ns;
            let source_ns = base + 20 * surveillance::SECOND_NS;
            let issue_receipt_ns = base
                + if offset_ns > 0 {
                    31 * surveillance::SECOND_NS
                } else {
                    30 * surveillance::SECOND_NS
                };
            let identity = format!(
                "v1\t{issue_receipt_ns}\t{source_ns}\t3\tFLIGHT\t{gufi:032x}\t{address:06x}\tZAU\tFLIGHT\tIDENTITY_PARTIAL\tN123AB\t-\t-\tRAWRECEIPT\t{raw_receipt_ns}"
            );
            let accepted = regional_step(
                &mut runtime,
                &ReceiptBatch {
                    receipt_ns: issue_receipt_ns,
                    source_clock_regressions: 0,
                    events: vec![parse_event_line(&identity).unwrap()],
                },
            )
            .unwrap();
            assert!(
                accepted
                    .flight_operations
                    .iter()
                    .any(|outcome| matches!(outcome.operation, Some(oa_flight::Outcome::Applied))),
                "expected current aligned flight identity to apply, got {:?}",
                accepted.flight_operations
            );

            for tick in 1..=9i64 {
                let receipt_ns = issue_receipt_ns + tick * 10 * surveillance::SECOND_NS;
                let line = format!(
                    "v1\t{receipt_ns}\t{receipt_ns}\t{}\tSURVEILLANCE\t00000000000000000000000000000000\t{address:06x}\tZAU\tPOS\t410000000\t-870000000\t10000\t0\t0\t0\tCALLSIGN\tN123AB",
                    tick + 10
                );
                regional_step(
                    &mut runtime,
                    &ReceiptBatch {
                        receipt_ns,
                        source_clock_regressions: 0,
                        events: vec![parse_event_line(&line).unwrap()],
                    },
                )
                .unwrap();
            }

            let mut events = Vec::with_capacity(4);
            for (index, source_ns) in [
                now_ns - 2 * 10 * surveillance::SECOND_NS,
                now_ns - 10 * surveillance::SECOND_NS,
                now_ns,
            ]
            .into_iter()
            .enumerate()
            {
                let line = format!(
                    "v1\t{now_ns}\t{source_ns}\t{}\tSURVEILLANCE\t00000000000000000000000000000000\t{address:06x}\tZAU\tPOS\t410000000\t-870000000\t10000\t0\t0\t0\tCALLSIGN\tN123AB",
                    index + 1
                );
                events.push(parse_event_line(&line).unwrap());
            }
            let route = format!(
                "v1\t{now_ns}\t{now_ns}\t1\tROUTE\t{gufi:032x}\t{address:06x}\tZAU\tFILED_ROUTE\tPLAN00000001\tFIXB\tFIX:FIXA:K1:1:- FIX:FIXB:K1:1:-"
            );
            events.push(parse_event_line(&route).unwrap());
            let frame = regional_step(
                &mut runtime,
                &ReceiptBatch {
                    receipt_ns: now_ns,
                    source_clock_regressions: 0,
                    events,
                },
            )
            .unwrap();
            assert_eq!(
                runtime
                    .known_flights
                    .first()
                    .unwrap()
                    .last_consumed_receipt_ns,
                Some(raw_receipt_ns)
            );
            if should_be_fresh {
                assert_eq!(frame.flight_correlation.matched_observed_tracks, 1);
                assert!(frame
                    .surveillance
                    .traffic_stats
                    .is_some_and(|stats| stats.full_blocks > 0));
            } else {
                assert_eq!(frame.flight_correlation.matched_observed_tracks, 0);
                assert!(frame
                    .surveillance
                    .traffic_stats
                    .is_some_and(|stats| stats.full_blocks == 0));
            }
        }
    }

    // Kills inconsistent raw clocks and a sealed raw receipt hidden by an outside issue cutoff.
    // REQ: OA-REG-013, OA-SEAL-014
    #[test]
    fn runtime_refuses_inconsistent_raw_receipt_at_the_batch_boundary() {
        let mut runtime = runtime_with_centre(*b"ZAU1");
        let receipt_ns = 1_800_000_001_000_000_000i64;
        let mut event = parse_event_line(&format!(
            "v1\t{receipt_ns}\t{receipt_ns}\t1\tFLIGHT\t00000000000000000000000000000001\t000000\tZAU1\tFLIGHT\tIDENTITY_PARTIAL\tABC123\t-\t-"
        ))
        .unwrap();
        event.input_receipt_ns = receipt_ns + 1;
        let result = regional_step(
            &mut runtime,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events: vec![event],
            },
        );
        assert!(result.is_err());
        assert!(runtime.known_flights.is_empty());
        assert_eq!(flight::flight_driver_report(&runtime.flights).applied, 0);

        let second_ns = surveillance::SECOND_NS;
        let (source_ns, input_receipt_ns, receipt_ns) = oa_sealed::sealed_windows()
            .iter()
            .find_map(|window| {
                let input_receipt_ns = window.to_ns.checked_sub(1)?;
                let source_ns = window.from_ns.checked_sub(1)?;
                let receipt_ns = input_receipt_ns
                    .checked_add(second_ns - 1)?
                    .checked_div(second_ns)?
                    .checked_mul(second_ns)?;
                (source_ns >= 0
                    && oa_sealed::in_sealed_window(input_receipt_ns)
                    && !oa_sealed::in_sealed_window(source_ns)
                    && !oa_sealed::in_sealed_window(receipt_ns))
                .then_some((source_ns, input_receipt_ns, receipt_ns))
            })
            .expect("public sealed list has a raw/issue boundary fixture");
        let row = format!(
            "v1\t{receipt_ns}\t{source_ns}\t1\tFLIGHT\t00000000000000000000000000000001\t000000\tZAU1\tFLIGHT\tIDENTITY_PARTIAL\tABC123\t-\t-"
        );
        let mut sealed_raw_event = parse_event_line(&row).unwrap();
        sealed_raw_event.input_receipt_ns = input_receipt_ns;
        let mut sealed_runtime = runtime_with_centre(*b"ZAU1");
        let sealed_result = regional_step(
            &mut sealed_runtime,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events: vec![sealed_raw_event],
            },
        );
        assert!(
            matches!(sealed_result, Err(ref reason) if reason == "malformed receipt batch"),
            "sealed raw receipt must hit the direct batch clock guard; got {sealed_result:?}"
        );
        assert!(sealed_runtime.known_flights.is_empty());
        assert_eq!(
            flight::flight_driver_report(&sealed_runtime.flights).applied,
            0
        );

        let mut unsealed_runtime = runtime_with_centre(*b"ZAU1");
        let mut unsealed_event = parse_event_line(&row).unwrap();
        unsealed_event.input_receipt_ns = receipt_ns;
        let unsealed_result = regional_step(
            &mut unsealed_runtime,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events: vec![unsealed_event],
            },
        );
        assert!(
            unsealed_result.is_ok(),
            "same batch with unsealed raw receipt must pass the raw-seal boundary: {unsealed_result:?}"
        );
    }

    // Kills mismatched-call-sign authority and verifies missing-call-sign and terminal views stay advisory.
    // REQ: OA-REG-002
    #[test]
    fn flight_identity_requires_co_timed_observation_callsign_at_consumers() {
        let base = 1_800_000_000_000_000_000i64;
        let first_identity_receipt = base + surveillance::SECOND_NS;
        let unchanged_receipt = base + 2 * surveillance::SECOND_NS;
        let address = 0x00ab_cdef;
        let claims = [(
            0x1000_0000_0000_0000_0000_0000_0000_0001,
            address,
            *b"ZAU\0",
        )];

        let mut mismatch = runtime_with_centre(*b"ZAU\0");
        seeded_claim_frame(&mut mismatch, &claims, base);
        let frame = co_timed_identity_frame(
            &mut mismatch,
            address,
            first_identity_receipt,
            Some("N123AB"),
            Some(("N999ZZ", first_identity_receipt)),
        );
        assert_eq!(frame.flight_correlation.unmatched_observed_tracks, 1);
        assert!(frame
            .flights
            .first()
            .is_some_and(|row| row.view.is_none() && row.trajectory.is_none()));
        assert!(frame
            .controller_views
            .first()
            .is_some_and(|row| row.identity_present == 0));
        assert!(frame
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks == 0));

        let mut missing_callsign = runtime_with_centre(*b"ZAU\0");
        seeded_claim_frame(&mut missing_callsign, &claims, base);
        co_timed_identity_frame(
            &mut missing_callsign,
            address,
            first_identity_receipt,
            Some("N123AB"),
            Some(("N123AB", first_identity_receipt)),
        );
        let missing = co_timed_identity_frame(
            &mut missing_callsign,
            address,
            unchanged_receipt,
            None,
            None,
        );
        assert_eq!(missing.flight_correlation.matched_observed_tracks, 0);
        assert!(missing
            .controller_views
            .first()
            .is_some_and(|row| row.identity_present == 0));
        assert!(missing
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks == 0));

        let mut terminal = runtime_with_centre(*b"ZAU\0");
        seeded_claim_frame(&mut terminal, &claims, base);
        co_timed_identity_frame(
            &mut terminal,
            address,
            first_identity_receipt,
            Some("N123AB"),
            Some(("N123AB", first_identity_receipt)),
        );
        let completed = co_timed_frame_with_plan_refusal(
            &mut terminal,
            address,
            unchanged_receipt,
            Some("N123AB"),
            None,
            None,
            true,
        );
        assert!(completed
            .flight_operations
            .iter()
            .any(|outcome| matches!(outcome.operation, Some(oa_flight::Outcome::Applied))));
        assert!(!completed
            .controller_views
            .iter()
            .any(|row| row.identity_present == 1));
        assert!(completed
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks == 0));
    }

    // Kills the source-row callsign drop between ADS-B adaptation, normalized reparse, runtime
    // correlation, and the full traffic presenter block.
    // REQ: OA-REG-002
    #[test]
    fn adsb_source_callsign_reaches_co_timed_runtime_presenter() {
        let base = 1_800_000_000_000_000_000i64;
        let first_time = base + surveillance::SECOND_NS;
        let second_time = first_time + surveillance::SECOND_NS;
        let address = 0x00ab_cdef;
        let gufi = 0x1000_0000_0000_0000_0000_0000_0000_0001u128;
        let centre = *b"ZAU\0";
        let mut reports = oa_adsb::Reports::default();
        for time_ns in [first_time, second_time] {
            oa_adsb::reports_push(
                &mut reports,
                &oa_adsb::Row {
                    callsign: *b"N123AB  ",
                    time_ns,
                    icao: address,
                    lat_e7: 410_000_000,
                    lon_e7: -870_000_000,
                    baro_alt_ft: 10_000,
                    gs_dkt: 1_000,
                    track_ddeg: 900,
                    vrate_fpm: 0,
                    nic: 8,
                    nac_p: 9,
                    nac_v: 2,
                    sil: 3,
                    version: 2,
                    source: 0,
                    ..oa_adsb::ROW_EMPTY
                },
            );
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &reports, 0, 2).unwrap();
        let input = crate::recordings_reader::RecordedFrame {
            source: crate::recordings_reader::RecordingSource::Adsb,
            stream_index: 0,
            receipt_ns: first_time,
            sequence: 1,
            jms_timestamp_ms: first_time / 1_000_000,
            mono_ns: None,
            oaf_flags: oa_feed::FLAG_BYTES,
            byte_offset: 0,
            skipped_before: 0,
            kind: oa_feed::Kind::Message,
            payload,
        };
        let converted = crate::recordings::adapt_recorded_frames(
            core::slice::from_ref(&input),
            crate::recordings_reader::RecordingCounts {
                total_decoded_bytes: u64::try_from(input.payload.len()).unwrap(),
                ..crate::recordings_reader::RecordingCounts::default()
            },
            8,
            centre,
        )
        .unwrap();
        let normalized = crate::recordings::serialize_recorded_events(&converted.events).unwrap();
        let positions = normalized
            .split_terminator('\n')
            .skip(1)
            .map(parse_event_line)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(positions.len(), 2);
        assert!(positions.iter().all(|event| matches!(
            event.kind,
            EventKind::Position { track_callsign: Some(callsign), .. }
                if callsign == *b"N123AB\0\0"
        )));

        let mut engine = runtime_with_centre(centre);
        let claims = [(gufi, address, centre)];
        seeded_claim_frame(&mut engine, &claims, base);
        let first = positions.first().copied().unwrap();
        regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: first.receipt_ns,
                source_clock_regressions: 0,
                events: vec![first],
            },
        )
        .unwrap();
        let second = positions.get(1).copied().unwrap();
        let identity = parse_event_line(&format!(
            "v1\t{second_time}\t{second_time}\t3\tFLIGHT\t{gufi:032x}\t{address:06x}\tZAU\tFLIGHT\tIDENTITY_PARTIAL\tN123AB\t-\t-"
        ))
        .unwrap();
        let route = parse_event_line(&format!(
            "v1\t{second_time}\t{second_time}\t1\tROUTE\t{gufi:032x}\t{address:06x}\tZAU\tFILED_ROUTE\tPLAN00000001\tFIXB\tFIX:FIXA:K1:1:- FIX:FIXB:K1:1:-"
        ))
        .unwrap();
        let output = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: second.receipt_ns,
                source_clock_regressions: 0,
                events: vec![second, identity, route],
            },
        )
        .unwrap();
        assert!(output
            .flights
            .first()
            .is_some_and(|row| { row.view.is_some() && row.trajectory.is_some() }));
        assert!(output
            .controller_views
            .first()
            .is_some_and(|view| view.identity_present == 1));
        assert!(output
            .surveillance
            .traffic_stats
            .is_some_and(|stats| stats.full_blocks > 0));
    }

    #[test]
    fn native_three_character_zau_centre_allows_exact_current_view_block() {
        let receipt = 1_800_000_000_000_000_000i64;
        let address = 0x00ab_cdef;
        let gufi = 0x1000_0000_0000_0000_0000_0000_0000_0001;
        let centre = *b"ZAU\0";
        let mut engine = runtime_with_centre(centre);
        seeded_claim_frame(&mut engine, &[(gufi, address, centre)], receipt);
        let frame = co_timed_identity_frame(
            &mut engine,
            address,
            receipt + surveillance::SECOND_NS,
            Some("N123AB"),
            Some(("N123AB", receipt + surveillance::SECOND_NS)),
        );
        assert_eq!(frame.flight_correlation.matched_observed_tracks, 1);
        let traffic = frame
            .surveillance
            .traffic_stats
            .expect("regional raster carries traffic block accounting");
        assert_eq!(traffic.full_blocks, 1);
        assert_eq!(traffic.altitude_only_blocks, 0);
    }

    #[test]
    fn recorded_profile_is_separate_and_stays_within_predeclared_bounds() {
        let default = DEFAULT_REGIONAL_RUN_PROFILE;
        assert_eq!(default.surveillance.max_tracks, surveillance::MAX_TRACKS);
        assert_eq!(default.surveillance.max_frames, surveillance::MAX_FRAMES);
        assert_eq!(
            default.surveillance.tracker_timing,
            surveillance::TrackerTimingProfile::DefaultV1
        );
        assert_eq!(default.max_flights, MAX_DRIVER_FLIGHTS);
        assert_eq!(default.max_total_events, crate::session::MAX_SESSION_EVENTS);
        assert_eq!(default.max_events_per_receipt, MAX_EVENTS_PER_RECEIPT);

        let recorded = RegionalRunProfile::recorded_peak();
        assert_eq!(
            recorded.surveillance.tracker_timing,
            surveillance::TrackerTimingProfile::DefaultV1
        );
        assert_eq!(recorded.surveillance.max_tracks, 4096);
        assert_eq!(recorded.surveillance.max_frames, 20_000_000);
        assert_eq!(recorded.surveillance.max_drain_rows, 262_144);
        assert_eq!(recorded.max_flights, 8192);
        assert_eq!(recorded.max_total_events, 10_000_000);
        assert_eq!(recorded.max_events_per_receipt, MAX_EVENTS_PER_RECEIPT);
        assert!(!recorded.surveillance.render_scope);
        let pairs = recorded.surveillance.max_tracks * (recorded.surveillance.max_tracks - 1) / 2;
        assert!(pairs <= recorded.surveillance.max_candidate_pairs);
        assert!(recorded.surveillance.max_candidate_pairs <= 1 << 24);

        let rendered = RegionalRunProfile::recorded_peak_rendered();
        assert_eq!(
            rendered.surveillance.max_tracks,
            recorded.surveillance.max_tracks
        );
        assert_eq!(
            rendered.surveillance.max_open_alerts,
            recorded.surveillance.max_open_alerts
        );
        assert_eq!(
            rendered.surveillance.max_candidate_pairs,
            recorded.surveillance.max_candidate_pairs
        );
        assert_eq!(
            rendered.surveillance.max_ca_work_per_frame,
            recorded.surveillance.max_ca_work_per_frame
        );
        assert_eq!(rendered.max_total_events, recorded.max_total_events);
        assert_eq!(rendered.max_events_per_receipt, MAX_EVENTS_PER_RECEIPT);
        assert_eq!(rendered.max_flights, rendered.surveillance.max_tracks);
        assert!(rendered.surveillance.render_scope);
        assert_eq!(
            rendered.surveillance.max_presentation_updates,
            surveillance::MAX_PRESENTATION_UPDATES
        );

        let hour = RegionalRunProfile::recorded_hour_rendered();
        assert_eq!(
            hour.max_events_per_receipt,
            RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
        );
        assert_eq!(
            hour.surveillance.max_frames,
            crate::cli::RECORDED_HOUR_FRAMES
        );
        assert!(hour.surveillance.render_scope);
    }

    #[test]
    fn recorded_hour_runtime_accepts_dense_cutoff_without_raising_legacy_cap() {
        let model = synthetic_model().unwrap();
        let snapshot_bytes = oa_airspace::core::snapshot::encode(&model).unwrap();
        let snapshot_sha = oa_sha256::sha256(&snapshot_bytes);
        let config = SurveillanceConfig {
            centre_lat_e7: 410_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 180.0,
        };
        let receipt_ns = 1_800_000_000_000_000_000;
        let batch = ReceiptBatch {
            receipt_ns,
            source_clock_regressions: 0,
            events: (1..=MAX_EVENTS_PER_RECEIPT + 1)
                .map(|sequence| NormalizedEvent {
                    receipt_ns,
                    input_receipt_ns: receipt_ns,
                    source_ns: receipt_ns,
                    sequence: sequence as u64,
                    source_message_id: None,
                    source: SourceKind::Surveillance,
                    identity: Identity {
                        flight_gufi: 0,
                        aircraft_address: sequence as u32,
                    },
                    centre: *b"ZAU1",
                    kind: EventKind::Position {
                        latitude_e7: 410_000_000,
                        longitude_e7: -870_000_000,
                        altitude_ft: Some(10_000),
                        east_mmps: 0,
                        north_mmps: 0,
                        vertical_rate_fpm: None,
                        track_callsign: None,
                    },
                })
                .collect(),
        };
        let mut legacy = regional_new_with_profile(
            model.clone(),
            snapshot_sha,
            *b"ZAU1",
            config,
            RegionalRunProfile::recorded_peak_rendered(),
        )
        .unwrap();
        assert_eq!(
            surveillance::tracker_drop_after_ns(&legacy.surveillance),
            oa_track::DEFAULT_CONFIG.drop_after_ns
        );
        assert!(regional_step(&mut legacy, &batch).is_err());

        let mut hour = regional_new_with_profile(
            model,
            snapshot_sha,
            *b"ZAU1",
            config,
            RegionalRunProfile::recorded_hour_rendered(),
        )
        .unwrap();
        assert_eq!(
            surveillance::tracker_drop_after_ns(&hour.surveillance),
            oa_track::DEFAULT_CONFIG.drop_after_ns
        );
        let frame = regional_step(&mut hour, &batch).unwrap();
        assert_eq!(
            frame.input.input_events as usize,
            MAX_EVENTS_PER_RECEIPT + 1
        );
    }

    // kills the mutation that rejects a native three-character ARTCC after
    // it has been faithfully zero-padded to the four-byte runtime identity.
    // REQ: OA-REG-013
    #[test]
    fn runtime_accepts_zero_padded_source_artcc_and_refuses_embedded_padding() {
        let mut model = synthetic_model().expect("public synthetic model");
        model.meta.artcc = "ZAU".to_owned();
        let snapshot = oa_airspace::core::snapshot::encode(&model).expect("encode model");
        let snapshot_sha = oa_sha256::sha256(&snapshot);
        let cfg = SurveillanceConfig {
            centre_lat_e7: 410_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 180.0,
        };

        let runtime = regional_new(model.clone(), snapshot_sha, *b"ZAU\0", cfg)
            .expect("zero-padded source ARTCC remains a valid runtime identity");
        assert_eq!(runtime.centre, *b"ZAU\0");

        let refusal = regional_new(model, snapshot_sha, *b"ZA\0U", cfg);
        assert!(refusal.is_err(), "embedded zero padding must be refused");
    }

    // REQ: OA-REG-019
    #[test]
    fn explicit_rendered_recording_profile_processes_more_than_default_track_capacity() {
        let model = synthetic_model().unwrap();
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        let mut engine = regional_new_with_profile(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            RegionalRunProfile::recorded_peak_rendered(),
        )
        .unwrap();
        for second in 1..=3_i64 {
            let time_ns = second * 1_000_000_000;
            let longitude_e7 = -870_000_000 + i32::try_from(second - 1).unwrap() * 12_000;
            let events = (0..33_u32)
                .map(|index| {
                    let sequence = (second as u32 - 1) * 33 + index + 1;
                    let identity = u128::from(index + 1);
                    let address = 0x00aa_0000 + index;
                    parse_event_line(&format!(
                        "v1\t{time_ns}\t{time_ns}\t{sequence}\tSURVEILLANCE\t{identity:032x}\t{address:06x}\tZAU1\tPOS\t410000000\t{longitude_e7}\t10000\t100000\t0\t0"
                    ))
                    .unwrap()
                })
                .collect();
            let frame = regional_step(
                &mut engine,
                &ReceiptBatch {
                    receipt_ns: time_ns,
                    source_clock_regressions: 0,
                    events,
                },
            )
            .unwrap();
            if second == 3 {
                assert_eq!(frame.surveillance.tracks.len(), 33);
                assert_eq!(regional_pixels(&engine).unwrap().len(), 800 * 600 * 4);
            }
        }
    }

    // REQ: OA-REG-019
    #[test]
    fn recorded_render_profile_refuses_candidate_capacity_below_its_track_pair_bound() {
        let model = synthetic_model().unwrap();
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        let mut invalid = RegionalRunProfile::recorded_peak_rendered();
        invalid.surveillance.max_candidate_pairs = 1;
        let result = regional_new_with_profile(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            invalid,
        );
        assert!(result
            .err()
            .is_some_and(|error| error.contains("bounded capacity contract")));
    }

    // REQ: OA-NAM-039
    #[test]
    fn composed_runtime_receives_session_admitted_nam_fpl() {
        use crate::nam_flight::NamFlightContext;
        use crate::nam_handoff::{
            nam_session_ingress_local_event, nam_session_ingress_new, nam_session_ingress_receive,
        };
        use oa_nam::frame::frame;
        use oa_nam::{
            encode, model as nam_model, parse, session, Aerodrome, Field13, Field16, Hhmm,
        };

        let model = synthetic_model().expect("synthetic model");
        let mut engine = runtime_with_model(model);
        let local = oa_nam::Facility(*b"CZWG");
        let peer = oa_nam::Facility(*b"KZMP");
        let mut ingress = nam_session_ingress_new(session::default_config(local, peer));
        let actions = nam_session_ingress_local_event(&mut ingress, session::Event::Activate, 0)
            .expect("activate NAM session");
        let number = actions
            .as_slice()
            .first()
            .and_then(|action| match action {
                session::Action::SendIrq { number } => Some(*number),
                _ => None,
            })
            .expect("session activation requests IRQ");
        assert_eq!(actions.as_slice().len(), 1);
        let irs = nam_model::bare(nam_model::Header {
            kind: oa_nam::Kind::Irs,
            id: nam_model::MsgId {
                sender: peer,
                receiver: local,
                number: 0,
            },
            reference: Some(nam_model::MsgId {
                sender: local,
                receiver: peer,
                number,
            }),
        });
        let irs_frame = frame(&encode(&irs).expect("IRS encodes")).expect("IRS frames");
        let admitted_irs =
            nam_session_ingress_receive(&mut ingress, &irs_frame, 1).expect("IRS admitted");
        assert!(admitted_irs.accepted.is_none());

        let mut fpl = parse(b"(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120F350 CYSC DCT KMPV-KMPV0053-EET/KZBW0003)")
            .expect("FPL parses");
        fpl.header.id.sender = peer;
        fpl.header.id.receiver = local;
        fpl.header.id.number = 1;
        fpl.f13 = Some(Field13 {
            aerodrome: Aerodrome(*b"KAAA"),
            time: Some(Hhmm {
                hour: 12,
                minute: 0,
            }),
        });
        fpl.f16 = Some(Field16 {
            aerodrome: Aerodrome(*b"KBBB"),
            eet: Some(Hhmm {
                hour: 0,
                minute: 53,
            }),
            alternates: Vec::new(),
        });
        fpl.f15.as_mut().expect("FPL Item 15").route = vec!["FIXA".into(), "FIXB".into()];
        let bytes = frame(&encode(&fpl).expect("FPL encodes")).expect("FPL frames");
        let gufi = 0x1020_3040_5060_7080_90a0_b0c0_d0e0_f001;
        let context = NamFlightContext {
            key: oa_flight::key_gufi(gufi),
            route_key: oa_legacy_traj::core::trajectory::FlightKey {
                gufi: gufi.to_be_bytes(),
                aircraft_address: 0x123456,
                centre: *b"KZMP",
                plan_id: *b"PLAN00000001",
            },
            plan_id: *b"PLAN00000001",
            centre: *b"KZMP",
            plan_status: oa_flight::ST_PROPOSED,
            source_ns: 1_000_000,
            receipt_ns: 1_000_000,
            response_number: 101,
        };
        let (session, result) = regional_receive_nam_flight(
            &mut engine,
            &mut ingress,
            &bytes,
            2,
            context,
            &[],
            oa_nam::Profile::Core,
        )
        .expect("composed runtime NAM receive");
        assert!(session.accepted.is_some());
        let result = result.expect("admitted FPL reaches application");
        assert!(result.applied);
        assert_eq!(result.reply.header.kind, oa_nam::Kind::Lam);
        assert_eq!(crate::flight::flight_driver_len(&engine.flights), 1);
        assert_eq!(engine.routes.revisions.len(), 1);
        assert_eq!(
            engine
                .routes
                .revisions
                .first()
                .expect("FPL route revision")
                .evidence
                .key
                .gufi,
            gufi.to_be_bytes()
        );
    }

    const EXACT_FINAL_MONITOR_PERIOD_NS: i64 = 4_800_000_000;

    const OVER_FINAL_MONITOR_PERIOD_NS: i64 = 4_801_000_000;

    fn final_monitor_config(max_r95_m: f64) -> RuntimeFinalMonitorConfig {
        RuntimeFinalMonitorConfig {
            pair: final_monitor_core::FinalPair {
                id: final_monitor_core::FinalPairId {
                    first_runway_id: 27_001,
                    first_end_name: *b"27R",
                    second_runway_id: 27_002,
                    second_end_name: *b"27L",
                },
                projection_origin: Some(oa_runway::plane::Geo {
                    lat_deg: 41.0,
                    lon_deg: -87.0,
                }),
                origin: oa_runway::plane::Local {
                    east_m: 0.0,
                    north_m: 0.0,
                },
                axis_east: 1.0,
                axis_north: 0.0,
                normal_east: 0.0,
                normal_north: 1.0,
                course_length_m: 10_000.0,
                ntz_half_width_m: 10.0,
                max_r95_m,
                lookahead_ns: 60_000_000_000,
                update_ns: EXACT_FINAL_MONITOR_PERIOD_NS as u64,
            },
            assignments: vec![FinalApproachAssignment {
                flight: oa_flight::key_gufi(1),
                runway_id: 27_001,
                end_name: *b"27R",
            }],
        }
    }

    fn configured_runtime(max_r95_m: f64) -> RegionalRuntime {
        let model = synthetic_model().unwrap();
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        regional_new_with_runtime_final_monitor(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            DEFAULT_REGIONAL_RUN_PROFILE,
            final_monitor_config(max_r95_m),
        )
        .unwrap()
    }

    fn configured_runtime_with_profile(
        profile: RegionalRunProfile,
        monitor: RuntimeFinalMonitorConfig,
    ) -> Result<RegionalRuntime, String> {
        let model = synthetic_model().expect("public synthetic model");
        let snapshot = oa_airspace::core::snapshot::encode(&model).expect("encode model");
        regional_new_with_runtime_final_monitor(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            profile,
            monitor,
        )
    }

    // Mutation killed: admit more assignments than the presenter can represent, then omit or
    // truncate the excess while rendering final-monitor markers.
    #[test]
    fn final_monitor_refuses_assignments_beyond_renderable_marker_capacity() {
        let model = synthetic_model().unwrap();
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        let mut monitor = final_monitor_config(100.0);
        monitor.assignments = (1..=MAX_FINAL_APPROACH_ASSIGNMENTS + 1)
            .map(|key| FinalApproachAssignment {
                flight: oa_flight::key_gufi(u128::try_from(key).expect("bounded key")),
                runway_id: 27_001,
                end_name: *b"27R",
            })
            .collect();
        let result = regional_new_with_runtime_final_monitor(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            DEFAULT_REGIONAL_RUN_PROFILE,
            monitor,
        );
        assert!(result.is_err(), "unrenderable assignments must be refused");
    }

    // REQ: OA-REG-019
    // Mutation killed: accept a configured assignment count larger than the selected track cap.
    #[test]
    fn final_monitor_admission_uses_selected_profile_track_capacity() {
        let mut profile = DEFAULT_REGIONAL_RUN_PROFILE;
        profile.surveillance.max_tracks = 32;
        let mut at_edge = final_monitor_config(100.0);
        at_edge.assignments = (1..=32)
            .map(|key| FinalApproachAssignment {
                flight: oa_flight::key_gufi(key),
                runway_id: 27_001,
                end_name: *b"27R",
            })
            .collect();
        assert!(configured_runtime_with_profile(profile, at_edge).is_ok());

        let mut beyond_edge = final_monitor_config(100.0);
        beyond_edge.assignments = (1..=33)
            .map(|key| FinalApproachAssignment {
                flight: oa_flight::key_gufi(key),
                runway_id: 27_001,
                end_name: *b"27R",
            })
            .collect();
        let error = configured_runtime_with_profile(profile, beyond_edge)
            .err()
            .expect("first count past the 32-track profile must refuse at startup");
        assert!(error.contains("selected profile track capacity 32"));
    }

    // REQ: OA-REG-019, OA-DSP-078
    // Mutation killed: install a final monitor on a frame too narrow for its real presenter.
    #[test]
    fn final_monitor_admission_refuses_unrenderable_profile_width() {
        let mut at_boundary = DEFAULT_REGIONAL_RUN_PROFILE;
        at_boundary.surveillance.frame_width = 500;
        assert!(configured_runtime_with_profile(at_boundary, final_monitor_config(100.0)).is_ok());

        let mut profile = DEFAULT_REGIONAL_RUN_PROFILE;
        profile.surveillance.frame_width = 480;
        let error = configured_runtime_with_profile(profile, final_monitor_config(100.0))
            .err()
            .expect("narrow rendered profile must refuse before the first receipt");
        assert!(error.contains("width at least 500 px"));
    }

    // REQ: OA-REG-019, OA-DSP-078
    // Mutation killed: allow the MSAW provider and final monitor to exceed their shared marker
    // rows, then fail when both real product packets reach the runtime presenter.
    #[test]
    fn dual_alert_families_fit_the_composed_edge_and_refuse_the_next_msaw_row() {
        let mut edge_runtime = configured_runtime(100.0);
        let edge_service = crate::msaw_bridge::regional_msaw_synthetic_for_profile(
            31,
            edge_runtime.profile.surveillance.max_frames,
        )
        .expect("31 MSAW rows plus one assigned final row fit the 32-row envelope");
        regional_attach_msaw(&mut edge_runtime, edge_service)
            .expect("both provider capacities are admitted before the first frame");

        let mut too_wide = configured_runtime(100.0);
        let over_service = crate::msaw_bridge::regional_msaw_synthetic_for_profile(
            32,
            too_wide.profile.surveillance.max_frames,
        )
        .expect("provider can independently construct a 32-row service");
        let error = regional_attach_msaw(&mut too_wide, over_service)
            .expect_err("one final row plus 32 MSAW rows exceeds shared capacity");
        assert!(error.contains("open-alert capacity 32 exceeds composed profile allowance 31"));
        assert_eq!(too_wide.frame_sequence, 0);
        assert!(!too_wide.failed);

        let mut plain_runtime = runtime();
        let plain_frame = seed_low_altitude_population(&mut plain_runtime);
        let plain_counts =
            surveillance::surveillance_overlay_primitive_counts(&plain_runtime.surveillance);
        let frame = seed_low_altitude_population(&mut edge_runtime);
        assert!(frame
            .final_monitor
            .alerts
            .iter()
            .any(|alert| alert.severity == final_monitor_core::FinalSeverity::Warning));
        let msaw = frame
            .surveillance
            .msaw
            .as_ref()
            .expect("attached MSAW service runs in the actual frame");
        assert_eq!(msaw.open.len(), 31, "each admitted MSAW row remains open");
        assert!(frame
            .surveillance
            .msaw_markers
            .is_some_and(|stats| stats.drawn == 31));
        assert!(frame.surveillance.raster_rendered);
        let edge_counts =
            surveillance::surveillance_overlay_primitive_counts(&edge_runtime.surveillance);
        assert_eq!(
            edge_counts.circles.checked_sub(plain_counts.circles),
            Some(64),
            "31 MSAW rows and one final row each emit two circles at the runtime consumer"
        );
        assert_eq!(plain_frame.surveillance.tracks.len(), 31);
        assert_eq!(frame.surveillance.tracks.len(), 31);
    }

    fn seed_low_altitude_population(runtime: &mut RegionalRuntime) -> RegionalFrame {
        let mut last = None;
        for scan in 0_i64..4 {
            let time_ns = (scan + 1) * 1_000_000_000;
            last = Some(
                regional_step(runtime, &low_altitude_population_batch(time_ns, scan == 0))
                    .expect("bounded low-altitude population produces a rendered frame"),
            );
        }
        last.expect("four scans produce an admitted runtime frame")
    }

    fn low_altitude_population_batch(receipt_ns: i64, include_flight_setup: bool) -> ReceiptBatch {
        let mut batch = final_monitor_batch_with_altitude(
            receipt_ns,
            receipt_ns,
            410_000_000,
            0,
            100,
            include_flight_setup,
        );
        for row in 1_u32..31 {
            let address = 0x000abd_u32 + row;
            let latitude_e7 = 410_000_000 + i32::try_from(row).expect("30 row offset") * 100;
            let sequence = row + 1;
            let callsign = format!("N{row:05}");
            let line = format!(
                "v1\t{receipt_ns}\t{receipt_ns}\t{sequence}\tSURVEILLANCE\t00000000000000000000000000000000\t{address:06x}\tZAU1\tPOS\t{latitude_e7}\t-870000000\t100\t0\t0\t0\tCALLSIGN\t{callsign}"
            );
            batch
                .events
                .push(parse_event_line(&line).expect("bounded synthetic airborne report"));
        }
        batch
    }

    fn final_monitor_batch(
        receipt_ns: i64,
        source_ns: i64,
        latitude_e7: i32,
        north_mmps: i32,
        include_flight_setup: bool,
    ) -> ReceiptBatch {
        final_monitor_batch_with_altitude(
            receipt_ns,
            source_ns,
            latitude_e7,
            north_mmps,
            10_000,
            include_flight_setup,
        )
    }

    fn final_monitor_batch_with_altitude(
        receipt_ns: i64,
        source_ns: i64,
        latitude_e7: i32,
        north_mmps: i32,
        altitude_ft: i32,
        include_flight_setup: bool,
    ) -> ReceiptBatch {
        let mut events = vec![parse_event_line(&format!(
            "v1\t{receipt_ns}\t{source_ns}\t1\tSURVEILLANCE\t00000000000000000000000000000000\t000abc\tZAU1\tPOS\t{latitude_e7}\t-870000000\t{altitude_ft}\t0\t{north_mmps}\t0\tCALLSIGN\tN123AB"
        )).unwrap()];
        if include_flight_setup {
            events.push(parse_event_line(&format!(
                "v1\t{receipt_ns}\t{receipt_ns}\t1\tFLIGHT\t{:032x}\t000abc\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2",
                1_u128
            )).unwrap());
            events.push(
                parse_event_line(&format!(
                "v1\t{receipt_ns}\t{receipt_ns}\t2\tFLIGHT\t{:032x}\t000abc\tZAU1\tFLIGHT\tADDRESS",
                1_u128
            ))
                .unwrap(),
            );
            events.push(parse_event_line(&format!(
                "v1\t{receipt_ns}\t{receipt_ns}\t3\tFLIGHT\t{:032x}\t000abc\tZAU1\tFLIGHT\tIDENTITY_PARTIAL\tN123AB\t-\t-",
                1_u128
            )).unwrap());
        }
        ReceiptBatch {
            receipt_ns,
            source_clock_regressions: 0,
            events,
        }
    }

    fn assert_runtime_status_layout(
        runtime: &RegionalRuntime,
        console: &str,
        degraded: bool,
        expected_safety_rows: usize,
    ) {
        let rows = surveillance::surveillance_text_rows(&runtime.surveillance)
            .expect("runtime renderer text rows");
        assert_eq!(
            runtime.last_automation.effective,
            EffectiveAutomationMode::Surveillance,
            "synthetic TRACK source remains available during this presentation test"
        );
        let mode_text = format!("{console} | MODE SURVEILLANCE | FUSION OFF");
        let expected_mode_lines: Vec<String> = mode_text
            .as_bytes()
            .chunks(64)
            .map(|line| {
                std::str::from_utf8(line)
                    .expect("mode text is ASCII")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            expected_mode_lines.len(),
            3,
            "128-byte console uses three rows"
        );
        let mut mode_bounds = Vec::with_capacity(expected_mode_lines.len());
        for expected in &expected_mode_lines {
            let found: Vec<_> = rows.iter().filter(|row| row.text == *expected).collect();
            assert_eq!(
                found.len(),
                1,
                "every console/mode row appears exactly once"
            );
            mode_bounds.push(found.first().expect("one operator row").bounds);
        }
        let final_rows: Vec<_> = rows
            .iter()
            .filter(|row| row.text.starts_with("FINAL") && row.text.contains("PROTECTION LOST"))
            .collect();
        assert_eq!(
            final_rows.len(),
            1,
            "final-monitor summary is rendered once"
        );
        assert_eq!(
            final_rows
                .first()
                .map(|row| row.text.starts_with("FINAL DEGRADED")),
            Some(degraded),
            "runtime status text reflects the completed monitor result"
        );
        let msaw_rows: Vec<_> = rows
            .iter()
            .filter(|row| row.text.starts_with("MSAW WARNING"))
            .collect();
        assert_eq!(
            msaw_rows.len(),
            1,
            "configured MSAW summary is rendered once"
        );
        let final_bounds = final_rows.first().expect("one final summary").bounds;
        let safety_rows: Vec<_> = rows
            .iter()
            .filter(|row| {
                !expected_mode_lines.iter().any(|line| line == &row.text)
                    && row.bounds.y0 > final_bounds.y0
                    && row.bounds.y1
                        < i32::try_from(surveillance::REGIONAL_STATUS_HEADER_INSET_PX)
                            .expect("small status inset")
            })
            .collect();
        assert_eq!(
            safety_rows.len(),
            expected_safety_rows,
            "all compact safety rows reported by the runtime remain visible"
        );
        let mut checked_bounds = mode_bounds;
        checked_bounds.push(msaw_rows.first().expect("one MSAW summary").bounds);
        checked_bounds.push(final_bounds);
        checked_bounds.extend(safety_rows.iter().map(|row| row.bounds));
        let frame_width =
            i32::try_from(runtime.profile.surveillance.frame_width).expect("bounded frame width");
        for (index, bounds) in checked_bounds.iter().enumerate() {
            assert!(bounds.x0 >= 0 && bounds.y0 >= 0);
            assert!(bounds.x1 <= frame_width);
            assert!(
                bounds.y1
                    <= i32::try_from(surveillance::REGIONAL_STATUS_HEADER_INSET_PX)
                        .expect("small status inset"),
                "status row remains inside the reserved band: {bounds:?}"
            );
            for other in checked_bounds.iter().skip(index + 1) {
                assert_eq!(
                    status_bounds_overlap(*bounds, *other),
                    0,
                    "runtime status text bounds overlap: {bounds:?} / {other:?}"
                );
            }
        }
    }

    fn status_bounds_overlap(
        first: oa_display::core::font::TextBounds,
        second: oa_display::core::font::TextBounds,
    ) -> i64 {
        let width = i64::from(first.x1.min(second.x1) - first.x0.max(second.x0)).max(0);
        let height = i64::from(first.y1.min(second.y1) - first.y0.max(second.y0)).max(0);
        width * height
    }

    fn seed_confirmed_final_monitor_track(
        runtime: &mut RegionalRuntime,
        latitude_e7: i32,
        north_mmps: i32,
    ) -> RegionalFrame {
        seed_confirmed_final_monitor_track_from(runtime, latitude_e7, north_mmps, 1_000_000_000)
    }

    fn seed_confirmed_final_monitor_track_from(
        runtime: &mut RegionalRuntime,
        latitude_e7: i32,
        north_mmps: i32,
        first_scan_ns: i64,
    ) -> RegionalFrame {
        let mut latest = None;
        for scan in 0_i64..4 {
            let time_ns = first_scan_ns + scan * 1_000_000_000;
            latest = Some(
                regional_step(
                    runtime,
                    &final_monitor_batch(time_ns, time_ns, latitude_e7, north_mmps, scan == 0),
                )
                .unwrap(),
            );
        }
        latest.expect("four synthetic scans produce a final frame")
    }

    // REQ: OA-RWY-015
    #[test]
    fn configured_runtime_runs_caution_on_exact_4_8_second_source_period() {
        // Mutation killed: configured runtime ignores the explicit flight-key assignment and emits no caution.
        // Mutation killed: measure input cadence from the last evaluation, making 4.0→8.8 look like 6.8 s.
        let mut runtime = configured_runtime(100.0);
        let first = seed_confirmed_final_monitor_track(&mut runtime, 410_009_000, -10_000);
        assert_eq!(
            first.final_monitor.status,
            FinalMonitorStatus::Available,
            "flight-data={:?} unavailable={} tracks={:?} ops={:?} quarantine={:?} feeds={:?} tracker={:?}",
            first.flight_data,
            first.flight_data_unavailable,
            first.surveillance.tracks,
            first.flight_operations,
            first.quarantine.records,
            first.surveillance.feeds,
            first.surveillance.tracker_stats
        );
        assert!(first
            .final_monitor
            .alerts
            .iter()
            .any(|alert| { alert.severity == final_monitor_core::FinalSeverity::Caution }));

        let last_input_ns = runtime
            .final_monitor
            .as_ref()
            .expect("explicit monitor config remains attached")
            .state
            .last_input_frame_ns;
        assert!(
            last_input_ns <= u64::try_from(first.surveillance.time_ns).unwrap(),
            "the monitor clock cannot be ahead of the latest processed frame"
        );
        let monitor_state = runtime.final_monitor.as_ref().unwrap().state;
        assert_eq!(monitor_state.last_input_frame_ns, 4_000_000_000);
        assert_eq!(monitor_state.last_evaluation_ns, 2_000_000_000);
        let next_scan_ns =
            i64::try_from(last_input_ns + EXACT_FINAL_MONITOR_PERIOD_NS as u64).unwrap();
        assert_eq!(
            runtime.final_monitor.as_ref().unwrap().pair.update_ns,
            EXACT_FINAL_MONITOR_PERIOD_NS as u64
        );
        assert_eq!(
            next_scan_ns as u64 - last_input_ns,
            EXACT_FINAL_MONITOR_PERIOD_NS as u64,
            "fixture advances by the exact monitor period"
        );
        assert!(first
            .surveillance
            .tracks
            .iter()
            .all(|track| track.newest_source_ns == first.surveillance.time_ns));
        let second = regional_step(
            &mut runtime,
            &final_monitor_batch(next_scan_ns, next_scan_ns, 410_004_700, -10_000, false),
        )
        .unwrap();
        assert_eq!(
            second.final_monitor.status,
            FinalMonitorStatus::Available,
            "first_frame_time={} first_track_source={:?} previous_update={} actual_interval={} limit={} second_frame_time={} second_track_sources={:?} tracker={:?}",
            first.surveillance.time_ns,
            first
                .surveillance
                .tracks
                .iter()
                .map(|track| track.newest_source_ns)
                .collect::<Vec<_>>(),
            last_input_ns,
            next_scan_ns as u64 - last_input_ns,
            runtime.final_monitor.as_ref().unwrap().pair.update_ns,
            second.surveillance.time_ns,
            second
                .surveillance
                .tracks
                .iter()
                .map(|track| track.newest_source_ns)
                .collect::<Vec<_>>(),
            second.surveillance.tracker_stats
        );
        assert_eq!(
            second
                .final_monitor
                .timing
                .unwrap()
                .observed_input_interval_ns,
            EXACT_FINAL_MONITOR_PERIOD_NS as u64
        );
        assert_eq!(
            second
                .final_monitor
                .timing
                .unwrap()
                .observed_evaluation_interval_ns,
            6_800_000_000
        );
        assert_eq!(second.final_monitor.timing.unwrap().prediction_age_ns, 0);
        assert!(second
            .surveillance
            .tracks
            .iter()
            .all(|track| track.newest_source_ns == second.surveillance.time_ns));

        let mut last_exact_scan_ns = next_scan_ns;
        for scan_index in 1..=3_i64 {
            last_exact_scan_ns += EXACT_FINAL_MONITOR_PERIOD_NS;
            let frame = regional_step(
                &mut runtime,
                &final_monitor_batch(
                    last_exact_scan_ns,
                    last_exact_scan_ns,
                    410_004_700,
                    -10_000,
                    false,
                ),
            )
            .unwrap();
            assert_eq!(frame.final_monitor.status, FinalMonitorStatus::Available);
            let timing = frame.final_monitor.timing.unwrap();
            assert_eq!(
                timing.observed_input_interval_ns,
                EXACT_FINAL_MONITOR_PERIOD_NS as u64
            );
            assert_eq!(timing.prediction_age_ns, 0);
            assert_eq!(
                last_exact_scan_ns - next_scan_ns,
                scan_index * EXACT_FINAL_MONITOR_PERIOD_NS
            );
        }

        let beyond_limit_ns = last_exact_scan_ns + OVER_FINAL_MONITOR_PERIOD_NS;
        let beyond_limit = regional_step(
            &mut runtime,
            &final_monitor_batch(beyond_limit_ns, beyond_limit_ns, 410_000_000, 0, false),
        )
        .unwrap();
        assert_eq!(
            beyond_limit.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::InputCadence)
        );
        let timing = beyond_limit.final_monitor.timing.unwrap();
        assert_eq!(
            timing.observed_input_interval_ns,
            OVER_FINAL_MONITOR_PERIOD_NS as u64
        );
        assert_eq!(timing.prediction_age_ns, 0);
        assert!(beyond_limit.final_monitor.alerts.is_empty());
    }

    // Mutation killed: turn a configured assignment with no corroborated flight into an empty,
    // apparently available monitor update when there are no unavailable-flight rows.
    // REQ: OA-RWY-015
    #[test]
    fn configured_runtime_degrades_when_assignment_has_no_corroborated_flight() {
        let mut configured = configured_runtime(100.0);
        let configured_frame = regional_step(
            &mut configured,
            &final_monitor_batch(1_000_000_000, 1_000_000_000, 410_000_000, 0, false),
        )
        .unwrap();
        assert!(configured_frame.flight_data.is_empty());
        assert_eq!(configured_frame.flight_data_unavailable, 0);
        assert_eq!(
            configured_frame.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::MissingAssignedTrack)
        );
        assert!(configured_frame.final_monitor.alerts.is_empty());

        let mut plain = runtime();
        let plain_frame = regional_step(
            &mut plain,
            &final_monitor_batch(1_000_000_000, 1_000_000_000, 410_000_000, 0, false),
        )
        .unwrap();
        assert_ne!(
            configured_frame.surveillance.frame_sha256, plain_frame.surveillance.frame_sha256,
            "the actual presenter must expose missing-assignment degradation"
        );
    }

    // REQ: OA-RWY-015
    #[test]
    fn configured_runtime_warning_reaches_frame_and_raster_consumer() {
        // Mutation killed: final-monitor output is not consumed by the actual scope presenter.
        let mut configured = configured_runtime(100.0);
        let frame = seed_confirmed_final_monitor_track(&mut configured, 410_000_000, 0);
        assert_eq!(frame.final_monitor.status, FinalMonitorStatus::Available);
        assert!(frame
            .final_monitor
            .alerts
            .iter()
            .any(|alert| { alert.severity == final_monitor_core::FinalSeverity::Warning }));
        assert!(!regional_pixels(&configured).unwrap().is_empty());

        let mut plain = runtime();
        let plain_frame = seed_confirmed_final_monitor_track(&mut plain, 410_000_000, 0);
        assert_ne!(
            frame.surveillance.frame_sha256,
            plain_frame.surveillance.frame_sha256
        );
    }

    // REQ: OA-RWY-015
    #[test]
    fn configured_runtime_withholds_stale_and_unusable_prediction_alerts() {
        // Mutation killed: stale data or unusable uncertainty leaves a previously rendered warning active.
        let mut configured = configured_runtime(100.0);
        let warning = seed_confirmed_final_monitor_track(&mut configured, 410_000_000, 0);
        assert!(warning
            .final_monitor
            .alerts
            .iter()
            .any(|alert| alert.severity == final_monitor_core::FinalSeverity::Warning));
        let target = warning
            .surveillance
            .tracks
            .iter()
            .find(|track| {
                warning
                    .final_monitor
                    .alerts
                    .iter()
                    .any(|alert| alert.address == track.address)
            })
            .expect("warning has a corresponding live track");
        let target_address = target.address;
        let last_source_ns = target.newest_source_ns;
        let stale_frame = regional_step(
            &mut configured,
            &ReceiptBatch {
                receipt_ns: 8_801_000_000,
                source_clock_regressions: 0,
                events: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(
            stale_frame.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::StalePredictionAndInputCadence)
        );
        assert!(stale_frame.final_monitor.alerts.is_empty());
        assert!(stale_frame
            .surveillance
            .tracks
            .iter()
            .any(|track| track.address == target_address
                && track.state_ns == stale_frame.surveillance.time_ns
                && track.newest_source_ns == last_source_ns));
        assert_ne!(
            warning.surveillance.frame_sha256, stale_frame.surveillance.frame_sha256,
            "the ordinary presenter replaces the prior warning with stale degraded status"
        );

        let mut wide = configured_runtime(100.0);
        let wide_warning = seed_confirmed_final_monitor_track(&mut wide, 410_000_000, 0);
        assert!(wide_warning
            .final_monitor
            .alerts
            .iter()
            .any(|alert| alert.severity == final_monitor_core::FinalSeverity::Warning));
        wide.final_monitor.as_mut().unwrap().pair.max_r95_m = 0.01;
        let wide_frame = regional_step(
            &mut wide,
            &final_monitor_batch(8_800_000_000, 8_800_000_000, 410_000_000, 0, false),
        )
        .unwrap();
        assert_eq!(
            wide_frame.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::UnusableUncertainty)
        );
        assert!(wide_frame.final_monitor.alerts.is_empty());
        assert_ne!(
            wide_warning.surveillance.frame_sha256, wide_frame.surveillance.frame_sha256,
            "the ordinary presenter removes the warning when prediction uncertainty is unusable"
        );
        let mut plain = runtime();
        let plain_frame = seed_confirmed_final_monitor_track(&mut plain, 410_000_000, 0);
        assert_ne!(
            wide_frame.surveillance.frame_sha256, plain_frame.surveillance.frame_sha256,
            "the configured runtime must render a degraded status through its real consumer"
        );
    }

    // REQ: OA-RWY-015, OA-RWY-016, OA-RWY-017
    #[test]
    fn configured_runtime_clears_stale_warning_and_recovers_on_fresh_source() {
        // Mutation killed: prediction staleness leaves a prior warning displayed or never recovers.
        let mut runtime = configured_runtime(100.0);
        let warning =
            seed_confirmed_final_monitor_track_from(&mut runtime, 410_000_000, 0, 1_200_000_000);
        assert!(warning
            .final_monitor
            .alerts
            .iter()
            .any(|alert| alert.severity == final_monitor_core::FinalSeverity::Warning));

        let near_limit = regional_step(
            &mut runtime,
            &final_monitor_batch(9_000_000_000, 4_201_000_000, 410_000_000, 0, false),
        )
        .unwrap();
        assert_eq!(
            near_limit.final_monitor.status,
            FinalMonitorStatus::Available
        );
        let near_limit_timing = near_limit.final_monitor.timing.unwrap();
        assert_eq!(
            near_limit_timing.observed_input_interval_ns,
            EXACT_FINAL_MONITOR_PERIOD_NS as u64
        );
        assert_eq!(near_limit_timing.prediction_age_ns, 4_799_000_000);

        let stale = regional_step(
            &mut runtime,
            &final_monitor_batch(14_000_000_000, 8_999_000_000, 410_000_000, 0, false),
        )
        .unwrap();
        assert_eq!(
            stale.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::StalePredictionAndInputCadence)
        );
        assert!(stale.final_monitor.alerts.is_empty());
        assert_eq!(
            stale
                .final_monitor
                .timing
                .unwrap()
                .observed_input_interval_ns,
            5_000_000_000,
            "the consumer reports the actual frame gap alongside stale source age"
        );
        assert_ne!(
            near_limit.surveillance.frame_sha256, stale.surveillance.frame_sha256,
            "the ordinary presenter removes the stale warning"
        );

        let fresh_but_not_due = regional_step(
            &mut runtime,
            &final_monitor_batch(15_000_000_000, 15_000_000_000, 410_000_000, 0, false),
        )
        .unwrap();
        assert_eq!(
            fresh_but_not_due.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::StalePredictionAndInputCadence),
            "a fresh frame does not clear a loss state before the next scheduled evaluation"
        );
        assert!(fresh_but_not_due.final_monitor.alerts.is_empty());

        let recovered = regional_step(
            &mut runtime,
            &final_monitor_batch(18_800_000_000, 18_800_000_000, 410_000_000, 0, false),
        )
        .unwrap();
        assert_eq!(
            recovered.final_monitor.status,
            FinalMonitorStatus::Available
        );
        assert_eq!(recovered.final_monitor.timing.unwrap().prediction_age_ns, 0);
        assert!(recovered
            .final_monitor
            .alerts
            .iter()
            .any(|alert| alert.severity == final_monitor_core::FinalSeverity::Warning));
    }

    // Mutation killed: positioned final-monitor circles accumulate on each operator redraw.
    #[test]
    fn operator_redraws_reuse_positioned_warning_overlay() {
        let mut monitored = configured_runtime(100.0);
        let active = seed_confirmed_final_monitor_track(&mut monitored, 410_000_000, 0);
        let mut plain = runtime();
        seed_confirmed_final_monitor_track(&mut plain, 410_000_000, 0);
        assert!(active
            .final_monitor
            .alerts
            .iter()
            .any(|alert| alert.severity == final_monitor_core::FinalSeverity::Warning));
        let active_counts =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        assert!(
            active_counts.circles > 0,
            "warning marker is positioned on the raster"
        );

        let console = "operator status";
        regional_console_overlay(&mut monitored, console).expect("warning console redraw");
        regional_console_overlay(&mut plain, console).expect("plain console redraw");
        let after_console =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        let expected_console_bytes = active_counts
            .text_bytes
            .checked_add(console.len())
            .and_then(|bytes| bytes.checked_add(3))
            .expect("bounded console suffix");
        assert_eq!(after_console.text_bytes, expected_console_bytes);
        regional_console_overlay(&mut monitored, console).expect("repeat warning console redraw");
        regional_console_overlay(&mut plain, console).expect("repeat plain console redraw");
        assert_eq!(
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance),
            after_console,
            "repeating the same console replaces all overlay primitives and bytes"
        );

        let monitored_target = monitored
            .views
            .first()
            .copied()
            .expect("monitored track view");
        let plain_target = plain.views.first().copied().expect("plain track view");
        let monitored_identity =
            (monitored_target.identity_present == 1).then_some(monitored_target.identity);
        let plain_identity = (plain_target.identity_present == 1).then_some(plain_target.identity);
        regional_select(
            &mut monitored,
            monitored_target.address,
            monitored_target.generation,
            monitored_identity,
        )
        .expect("select the rendered warning track");
        regional_select(
            &mut plain,
            plain_target.address,
            plain_target.generation,
            plain_identity,
        )
        .expect("select the equivalent plain track");
        let after_selection =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        regional_select(
            &mut monitored,
            monitored_target.address,
            monitored_target.generation,
            monitored_identity,
        )
        .expect("repeat warning track selection");
        regional_select(
            &mut plain,
            plain_target.address,
            plain_target.generation,
            plain_identity,
        )
        .expect("repeat plain track selection");
        assert_eq!(
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance),
            after_selection,
            "repeating selection does not accumulate circles, lines, or text"
        );
        regional_clear_selection(&mut monitored).expect("clear warning selection");
        regional_clear_selection(&mut plain).expect("clear plain selection");
        let after_clear =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        regional_clear_selection(&mut monitored).expect("repeat clear warning selection");
        regional_clear_selection(&mut plain).expect("repeat clear plain selection");
        assert_eq!(
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance),
            after_clear,
            "repeating clear-selection does not accumulate overlay primitives or bytes"
        );
    }

    // Mutation killed: the runtime's widest console/status rows clip or overlap alert and safety rows.
    #[test]
    fn runtime_status_layout_handles_maximum_console_before_and_after_loss() {
        let mut monitored = configured_runtime(100.0);
        let synthetic_msaw = crate::msaw_bridge::regional_msaw_synthetic_for_profile(
            31,
            monitored.profile.surveillance.max_frames,
        )
        .expect("synthetic service reserves the 31 rows left beside the final assignment");
        regional_attach_msaw(&mut monitored, synthetic_msaw)
            .expect("composed synthetic MSAW capacity is admitted");
        let warning = seed_confirmed_final_monitor_track(&mut monitored, 410_000_000, 0);
        assert_eq!(warning.final_monitor.status, FinalMonitorStatus::Available);
        assert!(warning
            .final_monitor
            .alerts
            .iter()
            .any(|alert| alert.severity == final_monitor_core::FinalSeverity::Warning));
        // Distinct full rows make every expected operator row independently identifiable.
        let console = format!("{}{}", "A".repeat(64), "B".repeat(64));
        regional_console_overlay(&mut monitored, &console).expect("pre-loss console state");
        let warning_safety_rows = usize::try_from(
            warning
                .surveillance
                .safety_stats
                .expect("warning frame safety summary")
                .banner_lines,
        )
        .expect("bounded safety row count");
        assert_runtime_status_layout(&monitored, &console, false, warning_safety_rows);
        let warning_counts =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        assert!(
            warning_counts.circles > 0,
            "warning has a positioned runtime marker"
        );

        // The seeded data ends at receipt/source 4 s; this cutoff is 9 s with no newer source.
        let stale = regional_step(
            &mut monitored,
            &final_monitor_batch(9_000_000_000, 4_000_000_000, 410_000_000, 0, false),
        )
        .expect("synthetic stale frame is admitted");
        assert_eq!(
            stale.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::StalePredictionAndInputCadence)
        );
        let timing = stale.final_monitor.timing.expect("stale evaluation timing");
        assert_eq!(timing.observed_input_interval_ns, 5_000_000_000);
        assert_eq!(timing.prediction_age_ns, 5_000_000_000);
        let stale_safety_rows = usize::try_from(
            stale
                .surveillance
                .safety_stats
                .expect("stale frame safety summary")
                .banner_lines,
        )
        .expect("bounded safety row count");

        regional_console_overlay(&mut monitored, &console).expect("loss console redraw");
        assert_runtime_status_layout(&monitored, &console, true, stale_safety_rows);
        let stale_counts =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        regional_console_overlay(&mut monitored, &console).expect("repeat loss console redraw");
        assert_eq!(
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance),
            stale_counts,
            "repeating protection-loss console replaces every primitive and byte"
        );

        let monitored_target = monitored
            .views
            .first()
            .copied()
            .expect("stale monitored view");
        regional_select(
            &mut monitored,
            monitored_target.address,
            monitored_target.generation,
            (monitored_target.identity_present == 1).then_some(monitored_target.identity),
        )
        .expect("select stale monitored track");
        assert_runtime_status_layout(&monitored, &console, true, stale_safety_rows);
        let after_loss_selection =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        regional_select(
            &mut monitored,
            monitored_target.address,
            monitored_target.generation,
            (monitored_target.identity_present == 1).then_some(monitored_target.identity),
        )
        .expect("repeat stale monitored selection");
        assert_eq!(
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance),
            after_loss_selection,
            "repeating protection-loss selection replaces the prior marker set"
        );
        assert_runtime_status_layout(&monitored, &console, true, stale_safety_rows);

        regional_clear_selection(&mut monitored).expect("monitored clear redraw");
        assert_runtime_status_layout(&monitored, &console, true, stale_safety_rows);
        let after_loss_clear =
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance);
        regional_clear_selection(&mut monitored).expect("repeat monitored clear redraw");
        assert_eq!(
            surveillance::surveillance_overlay_primitive_counts(&monitored.surveillance),
            after_loss_clear,
            "repeating protection-loss clear redraw keeps primitive counts stable"
        );
        assert_runtime_status_layout(&monitored, &console, true, stale_safety_rows);
    }
}

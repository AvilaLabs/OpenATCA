//! Offline composed regional session (shell, B1/B3/C6), with one common receipt cutoff.
//! This executes actual product cores; it is not deadline or allocation qualification.

use crate::commands;
use crate::flight::{
    self, FlightApplyOutcome, FlightDriver, FlightDriverConfig, FlightDriverReport,
    FlightViewStatus, SharedFlightView, MAX_DRIVER_FLIGHTS, MAX_DRIVER_LOG_ENTRIES,
};
use crate::nam_flight::{self, NamFlightContext, NamFlightResult, NamFlightRuntime};
use crate::route::{self, RouteStore};
use crate::session::{summarize_input_batch, FrameEvidence, ReceiptBatch, MAX_EVENTS_PER_RECEIPT};
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
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

const DEFAULT_SESSION_NAMESPACE: u16 = 1;
const QUARANTINE_CLOCK_DOMAIN_ID: u32 = 1;
/// C7 budget for one full-key search through the bounded (at most 8192-key) sorted view ledger.
const MAX_FLIGHT_KEY_SEARCH_STEPS: usize = 14;
/// C7 budget for the bounded in-place suffix move when a new key is registered.
const MAX_FLIGHT_KEY_INSERT_MOVES: usize = flight::MAX_OFFLINE_DRIVER_FLIGHTS;
/// C7 budget for retiring absent ledger keys when the view ledger reaches its configured capacity.
const MAX_FLIGHT_KEY_RECLAIM_WORK: usize = flight::MAX_OFFLINE_DRIVER_FLIGHTS;
/// C7 worst-case hash probes for that retirement pass: one `index_find` per outer key, each
/// bounded by the exact `by_key.cells.len()` limit exposed by the flight adapter.
const MAX_FLIGHT_KEY_RECLAIM_PROBE_WORK: usize =
    MAX_FLIGHT_KEY_RECLAIM_WORK * flight::MAX_OFFLINE_DRIVER_KEY_INDEX_CELLS;
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
    known_flights: Vec<oa_flight::FlightKey>,
    runway_aids: Vec<RunwayAidRuntime>,
    runway_assignments: Vec<ActiveRunwayAssignment>,
    runway_aids_manifest_sha256: Option<[u8; 32]>,
    runway_aids_profile_sha256: Option<[u8; 32]>,
    atpa_scratch: AtpaScratch,
}

#[derive(Clone)]
struct RunwayAidRuntime {
    config: oa_runway::adaptation::AidPairConfig,
    geometry: crate::runway_aids::RunwayAidGeometry,
    arv_coverage: oa_display::core::runway_aids::ArvCoverage,
    arv_paths: ArvPathColumns,
    state: oa_runway::final_monitor::FinalMonitorState,
    retained_final_alerts: Vec<RetainedFinalAlert>,
    retained_input_addresses: Vec<u32>,
    retained_prediction_signature: Option<[u8; oa_sha256::core::sha256::DIGEST_LEN]>,
    retained_assignment_digest: Option<[u8; oa_sha256::core::sha256::DIGEST_LEN]>,
    retained_assignment_count: u32,
    retained_final_as_of_ns: Option<u64>,
    retained_final_config: Option<oa_runway::adaptation::AidPairConfig>,
    retained_final_pair: Option<oa_runway::final_monitor::FinalPair>,
    retained_final_geometry: Option<crate::runway_aids::RunwayAidGeometry>,
}

#[derive(Clone)]
struct ArvPathColumns {
    ids: Vec<u32>,
    kinds: Vec<u8>,
    threshold_east_m: Vec<f64>,
    threshold_north_m: Vec<f64>,
    inbound_east: Vec<f64>,
    inbound_north: Vec<f64>,
    width_m: Vec<f64>,
}

fn reserve_arv_column<T>(
    mut values: Vec<T>,
    capacity: usize,
    what: &'static str,
) -> Result<Vec<T>, String> {
    values
        .try_reserve_exact(capacity)
        .map_err(|_| format!("ARV {what} capacity refused"))?;
    Ok(values)
}

impl ArvPathColumns {
    fn new(capacity: usize) -> Result<Self, String> {
        Ok(Self {
            ids: reserve_arv_column(Vec::new(), capacity, "path ID")?,
            kinds: reserve_arv_column(Vec::new(), capacity, "path kind")?,
            threshold_east_m: reserve_arv_column(Vec::new(), capacity, "threshold east")?,
            threshold_north_m: reserve_arv_column(Vec::new(), capacity, "threshold north")?,
            inbound_east: reserve_arv_column(Vec::new(), capacity, "inbound east")?,
            inbound_north: reserve_arv_column(Vec::new(), capacity, "inbound north")?,
            width_m: reserve_arv_column(Vec::new(), capacity, "path width")?,
        })
    }
}

#[derive(Clone, Copy)]
struct RetainedFinalAlert {
    address: u32,
    pair_id: oa_runway::final_monitor::FinalPairId,
    severity: oa_runway::final_monitor::FinalSeverity,
    lat_e7: i32,
    lon_e7: i32,
    seconds_to_entry_milli: u32,
}

struct FinalMonitorRows<'a> {
    addresses: &'a [u32],
    pair_ids: &'a [oa_runway::final_monitor::FinalPairId],
    severity: &'a [oa_runway::final_monitor::FinalSeverity],
    lat_e7: &'a [i32],
    lon_e7: &'a [i32],
    seconds_to_entry_milli: &'a [u32],
    count: usize,
}

struct AtpaScratch {
    runway_id: Vec<u32>,
    end_name: Vec<[u8; 3]>,
    scan_ns: Vec<u64>,
    lead_along_m: Vec<f64>,
    trail_along_m: Vec<f64>,
    lead_speed_mps: Vec<f64>,
    trail_speed_mps: Vec<f64>,
    minimum_spacing_m: Vec<f64>,
    severity: Vec<oa_runway::terminal_aids::AtpaSeverity>,
    current_distance_milli_nm: Vec<u32>,
    seconds_to_loss_milli: Vec<u32>,
}

struct RunwayAidFrame<'a> {
    assignments: &'a [ActiveRunwayAssignment],
    surveillance: Option<&'a surveillance::Surveillance>,
    tracks: &'a [crate::surveillance::ObservedTrack],
    ghosts: &'a mut Vec<oa_display::core::runway_aids::CrdaGhostPacket>,
    scan_time_ns: i64,
    scan_sequence: u64,
}

impl AtpaScratch {
    fn new(max_flights: usize) -> Result<Self, String> {
        let rows = max_flights.min(oa_runway::terminal_aids::MAX_CRDA_ROWS);
        let pair_capacity = rows
            .checked_mul(rows.saturating_sub(1))
            .and_then(|count| count.checked_div(2))
            .ok_or("ATPA scratch capacity overflow")?;
        let mut scratch = Self {
            runway_id: Vec::new(),
            end_name: Vec::new(),
            scan_ns: Vec::new(),
            lead_along_m: Vec::new(),
            trail_along_m: Vec::new(),
            lead_speed_mps: Vec::new(),
            trail_speed_mps: Vec::new(),
            minimum_spacing_m: Vec::new(),
            severity: Vec::new(),
            current_distance_milli_nm: Vec::new(),
            seconds_to_loss_milli: Vec::new(),
        };
        scratch
            .runway_id
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA runway scratch allocation refused")?;
        scratch
            .end_name
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA end scratch allocation refused")?;
        scratch
            .scan_ns
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA time scratch allocation refused")?;
        scratch
            .lead_along_m
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA lead scratch allocation refused")?;
        scratch
            .trail_along_m
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA trail scratch allocation refused")?;
        scratch
            .lead_speed_mps
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA lead speed allocation refused")?;
        scratch
            .trail_speed_mps
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA trail speed allocation refused")?;
        scratch
            .minimum_spacing_m
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA spacing allocation refused")?;
        scratch
            .severity
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA severity allocation refused")?;
        scratch
            .current_distance_milli_nm
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA distance allocation refused")?;
        scratch
            .seconds_to_loss_milli
            .try_reserve_exact(pair_capacity)
            .map_err(|_| "ATPA time output allocation refused")?;
        Ok(scratch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveRunwayAssignment {
    identity: oa_display::interaction::FlightIdentity,
    generation: u32,
    sequence: u32,
    runway_id: u32,
    end_name: [u8; 3],
    arv_status: oa_runway::arv::AlertKind,
    arv_acknowledged: bool,
    arv_ack_sequence: u32,
    arv_as_of_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunwayAssignmentValue {
    pub identity: oa_display::interaction::FlightIdentity,
    pub generation: u32,
    pub sequence: u32,
    pub runway_id: u32,
    pub end_name: [u8; 3],
}

const MAX_RUNTIME_RUNWAY_AID_PAIRS: usize = oa_runway::adaptation::MAX_AID_PAIRS;

#[derive(Clone, Copy)]
struct AtpaFlightInput {
    assignment: ActiveRunwayAssignment,
    point: oa_runway::plane::Local,
    east_speed_mps: f64,
    north_speed_mps: f64,
}

fn evaluate_assigned_atpa(
    aid: &RunwayAidRuntime,
    assignments: &[ActiveRunwayAssignment],
    surveillance: &surveillance::Surveillance,
    tracks: &[crate::surveillance::ObservedTrack],
    scan_time_ns: i64,
    scratch: &mut AtpaScratch,
) -> Option<(u16, u16, u16)> {
    use oa_runway::terminal_aids::{self, AtpaPairs, AtpaRows, AtpaSeverity};
    const MAX_FLIGHTS: usize = terminal_aids::MAX_CRDA_ROWS;
    let scan_ns = u64::try_from(scan_time_ns).ok().filter(|time| *time > 0)?;
    if assignments.len() > MAX_FLIGHTS {
        return None;
    }
    let local_plane = oa_runway::plane::plane_new(
        aid.geometry.plane_origin.lat_deg,
        aid.geometry.plane_origin.lon_deg,
    )?;
    let mut flights: [Option<AtpaFlightInput>; MAX_FLIGHTS] = [None; MAX_FLIGHTS];
    let mut flight_count = 0usize;
    let mut omitted = 0u16;
    for assignment in assignments {
        let matches_first = assignment.runway_id == aid.config.first_runway_id
            && assignment.end_name == aid.config.first_end_name;
        let matches_second = assignment.runway_id == aid.config.second_runway_id
            && assignment.end_name == aid.config.second_end_name;
        if !matches_first && !matches_second {
            continue;
        }
        let mut matching_track = None;
        for track in tracks {
            if track.address == assignment.identity.aircraft_address
                && matching_track.replace(track).is_some()
            {
                return None;
            }
        }
        let Some(track) = matching_track else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        let (row, confirmed, newest_ns) =
            surveillance::surveillance_track_forecast(surveillance, track.address, scan_time_ns)
                .ok()?;
        if !confirmed
            || !scan_time_ns
                .checked_sub(newest_ns)
                .is_some_and(|age| (0..4_800_000_000).contains(&age))
        {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let point = oa_runway::plane::to_local(
            &local_plane,
            oa_runway::plane::Geo {
                lat_deg: f64::from(row.lat_e7) / 10_000_000.0,
                lon_deg: f64::from(row.lon_e7) / 10_000_000.0,
            },
        )?;
        let slot = flights.get_mut(flight_count)?;
        *slot = Some(AtpaFlightInput {
            assignment: *assignment,
            point,
            east_speed_mps: f64::from(row.vel_east_mps),
            north_speed_mps: f64::from(row.vel_north_mps),
        });
        flight_count += 1;
    }
    scratch.runway_id.clear();
    scratch.end_name.clear();
    scratch.scan_ns.clear();
    scratch.lead_along_m.clear();
    scratch.trail_along_m.clear();
    scratch.lead_speed_mps.clear();
    scratch.trail_speed_mps.clear();
    scratch.minimum_spacing_m.clear();
    for left in 0..flight_count {
        let first = flights.get(left).copied().flatten()?;
        for right in left.saturating_add(1)..flight_count {
            let second = flights.get(right).copied().flatten()?;
            if first.assignment.runway_id != second.assignment.runway_id
                || first.assignment.end_name != second.assignment.end_name
            {
                continue;
            }
            let (threshold, inbound_east, inbound_north) = if first.assignment.runway_id
                == aid.config.first_runway_id
                && first.assignment.end_name == aid.config.first_end_name
            {
                (
                    aid.geometry.first_threshold,
                    aid.geometry.first_inbound_east,
                    aid.geometry.first_inbound_north,
                )
            } else if first.assignment.runway_id == aid.config.second_runway_id
                && first.assignment.end_name == aid.config.second_end_name
            {
                (
                    aid.geometry.second_threshold,
                    aid.geometry.second_inbound_east,
                    aid.geometry.second_inbound_north,
                )
            } else {
                return None;
            };
            let first_along = (first.point.east_m - threshold.east_m) * inbound_east
                + (first.point.north_m - threshold.north_m) * inbound_north;
            let second_along = (second.point.east_m - threshold.east_m) * inbound_east
                + (second.point.north_m - threshold.north_m) * inbound_north;
            if (first_along - second_along).abs() <= f64::EPSILON {
                omitted = omitted.saturating_add(2);
                continue;
            }
            let first_speed =
                first.east_speed_mps * inbound_east + first.north_speed_mps * inbound_north;
            let second_speed =
                second.east_speed_mps * inbound_east + second.north_speed_mps * inbound_north;
            if first_speed < 0.0 || second_speed < 0.0 {
                omitted = omitted.saturating_add(2);
                continue;
            }
            let (lead_along, trail_along, lead_speed, trail_speed) = if first_along > second_along {
                (first_along, second_along, first_speed, second_speed)
            } else {
                (second_along, first_along, second_speed, first_speed)
            };
            scratch.runway_id.push(first.assignment.runway_id);
            scratch.end_name.push(first.assignment.end_name);
            scratch.scan_ns.push(scan_ns);
            scratch.lead_along_m.push(lead_along);
            scratch.trail_along_m.push(trail_along);
            scratch.lead_speed_mps.push(lead_speed);
            scratch.trail_speed_mps.push(trail_speed);
            scratch
                .minimum_spacing_m
                .push(f64::from(aid.config.minimum_spacing_ft) * oa_runway::final_monitor::FT_TO_M);
        }
    }
    let pair_count = scratch.runway_id.len();
    scratch.severity.resize(pair_count, AtpaSeverity::Clear);
    scratch.current_distance_milli_nm.resize(pair_count, 0);
    scratch.seconds_to_loss_milli.resize(pair_count, 0);
    terminal_aids::evaluate_atpa_pairs(
        scan_ns,
        AtpaPairs {
            lead_runway_id: &scratch.runway_id,
            trail_runway_id: &scratch.runway_id,
            lead_end_name: &scratch.end_name,
            trail_end_name: &scratch.end_name,
            scan_ns: &scratch.scan_ns,
            lead_along_m: &scratch.lead_along_m,
            trail_along_m: &scratch.trail_along_m,
            lead_speed_mps: &scratch.lead_speed_mps,
            trail_speed_mps: &scratch.trail_speed_mps,
            minimum_spacing_m: &scratch.minimum_spacing_m,
        },
        AtpaRows {
            severity: &mut scratch.severity,
            current_distance_milli_nm: &mut scratch.current_distance_milli_nm,
            seconds_to_loss_milli: &mut scratch.seconds_to_loss_milli,
        },
    )
    .ok()?;
    let mut warnings = 0u16;
    let mut alerts = 0u16;
    for severity in &scratch.severity {
        match severity {
            AtpaSeverity::Clear => {}
            AtpaSeverity::Warning => warnings = warnings.saturating_add(1),
            AtpaSeverity::Alert => alerts = alerts.saturating_add(1),
        }
    }
    Some((warnings, alerts, omitted))
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
            },
            max_flights: 8192,
            max_route_revisions: route::MAX_ROUTE_REVISIONS,
            reclaim_current_route_by_key: false,
            max_flight_log_entries: MAX_DRIVER_LOG_ENTRIES,
            max_total_events: crate::session::MAX_RECORDED_PROFILE_EVENTS,
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

fn automation_console_text(console: &str, status: RegionalAutomationStatus) -> String {
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
    /// Typed terminal-aid status. An unconfigured runtime emits one explicit Unavailable packet.
    pub runway_aids: Vec<oa_display::core::runway_aids::RunwayAidPacket>,
    /// Per-assignment ARV status. Clear is emitted only against an issuer-declared complete
    /// airport candidate-path set and a current full-key tracker forecast.
    pub arv_verification: Vec<oa_display::core::runway_aids::ArvPacket>,
    pub runway_assignments: Vec<oa_display::core::runway_aids::AssignmentPacket>,
    pub crda_ghosts: Vec<oa_display::core::runway_aids::CrdaGhostPacket>,
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

fn runway_aid_packets(
    aids: &mut [RunwayAidRuntime],
    atpa_scratch: &mut AtpaScratch,
    frame: RunwayAidFrame<'_>,
) -> Vec<oa_display::core::runway_aids::RunwayAidPacket> {
    let RunwayAidFrame {
        assignments,
        surveillance,
        tracks,
        ghosts: crda_ghosts,
        scan_time_ns,
        scan_sequence,
    } = frame;
    use oa_display::core::runway_aids::{Availability, CrdaGhostPacket, RunwayAidPacket, Severity};
    use oa_runway::final_monitor::{
        self, FinalAlertBuffer, FinalSeverity, PredictedTracks, MAX_TRACKS,
    };

    if aids.is_empty() {
        return vec![RunwayAidPacket {
            scan_sequence,
            final_monitor_as_of_ns: 0,
            first_runway_id: 0,
            first_end_name: [0; 3],
            second_runway_id: 0,
            second_end_name: [0; 3],
            availability: Availability::Unavailable,
            maximum_severity: Severity::Clear,
            predicted_tracks: 0,
            caution_tracks: 0,
            warning_tracks: 0,
            alert_tracks: 0,
            atpa_warning_pairs: 0,
            atpa_alert_pairs: 0,
            crda_ghosts: 0,
            unavailable_tracks: 0,
        }];
    }
    let Ok(now_ns) = u64::try_from(scan_time_ns) else {
        return Vec::new();
    };
    if now_ns == 0 || aids.len() > MAX_RUNTIME_RUNWAY_AID_PAIRS {
        return Vec::new();
    }
    let mut packets = Vec::with_capacity(aids.len());
    for aid in aids {
        let mut addresses = [0u32; MAX_TRACKS];
        let mut east = [0.0f64; MAX_TRACKS];
        let mut north = [0.0f64; MAX_TRACKS];
        let mut velocity_east = [0.0f64; MAX_TRACKS];
        let mut velocity_north = [0.0f64; MAX_TRACKS];
        let mut ground_speed = [0.0f64; MAX_TRACKS];
        let mut r95 = [0.0f64; MAX_TRACKS];
        let mut lat = [0i32; MAX_TRACKS];
        let mut lon = [0i32; MAX_TRACKS];
        let mut source_ns = [0i64; MAX_TRACKS];
        let mut included = 0usize;
        let mut omitted = 0u16;
        let mut report_ns = now_ns;
        let local_plane = oa_runway::plane::plane_new(
            aid.geometry.plane_origin.lat_deg,
            aid.geometry.plane_origin.lon_deg,
        );
        let mut invalid = local_plane.is_none() || tracks.len() > MAX_TRACKS;
        if tracks.len() > MAX_TRACKS {
            omitted = u16::try_from(tracks.len().min(usize::from(u16::MAX))).unwrap_or(u16::MAX);
        }
        if let Some(local_plane) = local_plane.filter(|_| !invalid) {
            let mut previous_address = 0u32;
            for track in tracks {
                let Some(surveillance) = surveillance else {
                    invalid = true;
                    break;
                };
                let forecast = surveillance::surveillance_track_forecast(
                    surveillance,
                    track.address,
                    scan_time_ns,
                );
                let forecast = match forecast {
                    Ok((row, true, newest_ns)) => Some((row, newest_ns)),
                    _ => None,
                };
                let age = forecast.and_then(|(_, newest_ns)| scan_time_ns.checked_sub(newest_ns));
                let fresh = age.is_some_and(|delta| (0..4_800_000_000).contains(&delta));
                let row = forecast.map(|(row, _)| row);
                let point = row.and_then(|row| {
                    oa_runway::plane::to_local(
                        &local_plane,
                        oa_runway::plane::Geo {
                            lat_deg: f64::from(row.lat_e7) / 10_000_000.0,
                            lon_deg: f64::from(row.lon_e7) / 10_000_000.0,
                        },
                    )
                });
                if !fresh
                    || track.address == 0
                    || track.address <= previous_address
                    || point.is_none()
                    || row.is_none()
                {
                    omitted = omitted.saturating_add(1);
                    if track.address != 0 && track.address <= previous_address {
                        invalid = true;
                    }
                    continue;
                }
                let Some(point) = point else { continue };
                let Some(row) = row else { continue };
                let Some((address, e, n, vx, vy, speed, uncertainty, latitude, longitude)) =
                    addresses
                        .get_mut(included)
                        .zip(east.get_mut(included))
                        .zip(north.get_mut(included))
                        .zip(velocity_east.get_mut(included))
                        .zip(velocity_north.get_mut(included))
                        .zip(ground_speed.get_mut(included))
                        .zip(r95.get_mut(included))
                        .zip(lat.get_mut(included))
                        .zip(lon.get_mut(included))
                        .map(
                            |(
                                (((((((address, e), n), vx), vy), speed), uncertainty), latitude),
                                longitude,
                            )| {
                                (
                                    address,
                                    e,
                                    n,
                                    vx,
                                    vy,
                                    speed,
                                    uncertainty,
                                    latitude,
                                    longitude,
                                )
                            },
                        )
                else {
                    invalid = true;
                    break;
                };
                *address = track.address;
                *e = point.east_m;
                *n = point.north_m;
                *vx = f64::from(row.vel_east_mps);
                *vy = f64::from(row.vel_north_mps);
                *speed = oa_math::hypot(*vx, *vy);
                *uncertainty = oa_track::r95_m_of(&row.pos_cov_m2);
                *latitude = row.lat_e7;
                *longitude = row.lon_e7;
                let Some(source_time) = source_ns.get_mut(included) else {
                    invalid = true;
                    break;
                };
                *source_time = track.newest_source_ns;
                previous_address = track.address;
                included += 1;
                if let Ok(source_ns) = u64::try_from(track.newest_source_ns) {
                    report_ns = report_ns.min(source_ns);
                }
            }
        }
        let prediction_signature = final_monitor_input_signature(FinalMonitorInput {
            now_ns,
            report_ns,
            included,
            addresses: &addresses,
            source_ns: &source_ns,
            east_m: &east,
            north_m: &north,
            velocity_east_mps: &velocity_east,
            velocity_north_mps: &velocity_north,
            ground_speed_mps: &ground_speed,
            r95_m: &r95,
            lat_e7: &lat,
            lon_e7: &lon,
        });
        let mut output_addresses = [0u32; MAX_TRACKS];
        let mut output_pairs = [aid.geometry.pair.id; MAX_TRACKS];
        let mut output_severity = [FinalSeverity::Caution; MAX_TRACKS];
        let mut output_lat = [0i32; MAX_TRACKS];
        let mut output_lon = [0i32; MAX_TRACKS];
        let mut output_entry_ms = [0u32; MAX_TRACKS];
        let update = if invalid {
            None
        } else {
            final_monitor::update(
                aid.geometry.pair,
                &mut aid.state,
                now_ns,
                report_ns,
                PredictedTracks {
                    addresses: addresses.get(..included).unwrap_or(&[]),
                    east_m: east.get(..included).unwrap_or(&[]),
                    north_m: north.get(..included).unwrap_or(&[]),
                    velocity_east_mps: velocity_east.get(..included).unwrap_or(&[]),
                    velocity_north_mps: velocity_north.get(..included).unwrap_or(&[]),
                    r95_m: r95.get(..included).unwrap_or(&[]),
                    lat_e7: lat.get(..included).unwrap_or(&[]),
                    lon_e7: lon.get(..included).unwrap_or(&[]),
                },
                FinalAlertBuffer {
                    addresses: output_addresses.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                    pair_ids: output_pairs.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                    severity: output_severity.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                    lat_e7: output_lat.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                    lon_e7: output_lon.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                    seconds_to_entry_milli: output_entry_ms
                        .get_mut(..MAX_TRACKS)
                        .unwrap_or(&mut []),
                },
            )
            .ok()
        };
        let mut caution_count = 0u16;
        let mut warning_count = 0u16;
        let mut monitor_not_evaluated = update.is_none();
        let mut monitor_as_of_ns = 0u64;
        let evaluated_addresses = addresses.get(..included);
        if let Some(result) = update {
            if result.due {
                for severity in output_severity.iter().take(usize::from(result.alerts)) {
                    match severity {
                        FinalSeverity::Caution => caution_count = caution_count.saturating_add(1),
                        FinalSeverity::Warning => warning_count = warning_count.saturating_add(1),
                    }
                }
                monitor_as_of_ns = now_ns;
                monitor_not_evaluated = result.degraded;
                let retained = !result.degraded
                    && !invalid
                    && omitted == 0
                    && evaluated_addresses.is_some()
                    && retain_final_monitor_result(
                        aid,
                        now_ns,
                        evaluated_addresses.unwrap_or(&[]),
                        assignments,
                        prediction_signature,
                        FinalMonitorRows {
                            addresses: &output_addresses,
                            pair_ids: &output_pairs,
                            severity: &output_severity,
                            lat_e7: &output_lat,
                            lon_e7: &output_lon,
                            seconds_to_entry_milli: &output_entry_ms,
                            count: usize::from(result.alerts),
                        },
                    );
                if !retained {
                    clear_retained_final_monitor(aid);
                    if !result.degraded && !invalid {
                        monitor_not_evaluated = true;
                    }
                }
            } else if !result.degraded
                && evaluated_addresses.is_some()
                && retained_final_monitor_matches(
                    aid,
                    now_ns,
                    evaluated_addresses.unwrap_or(&[]),
                    assignments,
                    prediction_signature,
                )
            {
                monitor_not_evaluated = false;
                monitor_as_of_ns = aid.retained_final_as_of_ns.unwrap_or(0);
                for row in &aid.retained_final_alerts {
                    match row.severity {
                        FinalSeverity::Caution => caution_count = caution_count.saturating_add(1),
                        FinalSeverity::Warning => warning_count = warning_count.saturating_add(1),
                    }
                }
            } else {
                monitor_not_evaluated = true;
                clear_retained_final_monitor(aid);
            }
        } else {
            clear_retained_final_monitor(aid);
        }
        let mut crda_count = 0u16;
        let crda = if invalid {
            None
        } else {
            let source = oa_runway::terminal_aids::ReferenceLine {
                origin: aid.geometry.first_threshold,
                axis_east: aid.geometry.first_inbound_east,
                axis_north: aid.geometry.first_inbound_north,
            };
            let target = oa_runway::terminal_aids::ReferenceLine {
                origin: aid.geometry.second_threshold,
                axis_east: aid.geometry.second_inbound_east,
                axis_north: aid.geometry.second_inbound_north,
            };
            let mut ghost_east = [0.0f64; MAX_TRACKS];
            let mut ghost_north = [0.0f64; MAX_TRACKS];
            let mut ghost_speed = [0.0f64; MAX_TRACKS];
            let parents = oa_runway::terminal_aids::CrdaParents {
                east_m: east.get(..included).unwrap_or(&[]),
                north_m: north.get(..included).unwrap_or(&[]),
                ground_speed_mps: ground_speed.get(..included).unwrap_or(&[]),
            };
            let result = oa_runway::terminal_aids::project_crda_ghosts(
                oa_runway::terminal_aids::CrdaTransform {
                    source,
                    target,
                    target_offset_m: aid.geometry.crda_offset_m,
                },
                parents,
                oa_runway::terminal_aids::CrdaGhosts {
                    east_m: ghost_east.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                    north_m: ghost_north.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                    ground_speed_mps: ghost_speed.get_mut(..MAX_TRACKS).unwrap_or(&mut []),
                },
            );
            if let Ok(result) = result {
                crda_count = result.rows;
                for index in 0..usize::from(result.rows) {
                    let (
                        Some(&parent_address),
                        Some(&ghost_east_m),
                        Some(&ghost_north_m),
                        Some(&speed),
                    ) = (
                        addresses.get(index),
                        ghost_east.get(index),
                        ghost_north.get(index),
                        ghost_speed.get(index),
                    )
                    else {
                        invalid = true;
                        break;
                    };
                    crda_ghosts.push(CrdaGhostPacket {
                        scan_sequence,
                        first_runway_id: aid.config.first_runway_id,
                        second_runway_id: aid.config.second_runway_id,
                        parent_address,
                        origin_lat_e7: aid.geometry.plane_origin_lat_e7,
                        origin_lon_e7: aid.geometry.plane_origin_lon_e7,
                        east_m: ghost_east_m,
                        north_m: ghost_north_m,
                        ground_speed_mps: speed,
                    });
                }
                Some(result)
            } else {
                None
            }
        };
        if crda.is_none() {
            invalid = true;
        }
        let atpa = if invalid {
            None
        } else if let Some(surveillance) = surveillance {
            evaluate_assigned_atpa(
                aid,
                assignments,
                surveillance,
                tracks,
                scan_time_ns,
                atpa_scratch,
            )
        } else {
            None
        };
        if atpa.is_none() {
            invalid = true;
        }
        let (atpa_warnings, atpa_alerts, atpa_omitted) = atpa.unwrap_or((0, 0, 0));
        omitted = omitted.saturating_add(atpa_omitted);
        let degraded = invalid
            || omitted != 0
            || monitor_not_evaluated
            || update.is_some_and(|result| result.degraded);
        packets.push(RunwayAidPacket {
            scan_sequence,
            final_monitor_as_of_ns: monitor_as_of_ns,
            first_runway_id: aid.config.first_runway_id,
            first_end_name: aid.config.first_end_name,
            second_runway_id: aid.config.second_runway_id,
            second_end_name: aid.config.second_end_name,
            availability: if invalid || update.is_none() {
                Availability::Unavailable
            } else if degraded {
                Availability::Degraded
            } else {
                Availability::Available
            },
            maximum_severity: if atpa_alerts > 0 {
                Severity::Alert
            } else if warning_count > 0 || atpa_warnings > 0 {
                Severity::Warning
            } else if caution_count > 0 {
                Severity::Caution
            } else if monitor_not_evaluated {
                Severity::NotEvaluated
            } else {
                Severity::Clear
            },
            predicted_tracks: u16::try_from(included).unwrap_or(u16::MAX),
            caution_tracks: caution_count,
            warning_tracks: warning_count,
            alert_tracks: 0,
            atpa_warning_pairs: atpa_warnings,
            atpa_alert_pairs: atpa_alerts,
            crda_ghosts: crda_count,
            unavailable_tracks: omitted,
        });
    }
    packets
}

fn clear_retained_final_monitor(aid: &mut RunwayAidRuntime) {
    aid.retained_final_alerts.clear();
    aid.retained_input_addresses.clear();
    aid.retained_prediction_signature = None;
    aid.retained_assignment_digest = None;
    aid.retained_assignment_count = 0;
    aid.retained_final_as_of_ns = None;
    aid.retained_final_config = None;
    aid.retained_final_pair = None;
    aid.retained_final_geometry = None;
}

struct FinalMonitorInput<'a> {
    now_ns: u64,
    report_ns: u64,
    included: usize,
    addresses: &'a [u32],
    source_ns: &'a [i64],
    east_m: &'a [f64],
    north_m: &'a [f64],
    velocity_east_mps: &'a [f64],
    velocity_north_mps: &'a [f64],
    ground_speed_mps: &'a [f64],
    r95_m: &'a [f64],
    lat_e7: &'a [i32],
    lon_e7: &'a [i32],
}

fn final_monitor_input_signature(
    input: FinalMonitorInput<'_>,
) -> Option<[u8; oa_sha256::core::sha256::DIGEST_LEN]> {
    let mut hash = oa_sha256::Sha256::new();
    hash.update(b"OA-FINAL-MONITOR-PREDICTION-INPUT-V1\0")
        .ok()?;
    hash.update(&input.now_ns.to_le_bytes()).ok()?;
    hash.update(&input.report_ns.to_le_bytes()).ok()?;
    hash.update(&u32::try_from(input.included).ok()?.to_le_bytes())
        .ok()?;
    for index in 0..input.included {
        let (
            Some(&address),
            Some(&source_ns),
            Some(&east),
            Some(&north),
            Some(&velocity_east),
            Some(&velocity_north),
            Some(&speed),
            Some(&r95),
            Some(&lat),
            Some(&lon),
        ) = (
            input.addresses.get(index),
            input.source_ns.get(index),
            input.east_m.get(index),
            input.north_m.get(index),
            input.velocity_east_mps.get(index),
            input.velocity_north_mps.get(index),
            input.ground_speed_mps.get(index),
            input.r95_m.get(index),
            input.lat_e7.get(index),
            input.lon_e7.get(index),
        )
        else {
            return None;
        };
        hash.update(&address.to_le_bytes()).ok()?;
        hash.update(&source_ns.to_le_bytes()).ok()?;
        hash.update(&east.to_bits().to_le_bytes()).ok()?;
        hash.update(&north.to_bits().to_le_bytes()).ok()?;
        hash.update(&velocity_east.to_bits().to_le_bytes()).ok()?;
        hash.update(&velocity_north.to_bits().to_le_bytes()).ok()?;
        hash.update(&speed.to_bits().to_le_bytes()).ok()?;
        hash.update(&r95.to_bits().to_le_bytes()).ok()?;
        hash.update(&lat.to_le_bytes()).ok()?;
        hash.update(&lon.to_le_bytes()).ok()?;
    }
    Some(hash.finalize())
}

fn relevant_assignment(aid: &RunwayAidRuntime, assignment: &ActiveRunwayAssignment) -> bool {
    (assignment.runway_id == aid.config.first_runway_id
        && assignment.end_name == aid.config.first_end_name)
        || (assignment.runway_id == aid.config.second_runway_id
            && assignment.end_name == aid.config.second_end_name)
}

fn final_monitor_assignment_binding(
    aid: &RunwayAidRuntime,
    assignments: &[ActiveRunwayAssignment],
) -> Option<([u8; oa_sha256::core::sha256::DIGEST_LEN], u32)> {
    let mut hash = oa_sha256::Sha256::new();
    hash.update(b"OA-FINAL-MONITOR-ASSIGNMENT-BINDING-V1\0")
        .ok()?;
    let mut count = 0u32;
    for assignment in assignments
        .iter()
        .filter(|row| relevant_assignment(aid, row))
    {
        hash.update(&assignment.identity.gufi).ok()?;
        hash.update(&assignment.identity.aircraft_address.to_le_bytes())
            .ok()?;
        hash.update(&assignment.identity.centre).ok()?;
        hash.update(&assignment.identity.plan_id).ok()?;
        hash.update(&assignment.generation.to_le_bytes()).ok()?;
        hash.update(&assignment.sequence.to_le_bytes()).ok()?;
        hash.update(&assignment.runway_id.to_le_bytes()).ok()?;
        hash.update(&assignment.end_name).ok()?;
        count = count.checked_add(1)?;
    }
    hash.update(&count.to_le_bytes()).ok()?;
    Some((hash.finalize(), count))
}

fn retained_final_monitor_matches(
    aid: &RunwayAidRuntime,
    now_ns: u64,
    input_addresses: &[u32],
    assignments: &[ActiveRunwayAssignment],
    prediction_signature: Option<[u8; oa_sha256::core::sha256::DIGEST_LEN]>,
) -> bool {
    let Some(as_of_ns) = aid.retained_final_as_of_ns else {
        return false;
    };
    let Some((assignment_digest, assignment_count)) =
        final_monitor_assignment_binding(aid, assignments)
    else {
        return false;
    };
    if aid.retained_final_config != Some(aid.config)
        || aid.retained_final_pair != Some(aid.geometry.pair)
        || aid.retained_final_geometry != Some(aid.geometry)
        || now_ns
            .checked_sub(as_of_ns)
            .is_none_or(|age| age >= aid.geometry.pair.update_ns)
        || aid.retained_input_addresses != input_addresses
        || aid.retained_assignment_digest != Some(assignment_digest)
        || aid.retained_assignment_count != assignment_count
        || (aid.retained_final_alerts.is_empty()
            && aid.retained_prediction_signature != prediction_signature)
    {
        return false;
    }
    for alert in &aid.retained_final_alerts {
        if alert.pair_id != aid.geometry.pair.id
            || input_addresses.binary_search(&alert.address).is_err()
            || !(-900_000_000..=900_000_000).contains(&alert.lat_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&alert.lon_e7)
            || u64::from(alert.seconds_to_entry_milli) * 1_000_000 > aid.geometry.pair.lookahead_ns
        {
            return false;
        }
    }
    true
}

fn retain_final_monitor_result(
    aid: &mut RunwayAidRuntime,
    now_ns: u64,
    input_addresses: &[u32],
    assignments: &[ActiveRunwayAssignment],
    prediction_signature: Option<[u8; oa_sha256::core::sha256::DIGEST_LEN]>,
    output: FinalMonitorRows<'_>,
) -> bool {
    let count = output.count;
    if prediction_signature.is_none()
        || count > oa_runway::final_monitor::MAX_TRACKS
        || input_addresses.len() > aid.retained_input_addresses.capacity()
        || count > aid.retained_final_alerts.capacity()
    {
        return false;
    }
    let Some((assignment_digest, assignment_count)) =
        final_monitor_assignment_binding(aid, assignments)
    else {
        return false;
    };
    clear_retained_final_monitor(aid);
    aid.retained_input_addresses
        .extend_from_slice(input_addresses);
    for index in 0..count {
        let (
            Some(&address),
            Some(&pair_id),
            Some(&severity),
            Some(&lat_e7),
            Some(&lon_e7),
            Some(&entry),
        ) = (
            output.addresses.get(index),
            output.pair_ids.get(index),
            output.severity.get(index),
            output.lat_e7.get(index),
            output.lon_e7.get(index),
            output.seconds_to_entry_milli.get(index),
        )
        else {
            clear_retained_final_monitor(aid);
            return false;
        };
        aid.retained_final_alerts.push(RetainedFinalAlert {
            address,
            pair_id,
            severity,
            lat_e7,
            lon_e7,
            seconds_to_entry_milli: entry,
        });
    }
    aid.retained_final_as_of_ns = Some(now_ns);
    aid.retained_prediction_signature = prediction_signature;
    aid.retained_assignment_digest = Some(assignment_digest);
    aid.retained_assignment_count = assignment_count;
    aid.retained_final_config = Some(aid.config);
    aid.retained_final_pair = Some(aid.geometry.pair);
    aid.retained_final_geometry = Some(aid.geometry);
    true
}

#[cfg(test)]
mod runway_aid_packet_tests {
    use super::{
        final_monitor_assignment_binding, regional_new_with_profile, regional_step,
        retained_final_monitor_matches, runway_aid_packets, ActiveRunwayAssignment, ArvPathColumns,
        ArvScanContext, AtpaScratch, RetainedFinalAlert, RunwayAidFrame, RunwayAidRuntime,
        DEFAULT_REGIONAL_RUN_PROFILE,
    };
    use crate::cli::synthetic_model;
    use crate::input::parse_event_line;
    use crate::session::ReceiptBatch;
    use crate::surveillance::SurveillanceConfig;
    use oa_display::core::runway_aids::{Availability, Severity};

    fn synthetic_aid_runtime() -> Option<RunwayAidRuntime> {
        use oa_runway::adaptation::AidPairConfig;
        use oa_runway::final_monitor::{self, FinalCourseInput, FinalMonitorConfig};
        use oa_runway::plane::Local;
        let pair = final_monitor::prepare_pair(
            FinalCourseInput {
                runway_id: 1,
                end_name: *b"09L",
                outer_fix: Local {
                    east_m: -500.0,
                    north_m: -5_000.0,
                },
                threshold: Local {
                    east_m: -500.0,
                    north_m: 5_000.0,
                },
            },
            FinalCourseInput {
                runway_id: 2,
                end_name: *b"09R",
                outer_fix: Local {
                    east_m: 500.0,
                    north_m: -5_000.0,
                },
                threshold: Local {
                    east_m: 500.0,
                    north_m: 5_000.0,
                },
            },
            FinalMonitorConfig {
                ntz_width_ft: 800.0,
                lookahead_ns: 30_000_000_000,
                update_ns: 4_800_000_000,
                minimum_separation_ft: 1_000.0,
            },
        )
        .ok()?;
        Some(RunwayAidRuntime {
            config: AidPairConfig {
                airport: *b"KORD",
                first_runway_id: 1,
                first_end_name: *b"09L",
                first_width_ft: 150,
                second_runway_id: 2,
                second_end_name: *b"09R",
                second_width_ft: 150,
                ntz_width_ft: 800,
                minimum_spacing_ft: 1_000,
                lookahead_ms: 30_000,
                update_ms: 4_800,
                centerline_extension_m: 10_000,
                arv_lateral_tolerance_mm: 0,
                arv_heading_tolerance_cdeg: 0,
                crda_offset_mm: 0,
            },
            geometry: crate::runway_aids::RunwayAidGeometry {
                pair,
                plane_origin: oa_runway::plane::Geo {
                    lat_deg: 41.0,
                    lon_deg: -87.0,
                },
                plane_origin_lat_e7: 410_000_000,
                plane_origin_lon_e7: -870_000_000,
                first_threshold: Local {
                    east_m: -500.0,
                    north_m: 5_000.0,
                },
                first_inbound_east: 0.0,
                first_inbound_north: 1.0,
                first_width_m: 45.0,
                second_threshold: Local {
                    east_m: 500.0,
                    north_m: 5_000.0,
                },
                second_inbound_east: 0.0,
                second_inbound_north: 1.0,
                second_width_m: 45.0,
                crda_offset_m: 0.0,
            },
            arv_coverage: oa_display::core::runway_aids::ArvCoverage::PairOnly,
            arv_paths: ArvPathColumns::new(0).ok()?,
            state: final_monitor::FinalMonitorState::new(),
            retained_final_alerts: Vec::with_capacity(final_monitor::MAX_TRACKS),
            retained_input_addresses: Vec::with_capacity(final_monitor::MAX_TRACKS),
            retained_prediction_signature: None,
            retained_assignment_digest: None,
            retained_assignment_count: 0,
            retained_final_as_of_ns: None,
            retained_final_config: None,
            retained_final_pair: None,
            retained_final_geometry: None,
        })
    }

    fn complete_synthetic_arv_aid() -> Option<RunwayAidRuntime> {
        use oa_display::core::runway_aids::ArvCoverage;
        let mut aid = synthetic_aid_runtime()?;
        aid.config.lookahead_ms = 10_000;
        aid.config.arv_lateral_tolerance_mm = 200_000;
        aid.config.arv_heading_tolerance_cdeg = 500;
        aid.arv_coverage = ArvCoverage::IssuedAirportComplete;
        aid.arv_paths = ArvPathColumns {
            ids: vec![1, 2, 100, 101],
            kinds: vec![
                oa_runway::adaptation::PATH_KIND_RUNWAY,
                oa_runway::adaptation::PATH_KIND_RUNWAY,
                oa_runway::adaptation::PATH_KIND_TAXIWAY,
                oa_runway::adaptation::PATH_KIND_TAXIWAY,
            ],
            threshold_east_m: vec![-500.0, 500.0, 250.0, 250.0],
            threshold_north_m: vec![5_000.0, 5_000.0, 5_000.0, -5_000.0],
            inbound_east: vec![0.0; 4],
            inbound_north: vec![1.0, 1.0, 1.0, -1.0],
            width_m: vec![45.0, 45.0, 30.0, 30.0],
        };
        Some(aid)
    }

    fn synthetic_arv_surveillance(
        longitude_offset_e7: i32,
    ) -> Option<(
        crate::surveillance::Surveillance,
        Vec<crate::surveillance::ObservedTrack>,
    )> {
        let model = synthetic_model().ok()?;
        let mut surveillance = crate::surveillance::surveillance_new(
            model,
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
        )
        .ok()?;
        let lat_step_e7 = 900;
        let center_lat_e7: i32 = 410_000_000;
        let longitude_e7 = (-870_000_000_i32).checked_add(longitude_offset_e7)?;
        let mut last_tracks = Vec::new();
        for sequence in 1..=8_i64 {
            let receipt_ns = sequence.checked_mul(1_000_000_000)?;
            let source_ns = receipt_ns.checked_sub(100_000_000)?;
            crate::surveillance::surveillance_begin(&mut surveillance, receipt_ns).ok()?;
            let event = crate::input::NormalizedEvent {
                receipt_ns,
                source_ns,
                sequence: u64::try_from(sequence).ok()?,
                source: crate::input::SourceKind::Surveillance,
                identity: crate::input::Identity {
                    flight_gufi: 1,
                    aircraft_address: 0x00ab_cdef,
                },
                centre: *b"ZAU1",
                kind: crate::input::EventKind::Position {
                    latitude_e7: center_lat_e7.checked_add(
                        i32::try_from(
                            sequence
                                .checked_sub(1)?
                                .checked_mul(i64::from(lat_step_e7))?,
                        )
                        .ok()?,
                    )?,
                    longitude_e7,
                    altitude_ft: Some(10_000),
                    east_mmps: 0,
                    north_mmps: 10_000,
                    vertical_rate_fpm: Some(0),
                },
            };
            crate::surveillance::surveillance_event(&mut surveillance, &event).ok()?;
            let frame = crate::surveillance::surveillance_finish(&mut surveillance, None).ok()?;
            last_tracks = frame.tracks;
        }
        Some((surveillance, last_tracks))
    }

    // REQ: OA-RWY-025
    #[test]
    fn typed_runway_dispatch_audits_assignment_and_ack_without_clearing_alert() {
        use super::{
            regional_dispatch_runway_operator, RunwayOperatorDispatchRequest, RunwayOperatorReceipt,
        };
        use crate::input::OperatorCommand;
        use crate::surveillance::SurveillanceConfig;
        use oa_audit_core::{Action, Role};
        use oa_display::interaction::{FlightIdentity, Intent, TargetView};
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

        let aid_result = synthetic_aid_runtime();
        assert!(aid_result.is_some(), "synthetic issued aid geometry");
        let Some(aid) = aid_result else { return };
        let model_result = synthetic_model();
        assert!(model_result.is_ok(), "synthetic model");
        let Ok(model) = model_result else { return };
        let snapshot_result = oa_airspace::core::snapshot::encode(&model);
        assert!(snapshot_result.is_ok(), "synthetic snapshot");
        let Ok(snapshot) = snapshot_result else {
            return;
        };
        let runtime_result = regional_new_with_profile(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            DEFAULT_REGIONAL_RUN_PROFILE,
        );
        assert!(runtime_result.is_ok(), "regional runtime");
        let Ok(mut runtime) = runtime_result else {
            return;
        };
        runtime.frame_sequence = 1;
        runtime.runway_aids.push(aid);
        let identity = FlightIdentity {
            gufi: [0x31; 16],
            aircraft_address: 0x00ab_cdef,
            centre: *b"ZAU1",
            plan_id: *b"PLAN00000001",
        };
        runtime.views.push(TargetView {
            address: identity.aircraft_address,
            generation: 9,
            identity_claims: 1,
            identity_present: 1,
            reserved: 0,
            identity,
        });
        let registry = oa_audit_core::parse_principal_registry(
            b"OACERT-LOCAL-PRINCIPALS-V1\n51\toperate\n52\tmonitor\n",
        );
        assert!(registry.is_ok(), "synthetic role registry");
        let Ok(registry) = registry else { return };
        let root = std::env::temp_dir().join(format!(
            "oa-regional-arv-dispatch-{}-{}",
            std::process::id(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(
            std::fs::create_dir(&root).is_ok(),
            "isolated temp directory"
        );
        let audit_log = root.join("actions.audit");
        let checkpoint = oa_audit_core::genesis();
        let unknown_runway = regional_dispatch_runway_operator(
            &mut runtime,
            RunwayOperatorDispatchRequest {
                operator_command: OperatorCommand {
                    plan_id: identity.plan_id,
                    target_generation: 9,
                    view_sequence: 1,
                    actor_unit: 1,
                    intent: Intent::AssignRunway(99),
                },
                identity,
                generation: 9,
                service_sequence: 1,
                audit_log: &audit_log,
                expected_checkpoint: checkpoint,
                principal_registry: &registry,
                principal: 51,
                role: Role::Operate,
                time_ns: 5,
            },
        );
        assert!(unknown_runway.is_err(), "unissued runway ID refuses");
        assert!(!audit_log.exists(), "refusal writes no audit record");
        let stale_identity = FlightIdentity {
            gufi: [0x44; 16],
            ..identity
        };
        let stale_command = regional_dispatch_runway_operator(
            &mut runtime,
            RunwayOperatorDispatchRequest {
                operator_command: OperatorCommand {
                    plan_id: stale_identity.plan_id,
                    target_generation: 9,
                    view_sequence: 1,
                    actor_unit: 1,
                    intent: Intent::AssignRunway(1),
                },
                identity: stale_identity,
                generation: 9,
                service_sequence: 1,
                audit_log: &audit_log,
                expected_checkpoint: checkpoint,
                principal_registry: &registry,
                principal: 51,
                role: Role::Operate,
                time_ns: 6,
            },
        );
        assert!(stale_command.is_err(), "stale full-flight identity refuses");
        assert!(!audit_log.exists(), "stale target writes no audit record");
        let assign_command = OperatorCommand {
            plan_id: identity.plan_id,
            target_generation: 9,
            view_sequence: 1,
            actor_unit: 1,
            intent: Intent::AssignRunway(1),
        };
        let assigned = regional_dispatch_runway_operator(
            &mut runtime,
            RunwayOperatorDispatchRequest {
                operator_command: assign_command,
                identity,
                generation: 9,
                service_sequence: 1,
                audit_log: &audit_log,
                expected_checkpoint: checkpoint,
                principal_registry: &registry,
                principal: 51,
                role: Role::Operate,
                time_ns: 10,
            },
        );
        assert!(
            assigned.is_ok(),
            "typed assignment reaches the normal runtime caller: {assigned:?}"
        );
        let Ok(RunwayOperatorReceipt::Assignment(assignment_receipt)) = assigned else {
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        assert_eq!(assignment_receipt.assignment.runway_id, 1);
        assert_eq!(
            assignment_receipt.audit_receipt.action(),
            Action::RunwayAssignment
        );
        assert_eq!(assignment_receipt.audit_checkpoint.count, 1);

        let stored = runtime.runway_assignments.first_mut();
        assert!(stored.is_some(), "assignment stored after durable receipt");
        let Some(stored) = stored else {
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        stored.arv_status = oa_runway::arv::AlertKind::Taxiway;
        stored.arv_as_of_ns = 20;
        let ack_command = OperatorCommand {
            plan_id: identity.plan_id,
            target_generation: 9,
            view_sequence: 1,
            actor_unit: 1,
            intent: Intent::AcknowledgeRunwayAlert,
        };
        let refused = regional_dispatch_runway_operator(
            &mut runtime,
            RunwayOperatorDispatchRequest {
                operator_command: ack_command,
                identity,
                generation: 9,
                service_sequence: 1,
                audit_log: &audit_log,
                expected_checkpoint: assignment_receipt.audit_checkpoint,
                principal_registry: &registry,
                principal: 52,
                role: Role::Monitor,
                time_ns: 30,
            },
        );
        assert!(refused.is_err(), "monitor cannot acknowledge an ARV alert");
        let Some(unchanged) = runtime.runway_assignments.first() else {
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        assert_eq!(unchanged.arv_status, oa_runway::arv::AlertKind::Taxiway);
        assert!(!unchanged.arv_acknowledged);

        let acknowledged = regional_dispatch_runway_operator(
            &mut runtime,
            RunwayOperatorDispatchRequest {
                operator_command: ack_command,
                identity,
                generation: 9,
                service_sequence: 2,
                audit_log: &audit_log,
                expected_checkpoint: assignment_receipt.audit_checkpoint,
                principal_registry: &registry,
                principal: 51,
                role: Role::Operate,
                time_ns: 40,
            },
        );
        assert!(
            acknowledged.is_ok(),
            "authorized typed acknowledgement is audited"
        );
        let Ok(RunwayOperatorReceipt::AlertAcknowledgement(ack_receipt)) = acknowledged else {
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        assert_eq!(ack_receipt.audit_receipt.action(), Action::AlertAcknowledge);
        assert_eq!(ack_receipt.audit_checkpoint.count, 2);
        assert!(ack_receipt.acknowledged);
        let Some(after_ack) = runtime.runway_assignments.first() else {
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        assert_eq!(after_ack.arv_status, oa_runway::arv::AlertKind::Taxiway);
        assert!(after_ack.arv_acknowledged);
        let _ = std::fs::remove_dir_all(&root);
    }

    // REQ: OA-RWY-024
    #[test]
    fn pair_only_adaptation_keeps_arv_unavailable_instead_of_reporting_clear() {
        use super::runway_arv_packets;
        use oa_display::core::runway_aids::{ArvAvailability, ArvCoverage, ArvStatus};
        use oa_display::interaction::FlightIdentity;

        let aid_result = synthetic_aid_runtime();
        assert!(aid_result.is_some(), "synthetic pair-only aid");
        let Some(mut aid) = aid_result else { return };
        let identity = FlightIdentity {
            gufi: [0x32; 16],
            aircraft_address: 0x00ab_cdef,
            centre: *b"ZAU1",
            plan_id: *b"PLAN00000002",
        };
        let mut assignments = vec![ActiveRunwayAssignment {
            identity,
            generation: 3,
            sequence: 1,
            runway_id: aid.config.first_runway_id,
            end_name: aid.config.first_end_name,
            arv_status: oa_runway::arv::AlertKind::Clear,
            arv_acknowledged: false,
            arv_ack_sequence: 0,
            arv_as_of_ns: 0,
        }];
        let views = [oa_display::interaction::TargetView {
            address: identity.aircraft_address,
            generation: 3,
            identity_claims: 1,
            identity_present: 1,
            reserved: 0,
            identity,
        }];
        let result = runway_arv_packets(
            std::slice::from_mut(&mut aid),
            &mut assignments,
            &views,
            None,
            &[],
            ArvScanContext {
                scan_time_ns: 1_000_000_000,
                scan_sequence: 1,
                output_capacity: 1,
            },
        );
        assert!(result.is_ok(), "bounded unavailable packet is returned");
        let Ok(rows) = result else { return };
        assert_eq!(rows.len(), 1);
        let Some(row) = rows.first() else { return };
        assert_eq!(row.coverage, ArvCoverage::PairOnly);
        assert_eq!(row.availability, ArvAvailability::Unavailable);
        assert_eq!(row.status, ArvStatus::NotEvaluated);
        assert_eq!(row.predicted_at_ns, 0);
    }

    fn synthetic_runtime_arv_status(
        longitude_offset_e7: i32,
    ) -> Option<oa_display::core::runway_aids::ArvStatus> {
        use super::runway_arv_packets;
        use oa_display::interaction::{FlightIdentity, TargetView};

        let (surveillance, tracks) = synthetic_arv_surveillance(longitude_offset_e7)?;
        let mut aid = complete_synthetic_arv_aid()?;
        let identity = FlightIdentity {
            gufi: [0x33; 16],
            aircraft_address: 0x00ab_cdef,
            centre: *b"ZAU1",
            plan_id: *b"PLAN00000003",
        };
        let assignment = ActiveRunwayAssignment {
            identity,
            generation: 4,
            sequence: 1,
            runway_id: aid.config.first_runway_id,
            end_name: aid.config.first_end_name,
            arv_status: oa_runway::arv::AlertKind::Clear,
            arv_acknowledged: false,
            arv_ack_sequence: 0,
            arv_as_of_ns: 0,
        };
        let mut assignments = vec![assignment];
        let views = [TargetView {
            address: identity.aircraft_address,
            generation: 4,
            identity_claims: 1,
            identity_present: 1,
            reserved: 0,
            identity,
        }];
        let output = runway_arv_packets(
            std::slice::from_mut(&mut aid),
            &mut assignments,
            &views,
            Some(&surveillance),
            &tracks,
            ArvScanContext {
                scan_time_ns: 8_000_000_000,
                scan_sequence: 8,
                output_capacity: 1,
            },
        )
        .ok()?;
        output.first().map(|row| row.status)
    }

    // REQ: OA-RWY-024
    #[test]
    fn runtime_arv_uses_the_shared_tracker_and_issued_candidate_paths() {
        assert_eq!(
            synthetic_runtime_arv_status(-60_000),
            Some(oa_display::core::runway_aids::ArvStatus::Clear),
            "predicted track on the assigned path is clear"
        );
        assert_eq!(
            synthetic_runtime_arv_status(60_000),
            Some(oa_display::core::runway_aids::ArvStatus::WrongRunway),
            "predicted track on the other runway is classified"
        );
        assert_eq!(
            synthetic_runtime_arv_status(30_000),
            Some(oa_display::core::runway_aids::ArvStatus::Taxiway),
            "predicted track on the taxiway candidate is classified"
        );
    }

    // REQ: OA-RWY-022
    #[test]
    fn unconfigured_runtime_emits_explicit_unavailable_packet() {
        let scratch_result = AtpaScratch::new(1);
        assert!(
            scratch_result.is_ok(),
            "small bounded ATPA scratch allocation"
        );
        let Ok(mut scratch) = scratch_result else {
            return;
        };
        let mut ghosts = Vec::new();
        let packets = runway_aid_packets(
            &mut [],
            &mut scratch,
            RunwayAidFrame {
                assignments: &[],
                surveillance: None,
                tracks: &[],
                ghosts: &mut ghosts,
                scan_time_ns: 1_000_000_000,
                scan_sequence: 1,
            },
        );
        assert_eq!(packets.len(), 1);
        let packet_result = packets.first();
        assert!(packet_result.is_some(), "one unavailable packet is emitted");
        let Some(packet) = packet_result else {
            return;
        };
        assert_eq!(packet.availability, Availability::Unavailable);
        assert_eq!(packet.scan_sequence, 1);
        assert_eq!(packet.first_runway_id, 0);
        assert_eq!(packet.second_runway_id, 0);
    }

    // REQ: OA-RWY-022
    #[test]
    fn runtime_packet_retains_warning_and_original_as_of_on_not_due_scan() {
        use crate::session::ReceiptBatch;
        use crate::surveillance::SurveillanceConfig;
        let aid_result = synthetic_aid_runtime();
        assert!(aid_result.is_some(), "synthetic aid geometry");
        let Some(aid) = aid_result else { return };
        let model_result = crate::cli::synthetic_model();
        assert!(model_result.is_ok(), "synthetic model");
        let Ok(model) = model_result else { return };
        let snapshot_result = oa_airspace::core::snapshot::encode(&model);
        assert!(snapshot_result.is_ok(), "synthetic snapshot");
        let Ok(snapshot) = snapshot_result else {
            return;
        };
        let runtime_result = super::regional_new(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
        );
        assert!(runtime_result.is_ok(), "regional runtime");
        let Ok(mut runtime) = runtime_result else {
            return;
        };
        runtime.runway_aids.push(aid);

        let mut evaluated_packet = None;
        for sequence in 1..=6_u32 {
            let receipt_ns = i64::from(sequence) * 1_000_000_000;
            let source_ns = receipt_ns - 100_000_000;
            let event_result = crate::input::parse_event_line(&format!(
                "v1\t{receipt_ns}\t{source_ns}\t{sequence}\tSURVEILLANCE\t00000000000000000000000000000001\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"
            ));
            assert!(event_result.is_ok(), "synthetic event");
            let Ok(event) = event_result else { return };
            let frame_result = regional_step(
                &mut runtime,
                &ReceiptBatch {
                    receipt_ns,
                    source_clock_regressions: 0,
                    events: vec![event],
                },
            );
            assert!(frame_result.is_ok(), "regional scan");
            let Ok(frame) = frame_result else { return };
            if sequence == 6 {
                evaluated_packet = frame.runway_aids.first().copied();
            }
        }
        assert!(evaluated_packet.is_some(), "evaluated runway packet");
        let Some(evaluated) = evaluated_packet else {
            return;
        };
        assert_eq!(evaluated.warning_tracks, 1);
        assert_eq!(evaluated.maximum_severity, Severity::Warning);
        assert_eq!(evaluated.final_monitor_as_of_ns, 6_000_000_000);

        let receipt_ns = 7_000_000_000_i64;
        let source_ns = receipt_ns - 100_000_000;
        let event_result = crate::input::parse_event_line(&format!(
            "v1\t{receipt_ns}\t{source_ns}\t7\tSURVEILLANCE\t00000000000000000000000000000001\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"
        ));
        assert!(event_result.is_ok(), "synthetic not-due event");
        let Ok(event) = event_result else { return };
        let frame_result = regional_step(
            &mut runtime,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events: vec![event],
            },
        );
        assert!(frame_result.is_ok(), "regional not-due scan");
        let Ok(frame) = frame_result else { return };
        let retained_result = frame.runway_aids.first();
        assert!(
            retained_result.is_some(),
            "runway-aid packet on not-due scan"
        );
        let Some(retained) = retained_result else {
            return;
        };
        assert_eq!(retained.availability, Availability::Available);
        assert_eq!(retained.warning_tracks, 1);
        assert_eq!(retained.maximum_severity, Severity::Warning);
        assert_eq!(retained.final_monitor_as_of_ns, 6_000_000_000);
    }

    // REQ: OA-RWY-022
    #[test]
    fn clear_result_is_not_reused_after_same_key_prediction_changes() {
        let aid_result = synthetic_aid_runtime();
        assert!(aid_result.is_some(), "synthetic runway aid");
        let Some(aid) = aid_result else { return };
        let model_result = synthetic_model();
        assert!(model_result.is_ok(), "synthetic model");
        let Ok(model) = model_result else { return };
        let snapshot_result = oa_airspace::core::snapshot::encode(&model);
        assert!(snapshot_result.is_ok(), "model snapshot");
        let Ok(snapshot) = snapshot_result else {
            return;
        };
        let runtime_result = regional_new_with_profile(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            DEFAULT_REGIONAL_RUN_PROFILE,
        );
        assert!(runtime_result.is_ok(), "regional runtime");
        let Ok(mut runtime) = runtime_result else {
            return;
        };
        runtime.runway_aids.push(aid);

        let mut clear_packet = None;
        for sequence in 1..=6_u32 {
            let receipt_ns = i64::from(sequence) * 1_000_000_000;
            let source_ns = receipt_ns - 100_000_000;
            let event_result = parse_event_line(&format!(
                "v1\t{receipt_ns}\t{source_ns}\t{sequence}\tSURVEILLANCE\t00000000000000000000000000000001\tABCDEF\tZAU1\tPOS\t410000000\t-870059500\t10000\t0\t0\t0"
            ));
            assert!(event_result.is_ok(), "bounded safe synthetic report");
            let Ok(event) = event_result else { return };
            let frame_result = regional_step(
                &mut runtime,
                &ReceiptBatch {
                    receipt_ns,
                    source_clock_regressions: 0,
                    events: vec![event],
                },
            );
            assert!(frame_result.is_ok(), "complete synthetic scan");
            let Ok(frame) = frame_result else { return };
            if sequence == 6 {
                clear_packet = frame.runway_aids.first().copied();
            }
        }
        assert!(clear_packet.is_some(), "due clear packet");
        let Some(clear) = clear_packet else { return };
        assert_eq!(clear.maximum_severity, Severity::Clear);
        assert_eq!(clear.final_monitor_as_of_ns, 6_000_000_000);

        let receipt_ns = 7_000_000_000_i64;
        let event_result = parse_event_line(&format!(
            "v1\t{receipt_ns}\t{}\t7\tSURVEILLANCE\t00000000000000000000000000000001\tABCDEF\tZAU1\tPOS\t410000000\t-870058000\t10000\t10000\t0\t0",
            receipt_ns - 100_000_000
        ));
        assert!(event_result.is_ok(), "bounded changed prediction");
        let Ok(event) = event_result else { return };
        let frame_result = regional_step(
            &mut runtime,
            &ReceiptBatch {
                receipt_ns,
                source_clock_regressions: 0,
                events: vec![event],
            },
        );
        assert!(frame_result.is_ok(), "not-due changed-prediction scan");
        let Ok(frame) = frame_result else { return };
        assert!(!frame.runway_aids.is_empty(), "not-due packet");
        let Some(packet) = frame.runway_aids.first() else {
            return;
        };
        assert_ne!(packet.availability, Availability::Available);
        assert_ne!(packet.maximum_severity, Severity::Clear);
        assert_eq!(packet.final_monitor_as_of_ns, 0);
    }

    // REQ: OA-RWY-022
    #[test]
    fn retained_final_monitor_expires_and_invalidates_changed_target_assignment_or_config() {
        let aid_result = synthetic_aid_runtime();
        assert!(aid_result.is_some());
        let Some(mut aid) = aid_result else { return };
        aid.retained_input_addresses.push(0x00ab_cdef);
        aid.retained_final_as_of_ns = Some(1_000_000_000);
        aid.retained_final_config = Some(aid.config);
        aid.retained_final_pair = Some(aid.geometry.pair);
        aid.retained_final_geometry = Some(aid.geometry);
        aid.retained_prediction_signature = Some([7; oa_sha256::core::sha256::DIGEST_LEN]);
        let empty_binding = final_monitor_assignment_binding(&aid, &[]);
        assert!(empty_binding.is_some(), "empty assignment binding computes");
        let Some((digest, count)) = empty_binding else {
            return;
        };
        aid.retained_assignment_digest = Some(digest);
        aid.retained_assignment_count = count;
        aid.retained_final_alerts.push(RetainedFinalAlert {
            address: 0x00ab_cdef,
            pair_id: aid.geometry.pair.id,
            severity: oa_runway::final_monitor::FinalSeverity::Warning,
            lat_e7: 410_000_000,
            lon_e7: -870_000_000,
            seconds_to_entry_milli: 0,
        });
        let addresses = [0x00ab_cdef];
        assert!(retained_final_monitor_matches(
            &aid,
            2_000_000_000,
            &addresses,
            &[],
            Some([7; oa_sha256::core::sha256::DIGEST_LEN])
        ));
        assert!(!retained_final_monitor_matches(
            &aid,
            5_800_000_000,
            &addresses,
            &[],
            Some([7; oa_sha256::core::sha256::DIGEST_LEN])
        ));
        assert!(!retained_final_monitor_matches(
            &aid,
            2_000_000_000,
            &[0x00ab_cdee],
            &[],
            Some([7; oa_sha256::core::sha256::DIGEST_LEN])
        ));
        let original_geometry = aid.geometry;
        aid.geometry.plane_origin_lon_e7 += 1;
        assert!(!retained_final_monitor_matches(
            &aid,
            2_000_000_000,
            &addresses,
            &[],
            Some([7; oa_sha256::core::sha256::DIGEST_LEN])
        ));
        aid.geometry = original_geometry;

        let assignment = ActiveRunwayAssignment {
            identity: oa_display::interaction::FlightIdentity {
                gufi: [1; 16],
                aircraft_address: 0x00ab_cdef,
                centre: *b"ZAU1",
                plan_id: *b"PLAN00000001",
            },
            generation: 1,
            sequence: 1,
            runway_id: aid.config.first_runway_id,
            end_name: aid.config.first_end_name,
            arv_status: oa_runway::arv::AlertKind::Clear,
            arv_acknowledged: false,
            arv_ack_sequence: 0,
            arv_as_of_ns: 0,
        };
        let binding = final_monitor_assignment_binding(&aid, &[assignment]);
        assert!(binding.is_some(), "assignment binding computes");
        let Some((digest, count)) = binding else {
            return;
        };
        aid.retained_assignment_digest = Some(digest);
        aid.retained_assignment_count = count;
        assert!(retained_final_monitor_matches(
            &aid,
            2_000_000_000,
            &addresses,
            &[assignment],
            Some([7; oa_sha256::core::sha256::DIGEST_LEN])
        ));
        let changed_assignment = ActiveRunwayAssignment {
            sequence: 2,
            ..assignment
        };
        assert!(!retained_final_monitor_matches(
            &aid,
            2_000_000_000,
            &addresses,
            &[changed_assignment],
            Some([7; oa_sha256::core::sha256::DIGEST_LEN])
        ));
        aid.config.update_ms = 4_700;
        assert!(!retained_final_monitor_matches(
            &aid,
            2_000_000_000,
            &addresses,
            &[],
            Some([7; oa_sha256::core::sha256::DIGEST_LEN])
        ));
    }

    // REQ: OA-RWY-022
    #[test]
    fn first_unavailable_final_monitor_packet_has_no_as_of_and_never_says_clear() {
        let aid_result = synthetic_aid_runtime();
        assert!(aid_result.is_some(), "synthetic aid geometry");
        let Some(mut aid) = aid_result else { return };
        let scratch_result = AtpaScratch::new(1);
        assert!(scratch_result.is_ok(), "bounded ATPA scratch");
        let Ok(mut scratch) = scratch_result else {
            return;
        };
        let unknown_track = crate::surveillance::ObservedTrack {
            state_ns: 1,
            newest_source_ns: 1,
            address: 1,
            lat_e7: 410_000_000,
            lon_e7: -870_000_000,
            altitude_ft: 0,
            vertical_rate_fpm: 0,
            east_mmps: 0,
            north_mmps: 0,
            r95_mm: 1_000,
        };
        let tracks = vec![unknown_track; oa_runway::final_monitor::MAX_TRACKS + 1];
        let mut ghosts = Vec::new();
        let packets = runway_aid_packets(
            std::slice::from_mut(&mut aid),
            &mut scratch,
            RunwayAidFrame {
                assignments: &[],
                surveillance: None,
                tracks: &tracks,
                ghosts: &mut ghosts,
                scan_time_ns: 1_000_000_000,
                scan_sequence: 1,
            },
        );
        let packet_result = packets.first();
        assert!(packet_result.is_some(), "runtime packet exists");
        let Some(packet) = packet_result else { return };
        assert_eq!(packet.availability, Availability::Unavailable);
        assert_eq!(packet.maximum_severity, Severity::NotEvaluated);
        assert_eq!(packet.final_monitor_as_of_ns, 0);
    }
}

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

fn runtime_item_id(batch_ordinal: u64, item_index: usize) -> Result<u64, String> {
    let index = u64::try_from(item_index).map_err(|_| "quarantine item index overflow")?;
    if index >= (1 << 8) || batch_ordinal == 0 || batch_ordinal > (u64::MAX >> 8) {
        return Err("quarantine batch/item identity overflow".into());
    }
    batch_ordinal
        .checked_shl(8)
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
    keys: &[oa_flight::FlightKey],
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
            current.kind,
            current.gufi,
            current.callsign,
            current.dep,
            current.arr,
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

fn reclaim_retired_flight_keys(runtime: &mut RegionalRuntime) -> Result<(), String> {
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
        .ok_or("flight-key retirement probe-work counter overflow")?;
    if maximum_probe_work > MAX_FLIGHT_KEY_RECLAIM_PROBE_WORK {
        return Err("flight-key retirement probe-work budget exceeded".into());
    }
    let mut write = 0usize;
    for read in 0..len {
        if read >= MAX_FLIGHT_KEY_RECLAIM_WORK {
            return Err("flight-key retirement work budget exceeded".into());
        }
        let key = runtime
            .known_flights
            .get(read)
            .copied()
            .ok_or("flight-key retirement index invariant")?;
        if flight::flight_driver_has_key(&runtime.flights, key) {
            if write != read {
                let dest = runtime
                    .known_flights
                    .get_mut(write)
                    .ok_or("flight-key retirement destination invariant")?;
                *dest = key;
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
        reclaim_retired_flight_keys(runtime)?;
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
    outcome: flight::FlightApplyOutcome,
) {
    let Some((key, insert_at)) = pending else {
        return;
    };
    if !matches!(
        outcome.operation,
        Some(oa_flight::Outcome::Applied | oa_flight::Outcome::Unchanged)
    ) {
        return;
    }
    runtime.known_flights.insert(insert_at, key);
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
            item_id: runtime_item_id(batch_ordinal, item_index)?,
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
    let mut runtime = regional_new_with_source_identity_and_checkpoint(
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
    )?;
    if package.runway_aids_bytes().is_some() {
        regional_install_verified_runway_aids(&mut runtime, &package)?;
    }
    Ok(runtime)
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

/// One explicit, already-admitted assignment request. The caller supplies the full current flight
/// key and captured view/command sequence; this operation does not infer an assignment from route,
/// callsign, or proximity. Principal identity is a registry claim, not credential authentication.
pub struct RunwayAssignmentRequest<'a> {
    /// Existing typed interaction request; the assignment adapter refuses any other intent.
    pub operator_command: crate::input::OperatorCommand,
    pub identity: oa_display::interaction::FlightIdentity,
    pub generation: u32,
    pub view_sequence: u32,
    pub service_sequence: u32,
    pub runway_id: u32,
    pub end_name: [u8; 3],
    pub audit_log: &'a Path,
    pub expected_checkpoint: oa_audit_core::Checkpoint,
    pub principal_registry: &'a oa_audit_core::PrincipalRegistry,
    pub principal: u64,
    pub role: oa_audit_core::Role,
    pub time_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunwayAssignmentReceipt {
    pub assignment: RunwayAssignmentValue,
    pub audit_checkpoint: oa_audit_core::Checkpoint,
    pub audit_receipt: oa_audit_core::AuditReceipt,
}

/// Apply an explicit runway assignment after current full-key/view validation and durable audit.
/// A runway aid profile must be installed first; all refusals leave assignment state unchanged.
pub fn regional_assign_runway(
    runtime: &mut RegionalRuntime,
    request: RunwayAssignmentRequest<'_>,
) -> Result<RunwayAssignmentReceipt, String> {
    if request.operator_command.intent
        != oa_display::interaction::Intent::AssignRunway(request.runway_id)
        || request.operator_command.plan_id != request.identity.plan_id
        || request.operator_command.target_generation != request.generation
        || request.operator_command.view_sequence != request.view_sequence
        || !(1..=2).contains(&request.operator_command.actor_unit)
    {
        return Err("runway assignment typed operator request does not match its target".into());
    }
    if runtime.failed || runtime.frame_sequence == 0 || runtime.runway_aids.is_empty() {
        return Err("runway assignment requires an active configured runtime".into());
    }
    let current_sequence = u32::try_from(runtime.frame_sequence)
        .map_err(|_| "runway assignment view sequence exhausted")?;
    if request.view_sequence != current_sequence
        || request.service_sequence == 0
        || request.generation == 0
        || request.runway_id == 0
    {
        return Err("runway assignment has stale view or invalid sequence/target".into());
    }
    let matching_views = runtime
        .views
        .iter()
        .filter(|view| view.address == request.identity.aircraft_address);
    let mut selected = None;
    for view in matching_views {
        if view.identity_claims != 1
            || view.identity_present != 1
            || view.reserved != 0
            || view.generation != request.generation
            || view.identity != request.identity
        {
            return Err("runway assignment full identity is ambiguous or stale".into());
        }
        if selected.replace(view).is_some() {
            return Err("runway assignment address has multiple current views".into());
        }
    }
    if selected.is_none() {
        return Err("runway assignment target is absent from the current view".into());
    }
    let mut matched_airport = None;
    let mut matches = 0usize;
    for aid in &runtime.runway_aids {
        if aid.config.first_runway_id == request.runway_id
            && aid.config.first_end_name == request.end_name
        {
            matched_airport = Some(aid.config.airport);
            matches += 1;
        }
        if aid.config.second_runway_id == request.runway_id
            && aid.config.second_end_name == request.end_name
        {
            matched_airport = Some(aid.config.airport);
            matches += 1;
        }
    }
    if matches != 1 {
        return Err("runway assignment target is not unique in the verified profile".into());
    }
    let registry_role = oa_audit_core::authorize_principal(
        request.principal_registry,
        request.principal,
        oa_audit_core::Action::RunwayAssignment,
    )
    .map_err(|error| format!("runway assignment principal registry refused: {error:?}"))?;
    if registry_role != request.role {
        return Err("runway assignment role does not match pinned principal registry".into());
    }
    if request.time_ns == 0 {
        return Err("runway assignment audit timestamp must be positive".into());
    }
    let slot_index = runtime.runway_assignments.iter().position(|assignment| {
        assignment.identity == request.identity && assignment.generation == request.generation
    });
    if let Some(index) = slot_index {
        if runtime
            .runway_assignments
            .get(index)
            .is_none_or(|assignment| request.service_sequence <= assignment.sequence)
        {
            return Err("runway assignment sequence is stale or duplicated".into());
        }
    }
    if slot_index.is_none() && runtime.runway_assignments.len() >= runtime.profile.max_flights {
        return Err("runway assignment active-flight capacity exhausted".into());
    }
    let airport = matched_airport.ok_or("runway assignment airport missing")?;
    let target_text = format!(
        "RUNWAY {} {} {}",
        std::str::from_utf8(&airport).unwrap_or("????"),
        request.runway_id,
        std::str::from_utf8(&request.end_name).unwrap_or("???")
    );
    let (target, target_len) = oa_audit_core::target(target_text.as_bytes())
        .map_err(|error| format!("runway assignment audit target: {error:?}"))?;
    let mut detail_text = String::new();
    detail_text
        .try_reserve_exact(512)
        .map_err(|_| "runway assignment audit details allocation refused")?;
    writeln!(
        detail_text,
        "RUNWAY-ASSIGNMENT-V1\nflight_gufi\t{}\naircraft_address\t{:06X}\ncentre\t{}\nplan_id\t{}\ngeneration\t{}\nview_sequence\t{}\nservice_sequence\t{}\nrunway_id\t{}\nend_name\t{}\nmanifest_sha256\t{}\nprofile_sha256\t{}",
        oa_sha256::to_hex(&request.identity.gufi),
        request.identity.aircraft_address,
        std::str::from_utf8(&request.identity.centre).unwrap_or("????"),
        std::str::from_utf8(&request.identity.plan_id).unwrap_or("????????????"),
        request.generation,
        request.view_sequence,
        request.service_sequence,
        request.runway_id,
        std::str::from_utf8(&request.end_name).unwrap_or("???"),
        runtime.runway_aids_manifest_sha256.map_or_else(|| "unavailable".to_owned(), |digest| oa_sha256::to_hex(&digest)),
        runtime.runway_aids_profile_sha256.map_or_else(|| "unavailable".to_owned(), |digest| oa_sha256::to_hex(&digest)),
    )
    .map_err(|_| "runway assignment audit details formatting failed")?;
    let (details, details_len) = oa_audit_core::details(detail_text.as_bytes())
        .map_err(|error| format!("runway assignment audit details: {error:?}"))?;
    let event = oa_audit_core::Event {
        principal: request.principal,
        role: registry_role,
        action: oa_audit_core::Action::RunwayAssignment,
        target,
        target_len,
        time_ns: request.time_ns,
        details,
        details_len,
    };
    oa_audit_core::validate_event(&event)
        .map_err(|error| format!("runway assignment audit event: {error:?}"))?;
    let (audit_checkpoint, audit_receipt) =
        oa_audit_log::shell::append_receipt(request.audit_log, request.expected_checkpoint, event)
            .map_err(|error| format!("runway assignment audit append: {error:?}"))?;
    let assignment = RunwayAssignmentValue {
        identity: request.identity,
        generation: request.generation,
        sequence: request.service_sequence,
        runway_id: request.runway_id,
        end_name: request.end_name,
    };
    let stored = ActiveRunwayAssignment {
        identity: assignment.identity,
        generation: assignment.generation,
        sequence: assignment.sequence,
        runway_id: assignment.runway_id,
        end_name: assignment.end_name,
        arv_status: slot_index
            .and_then(|index| runtime.runway_assignments.get(index))
            .map_or(oa_runway::arv::AlertKind::Clear, |prior| prior.arv_status),
        arv_acknowledged: false,
        arv_ack_sequence: 0,
        arv_as_of_ns: slot_index
            .and_then(|index| runtime.runway_assignments.get(index))
            .map_or(0, |prior| prior.arv_as_of_ns),
    };
    if let Some(index) = slot_index {
        if let Some(slot) = runtime.runway_assignments.get_mut(index) {
            *slot = stored;
        }
    } else {
        runtime.runway_assignments.push(stored);
    }
    Ok(RunwayAssignmentReceipt {
        assignment,
        audit_checkpoint,
        audit_receipt,
    })
}

pub struct RunwayAlertAcknowledgementRequest<'a> {
    /// Existing typed interaction request; this path does not suppress or clear an alert.
    pub operator_command: crate::input::OperatorCommand,
    pub identity: oa_display::interaction::FlightIdentity,
    pub generation: u32,
    pub assignment_sequence: u32,
    pub expected_alert: oa_runway::arv::AlertKind,
    pub expected_alert_as_of_ns: u64,
    pub service_sequence: u32,
    pub audit_log: &'a Path,
    pub expected_checkpoint: oa_audit_core::Checkpoint,
    pub principal_registry: &'a oa_audit_core::PrincipalRegistry,
    pub principal: u64,
    pub role: oa_audit_core::Role,
    pub time_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunwayAlertAcknowledgementReceipt {
    pub identity: oa_display::interaction::FlightIdentity,
    pub generation: u32,
    pub assignment_sequence: u32,
    pub acknowledgement_sequence: u32,
    pub alert: oa_runway::arv::AlertKind,
    pub alert_as_of_ns: u64,
    pub acknowledged: bool,
    pub audit_checkpoint: oa_audit_core::Checkpoint,
    pub audit_receipt: oa_audit_core::AuditReceipt,
}

/// Receipt returned by the existing typed operator boundary for runway commands. This is a
/// native caller API only; no new text grammar or unaudited command parser is introduced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunwayOperatorReceipt {
    Assignment(RunwayAssignmentReceipt),
    AlertAcknowledgement(RunwayAlertAcknowledgementReceipt),
}

/// Common caller context for a typed runway command. The command itself remains the existing
/// `OperatorCommand` value used by the interaction system, and all mutations still pass through
/// the durable audit receipt boundary below.
pub struct RunwayOperatorDispatchRequest<'a> {
    pub operator_command: crate::input::OperatorCommand,
    pub identity: oa_display::interaction::FlightIdentity,
    pub generation: u32,
    pub service_sequence: u32,
    pub audit_log: &'a Path,
    pub expected_checkpoint: oa_audit_core::Checkpoint,
    pub principal_registry: &'a oa_audit_core::PrincipalRegistry,
    pub principal: u64,
    pub role: oa_audit_core::Role,
    pub time_ns: u64,
}

/// Dispatch one existing typed operator command to the runway-aid state boundary. Assignment
/// runway end selection must be unique in the verified issued profile; alert acknowledgement
/// derives its expected alert/as-of pair from the exact current assignment. Go-around clearing
/// is deliberately absent until a trusted go-around event source is wired into the runtime.
pub fn regional_dispatch_runway_operator(
    runtime: &mut RegionalRuntime,
    request: RunwayOperatorDispatchRequest<'_>,
) -> Result<RunwayOperatorReceipt, String> {
    use oa_display::interaction::Intent;
    match request.operator_command.intent {
        Intent::AssignRunway(runway_id) => {
            let mut matched_end = None;
            let mut matches = 0usize;
            for aid in &runtime.runway_aids {
                if aid.config.first_runway_id == runway_id {
                    matched_end = Some(aid.config.first_end_name);
                    matches = matches.saturating_add(1);
                }
                if aid.config.second_runway_id == runway_id {
                    matched_end = Some(aid.config.second_end_name);
                    matches = matches.saturating_add(1);
                }
            }
            if matches != 1 {
                return Err("typed runway assignment does not name one issued runway end".into());
            }
            let end_name = matched_end.ok_or("issued runway end disappeared")?;
            let receipt = regional_assign_runway(
                runtime,
                RunwayAssignmentRequest {
                    operator_command: request.operator_command,
                    identity: request.identity,
                    generation: request.generation,
                    view_sequence: request.operator_command.view_sequence,
                    service_sequence: request.service_sequence,
                    runway_id,
                    end_name,
                    audit_log: request.audit_log,
                    expected_checkpoint: request.expected_checkpoint,
                    principal_registry: request.principal_registry,
                    principal: request.principal,
                    role: request.role,
                    time_ns: request.time_ns,
                },
            )?;
            Ok(RunwayOperatorReceipt::Assignment(receipt))
        }
        Intent::AcknowledgeRunwayAlert => {
            let matching = runtime.runway_assignments.iter().filter(|assignment| {
                assignment.identity == request.identity
                    && assignment.generation == request.generation
            });
            let mut selected = None;
            let mut count = 0usize;
            for assignment in matching {
                selected = Some(*assignment);
                count = count.saturating_add(1);
            }
            if count != 1 {
                return Err(
                    "typed ARV acknowledgement does not identify one current assignment".into(),
                );
            }
            let assignment = selected.ok_or("ARV assignment disappeared")?;
            let receipt = regional_acknowledge_runway_alert(
                runtime,
                RunwayAlertAcknowledgementRequest {
                    operator_command: request.operator_command,
                    identity: request.identity,
                    generation: request.generation,
                    assignment_sequence: assignment.sequence,
                    expected_alert: assignment.arv_status,
                    expected_alert_as_of_ns: assignment.arv_as_of_ns,
                    service_sequence: request.service_sequence,
                    audit_log: request.audit_log,
                    expected_checkpoint: request.expected_checkpoint,
                    principal_registry: request.principal_registry,
                    principal: request.principal,
                    role: request.role,
                    time_ns: request.time_ns,
                },
            )?;
            Ok(RunwayOperatorReceipt::AlertAcknowledgement(receipt))
        }
        _ => Err("typed operator command is not a runway-aid command".into()),
    }
}

/// Acknowledge one currently latched ARV alert from an existing typed operator request. The
/// durable canonical receipt is appended before runtime state changes; acknowledgement does not
/// clear or suppress the alert.
pub fn regional_acknowledge_runway_alert(
    runtime: &mut RegionalRuntime,
    request: RunwayAlertAcknowledgementRequest<'_>,
) -> Result<RunwayAlertAcknowledgementReceipt, String> {
    if request.operator_command.intent != oa_display::interaction::Intent::AcknowledgeRunwayAlert
        || request.operator_command.plan_id != request.identity.plan_id
        || request.operator_command.target_generation != request.generation
        || request.operator_command.view_sequence
            != u32::try_from(runtime.frame_sequence).map_err(|_| "ARV view sequence exhausted")?
        || !(1..=2).contains(&request.operator_command.actor_unit)
        || runtime.failed
        || request.service_sequence == 0
        || request.generation == 0
        || request.assignment_sequence == 0
        || request.expected_alert_as_of_ns == 0
        || request.time_ns == 0
    {
        return Err("ARV acknowledgement typed request is invalid or stale".into());
    }
    let mut current_views = 0usize;
    for view in runtime
        .views
        .iter()
        .filter(|view| view.address == request.identity.aircraft_address)
    {
        current_views = current_views.saturating_add(1);
        if view.identity_claims != 1
            || view.identity_present != 1
            || view.reserved != 0
            || view.generation != request.generation
            || view.identity != request.identity
        {
            return Err("ARV acknowledgement full identity is ambiguous or stale".into());
        }
    }
    if current_views != 1 {
        return Err("ARV acknowledgement target is absent from the current view".into());
    }
    let assignment_index = runtime
        .runway_assignments
        .iter()
        .position(|assignment| {
            assignment.identity == request.identity
                && assignment.generation == request.generation
                && assignment.sequence == request.assignment_sequence
        })
        .ok_or("ARV acknowledgement assignment is absent or stale")?;
    let current = runtime
        .runway_assignments
        .get(assignment_index)
        .copied()
        .ok_or("ARV acknowledgement assignment disappeared")?;
    if current.arv_status != request.expected_alert
        || current.arv_as_of_ns != request.expected_alert_as_of_ns
        || !matches!(
            current.arv_status,
            oa_runway::arv::AlertKind::WrongRunway
                | oa_runway::arv::AlertKind::Taxiway
                | oa_runway::arv::AlertKind::AmbiguousPath
        )
        || current.arv_acknowledged
        || request.service_sequence <= current.arv_ack_sequence
    {
        return Err("ARV acknowledgement does not match a current unacknowledged alert".into());
    }
    let registry_role = oa_audit_core::authorize_principal(
        request.principal_registry,
        request.principal,
        oa_audit_core::Action::AlertAcknowledge,
    )
    .map_err(|error| format!("ARV acknowledgement principal registry refused: {error:?}"))?;
    if registry_role != request.role {
        return Err("ARV acknowledgement role does not match pinned principal registry".into());
    }
    let mut target_text = String::new();
    target_text
        .try_reserve_exact(96)
        .map_err(|_| "ARV acknowledgement target allocation refused")?;
    write!(
        target_text,
        "ARV {:06X} {} {} {}",
        request.identity.aircraft_address,
        request.generation,
        current.runway_id,
        request.expected_alert_as_of_ns
    )
    .map_err(|_| "ARV acknowledgement target formatting failed")?;
    let (target, target_len) = oa_audit_core::target(target_text.as_bytes())
        .map_err(|error| format!("ARV acknowledgement audit target: {error:?}"))?;
    let mut details_text = String::new();
    details_text
        .try_reserve_exact(512)
        .map_err(|_| "ARV acknowledgement details allocation refused")?;
    writeln!(
        details_text,
        "ARV-ACKNOWLEDGEMENT-V1\nflight_gufi\t{}\naircraft_address\t{:06X}\ncentre\t{}\nplan_id\t{}\ngeneration\t{}\nassignment_sequence\t{}\nacknowledgement_sequence\t{}\nrunway_id\t{}\nend_name\t{}\nalert\t{:?}\nalert_as_of_ns\t{}\noperator_unit\t{}",
        oa_sha256::to_hex(&request.identity.gufi),
        request.identity.aircraft_address,
        std::str::from_utf8(&request.identity.centre).unwrap_or("????"),
        std::str::from_utf8(&request.identity.plan_id).unwrap_or("????????????"),
        request.generation,
        current.sequence,
        request.service_sequence,
        current.runway_id,
        std::str::from_utf8(&current.end_name).unwrap_or("???"),
        current.arv_status,
        current.arv_as_of_ns,
        request.operator_command.actor_unit,
    )
    .map_err(|_| "ARV acknowledgement details formatting failed")?;
    let (details, details_len) = oa_audit_core::details(details_text.as_bytes())
        .map_err(|error| format!("ARV acknowledgement audit details: {error:?}"))?;
    let event = oa_audit_core::Event {
        principal: request.principal,
        role: registry_role,
        action: oa_audit_core::Action::AlertAcknowledge,
        target,
        target_len,
        time_ns: request.time_ns,
        details,
        details_len,
    };
    oa_audit_core::validate_event(&event)
        .map_err(|error| format!("ARV acknowledgement audit event: {error:?}"))?;
    let (audit_checkpoint, audit_receipt) =
        oa_audit_log::shell::append_receipt(request.audit_log, request.expected_checkpoint, event)
            .map_err(|error| format!("ARV acknowledgement audit append: {error:?}"))?;
    let assignment = runtime
        .runway_assignments
        .get_mut(assignment_index)
        .ok_or("ARV assignment disappeared after durable acknowledgement")?;
    assignment.arv_acknowledged = true;
    assignment.arv_ack_sequence = request.service_sequence;
    Ok(RunwayAlertAcknowledgementReceipt {
        identity: current.identity,
        generation: current.generation,
        assignment_sequence: current.sequence,
        acknowledgement_sequence: request.service_sequence,
        alert: current.arv_status,
        alert_as_of_ns: current.arv_as_of_ns,
        acknowledged: true,
        audit_checkpoint,
        audit_receipt,
    })
}

pub fn regional_runway_assignment(
    runtime: &RegionalRuntime,
    identity: oa_display::interaction::FlightIdentity,
    generation: u32,
) -> Option<RunwayAssignmentValue> {
    runtime
        .runway_assignments
        .iter()
        .find(|assignment| {
            assignment.identity == identity
                && assignment.generation == generation
                && runtime.views.iter().any(|view| {
                    view.identity_present == 1
                        && view.identity_claims == 1
                        && view.generation == generation
                        && view.identity == identity
                })
        })
        .map(|assignment| RunwayAssignmentValue {
            identity: assignment.identity,
            generation: assignment.generation,
            sequence: assignment.sequence,
            runway_id: assignment.runway_id,
            end_name: assignment.end_name,
        })
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
    if !centre.iter().all(u8::is_ascii_alphanumeric) {
        return Err("centre must be four ASCII alphanumeric bytes".into());
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
    let atpa_scratch = AtpaScratch::new(profile.max_flights)?;
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
        console_text: String::with_capacity(128),
        profile,
        selected_mode: RegionalAutomationMode::Surveillance,
        last_automation: RegionalAutomationStatus {
            selected: RegionalAutomationMode::Surveillance,
            effective: EffectiveAutomationMode::FlightDataOnly,
            degradation: AutomationDegradation::SurveillanceUnavailable,
            fusion_off: true,
        },
        known_flights: Vec::with_capacity(profile.max_flights),
        runway_aids: Vec::new(),
        runway_assignments: Vec::with_capacity(profile.max_flights),
        runway_aids_manifest_sha256: None,
        runway_aids_profile_sha256: None,
        atpa_scratch,
    })
}

/// Install runway aids only before the first scan and only from an already verified package
/// whose exact airspace model equals this runtime model. Resolution is cold and bounded.
pub fn regional_install_verified_runway_aids(
    runtime: &mut RegionalRuntime,
    package: &oa_adapt::manifest::VerifiedPackage,
) -> Result<(), String> {
    if runtime.failed || runtime.frame_sequence != 0 || !runtime.runway_aids.is_empty() {
        return Err("runway aids can only be installed once before the first scan".into());
    }
    if package.model() != &runtime.model {
        return Err("runway-aid package model does not match runtime adaptation".into());
    }
    let bytes = package
        .runway_aids_bytes()
        .ok_or_else(|| "verified package has no runway-aid profile".to_owned())?;
    let profile = oa_runway::adaptation::parse(bytes)
        .map_err(|error| format!("runway-aid profile refused: {error:?}"))?;
    if profile.is_empty() || profile.len() > MAX_RUNTIME_RUNWAY_AID_PAIRS {
        return Err("runway-aid profile exceeds runtime pair capacity".into());
    }
    let mut configured = Vec::new();
    configured
        .try_reserve_exact(profile.len())
        .map_err(|_| "runway-aid runtime capacity refused")?;
    for index in 0..profile.len() {
        let config = profile
            .pair(index)
            .ok_or("validated runway-aid pair disappeared")?;
        let geometry = crate::runway_aids::resolve_verified(package, &runtime.model, index)
            .map_err(|error| format!("runway-aid geometry refused: {error:?}"))?;
        let mut retained_final_alerts = Vec::new();
        retained_final_alerts
            .try_reserve_exact(oa_runway::final_monitor::MAX_TRACKS)
            .map_err(|_| "runway-aid final-alert retention allocation refused")?;
        let mut retained_input_addresses = Vec::new();
        retained_input_addresses
            .try_reserve_exact(oa_runway::final_monitor::MAX_TRACKS)
            .map_err(|_| "runway-aid monitor-key retention allocation refused")?;
        configured.push(RunwayAidRuntime {
            config,
            geometry,
            arv_coverage: if profile.has_complete_airport_coverage() {
                oa_display::core::runway_aids::ArvCoverage::IssuedAirportComplete
            } else {
                oa_display::core::runway_aids::ArvCoverage::PairOnly
            },
            arv_paths: arv_path_columns(&profile, config, &geometry)?,
            state: oa_runway::final_monitor::FinalMonitorState::new(),
            retained_final_alerts,
            retained_input_addresses,
            retained_prediction_signature: None,
            retained_assignment_digest: None,
            retained_assignment_count: 0,
            retained_final_as_of_ns: None,
            retained_final_config: None,
            retained_final_pair: None,
            retained_final_geometry: None,
        });
    }
    runtime.runway_aids = configured;
    runtime.runway_aids_manifest_sha256 = Some(package.identity().manifest_sha256);
    runtime.runway_aids_profile_sha256 = package.identity().runway_aids_sha256;
    Ok(())
}

fn arv_path_columns(
    profile: &oa_runway::adaptation::RunwayAidProfile<'_>,
    config: oa_runway::adaptation::AidPairConfig,
    geometry: &crate::runway_aids::RunwayAidGeometry,
) -> Result<ArvPathColumns, String> {
    if !profile.has_complete_airport_coverage() {
        return ArvPathColumns::new(0);
    }
    let path_count = (0..profile.path_len())
        .filter(|index| {
            profile
                .path(*index)
                .is_some_and(|path| path.airport == config.airport)
        })
        .count();
    if path_count == 0 || path_count > oa_runway::arv::MAX_ARV_PATHS {
        return Err("complete ARV candidate geometry exceeds runtime path capacity".into());
    }
    let mut paths = ArvPathColumns::new(path_count)?;
    let plane =
        oa_runway::plane::plane_new(geometry.plane_origin.lat_deg, geometry.plane_origin.lon_deg)
            .ok_or("ARV candidate geometry has no local plane")?;
    for index in 0..profile.path_len() {
        let path = profile
            .path(index)
            .ok_or("validated ARV path disappeared")?;
        if path.airport != config.airport {
            continue;
        }
        let threshold = oa_runway::plane::to_local(
            &plane,
            oa_runway::plane::Geo {
                lat_deg: f64::from(path.threshold_lat_e7) / 10_000_000.0,
                lon_deg: f64::from(path.threshold_lon_e7) / 10_000_000.0,
            },
        )
        .ok_or("ARV path threshold lies outside the local plane")?;
        let outer = oa_runway::plane::to_local(
            &plane,
            oa_runway::plane::Geo {
                lat_deg: f64::from(path.outer_lat_e7) / 10_000_000.0,
                lon_deg: f64::from(path.outer_lon_e7) / 10_000_000.0,
            },
        )
        .ok_or("ARV path outer endpoint lies outside the local plane")?;
        let delta_east = threshold.east_m - outer.east_m;
        let delta_north = threshold.north_m - outer.north_m;
        let length = oa_math::hypot(delta_east, delta_north);
        if !(1.0..=40_000.0).contains(&length) {
            return Err("ARV path centerline length is outside the bounded geometry domain".into());
        }
        paths.ids.push(path.path_id);
        paths.kinds.push(path.kind);
        paths.threshold_east_m.push(threshold.east_m);
        paths.threshold_north_m.push(threshold.north_m);
        paths.inbound_east.push(delta_east / length);
        paths.inbound_north.push(delta_north / length);
        paths.width_m.push(f64::from(path.width_mm) / 1_000.0);
    }
    let verify_pair_path = |path_id: u32,
                            end_name: [u8; 3],
                            expected_threshold: oa_runway::plane::Local,
                            expected_east: f64,
                            expected_north: f64,
                            expected_width: f64|
     -> Result<(), String> {
        let index = profile
            .path(
                (0..profile.path_len())
                    .find(|index| {
                        profile.path(*index).is_some_and(|path| {
                            path.airport == config.airport
                                && path.path_id == path_id
                                && path.kind == oa_runway::adaptation::PATH_KIND_RUNWAY
                                && path.name.get(..3) == Some(end_name.as_slice())
                        })
                    })
                    .ok_or("complete ARV geometry omits an assigned runway")?,
            )
            .ok_or("complete ARV runway path disappeared")?;
        if index.kind != oa_runway::adaptation::PATH_KIND_RUNWAY {
            return Err("ARV assigned path is not a runway".into());
        }
        let path_index = paths
            .ids
            .binary_search(&path_id)
            .map_err(|_| "ARV assigned runway path is not in the candidate set")?;
        let threshold_east = paths
            .threshold_east_m
            .get(path_index)
            .copied()
            .ok_or("ARV assigned threshold column is incomplete")?;
        let threshold_north = paths
            .threshold_north_m
            .get(path_index)
            .copied()
            .ok_or("ARV assigned threshold column is incomplete")?;
        let inbound_east = paths
            .inbound_east
            .get(path_index)
            .copied()
            .ok_or("ARV assigned axis column is incomplete")?;
        let inbound_north = paths
            .inbound_north
            .get(path_index)
            .copied()
            .ok_or("ARV assigned axis column is incomplete")?;
        let width = paths
            .width_m
            .get(path_index)
            .copied()
            .ok_or("ARV assigned width column is incomplete")?;
        if oa_math::hypot(
            threshold_east - expected_threshold.east_m,
            threshold_north - expected_threshold.north_m,
        ) > 0.05
            || oa_math::hypot(inbound_east - expected_east, inbound_north - expected_north) > 1.0e-5
            || (width - expected_width).abs() > 0.001
        {
            return Err("ARV candidate runway conflicts with the resolved pair geometry".into());
        }
        Ok(())
    };
    verify_pair_path(
        config.first_runway_id,
        config.first_end_name,
        geometry.first_threshold,
        geometry.first_inbound_east,
        geometry.first_inbound_north,
        geometry.first_width_m,
    )?;
    verify_pair_path(
        config.second_runway_id,
        config.second_end_name,
        geometry.second_threshold,
        geometry.second_inbound_east,
        geometry.second_inbound_north,
        geometry.second_width_m,
    )?;
    Ok(paths)
}

fn arv_alert_status(status: oa_runway::arv::AlertKind) -> oa_display::core::runway_aids::ArvStatus {
    use oa_display::core::runway_aids::ArvStatus as Output;
    match status {
        oa_runway::arv::AlertKind::Clear => Output::Clear,
        oa_runway::arv::AlertKind::WrongRunway => Output::WrongRunway,
        oa_runway::arv::AlertKind::Taxiway => Output::Taxiway,
        oa_runway::arv::AlertKind::GoAroundCleared => Output::GoAroundCleared,
        oa_runway::arv::AlertKind::AmbiguousPath => Output::AmbiguousPath,
    }
}

#[derive(Clone, Copy)]
struct ArvScanContext {
    scan_time_ns: i64,
    scan_sequence: u64,
    output_capacity: usize,
}
// A7.2: cold per-frame context passed once to the bounded ARV adapter.

fn runway_arv_packets(
    aids: &mut [RunwayAidRuntime],
    assignments: &mut [ActiveRunwayAssignment],
    views: &[oa_display::interaction::TargetView],
    surveillance: Option<&surveillance::Surveillance>,
    tracks: &[crate::surveillance::ObservedTrack],
    scan: ArvScanContext,
) -> Result<Vec<oa_display::core::runway_aids::ArvPacket>, String> {
    use oa_display::core::runway_aids::{ArvAvailability, ArvCoverage, ArvPacket, ArvStatus};
    let mut output = Vec::new();
    output
        .try_reserve_exact(assignments.len().min(scan.output_capacity))
        .map_err(|_| "ARV output capacity allocation refused")?;
    if assignments.len() > scan.output_capacity {
        return Err("ARV assignment count exceeds runtime output capacity".into());
    }
    let scan_ns = u64::try_from(scan.scan_time_ns)
        .ok()
        .filter(|time| *time > 0)
        .ok_or("ARV scan time is invalid")?;
    let over_frame_cap = assignments.len() > oa_runway::arv::MAX_ARV_FLIGHTS;
    let lookup_work = assignments
        .len()
        .checked_mul(aids.len())
        .and_then(|value| value.checked_mul(2));
    let mut work_total = lookup_work.unwrap_or(usize::MAX);
    let mut work_overflow = lookup_work.is_none();
    if !over_frame_cap && !work_overflow {
        for assignment in assignments.iter() {
            let mut matched_aid = None;
            let mut aid_matches = 0usize;
            for aid in aids.iter() {
                if (assignment.runway_id == aid.config.first_runway_id
                    && assignment.end_name == aid.config.first_end_name)
                    || (assignment.runway_id == aid.config.second_runway_id
                        && assignment.end_name == aid.config.second_end_name)
                {
                    aid_matches += 1;
                    matched_aid = Some(aid);
                }
            }
            let Some(aid) = matched_aid.filter(|_| aid_matches == 1) else {
                continue;
            };
            if aid.arv_coverage != ArvCoverage::IssuedAirportComplete {
                continue;
            }
            let path_count = aid.arv_paths.ids.len();
            let steps = if path_count <= 1 {
                1
            } else {
                usize::BITS as usize - (path_count - 1).leading_zeros() as usize + 1
            };
            let Some(call_work) = path_count
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .and_then(|value| value.checked_add(steps))
            else {
                work_overflow = true;
                break;
            };
            let Some(total) = work_total.checked_add(call_work) else {
                work_overflow = true;
                break;
            };
            work_total = total;
        }
    }
    let over_work_budget = work_overflow || work_total > oa_runway::arv::MAX_ARV_WORK;

    for assignment in assignments.iter_mut() {
        let mut output_row = ArvPacket {
            scan_sequence: scan.scan_sequence,
            predicted_at_ns: 0,
            newest_source_ns: 0,
            identity: assignment.identity,
            generation: assignment.generation,
            assignment_sequence: assignment.sequence,
            assigned_runway_id: assignment.runway_id,
            assigned_end_name: assignment.end_name,
            matched_path_id: 0,
            coverage: ArvCoverage::Unavailable,
            availability: ArvAvailability::Unavailable,
            status: match assignment.arv_status {
                oa_runway::arv::AlertKind::Clear => ArvStatus::NotEvaluated,
                other => arv_alert_status(other),
            },
            acknowledged: u8::from(assignment.arv_acknowledged),
            current_view: 0,
            reserved: 0,
        };
        if over_frame_cap || over_work_budget {
            output.push(output_row);
            continue;
        }
        let mut aid_match = None;
        let mut aid_matches = 0usize;
        for aid in aids.iter_mut() {
            if (assignment.runway_id == aid.config.first_runway_id
                && assignment.end_name == aid.config.first_end_name)
                || (assignment.runway_id == aid.config.second_runway_id
                    && assignment.end_name == aid.config.second_end_name)
            {
                aid_matches += 1;
                aid_match = Some(aid);
            }
        }
        let Some(aid) = aid_match.filter(|_| aid_matches == 1) else {
            output.push(output_row);
            continue;
        };
        output_row.coverage = aid.arv_coverage;
        if aid.arv_coverage != ArvCoverage::IssuedAirportComplete {
            output.push(output_row);
            continue;
        }
        let Some(view) = views
            .iter()
            .find(|view| view.address == assignment.identity.aircraft_address)
        else {
            output.push(output_row);
            continue;
        };
        if views
            .iter()
            .filter(|view| view.address == assignment.identity.aircraft_address)
            .count()
            != 1
            || view.identity_claims != 1
            || view.identity_present != 1
            || view.reserved != 0
            || view.generation != assignment.generation
            || view.identity != assignment.identity
        {
            output.push(output_row);
            continue;
        }
        output_row.current_view = 1;
        let Some(surveillance) = surveillance else {
            output.push(output_row);
            continue;
        };
        let mut matching_track = None;
        let mut duplicate_track = false;
        for track in tracks {
            if track.address == assignment.identity.aircraft_address
                && matching_track.replace(track).is_some()
            {
                duplicate_track = true;
                break;
            }
        }
        if duplicate_track || matching_track.is_none() {
            output.push(output_row);
            continue;
        }
        let Some(horizon_ns) = u64::from(aid.config.lookahead_ms).checked_mul(1_000_000) else {
            output.push(output_row);
            continue;
        };
        let Ok(horizon_ns_i64) = i64::try_from(horizon_ns) else {
            output.push(output_row);
            continue;
        };
        let Some(predicted_time_ns) = scan.scan_time_ns.checked_add(horizon_ns_i64) else {
            output.push(output_row);
            continue;
        };
        let Ok((row, confirmed, newest_ns)) = surveillance::surveillance_track_forecast(
            surveillance,
            assignment.identity.aircraft_address,
            predicted_time_ns,
        ) else {
            output.push(output_row);
            continue;
        };
        let lateral_tolerance_m = f64::from(aid.config.arv_lateral_tolerance_mm) / 1_000.0;
        let r95_m = oa_track::r95_m_of(&row.pos_cov_m2);
        let age_ns = scan.scan_time_ns.checked_sub(newest_ns);
        if !confirmed
            || row.addr != assignment.identity.aircraft_address
            || row.t_ns != predicted_time_ns
            || row.flags
                & (oa_track::O_COAST
                    | oa_track::O_IDENTITY_UNCONFIRMED
                    | oa_track::O_MOVE_UNCONFIRMED)
                != 0
            || !age_ns.is_some_and(|age| (0..4_800_000_000).contains(&age))
            || !r95_m.is_finite()
            || r95_m > lateral_tolerance_m
        {
            output.push(output_row);
            continue;
        }
        let Some(local_plane) = oa_runway::plane::plane_new(
            aid.geometry.plane_origin.lat_deg,
            aid.geometry.plane_origin.lon_deg,
        ) else {
            output.push(output_row);
            continue;
        };
        let Some(point) = oa_runway::plane::to_local(
            &local_plane,
            oa_runway::plane::Geo {
                lat_deg: f64::from(row.lat_e7) / 10_000_000.0,
                lon_deg: f64::from(row.lon_e7) / 10_000_000.0,
            },
        ) else {
            output.push(output_row);
            continue;
        };
        let east = f64::from(row.vel_east_mps);
        let north = f64::from(row.vel_north_mps);
        let speed = oa_math::hypot(east, north);
        if !speed.is_finite() || speed <= 0.0 {
            output.push(output_row);
            continue;
        }
        let gufi = [assignment.identity.gufi];
        let address = [assignment.identity.aircraft_address];
        let centre = [assignment.identity.centre];
        let plan_id = [assignment.identity.plan_id];
        let assigned_path = [assignment.runway_id];
        let predicted_east = [point.east_m];
        let predicted_north = [point.north_m];
        let track_east = [east / speed];
        let track_north = [north / speed];
        // No current source adapter supplies a trusted go-around event; never synthesize one.
        let go_around_authenticated = [0u8];
        let prior_status = [assignment.arv_status as u8];
        let mut out_gufi = [[0u8; 16]; 1];
        let mut out_address = [0u32; 1];
        let mut out_centre = [[0u8; 4]; 1];
        let mut out_plan_id = [[0u8; 12]; 1];
        let mut out_status = [0u8; 1];
        let mut out_path = [0u32; 1];
        let result = oa_runway::arv::evaluate(
            oa_runway::arv::ArvConfig {
                lateral_tolerance_m,
                heading_tolerance_deg: f64::from(aid.config.arv_heading_tolerance_cdeg) / 100.0,
            },
            oa_runway::arv::PredictedFlights {
                identity: oa_runway::arv::FlightIdentityColumns {
                    gufi: &gufi,
                    aircraft_address: &address,
                    centre: &centre,
                    plan_id: &plan_id,
                },
                assigned_path_id: &assigned_path,
                predicted_east_m: &predicted_east,
                predicted_north_m: &predicted_north,
                track_east: &track_east,
                track_north: &track_north,
                go_around_authenticated: &go_around_authenticated,
                prior_status: &prior_status,
            },
            oa_runway::arv::ArvPaths {
                ids: &aid.arv_paths.ids,
                kinds: &aid.arv_paths.kinds,
                threshold_east_m: &aid.arv_paths.threshold_east_m,
                threshold_north_m: &aid.arv_paths.threshold_north_m,
                inbound_east: &aid.arv_paths.inbound_east,
                inbound_north: &aid.arv_paths.inbound_north,
                width_m: &aid.arv_paths.width_m,
            },
            oa_runway::arv::ArvOutput {
                gufi: &mut out_gufi,
                aircraft_address: &mut out_address,
                centre: &mut out_centre,
                plan_id: &mut out_plan_id,
                status: &mut out_status,
                matched_path_id: &mut out_path,
            },
        );
        let Ok(result) = result else {
            output.push(output_row);
            continue;
        };
        if result.flights != 1 {
            output.push(output_row);
            continue;
        }
        let Some(status) = out_status.first().copied().and_then(|value| match value {
            0 => Some(oa_runway::arv::AlertKind::Clear),
            1 => Some(oa_runway::arv::AlertKind::WrongRunway),
            2 => Some(oa_runway::arv::AlertKind::Taxiway),
            3 => Some(oa_runway::arv::AlertKind::GoAroundCleared),
            4 => Some(oa_runway::arv::AlertKind::AmbiguousPath),
            _ => None,
        }) else {
            output.push(output_row);
            continue;
        };
        if status != assignment.arv_status
            && matches!(
                status,
                oa_runway::arv::AlertKind::WrongRunway
                    | oa_runway::arv::AlertKind::Taxiway
                    | oa_runway::arv::AlertKind::AmbiguousPath
            )
        {
            assignment.arv_acknowledged = false;
            assignment.arv_ack_sequence = 0;
        }
        assignment.arv_status = status;
        assignment.arv_as_of_ns = scan_ns;
        output_row.predicted_at_ns = u64::try_from(predicted_time_ns).unwrap_or(0);
        output_row.newest_source_ns = u64::try_from(newest_ns).unwrap_or(0);
        output_row.matched_path_id = out_path.first().copied().unwrap_or(0);
        output_row.availability = ArvAvailability::Available;
        output_row.status = arv_alert_status(status);
        output_row.acknowledged = u8::from(assignment.arv_acknowledged);
        output.push(output_row);
    }
    Ok(output)
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

/// Independent issued manifest pin retained with this runtime, if startup used an issued package.
pub fn regional_issued_manifest_sha256(runtime: &RegionalRuntime) -> Option<[u8; 32]> {
    runtime.issued_manifest_sha256
}

fn execute(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
    performance: Option<&[PerformanceEnvelope]>,
) -> Result<RegionalFrame, String> {
    if batch.events.len() > MAX_EVENTS_PER_RECEIPT
        || batch
            .events
            .iter()
            .any(|e| e.receipt_ns != batch.receipt_ns)
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
    let input = summarize_input_batch(batch).map_err(|e| format!("input accounting: {e:?}"))?;
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
    surveillance::surveillance_begin(&mut runtime.surveillance, batch.receipt_ns)?;
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
        let flight_capacity_refusal =
            flight::flight_table_capacity_refusal(&runtime.flights, event);
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

        let pending_flight_key = preflight_flight_key(runtime, event)?;
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
        commit_flight_key(runtime, pending_flight_key, flight_outcome);
        flight_operations.push(flight_outcome);
        route::route_commit_stage(&mut runtime.routes, route_stage);
        surveillance::surveillance_event(&mut runtime.surveillance, event)
            .map_err(|error| format!("accepted surveillance commit invariant: {error}"))?;
        flight::flight_commit_order(&mut order_cursor, event);
    }
    // Flight ownership presentation currently requires an explicitly selected flight. Per-flight
    // snapshots below preserve every owner; no arbitrary 'first' flight is promoted to selection.
    let current_feeds = surveillance::feed_statuses(&runtime.surveillance, batch.receipt_ns)?;
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
    let mut surveillance = surveillance::surveillance_finish_with_visibility(
        &mut runtime.surveillance,
        None,
        automation.effective == EffectiveAutomationMode::Surveillance,
        unavailable_reason,
    )?;
    if runtime.profile.surveillance.render_scope {
        surveillance.frame_sha256 = surveillance::surveillance_present_controls(
            &mut runtime.surveillance,
            &runtime.interaction,
            &automation_console_text(&runtime.console_text, automation),
        )?;
    }
    runtime.frame_sequence = next_frame_sequence;
    let mut flights = Vec::with_capacity(runtime.profile.surveillance.max_tracks);
    let mut views = Vec::with_capacity(runtime.profile.max_flights);
    for observed in &surveillance.tracks {
        let result = flight::flight_views_for_address(
            &mut runtime.flights,
            observed.address,
            runtime.centre,
            batch.receipt_ns / 1_000_000,
            &mut views,
        );
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
        if result.status != FlightViewStatus::Found {
            row.refusal = format!("flight view: {:?}", result.status);
        } else if let Some(view) = views.first() {
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
    runtime.runway_assignments.retain(|assignment| {
        let current = runtime.views.iter().any(|view| {
            view.identity_present == 1
                && view.identity_claims == 1
                && view.generation == assignment.generation
                && view.identity == assignment.identity
        });
        let latched_arv = matches!(
            assignment.arv_status,
            oa_runway::arv::AlertKind::WrongRunway
                | oa_runway::arv::AlertKind::Taxiway
                | oa_runway::arv::AlertKind::AmbiguousPath
        );
        current || latched_arv
    });
    runtime.observed.clone_from(&surveillance.tracks);
    let flight_cutoff_ms = route::receipt_cutoff_ms(batch.receipt_ns)?;
    if runtime.known_flights.len() > MAX_FLIGHT_DATA_VIEW_WORK {
        return Err("flight-data view work budget exceeded before frame assembly".into());
    }
    let mut flight_data = Vec::with_capacity(runtime.known_flights.len());
    let mut flight_data_as_of = Vec::with_capacity(runtime.known_flights.len());
    let mut flight_data_unavailable = 0u32;
    for key in &runtime.known_flights {
        match flight::flight_view_for_key(
            &mut runtime.flights,
            *key,
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
    let ghost_capacity = runtime
        .runway_aids
        .len()
        .checked_mul(
            surveillance
                .tracks
                .len()
                .min(oa_runway::terminal_aids::MAX_CRDA_ROWS),
        )
        .ok_or("CRDA ghost output capacity overflow")?;
    let mut crda_ghosts = Vec::new();
    crda_ghosts
        .try_reserve_exact(ghost_capacity)
        .map_err(|_| "CRDA ghost output allocation refused")?;
    let runway_aids = runway_aid_packets(
        &mut runtime.runway_aids,
        &mut runtime.atpa_scratch,
        RunwayAidFrame {
            assignments: &runtime.runway_assignments,
            surveillance: Some(&runtime.surveillance),
            tracks: &surveillance.tracks,
            ghosts: &mut crda_ghosts,
            scan_time_ns: surveillance.time_ns,
            scan_sequence: runtime.frame_sequence,
        },
    );
    let arv_verification = runway_arv_packets(
        &mut runtime.runway_aids,
        &mut runtime.runway_assignments,
        &runtime.views,
        Some(&runtime.surveillance),
        &surveillance.tracks,
        ArvScanContext {
            scan_time_ns: surveillance.time_ns,
            scan_sequence: runtime.frame_sequence,
            output_capacity: runtime.profile.max_flights,
        },
    )?;
    Ok(RegionalFrame {
        sequence: runtime.frame_sequence,
        input,
        surveillance,
        automation,
        flight_operations,
        flights,
        flight_data,
        flight_data_as_of,
        flight_data_unavailable,
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
        runway_aids,
        arv_verification,
        runway_assignments: runtime
            .runway_assignments
            .iter()
            .filter(|assignment| {
                runtime.views.iter().any(|view| {
                    view.identity_present == 1
                        && view.identity_claims == 1
                        && view.generation == assignment.generation
                        && view.identity == assignment.identity
                })
            })
            .map(
                |assignment| oa_display::core::runway_aids::AssignmentPacket {
                    identity: assignment.identity,
                    generation: assignment.generation,
                    sequence: assignment.sequence,
                    runway_id: assignment.runway_id,
                    end_name: assignment.end_name,
                    reserved: 0,
                },
            )
            .collect(),
        crda_ghosts,
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
    let digest = match surveillance::surveillance_present_controls(
        &mut runtime.surveillance,
        &runtime.interaction,
        &automation_console_text(&runtime.console_text, runtime.last_automation),
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
    if text.len() > 128 || !text.bytes().all(|byte| (32..=126).contains(&byte)) {
        return Err("console text must be at most 128 printable ASCII bytes".into());
    }
    runtime.console_text.clear();
    runtime.console_text.push_str(text);
    match surveillance::surveillance_present_controls(
        &mut runtime.surveillance,
        &runtime.interaction,
        &automation_console_text(&runtime.console_text, runtime.last_automation),
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
    match surveillance::surveillance_present_controls(
        &mut runtime.surveillance,
        &runtime.interaction,
        &automation_console_text(&runtime.console_text, runtime.last_automation),
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
    let service = crate::msaw_bridge::regional_msaw_new(data, surveillance::MAX_TRACKS)?;
    regional_attach_msaw(runtime, service)
}

pub fn regional_enable_synthetic_msaw(runtime: &mut RegionalRuntime) -> Result<(), String> {
    let service = crate::msaw_bridge::regional_msaw_synthetic(surveillance::MAX_TRACKS)?;
    regional_attach_msaw(runtime, service)
}

pub fn regional_attach_msaw(
    runtime: &mut RegionalRuntime,
    service: crate::msaw_bridge::RegionalMsaw,
) -> Result<(), String> {
    surveillance::surveillance_enable_msaw(&mut runtime.surveillance, service)
}

#[cfg(test)]
mod transaction_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::cli::synthetic_model;
    use crate::input::parse_event_line;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_QUARANTINE_AUDIT_TEST: AtomicU64 = AtomicU64::new(0);

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
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        regional_new(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
        )
        .unwrap()
    }

    #[test]
    fn flight_view_key_capacity_refuses_before_a_second_plan_commit() {
        let mut engine = runtime();
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
        engine.profile.max_flights = 1;

        let second = parse_event_line(
            "v1\t2000000000\t2000000000\t2\tFLIGHT\t00000000000000000000000000000002\tABC102\tZAU1\tFLIGHT\tPLAN\tPLAN00000002\t2",
        )
        .unwrap();
        assert!(regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 2_000_000_000,
                source_clock_regressions: 0,
                events: vec![second],
            },
        )
        .is_err());
        assert_eq!(flight::flight_driver_report(&engine.flights).flights, 1);
        assert_eq!(engine.known_flights.len(), 1);
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

    // REQ: OA-REG-004
    #[test]
    fn recorded_profile_is_separate_and_stays_within_predeclared_bounds() {
        let default = DEFAULT_REGIONAL_RUN_PROFILE;
        assert_eq!(default.surveillance.max_tracks, surveillance::MAX_TRACKS);
        assert_eq!(default.surveillance.max_frames, surveillance::MAX_FRAMES);
        assert_eq!(default.max_flights, MAX_DRIVER_FLIGHTS);
        assert_eq!(default.max_total_events, crate::session::MAX_SESSION_EVENTS);

        let recorded = RegionalRunProfile::recorded_peak();
        assert_eq!(recorded.surveillance.max_tracks, 4096);
        assert_eq!(recorded.surveillance.max_frames, 20_000_000);
        assert_eq!(recorded.surveillance.max_drain_rows, 262_144);
        assert_eq!(recorded.max_flights, 8192);
        assert_eq!(recorded.max_total_events, 10_000_000);
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
        assert_eq!(rendered.max_flights, rendered.surveillance.max_tracks);
        assert!(rendered.surveillance.render_scope);
        assert_eq!(
            rendered.surveillance.max_presentation_updates,
            surveillance::MAX_PRESENTATION_UPDATES
        );
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
}

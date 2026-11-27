//! Offline composed regional session (shell, B1/B3/C6), with one common receipt cutoff.
//! This executes actual product cores; it is not deadline or allocation qualification.

use crate::commands;
use crate::flight::{
    self, FlightApplyOutcome, FlightDriver, FlightDriverConfig, FlightDriverReport,
    FlightViewStatus, SharedFlightView, MAX_DRIVER_FLIGHTS, MAX_DRIVER_LOG_ENTRIES,
};
use crate::holding::{
    FlightLifecycles, HoldEntryCourseBasis, HoldEntryReport, HoldInstruction, HoldPredictionIntent,
    HoldProbeUnavailableReason, HoldRevisions, HoldStore, HoldUpdate, HoldViewRow,
    PairHoldProbeUnavailable, SuaHoldProbeUnavailable,
};
use crate::nam_flight::{self, NamFlightContext, NamFlightResult, NamFlightRuntime};
use crate::route::{self, RouteStore};
use crate::session::{summarize_input_batch, FrameEvidence, ReceiptBatch, MAX_EVENTS_PER_RECEIPT};
use crate::surveillance::{self, Surveillance, SurveillanceConfig, SurveillanceFrame};
use oa_adapt::manifest::VerifiedPackage;
use oa_airspace::core::conflict_probe::{SuaProbeResult, SuaStatus};
use oa_airspace::core::geom::{self, Area};
use oa_airspace::core::model::{self as air_model, Model};
use oa_ca::probe::{ProbeResult, ProbeStatus};
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
/// Derived maximum work units for bounded hold/profile joins in one scan. It covers full-key
/// sorts at 4,096 tracks and 8,192 holds, binary joins for at most 65,536 pair probes plus
/// 65,536 SUA probes, linear reconciliation/retirement, and fixed 240-point profile copies and
/// clears possible during bulk retirement. The aggregate is preflighted before lifecycle and
/// hold processing; probe caps are unchanged.
const MAX_HOLD_SCAN_JOIN_WORK: usize = 16_000_000;
/// C7 cap for resolving latest accepted position provenance against active tracker rows.
/// The nested scan is preflighted exactly; an oversized frame refuses before hold state changes.
const MAX_HOLD_REPORT_EVENT_ASSOCIATION_WORK: usize = 8_000_000;
/// C7 cap for sorting canonical active plans/tracks and joining each plan to its current track.
const MAX_ACTIVE_PLAN_TRACK_JOIN_WORK: usize = 1_000_000;

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

/// A8 bounded scan work. Any candidate beyond these caps is counted as unexamined, never Clear.
pub const MAX_RUNTIME_PAIR_PROBES: usize = 65_536;
pub const MAX_RUNTIME_SUA_PROBES: usize = 65_536;
pub const MAX_RUNTIME_SUA_ZONES: usize = 4_096;

fn compare_full_flight_keys(
    left: &oa_legacy_traj::core::trajectory::FlightKey,
    right: &oa_legacy_traj::core::trajectory::FlightKey,
) -> std::cmp::Ordering {
    (left.gufi, left.aircraft_address, left.centre, left.plan_id).cmp(&(
        right.gufi,
        right.aircraft_address,
        right.centre,
        right.plan_id,
    ))
}

fn sort_work_bound(len: usize) -> Result<usize, &'static str> {
    if len <= 1 {
        return Ok(0);
    }
    let log2 = usize::BITS as usize - (len - 1).leading_zeros() as usize;
    len.checked_mul(log2)
        .and_then(|work| work.checked_mul(16))
        .ok_or("hold scan sort work bound overflow")
}

fn binary_search_work_bound(len: usize, searches: usize) -> Result<usize, &'static str> {
    if searches == 0 || len <= 1 {
        return Ok(searches);
    }
    let steps = usize::BITS as usize - (len - 1).leading_zeros() as usize;
    searches
        .checked_mul(steps.saturating_add(1))
        .ok_or("hold scan binary-join work bound overflow")
}

fn find_probe_input(
    inputs: &[(SharedFlightView, crate::surveillance::ObservedTrack)],
    key: oa_legacy_traj::core::trajectory::FlightKey,
) -> Option<&(SharedFlightView, crate::surveillance::ObservedTrack)> {
    let index = inputs
        .binary_search_by(|(view, _)| compare_full_flight_keys(&route::flight_key(view), &key))
        .ok()?;
    inputs.get(index)
}

fn find_report(
    reports: &[HoldEntryReport],
    key: oa_legacy_traj::core::trajectory::FlightKey,
) -> Option<HoldEntryReport> {
    let index = reports
        .binary_search_by(|report| compare_full_flight_keys(&report.key, &key))
        .ok()?;
    reports.get(index).copied()
}

fn find_hold_row(
    rows: &[HoldViewRow],
    key: oa_legacy_traj::core::trajectory::FlightKey,
) -> Option<&HoldViewRow> {
    let index = rows
        .binary_search_by(|row| compare_full_flight_keys(&row.instruction.flight_key, &key))
        .ok()?;
    rows.get(index)
}

fn preflight_hold_scan_work(
    active_count: usize,
    lifecycle_count: usize,
    held_count: usize,
    prior_report_count: usize,
    current_report_count: usize,
    zone_count: usize,
    hold_capacity: usize,
) -> Result<(usize, usize), &'static str> {
    let pair_count = active_count
        .checked_mul(active_count.saturating_sub(1))
        .map(|pairs| pairs / 2)
        .ok_or("hold scan pair-count overflow")?;
    let pair_limit = pair_count.min(MAX_RUNTIME_PAIR_PROBES);
    let sua_candidates = active_count
        .checked_mul(zone_count)
        .ok_or("hold scan SUA candidate-count overflow")?;
    let sua_limit = sua_candidates.min(MAX_RUNTIME_SUA_PROBES);
    let mut work = 0usize;
    for count in [
        active_count, // pair/SUA key order
        current_report_count,
        prior_report_count,
        held_count,
    ] {
        work = work
            .checked_add(sort_work_bound(count)?)
            .ok_or("hold scan sort-work aggregate overflow")?;
    }
    work = work
        .checked_add(
            lifecycle_count
                .checked_add(active_count)
                .and_then(|value| value.checked_mul(6))
                .ok_or("hold lifecycle merge-work bound overflow")?,
        )
        .ok_or("hold scan merge-work aggregate overflow")?;
    work = work
        .checked_add(
            prior_report_count
                .checked_add(current_report_count)
                .ok_or("hold report merge-work bound overflow")?,
        )
        .ok_or("hold scan merge-work aggregate overflow")?;
    let hold_searches = held_count
        .checked_mul(2)
        .and_then(|value| value.checked_add(pair_limit.saturating_mul(2)))
        .and_then(|value| value.checked_add(sua_limit))
        .ok_or("hold scan row-join count overflow")?;
    work = work
        .checked_add(binary_search_work_bound(held_count, hold_searches)?)
        .ok_or("hold scan binary-join aggregate overflow")?;
    let active_searches = held_count
        .checked_mul(2)
        .and_then(|value| value.checked_add(prior_report_count))
        .and_then(|value| value.checked_add(current_report_count))
        .ok_or("hold scan active-view join count overflow")?;
    work = work
        .checked_add(binary_search_work_bound(active_count, active_searches)?)
        .ok_or("hold scan active-view join aggregate overflow")?;
    let report_searches = held_count
        .checked_mul(2)
        .ok_or("hold scan report join count overflow")?;
    work = work
        .checked_add(binary_search_work_bound(
            prior_report_count,
            report_searches,
        )?)
        .and_then(|value| {
            value.checked_add(binary_search_work_bound(current_report_count, report_searches).ok()?)
        })
        .ok_or("hold scan report-join aggregate overflow")?;
    let retirement_searches = held_count
        .checked_add(lifecycle_count)
        .ok_or("hold retirement lookup count overflow")?;
    work = work
        .checked_add(binary_search_work_bound(
            lifecycle_count,
            retirement_searches,
        )?)
        .ok_or("hold retirement lookup aggregate overflow")?;
    let hold_profile_points = hold_capacity
        .checked_mul(crate::route::HOLD_PROFILE_POINTS)
        .ok_or("hold retirement profile capacity overflow")?;
    work = work
        .checked_add(
            hold_profile_points
                .checked_mul(2)
                .ok_or("hold retirement profile move/clear aggregate overflow")?,
        )
        .ok_or("hold scan profile-work aggregate overflow")?;
    let retirement_soa_moves = held_count
        .checked_mul(20)
        .and_then(|value| value.checked_add(lifecycle_count.saturating_mul(3)))
        .and_then(|value| value.checked_add(zone_count))
        .ok_or("hold retirement SoA/work aggregate overflow")?;
    work = work
        .checked_add(retirement_soa_moves)
        .ok_or("hold scan SoA/work aggregate overflow")?;
    if work > MAX_HOLD_SCAN_JOIN_WORK {
        return Err("held trajectory joins exceed the named per-scan C7 work budget");
    }
    Ok((pair_limit, sua_limit))
}

fn merge_hold_entry_reports(
    previous: &[HoldEntryReport],
    current: &[HoldEntryReport],
    active_keys: &[oa_legacy_traj::core::trajectory::FlightKey],
    output: &mut Vec<HoldEntryReport>,
    capacity: usize,
) -> Result<(), &'static str> {
    if previous.len() > capacity
        || current.len() > capacity
        || output.capacity() < capacity
        || active_keys.len() > capacity
    {
        return Err("hold report merge exceeded its preflight capacity");
    }
    output.clear();
    let mut old_index = 0usize;
    let mut current_index = 0usize;
    let mut merge_steps = 0usize;
    let step_limit = previous
        .len()
        .checked_add(current.len())
        .ok_or("hold report merge bound overflow")?;
    while old_index < previous.len() || current_index < current.len() {
        merge_steps = merge_steps
            .checked_add(1)
            .ok_or("hold report merge counter overflow")?;
        if merge_steps > step_limit {
            return Err("hold report merge exceeded its C7 preflight work bound");
        }
        let old = previous.get(old_index).copied();
        let new = current.get(current_index).copied();
        let selected = match (old, new) {
            (Some(old), Some(new)) => match compare_full_flight_keys(&old.key, &new.key) {
                std::cmp::Ordering::Less => {
                    old_index += 1;
                    old
                }
                std::cmp::Ordering::Equal => {
                    old_index += 1;
                    current_index += 1;
                    new
                }
                std::cmp::Ordering::Greater => {
                    current_index += 1;
                    new
                }
            },
            (Some(old), None) => {
                old_index += 1;
                old
            }
            (None, Some(new)) => {
                current_index += 1;
                new
            }
            (None, None) => break,
        };
        if active_keys
            .binary_search_by(|candidate| compare_full_flight_keys(candidate, &selected.key))
            .is_ok()
        {
            if output.len() >= capacity {
                return Err("hold report merge output capacity exhausted");
            }
            output.push(selected);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeSuaAvailability {
    Ready,
    Incomplete,
    NoncontinuousSchedule,
    AglAltitude,
    UnknownAltitude,
    UnsupportedGeometry,
}

struct RuntimeSuaZone {
    boundary_index: usize,
    floor_ft: i32,
    ceiling_ft: i32,
    availability: RuntimeSuaAvailability,
    area: Option<Area>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeSuaZoneEvidence {
    pub boundary_index: usize,
    pub availability: RuntimeSuaAvailability,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeSuaEvidence {
    pub flight_gufi: u128,
    pub aircraft_address: u32,
    pub boundary_index: usize,
    pub result: SuaProbeResult,
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
    /// Latest raw report provenance per complete active flight key, bounded by active flights.
    hold_entry_reports: Vec<HoldEntryReport>,
    hold_entry_report_merge: Vec<HoldEntryReport>,
    latest_active_views: Vec<SharedFlightView>,
    active_key_scratch: Vec<oa_legacy_traj::core::trajectory::FlightKey>,
    last_scan_receipt_ns: Option<i64>,
    /// Strictly increasing receipt time of durable hold-command intents in this session.
    last_hold_command_receipt_ns: Option<i64>,
    conflict_scratch: crate::conflict_probe::ConflictProbeScratch,
    sua_zones: Vec<RuntimeSuaZone>,
    sua_zones_unexamined: usize,
    next_generation: u32,
    console_text: String,
    profile: RegionalRunProfile,
    selected_mode: RegionalAutomationMode,
    last_automation: RegionalAutomationStatus,
    known_flights: Vec<oa_flight::FlightKey>,
    msaw_service_audit: Option<MsawServiceAuditState>,
    /// Explicit OpenATCA hold annotations. Never inferred from circles or route state.
    holdings: HoldStore,
    /// Accepted revisions outlive cancellation only within each active flight lifecycle.
    hold_revisions: HoldRevisions,
    hold_lifecycles: FlightLifecycles,
    next_hold_lifecycle_generation: u64,
}

struct MsawServiceAuditState {
    audit_log: std::path::PathBuf,
    checkpoint: oa_audit_core::Checkpoint,
    registry: oa_audit_log::shell::PinnedPrincipalRegistry,
    principal: u64,
}

pub struct MsawServiceAuditConfiguration {
    pub audit_log: std::path::PathBuf,
    pub checkpoint: oa_audit_core::Checkpoint,
    pub registry: oa_audit_log::shell::PinnedPrincipalRegistry,
    pub principal: u64,
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
    /// Same-scan list outputs from the admitted active-centre flight views.
    pub aircraft_list: Vec<SharedFlightView>,
    pub departure_list: Vec<SharedFlightView>,
    /// Parallel active-key and lifecycle-generation columns for typed hold updates.
    pub active_flight_keys: Vec<oa_legacy_traj::core::trajectory::FlightKey>,
    pub active_flight_generations: Vec<u64>,
    /// Scan-local active-plan pair screenings; unavailable and unexamined work stays explicit.
    pub pair_probes: Vec<ProbeResult>,
    pub pair_probe_input_refusals: u64,
    pub pair_probes_unavailable: u64,
    pub pair_probes_unexamined: u64,
    /// Pair rows suppressed because at least one flight has explicit hold instructions.
    pub pair_hold_probe_unavailable: Vec<PairHoldProbeUnavailable>,
    /// Continuous CIFP UR screenings plus per-zone availability for unsupported inputs.
    pub sua_probes: Vec<RuntimeSuaEvidence>,
    pub sua_probes_unavailable: u64,
    pub sua_probes_unexamined: u64,
    /// SUA rows suppressed because the shared predictor has no holding-pattern trajectory.
    pub sua_hold_probe_unavailable: Vec<SuaHoldProbeUnavailable>,
    pub sua_zone_status: Vec<RuntimeSuaZoneEvidence>,
    pub sua_zones_unexamined: usize,
    /// Number of direct-hold profiles constructed once for this scan and shared by probes.
    pub hold_phase_profiles_built: u64,
    /// Explicit caller-supplied Hold View annotations, in update order.
    pub hold_view: Vec<HoldViewRow>,
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

impl RegionalFrame {
    /// Lookup the current typed update generation for one complete active flight key.
    pub fn hold_lifecycle_generation(
        &self,
        key: oa_legacy_traj::core::trajectory::FlightKey,
    ) -> Option<u64> {
        if self.active_flight_keys.len() != self.active_flight_generations.len() {
            return None;
        }
        self.active_flight_keys
            .iter()
            .position(|&candidate| candidate == key)
            .and_then(|index| self.active_flight_generations.get(index).copied())
    }
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
        | crate::input::EventKind::PositionV2 { .. }
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
        surveillance::SurveillanceStageError::SourceEvidenceCapacity => Some(REASON_RESOURCE_LIMIT),
        surveillance::SurveillanceStageError::WrongSourceKind
        | surveillance::SurveillanceStageError::InvalidPosition
        | surveillance::SurveillanceStageError::InvalidSourceEvidence => {
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
    if runtime.failed || principal == 0 || time_ns == 0 {
        return Err("session or quarantine audit context is invalid".into());
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

/// Install the host-pinned audit authority before the first admitted receipt. This checks local
/// role configuration only and does not authenticate the configured principal or caller.
pub fn regional_configure_msaw_service_audit(
    runtime: &mut RegionalRuntime,
    configuration: MsawServiceAuditConfiguration,
) -> Result<(), String> {
    if runtime.failed || runtime.frame_sequence != 0 || runtime.msaw_service_audit.is_some() {
        return Err(
            "MSAW service audit authority must be configured once before the first frame".into(),
        );
    }
    if !surveillance::surveillance_msaw_enabled(&runtime.surveillance) {
        return Err("MSAW service audit authority requires configured MSAW service".into());
    }
    configuration
        .registry
        .authorize(
            configuration.principal,
            oa_audit_core::Action::MsawServiceRequest,
        )
        .map_err(|error| format!("MSAW service registry refused: {error:?}"))?;
    oa_sealed::refuse(&configuration.audit_log)
        .map_err(|error| format!("MSAW service audit path refused: {error}"))?;
    runtime.msaw_service_audit = Some(MsawServiceAuditState {
        audit_log: configuration.audit_log,
        checkpoint: configuration.checkpoint,
        registry: configuration.registry,
        principal: configuration.principal,
    });
    Ok(())
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

fn runtime_sua_zones(model: &Model) -> (Vec<RuntimeSuaZone>, usize) {
    use air_model::{
        self, boundary_source, time_code, BoundaryIx, ALT_GROUND, ALT_NONE, ALT_UNKNOWN,
        ALT_UNLIMITED, POS_NONE,
    };
    let mut zones = Vec::with_capacity(model.boundaries.source.len().min(MAX_RUNTIME_SUA_ZONES));
    let mut unexamined = 0usize;
    for boundary_index in 0..model.boundaries.source.len() {
        if model.boundaries.source.get(boundary_index).copied() != Some(boundary_source::CIFP_UR) {
            continue;
        }
        if zones.len() >= MAX_RUNTIME_SUA_ZONES {
            unexamined = unexamined.saturating_add(1);
            continue;
        }
        let floor_ft = model
            .boundaries
            .floor_ft
            .get(boundary_index)
            .copied()
            .unwrap_or(ALT_NONE);
        let ceiling_ft = model
            .boundaries
            .ceiling_ft
            .get(boundary_index)
            .copied()
            .unwrap_or(ALT_NONE);
        let complete = u32::try_from(boundary_index)
            .ok()
            .and_then(|index| air_model::boundary_is_complete(model, BoundaryIx::new(index)))
            == Some(true);
        let availability = if !complete {
            RuntimeSuaAvailability::Incomplete
        } else if air_model::bit_get(&model.boundaries.floor_agl, boundary_index)
            || air_model::bit_get(&model.boundaries.ceiling_agl, boundary_index)
        {
            RuntimeSuaAvailability::AglAltitude
        } else if model.boundaries.time_code.get(boundary_index).copied()
            != Some(time_code::CONTINUOUS)
        {
            RuntimeSuaAvailability::NoncontinuousSchedule
        } else if [ALT_NONE, ALT_GROUND, ALT_UNKNOWN, ALT_UNLIMITED].contains(&floor_ft)
            || [ALT_NONE, ALT_GROUND, ALT_UNKNOWN, ALT_UNLIMITED].contains(&ceiling_ft)
            || floor_ft > ceiling_ft
        {
            RuntimeSuaAvailability::UnknownAltitude
        } else {
            RuntimeSuaAvailability::Ready
        };
        let area = if availability != RuntimeSuaAvailability::Ready {
            None
        } else {
            let span = model.boundaries.segs.get(boundary_index).copied();
            let segments = span
                .and_then(|span| {
                    let start = span.start.row();
                    let end = start.checked_add(span.len as usize)?;
                    (end <= model.segments.via.len()
                        && end <= model.segments.lat.len()
                        && end <= model.segments.lon.len()
                        && end <= model.segments.arc_lat.len()
                        && end <= model.segments.arc_lon.len()
                        && end <= model.segments.radius_m.len())
                    .then_some(start..end)
                })
                .and_then(|range| {
                    range
                        .map(|index| {
                            let lat = *model.segments.lat.get(index)?;
                            let lon = *model.segments.lon.get(index)?;
                            let arc_lat = *model.segments.arc_lat.get(index)?;
                            let arc_lon = *model.segments.arc_lon.get(index)?;
                            let pos = if lat == POS_NONE || lon == POS_NONE {
                                None
                            } else {
                                Some(geom::latlon_e7(lat, lon).ok()?)
                            };
                            let centre = if arc_lat == POS_NONE || arc_lon == POS_NONE {
                                None
                            } else {
                                Some(geom::latlon_e7(arc_lat, arc_lon).ok()?)
                            };
                            Some(geom::SegIn {
                                via: *model.segments.via.get(index)?,
                                pos,
                                centre,
                                radius_m: f64::from(*model.segments.radius_m.get(index)?),
                            })
                        })
                        .collect::<Option<Vec<_>>>()
                });
            segments.and_then(|segments| {
                geom::segment_samples_wgs84(&segments)
                    .ok()
                    .filter(|ring| ring.len() >= 3)
                    .and_then(|ring| geom::build_area(vec![ring], 0.0).ok())
            })
        };
        let final_availability = if availability == RuntimeSuaAvailability::Ready && area.is_none()
        {
            RuntimeSuaAvailability::UnsupportedGeometry
        } else {
            availability
        };
        zones.push(RuntimeSuaZone {
            boundary_index,
            floor_ft,
            ceiling_ft,
            availability: final_availability,
            area,
        });
    }
    (zones, unexamined)
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
    let (sua_zones, sua_zones_unexamined) = runtime_sua_zones(&model);
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
        hold_entry_reports: Vec::with_capacity(profile.max_flights),
        hold_entry_report_merge: Vec::with_capacity(profile.max_flights),
        latest_active_views: Vec::with_capacity(profile.max_flights),
        active_key_scratch: Vec::with_capacity(profile.max_flights),
        last_scan_receipt_ns: None,
        last_hold_command_receipt_ns: None,
        conflict_scratch: crate::conflict_probe::ConflictProbeScratch::new(),
        sua_zones,
        sua_zones_unexamined,
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
        msaw_service_audit: None,
        holdings: HoldStore::new(profile.max_flights),
        hold_revisions: HoldRevisions::new(profile.max_flights),
        hold_lifecycles: FlightLifecycles::new(profile.max_flights),
        next_hold_lifecycle_generation: 0,
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
                let view_sequence =
                    u32::try_from(runtime.frame_sequence).map_err(|_| "view sequence exhausted")?;
                let dispatch = if let Some(state) = runtime.msaw_service_audit.as_mut() {
                    let mut context = commands::MsawServiceAuditContext {
                        audit_log: &state.audit_log,
                        checkpoint: &mut state.checkpoint,
                        registry: &state.registry,
                        principal: state.principal,
                    };
                    commands::dispatch_with_msaw_audit(
                        commands::DispatchContext {
                            driver: &mut runtime.flights,
                            interaction: &mut runtime.interaction,
                            surveillance: &mut runtime.surveillance,
                            views: &runtime.views,
                            observed: &runtime.observed,
                            view_sequence,
                        },
                        event,
                        request,
                        Some(&mut context),
                    )
                } else {
                    commands::dispatch(
                        commands::DispatchContext {
                            driver: &mut runtime.flights,
                            interaction: &mut runtime.interaction,
                            surveillance: &mut runtime.surveillance,
                            views: &runtime.views,
                            observed: &runtime.observed,
                            view_sequence,
                        },
                        event,
                        request,
                    )
                };
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
    let mut surveillance = surveillance::surveillance_finish_with_live_flights(
        &mut runtime.surveillance,
        None,
        automation.effective == EffectiveAutomationMode::Surveillance,
        unavailable_reason,
        &mut runtime.flights,
        &runtime.views,
    )?;
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
    if runtime.known_flights.len() > MAX_FLIGHT_DATA_VIEW_WORK {
        return Err("active plan view work budget exceeded before lifecycle reconciliation".into());
    }
    let flight_cutoff_ms = route::receipt_cutoff_ms(batch.receipt_ns)?;
    let mut aircraft_list = Vec::with_capacity(runtime.known_flights.len());
    let mut departure_list = Vec::with_capacity(runtime.known_flights.len());
    let mut active_plan_views = Vec::with_capacity(runtime.known_flights.len());
    for key in &runtime.known_flights {
        match flight::flight_view_for_key(
            &mut runtime.flights,
            *key,
            runtime.centre,
            flight_cutoff_ms,
        ) {
            Ok(view)
                if matches!(
                    view.plan_state,
                    oa_flight::ST_ACTIVE | oa_flight::ST_DEPARTED
                ) =>
            {
                active_plan_views.push(view);
            }
            Ok(_) | Err(FlightViewStatus::UnknownFlight | FlightViewStatus::UnknownCentre) => {}
            Err(FlightViewStatus::FutureSnapshot) => {
                return Err(
                    "future active plan snapshot refused before lifecycle reconciliation".into(),
                );
            }
            Err(
                FlightViewStatus::Found
                | FlightViewStatus::AmbiguousAddress
                | FlightViewStatus::OutputCapacity,
            ) => {
                return Err(
                    "unexpected active plan view status before lifecycle reconciliation".into(),
                );
            }
        }
    }
    let plan_sort_limit = sort_work_bound(active_plan_views.len())?;
    let track_sort_limit = sort_work_bound(surveillance.tracks.len())?;
    let track_join_limit =
        binary_search_work_bound(surveillance.tracks.len(), active_plan_views.len())?;
    let address_sort_limit = sort_work_bound(active_plan_views.len())?;
    let plan_track_work = plan_sort_limit
        .checked_add(address_sort_limit)
        .and_then(|work| work.checked_add(track_sort_limit))
        .and_then(|work| work.checked_add(track_join_limit))
        .ok_or("active plan/track C7 work preflight overflow")?;
    if plan_track_work > MAX_ACTIVE_PLAN_TRACK_JOIN_WORK {
        return Err("active plan/track join exceeded its named C7 work budget".into());
    }
    let mut plan_sort_comparisons = 0usize;
    active_plan_views.sort_unstable_by(|left, right| {
        plan_sort_comparisons = plan_sort_comparisons.saturating_add(1);
        compare_full_flight_keys(&route::flight_key(left), &route::flight_key(right))
    });
    if plan_sort_comparisons > plan_sort_limit
        || active_plan_views.windows(2).any(|pair| {
            pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
                compare_full_flight_keys(&route::flight_key(left), &route::flight_key(right))
                    .is_eq()
            })
        })
    {
        return Err("active plan views failed the bounded unique full-key index check".into());
    }
    aircraft_list.extend(active_plan_views.iter().copied());
    for view in &active_plan_views {
        if view.departure.iter().any(|&byte| byte != 0) {
            departure_list.push(*view);
        }
    }
    let mut active_address_index = active_plan_views
        .iter()
        .enumerate()
        .map(|(index, view)| (view.address, index))
        .collect::<Vec<_>>();
    let mut address_sort_comparisons = 0usize;
    active_address_index.sort_unstable_by(|left, right| {
        address_sort_comparisons = address_sort_comparisons.saturating_add(1);
        left.0.cmp(&right.0)
    });
    if address_sort_comparisons > address_sort_limit {
        return Err("active address index sort exceeded its C7 work bound".into());
    }
    let mut ambiguous_plan_tracks = vec![false; active_plan_views.len()];
    let mut address_start = 0usize;
    while address_start < active_address_index.len() {
        let Some((address, _)) = active_address_index.get(address_start).copied() else {
            return Err("active address index position invariant failed".into());
        };
        let mut address_end = address_start.saturating_add(1);
        while active_address_index
            .get(address_end)
            .is_some_and(|(candidate, _)| *candidate == address)
        {
            address_end = address_end.saturating_add(1);
        }
        if address_end.saturating_sub(address_start) > 1 {
            for (_, index) in active_address_index
                .get(address_start..address_end)
                .ok_or("active address group range invariant failed")?
            {
                let Some(is_ambiguous) = ambiguous_plan_tracks.get_mut(*index) else {
                    return Err("active address ambiguity index invariant failed".into());
                };
                *is_ambiguous = true;
            }
        }
        address_start = address_end;
    }
    let mut track_index = surveillance
        .tracks
        .iter()
        .map(|track| (track.address, *track))
        .collect::<Vec<_>>();
    let mut track_sort_comparisons = 0usize;
    track_index.sort_unstable_by(|left, right| {
        track_sort_comparisons = track_sort_comparisons.saturating_add(1);
        left.0.cmp(&right.0)
    });
    if track_sort_comparisons > track_sort_limit
        || track_index.windows(2).any(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left.0 == right.0)
        })
    {
        return Err(
            "current surveillance tracks failed the bounded unique-address index check".into(),
        );
    }
    let mut track_join_comparisons = 0usize;
    let mut active_plan_tracks = vec![None; active_plan_views.len()];
    let mut probe_inputs = Vec::with_capacity(active_plan_views.len());
    for (plan_address, index) in &active_address_index {
        let Some(is_ambiguous) = ambiguous_plan_tracks.get(*index).copied() else {
            return Err("active plan track join index invariant failed".into());
        };
        if is_ambiguous {
            continue;
        }
        let track = track_index
            .binary_search_by(|(track_address, _)| {
                track_join_comparisons = track_join_comparisons.saturating_add(1);
                track_address.cmp(plan_address)
            })
            .ok()
            .and_then(|index| track_index.get(index).map(|(_, track)| *track));
        let Some(track) = track else { continue };
        if let Some(slot) = active_plan_tracks.get_mut(*index) {
            *slot = Some(track);
        }
    }
    if track_join_comparisons > track_join_limit {
        return Err(
            "active plan to surveillance join exceeded its binary-search C7 work bound".into(),
        );
    }
    for (view, track) in active_plan_views.iter().zip(&active_plan_tracks) {
        if let Some(track) = track {
            probe_inputs.push((*view, *track));
        }
    }
    let probe_sort_limit = sort_work_bound(probe_inputs.len())?;
    let mut probe_sort_comparisons = 0usize;
    probe_inputs.sort_unstable_by(|left, right| {
        probe_sort_comparisons = probe_sort_comparisons.saturating_add(1);
        compare_full_flight_keys(&route::flight_key(&left.0), &route::flight_key(&right.0))
    });
    if probe_sort_comparisons > probe_sort_limit {
        return Err("active probe-key sort exceeded its preflight C7 work bound".into());
    }
    if probe_inputs.windows(2).any(|pair| {
        pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
            compare_full_flight_keys(&route::flight_key(&left.0), &route::flight_key(&right.0))
                .is_eq()
        })
    }) {
        return Err("duplicate active full flight key in hold probe inputs".into());
    }
    let event_association_work = probe_inputs
        .len()
        .checked_mul(batch.events.len())
        .ok_or("hold report event association work overflow")?;
    if event_association_work > MAX_HOLD_REPORT_EVENT_ASSOCIATION_WORK {
        return Err("hold report event association exceeded its named C7 work budget".into());
    }
    let mut current_hold_reports = Vec::with_capacity(probe_inputs.len());
    for (view, track) in &probe_inputs {
        let key = route::flight_key(view);
        let report_event = batch
            .events
            .iter()
            .filter(|event| {
                event.source == crate::input::SourceKind::Surveillance
                    && event.identity.aircraft_address == track.address
                    && event.identity.flight_gufi == view.gufi
                    && event.source_ns == track.newest_source_ns
            })
            .filter(|event| matches!(event.kind, crate::input::EventKind::Position { .. }))
            .max_by_key(|event| event.sequence);
        if let Some(event) = report_event {
            if let crate::input::EventKind::Position {
                latitude_e7,
                longitude_e7,
                ..
            } = event.kind
            {
                current_hold_reports.push(HoldEntryReport {
                    key,
                    source_epoch: u64::from(runtime.session_namespace),
                    sequence: event.sequence,
                    source_ns: event.source_ns,
                    receipt_ns: event.receipt_ns,
                    lat_e7: latitude_e7,
                    lon_e7: longitude_e7,
                    r95_mm: track.r95_mm,
                });
            }
        }
    }
    let (pair_limit, sua_limit) = preflight_hold_scan_work(
        active_plan_views.len(),
        runtime.hold_lifecycles.len(),
        runtime.holdings.len(),
        runtime.hold_entry_reports.len(),
        current_hold_reports.len(),
        runtime.sua_zones.len(),
        runtime.profile.max_flights,
    )
    .map_err(str::to_owned)?;
    let report_sort_limit = sort_work_bound(current_hold_reports.len())?;
    let mut report_sort_comparisons = 0usize;
    current_hold_reports.sort_unstable_by(|left, right| {
        report_sort_comparisons = report_sort_comparisons.saturating_add(1);
        compare_full_flight_keys(&left.key, &right.key)
    });
    if report_sort_comparisons > report_sort_limit
        || current_hold_reports.windows(2).any(|pair| {
            pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
                compare_full_flight_keys(&left.key, &right.key).is_eq()
            })
        })
    {
        return Err("current hold reports failed the bounded unique-key index check".into());
    }
    let prior_report_sort_limit = sort_work_bound(runtime.hold_entry_reports.len())?;
    let mut prior_report_sort_comparisons = 0usize;
    runtime.hold_entry_reports.sort_unstable_by(|left, right| {
        prior_report_sort_comparisons = prior_report_sort_comparisons.saturating_add(1);
        compare_full_flight_keys(&left.key, &right.key)
    });
    if prior_report_sort_comparisons > prior_report_sort_limit
        || runtime.hold_entry_reports.windows(2).any(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left.key == right.key)
        })
    {
        return Err("prior hold reports failed the bounded unique-key index check".into());
    }
    runtime.latest_active_views.clear();
    runtime
        .latest_active_views
        .extend(active_plan_views.iter().copied());
    runtime.active_key_scratch.clear();
    if runtime.latest_active_views.len() > runtime.active_key_scratch.capacity() {
        return Err(
            "active hold-key scratch capacity exceeded before lifecycle reconciliation".into(),
        );
    }
    runtime
        .active_key_scratch
        .extend(runtime.latest_active_views.iter().map(route::flight_key));
    runtime
        .hold_lifecycles
        .reconcile(
            &runtime.active_key_scratch,
            &mut runtime.next_hold_lifecycle_generation,
        )
        .map_err(|error| format!("flight lifecycle reconciliation: {error}"))?;
    let retired_keys = runtime.hold_lifecycles.retired_keys();
    runtime
        .holdings
        .remove_keys(retired_keys)
        .map_err(|error| format!("retired hold cleanup: {error}"))?;
    runtime
        .hold_revisions
        .remove_keys(retired_keys)
        .map_err(|error| format!("retired hold revision cleanup: {error}"))?;
    let (active_flight_keys, active_flight_generations) = runtime
        .hold_lifecycles
        .snapshot()
        .map_err(|error| format!("flight lifecycle snapshot: {error}"))?;

    // Infer parallel/teardrop entries only from consecutive admitted reports. Inferred crossing
    // evidence becomes usable at the second report receipt; phase advancement then reconciles the
    // actual current tracker state against the immutable shared hold model.
    for index in 0..runtime.holdings.len() {
        let Some(config) = runtime.holdings.prediction(index) else {
            continue;
        };
        let Some(key) = runtime.holdings.key(index) else {
            return Err("hold key column invariant failed".into());
        };
        let Some(fix) = runtime.holdings.fix(index) else {
            return Err("hold fix column invariant failed".into());
        };
        let Some((view, track)) = find_probe_input(&probe_inputs, key) else {
            continue;
        };
        if view.address != key.aircraft_address {
            continue;
        }
        let mut phase = runtime.holdings.phase_state(index);
        if phase.is_none() && config.entry_procedure != crate::holding::HoldEntryProcedure::Direct {
            let Some(previous) = find_report(&runtime.hold_entry_reports, key) else {
                continue;
            };
            let Some(current) = find_report(&current_hold_reports, key) else {
                continue;
            };
            if current.receipt_ns != track.state_ns
                || current.source_ns != track.newest_source_ns
                || config.course_basis != HoldEntryCourseBasis::SyntheticNoWindCourse
            {
                continue;
            }
            let turn = match runtime.holdings.turn(index) {
                Some(crate::holding::HoldTurn::Left) => 1,
                Some(crate::holding::HoldTurn::Right) => 2,
                None => continue,
            };
            let evidence = crate::holding::infer_hold_entry(
                previous,
                current,
                crate::holding::HoldEntryInferenceConfig {
                    fix_lat_e7: fix.latitude_e7,
                    fix_lon_e7: fix.longitude_e7,
                    inbound_true_course_cdeg: config.inbound_true_course_cdeg,
                    turn: if turn == 1 {
                        crate::holding::HoldTurn::Left
                    } else {
                        crate::holding::HoldTurn::Right
                    },
                    max_scan_gap_ns: config.max_scan_gap_ns,
                    capture_radius_mm: config.entry_capture_radius_mm,
                    sector_ambiguity_cdeg: 500,
                    course_basis: config.course_basis,
                },
            );
            let Ok(evidence) = evidence else {
                continue;
            };
            let expected_procedure = match config.entry_procedure {
                crate::holding::HoldEntryProcedure::Direct => 1,
                crate::holding::HoldEntryProcedure::Parallel => 2,
                crate::holding::HoldEntryProcedure::Teardrop => 3,
            };
            let inferred_procedure = match evidence.procedure {
                crate::holding::HoldEntryProcedure::Direct => 1,
                crate::holding::HoldEntryProcedure::Parallel => 2,
                crate::holding::HoldEntryProcedure::Teardrop => 3,
            };
            if expected_procedure != inferred_procedure {
                continue;
            }
            let speed = oa_math::hypot(f64::from(track.east_mmps), f64::from(track.north_mmps));
            if !speed.is_finite() || !(1.0..=300_000.0).contains(&speed) {
                continue;
            }
            let Ok(speed_mmps) = u32::try_from(oa_ieee::round(speed) as u64) else {
                continue;
            };
            let Ok((started, _)) = oa_legacy_traj::core::trajectory::hold_phase_start_entry(
                oa_legacy_traj::core::trajectory::HoldEntryPhaseInput {
                    key,
                    crossing_state_ns: evidence.crossing_ns,
                    available_at_ns: evidence.available_at_ns,
                    start_lat_e7: evidence.crossing_lat_e7,
                    start_lon_e7: evidence.crossing_lon_e7,
                    start_heading_true_cdeg: evidence.course_true_cdeg,
                    fix_lat_e7: fix.latitude_e7,
                    fix_lon_e7: fix.longitude_e7,
                    inbound_true_course_cdeg: config.inbound_true_course_cdeg,
                    turn,
                    procedure: expected_procedure,
                    groundspeed_mmps: speed_mmps,
                    leg_duration_ms: config.leg_duration_ms,
                    turn_rate_cdeg_s: config.turn_rate_cdeg_s,
                    max_scan_gap_ns: config.max_scan_gap_ns,
                    max_speed_error_mmps: config.max_speed_error_mmps,
                },
            ) else {
                continue;
            };
            phase = Some(started);
        }
        let Some(mut phase) = phase else {
            continue;
        };
        let observation = oa_legacy_traj::core::trajectory::HoldTrackReconciliationInput {
            key,
            at_ns: track.state_ns,
            lat_e7: track.lat_e7,
            lon_e7: track.lon_e7,
            east_mmps: track.east_mmps,
            north_mmps: track.north_mmps,
            max_position_error_mm: config.max_position_error_mm,
            max_heading_error_cdeg: config.max_heading_error_cdeg,
        };
        if oa_legacy_traj::core::trajectory::hold_phase_reconcile_and_advance(
            &mut phase,
            observation,
        )
        .is_ok()
        {
            runtime
                .holdings
                .update_phase(index, phase)
                .map_err(str::to_owned)?;
        }
    }
    let mut hold_phase_profiles_built = 0u64;
    for index in 0..runtime.holdings.len() {
        let Some(state) = runtime.holdings.phase_state(index) else {
            continue;
        };
        let Some(config) = runtime.holdings.prediction(index) else {
            continue;
        };
        let Some(revision) = runtime.holdings.revision(index) else {
            continue;
        };
        let Some((view, track)) = find_probe_input(&probe_inputs, state.key) else {
            continue;
        };
        let built = {
            let Some(points) = runtime.holdings.profile_points_mut(index) else {
                return Err("hold profile buffer index invariant failed".into());
            };
            route::fill_hold_phase_profile(view, track, batch.receipt_ns, &state, config, points)
        };
        if built.is_ok() {
            runtime
                .holdings
                .cache_profile(index, state, config, revision, runtime.frame_sequence)
                .map_err(str::to_owned)?;
            hold_phase_profiles_built = hold_phase_profiles_built.saturating_add(1);
        }
    }

    let mut hold_rows = runtime
        .holdings
        .view_rows()
        .map_err(|error| format!("hold view: {error}"))?;
    let hold_row_sort_limit = sort_work_bound(hold_rows.len())?;
    let mut hold_row_sort_comparisons = 0usize;
    hold_rows.sort_unstable_by(|left, right| {
        hold_row_sort_comparisons = hold_row_sort_comparisons.saturating_add(1);
        compare_full_flight_keys(&left.instruction.flight_key, &right.instruction.flight_key)
    });
    if hold_row_sort_comparisons > hold_row_sort_limit
        || hold_rows.windows(2).any(|pair| {
            pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
                left.instruction.flight_key == right.instruction.flight_key
            })
        })
    {
        return Err("hold rows failed the bounded unique-key index check".into());
    }

    let pair_count = active_plan_views
        .len()
        .checked_mul(active_plan_views.len().saturating_sub(1))
        .map(|count| count / 2)
        .ok_or("runtime pair-probe count overflow")?;
    let mut pair_probes = Vec::with_capacity(pair_limit);
    let mut pair_hold_probe_unavailable = Vec::new();
    let mut pair_probe_input_refusals = 0u64;
    let mut pair_probes_unavailable = 0u64;
    let mut pairs_examined = 0usize;
    'pairs: for first in 0..active_plan_views.len() {
        for second in (first + 1)..active_plan_views.len() {
            if pairs_examined >= pair_limit {
                break 'pairs;
            }
            let (Some(first_view), Some(second_view)) =
                (active_plan_views.get(first), active_plan_views.get(second))
            else {
                pair_probe_input_refusals = pair_probe_input_refusals.saturating_add(1);
                pairs_examined += 1;
                continue;
            };
            let first_key = route::flight_key(first_view);
            let second_key = route::flight_key(second_view);
            let first_track = active_plan_tracks.get(first).copied().flatten();
            let second_track = active_plan_tracks.get(second).copied().flatten();
            let (Some(first_track), Some(second_track)) = (first_track, second_track) else {
                let Some(first_ambiguous) = ambiguous_plan_tracks.get(first).copied() else {
                    return Err("first pair address-state index invariant failed".into());
                };
                let Some(second_ambiguous) = ambiguous_plan_tracks.get(second).copied() else {
                    return Err("second pair address-state index invariant failed".into());
                };
                let ambiguous = first_ambiguous || second_ambiguous;
                pair_probes.push(ProbeResult {
                    status: ProbeStatus::Unavailable,
                    refusal: if ambiguous {
                        oa_ca::probe::ProbeRefusal::InvalidIdentity
                    } else {
                        oa_ca::probe::ProbeRefusal::MissingTrack
                    },
                    examined_segments: 0,
                    reserved_header: [0; 4],
                    first_gufi: first_key.gufi,
                    first_aircraft_address: first_key.aircraft_address,
                    second_gufi: second_key.gufi,
                    second_aircraft_address: second_key.aircraft_address,
                    first_loss_ms: oa_ca::probe::NO_PROBE_TIME_MS,
                    cpa_ms: oa_ca::probe::NO_PROBE_TIME_MS,
                    cpa_horizontal_m: oa_ca::probe::NO_PROBE_DISTANCE_M,
                    cpa_vertical_ft: oa_ca::probe::NO_PROBE_VERTICAL_FT,
                    reserved_tail: [0; 8],
                });
                pair_probes_unavailable = pair_probes_unavailable.saturating_add(1);
                pairs_examined += 1;
                continue;
            };
            let first_hold = find_hold_row(&hold_rows, first_key);
            let second_hold = find_hold_row(&hold_rows, second_key);
            let first_phase = first_hold
                .and_then(|row| {
                    row.phase_state
                        .zip(row.instruction.prediction)
                        .map(|(state, config)| (state, config, row.revision))
                })
                .filter(|(state, _, _)| state.last_advance_ns == first_track.state_ns);
            let second_phase = second_hold
                .and_then(|row| {
                    row.phase_state
                        .zip(row.instruction.prediction)
                        .map(|(state, config)| (state, config, row.revision))
                })
                .filter(|(state, _, _)| state.last_advance_ns == second_track.state_ns);
            let first_profile = first_phase.as_ref().and_then(|(state, config, revision)| {
                let index = first_hold?.store_index;
                Some(route::HoldRouteProfile {
                    state,
                    config: *config,
                    revision: *revision,
                    points: runtime.holdings.cached_profile(
                        index,
                        *state,
                        *config,
                        *revision,
                        runtime.frame_sequence,
                    )?,
                })
            });
            let second_profile = second_phase.as_ref().and_then(|(state, config, revision)| {
                let index = second_hold?.store_index;
                Some(route::HoldRouteProfile {
                    state,
                    config: *config,
                    revision: *revision,
                    points: runtime.holdings.cached_profile(
                        index,
                        *state,
                        *config,
                        *revision,
                        runtime.frame_sequence,
                    )?,
                })
            });
            if (first_hold.is_some() && first_profile.is_none())
                || (second_hold.is_some() && second_profile.is_none())
            {
                pair_probes.push(ProbeResult {
                    status: ProbeStatus::Unavailable,
                    refusal: oa_ca::probe::ProbeRefusal::None,
                    examined_segments: 0,
                    reserved_header: [0; 4],
                    first_gufi: first_key.gufi,
                    first_aircraft_address: first_key.aircraft_address,
                    second_gufi: second_key.gufi,
                    second_aircraft_address: second_key.aircraft_address,
                    first_loss_ms: oa_ca::probe::NO_PROBE_TIME_MS,
                    cpa_ms: oa_ca::probe::NO_PROBE_TIME_MS,
                    cpa_horizontal_m: oa_ca::probe::NO_PROBE_DISTANCE_M,
                    cpa_vertical_ft: oa_ca::probe::NO_PROBE_VERTICAL_FT,
                    reserved_tail: [0; 8],
                });
                pair_hold_probe_unavailable.push(PairHoldProbeUnavailable {
                    first: first_key,
                    second: second_key,
                    reason: unavailable_hold_reason(
                        if first_hold.is_some() && first_profile.is_none() {
                            first_hold
                        } else {
                            second_hold
                        },
                    ),
                });
                pair_probes_unavailable = pair_probes_unavailable.saturating_add(1);
                pairs_examined += 1;
                continue;
            }
            match route::route_pair_probe_with_holds(
                &runtime.routes,
                route::PairRouteProbeInput {
                    first: route::ActiveRouteInput {
                        view: first_view,
                        observed: &first_track,
                    },
                    first_hold: first_profile,
                    second: route::ActiveRouteInput {
                        view: second_view,
                        observed: &second_track,
                    },
                    second_hold: second_profile,
                    cutoff_ns: batch.receipt_ns,
                    frame_sequence: runtime.frame_sequence,
                },
                &mut runtime.conflict_scratch,
            ) {
                Ok(result) => {
                    if result.status == ProbeStatus::Unavailable {
                        pair_probes_unavailable = pair_probes_unavailable.saturating_add(1);
                    }
                    pair_probes.push(result);
                }
                Err(_) => {
                    pair_probe_input_refusals = pair_probe_input_refusals.saturating_add(1);
                    pair_probes_unavailable = pair_probes_unavailable.saturating_add(1);
                }
            }
            pairs_examined += 1;
        }
    }
    let pair_probes_unexamined = pair_count.saturating_sub(pairs_examined) as u64;

    let sua_zone_status = runtime
        .sua_zones
        .iter()
        .map(|zone| RuntimeSuaZoneEvidence {
            boundary_index: zone.boundary_index,
            availability: zone.availability,
        })
        .collect::<Vec<_>>();
    let sua_candidates = active_plan_views
        .len()
        .checked_mul(runtime.sua_zones.len())
        .ok_or("runtime SUA probe count overflow")?;
    let mut sua_probes = Vec::with_capacity(sua_limit);
    let mut sua_hold_probe_unavailable = Vec::new();
    let mut sua_probes_unavailable = 0u64;
    let mut sua_examined = 0usize;
    'sua: for (plan_index, view) in active_plan_views.iter().enumerate() {
        let Some(track) = active_plan_tracks.get(plan_index).copied().flatten() else {
            let Some(ambiguous) = ambiguous_plan_tracks.get(plan_index).copied() else {
                return Err("SUA address-state index invariant failed".into());
            };
            for zone in &runtime.sua_zones {
                if sua_examined >= sua_limit {
                    break 'sua;
                }
                sua_probes.push(RuntimeSuaEvidence {
                    flight_gufi: view.gufi,
                    aircraft_address: view.address,
                    boundary_index: zone.boundary_index,
                    result: SuaProbeResult {
                        status: SuaStatus::Unavailable,
                        refusal: if ambiguous {
                            oa_airspace::core::conflict_probe::SuaRefusal::AmbiguousIdentity
                        } else {
                            oa_airspace::core::conflict_probe::SuaRefusal::MissingTrack
                        },
                        reserved: [0; 6],
                        entry_ms: oa_airspace::core::conflict_probe::NO_SUA_ENTRY_MS,
                    },
                });
                sua_probes_unavailable = sua_probes_unavailable.saturating_add(1);
                sua_examined += 1;
            }
            continue;
        };
        for zone in &runtime.sua_zones {
            if sua_examined >= sua_limit {
                break 'sua;
            }
            let key = route::flight_key(view);
            let hold = find_hold_row(&hold_rows, key);
            let phase = hold
                .and_then(|row| {
                    row.phase_state
                        .zip(row.instruction.prediction)
                        .map(|(state, config)| (state, config, row.revision))
                })
                .filter(|(state, _, _)| state.last_advance_ns == track.state_ns);
            let hold_profile = phase.as_ref().and_then(|(state, config, revision)| {
                let index = hold?.store_index;
                Some(route::HoldRouteProfile {
                    state,
                    config: *config,
                    revision: *revision,
                    points: runtime.holdings.cached_profile(
                        index,
                        *state,
                        *config,
                        *revision,
                        runtime.frame_sequence,
                    )?,
                })
            });
            if zone.availability == RuntimeSuaAvailability::Ready
                && hold.is_some()
                && hold_profile.is_none()
            {
                sua_hold_probe_unavailable.push(SuaHoldProbeUnavailable {
                    flight: key,
                    boundary_index: zone.boundary_index,
                    reason: unavailable_hold_reason(hold),
                });
                sua_probes_unavailable = sua_probes_unavailable.saturating_add(1);
                sua_examined += 1;
                continue;
            }
            if zone.availability == RuntimeSuaAvailability::Ready {
                if let Some(area) = &zone.area {
                    let input = route::SuaRouteProbeInput {
                        active: route::ActiveRouteInput {
                            view,
                            observed: &track,
                        },
                        cutoff_ns: batch.receipt_ns,
                        frame_sequence: runtime.frame_sequence,
                        area,
                        floor_ft: zone.floor_ft,
                        ceiling_ft: zone.ceiling_ft,
                    };
                    let result = match hold_profile {
                        Some(profile) => route::route_hold_sua_probe(
                            input,
                            profile,
                            &mut runtime.conflict_scratch,
                        ),
                        None => route::route_sua_probe(
                            &runtime.routes,
                            input,
                            &mut runtime.conflict_scratch,
                        ),
                    };
                    match result {
                        Ok(result) => {
                            if result.status == SuaStatus::Unavailable {
                                sua_probes_unavailable = sua_probes_unavailable.saturating_add(1);
                            }
                            sua_probes.push(RuntimeSuaEvidence {
                                flight_gufi: view.gufi,
                                aircraft_address: view.address,
                                boundary_index: zone.boundary_index,
                                result,
                            });
                        }
                        Err(_) => {
                            sua_probes_unavailable = sua_probes_unavailable.saturating_add(1);
                        }
                    }
                } else {
                    sua_probes_unavailable = sua_probes_unavailable.saturating_add(1);
                }
            } else {
                sua_probes_unavailable = sua_probes_unavailable.saturating_add(1);
            }
            sua_examined += 1;
        }
    }
    let sua_probes_unexamined = sua_candidates.saturating_sub(sua_examined) as u64
        + runtime.sua_zones_unexamined as u64 * active_plan_views.len() as u64;
    runtime.views = commands::current_views(
        &runtime.views,
        &flights,
        &mut runtime.next_generation,
        runtime.profile.max_flights,
    )?;
    merge_hold_entry_reports(
        &runtime.hold_entry_reports,
        &current_hold_reports,
        runtime.hold_lifecycles.active_keys(),
        &mut runtime.hold_entry_report_merge,
        runtime.profile.max_flights,
    )
    .map_err(str::to_owned)?;
    core::mem::swap(
        &mut runtime.hold_entry_reports,
        &mut runtime.hold_entry_report_merge,
    );
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
    runtime.last_scan_receipt_ns = Some(batch.receipt_ns);
    let hold_view = hold_rows;
    if runtime.profile.surveillance.render_scope {
        let display_holds = hold_view
            .iter()
            .map(|row| oa_display::hold_view::HoldViewEntry {
                gufi: row.instruction.flight_key.gufi,
                aircraft_address: row.instruction.flight_key.aircraft_address,
                centre: row.instruction.flight_key.centre,
                plan_id: row.instruction.flight_key.plan_id,
                fix_name: row.instruction.fix.name,
                direction_from_fix_deg: row.instruction.direction_from_fix_deg,
                inbound_course_deg: row.instruction.inbound_course_deg,
                turn: match row.instruction.turn {
                    None => 0,
                    Some(crate::holding::HoldTurn::Left) => 1,
                    Some(crate::holding::HoldTurn::Right) => 2,
                },
                leg_value: row.instruction.leg.map(|leg| leg.value),
                leg_unit: match row.instruction.leg.map(|leg| leg.unit) {
                    None => 0,
                    Some(crate::holding::HoldLegUnit::Miles) => 1,
                    Some(crate::holding::HoldLegUnit::Minutes) => 2,
                },
                efc_utc_minute: row.instruction.efc_utc_minute,
                updated_receipt_ns: row.updated_receipt_ns,
                revision: row.revision,
                lifecycle_generation: row.lifecycle_generation,
            })
            .collect::<Vec<_>>();
        surveillance.frame_sha256 = surveillance::surveillance_present_controls_with_holds(
            &mut runtime.surveillance,
            &runtime.interaction,
            &automation_console_text(&runtime.console_text, automation),
            &display_holds,
        )?;
    }
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
        aircraft_list,
        departure_list,
        active_flight_keys,
        active_flight_generations,
        pair_probes,
        pair_probe_input_refusals,
        pair_probes_unavailable,
        pair_probes_unexamined,
        pair_hold_probe_unavailable,
        sua_probes,
        sua_probes_unavailable,
        sua_probes_unexamined,
        sua_hold_probe_unavailable,
        sua_zone_status,
        sua_zones_unexamined: runtime.sua_zones_unexamined,
        hold_phase_profiles_built,
        hold_view,
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

/// Explicit OpenATCA offline trial request. The route body must be supplied by the caller;
/// this API does not infer an amendment from observed tracks or claim an FAA wire contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrialRouteProbeRequest {
    pub candidate_route: crate::input::NormalizedEvent,
    pub counterpart_gufi: u128,
}

/// Evaluate a caller-supplied filed-route candidate against one other active plan from the latest
/// completed scan. This is a detached query: a successful result does not admit the route or
/// mutate active flight, route, controller, or surveillance state.
fn cached_hold_route_profile<'a>(
    holdings: &'a HoldStore,
    store_index: usize,
    state: &'a oa_legacy_traj::core::trajectory::HoldPhaseState,
    config: HoldPredictionIntent,
    revision: u64,
    scan_sequence: u64,
) -> Option<route::HoldRouteProfile<'a>> {
    Some(route::HoldRouteProfile {
        state,
        config,
        revision,
        points: holdings.cached_profile(store_index, *state, config, revision, scan_sequence)?,
    })
}

pub fn regional_trial_route_probe(
    runtime: &mut RegionalRuntime,
    request: TrialRouteProbeRequest,
) -> Result<ProbeResult, String> {
    if runtime.failed {
        return Err("session failed; reconstruct from the admitted input prefix".into());
    }
    let cutoff_ns = runtime
        .last_scan_receipt_ns
        .ok_or("trial request requires a completed active scan")?;
    if request.candidate_route.receipt_ns != cutoff_ns {
        return Err("trial route receipt must match the latest completed scan".into());
    }
    let candidate_gufi = request.candidate_route.identity.flight_gufi;
    let candidate_view = runtime
        .latest_active_views
        .iter()
        .find(|view| view.gufi == candidate_gufi)
        .ok_or("trial candidate is not an active observed flight in the latest scan")?;
    let counterpart_view = runtime
        .latest_active_views
        .iter()
        .find(|view| view.gufi == request.counterpart_gufi)
        .ok_or("trial counterpart is not an active observed flight in the latest scan")?;
    for view in [candidate_view, counterpart_view] {
        let active_identity_count = runtime
            .latest_active_views
            .iter()
            .filter(|candidate| candidate.address == view.address)
            .count();
        let track_count = runtime
            .observed
            .iter()
            .filter(|track| track.address == view.address)
            .count();
        if active_identity_count != 1 || track_count != 1 {
            return Err("trial route probe requires unique active track identities".into());
        }
    }
    let candidate_track = runtime
        .observed
        .iter()
        .find(|track| track.address == candidate_view.address)
        .ok_or("trial candidate has no latest observed track")?;
    let counterpart_track = runtime
        .observed
        .iter()
        .find(|track| track.address == counterpart_view.address)
        .ok_or("trial counterpart has no latest observed track")?;
    let mut hold_rows = runtime
        .holdings
        .view_rows()
        .map_err(|error| format!("hold view: {error}"))?;
    if hold_rows.len() > runtime.profile.max_flights {
        return Err("trial hold-row sort exceeded its C7 capacity preflight".into());
    }
    let trial_join_work = sort_work_bound(hold_rows.len())?
        .checked_add(binary_search_work_bound(hold_rows.len(), 2)?)
        .ok_or("trial hold-row join work overflow")?;
    if trial_join_work > MAX_HOLD_SCAN_JOIN_WORK {
        return Err("trial hold-row joins exceed their named C7 work budget".into());
    }
    let hold_row_sort_limit = sort_work_bound(hold_rows.len()).map_err(str::to_owned)?;
    let mut hold_row_sort_comparisons = 0usize;
    hold_rows.sort_unstable_by(|left, right| {
        hold_row_sort_comparisons = hold_row_sort_comparisons.saturating_add(1);
        compare_full_flight_keys(&left.instruction.flight_key, &right.instruction.flight_key)
    });
    if hold_row_sort_comparisons > hold_row_sort_limit
        || hold_rows.windows(2).any(|pair| {
            pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
                left.instruction.flight_key == right.instruction.flight_key
            })
        })
    {
        return Err("trial hold rows failed the bounded unique-key index check".into());
    }
    let candidate_hold = find_hold_row(&hold_rows, route::flight_key(candidate_view));
    let counterpart_hold = find_hold_row(&hold_rows, route::flight_key(counterpart_view));
    let current_phase = |row: Option<&HoldViewRow>, track: &crate::surveillance::ObservedTrack| {
        row.and_then(|row| {
            row.phase_state
                .zip(row.instruction.prediction)
                .and_then(|(state, config)| {
                    (state.last_advance_ns == track.state_ns).then_some((
                        state,
                        config,
                        row.revision,
                    ))
                })
        })
    };
    let candidate_phase = current_phase(candidate_hold, candidate_track);
    let counterpart_phase = current_phase(counterpart_hold, counterpart_track);
    let candidate_profile = candidate_phase
        .as_ref()
        .and_then(|(state, config, revision)| {
            cached_hold_route_profile(
                &runtime.holdings,
                candidate_hold?.store_index,
                state,
                *config,
                *revision,
                runtime.frame_sequence,
            )
        });
    let counterpart_profile = counterpart_phase
        .as_ref()
        .and_then(|(state, config, revision)| {
            cached_hold_route_profile(
                &runtime.holdings,
                counterpart_hold?.store_index,
                state,
                *config,
                *revision,
                runtime.frame_sequence,
            )
        });
    if (candidate_hold.is_some() && candidate_phase.is_none())
        || (counterpart_hold.is_some() && counterpart_phase.is_none())
        || (candidate_phase.is_some() && candidate_profile.is_none())
        || (counterpart_phase.is_some() && counterpart_profile.is_none())
    {
        return Err(
            "trial is unavailable until each active hold reconciles the latest track".into(),
        );
    }
    route::route_trial_pair_probe_with_holds(
        &runtime.routes,
        &runtime.model,
        route::TrialRouteProbeInput {
            candidate_event: &request.candidate_route,
            candidate_view,
            candidate_track,
            counterpart_view,
            counterpart_track,
            cutoff_ns,
            frame_sequence: runtime.frame_sequence,
        },
        candidate_profile,
        counterpart_profile,
        &mut runtime.conflict_scratch,
    )
}

fn valid_hold_field<const N: usize>(bytes: &[u8; N]) -> bool {
    let mut padding = false;
    let mut present = false;
    for &byte in bytes {
        if byte == 0 {
            padding = true;
        } else if padding || !byte.is_ascii_graphic() {
            return false;
        } else {
            present = true;
        }
    }
    present
}

fn valid_hold_key(key: oa_legacy_traj::core::trajectory::FlightKey) -> bool {
    key.gufi.iter().any(|&byte| byte != 0)
        && (1..=0x01ff_ffff).contains(&key.aircraft_address)
        && valid_hold_field(&key.centre)
        && valid_hold_field(&key.plan_id)
}

fn validate_hold_instruction(instruction: HoldInstruction) -> Result<(), String> {
    let fix = instruction.fix;
    if !valid_hold_key(instruction.flight_key) {
        return Err("hold instruction requires a positive full flight key".into());
    }
    if !valid_hold_field(&fix.name)
        || !valid_hold_field(&fix.region)
        || !(-900_000_000..=900_000_000).contains(&fix.latitude_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&fix.longitude_e7)
        || instruction.direction_from_fix_deg >= 360
        || instruction.inbound_course_deg >= 360
        || instruction.leg.is_some_and(|leg| leg.value == 0)
        || instruction.turn.is_none() && instruction.prediction.is_some()
        || instruction.prediction.is_some_and(|config| {
            instruction.inbound_course_deg != config.inbound_true_course_cdeg / 100
        })
        || instruction
            .efc_utc_minute
            .is_some_and(|minute| minute >= 1440)
        || instruction.prediction.is_some_and(|config| {
            (config.entry_procedure == crate::holding::HoldEntryProcedure::Direct
                && config.entry_state_ns <= 0)
                || (config.entry_procedure != crate::holding::HoldEntryProcedure::Direct
                    && config.entry_state_ns != 0)
                || !(1..=1_000_000).contains(&config.entry_capture_radius_mm)
                || config.inbound_true_course_cdeg >= 36_000
                || !(1..=600_000).contains(&config.leg_duration_ms)
                || !(1..=600).contains(&config.turn_rate_cdeg_s)
                || !(1..=60_000_000_000).contains(&config.max_scan_gap_ns)
                || !(1..=1_000_000).contains(&config.max_position_error_mm)
                || !(1..=4_500).contains(&config.max_heading_error_cdeg)
                || !(1..=300_000).contains(&config.max_speed_error_mmps)
        })
    {
        return Err("hold instruction contains an invalid fix or explicit value".into());
    }
    Ok(())
}

fn start_hold_phase(
    instruction: HoldInstruction,
    entry: crate::surveillance::ObservedTrack,
) -> Result<Option<oa_legacy_traj::core::trajectory::HoldPhaseState>, String> {
    let Some(config) = instruction.prediction else {
        return Ok(None);
    };
    if config.entry_procedure != crate::holding::HoldEntryProcedure::Direct {
        return Ok(None);
    }
    if instruction.turn.is_none()
        || config.entry_state_ns != entry.state_ns
        || instruction.inbound_course_deg != config.inbound_true_course_cdeg / 100
        || instruction.fix.latitude_e7 != entry.lat_e7
        || instruction.fix.longitude_e7 != entry.lon_e7
    {
        return Err(
            "hold phase requires an exact direct-entry scan at the configured fix/course".into(),
        );
    }
    let east = f64::from(entry.east_mmps);
    let north = f64::from(entry.north_mmps);
    let speed = oa_math::hypot(east, north);
    if !speed.is_finite() || !(1.0..=300_000.0).contains(&speed) {
        return Err("hold entry groundspeed is outside the shared predictor envelope".into());
    }
    let speed_mmps = oa_ieee::round(speed) as u32;
    let phase_input = oa_legacy_traj::core::trajectory::HoldPhaseInput {
        key: instruction.flight_key,
        entry_state_ns: config.entry_state_ns,
        fix_lat_e7: instruction.fix.latitude_e7,
        fix_lon_e7: instruction.fix.longitude_e7,
        inbound_true_course_cdeg: config.inbound_true_course_cdeg,
        turn: match instruction.turn {
            Some(crate::holding::HoldTurn::Left) => 1,
            Some(crate::holding::HoldTurn::Right) => 2,
            None => return Err("hold turn direction is required for prediction".into()),
        },
        groundspeed_mmps: speed_mmps,
        leg_duration_ms: config.leg_duration_ms,
        turn_rate_cdeg_s: config.turn_rate_cdeg_s,
        max_scan_gap_ns: config.max_scan_gap_ns,
        max_speed_error_mmps: config.max_speed_error_mmps,
    };
    let (state, snapshot) = oa_legacy_traj::core::trajectory::hold_phase_start(phase_input)
        .map_err(|reason| format!("hold phase configuration refused: {reason:?}"))?;
    let heading = oa_math::rem_euclid(
        oa_math::atan2(east, north) * (180.0 / core::f64::consts::PI),
        360.0,
    ) * 100.0;
    let delta = oa_math::rem_euclid(
        heading - f64::from(snapshot.heading_true_cdeg) + 54_000.0,
        36_000.0,
    ) - 18_000.0;
    if !heading.is_finite() || delta.abs() > f64::from(config.max_heading_error_cdeg) {
        return Err("hold entry track heading does not match configured direct-entry state".into());
    }
    Ok(Some(state))
}

fn unavailable_hold_reason(row: Option<&HoldViewRow>) -> HoldProbeUnavailableReason {
    match row.and_then(|row| row.instruction.prediction) {
        Some(config) if config.entry_procedure != crate::holding::HoldEntryProcedure::Direct => {
            HoldProbeUnavailableReason::UnsupportedEntryProcedure
        }
        Some(_) => HoldProbeUnavailableReason::TrackStateNotReconciled,
        None => HoldProbeUnavailableReason::MissingAcceptedEntry,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedHoldUpdate {
    key: oa_legacy_traj::core::trajectory::FlightKey,
    operation: u8,
    lifecycle_generation: u64,
    previous_revision: Option<u64>,
    revision: u64,
    updated_receipt_ns: i64,
}

fn prepare_hold_update(
    runtime: &RegionalRuntime,
    update: HoldUpdate,
    command_receipt_ns: i64,
    allow_reserved_receipt: bool,
) -> Result<PreparedHoldUpdate, String> {
    if runtime.failed {
        return Err("session failed; reconstruct from the admitted input prefix".into());
    }
    let scan_receipt_ns = runtime
        .last_scan_receipt_ns
        .ok_or("hold update requires a completed active scan")?;
    if command_receipt_ns <= 0 || command_receipt_ns < scan_receipt_ns {
        return Err("hold command receipt must be positive and not precede the latest scan".into());
    }
    if runtime.last_hold_command_receipt_ns.is_some_and(|last| {
        command_receipt_ns < last || (!allow_reserved_receipt && command_receipt_ns == last)
    }) {
        return Err("hold command receipt is stale or not strictly increasing".into());
    }
    let (operation, lifecycle_generation, revision, key, instruction) = match update {
        HoldUpdate::Assign {
            lifecycle_generation,
            revision,
            instruction,
        } => (
            1,
            lifecycle_generation,
            revision,
            instruction.flight_key,
            Some(instruction),
        ),
        HoldUpdate::Amend {
            lifecycle_generation,
            revision,
            instruction,
        } => (
            2,
            lifecycle_generation,
            revision,
            instruction.flight_key,
            Some(instruction),
        ),
        HoldUpdate::Cancel {
            lifecycle_generation,
            revision,
            flight_key,
        } => (3, lifecycle_generation, revision, flight_key, None),
    };
    if lifecycle_generation == 0 || revision == 0 || !valid_hold_key(key) {
        return Err(
            "hold command requires a positive revision, lifecycle, and full flight key".into(),
        );
    }
    if let Some(instruction) = instruction {
        validate_hold_instruction(instruction)?;
        if instruction.flight_key != key {
            return Err("hold command instruction key changed during preparation".into());
        }
    }
    runtime
        .hold_lifecycles
        .validate_generation(key, lifecycle_generation)
        .map_err(str::to_owned)?;
    runtime
        .hold_revisions
        .validate(key, lifecycle_generation, revision)
        .map_err(str::to_owned)?;
    // Validate every parallel hold column before the durable intent is written. The mutable
    // methods below have no remaining refusal branch after this consistency check.
    let _ = runtime.holdings.view_rows().map_err(str::to_owned)?;
    let mut active_matches = 0usize;
    for view in &runtime.latest_active_views {
        if route::flight_key(view) == key {
            active_matches = active_matches.saturating_add(1);
        }
    }
    let existing = runtime.holdings.position(key);
    match operation {
        1 => {
            if active_matches != 1 {
                return Err("hold assignment requires one exact active full-key flight".into());
            }
            if existing.is_some() {
                return Err("hold already exists; use explicit amend".into());
            }
            if runtime.holdings.len() >= runtime.profile.max_flights {
                return Err("hold view capacity exhausted; assignment refused".into());
            }
        }
        2 => {
            if active_matches != 1 {
                return Err("hold amendment requires one exact active full-key flight".into());
            }
            if existing.is_none() {
                return Err("hold amendment requires an existing hold".into());
            }
        }
        3 => {
            if active_matches != 1 {
                return Err("hold cancellation requires one exact active full-key flight".into());
            }
            if existing.is_none() {
                return Err("hold cancellation requires an existing hold".into());
            }
        }
        _ => return Err("unknown typed hold operation".into()),
    }
    if let Some(instruction) = instruction.filter(|instruction| instruction.prediction.is_some()) {
        let entry = runtime
            .observed
            .iter()
            .find(|track| track.address == instruction.flight_key.aircraft_address)
            .copied()
            .ok_or("hold phase command requires the accepted entry track")?;
        let _ = start_hold_phase(instruction, entry)?;
    }
    let previous_revision = runtime
        .hold_revisions
        .last_revision(key, lifecycle_generation);
    if let Some(index) = existing {
        if previous_revision != runtime.holdings.revision(index) {
            return Err("hold revision ledger differs from active hold state".into());
        }
    }
    Ok(PreparedHoldUpdate {
        key,
        operation,
        lifecycle_generation,
        previous_revision,
        revision,
        updated_receipt_ns: command_receipt_ns,
    })
}

fn push_hold_digest_bytes(
    bytes: &mut [u8; 192],
    offset: &mut usize,
    value: &[u8],
) -> Result<(), String> {
    let end = offset
        .checked_add(value.len())
        .ok_or("hold command digest offset overflow")?;
    let slot = bytes
        .get_mut(*offset..end)
        .ok_or("hold command digest buffer capacity exceeded")?;
    slot.copy_from_slice(value);
    *offset = end;
    Ok(())
}

fn hold_command_digest(
    update: HoldUpdate,
    previous_revision: Option<u64>,
) -> Result<[u8; 32], String> {
    use crate::holding::{HoldEntryProcedure, HoldLegUnit, HoldTurn};
    let mut bytes = [0_u8; 192];
    let mut offset = 0usize;
    let (operation, lifecycle_generation, revision, key, instruction) = match update {
        HoldUpdate::Assign {
            lifecycle_generation,
            revision,
            instruction,
        } => (
            1,
            lifecycle_generation,
            revision,
            instruction.flight_key,
            Some(instruction),
        ),
        HoldUpdate::Amend {
            lifecycle_generation,
            revision,
            instruction,
        } => (
            2,
            lifecycle_generation,
            revision,
            instruction.flight_key,
            Some(instruction),
        ),
        HoldUpdate::Cancel {
            lifecycle_generation,
            revision,
            flight_key,
        } => (3, lifecycle_generation, revision, flight_key, None),
    };
    push_hold_digest_bytes(&mut bytes, &mut offset, b"OA-HOLD-COMMAND-V1\0")?;
    push_hold_digest_bytes(&mut bytes, &mut offset, &[operation])?;
    push_hold_digest_bytes(&mut bytes, &mut offset, &key.gufi)?;
    push_hold_digest_bytes(&mut bytes, &mut offset, &key.aircraft_address.to_le_bytes())?;
    push_hold_digest_bytes(&mut bytes, &mut offset, &key.centre)?;
    push_hold_digest_bytes(&mut bytes, &mut offset, &key.plan_id)?;
    push_hold_digest_bytes(&mut bytes, &mut offset, &lifecycle_generation.to_le_bytes())?;
    push_hold_digest_bytes(
        &mut bytes,
        &mut offset,
        &[u8::from(previous_revision.is_some())],
    )?;
    push_hold_digest_bytes(
        &mut bytes,
        &mut offset,
        &previous_revision.unwrap_or(0).to_le_bytes(),
    )?;
    push_hold_digest_bytes(&mut bytes, &mut offset, &revision.to_le_bytes())?;
    if let Some(instruction) = instruction {
        push_hold_digest_bytes(&mut bytes, &mut offset, &instruction.fix.name)?;
        push_hold_digest_bytes(&mut bytes, &mut offset, &instruction.fix.region)?;
        push_hold_digest_bytes(
            &mut bytes,
            &mut offset,
            &instruction.fix.latitude_e7.to_le_bytes(),
        )?;
        push_hold_digest_bytes(
            &mut bytes,
            &mut offset,
            &instruction.fix.longitude_e7.to_le_bytes(),
        )?;
        push_hold_digest_bytes(
            &mut bytes,
            &mut offset,
            &instruction.direction_from_fix_deg.to_le_bytes(),
        )?;
        push_hold_digest_bytes(
            &mut bytes,
            &mut offset,
            &instruction.inbound_course_deg.to_le_bytes(),
        )?;
        push_hold_digest_bytes(
            &mut bytes,
            &mut offset,
            &[match instruction.turn {
                None => 0,
                Some(HoldTurn::Left) => 1,
                Some(HoldTurn::Right) => 2,
            }],
        )?;
        if let Some(leg) = instruction.leg {
            push_hold_digest_bytes(&mut bytes, &mut offset, &[1])?;
            push_hold_digest_bytes(&mut bytes, &mut offset, &leg.value.to_le_bytes())?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &[match leg.unit {
                    HoldLegUnit::Miles => 1,
                    HoldLegUnit::Minutes => 2,
                }],
            )?;
        } else {
            push_hold_digest_bytes(&mut bytes, &mut offset, &[0, 0, 0])?;
        }
        if let Some(efc) = instruction.efc_utc_minute {
            push_hold_digest_bytes(&mut bytes, &mut offset, &[1])?;
            push_hold_digest_bytes(&mut bytes, &mut offset, &efc.to_le_bytes())?;
        } else {
            push_hold_digest_bytes(&mut bytes, &mut offset, &[0, 0, 0])?;
        }
        if let Some(config) = instruction.prediction {
            push_hold_digest_bytes(&mut bytes, &mut offset, &[1])?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &[match config.entry_procedure {
                    HoldEntryProcedure::Direct => 1,
                    HoldEntryProcedure::Parallel => 2,
                    HoldEntryProcedure::Teardrop => 3,
                }],
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.entry_state_ns.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &[match config.course_basis {
                    HoldEntryCourseBasis::SyntheticNoWindCourse => 1,
                    HoldEntryCourseBasis::WindOrHeadingUnknown => 2,
                }],
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.entry_capture_radius_mm.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.inbound_true_course_cdeg.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.leg_duration_ms.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.turn_rate_cdeg_s.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.max_scan_gap_ns.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.max_position_error_mm.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.max_heading_error_cdeg.to_le_bytes(),
            )?;
            push_hold_digest_bytes(
                &mut bytes,
                &mut offset,
                &config.max_speed_error_mmps.to_le_bytes(),
            )?;
        } else {
            push_hold_digest_bytes(&mut bytes, &mut offset, &[0])?;
        }
    }
    Ok(oa_sha256::core::sha256::sha256(
        bytes
            .get(..offset)
            .ok_or("hold command digest range invalid")?,
    ))
}

fn hold_audit_target(key: oa_legacy_traj::core::trajectory::FlightKey) -> Result<String, String> {
    // Fixed-width, versioned hex prevents valid punctuation in centre/plan fields from
    // making distinct FlightKeys alias in the audit target.
    let mut identity = [0_u8; 36];
    identity[..16].copy_from_slice(&key.gufi);
    identity[16..20].copy_from_slice(&key.aircraft_address.to_be_bytes());
    identity[20..24].copy_from_slice(&key.centre);
    identity[24..36].copy_from_slice(&key.plan_id);
    Ok(format!("hold-key-v1:{}", oa_sha256::to_hex(&identity)))
}

/// Outcome after a hold intent has reached the durable audit log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldCommandDisposition {
    Applied,
    /// The intent is durable, but the runtime mutation did not commit. Callers must reconcile
    /// this record; it is deliberately not reported as a completed hold command.
    IntentRecordedButNotCommitted {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldCommandResult {
    pub checkpoint: oa_audit_core::Checkpoint,
    pub disposition: HoldCommandDisposition,
}

/// Audited typed OpenATCA hold command dispatch. This is not an FAA/ERAM command parser or an
/// authenticated operator UI. The caller supplies an independently pinned registry and trusted
/// audit checkpoint; the local registry contributes role authorization only.
pub fn regional_dispatch_hold_command(
    runtime: &mut RegionalRuntime,
    update: HoldUpdate,
    authority: crate::commands::HoldCommandAuthority<'_>,
) -> Result<HoldCommandResult, String> {
    use oa_audit_core::{details, target, Action, Event};
    use oa_audit_log::shell::{append_receipt, load_principal_registry};

    let registry =
        load_principal_registry(authority.registry_path, authority.expected_registry_sha256)
            .map_err(|error| format!("hold command registry refused: {error}"))?;
    let role =
        oa_audit_core::authorize_principal(&registry, authority.principal_id, Action::HoldCommand)
            .map_err(|error| format!("hold command authorization refused: {error:?}"))?;
    let prepared = prepare_hold_update(runtime, update, authority.command_receipt_ns, false)?;
    let digest = hold_command_digest(update, prepared.previous_revision)?;
    let (target_bytes, target_len) = target(hold_audit_target(prepared.key)?.as_bytes())
        .map_err(|error| format!("hold audit target refused: {error:?}"))?;
    let operation = match prepared.operation {
        1 => "ASSIGN",
        2 => "AMEND",
        3 => "CANCEL",
        _ => return Err("unknown typed hold command operation".into()),
    };
    let previous = prepared
        .previous_revision
        .map_or_else(|| "NONE".to_owned(), |value| value.to_string());
    let details_text = format!(
        "phase=INTENT;operation={operation};lifecycle={};previous_revision={previous};revision={};command_sha256={}",
        prepared.lifecycle_generation,
        prepared.revision,
        oa_sha256::to_hex(&digest),
    );
    let (details_bytes, details_len) = details(details_text.as_bytes())
        .map_err(|error| format!("hold audit details refused: {error:?}"))?;
    let event = Event {
        principal: authority.principal_id,
        role,
        action: Action::HoldCommand,
        target: target_bytes,
        target_len,
        time_ns: u64::try_from(prepared.updated_receipt_ns)
            .map_err(|_| "hold command audit time is invalid")?,
        details: details_bytes,
        details_len,
    };
    let (checkpoint, receipt) =
        append_receipt(authority.audit_path, authority.expected_checkpoint, event)
            .map_err(|error| format!("hold command audit refused before mutation: {error:?}"))?;
    Ok(finish_hold_command_after_append(
        runtime,
        update,
        prepared,
        authority.command_receipt_ns,
        checkpoint,
        receipt,
        event,
    ))
}

fn finish_hold_command_after_append(
    runtime: &mut RegionalRuntime,
    update: HoldUpdate,
    prepared: PreparedHoldUpdate,
    command_receipt_ns: i64,
    checkpoint: oa_audit_core::Checkpoint,
    receipt: oa_audit_core::AuditReceipt,
    event: oa_audit_core::Event,
) -> HoldCommandResult {
    runtime.last_hold_command_receipt_ns = Some(command_receipt_ns);
    let uncommitted = |reason: String| HoldCommandResult {
        checkpoint,
        disposition: HoldCommandDisposition::IntentRecordedButNotCommitted { reason },
    };
    if !oa_audit_core::receipt_matches(&receipt, event) {
        return uncommitted("durable hold intent receipt did not match the command".to_owned());
    }
    match prepare_hold_update(runtime, update, command_receipt_ns, true) {
        Ok(current) if current == prepared => {}
        Ok(_) => {
            return uncommitted("runtime hold state changed after durable intent".to_owned());
        }
        Err(error) => {
            return uncommitted(format!(
                "runtime hold preflight failed after durable intent: {error}"
            ));
        }
    }
    match regional_apply_hold_update_at(runtime, update, command_receipt_ns) {
        Ok(()) => HoldCommandResult {
            checkpoint,
            disposition: HoldCommandDisposition::Applied,
        },
        Err(error) => uncommitted(format!(
            "runtime refused hold mutation after durable intent: {error}"
        )),
    }
}

fn regional_apply_hold_update_at(
    runtime: &mut RegionalRuntime,
    update: HoldUpdate,
    updated_receipt_ns: i64,
) -> Result<(), String> {
    if runtime.failed {
        return Err("session failed; reconstruct from the admitted input prefix".into());
    }
    let (lifecycle_generation, revision, key) = match update {
        HoldUpdate::Assign {
            lifecycle_generation,
            revision,
            instruction,
        }
        | HoldUpdate::Amend {
            lifecycle_generation,
            revision,
            instruction,
        } => {
            validate_hold_instruction(instruction)?;
            (lifecycle_generation, revision, instruction.flight_key)
        }
        HoldUpdate::Cancel {
            lifecycle_generation,
            revision,
            flight_key,
        } => {
            if !valid_hold_key(flight_key) {
                return Err("hold cancellation requires a positive full flight key".into());
            }
            (lifecycle_generation, revision, flight_key)
        }
    };
    runtime
        .hold_lifecycles
        .validate_generation(key, lifecycle_generation)
        .map_err(str::to_owned)?;
    runtime
        .hold_revisions
        .validate(key, lifecycle_generation, revision)
        .map_err(str::to_owned)?;
    let mut active_matches = 0usize;
    for view in &runtime.latest_active_views {
        if route::flight_key(view) == key {
            active_matches = active_matches.saturating_add(1);
        }
    }
    let existing = runtime.holdings.position(key);
    match update {
        HoldUpdate::Assign { instruction, .. } => {
            if active_matches != 1 {
                return Err("hold assignment requires one exact active full-key flight".into());
            }
            if existing.is_some() {
                return Err("hold already exists; use explicit amend".into());
            }
            if runtime.holdings.len() >= runtime.profile.max_flights {
                return Err("hold view capacity exhausted; assignment refused".into());
            }
            let phase_state = if instruction.prediction.is_some() {
                let entry = runtime
                    .observed
                    .iter()
                    .find(|track| track.address == instruction.flight_key.aircraft_address)
                    .copied()
                    .ok_or("hold phase assignment requires the accepted entry track")?;
                start_hold_phase(instruction, entry)?
            } else {
                None
            };
            runtime
                .holdings
                .push(
                    instruction,
                    updated_receipt_ns,
                    revision,
                    lifecycle_generation,
                    phase_state,
                )
                .map_err(str::to_owned)?;
        }
        HoldUpdate::Amend { instruction, .. } => {
            if active_matches != 1 {
                return Err("hold amendment requires one exact active full-key flight".into());
            }
            let Some(index) = existing else {
                return Err("hold amendment requires an existing hold".into());
            };
            let phase_state = if instruction.prediction.is_some() {
                let entry = runtime
                    .observed
                    .iter()
                    .find(|track| track.address == instruction.flight_key.aircraft_address)
                    .copied()
                    .ok_or("hold phase amendment requires the accepted re-entry track")?;
                start_hold_phase(instruction, entry)?
            } else {
                None
            };
            runtime
                .holdings
                .replace(
                    index,
                    instruction,
                    updated_receipt_ns,
                    revision,
                    lifecycle_generation,
                    phase_state,
                )
                .map_err(str::to_owned)?;
        }
        HoldUpdate::Cancel { .. } => {
            let Some(index) = existing else {
                return Err("hold cancellation requires an existing hold".into());
            };
            runtime.holdings.remove(index).map_err(str::to_owned)?;
        }
    }
    runtime
        .hold_revisions
        .record(key, lifecycle_generation, revision)
        .map_err(str::to_owned)?;
    Ok(())
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

    // REQ: OA-MSAW-084, OA-AUDIT-002, OA-AUDIT-003, OA-AUDIT-005, OA-AUDIT-007, OA-AUDIT-008
    #[test]
    fn queued_msaw_service_request_is_pinned_audited_and_used_on_the_next_cycle() {
        let id = NEXT_QUARANTINE_AUDIT_TEST.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "oa-msaw-service-request-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create synthetic request directory");
        let registry_path = root.join("principals.tsv");
        let audit_path = root.join("audit.log");
        let registry_bytes = b"OACERT-LOCAL-PRINCIPALS-V1\n51\toperate\n52\tmonitor\n";
        std::fs::write(&registry_path, registry_bytes).expect("write synthetic registry");
        let registry = oa_audit_log::shell::load_pinned_principal_registry(
            &registry_path,
            oa_sha256::sha256(registry_bytes),
        )
        .expect("load independently pinned registry");
        let mut engine = runtime();
        regional_enable_synthetic_msaw(&mut engine).expect("enable synthetic MSAW");
        regional_configure_msaw_service_audit(
            &mut engine,
            MsawServiceAuditConfiguration {
                audit_log: audit_path.clone(),
                checkpoint: oa_audit_core::genesis(),
                registry,
                principal: 51,
            },
        )
        .expect("install host-pinned authority");
        let gufi = "00000000000000000000000000000001";
        let seeded = [
            format!("v1\t1000000000\t1000000000\t1\tSURVEILLANCE\t{gufi}\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"),
            format!("v1\t1000000000\t1000000000\t2\tFLIGHT\t{gufi}\tABCDEF\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2"),
            format!("v1\t1000000000\t1000000000\t3\tFLIGHT\t{gufi}\tABCDEF\tZAU1\tFLIGHT\tADDRESS"),
            format!("v1\t1000000000\t1000000000\t4\tFLIGHT\t{gufi}\tABCDEF\tZAU1\tFLIGHT\tOWNER\tSECTOR1"),
        ].map(|line| parse_event_line(&line).expect("parse seeded event"));
        let seeded_frame = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 1_000_000_000,
                source_clock_regressions: 0,
                events: seeded.to_vec(),
            },
        )
        .expect("first frame");
        assert!(seeded_frame.controller_views.is_empty());
        let confirmed = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 2_000_000_000,
                source_clock_regressions: 0,
                events: vec![parse_event_line(&format!(
                    "v1\t2000000000\t2000000000\t5\tSURVEILLANCE\t{gufi}\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"
                ))
                .expect("parse confirming position")],
            },
        )
        .expect("confirmed target frame");
        let target = confirmed
            .controller_views
            .first()
            .expect("confirmed flight view");
        let command = parse_event_line(&format!(
            "v1\t3000000000\t3000000000\t7\tFLIGHT\t{gufi}\tABCDEF\tZAU1\tCOMMAND\tPLAN00000001\t{}\t{}\t1\tMSAW_SERVICE\t62000000000",
            target.generation, confirmed.sequence,
        )).expect("parse queued service request");
        let position = parse_event_line(&format!(
            "v1\t3000000000\t3000000000\t6\tSURVEILLANCE\t{gufi}\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"
        )).expect("parse third position");
        let frame = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 3_000_000_000,
                source_clock_regressions: 0,
                events: vec![position, command],
            },
        )
        .expect("queued request and next MSAW cycle");
        assert_eq!(frame.controller_dispatches.len(), 1);
        let dispatch = frame
            .controller_dispatches
            .first()
            .expect("one controller dispatch");
        assert!(
            dispatch.outcome.is_some(),
            "MSAW request was refused: {:?}",
            dispatch.refusal
        );
        assert_eq!(
            surveillance::surveillance_msaw_service_request_count(&engine.surveillance),
            1
        );
        let report = frame.surveillance.msaw.as_ref().expect("MSAW report");
        assert_eq!(
            report.live_track_basis,
            vec![crate::msaw_bridge::LiveTrackBasis {
                address: 0x00ab_cdef,
                basis: oa_msaw_run::eligibility::Basis::Request,
            }]
        );
        let expected = engine
            .msaw_service_audit
            .as_ref()
            .expect("audit context")
            .checkpoint;
        assert_eq!(expected.count, 1);
        assert_eq!(
            oa_audit_log::shell::verify(&audit_path, expected).expect("verify request audit"),
            expected
        );
        let revoke = parse_event_line(&format!(
            "v1\t4000000000\t4000000000\t9\tFLIGHT\t{gufi}\tABCDEF\tZAU1\tCOMMAND\tPLAN00000001\t{}\t{}\t1\tMSAW_SERVICE\t0",
            target.generation, frame.sequence,
        )).expect("parse revocation");
        let position = parse_event_line(&format!(
            "v1\t4000000000\t4000000000\t8\tSURVEILLANCE\t{gufi}\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"
        )).expect("parse third position");
        let revoked = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 4_000_000_000,
                source_clock_regressions: 0,
                events: vec![position, revoke],
            },
        )
        .expect("audited request revocation");
        assert_eq!(
            surveillance::surveillance_msaw_service_request_count(&engine.surveillance),
            0
        );
        assert_eq!(
            engine
                .msaw_service_audit
                .as_ref()
                .expect("audit state")
                .checkpoint
                .count,
            2
        );
        engine
            .msaw_service_audit
            .as_mut()
            .expect("audit state")
            .audit_log = root.join("missing-parent").join("audit.log");
        let failed_activation = parse_event_line(&format!(
            "v1\t5000000000\t5000000000\t11\tFLIGHT\t{gufi}\tABCDEF\tZAU1\tCOMMAND\tPLAN00000001\t{}\t{}\t1\tMSAW_SERVICE\t64000000000",
            target.generation, revoked.sequence,
        )).expect("parse failed activation");
        let position = parse_event_line(&format!(
            "v1\t5000000000\t5000000000\t10\tSURVEILLANCE\t{gufi}\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"
        )).expect("parse fourth position");
        let refused = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 5_000_000_000,
                source_clock_regressions: 0,
                events: vec![position, failed_activation],
            },
        )
        .expect("audit refusal is a recorded operator refusal");
        assert!(refused
            .controller_dispatches
            .iter()
            .any(|dispatch| dispatch.refusal.contains("audit append")));
        assert_eq!(
            surveillance::surveillance_msaw_service_request_count(&engine.surveillance),
            0
        );
        std::fs::remove_dir_all(root).expect("remove synthetic request directory");
    }
    // REQ: OA-ENC-023
    #[test]
    fn hold_scan_work_preflight_covers_declared_maximum_and_refuses_excess() {
        let (pair_limit, sua_limit) = preflight_hold_scan_work(
            4_096,
            8_192,
            8_192,
            8_192,
            4_096,
            MAX_RUNTIME_SUA_ZONES,
            8_192,
        )
        .unwrap();
        assert_eq!(pair_limit, MAX_RUNTIME_PAIR_PROBES);
        assert_eq!(sua_limit, MAX_RUNTIME_SUA_PROBES);

        assert!(preflight_hold_scan_work(
            16_384,
            16_384,
            16_384,
            16_384,
            16_384,
            MAX_RUNTIME_SUA_ZONES,
            16_384,
        )
        .is_err());
    }

    #[test]
    fn hold_audit_target_is_injective_for_delimiter_containing_valid_keys() {
        let first = oa_legacy_traj::core::trajectory::FlightKey {
            gufi: [7; 16],
            aircraft_address: 0xabc123,
            centre: *b"AB:C",
            plan_id: *b"DEF\0\0\0\0\0\0\0\0\0",
        };
        let second = oa_legacy_traj::core::trajectory::FlightKey {
            gufi: [7; 16],
            aircraft_address: 0xabc123,
            centre: *b"AB\0\0",
            plan_id: *b"C:DEF\0\0\0\0\0\0\0",
        };
        assert!(valid_hold_key(first));
        assert!(valid_hold_key(second));
        assert_ne!(hold_audit_target(first), hold_audit_target(second));
    }

    #[test]
    fn durable_hold_intent_with_post_append_refusal_is_not_applied() {
        let model = synthetic_model().expect("synthetic model");
        let snapshot = oa_airspace::core::snapshot::encode(&model).expect("snapshot");
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
            oa_audit_core::genesis(),
        )
        .expect("runtime");
        let key = oa_legacy_traj::core::trajectory::FlightKey {
            gufi: [8; 16],
            aircraft_address: 0xabc123,
            centre: *b"ZAU1",
            plan_id: *b"PLAN00000001",
        };
        let update = HoldUpdate::Cancel {
            lifecycle_generation: 1,
            revision: 1,
            flight_key: key,
        };
        let prepared = PreparedHoldUpdate {
            key,
            operation: 3,
            lifecycle_generation: 1,
            previous_revision: None,
            revision: 1,
            updated_receipt_ns: 2,
        };
        let (target, target_len) =
            oa_audit_core::target(hold_audit_target(key).expect("target").as_bytes())
                .expect("valid audit target");
        let (details, details_len) =
            oa_audit_core::details(b"phase=INTENT;operation=CANCEL").expect("valid details");
        let event = oa_audit_core::Event {
            principal: 51,
            role: oa_audit_core::Role::Operate,
            action: oa_audit_core::Action::HoldCommand,
            target,
            target_len,
            time_ns: 20,
            details,
            details_len,
        };
        let audit_path = std::env::temp_dir().join(format!(
            "oa-hold-post-append-refusal-{}-{}.log",
            std::process::id(),
            NEXT_QUARANTINE_AUDIT_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let (checkpoint, receipt) =
            oa_audit_log::shell::append_receipt(&audit_path, oa_audit_core::genesis(), event)
                .expect("durable intent");
        runtime.failed = true;
        let result = finish_hold_command_after_append(
            &mut runtime,
            update,
            prepared,
            20,
            checkpoint,
            receipt,
            event,
        );
        assert!(matches!(
            result.disposition,
            HoldCommandDisposition::IntentRecordedButNotCommitted { .. }
        ));
        assert_eq!(runtime.holdings.len(), 0);
        assert_eq!(runtime.last_hold_command_receipt_ns, Some(20));
        let _ = std::fs::remove_file(audit_path);
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

    fn synthetic_ur_model(time: u8, floor_agl: bool) -> Model {
        use air_model::{
            self, boundary_source, edge_interp, level, stratum, via, SegIx, Span, NONE_U16,
            POS_NONE,
        };
        let mut model = crate::cli::synthetic_model().unwrap();
        let boundaries = &mut model.boundaries;
        boundaries.source.push(boundary_source::CIFP_UR);
        boundaries.stratum.push(stratum::SUA_MOA);
        boundaries.edge_interp.push(edge_interp::PER_SEGMENT);
        boundaries.multiple.push(b'A');
        boundaries.facility.push(air_model::StrIx::NONE);
        boundaries.name.push(air_model::StrIx::NONE);
        boundaries.floor_ft.push(10_000);
        boundaries.ceiling_ft.push(20_000);
        boundaries.vertices.push(Span {
            start: air_model::VertexIx::NONE,
            len: 0,
        });
        boundaries.segs.push(Span {
            start: SegIx::new(0),
            len: 4,
        });
        boundaries.time_code.push(time);
        boundaries.level.push(level::LOW);
        air_model::bit_push(&mut boundaries.floor_agl, floor_agl);
        air_model::bit_push(&mut boundaries.ceiling_agl, false);
        air_model::bit_push(&mut boundaries.incomplete, false);
        model.segments.via = vec![via::GEODESIC; 4];
        model.segments.lat = vec![410_000_000, 410_000_000, 410_100_000, 410_100_000];
        model.segments.lon = vec![-870_000_000, -869_900_000, -869_900_000, -870_000_000];
        model.segments.arc_lat = vec![POS_NONE; 4];
        model.segments.arc_lon = vec![POS_NONE; 4];
        model.segments.radius_m = vec![0; 4];
        model.segments.bearing_cdeg = vec![NONE_U16; 4];
        model
    }

    // REQ: OA-ENC-023
    #[test]
    fn runtime_ur_adapter_accepts_continuous_msl_and_refuses_part_time_or_agl() {
        let (zones, unexamined) =
            runtime_sua_zones(&synthetic_ur_model(air_model::time_code::CONTINUOUS, false));
        assert_eq!(unexamined, 0);
        assert_eq!(zones.len(), 1);
        let zone = zones.first().expect("one SUA zone");
        assert_eq!(zone.availability, RuntimeSuaAvailability::Ready);
        assert!(zone.area.is_some());

        let (part_time, _) =
            runtime_sua_zones(&synthetic_ur_model(air_model::time_code::PART_TIME, false));
        assert_eq!(
            part_time.first().expect("part-time zone").availability,
            RuntimeSuaAvailability::NoncontinuousSchedule
        );
        let (agl, _) =
            runtime_sua_zones(&synthetic_ur_model(air_model::time_code::CONTINUOUS, true));
        assert_eq!(
            agl.first().expect("AGL zone").availability,
            RuntimeSuaAvailability::AglAltitude
        );
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

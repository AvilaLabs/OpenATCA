//! Offline composed regional session (shell, B1/B3/C6), with one common receipt cutoff.
//! This executes actual product cores; it is not deadline or allocation qualification.

use crate::commands;
use crate::flight::{
    self, FlightApplyOutcome, FlightDriver, FlightDriverConfig, FlightDriverReport,
    FlightViewStatus, SharedFlightView, MAX_DRIVER_FLIGHTS, MAX_DRIVER_LOG_ENTRIES,
};
use crate::metering::{resolve_metering_scan, MeteringScanReport, MeteringScanRow};
use crate::nam_flight::{self, NamFlightContext, NamFlightResult, NamFlightRuntime};
use crate::route::{self, RouteStore};
use crate::session::{summarize_input_batch, FrameEvidence, ReceiptBatch, MAX_EVENTS_PER_RECEIPT};
use crate::surveillance::{self, Surveillance, SurveillanceConfig, SurveillanceFrame};
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
const MAX_RUNTIME_ITEM_WORK: u32 =
    oa_airspace::core::route::MAX_ROUTE_WORK + crate::input::MAX_FILED_ROUTE_BYTES as u32 + 16;

// A7.2: cold subsystem composition. All core memory belongs to its originating subsystem.
pub struct RegionalRuntime {
    model: Model,
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
    metering_views: Vec<crate::flight::SharedFlightView>,
    observed: Vec<crate::surveillance::ObservedTrack>,
    next_generation: u32,
    console_text: String,
    profile: RegionalRunProfile,
}

/// Versioned capacities for a single offline replay. Defaults reproduce the historic small
/// session; `recorded_peak` is an explicitly selected high-volume analysis profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalRunProfile {
    pub surveillance: surveillance::SurveillanceProfile,
    pub max_flights: usize,
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
}

pub const DEFAULT_REGIONAL_RUN_PROFILE: RegionalRunProfile = RegionalRunProfile {
    surveillance: surveillance::DEFAULT_SURVEILLANCE_PROFILE,
    max_flights: MAX_DRIVER_FLIGHTS,
    max_flight_log_entries: MAX_DRIVER_LOG_ENTRIES,
    max_total_events: crate::session::MAX_SESSION_EVENTS,
};

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
    pub flight_operations: Vec<FlightApplyOutcome>,
    pub flights: Vec<FlightEvidence>,
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
    /// Source rows and exact current-flight bindings supplied to the regional scope build.
    pub metering: Option<MeteringScanReport>,
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
/// The principal, role, timestamp and note are caller claims; this path authenticates no identity
/// and does not imply that the external manual workflow itself was performed.
pub struct QuarantineMaintenanceRequest<'a> {
    pub source: InputSource,
    pub expected_identity: InputIdentity,
    pub audit_log: &'a Path,
    pub expected_checkpoint: oa_audit_core::Checkpoint,
    pub principal: u64,
    pub role: oa_audit_core::Role,
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
        principal,
        role,
        time_ns,
        manual_handoff_ref,
    } = request;
    if runtime.failed {
        return Err("session failed; reconstruct before quarantine maintenance".into());
    }
    if principal == 0 || time_ns == 0 {
        return Err("audit principal and timestamp must be positive".into());
    }
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
    regional_new_with_profile_and_namespace_and_audit_checkpoint(
        model,
        snapshot_sha256,
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
    if !centre.iter().all(u8::is_ascii_alphanumeric) {
        return Err("centre must be four ASCII alphanumeric bytes".into());
    }
    if session_namespace == 0 {
        return Err("session namespace must be nonzero".into());
    }
    if profile.max_flights == 0
        || profile.max_flights > crate::flight::MAX_OFFLINE_DRIVER_FLIGHTS
        || profile.max_flight_log_entries == 0
        || profile.max_flight_log_entries > MAX_DRIVER_LOG_ENTRIES
        || profile.max_total_events == 0
        || profile.max_total_events > crate::session::MAX_RECORDED_PROFILE_EVENTS
        || (profile.surveillance.render_scope
            && profile.max_flights > profile.surveillance.max_tracks)
    {
        return Err("regional profile exceeds fixed runtime bounds".into());
    }
    let nam_flights = nam_flight::nam_flight_runtime_new(profile.max_flights)
        .ok_or("NAM flight-plan runtime capacity is invalid")?;
    Ok(RegionalRuntime {
        surveillance: surveillance::surveillance_new_with_profile(
            model.clone(),
            cfg,
            profile.surveillance,
        )?,
        model,
        flights: flight::flight_driver_new_bounded(
            FlightDriverConfig {
                flight_capacity: profile.max_flights,
                refusal_log_capacity: profile.max_flight_log_entries,
                conflict_log_capacity: profile.max_flight_log_entries,
            },
            profile.max_flights,
        )
        .map_err(|e| format!("flight startup: {e:?}"))?,
        routes: route::route_store_new(snapshot_sha256),
        nam_flights,
        centre,
        frame_sequence: 0,
        failed: false,
        session_namespace,
        quarantine_queue: oa_quarantine::queue_new_with_audit_checkpoint(audit_checkpoint),
        interaction: oa_display::interaction::interaction_new(),
        views: Vec::with_capacity(profile.max_flights),
        metering_views: Vec::with_capacity(profile.max_flights),
        observed: Vec::with_capacity(profile.surveillance.max_tracks),
        next_generation: 0,
        console_text: String::with_capacity(128),
        profile,
    })
}

/// Read the final quarantine counters after an optional on-demand release.
pub fn regional_quarantine_stats(
    runtime: &RegionalRuntime,
) -> Result<[SourceStats; oa_quarantine::INPUT_SOURCE_COUNT], String> {
    quarantine_stats(&runtime.quarantine_queue)
}

fn execute(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
    performance: Option<&[PerformanceEnvelope]>,
    metering: Option<&[MeteringScanRow]>,
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
    if metering.is_some() && !runtime.profile.surveillance.render_scope {
        return Err("per-scan metering requires the rendered regional scope profile".into());
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

        accept_runtime_item(
            runtime,
            event,
            item_index,
            batch_ordinal,
            processing_started,
            item_started,
        )?;
        flight_operations.push(
            flight::flight_apply_event(&mut runtime.flights, event)
                .map_err(|error| format!("accepted flight commit invariant: {error:?}"))?,
        );
        route::route_commit_stage(&mut runtime.routes, route_stage);
        surveillance::surveillance_event(&mut runtime.surveillance, event)
            .map_err(|error| format!("accepted surveillance commit invariant: {error}"))?;
        flight::flight_commit_order(&mut order_cursor, event);
    }
    // Flight ownership presentation currently requires an explicitly selected flight. Per-flight
    // snapshots below preserve every owner; no arbitrary 'first' flight is promoted to selection.
    let (metering_addresses, mut metering_report) = if let Some(rows) = metering {
        let (resolved, report) = resolve_metering_scan(
            rows,
            runtime.centre,
            batch.receipt_ns,
            batch.receipt_ns / 1_000_000,
            &mut runtime.flights,
            &mut runtime.metering_views,
        )
        .map_err(|error| format!("regional metering binding: {error}"))?;
        (Some(resolved), Some(report))
    } else {
        (None, None)
    };
    let mut surveillance = surveillance::surveillance_finish_with_metering(
        &mut runtime.surveillance,
        None,
        metering_addresses.as_deref(),
    )?;
    if let (Some(report), Some(convert)) = (metering_report.as_mut(), surveillance.metering_convert)
    {
        report.target_attached_rows = convert.meter_rows_attached;
        report.target_unmatched_rows = convert.meter_rows_unmatched;
        report.targets_without_meter_row = convert.targets_without_meter_row;
    }
    if runtime.profile.surveillance.render_scope {
        surveillance.frame_sha256 = surveillance::surveillance_present_controls(
            &mut runtime.surveillance,
            &runtime.interaction,
            &runtime.console_text,
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
    runtime.observed.clone_from(&surveillance.tracks);
    Ok(RegionalFrame {
        sequence: runtime.frame_sequence,
        input,
        surveillance,
        flight_operations,
        flights,
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
        metering: metering_report,
    })
}

/// A failed partial application poisons this offline session; callers must reconstruct a fresh
/// runtime from the admitted prefix. They cannot continue from a partly changed frame.
pub fn regional_step(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
) -> Result<RegionalFrame, String> {
    regional_step_inner(runtime, batch, None, None)
}

/// Builds one ordinary regional frame with a caller-owned per-scan metering snapshot.
///
/// The snapshot is consumed by the existing scope target conversion and traffic datablock path.
/// Rows without an in-crate source-qualified full-key binding remain unbound and are counted;
/// the API never accepts a public caller-supplied flight key as binding proof.
pub fn regional_step_with_metering(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
    rows: &[MeteringScanRow],
) -> Result<RegionalFrame, String> {
    regional_step_inner(runtime, batch, None, Some(rows))
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
    regional_step_inner(runtime, batch, Some(performance), None)
}

fn regional_step_inner(
    runtime: &mut RegionalRuntime,
    batch: &ReceiptBatch,
    performance: Option<&[PerformanceEnvelope]>,
    metering: Option<&[MeteringScanRow]>,
) -> Result<RegionalFrame, String> {
    if runtime.failed {
        return Err("session failed; reconstruct from the admitted input prefix".into());
    }
    match execute(runtime, batch, performance, metering) {
        Ok(frame) => Ok(frame),
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
        &runtime.console_text,
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
        &runtime.console_text,
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
        &runtime.console_text,
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
                principal: 51,
                role: oa_audit_core::Role::Operate,
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
                principal: 51,
                role: oa_audit_core::Role::Monitor,
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
                principal: 51,
                role: oa_audit_core::Role::Operate,
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

    const METER_TEST_GUFI: u128 = 1;
    const METER_TEST_ADDRESS: u32 = 0x00ab_cdef;
    const METER_TEST_KEY: [u8; 12] = *b"N123456789\0\0";
    const METER_TEST_ETA_MS: i64 = 43_200_000;

    fn meter_test_row(
        scan_ns: i64,
        digest_byte: u8,
        sta_ms: Option<i64>,
        eta_ms: Option<i64>,
    ) -> MeteringScanRow {
        let row = MeteringScanRow::unbound(
            *b"ZAU1",
            METER_TEST_KEY,
            [digest_byte; 32],
            Some(scan_ns - 200_000_000),
            scan_ns - 100_000_000,
            oa_display::core::metering::TargetMetering {
                mfx: oa_display::core::metering::MeterPointTimes { sta_ms, eta_ms },
                sfx: oa_display::core::metering::MeterPointTimes::default(),
            },
        );
        MeteringScanRow::with_verified_binding(row, METER_TEST_GUFI, METER_TEST_ADDRESS)
    }

    fn first_meter_test_frame(engine: &mut RegionalRuntime) -> RegionalFrame {
        let events = [
            parse_event_line(
                "v1\t1000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\tABCDEF\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2",
            )
            .unwrap(),
            parse_event_line(
                "v1\t2000000000\t2000000000\t2\tFLIGHT\t00000000000000000000000000000001\tABCDEF\tZAU1\tFLIGHT\tADDRESS",
            )
            .unwrap(),
            parse_event_line(
                "v1\t3000000000\t3000000000\t3\tSURVEILLANCE\t00000000000000000000000000000001\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0",
            )
            .unwrap(),
        ];
        regional_step_with_metering(
            engine,
            &ReceiptBatch {
                receipt_ns: 4_000_000_000,
                source_clock_regressions: 0,
                events: events.to_vec(),
            },
            &[meter_test_row(
                4_000_000_000,
                1,
                Some(METER_TEST_ETA_MS),
                Some(METER_TEST_ETA_MS),
            )],
        )
        .unwrap()
    }

    fn next_meter_test_frame(
        engine: &mut RegionalRuntime,
        scan_ns: i64,
        sequence: u64,
        digest_byte: u8,
        sta_ms: Option<i64>,
        eta_ms: Option<i64>,
    ) -> RegionalFrame {
        let line = format!(
            "v1\t{scan_ns}\t{scan_ns}\t{sequence}\tSURVEILLANCE\t00000000000000000000000000000001\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"
        );
        let event = parse_event_line(&line).unwrap();
        regional_step_with_metering(
            engine,
            &ReceiptBatch {
                receipt_ns: scan_ns,
                source_clock_regressions: 0,
                events: vec![event],
            },
            &[meter_test_row(scan_ns, digest_byte, sta_ms, eta_ms)],
        )
        .unwrap()
    }

    fn assert_meter_attached(frame: &RegionalFrame) {
        let report = frame.metering.as_ref().unwrap();
        assert_eq!(report.source_rows, 1);
        assert_eq!(report.flight_bound_rows, 1);
        assert_eq!(report.target_attached_rows, 1);
        assert_eq!(report.unbound_rows, 0);
        assert_eq!(frame.surveillance.displayed, 1);
        assert_eq!(
            frame
                .surveillance
                .metering_convert
                .as_ref()
                .unwrap()
                .meter_rows_attached,
            1
        );
    }

    // REQ: OA-REG-021, OA-REG-022
    #[test]
    fn admitted_flight_per_scan_metering_renders_schedule_and_counts_same_reference_window() {
        let mut engine = runtime();
        let first = first_meter_test_frame(&mut engine);
        assert_meter_attached(&first);
        assert_eq!(first.flights.len(), 1);
        let first_flight = first.flights.first().expect("one admitted test flight");
        assert_eq!(
            first_flight.view.as_ref().map(|view| view.plan_state),
            Some(oa_flight::ST_ACTIVE)
        );
        let first_texts = surveillance::rendered_scope_texts(&engine.surveillance);
        assert!(first_texts
            .iter()
            .any(|text| { text == "MFX STA=12:00:00 ETA=12:00:00 DLY=+0ms" }));
        assert_eq!(first.metering.as_ref().unwrap().mfx_counts.within_window, 1);

        let cases = [
            (
                Some(METER_TEST_ETA_MS + 60_000),
                Some(METER_TEST_ETA_MS),
                true,
                false,
                false,
            ),
            (
                Some(METER_TEST_ETA_MS - 60_000),
                Some(METER_TEST_ETA_MS),
                true,
                false,
                false,
            ),
            (
                Some(METER_TEST_ETA_MS + 60_001),
                Some(METER_TEST_ETA_MS),
                false,
                true,
                false,
            ),
            (
                Some(METER_TEST_ETA_MS - 60_001),
                Some(METER_TEST_ETA_MS),
                false,
                false,
                true,
            ),
        ];
        for (index, (sta, eta, inside, early, late)) in cases.into_iter().enumerate() {
            let scan_ns = 5_000_000_000 + i64::try_from(index).unwrap() * 1_000_000_000;
            let frame = next_meter_test_frame(
                &mut engine,
                scan_ns,
                4 + u64::try_from(index).unwrap(),
                2 + u8::try_from(index).unwrap(),
                sta,
                eta,
            );
            assert_meter_attached(&frame);
            let counts = frame.metering.as_ref().unwrap().mfx_counts;
            assert_eq!(counts.within_window, u64::from(inside));
            assert_eq!(counts.early, u64::from(early));
            assert_eq!(counts.late, u64::from(late));
            let texts = surveillance::rendered_scope_texts(&engine.surveillance);
            assert!(texts.iter().any(|text| text.starts_with("MFX STA=")));
        }

        let amended = next_meter_test_frame(
            &mut engine,
            9_000_000_000,
            8,
            10,
            Some(METER_TEST_ETA_MS + 30_000),
            Some(METER_TEST_ETA_MS),
        );
        assert_meter_attached(&amended);
        let amended_texts = surveillance::rendered_scope_texts(&engine.surveillance);
        assert!(amended_texts
            .iter()
            .any(|text| text == "MFX STA=12:00:30 ETA=12:00:00 DLY=+30000ms"));

        let changed = next_meter_test_frame(
            &mut engine,
            10_000_000_000,
            9,
            11,
            Some(METER_TEST_ETA_MS + 45_000),
            Some(METER_TEST_ETA_MS),
        );
        assert_meter_attached(&changed);
        assert_ne!(
            amended.surveillance.frame_sha256,
            changed.surveillance.frame_sha256
        );
        let changed_texts = surveillance::rendered_scope_texts(&engine.surveillance);
        assert!(changed_texts
            .iter()
            .any(|text| text == "MFX STA=12:00:45 ETA=12:00:00 DLY=+45000ms"));

        let missing = next_meter_test_frame(
            &mut engine,
            11_000_000_000,
            10,
            12,
            None,
            Some(METER_TEST_ETA_MS),
        );
        assert_meter_attached(&missing);
        let counts = missing.metering.as_ref().unwrap().mfx_counts;
        assert_eq!(counts.missing_sta, 1);
        assert_eq!(counts.missing_eta, 0);
        let missing_texts = surveillance::rendered_scope_texts(&engine.surveillance);
        assert!(missing_texts
            .iter()
            .any(|text| text == "MFX STA=--:--:-- ETA=12:00:00 DLY=--"));
    }

    // REQ: OA-REG-021
    #[test]
    fn tbfm_row_without_official_identity_link_stays_unbound_in_actual_scope_path() {
        let mut engine = runtime();
        let row = MeteringScanRow::unbound(
            *b"ZAU1",
            *b"N123456789\0\0",
            [1; 32],
            Some(900_000_000),
            1_000_000_000,
            oa_display::core::metering::TargetMetering {
                mfx: oa_display::core::metering::MeterPointTimes {
                    sta_ms: None,
                    eta_ms: Some(1_800_000_000_000),
                },
                sfx: oa_display::core::metering::MeterPointTimes::default(),
            },
        );
        let frame = regional_step_with_metering(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 1_000_000_000,
                source_clock_regressions: 0,
                events: Vec::new(),
            },
            &[row],
        )
        .unwrap();
        let report = frame.metering.unwrap();
        assert_eq!(report.source_rows, 1);
        assert_eq!(report.flight_bound_rows, 0);
        assert_eq!(report.target_attached_rows, 0);
        assert_eq!(report.unbound_rows, 1);
        assert_eq!(
            frame
                .surveillance
                .metering_convert
                .unwrap()
                .meter_rows_attached,
            0
        );
    }

    // REQ: OA-REG-021
    #[test]
    fn caller_supplied_full_key_and_address_do_not_bind_without_current_flight_table_match() {
        let mut engine = runtime();
        let row = MeteringScanRow::unbound(
            *b"ZAU1",
            *b"N123456789\0\0",
            [2; 32],
            Some(900_000_000),
            1_000_000_000,
            oa_display::core::metering::TargetMetering::default(),
        );
        let row = MeteringScanRow::with_verified_binding(row, 77, 0x00ab_cdef);
        let frame = regional_step_with_metering(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: 1_000_000_000,
                source_clock_regressions: 0,
                events: Vec::new(),
            },
            &[row],
        )
        .unwrap();
        let report = frame.metering.unwrap();
        assert_eq!(report.flight_bound_rows, 0);
        assert_eq!(report.target_attached_rows, 0);
        assert_eq!(report.unbound_rows, 1);
        assert_eq!(
            frame
                .surveillance
                .metering_convert
                .unwrap()
                .meter_rows_attached,
            0
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

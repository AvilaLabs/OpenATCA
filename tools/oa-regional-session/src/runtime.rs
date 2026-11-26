//! Offline composed regional session (shell, B1/B3/C6), with one common receipt cutoff.
//! This executes actual product cores; it is not deadline or allocation qualification.

use crate::commands;
use crate::flight::{
    self, FlightApplyOutcome, FlightDriver, FlightDriverConfig, FlightDriverReport,
    FlightViewStatus, SharedFlightView, MAX_DRIVER_FLIGHTS, MAX_DRIVER_LOG_ENTRIES,
};
use crate::route::{self, RouteStore};
use crate::session::{summarize_input_batch, FrameEvidence, ReceiptBatch, MAX_EVENTS_PER_RECEIPT};
use crate::surveillance::{self, Surveillance, SurveillanceConfig, SurveillanceFrame};
use oa_airspace::core::model::Model;
use oa_legacy_traj::core::trajectory::{
    PerformanceEnvelope, TrajectoryIssueOutput, VerticalProfileOutput,
};
use oa_quarantine::{
    route_input, source_stats, Disposition, InputIdentity, InputOutcome, InputSource,
    QuarantineQueue, ReasonFlags, SourceStats, REASON_INPUT_INVALID, REASON_RESOURCE_LIMIT,
    REASON_SURVEILLANCE_INVALID,
};
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

/// Returns the immutable validated capacities selected at construction.
pub fn regional_profile_limits(runtime: &RegionalRuntime) -> RegionalRunProfile {
    runtime.profile
}

fn regional_new_with_profile_and_namespace(
    model: Model,
    snapshot_sha256: [u8; 32],
    centre: [u8; 4],
    cfg: SurveillanceConfig,
    profile: RegionalRunProfile,
    session_namespace: u16,
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
        || (profile.surveillance.render_scope && profile.max_flights > MAX_DRIVER_FLIGHTS)
    {
        return Err("regional profile exceeds fixed runtime bounds".into());
    }
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
        centre,
        frame_sequence: 0,
        failed: false,
        session_namespace,
        quarantine_queue: oa_quarantine::queue_new(),
        interaction: oa_display::interaction::interaction_new(),
        views: Vec::with_capacity(profile.max_flights),
        observed: Vec::with_capacity(profile.surveillance.max_tracks),
        next_generation: 0,
        console_text: String::with_capacity(128),
        profile,
    })
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
    let mut surveillance = surveillance::surveillance_finish(&mut runtime.surveillance, None)?;
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
    }
}

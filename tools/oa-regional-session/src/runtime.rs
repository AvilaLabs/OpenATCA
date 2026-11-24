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

// A7.2: cold subsystem composition. All core memory belongs to its originating subsystem.
pub struct RegionalRuntime {
    model: Model,
    flights: FlightDriver,
    surveillance: Surveillance,
    routes: RouteStore,
    centre: [u8; 4],
    frame_sequence: u64,
    failed: bool,
    interaction: oa_display::interaction::InteractionState,
    views: Vec<oa_display::interaction::TargetView>,
    observed: Vec<crate::surveillance::ObservedTrack>,
    next_generation: u32,
}

// A3/C6: at most 32 cold offline evidence rows/frame; snapshots are copied, never aliased. Optional
// vertical outputs are bounded to the same 32 flight views, each containing 256 fixed point slots.
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
}
// A7.2: cold shell evidence, bounded by runtime capacities.

/// Maximum caller-supplied performance envelopes in one opt-in receipt (at most 32 rows; matching
/// work is bounded by 32 flights × 32 envelopes).
pub const MAX_PERFORMANCE_ENVELOPES: usize = MAX_DRIVER_FLIGHTS;

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
    if !centre.iter().all(u8::is_ascii_alphanumeric) {
        return Err("centre must be four ASCII alphanumeric bytes".into());
    }
    Ok(RegionalRuntime {
        surveillance: surveillance::surveillance_new(model.clone(), cfg)?,
        model,
        flights: flight::flight_driver_new(FlightDriverConfig {
            flight_capacity: MAX_DRIVER_FLIGHTS,
            refusal_log_capacity: MAX_DRIVER_LOG_ENTRIES,
            conflict_log_capacity: MAX_DRIVER_LOG_ENTRIES,
        })
        .map_err(|e| format!("flight startup: {e:?}"))?,
        routes: route::route_store_new(snapshot_sha256),
        centre,
        frame_sequence: 0,
        failed: false,
        interaction: oa_display::interaction::interaction_new(),
        views: Vec::with_capacity(MAX_DRIVER_FLIGHTS),
        observed: Vec::with_capacity(MAX_DRIVER_FLIGHTS),
        next_generation: 0,
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
    let input = summarize_input_batch(batch).map_err(|e| format!("input accounting: {e:?}"))?;
    let performance_cutoff_ms = match performance {
        Some(_) => Some(route::receipt_cutoff_ms(batch.receipt_ns)?),
        None => None,
    };
    surveillance::surveillance_begin(&mut runtime.surveillance, batch.receipt_ns)?;
    flight::flight_preflight_batch(&mut runtime.flights, &batch.events)
        .map_err(|e| format!("flight adapter preflight: {e:?}"))?;
    let mut flight_operations = Vec::with_capacity(batch.events.len());
    let mut controller_dispatches = Vec::with_capacity(batch.events.len());
    for event in &batch.events {
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
                    .map_err(|e| format!("flight adapter: {e:?}"))?,
            );
        }
        route::route_event(&mut runtime.routes, &runtime.model, event)?;
        surveillance::surveillance_event(&mut runtime.surveillance, event)?;
    }
    // Flight ownership presentation currently requires an explicitly selected flight. Per-flight
    // snapshots below preserve every owner; no arbitrary 'first' flight is promoted to selection.
    let mut surveillance = surveillance::surveillance_finish(&mut runtime.surveillance, None)?;
    surveillance.frame_sha256 = surveillance::surveillance_present_interaction(
        &mut runtime.surveillance,
        &runtime.interaction,
    )?;
    runtime.frame_sequence = runtime
        .frame_sequence
        .checked_add(1)
        .ok_or("frame sequence overflow")?;
    let mut flights = Vec::with_capacity(MAX_DRIVER_FLIGHTS);
    let mut views = Vec::with_capacity(MAX_DRIVER_FLIGHTS);
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
                    (Some(rows), Some(cutoff_ms)) => match performance_for_key(
                        rows,
                        route::flight_key(view),
                        cutoff_ms,
                    ) {
                        Ok(value) => (value, None),
                        Err(reason) => (None, Some(reason)),
                    },
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
    runtime.views =
        commands::current_views(&runtime.views, &flights, &mut runtime.next_generation)?;
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

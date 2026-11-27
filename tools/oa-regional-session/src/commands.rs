//! Local controller request bridge (shell, B1/B3/C6). UI preflight is followed by a fresh
//! authoritative flight/address/plan/actor check; transport or UI acceptance is not ownership.

use crate::flight::{self, FlightApplyOutcome, FlightDriver, FlightViewStatus};
use crate::input::{
    EventKind, FlightOperation, HandoffOperation, NormalizedEvent, OperatorCommand,
};
use crate::runtime::FlightEvidence;
use crate::surveillance::ObservedTrack;
use oa_display::interaction::{self as ui, FlightIdentity, Intent, InteractionResult, TargetView};
use std::path::Path;

pub struct MsawServiceAuditContext<'a> {
    pub audit_log: &'a std::path::Path,
    pub checkpoint: &'a mut oa_audit_core::Checkpoint,
    pub registry: &'a oa_audit_log::shell::PinnedPrincipalRegistry,
    pub principal: u64,
}

pub struct DispatchContext<'a> {
    pub driver: &'a mut FlightDriver,
    pub interaction: &'a mut ui::InteractionState,
    pub surveillance: &'a mut crate::surveillance::Surveillance,
    pub views: &'a [TargetView],
    pub observed: &'a [ObservedTrack],
    pub view_sequence: u32,
}

/// Development local actor registry, stable across replay. This is explicit local actor input,
/// not network authentication; no UI request can bootstrap an authoritative owner.
pub const UNITS: [[u8; 8]; 2] = [*b"SECTOR1\0", *b"SECTOR2\0"];

// A7.2: cold offline outcome; at most 256 requests per receipt batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispatch {
    pub input_sequence: u64,
    pub request: OperatorCommand,
    pub outcome: Option<FlightApplyOutcome>,
    pub refusal: String,
    /// A queue invariant failure means the runtime cannot safely publish or continue this frame.
    pub terminal_failure: bool,
}

/// Audit configuration for the typed OpenATCA hold command caller. The registry digest and
/// checkpoint must come from an independent trusted configuration source; this is not a login or
/// ERAM command grammar.
#[derive(Clone, Copy, Debug)]
pub struct HoldCommandAuthority<'a> {
    pub principal_id: u64,
    pub registry_path: &'a Path,
    pub expected_registry_sha256: [u8; 32],
    pub audit_path: &'a Path,
    pub expected_checkpoint: oa_audit_core::Checkpoint,
    /// Receipt-clock time for this command, in the same nanosecond domain as regional scans.
    pub command_receipt_ns: i64,
}

/// Dispatch one typed project hold assign/amend/cancel operation through the regional runtime's
/// pinned-role and durable-intent gate. Exact deployed ERAM syntax and UI behavior remain outside
/// this API.
pub fn dispatch_hold_command(
    runtime: &mut crate::runtime::RegionalRuntime,
    update: crate::holding::HoldUpdate,
    authority: HoldCommandAuthority<'_>,
) -> Result<crate::runtime::HoldCommandResult, String> {
    crate::runtime::regional_dispatch_hold_command(runtime, update, authority)
}

#[derive(Debug)]
struct DispatchError {
    reason: String,
    terminal_failure: bool,
}

fn refusal(reason: impl Into<String>) -> DispatchError {
    DispatchError {
        reason: reason.into(),
        terminal_failure: false,
    }
}
fn terminal_failure(reason: impl Into<String>) -> DispatchError {
    DispatchError {
        reason: reason.into(),
        terminal_failure: true,
    }
}

fn unit(id: u32) -> Result<[u8; 8], String> {
    let ix = id.checked_sub(1).ok_or("unknown local controller unit")?;
    UNITS
        .get(usize::try_from(ix).map_err(|_| "unit index width")?)
        .copied()
        .ok_or_else(|| "unknown local controller unit".into())
}

fn identity(view: &crate::flight::SharedFlightView) -> FlightIdentity {
    FlightIdentity {
        gufi: view.gufi.to_be_bytes(),
        aircraft_address: view.address,
        centre: view.centre,
        plan_id: view.plan_id,
    }
}

/// Generation values are monotonic across all appearances/binding changes in this session;
/// deleting an address from the current view never frees its generation for later reuse.
pub fn current_views(
    prior: &[TargetView],
    flights: &[FlightEvidence],
    next_generation: &mut u32,
    capacity: usize,
) -> Result<Vec<TargetView>, String> {
    if capacity == 0
        || capacity > crate::flight::MAX_OFFLINE_DRIVER_FLIGHTS
        || prior.len() > capacity
        || flights.len() > capacity
    {
        return Err("controller view capacity exceeded".into());
    }
    let empty = FlightIdentity {
        gufi: [0; 16],
        aircraft_address: 0,
        centre: [0; 4],
        plan_id: [0; 12],
    };
    let mut out = Vec::with_capacity(capacity);
    for flight in flights {
        let bound = flight.view.map(|view| identity(&view));
        let claims = u16::try_from(flight.claimants)
            .map_err(|_| "controller identity claimant count overflow")?;
        let mut row = TargetView {
            address: flight.address,
            generation: 0,
            identity_claims: claims,
            identity_present: u8::from(bound.is_some()),
            reserved: 0,
            identity: bound.unwrap_or(empty),
        };
        let previous = prior.iter().find(|row| row.address == flight.address);
        if let Some(previous) = previous.filter(|p| {
            p.identity_claims == claims
                && p.identity_present == row.identity_present
                && p.identity == row.identity
        }) {
            row.generation = previous.generation;
        } else {
            *next_generation = next_generation
                .checked_add(1)
                .ok_or("controller generation exhausted")?;
            row.generation = *next_generation;
        }
        out.push(row);
    }
    Ok(out)
}

// Private dispatch flow returns a named refusal; no refusal silently changes a flight.
fn finish_queued(
    interaction: &mut ui::InteractionState,
    seq: u32,
    outcome: ui::CommandOutcome,
) -> Result<(), DispatchError> {
    let Some(head) = ui::next_intent(interaction) else {
        return Err(terminal_failure(
            "operator queue head missing after enqueue",
        ));
    };
    if head.input_sequence != seq {
        return Err(terminal_failure(
            "operator queue head changed after enqueue",
        ));
    }
    if ui::complete_oldest(interaction, seq, outcome) != InteractionResult::Applied {
        return Err(terminal_failure(
            "operator queue completion invariant failed",
        ));
    }
    if ui::next_intent(interaction).is_some() {
        return Err(terminal_failure(
            "operator queue retained an unexpected command after completion",
        ));
    }
    Ok(())
}

fn refuse_queued(
    interaction: &mut ui::InteractionState,
    seq: u32,
    reason: impl Into<String>,
    stale: bool,
) -> Result<FlightApplyOutcome, DispatchError> {
    finish_queued(
        interaction,
        seq,
        if stale {
            ui::CommandOutcome::Stale
        } else {
            ui::CommandOutcome::Refused
        },
    )?;
    Err(refusal(reason))
}

fn apply(
    context: DispatchContext<'_>,
    event: &NormalizedEvent,
    request: OperatorCommand,
    msaw_audit: Option<&mut MsawServiceAuditContext<'_>>,
) -> Result<FlightApplyOutcome, DispatchError> {
    let DispatchContext {
        driver,
        interaction,
        surveillance,
        views,
        observed,
        view_sequence,
    } = context;
    if ui::next_intent(interaction).is_some() {
        return Err(terminal_failure(
            "operator queue was nonempty before dispatch",
        ));
    }
    if !matches!(event.kind, EventKind::Operator(command) if command == request)
        || event.source != crate::input::SourceKind::Flight
    {
        return Err(refusal(
            "operator bridge requires a Flight sourced typed command",
        ));
    }
    if event.source_ns < 0
        || event.receipt_ns < 0
        || event.source_ns > event.receipt_ns
        || event.source_ns % 1_000_000 != 0
        || event.receipt_ns % 1_000_000 != 0
        || oa_sealed::in_sealed_window(event.source_ns)
        || oa_sealed::in_sealed_window(event.receipt_ns)
    {
        return Err(refusal(
            "operator timestamp is invalid, sealed, or loses millisecond precision",
        ));
    }
    let seq = u32::try_from(event.sequence)
        .map_err(|_| refusal("operator input sequence exceeds UI bound"))?;
    let key = FlightIdentity {
        gufi: event.identity.flight_gufi.to_be_bytes(),
        aircraft_address: event.identity.aircraft_address,
        centre: event.centre,
        plan_id: request.plan_id,
    };
    match ui::select_target(
        interaction,
        views,
        view_sequence,
        key.aircraft_address,
        request.target_generation,
        Some(key),
    ) {
        InteractionResult::Applied => {}
        other => return Err(refusal(format!("operator selection: {other:?}"))),
    }
    let time = u64::try_from(event.source_ns / 1000)
        .map_err(|_| refusal("operator issue timestamp overflow"))?;
    match ui::enqueue_intent(interaction, views, view_sequence, seq, time, request.intent) {
        InteractionResult::Applied => {}
        other => return Err(refusal(format!("operator enqueue: {other:?}"))),
    }
    let queued_refusal = |interaction: &mut ui::InteractionState, reason: String, stale: bool| {
        refuse_queued(interaction, seq, reason, stale)
    };
    if ui::next_intent(interaction).is_none_or(|head| head.input_sequence != seq) {
        return Err(terminal_failure(
            "operator queue head changed immediately after enqueue",
        ));
    }
    if request.view_sequence != view_sequence || view_sequence == 0 {
        return queued_refusal(
            interaction,
            "operator request names a stale or unavailable frame".into(),
            true,
        );
    }
    let Some(age) = event.receipt_ns.checked_sub(event.source_ns) else {
        return queued_refusal(interaction, "operator source age overflow".into(), true);
    };
    if age > 5_000_000_000 {
        return queued_refusal(interaction, "operator request itself is stale".into(), true);
    }
    let Some(target) = observed
        .iter()
        .find(|t| t.address == event.identity.aircraft_address)
    else {
        return queued_refusal(
            interaction,
            "operator track is no longer visible".into(),
            true,
        );
    };
    let Some(track_age) = event.receipt_ns.checked_sub(target.newest_source_ns) else {
        return queued_refusal(interaction, "operator track age overflow".into(), true);
    };
    if !(0..=5_000_000_000).contains(&track_age) {
        return queued_refusal(interaction, "operator track evidence is stale".into(), true);
    }
    let actor = match unit(request.actor_unit) {
        Ok(actor) => actor,
        Err(e) => return queued_refusal(interaction, e, false),
    };
    let mut current = Vec::with_capacity(flight::flight_driver_capacity(driver));
    let result = flight::flight_views_for_address(
        driver,
        key.aircraft_address,
        event.centre,
        event.receipt_ns / 1_000_000,
        &mut current,
    );
    if result.status != FlightViewStatus::Found || current.len() != 1 {
        return queued_refusal(
            interaction,
            format!("operator authoritative identity: {:?}", result.status),
            false,
        );
    }
    let Some(view) = current.first() else {
        return Err(terminal_failure(
            "operator authoritative view missing after Found",
        ));
    };
    if identity(view) != key {
        return queued_refusal(
            interaction,
            "operator full identity changed after displayed frame".into(),
            true,
        );
    }
    if view.plan_state != oa_flight::ST_ACTIVE && view.plan_state != oa_flight::ST_DEPARTED {
        return queued_refusal(
            interaction,
            "operator flight is not active/departed".into(),
            false,
        );
    }
    let aural_ack = match request.intent {
        Intent::AcknowledgeMsaw { alert_start_ns } if actor == view.owner_unit => {
            Some(alert_start_ns)
        }
        Intent::AcknowledgeMsaw { .. } => {
            return queued_refusal(
                interaction,
                "operator unit does not own the selected flight for MSAW acknowledgement".into(),
                false,
            )
        }
        _ => None,
    };
    let service_request = match request.intent {
        Intent::RequestMsawService { expires_at_ns } if actor == view.owner_unit => {
            Some(expires_at_ns)
        }
        Intent::RequestMsawService { .. } => {
            return queued_refusal(
                interaction,
                "operator unit does not own the selected flight for MSAW service request".into(),
                false,
            )
        }
        _ => None,
    };
    let operation = if aural_ack.is_some() || service_request.is_some() {
        None
    } else {
        Some(match request.intent {
            Intent::SetAssignedAltitude(altitude_ft) if actor == view.owner_unit => {
                FlightOperation::AssignAltitude { altitude_ft }
            }
            Intent::ClearAssignedAltitude if actor == view.owner_unit => {
                FlightOperation::ClearAssignedAltitude
            }
            Intent::SetInterimAltitude(altitude_ft) if actor == view.owner_unit => {
                FlightOperation::SetInterimAltitude {
                    altitude_ft: Some(altitude_ft),
                }
            }
            Intent::ClearInterimAltitude if actor == view.owner_unit => {
                FlightOperation::SetInterimAltitude { altitude_ft: None }
            }
            Intent::OfferHandoff(to) if actor == view.owner_unit && to != request.actor_unit => {
                let offered = match unit(to) {
                    Ok(unit) => unit,
                    Err(e) => return queued_refusal(interaction, e, false),
                };
                FlightOperation::Handoff {
                    operation: HandoffOperation::Offer,
                    unit: offered,
                }
            }
            Intent::AcceptHandoff(by) if by == request.actor_unit && actor == view.handoff_to => {
                FlightOperation::Handoff {
                    operation: HandoffOperation::Accept,
                    unit: actor,
                }
            }
            Intent::TakeHandoff(by) if by == request.actor_unit && actor == view.handoff_to => {
                FlightOperation::Handoff {
                    operation: HandoffOperation::Take,
                    unit: actor,
                }
            }
            Intent::CancelHandoff(by) if by == request.actor_unit && actor == view.owner_unit => {
                FlightOperation::Handoff {
                    operation: HandoffOperation::Fail,
                    unit: actor,
                }
            }
            _ => {
                return queued_refusal(
                    interaction,
                    "operator unit has no authority for this request".into(),
                    false,
                )
            }
        })
    };
    // Recheck both queue identity and command payload before applying authoritative state.
    if ui::next_intent(interaction).is_none_or(|head| {
        head.input_sequence != seq || head.identity != key || head.intent != request.intent
    }) {
        return Err(terminal_failure(
            "operator queued command does not match validated request",
        ));
    }
    if let other @ InteractionResult::Refused(_) =
        ui::preflight_oldest(interaction, views, view_sequence)
    {
        return queued_refusal(interaction, format!("operator dispatch: {other:?}"), true);
    }
    let result = if let Some(alert_start_ns) = aural_ack {
        crate::surveillance::surveillance_acknowledge_msaw_aural(
            surveillance,
            key.aircraft_address,
            alert_start_ns,
            event.source_ns,
        )
        .map_err(refusal)
        .map(|()| FlightApplyOutcome {
            operation: None,
            adapter_refusal: None,
            ignored_non_flight_event: true,
        })
    } else if let Some(expires_at_ns) = service_request {
        let Some(audit) = msaw_audit else {
            finish_queued(interaction, seq, ui::CommandOutcome::Refused)?;
            return Err(refusal(
                "MSAW service request requires pinned registry and durable audit context",
            ));
        };
        let principal = audit.principal;
        audit_msaw_service_request(
            audit,
            surveillance,
            crate::surveillance::MsawServiceRequestUpdate {
                identity: key,
                generation: request.target_generation,
                identity_stamp: view.identity_stamp,
                address_stamp: view.address_stamp,
                expires_at_ns,
                input_sequence: event.sequence,
                principal,
                now_ns: event.receipt_ns,
                view_sequence: request.view_sequence,
            },
        )
        .map_err(refusal)
        .map(|()| FlightApplyOutcome {
            operation: None,
            adapter_refusal: None,
            ignored_non_flight_event: true,
        })
    } else if let Some(operation) = operation {
        let mut authoritative = *event;
        authoritative.kind = EventKind::Flight(operation);
        flight::flight_apply_event(driver, &authoritative).map_err(|e| {
            terminal_failure(format!("operator flight operation after preflight: {e:?}"))
        })
    } else {
        Err(terminal_failure(
            "operator operation was absent after preflight",
        ))
    };
    let accepted = result.as_ref().is_ok_and(|outcome| {
        aural_ack.is_some()
            || service_request.is_some()
            || (outcome.adapter_refusal.is_none()
                && outcome.operation.is_some_and(|operation| {
                    matches!(
                        operation,
                        oa_flight::Outcome::Applied | oa_flight::Outcome::Unchanged
                    )
                }))
    });
    finish_queued(
        interaction,
        seq,
        if accepted {
            ui::CommandOutcome::Accepted
        } else {
            ui::CommandOutcome::Refused
        },
    )?;
    result
}

pub fn dispatch(
    context: DispatchContext<'_>,
    event: &NormalizedEvent,
    request: OperatorCommand,
) -> Dispatch {
    dispatch_with_msaw_audit(context, event, request, None)
}

pub fn dispatch_with_msaw_audit(
    context: DispatchContext<'_>,
    event: &NormalizedEvent,
    request: OperatorCommand,
    msaw_audit: Option<&mut MsawServiceAuditContext<'_>>,
) -> Dispatch {
    let mut record = Dispatch {
        input_sequence: event.sequence,
        request,
        outcome: None,
        refusal: String::new(),
        terminal_failure: false,
    };
    let capacity = flight::flight_driver_capacity(context.driver);
    if context.views.len() > capacity
        || context.observed.len() > crate::flight::MAX_OFFLINE_DRIVER_FLIGHTS
    {
        record.refusal = "operator input budget exceeded".into();
        return record;
    }
    match apply(context, event, request, msaw_audit) {
        Ok(outcome) => record.outcome = Some(outcome),
        Err(error) => {
            record.refusal = error.reason;
            record.terminal_failure = error.terminal_failure;
        }
    }
    record
}

fn audit_msaw_service_request(
    context: &mut MsawServiceAuditContext<'_>,
    surveillance: &mut crate::surveillance::Surveillance,
    request: crate::surveillance::MsawServiceRequestUpdate,
) -> Result<(), String> {
    if context.principal == 0 || request.now_ns <= 0 || request.principal != context.principal {
        return Err("MSAW service audit principal/time must be positive".into());
    }
    if request.expires_at_ns != 0 {
        let duration = request
            .expires_at_ns
            .checked_sub(request.now_ns)
            .ok_or("MSAW request expiry overflow")?;
        if duration <= 0 || duration > crate::surveillance::MAX_MSAW_SERVICE_REQUEST_NS {
            return Err("MSAW request expiry is outside the five-minute bound".into());
        }
    }
    let role = context
        .registry
        .authorize(context.principal, oa_audit_core::Action::MsawServiceRequest)
        .map_err(|error| format!("MSAW service registry refused: {error:?}"))?;
    crate::surveillance::surveillance_validate_msaw_service_request(surveillance, request)?;
    let target_text = format!(
        "MSAW SERVICE {:032X} {:06X}",
        u128::from_be_bytes(request.identity.gufi),
        request.identity.aircraft_address
    );
    let (target, target_len) = oa_audit_core::target(target_text.as_bytes())
        .map_err(|error| format!("MSAW service target: {error:?}"))?;
    let mut text = String::new();
    text.try_reserve_exact(384)
        .map_err(|_| "MSAW service details allocation refused")?;
    use std::fmt::Write as _;
    writeln!(text,
        "MSAW-SERVICE-REQUEST-V1\noperation\t{}\nflight_gufi\t{:032X}\naircraft_address\t{:06X}\ncentre\t{}\nplan_id\t{}\ngeneration\t{}\nidentity_stamp\t{}:{}:{}\naddress_stamp\t{}:{}:{}\nview_sequence\t{}\ninput_sequence\t{}\nexpires_at_ns\t{}",
        if request.expires_at_ns == 0 { "REVOKE" } else { "ACTIVATE" },
        u128::from_be_bytes(request.identity.gufi), request.identity.aircraft_address,
        std::str::from_utf8(&request.identity.centre).unwrap_or("????"),
        std::str::from_utf8(&request.identity.plan_id).unwrap_or("????????????"),
        request.generation,
        request.identity_stamp.ts_ms, request.identity_stamp.avail_ms, request.identity_stamp.msg,
        request.address_stamp.ts_ms, request.address_stamp.avail_ms, request.address_stamp.msg,
        request.view_sequence, request.input_sequence, request.expires_at_ns)
        .map_err(|_| "MSAW service details formatting failed")?;
    let (details, details_len) = oa_audit_core::details(text.as_bytes())
        .map_err(|error| format!("MSAW service details: {error:?}"))?;
    let audit_event = oa_audit_core::Event {
        principal: context.principal,
        role,
        action: oa_audit_core::Action::MsawServiceRequest,
        target,
        target_len,
        time_ns: u64::try_from(request.now_ns).map_err(|_| "invalid MSAW audit timestamp")?,
        details,
        details_len,
    };
    oa_audit_core::validate_event(&audit_event)
        .map_err(|error| format!("MSAW service event: {error:?}"))?;
    let next_sequence = context
        .checkpoint
        .count
        .checked_add(1)
        .ok_or("MSAW service audit sequence exhausted")?;
    let (checkpoint, receipt) =
        oa_audit_log::shell::append_receipt(context.audit_log, *context.checkpoint, audit_event)
            .map_err(|error| format!("MSAW service audit append failed: {error:?}"))?;
    if checkpoint.count != next_sequence
        || checkpoint.tail != receipt.digest()
        || receipt.sequence() != next_sequence
        || !oa_audit_core::receipt_matches(&receipt, audit_event)
    {
        return Err("MSAW service audit receipt does not match request".into());
    }
    crate::surveillance::surveillance_commit_msaw_service_request(surveillance, request)?;
    *context.checkpoint = checkpoint;
    Ok(())
}

//! Session-admitted NAM point-out state, kept separate from flight ownership.
//!
//! Rev G §3.7 distinguishes POI/PLA computer receipt from the controller's operational decision:
//! Basic point-outs require manual coordination; only Enhanced POA/POJ messages express approval
//! or denial. This module never changes `oa-flight` ownership and never treats PLA/LAM as approval.

use crate::flight::{self, FlightDriver, FlightViewStatus, SharedFlightView};
use crate::nam_handoff::{
    accepted_delivery_seq, accepted_message, accepted_pointout_response,
    nam_session_ingress_local_event, nam_session_ingress_receive, nam_session_ingress_state,
    NamIngressResult, NamSessionIngress,
};
use crate::route;
use crate::surveillance::ObservedTrack;
use oa_flight::FlightKey;
use oa_legacy_traj::core::trajectory::FlightKey as RouteKey;
use oa_nam::{
    bare, encode,
    frame::{frame, unframe},
    parse, session, Field07, Field31, Header, Ident, Kind, Message, MsgId, Ssr,
};

pub const MAX_POINT_OUTS: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointOutMode {
    Basic,
    Enhanced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointOutStatus {
    /// A locally sent POI is awaiting its logical computer response.
    AwaitingComputerResponse,
    /// POI is computer-received only; operational coordination remains manual.
    AwaitingManualCoordination,
    /// Enhanced POI is displayed for an explicit controller decision.
    AwaitingControllerDecision,
    /// A locally sent Enhanced POI has received PLA and is waiting on the remote controller.
    AwaitingRemoteControllerDecision,
    /// The peer did not logically accept the point-out before the NAM response timeout.
    ResponseTimedOut,
    Approved,
    Denied,
    /// The computer refused the locally sent POI; this is a protocol refusal, not an operational decision.
    ComputerRejected,
}

/// The Rev G ICD's two conflicting POA/POJ response statements are explicit configuration.
/// §3.7.3.1/.4.1 expects a LAM/LRM; Annex B.1.8.5/.6 says no response is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointOutResponseProfile {
    IcdPartIiSection37Lam,
    IcdAnnexBNoResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointOutAckStatus {
    NotExpected,
    Awaiting,
    Received,
    Rejected,
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeaconComparison {
    Unknown,
    Stale,
    Future,
    Match,
    Mismatch,
    Conflict,
}

/// Explicit source binding. `None` preserves an unbound point-out; no callsign/code join is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointOutBinding {
    pub key: FlightKey,
    pub route_key: RouteKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointOutContext {
    pub binding: Option<PointOutBinding>,
    pub source_ns: i64,
    pub receipt_ns: i64,
    pub sequence: u64,
    /// Configured local sector required in the logical PLA response to a received POI.
    pub response_sector: Option<[u8; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointOutView {
    pub point_out_id: MsgId,
    pub from: [u8; 4],
    pub to: [u8; 4],
    pub key: Option<FlightKey>,
    pub route_key: Option<RouteKey>,
    pub address: Option<u32>,
    pub status: PointOutStatus,
    pub ack_status: PointOutAckStatus,
    /// Established code carried by the POI, if present.
    pub established_mode_a: Option<u16>,
    /// Receiving-controller assigned code carried by POA, if present.
    pub assigned_mode_a: Option<u16>,
    /// POI established code compared with the positively bound observed track.
    pub observed_comparison: BeaconComparison,
    /// POA assigned code compared with the POI established code.
    pub assigned_comparison: BeaconComparison,
    pub source_ns: i64,
    pub receipt_ns: i64,
    pub context_sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointOutError {
    NotAdmitted,
    WrongKind,
    WrongPeer,
    MissingReference,
    ReferenceMismatch,
    UnsupportedEnhancedMessage,
    InvalidContext,
    ResponseProfileRequired,
    MissingFields,
    InvalidCode,
    Duplicate,
    Capacity,
    InvalidTime,
    UnknownFlight,
    AmbiguousFlight,
    FutureFlight,
    FlightMismatch,
    SessionResponse,
    MessageNumberInUse,
    PendingResponse,
    InvalidTransition,
}

#[derive(Clone, Debug)]
pub struct PointOutResult {
    pub ingress: NamIngressResult,
    /// Logical computer response actions, when POI needs a PLA. They do not represent approval.
    pub response_actions: Option<session::Actions>,
    /// Exact encoded PLA when the caller supplied its configured response sector.
    pub response_framed: Option<Vec<u8>>,
    pub view: Option<PointOutView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointOutDecision {
    Approve { assigned_mode_a: Option<u16> },
    Deny,
}

#[derive(Clone, Debug)]
pub struct PointOutDecisionResult {
    /// Encoded POA or POJ for the caller's configured peer transport.
    pub framed_bytes: Vec<u8>,
    pub message: Message,
    pub actions: session::Actions,
    pub view: PointOutView,
}

#[derive(Clone, Debug)]
pub struct PointOutTransmitResult {
    pub framed_bytes: Vec<u8>,
    pub message: Message,
    pub actions: session::Actions,
    pub view: PointOutView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointOutRetirementReason {
    /// The operator confirms the aircraft exited delegated airspace and potential conflicts resolved.
    AirspaceExitedAndConflictsResolved,
    /// Basic verbal point-out coordination was closed by the operator.
    ManualCoordinationClosed,
}

#[derive(Clone, Debug)]
pub struct PointOutTickResult {
    pub actions: session::Actions,
    /// Exact prior ATM messages to resend unchanged after session timeout processing.
    pub retransmissions: Vec<Vec<u8>>,
    /// Point-out identifiers whose computer response timed out; operational POA/POJ status is retained.
    pub timed_out: Vec<MsgId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    view: PointOutView,
    source: Message,
    outbound: bool,
    decision_message: Option<Message>,
}

pub struct NamPointOutRuntime {
    mode: PointOutMode,
    response_profile: Option<PointOutResponseProfile>,
    records: Vec<Record>,
    capacity: usize,
}

pub fn nam_pointout_runtime_new(capacity: usize, mode: PointOutMode) -> Option<NamPointOutRuntime> {
    if capacity == 0 || capacity > MAX_POINT_OUTS {
        return None;
    }
    Some(NamPointOutRuntime {
        mode,
        response_profile: None,
        records: Vec::with_capacity(capacity),
        capacity,
    })
}

pub fn nam_pointout_set_mode(
    runtime: &mut NamPointOutRuntime,
    mode: PointOutMode,
) -> Result<(), PointOutError> {
    if !runtime.records.is_empty() {
        return Err(PointOutError::InvalidContext);
    }
    if mode == PointOutMode::Enhanced && runtime.response_profile.is_none() {
        return Err(PointOutError::ResponseProfileRequired);
    }
    runtime.mode = mode;
    Ok(())
}

pub fn nam_pointout_set_response_profile(
    runtime: &mut NamPointOutRuntime,
    response_profile: PointOutResponseProfile,
) -> Result<(), PointOutError> {
    if !runtime.records.is_empty() {
        return Err(PointOutError::InvalidContext);
    }
    runtime.response_profile = Some(response_profile);
    Ok(())
}

pub fn nam_pointout_views(runtime: &NamPointOutRuntime) -> impl Iterator<Item = &PointOutView> {
    runtime.records.iter().map(|record| &record.view)
}

/// Drive the actual NAM peer timer and map POI/POA/POJ resend and timeout actions back to their
/// bounded point-out records. Other NAM session actions remain visible in `actions` to the caller.
pub fn nam_pointout_tick(
    runtime: &mut NamPointOutRuntime,
    ingress: &mut NamSessionIngress,
    now_ms: u64,
) -> Result<PointOutTickResult, PointOutError> {
    let actions = nam_session_ingress_local_event(ingress, session::Event::Tick, now_ms)
        .map_err(|_| PointOutError::SessionResponse)?;
    let mut retransmissions = Vec::with_capacity(session::MAX_ACTIONS);
    let mut timed_out = Vec::with_capacity(session::MAX_ACTIONS);
    for action in actions.as_slice() {
        match action {
            session::Action::ResendAtm { kind, number }
                if matches!(kind, Kind::Poi | Kind::Poa | Kind::Poj) =>
            {
                let message = runtime
                    .records
                    .iter()
                    .find_map(|record| {
                        if record.outbound
                            && record.source.header.kind == *kind
                            && record.source.header.id.number == *number
                        {
                            Some(&record.source)
                        } else {
                            record.decision_message.as_ref().filter(|message| {
                                message.header.kind == *kind && message.header.id.number == *number
                            })
                        }
                    })
                    .ok_or(PointOutError::ReferenceMismatch)?;
                let bytes = frame(&encode(message).map_err(|_| PointOutError::MissingFields)?)
                    .map_err(|_| PointOutError::MissingFields)?;
                retransmissions.push(bytes);
            }
            session::Action::ResponseTimeout { kind, number }
                if matches!(kind, Kind::Poi | Kind::Poa | Kind::Poj) =>
            {
                let record = runtime
                    .records
                    .iter_mut()
                    .find(|record| {
                        (record.outbound
                            && record.source.header.kind == *kind
                            && record.source.header.id.number == *number)
                            || record.decision_message.as_ref().is_some_and(|message| {
                                message.header.kind == *kind && message.header.id.number == *number
                            })
                    })
                    .ok_or(PointOutError::ReferenceMismatch)?;
                record.view.ack_status = PointOutAckStatus::TimedOut;
                if *kind == Kind::Poi {
                    record.view.status = PointOutStatus::ResponseTimedOut;
                }
                timed_out.push(record.view.point_out_id);
            }
            _ => {}
        }
    }
    Ok(PointOutTickResult {
        actions,
        retransmissions,
        timed_out,
    })
}

/// Retire a point-out only after an explicit operational lifecycle signal. Pending NAM response
/// timers block retirement; removing the record releases its wrapped message identifier for reuse.
pub fn nam_pointout_retire(
    runtime: &mut NamPointOutRuntime,
    point_out_id: MsgId,
    expected_key: Option<FlightKey>,
    reason: PointOutRetirementReason,
) -> Result<PointOutView, PointOutError> {
    let index = runtime
        .records
        .iter()
        .position(|record| record.view.point_out_id == point_out_id)
        .ok_or(PointOutError::ReferenceMismatch)?;
    let view = runtime
        .records
        .get(index)
        .ok_or(PointOutError::ReferenceMismatch)?
        .view;
    if view.key != expected_key {
        return Err(PointOutError::FlightMismatch);
    }
    if view.ack_status == PointOutAckStatus::Awaiting {
        return Err(PointOutError::PendingResponse);
    }
    if reason == PointOutRetirementReason::ManualCoordinationClosed
        && runtime.mode != PointOutMode::Basic
    {
        return Err(PointOutError::InvalidContext);
    }
    runtime.records.remove(index);
    Ok(view)
}

fn next_id(
    runtime: &NamPointOutRuntime,
    ingress: &NamSessionIngress,
    sender: oa_nam::Facility,
    receiver: oa_nam::Facility,
) -> Result<MsgId, PointOutError> {
    let next = nam_session_ingress_state(ingress).next_number % 1000;
    let candidate = MsgId {
        sender,
        receiver,
        number: next,
    };
    if runtime.records.iter().any(|record| {
        record.view.point_out_id == candidate
            || record
                .decision_message
                .as_ref()
                .is_some_and(|message| message.header.id == candidate)
    }) {
        return Err(PointOutError::MessageNumberInUse);
    }
    Ok(candidate)
}

/// Send a caller-prepared POI through the active peer session. Flight identity is an explicit
/// full-key/address binding; no callsign or beacon-code lookup is performed.
pub fn nam_pointout_send_offer(
    runtime: &mut NamPointOutRuntime,
    flights: &mut FlightDriver,
    ingress: &mut NamSessionIngress,
    mut message: Message,
    context: PointOutContext,
    now_ms: u64,
    local_facility: [u8; 4],
    observed: &[ObservedTrack],
) -> Result<PointOutTransmitResult, PointOutError> {
    if runtime.mode == PointOutMode::Enhanced && runtime.response_profile.is_none() {
        return Err(PointOutError::ResponseProfileRequired);
    }
    if message.header.kind != Kind::Poi {
        return Err(PointOutError::WrongKind);
    }
    if message.header.id.sender.0 != local_facility
        || message.header.id.receiver.0 == local_facility
        || message
            .f31
            .as_ref()
            .is_some_and(|f31| f31.facility.0 != message.header.id.receiver.0)
    {
        return Err(PointOutError::WrongPeer);
    }
    if context.source_ns < 0
        || context.source_ns != context.receipt_ns
        || context.sequence == 0
        || context.receipt_ns % 1_000_000 != 0
    {
        return Err(PointOutError::InvalidTime);
    }
    if runtime.records.len() >= runtime.capacity {
        return Err(PointOutError::Capacity);
    }
    if runtime
        .records
        .iter()
        .any(|record| record.view.point_out_id == message.header.id)
    {
        return Err(PointOutError::Duplicate);
    }
    let bound = positive_binding(flights, context.binding, context.receipt_ns)?;
    let established_mode_a = field_code(message.f07.as_ref())?;
    let address = bound.map(|view| view.address);
    let observed_track =
        address.and_then(|address| observed.iter().find(|track| track.address == address));
    let observed_comparison = compare_observed(
        established_mode_a,
        observed_track,
        context.source_ns,
        context.receipt_ns,
    );
    // Validate the caller's message before consuming a session message number.
    let candidate = next_id(
        runtime,
        ingress,
        message.header.id.sender,
        message.header.id.receiver,
    )?;
    message.header.id.number = candidate.number;
    encode(&message).map_err(|_| PointOutError::MissingFields)?;
    let actions =
        nam_session_ingress_local_event(ingress, session::Event::SendAtm(Kind::Poi), now_ms)
            .map_err(|_| PointOutError::SessionResponse)?;
    let number = actions
        .as_slice()
        .iter()
        .find_map(|action| match action {
            session::Action::SendAtm {
                kind: Kind::Poi,
                number,
            } => Some(*number),
            _ => None,
        })
        .ok_or(PointOutError::SessionResponse)?;
    if number != candidate.number {
        return Err(PointOutError::SessionResponse);
    }
    message.header.id.number = number;
    let framed_bytes = frame(&encode(&message).map_err(|_| PointOutError::MissingFields)?)
        .map_err(|_| PointOutError::MissingFields)?;
    let view = PointOutView {
        point_out_id: message.header.id,
        from: message.header.id.sender.0,
        to: message.header.id.receiver.0,
        key: context.binding.map(|binding| binding.key),
        route_key: context.binding.map(|binding| binding.route_key),
        address,
        status: PointOutStatus::AwaitingComputerResponse,
        ack_status: PointOutAckStatus::Awaiting,
        established_mode_a,
        assigned_mode_a: None,
        observed_comparison,
        assigned_comparison: BeaconComparison::Unknown,
        source_ns: context.source_ns,
        receipt_ns: context.receipt_ns,
        context_sequence: context.sequence,
    };
    runtime.records.push(Record {
        view,
        source: message.clone(),
        outbound: true,
        decision_message: None,
    });
    Ok(PointOutTransmitResult {
        framed_bytes,
        message,
        actions,
        view,
    })
}

/// Apply a controller decision to one Enhanced inbound POI and return the exact encoded POA/POJ
/// plus session actions. The caller supplies its configured local Field 31 (facility and sector);
/// this adapter does not invent a sector. The session action is generated before any external
/// transport write, and is not itself a network acknowledgement.
pub fn nam_pointout_decide(
    runtime: &mut NamPointOutRuntime,
    ingress: &mut NamSessionIngress,
    point_out_id: MsgId,
    decision: PointOutDecision,
    responding_route: Field31,
    now_ms: u64,
    local_facility: [u8; 4],
) -> Result<PointOutDecisionResult, PointOutError> {
    if runtime.mode != PointOutMode::Enhanced
        || responding_route.facility.0 != local_facility
        || responding_route.sector.is_none()
    {
        return Err(PointOutError::UnsupportedEnhancedMessage);
    }
    let response_profile = runtime
        .response_profile
        .ok_or(PointOutError::ResponseProfileRequired)?;
    let index = runtime
        .records
        .iter()
        .position(|record| {
            record.view.point_out_id == point_out_id
                && record.view.status == PointOutStatus::AwaitingControllerDecision
        })
        .ok_or(PointOutError::ReferenceMismatch)?;
    let record = runtime
        .records
        .get(index)
        .ok_or(PointOutError::ReferenceMismatch)?;
    let source = &record.source;
    let mut id = MsgId {
        sender: oa_nam::Facility(local_facility),
        receiver: source.header.id.sender,
        number: 0,
    };
    let candidate = next_id(runtime, ingress, id.sender, id.receiver)?;
    id.number = candidate.number;
    let (kind, f07, status, assigned_mode_a) = match decision {
        PointOutDecision::Approve { assigned_mode_a } => {
            if assigned_mode_a.is_some_and(|code| code > 0o7777) {
                return Err(PointOutError::InvalidCode);
            }
            let acid = source_acid(source.f07.as_ref())?;
            let ssr = assigned_mode_a.map(mode_a_ssr);
            (
                Kind::Poa,
                Some(Field07 {
                    ident: Ident::Acid(acid),
                    ssr,
                }),
                PointOutStatus::Approved,
                assigned_mode_a,
            )
        }
        PointOutDecision::Deny => {
            let acid = source_acid(source.f07.as_ref())?;
            (
                Kind::Poj,
                Some(Field07 {
                    ident: Ident::Acid(acid),
                    ssr: None,
                }),
                PointOutStatus::Denied,
                None,
            )
        }
    };
    // Validate the copied POI fields and caller route before mutating the session state.
    let mut message = bare(Header {
        kind,
        id,
        reference: Some(point_out_id),
    });
    message.f07 = f07;
    message.f13 = source.f13;
    message.f16 = source.f16.clone();
    message.f31 = Some(responding_route);
    encode(&message).map_err(|_| PointOutError::MissingFields)?;
    let event = if response_profile == PointOutResponseProfile::IcdAnnexBNoResponse {
        session::Event::SendAtmWithoutResponse(kind)
    } else {
        session::Event::SendAtm(kind)
    };
    let actions = nam_session_ingress_local_event(ingress, event, now_ms)
        .map_err(|_| PointOutError::SessionResponse)?;
    let number = actions
        .as_slice()
        .iter()
        .find_map(|action| match action {
            session::Action::SendAtm { kind: sent, number } if *sent == kind => Some(*number),
            _ => None,
        })
        .ok_or(PointOutError::SessionResponse)?;
    if number != candidate.number {
        return Err(PointOutError::SessionResponse);
    }
    message.header.id.number = number;
    let framed_bytes = frame(&encode(&message).map_err(|_| PointOutError::MissingFields)?)
        .map_err(|_| PointOutError::MissingFields)?;
    let record = runtime
        .records
        .get_mut(index)
        .ok_or(PointOutError::ReferenceMismatch)?;
    record.view.status = status;
    record.view.assigned_mode_a = assigned_mode_a;
    record.view.assigned_comparison =
        compare_codes(record.view.established_mode_a, assigned_mode_a);
    record.view.ack_status = match response_profile {
        PointOutResponseProfile::IcdPartIiSection37Lam => PointOutAckStatus::Awaiting,
        PointOutResponseProfile::IcdAnnexBNoResponse => PointOutAckStatus::NotExpected,
    };
    record.decision_message = Some(message.clone());
    Ok(PointOutDecisionResult {
        framed_bytes,
        message,
        actions,
        view: record.view,
    })
}

fn source_acid(field: Option<&Field07>) -> Result<String, PointOutError> {
    match field.map(|field| &field.ident) {
        Some(Ident::Acid(acid)) => Ok(acid.clone()),
        _ => Err(PointOutError::MissingFields),
    }
}

fn mode_a_ssr(code: u16) -> Ssr {
    let mut value = code;
    let mut digits = [b'0'; 4];
    for digit in digits.iter_mut().rev() {
        *digit = b'0' + (value % 8) as u8;
        value /= 8;
    }
    Ssr {
        mode: b'A',
        code: digits,
    }
}

/// Receive actual encoded POI/PLA/POA/POJ through NAM session admission and update only local
/// point-out presentation state. POI gets PLA after syntactic/logic acceptance; Enhanced POA/POJ
/// are correlated to one exact POI and reverse peer pair before being applied.
pub fn nam_pointout_receive(
    runtime: &mut NamPointOutRuntime,
    flights: &mut FlightDriver,
    ingress: &mut NamSessionIngress,
    framed_bytes: &[u8],
    now_ms: u64,
    local_facility: [u8; 4],
    context: PointOutContext,
    observed: &[ObservedTrack],
) -> Result<PointOutResult, PointOutError> {
    if context.source_ns < 0
        || context.source_ns != context.receipt_ns
        || context.sequence == 0
        || context.receipt_ns % 1_000_000 != 0
    {
        return Err(PointOutError::InvalidTime);
    }
    let message = parse_exact(framed_bytes)?;
    let context_view = positive_binding(flights, context.binding, context.receipt_ns)?;
    prevalidate_pointout(runtime, &message, context, context_view, local_facility)?;
    let admission = nam_session_ingress_receive(ingress, framed_bytes, now_ms)
        .map_err(|_| PointOutError::NotAdmitted)?;
    let accepted = admission
        .accepted
        .as_ref()
        .ok_or(PointOutError::NotAdmitted)?;
    let delivered = matches!(
        accepted_delivery_seq(accepted),
        Some(session::Seq::First | session::Seq::InOrder | session::Seq::Gap(_))
    );
    let correlated_pointout_response = accepted_pointout_response(accepted)
        && matches!(message.header.kind, Kind::Pla | Kind::Lam | Kind::Lrm);
    if accepted_message(accepted).header != message.header
        || (!delivered && !correlated_pointout_response)
    {
        return Err(PointOutError::NotAdmitted);
    }
    let message = accepted_message(accepted);
    match message.header.kind {
        Kind::Poi => receive_poi(
            runtime,
            ingress,
            &admission,
            message,
            context,
            context_view,
            observed,
            local_facility,
            now_ms,
        ),
        Kind::Poa | Kind::Poj => receive_decision(
            runtime,
            ingress,
            &admission,
            message,
            context,
            context_view,
            local_facility,
            now_ms,
        ),
        Kind::Pla => receive_logical_ack(runtime, &admission, message, context, context_view),
        Kind::Lam => receive_decision_ack(runtime, &admission, message, context, context_view),
        Kind::Lrm => {
            let record = pointout_response_record(runtime, message)?;
            if record.outbound {
                receive_logical_ack(runtime, &admission, message, context, context_view)
            } else {
                receive_decision_ack(runtime, &admission, message, context, context_view)
            }
        }
        _ => Err(PointOutError::WrongKind),
    }
}

/// Perform all point-out-owned semantic checks before the irreversible NAM sequence step.
fn prevalidate_pointout(
    runtime: &NamPointOutRuntime,
    message: &Message,
    context: PointOutContext,
    binding_view: Option<SharedFlightView>,
    local_facility: [u8; 4],
) -> Result<(), PointOutError> {
    match message.header.kind {
        Kind::Poi => {
            if message.header.id.receiver.0 != local_facility
                || message
                    .f31
                    .as_ref()
                    .is_none_or(|field| field.facility.0 != local_facility)
            {
                return Err(PointOutError::WrongPeer);
            }
            if context.response_sector.is_none() {
                return Err(PointOutError::MissingFields);
            }
            if runtime.records.len() >= runtime.capacity {
                return Err(PointOutError::Capacity);
            }
            if runtime
                .records
                .iter()
                .any(|record| record.view.point_out_id == message.header.id)
            {
                return Err(PointOutError::Duplicate);
            }
            let _ = field_code(message.f07.as_ref())?;
        }
        Kind::Pla | Kind::Lam | Kind::Lrm => {
            let record = pointout_response_record(runtime, message)?;
            let response_to_poi = record.outbound
                && record.view.status == PointOutStatus::AwaitingComputerResponse
                && matches!(message.header.kind, Kind::Pla | Kind::Lrm);
            let response_to_decision = !record.outbound
                && record.decision_message.is_some()
                && record.view.ack_status == PointOutAckStatus::Awaiting
                && matches!(message.header.kind, Kind::Lam | Kind::Lrm);
            if !response_to_poi && !response_to_decision {
                return Err(PointOutError::InvalidTransition);
            }
            let (expected_sender, expected_receiver) = if response_to_poi {
                (record.view.to, record.view.from)
            } else {
                (record.view.from, record.view.to)
            };
            if message.header.id.sender.0 != expected_sender
                || message.header.id.receiver.0 != expected_receiver
                || !same_context(&record.view, context)
                || binding_view.is_some_and(|view| Some(view.address) != record.view.address)
                || (message.header.kind == Kind::Pla
                    && message
                        .f31
                        .as_ref()
                        .is_none_or(|field| field.facility.0 != message.header.id.sender.0))
            {
                return Err(PointOutError::WrongPeer);
            }
        }
        Kind::Poa | Kind::Poj => {
            if runtime.mode != PointOutMode::Enhanced {
                return Err(PointOutError::UnsupportedEnhancedMessage);
            }
            if runtime.response_profile.is_none() {
                return Err(PointOutError::ResponseProfileRequired);
            }
            let reference = message
                .header
                .reference
                .ok_or(PointOutError::MissingReference)?;
            let record = runtime
                .records
                .iter()
                .find(|record| record.view.point_out_id == reference)
                .ok_or(PointOutError::ReferenceMismatch)?;
            if !record.outbound
                || record.view.status != PointOutStatus::AwaitingRemoteControllerDecision
            {
                return Err(PointOutError::InvalidTransition);
            }
            if message.header.id.sender.0 != record.view.to
                || message.header.id.receiver.0 != record.view.from
                || local_facility != record.view.from
                || !same_context(&record.view, context)
                || binding_view.is_some_and(|view| Some(view.address) != record.view.address)
                || message
                    .f31
                    .as_ref()
                    .is_none_or(|field| field.facility.0 != message.header.id.sender.0)
            {
                return Err(PointOutError::WrongPeer);
            }
            let expected_acid = source_acid(record.source.f07.as_ref())?;
            if source_acid(message.f07.as_ref())? != expected_acid {
                return Err(PointOutError::FlightMismatch);
            }
            if message.header.kind == Kind::Poa {
                let _ = optional_field_code(message.f07.as_ref())?;
            }
        }
        _ => return Err(PointOutError::WrongKind),
    }
    Ok(())
}

fn pointout_response_record<'a>(
    runtime: &'a NamPointOutRuntime,
    message: &Message,
) -> Result<&'a Record, PointOutError> {
    let reference = message
        .header
        .reference
        .ok_or(PointOutError::MissingReference)?;
    let record = match message.header.kind {
        Kind::Pla => runtime
            .records
            .iter()
            .find(|record| record.outbound && record.view.point_out_id == reference),
        Kind::Lam => runtime.records.iter().find(|record| {
            !record.outbound
                && record
                    .decision_message
                    .as_ref()
                    .is_some_and(|decision| decision.header.id == reference)
        }),
        Kind::Lrm => runtime
            .records
            .iter()
            .find(|record| record.outbound && record.view.point_out_id == reference)
            .or_else(|| {
                runtime.records.iter().find(|record| {
                    !record.outbound
                        && record
                            .decision_message
                            .as_ref()
                            .is_some_and(|decision| decision.header.id == reference)
                })
            }),
        _ => None,
    };
    record.ok_or(PointOutError::ReferenceMismatch)
}

fn receive_poi(
    runtime: &mut NamPointOutRuntime,
    ingress: &mut NamSessionIngress,
    ingress_result: &NamIngressResult,
    message: &Message,
    context: PointOutContext,
    view: Option<SharedFlightView>,
    observed: &[ObservedTrack],
    local_facility: [u8; 4],
    now_ms: u64,
) -> Result<PointOutResult, PointOutError> {
    if message.header.id.receiver.0 != local_facility
        || message
            .f31
            .as_ref()
            .is_none_or(|f31| f31.facility.0 != local_facility)
    {
        return Err(PointOutError::WrongPeer);
    }
    if runtime
        .records
        .iter()
        .any(|record| record.view.point_out_id == message.header.id)
    {
        return Err(PointOutError::Duplicate);
    }
    if runtime.records.len() >= runtime.capacity {
        return Err(PointOutError::Capacity);
    }
    let established_mode_a = field_code(message.f07.as_ref())?;
    let binding = context.binding;
    let address = view.map(|view| view.address);
    let observed_track =
        address.and_then(|address| observed.iter().find(|track| track.address == address));
    let observed_comparison = compare_observed(
        established_mode_a,
        observed_track,
        context.source_ns,
        context.receipt_ns,
    );
    let status = match runtime.mode {
        PointOutMode::Basic => PointOutStatus::AwaitingManualCoordination,
        PointOutMode::Enhanced => PointOutStatus::AwaitingControllerDecision,
    };
    let view = PointOutView {
        point_out_id: message.header.id,
        from: message.header.id.sender.0,
        to: message.header.id.receiver.0,
        key: binding.map(|binding| binding.key),
        route_key: binding.map(|binding| binding.route_key),
        address,
        status,
        ack_status: PointOutAckStatus::NotExpected,
        established_mode_a,
        assigned_mode_a: None,
        observed_comparison,
        assigned_comparison: BeaconComparison::Unknown,
        source_ns: context.source_ns,
        receipt_ns: context.receipt_ns,
        context_sequence: context.sequence,
    };
    let sector = context
        .response_sector
        .ok_or(PointOutError::MissingFields)?;
    let mut response = bare(Header {
        kind: Kind::Pla,
        id: MsgId {
            sender: oa_nam::Facility(local_facility),
            receiver: message.header.id.sender,
            number: 0,
        },
        reference: Some(message.header.id),
    });
    response.f31 = Some(Field31 {
        facility: oa_nam::Facility(local_facility),
        sector: Some(sector),
        text: None,
    });
    encode(&response).map_err(|_| PointOutError::MissingFields)?;
    let response_actions = nam_session_ingress_local_event(
        ingress,
        session::Event::SendResponse {
            kind: Kind::Pla,
            reference: message.header.id,
        },
        now_ms,
    )
    .map_err(|_| PointOutError::SessionResponse)?;
    let number = response_actions
        .as_slice()
        .iter()
        .find_map(|action| match action {
            session::Action::SendResponse {
                kind: Kind::Pla,
                number,
                reference,
            } if *reference == message.header.id => Some(*number),
            _ => None,
        })
        .ok_or(PointOutError::SessionResponse)?;
    response.header.id.number = number;
    let response_framed = frame(&encode(&response).map_err(|_| PointOutError::MissingFields)?)
        .map_err(|_| PointOutError::MissingFields)?;
    runtime.records.push(Record {
        view,
        source: message.clone(),
        outbound: false,
        decision_message: None,
    });
    Ok(PointOutResult {
        ingress: ingress_result.clone(),
        response_actions: Some(response_actions),
        response_framed: Some(response_framed),
        view: Some(view),
    })
}

fn receive_decision(
    runtime: &mut NamPointOutRuntime,
    ingress_state: &mut NamSessionIngress,
    ingress: &NamIngressResult,
    message: &Message,
    context: PointOutContext,
    binding_view: Option<SharedFlightView>,
    local_facility: [u8; 4],
    now_ms: u64,
) -> Result<PointOutResult, PointOutError> {
    if runtime.mode != PointOutMode::Enhanced {
        return Err(PointOutError::UnsupportedEnhancedMessage);
    }
    let reference = message
        .header
        .reference
        .ok_or(PointOutError::MissingReference)?;
    let index = runtime
        .records
        .iter()
        .position(|record| record.view.point_out_id == reference)
        .ok_or(PointOutError::ReferenceMismatch)?;
    let record = runtime
        .records
        .get(index)
        .ok_or(PointOutError::ReferenceMismatch)?;
    if !record.outbound {
        return Err(PointOutError::InvalidTransition);
    }
    let response_profile = runtime
        .response_profile
        .ok_or(PointOutError::ResponseProfileRequired)?;
    let current = record.view;
    if current.status != PointOutStatus::AwaitingRemoteControllerDecision {
        return Err(PointOutError::InvalidTransition);
    }
    if message.header.id.sender.0 != current.to
        || message.header.id.receiver.0 != current.from
        || local_facility != current.from
        || !same_context(&current, context)
        || binding_view.is_some_and(|view| Some(view.address) != current.address)
    {
        return Err(PointOutError::WrongPeer);
    }
    if message
        .f31
        .as_ref()
        .is_none_or(|field| field.facility.0 != message.header.id.sender.0)
    {
        return Err(PointOutError::WrongPeer);
    }
    let mut updated = current;
    updated.status = if message.header.kind == Kind::Poa {
        PointOutStatus::Approved
    } else {
        PointOutStatus::Denied
    };
    if message.header.kind == Kind::Poa {
        updated.assigned_mode_a = optional_field_code(message.f07.as_ref())?;
        updated.assigned_comparison =
            compare_codes(updated.established_mode_a, updated.assigned_mode_a);
    }
    let (response_actions, response_framed) = match response_profile {
        PointOutResponseProfile::IcdAnnexBNoResponse => (None, None),
        PointOutResponseProfile::IcdPartIiSection37Lam => {
            let actions = nam_session_ingress_local_event(
                ingress_state,
                session::Event::SendResponse {
                    kind: Kind::Lam,
                    reference: message.header.id,
                },
                now_ms,
            )
            .map_err(|_| PointOutError::SessionResponse)?;
            let number = actions
                .as_slice()
                .iter()
                .find_map(|action| match action {
                    session::Action::SendResponse {
                        kind: Kind::Lam,
                        number,
                        reference,
                    } if *reference == message.header.id => Some(*number),
                    _ => None,
                })
                .ok_or(PointOutError::SessionResponse)?;
            let response = bare(Header {
                kind: Kind::Lam,
                id: MsgId {
                    sender: oa_nam::Facility(local_facility),
                    receiver: message.header.id.sender,
                    number,
                },
                reference: Some(message.header.id),
            });
            let bytes = frame(&encode(&response).map_err(|_| PointOutError::MissingFields)?)
                .map_err(|_| PointOutError::MissingFields)?;
            (Some(actions), Some(bytes))
        }
    };
    updated.context_sequence = context.sequence;
    runtime
        .records
        .get_mut(index)
        .ok_or(PointOutError::ReferenceMismatch)?
        .view = updated;
    Ok(PointOutResult {
        ingress: ingress.clone(),
        response_actions,
        response_framed,
        view: Some(updated),
    })
}

fn receive_logical_ack(
    runtime: &mut NamPointOutRuntime,
    ingress: &NamIngressResult,
    message: &Message,
    context: PointOutContext,
    binding_view: Option<SharedFlightView>,
) -> Result<PointOutResult, PointOutError> {
    let reference = message
        .header
        .reference
        .ok_or(PointOutError::MissingReference)?;
    let record = runtime
        .records
        .iter()
        .find(|record| record.view.point_out_id == reference)
        .ok_or(PointOutError::ReferenceMismatch)?;
    if !record.outbound || record.view.status != PointOutStatus::AwaitingComputerResponse {
        return Err(PointOutError::InvalidTransition);
    }
    if !matches!(message.header.kind, Kind::Pla | Kind::Lrm)
        || message.header.id.sender.0 != record.view.to
        || message.header.id.receiver.0 != record.view.from
        || !same_context(&record.view, context)
        || binding_view.is_some_and(|view| Some(view.address) != record.view.address)
        || (message.header.kind == Kind::Pla
            && message
                .f31
                .as_ref()
                .is_none_or(|field| field.facility.0 != message.header.id.sender.0))
    {
        return Err(PointOutError::WrongPeer);
    }
    // PLA only means the computer accepted the POI. Basic operation still requires manual
    // coordination; it is not an approval state transition.
    let record = runtime
        .records
        .iter_mut()
        .find(|record| record.view.point_out_id == reference)
        .ok_or(PointOutError::ReferenceMismatch)?;
    record.view.status = match message.header.kind {
        Kind::Lrm => PointOutStatus::ComputerRejected,
        Kind::Pla => match runtime.mode {
            PointOutMode::Basic => PointOutStatus::AwaitingManualCoordination,
            PointOutMode::Enhanced => PointOutStatus::AwaitingRemoteControllerDecision,
        },
        _ => return Err(PointOutError::WrongKind),
    };
    record.view.ack_status = if message.header.kind == Kind::Pla {
        PointOutAckStatus::Received
    } else {
        PointOutAckStatus::Rejected
    };
    record.view.context_sequence = context.sequence;
    Ok(PointOutResult {
        ingress: ingress.clone(),
        response_actions: None,
        response_framed: None,
        view: Some(record.view),
    })
}

fn receive_decision_ack(
    runtime: &mut NamPointOutRuntime,
    ingress: &NamIngressResult,
    message: &Message,
    context: PointOutContext,
    binding_view: Option<SharedFlightView>,
) -> Result<PointOutResult, PointOutError> {
    let reference = message
        .header
        .reference
        .ok_or(PointOutError::MissingReference)?;
    let index = runtime
        .records
        .iter()
        .position(|record| {
            !record.outbound
                && record
                    .decision_message
                    .as_ref()
                    .is_some_and(|decision| decision.header.id == reference)
        })
        .ok_or(PointOutError::ReferenceMismatch)?;
    let record = runtime
        .records
        .get_mut(index)
        .ok_or(PointOutError::ReferenceMismatch)?;
    let decision = record
        .decision_message
        .as_ref()
        .ok_or(PointOutError::ReferenceMismatch)?;
    if record.view.ack_status != PointOutAckStatus::Awaiting
        || message.header.id.sender != decision.header.id.receiver
        || message.header.id.receiver != decision.header.id.sender
        || !same_context(&record.view, context)
        || binding_view.is_some_and(|view| Some(view.address) != record.view.address)
    {
        return Err(PointOutError::WrongPeer);
    }
    record.view.ack_status = if message.header.kind == Kind::Lam {
        PointOutAckStatus::Received
    } else if message.header.kind == Kind::Lrm {
        PointOutAckStatus::Rejected
    } else {
        return Err(PointOutError::WrongKind);
    };
    record.view.context_sequence = context.sequence;
    Ok(PointOutResult {
        ingress: ingress.clone(),
        response_actions: None,
        response_framed: None,
        view: Some(record.view),
    })
}

fn same_context(view: &PointOutView, context: PointOutContext) -> bool {
    view.key == context.binding.map(|binding| binding.key)
        && view.route_key == context.binding.map(|binding| binding.route_key)
        && context.source_ns >= view.source_ns
        && context.receipt_ns >= view.receipt_ns
        && context.sequence > view.context_sequence
}

fn positive_binding(
    flights: &mut FlightDriver,
    binding: Option<PointOutBinding>,
    receipt_ns: i64,
) -> Result<Option<SharedFlightView>, PointOutError> {
    let Some(binding) = binding else {
        return Ok(None);
    };
    let route_key = binding.route_key;
    if binding.key.kind != oa_flight::KEY_GUFI
        || binding.key.gufi.to_be_bytes() != route_key.gufi
        || route_key.aircraft_address == 0
        || route_key.aircraft_address > 0x00ff_ffff
        || route_key.plan_id.iter().all(|byte| *byte == 0)
        || receipt_ns < 0
    {
        return Err(PointOutError::InvalidContext);
    }
    let mut views = Vec::with_capacity(flight::MAX_OFFLINE_DRIVER_FLIGHTS);
    let result = flight::flight_views_for_address(
        flights,
        route_key.aircraft_address,
        route_key.centre,
        receipt_ns / 1_000_000,
        &mut views,
    );
    match result.status {
        FlightViewStatus::Found => {}
        FlightViewStatus::UnknownFlight => return Err(PointOutError::UnknownFlight),
        FlightViewStatus::AmbiguousAddress => return Err(PointOutError::AmbiguousFlight),
        FlightViewStatus::FutureSnapshot => return Err(PointOutError::FutureFlight),
        _ => return Err(PointOutError::FlightMismatch),
    }
    let view = views
        .iter()
        .find(|view| route::flight_key(view) == route_key)
        .copied()
        .ok_or(PointOutError::FlightMismatch)?;
    if view.gufi != binding.key.gufi {
        return Err(PointOutError::FlightMismatch);
    }
    Ok(Some(view))
}

fn field_code(field: Option<&Field07>) -> Result<Option<u16>, PointOutError> {
    let field = field.ok_or(PointOutError::MissingFields)?;
    if !matches!(field.ident, Ident::Acid(_)) || field.ssr.is_none() {
        return Err(PointOutError::MissingFields);
    }
    optional_field_code(Some(field))
}

fn optional_field_code(field: Option<&Field07>) -> Result<Option<u16>, PointOutError> {
    let Some(ssr) = field.and_then(|field| field.ssr) else {
        return Ok(None);
    };
    if ssr.mode != b'A' || !ssr.code.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
        return Err(PointOutError::InvalidCode);
    }
    let mut code = 0u16;
    for digit in ssr.code {
        code = code
            .checked_mul(8)
            .and_then(|value| value.checked_add(u16::from(digit - b'0')))
            .ok_or(PointOutError::InvalidCode)?;
    }
    Ok(Some(code))
}

fn compare_observed(
    code: Option<u16>,
    track: Option<&ObservedTrack>,
    source_ns: i64,
    receipt_ns: i64,
) -> BeaconComparison {
    let Some(track) = track else {
        return BeaconComparison::Unknown;
    };
    if track.ident_conflict != 0 {
        return BeaconComparison::Conflict;
    }
    if track.ident == oa_track::NO_IDENT || track.ident_ns == oa_track::NO_IDENT_NS {
        return BeaconComparison::Unknown;
    }
    // No facility freshness threshold is inferred. The code is stale only when its recorded
    // source time precedes this point-out's source time, and future when it exceeds receipt time.
    if track.ident_ns < source_ns {
        return BeaconComparison::Stale;
    }
    if track.ident_ns > receipt_ns {
        return BeaconComparison::Future;
    }
    compare_codes(code, Some(track.ident))
}

fn compare_codes(left: Option<u16>, right: Option<u16>) -> BeaconComparison {
    match (left, right) {
        (Some(left), Some(right)) if left == right => BeaconComparison::Match,
        (Some(_), Some(_)) => BeaconComparison::Mismatch,
        _ => BeaconComparison::Unknown,
    }
}

fn parse_exact(bytes: &[u8]) -> Result<Message, PointOutError> {
    let (message, consumed) = unframe(bytes)
        .map_err(|_| PointOutError::NotAdmitted)?
        .ok_or(PointOutError::NotAdmitted)?;
    if consumed != bytes.len() {
        return Err(PointOutError::NotAdmitted);
    }
    parse(message).map_err(|_| PointOutError::NotAdmitted)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::nam_handoff::nam_session_ingress_state;
    use oa_nam::{
        Aerodrome, Angle, Facility, Field13, Field16, Field32, GroundSpeed, Heading, Level, Time32,
    };

    const LOCAL: Facility = Facility(*b"KZMP");
    const PEER: Facility = Facility(*b"CZWG");

    fn header(
        kind: Kind,
        sender: Facility,
        receiver: Facility,
        number: u16,
        reference: Option<MsgId>,
    ) -> Header {
        Header {
            kind,
            id: MsgId {
                sender,
                receiver,
                number,
            },
            reference,
        }
    }

    fn active_ingress(local: Facility, peer: Facility) -> NamSessionIngress {
        let mut ingress =
            crate::nam_handoff::nam_session_ingress_new(session::default_config(local, peer));
        let actions = nam_session_ingress_local_event(&mut ingress, session::Event::Activate, 0)
            .expect("activation");
        let [session::Action::SendIrq { number }] = actions.as_slice() else {
            panic!("activation sends IRQ")
        };
        let irs = bare(header(
            Kind::Irs,
            peer,
            local,
            0,
            Some(MsgId {
                sender: local,
                receiver: peer,
                number: *number,
            }),
        ));
        let bytes = frame(&encode(&irs).expect("IRS encodes")).expect("IRS frames");
        nam_session_ingress_receive(&mut ingress, &bytes, 1).expect("IRS admission");
        assert_eq!(
            nam_session_ingress_state(&ingress).state,
            session::State::Active
        );
        ingress
    }

    fn poi(sender: Facility, receiver: Facility, number: u16) -> Message {
        let mut message = bare(header(Kind::Poi, sender, receiver, number, None));
        message.f07 = Some(Field07 {
            ident: Ident::Acid("ABC123".to_owned()),
            ssr: Some(Ssr {
                mode: b'A',
                code: *b"1234",
            }),
        });
        message.f13 = Some(Field13 {
            aerodrome: Aerodrome(*b"KAAA"),
            time: None,
        });
        message.f16 = Some(Field16 {
            aerodrome: Aerodrome(*b"KBBB"),
            eet: None,
            alternates: Vec::new(),
        });
        message.f32 = Some(Field32 {
            time: Time32 {
                hour: 12,
                minute: 0,
                second: 0,
                hundredths: 0,
            },
            latitude: Angle {
                degrees: 41,
                minutes: 0,
                seconds: 0,
                hemisphere: b'N',
            },
            longitude: Angle {
                degrees: 87,
                minutes: 0,
                seconds: 0,
                hemisphere: b'W',
            },
            speed: GroundSpeed::Knots(250),
            heading: Heading::Hundredths(9_000),
            altitude: Level::FlightLevel(350),
        });
        message.f31 = Some(Field31 {
            facility: receiver,
            sector: Some(*b"1A"),
            text: None,
        });
        message
    }

    fn context(receipt_ns: i64) -> PointOutContext {
        PointOutContext {
            binding: None,
            source_ns: receipt_ns,
            receipt_ns,
            sequence: u64::try_from(receipt_ns / 1_000_000_000).expect("positive sequence"),
            response_sector: Some(*b"2A"),
        }
    }

    fn enhanced_runtime(profile: PointOutResponseProfile) -> NamPointOutRuntime {
        let mut runtime =
            nam_pointout_runtime_new(4, PointOutMode::Basic).expect("bounded runtime");
        nam_pointout_set_response_profile(&mut runtime, profile)
            .expect("explicit response profile");
        nam_pointout_set_mode(&mut runtime, PointOutMode::Enhanced)
            .expect("configured Enhanced mode");
        runtime
    }

    fn framed(message: &Message) -> Vec<u8> {
        frame(&encode(message).expect("NAM message encodes")).expect("NAM message frames")
    }

    fn empty_flights() -> FlightDriver {
        flight::flight_driver_new(crate::flight::FlightDriverConfig {
            flight_capacity: 2,
            refusal_log_capacity: 2,
            conflict_log_capacity: 2,
        })
        .expect("bounded flight driver")
    }

    #[test]
    fn incoming_poi_is_displayed_and_only_explicit_enhanced_decision_generates_poa() {
        let mut runtime = enhanced_runtime(PointOutResponseProfile::IcdPartIiSection37Lam);
        let mut flights = empty_flights();
        let mut ingress = active_ingress(LOCAL, PEER);
        let request = poi(PEER, LOCAL, 21);
        let received = nam_pointout_receive(
            &mut runtime,
            &mut flights,
            &mut ingress,
            &framed(&request),
            2,
            LOCAL.0,
            context(2_000_000_000),
            &[],
        )
        .expect("encoded POI is admitted and displayed");
        let view = received.view.expect("point-out view");
        assert_eq!(view.status, PointOutStatus::AwaitingControllerDecision);
        assert_eq!(view.address, None);
        assert_eq!(view.established_mode_a, Some(0o1234));
        assert!(
            received.response_actions.is_some(),
            "PLA is only the logical computer response"
        );
        assert_eq!(
            parse_exact(received.response_framed.as_deref().expect("encoded PLA"))
                .expect("PLA parses")
                .header
                .kind,
            Kind::Pla,
        );

        let response = nam_pointout_decide(
            &mut runtime,
            &mut ingress,
            view.point_out_id,
            PointOutDecision::Approve {
                assigned_mode_a: Some(0o4321),
            },
            Field31 {
                facility: LOCAL,
                sector: Some(*b"2A"),
                text: None,
            },
            3,
            LOCAL.0,
        )
        .expect("explicit operator decision creates POA");
        assert_eq!(response.message.header.kind, Kind::Poa);
        assert_eq!(response.message.header.reference, Some(view.point_out_id));
        assert_eq!(
            optional_field_code(response.message.f07.as_ref()).expect("Mode A parses"),
            Some(0o4321)
        );
        assert_eq!(response.view.status, PointOutStatus::Approved);
        assert_eq!(
            response.view.assigned_comparison,
            BeaconComparison::Mismatch
        );
        assert!(parse_exact(&response.framed_bytes).is_ok());
        let logical_ack = bare(header(
            Kind::Lam,
            PEER,
            LOCAL,
            22,
            Some(response.message.header.id),
        ));
        let acknowledged = nam_pointout_receive(
            &mut runtime,
            &mut flights,
            &mut ingress,
            &framed(&logical_ack),
            4,
            LOCAL.0,
            context(3_000_000_000),
            &[],
        )
        .expect("LAM is correlated to the sent POA");
        let acknowledged = acknowledged.view.expect("point-out view");
        assert_eq!(acknowledged.status, PointOutStatus::Approved);
        assert_eq!(acknowledged.ack_status, PointOutAckStatus::Received);
    }

    #[test]
    fn no_response_profile_is_explicit_and_poi_refusal_does_not_consume_sequence() {
        let mut runtime = nam_pointout_runtime_new(2, PointOutMode::Basic).expect("runtime");
        assert_eq!(
            nam_pointout_set_mode(&mut runtime, PointOutMode::Enhanced),
            Err(PointOutError::ResponseProfileRequired)
        );
        let mut flights = empty_flights();
        let mut ingress = active_ingress(LOCAL, PEER);
        let request = poi(PEER, LOCAL, 21);
        let before = nam_session_ingress_state(&ingress);
        let mut missing_sector = context(2_000_000_000);
        missing_sector.response_sector = None;
        assert_eq!(
            nam_pointout_receive(
                &mut runtime,
                &mut flights,
                &mut ingress,
                &framed(&request),
                2,
                LOCAL.0,
                missing_sector,
                &[],
            )
            .expect_err("missing configured local sector refuses before session commit"),
            PointOutError::MissingFields
        );
        assert_eq!(nam_session_ingress_state(&ingress), before);
        assert!(nam_pointout_receive(
            &mut runtime,
            &mut flights,
            &mut ingress,
            &framed(&request),
            2,
            LOCAL.0,
            context(2_000_000_000),
            &[],
        )
        .is_ok());
    }

    #[test]
    fn uncorrelated_poa_is_refused_without_committing_nam_delivery_state() {
        let mut runtime = enhanced_runtime(PointOutResponseProfile::IcdPartIiSection37Lam);
        let mut flights = empty_flights();
        let mut ingress = active_ingress(LOCAL, PEER);
        let sent = nam_pointout_send_offer(
            &mut runtime,
            &mut flights,
            &mut ingress,
            poi(LOCAL, PEER, 0),
            context(1_000_000_000),
            2,
            LOCAL.0,
            &[],
        )
        .expect("outgoing POI");
        let mut poa = poi(PEER, LOCAL, 9);
        poa.header.kind = Kind::Poa;
        poa.header.reference = Some(MsgId {
            sender: LOCAL,
            receiver: PEER,
            number: sent.view.point_out_id.number.wrapping_add(1) % 1000,
        });
        poa.f32 = None;
        poa.f31 = Some(Field31 {
            facility: PEER,
            sector: Some(*b"2A"),
            text: None,
        });
        let before = nam_session_ingress_state(&ingress);
        assert_eq!(
            nam_pointout_receive(
                &mut runtime,
                &mut flights,
                &mut ingress,
                &framed(&poa),
                3,
                LOCAL.0,
                context(2_000_000_000),
                &[],
            )
            .expect_err("unknown POA reference is refused before ingress commit"),
            PointOutError::ReferenceMismatch
        );
        assert_eq!(nam_session_ingress_state(&ingress), before);
        assert_eq!(
            nam_pointout_views(&runtime)
                .next()
                .expect("outbound point-out retained")
                .status,
            PointOutStatus::AwaitingComputerResponse
        );
    }

    #[test]
    fn annex_b_no_response_is_explicit_and_terminal_records_can_be_retired() {
        let mut runtime = enhanced_runtime(PointOutResponseProfile::IcdAnnexBNoResponse);
        let mut flights = empty_flights();
        let mut ingress = active_ingress(LOCAL, PEER);
        let received = nam_pointout_receive(
            &mut runtime,
            &mut flights,
            &mut ingress,
            &framed(&poi(PEER, LOCAL, 21)),
            2,
            LOCAL.0,
            context(2_000_000_000),
            &[],
        )
        .expect("POI admitted");
        let id = received.view.expect("point-out").point_out_id;
        let decision = nam_pointout_decide(
            &mut runtime,
            &mut ingress,
            id,
            PointOutDecision::Deny,
            Field31 {
                facility: LOCAL,
                sector: Some(*b"2A"),
                text: None,
            },
            3,
            LOCAL.0,
        )
        .expect("explicit configured no-response decision");
        assert_eq!(decision.message.header.kind, Kind::Poj);
        assert_eq!(decision.view.ack_status, PointOutAckStatus::NotExpected);
        assert_eq!(nam_pointout_views(&runtime).count(), 1);
        let retired = nam_pointout_retire(
            &mut runtime,
            id,
            None,
            PointOutRetirementReason::AirspaceExitedAndConflictsResolved,
        )
        .expect("operator lifecycle event retires terminal record");
        assert_eq!(retired.status, PointOutStatus::Denied);
        assert_eq!(nam_pointout_views(&runtime).count(), 0);
    }

    #[test]
    fn pending_poi_timeout_is_visible_and_unblocks_safe_retirement() {
        let mut runtime = enhanced_runtime(PointOutResponseProfile::IcdPartIiSection37Lam);
        let mut flights = empty_flights();
        let mut ingress = active_ingress(LOCAL, PEER);
        let sent = nam_pointout_send_offer(
            &mut runtime,
            &mut flights,
            &mut ingress,
            poi(LOCAL, PEER, 0),
            context(1_000_000_000),
            2,
            LOCAL.0,
            &[],
        )
        .expect("outgoing POI");
        let tick = nam_pointout_tick(&mut runtime, &mut ingress, 60_002).expect("session tick");
        assert_eq!(tick.timed_out, vec![sent.view.point_out_id]);
        let view = nam_pointout_views(&runtime).next().expect("retained view");
        assert_eq!(view.status, PointOutStatus::ResponseTimedOut);
        assert_eq!(view.ack_status, PointOutAckStatus::TimedOut);
        nam_pointout_retire(
            &mut runtime,
            sent.view.point_out_id,
            None,
            PointOutRetirementReason::AirspaceExitedAndConflictsResolved,
        )
        .expect("timeout ends pending response lifecycle");
        assert_eq!(nam_pointout_views(&runtime).count(), 0);
    }

    #[test]
    fn outbound_poi_requires_correlated_pla_before_enhanced_poa_and_duplicate_is_refused() {
        let mut runtime = enhanced_runtime(PointOutResponseProfile::IcdPartIiSection37Lam);
        let mut flights = empty_flights();
        let mut ingress = active_ingress(LOCAL, PEER);
        let sent = nam_pointout_send_offer(
            &mut runtime,
            &mut flights,
            &mut ingress,
            poi(LOCAL, PEER, 0),
            context(1_000_000_000),
            2,
            LOCAL.0,
            &[],
        )
        .expect("outgoing POI uses active session");
        assert_eq!(sent.view.status, PointOutStatus::AwaitingComputerResponse);

        let mut pla = bare(header(
            Kind::Pla,
            PEER,
            LOCAL,
            8,
            Some(sent.view.point_out_id),
        ));
        pla.f31 = Some(Field31 {
            facility: PEER,
            sector: Some(*b"1A"),
            text: None,
        });
        let accepted = nam_pointout_receive(
            &mut runtime,
            &mut flights,
            &mut ingress,
            &framed(&pla),
            3,
            LOCAL.0,
            context(2_000_000_000),
            &[],
        )
        .expect("PLA is correlated to the outstanding POI");
        assert_eq!(
            accepted.view.expect("view").status,
            PointOutStatus::AwaitingRemoteControllerDecision
        );

        let mut poa = poi(PEER, LOCAL, 9);
        poa.header.kind = Kind::Poa;
        poa.header.reference = Some(sent.view.point_out_id);
        poa.f32 = None;
        poa.f31 = Some(Field31 {
            facility: PEER,
            sector: Some(*b"2A"),
            text: None,
        });
        let accepted = nam_pointout_receive(
            &mut runtime,
            &mut flights,
            &mut ingress,
            &framed(&poa),
            4,
            LOCAL.0,
            context(3_000_000_000),
            &[],
        )
        .expect("encoded POA is admitted on the real delivery path");
        let view = accepted.view.expect("view");
        assert_eq!(view.status, PointOutStatus::Approved);
        assert_eq!(view.assigned_mode_a, Some(0o1234));
        assert_eq!(view.assigned_comparison, BeaconComparison::Match);
        assert!(accepted.response_actions.as_ref().is_some_and(|actions| {
            actions.as_slice().iter().any(|action| {
                matches!(
                    action,
                    session::Action::SendResponse { kind: Kind::Lam, reference, .. }
                        if *reference == poa.header.id
                )
            })
        }));
        assert_eq!(
            parse_exact(accepted.response_framed.as_deref().expect("encoded LAM"))
                .expect("LAM parses")
                .header
                .reference,
            Some(poa.header.id)
        );
        assert_eq!(
            nam_pointout_receive(
                &mut runtime,
                &mut flights,
                &mut ingress,
                &framed(&poa),
                5,
                LOCAL.0,
                context(4_000_000_000),
                &[],
            )
            .expect_err("duplicate delivery must not repeat state transition"),
            PointOutError::InvalidTransition,
        );
    }

    #[test]
    fn beacon_comparison_preserves_unknown_conflict_stale_and_future() {
        let track = ObservedTrack {
            state_ns: 10,
            newest_source_ns: 10,
            address: 0xabc123,
            ident: 0o1234,
            ident_ns: 10,
            ident_conflict: 0,
            lat_e7: 0,
            lon_e7: 0,
            altitude_ft: 0,
            vertical_rate_fpm: 0,
            east_mmps: 0,
            north_mmps: 0,
            r95_mm: 0,
        };
        assert_eq!(
            compare_observed(Some(0o1234), Some(&track), 10, 10),
            BeaconComparison::Match
        );
        assert_eq!(
            compare_observed(Some(0o1234), Some(&track), 11, 11),
            BeaconComparison::Stale
        );
        assert_eq!(
            compare_observed(Some(0o1234), Some(&track), 9, 9),
            BeaconComparison::Future
        );
        let mut conflict = track;
        conflict.ident_conflict = 1;
        assert_eq!(
            compare_observed(Some(0o1234), Some(&conflict), 10, 10),
            BeaconComparison::Conflict
        );
        assert_eq!(
            compare_observed(Some(0o1234), None, 10, 10),
            BeaconComparison::Unknown
        );
    }
}

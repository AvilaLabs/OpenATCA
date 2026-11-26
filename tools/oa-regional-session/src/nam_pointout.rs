//! Session-admitted NAM point-out state, kept separate from flight ownership.
//!
//! Rev G §3.7 distinguishes POI/PLA computer receipt from the controller's operational decision:
//! Basic point-outs require manual coordination; only Enhanced POA/POJ messages express approval
//! or denial. This module never changes `oa-flight` ownership and never treats PLA/LAM as approval.

use crate::flight::{self, FlightDriver, FlightViewStatus, SharedFlightView};
use crate::nam_handoff::{
    accepted_delivery_seq, accepted_message, nam_session_ingress_local_event,
    nam_session_ingress_receive, NamIngressResult, NamSessionIngress,
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
    Approved,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeaconComparison {
    Unknown,
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
    InvalidTransition,
}

#[derive(Clone, Debug)]
pub struct PointOutResult {
    pub ingress: NamIngressResult,
    /// Logical computer response actions, when POI needs a PLA. They do not represent approval.
    pub response_actions: Option<session::Actions>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    view: PointOutView,
    source: Message,
    outbound: bool,
}

pub struct NamPointOutRuntime {
    mode: PointOutMode,
    records: Vec<Record>,
    capacity: usize,
}

pub fn nam_pointout_runtime_new(capacity: usize, mode: PointOutMode) -> Option<NamPointOutRuntime> {
    if capacity == 0 || capacity > MAX_POINT_OUTS {
        return None;
    }
    Some(NamPointOutRuntime {
        mode,
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
    runtime.mode = mode;
    Ok(())
}

pub fn nam_pointout_views(runtime: &NamPointOutRuntime) -> impl Iterator<Item = &PointOutView> {
    runtime.records.iter().map(|record| &record.view)
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
    let observed_comparison = compare_observed(established_mode_a, observed_track);
    // Validate the caller's message before consuming a session message number.
    message.header.id.number = 0;
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
        established_mode_a,
        assigned_mode_a: None,
        observed_comparison,
        assigned_comparison: BeaconComparison::Unknown,
        source_ns: context.source_ns,
        receipt_ns: context.receipt_ns,
    };
    runtime.records.push(Record {
        view,
        source: message.clone(),
        outbound: true,
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
    let id = MsgId {
        sender: oa_nam::Facility(local_facility),
        receiver: source.header.id.sender,
        number: 0,
    };
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
    let actions = nam_session_ingress_local_event(ingress, session::Event::SendAtm(kind), now_ms)
        .map_err(|_| PointOutError::SessionResponse)?;
    let number = actions
        .as_slice()
        .iter()
        .find_map(|action| match action {
            session::Action::SendAtm { kind: sent, number } if *sent == kind => Some(*number),
            _ => None,
        })
        .ok_or(PointOutError::SessionResponse)?;
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
    let admission = nam_session_ingress_receive(ingress, framed_bytes, now_ms)
        .map_err(|_| PointOutError::NotAdmitted)?;
    let accepted = admission
        .accepted
        .as_ref()
        .ok_or(PointOutError::NotAdmitted)?;
    if accepted_message(accepted).header != message.header
        || !matches!(
            accepted_delivery_seq(accepted),
            Some(session::Seq::First | session::Seq::InOrder | session::Seq::Gap(_))
        )
    {
        return Err(PointOutError::NotAdmitted);
    }
    let context_view = positive_binding(flights, context.binding, context.receipt_ns)?;
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
            &admission,
            message,
            context,
            context_view,
            local_facility,
        ),
        Kind::Pla => receive_logical_ack(runtime, &admission, message, context, context_view),
        _ => Err(PointOutError::WrongKind),
    }
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
    let observed_comparison = compare_observed(established_mode_a, observed_track);
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
        established_mode_a,
        assigned_mode_a: None,
        observed_comparison,
        assigned_comparison: BeaconComparison::Unknown,
        source_ns: context.source_ns,
        receipt_ns: context.receipt_ns,
    };
    let response_actions = nam_session_ingress_local_event(
        ingress,
        session::Event::SendResponse {
            kind: Kind::Pla,
            reference: message.header.id,
        },
        now_ms,
    )
    .map_err(|_| PointOutError::SessionResponse)?;
    runtime.records.push(Record {
        view,
        source: message.clone(),
        outbound: false,
    });
    Ok(PointOutResult {
        ingress: ingress_result.clone(),
        response_actions: Some(response_actions),
        view: Some(view),
    })
}

fn receive_decision(
    runtime: &mut NamPointOutRuntime,
    ingress: &NamIngressResult,
    message: &Message,
    context: PointOutContext,
    binding_view: Option<SharedFlightView>,
    local_facility: [u8; 4],
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
        .get_mut(index)
        .ok_or(PointOutError::ReferenceMismatch)?;
    if !record.outbound {
        return Err(PointOutError::InvalidTransition);
    }
    let current = &mut record.view;
    if message.header.id.sender.0 != current.to
        || message.header.id.receiver.0 != current.from
        || local_facility != current.from
        || !same_context(current, context)
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
    current.status = if message.header.kind == Kind::Poa {
        PointOutStatus::Approved
    } else {
        PointOutStatus::Denied
    };
    if message.header.kind == Kind::Poa {
        current.assigned_mode_a = optional_field_code(message.f07.as_ref())?;
        current.assigned_comparison =
            compare_codes(current.established_mode_a, current.assigned_mode_a);
    }
    Ok(PointOutResult {
        ingress: ingress.clone(),
        response_actions: None,
        view: Some(*current),
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
    if message.header.id.sender.0 != record.view.to
        || message.header.id.receiver.0 != record.view.from
        || !same_context(&record.view, context)
        || binding_view.is_some_and(|view| Some(view.address) != record.view.address)
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
    record.view.status = match runtime.mode {
        PointOutMode::Basic => PointOutStatus::AwaitingManualCoordination,
        PointOutMode::Enhanced => PointOutStatus::AwaitingControllerDecision,
    };
    Ok(PointOutResult {
        ingress: ingress.clone(),
        response_actions: None,
        view: Some(record.view),
    })
}

fn same_context(view: &PointOutView, context: PointOutContext) -> bool {
    view.key == context.binding.map(|binding| binding.key)
        && view.route_key == context.binding.map(|binding| binding.route_key)
        && context.source_ns >= view.source_ns
        && context.receipt_ns >= view.receipt_ns
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

fn compare_observed(code: Option<u16>, track: Option<&ObservedTrack>) -> BeaconComparison {
    let Some(track) = track else {
        return BeaconComparison::Unknown;
    };
    if track.ident_conflict != 0 {
        return BeaconComparison::Conflict;
    }
    if track.ident == oa_track::NO_IDENT || track.ident_ns == oa_track::NO_IDENT_NS {
        return BeaconComparison::Unknown;
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

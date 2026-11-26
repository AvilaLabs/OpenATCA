//! Recorded/synthetic NAM handoff projection into the flight-table adapter (NAS-IC-21009205
//! Rev G Part II §§3.6.1–3.6.4 and B.1.8.1–.3).
//!
//! This module is an unapplied F1 integration draft. Its ingress owns framing and feeds parsed
//! headers to `oa_nam`'s session state machine; only session-admitted application messages become
//! opaque mapper tokens. Callers supply the explicit flight key and endpoint-unit mapping. It
//! does not infer flight identity from callsign, facility, or route.
//! NAM carries no absolute event timestamp; supplied source/receipt times are external capture
//! metadata and must be equal when no independent source clock exists. No RTI/RTA decision timeout
//! is synthesized (Part III §6.1 gives transport/flight-plan response limits, not one).

use crate::input::{
    EventKind, FlightOperation, HandoffOperation, Identity, NormalizedEvent, SourceKind,
};
use oa_flight::FlightKey;
use oa_nam::{frame::unframe, parse, session, Field31, Kind, Message, MsgId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamHandoffError {
    InvalidFrame,
    InvalidMessage,
    UnexpectedKind,
    MissingDestination,
    IdentityMismatch,
    ReferenceMismatch,
    WrongPeer,
    MissingRoutingMap,
    RoutingMismatch,
    InvalidTransition,
    ClosedHandoff,
    InvalidTime,
    InvalidSequence,
    SessionNotAdmitted,
    SequenceGapCountOverflow,
}

/// The only frame-to-message ingress. The NAM session owns the framed bytes' header admission,
/// peer check, response correlation, and sequence classification before a handoff mapper sees
/// a message.
#[derive(Clone, Debug)]
// A7.2: cold shell adapter state, containing one bounded oa-nam session.
pub struct NamSessionIngress {
    config: session::Config,
    state: session::Session,
    sequence_gap_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamAdmission {
    Deliver(session::Seq),
    CorrelatedRtiResponse,
}

/// Opaque token proving a complete frame was parsed and admitted by `oa-nam::session::step`.
/// Its fields are private so callers cannot construct a raw-message bypass.
#[derive(Clone, Debug)]
pub struct SessionAcceptedMessage {
    message: Message,
    admission: NamAdmission,
}

/// Read the parsed message carried by an opaque token; callers still cannot construct tokens.
pub const fn accepted_message(accepted: &SessionAcceptedMessage) -> &Message {
    &accepted.message
}

/// Return a delivery classification for application messages; correlated protocol responses
/// intentionally return `None` because they are admitted by response correlation instead.
pub const fn accepted_delivery_seq(accepted: &SessionAcceptedMessage) -> Option<session::Seq> {
    match accepted.admission {
        NamAdmission::Deliver(seq) => Some(seq),
        NamAdmission::CorrelatedRtiResponse => None,
    }
}

#[derive(Clone, Debug)]
pub struct NamIngressResult {
    pub accepted: Option<SessionAcceptedMessage>,
    pub actions: session::Actions,
    /// Cumulative count of application deliveries classified by NAM as sequence gaps.
    pub sequence_gap_count: u64,
}

pub const fn nam_session_ingress_new(config: session::Config) -> NamSessionIngress {
    NamSessionIngress {
        config,
        state: session::new_session(),
        sequence_gap_count: 0,
    }
}

pub const fn nam_session_ingress_state(ingress: &NamSessionIngress) -> session::Session {
    ingress.state
}

pub const fn nam_session_ingress_sequence_gap_count(ingress: &NamSessionIngress) -> u64 {
    ingress.sequence_gap_count
}

/// Drive local events through the NAM state machine. `Received` is forbidden here; inbound
/// frames must use `nam_session_ingress_receive` so callers cannot bypass parsing or peer checks.
pub fn nam_session_ingress_local_event(
    ingress: &mut NamSessionIngress,
    event: session::Event,
    now_ms: u64,
) -> Result<session::Actions, NamHandoffError> {
    if matches!(event, session::Event::Received(_)) {
        return Err(NamHandoffError::SessionNotAdmitted);
    }
    let (next, actions) = session::step(&ingress.config, ingress.state, event, now_ms);
    ingress.state = next;
    if actions.dropped != 0 {
        return Err(NamHandoffError::SessionNotAdmitted);
    }
    Ok(actions)
}

/// Parse one exact framed message, run the public NAM session state machine, and only create a
/// mapper token for `Deliver` or a response correlated to an outstanding local RTI.
pub fn nam_session_ingress_receive(
    ingress: &mut NamSessionIngress,
    bytes: &[u8],
    now_ms: u64,
) -> Result<NamIngressResult, NamHandoffError> {
    let message = parse_one_frame(bytes)?;
    let (next, actions) = session::step(
        &ingress.config,
        ingress.state,
        session::Event::Received(message.header),
        now_ms,
    );
    ingress.state = next;
    if actions.dropped != 0 {
        return Err(NamHandoffError::SessionNotAdmitted);
    }
    let delivered_seq = actions.as_slice().iter().find_map(|action| match action {
        session::Action::Deliver { seq, .. } => Some(*seq),
        _ => None,
    });
    if matches!(delivered_seq, Some(session::Seq::Gap(_))) {
        ingress.sequence_gap_count = ingress
            .sequence_gap_count
            .checked_add(1)
            .ok_or(NamHandoffError::SequenceGapCountOverflow)?;
    }
    let admission = match actions.as_slice() {
        [session::Action::Deliver { header, seq }] if *header == message.header => {
            Some(NamAdmission::Deliver(*seq))
        }
        [session::Action::Response {
            header,
            answers: Kind::Rti,
        }] if *header == message.header && matches!(message.header.kind, Kind::Rla | Kind::Lrm) => {
            Some(NamAdmission::CorrelatedRtiResponse)
        }
        _ => None,
    };
    let accepted = admission.map(|admission| SessionAcceptedMessage { message, admission });
    Ok(NamIngressResult {
        accepted,
        actions,
        sequence_gap_count: ingress.sequence_gap_count,
    })
}

/// A concrete caller-provided association. `key` must be the full existing `oa-flight` key;
/// units are configured endpoint identities, never padded from a facility code by this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtiBinding {
    key: FlightKey,
    rti_id: MsgId,
    origin: [u8; 4],
    destination: [u8; 4],
    origin_unit: [u8; 8],
    destination_unit: [u8; 8],
}

/// Accepted RTI plus the flight-side handoff phase. NAM sequence state remains owned by the
/// associated `NamSessionIngress` rather than a second adapter cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandoffState {
    binding: RtiBinding,
    phase: HandoffPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandoffPhase {
    AwaitingResponse,
    Routed,
    Closed,
}

impl HandoffState {
    /// Exact message identifier of the RTI this state was created from.
    pub const fn rti_id(&self) -> MsgId {
        self.binding.rti_id
    }
}

/// Caller-supplied exact target and unit mapping for a newly received RTI. The caller must
/// obtain `key` from its existing flight identity binding, not a callsign-only search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtiTarget {
    pub key: FlightKey,
    pub expected_destination_sector: Option<[u8; 2]>,
    pub origin_unit: [u8; 8],
    pub destination_unit: [u8; 8],
}

/// Exact endpoint configuration for the sector named by an RLA's Field 31.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RlaRoute {
    pub facility: [u8; 4],
    pub sector: Option<[u8; 2]>,
    pub unit: [u8; 8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NamEventTime {
    source_ns: i64,
    receipt_ns: i64,
    sequence: u64,
}

fn event(
    message: &Message,
    key: FlightKey,
    operation: HandoffOperation,
    from_unit: [u8; 8],
    to_unit: [u8; 8],
    time: NamEventTime,
) -> Result<NormalizedEvent, NamHandoffError> {
    if time.source_ns < 0
        || time.receipt_ns < time.source_ns
        || time.source_ns % 1_000_000 != 0
        || time.receipt_ns % 1_000_000 != 0
    {
        return Err(NamHandoffError::InvalidTime);
    }
    if time.sequence == 0 || message.header.id.sender.0 == [0; 4] {
        return Err(NamHandoffError::InvalidSequence);
    }
    Ok(NormalizedEvent {
        receipt_ns: time.receipt_ns,
        source_ns: time.source_ns,
        sequence: time.sequence,
        source: SourceKind::Flight,
        // NAM does not carry GUFI. The exact key is carried by HandoffByKey instead.
        identity: Identity {
            flight_gufi: 0,
            aircraft_address: 0,
        },
        centre: message.header.id.sender.0,
        kind: EventKind::Flight(FlightOperation::HandoffByKey {
            key,
            operation,
            from_unit,
            to_unit,
        }),
    })
}

fn parse_one_frame(bytes: &[u8]) -> Result<Message, NamHandoffError> {
    let (message, consumed) = unframe(bytes)
        .map_err(|_| NamHandoffError::InvalidFrame)?
        .ok_or(NamHandoffError::InvalidFrame)?;
    if consumed != bytes.len() {
        return Err(NamHandoffError::InvalidFrame);
    }
    parse(message).map_err(|_| NamHandoffError::InvalidMessage)
}

fn destination(message: &Message) -> Result<Field31, NamHandoffError> {
    message
        .f31
        .clone()
        .ok_or(NamHandoffError::MissingDestination)
}

/// Translate an encoded+framed RTI using an explicit full-key binding. The returned binding is
/// required for later RLA/RTA/LRM matching. Header facility and Field 31 must agree exactly.
pub fn receive_rti(
    accepted: &SessionAcceptedMessage,
    target: RtiTarget,
    source_ns: i64,
    receipt_ns: i64,
    sequence: u64,
) -> Result<(HandoffState, NormalizedEvent), NamHandoffError> {
    let message = &accepted.message;
    match accepted.admission {
        NamAdmission::Deliver(
            session::Seq::First | session::Seq::InOrder | session::Seq::Gap(_),
        ) => {}
        NamAdmission::Deliver(session::Seq::Duplicate) => {
            return Err(NamHandoffError::SessionNotAdmitted)
        }
        NamAdmission::CorrelatedRtiResponse => return Err(NamHandoffError::SessionNotAdmitted),
    }
    if message.header.kind != Kind::Rti {
        return Err(NamHandoffError::UnexpectedKind);
    }
    let f31 = destination(message)?;
    if message.header.id.receiver.0 != f31.facility.0
        || message.header.id.sender.0 == f31.facility.0
        || f31.sector != target.expected_destination_sector
    {
        return Err(NamHandoffError::IdentityMismatch);
    }
    let binding = RtiBinding {
        key: target.key,
        rti_id: message.header.id,
        origin: message.header.id.sender.0,
        destination: f31.facility.0,
        origin_unit: target.origin_unit,
        destination_unit: target.destination_unit,
    };
    let output = event(
        message,
        binding.key,
        HandoffOperation::Offer,
        binding.origin_unit,
        binding.destination_unit,
        NamEventTime {
            source_ns,
            receipt_ns,
            sequence,
        },
    )?;
    Ok((
        HandoffState {
            binding,
            phase: HandoffPhase::AwaitingResponse,
        },
        output,
    ))
}

/// Translate a response to one exact pending RTI. RLA is only OP_HO_UPDATE; target-origin RTA
/// is acceptance; initiator-origin RTA is retraction; a target LRM is failure. RTU has no
/// ownership operation and returns `None`. Timeout is intentionally not synthesized here.
pub fn receive_response(
    accepted: &SessionAcceptedMessage,
    state: &mut HandoffState,
    rla_route: Option<RlaRoute>,
    source_ns: i64,
    receipt_ns: i64,
    sequence: u64,
) -> Result<Option<NormalizedEvent>, NamHandoffError> {
    let binding = state.binding;
    if state.phase == HandoffPhase::Closed {
        return Err(NamHandoffError::ClosedHandoff);
    }
    let message = &accepted.message;
    match (message.header.kind, accepted.admission) {
        (Kind::Rla | Kind::Lrm, NamAdmission::CorrelatedRtiResponse) => {}
        (
            Kind::Rta | Kind::Rtu,
            NamAdmission::Deliver(
                session::Seq::First | session::Seq::InOrder | session::Seq::Gap(_),
            ),
        ) => {}
        (_, NamAdmission::Deliver(session::Seq::Duplicate)) => {
            return Err(NamHandoffError::SessionNotAdmitted)
        }
        _ => return Err(NamHandoffError::SessionNotAdmitted),
    }
    if message.header.reference != Some(binding.rti_id) {
        return Err(NamHandoffError::ReferenceMismatch);
    }
    let id = message.header.id;
    let (operation, expected_sender, expected_receiver) = match message.header.kind {
        Kind::Rla => (
            Some(HandoffOperation::Update),
            binding.destination,
            binding.origin,
        ),
        Kind::Rta if id.sender.0 == binding.destination => (
            Some(HandoffOperation::Accept),
            binding.destination,
            binding.origin,
        ),
        Kind::Rta if id.sender.0 == binding.origin => (
            Some(HandoffOperation::Retract),
            binding.origin,
            binding.destination,
        ),
        Kind::Lrm => (
            Some(HandoffOperation::Fail),
            binding.destination,
            binding.origin,
        ),
        Kind::Rtu => (None, binding.origin, binding.destination),
        _ => return Err(NamHandoffError::UnexpectedKind),
    };
    if id.sender.0 != expected_sender || id.receiver.0 != expected_receiver {
        return Err(NamHandoffError::WrongPeer);
    }
    match message.header.kind {
        Kind::Rta if id.sender.0 == binding.destination && state.phase != HandoffPhase::Routed => {
            return Err(NamHandoffError::InvalidTransition);
        }
        Kind::Rtu if state.phase != HandoffPhase::Routed => {
            return Err(NamHandoffError::InvalidTransition);
        }
        _ => {}
    }
    let routed_unit = if message.header.kind == Kind::Rla {
        let mapped = rla_route.ok_or(NamHandoffError::MissingRoutingMap)?;
        let f31 = destination(message)?;
        if mapped.facility != f31.facility.0 || mapped.sector != f31.sector {
            return Err(NamHandoffError::RoutingMismatch);
        }
        Some(mapped.unit)
    } else {
        if rla_route.is_some() {
            return Err(NamHandoffError::RoutingMismatch);
        }
        None
    };
    let translated = operation
        .map(|operation| {
            event(
                message,
                binding.key,
                operation,
                binding.origin_unit,
                routed_unit.unwrap_or(binding.destination_unit),
                NamEventTime {
                    source_ns,
                    receipt_ns,
                    sequence,
                },
            )
        })
        .transpose()?;
    if operation.is_none() && (source_ns < 0 || receipt_ns < source_ns || sequence == 0) {
        return Err(NamHandoffError::InvalidTime);
    }
    if let Some(unit) = routed_unit {
        state.binding.destination_unit = unit;
        state.phase = HandoffPhase::Routed;
    }
    if matches!(message.header.kind, Kind::Rta | Kind::Lrm) {
        state.phase = HandoffPhase::Closed;
    }
    Ok(translated)
}

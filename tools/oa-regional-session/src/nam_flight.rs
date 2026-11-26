//! Session-admitted NAM FPL/CHG/DLA/CNL application to the regional flight runtime.
//! Shell (B1, B3, B7): NAM contains no absolute event clock or positive flight key, so this
//! adapter requires verified caller context and consumes only opaque session-delivery tokens.
//! Dependency reason: only the regional shell knows the verified source-to-full-key binding and
//! owns the existing flight, route-revision, and trajectory-consumer stores.

use crate::flight::{flight_apply_event, FlightDriver, FlightDriverError};
use crate::input::{EventKind, FlightOperation, Identity, NormalizedEvent, SourceKind};
use crate::nam_handoff::{
    accepted_delivery_seq, accepted_message, nam_session_ingress_receive, NamHandoffError,
    NamIngressResult, NamSessionIngress, SessionAcceptedMessage,
};
use crate::route::{self, RouteStore, MAX_ROUTE_REVISIONS};
use oa_airspace::core::model::{self as air_model, Model};
use oa_airspace::shell::route_parse::{
    resolve_icao_item15, Item15Reject as RouteReject, MagneticVariationSource,
};
use oa_flight::{FlightKey, Outcome, KEY_GUFI, ST_ACTIVE, ST_CANCELLED, ST_PROPOSED};
use oa_nam::item15::{catalogue_lrm_reply, filed_change, lrm_reply, FiledChange, Item15Reject};
use oa_nam::{model, Kind, Message, MsgId, Profile};

/// Maximum filed plans retained by one regional NAM runtime.
pub const MAX_NAM_PLANS: usize = 8192;
const MANUAL_COORDINATION_CODE: u8 = 62;

/// Verified binding and capture metadata supplied by the existing source context. None of these
/// values are derived from a NAM callsign, message number, or route text. NAM has no timestamp;
/// absent an independent source clock, the capture supplies one shared source/receipt instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamFlightContext {
    pub key: FlightKey,
    /// Complete trajectory identity supplied by the source context for the existing route store.
    pub route_key: oa_legacy_traj::core::trajectory::FlightKey,
    pub plan_id: [u8; 12],
    pub centre: [u8; 4],
    pub plan_status: u8,
    pub source_ns: i64,
    pub receipt_ns: i64,
    /// The NAM session supplies this response number for the next outbound message.
    pub response_number: u16,
}

/// Externally visible result; `reply` is an encoded-model ACK/REJ value, not a wire send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamFlightResult {
    pub reply: Message,
    pub applied: bool,
    pub rejection: Option<NamFlightReject>,
    /// Advisory result from the exact verified NASR PFR source. It never controls ACK/REJ.
    pub preferred_route: PreferredRouteAssessment,
}

/// Status of advisory PFR comparison for an applied filed route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PreferredRouteStatus {
    /// This message did not replace a route.
    NotAssessed = 0,
    /// The verified adaptation predates PFR data or has no PFR source.
    SourceUnavailable = 1,
    /// The published data has no row for this airport pair.
    NoPublishedPair = 2,
    /// PFR records exist for the pair, but none of their converted routes match this route.
    RouteNotPreferred = 3,
    /// A published route and all applicable known type/direction fields match; no unsupported
    /// condition description is present.
    Preferred = 4,
    /// A candidate route matches, but one or more source condition descriptions need
    /// coordination because FAA publishes no machine grammar for them.
    CoordinationRequired = 5,
    /// The PFR row or route could not be converted safely against this adaptation.
    Unavailable = 6,
}

/// Bounded PFR outcome tied to the exact NASR row included in the verified adaptation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreferredRouteAssessment {
    pub status: PreferredRouteStatus,
    /// Row in `Model::preferred_routes`, or `u32::MAX` when no row was selected.
    pub route_row: u32,
    /// Effective date from the source archive bound into the model.
    pub nasr_effective: u32,
    /// SHA-256 of the exact NASR archive containing the PFR source.
    pub nasr_sha256: [u8; 32],
}

fn no_pfr_assessment(status: PreferredRouteStatus) -> PreferredRouteAssessment {
    PreferredRouteAssessment {
        status,
        route_row: u32::MAX,
        nasr_effective: 0,
        nasr_sha256: [0; 32],
    }
}

fn model_pfr_assessment(
    model: &Model,
    status: PreferredRouteStatus,
    route_row: Option<usize>,
) -> PreferredRouteAssessment {
    PreferredRouteAssessment {
        status,
        route_row: route_row
            .and_then(|row| u32::try_from(row).ok())
            .unwrap_or(u32::MAX),
        nasr_effective: model.meta.nasr_effective,
        nasr_sha256: model.meta.nasr_sha256,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamFlightReject {
    NotSessionAdmitted,
    DuplicateOrOutOfOrder,
    MissingOrInvalidContext,
    WrongMessageKind,
    UnknownPlan,
    DuplicatePlan,
    CancelledPlan,
    FlightRuntimeRefusal,
    Route(Item15Reject),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FiledPlan {
    key: FlightKey,
    plan_id: [u8; 12],
    status: u8,
    departure_airport: [u8; 4],
    destination_airport: [u8; 4],
    departure_time: Option<model::Hhmm>,
    cancelled: bool,
    preferred_route: PreferredRouteAssessment,
}

/// Bounded application state for filed NAM plans. The authoritative flight state remains in
/// `FlightDriver`; this table retains only the filed route and Appendix 3 departure time that the
/// flight core does not model.
pub struct NamFlightRuntime {
    plans: Vec<FiledPlan>,
    capacity: usize,
    last_applied_number: Option<u16>,
}

pub fn nam_flight_runtime_new(capacity: usize) -> Option<NamFlightRuntime> {
    if capacity == 0 || capacity > MAX_NAM_PLANS {
        return None;
    }
    Some(NamFlightRuntime {
        plans: Vec::with_capacity(capacity),
        capacity,
        last_applied_number: None,
    })
}

/// Receive one framed NAM flight-plan message through the session-enforced ingress, then apply
/// its opaque delivery token to the same flight and route stores the regional runtime consumes.
#[allow(clippy::too_many_arguments)] // The shell composes the explicit ingress and subsystem-owned stores.
pub fn nam_flight_receive(
    runtime: &mut NamFlightRuntime,
    flights: &mut FlightDriver,
    routes: &mut RouteStore,
    ingress: &mut NamSessionIngress,
    framed_bytes: &[u8],
    now_ms: u64,
    context: NamFlightContext,
    model: &Model,
    variations: &[MagneticVariationSource],
    profile: Profile,
) -> Result<(NamIngressResult, Option<NamFlightResult>), NamHandoffError> {
    let result = nam_session_ingress_receive(ingress, framed_bytes, now_ms)?;
    let applied = result.accepted.as_ref().map(|accepted| {
        nam_flight_apply(
            runtime, flights, routes, accepted, context, model, variations, profile,
        )
    });
    Ok((result, applied))
}

/// Apply one admitted flight-plan message. Session sequence gaps, duplicates, raw messages,
/// missing context, and all semantic refusals produce a catalogued LRM and leave both runtime
/// state stores unchanged. This function never infers identity from Field 07.
#[allow(clippy::too_many_arguments)] // Explicit stores and source model are shell-owned dependencies.
pub fn nam_flight_apply(
    runtime: &mut NamFlightRuntime,
    flights: &mut FlightDriver,
    routes: &mut RouteStore,
    accepted: &SessionAcceptedMessage,
    context: NamFlightContext,
    model: &Model,
    variations: &[MagneticVariationSource],
    profile: Profile,
) -> NamFlightResult {
    let message = accepted_message(accepted);
    let fail = |reason| rejected(message, context.response_number, profile, reason);
    let number = message.header.id.number;
    let rejection = match accepted_delivery_seq(accepted) {
        Some(oa_nam::session::Seq::First) if runtime.last_applied_number.is_none() => None,
        Some(oa_nam::session::Seq::InOrder)
            if runtime
                .last_applied_number
                .is_some_and(|prior| number == (prior + 1) % 1000) =>
        {
            None
        }
        _ => Some(NamFlightReject::DuplicateOrOutOfOrder),
    };
    if let Some(reason) = rejection {
        return fail(reason);
    }
    runtime.last_applied_number = Some(number);
    if !valid_context(context) {
        return fail(NamFlightReject::MissingOrInvalidContext);
    }
    if !matches!(
        message.header.kind,
        Kind::Fpl | Kind::Chg | Kind::Dla | Kind::Cnl
    ) {
        return fail(NamFlightReject::WrongMessageKind);
    }
    if message.header.kind == Kind::Fpl {
        if runtime.plan_index(context.key, context.plan_id).is_some() {
            return fail(NamFlightReject::DuplicatePlan);
        }
        if runtime.plans.len() >= runtime.capacity {
            return fail(NamFlightReject::FlightRuntimeRefusal);
        }
    }
    let plan_index = runtime.plan_index(context.key, context.plan_id);
    let existing = plan_index.and_then(|index| runtime.plans.get(index));
    if matches!(message.header.kind, Kind::Chg | Kind::Dla | Kind::Cnl) && existing.is_none() {
        return fail(NamFlightReject::UnknownPlan);
    }
    if existing.is_some_and(|plan| plan.cancelled) && message.header.kind != Kind::Fpl {
        return fail(NamFlightReject::CancelledPlan);
    }

    let staged_route = match filed_change(message) {
        Ok(FiledChange::Replace(field)) => {
            let departure = match message.header.kind {
                Kind::Fpl => message.f13.map(|field| field.aerodrome.0),
                _ => existing.map(|plan| plan.departure_airport),
            };
            let destination = match message.header.kind {
                Kind::Fpl => message.f16.as_ref().map(|field| field.aerodrome.0),
                _ => existing.map(|plan| plan.destination_airport),
            };
            let (Some(departure), Some(destination)) = (departure, destination) else {
                return fail(NamFlightReject::Route(Item15Reject::MalformedGroup));
            };
            let level_ft = level_feet(field.level);
            match resolve_icao_item15(
                model,
                &field.route,
                level_ft,
                ascii_code(&departure),
                ascii_code(&destination),
                variations,
            ) {
                Ok(route) => {
                    let preferred_route = assess_preferred_route(
                        model,
                        &departure,
                        &destination,
                        &route,
                        level_ft,
                        variations,
                    );
                    Some((route, departure, destination, preferred_route))
                }
                Err(reason) => return fail(NamFlightReject::Route(nam_route_reject(reason))),
            }
        }
        Ok(FiledChange::Delay {
            departure,
            destination,
        }) => {
            if existing.is_none_or(|plan| {
                plan.departure_airport != departure.aerodrome.0
                    || plan.destination_airport != destination.aerodrome.0
            }) {
                return fail(NamFlightReject::UnknownPlan);
            }
            if departure.time.is_none() {
                return fail(NamFlightReject::Route(Item15Reject::MalformedGroup));
            }
            None
        }
        Ok(FiledChange::Cancel | FiledChange::NoRouteChange) => None,
        Err(reason) => return fail(NamFlightReject::Route(reason)),
    };

    let staged_revision = if let Some((resolved, _, _, _)) = staged_route.as_ref() {
        if routes.revisions.len() + routes.route_refusals.len() >= MAX_ROUTE_REVISIONS {
            return fail(NamFlightReject::FlightRuntimeRefusal);
        }
        let encoded = match oa_nam::encode(message) {
            Ok(value) => value,
            Err(_) => return fail(NamFlightReject::FlightRuntimeRefusal),
        };
        match route::icao_route_revision(
            context.route_key,
            resolved,
            context.source_ns / 1_000_000,
            context.receipt_ns / 1_000_000,
            u64::from(number),
            oa_sha256::sha256(&encoded),
            routes.snapshot_sha256,
        ) {
            Ok(revision) => Some(revision),
            Err(_) => return fail(NamFlightReject::FlightRuntimeRefusal),
        }
    } else {
        None
    };

    let existing_status = existing.map_or(context.plan_status, |plan| plan.status);
    let operation = match message.header.kind {
        Kind::Fpl => FlightOperation::SetPlanByKey {
            key: context.key,
            plan_id: context.plan_id,
            status: context.plan_status,
        },
        Kind::Chg => FlightOperation::SetPlanByKey {
            key: context.key,
            plan_id: context.plan_id,
            status: existing_status,
        },
        Kind::Cnl => FlightOperation::CancelPlanByKey {
            key: context.key,
            plan_id: context.plan_id,
        },
        Kind::Dla => {
            runtime.commit_departure_time(
                context.key,
                context.plan_id,
                message.f13.and_then(|f| f.time),
            );
            let preferred_route = runtime
                .plan_index(context.key, context.plan_id)
                .and_then(|index| runtime.plans.get(index))
                .map_or_else(
                    || model_pfr_assessment(model, PreferredRouteStatus::NotAssessed, None),
                    |plan| plan.preferred_route,
                );
            return accepted_reply(message, context.response_number, preferred_route);
        }
        _ => return fail(NamFlightReject::WrongMessageKind),
    };
    let event = NormalizedEvent {
        receipt_ns: context.receipt_ns,
        source_ns: context.source_ns,
        sequence: u64::from(number),
        source: SourceKind::Flight,
        identity: Identity {
            flight_gufi: context.key.gufi,
            aircraft_address: 0,
        },
        centre: context.centre,
        kind: EventKind::Flight(operation),
    };
    let applied = match flight_apply_event(flights, &event) {
        Ok(result) => matches!(
            result.operation,
            Some(Outcome::Applied | Outcome::Unchanged)
        ),
        Err(
            FlightDriverError::InvalidConfig
            | FlightDriverError::InvalidTimestamp
            | FlightDriverError::SealedTimestamp
            | FlightDriverError::NonMillisecondAdapterTimestamp
            | FlightDriverError::WrongSourceKind
            | FlightDriverError::InvalidBatchOrder
            | FlightDriverError::ReceiptBatchTooLarge,
        ) => false,
    };
    if !applied {
        return fail(NamFlightReject::FlightRuntimeRefusal);
    }
    match message.header.kind {
        Kind::Fpl => {
            let Some((_, departure_airport, destination_airport, preferred_route)) = staged_route
            else {
                return fail(NamFlightReject::Route(Item15Reject::Model));
            };
            runtime.plans.push(FiledPlan {
                key: context.key,
                plan_id: context.plan_id,
                status: context.plan_status,
                departure_airport,
                destination_airport,
                departure_time: message.f13.and_then(|field| field.time),
                cancelled: false,
                preferred_route,
            });
        }
        Kind::Chg => {
            if let Some(index) = plan_index {
                if let Some(plan) = runtime.plans.get_mut(index) {
                    plan.status = existing_status;
                    if let Some((_, _, _, preferred_route)) = staged_route.as_ref() {
                        plan.preferred_route = *preferred_route;
                    }
                }
            }
        }
        Kind::Cnl => {
            if let Some(index) = plan_index {
                if let Some(plan) = runtime.plans.get_mut(index) {
                    plan.cancelled = true;
                    plan.status = ST_CANCELLED;
                }
            }
        }
        _ => {}
    }
    if let Some(revision) = staged_revision {
        routes.revisions.push(revision);
    }
    let preferred_route = runtime
        .plan_index(context.key, context.plan_id)
        .and_then(|index| runtime.plans.get(index))
        .map_or_else(
            || model_pfr_assessment(model, PreferredRouteStatus::NotAssessed, None),
            |plan| plan.preferred_route,
        );
    accepted_reply(message, context.response_number, preferred_route)
}

impl NamFlightRuntime {
    fn plan_index(&self, key: FlightKey, plan_id: [u8; 12]) -> Option<usize> {
        self.plans
            .iter()
            .position(|plan| plan.key == key && plan.plan_id == plan_id)
    }

    fn commit_departure_time(
        &mut self,
        key: FlightKey,
        plan_id: [u8; 12],
        time: Option<model::Hhmm>,
    ) {
        if let (Some(index), Some(time)) = (self.plan_index(key, plan_id), time) {
            if let Some(plan) = self.plans.get_mut(index) {
                plan.departure_time = Some(time);
            }
        }
    }
}

fn valid_context(context: NamFlightContext) -> bool {
    let route_key = context.route_key;
    context.key.kind == KEY_GUFI
        && context.key.gufi != 0
        && context.key.callsign == [0; 8]
        && context.key.dep == [0; 12]
        && context.key.arr == [0; 12]
        && route_key.gufi == context.key.gufi.to_be_bytes()
        && (1..=0x00ff_ffff).contains(&route_key.aircraft_address)
        && route_key.centre == context.centre
        && route_key.plan_id == context.plan_id
        && context.plan_id.iter().any(|byte| *byte != 0)
        && context.centre.iter().all(u8::is_ascii_alphanumeric)
        && matches!(context.plan_status, ST_PROPOSED | ST_ACTIVE)
        && context.source_ns >= 0
        && context.source_ns == context.receipt_ns
        && context.response_number <= 999
}

fn level_feet(level: model::Level) -> Option<i32> {
    match level {
        model::Level::Altitude(value) | model::Level::FlightLevel(value) => {
            i32::from(value).checked_mul(100)
        }
        model::Level::Vfr => None,
        model::Level::MetricLevel(_) | model::Level::MetricAltitude(_) => None,
    }
}

fn ascii_code(value: &[u8; 4]) -> &str {
    // The NAM parser guarantees an ASCII four-byte aerodrome designator.
    core::str::from_utf8(value).unwrap_or_default()
}

fn alternate_airport_ident<'a>(model: &'a Model, filed_code: &str) -> Option<&'a str> {
    let mut alternate = None;
    for row in 0..model.points.kind.len() {
        if *model.points.kind.get(row)? != air_model::point_kind::AIRPORT {
            continue;
        }
        let ident = air_model::str_at(&model.strings, *model.points.ident.get(row)?)?;
        let alt = air_model::str_at(&model.strings, *model.points.alt_ident.get(row)?)?;
        if ident == filed_code || alt == filed_code {
            let candidate = if ident == filed_code { alt } else { ident };
            if candidate.is_empty() || alternate.is_some() {
                return None;
            }
            alternate = Some(candidate);
        }
    }
    alternate
}

fn assess_preferred_route(
    model: &Model,
    departure: &[u8; 4],
    destination: &[u8; 4],
    filed: &oa_airspace::shell::route_parse::ResolvedItem15,
    level_ft: Option<i32>,
    variations: &[MagneticVariationSource],
) -> PreferredRouteAssessment {
    use PreferredRouteStatus as S;
    if model.preferred_route_state != air_model::preferred_route_state::PRESENT {
        return model_pfr_assessment(model, S::SourceUnavailable, None);
    }
    let departure = ascii_code(departure);
    let destination = ascii_code(destination);
    let mut range = air_model::preferred_route_range(model, departure, destination);
    if range.is_empty() {
        let (Some(origin_alias), Some(destination_alias)) = (
            alternate_airport_ident(model, departure),
            alternate_airport_ident(model, destination),
        ) else {
            return model_pfr_assessment(model, S::NoPublishedPair, None);
        };
        range = air_model::preferred_route_range(model, origin_alias, destination_alias);
    }
    if range.is_empty() {
        return model_pfr_assessment(model, S::NoPublishedPair, None);
    }
    // Candidate conversion is bounded by the same fixed point cap as one converted route.
    if range.len() > oa_airspace::core::route::MAX_ROUTE_POINTS {
        return model_pfr_assessment(model, S::Unavailable, None);
    }
    if filed.route.len == 0 {
        return model_pfr_assessment(model, S::Unavailable, None);
    }
    let table = &model.preferred_routes;
    let mut conversion_unavailable = false;
    let mut altitude_indeterminate = false;
    let mut selected: Option<(usize, bool)> = None;
    for row in range {
        let Some(route_text_ix) = table.route_string.get(row).copied() else {
            return model_pfr_assessment(model, S::Unavailable, None);
        };
        let Some(route_text) = air_model::str_at(&model.strings, route_text_ix) else {
            conversion_unavailable = true;
            continue;
        };
        if route_text.is_empty()
            || route_text.len() > oa_airspace::shell::route_parse::MAX_ROUTE_TEXT_BYTES
        {
            conversion_unavailable = true;
            continue;
        }
        let Some(route_type) = table
            .route_type
            .get(row)
            .and_then(|ix| air_model::str_at(&model.strings, *ix))
        else {
            return model_pfr_assessment(model, S::Unavailable, None);
        };
        let (altitude_type_matches, type_needs_coordination) = match route_type {
            "L" | "LSD" | "SLD" => match level_ft {
                Some(level) => (level < 18_000, route_type == "SLD"),
                None => {
                    altitude_indeterminate = true;
                    (true, true)
                }
            },
            "H" | "HSD" | "SHD" => match level_ft {
                Some(level) => (level >= 18_000, route_type == "SHD"),
                None => {
                    altitude_indeterminate = true;
                    (true, true)
                }
            },
            // The source identifies these preferred-route families, but this adapter does not
            // infer their operating envelope from the code or free-text descriptions.
            _ => (true, true),
        };
        if !altitude_type_matches {
            continue;
        }
        let route_elements = route_text
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let candidate = match resolve_icao_item15(
            model,
            &route_elements,
            level_ft,
            departure,
            destination,
            variations,
        ) {
            Ok(candidate) => candidate,
            Err(RouteReject::AltitudeBelowMea) => continue,
            Err(_) => {
                conversion_unavailable = true;
                continue;
            }
        };
        if !same_converted_route_geometry(&filed.route, &candidate.route) {
            continue;
        }
        let unsupported_condition = type_needs_coordination
            || [
                table.area_description.get(row),
                table.altitude_description.get(row),
                table.aircraft_description.get(row),
                table.hours_description.get(row),
                table.direction_description.get(row),
            ]
            .into_iter()
            .flatten()
            .any(|ix| {
                *ix != air_model::StrIx::NONE
                    && air_model::str_at(&model.strings, *ix).is_none_or(|value| !value.is_empty())
            });
        if selected.is_none_or(|(_, old_unknown)| old_unknown && !unsupported_condition) {
            selected = Some((row, unsupported_condition));
        }
    }
    if let Some((row, needs_coordination)) = selected {
        return model_pfr_assessment(
            model,
            if needs_coordination {
                S::CoordinationRequired
            } else {
                S::Preferred
            },
            Some(row),
        );
    }
    model_pfr_assessment(
        model,
        if conversion_unavailable {
            S::Unavailable
        } else if altitude_indeterminate {
            S::CoordinationRequired
        } else {
            S::RouteNotPreferred
        },
        None,
    )
}

/// Compares the resolved navigation path while allowing an airway expansion and an explicit-fix
/// filing to describe the same geometry. The source route has already passed the same resolver;
/// unsupported or ambiguous candidate conversions never reach this comparison.
fn same_converted_route_geometry(
    left: &oa_airspace::core::route::ConvertedRoute,
    right: &oa_airspace::core::route::ConvertedRoute,
) -> bool {
    let len = usize::from(left.len);
    if len == 0 || usize::from(right.len) != len {
        return false;
    }
    left.lat_e7.get(..len) == right.lat_e7.get(..len)
        && left.lon_e7.get(..len) == right.lon_e7.get(..len)
        && left.ident.get(..len) == right.ident.get(..len)
        && left.region.get(..len) == right.region.get(..len)
        && left.scope.get(..len) == right.scope.get(..len)
}

fn accepted_reply(
    message: &Message,
    response_number: u16,
    preferred_route: PreferredRouteAssessment,
) -> NamFlightResult {
    let reply = model::bare(model::Header {
        kind: Kind::Lam,
        id: MsgId {
            sender: message.header.id.receiver,
            receiver: message.header.id.sender,
            number: response_number,
        },
        reference: Some(message.header.id),
    });
    NamFlightResult {
        reply,
        applied: true,
        rejection: None,
        preferred_route,
    }
}

fn nam_route_reject(reason: RouteReject) -> Item15Reject {
    match reason {
        RouteReject::MalformedGroup => Item15Reject::MalformedGroup,
        RouteReject::UnknownPoint => Item15Reject::UnknownPoint,
        RouteReject::AmbiguousPoint => Item15Reject::AmbiguousPoint,
        RouteReject::UnknownAirway => Item15Reject::UnknownAirway,
        RouteReject::AirwayDoesNotContainPoint => Item15Reject::AirwayDoesNotContainPoint,
        RouteReject::Discontinuity => Item15Reject::Discontinuity,
        RouteReject::UnsupportedProcedure => Item15Reject::UnsupportedProcedure,
        RouteReject::MissingVariation => Item15Reject::MissingVariation,
        RouteReject::InvalidVariation => Item15Reject::InvalidVariation,
        RouteReject::MissingRouteLevel => Item15Reject::MissingRouteLevel,
        RouteReject::AltitudeBelowMea => Item15Reject::AltitudeBelowMea,
        RouteReject::Capacity => Item15Reject::Capacity,
        RouteReject::Model => Item15Reject::Model,
    }
}

fn rejected(
    message: &Message,
    response_number: u16,
    profile: Profile,
    reason: NamFlightReject,
) -> NamFlightResult {
    let reply = match reason {
        NamFlightReject::Route(route_reason) => {
            lrm_reply(message, profile, response_number, route_reason)
        }
        _ => catalogue_lrm_reply(message, response_number, MANUAL_COORDINATION_CODE, 0),
    }
    .or_else(|_| catalogue_lrm_reply(message, response_number, MANUAL_COORDINATION_CODE, 0))
    .unwrap_or_else(|_| {
        model::bare(model::Header {
            kind: Kind::Lrm,
            id: MsgId {
                sender: message.header.id.receiver,
                receiver: message.header.id.sender,
                number: response_number.min(999),
            },
            reference: Some(message.header.id),
        })
    });
    NamFlightResult {
        reply,
        applied: false,
        rejection: Some(reason),
        preferred_route: no_pfr_assessment(PreferredRouteStatus::NotAssessed),
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use crate::flight::{flight_driver_len, flight_driver_new, FlightDriverConfig};
    use crate::nam_handoff::{
        nam_session_ingress_local_event, nam_session_ingress_new, nam_session_ingress_receive,
        NamSessionIngress,
    };
    use oa_airspace::core::model::{self as air_model, Meta, StrIx};
    use oa_flight::key_gufi;
    use oa_nam::frame::frame;
    use oa_nam::{
        encode, parse, session, Aerodrome, AmendData, Amendment, Facility, Field13, Field16, Hhmm,
    };

    const LOCAL: Facility = Facility(*b"CZWG");
    const PEER: Facility = Facility(*b"KZMP");

    fn model_fixture() -> Model {
        let meta = Meta {
            nasr_effective: 20261101,
            cifp_cycle: 2611,
            artcc: "TEST".into(),
            buffer_m: 0,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        };
        let mut m = air_model::empty_model(meta);
        for (ident, lat, lon) in [("FIXA", 100, 200), ("FIXB", 200, 300), ("FIXC", 300, 400)] {
            let ix = u32::try_from(m.points.kind.len()).expect("bounded test index");
            let ident_ix = intern(&mut m, ident);
            let region_ix = intern(&mut m, "K1");
            m.points.kind.push(air_model::point_kind::FIX);
            m.points.lat.push(lat);
            m.points.lon.push(lon);
            m.points.elev_ft.push(air_model::ELEV_NONE);
            m.points.ident.push(ident_ix);
            m.points.alt_ident.push(StrIx::NONE);
            m.points.region.push(region_ix);
            m.points.scope.push(StrIx::NONE);
            m.points.name.push(StrIx::NONE);
            assert_eq!(ix, m.points.kind.len() as u32 - 1);
        }
        m
    }

    fn intern(m: &mut Model, value: &str) -> StrIx {
        let raw = u32::try_from(m.strings.offsets.len() - 1).expect("bounded test string index");
        m.strings.text.push_str(value);
        m.strings
            .offsets
            .push(u32::try_from(m.strings.text.len()).expect("bounded test text"));
        StrIx::new(raw)
    }

    fn add_test_airway(model: &mut Model, from: u32, to: u32) {
        let airway = intern(model, "V1");
        let leg_start = u32::try_from(model.airway_legs.from.len()).expect("bounded test index");
        model.airways.designator.push(airway);
        model.airways.location.push(b'V');
        model.airways.legs.push(air_model::Span {
            start: air_model::AirwayLegIx::new(leg_start),
            len: 1,
        });
        model.airway_legs.from.push(air_model::PointIx::new(from));
        model.airway_legs.to.push(air_model::PointIx::new(to));
        model.airway_legs.mea_ft.push(air_model::ALT_NONE);
        model.airway_legs.mea_opp_ft.push(air_model::ALT_NONE);
        model.airway_legs.gps_mea_ft.push(air_model::ALT_NONE);
        model.airway_legs.mea_dir.push(air_model::compass::NONE);
        model.airway_legs.gps_mea_dir.push(air_model::compass::NONE);
        model.airway_legs.gap = air_model::bit_zeros(1);
        model.airway_legs.signal_gap = air_model::bit_zeros(1);
        model.airway_legs.dogleg = air_model::bit_zeros(1);
    }

    fn add_test_airport(model: &mut Model, ident: &str) {
        model.points.kind.push(air_model::point_kind::AIRPORT);
        model.points.lat.push(0);
        model.points.lon.push(0);
        model.points.elev_ft.push(air_model::ELEV_NONE);
        model.points.ident.push(intern(model, ident));
        model.points.alt_ident.push(StrIx::NONE);
        model.points.region.push(StrIx::NONE);
        model.points.scope.push(StrIx::NONE);
        model.points.name.push(StrIx::NONE);
    }

    fn preferred_route(
        model: &mut Model,
        origin: &str,
        destination: &str,
        route_type: &str,
        route_text: &str,
        altitude: Option<&str>,
        aircraft: Option<&str>,
    ) {
        let eff_date = intern(model, "2026/11/01");
        let origin = intern(model, origin);
        let destination = intern(model, destination);
        let route_type = intern(model, route_type);
        let area = StrIx::NONE;
        let altitude = altitude.map_or(StrIx::NONE, |value| intern(model, value));
        let aircraft = aircraft.map_or(StrIx::NONE, |value| intern(model, value));
        let hours = StrIx::NONE;
        let direction = StrIx::NONE;
        let route_text = intern(model, route_text);
        let segment_start = u32::try_from(model.preferred_route_segments.sequence.len())
            .expect("bounded test segment index");
        let table = &mut model.preferred_routes;
        table.eff_date.push(eff_date);
        table.origin.push(origin);
        table.destination.push(destination);
        table.route_type.push(route_type);
        table.route_no.push(1);
        table.area_description.push(area);
        table.altitude_description.push(altitude);
        table.aircraft_description.push(aircraft);
        table.hours_description.push(hours);
        table.direction_description.push(direction);
        table.route_string.push(route_text);
        table.segments.push(air_model::Span {
            start: air_model::PreferredRouteSegmentIx::new(segment_start),
            len: 0,
        });
        model.preferred_route_state = air_model::preferred_route_state::PRESENT;
    }

    fn active_ingress() -> NamSessionIngress {
        let mut ingress = nam_session_ingress_new(session::default_config(LOCAL, PEER));
        let actions = nam_session_ingress_local_event(&mut ingress, session::Event::Activate, 0)
            .expect("activate local session");
        let [session::Action::SendIrq { number }] = actions.as_slice() else {
            panic!("activation requests IRQ");
        };
        let irs = model::bare(model::Header {
            kind: Kind::Irs,
            id: model::MsgId {
                sender: PEER,
                receiver: LOCAL,
                number: 0,
            },
            reference: Some(model::MsgId {
                sender: LOCAL,
                receiver: PEER,
                number: *number,
            }),
        });
        let bytes = frame(&encode(&irs).expect("IRS encodes")).expect("IRS frames");
        let result = nam_session_ingress_receive(&mut ingress, &bytes, 1).expect("IRS admitted");
        assert!(result.accepted.is_none());
        ingress
    }

    fn message(kind: Kind, number: u16) -> Message {
        let mut result = match kind {
            Kind::Fpl => parse(b"(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120F350 CYSC DCT KMPV-KMPV0053-EET/KZBW0003)").expect("FPL fixture parses"),
            Kind::Chg => parse(b"(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA2042-CYOW-RMK/ROUTE UPDATE-15/N0120F350 KSEA DCT CYOW)").expect("CHG fixture parses"),
            Kind::Dla => parse(b"(DLAKZMP/CZWG776KZMP/CZWG603-KLM671-LIRF0900-LYDU-0)").expect("DLA fixture parses"),
            Kind::Cnl => parse(b"(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)").expect("CNL fixture parses"),
            _ => panic!("fixture kind"),
        };
        result.header.kind = kind;
        result.header.id = model::MsgId {
            sender: PEER,
            receiver: LOCAL,
            number,
        };
        match kind {
            Kind::Fpl => {
                result.f13 = Some(Field13 {
                    aerodrome: Aerodrome(*b"KAAA"),
                    time: Some(Hhmm {
                        hour: 12,
                        minute: 0,
                    }),
                });
                result.f16 = Some(Field16 {
                    aerodrome: Aerodrome(*b"KBBB"),
                    eet: Some(Hhmm {
                        hour: 0,
                        minute: 53,
                    }),
                    alternates: Vec::new(),
                });
                let field = result.f15.as_mut().expect("FPL Field 15");
                field.route = vec!["FIXA".into(), "FIXB".into()];
            }
            Kind::Chg => {
                let field = result
                    .f22
                    .iter_mut()
                    .find_map(|amendment| match &mut amendment.data {
                        AmendData::F15(field) => Some(field),
                        _ => None,
                    })
                    .expect("CHG Field 15");
                field.route = vec!["FIXA".into(), "FIXC".into()];
            }
            Kind::Dla => {
                result.f13 = Some(Field13 {
                    aerodrome: Aerodrome(*b"KAAA"),
                    time: Some(Hhmm {
                        hour: 13,
                        minute: 15,
                    }),
                });
                result.f16 = Some(Field16 {
                    aerodrome: Aerodrome(*b"KBBB"),
                    eet: None,
                    alternates: Vec::new(),
                });
            }
            _ => {}
        }
        result
    }

    fn admitted(
        ingress: &mut NamSessionIngress,
        kind: Kind,
        number: u16,
    ) -> SessionAcceptedMessage {
        let encoded = encode(&message(kind, number)).expect("flight-plan message encodes");
        let bytes = frame(&encoded).expect("flight-plan message frames");
        nam_session_ingress_receive(ingress, &bytes, u64::from(number) + 2)
            .expect("NAM session processes frame")
            .accepted
            .expect("session delivers application message")
    }

    #[allow(clippy::too_many_arguments)]
    fn receive(
        runtime: &mut NamFlightRuntime,
        flights: &mut FlightDriver,
        routes: &mut RouteStore,
        ingress: &mut NamSessionIngress,
        kind: Kind,
        number: u16,
        context: NamFlightContext,
        model: &Model,
    ) -> NamFlightResult {
        let encoded = encode(&message(kind, number)).expect("flight-plan message encodes");
        let bytes = frame(&encoded).expect("flight-plan message frames");
        let (session, applied) = nam_flight_receive(
            runtime,
            flights,
            routes,
            ingress,
            &bytes,
            u64::from(number) + 2,
            context,
            model,
            &[],
            Profile::Core,
        )
        .expect("normal NAM ingress path");
        assert!(session.accepted.is_some());
        applied.expect("session token reaches flight application")
    }

    fn context(source_ms: i64, response_number: u16) -> NamFlightContext {
        let gufi = 0x1020_3040_5060_7080_90a0_b0c0_d0e0_f001;
        NamFlightContext {
            key: key_gufi(gufi),
            route_key: oa_legacy_traj::core::trajectory::FlightKey {
                gufi: gufi.to_be_bytes(),
                aircraft_address: 0x123456,
                centre: *b"KZMP",
                plan_id: *b"PLAN00000001",
            },
            plan_id: *b"PLAN00000001",
            centre: *b"KZMP",
            plan_status: ST_PROPOSED,
            source_ns: source_ms * 1_000_000,
            receipt_ns: source_ms * 1_000_000,
            response_number,
        }
    }

    // REQ: OA-NAM-039
    #[test]
    fn positive_key_context_is_required_without_identity_synthesis() {
        let key = FlightKey {
            gufi: 0x1020_3040_5060_7080_90a0_b0c0_d0e0_f001,
            callsign: [0; 8],
            dep: [0; 12],
            arr: [0; 12],
            kind: KEY_GUFI,
        };
        assert!(valid_context(NamFlightContext {
            key,
            route_key: oa_legacy_traj::core::trajectory::FlightKey {
                gufi: key.gufi.to_be_bytes(),
                aircraft_address: 0x123456,
                centre: *b"KZMP",
                plan_id: *b"PLAN00000001",
            },
            plan_id: *b"PLAN00000001",
            centre: *b"KZMP",
            plan_status: ST_PROPOSED,
            source_ns: 1_000_000,
            receipt_ns: 1_000_000,
            response_number: 1,
        }));
        assert!(!valid_context(NamFlightContext {
            key: FlightKey { gufi: 0, ..key },
            route_key: oa_legacy_traj::core::trajectory::FlightKey {
                gufi: [0; 16],
                aircraft_address: 0x123456,
                centre: *b"KZMP",
                plan_id: *b"PLAN00000001",
            },
            plan_id: *b"PLAN00000001",
            centre: *b"KZMP",
            plan_status: ST_PROPOSED,
            source_ns: 1_000_000,
            receipt_ns: 1_000_000,
            response_number: 1,
        }));
    }

    // REQ: OA-NAM-039
    #[test]
    fn admitted_fpl_chg_dla_cnl_apply_to_the_full_key_and_preserve_route_on_delay() {
        let mut ingress = active_ingress();
        let mut runtime = nam_flight_runtime_new(4).expect("bounded NAM plans");
        let mut routes = route::route_store_new([0; 32]);
        let mut flights = flight_driver_new(FlightDriverConfig {
            flight_capacity: 4,
            refusal_log_capacity: 8,
            conflict_log_capacity: 8,
        })
        .expect("bounded flight driver");
        let model = model_fixture();

        let created = receive(
            &mut runtime,
            &mut flights,
            &mut routes,
            &mut ingress,
            Kind::Fpl,
            1,
            context(1, 100),
            &model,
        );
        assert!(created.applied);
        assert_eq!(created.reply.header.kind, Kind::Lam);
        assert_eq!(flight_driver_len(&flights), 1);
        let index = runtime
            .plan_index(context(1, 100).key, context(1, 100).plan_id)
            .expect("FPL stored");
        assert_eq!(routes.revisions.len(), 1);
        assert_eq!(routes.revisions[0].converted.len, 2);
        assert_eq!(
            routes.revisions[0]
                .converted
                .ident
                .get(1)
                .and_then(|label| label.bytes.get(..4)),
            Some(&b"FIXB"[..])
        );

        let changed = receive(
            &mut runtime,
            &mut flights,
            &mut routes,
            &mut ingress,
            Kind::Chg,
            2,
            context(2, 101),
            &model,
        );
        assert!(changed.applied);
        assert_eq!(routes.revisions.len(), 2);
        assert_eq!(
            routes.revisions[1]
                .converted
                .ident
                .get(1)
                .and_then(|label| label.bytes.get(..4)),
            Some(&b"FIXC"[..])
        );

        let delayed = receive(
            &mut runtime,
            &mut flights,
            &mut routes,
            &mut ingress,
            Kind::Dla,
            3,
            context(3, 102),
            &model,
        );
        assert!(delayed.applied);
        let plan = runtime
            .plans
            .get(index)
            .expect("delayed plan remains stored");
        assert_eq!(
            plan.departure_time,
            Some(Hhmm {
                hour: 13,
                minute: 15
            })
        );
        assert_eq!(routes.revisions.len(), 2);
        assert_eq!(
            routes.revisions[1]
                .converted
                .ident
                .get(1)
                .and_then(|label| label.bytes.get(..4)),
            Some(&b"FIXC"[..])
        );

        let cancelled = receive(
            &mut runtime,
            &mut flights,
            &mut routes,
            &mut ingress,
            Kind::Cnl,
            4,
            context(4, 103),
            &model,
        );
        assert!(cancelled.applied);
        assert_eq!(
            runtime.plans.get(index).map(|plan| plan.cancelled),
            Some(true)
        );
        assert_eq!(cancelled.reply.header.kind, Kind::Lam);
    }

    // REQ: OA-NAM-039
    #[test]
    fn preferred_route_advice_is_attached_to_the_normal_admitted_fpl_path() {
        let mut ingress = active_ingress();
        let mut runtime = nam_flight_runtime_new(4).expect("bounded NAM plans");
        let mut routes = route::route_store_new([0; 32]);
        let mut flights = flight_driver_new(FlightDriverConfig {
            flight_capacity: 4,
            refusal_log_capacity: 8,
            conflict_log_capacity: 8,
        })
        .expect("bounded flight driver");
        let mut model = model_fixture();
        add_test_airway(&mut model, 0, 1);
        preferred_route(&mut model, "KAAA", "KBBB", "H", "FIXA V1 FIXB", None, None);

        let result = receive(
            &mut runtime,
            &mut flights,
            &mut routes,
            &mut ingress,
            Kind::Fpl,
            1,
            context(1, 100),
            &model,
        );
        assert!(result.applied);
        assert_eq!(result.reply.header.kind, Kind::Lam);
        assert_eq!(
            result.preferred_route.status,
            PreferredRouteStatus::Preferred
        );
        assert_eq!(result.preferred_route.route_row, 0);
        assert_eq!(runtime.plans[0].preferred_route, result.preferred_route);
        assert_eq!(routes.revisions.len(), 1);
    }

    // REQ: OA-NAM-039
    #[test]
    fn pfr_type_direction_and_free_text_are_advisory_and_source_bound() {
        let mut model = model_fixture();
        preferred_route(&mut model, "KAAA", "KBBB", "HSD", "FIXA FIXB", None, None);
        let elements = vec!["FIXA".to_string(), "FIXB".to_string()];
        let filed = resolve_icao_item15(&model, &elements, Some(35_000), "KAAA", "KBBB", &[])
            .expect("accepted route geometry");
        assert_eq!(
            assess_preferred_route(&model, b"KAAA", b"KBBB", &filed, Some(35_000), &[]).status,
            PreferredRouteStatus::Preferred
        );
        assert_eq!(
            assess_preferred_route(&model, b"KBBB", b"KAAA", &filed, Some(35_000), &[]).status,
            PreferredRouteStatus::NoPublishedPair
        );
        assert_eq!(
            assess_preferred_route(&model, b"KAAA", b"KBBB", &filed, Some(10_000), &[]).status,
            PreferredRouteStatus::RouteNotPreferred
        );
        assert_eq!(
            assess_preferred_route(&model, b"KAAA", b"KBBB", &filed, None, &[]).status,
            PreferredRouteStatus::CoordinationRequired
        );

        let mut special_low = model_fixture();
        preferred_route(
            &mut special_low,
            "KAAA",
            "KBBB",
            "SLD",
            "FIXA FIXB",
            None,
            None,
        );
        let filed = resolve_icao_item15(&special_low, &elements, Some(10_000), "KAAA", "KBBB", &[])
            .expect("accepted route geometry");
        assert_eq!(
            assess_preferred_route(&special_low, b"KAAA", b"KBBB", &filed, Some(10_000), &[])
                .status,
            PreferredRouteStatus::CoordinationRequired
        );
        assert_eq!(
            assess_preferred_route(&special_low, b"KAAA", b"KBBB", &filed, Some(35_000), &[])
                .status,
            PreferredRouteStatus::RouteNotPreferred
        );

        let mut constrained = model_fixture();
        preferred_route(
            &mut constrained,
            "KAAA",
            "KBBB",
            "HSD",
            "FIXA FIXB",
            Some("FL 240 AND ABOVE"),
            Some("2J/H"),
        );
        let filed = resolve_icao_item15(&constrained, &elements, Some(35_000), "KAAA", "KBBB", &[])
            .expect("accepted route geometry");
        assert_eq!(
            assess_preferred_route(&constrained, b"KAAA", b"KBBB", &filed, Some(35_000), &[],)
                .status,
            PreferredRouteStatus::CoordinationRequired
        );

        let mut directional = model_fixture();
        preferred_route(
            &mut directional,
            "KAAA",
            "KBBB",
            "H",
            "FIXA FIXB",
            None,
            None,
        );
        let direction_ix = intern(&mut directional, "NORTHBOUND ONLY");
        directional.preferred_routes.direction_description[0] = direction_ix;
        let filed = resolve_icao_item15(&directional, &elements, Some(35_000), "KAAA", "KBBB", &[])
            .expect("accepted route geometry");
        assert_eq!(
            assess_preferred_route(&directional, b"KAAA", b"KBBB", &filed, Some(35_000), &[],)
                .status,
            PreferredRouteStatus::CoordinationRequired
        );

        let mut terminal = model_fixture();
        preferred_route(
            &mut terminal,
            "KAAA",
            "KBBB",
            "TEC",
            "FIXA FIXB",
            None,
            None,
        );
        let filed = resolve_icao_item15(&terminal, &elements, Some(35_000), "KAAA", "KBBB", &[])
            .expect("accepted route geometry");
        assert_eq!(
            assess_preferred_route(&terminal, b"KAAA", b"KBBB", &filed, Some(35_000), &[],).status,
            PreferredRouteStatus::CoordinationRequired
        );

        constrained.preferred_route_state = air_model::preferred_route_state::UNAVAILABLE;
        assert_eq!(
            assess_preferred_route(&constrained, b"KAAA", b"KBBB", &filed, Some(35_000), &[],)
                .status,
            PreferredRouteStatus::SourceUnavailable
        );
    }

    // REQ: OA-NAM-039
    #[test]
    fn iata_only_airport_aliases_cannot_bind_a_pfr_pair() {
        let mut model = model_fixture();
        add_test_airport(&mut model, "KAAA");
        add_test_airport(&mut model, "KBBB");
        preferred_route(&mut model, "ORD", "JFK", "H", "FIXA FIXB", None, None);
        let elements = vec!["FIXA".to_string(), "FIXB".to_string()];
        let filed = resolve_icao_item15(&model, &elements, Some(35_000), "KAAA", "KBBB", &[])
            .expect("accepted route geometry");

        assert_eq!(
            assess_preferred_route(&model, b"KAAA", b"KBBB", &filed, Some(35_000), &[]).status,
            PreferredRouteStatus::NoPublishedPair
        );
    }

    // REQ: OA-NAM-039
    #[test]
    fn duplicate_gap_unknown_route_and_catalogue_refusals_do_not_apply() {
        let mut ingress = active_ingress();
        let mut runtime = nam_flight_runtime_new(2).expect("bounded NAM plans");
        let mut routes = route::route_store_new([0; 32]);
        let mut flights = flight_driver_new(FlightDriverConfig {
            flight_capacity: 2,
            refusal_log_capacity: 4,
            conflict_log_capacity: 4,
        })
        .expect("bounded flight driver");
        let model = model_fixture();
        let first = admitted(&mut ingress, Kind::Fpl, 1);
        let mut bad = message(Kind::Fpl, 1);
        bad.f15.as_mut().expect("Field 15").route = vec!["FIXA".into(), "MISSING".into()];
        // Admit a valid frame carrying a semantically unknown route through the actual session.
        let bad_bytes =
            frame(&encode(&bad).expect("unknown route message encodes")).expect("frames");
        let duplicate_token = nam_session_ingress_receive(&mut ingress, &bad_bytes, 3)
            .expect("session classifies duplicate")
            .accepted;
        assert!(duplicate_token.is_some());
        let rejected = nam_flight_apply(
            &mut runtime,
            &mut flights,
            &mut routes,
            &first,
            context(1, 100),
            &model,
            &[],
            Profile::Core,
        );
        assert!(rejected.applied);
        let replay = nam_flight_apply(
            &mut runtime,
            &mut flights,
            &mut routes,
            &first,
            context(1, 100),
            &model,
            &[],
            Profile::Core,
        );
        assert!(!replay.applied);
        assert_eq!(replay.reply.header.kind, Kind::Lrm);
        assert_eq!(flight_driver_len(&flights), 1);
        let duplicate = nam_flight_apply(
            &mut runtime,
            &mut flights,
            &mut routes,
            &duplicate_token.expect("duplicate delivery remains classified"),
            context(1, 100),
            &model,
            &[],
            Profile::Core,
        );
        assert!(!duplicate.applied);
        assert_eq!(duplicate.reply.header.kind, Kind::Lrm);
        assert_eq!(flight_driver_len(&flights), 1);

        // The next delivered number has a gap. The session token is visible, but application is
        // refused so the later plan cannot double-apply an out-of-order update.
        let gap = admitted(&mut ingress, Kind::Chg, 3);
        let gap_result = nam_flight_apply(
            &mut runtime,
            &mut flights,
            &mut routes,
            &gap,
            context(3, 101),
            &model,
            &[],
            Profile::Core,
        );
        assert!(!gap_result.applied);
        assert_eq!(gap_result.reply.header.kind, Kind::Lrm);
        assert_eq!(flight_driver_len(&flights), 1);

        // A separate in-order path verifies unknown and duplicate route amendments against the
        // stable catalog, without attempting to recover from the deliberately refused gap above.
        let mut catalog_ingress = active_ingress();
        let mut catalog_runtime = nam_flight_runtime_new(2).expect("bounded plan store");
        let mut catalog_routes = route::route_store_new([0; 32]);
        let mut catalog_flights = flight_driver_new(FlightDriverConfig {
            flight_capacity: 2,
            refusal_log_capacity: 4,
            conflict_log_capacity: 4,
        })
        .expect("bounded flight driver");
        let created = receive(
            &mut catalog_runtime,
            &mut catalog_flights,
            &mut catalog_routes,
            &mut catalog_ingress,
            Kind::Fpl,
            1,
            context(1, 100),
            &model,
        );
        assert!(created.applied);

        let mut next = message(Kind::Chg, 2);
        let field = next
            .f22
            .iter_mut()
            .find_map(|amendment| match &mut amendment.data {
                AmendData::F15(field) => Some(field),
                _ => None,
            })
            .expect("CHG Field 15");
        field.route = vec!["FIXA".into(), "MISSING".into()];
        let bytes =
            frame(&encode(&next).expect("unknown route amendment encodes")).expect("frames");
        let (session, response) = nam_flight_receive(
            &mut catalog_runtime,
            &mut catalog_flights,
            &mut catalog_routes,
            &mut catalog_ingress,
            &bytes,
            5,
            context(2, 101),
            &model,
            &[],
            Profile::Core,
        )
        .expect("in-order session message");
        let route_refusal = response.expect("delivered unknown route");
        assert!(session.accepted.is_some());
        assert!(!route_refusal.applied);
        assert_eq!(route_refusal.reply.header.kind, Kind::Lrm);
        assert_eq!(flight_driver_len(&catalog_flights), 1);
        let expected_code = Item15Reject::UnknownPoint.lrm(Profile::Core, Kind::Chg).0;
        let lrm_text = match route_refusal.reply.f18.as_ref() {
            Some(model::Field18::Items(items)) => items.first().map(|item| item.text.as_str()),
            _ => None,
        }
        .expect("catalogued route refusal carries RMK");
        assert_eq!(
            oa_nam::field::parse_lrm_text(lrm_text)
                .expect("stable LRM text")
                .code,
            expected_code
        );

        let mut duplicate = message(Kind::Chg, 3);
        let original = duplicate
            .f22
            .iter()
            .find_map(|amendment| match &amendment.data {
                AmendData::F15(field) => Some(field.clone()),
                _ => None,
            })
            .expect("CHG Field 15");
        duplicate.f22.push(Amendment {
            padded: false,
            data: AmendData::F15(original),
        });
        assert_eq!(
            filed_change(&duplicate),
            Err(Item15Reject::DuplicateItem15Amendment)
        );
        let duplicate_response = lrm_reply(
            &duplicate,
            Profile::Core,
            102,
            Item15Reject::DuplicateItem15Amendment,
        )
        .expect("duplicate amendment has a catalogued LRM");
        assert_eq!(duplicate_response.header.kind, Kind::Lrm);
        assert_eq!(flight_driver_len(&catalog_flights), 1);
    }
}

//! The schema the decoder knows, as static tables: which element and attribute names, in which
//! namespace, hold which [`Field`]. Plain data. Core (pure).
//!
//! A table entry is an element ([`El`]) with the attributes it may carry ([`At`]) and the
//! children it may hold. An element or attribute with a field is stored; one without is known
//! and deliberately not stored (counted as ignored); one that is not in the table is unknown
//! (counted as unknown). An `ignore` element is never entered: its whole subtree counts once as
//! ignored. The tables are built from docs/TFMS_SURVEY.md and cover every path in the sample.

use crate::fields::Field;

/// The namespaces of the sampled feed, by the part after `urn:us:gov:dot:faa:atm:tfm:`.
/// Elements are matched by namespace URI, never by prefix: the sampled messages use `ns5:`,
/// `ds:`, default-namespace and other prefixings of the same URIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nsx {
    /// `tfmdataservice`: the root and its outputs.
    Svc,
    /// `flightdata`: the flight data message element and its body.
    Fd,
    /// `flowinformation`: the flow information message element and its body.
    Fi,
    /// `flightdatacommonmessages`: the children of a flight data body.
    Fcm,
    /// `tfmdatacoreelements`: identifiers, airports, positions, altitudes.
    Core,
    /// `ficommonmessages`: the children of a flow information body.
    Fic,
    /// `ficommonmessages2`: the TMI flight list entries.
    Fic2,
    /// `ficommondatatypes`: the period of an advisory.
    Fidt,
}

/// The full namespace URI of `n`.
pub fn ns_uri(n: Nsx) -> &'static [u8] {
    match n {
        Nsx::Svc => b"urn:us:gov:dot:faa:atm:tfm:tfmdataservice",
        Nsx::Fd => b"urn:us:gov:dot:faa:atm:tfm:flightdata",
        Nsx::Fi => b"urn:us:gov:dot:faa:atm:tfm:flowinformation",
        Nsx::Fcm => b"urn:us:gov:dot:faa:atm:tfm:flightdatacommonmessages",
        Nsx::Core => b"urn:us:gov:dot:faa:atm:tfm:tfmdatacoreelements",
        Nsx::Fic => b"urn:us:gov:dot:faa:atm:tfm:ficommonmessages",
        Nsx::Fic2 => b"urn:us:gov:dot:faa:atm:tfm:ficommonmessages2",
        Nsx::Fidt => b"urn:us:gov:dot:faa:atm:tfm:ficommondatatypes",
    }
}

/// A known attribute: its name (unprefixed) and where its value goes.
// A7.2: cold struct, size guard waived (static schema data)
#[derive(Clone, Copy, Debug)]
pub struct At {
    pub name: &'static [u8],
    pub f: Option<Field>,
}

/// A known element.
// A7.2: cold struct, size guard waived (static schema data)
#[derive(Clone, Copy, Debug)]
pub struct El {
    pub ns: Nsx,
    pub name: &'static [u8],
    /// Where the element's text goes; `None` stores no text.
    pub text: Option<Field>,
    /// A counter bumped when the element is met (the `fxaFlight` count).
    pub enter: Option<Field>,
    pub attrs: &'static [At],
    pub kids: &'static [El],
    /// Known and not stored: never entered, counted once as ignored.
    pub ignore: bool,
}

const fn at(name: &'static [u8], f: Field) -> At {
    At { name, f: Some(f) }
}

/// A known attribute that is not stored.
const fn at_skip(name: &'static [u8]) -> At {
    At { name, f: None }
}

const fn el(
    ns: Nsx,
    name: &'static [u8],
    text: Option<Field>,
    attrs: &'static [At],
    kids: &'static [El],
) -> El {
    El {
        ns,
        name,
        text,
        enter: None,
        attrs,
        kids,
        ignore: false,
    }
}

/// An element holding a value.
const fn t(ns: Nsx, name: &'static [u8], f: Field) -> El {
    el(ns, name, Some(f), &[], &[])
}

/// An element holding a value and attributes.
const fn ta(ns: Nsx, name: &'static [u8], f: Field, attrs: &'static [At]) -> El {
    el(ns, name, Some(f), attrs, &[])
}

/// An element holding only attributes.
const fn a(ns: Nsx, name: &'static [u8], attrs: &'static [At]) -> El {
    el(ns, name, None, attrs, &[])
}

/// A container.
const fn n(ns: Nsx, name: &'static [u8], kids: &'static [El]) -> El {
    el(ns, name, None, &[], kids)
}

/// An element known and not stored: its subtree is skipped.
const fn skip(ns: Nsx, name: &'static [u8]) -> El {
    El {
        ns,
        name,
        text: None,
        enter: None,
        attrs: &[],
        kids: &[],
        ignore: true,
    }
}

/// An ignored element that also counts a field each time it is met.
const fn skip_count(ns: Nsx, name: &'static [u8], f: Field) -> El {
    El {
        ns,
        name,
        text: None,
        enter: Some(f),
        attrs: &[],
        kids: &[],
        ignore: true,
    }
}

use Nsx::{Core, Fcm, Fd, Fi, Fic, Fic2, Fidt};

/// The root of a walk that has no element of its own (a placeholder for empty stack slots).
pub static EMPTY: El = skip(Nsx::Svc, b"");

// ---- the message elements ------------------------------------------------------------------

/// Attributes of `fltdMessage`.
pub static FLTD_ATTRS: &[At] = &[
    at(b"acid", Field::Acid),
    at(b"airline", Field::Airline),
    at(b"arrArpt", Field::Arr),
    at(b"cdmPart", Field::CdmPart),
    at(b"depArpt", Field::Dep),
    at(b"fdTrigger", Field::Trigger),
    at(b"flightRef", Field::FlightRef),
    at(b"major", Field::Major),
    at(b"msgType", Field::MsgType),
    at_skip(b"sensitivity"),
    at(b"sourceFacility", Field::SrcFac),
    at(b"sourceTimeStamp", Field::SrcTime),
];

/// Attributes of `fiMessage`.
pub static FI_ATTRS: &[At] = &[
    at_skip(b"sensitivity"),
    at_skip(b"visDomain"),
    at_skip(b"destinationCodes"),
    at_skip(b"refresh"),
    at(b"sourceFacility", Field::SrcFac),
    at(b"sourceTimeStamp", Field::SrcTime),
    at(b"msgType", Field::MsgType),
];

// ---- shared pieces of flight data bodies ---------------------------------------------------

const LAT_ATTRS: &[At] = &[
    at(b"degrees", Field::LatDeg),
    at(b"minutes", Field::LatMin),
    at(b"seconds", Field::LatSec),
    at(b"direction", Field::LatDir),
];
const LON_ATTRS: &[At] = &[
    at(b"degrees", Field::LonDeg),
    at(b"minutes", Field::LonMin),
    at(b"seconds", Field::LonSec),
    at(b"direction", Field::LonDir),
];

const LAT_KIDS: &[El] = &[a(Core, b"latitudeDMS", LAT_ATTRS)];
const LON_KIDS: &[El] = &[a(Core, b"longitudeDMS", LON_ATTRS)];
const POSITION_KIDS: &[El] = &[
    n(Core, b"latitude", LAT_KIDS),
    n(Core, b"longitude", LON_KIDS),
];

const QAID_ATTRS: &[At] = &[
    at(b"aircraftCategory", Field::AcCat),
    at(b"userCategory", Field::UserCat),
];
const COMPUTER_ID_KIDS: &[El] = &[
    t(Core, b"facilityIdentifier", Field::CidFac),
    t(Core, b"idNumber", Field::CidNum),
];
const DEP_KIDS: &[El] = &[t(Core, b"airport", Field::Dep), skip(Core, b"fix")];
const ARR_KIDS: &[El] = &[t(Core, b"airport", Field::Arr), skip(Core, b"fix")];
const QAID_KIDS: &[El] = &[
    t(Core, b"aircraftId", Field::Acid),
    n(Core, b"computerId", COMPUTER_ID_KIDS),
    t(Core, b"gufi", Field::Gufi),
    t(Core, b"igtd", Field::Igtd),
    n(Core, b"departurePoint", DEP_KIDS),
    n(Core, b"arrivalPoint", ARR_KIDS),
];
const QAID: El = el(Fcm, b"qualifiedAircraftId", None, QAID_ATTRS, QAID_KIDS);

const ETA: El = a(
    Fcm,
    b"eta",
    &[at(b"etaType", Field::EtaType), at(b"timeValue", Field::Eta)],
);
const ETD: El = a(
    Fcm,
    b"etd",
    &[at(b"etdType", Field::EtdType), at(b"timeValue", Field::Etd)],
);
const ARR_FIX: El = a(
    Fcm,
    b"arrivalFixAndTime",
    &[
        at(b"fixName", Field::ArrFix),
        at(b"arrTime", Field::ArrFixTime),
    ],
);
const DEP_FIX: El = a(
    Fcm,
    b"departureFixAndTime",
    &[
        at(b"fixName", Field::DepFix),
        at(b"arrTime", Field::DepFixTime),
    ],
);
const RVSM: El = a(
    Fcm,
    b"rvsmData",
    &[
        at(b"equipped", Field::RvsmEq),
        at(b"currentCompliance", Field::RvsmCur),
        at(b"futureCompliance", Field::RvsmFut),
    ],
);
const DIVERSION: El = t(Fcm, b"diversionIndicator", Field::Diversion);
const REMARKS: El = t(Fcm, b"remarksKeywords", Field::Remarks);

const SPEC_ATTRS: &[At] = &[
    at(b"aircraftEngineClass", Field::EngClass),
    at(b"equipmentQualifier", Field::Equip),
    at_skip(b"numberOfAircraft"),
    at(b"specialAircraftQualifier", Field::Special),
];
const STATUS_AND_SPEC_KIDS: &[El] = &[
    t(Fcm, b"aircraftModel", Field::AcType),
    // The text repeats `aircraftModel` (1,302 of 1,302 in the sample); only its attributes are
    // stored.
    a(Fcm, b"aircraftSpecification", SPEC_ATTRS),
    t(Fcm, b"flightStatus", Field::Status),
];
const STATUS_AND_SPEC: El = n(Fcm, b"flightStatusAndSpec", STATUS_AND_SPEC_KIDS);

const SPECS_ATTRS: &[At] = &[
    at(b"equipmentQualifier", Field::Equip),
    at(b"specialAircraftQualifier", Field::Special),
    at_skip(b"numberOfAircraft"),
];
const FLIGHT_AIRCRAFT_SPECS: El = ta(Fcm, b"flightAircraftSpecs", Field::AcType, SPECS_ATTRS);

const FLIGHT_TIME_ATTRS: &[At] = &[
    at(b"airlineInTime", Field::InT),
    at(b"airlineOffTime", Field::OffT),
    at(b"airlineOnTime", Field::OnT),
    at(b"airlineOutTime", Field::OutT),
    at(b"flightCreation", Field::Created),
    at(b"gateArrival", Field::GateArr),
    at(b"gateDeparture", Field::GateDep),
    at(b"originalArrival", Field::OrigArr),
    at(b"originalDeparture", Field::OrigDep),
    at(b"runwayArrival", Field::RwyArr),
    at(b"runwayDeparture", Field::RwyDep),
];

const ROUTE_KIDS: &[El] = &[
    ARR_FIX,
    DEP_FIX,
    DIVERSION,
    a(
        Fcm,
        b"dp",
        &[at(b"routeName", Field::Dp), at_skip(b"routeType")],
    ),
    skip(Fcm, b"dpTransitionFix"),
    ETA,
    ETD,
    skip(Fcm, b"flightTraversalData2"),
    a(
        Fcm,
        b"nextPosition",
        &[
            at(b"latitudeDecimal", Field::NextLat),
            at(b"longitudeDecimal", Field::NextLon),
        ],
    ),
    REMARKS,
    t(Fcm, b"routeOfFlight", Field::Route),
    RVSM,
    a(
        Fcm,
        b"star",
        &[at(b"routeName", Field::Star), at_skip(b"routeType")],
    ),
    skip(Fcm, b"starTransitionFix"),
];
const NCSM_ROUTE: El = n(Fcm, b"ncsmRouteData", ROUTE_KIDS);

const TRACK_KIDS: &[El] = &[
    ARR_FIX,
    DEP_FIX,
    ETA,
    a(
        Fcm,
        b"nextEvent",
        &[
            at(b"latitudeDecimal", Field::NextLat),
            at(b"longitudeDecimal", Field::NextLon),
        ],
    ),
    RVSM,
];
const NCSM_TRACK: El = n(Fcm, b"ncsmTrackData", TRACK_KIDS);

const POSITION: El = n(Fcm, b"position", POSITION_KIDS);

/// `altitude` with an assigned and a requested value (flight plans, activations, routes).
const ASSIGNED_REQUESTED_KIDS: &[El] = &[
    n(
        Core,
        b"assignedAltitude",
        &[t(Core, b"simpleAltitude", Field::AsgAlt)],
    ),
    n(
        Core,
        b"requestedAltitude",
        &[t(Core, b"simpleAltitude", Field::ReqAlt)],
    ),
];
const ALTITUDE: El = n(Fcm, b"altitude", ASSIGNED_REQUESTED_KIDS);
const NEW_ALTITUDE: El = n(Fcm, b"newAltitude", ASSIGNED_REQUESTED_KIDS);

const SPEED_KIDS: &[El] = &[
    t(Core, b"filedTrueAirSpeed", Field::Tas),
    skip(Core, b"groundSpeed"),
    skip(Core, b"mach"),
];
const SPEED_FILED: El = n(Fcm, b"speed", SPEED_KIDS);
const NEW_SPEED: El = n(Fcm, b"newSpeed", SPEED_KIDS);

const COORD_POINT_KIDS: &[El] = &[
    t(Core, b"namedFix", Field::CoordFix),
    skip(Core, b"fixRadialDistance"),
    skip(Core, b"latLong"),
];
const COORD_TYPE: &[At] = &[at(b"type", Field::CoordType)];

const AIRLINE_KIDS: &[El] = &[
    ARR_FIX,
    DEP_FIX,
    DIVERSION,
    ETA,
    ETD,
    STATUS_AND_SPEC,
    a(Fcm, b"flightTimeData", FLIGHT_TIME_ATTRS),
    REMARKS,
    RVSM,
];
const AIRLINE_DATA: El = n(Fcm, b"airlineData", AIRLINE_KIDS);

// ---- the flight data bodies ----------------------------------------------------------------

const TRACK_BODY: &[El] = &[
    QAID,
    t(Fcm, b"speed", Field::Speed),
    n(
        Fcm,
        b"reportedAltitude",
        &[n(
            Core,
            b"assignedAltitude",
            &[t(Core, b"simpleAltitude", Field::Alt)],
        )],
    ),
    POSITION,
    t(Fcm, b"timeAtPosition", Field::PosTime),
    NCSM_ROUTE,
    NCSM_TRACK,
];

const PLAN_BODY: &[El] = &[
    ALTITUDE,
    n(Fcm, b"coordinationPoint", COORD_POINT_KIDS),
    ta(Fcm, b"coordinationTime", Field::Coord, COORD_TYPE),
    FLIGHT_AIRCRAFT_SPECS,
    NCSM_ROUTE,
    QAID,
    a(
        Fcm,
        b"routeOfFlight",
        &[at(b"legacyFormat", Field::RouteLegacy)],
    ),
    SPEED_FILED,
];

const AMENDMENT_KIDS: &[El] = &[
    NEW_ALTITUDE,
    n(Fcm, b"newCoordinationPoint", COORD_POINT_KIDS),
    ta(Fcm, b"newCoordinationTime", Field::Coord, COORD_TYPE),
    ta(Fcm, b"newFlightAircraftSpecs", Field::AcType, SPECS_ATTRS),
    a(
        Fcm,
        b"newRouteOfFlight",
        &[at(b"legacyFormat", Field::RouteLegacy)],
    ),
    NEW_SPEED,
];
const AMENDMENT_BODY: &[El] = &[n(Fcm, b"amendmentData", AMENDMENT_KIDS), NCSM_ROUTE, QAID];

const TIME_DATA_KIDS: &[El] = &[ETA, ETD, RVSM];
const DEPARTURE_BODY: &[El] = &[
    FLIGHT_AIRCRAFT_SPECS,
    n(Fcm, b"ncsmFlightTimeData", TIME_DATA_KIDS),
    QAID,
    ta(
        Fcm,
        b"timeOfArrival",
        Field::Toa,
        &[at(b"estimated", Field::ToaEst)],
    ),
    ta(
        Fcm,
        b"timeOfDeparture",
        Field::Tod,
        &[at(b"estimated", Field::TodEst)],
    ),
];
const ARRIVAL_BODY: &[El] = &[
    n(Fcm, b"ncsmFlightTimeData", TIME_DATA_KIDS),
    QAID,
    ta(
        Fcm,
        b"timeOfArrival",
        Field::Toa,
        &[at(b"estimated", Field::ToaEst)],
    ),
];

const CANCELLATION_BODY: &[El] = &[QAID];

const BOUNDARY_BODY: &[El] = &[
    el(
        Fcm,
        b"boundaryPosition",
        None,
        &[at(b"boundaryCrossingTime", Field::Bnd)],
        &[skip(Core, b"fixRadialDistance"), skip(Core, b"latLong")],
    ),
    FLIGHT_AIRCRAFT_SPECS,
    NCSM_ROUTE,
    QAID,
    n(
        Fcm,
        b"reportedAltitude",
        &[n(
            Core,
            b"assignedAltitude",
            &[t(Core, b"simpleAltitude", Field::Alt)],
        )],
    ),
    a(
        Fcm,
        b"routeOfFlight",
        &[at(b"legacyFormat", Field::RouteLegacy)],
    ),
    SPEED_FILED,
];

const OCEANIC_BODY: &[El] = &[
    NCSM_ROUTE,
    NCSM_TRACK,
    skip(Fcm, b"plannedPositionData"),
    QAID,
    n(
        Fcm,
        b"reportedPositionData",
        &[
            t(Core, b"altitude", Field::Alt),
            n(Core, b"position", POSITION_KIDS),
            t(Core, b"time", Field::PosTime),
        ],
    ),
    t(Fcm, b"speed", Field::Speed),
];

const CREATE_BODY: &[El] = &[AIRLINE_DATA, skip(Fcm, b"diversionCancelData"), QAID];
const MODIFY_BODY: &[El] = &[AIRLINE_DATA, QAID];

const TIMES_BODY: &[El] = &[ARR_FIX, DEP_FIX, ETA, ETD, STATUS_AND_SPEC, QAID, RVSM];

const ROUTE_BODY: &[El] = &[ALTITUDE, STATUS_AND_SPEC, NCSM_ROUTE, QAID, SPEED_FILED];

const SECTORS_BODY: &[El] = &[skip(Fcm, b"flightTraversalData2"), QAID];

const ACTIVATE_BODY: &[El] = &[ALTITUDE, STATUS_AND_SPEC, NCSM_ROUTE, QAID, SPEED_FILED];

/// A flight data message type: its `msgType`, the code in [`crate::rows::kind`] and the body
/// element with its children.
// A7.2: cold struct, size guard waived (static schema data)
#[derive(Clone, Copy, Debug)]
pub struct Body {
    pub msg_type: &'static [u8],
    pub kind: u8,
    pub el: El,
}

use crate::rows::kind as k;

const fn body(msg_type: &'static [u8], kind: u8, name: &'static [u8], kids: &'static [El]) -> Body {
    Body {
        msg_type,
        kind,
        el: n(Fd, name, kids),
    }
}

/// Every flight data message type of the sample.
pub static BODIES: [Body; 14] = [
    body(
        b"trackInformation",
        k::TRACK,
        b"trackInformation",
        TRACK_BODY,
    ),
    body(
        b"flightPlanInformation",
        k::FLIGHT_PLAN,
        b"flightPlanInformation",
        PLAN_BODY,
    ),
    body(
        b"flightPlanAmendmentInformation",
        k::AMENDMENT,
        b"flightPlanAmendmentInformation",
        AMENDMENT_BODY,
    ),
    body(
        b"departureInformation",
        k::DEPARTURE,
        b"departureInformation",
        DEPARTURE_BODY,
    ),
    body(
        b"arrivalInformation",
        k::ARRIVAL,
        b"arrivalInformation",
        ARRIVAL_BODY,
    ),
    body(
        b"flightPlanCancellation",
        k::CANCELLATION,
        b"flightPlanCancellation",
        CANCELLATION_BODY,
    ),
    body(
        b"boundaryCrossingUpdate",
        k::BOUNDARY,
        b"boundaryCrossingUpdate",
        BOUNDARY_BODY,
    ),
    body(b"oceanicReport", k::OCEANIC, b"oceanicReport", OCEANIC_BODY),
    body(b"FlightCreate", k::CREATE, b"ncsmFlightCreate", CREATE_BODY),
    body(b"FlightModify", k::MODIFY, b"ncsmFlightModify", MODIFY_BODY),
    body(b"FlightTimes", k::TIMES, b"ncsmFlightTimes", TIMES_BODY),
    body(b"FlightRoute", k::ROUTE, b"ncsmFlightRoute", ROUTE_BODY),
    body(
        b"FlightSectors",
        k::SECTORS,
        b"ncsmFlightSectors",
        SECTORS_BODY,
    ),
    body(
        b"FlightScheduleActivate",
        k::ACTIVATE,
        b"ncsmFlightScheduleActivate",
        ACTIVATE_BODY,
    ),
];

// ---- flow information ----------------------------------------------------------------------

/// A flow information message type that becomes a flow row: its `msgType`, the code in
/// [`crate::rows::fkind`] and the body element with its children.
// A7.2: cold struct, size guard waived (static schema data)
#[derive(Clone, Copy, Debug)]
pub struct FlowBody {
    pub msg_type: &'static [u8],
    pub kind: u8,
    pub el: El,
}

const RSTR_KIDS: &[El] = &[
    t(Fic, b"action", Field::FAction),
    skip(Fic, b"aircraftType"),
    t(Fic, b"airports", Field::FAirports),
    skip(Fic, b"altitude"),
    skip(Fic, b"approvalTime"),
    skip(Fic, b"entryTime"),
    t(Fic, b"eventTime", Field::FEvent),
    skip(Fic, b"exclusions"),
    skip(Fic, b"facility"),
    skip(Fic, b"impactedElements"),
    t(Fic, b"mitNumber", Field::FNumber),
    t(Fic, b"mitValue", Field::FRate),
    skip(Fic, b"passback"),
    skip(Fic, b"providerStatus"),
    t(Fic, b"qualifier", Field::FTitle),
    t(Fic, b"reasonText", Field::FText),
    skip(Fic, b"referenceRestrictionEndTime"),
    t(Fic, b"referenceRestrictionId", Field::FRefId),
    t(Fic, b"remarks", Field::FRemarks),
    t(Fic, b"restrictedNasElements", Field::FElement),
    t(Fic, b"restrictionCategory", Field::Category),
    t(Fic, b"restrictionId", Field::FId),
    t(Fic, b"restrictionType", Field::Scope),
    skip(Fic, b"speed"),
    t(Fic, b"startTime", Field::FStart),
    t(Fic, b"stopTime", Field::FEnd),
];

const GADV_KIDS: &[El] = &[
    t(Fic, b"advisoryNumber", Field::FNumber),
    t(Fic, b"advisoryText", Field::FText),
    t(Fic, b"advisoryTitle", Field::FTitle),
    t(Fic, b"dateSent", Field::FEvent),
    n(
        Fic,
        b"effectivePeriod",
        &[
            t(Fidt, b"endTime", Field::FEnd),
            t(Fidt, b"startTime", Field::FStart),
        ],
    ),
    t(Fic, b"facilities", Field::FElement),
    t(Fic, b"origin", Field::FOrigin),
];

const FXA_KIDS: &[El] = &[
    skip(Fic, b"colorId"),
    skip(Fic, b"createSiteId"),
    skip(Fic, b"createWorkstationId"),
    t(Fic, b"endTime", Field::FEnd),
    skip(Fic, b"extendedFeaFlag"),
    skip(Fic, b"fcaDisplayPreferences"),
    skip(Fic, b"fcaDomain"),
    t(Fic, b"fcaId", Field::FTitle),
    t(Fic, b"fcaName", Field::FElement),
    t(Fic, b"fcaReason", Field::FText),
    t(Fic, b"feaFcaType", Field::FcaType),
    skip(Fic, b"fsmEligibleFlag"),
    t(Fic, b"lastUpdate", Field::FEvent),
    skip(Fic, b"lastUpdateSiteId"),
    skip(Fic, b"lastUpdateWorkstationId"),
    skip(Fic, b"line"),
    skip(Fic, b"lookAheadTime"),
    n(Fic, b"nas", &[t(Fic, b"airportName", Field::FAirports)]),
    skip(Fic, b"polygon"),
    skip(Fic, b"primaryFilter"),
    skip(Fic, b"secondaryFilter"),
    t(Fic, b"startTime", Field::FStart),
    t(Fic, b"tmiStatus", Field::FStatus),
];

const APTC_KIDS: &[El] = &[
    skip(Fic, b"aarAdjust"),
    skip(Fic, b"adrEnteredOnPanel"),
    t(Fic, b"airport", Field::FElement),
    skip(Fic, b"airportInFile"),
    t(Fic, b"arrRate", Field::FRate),
    t(Fic, b"arrRunwayConf", Field::FTitle),
    skip(Fic, b"arrUserSpecified"),
    t(Fic, b"depRate", Field::FRateDep),
    skip(Fic, b"depRunwayConf"),
    skip(Fic, b"depUserSpecified"),
    skip(Fic, b"dynArrEnteredOnPanel"),
    skip(Fic, b"enteringFacility"),
    skip(Fic, b"entryTime"),
    t(Fic, b"eventTime", Field::FEvent),
    skip(Fic, b"facility"),
    t(Fic, b"remarks", Field::FRemarks),
    skip(Fic, b"remarksGroupColor"),
    t(Fic, b"stratAar", Field::FRateStrat),
    skip(Fic, b"updateTime"),
    t(Fic, b"weather", Field::FText),
];

use crate::rows::fkind as fk;

/// Every flow information message type that becomes a flow row.
pub static FLOW_BODIES: [FlowBody; 4] = [
    FlowBody {
        msg_type: b"RSTR",
        kind: fk::RSTR,
        el: n(Fi, b"restrictionMessage", RSTR_KIDS),
    },
    FlowBody {
        msg_type: b"GADV",
        kind: fk::GADV,
        el: n(Fi, b"generalAdvisory", GADV_KIDS),
    },
    FlowBody {
        msg_type: b"FXA",
        kind: fk::FXA,
        el: n(Fi, b"feaFca", FXA_KIDS),
    },
    FlowBody {
        msg_type: b"APTC",
        kind: fk::APTC,
        el: n(Fi, b"airportConfigMessage", APTC_KIDS),
    },
];

/// The `msgType` of a TMI flight list.
pub const TMI_LIST_TYPE: &[u8] = b"TMI_FLIGHT_LIST";
/// The body element of a TMI flight list, whose `flightData` children each become a row.
pub const TMI_LIST_BODY: &[u8] = b"tmiFlightDataList";

const TMI_FLIGHT_KIDS: &[El] = &[
    t(Core, b"aircraftId", Field::Acid),
    n(Core, b"arrivalPoint", &[t(Core, b"airport", Field::Arr)]),
    n(Core, b"departurePoint", &[t(Core, b"airport", Field::Dep)]),
    t(Core, b"gufi", Field::Gufi),
    t(Core, b"igtd", Field::Igtd),
];
const FXA_FLIGHT_DATA_KIDS: &[El] = &[skip_count(Fic2, b"fxaFlight", Field::NFxa)];
const TMI_INFO_KIDS: &[El] = &[
    n(Fic2, b"fxaFlightData", FXA_FLIGHT_DATA_KIDS),
    el(
        Fic2,
        b"tmi",
        None,
        &[
            at(b"updateType", Field::TmiUpdate),
            at(b"lastUpdateTime", Field::TmiTime),
        ],
        &[t(Fic2, b"fcaId", Field::Fca)],
    ),
];
const FLIGHT_DATA_KIDS: &[El] = &[
    skip(Fic2, b"ARTD"),
    skip(Fic2, b"BETA"),
    skip(Fic2, b"BETD"),
    skip(Fic2, b"CANCELLED_BUT_FLEW"),
    skip(Fic2, b"CDM_MBR"),
    skip(Fic2, b"CTA_COMPLIANCE"),
    skip(Fic2, b"CTD_COMPLIANCE"),
    skip(Fic2, b"DVSTATUS"),
    skip(Fic2, b"ETE_COMPLIANCE"),
    skip(Fic2, b"GDC"),
    skip(Fic2, b"IGTA"),
    skip(Fic2, b"MAJOR"),
    skip(Fic2, b"OETA"),
    skip(Fic2, b"OETD"),
    skip(Fic2, b"PETE"),
    skip(Fic2, b"PGTA"),
    skip(Fic2, b"PGTD"),
    skip(Fic2, b"SPURIOUS_FLT"),
    skip(Fic2, b"aircraftCategory"),
    skip(Fic2, b"engineClass"),
    n(Fic2, b"flight", TMI_FLIGHT_KIDS),
    t(Fic2, b"flightReference", Field::FlightRef),
    skip(Fic2, b"fltTraversals"),
    skip(Fic2, b"rvsmData"),
    t(Fic2, b"status", Field::TmiStatus),
    n(Fic2, b"tmiFlightInfoList", TMI_INFO_KIDS),
    skip(Fic2, b"userCategory"),
    skip(Fic2, b"weightClass"),
];

/// One `flightData` entry of a TMI flight list.
pub static FLIGHT_DATA: El = n(Fi, b"flightData", FLIGHT_DATA_KIDS);

/// The deepest chain of entered (not ignored) elements below a message element, for the
/// decoder's fixed walk stack; checked by a test.
pub const MAX_DEPTH: usize = 12;

/// The `fltdMessage` element with the attributes it carries and no children (the bodies are
/// looked up by `msgType`).
pub static FLTD_MESSAGE: El = el(Fd, b"fltdMessage", None, FLTD_ATTRS, &[]);
/// The `fiMessage` element.
pub static FI_MESSAGE: El = el(Fi, b"fiMessage", None, FI_ATTRS, &[]);

//! The fields of the TFMS data service, the ways a field can be refused and the whole-message
//! errors. Plain data. Core (pure).

use crate::time::TimeError;
use oa_xml::XmlError;

/// Every attribute and element value the decoder reads, for error attribution and for the
/// duplicate check (one bit per field in a `u128`, so there are at most 128). The discriminant
/// indexes [`crate::Stats::field_errors`]. Fields shared by several message types (`FStart` is
/// `startTime` of a restriction, of an FCA and of an advisory period) carry the generic name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Field {
    // fltdMessage / fiMessage attributes
    MsgType,
    Acid,
    FlightRef,
    Airline,
    Major,
    Dep,
    Arr,
    SrcFac,
    SrcTime,
    Trigger,
    CdmPart,
    // qualifiedAircraftId
    Gufi,
    Igtd,
    CidFac,
    CidNum,
    AcCat,
    UserCat,
    // times, status, aircraft
    Eta,
    EtaType,
    Etd,
    EtdType,
    Status,
    AcType,
    EngClass,
    Equip,
    Special,
    Diversion,
    RvsmEq,
    RvsmCur,
    RvsmFut,
    // flightTimeData attributes
    OutT,
    OffT,
    OnT,
    InT,
    RwyDep,
    RwyArr,
    GateDep,
    GateArr,
    Created,
    OrigDep,
    OrigArr,
    // fixes and times, positions, speeds, altitudes
    ArrFix,
    ArrFixTime,
    DepFix,
    DepFixTime,
    LatDeg,
    LatMin,
    LatSec,
    LatDir,
    LonDeg,
    LonMin,
    LonSec,
    LonDir,
    /// Latitude and longitude together: an incomplete or out-of-range pair.
    Position,
    NextLat,
    NextLon,
    PosTime,
    Alt,
    AsgAlt,
    ReqAlt,
    Speed,
    Tas,
    // route
    Dp,
    Star,
    Route,
    RouteLegacy,
    Remarks,
    // departure, arrival, coordination, boundary crossing
    Tod,
    TodEst,
    Toa,
    ToaEst,
    Coord,
    CoordType,
    CoordFix,
    Bnd,
    // TMI flight list
    TmiStatus,
    TmiUpdate,
    TmiTime,
    Fca,
    NFxa,
    // flow information messages
    Category,
    FcaType,
    Scope,
    FId,
    FNumber,
    FRefId,
    FAction,
    FStart,
    FEnd,
    FEvent,
    FOrigin,
    FRate,
    FRateDep,
    FRateStrat,
    FText,
    FTitle,
    FElement,
    FAirports,
    FRemarks,
    FStatus,
}

/// Number of [`Field`] values.
pub const FIELD_COUNT: usize = 100;
const _: () = assert!(FIELD_COUNT <= 128);

/// The wire name of a field (`airlineOutTime`, `sourceTimeStamp`).
pub fn field_name(f: Field) -> &'static str {
    const NAMES: [&str; FIELD_COUNT] = [
        "msgType",
        "acid",
        "flightRef",
        "airline",
        "major",
        "depArpt",
        "arrArpt",
        "sourceFacility",
        "sourceTimeStamp",
        "fdTrigger",
        "cdmPart",
        "gufi",
        "igtd",
        "facilityIdentifier",
        "idNumber",
        "aircraftCategory",
        "userCategory",
        "eta",
        "etaType",
        "etd",
        "etdType",
        "flightStatus",
        "aircraftModel",
        "aircraftEngineClass",
        "equipmentQualifier",
        "specialAircraftQualifier",
        "diversionIndicator",
        "rvsm.equipped",
        "rvsm.currentCompliance",
        "rvsm.futureCompliance",
        "airlineOutTime",
        "airlineOffTime",
        "airlineOnTime",
        "airlineInTime",
        "runwayDeparture",
        "runwayArrival",
        "gateDeparture",
        "gateArrival",
        "flightCreation",
        "originalDeparture",
        "originalArrival",
        "arrivalFixAndTime.fixName",
        "arrivalFixAndTime.arrTime",
        "departureFixAndTime.fixName",
        "departureFixAndTime.arrTime",
        "latitudeDMS.degrees",
        "latitudeDMS.minutes",
        "latitudeDMS.seconds",
        "latitudeDMS.direction",
        "longitudeDMS.degrees",
        "longitudeDMS.minutes",
        "longitudeDMS.seconds",
        "longitudeDMS.direction",
        "position",
        "latitudeDecimal",
        "longitudeDecimal",
        "timeAtPosition",
        "reportedAltitude",
        "assignedAltitude",
        "requestedAltitude",
        "speed",
        "filedTrueAirSpeed",
        "dp.routeName",
        "star.routeName",
        "routeOfFlight",
        "routeOfFlight.legacyFormat",
        "remarksKeywords",
        "timeOfDeparture",
        "timeOfDeparture.estimated",
        "timeOfArrival",
        "timeOfArrival.estimated",
        "coordinationTime",
        "coordinationTime.type",
        "coordinationPoint",
        "boundaryCrossingTime",
        "flightData.status",
        "tmi.updateType",
        "tmi.lastUpdateTime",
        "tmi.fcaId",
        "fxaFlight",
        "restrictionCategory",
        "feaFcaType",
        "restrictionType",
        "restrictionId",
        "mitNumber|advisoryNumber",
        "referenceRestrictionId",
        "action",
        "startTime",
        "stopTime",
        "eventTime",
        "origin",
        "mitValue|arrRate",
        "depRate",
        "stratAar",
        "reasonText|advisoryText|fcaReason|weather",
        "qualifier|advisoryTitle|fcaId|arrRunwayConf",
        "restrictedNasElements|facilities|fcaName|airport",
        "airports|nas.airportName",
        "remarks",
        "tmiStatus",
    ];
    NAMES.get(f as usize).copied().unwrap_or("?")
}

/// Why one field was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldErrorKind {
    /// A required attribute is absent.
    Missing,
    /// A name longer than its column.
    TooLong,
    /// A byte that is not printable ASCII (or not in the field's alphabet), or a control
    /// character in free text.
    BadChar,
    /// A value that does not have the field's shape (a number, an altitude, a flag).
    Format,
    /// A malformed timestamp.
    Time(TimeError),
    /// A word outside the field's vocabulary (a status, a category, a time type).
    Vocabulary,
    /// A number outside its range (degrees, minutes, a latitude beyond the poles).
    Range,
    /// The same field twice in one message.
    Duplicate,
    /// Two copies of one value disagree (the message attribute and the body's copy).
    Inconsistent,
    /// A value element that has child elements.
    NotLeaf,
    /// Text that is split over several nodes, or whose references do not decode.
    Text,
}

/// A refused field: which, why, and a byte offset in the message: the value of a refused
/// attribute, the name of a refused element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: Field,
    pub kind: FieldErrorKind,
    pub at: u32,
}

/// Which of the caller's tables was full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapKind {
    Flights,
    Flows,
    Strings,
}

/// Why a whole document was refused (it yields no rows). A refused field no longer refuses the
/// document: the message is dropped and counted in [`crate::Outcome::rejected`]; `Field` is the
/// reason carried by those counts and by [`crate::Outcome::first_reject`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocError {
    /// The XML parser refused the bytes (only from [`crate::decode_bytes`]).
    Xml(XmlError),
    /// The root is not a `tfmDataService` element in the TFM data service namespace.
    NotTfms { at: u32 },
    /// A field was refused.
    Field(FieldError),
    /// The message has more rows, or more text, than the caller's [`crate::Rows`] can hold.
    Capacity(CapKind),
}

/// Reject reasons for statistics: the index into [`crate::Stats::rejects`].
pub const REJECT_XML: usize = 0;
pub const REJECT_NOT_TFMS: usize = 1;
pub const REJECT_FIELD: usize = 2;
pub const REJECT_CAP_FLIGHTS: usize = 3;
pub const REJECT_CAP_FLOWS: usize = 4;
pub const REJECT_CAP_STRINGS: usize = 5;
pub const REJECT_COUNT: usize = 6;

/// Display names of the reject reasons.
pub const REJECT_NAMES: [&str; REJECT_COUNT] = [
    "xml",
    "not-tfms",
    "field",
    "capacity-flights",
    "capacity-flows",
    "capacity-strings",
];

/// The statistics index of an error.
pub fn reject_reason(e: &DocError) -> usize {
    match e {
        DocError::Xml(_) => REJECT_XML,
        DocError::NotTfms { .. } => REJECT_NOT_TFMS,
        DocError::Field(_) => REJECT_FIELD,
        DocError::Capacity(CapKind::Flights) => REJECT_CAP_FLIGHTS,
        DocError::Capacity(CapKind::Flows) => REJECT_CAP_FLOWS,
        DocError::Capacity(CapKind::Strings) => REJECT_CAP_STRINGS,
    }
}

/// Number of [`FieldErrorKind`] values.
pub const ERROR_KIND_COUNT: usize = 11;

/// The index of a field error kind (for per-reason counts), in declaration order.
pub fn error_kind_index(k: FieldErrorKind) -> usize {
    match k {
        FieldErrorKind::Missing => 0,
        FieldErrorKind::TooLong => 1,
        FieldErrorKind::BadChar => 2,
        FieldErrorKind::Format => 3,
        FieldErrorKind::Time(_) => 4,
        FieldErrorKind::Vocabulary => 5,
        FieldErrorKind::Range => 6,
        FieldErrorKind::Duplicate => 7,
        FieldErrorKind::Inconsistent => 8,
        FieldErrorKind::NotLeaf => 9,
        FieldErrorKind::Text => 10,
    }
}

/// Display names of the field error kinds, for reports.
pub fn error_kind_name(k: FieldErrorKind) -> &'static str {
    match k {
        FieldErrorKind::Missing => "missing",
        FieldErrorKind::TooLong => "too-long",
        FieldErrorKind::BadChar => "bad-char",
        FieldErrorKind::Format => "format",
        FieldErrorKind::Time(_) => "time",
        FieldErrorKind::Vocabulary => "vocabulary",
        FieldErrorKind::Range => "range",
        FieldErrorKind::Duplicate => "duplicate",
        FieldErrorKind::Inconsistent => "inconsistent",
        FieldErrorKind::NotLeaf => "not-leaf",
        FieldErrorKind::Text => "text",
    }
}

/// What a document turned out to be: the kind of its output element(s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DocKind {
    /// `fltdOutput`: flight data messages.
    Flight,
    /// `fiOutput`: flow information messages.
    FlowInfo,
    /// `tfmsStatusOutput`: interface status table; no rows.
    Status,
    /// More than one of the above in one document.
    Mixed,
    /// No output element the decoder knows.
    Empty,
}

/// Number of [`DocKind`] values.
pub const DOC_KIND_COUNT: usize = 5;

/// Display names of the document kinds, in [`DocKind`] order.
pub const DOC_KIND_NAMES: [&str; DOC_KIND_COUNT] =
    ["flight", "flow-info", "status", "mixed", "empty"];

const _: () = assert!(Field::FStatus as usize + 1 == FIELD_COUNT);

//! The fields a row carries, the feature kinds, and the reasons a message is refused. Plain
//! data. Core (pure).

use oa_xml::XmlError;

/// The fields of a row, in bit order of [`crate::Notams::present`] and [`crate::Notams::errors`].
/// The event identifier is not here: a row always has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Field {
    /// `NOTAM/series`.
    Series = 0,
    /// `NOTAM/number`.
    Number,
    /// `NOTAM/year`.
    Year,
    /// `NOTAM/type`: new, replace, cancel (absent in two of the 112 surveyed messages).
    Type,
    /// `NOTAM/referredSeries`, `referredNumber`, `referredYear`, which are one field: all
    /// three, or number and year alone (a local-format reference, series left zero), or none.
    Ref,
    /// `NOTAM/location`.
    Location,
    /// `NOTAM/affectedFIR`.
    Fir,
    /// `EventExtension/icaoLocation`.
    IcaoLocation,
    /// `NOTAM/issued`.
    Issued,
    /// `validTime/TimePeriod/beginPosition` and `NOTAM/effectiveStart`.
    Start,
    /// `validTime/TimePeriod/endPosition` and `NOTAM/effectiveEnd`; an unknown end is in flags.
    End,
    /// `NOTAM/schedule`, kept in the string table.
    Schedule,
    /// `NOTAM/text`, kept in the string table.
    Text,
    /// `NOTAM/selectionCode`.
    Selection,
    /// `EventTimeSlice/scenario`.
    Scenario,
    /// `EventExtension/classification`.
    Classification,
    /// `NOTAM/traffic`.
    Traffic,
    /// `NOTAM/purpose`.
    Purpose,
    /// `NOTAM/scope`.
    Scope,
    /// `EventExtension/canceled`.
    Canceled,
    /// `EventExtension/xovernotamID`.
    Xover,
    /// `NOTAM/coordinates`, the centre of the circle.
    Centre,
    /// `NOTAM/radius`.
    Radius,
    /// `NOTAM/lowerLimit`, else `minimumFL`.
    Floor,
    /// `NOTAM/upperLimit`, else `maximumFL`.
    Ceiling,
    /// Every `posList` of the message's features; one bad polygon fails the field and none is
    /// kept.
    Polygons,
    /// `AirportHeliport/designator`.
    Airport,
    /// `AirportHeliport/locationIndicatorICAO`.
    AirportIcao,
    /// `Runway/designator`, else `RunwayDirection/designator`.
    Runway,
    /// `Navaid/designator`, else `Navaid/type`.
    Navaid,
    /// `Unit/designator`.
    Unit,
}

/// Number of [`Field`] values.
pub const FIELD_COUNT: usize = 31;

/// Display names, indexed by `Field as usize`.
pub const FIELD_NAMES: [&str; FIELD_COUNT] = [
    "series",
    "number",
    "year",
    "type",
    "ref",
    "location",
    "fir",
    "icao_location",
    "issued",
    "start",
    "end",
    "schedule",
    "text",
    "selection",
    "scenario",
    "classification",
    "traffic",
    "purpose",
    "scope",
    "canceled",
    "xover",
    "centre",
    "radius",
    "floor",
    "ceiling",
    "polygons",
    "airport",
    "airport_icao",
    "runway",
    "navaid",
    "unit",
];

/// All fields, in bit order.
pub const ALL_FIELDS: [Field; FIELD_COUNT] = [
    Field::Series,
    Field::Number,
    Field::Year,
    Field::Type,
    Field::Ref,
    Field::Location,
    Field::Fir,
    Field::IcaoLocation,
    Field::Issued,
    Field::Start,
    Field::End,
    Field::Schedule,
    Field::Text,
    Field::Selection,
    Field::Scenario,
    Field::Classification,
    Field::Traffic,
    Field::Purpose,
    Field::Scope,
    Field::Canceled,
    Field::Xover,
    Field::Centre,
    Field::Radius,
    Field::Floor,
    Field::Ceiling,
    Field::Polygons,
    Field::Airport,
    Field::AirportIcao,
    Field::Runway,
    Field::Navaid,
    Field::Unit,
];

/// The wire-ish name of a field.
pub fn field_name(f: Field) -> &'static str {
    FIELD_NAMES.get(f as usize).copied().unwrap_or("?")
}

/// `Notams::ntype` codes (0 is "not present").
pub const TYPE_NEW: u8 = 1;
pub const TYPE_REPLACE: u8 = 2;
pub const TYPE_CANCEL: u8 = 3;

/// `Notams::class` codes (0 is "not present").
pub const CLASS_INTL: u8 = 1;
pub const CLASS_DOM: u8 = 2;
pub const CLASS_MIL: u8 = 3;

/// `Notams::traffic` bits.
pub const TRAFFIC_I: u8 = 1;
pub const TRAFFIC_V: u8 = 2;
pub const TRAFFIC_K: u8 = 4;

/// `Notams::purpose` bits.
pub const PURPOSE_N: u8 = 1;
pub const PURPOSE_B: u8 = 2;
pub const PURPOSE_O: u8 = 4;
pub const PURPOSE_M: u8 = 8;
pub const PURPOSE_K: u8 = 16;

/// `Notams::scope` bits.
pub const SCOPE_A: u8 = 1;
pub const SCOPE_E: u8 = 2;
pub const SCOPE_W: u8 = 4;
pub const SCOPE_K: u8 = 8;

/// `Notams::floor_ref` and `ceil_ref` codes (0 is "not present").
pub const REF_MSL: u8 = 1;
pub const REF_AGL: u8 = 2;
pub const REF_FL: u8 = 3;

/// Altitude value for "unlimited" (`UNL`), the same sentinel the airspace model uses.
pub const ALT_UNLIMITED: i32 = i32::MAX;

/// `Notams::flags` bits.
/// The end of the period is permanent (`effectiveEnd` reads `PERM`).
pub const FLAG_PERMANENT: u32 = 1;
/// The end of the period is an estimate (`effectiveEnd` ends in `EST`).
pub const FLAG_ESTIMATED: u32 = 2;
/// The source has no concrete valid-time end. Canonical GML uses
/// `endPosition indeterminatePosition="unknown"`; this does not mean `after`, `before` or `now`.
pub const FLAG_END_UNKNOWN: u32 = 4;

/// AIXM time-slice `interpretation` values retained from the source event.
pub const INTERPRETATION_BASELINE: u8 = 1;
pub const INTERPRETATION_SNAPSHOT: u8 = 2;
pub const INTERPRETATION_TEMPDELTA: u8 = 3;
pub const INTERPRETATION_PERMDELTA: u8 = 4;

/// Event time-slice metadata presence bit: the AIXM `sequenceNumber` element was present.
pub const SLICE_SEQUENCE_PRESENT: u8 = 1;
/// Event time-slice metadata presence bit: the AIXM `correctionNumber` element was present.
pub const SLICE_CORRECTION_PRESENT: u8 = 2;
/// Event time-slice metadata error bit: the AIXM `interpretation` was malformed or repeated.
pub const SLICE_INTERPRETATION_ERROR: u8 = 1;
/// Event time-slice metadata error bit: the AIXM `sequenceNumber` was malformed or repeated.
pub const SLICE_SEQUENCE_ERROR: u8 = 2;
/// Event time-slice metadata error bit: the AIXM `correctionNumber` was malformed or repeated.
pub const SLICE_CORRECTION_ERROR: u8 = 4;

/// Feature kinds (`Notams::feat_kind`, and the bit index in `Notams::feat_mask`); 0 is none.
pub const FEAT_AIRPORT: u8 = 1;
pub const FEAT_RUNWAY: u8 = 2;
pub const FEAT_RUNWAY_DIRECTION: u8 = 3;
pub const FEAT_RUNWAY_ELEMENT: u8 = 4;
pub const FEAT_TAXIWAY: u8 = 5;
pub const FEAT_TAXIWAY_ELEMENT: u8 = 6;
pub const FEAT_NAVAID: u8 = 7;
pub const FEAT_UNIT: u8 = 8;
pub const FEAT_OBSTACLE_AREA: u8 = 9;
pub const FEAT_VERTICAL_STRUCTURE: u8 = 10;
pub const FEAT_RADAR_SYSTEM: u8 = 11;
pub const FEAT_PRIMARY_RADAR: u8 = 12;
/// Number of feature kinds (the codes run 1 to this).
pub const FEAT_COUNT: usize = 12;

/// Display names of the feature kinds, indexed by code minus one.
pub const FEAT_NAMES: [&str; FEAT_COUNT] = [
    "AirportHeliport",
    "Runway",
    "RunwayDirection",
    "RunwayElement",
    "Taxiway",
    "TaxiwayElement",
    "Navaid",
    "Unit",
    "ObstacleArea",
    "VerticalStructure",
    "RadarSystem",
    "PrimarySurveillanceRadar",
];

/// The display name of a feature kind code (`"none"` for 0 or an unknown code).
pub fn feat_name(kind: u8) -> &'static str {
    usize::from(kind)
        .checked_sub(1)
        .and_then(|i| FEAT_NAMES.get(i))
        .copied()
        .unwrap_or("none")
}

/// Why a whole message was refused (it yields no row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reject {
    /// The root is not an AIXM 5.1 `AIXMBasicMessage`.
    NotAixm = 0,
    /// No `event:Event` member.
    NoEvent,
    /// More than one `event:Event` member (the feature-to-event links are not followed, so a
    /// feature could not be assigned to one of them).
    MultipleEvents,
    /// The event has no `gml:identifier`, or it is empty.
    MissingId,
    /// An identifier that is not a UUID with `codeSpace` `urn:uuid:`, or a second one.
    BadId,
    /// The event has no `textNOTAM/NOTAM` element, or has two.
    NoNotam,
}

/// Number of [`Reject`] values.
pub const REJECT_COUNT: usize = 6;

/// Display names, indexed by `Reject as usize`.
pub const REJECT_NAMES: [&str; REJECT_COUNT] = [
    "not an AIXM basic message",
    "no event",
    "several events",
    "missing event id",
    "bad event id",
    "no NOTAM element",
];

/// Which table of [`crate::Notams`] was full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityKind {
    Rows,
    Strings,
    Polygons,
    Vertices,
    Features,
}

/// Why a document yielded no row. All or nothing: the caller's tables are as they were.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocError {
    /// The XML parser refused the bytes (only from [`crate::decode_bytes`]).
    Xml(XmlError),
    /// The message was refused whole.
    Rejected(Reject),
    /// The document has more elements than [`crate::Limits::max_elements`].
    TooManyElements { elements: usize, limit: usize },
    /// A table of the caller's [`crate::Notams`] is full.
    Capacity(CapacityKind),
}

/// Reject-statistics index of an error: the [`Reject`] value, or `REJECT_COUNT + 0` for XML,
/// `+ 1` for too many elements, `+ 2` for capacity.
pub fn reject_index(e: &DocError) -> usize {
    match e {
        DocError::Rejected(r) => *r as usize,
        DocError::Xml(_) => REJECT_COUNT,
        DocError::TooManyElements { .. } => REJECT_COUNT + 1,
        DocError::Capacity(_) => REJECT_COUNT + 2,
    }
}

/// Number of reject-statistics slots: the [`Reject`] reasons, then xml, too-many-elements and
/// capacity.
pub const REJECT_SLOTS: usize = REJECT_COUNT + 3;

/// Display names of the reject-statistics slots.
pub const REJECT_SLOT_NAMES: [&str; REJECT_SLOTS] = [
    "not an AIXM basic message",
    "no event",
    "several events",
    "missing event id",
    "bad event id",
    "no NOTAM element",
    "xml",
    "too many elements",
    "capacity",
];

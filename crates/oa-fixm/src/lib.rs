#![forbid(unsafe_code)]
//! oa-fixm: decodes FAA SWIM SFDPS messages (FIXM 3.0 core with the FAA NAS 3.0 extension, root
//! `MessageCollection` in `http://www.faa.aero/nas/3.0`) into flat flight-event rows. Core
//! (pure): a function of the parsed document and the bytes it came from, no I/O, no clock
//! (RULESET B1, B2).
//!
//! * One row per `<message>`, appended to a caller-owned struct-of-arrays [`Events`] with a
//!   stated capacity. Capacity is checked for the whole document before anything is kept: a
//!   document that does not fit is refused with [`FixmError::Capacity`] and the rows and string
//!   table are exactly as they were (INTERFACES IF-R1).
//! * Strict fields. A field that is present but malformed (bad range, unknown unit, bad
//!   syntax, duplicated) is counted, the row is kept with the field marked absent and its bit
//!   set in the row's error word. Nothing is defaulted silently. A message without a usable
//!   GUFI or timestamp is rejected whole and counted with its reason.
//! * Unknown elements and attributes (anything outside the structure seen in real data, see
//!   `docs/SFDPS_SURVEY.md`) are counted per path in a bounded table; they are never errors.
//!   Only the topmost unknown element of a subtree is counted.
//! * Units are read from the `uom` attributes and converted to the model units (RULESET A9):
//!   feet, 0.1 knot, 1e-7 degree, Unix milliseconds. An unknown `uom` is a field error.
//! * A [`Decoder`] preallocates every table at construction; [`Decoder::decode`] allocates
//!   nothing (RULESET C1). Rows are plain data and are read through the free functions in this
//!   crate (RULESET A1).

mod decode;
mod known;
mod scalar;

pub use decode::Decoder;
pub use scalar::{parse_beacon, parse_iso_ms, parse_scaled, parse_uuid};

use std::fmt;

/// The fields a row can carry, in bit order of [`Events::present`] and [`Events::errors`].
/// The message timestamp and the GUFI are not here: a row always has both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Field {
    /// `flight@source`, a two-letter message source code.
    Source = 0,
    /// `flight@centre`, the originating ARTCC.
    Centre,
    /// `flightIdentification@aircraftIdentification`.
    Callsign,
    /// `flightIdentification@computerId`.
    ComputerId,
    /// `flightPlan@identifier`.
    FlightPlanId,
    /// `departure@departurePoint`.
    Departure,
    /// `arrival@arrivalPoint`.
    Arrival,
    /// `flightStatus@fdpsFlightStatus`, see the `STATUS_*` codes.
    Status,
    /// `flight@flightType`, see the `FTYPE_*` codes.
    FlightType,
    /// `assignedAltitude/simple` or `vfrPlus`.
    AssignedAlt,
    /// `enRoute/position/altitude`.
    ReportedAlt,
    /// `enRoute/position/position/location/pos`.
    Position,
    /// `enRoute/position@positionTime`.
    PositionTime,
    /// `enRoute/position/actualSpeed/surveillance`.
    GroundSpeed,
    /// `enRoute/position/trackVelocity` x and y together.
    Velocity,
    /// `enRoute/position@coastIndicator`.
    Coast,
    /// `aircraftDescription/aircraftType/icaoModelIdentifier`.
    AircraftType,
    /// `agreed/route@nasRouteText`, kept in the string table.
    Route,
    /// `operator/operatingOrganization/organization@name`.
    Operator,
    /// `enRoute/beaconCodeAssignment/currentBeaconCode`.
    Beacon,
    /// `controllingUnit` unit and sector.
    ControllingUnit,
    /// `enRoute/boundaryCrossings/handoff/receivingUnit`.
    HandoffReceiving,
    /// `enRoute/boundaryCrossings/handoff/transferringUnit`.
    HandoffTransferring,
    /// `departure/runwayPositionAndTime/runwayTime/actual@time`.
    DepartureActual,
    /// `arrival/runwayPositionAndTime/runwayTime/estimated@time`.
    ArrivalEstimated,
    /// `nameValue MSG_SEQ_NO`.
    MsgSeq,
    /// `nameValue FLIGHT_PLAN_SEQ_NO`.
    PlanSeq,
}

/// Number of [`Field`] values.
pub const FIELD_COUNT: usize = 27;

/// Display names, indexed by `Field as usize`.
pub const FIELD_NAMES: [&str; FIELD_COUNT] = [
    "source",
    "centre",
    "callsign",
    "computer_id",
    "flight_plan_id",
    "departure",
    "arrival",
    "status",
    "flight_type",
    "assigned_alt",
    "reported_alt",
    "position",
    "position_time",
    "ground_speed",
    "velocity",
    "coast",
    "aircraft_type",
    "route",
    "operator",
    "beacon",
    "controlling_unit",
    "handoff_receiving",
    "handoff_transferring",
    "departure_actual",
    "arrival_estimated",
    "msg_seq",
    "plan_seq",
];

/// All fields, in bit order.
pub const ALL_FIELDS: [Field; FIELD_COUNT] = [
    Field::Source,
    Field::Centre,
    Field::Callsign,
    Field::ComputerId,
    Field::FlightPlanId,
    Field::Departure,
    Field::Arrival,
    Field::Status,
    Field::FlightType,
    Field::AssignedAlt,
    Field::ReportedAlt,
    Field::Position,
    Field::PositionTime,
    Field::GroundSpeed,
    Field::Velocity,
    Field::Coast,
    Field::AircraftType,
    Field::Route,
    Field::Operator,
    Field::Beacon,
    Field::ControllingUnit,
    Field::HandoffReceiving,
    Field::HandoffTransferring,
    Field::DepartureActual,
    Field::ArrivalEstimated,
    Field::MsgSeq,
    Field::PlanSeq,
];

/// `Events::status` codes (0 is "not present").
pub const STATUS_PROPOSED: u8 = 1;
pub const STATUS_ACTIVE: u8 = 2;
pub const STATUS_COMPLETED: u8 = 3;
pub const STATUS_CANCELLED: u8 = 4;
pub const STATUS_DROPPED: u8 = 5;

/// `Events::flight_type` codes (0 is "not present").
pub const FTYPE_SCHEDULED: u8 = 1;
pub const FTYPE_NON_SCHEDULED: u8 = 2;
pub const FTYPE_GENERAL: u8 = 3;

/// `Events::flags` bits.
/// The assigned altitude is "VFR" (no value).
pub const FLAG_ASSIGNED_VFR: u32 = 1;
/// The assigned altitude is "VFR on top plus" the stated value.
pub const FLAG_ASSIGNED_VFR_PLUS: u32 = 2;

/// Why a whole message was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reject {
    /// No `gufi` element, or it is empty.
    MissingGufi = 0,
    /// A GUFI that is not a `urn:uuid` UUID, or a second `gufi` element.
    BadGufi,
    /// No `timestamp` attribute on `flight`.
    MissingTimestamp,
    /// A timestamp that is not strict ISO-8601 UTC, or a duplicated one.
    BadTimestamp,
    /// The message is not an `xsi:type` `FlightMessageType`.
    NotFlightMessage,
    /// A second `flight` element in one message.
    DuplicateFlight,
}

/// Number of [`Reject`] values.
pub const REJECT_COUNT: usize = 6;

/// Display names, indexed by `Reject as usize`.
pub const REJECT_NAMES: [&str; REJECT_COUNT] = [
    "missing GUFI",
    "bad GUFI",
    "missing timestamp",
    "bad timestamp",
    "not a FlightMessageType",
    "duplicate flight element",
];

/// Hard bounds on decoding. Defaults are above the measured SWIM feed (60 s sample,
/// 2026-11-14: up to 100 messages and 3,000 elements per file, routes up to 80 bytes).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest document, in elements, the decoder accepts (scratch tables are this long).
    pub max_elements: usize,
    /// Longest route text kept; a longer one is a field error, never truncated.
    pub max_route_len: usize,
    /// Rows of the unknown-path table; paths beyond it are counted in `unknown_overflow`.
    pub max_unknown_paths: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_elements: 50_000,
            max_route_len: 2048,
            max_unknown_paths: 256,
        }
    }
}

/// Longest unknown-path text kept; longer paths are cut and end in `~`.
pub const MAX_PATH_TEXT: usize = 160;

/// One unknown element or attribute path and how often it was seen.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct UnknownPath {
    pub hash: u64,
    pub count: u64,
    /// True for an attribute (`path@name`), false for an element.
    pub is_attr: bool,
    pub len: u8,
    pub text: [u8; MAX_PATH_TEXT],
}

/// Cumulative counts over every document a [`Decoder`] has seen.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Stats {
    pub docs: u64,
    /// Documents whose root is not a `MessageCollection` in the FAA NAS 3.0 namespace.
    pub unsupported_roots: u64,
    /// `message` elements seen.
    pub messages: u64,
    pub rows: u64,
    /// Rejected messages by [`Reject`] reason.
    pub rejected: [u64; REJECT_COUNT],
    /// Field errors by [`Field`].
    pub field_errors: [u64; FIELD_COUNT],
    pub unknown_elements: u64,
    pub unknown_attributes: u64,
    /// Unknown occurrences that did not fit the unknown-path table.
    pub unknown_overflow: u64,
    pub unknown: Vec<UnknownPath>,
}

/// The outcome of one [`Decoder::decode`] call.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// `message` elements in the document.
    pub messages: u32,
    /// Rows appended.
    pub rows: u32,
    /// Messages refused whole.
    pub rejected: u32,
    /// Field errors across the appended rows.
    pub field_errors: u32,
    pub unknown_elements: u32,
    pub unknown_attributes: u32,
    /// Messages not decoded because `Events` filled up ([`Decoder::decode_prefix`] only; always
    /// 0 from [`Decoder::decode`]).
    pub not_decoded: u32,
}

/// Why a document was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixmError {
    /// The root is not a `MessageCollection` in `http://www.faa.aero/nas/3.0`.
    UnsupportedRoot,
    /// The document has more elements than `Limits::max_elements`.
    TooManyElements { elements: usize, limit: usize },
    /// `input` is not the slice the document was parsed from.
    InputMismatch,
    /// The row capacity of `Events` cannot hold the document's messages.
    RowCapacity { capacity: usize },
    /// The route string table of `Events` is full.
    StringCapacity { capacity: usize },
}

impl fmt::Display for FixmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixmError::UnsupportedRoot => write!(f, "root is not a FAA NAS 3.0 MessageCollection"),
            FixmError::TooManyElements { elements, limit } => {
                write!(f, "{elements} elements exceed the limit of {limit}")
            }
            FixmError::InputMismatch => write!(f, "input is not the parsed document's bytes"),
            FixmError::RowCapacity { capacity } => {
                write!(f, "events full: row capacity {capacity}")
            }
            FixmError::StringCapacity { capacity } => {
                write!(f, "events full: string table capacity {capacity} bytes")
            }
        }
    }
}

impl std::error::Error for FixmError {}

/// Flight-event rows, struct-of-arrays: element `i` of every column is row `i`. Every column
/// is allocated at `capacity` once; rows past `len` are not meaningful. A column value is
/// meaningful only where the row's `present` word has the field's bit (bit `Field as u32`);
/// `errors` has the bit of every field that was present but malformed or duplicated.
/// Short text is stored zero-padded in fixed arrays; read it with [`code_str`].
// A3: one Vec per column.
#[derive(Clone, Debug)]
pub struct Events {
    pub capacity: usize,
    pub len: usize,
    pub present: Vec<u64>,
    pub errors: Vec<u64>,
    pub flags: Vec<u32>,
    pub source: Vec<[u8; 2]>,
    pub centre: Vec<[u8; 4]>,
    /// Message timestamp (`flight@timestamp`), Unix milliseconds.
    pub ts_ms: Vec<i64>,
    /// The GUFI as the 128 bits of its UUID.
    pub gufi: Vec<u128>,
    pub callsign: Vec<[u8; 8]>,
    pub computer_id: Vec<[u8; 4]>,
    pub plan_id: Vec<[u8; 12]>,
    pub departure: Vec<[u8; 12]>,
    pub arrival: Vec<[u8; 12]>,
    pub status: Vec<u8>,
    pub flight_type: Vec<u8>,
    /// Assigned altitude, feet.
    pub assigned_ft: Vec<i32>,
    /// Reported (surveillance) altitude, feet.
    pub reported_ft: Vec<i32>,
    /// Latitude and longitude, 1e-7 degree.
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    pub pos_time_ms: Vec<i64>,
    /// Ground speed, 0.1 knot.
    pub gs_dkt: Vec<u16>,
    /// Track velocity east and north components, 0.1 knot.
    pub vx_dkt: Vec<i16>,
    pub vy_dkt: Vec<i16>,
    pub aircraft_type: Vec<[u8; 4]>,
    /// Route text: byte offset and length in `strings`.
    pub route_off: Vec<u32>,
    pub route_len: Vec<u32>,
    pub operator: Vec<[u8; 24]>,
    /// Transponder code, four octal digits read as a decimal number (2604 for code 2604).
    pub beacon: Vec<u16>,
    /// Unit (bytes 0 to 3, `???` when the FAA does not know it) and sector (bytes 4 to 7),
    /// zero padded.
    pub ctrl: Vec<[u8; 8]>,
    pub recv: Vec<[u8; 8]>,
    pub xfer: Vec<[u8; 8]>,
    pub dep_actual_ms: Vec<i64>,
    pub arr_est_ms: Vec<i64>,
    pub msg_seq: Vec<u64>,
    pub plan_seq: Vec<u32>,
    /// The route string table: `strings_cap` bytes, `strings_used` of them in use.
    pub strings: Vec<u8>,
    pub strings_used: usize,
}

/// An empty `Events` with room for `capacity` rows and `string_bytes` bytes of route text.
pub fn new_events(capacity: usize, string_bytes: usize) -> Events {
    Events {
        capacity,
        len: 0,
        present: Vec::with_capacity(capacity),
        errors: Vec::with_capacity(capacity),
        flags: Vec::with_capacity(capacity),
        source: Vec::with_capacity(capacity),
        centre: Vec::with_capacity(capacity),
        ts_ms: Vec::with_capacity(capacity),
        gufi: Vec::with_capacity(capacity),
        callsign: Vec::with_capacity(capacity),
        computer_id: Vec::with_capacity(capacity),
        plan_id: Vec::with_capacity(capacity),
        departure: Vec::with_capacity(capacity),
        arrival: Vec::with_capacity(capacity),
        status: Vec::with_capacity(capacity),
        flight_type: Vec::with_capacity(capacity),
        assigned_ft: Vec::with_capacity(capacity),
        reported_ft: Vec::with_capacity(capacity),
        lat_e7: Vec::with_capacity(capacity),
        lon_e7: Vec::with_capacity(capacity),
        pos_time_ms: Vec::with_capacity(capacity),
        gs_dkt: Vec::with_capacity(capacity),
        vx_dkt: Vec::with_capacity(capacity),
        vy_dkt: Vec::with_capacity(capacity),
        aircraft_type: Vec::with_capacity(capacity),
        route_off: Vec::with_capacity(capacity),
        route_len: Vec::with_capacity(capacity),
        operator: Vec::with_capacity(capacity),
        beacon: Vec::with_capacity(capacity),
        ctrl: Vec::with_capacity(capacity),
        recv: Vec::with_capacity(capacity),
        xfer: Vec::with_capacity(capacity),
        dep_actual_ms: Vec::with_capacity(capacity),
        arr_est_ms: Vec::with_capacity(capacity),
        msg_seq: Vec::with_capacity(capacity),
        plan_seq: Vec::with_capacity(capacity),
        strings: vec![0; string_bytes],
        strings_used: 0,
    }
}

/// Drops every row and the string table contents; capacity is unchanged.
pub fn clear_events(ev: &mut Events) {
    truncate_events(ev, 0, 0);
}

/// Keeps the first `len` rows and `strings_used` bytes of the string table.
pub(crate) fn truncate_events(ev: &mut Events, len: usize, strings_used: usize) {
    ev.len = len;
    ev.strings_used = strings_used;
    ev.present.truncate(len);
    ev.errors.truncate(len);
    ev.flags.truncate(len);
    ev.source.truncate(len);
    ev.centre.truncate(len);
    ev.ts_ms.truncate(len);
    ev.gufi.truncate(len);
    ev.callsign.truncate(len);
    ev.computer_id.truncate(len);
    ev.plan_id.truncate(len);
    ev.departure.truncate(len);
    ev.arrival.truncate(len);
    ev.status.truncate(len);
    ev.flight_type.truncate(len);
    ev.assigned_ft.truncate(len);
    ev.reported_ft.truncate(len);
    ev.lat_e7.truncate(len);
    ev.lon_e7.truncate(len);
    ev.pos_time_ms.truncate(len);
    ev.gs_dkt.truncate(len);
    ev.vx_dkt.truncate(len);
    ev.vy_dkt.truncate(len);
    ev.aircraft_type.truncate(len);
    ev.route_off.truncate(len);
    ev.route_len.truncate(len);
    ev.operator.truncate(len);
    ev.beacon.truncate(len);
    ev.ctrl.truncate(len);
    ev.recv.truncate(len);
    ev.xfer.truncate(len);
    ev.dep_actual_ms.truncate(len);
    ev.arr_est_ms.truncate(len);
    ev.msg_seq.truncate(len);
    ev.plan_seq.truncate(len);
}

/// The bit of `f` in a `present` or `errors` word.
pub fn field_bit(f: Field) -> u64 {
    1u64 << (f as u32)
}

/// Whether row `row` carries field `f`.
pub fn has_field(ev: &Events, row: usize, f: Field) -> bool {
    ev.present.get(row).is_some_and(|w| w & field_bit(f) != 0)
}

/// Whether field `f` of row `row` was present but malformed or duplicated.
pub fn field_error(ev: &Events, row: usize, f: Field) -> bool {
    ev.errors.get(row).is_some_and(|w| w & field_bit(f) != 0)
}

/// The text of a zero-padded code array (empty if it is not UTF-8, which parsed codes never
/// are: they are ASCII letters and digits).
pub fn code_str(code: &[u8]) -> &str {
    let n = code.iter().position(|&b| b == 0).unwrap_or(code.len());
    code.get(..n)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

/// The unit part of a `ctrl`, `recv` or `xfer` value.
pub fn unit_code(v: &[u8; 8]) -> &str {
    code_str(v.get(..4).unwrap_or(&[]))
}

/// The sector part of a `ctrl`, `recv` or `xfer` value.
pub fn sector_code(v: &[u8; 8]) -> &str {
    code_str(v.get(4..).unwrap_or(&[]))
}

/// The route text of row `row`, empty when the row has none.
pub fn route_text(ev: &Events, row: usize) -> &str {
    if !has_field(ev, row, Field::Route) {
        return "";
    }
    let (Some(&off), Some(&len)) = (ev.route_off.get(row), ev.route_len.get(row)) else {
        return "";
    };
    let a = off as usize;
    ev.strings
        .get(a..a.saturating_add(len as usize))
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

/// Ground track of row `row` in 0.1 degree (0 to 3599, clockwise from north), from the
/// velocity components; `None` when the row has no velocity or it is zero. Computed with
/// `f64::atan2` and rounded, so it can differ from another platform's libm only on exact
/// rounding ties of the last place, which the 0.1 degree rounding absorbs in practice.
pub fn track_ddeg(ev: &Events, row: usize) -> Option<u16> {
    if !has_field(ev, row, Field::Velocity) {
        return None;
    }
    let x = f64::from(*ev.vx_dkt.get(row)?);
    let y = f64::from(*ev.vy_dkt.get(row)?);
    if x == 0.0 && y == 0.0 {
        return None;
    }
    let mut deg = x.atan2(y).to_degrees();
    if deg < 0.0 {
        deg += 360.0;
    }
    let t = (deg * 10.0).round() as u32;
    u16::try_from(t % 3600).ok()
}

/// The display text of an unknown-path entry: element paths are `a/b/c` from the `message`
/// element's parent, attribute paths end in `@name`.
pub fn unknown_text(u: &UnknownPath) -> &str {
    code_str_len(&u.text, usize::from(u.len))
}

fn code_str_len(b: &[u8], n: usize) -> &str {
    b.get(..n)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

//! The output of the decoder: one flight row per flight data message (and per flight of a TMI
//! flight list) and one flow row per flow information message, in a caller-owned
//! struct-of-arrays. Plain data (RULESET A1, A3): [`FlightRow`] and [`FlowRow`] are the staged
//! records, [`Flights`] and [`Flows`] the columns; behaviour is in the free functions below.
//!
//! Absent values: a name column is all zero bytes, a time column is [`NO_TIME`], a number or
//! coordinate column is [`NO_NUM`] or [`NO_COORD`], a code column is 0, and a text reference is
//! the empty [`StrRef`]. Free text (route, remarks, advisory text) lives in one bounded string
//! table owned by [`Rows`]; a row holds an offset and a length into it.
//!
//! The column definitions come from one `soa!` list per row type, so the record, its blank
//! value, the columns and the push, truncate and rebuild functions cannot drift apart. The macro
//! expands only to a struct definition and straight-line code (RULESET F3: no hidden control
//! flow).

/// Short ASCII names, zero-padded on the right; all zero means absent. Widths come from the
/// longest value in the 4,237-message survey (docs/TFMS_SURVEY.md) plus headroom; a longer value
/// is a field error, never truncated.
pub type Name4 = [u8; 4];
pub type Name8 = [u8; 8];
pub type Name12 = [u8; 12];
pub type Name16 = [u8; 16];

/// Absent time: Unix milliseconds can never be this.
pub const NO_TIME: i64 = i64::MIN;
/// Absent altitude, speed or other number.
pub const NO_NUM: i32 = i32::MIN;
/// Absent latitude or longitude (1e-7 degree units).
pub const NO_COORD: i32 = i32::MIN;
/// A word the decoder does not know in a vocabulary column (a trigger, a status, a category, a
/// time type, a program, a scope): the message is kept and the column holds this code, which
/// is never a valid index into the field's `NAMES`. The tri-state flags use
/// [`flag::UNKNOWN`].
pub const UNKNOWN_WORD: u8 = u8::MAX;

/// A piece of free text in the string table: byte offset and length. The empty reference
/// (length 0) means absent.
// Budget: three per flight row and five per flow row; two u32.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct StrRef {
    pub off: u32,
    pub len: u32,
}
const _: () = assert!(core::mem::size_of::<StrRef>() == 8);

/// The empty text reference.
pub const NO_STR: StrRef = StrRef { off: 0, len: 0 };

/// Flight message kinds ([`FlightRow::kind`]): the `msgType` of a flight data message, or the
/// TMI flight list. 0 is a message type the decoder does not know (identity columns only).
pub mod kind {
    pub const OTHER: u8 = 0;
    pub const TRACK: u8 = 1;
    pub const FLIGHT_PLAN: u8 = 2;
    pub const AMENDMENT: u8 = 3;
    pub const DEPARTURE: u8 = 4;
    pub const ARRIVAL: u8 = 5;
    pub const CANCELLATION: u8 = 6;
    pub const BOUNDARY: u8 = 7;
    pub const OCEANIC: u8 = 8;
    pub const CREATE: u8 = 9;
    pub const MODIFY: u8 = 10;
    pub const TIMES: u8 = 11;
    pub const ROUTE: u8 = 12;
    pub const SECTORS: u8 = 13;
    pub const ACTIVATE: u8 = 14;
    pub const TMI_LIST: u8 = 15;
    pub const COUNT: usize = 16;
    /// Display names, indexed by kind.
    pub const NAMES: [&str; COUNT] = [
        "other",
        "trackInformation",
        "flightPlanInformation",
        "flightPlanAmendmentInformation",
        "departureInformation",
        "arrivalInformation",
        "flightPlanCancellation",
        "boundaryCrossingUpdate",
        "oceanicReport",
        "FlightCreate",
        "FlightModify",
        "FlightTimes",
        "FlightRoute",
        "FlightSectors",
        "FlightScheduleActivate",
        "TMI_FLIGHT_LIST",
    ];
}

/// Flow message kinds ([`FlowRow::kind`]).
pub mod fkind {
    pub const RSTR: u8 = 1;
    pub const GADV: u8 = 2;
    pub const FXA: u8 = 3;
    pub const APTC: u8 = 4;
    pub const COUNT: usize = 5;
    /// Display names, indexed by kind (index 0 unused).
    pub const NAMES: [&str; COUNT] = ["?", "RSTR", "GADV", "FXA", "APTC"];
}

/// Program types of a flow row ([`FlowRow::program`]). A restriction (`RSTR`) is typed by its
/// `restrictionCategory`, an FCA or FEA by its `feaFcaType`, an advisory (`GADV`) by the words
/// of its title, an airport configuration (`APTC`) is its own type.
pub mod prog {
    pub const NONE: u8 = 0;
    pub const GROUND_DELAY: u8 = 1;
    pub const GROUND_STOP: u8 = 2;
    pub const REROUTE: u8 = 3;
    pub const AIRSPACE_FLOW: u8 = 4;
    pub const MIT: u8 = 5;
    pub const MINIT: u8 = 6;
    pub const STOP: u8 = 7;
    pub const APREQ: u8 = 8;
    pub const TBM: u8 = 9;
    pub const FEA: u8 = 10;
    pub const FCA: u8 = 11;
    pub const AIRPORT_CONFIG: u8 = 12;
    pub const ADVISORY: u8 = 13;
    pub const COUNT: usize = 14;
    pub const NAMES: [&str; COUNT] = [
        "none",
        "ground-delay-program",
        "ground-stop",
        "reroute",
        "airspace-flow-program",
        "MIT",
        "MINIT",
        "STOP",
        "APREQ",
        "TBM",
        "FEA",
        "FCA",
        "airport-config",
        "advisory",
    ];
}

/// `scope` of a flow row (`restrictionType`).
pub mod scope {
    pub const ARRIVAL: u8 = 1;
    pub const DEPARTURE: u8 = 2;
    pub const ENROUTE: u8 = 3;
}

/// `status` of a flow row (`tmiStatus` of an FCA).
pub mod fstatus {
    pub const NEW: u8 = 1;
    pub const UPDATED: u8 = 2;
}

/// Bits of [`FlowRow::flags`].
pub mod fflag {
    /// The advisory title says cancellation (`CNX` or `CANCELLATION`).
    pub const CANCEL: u8 = 1;
}

/// `status` of a flight row (`flightStatus`, or the status of a TMI flight list entry).
pub mod status {
    pub const PLANNED: u8 = 1;
    pub const ACTIVE: u8 = 2;
    pub const COMPLETED: u8 = 3;
    pub const CANCELED: u8 = 4;
    pub const NAMES: [&str; 5] = ["-", "PLANNED", "ACTIVE", "COMPLETED", "CANCELED"];
}

/// Time types (`etaType`, `etdType`, `coordinationTime@type`).
pub mod ttype {
    pub const ESTIMATED: u8 = 1;
    pub const ACTUAL: u8 = 2;
    pub const SCHEDULED: u8 = 3;
    pub const PROPOSED: u8 = 4;
    pub const METERED: u8 = 5;
    pub const NAMES: [&str; 6] = [
        "-",
        "ESTIMATED",
        "ACTUAL",
        "SCHEDULED",
        "PROPOSED",
        "METERED",
    ];
}

/// `diversionIndicator` codes.
pub mod divert {
    pub const NONE: u8 = 1;
    pub const AIRBORNE: u8 = 2;
    pub const GROUND: u8 = 3;
    pub const NAMES: [&str; 4] = ["-", "NO_DIVERSION", "AIRBORN_NOCTL", "GROUND_NOCTL"];
}

/// Aircraft category and engine class codes.
pub mod accat {
    pub const JET: u8 = 1;
    pub const TURBO: u8 = 2;
    pub const PISTON: u8 = 3;
    pub const OTHER: u8 = 4;
    pub const NAMES: [&str; 5] = ["-", "JET", "TURBO", "PISTON", "OTHER"];
}

/// `userCategory` codes.
pub mod ucat {
    pub const COMMERCIAL: u8 = 1;
    pub const GENERAL: u8 = 2;
    pub const AIR_TAXI: u8 = 3;
    pub const CARGO: u8 = 4;
    pub const MILITARY: u8 = 5;
    pub const OTHER: u8 = 6;
    pub const NAMES: [&str; 7] = [
        "-",
        "COMMERCIAL",
        "GENERAL AVIATION",
        "AIR TAXI",
        "CARGO",
        "MILITARY",
        "OTHER",
    ];
}

/// `tmi@updateType` codes.
pub mod tmiu {
    pub const UPDATE: u8 = 1;
    pub const NEW: u8 = 2;
    pub const DROPOUT: u8 = 3;
}

/// `fdTrigger` codes: the event inside TFMS that produced the message (meanings unconfirmed).
pub mod trig {
    pub const NAMES: [&str; 19] = [
        "-",
        "HCS_TRACK_MSG",
        "FD_FLIGHT_CREATE_MSG",
        "FD_FLIGHT_MODIFY_MSG",
        "ACTIVATE_SCHEDULED_FLIGHTS",
        "AIRSPACE_ASSIGNMENT",
        "UPDATE_DEPARTURE_TIMEOUT",
        "HCS_AMENDMENT_MSG",
        "NEMS_TBFM_FLT_DEPARTURE_MSG",
        "HCS_ARRIVAL_MSG",
        "UPDATE_COMPLETE_TIMEOUT",
        "HCS_UPDATE_MSG",
        "HCS_FLIGHT_PLAN_MSG",
        "HCS_DEPARTURE_MSG",
        "FD_FLIGHT_CANCEL_MSG",
        "UPDATE_INTERNATIONAL_CANCEL_TIMEOUT",
        "HCS_CANCELLATION_MSG",
        "CANCEL_CMD",
        "HCS_OCEANIC_POSITION_REPORT_MSG",
    ];
}

/// Tri-state flag positions in [`FlightRow::flags`]: two bits each, 0 absent, 1 `false`,
/// 2 `true`, 3 a word that is neither (kept as unknown); and one plain bit.
pub mod flag {
    pub const CDM_PART: u8 = 0;
    pub const TOD_EST: u8 = 2;
    pub const TOA_EST: u8 = 4;
    pub const RVSM_EQ: u8 = 6;
    pub const RVSM_CUR: u8 = 8;
    pub const RVSM_FUT: u8 = 10;
    pub const ABSENT: u16 = 0;
    pub const FALSE: u16 = 1;
    pub const TRUE: u16 = 2;
    pub const UNKNOWN: u16 = 3;
    /// The route text came from `routeOfFlight@legacyFormat` (bit 12), not from the element
    /// text of `ncsmRouteData/routeOfFlight`.
    pub const ROUTE_LEGACY: u16 = 1 << 12;
    /// The message carried a `flightRef` (bit 13); without it `flight_ref` is 0 and means
    /// nothing, and a carried reference of 0 is a real one.
    pub const FLIGHT_REF: u16 = 1 << 13;
}

/// The unit of a stored speed ([`FlightRow::speed_unit`], [`FlightRow::tas_unit`]). A whole
/// number or `ddd.d` is knots (the survey's reading for `speed` and `filedTrueAirSpeed`), `d.dd`
/// and the `Mddd` form are Mach numbers; a value of any other shape is kept with unit
/// [`UNKNOWN`](spd_unit::UNKNOWN) and counted, never guessed.
pub mod spd_unit {
    /// No speed in the row.
    pub const NONE: u8 = 0;
    pub const KNOTS: u8 = 1;
    pub const MACH: u8 = 2;
    pub const UNKNOWN: u8 = 3;
    pub const COUNT: usize = 4;
    pub const NAMES: [&str; COUNT] = ["-", "knots", "mach", "unknown"];
}

/// Declares a row record and its columns from one field list: the `repr(C)` record, its blank
/// value, the columns struct, and the new, length, truncate, push and rebuild functions.
macro_rules! soa {
    (
        row $row:ident, cols $cols:ident, blank $blank:ident, len_of $len_col:ident,
        new $new:ident, len $len:ident, truncate $trunc:ident, push $push:ident, at $at:ident;
        $( $(#[$m:meta])* $f:ident : $t:ty = $d:expr ),+ $(,)?
    ) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(C)]
        pub struct $row { $( $(#[$m])* pub $f: $t ),+ }

        /// A row with every value absent.
        pub const $blank: $row = $row { $( $f: $d ),+ };

        /// The columns: one `Vec` per field, sharing an index.
        // A7.2: cold struct, size guard waived (one per decoder; the hot record is the row)
        #[derive(Clone, Debug)]
        pub struct $cols {
            /// Rows the columns can hold; every column's allocation is reserved up front.
            pub cap: usize,
            $( $(#[$m])* pub $f: Vec<$t> ),+
        }

        /// Empty columns able to hold `cap` rows.
        pub fn $new(cap: usize) -> $cols {
            $cols { cap, $( $f: Vec::with_capacity(cap) ),+ }
        }

        /// Number of rows held.
        pub fn $len(c: &$cols) -> usize {
            c.$len_col.len()
        }

        /// Keeps the first `n` rows.
        pub fn $trunc(c: &mut $cols, n: usize) {
            $( c.$f.truncate(n); )+
        }

        /// Appends `r`; false (and nothing written) when the columns are at capacity.
        pub fn $push(c: &mut $cols, r: &$row) -> bool {
            if c.$len_col.len() >= c.cap {
                return false;
            }
            $( c.$f.push(r.$f); )+
            true
        }

        /// Row `i` rebuilt from the columns; `None` when out of range.
        pub fn $at(c: &$cols, i: usize) -> Option<$row> {
            Some($row { $( $f: *c.$f.get(i)? ),+ })
        }
    };
}

soa! {
    row FlightRow, cols Flights, blank BLANK_FLIGHT, len_of kind,
    new new_flights, len flights_len, truncate truncate_flights, push push_flight, at flight_at;
    /// `sourceTimeStamp` of the message (whole seconds), Unix ms. A TMI flight list entry
    /// carries the one of its `fiMessage`.
    src_ms: i64 = NO_TIME,
    /// `igtd`, an initial departure time of the flight (meaning unconfirmed).
    igtd_ms: i64 = NO_TIME,
    /// `eta@timeValue`; the type is `eta_type`.
    eta_ms: i64 = NO_TIME,
    /// `etd@timeValue`; the type is `etd_type`.
    etd_ms: i64 = NO_TIME,
    /// `timeAtPosition` of a track message, or the time of an oceanic reported position.
    pos_ms: i64 = NO_TIME,
    /// `flightTimeData@airlineOutTime` (gate out).
    out_ms: i64 = NO_TIME,
    /// `flightTimeData@airlineOffTime` (wheels off).
    off_ms: i64 = NO_TIME,
    /// `flightTimeData@airlineOnTime` (wheels on).
    on_ms: i64 = NO_TIME,
    /// `flightTimeData@airlineInTime` (gate in).
    in_ms: i64 = NO_TIME,
    /// `flightTimeData@runwayDeparture`.
    rwy_dep_ms: i64 = NO_TIME,
    /// `flightTimeData@runwayArrival`.
    rwy_arr_ms: i64 = NO_TIME,
    /// `flightTimeData@gateDeparture`.
    gate_dep_ms: i64 = NO_TIME,
    /// `flightTimeData@gateArrival`.
    gate_arr_ms: i64 = NO_TIME,
    /// `flightTimeData@flightCreation`.
    created_ms: i64 = NO_TIME,
    /// `flightTimeData@originalDeparture`: the departure time the flight was first given, so
    /// a departure delay is `etd` (or an actual time) minus this.
    orig_dep_ms: i64 = NO_TIME,
    /// `flightTimeData@originalArrival`.
    orig_arr_ms: i64 = NO_TIME,
    /// `timeOfDeparture` (departure messages); `flag::TOD_EST` says whether it is an estimate.
    tod_ms: i64 = NO_TIME,
    /// `timeOfArrival`; `flag::TOA_EST`.
    toa_ms: i64 = NO_TIME,
    /// `coordinationTime` or `newCoordinationTime`; the type is `coord_type`.
    coord_ms: i64 = NO_TIME,
    /// `boundaryPosition@boundaryCrossingTime`.
    bnd_ms: i64 = NO_TIME,
    /// `arrivalFixAndTime@arrTime`, the time at the arrival fix `arr_fix`.
    arrfix_ms: i64 = NO_TIME,
    /// `departureFixAndTime@arrTime`, the time at the departure fix `dep_fix`.
    depfix_ms: i64 = NO_TIME,
    /// `tmi@lastUpdateTime` of a TMI flight list entry.
    tmi_ms: i64 = NO_TIME,
    /// Route text: `routeOfFlight@legacyFormat` when present, else `ncsmRouteData/routeOfFlight`.
    route: StrRef = NO_STR,
    /// `remarksKeywords`.
    remarks: StrRef = NO_STR,
    /// `tmi/fcaId` of a TMI flight list entry (the first one).
    fca: StrRef = NO_STR,
    /// `flightRef` (a flight reference number, nine digits in the sample, ten allowed). Zero is a
    /// valid reference ("0000000000"); whether the message carried one is `flag::FLIGHT_REF`.
    flight_ref: u64 = 0,
    /// Latitude and longitude, 1e-7 degree (from degrees, minutes and optional seconds:
    /// rounded to the nearest unit).
    lat_e7: i32 = NO_COORD,
    lon_e7: i32 = NO_COORD,
    /// `nextPosition` or `nextEvent` (`latitudeDecimal`, `longitudeDecimal`), 1e-7 degree.
    next_lat_e7: i32 = NO_COORD,
    next_lon_e7: i32 = NO_COORD,
    /// Reported altitude, feet (the survey reads the text as hundreds of feet; unconfirmed).
    alt_ft: i32 = NO_NUM,
    /// Assigned altitude (`assignedAltitude`), feet.
    asg_ft: i32 = NO_NUM,
    /// Requested altitude, feet.
    req_ft: i32 = NO_NUM,
    /// `speed` of a track or oceanic message, in thousandths of the unit in `speed_unit`.
    speed_milli: i32 = NO_NUM,
    /// `filedTrueAirSpeed`, in thousandths of the unit in `tas_unit`.
    tas_milli: i32 = NO_NUM,
    /// Fix and sector counts are not stored; this counts the `fxaFlight` entries of a TMI
    /// flight list entry.
    n_fxa: u16 = 0,
    /// Tri-state and plain flags, see [`flag`].
    flags: u16 = 0,
    /// Message kind, see [`kind`].
    kind: u8 = kind::OTHER,
    /// `fdTrigger`, an index into [`trig::NAMES`].
    trigger: u8 = 0,
    /// `flightStatus`, see [`status`].
    status: u8 = 0,
    diversion: u8 = 0,
    user_cat: u8 = 0,
    ac_cat: u8 = 0,
    eng_class: u8 = 0,
    /// `equipmentQualifier`, one upper-case letter, or 0.
    equip: u8 = 0,
    eta_type: u8 = 0,
    etd_type: u8 = 0,
    coord_type: u8 = 0,
    /// What `speed_milli` is, see [`spd_unit`]; the value is never read without it.
    speed_unit: u8 = spd_unit::NONE,
    /// What `tas_milli` is, see [`spd_unit`].
    tas_unit: u8 = spd_unit::NONE,
    /// The letter after a reported altitude (`C` on about half the track messages; meaning
    /// unconfirmed), or 0.
    alt_sfx: u8 = 0,
    tmi_update: u8 = 0,
    acid: Name8 = [0; 8],
    /// A 10-character flight identifier, the same value SFDPS carries as `flightPlan@identifier`.
    gufi: Name16 = [0; 16],
    airline: Name4 = [0; 4],
    major: Name4 = [0; 4],
    dep: Name8 = [0; 8],
    arr: Name8 = [0; 8],
    /// `sourceFacility` of the message.
    src_fac: Name8 = [0; 8],
    /// `computerId/facilityIdentifier` and `idNumber`.
    cid_fac: Name8 = [0; 8],
    cid_num: Name8 = [0; 8],
    ac_type: Name8 = [0; 8],
    arr_fix: Name8 = [0; 8],
    dep_fix: Name8 = [0; 8],
    /// `dp@routeName` and `star@routeName`.
    dp: Name8 = [0; 8],
    star: Name8 = [0; 8],
    /// `coordinationPoint` fix name.
    coord_fix: Name8 = [0; 8],
    /// `specialAircraftQualifier` (words, spaces allowed).
    special: Name16 = [0; 16],
}
// Budget: one per flight message, held in quantity; the exact packed size of its fields
// (the columns above store the same fields). A7.1: the name widths were chosen so that no
// padding is hidden. `flight_ref` grew from u32 to u64 (a ten-digit reference overflows u32);
// placed on an 8-byte boundary it fits the four bytes the columns after it left free, so the
// size is unchanged.
const _: () = assert!(core::mem::size_of::<FlightRow>() == 408);

soa! {
    row FlowRow, cols Flows, blank BLANK_FLOW, len_of kind,
    new new_flows, len flows_len, truncate truncate_flows, push push_flow, at flow_at;
    /// `sourceTimeStamp` of the message.
    src_ms: i64 = NO_TIME,
    /// Start of the restriction, FCA or advisory period (`startTime`).
    start_ms: i64 = NO_TIME,
    /// End of it (`stopTime`, `endTime`).
    end_ms: i64 = NO_TIME,
    /// When it was entered or last changed (`eventTime`, `dateSent`, `lastUpdate`).
    event_ms: i64 = NO_TIME,
    /// Main free text: `reasonText`, `advisoryText`, `fcaReason`, `weather`.
    text: StrRef = NO_STR,
    /// A title or identifier: `qualifier`, `advisoryTitle`, `fcaId`, `arrRunwayConf`.
    title: StrRef = NO_STR,
    /// The element: `restrictedNasElements`, `facilities`, `fcaName`, the airport of an
    /// airport configuration.
    element: StrRef = NO_STR,
    /// Airports: `airports` of a restriction, `nas/airportName` of an FCA.
    airports: StrRef = NO_STR,
    /// `remarks`.
    remarks: StrRef = NO_STR,
    /// `restrictionId`.
    id: u64 = 0,
    /// `referenceRestrictionId`.
    ref_id: u64 = 0,
    /// `mitNumber` of a restriction, `advisoryNumber` of an advisory.
    number: u64 = 0,
    /// Miles in trail of a restriction (`mitValue`), the arrival rate of an airport
    /// configuration (`arrRate`).
    rate: i32 = NO_NUM,
    /// `depRate`.
    rate_dep: i32 = NO_NUM,
    /// `stratAar`.
    rate_strat: i32 = NO_NUM,
    /// `action`, a one or two digit code (meaning unconfirmed).
    action: u16 = 0,
    kind: u8 = 0,
    /// Program type, see [`prog`].
    program: u8 = prog::NONE,
    /// `restrictionType`, see [`scope`].
    scope: u8 = 0,
    /// `tmiStatus` of an FCA, see [`fstatus`].
    status: u8 = 0,
    /// See [`fflag`].
    flags: u8 = 0,
    /// `sourceFacility` of the message.
    facility: Name8 = [0; 8],
    /// `origin` of an advisory.
    origin: Name16 = [0; 16],
}
// Budget: one per flow information message; the packed size of its fields. A7.1: `id`, `ref_id`
// and `number` grew from u32 to u64 (ten-digit identifiers overflow u32), which takes the 128
// packed bytes to 140 and the size to 144, four bytes of tail padding.
const _: () = assert!(core::mem::size_of::<FlowRow>() == 144);

/// The rows and the text table, owned by the caller. Created once by [`new_rows`] with stated
/// capacities (RULESET C2) and reused after [`clear_rows`]; every column and the text table are
/// allocated up front, so a push within capacity never allocates (C1). Reaching a capacity is
/// the [`crate::DocError::Capacity`] reject, never a reallocation.
// A7.2: cold struct, size guard waived (one per decoder)
#[derive(Clone, Debug)]
pub struct Rows {
    pub flights: Flights,
    pub flows: Flows,
    /// The text table: `strings.len()` is its capacity, `strings_used` bytes are in use.
    pub strings: Vec<u8>,
    pub strings_used: usize,
}

/// Flight rows per document the decoder is sized for by default. Analysis: the 4,237 sampled
/// documents hold at most 20 flight messages each; a message needs at least a few hundred bytes
/// and a document is at most 1 MiB (the parser limit), so 256 is 12 times the observed peak and
/// a small allocation (about 100 KB).
pub const DEFAULT_FLIGHT_CAPACITY: usize = 256;
/// Flow rows per document: the sample has one flow message per document; 64 is generous.
pub const DEFAULT_FLOW_CAPACITY: usize = 64;
/// Text bytes per document: the longest flow text in the sample is 2,679 bytes (one advisory)
/// and the longest route 158 bytes; 256 KiB holds a whole 1 MiB document's worth of the
/// largest kinds with room to spare and is still a small allocation.
pub const DEFAULT_STRING_BYTES: usize = 256 * 1024;

/// Allocates empty tables with room for `flights` flight rows, `flows` flow rows and
/// `string_bytes` bytes of text.
pub fn new_rows(flights: usize, flows: usize, string_bytes: usize) -> Rows {
    Rows {
        flights: new_flights(flights),
        flows: new_flows(flows),
        strings: vec![0; string_bytes],
        strings_used: 0,
    }
}

/// Drops every row and the text, keeping the allocations.
pub fn clear_rows(r: &mut Rows) {
    truncate_rows(r, 0, 0, 0);
}

/// Keeps the first `flights` flight rows, `flows` flow rows and `strings_used` text bytes.
pub fn truncate_rows(r: &mut Rows, flights: usize, flows: usize, strings_used: usize) {
    truncate_flights(&mut r.flights, flights);
    truncate_flows(&mut r.flows, flows);
    r.strings_used = r.strings_used.min(strings_used);
}

/// The text a reference points at; empty when the reference is empty or lies outside the used
/// part of the table (the table holds only valid UTF-8 text, checked on decode).
pub fn text(r: &Rows, s: StrRef) -> &str {
    let a = s.off as usize;
    let Some(b) = a.checked_add(s.len as usize) else {
        return "";
    };
    if b > r.strings_used {
        return "";
    }
    r.strings
        .get(a..b)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

/// The text of a name column value: the bytes up to the first zero, as `&str` (names are
/// printable ASCII, checked on decode; anything else shows as `?`).
pub fn name_str(n: &[u8]) -> &str {
    let end = n.iter().position(|&b| b == 0).unwrap_or(n.len());
    n.get(..end)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("?")
}

/// The value of a tri-state flag ([`flag`]) in `flags`: 0 absent, 1 false, 2 true.
pub fn flag_get(flags: u16, pos: u8) -> u16 {
    (flags >> pos) & 3
}

//! Typed, pointer-free projection of the existing FAA CIFP decoder.
#![forbid(unsafe_code)]

use oa_arinc424 as cifp;
use std::mem::{align_of, offset_of, size_of};

pub const MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_PHYSICAL_LINES: usize = 600_000;
pub const MAX_SOURCE_BYTES: usize = 256;
pub const MAX_FLAT_OUTPUT_BYTES: usize = 768 * 1024 * 1024;
pub const NO_OFFSET: u64 = u64::MAX;

pub const PRESENT_IATA: u64 = 1 << 0;
pub const PRESENT_LONGEST_RUNWAY: u64 = 1 << 1;
pub const PRESENT_MAGVAR: u64 = 1 << 2;
pub const PRESENT_ELEVATION: u64 = 1 << 3;
pub const PRESENT_TRANSITION_ALT: u64 = 1 << 4;
pub const PRESENT_TRANSITION_LEVEL: u64 = 1 << 5;
pub const PRESENT_PUBLIC_MILITARY: u64 = 1 << 6;
pub const PRESENT_NORTH_REFERENCE: u64 = 1 << 7;
pub const PRESENT_BEARING: u64 = 1 << 8;
pub const PRESENT_ELLIPSOID_HEIGHT: u64 = 1 << 9;
pub const PRESENT_THRESHOLD_ELEVATION: u64 = 1 << 10;
pub const PRESENT_TCH: u64 = 1 << 11;
pub const PRESENT_WIDTH: u64 = 1 << 12;
pub const PRESENT_LOCALIZER_IDENT: u64 = 1 << 13;
pub const PRESENT_LOCALIZER_CATEGORY: u64 = 1 << 14;
pub const PRESENT_GLIDE_POSITION: u64 = 1 << 15;
pub const PRESENT_GLIDE_ANGLE: u64 = 1 << 16;
pub const PRESENT_STATION_DECLINATION: u64 = 1 << 17;
pub const PRESENT_GLIDE_ELEVATION: u64 = 1 << 18;
pub const PRESENT_AIRPORT_IDENT: u64 = 1 << 19;
pub const PRESENT_AIRPORT_REGION: u64 = 1 << 20;
pub const PRESENT_FREQUENCY: u64 = 1 << 21;
pub const PRESENT_VOR_POSITION: u64 = 1 << 22;
pub const PRESENT_DME_IDENT: u64 = 1 << 23;
pub const PRESENT_DME_POSITION: u64 = 1 << 24;
pub const PRESENT_DME_ELEVATION: u64 = 1 << 25;
pub const PRESENT_FIGURE_OF_MERIT: u64 = 1 << 26;
pub const PRESENT_POSITION: u64 = 1 << 27;
pub const PRESENT_USAGE: u64 = 1 << 28;
pub const PRESENT_WAYPOINT_TYPE: u64 = 1 << 29;
pub const PRESENT_ALTITUDE_1: u64 = 1 << 30;
pub const PRESENT_ALTITUDE_2: u64 = 1 << 31;
pub const PRESENT_LEVEL: u64 = 1 << 33;
pub const PRESENT_OUTBOUND_COURSE: u64 = 1 << 34;
pub const PRESENT_DISTANCE: u64 = 1 << 35;
pub const PRESENT_INBOUND_COURSE: u64 = 1 << 36;
pub const PRESENT_MAX_ALTITUDE: u64 = 1 << 37;
pub const PRESENT_FIX: u64 = 1 << 38;
pub const PRESENT_TURN_DIRECTION: u64 = 1 << 39;
pub const PRESENT_RNP: u64 = 1 << 40;
pub const PRESENT_RECOMMENDED_NAVAID: u64 = 1 << 41;
pub const PRESENT_ARC_RADIUS: u64 = 1 << 42;
pub const PRESENT_THETA: u64 = 1 << 43;
pub const PRESENT_RHO: u64 = 1 << 44;
pub const PRESENT_COURSE: u64 = 1 << 45;
pub const PRESENT_DISTANCE_OR_TIME: u64 = 1 << 46;
pub const PRESENT_ALT_DESC: u64 = 1 << 47;
pub const PRESENT_TRANSITION_ALTITUDE: u64 = 1 << 48;
pub const PRESENT_SPEED_LIMIT: u64 = 1 << 49;
pub const PRESENT_VERTICAL_ANGLE: u64 = 1 << 50;
pub const PRESENT_CENTER_FIX: u64 = 1 << 51;
pub const PRESENT_GNSS_FMS: u64 = 1 << 52;
pub const PRESENT_SPEED_DESC: u64 = 1 << 53;
pub const PRESENT_CLASSIFICATION: u64 = 1 << 54;
pub const PRESENT_TIME_CODE: u64 = 1 << 55;
pub const PRESENT_LOWER: u64 = 1 << 56;
pub const PRESENT_UPPER: u64 = 1 << 57;
pub const PRESENT_ARC_ORIGIN: u64 = 1 << 59;
pub const PRESENT_ARC_DISTANCE: u64 = 1 << 60;
pub const PRESENT_ARC_BEARING: u64 = 1 << 61;
pub const PRESENT_LONGEST_RUNWAY_SURFACE: u64 = 1 << 63;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpSpan {
    pub offset: u64,
    pub length: u64,
}
const _: () = assert!(size_of::<OaCifpSpan>() == 16);
const _: () = assert!(align_of::<OaCifpSpan>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpPosition {
    pub lat: f64,
    pub lon: f64,
}
const _: () = assert!(size_of::<OaCifpPosition>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpMeta {
    pub line: u64,
    pub cycle_year: u8,
    pub cycle_release: u8,
    pub reserved: [u8; 6],
    pub customer_area: OaCifpSpan,
    pub file_record_number: OaCifpSpan,
}
const _: () = assert!(size_of::<OaCifpMeta>() == 48);
const _: () = assert!(offset_of!(OaCifpMeta, customer_area) == 16);
const _: () = assert!(offset_of!(OaCifpMeta, file_record_number) == 32);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpAltitude {
    pub kind: u8,
    pub reserved: [u8; 3],
    pub value: i32,
}
const ALT_NONE: u8 = 0;
const ALT_FEET: u8 = 1;
const ALT_FL: u8 = 2;
const ALT_GROUND: u8 = 3;
const ALT_UNLIMITED: u8 = 4;
const ALT_UNKNOWN: u8 = 5;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpFixRef {
    pub ident: OaCifpSpan,
    pub region: OaCifpSpan,
    pub section: u8,
    pub subsection: u8,
    pub reserved: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpAirport {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub ident: OaCifpSpan,
    pub icao_region: OaCifpSpan,
    pub iata: OaCifpSpan,
    pub datum: OaCifpSpan,
    pub name: OaCifpSpan,
    pub position: OaCifpPosition,
    pub magnetic_variation: f64,
    pub elevation_ft: i32,
    pub longest_runway_ft: u32,
    pub transition_altitude: OaCifpAltitude,
    pub transition_level: OaCifpAltitude,
    pub ifr_capable: u8,
    pub longest_runway_surface: u8,
    pub public_military: u8,
    pub north_reference: u8,
    pub reserved: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpRunway {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub airport_ident: OaCifpSpan,
    pub airport_region: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub localizer_ident: OaCifpSpan,
    pub threshold: OaCifpPosition,
    pub length_ft: u32,
    pub displaced_threshold_ft: u32,
    pub magnetic_bearing: f64,
    pub ellipsoid_height_m: f64,
    pub threshold_elevation_ft: i32,
    pub threshold_crossing_height_ft: u32,
    pub width_ft: u32,
    pub tch_indicator: u8,
    pub localizer_category: u8,
    pub reserved: [u8; 6],
    pub reserved2: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpLocalizer {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub airport_ident: OaCifpSpan,
    pub airport_region: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub runway_ident: OaCifpSpan,
    pub localizer_position: OaCifpPosition,
    pub glide_slope_position: OaCifpPosition,
    pub category: u8,
    pub reserved0: [u8; 7],
    pub frequency_mhz: f64,
    pub bearing: f64,
    pub localizer_width_deg: f64,
    pub glide_slope_angle_deg: f64,
    pub station_declination: f64,
    pub glide_slope_tch_ft: u32,
    pub glide_slope_elevation_ft: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpVhfNavaid {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub airport_ident: OaCifpSpan,
    pub airport_region: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub icao_region: OaCifpSpan,
    pub dme_ident: OaCifpSpan,
    pub datum: OaCifpSpan,
    pub name: OaCifpSpan,
    pub vor_position: OaCifpPosition,
    pub dme_position: OaCifpPosition,
    pub frequency_mhz: f64,
    pub station_declination: f64,
    pub dme_elevation_ft: i32,
    pub class: [u8; 5],
    pub figure_of_merit: u8,
    pub reserved: [u8; 2],
    pub reserved2: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpNdb {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub airport_ident: OaCifpSpan,
    pub airport_region: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub icao_region: OaCifpSpan,
    pub class: OaCifpSpan,
    pub datum: OaCifpSpan,
    pub name: OaCifpSpan,
    pub position: OaCifpPosition,
    pub frequency_khz: f64,
    pub magnetic_variation: f64,
    pub terminal: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpWaypoint {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub area: OaCifpSpan,
    pub area_region: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub icao_region: OaCifpSpan,
    pub waypoint_type: OaCifpSpan,
    pub usage: OaCifpSpan,
    pub datum: OaCifpSpan,
    pub name: OaCifpSpan,
    pub position: OaCifpPosition,
    pub magnetic_variation: f64,
    pub kind: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpAirway {
    pub customer_area: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub first_line: u64,
    pub fix_start: u64,
    pub fix_count: u64,
    pub route_type: u8,
    pub level: u8,
    pub reserved: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpAirwayFix {
    pub meta: OaCifpMeta,
    pub route_ident: OaCifpSpan,
    pub fix: OaCifpFixRef,
    pub sequence: u32,
    pub description: [u8; 4],
    pub route_type: u8,
    pub level: u8,
    pub reserved0: [u8; 6],
    pub present: u64,
    pub outbound_course: f64,
    pub distance_nm: f64,
    pub inbound_course: f64,
    pub min_altitude: OaCifpAltitude,
    pub min_altitude_2: OaCifpAltitude,
    pub max_altitude: OaCifpAltitude,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpProcedure {
    pub customer_area: OaCifpSpan,
    pub airport_ident: OaCifpSpan,
    pub airport_region: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub first_line: u64,
    pub route_start: u64,
    pub route_count: u64,
    pub kind: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpRoute {
    pub transition: OaCifpSpan,
    pub first_line: u64,
    pub leg_start: u64,
    pub leg_count: u64,
    pub route_type: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpLeg {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub airport_ident: OaCifpSpan,
    pub airport_region: OaCifpSpan,
    pub ident: OaCifpSpan,
    pub transition: OaCifpSpan,
    pub rnp_code: OaCifpSpan,
    pub fix: OaCifpFixRef,
    pub recommended_navaid: OaCifpFixRef,
    pub center_fix: OaCifpFixRef,
    pub sequence: u32,
    pub description: [u8; 4],
    pub kind: u8,
    pub route_type: u8,
    pub turn_direction: u8,
    pub path_terminator: u8,
    pub altitude_description: u8,
    pub gnss_fms_indication: u8,
    pub speed_limit_description: u8,
    pub reserved0: u8,
    pub arc_radius_nm: f64,
    pub theta_deg: f64,
    pub rho_nm: f64,
    pub course_deg: f64,
    pub distance_or_time_value: f64,
    pub distance_or_time_kind: u8,
    pub reserved1: [u8; 7],
    pub altitude1: OaCifpAltitude,
    pub altitude2: OaCifpAltitude,
    pub transition_altitude: OaCifpAltitude,
    pub speed_limit_kt: u32,
    pub reserved2: [u8; 4],
    pub vertical_angle_deg: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpContinuation {
    pub meta: OaCifpMeta,
    pub owner_index: u64,
    pub text: OaCifpSpan,
    pub number: u8,
    pub application_type: u8,
    pub owner_kind: u8,
    pub reserved: [u8; 5],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpLevelOfService {
    pub continuation_index: u64,
    pub name: OaCifpSpan,
    pub authorized: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpAirspace {
    pub meta: OaCifpMeta,
    pub present: u64,
    pub icao_region: OaCifpSpan,
    pub designation: OaCifpSpan,
    pub name: OaCifpSpan,
    pub boundary_start: u64,
    pub boundary_count: u64,
    pub continuation_start: u64,
    pub continuation_count: u64,
    pub kind: u8,
    pub airspace_type: u8,
    pub classification: u8,
    pub multiple_code: u8,
    pub level: u8,
    pub time_code: u8,
    pub reserved: [u8; 2],
    pub lower: OaCifpAltitude,
    pub lower_unit: u8,
    pub upper_unit: u8,
    pub reserved2: [u8; 6],
    pub upper: OaCifpAltitude,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpBoundary {
    pub position: OaCifpPosition,
    pub arc_origin: OaCifpPosition,
    pub sequence: u32,
    pub via: u8,
    pub end: u8,
    pub present: u8,
    pub reserved: u8,
    pub arc_distance_nm: f64,
    pub arc_bearing_deg: f64,
    pub source_line: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OaCifpMora {
    pub meta: OaCifpMeta,
    pub start_lat: i16,
    pub start_lon: i16,
    pub present_mask: u32,
    pub cells_ft: [u32; 30],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpHeader {
    pub file_name: OaCifpSpan,
    pub version: OaCifpSpan,
    pub creation_date: OaCifpSpan,
    pub creation_time: OaCifpSpan,
    pub data_supplier: OaCifpSpan,
    pub crc: OaCifpSpan,
    pub record_length: u32,
    pub reserved0: [u8; 4],
    pub record_count: u64,
    pub cycle_year: u8,
    pub cycle_release: u8,
    pub production_flag: u8,
    pub crc_verified: u8,
    pub reserved: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpHeaderText {
    pub text: OaCifpSpan,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpSectionCount {
    pub section: OaCifpSpan,
    pub count: u64,
    pub supported: u8,
    pub reserved: [u8; 7],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpCycleCount {
    pub count: u64,
    pub year: u8,
    pub release: u8,
    pub reserved: [u8; 6],
}

pub const ERROR_LINE_TOO_LONG: u32 = 1;
pub const ERROR_TOO_MANY_RECORDS: u32 = 2;
pub const ERROR_TOO_MANY_CONTINUATIONS: u32 = 3;
pub const ERROR_TOO_MANY_IN_GROUP: u32 = 4;
pub const ERROR_TOO_MANY_HEADER_RECORDS: u32 = 5;
pub const ERROR_IO: u32 = 6;
pub const ERROR_WRONG_LENGTH: u32 = 7;
pub const ERROR_BAD_BYTE: u32 = 8;
pub const ERROR_BAD_RECORD_TYPE: u32 = 9;
pub const ERROR_FIELD: u32 = 10;
pub const ERROR_MISSING_HEADER: u32 = 11;
pub const ERROR_HEADER_AFTER_DATA: u32 = 12;
pub const ERROR_BAD_HEADER: u32 = 13;
pub const ERROR_RECORD_COUNT_MISMATCH: u32 = 14;
pub const ERROR_CYCLE_AFTER_HEADER: u32 = 15;
pub const ERROR_ORPHAN_CONTINUATION: u32 = 16;
pub const ERROR_UNSUPPORTED_CONTINUATION: u32 = 17;
pub const ERROR_SEQUENCE_NOT_INCREASING: u32 = 18;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OaCifpError {
    pub status: u32,
    pub kind: u32,
    pub line: u64,
    pub first_column: u32,
    pub last_column: u32,
    pub limit: u64,
    pub actual: u64,
    pub previous: u64,
    pub found: u64,
    pub code: u32,
    pub flags: u32,
    pub field: [u8; 32],
    pub value: [u8; 132],
    pub reason: [u8; 64],
    pub reserved: [u8; 4],
}

impl Default for OaCifpError {
    fn default() -> Self {
        Self {
            status: 0,
            kind: 0,
            line: 0,
            first_column: 0,
            last_column: 0,
            limit: 0,
            actual: 0,
            previous: 0,
            found: 0,
            code: 0,
            flags: 0,
            field: [0; 32],
            value: [0; 132],
            reason: [0; 64],
            reserved: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCifpCounts {
    pub airports: u64,
    pub runways: u64,
    pub localizers: u64,
    pub vhf_navaids: u64,
    pub ndbs: u64,
    pub waypoints: u64,
    pub moras: u64,
    pub airspaces: u64,
    pub airways: u64,
    pub airway_fixes: u64,
    pub procedures: u64,
    pub routes: u64,
    pub legs: u64,
    pub continuations: u64,
    pub levels_of_service: u64,
    pub boundaries: u64,
    pub header_records: u64,
    pub sections: u64,
    pub cycles: u64,
    pub errors: u64,
    pub errors_dropped: u64,
    pub strings_bytes: u64,
    pub output_bytes: u64,
    pub is_clean: u8,
    pub reserved: [u8; 7],
}
const _: () = assert!(size_of::<OaCifpCounts>() == 192);

#[derive(Debug, Clone, PartialEq)]
pub struct CifpFlat {
    pub counts: OaCifpCounts,
    pub header: OaCifpHeader,
    pub header_records: Vec<OaCifpHeaderText>,
    pub airports: Vec<OaCifpAirport>,
    pub runways: Vec<OaCifpRunway>,
    pub localizers: Vec<OaCifpLocalizer>,
    pub vhf_navaids: Vec<OaCifpVhfNavaid>,
    pub ndbs: Vec<OaCifpNdb>,
    pub waypoints: Vec<OaCifpWaypoint>,
    pub moras: Vec<OaCifpMora>,
    pub airspaces: Vec<OaCifpAirspace>,
    pub airways: Vec<OaCifpAirway>,
    pub airway_fixes: Vec<OaCifpAirwayFix>,
    pub procedures: Vec<OaCifpProcedure>,
    pub routes: Vec<OaCifpRoute>,
    pub legs: Vec<OaCifpLeg>,
    pub continuations: Vec<OaCifpContinuation>,
    pub levels_of_service: Vec<OaCifpLevelOfService>,
    pub boundaries: Vec<OaCifpBoundary>,
    pub sections: Vec<OaCifpSectionCount>,
    pub cycles: Vec<OaCifpCycleCount>,
    pub errors: Vec<OaCifpError>,
    pub strings: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CifpFailure {
    InputTooLarge,
    TooManyLines,
    SourceTooLarge,
    OutputTooLarge,
    Allocation,
    Decode(Box<OaCifpError>),
}

#[derive(Default)]
struct Pool {
    bytes: Vec<u8>,
    max_bytes: usize,
}
impl Pool {
    fn with_limit(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
        }
    }
    fn put(&mut self, text: &str) -> Result<OaCifpSpan, CifpFailure> {
        let offset = u64::try_from(self.bytes.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
        let length = u64::try_from(text.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
        let end = self
            .bytes
            .len()
            .checked_add(text.len())
            .ok_or(CifpFailure::OutputTooLarge)?;
        if end > self.max_bytes {
            return Err(CifpFailure::OutputTooLarge);
        }
        self.bytes
            .try_reserve(text.len())
            .map_err(|_| CifpFailure::Allocation)?;
        self.bytes.extend_from_slice(text.as_bytes());
        Ok(OaCifpSpan { offset, length })
    }
    fn opt(&mut self, value: Option<&str>) -> Result<OaCifpSpan, CifpFailure> {
        value.map_or(
            Ok(OaCifpSpan {
                offset: NO_OFFSET,
                length: 0,
            }),
            |s| self.put(s),
        )
    }
}
fn meta(pool: &mut Pool, m: &cifp::types::Meta) -> Result<OaCifpMeta, CifpFailure> {
    Ok(OaCifpMeta {
        line: u64::try_from(m.line).map_err(|_| CifpFailure::OutputTooLarge)?,
        cycle_year: m.cycle.year,
        cycle_release: m.cycle.release,
        reserved: [0; 6],
        customer_area: pool.put(&m.customer_area)?,
        file_record_number: pool.put(&m.file_record_number)?,
    })
}
fn pos(p: cifp::types::Position) -> OaCifpPosition {
    OaCifpPosition {
        lat: p.lat,
        lon: p.lon,
    }
}
fn alt(a: Option<cifp::types::Altitude>) -> OaCifpAltitude {
    match a {
        None => OaCifpAltitude {
            kind: ALT_NONE,
            reserved: [0; 3],
            value: 0,
        },
        Some(cifp::types::Altitude::Feet(v)) => OaCifpAltitude {
            kind: ALT_FEET,
            reserved: [0; 3],
            value: v,
        },
        Some(cifp::types::Altitude::FlightLevel(v)) => OaCifpAltitude {
            kind: ALT_FL,
            reserved: [0; 3],
            value: i32::from(v),
        },
        Some(cifp::types::Altitude::Ground) => OaCifpAltitude {
            kind: ALT_GROUND,
            reserved: [0; 3],
            value: 0,
        },
        Some(cifp::types::Altitude::Unlimited) => OaCifpAltitude {
            kind: ALT_UNLIMITED,
            reserved: [0; 3],
            value: 0,
        },
        Some(cifp::types::Altitude::Unknown) => OaCifpAltitude {
            kind: ALT_UNKNOWN,
            reserved: [0; 3],
            value: 0,
        },
    }
}
fn fixref(pool: &mut Pool, f: Option<&cifp::types::FixRef>) -> Result<OaCifpFixRef, CifpFailure> {
    match f {
        None => Ok(OaCifpFixRef {
            ident: OaCifpSpan {
                offset: NO_OFFSET,
                length: 0,
            },
            region: OaCifpSpan {
                offset: NO_OFFSET,
                length: 0,
            },
            section: 0,
            subsection: 0,
            reserved: [0; 6],
        }),
        Some(x) => Ok(OaCifpFixRef {
            ident: pool.put(&x.ident)?,
            region: pool.put(&x.icao_region)?,
            section: x.section as u8,
            subsection: x.subsection as u8,
            reserved: [0; 6],
        }),
    }
}
fn optional_char(c: Option<char>, present: u64, flags: &mut u64) -> u8 {
    if let Some(v) = c {
        *flags |= present;
        v as u8
    } else {
        0
    }
}
fn alt_unit(v: Option<cifp::types::AltitudeUnit>) -> u8 {
    match v {
        None => 0,
        Some(cifp::types::AltitudeUnit::Agl) => 1,
        Some(cifp::types::AltitudeUnit::Msl) => 2,
        Some(cifp::types::AltitudeUnit::Other(c)) => c as u8,
    }
}
fn error(e: &cifp::Error) -> OaCifpError {
    let mut o = OaCifpError {
        line: u64::try_from(e.line).unwrap_or(u64::MAX),
        ..OaCifpError::default()
    };
    match &e.kind {
        cifp::ErrorKind::LineTooLong { limit } => {
            o.kind = ERROR_LINE_TOO_LONG;
            o.limit = *limit as u64;
        }
        cifp::ErrorKind::TooManyRecords { limit } => {
            o.kind = ERROR_TOO_MANY_RECORDS;
            o.limit = *limit as u64;
        }
        cifp::ErrorKind::TooManyContinuations { limit } => {
            o.kind = ERROR_TOO_MANY_CONTINUATIONS;
            o.limit = *limit as u64;
        }
        cifp::ErrorKind::TooManyInGroup { what, limit } => {
            o.kind = ERROR_TOO_MANY_IN_GROUP;
            o.limit = *limit as u64;
            copy_bytes(&mut o.field, what.as_bytes());
        }
        cifp::ErrorKind::TooManyHeaderRecords { limit } => {
            o.kind = ERROR_TOO_MANY_HEADER_RECORDS;
            o.limit = *limit as u64;
        }
        cifp::ErrorKind::Io(_) => o.kind = ERROR_IO,
        cifp::ErrorKind::WrongLength { len, expected } => {
            o.kind = ERROR_WRONG_LENGTH;
            o.actual = *len as u64;
            o.limit = *expected as u64;
        }
        cifp::ErrorKind::BadByte { column } => {
            o.kind = ERROR_BAD_BYTE;
            o.first_column = *column as u32;
            o.last_column = *column as u32;
        }
        cifp::ErrorKind::BadRecordType { found } => {
            o.kind = ERROR_BAD_RECORD_TYPE;
            o.code = *found as u32;
        }
        cifp::ErrorKind::Field {
            field,
            first_col,
            last_col,
            value,
            reason,
        } => {
            o.kind = ERROR_FIELD;
            o.first_column = *first_col as u32;
            o.last_column = *last_col as u32;
            copy_bytes(&mut o.field, field.as_bytes());
            copy_bytes(&mut o.value, value.as_bytes());
            copy_bytes(&mut o.reason, reason.as_bytes());
        }
        cifp::ErrorKind::MissingHeader => o.kind = ERROR_MISSING_HEADER,
        cifp::ErrorKind::HeaderAfterData => o.kind = ERROR_HEADER_AFTER_DATA,
        cifp::ErrorKind::BadHeader(why) => {
            o.kind = ERROR_BAD_HEADER;
            copy_bytes(&mut o.reason, why.as_bytes());
        }
        cifp::ErrorKind::RecordCountMismatch { declared, found } => {
            o.kind = ERROR_RECORD_COUNT_MISMATCH;
            o.actual = *found as u64;
            o.limit = *declared as u64;
        }
        cifp::ErrorKind::CycleAfterHeader { header, record } => {
            o.kind = ERROR_CYCLE_AFTER_HEADER;
            copy_bytes(&mut o.field, header.as_bytes());
            copy_bytes(&mut o.value, record.as_bytes());
        }
        cifp::ErrorKind::OrphanContinuation => o.kind = ERROR_ORPHAN_CONTINUATION,
        cifp::ErrorKind::UnsupportedContinuation { number } => {
            o.kind = ERROR_UNSUPPORTED_CONTINUATION;
            o.code = *number as u32;
        }
        cifp::ErrorKind::SequenceNotIncreasing { previous, found } => {
            o.kind = ERROR_SEQUENCE_NOT_INCREASING;
            o.previous = u64::from(*previous);
            o.found = u64::from(*found);
        }
    }
    o
}
fn copy_bytes<const N: usize>(out: &mut [u8; N], src: &[u8]) {
    let n = src.len().min(N);
    if let Some(dst) = out.get_mut(..n) {
        dst.copy_from_slice(src.get(..n).unwrap_or(&[]));
    }
}

fn preflight(input: &[u8], source: &str) -> Result<usize, CifpFailure> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(CifpFailure::InputTooLarge);
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(CifpFailure::SourceTooLarge);
    }
    let lines = input.iter().filter(|&&b| b == b'\n').count()
        + usize::from(!input.is_empty() && input.last() != Some(&b'\n'));
    if lines > MAX_PHYSICAL_LINES {
        return Err(CifpFailure::TooManyLines);
    }
    Ok(lines)
}

fn add_string_bytes(total: &mut usize, s: &str) -> Result<(), CifpFailure> {
    *total = total
        .checked_add(s.len())
        .ok_or(CifpFailure::OutputTooLarge)?;
    Ok(())
}

fn add_row_bytes(total: &mut usize, count: usize, row_size: usize) -> Result<(), CifpFailure> {
    let bytes = count
        .checked_mul(row_size)
        .ok_or(CifpFailure::OutputTooLarge)?;
    *total = total
        .checked_add(bytes)
        .ok_or(CifpFailure::OutputTooLarge)?;
    Ok(())
}

fn add_opt_string_bytes(total: &mut usize, s: Option<&str>) -> Result<(), CifpFailure> {
    if let Some(v) = s {
        add_string_bytes(total, v)?;
    }
    Ok(())
}
fn meta_bytes(total: &mut usize, m: &cifp::types::Meta) -> Result<(), CifpFailure> {
    add_string_bytes(total, &m.customer_area)?;
    add_string_bytes(total, &m.file_record_number)
}
fn fix_bytes(total: &mut usize, f: Option<&cifp::types::FixRef>) -> Result<(), CifpFailure> {
    if let Some(v) = f {
        add_string_bytes(total, &v.ident)?;
        add_string_bytes(total, &v.icao_region)?;
    }
    Ok(())
}
fn altitude_present(a: Option<cifp::types::Altitude>) -> bool {
    a.is_some()
}
fn plan_counts(c: &cifp::Cifp) -> Result<(OaCifpCounts, usize), CifpFailure> {
    let mut counts = OaCifpCounts::default();
    let airports = c.airports.len();
    let runways = c.runways.len();
    let localizers = c.localizers.len();
    let vhf_navaids = c.vhf_navaids.len();
    let ndbs = c
        .ndbs
        .len()
        .checked_add(c.terminal_ndbs.len())
        .ok_or(CifpFailure::OutputTooLarge)?;
    let waypoints = c
        .enroute_waypoints
        .len()
        .checked_add(c.terminal_waypoints.len())
        .ok_or(CifpFailure::OutputTooLarge)?;
    let moras = c.mora.len();
    let airspaces = c
        .controlled_airspace
        .len()
        .checked_add(c.special_use_airspace.len())
        .ok_or(CifpFailure::OutputTooLarge)?;
    let airway_fixes = c.airways.iter().try_fold(0usize, |n, a| {
        n.checked_add(a.fixes.len())
            .ok_or(CifpFailure::OutputTooLarge)
    })?;
    let (procedures, routes, legs) =
        c.procedures
            .iter()
            .try_fold((0usize, 0usize, 0usize), |(p, r, l), proc| {
                let nr = proc.routes.len();
                let nl = proc.routes.iter().try_fold(0usize, |n, x| {
                    n.checked_add(x.legs.len())
                        .ok_or(CifpFailure::OutputTooLarge)
                })?;
                Ok::<_, CifpFailure>((
                    p + 1,
                    r.checked_add(nr).ok_or(CifpFailure::OutputTooLarge)?,
                    l.checked_add(nl).ok_or(CifpFailure::OutputTooLarge)?,
                ))
            })?;
    let (boundaries, airspace_cont) = c
        .controlled_airspace
        .iter()
        .chain(&c.special_use_airspace)
        .try_fold((0usize, 0usize), |(b, cc), a| {
            Ok::<_, CifpFailure>((
                b.checked_add(a.boundary.len())
                    .ok_or(CifpFailure::OutputTooLarge)?,
                cc.checked_add(a.continuations.len())
                    .ok_or(CifpFailure::OutputTooLarge)?,
            ))
        })?;
    let leg_cont = c
        .procedures
        .iter()
        .flat_map(|p| p.routes.iter())
        .flat_map(|r| r.legs.iter())
        .try_fold(0usize, |n, l| {
            n.checked_add(l.continuations.len())
                .ok_or(CifpFailure::OutputTooLarge)
        })?;
    let continuations = leg_cont
        .checked_add(airspace_cont)
        .ok_or(CifpFailure::OutputTooLarge)?;
    let levels = c
        .procedures
        .iter()
        .flat_map(|p| p.routes.iter())
        .flat_map(|r| r.legs.iter())
        .flat_map(|l| l.continuations.iter())
        .filter_map(cifp::procedure::levels_of_service)
        .try_fold(0usize, |n, v| {
            n.checked_add(v.len()).ok_or(CifpFailure::OutputTooLarge)
        })?;
    let header_records = c.header.as_ref().map_or(0, |h| h.records.len());
    let errors = c.errors.len();
    // Fixed row bytes are known before any projection buffers are allocated.
    let mut fixed = 0usize;
    add_row_bytes(&mut fixed, airports, size_of::<OaCifpAirport>())?;
    add_row_bytes(&mut fixed, runways, size_of::<OaCifpRunway>())?;
    add_row_bytes(&mut fixed, localizers, size_of::<OaCifpLocalizer>())?;
    add_row_bytes(&mut fixed, vhf_navaids, size_of::<OaCifpVhfNavaid>())?;
    add_row_bytes(&mut fixed, ndbs, size_of::<OaCifpNdb>())?;
    add_row_bytes(&mut fixed, waypoints, size_of::<OaCifpWaypoint>())?;
    add_row_bytes(&mut fixed, moras, size_of::<OaCifpMora>())?;
    add_row_bytes(&mut fixed, airspaces, size_of::<OaCifpAirspace>())?;
    add_row_bytes(&mut fixed, c.airways.len(), size_of::<OaCifpAirway>())?;
    add_row_bytes(&mut fixed, airway_fixes, size_of::<OaCifpAirwayFix>())?;
    add_row_bytes(&mut fixed, procedures, size_of::<OaCifpProcedure>())?;
    add_row_bytes(&mut fixed, routes, size_of::<OaCifpRoute>())?;
    add_row_bytes(&mut fixed, legs, size_of::<OaCifpLeg>())?;
    add_row_bytes(&mut fixed, continuations, size_of::<OaCifpContinuation>())?;
    add_row_bytes(&mut fixed, levels, size_of::<OaCifpLevelOfService>())?;
    add_row_bytes(&mut fixed, boundaries, size_of::<OaCifpBoundary>())?;
    add_row_bytes(&mut fixed, header_records, size_of::<OaCifpHeaderText>())?;
    add_row_bytes(&mut fixed, c.counts.len(), size_of::<OaCifpSectionCount>())?;
    add_row_bytes(
        &mut fixed,
        c.cycle_counts.len(),
        size_of::<OaCifpCycleCount>(),
    )?;
    add_row_bytes(&mut fixed, errors, size_of::<OaCifpError>())?;
    add_row_bytes(&mut fixed, 1, size_of::<OaCifpHeader>())?;
    if fixed > MAX_FLAT_OUTPUT_BYTES {
        return Err(CifpFailure::OutputTooLarge);
    }
    let mut strings = 0usize;
    if let Some(h) = &c.header {
        add_string_bytes(&mut strings, &h.file_name)?;
        add_string_bytes(&mut strings, &h.version)?;
        add_string_bytes(&mut strings, &h.creation_date)?;
        add_string_bytes(&mut strings, &h.creation_time)?;
        add_string_bytes(&mut strings, &h.data_supplier)?;
        add_string_bytes(&mut strings, &h.crc)?;
        for x in &h.records {
            add_string_bytes(&mut strings, x)?;
        }
    }
    for a in &c.airports {
        meta_bytes(&mut strings, &a.meta)?;
        add_string_bytes(&mut strings, &a.ident)?;
        add_string_bytes(&mut strings, &a.icao_region)?;
        add_opt_string_bytes(&mut strings, a.iata.as_deref())?;
        add_string_bytes(&mut strings, &a.datum)?;
        add_string_bytes(&mut strings, &a.name)?;
    }
    for r in &c.runways {
        meta_bytes(&mut strings, &r.meta)?;
        add_string_bytes(&mut strings, &r.airport_ident)?;
        add_string_bytes(&mut strings, &r.airport_region)?;
        add_string_bytes(&mut strings, &r.ident)?;
        add_opt_string_bytes(&mut strings, r.localizer_ident.as_deref())?;
    }
    for l in &c.localizers {
        meta_bytes(&mut strings, &l.meta)?;
        add_string_bytes(&mut strings, &l.airport_ident)?;
        add_string_bytes(&mut strings, &l.airport_region)?;
        add_string_bytes(&mut strings, &l.ident)?;
        add_string_bytes(&mut strings, &l.runway_ident)?;
    }
    for n in &c.vhf_navaids {
        meta_bytes(&mut strings, &n.meta)?;
        add_opt_string_bytes(&mut strings, n.airport_ident.as_deref())?;
        add_opt_string_bytes(&mut strings, n.airport_region.as_deref())?;
        add_string_bytes(&mut strings, &n.ident)?;
        add_string_bytes(&mut strings, &n.icao_region)?;
        add_opt_string_bytes(&mut strings, n.dme_ident.as_deref())?;
        add_string_bytes(&mut strings, &n.datum)?;
        add_string_bytes(&mut strings, &n.name)?;
    }
    for n in c.ndbs.iter().chain(&c.terminal_ndbs) {
        meta_bytes(&mut strings, &n.meta)?;
        add_opt_string_bytes(&mut strings, n.airport_ident.as_deref())?;
        add_opt_string_bytes(&mut strings, n.airport_region.as_deref())?;
        add_string_bytes(&mut strings, &n.ident)?;
        add_string_bytes(&mut strings, &n.icao_region)?;
        add_string_bytes(&mut strings, &n.class)?;
        add_string_bytes(&mut strings, &n.datum)?;
        add_string_bytes(&mut strings, &n.name)?;
    }
    for w in c.enroute_waypoints.iter().chain(&c.terminal_waypoints) {
        meta_bytes(&mut strings, &w.meta)?;
        add_string_bytes(&mut strings, &w.area)?;
        add_string_bytes(&mut strings, &w.area_region)?;
        add_string_bytes(&mut strings, &w.ident)?;
        add_string_bytes(&mut strings, &w.icao_region)?;
        add_string_bytes(&mut strings, &w.waypoint_type)?;
        add_string_bytes(&mut strings, &w.usage)?;
        add_string_bytes(&mut strings, &w.datum)?;
        add_string_bytes(&mut strings, &w.name)?;
    }
    for a in &c.airways {
        add_string_bytes(&mut strings, &a.customer_area)?;
        add_string_bytes(&mut strings, &a.ident)?;
        for f in &a.fixes {
            meta_bytes(&mut strings, &f.meta)?;
            add_string_bytes(&mut strings, &f.route_ident)?;
            add_string_bytes(&mut strings, &f.fix.ident)?;
            add_string_bytes(&mut strings, &f.fix.icao_region)?;
        }
    }
    for p in &c.procedures {
        add_string_bytes(&mut strings, &p.customer_area)?;
        add_string_bytes(&mut strings, &p.airport_ident)?;
        add_string_bytes(&mut strings, &p.airport_region)?;
        add_string_bytes(&mut strings, &p.ident)?;
        for r in &p.routes {
            add_string_bytes(&mut strings, &r.transition)?;
            for l in &r.legs {
                meta_bytes(&mut strings, &l.meta)?;
                add_string_bytes(&mut strings, &l.airport_ident)?;
                add_string_bytes(&mut strings, &l.airport_region)?;
                add_string_bytes(&mut strings, &l.ident)?;
                add_string_bytes(&mut strings, &l.transition)?;
                add_opt_string_bytes(&mut strings, l.rnp_code.as_deref())?;
                fix_bytes(&mut strings, l.fix.as_ref())?;
                fix_bytes(&mut strings, l.recommended_navaid.as_ref())?;
                fix_bytes(&mut strings, l.center_fix.as_ref())?;
                for c in &l.continuations {
                    meta_bytes(&mut strings, &c.meta)?;
                    add_string_bytes(&mut strings, &c.text)?;
                    if let Some(levels) = cifp::procedure::levels_of_service(c) {
                        for x in levels {
                            add_string_bytes(&mut strings, &x.name)?;
                        }
                    }
                }
            }
        }
    }
    for key in c.counts.keys() {
        add_string_bytes(&mut strings, key)?;
    }
    for a in c.controlled_airspace.iter().chain(&c.special_use_airspace) {
        meta_bytes(&mut strings, &a.meta)?;
        add_string_bytes(&mut strings, &a.icao_region)?;
        add_string_bytes(&mut strings, &a.designation)?;
        add_string_bytes(&mut strings, &a.name)?;
        for c in &a.continuations {
            meta_bytes(&mut strings, &c.meta)?;
            add_string_bytes(&mut strings, &c.controlling_agency)?;
        }
    }
    let output = fixed
        .checked_add(strings)
        .ok_or(CifpFailure::OutputTooLarge)?;
    if output > MAX_FLAT_OUTPUT_BYTES {
        return Err(CifpFailure::OutputTooLarge);
    }
    counts.airports = u64::try_from(airports).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.runways = u64::try_from(runways).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.localizers = u64::try_from(localizers).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.vhf_navaids = u64::try_from(vhf_navaids).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.ndbs = u64::try_from(ndbs).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.waypoints = u64::try_from(waypoints).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.moras = u64::try_from(moras).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.airspaces = u64::try_from(airspaces).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.airways = u64::try_from(c.airways.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.airway_fixes = u64::try_from(airway_fixes).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.procedures = u64::try_from(procedures).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.routes = u64::try_from(routes).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.legs = u64::try_from(legs).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.continuations = u64::try_from(continuations).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.levels_of_service = u64::try_from(levels).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.boundaries = u64::try_from(boundaries).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.header_records =
        u64::try_from(header_records).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.sections = u64::try_from(c.counts.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.cycles = u64::try_from(c.cycle_counts.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.errors = u64::try_from(errors).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.errors_dropped =
        u64::try_from(c.errors_dropped).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.strings_bytes = u64::try_from(strings).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.output_bytes = u64::try_from(output).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.is_clean = u8::from(cifp::is_clean(c));
    Ok((counts, output))
}

fn reserve<T>(v: &mut Vec<T>, n: usize) -> Result<(), CifpFailure> {
    v.try_reserve_exact(n).map_err(|_| CifpFailure::Allocation)
}
fn push_meta(pool: &mut Pool, m: &cifp::types::Meta) -> Result<OaCifpMeta, CifpFailure> {
    meta(pool, m)
}
fn source_error(input: &[u8], line: usize) -> OaCifpError {
    let o = OaCifpError {
        kind: ERROR_LINE_TOO_LONG,
        line: u64::try_from(line).unwrap_or(u64::MAX),
        limit: cifp::MAX_LINE_LEN as u64,
        ..OaCifpError::default()
    };
    let _ = input;
    o
}
fn preflight_checked(input: &[u8], source: &str) -> Result<usize, CifpFailure> {
    let line_count = preflight(input, source)?;
    let mut line = 1usize;
    let mut start = 0usize;
    for (i, &b) in input.iter().enumerate() {
        if b == b'\n' {
            let length = i.saturating_sub(start) + 1;
            if length > cifp::MAX_LINE_LEN {
                return Err(CifpFailure::Decode(Box::new(source_error(input, line))));
            };
            line = line.saturating_add(1);
            start = i.saturating_add(1);
        }
    }
    if start < input.len() && input.len().saturating_sub(start) > cifp::MAX_LINE_LEN {
        return Err(CifpFailure::Decode(Box::new(source_error(input, line))));
    }
    Ok(line_count)
}

#[allow(clippy::too_many_arguments)] // The mutable vectors are the separate ABI output tables.
fn encode_record(
    r: &cifp::Cifp,
    pool: &mut Pool,
    airports: &mut Vec<OaCifpAirport>,
    runways: &mut Vec<OaCifpRunway>,
    localizers: &mut Vec<OaCifpLocalizer>,
    vhf_navaids: &mut Vec<OaCifpVhfNavaid>,
    ndbs: &mut Vec<OaCifpNdb>,
    waypoints: &mut Vec<OaCifpWaypoint>,
    moras: &mut Vec<OaCifpMora>,
    airspaces: &mut Vec<OaCifpAirspace>,
    boundaries: &mut Vec<OaCifpBoundary>,
    continuations: &mut Vec<OaCifpContinuation>,
    legs: &mut Vec<OaCifpLeg>,
    levels: &mut Vec<OaCifpLevelOfService>,
    routes: &mut Vec<OaCifpRoute>,
    procedures: &mut Vec<OaCifpProcedure>,
    airways: &mut Vec<OaCifpAirway>,
    airway_fixes: &mut Vec<OaCifpAirwayFix>,
) -> Result<(), CifpFailure> {
    for a in &r.airports {
        let mut present = 0;
        let iata = pool.opt(a.iata.as_deref())?;
        if a.iata.is_some() {
            present |= PRESENT_IATA;
        }
        let mut row = OaCifpAirport {
            meta: push_meta(pool, &a.meta)?,
            ident: pool.put(&a.ident)?,
            icao_region: pool.put(&a.icao_region)?,
            iata,
            datum: pool.put(&a.datum)?,
            name: pool.put(&a.name)?,
            position: pos(a.position),
            longest_runway_ft: a.longest_runway_ft.unwrap_or(0),
            ifr_capable: u8::from(a.ifr_capable),
            longest_runway_surface: optional_char(
                a.longest_runway_surface,
                PRESENT_LONGEST_RUNWAY_SURFACE,
                &mut present,
            ),
            ..OaCifpAirport::default()
        };
        if a.longest_runway_ft.is_some() {
            present |= PRESENT_LONGEST_RUNWAY;
        }
        if let Some(v) = a.magnetic_variation {
            present |= PRESENT_MAGVAR;
            row.magnetic_variation = v;
        }
        if let Some(v) = a.elevation_ft {
            present |= PRESENT_ELEVATION;
            row.elevation_ft = v;
        }
        row.transition_altitude = alt(a.transition_altitude);
        if altitude_present(a.transition_altitude) {
            present |= PRESENT_TRANSITION_ALT;
        }
        row.transition_level = alt(a.transition_level);
        if altitude_present(a.transition_level) {
            present |= PRESENT_TRANSITION_LEVEL;
        }
        row.public_military =
            optional_char(a.public_military, PRESENT_PUBLIC_MILITARY, &mut present);
        row.north_reference =
            optional_char(a.north_reference, PRESENT_NORTH_REFERENCE, &mut present);
        row.present = present;
        airports.push(row);
    }
    for a in &r.runways {
        let mut present = 0;
        let mut row = OaCifpRunway {
            meta: push_meta(pool, &a.meta)?,
            airport_ident: pool.put(&a.airport_ident)?,
            airport_region: pool.put(&a.airport_region)?,
            ident: pool.put(&a.ident)?,
            localizer_ident: pool.opt(a.localizer_ident.as_deref())?,
            length_ft: a.length_ft,
            displaced_threshold_ft: a.displaced_threshold_ft,
            threshold: pos(a.threshold),
            tch_indicator: optional_char(a.tch_indicator, PRESENT_TCH, &mut present),
            localizer_category: optional_char(
                a.localizer_category,
                PRESENT_LOCALIZER_CATEGORY,
                &mut present,
            ),
            ..OaCifpRunway::default()
        };
        if let Some(v) = a.magnetic_bearing {
            present |= PRESENT_BEARING;
            row.magnetic_bearing = v;
        }
        if let Some(v) = a.ellipsoid_height_m {
            present |= PRESENT_ELLIPSOID_HEIGHT;
            row.ellipsoid_height_m = v;
        }
        if let Some(v) = a.threshold_elevation_ft {
            present |= PRESENT_THRESHOLD_ELEVATION;
            row.threshold_elevation_ft = v;
        }
        if let Some(v) = a.threshold_crossing_height_ft {
            present |= PRESENT_TCH;
            row.threshold_crossing_height_ft = v;
        }
        if let Some(v) = a.width_ft {
            present |= PRESENT_WIDTH;
            row.width_ft = v;
        }
        if a.localizer_ident.is_some() {
            present |= PRESENT_LOCALIZER_IDENT;
        }
        row.present = present;
        runways.push(row);
    }
    for a in &r.localizers {
        let mut present = 0;
        let mut row = OaCifpLocalizer {
            meta: push_meta(pool, &a.meta)?,
            airport_ident: pool.put(&a.airport_ident)?,
            airport_region: pool.put(&a.airport_region)?,
            ident: pool.put(&a.ident)?,
            runway_ident: pool.put(&a.runway_ident)?,
            localizer_position: pos(a.localizer_position),
            category: a.category as u8,
            frequency_mhz: a.frequency_mhz,
            bearing: a.bearing,
            localizer_width_deg: a.localizer_width_deg,
            ..OaCifpLocalizer::default()
        };
        if let Some(v) = a.glide_slope_position {
            present |= PRESENT_GLIDE_POSITION;
            row.glide_slope_position = pos(v);
        }
        if let Some(v) = a.glide_slope_angle_deg {
            present |= PRESENT_GLIDE_ANGLE;
            row.glide_slope_angle_deg = v;
        }
        if let Some(v) = a.station_declination {
            present |= PRESENT_STATION_DECLINATION;
            row.station_declination = v;
        }
        if let Some(v) = a.glide_slope_tch_ft {
            present |= PRESENT_TCH;
            row.glide_slope_tch_ft = v;
        }
        if let Some(v) = a.glide_slope_elevation_ft {
            present |= PRESENT_GLIDE_ELEVATION;
            row.glide_slope_elevation_ft = v;
        }
        row.present = present;
        localizers.push(row);
    }
    for a in &r.vhf_navaids {
        let mut present = 0;
        let mut row = OaCifpVhfNavaid {
            meta: push_meta(pool, &a.meta)?,
            airport_ident: pool.opt(a.airport_ident.as_deref())?,
            airport_region: pool.opt(a.airport_region.as_deref())?,
            ident: pool.put(&a.ident)?,
            icao_region: pool.put(&a.icao_region)?,
            dme_ident: pool.opt(a.dme_ident.as_deref())?,
            datum: pool.put(&a.datum)?,
            name: pool.put(&a.name)?,
            class: a.class.map(|x| x as u8),
            ..OaCifpVhfNavaid::default()
        };
        if a.airport_ident.is_some() {
            present |= PRESENT_AIRPORT_IDENT;
        }
        if a.airport_region.is_some() {
            present |= PRESENT_AIRPORT_REGION;
        }
        if let Some(v) = a.frequency_mhz {
            present |= PRESENT_FREQUENCY;
            row.frequency_mhz = v;
        }
        if let Some(v) = a.vor_position {
            present |= PRESENT_VOR_POSITION;
            row.vor_position = pos(v);
        }
        if a.dme_ident.is_some() {
            present |= PRESENT_DME_IDENT;
        }
        if let Some(v) = a.dme_position {
            present |= PRESENT_DME_POSITION;
            row.dme_position = pos(v);
        }
        if let Some(v) = a.station_declination {
            present |= PRESENT_STATION_DECLINATION;
            row.station_declination = v;
        }
        if let Some(v) = a.dme_elevation_ft {
            present |= PRESENT_DME_ELEVATION;
            row.dme_elevation_ft = v;
        }
        row.figure_of_merit =
            optional_char(a.figure_of_merit, PRESENT_FIGURE_OF_MERIT, &mut present);
        row.present = present;
        vhf_navaids.push(row);
    }
    for (terminal, items) in [(false, &r.ndbs), (true, &r.terminal_ndbs)] {
        for a in items {
            let mut present = 0;
            let mut row = OaCifpNdb {
                meta: push_meta(pool, &a.meta)?,
                airport_ident: pool.opt(a.airport_ident.as_deref())?,
                airport_region: pool.opt(a.airport_region.as_deref())?,
                ident: pool.put(&a.ident)?,
                icao_region: pool.put(&a.icao_region)?,
                class: pool.put(&a.class)?,
                datum: pool.put(&a.datum)?,
                name: pool.put(&a.name)?,
                position: pos(a.position),
                terminal: u8::from(terminal),
                ..OaCifpNdb::default()
            };
            if a.airport_ident.is_some() {
                present |= PRESENT_AIRPORT_IDENT;
            }
            if a.airport_region.is_some() {
                present |= PRESENT_AIRPORT_REGION;
            }
            if let Some(v) = a.frequency_khz {
                present |= PRESENT_FREQUENCY;
                row.frequency_khz = v;
            }
            if let Some(v) = a.magnetic_variation {
                present |= PRESENT_MAGVAR;
                row.magnetic_variation = v;
            }
            row.present = present;
            ndbs.push(row);
        }
    }
    for (kind, items) in [(1u8, &r.enroute_waypoints), (2u8, &r.terminal_waypoints)] {
        for a in items {
            let mut present = PRESENT_POSITION;
            let mut row = OaCifpWaypoint {
                meta: push_meta(pool, &a.meta)?,
                area: pool.put(&a.area)?,
                area_region: pool.put(&a.area_region)?,
                ident: pool.put(&a.ident)?,
                icao_region: pool.put(&a.icao_region)?,
                waypoint_type: pool.put(&a.waypoint_type)?,
                usage: pool.put(&a.usage)?,
                datum: pool.put(&a.datum)?,
                name: pool.put(&a.name)?,
                position: pos(a.position),
                kind,
                ..OaCifpWaypoint::default()
            };
            if !a.waypoint_type.is_empty() {
                present |= PRESENT_WAYPOINT_TYPE;
            }
            if !a.usage.is_empty() {
                present |= PRESENT_USAGE;
            }
            if let Some(v) = a.magnetic_variation {
                present |= PRESENT_MAGVAR;
                row.magnetic_variation = v;
            }
            row.present = present;
            waypoints.push(row);
        }
    }
    for a in &r.mora {
        let mut present_mask = 0u32;
        let mut cells = [0u32; 30];
        for (i, v) in a.cells.iter().enumerate() {
            if let Some(x) = v {
                if let Some(slot) = cells.get_mut(i) {
                    *slot = x.saturating_mul(100);
                    present_mask |= 1u32
                        .checked_shl(u32::try_from(i).unwrap_or(32))
                        .unwrap_or(0);
                }
            }
        }
        moras.push(OaCifpMora {
            meta: push_meta(pool, &a.meta)?,
            start_lat: a.start_lat,
            start_lon: a.start_lon,
            present_mask,
            cells_ft: cells,
        });
    }
    // Airway fixes remain in source order within each explicit airway group.
    for a in &r.airways {
        let fix_start =
            u64::try_from(airway_fixes.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
        let first_line = a
            .fixes
            .first()
            .map_or(0, |f| u64::try_from(f.meta.line).unwrap_or(0));
        for f in &a.fixes {
            let mut present = 0;
            let mut row = OaCifpAirwayFix {
                meta: push_meta(pool, &f.meta)?,
                route_ident: pool.put(&f.route_ident)?,
                fix: fixref(pool, Some(&f.fix))?,
                sequence: f.sequence,
                description: f.description.map(|x| x as u8),
                route_type: f.route_type as u8,
                level: optional_char(f.level, PRESENT_LEVEL, &mut present),
                ..OaCifpAirwayFix::default()
            };
            if let Some(v) = f.outbound_course {
                present |= PRESENT_OUTBOUND_COURSE;
                row.outbound_course = v;
            }
            if let Some(v) = f.distance_nm {
                present |= PRESENT_DISTANCE;
                row.distance_nm = v;
            }
            if let Some(v) = f.inbound_course {
                present |= PRESENT_INBOUND_COURSE;
                row.inbound_course = v;
            }
            if let Some(v) = f.min_altitude {
                present |= PRESENT_ALTITUDE_1;
                row.min_altitude = alt(Some(v));
            }
            if let Some(v) = f.min_altitude_2 {
                present |= PRESENT_ALTITUDE_2;
                row.min_altitude_2 = alt(Some(v));
            }
            if let Some(v) = f.max_altitude {
                present |= PRESENT_MAX_ALTITUDE;
                row.max_altitude = alt(Some(v));
            }
            row.present = present;
            airway_fixes.push(row);
        }
        airways.push(OaCifpAirway {
            customer_area: pool.put(&a.customer_area)?,
            ident: pool.put(&a.ident)?,
            first_line,
            fix_start,
            fix_count: u64::try_from(a.fixes.len()).map_err(|_| CifpFailure::OutputTooLarge)?,
            route_type: a.route_type as u8,
            level: a.level.map_or(0, |v| v as u8),
            reserved: [0; 6],
        });
    }
    // Procedures are explicitly linked to contiguous route and leg ranges.
    for p in &r.procedures {
        let route_start = u64::try_from(routes.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
        let first_line = p
            .routes
            .first()
            .and_then(|r| r.legs.first())
            .map_or(0, |l| u64::try_from(l.meta.line).unwrap_or(0));
        for route in &p.routes {
            let leg_start = u64::try_from(legs.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
            let route_line = route
                .legs
                .first()
                .map_or(0, |l| u64::try_from(l.meta.line).unwrap_or(0));
            for l in &route.legs {
                let mut present = 0;
                let kind = match l.kind {
                    cifp::procedure::ProcedureKind::Sid => 1,
                    cifp::procedure::ProcedureKind::Star => 2,
                    cifp::procedure::ProcedureKind::Approach => 3,
                };
                let mut row = OaCifpLeg {
                    meta: push_meta(pool, &l.meta)?,
                    airport_ident: pool.put(&l.airport_ident)?,
                    airport_region: pool.put(&l.airport_region)?,
                    ident: pool.put(&l.ident)?,
                    transition: pool.put(&l.transition)?,
                    rnp_code: pool.opt(l.rnp_code.as_deref())?,
                    fix: fixref(pool, l.fix.as_ref())?,
                    recommended_navaid: fixref(pool, l.recommended_navaid.as_ref())?,
                    center_fix: fixref(pool, l.center_fix.as_ref())?,
                    sequence: l.sequence,
                    description: l.description.map(|x| x as u8),
                    kind,
                    route_type: l.route_type as u8,
                    turn_direction: l.turn_direction.map_or(0, |v| match v {
                        cifp::types::TurnDirection::Left => b'L',
                        cifp::types::TurnDirection::Right => b'R',
                        cifp::types::TurnDirection::Either => b'E',
                    }),
                    path_terminator: path_code(l.path_terminator),
                    ..OaCifpLeg::default()
                };
                if l.fix.is_some() {
                    present |= PRESENT_FIX;
                }
                if l.turn_direction.is_some() {
                    present |= PRESENT_TURN_DIRECTION;
                }
                if l.rnp_code.is_some() {
                    present |= PRESENT_RNP;
                }
                if l.recommended_navaid.is_some() {
                    present |= PRESENT_RECOMMENDED_NAVAID;
                }
                if l.center_fix.is_some() {
                    present |= PRESENT_CENTER_FIX;
                }
                if let Some(v) = l.arc_radius_nm {
                    present |= PRESENT_ARC_RADIUS;
                    row.arc_radius_nm = v;
                }
                if let Some(v) = l.theta_deg {
                    present |= PRESENT_THETA;
                    row.theta_deg = v;
                }
                if let Some(v) = l.rho_nm {
                    present |= PRESENT_RHO;
                    row.rho_nm = v;
                }
                if let Some(v) = l.course_deg {
                    present |= PRESENT_COURSE;
                    row.course_deg = v;
                }
                if let Some(v) = l.distance_or_time {
                    present |= PRESENT_DISTANCE_OR_TIME;
                    match v {
                        cifp::procedure::DistanceOrTime::Distance(x) => {
                            row.distance_or_time_kind = 1;
                            row.distance_or_time_value = x
                        }
                        cifp::procedure::DistanceOrTime::Time(x) => {
                            row.distance_or_time_kind = 2;
                            row.distance_or_time_value = x
                        }
                    }
                }
                if let Some(v) = l.altitude_description {
                    present |= PRESENT_ALT_DESC;
                    row.altitude_description = match v {
                        cifp::types::AltitudeDescription::AtOrAbove => b'+',
                        cifp::types::AltitudeDescription::AtOrBelow => b'-',
                        cifp::types::AltitudeDescription::At => b'@',
                        cifp::types::AltitudeDescription::Between => b'B',
                        cifp::types::AltitudeDescription::Other(c) => c as u8,
                    };
                }
                row.altitude1 = alt(l.altitude1);
                if l.altitude1.is_some() {
                    present |= PRESENT_ALTITUDE_1;
                }
                row.altitude2 = alt(l.altitude2);
                if l.altitude2.is_some() {
                    present |= PRESENT_ALTITUDE_2;
                }
                row.transition_altitude = alt(l.transition_altitude);
                if l.transition_altitude.is_some() {
                    present |= PRESENT_TRANSITION_ALTITUDE;
                }
                if let Some(v) = l.speed_limit_kt {
                    present |= PRESENT_SPEED_LIMIT;
                    row.speed_limit_kt = v;
                }
                if let Some(v) = l.vertical_angle_deg {
                    present |= PRESENT_VERTICAL_ANGLE;
                    row.vertical_angle_deg = v;
                }
                row.gnss_fms_indication =
                    optional_char(l.gnss_fms_indication, PRESENT_GNSS_FMS, &mut present);
                row.speed_limit_description =
                    optional_char(l.speed_limit_description, PRESENT_SPEED_DESC, &mut present);
                row.present = present;
                let leg_index =
                    u64::try_from(legs.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
                legs.push(row);
                for c in &l.continuations {
                    let ci = u64::try_from(continuations.len())
                        .map_err(|_| CifpFailure::OutputTooLarge)?;
                    continuations.push(OaCifpContinuation {
                        meta: push_meta(pool, &c.meta)?,
                        owner_index: leg_index,
                        text: pool.put(&c.text)?,
                        number: c.number as u8,
                        application_type: c.application_type as u8,
                        owner_kind: 1,
                        reserved: [0; 5],
                    });
                    if let Some(ls) = cifp::procedure::levels_of_service(c) {
                        for x in ls {
                            levels.push(OaCifpLevelOfService {
                                continuation_index: ci,
                                name: pool.put(&x.name)?,
                                authorized: u8::from(x.authorized),
                                reserved: [0; 7],
                            });
                        }
                    }
                }
            }
            routes.push(OaCifpRoute {
                transition: pool.put(&route.transition)?,
                first_line: route_line,
                leg_start,
                leg_count: u64::try_from(route.legs.len())
                    .map_err(|_| CifpFailure::OutputTooLarge)?,
                route_type: route.route_type as u8,
                reserved: [0; 7],
            });
        }
        procedures.push(OaCifpProcedure {
            customer_area: pool.put(&p.customer_area)?,
            airport_ident: pool.put(&p.airport_ident)?,
            airport_region: pool.put(&p.airport_region)?,
            ident: pool.put(&p.ident)?,
            first_line,
            route_start,
            route_count: u64::try_from(p.routes.len()).map_err(|_| CifpFailure::OutputTooLarge)?,
            kind: match p.kind {
                cifp::procedure::ProcedureKind::Sid => 1,
                cifp::procedure::ProcedureKind::Star => 2,
                cifp::procedure::ProcedureKind::Approach => 3,
            },
            reserved: [0; 7],
        });
    }
    // Boundaries are retained as typed points; source line is explicitly unavailable because
    // oa-arinc424's grouped Airspace type does not retain each point's physical line.
    for a in r.controlled_airspace.iter().chain(&r.special_use_airspace) {
        let ri = u64::try_from(airspaces.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
        let boundary_start =
            u64::try_from(boundaries.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
        let continuation_start =
            u64::try_from(continuations.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
        let mut present = 0;
        for b in &a.boundary {
            let mut flags = 0u8;
            let mut bp = OaCifpBoundary {
                sequence: b.sequence,
                via: via_code(b.via),
                end: u8::from(b.end),
                source_line: NO_OFFSET,
                ..OaCifpBoundary::default()
            };
            if let Some(v) = b.position {
                bp.position = pos(v);
                flags |= 1;
            }
            if let Some(v) = b.arc_origin {
                bp.arc_origin = pos(v);
                flags |= 2;
                present |= PRESENT_ARC_ORIGIN;
            }
            if let Some(v) = b.arc_distance_nm {
                bp.arc_distance_nm = v;
                flags |= 4;
                present |= PRESENT_ARC_DISTANCE;
            }
            if let Some(v) = b.arc_bearing_deg {
                bp.arc_bearing_deg = v;
                flags |= 8;
                present |= PRESENT_ARC_BEARING;
            }
            bp.present = flags;
            boundaries.push(bp);
        }
        for c in &a.continuations {
            continuations.push(OaCifpContinuation {
                meta: push_meta(pool, &c.meta)?,
                owner_index: ri,
                text: pool.put(&c.controlling_agency)?,
                number: c.number as u8,
                application_type: c.application_type as u8,
                owner_kind: 2,
                reserved: [0; 5],
            });
        }
        if a.classification.is_some() {
            present |= PRESENT_CLASSIFICATION;
        }
        if a.time_code.is_some() {
            present |= PRESENT_TIME_CODE;
        }
        if a.lower.is_some() {
            present |= PRESENT_LOWER;
        }
        if a.upper.is_some() {
            present |= PRESENT_UPPER;
        }
        let row = OaCifpAirspace {
            meta: push_meta(pool, &a.meta)?,
            present,
            icao_region: pool.put(&a.icao_region)?,
            designation: pool.put(&a.designation)?,
            name: pool.put(&a.name)?,
            boundary_start,
            boundary_count: u64::try_from(a.boundary.len())
                .map_err(|_| CifpFailure::OutputTooLarge)?,
            continuation_start,
            continuation_count: u64::try_from(a.continuations.len())
                .map_err(|_| CifpFailure::OutputTooLarge)?,
            kind: if a.kind == cifp::airspace::AirspaceKind::Controlled {
                1
            } else {
                2
            },
            airspace_type: a.airspace_type as u8,
            classification: a.classification.map_or(0, |v| v as u8),
            multiple_code: a.multiple_code as u8,
            level: a.level.map_or(0, |v| v as u8),
            time_code: a.time_code.map_or(0, |v| v as u8),
            reserved: [0; 2],
            lower: alt(a.lower.map(|x| x.altitude)),
            lower_unit: a.lower.map_or(0, |x| alt_unit(Some(x.unit))),
            reserved2: [0; 6],
            upper: alt(a.upper.map(|x| x.altitude)),
            upper_unit: a.upper.map_or(0, |x| alt_unit(Some(x.unit))),
        };
        airspaces.push(row);
    }
    Ok(())
}
fn path_code(p: cifp::procedure::PathTerminator) -> u8 {
    let s = match p {
        cifp::procedure::PathTerminator::AF => b"AF",
        cifp::procedure::PathTerminator::CA => b"CA",
        cifp::procedure::PathTerminator::CD => b"CD",
        cifp::procedure::PathTerminator::CF => b"CF",
        cifp::procedure::PathTerminator::CI => b"CI",
        cifp::procedure::PathTerminator::CR => b"CR",
        cifp::procedure::PathTerminator::DF => b"DF",
        cifp::procedure::PathTerminator::FA => b"FA",
        cifp::procedure::PathTerminator::FC => b"FC",
        cifp::procedure::PathTerminator::FD => b"FD",
        cifp::procedure::PathTerminator::FM => b"FM",
        cifp::procedure::PathTerminator::HA => b"HA",
        cifp::procedure::PathTerminator::HF => b"HF",
        cifp::procedure::PathTerminator::HM => b"HM",
        cifp::procedure::PathTerminator::IF => b"IF",
        cifp::procedure::PathTerminator::PI => b"PI",
        cifp::procedure::PathTerminator::RF => b"RF",
        cifp::procedure::PathTerminator::TF => b"TF",
        cifp::procedure::PathTerminator::VA => b"VA",
        cifp::procedure::PathTerminator::VD => b"VD",
        cifp::procedure::PathTerminator::VI => b"VI",
        cifp::procedure::PathTerminator::VM => b"VM",
        cifp::procedure::PathTerminator::VR => b"VR",
    };
    s.first().copied().unwrap_or(0)
}
fn via_code(v: cifp::airspace::BoundaryVia) -> u8 {
    match v {
        cifp::airspace::BoundaryVia::Circle => b'C',
        cifp::airspace::BoundaryVia::GreatCircle => b'G',
        cifp::airspace::BoundaryVia::RhumbLine => b'H',
        cifp::airspace::BoundaryVia::CounterClockwiseArc => b'L',
        cifp::airspace::BoundaryVia::ClockwiseArc => b'R',
    }
}

pub fn decode(input: &[u8], source: &str) -> Result<CifpFlat, CifpFailure> {
    let _physical = preflight_checked(input, source)?;
    let parsed =
        cifp::parse_bytes(input, source).map_err(|e| CifpFailure::Decode(Box::new(error(&e))))?;
    let (mut counts, expected) = plan_counts(&parsed)?;
    let fixed_bytes = expected
        .checked_sub(
            usize::try_from(counts.strings_bytes).map_err(|_| CifpFailure::OutputTooLarge)?,
        )
        .ok_or(CifpFailure::OutputTooLarge)?;
    let pool_limit = MAX_FLAT_OUTPUT_BYTES
        .checked_sub(fixed_bytes)
        .ok_or(CifpFailure::OutputTooLarge)?;
    let mut pool = Pool::with_limit(pool_limit);
    pool.bytes
        .try_reserve_exact(
            usize::try_from(counts.strings_bytes).map_err(|_| CifpFailure::OutputTooLarge)?,
        )
        .map_err(|_| CifpFailure::Allocation)?;
    let mut airports = Vec::new();
    let mut runways = Vec::new();
    let mut localizers = Vec::new();
    let mut vhf_navaids = Vec::new();
    let mut ndbs = Vec::new();
    let mut waypoints = Vec::new();
    let mut moras = Vec::new();
    let mut airspaces = Vec::new();
    let mut header_records = Vec::new();
    let mut airways = Vec::new();
    let mut airway_fixes = Vec::new();
    let mut procedures = Vec::new();
    let mut routes = Vec::new();
    let mut legs = Vec::new();
    let mut continuations = Vec::new();
    let mut levels = Vec::new();
    let mut boundaries = Vec::new();
    let mut sections = Vec::new();
    let mut cycles = Vec::new();
    let mut errors = Vec::new();
    reserve(
        &mut airports,
        usize::try_from(counts.airports).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut runways,
        usize::try_from(counts.runways).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut localizers,
        usize::try_from(counts.localizers).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut vhf_navaids,
        usize::try_from(counts.vhf_navaids).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut ndbs,
        usize::try_from(counts.ndbs).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut waypoints,
        usize::try_from(counts.waypoints).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut moras,
        usize::try_from(counts.moras).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut airspaces,
        usize::try_from(counts.airspaces).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut header_records,
        usize::try_from(counts.header_records).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut airways,
        usize::try_from(counts.airways).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut airway_fixes,
        usize::try_from(counts.airway_fixes).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut procedures,
        usize::try_from(counts.procedures).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut routes,
        usize::try_from(counts.routes).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut legs,
        usize::try_from(counts.legs).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut continuations,
        usize::try_from(counts.continuations).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut levels,
        usize::try_from(counts.levels_of_service).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut boundaries,
        usize::try_from(counts.boundaries).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut sections,
        usize::try_from(counts.sections).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut cycles,
        usize::try_from(counts.cycles).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    reserve(
        &mut errors,
        usize::try_from(counts.errors).map_err(|_| CifpFailure::OutputTooLarge)?,
    )?;
    let mut header = OaCifpHeader::default();
    if let Some(h) = &parsed.header {
        header = OaCifpHeader {
            file_name: pool.put(&h.file_name)?,
            version: pool.put(&h.version)?,
            creation_date: pool.put(&h.creation_date)?,
            creation_time: pool.put(&h.creation_time)?,
            data_supplier: pool.put(&h.data_supplier)?,
            crc: pool.put(&h.crc)?,
            record_length: u32::try_from(h.record_length)
                .map_err(|_| CifpFailure::OutputTooLarge)?,
            reserved0: [0; 4],
            record_count: u64::try_from(h.record_count).map_err(|_| CifpFailure::OutputTooLarge)?,
            cycle_year: h.cycle.year,
            cycle_release: h.cycle.release,
            production_flag: h.production_flag as u8,
            crc_verified: 0,
            reserved: [0; 4],
        };
        for x in &h.records {
            header_records.push(OaCifpHeaderText { text: pool.put(x)? });
        }
    }
    encode_record(
        &parsed,
        &mut pool,
        &mut airports,
        &mut runways,
        &mut localizers,
        &mut vhf_navaids,
        &mut ndbs,
        &mut waypoints,
        &mut moras,
        &mut airspaces,
        &mut boundaries,
        &mut continuations,
        &mut legs,
        &mut levels,
        &mut routes,
        &mut procedures,
        &mut airways,
        &mut airway_fixes,
    )?;
    for (code, count) in &parsed.counts {
        let supported = matches!(
            code.as_str(),
            "PA" | "PG"
                | "PI"
                | "PC"
                | "PD"
                | "PE"
                | "PF"
                | "EA"
                | "ER"
                | "D"
                | "DB"
                | "PN"
                | "UC"
                | "UR"
                | "AS"
        );
        sections.push(OaCifpSectionCount {
            section: pool.put(code)?,
            count: u64::try_from(*count).map_err(|_| CifpFailure::OutputTooLarge)?,
            supported: u8::from(supported),
            reserved: [0; 7],
        });
    }
    for (cycle, count) in &parsed.cycle_counts {
        cycles.push(OaCifpCycleCount {
            count: u64::try_from(*count).map_err(|_| CifpFailure::OutputTooLarge)?,
            year: cycle.year,
            release: cycle.release,
            reserved: [0; 6],
        });
    }
    for e in &parsed.errors {
        errors.push(error(e));
    }
    counts.strings_bytes =
        u64::try_from(pool.bytes.len()).map_err(|_| CifpFailure::OutputTooLarge)?;
    counts.output_bytes = u64::try_from(
        fixed_bytes
            .checked_add(pool.bytes.len())
            .ok_or(CifpFailure::OutputTooLarge)?,
    )
    .map_err(|_| CifpFailure::OutputTooLarge)?;
    if usize::try_from(counts.output_bytes).map_err(|_| CifpFailure::OutputTooLarge)?
        > MAX_FLAT_OUTPUT_BYTES
    {
        return Err(CifpFailure::OutputTooLarge);
    }
    Ok(CifpFlat {
        counts,
        header,
        header_records,
        airports,
        runways,
        localizers,
        vhf_navaids,
        ndbs,
        waypoints,
        moras,
        airspaces,
        airways,
        airway_fixes,
        procedures,
        routes,
        legs,
        continuations,
        levels_of_service: levels,
        boundaries,
        sections,
        cycles,
        errors,
        strings: pool.bytes,
    })
}

// C ABI layout locks generated from the reviewed C11 header; supported target is 64-bit.
const _: () = assert!(offset_of!(OaCifpRunway, reserved2) == 180);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, reserved2) == 228);
const _: () = assert!(offset_of!(OaCifpAirwayFix, reserved0) == 114);
const _: () = assert!(offset_of!(OaCifpLeg, reserved2) == 348);
const _: () = assert!(offset_of!(OaCifpHeader, reserved0) == 100);
const _: () = assert!(align_of::<OaCifpSpan>() == 8);
const _: () = assert!(align_of::<OaCifpPosition>() == 8);
const _: () = assert!(align_of::<OaCifpMeta>() == 8);
const _: () = assert!(align_of::<OaCifpFixRef>() == 8);
const _: () = assert!(align_of::<OaCifpAirport>() == 8);
const _: () = assert!(align_of::<OaCifpRunway>() == 8);
const _: () = assert!(align_of::<OaCifpLocalizer>() == 8);
const _: () = assert!(align_of::<OaCifpVhfNavaid>() == 8);
const _: () = assert!(align_of::<OaCifpNdb>() == 8);
const _: () = assert!(align_of::<OaCifpWaypoint>() == 8);
const _: () = assert!(align_of::<OaCifpAirway>() == 8);
const _: () = assert!(align_of::<OaCifpAirwayFix>() == 8);
const _: () = assert!(align_of::<OaCifpProcedure>() == 8);
const _: () = assert!(align_of::<OaCifpRoute>() == 8);
const _: () = assert!(align_of::<OaCifpLeg>() == 8);
const _: () = assert!(align_of::<OaCifpContinuation>() == 8);
const _: () = assert!(align_of::<OaCifpLevelOfService>() == 8);
const _: () = assert!(align_of::<OaCifpAirspace>() == 8);
const _: () = assert!(align_of::<OaCifpBoundary>() == 8);
const _: () = assert!(align_of::<OaCifpMora>() == 8);
const _: () = assert!(align_of::<OaCifpHeader>() == 8);
const _: () = assert!(align_of::<OaCifpHeaderText>() == 8);
const _: () = assert!(align_of::<OaCifpSectionCount>() == 8);
const _: () = assert!(align_of::<OaCifpCycleCount>() == 8);
const _: () = assert!(align_of::<OaCifpError>() == 8);
const _: () = assert!(align_of::<OaCifpCounts>() == 8);
const _: () = assert!(size_of::<OaCifpSpan>() == 16);
const _: () = assert!(offset_of!(OaCifpSpan, offset) == 0);
const _: () = assert!(offset_of!(OaCifpSpan, length) == 8);
const _: () = assert!(size_of::<OaCifpPosition>() == 16);
const _: () = assert!(offset_of!(OaCifpPosition, lat) == 0);
const _: () = assert!(offset_of!(OaCifpPosition, lon) == 8);
const _: () = assert!(size_of::<OaCifpMeta>() == 48);
const _: () = assert!(offset_of!(OaCifpMeta, line) == 0);
const _: () = assert!(offset_of!(OaCifpMeta, cycle_year) == 8);
const _: () = assert!(offset_of!(OaCifpMeta, cycle_release) == 9);
const _: () = assert!(offset_of!(OaCifpMeta, reserved) == 10);
const _: () = assert!(offset_of!(OaCifpMeta, customer_area) == 16);
const _: () = assert!(offset_of!(OaCifpMeta, file_record_number) == 32);
const _: () = assert!(size_of::<OaCifpAltitude>() == 8);
const _: () = assert!(offset_of!(OaCifpAltitude, kind) == 0);
const _: () = assert!(offset_of!(OaCifpAltitude, reserved) == 1);
const _: () = assert!(offset_of!(OaCifpAltitude, value) == 4);
const _: () = assert!(size_of::<OaCifpFixRef>() == 40);
const _: () = assert!(offset_of!(OaCifpFixRef, ident) == 0);
const _: () = assert!(offset_of!(OaCifpFixRef, region) == 16);
const _: () = assert!(offset_of!(OaCifpFixRef, section) == 32);
const _: () = assert!(offset_of!(OaCifpFixRef, subsection) == 33);
const _: () = assert!(offset_of!(OaCifpFixRef, reserved) == 34);
const _: () = assert!(size_of::<OaCifpAirport>() == 192);
const _: () = assert!(offset_of!(OaCifpAirport, meta) == 0);
const _: () = assert!(offset_of!(OaCifpAirport, present) == 48);
const _: () = assert!(offset_of!(OaCifpAirport, ident) == 56);
const _: () = assert!(offset_of!(OaCifpAirport, icao_region) == 72);
const _: () = assert!(offset_of!(OaCifpAirport, iata) == 88);
const _: () = assert!(offset_of!(OaCifpAirport, datum) == 104);
const _: () = assert!(offset_of!(OaCifpAirport, name) == 120);
const _: () = assert!(offset_of!(OaCifpAirport, position) == 136);
const _: () = assert!(offset_of!(OaCifpAirport, magnetic_variation) == 152);
const _: () = assert!(offset_of!(OaCifpAirport, elevation_ft) == 160);
const _: () = assert!(offset_of!(OaCifpAirport, longest_runway_ft) == 164);
const _: () = assert!(offset_of!(OaCifpAirport, transition_altitude) == 168);
const _: () = assert!(offset_of!(OaCifpAirport, transition_level) == 176);
const _: () = assert!(offset_of!(OaCifpAirport, ifr_capable) == 184);
const _: () = assert!(offset_of!(OaCifpAirport, longest_runway_surface) == 185);
const _: () = assert!(offset_of!(OaCifpAirport, public_military) == 186);
const _: () = assert!(offset_of!(OaCifpAirport, north_reference) == 187);
const _: () = assert!(offset_of!(OaCifpAirport, reserved) == 188);
const _: () = assert!(size_of::<OaCifpRunway>() == 184);
const _: () = assert!(offset_of!(OaCifpRunway, meta) == 0);
const _: () = assert!(offset_of!(OaCifpRunway, present) == 48);
const _: () = assert!(offset_of!(OaCifpRunway, airport_ident) == 56);
const _: () = assert!(offset_of!(OaCifpRunway, airport_region) == 72);
const _: () = assert!(offset_of!(OaCifpRunway, ident) == 88);
const _: () = assert!(offset_of!(OaCifpRunway, localizer_ident) == 104);
const _: () = assert!(offset_of!(OaCifpRunway, threshold) == 120);
const _: () = assert!(offset_of!(OaCifpRunway, length_ft) == 136);
const _: () = assert!(offset_of!(OaCifpRunway, displaced_threshold_ft) == 140);
const _: () = assert!(offset_of!(OaCifpRunway, magnetic_bearing) == 144);
const _: () = assert!(offset_of!(OaCifpRunway, ellipsoid_height_m) == 152);
const _: () = assert!(offset_of!(OaCifpRunway, threshold_elevation_ft) == 160);
const _: () = assert!(offset_of!(OaCifpRunway, threshold_crossing_height_ft) == 164);
const _: () = assert!(offset_of!(OaCifpRunway, width_ft) == 168);
const _: () = assert!(offset_of!(OaCifpRunway, tch_indicator) == 172);
const _: () = assert!(offset_of!(OaCifpRunway, localizer_category) == 173);
const _: () = assert!(offset_of!(OaCifpRunway, reserved) == 174);
const _: () = assert!(size_of::<OaCifpLocalizer>() == 208);
const _: () = assert!(offset_of!(OaCifpLocalizer, meta) == 0);
const _: () = assert!(offset_of!(OaCifpLocalizer, present) == 48);
const _: () = assert!(offset_of!(OaCifpLocalizer, airport_ident) == 56);
const _: () = assert!(offset_of!(OaCifpLocalizer, airport_region) == 72);
const _: () = assert!(offset_of!(OaCifpLocalizer, ident) == 88);
const _: () = assert!(offset_of!(OaCifpLocalizer, runway_ident) == 104);
const _: () = assert!(offset_of!(OaCifpLocalizer, localizer_position) == 120);
const _: () = assert!(offset_of!(OaCifpLocalizer, glide_slope_position) == 136);
const _: () = assert!(offset_of!(OaCifpLocalizer, category) == 152);
const _: () = assert!(offset_of!(OaCifpLocalizer, reserved0) == 153);
const _: () = assert!(offset_of!(OaCifpLocalizer, frequency_mhz) == 160);
const _: () = assert!(offset_of!(OaCifpLocalizer, bearing) == 168);
const _: () = assert!(offset_of!(OaCifpLocalizer, localizer_width_deg) == 176);
const _: () = assert!(offset_of!(OaCifpLocalizer, glide_slope_angle_deg) == 184);
const _: () = assert!(offset_of!(OaCifpLocalizer, station_declination) == 192);
const _: () = assert!(offset_of!(OaCifpLocalizer, glide_slope_tch_ft) == 200);
const _: () = assert!(offset_of!(OaCifpLocalizer, glide_slope_elevation_ft) == 204);
const _: () = assert!(size_of::<OaCifpVhfNavaid>() == 232);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, meta) == 0);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, present) == 48);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, airport_ident) == 56);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, airport_region) == 72);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, ident) == 88);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, icao_region) == 104);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, dme_ident) == 120);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, datum) == 136);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, name) == 152);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, vor_position) == 168);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, dme_position) == 184);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, frequency_mhz) == 200);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, station_declination) == 208);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, dme_elevation_ft) == 216);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, class) == 220);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, figure_of_merit) == 225);
const _: () = assert!(offset_of!(OaCifpVhfNavaid, reserved) == 226);
const _: () = assert!(size_of::<OaCifpNdb>() == 208);
const _: () = assert!(offset_of!(OaCifpNdb, meta) == 0);
const _: () = assert!(offset_of!(OaCifpNdb, present) == 48);
const _: () = assert!(offset_of!(OaCifpNdb, airport_ident) == 56);
const _: () = assert!(offset_of!(OaCifpNdb, airport_region) == 72);
const _: () = assert!(offset_of!(OaCifpNdb, ident) == 88);
const _: () = assert!(offset_of!(OaCifpNdb, icao_region) == 104);
const _: () = assert!(offset_of!(OaCifpNdb, class) == 120);
const _: () = assert!(offset_of!(OaCifpNdb, datum) == 136);
const _: () = assert!(offset_of!(OaCifpNdb, name) == 152);
const _: () = assert!(offset_of!(OaCifpNdb, position) == 168);
const _: () = assert!(offset_of!(OaCifpNdb, frequency_khz) == 184);
const _: () = assert!(offset_of!(OaCifpNdb, magnetic_variation) == 192);
const _: () = assert!(offset_of!(OaCifpNdb, terminal) == 200);
const _: () = assert!(offset_of!(OaCifpNdb, reserved) == 201);
const _: () = assert!(size_of::<OaCifpWaypoint>() == 216);
const _: () = assert!(offset_of!(OaCifpWaypoint, meta) == 0);
const _: () = assert!(offset_of!(OaCifpWaypoint, present) == 48);
const _: () = assert!(offset_of!(OaCifpWaypoint, area) == 56);
const _: () = assert!(offset_of!(OaCifpWaypoint, area_region) == 72);
const _: () = assert!(offset_of!(OaCifpWaypoint, ident) == 88);
const _: () = assert!(offset_of!(OaCifpWaypoint, icao_region) == 104);
const _: () = assert!(offset_of!(OaCifpWaypoint, waypoint_type) == 120);
const _: () = assert!(offset_of!(OaCifpWaypoint, usage) == 136);
const _: () = assert!(offset_of!(OaCifpWaypoint, datum) == 152);
const _: () = assert!(offset_of!(OaCifpWaypoint, name) == 168);
const _: () = assert!(offset_of!(OaCifpWaypoint, position) == 184);
const _: () = assert!(offset_of!(OaCifpWaypoint, magnetic_variation) == 200);
const _: () = assert!(offset_of!(OaCifpWaypoint, kind) == 208);
const _: () = assert!(offset_of!(OaCifpWaypoint, reserved) == 209);
const _: () = assert!(size_of::<OaCifpAirway>() == 64);
const _: () = assert!(offset_of!(OaCifpAirway, customer_area) == 0);
const _: () = assert!(offset_of!(OaCifpAirway, ident) == 16);
const _: () = assert!(offset_of!(OaCifpAirway, first_line) == 32);
const _: () = assert!(offset_of!(OaCifpAirway, fix_start) == 40);
const _: () = assert!(offset_of!(OaCifpAirway, fix_count) == 48);
const _: () = assert!(offset_of!(OaCifpAirway, route_type) == 56);
const _: () = assert!(offset_of!(OaCifpAirway, level) == 57);
const _: () = assert!(offset_of!(OaCifpAirway, reserved) == 58);
const _: () = assert!(size_of::<OaCifpAirwayFix>() == 176);
const _: () = assert!(offset_of!(OaCifpAirwayFix, meta) == 0);
const _: () = assert!(offset_of!(OaCifpAirwayFix, route_ident) == 48);
const _: () = assert!(offset_of!(OaCifpAirwayFix, fix) == 64);
const _: () = assert!(offset_of!(OaCifpAirwayFix, sequence) == 104);
const _: () = assert!(offset_of!(OaCifpAirwayFix, description) == 108);
const _: () = assert!(offset_of!(OaCifpAirwayFix, route_type) == 112);
const _: () = assert!(offset_of!(OaCifpAirwayFix, level) == 113);
const _: () = assert!(offset_of!(OaCifpAirwayFix, present) == 120);
const _: () = assert!(offset_of!(OaCifpAirwayFix, outbound_course) == 128);
const _: () = assert!(offset_of!(OaCifpAirwayFix, distance_nm) == 136);
const _: () = assert!(offset_of!(OaCifpAirwayFix, inbound_course) == 144);
const _: () = assert!(offset_of!(OaCifpAirwayFix, min_altitude) == 152);
const _: () = assert!(offset_of!(OaCifpAirwayFix, min_altitude_2) == 160);
const _: () = assert!(offset_of!(OaCifpAirwayFix, max_altitude) == 168);
const _: () = assert!(size_of::<OaCifpProcedure>() == 96);
const _: () = assert!(offset_of!(OaCifpProcedure, customer_area) == 0);
const _: () = assert!(offset_of!(OaCifpProcedure, airport_ident) == 16);
const _: () = assert!(offset_of!(OaCifpProcedure, airport_region) == 32);
const _: () = assert!(offset_of!(OaCifpProcedure, ident) == 48);
const _: () = assert!(offset_of!(OaCifpProcedure, first_line) == 64);
const _: () = assert!(offset_of!(OaCifpProcedure, route_start) == 72);
const _: () = assert!(offset_of!(OaCifpProcedure, route_count) == 80);
const _: () = assert!(offset_of!(OaCifpProcedure, kind) == 88);
const _: () = assert!(offset_of!(OaCifpProcedure, reserved) == 89);
const _: () = assert!(size_of::<OaCifpRoute>() == 48);
const _: () = assert!(offset_of!(OaCifpRoute, transition) == 0);
const _: () = assert!(offset_of!(OaCifpRoute, first_line) == 16);
const _: () = assert!(offset_of!(OaCifpRoute, leg_start) == 24);
const _: () = assert!(offset_of!(OaCifpRoute, leg_count) == 32);
const _: () = assert!(offset_of!(OaCifpRoute, route_type) == 40);
const _: () = assert!(offset_of!(OaCifpRoute, reserved) == 41);
const _: () = assert!(size_of::<OaCifpLeg>() == 360);
const _: () = assert!(offset_of!(OaCifpLeg, meta) == 0);
const _: () = assert!(offset_of!(OaCifpLeg, present) == 48);
const _: () = assert!(offset_of!(OaCifpLeg, airport_ident) == 56);
const _: () = assert!(offset_of!(OaCifpLeg, airport_region) == 72);
const _: () = assert!(offset_of!(OaCifpLeg, ident) == 88);
const _: () = assert!(offset_of!(OaCifpLeg, transition) == 104);
const _: () = assert!(offset_of!(OaCifpLeg, rnp_code) == 120);
const _: () = assert!(offset_of!(OaCifpLeg, fix) == 136);
const _: () = assert!(offset_of!(OaCifpLeg, recommended_navaid) == 176);
const _: () = assert!(offset_of!(OaCifpLeg, center_fix) == 216);
const _: () = assert!(offset_of!(OaCifpLeg, sequence) == 256);
const _: () = assert!(offset_of!(OaCifpLeg, description) == 260);
const _: () = assert!(offset_of!(OaCifpLeg, kind) == 264);
const _: () = assert!(offset_of!(OaCifpLeg, route_type) == 265);
const _: () = assert!(offset_of!(OaCifpLeg, turn_direction) == 266);
const _: () = assert!(offset_of!(OaCifpLeg, path_terminator) == 267);
const _: () = assert!(offset_of!(OaCifpLeg, altitude_description) == 268);
const _: () = assert!(offset_of!(OaCifpLeg, gnss_fms_indication) == 269);
const _: () = assert!(offset_of!(OaCifpLeg, speed_limit_description) == 270);
const _: () = assert!(offset_of!(OaCifpLeg, reserved0) == 271);
const _: () = assert!(offset_of!(OaCifpLeg, arc_radius_nm) == 272);
const _: () = assert!(offset_of!(OaCifpLeg, theta_deg) == 280);
const _: () = assert!(offset_of!(OaCifpLeg, rho_nm) == 288);
const _: () = assert!(offset_of!(OaCifpLeg, course_deg) == 296);
const _: () = assert!(offset_of!(OaCifpLeg, distance_or_time_value) == 304);
const _: () = assert!(offset_of!(OaCifpLeg, distance_or_time_kind) == 312);
const _: () = assert!(offset_of!(OaCifpLeg, reserved1) == 313);
const _: () = assert!(offset_of!(OaCifpLeg, altitude1) == 320);
const _: () = assert!(offset_of!(OaCifpLeg, altitude2) == 328);
const _: () = assert!(offset_of!(OaCifpLeg, transition_altitude) == 336);
const _: () = assert!(offset_of!(OaCifpLeg, speed_limit_kt) == 344);
const _: () = assert!(offset_of!(OaCifpLeg, vertical_angle_deg) == 352);
const _: () = assert!(size_of::<OaCifpContinuation>() == 80);
const _: () = assert!(offset_of!(OaCifpContinuation, meta) == 0);
const _: () = assert!(offset_of!(OaCifpContinuation, owner_index) == 48);
const _: () = assert!(offset_of!(OaCifpContinuation, text) == 56);
const _: () = assert!(offset_of!(OaCifpContinuation, number) == 72);
const _: () = assert!(offset_of!(OaCifpContinuation, application_type) == 73);
const _: () = assert!(offset_of!(OaCifpContinuation, owner_kind) == 74);
const _: () = assert!(offset_of!(OaCifpContinuation, reserved) == 75);
const _: () = assert!(size_of::<OaCifpLevelOfService>() == 32);
const _: () = assert!(offset_of!(OaCifpLevelOfService, continuation_index) == 0);
const _: () = assert!(offset_of!(OaCifpLevelOfService, name) == 8);
const _: () = assert!(offset_of!(OaCifpLevelOfService, authorized) == 24);
const _: () = assert!(offset_of!(OaCifpLevelOfService, reserved) == 25);
const _: () = assert!(size_of::<OaCifpAirspace>() == 168);
const _: () = assert!(offset_of!(OaCifpAirspace, meta) == 0);
const _: () = assert!(offset_of!(OaCifpAirspace, present) == 48);
const _: () = assert!(offset_of!(OaCifpAirspace, icao_region) == 56);
const _: () = assert!(offset_of!(OaCifpAirspace, designation) == 72);
const _: () = assert!(offset_of!(OaCifpAirspace, name) == 88);
const _: () = assert!(offset_of!(OaCifpAirspace, boundary_start) == 104);
const _: () = assert!(offset_of!(OaCifpAirspace, boundary_count) == 112);
const _: () = assert!(offset_of!(OaCifpAirspace, continuation_start) == 120);
const _: () = assert!(offset_of!(OaCifpAirspace, continuation_count) == 128);
const _: () = assert!(offset_of!(OaCifpAirspace, kind) == 136);
const _: () = assert!(offset_of!(OaCifpAirspace, airspace_type) == 137);
const _: () = assert!(offset_of!(OaCifpAirspace, classification) == 138);
const _: () = assert!(offset_of!(OaCifpAirspace, multiple_code) == 139);
const _: () = assert!(offset_of!(OaCifpAirspace, level) == 140);
const _: () = assert!(offset_of!(OaCifpAirspace, time_code) == 141);
const _: () = assert!(offset_of!(OaCifpAirspace, reserved) == 142);
const _: () = assert!(offset_of!(OaCifpAirspace, lower) == 144);
const _: () = assert!(offset_of!(OaCifpAirspace, lower_unit) == 152);
const _: () = assert!(offset_of!(OaCifpAirspace, upper_unit) == 153);
const _: () = assert!(offset_of!(OaCifpAirspace, reserved2) == 154);
const _: () = assert!(offset_of!(OaCifpAirspace, upper) == 160);
const _: () = assert!(size_of::<OaCifpBoundary>() == 64);
const _: () = assert!(offset_of!(OaCifpBoundary, position) == 0);
const _: () = assert!(offset_of!(OaCifpBoundary, arc_origin) == 16);
const _: () = assert!(offset_of!(OaCifpBoundary, sequence) == 32);
const _: () = assert!(offset_of!(OaCifpBoundary, via) == 36);
const _: () = assert!(offset_of!(OaCifpBoundary, end) == 37);
const _: () = assert!(offset_of!(OaCifpBoundary, present) == 38);
const _: () = assert!(offset_of!(OaCifpBoundary, reserved) == 39);
const _: () = assert!(offset_of!(OaCifpBoundary, arc_distance_nm) == 40);
const _: () = assert!(offset_of!(OaCifpBoundary, arc_bearing_deg) == 48);
const _: () = assert!(offset_of!(OaCifpBoundary, source_line) == 56);
const _: () = assert!(size_of::<OaCifpMora>() == 176);
const _: () = assert!(offset_of!(OaCifpMora, meta) == 0);
const _: () = assert!(offset_of!(OaCifpMora, start_lat) == 48);
const _: () = assert!(offset_of!(OaCifpMora, start_lon) == 50);
const _: () = assert!(offset_of!(OaCifpMora, present_mask) == 52);
const _: () = assert!(offset_of!(OaCifpMora, cells_ft) == 56);
const _: () = assert!(size_of::<OaCifpHeader>() == 120);
const _: () = assert!(offset_of!(OaCifpHeader, file_name) == 0);
const _: () = assert!(offset_of!(OaCifpHeader, version) == 16);
const _: () = assert!(offset_of!(OaCifpHeader, creation_date) == 32);
const _: () = assert!(offset_of!(OaCifpHeader, creation_time) == 48);
const _: () = assert!(offset_of!(OaCifpHeader, data_supplier) == 64);
const _: () = assert!(offset_of!(OaCifpHeader, crc) == 80);
const _: () = assert!(offset_of!(OaCifpHeader, record_length) == 96);
const _: () = assert!(offset_of!(OaCifpHeader, record_count) == 104);
const _: () = assert!(offset_of!(OaCifpHeader, cycle_year) == 112);
const _: () = assert!(offset_of!(OaCifpHeader, cycle_release) == 113);
const _: () = assert!(offset_of!(OaCifpHeader, production_flag) == 114);
const _: () = assert!(offset_of!(OaCifpHeader, crc_verified) == 115);
const _: () = assert!(offset_of!(OaCifpHeader, reserved) == 116);
const _: () = assert!(size_of::<OaCifpHeaderText>() == 16);
const _: () = assert!(offset_of!(OaCifpHeaderText, text) == 0);
const _: () = assert!(size_of::<OaCifpSectionCount>() == 32);
const _: () = assert!(offset_of!(OaCifpSectionCount, section) == 0);
const _: () = assert!(offset_of!(OaCifpSectionCount, count) == 16);
const _: () = assert!(offset_of!(OaCifpSectionCount, supported) == 24);
const _: () = assert!(offset_of!(OaCifpSectionCount, reserved) == 25);
const _: () = assert!(size_of::<OaCifpCycleCount>() == 16);
const _: () = assert!(offset_of!(OaCifpCycleCount, count) == 0);
const _: () = assert!(offset_of!(OaCifpCycleCount, year) == 8);
const _: () = assert!(offset_of!(OaCifpCycleCount, release) == 9);
const _: () = assert!(offset_of!(OaCifpCycleCount, reserved) == 10);
const _: () = assert!(size_of::<OaCifpError>() == 296);
const _: () = assert!(offset_of!(OaCifpError, status) == 0);
const _: () = assert!(offset_of!(OaCifpError, kind) == 4);
const _: () = assert!(offset_of!(OaCifpError, line) == 8);
const _: () = assert!(offset_of!(OaCifpError, first_column) == 16);
const _: () = assert!(offset_of!(OaCifpError, last_column) == 20);
const _: () = assert!(offset_of!(OaCifpError, limit) == 24);
const _: () = assert!(offset_of!(OaCifpError, actual) == 32);
const _: () = assert!(offset_of!(OaCifpError, previous) == 40);
const _: () = assert!(offset_of!(OaCifpError, found) == 48);
const _: () = assert!(offset_of!(OaCifpError, code) == 56);
const _: () = assert!(offset_of!(OaCifpError, flags) == 60);
const _: () = assert!(offset_of!(OaCifpError, field) == 64);
const _: () = assert!(offset_of!(OaCifpError, value) == 96);
const _: () = assert!(offset_of!(OaCifpError, reason) == 228);
const _: () = assert!(offset_of!(OaCifpError, reserved) == 292);
const _: () = assert!(size_of::<OaCifpCounts>() == 192);
const _: () = assert!(offset_of!(OaCifpCounts, airports) == 0);
const _: () = assert!(offset_of!(OaCifpCounts, runways) == 8);
const _: () = assert!(offset_of!(OaCifpCounts, localizers) == 16);
const _: () = assert!(offset_of!(OaCifpCounts, vhf_navaids) == 24);
const _: () = assert!(offset_of!(OaCifpCounts, ndbs) == 32);
const _: () = assert!(offset_of!(OaCifpCounts, waypoints) == 40);
const _: () = assert!(offset_of!(OaCifpCounts, moras) == 48);
const _: () = assert!(offset_of!(OaCifpCounts, airspaces) == 56);
const _: () = assert!(offset_of!(OaCifpCounts, airways) == 64);
const _: () = assert!(offset_of!(OaCifpCounts, airway_fixes) == 72);
const _: () = assert!(offset_of!(OaCifpCounts, procedures) == 80);
const _: () = assert!(offset_of!(OaCifpCounts, routes) == 88);
const _: () = assert!(offset_of!(OaCifpCounts, legs) == 96);
const _: () = assert!(offset_of!(OaCifpCounts, continuations) == 104);
const _: () = assert!(offset_of!(OaCifpCounts, levels_of_service) == 112);
const _: () = assert!(offset_of!(OaCifpCounts, boundaries) == 120);
const _: () = assert!(offset_of!(OaCifpCounts, header_records) == 128);
const _: () = assert!(offset_of!(OaCifpCounts, sections) == 136);
const _: () = assert!(offset_of!(OaCifpCounts, cycles) == 144);
const _: () = assert!(offset_of!(OaCifpCounts, errors) == 152);
const _: () = assert!(offset_of!(OaCifpCounts, errors_dropped) == 160);
const _: () = assert!(offset_of!(OaCifpCounts, strings_bytes) == 168);
const _: () = assert!(offset_of!(OaCifpCounts, output_bytes) == 176);
const _: () = assert!(offset_of!(OaCifpCounts, is_clean) == 184);
const _: () = assert!(offset_of!(OaCifpCounts, reserved) == 185);

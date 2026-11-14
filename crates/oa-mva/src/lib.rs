#![forbid(unsafe_code)]
//! oa-mva: the FAA Minimum Vectoring Altitude (MVA) charts and the center Minimum IFR Altitude
//! (MIA, "TAV") chart as a queryable model: a safety-net input for the minimum safe altitude
//! warning (M5). Given a position it returns the minimum altitude that applies there.
//!
//! * [`aixm`] (shell, offline): [`load_message`] reads one AIXM 5.1 message (FAA
//!   `aeronav.faa.gov/MVA_Charts/aixm`) with `oa-xml` into the [`Model`]. It accepts exactly the
//!   structure listed in that module's documentation and refuses everything else with a typed
//!   [`MvaError`] (RULESET E5); nothing is guessed and nothing is repaired.
//! * [`Model`] (core): struct-of-arrays sectors (facility, chart set, sector id, minimum altitude
//!   in feet MSL, ring ranges into one shared vertex table). Positions are stored in 1e-7
//!   degrees (A9), exactly as the source's decimal degrees.
//! * [`min_altitude_at`], [`sectors_within_radius`] (core, pure): the queries. A point on a
//!   sector's edge is inside it, and a point on a hole's edge is inside the sector too, so the
//!   queries err toward the higher altitude (the conservative side for a warning).
//! * [`check`] (shell, offline): the validation behind `oa-mva-check`: ring integrity, sector
//!   coverage gaps and overlaps, the PDF cross-check, and the clearance of each sector against
//!   terrain and obstacles.
//!
//! Geometry is planar in longitude/latitude: the AIXM surfaces are `interpolation="planar"` in
//! CRS84 and the FAA densifies arcs into vertices, so a straight edge here is the straight edge
//! of the source. All predicates are exact (128-bit integer arithmetic on the 1e-7 degree
//! coordinates). Distances for the radius query use a deliberately small metric (see
//! [`NM_PER_DEG_LAT`], [`NM_PER_DEG_LON_EQUATOR`]) so the answer never omits a sector that the
//! WGS-84 ellipsoid would include.

pub mod aixm;
pub mod check;
mod geom;
mod model;
mod query;

#[cfg(test)]
mod tests;

pub use aixm::{
    default_limits, load_directory, load_message, new_parser, read_file, DirLoad, Loaded,
};
pub use geom::{locate_in_ring, Where};
pub use model::{
    chart_count, chart_set_text, facility_text, new_model, ring_is_hole, ring_range, ring_vertices,
    sector_count, sector_name_text, sector_range_of_chart, Model, SectorIx, SetKind,
    SetKindParseError, SET_NAMES,
};
pub use query::{
    min_altitude_at, min_altitude_with_sub, sector_contains, sector_distance_nm,
    sectors_containing, sectors_within_radius, MinAlt,
};

use std::fmt;

/// Degrees to the stored integer unit: 1e-7 degree, about 1.1 cm of latitude.
pub const E7: i64 = 10_000_000;
/// Largest message accepted. The real files are 11 KB to 217 KB; this is about 18 times the
/// largest. (The `oa-xml` input limit is set to the same value.)
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Most sectors in one chart. Real charts have 2 to 37; this is about 13 times the largest.
pub const MAX_SECTORS_PER_CHART: usize = 500;
/// Most rings (exterior plus holes) in one sector polygon. Real sectors have at most a dozen.
pub const MAX_RINGS_PER_SECTOR: usize = 64;
/// Most vertices in one ring, closing vertex included. The real maximum is 802; this is about
/// 5 times that.
pub const MAX_VERTICES_PER_RING: usize = 4_096;
/// Fewest vertices of a ring, closing vertex included (a triangle).
pub const MIN_VERTICES_PER_RING: usize = 4;
/// Most vertices in the whole model (35 real charts hold about 60,000).
pub const MAX_VERTICES: usize = 2_000_000;
/// Most sectors in the whole model.
pub const MAX_SECTORS: usize = 20_000;
/// Most charts (one per facility and set) in the whole model.
pub const MAX_CHARTS: usize = 256;
/// Most bytes of one `posList` text.
pub const MAX_POSLIST_BYTES: usize = 1 << 20;
/// Longest facility identifier (`C90`, `ZAU`), bytes.
pub const MAX_FACILITY_BYTES: usize = 4;
/// Longest sub-area name of a sub-area chart (`LHW`), bytes.
pub const MAX_SUB_BYTES: usize = 4;
/// Longest sector identifier (`A`, `AA`, `GALO01`), bytes.
pub const MAX_SECTOR_ID_BYTES: usize = 8;
/// Lowest minimum altitude accepted, feet MSL.
pub const MIN_ALT_FT: i32 = 0;
/// Highest minimum altitude accepted, feet MSL. Class A airspace begins at 18,000 ft: no
/// vectoring or IFR minimum chart carries a higher value, so one is a corrupt value.
pub const MAX_ALT_FT: i32 = 18_000;
/// Widest extent of one sector, in degrees of latitude or longitude, whose planar edges are
/// trusted as the source's edges. The largest real sector spans about 7 degrees.
pub const MAX_SECTOR_SPAN_DEG: i64 = 20;
/// Largest radius the radius query accepts, nautical miles.
pub const MAX_RADIUS_NM: f64 = 200.0;
/// Nautical miles per degree of latitude, lower bound over WGS-84 (110,574 m at the equator).
/// Using the minimum makes a distance too small, never too large.
pub const NM_PER_DEG_LAT: f64 = 110_574.0 / 1852.0;
/// Nautical miles per degree of longitude on the equator, lower bound (the WGS-84 equatorial
/// radius; the parallel radius is never smaller). Scaled by the cosine of the highest latitude
/// the query can reach.
pub const NM_PER_DEG_LON_EQUATOR: f64 = 6_378_137.0 * (std::f64::consts::PI / 180.0) / 1852.0;

/// Why a message was refused or a query was refused. Every variant that comes from the file
/// carries the byte offset of the offending element, so a refusal can be traced to the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MvaError {
    /// The file could not be read.
    Io(String),
    /// The file is larger than [`MAX_FILE_BYTES`].
    FileTooLarge(u64),
    /// The XML itself is not acceptable to `oa-xml`.
    Xml(oa_xml::XmlError),
    /// The root is not an AIXM 5.1 basic message.
    NotAixmMessage { at: u32 },
    /// An element this reader does not understand (not in the accepted structure) inside
    /// `context`. `name` is its local name.
    UnexpectedElement {
        context: &'static str,
        name: String,
        at: u32,
    },
    /// An attribute this reader does not understand, `name` its local name.
    UnexpectedAttribute { name: String, at: u32 },
    /// An element that must appear exactly once is missing or repeated.
    Cardinality {
        context: &'static str,
        name: &'static str,
        found: usize,
        at: u32,
    },
    /// A message description that is not one of the chart names this reader knows.
    UnknownChartName { name: String, at: u32 },
    /// A value outside what this reader accepts for `field`.
    BadValue { field: &'static str, at: u32 },
    /// A ring whose first and last vertices differ.
    RingNotClosed { at: u32 },
    /// A count over a stated bound: `what` names the bound, `max` its value.
    Limit {
        what: &'static str,
        max: u64,
        at: u32,
    },
    /// The same facility and chart set was already loaded.
    DuplicateChart { facility: String, set: String },
    /// Two sectors in one chart share an identifier.
    DuplicateSector { name: String, at: u32 },
    /// A query coordinate outside latitude [-90, 90] or longitude [-180, 180], or not finite.
    BadCoordinate,
    /// A radius that is not finite, not positive or over [`MAX_RADIUS_NM`].
    BadRadius,
}

impl fmt::Display for MvaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MvaError::Io(e) => write!(f, "cannot read file: {e}"),
            MvaError::FileTooLarge(n) => {
                write!(f, "file is {n} bytes, over the {MAX_FILE_BYTES} byte limit")
            }
            MvaError::Xml(e) => write!(f, "XML: {e}"),
            MvaError::NotAixmMessage { at } => {
                write!(f, "not an AIXM 5.1 basic message (byte {at})")
            }
            MvaError::UnexpectedElement { context, name, at } => {
                write!(f, "unsupported element <{name}> in {context} (byte {at})")
            }
            MvaError::UnexpectedAttribute { name, at } => {
                write!(f, "unsupported attribute {name} (byte {at})")
            }
            MvaError::Cardinality {
                context,
                name,
                found,
                at,
            } => write!(
                f,
                "{context} needs exactly one <{name}>, found {found} (byte {at})"
            ),
            MvaError::UnknownChartName { name, at } => write!(
                f,
                "unknown chart name {name:?} (byte {at}); accepted: <FAC>_MVA_FUS3|FUS5[_year][_vN], <FAC>_TAV[_year][_vN], <FAC>_<SUB>_MVA[_year][_vN]"
            ),
            MvaError::BadValue { field, at } => write!(f, "bad {field} (byte {at})"),
            MvaError::RingNotClosed { at } => {
                write!(
                    f,
                    "ring does not close: first and last vertex differ (byte {at})"
                )
            }
            MvaError::Limit { what, max, at } => {
                write!(f, "{what} exceeds the limit of {max} (byte {at})")
            }
            MvaError::DuplicateChart { facility, set } => {
                write!(f, "chart {facility} {set} is already loaded")
            }
            MvaError::DuplicateSector { name, at } => {
                write!(f, "sector {name} appears twice in one chart (byte {at})")
            }
            MvaError::BadCoordinate => {
                write!(f, "latitude or longitude not finite or out of range")
            }
            MvaError::BadRadius => {
                write!(
                    f,
                    "radius not positive, finite and at most {MAX_RADIUS_NM} nm"
                )
            }
        }
    }
}

impl std::error::Error for MvaError {}

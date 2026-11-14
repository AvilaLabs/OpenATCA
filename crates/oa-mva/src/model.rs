//! The chart model. Core (pure): plain data and free functions, no I/O.
//!
//! Sectors are struct-of-arrays (A3). A sector owns a contiguous range of rings (the first is
//! the exterior, the rest are holes); a ring owns a contiguous range of the shared vertex
//! table, closing vertex included, exactly as the source gives it. Charts (one per facility and
//! set) own contiguous ranges of sectors.

use crate::{MAX_FACILITY_BYTES, MAX_SECTOR_ID_BYTES};
use std::fmt;
use std::ops::Range;

/// Which published set a chart belongs to. FUS3 and FUS5 are the two MVA sets of a facility (3
/// and 5 nautical mile separation sectors); TAV is the center's Minimum IFR Altitude chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SetKind {
    Fus3 = 0,
    Fus5 = 1,
    Tav = 2,
}

/// Display names of [`SetKind`], indexed by the discriminant.
pub const SET_NAMES: [&str; 3] = ["FUS3", "FUS5", "TAV"];

impl SetKind {
    /// The published name (`FUS3`, `FUS5`, `TAV`).
    pub const fn name(self) -> &'static str {
        match self {
            SetKind::Fus3 => "FUS3",
            SetKind::Fus5 => "FUS5",
            SetKind::Tav => "TAV",
        }
    }
}

/// A set name that is not FUS3, FUS5 or TAV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetKindParseError;

impl fmt::Display for SetKindParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "set must be FUS3, FUS5 or TAV")
    }
}

impl std::str::FromStr for SetKind {
    type Err = SetKindParseError;
    fn from_str(s: &str) -> Result<SetKind, SetKindParseError> {
        match s {
            "FUS3" => Ok(SetKind::Fus3),
            "FUS5" => Ok(SetKind::Fus5),
            "TAV" => Ok(SetKind::Tav),
            _ => Err(SetKindParseError),
        }
    }
}

/// Index of a sector row. `NONE` means "no such sector".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SectorIx(pub u32);
const _: () = assert!(core::mem::size_of::<SectorIx>() == 4);

impl SectorIx {
    pub const NONE: SectorIx = SectorIx(u32::MAX);
}

/// All loaded charts. Plain data (A1). Index spaces: sector rows (`facility` .. `bbox_lon_max`),
/// ring rows (`ring_*`), vertex rows (`vert_*`), chart rows (`chart_*`). A ring's hole flag is a
/// bitset (A6). Built only by [`crate::load_message`], which keeps every range inside its table.
// A7.2: cold struct, size guard waived (the columns are the hot data; each is a plain Vec)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    /// Facility identifier, ASCII, NUL padded (`C90\0`).
    pub facility: Vec<[u8; MAX_FACILITY_BYTES]>,
    /// The chart set of the sector.
    pub set: Vec<SetKind>,
    /// Sector identifier, ASCII, NUL padded.
    pub sector_id: Vec<[u8; MAX_SECTOR_ID_BYTES]>,
    /// Minimum altitude, feet MSL.
    pub min_alt_ft: Vec<i32>,
    /// First ring of the sector (the exterior); its holes follow.
    pub ring_first: Vec<u32>,
    /// Rings of the sector, the exterior included (at least 1).
    pub ring_count: Vec<u32>,
    /// Bounding box of the exterior ring, 1e-7 degrees.
    pub bbox_lat_min: Vec<i32>,
    pub bbox_lat_max: Vec<i32>,
    pub bbox_lon_min: Vec<i32>,
    pub bbox_lon_max: Vec<i32>,
    /// First vertex row of the ring.
    pub ring_vert_first: Vec<u32>,
    /// Vertices of the ring, closing vertex included (at least 4).
    pub ring_vert_count: Vec<u32>,
    /// Bit `r` is set when ring `r` is a hole.
    pub ring_hole: Vec<u64>,
    /// Vertex latitude, 1e-7 degrees.
    pub vert_lat_e7: Vec<i32>,
    /// Vertex longitude, 1e-7 degrees.
    pub vert_lon_e7: Vec<i32>,
    /// Chart facility, as in `facility`.
    pub chart_facility: Vec<[u8; MAX_FACILITY_BYTES]>,
    /// Chart set.
    pub chart_set: Vec<SetKind>,
    /// First sector of the chart.
    pub chart_first_sector: Vec<u32>,
    /// Sectors in the chart.
    pub chart_sector_count: Vec<u32>,
}

/// An empty model.
pub fn new_model() -> Model {
    Model::default()
}

/// Sectors in the model.
pub fn sector_count(m: &Model) -> usize {
    m.min_alt_ft.len()
}

/// Charts in the model.
pub fn chart_count(m: &Model) -> usize {
    m.chart_set.len()
}

/// The sector rows of chart `c`; empty for a chart row that does not exist.
pub fn sector_range_of_chart(m: &Model, c: usize) -> Range<usize> {
    let first = m.chart_first_sector.get(c).map_or(0, |&v| v as usize);
    let n = m.chart_sector_count.get(c).map_or(0, |&v| v as usize);
    first..first.saturating_add(n)
}

/// The ring rows of sector `s` (the exterior first); empty when `s` does not exist.
pub fn ring_range(m: &Model, s: SectorIx) -> Range<usize> {
    let i = s.0 as usize;
    let first = m.ring_first.get(i).map_or(0, |&v| v as usize);
    let n = m.ring_count.get(i).map_or(0, |&v| v as usize);
    first..first.saturating_add(n)
}

/// Whether ring row `r` is a hole.
pub fn ring_is_hole(m: &Model, r: usize) -> bool {
    m.ring_hole
        .get(r >> 6)
        .is_some_and(|w| (w >> (r & 63)) & 1 == 1)
}

/// The vertices of ring row `r` as (latitude, longitude) slices in 1e-7 degrees; empty slices
/// when the ring does not exist.
pub fn ring_vertices(m: &Model, r: usize) -> (&[i32], &[i32]) {
    let first = m.ring_vert_first.get(r).map_or(0, |&v| v as usize);
    let n = m.ring_vert_count.get(r).map_or(0, |&v| v as usize);
    let end = first.saturating_add(n);
    (
        m.vert_lat_e7.get(first..end).unwrap_or(&[]),
        m.vert_lon_e7.get(first..end).unwrap_or(&[]),
    )
}

fn padded_text(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(b.get(..n).unwrap_or(&[])).into_owned()
}

/// The facility identifier of sector `s` as text; empty when `s` does not exist.
pub fn facility_text(m: &Model, s: SectorIx) -> String {
    m.facility
        .get(s.0 as usize)
        .map(|b| padded_text(b))
        .unwrap_or_default()
}

/// The sector identifier of sector `s` as text; empty when `s` does not exist.
pub fn sector_name_text(m: &Model, s: SectorIx) -> String {
    m.sector_id
        .get(s.0 as usize)
        .map(|b| padded_text(b))
        .unwrap_or_default()
}

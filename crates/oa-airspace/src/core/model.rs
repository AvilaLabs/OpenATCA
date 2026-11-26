//! The airspace model: plain tables, typed indexes, fixed-point units. **Core** (B1, B7): pure data
//! and pure functions over it; nothing here does I/O.
//!
//! Layout rules (docs/M0_DESIGN.md "Model layout", RULESET A1, A3, A4, A6, A7, A9):
//!
//! * Every table is struct-of-arrays (A3): one `Vec` per column, all columns of a table sharing
//!   one row index. The only array-element structs are [`PosE7`] and [`Span`], and each carries
//!   an exact `size_of` guard (A7).
//! * Cross-table references are `u32` index newtypes (A4) with a `NONE` sentinel instead of
//!   `Option`. A span (`start`, `len`) of a parent row selects the rows of a child table.
//! * Sparse flags are bitsets stored out of band (A6): [`BitSet`] columns next to the
//!   byte/word columns, not `bool` fields.
//! * Latitude and longitude are `i32` in 1e-7 degree, altitudes `i32` feet, distances `u32`
//!   metres, bearings `u16` hundredths of a degree, speeds `u16` knots (A9). Sentinels for "no
//!   value" are named constants below; none of them is ever a legitimate value.
//! * Records have no methods (A1). Behaviour is in free functions. [`StrTable`] and [`BitSet`]
//!   are containers (subsystems under D5), not records, and are reached through free functions
//!   too, to keep one calling style.
//!
//! Every table is kept sorted by its stable key so lookups are binary searches and so a snapshot
//! never depends on hash-map order; [`validate_model`] checks that, and every index and span.

use std::cmp::Ordering;

/// Sentinel for an absent `u32` index.
pub const NONE_U32: u32 = u32::MAX;

// Each index newtype below is written out in full: `u32` inside, a private field (A4, A5), a
// `NONE` sentinel, and `new`, `get` and `row`. Core code defines no macro (B8).

/// Index into the string table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct StrIx(u32);

impl StrIx {
    /// Absent reference.
    pub const NONE: StrIx = StrIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> StrIx {
        StrIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Points`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PointIx(u32);

impl PointIx {
    /// Absent reference.
    pub const NONE: PointIx = PointIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> PointIx {
        PointIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Vertices`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VertexIx(u32);

impl VertexIx {
    /// Absent reference.
    pub const NONE: VertexIx = VertexIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> VertexIx {
        VertexIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Segments`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SegIx(u32);

impl SegIx {
    /// Absent reference.
    pub const NONE: SegIx = SegIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> SegIx {
        SegIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Boundaries`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct BoundaryIx(u32);

impl BoundaryIx {
    /// Absent reference.
    pub const NONE: BoundaryIx = BoundaryIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> BoundaryIx {
        BoundaryIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Airways`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct AirwayIx(u32);

impl AirwayIx {
    /// Absent reference.
    pub const NONE: AirwayIx = AirwayIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> AirwayIx {
        AirwayIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`AirwayLegs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct AirwayLegIx(u32);

impl AirwayLegIx {
    /// Absent reference.
    pub const NONE: AirwayLegIx = AirwayLegIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> AirwayLegIx {
        AirwayLegIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Procs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ProcIx(u32);

impl ProcIx {
    /// Absent reference.
    pub const NONE: ProcIx = ProcIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> ProcIx {
        ProcIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Routes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RouteIx(u32);

impl RouteIx {
    /// Absent reference.
    pub const NONE: RouteIx = RouteIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> RouteIx {
        RouteIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`Legs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct LegIx(u32);

impl LegIx {
    /// Absent reference.
    pub const NONE: LegIx = LegIx(NONE_U32);

    /// The index for raw row number `raw`. The field is private (A4, A5): the raw value
    /// crosses a module boundary only through [`Self::new`] and [`Self::get`].
    pub const fn new(raw: u32) -> LegIx {
        LegIx(raw)
    }

    /// The raw row number (the [`NONE_U32`] sentinel for [`Self::NONE`]).
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The row as a `usize` for slicing a column; a `u32` always fits.
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// Row of [`PreferredRouteSegments`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PreferredRouteSegmentIx(u32);

impl PreferredRouteSegmentIx {
    pub const NONE: PreferredRouteSegmentIx = PreferredRouteSegmentIx(NONE_U32);
    pub const fn new(raw: u32) -> PreferredRouteSegmentIx {
        PreferredRouteSegmentIx(raw)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
    pub const fn row(self) -> usize {
        self.0 as usize
    }
}

/// A latitude/longitude pair in 1e-7 degree. Stored as an array element, so it is guarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PosE7 {
    pub lat: i32,
    pub lon: i32,
}
// Budget: two i32, no padding. Held in quantity (every boundary vertex), read as pairs.
const _: () = assert!(core::mem::size_of::<PosE7>() == 8);

/// A run of child rows: `len` rows starting at `start`. `I` is the child table's index type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Span<I> {
    pub start: I,
    pub len: u32,
}
// Budget: one u32 index and one u32 count, no padding.
const _: () = assert!(core::mem::size_of::<Span<VertexIx>>() == 8);
const _: () = assert!(core::mem::size_of::<Span<SegIx>>() == 8);
const _: () = assert!(core::mem::size_of::<Span<AirwayLegIx>>() == 8);
const _: () = assert!(core::mem::size_of::<Span<RouteIx>>() == 8);
const _: () = assert!(core::mem::size_of::<Span<LegIx>>() == 8);
const _: () = assert!(core::mem::size_of::<Span<PreferredRouteSegmentIx>>() == 8);

/// Latitude/longitude sentinel: no position (a circle's missing point, an absent arc centre).
pub const POS_NONE: i32 = i32::MIN;
/// Altitude sentinel: absent.
pub const ALT_NONE: i32 = i32::MIN;
/// Altitude sentinel: the source says ground level (`GND`).
pub const ALT_GROUND: i32 = i32::MIN + 1;
/// Altitude sentinel: the source says the value is unknown (`UNKNN`).
pub const ALT_UNKNOWN: i32 = i32::MIN + 2;
/// Altitude sentinel: unlimited (`UNLTD`).
pub const ALT_UNLIMITED: i32 = i32::MAX;
/// Elevation sentinel: absent.
pub const ELEV_NONE: i32 = i32::MIN;
/// `u16` sentinel (course, speed): absent.
pub const NONE_U16: u16 = u16::MAX;
/// MORA value: the FAA publishes `UNK` for the cell (a known unknown, not an absent cell).
pub const MORA_UNKNOWN: i16 = i16::MIN;

/// Point kinds (`Points::kind`).
pub mod point_kind {
    pub const FIX: u8 = 1;
    pub const VOR: u8 = 2;
    pub const VORTAC: u8 = 3;
    pub const VOR_DME: u8 = 4;
    pub const DME: u8 = 5;
    pub const TACAN: u8 = 6;
    pub const NDB: u8 = 7;
    pub const AIRPORT: u8 = 8;
    pub const THRESHOLD: u8 = 9;
    pub const TERMINAL_WAYPOINT: u8 = 10;
    pub const LOCALIZER: u8 = 11;
    pub const MAX: u8 = 11;

    /// Display name of a kind.
    pub fn name(k: u8) -> &'static str {
        match k {
            FIX => "fix",
            VOR => "VOR",
            VORTAC => "VORTAC",
            VOR_DME => "VOR/DME",
            DME => "DME",
            TACAN => "TACAN",
            NDB => "NDB",
            AIRPORT => "airport",
            THRESHOLD => "runway threshold",
            TERMINAL_WAYPOINT => "terminal waypoint",
            LOCALIZER => "localizer",
            _ => "unknown kind",
        }
    }

    /// The identity class of a kind: kinds in one class are the same object seen by different
    /// sources (a NASR fix and a CIFP enroute or terminal waypoint; a NASR VORTAC and a CIFP
    /// VHF navaid), so they may not coexist under one (ident, region).
    pub fn class(k: u8) -> u8 {
        match k {
            FIX | TERMINAL_WAYPOINT => FIX,
            VOR | VORTAC | VOR_DME | DME | TACAN => VOR,
            other => other,
        }
    }
}

/// Boundary source (`Boundaries::source`): which file the geometry came from.
pub mod boundary_source {
    pub const NASR_ARB: u8 = 1;
    pub const CIFP_UC: u8 = 2;
    pub const CIFP_UR: u8 = 3;
    pub const MAX: u8 = 3;
}

/// Special-use time code (`Boundaries::time_code`), from the source's UR column 27.
pub mod time_code {
    /// Not given: NASR and controlled-airspace boundaries, or a letter this reader does not know.
    pub const UNKNOWN: u8 = 0;
    /// `C`: continuous (FAA readme).
    pub const CONTINUOUS: u8 = 1;
    /// Blank: part-time (FAA readme); the schedule itself is not in the source.
    pub const PART_TIME: u8 = 2;
    pub const MAX: u8 = 2;
}

/// Special-use level (`Boundaries::level`), from the source's UR column 26.
pub mod level {
    /// Not given (NASR, controlled airspace, blank or unrecognised).
    pub const UNKNOWN: u8 = 0;
    pub const HIGH: u8 = 1;
    pub const LOW: u8 = 2;
    pub const BOTH: u8 = 3;
    pub const MAX: u8 = 3;
}

/// Boundary stratum (`Boundaries::stratum`).
pub mod stratum {
    pub const ARTCC_HIGH: u8 = 1;
    pub const ARTCC_LOW: u8 = 2;
    /// ARB altitude `UNLIMITED` with type ARTCC (does not occur in the current data).
    pub const ARTCC_UNLIMITED: u8 = 3;
    pub const FIR: u8 = 4;
    pub const CTA: u8 = 5;
    pub const CTA_FIR: u8 = 6;
    pub const UTA: u8 = 7;
    pub const CLASS_B: u8 = 8;
    pub const CLASS_C: u8 = 9;
    pub const CLASS_D: u8 = 10;
    /// UC volume whose classification letter is not B, C or D.
    pub const CLASS_OTHER: u8 = 11;
    pub const SUA_RESTRICTED: u8 = 12;
    pub const SUA_MOA: u8 = 13;
    pub const SUA_WARNING: u8 = 14;
    pub const SUA_ALERT: u8 = 15;
    pub const SUA_PROHIBITED: u8 = 16;
    pub const SUA_OTHER: u8 = 17;
    pub const MAX: u8 = 17;

    /// Display name of a stratum.
    pub fn name(s: u8) -> &'static str {
        match s {
            ARTCC_HIGH => "ARTCC HIGH",
            ARTCC_LOW => "ARTCC LOW",
            ARTCC_UNLIMITED => "ARTCC UNLIMITED",
            FIR => "FIR",
            CTA => "CTA",
            CTA_FIR => "CTA/FIR",
            UTA => "UTA",
            CLASS_B => "class B",
            CLASS_C => "class C",
            CLASS_D => "class D",
            CLASS_OTHER => "class other",
            SUA_RESTRICTED => "SUA restricted",
            SUA_MOA => "SUA MOA",
            SUA_WARNING => "SUA warning",
            SUA_ALERT => "SUA alert",
            SUA_PROHIBITED => "SUA prohibited",
            SUA_OTHER => "SUA other",
            _ => "unknown stratum",
        }
    }
}

/// How consecutive boundary vertices are joined (`Boundaries::edge_interp`).
pub mod edge_interp {
    /// The source does not say (NASR ARB, UK-001): both interpretations are possible.
    pub const UNKNOWN: u8 = 0;
    /// Each segment's own `via` code says (CIFP).
    pub const PER_SEGMENT: u8 = 1;
    pub const MAX: u8 = 1;
}

/// Segment via codes (`Segments::via`), ARINC 424 boundary via.
pub mod via {
    pub const CIRCLE: u8 = 1;
    pub const GEODESIC: u8 = 2;
    pub const RHUMB: u8 = 3;
    pub const ARC_CCW: u8 = 4;
    pub const ARC_CW: u8 = 5;
    pub const MAX: u8 = 5;
}

/// Procedure kinds (`Procs::kind`).
pub mod proc_kind {
    pub const SID: u8 = 1;
    pub const STAR: u8 = 2;
    pub const APPROACH: u8 = 3;
    /// Obstacle departure procedure (NASR `DP` of graphical type `OBSTACLE`); NASR only.
    pub const ODP: u8 = 4;
    pub const MAX: u8 = 4;

    /// Display name of a kind.
    pub fn name(k: u8) -> &'static str {
        match k {
            SID => "SID",
            STAR => "STAR",
            APPROACH => "approach",
            ODP => "ODP",
            _ => "unknown procedure kind",
        }
    }
}

/// Altitude description codes (`Legs::alt_desc`). Only the codes UNKNOWNS UK-002 does not leave
/// open are decoded; every other coded value becomes [`UNVERIFIED`].
pub mod alt_desc {
    pub const NONE: u8 = 0;
    /// `+`: at or above altitude 1.
    pub const AT_OR_ABOVE: u8 = 1;
    /// `B`: between (altitude 1 is the higher, altitude 2 the lower).
    pub const BETWEEN: u8 = 4;
    /// Some other code, not decoded (UK-002).
    pub const UNVERIFIED: u8 = 255;
}

/// Turn directions (`Legs::turn`).
pub mod turn {
    pub const NONE: u8 = 0;
    pub const LEFT: u8 = 1;
    pub const RIGHT: u8 = 2;
    pub const EITHER: u8 = 3;
}

/// Path terminators (`Legs::path_term`); 0 means the leg is not coded (NASR-only route point).
pub mod path_term {
    pub const NONE: u8 = 0;
    pub const NAMES: [&str; 23] = [
        "AF", "CA", "CD", "CF", "CI", "CR", "DF", "FA", "FC", "FD", "FM", "HA", "HF", "HM", "IF",
        "PI", "RF", "TF", "VA", "VD", "VI", "VM", "VR",
    ];
    pub const MAX: u8 = 23;

    /// Display name of a terminator code.
    pub fn name(c: u8) -> &'static str {
        match usize::from(c).checked_sub(1).and_then(|i| NAMES.get(i)) {
            Some(n) => n,
            None => "--",
        }
    }
}

/// MEA direction codes (`AirwayLegs::mea_dir`).
pub mod compass {
    pub const NONE: u8 = 0;
    pub const NORTH: u8 = 1;
    pub const NORTHEAST: u8 = 2;
    pub const EAST: u8 = 3;
    pub const SOUTHEAST: u8 = 4;
    pub const SOUTH: u8 = 5;
    pub const SOUTHWEST: u8 = 6;
    pub const WEST: u8 = 7;
    pub const NORTHWEST: u8 = 8;
    /// Some other text, not decoded.
    pub const OTHER: u8 = 255;
}

/// Roles of an unresolved reference (`Unresolved::role`).
pub mod unresolved_role {
    pub const AIRWAY_FROM: u8 = 1;
    pub const AIRWAY_TO: u8 = 2;
    pub const LEG_FIX: u8 = 3;
    pub const LEG_CENTER: u8 = 4;
    pub const ROUTE_POINT: u8 = 5;
    /// A NASR airway pseudo-point `U.S. CANADIAN BORDER-n` / `U.S. MEXICAN BORDER-n`: the
    /// airway crosses the border there and the point has no position. Expected, not an error:
    /// reports list these apart from real unresolved references.
    pub const BORDER_CROSSING: u8 = 6;
    pub const MAX: u8 = 6;

    /// Display name of a role.
    pub fn name(r: u8) -> &'static str {
        match r {
            AIRWAY_FROM => "airway leg start",
            AIRWAY_TO => "airway leg end",
            LEG_FIX => "procedure leg fix",
            LEG_CENTER => "procedure arc/MSA centre",
            ROUTE_POINT => "NASR route point",
            BORDER_CROSSING => "airway border crossing",
            _ => "unknown role",
        }
    }
}

/// Owner kinds of an unresolved reference.
pub mod owner_kind {
    pub const AIRWAY: u8 = 1;
    pub const PROCEDURE: u8 = 2;
    pub const MAX: u8 = 2;
}

/// A bitset column (A6): `len` bits packed into 64-bit words, bits at and beyond `len` zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct BitSet {
    pub words: Vec<u64>,
    pub len: u32,
}

/// Number of 64-bit words holding `len` bits.
pub fn bit_words(len: usize) -> usize {
    len.div_ceil(64)
}

/// An all-zero bitset of `len` bits.
pub fn bit_zeros(len: usize) -> BitSet {
    BitSet {
        words: vec![0; bit_words(len)],
        len: u32::try_from(len).unwrap_or(u32::MAX),
    }
}

/// Bit `i`; false when out of range.
pub fn bit_get(b: &BitSet, i: usize) -> bool {
    if i >= b.len as usize {
        return false;
    }
    b.words
        .get(i / 64)
        .is_some_and(|w| (w >> (i % 64)) & 1 == 1)
}

/// Sets bit `i`; out of range is ignored.
pub fn bit_set(b: &mut BitSet, i: usize, v: bool) {
    if i >= b.len as usize {
        return;
    }
    if let Some(w) = b.words.get_mut(i / 64) {
        if v {
            *w |= 1u64 << (i % 64);
        } else {
            *w &= !(1u64 << (i % 64));
        }
    }
}

/// Appends one bit.
pub fn bit_push(b: &mut BitSet, v: bool) {
    let i = b.len as usize;
    if i.is_multiple_of(64) {
        b.words.push(0);
    }
    b.len = b.len.saturating_add(1);
    bit_set(b, i, v);
}

/// Number of set bits.
pub fn bit_count(b: &BitSet) -> usize {
    b.words.iter().map(|w| w.count_ones() as usize).sum()
}

/// A string table: all strings concatenated, `offsets` holds `count + 1` byte positions.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct StrTable {
    pub text: String,
    pub offsets: Vec<u32>,
}

impl Default for StrTable {
    fn default() -> Self {
        StrTable {
            text: String::new(),
            offsets: vec![0],
        }
    }
}

/// Number of strings in the table.
pub fn str_count(t: &StrTable) -> usize {
    t.offsets.len().saturating_sub(1)
}

/// The string at `ix`; `None` for `StrIx::NONE` or an index out of range.
pub fn str_at(t: &StrTable, ix: StrIx) -> Option<&str> {
    let i = ix.0 as usize;
    let a = *t.offsets.get(i)? as usize;
    let b = *t.offsets.get(i.checked_add(1)?)? as usize;
    t.text.get(a..b)
}

/// The string at `ix`, or `""` when absent (for display and key comparison).
pub fn str_or_empty(t: &StrTable, ix: StrIx) -> &str {
    str_at(t, ix).unwrap_or("")
}

/// A column element by raw index; `None` when out of range (including every `NONE`).
pub fn at<T: Copy>(col: &[T], raw: u32) -> Option<T> {
    col.get(raw as usize).copied()
}

/// Input identity and selection parameters; cold (A7.2: cold struct, size guard waived).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// NASR effective date as `YYYYMMDD`.
    pub nasr_effective: u32,
    /// CIFP cycle as `YYRR` (2611 is release 11 of 2026).
    pub cifp_cycle: u32,
    /// The ARTCC identifier the model was selected for (`ZAU`).
    pub artcc: String,
    /// Selection buffer in metres.
    pub buffer_m: u32,
    /// SHA-256 of the NASR CSV archive.
    pub nasr_sha256: [u8; 32],
    /// SHA-256 of the CIFP archive.
    pub cifp_sha256: [u8; 32],
}

/// Points: fixes, navaids, airports, runway thresholds, terminal waypoints, localizers.
/// Identity is (class of kind, ident, region, scope); sorted by (ident, region, scope, class).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Points {
    pub kind: Vec<u8>,
    pub lat: Vec<i32>,
    pub lon: Vec<i32>,
    /// Feet; [`ELEV_NONE`] when not published.
    pub elev_ft: Vec<i32>,
    /// The point's identifier. Airports: the ICAO identifier when there is one, else the FAA's.
    pub ident: Vec<StrIx>,
    /// Airports: the FAA identifier when `ident` is the ICAO one and they differ.
    pub alt_ident: Vec<StrIx>,
    /// ICAO region code (`K5`); NONE where the sources give none (NASR-only navaids/airports).
    pub region: Vec<StrIx>,
    /// Runway thresholds and localizers: the airport's `ident`. NONE otherwise.
    pub scope: Vec<StrIx>,
    pub name: Vec<StrIx>,
    /// Row came from NASR.
    pub src_nasr: BitSet,
    /// Row came from CIFP.
    pub src_cifp: BitSet,
    /// Inside the selection area; unset for a point pulled in only because a selected
    /// procedure or airway references it.
    pub in_area: BitSet,
    /// Airports: NASR `CLS_ARSP` says the airport has class B, C, D, E airspace.
    pub cls_b: BitSet,
    pub cls_c: BitSet,
    pub cls_d: BitSet,
    pub cls_e: BitSet,
}

/// Boundary vertices of NASR-sourced boundaries, one shared array.
pub type Vertices = Vec<PosE7>;

/// Boundaries: ARTCC (NASR), class B/C/D and SUA (CIFP). Sorted by
/// (source, stratum, facility, multiple code, name).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Boundaries {
    pub source: Vec<u8>,
    pub stratum: Vec<u8>,
    pub edge_interp: Vec<u8>,
    /// UC/UR multiple code (an ASCII letter distinguishing volumes of one designation); 0 for
    /// NASR boundaries.
    pub multiple: Vec<u8>,
    /// ARTCC/FIR identifier (`ZAU`), UC airport identifier (`KORD`) or UR designation.
    pub facility: Vec<StrIx>,
    pub name: Vec<StrIx>,
    /// Feet; [`ALT_NONE`] where the source gives none (NASR boundaries).
    pub floor_ft: Vec<i32>,
    pub ceiling_ft: Vec<i32>,
    pub vertices: Vec<Span<VertexIx>>,
    pub segs: Vec<Span<SegIx>>,
    /// The floor is above ground level (otherwise mean sea level).
    pub floor_agl: BitSet,
    pub ceiling_agl: BitSet,
    /// Special-use time code ([`time_code`]); unknown for every other boundary.
    pub time_code: Vec<u8>,
    /// Special-use level ([`level`]); unknown for every other boundary.
    pub level: Vec<u8>,
    /// The boundary is a CIFP volume whose records end without the end marker (ARINC 424
    /// boundary via, second character `E`) and whose first and last points differ: the source
    /// does not say how the outline closes, so the shape is not precise (OA-ASP-069). Clear for
    /// a complete volume and for every NASR boundary. Use [`boundary_is_complete`].
    pub incomplete: BitSet,
}

/// Boundary segments of CIFP-sourced boundaries: one row per ARINC 424 boundary record. The
/// via code says how this row's point joins the next one's; arcs and circles carry their centre.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Segments {
    pub via: Vec<u8>,
    /// [`POS_NONE`] for a circle (the circle has no boundary point, only a centre).
    pub lat: Vec<i32>,
    pub lon: Vec<i32>,
    pub arc_lat: Vec<i32>,
    pub arc_lon: Vec<i32>,
    /// Arc or circle radius, metres; 0 where there is none.
    pub radius_m: Vec<u32>,
    /// True bearing from the arc centre to this row's point, hundredths of a degree;
    /// [`NONE_U16`] where there is none.
    pub bearing_cdeg: Vec<u16>,
}

/// Airways (NASR `AWY_BASE`/`AWY_SEG_ALT`). Sorted by (designator, location).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Airways {
    pub designator: Vec<StrIx>,
    /// NASR `AWY_LOCATION` letter (`C`, `A`, `H`).
    pub location: Vec<u8>,
    pub legs: Vec<Span<AirwayLegIx>>,
}

/// Airway legs in route order within each airway.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AirwayLegs {
    pub from: Vec<PointIx>,
    pub to: Vec<PointIx>,
    /// Minimum enroute altitude, feet; [`ALT_NONE`] when absent.
    pub mea_ft: Vec<i32>,
    pub mea_opp_ft: Vec<i32>,
    pub gps_mea_ft: Vec<i32>,
    /// Direction the MEA applies to ([`compass`]).
    pub mea_dir: Vec<u8>,
    pub gps_mea_dir: Vec<u8>,
    pub gap: BitSet,
    pub signal_gap: BitSet,
    pub dogleg: BitSet,
}

/// Procedures. Sorted by (airport ident, airport region, kind, ident).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Procs {
    pub kind: Vec<u8>,
    pub airport: Vec<PointIx>,
    pub ident: Vec<StrIx>,
    /// NASR name (`BENKY SIX`) where NASR has the procedure; NONE otherwise.
    pub name: Vec<StrIx>,
    pub routes: Vec<Span<RouteIx>>,
    pub src_nasr: BitSet,
    pub src_cifp: BitSet,
    /// The procedure has coded (CIFP) legs. Unset for procedures only NASR has (UK-003).
    pub coded: BitSet,
}

/// Routes: one per transition (or common route) of a procedure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Routes {
    /// CIFP route type character (`4`, `5`, `6`, `A`, `I`, ...); 0 for a NASR-only route.
    pub route_type: Vec<u8>,
    /// Transition identifier (CIFP) or route name (NASR).
    pub transition: Vec<StrIx>,
    pub legs: Vec<Span<LegIx>>,
}

/// Published FAA preferred IFR routes, retained as advisory source records. Nonempty condition
/// descriptions remain opaque source text because the NASR schema provides no parsing grammar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct PreferredRoutes {
    pub eff_date: Vec<StrIx>,
    pub origin: Vec<StrIx>,
    pub destination: Vec<StrIx>,
    pub route_type: Vec<StrIx>,
    pub route_no: Vec<u32>,
    pub area_description: Vec<StrIx>,
    pub altitude_description: Vec<StrIx>,
    pub aircraft_description: Vec<StrIx>,
    pub hours_description: Vec<StrIx>,
    pub direction_description: Vec<StrIx>,
    pub route_string: Vec<StrIx>,
    pub segments: Vec<Span<PreferredRouteSegmentIx>>,
}

/// Ordered FAA `PFR_SEG` rows for [`PreferredRoutes`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct PreferredRouteSegments {
    pub sequence: Vec<u32>,
    pub value: Vec<StrIx>,
    pub segment_type: Vec<StrIx>,
    pub state_code: Vec<StrIx>,
    pub country_code: Vec<StrIx>,
    pub icao_region_code: Vec<StrIx>,
    pub nav_type: Vec<StrIx>,
    pub next_segment: Vec<StrIx>,
}

/// Versioned capability for preferred-route data in an adaptation snapshot.
pub mod preferred_route_state {
    /// This model version predates the PFR table or the source was unavailable at build time.
    pub const UNAVAILABLE: u8 = 0;
    /// A complete NASR PFR source was parsed and included (it may contain zero records).
    pub const PRESENT: u8 = 1;
    pub const MAX: u8 = PRESENT;
}

/// Procedure legs in sequence order within each route.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Legs {
    pub fix: Vec<PointIx>,
    /// Arc centre or MSA centre point.
    pub center: Vec<PointIx>,
    pub path_term: Vec<u8>,
    pub turn: Vec<u8>,
    /// Magnetic course, hundredths of a degree (CIFP gives magnetic; true needs a variation
    /// model this milestone does not have). [`NONE_U16`] when absent.
    pub course_mag_cdeg: Vec<u16>,
    /// Metres, or seconds where `dist_is_time` is set; [`NONE_U32`] when absent.
    pub dist_or_time: Vec<u32>,
    pub alt_desc: Vec<u8>,
    pub alt1_ft: Vec<i32>,
    pub alt2_ft: Vec<i32>,
    pub speed_kt: Vec<u16>,
    pub dist_is_time: BitSet,
}

/// Grid MORA cells, one row per selected one-degree cell. Sorted by (lat, lon).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Mora {
    /// Southern edge of the cell, signed degrees.
    pub lat: Vec<i16>,
    /// Western edge of the cell, signed degrees.
    pub lon: Vec<i16>,
    /// Hundreds of feet; [`MORA_UNKNOWN`] where the FAA publishes `UNK`.
    pub hundreds_ft: Vec<i16>,
}

/// A reference that names a point neither source has (OA-ASP-004), plus the airway border
/// crossings, which are expected and carry the role [`unresolved_role::BORDER_CROSSING`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Unresolved {
    pub role: Vec<u8>,
    pub owner_kind: Vec<u8>,
    /// Airway designator or procedure identifier.
    pub owner: Vec<StrIx>,
    /// Procedure: the airport identifier. Airway: the location letter as a string.
    pub scope: Vec<StrIx>,
    /// Position of the reference within its airway or route (0-based leg or point number).
    pub ordinal: Vec<u32>,
    /// The identifier that could not be found.
    pub name: Vec<StrIx>,
    pub region: Vec<StrIx>,
    /// What the source said the point was (NASR point type, or CIFP `section/subsection`).
    pub wanted: Vec<StrIx>,
}

/// Points present in both NASR and CIFP whose positions differ by more than 1 m (OA-ASP-003).
/// The point row holds the CIFP position (the one taken); this table keeps the NASR one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Disagreements {
    pub point: Vec<PointIx>,
    pub nasr_lat: Vec<i32>,
    pub nasr_lon: Vec<i32>,
    /// Geodesic distance between the two positions in millimetres (metres would round the
    /// 1 m threshold away).
    pub dist_mm: Vec<u32>,
}

/// The model for one ARTCC and cycle. Cold container of the tables (A7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub meta: Meta,
    pub strings: StrTable,
    pub points: Points,
    pub vertices: Vertices,
    pub boundaries: Boundaries,
    pub segments: Segments,
    pub airways: Airways,
    pub airway_legs: AirwayLegs,
    pub procs: Procs,
    pub routes: Routes,
    pub legs: Legs,
    /// NASR PFR source availability. Old snapshot versions decode as [`preferred_route_state::UNAVAILABLE`].
    pub preferred_route_state: u8,
    pub preferred_routes: PreferredRoutes,
    pub preferred_route_segments: PreferredRouteSegments,
    pub mora: Mora,
    pub unresolved: Unresolved,
    pub disagreements: Disagreements,
    /// Set when the model was built from a NASR and a CIFP file of different AIRAC cycles with
    /// the mismatch allowed explicitly; says which cycles (OA-ASP-072). `None` otherwise.
    pub cycle_override: Option<String>,
}

/// Longest `cycle_override` text, bytes.
pub const MAX_CYCLE_OVERRIDE_BYTES: usize = 512;

/// Why a model failed validation: which table and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct ModelError {
    pub table: &'static str,
    pub what: String,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "model table {}: {}", self.table, self.what)
    }
}

impl std::error::Error for ModelError {}

fn bad(table: &'static str, what: impl Into<String>) -> ModelError {
    ModelError {
        table,
        what: what.into(),
    }
}

/// An empty model with the given metadata.
pub fn empty_model(meta: Meta) -> Model {
    Model {
        meta,
        strings: StrTable::default(),
        points: Points::default(),
        vertices: Vec::new(),
        boundaries: Boundaries::default(),
        segments: Segments::default(),
        airways: Airways::default(),
        airway_legs: AirwayLegs::default(),
        procs: Procs::default(),
        routes: Routes::default(),
        legs: Legs::default(),
        preferred_route_state: preferred_route_state::UNAVAILABLE,
        preferred_routes: PreferredRoutes::default(),
        preferred_route_segments: PreferredRouteSegments::default(),
        mora: Mora::default(),
        unresolved: Unresolved::default(),
        disagreements: Disagreements::default(),
        cycle_override: None,
    }
}

/// Every listed column must hold `n` rows.
fn same_len(
    table: &'static str,
    n: usize,
    cols: &[(&'static str, usize)],
) -> Result<(), ModelError> {
    for (name, len) in cols {
        if *len != n {
            return Err(bad(
                table,
                format!("column {name} has {len} rows, expected {n}"),
            ));
        }
    }
    Ok(())
}

fn bits_ok(
    table: &'static str,
    name: &'static str,
    b: &BitSet,
    n: usize,
) -> Result<(), ModelError> {
    if b.len as usize != n || b.words.len() != bit_words(n) {
        return Err(bad(table, format!("bitset {name} does not hold {n} bits")));
    }
    let tail = n % 64;
    if tail != 0 {
        if let Some(last) = b.words.last() {
            if last >> tail != 0 {
                return Err(bad(
                    table,
                    format!("bitset {name} has bits set beyond its length"),
                ));
            }
        }
    }
    Ok(())
}

fn str_ok(
    table: &'static str,
    name: &'static str,
    col: &[StrIx],
    n_str: usize,
    allow_none: bool,
) -> Result<(), ModelError> {
    for (row, s) in col.iter().enumerate() {
        let ok = (s.0 as usize) < n_str || (allow_none && *s == StrIx::NONE);
        if !ok {
            return Err(bad(
                table,
                format!("{name}[{row}] = {} is not a string index", s.0),
            ));
        }
    }
    Ok(())
}

fn point_ok(
    table: &'static str,
    name: &'static str,
    col: &[PointIx],
    n: usize,
    allow_none: bool,
) -> Result<(), ModelError> {
    for (row, p) in col.iter().enumerate() {
        let ok = (p.0 as usize) < n || (allow_none && *p == PointIx::NONE);
        if !ok {
            return Err(bad(
                table,
                format!("{name}[{row}] = {} is not a point index", p.0),
            ));
        }
    }
    Ok(())
}

fn code_ok(
    table: &'static str,
    name: &'static str,
    col: &[u8],
    max: u8,
    allow_zero: bool,
) -> Result<(), ModelError> {
    for (row, c) in col.iter().enumerate() {
        let ok = (*c >= 1 && *c <= max) || (allow_zero && *c == 0);
        if !ok {
            return Err(bad(
                table,
                format!("{name}[{row}] = {c} is not a valid code"),
            ));
        }
    }
    Ok(())
}

fn pos_ok(table: &'static str, lat: i32, lon: i32, none_ok: bool) -> Result<(), ModelError> {
    if none_ok && lat == POS_NONE && lon == POS_NONE {
        return Ok(());
    }
    if !(-900_000_000..=900_000_000).contains(&lat)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon)
    {
        return Err(bad(table, "a position lies outside the globe"));
    }
    Ok(())
}

/// Limit: the lowest altitude or elevation a row may hold, feet (below any land elevation).
pub const ALT_MIN_FT: i32 = -2_000;
/// Limit: the highest altitude or elevation a row may hold, feet.
pub const ALT_MAX_FT: i32 = 1_000_000;
/// Limit: the largest bearing a row may hold, hundredths of a degree (360 degrees; ARINC 424
/// codes north as 3600 tenths, so 360 is legitimate).
pub const BEARING_MAX_CDEG: u16 = 36_000;

/// Every altitude in `col` is one of the named sentinels or within [`ALT_MIN_FT`],
/// [`ALT_MAX_FT`].
fn alt_ok(table: &'static str, name: &'static str, col: &[i32]) -> Result<(), ModelError> {
    for (row, v) in col.iter().enumerate() {
        let sentinel = matches!(*v, ALT_NONE | ALT_GROUND | ALT_UNKNOWN | ALT_UNLIMITED);
        if !sentinel && !(ALT_MIN_FT..=ALT_MAX_FT).contains(v) {
            return Err(bad(
                table,
                format!("{name}[{row}] = {v} ft is outside the altitude range"),
            ));
        }
    }
    Ok(())
}

/// Every bearing in `col` is [`NONE_U16`] or at most [`BEARING_MAX_CDEG`].
fn bearing_ok(table: &'static str, name: &'static str, col: &[u16]) -> Result<(), ModelError> {
    for (row, v) in col.iter().enumerate() {
        if *v != NONE_U16 && *v > BEARING_MAX_CDEG {
            return Err(bad(
                table,
                format!("{name}[{row}] = {v} is not a bearing in 0 to 360 degrees"),
            ));
        }
    }
    Ok(())
}

/// Every value of a small code column is 0 (absent) or one of `allowed`.
fn enum_ok(
    table: &'static str,
    name: &'static str,
    col: &[u8],
    allowed: &[u8],
) -> Result<(), ModelError> {
    for (row, v) in col.iter().enumerate() {
        if *v != 0 && !allowed.contains(v) {
            return Err(bad(
                table,
                format!("{name}[{row}] = {v} is not a valid code"),
            ));
        }
    }
    Ok(())
}

/// Keys must be strictly increasing (sorted, no duplicates).
fn increasing<K: Ord>(
    table: &'static str,
    keys: impl Iterator<Item = K>,
) -> Result<(), ModelError> {
    let mut prev: Option<K> = None;
    for (row, k) in keys.enumerate() {
        if let Some(p) = &prev {
            if *p >= k {
                return Err(bad(
                    table,
                    format!(
                        "rows {} and {row} are out of order or share a key",
                        row.saturating_sub(1)
                    ),
                ));
            }
        }
        prev = Some(k);
    }
    Ok(())
}

/// Spans must tile the child table exactly: consecutive, in order, no gap, no overlap, covering
/// all `child_rows` rows. That is how the builder lays them out, and it makes a span unable to
/// reach another parent's rows.
fn spans_tile<I: Copy>(
    table: &'static str,
    name: &'static str,
    spans: &[Span<I>],
    raw: impl Fn(I) -> u32,
    child_rows: usize,
) -> Result<(), ModelError> {
    let mut next = 0usize;
    for (row, s) in spans.iter().enumerate() {
        if raw(s.start) as usize != next {
            return Err(bad(
                table,
                format!(
                    "{name}[{row}] starts at {} but rows must follow from {next}",
                    raw(s.start)
                ),
            ));
        }
        next = next
            .checked_add(s.len as usize)
            .ok_or_else(|| bad(table, format!("{name}[{row}] length overflows")))?;
        if next > child_rows {
            return Err(bad(
                table,
                format!("{name}[{row}] runs past the child table ({child_rows} rows)"),
            ));
        }
    }
    if next != child_rows {
        return Err(bad(
            table,
            format!("{name} covers {next} of {child_rows} child rows"),
        ));
    }
    Ok(())
}

/// The identity key of every point row, in row order.
fn point_keys(m: &Model) -> impl Iterator<Item = PointKey<'_>> {
    let p = &m.points;
    p.ident
        .iter()
        .zip(&p.region)
        .zip(&p.scope)
        .zip(&p.kind)
        .map(|(((ident, region), scope), kind)| PointKey {
            ident: str_or_empty(&m.strings, *ident),
            region: str_or_empty(&m.strings, *region),
            scope: str_or_empty(&m.strings, *scope),
            class: point_kind::class(*kind),
        })
}

/// Checks every column length, code, string index, point index, span and sort order. A model
/// that passes can be read by index without further checks; the snapshot reader runs it on
/// every loaded file.
pub fn validate_model(m: &Model) -> Result<(), ModelError> {
    let s = &m.strings;
    let n_str = str_count(s);
    if s.offsets.first() != Some(&0) || s.offsets.last().map(|&o| o as usize) != Some(s.text.len())
    {
        return Err(bad("strings", "offsets do not span the text"));
    }
    for w in s.offsets.windows(2) {
        // `windows(2)` yields pairs only; the other arm is unreachable and refuses all the same
        let [a, b] = w else {
            return Err(bad(
                "strings",
                "offsets are not increasing character boundaries",
            ));
        };
        let (a, b) = (*a as usize, *b as usize);
        if a > b || s.text.get(a..b).is_none() {
            return Err(bad(
                "strings",
                "offsets are not increasing character boundaries",
            ));
        }
    }

    let p = &m.points;
    let n = p.kind.len();
    same_len(
        "points",
        n,
        &[
            ("lat", p.lat.len()),
            ("lon", p.lon.len()),
            ("elev_ft", p.elev_ft.len()),
            ("ident", p.ident.len()),
            ("alt_ident", p.alt_ident.len()),
            ("region", p.region.len()),
            ("scope", p.scope.len()),
            ("name", p.name.len()),
        ],
    )?;
    for (name, b) in [
        ("src_nasr", &p.src_nasr),
        ("src_cifp", &p.src_cifp),
        ("in_area", &p.in_area),
        ("cls_b", &p.cls_b),
        ("cls_c", &p.cls_c),
        ("cls_d", &p.cls_d),
        ("cls_e", &p.cls_e),
    ] {
        bits_ok("points", name, b, n)?;
    }
    code_ok("points", "kind", &p.kind, point_kind::MAX, false)?;
    str_ok("points", "ident", &p.ident, n_str, false)?;
    for (name, col) in [
        ("alt_ident", &p.alt_ident),
        ("region", &p.region),
        ("scope", &p.scope),
        ("name", &p.name),
    ] {
        str_ok("points", name, col, n_str, true)?;
    }
    for (lat, lon) in p.lat.iter().zip(&p.lon) {
        pos_ok("points", *lat, *lon, false)?;
    }
    alt_ok("points", "elev_ft", &p.elev_ft)?;
    increasing("points", point_keys(m))?;

    let nv = m.vertices.len();
    let ns = m.segments.via.len();
    let b = &m.boundaries;
    let nb = b.source.len();
    same_len(
        "boundaries",
        nb,
        &[
            ("stratum", b.stratum.len()),
            ("edge_interp", b.edge_interp.len()),
            ("multiple", b.multiple.len()),
            ("facility", b.facility.len()),
            ("name", b.name.len()),
            ("floor_ft", b.floor_ft.len()),
            ("ceiling_ft", b.ceiling_ft.len()),
            ("vertices", b.vertices.len()),
            ("segs", b.segs.len()),
            ("time_code", b.time_code.len()),
            ("level", b.level.len()),
        ],
    )?;
    code_ok(
        "boundaries",
        "time_code",
        &b.time_code,
        time_code::MAX,
        true,
    )?;
    code_ok("boundaries", "level", &b.level, level::MAX, true)?;
    bits_ok("boundaries", "floor_agl", &b.floor_agl, nb)?;
    bits_ok("boundaries", "ceiling_agl", &b.ceiling_agl, nb)?;
    bits_ok("boundaries", "incomplete", &b.incomplete, nb)?;
    code_ok(
        "boundaries",
        "source",
        &b.source,
        boundary_source::MAX,
        false,
    )?;
    code_ok("boundaries", "stratum", &b.stratum, stratum::MAX, false)?;
    code_ok(
        "boundaries",
        "edge_interp",
        &b.edge_interp,
        edge_interp::MAX,
        true,
    )?;
    str_ok("boundaries", "facility", &b.facility, n_str, false)?;
    str_ok("boundaries", "name", &b.name, n_str, true)?;
    alt_ok("boundaries", "floor_ft", &b.floor_ft)?;
    alt_ok("boundaries", "ceiling_ft", &b.ceiling_ft)?;
    for (row, mu) in b.multiple.iter().enumerate() {
        if *mu != 0 && !mu.is_ascii_graphic() {
            return Err(bad(
                "boundaries",
                format!("multiple[{row}] = {mu} is not a printable code"),
            ));
        }
    }
    spans_tile("boundaries", "vertices", &b.vertices, |i| i.0, nv)?;
    spans_tile("boundaries", "segs", &b.segs, |i| i.0, ns)?;
    increasing(
        "boundaries",
        b.source
            .iter()
            .zip(&b.stratum)
            .zip(&b.facility)
            .zip(&b.multiple)
            .zip(&b.name)
            .map(|((((so, st), f), mu), nm)| {
                (*so, *st, str_or_empty(s, *f), *mu, str_or_empty(s, *nm))
            }),
    )?;
    for v in &m.vertices {
        pos_ok("vertices", v.lat, v.lon, false)?;
    }

    let g = &m.segments;
    same_len(
        "segments",
        ns,
        &[
            ("lat", g.lat.len()),
            ("lon", g.lon.len()),
            ("arc_lat", g.arc_lat.len()),
            ("arc_lon", g.arc_lon.len()),
            ("radius_m", g.radius_m.len()),
            ("bearing_cdeg", g.bearing_cdeg.len()),
        ],
    )?;
    code_ok("segments", "via", &g.via, via::MAX, false)?;
    for (lat, lon) in g.lat.iter().zip(&g.lon) {
        pos_ok("segments", *lat, *lon, true)?;
    }
    for (lat, lon) in g.arc_lat.iter().zip(&g.arc_lon) {
        pos_ok("segments", *lat, *lon, true)?;
    }
    bearing_ok("segments", "bearing_cdeg", &g.bearing_cdeg)?;
    // A circle or arc has a centre and a radius; every row but a circle has a point.
    for row in 0..ns {
        let (Some(&v), Some(&lat), Some(&arc_lat), Some(&radius)) = (
            g.via.get(row),
            g.lat.get(row),
            g.arc_lat.get(row),
            g.radius_m.get(row),
        ) else {
            continue;
        };
        let round = matches!(v, via::CIRCLE | via::ARC_CW | via::ARC_CCW);
        if round && (arc_lat == POS_NONE || radius == 0) {
            return Err(bad(
                "segments",
                format!("row {row} is a circle or arc without a centre and radius"),
            ));
        }
        if v != via::CIRCLE && lat == POS_NONE {
            return Err(bad("segments", format!("row {row} has no point")));
        }
    }

    let a = &m.airways;
    let na = a.designator.len();
    let nl = m.airway_legs.from.len();
    same_len(
        "airways",
        na,
        &[("location", a.location.len()), ("legs", a.legs.len())],
    )?;
    str_ok("airways", "designator", &a.designator, n_str, false)?;
    for (row, loc) in a.location.iter().enumerate() {
        if !loc.is_ascii_graphic() {
            return Err(bad(
                "airways",
                format!("location[{row}] = {loc} is not a printable letter"),
            ));
        }
    }
    spans_tile("airways", "legs", &a.legs, |i| i.0, nl)?;
    increasing(
        "airways",
        a.designator
            .iter()
            .zip(&a.location)
            .map(|(d, l)| (str_or_empty(s, *d), *l)),
    )?;
    let l = &m.airway_legs;
    same_len(
        "airway_legs",
        nl,
        &[
            ("to", l.to.len()),
            ("mea_ft", l.mea_ft.len()),
            ("mea_opp_ft", l.mea_opp_ft.len()),
            ("gps_mea_ft", l.gps_mea_ft.len()),
            ("mea_dir", l.mea_dir.len()),
            ("gps_mea_dir", l.gps_mea_dir.len()),
        ],
    )?;
    for (name, bs) in [
        ("gap", &l.gap),
        ("signal_gap", &l.signal_gap),
        ("dogleg", &l.dogleg),
    ] {
        bits_ok("airway_legs", name, bs, nl)?;
    }
    for (name, col) in [
        ("mea_ft", &l.mea_ft),
        ("mea_opp_ft", &l.mea_opp_ft),
        ("gps_mea_ft", &l.gps_mea_ft),
    ] {
        alt_ok("airway_legs", name, col)?;
    }
    for (name, col) in [("mea_dir", &l.mea_dir), ("gps_mea_dir", &l.gps_mea_dir)] {
        enum_ok(
            "airway_legs",
            name,
            col,
            &[
                compass::NORTH,
                compass::NORTHEAST,
                compass::EAST,
                compass::SOUTHEAST,
                compass::SOUTH,
                compass::SOUTHWEST,
                compass::WEST,
                compass::NORTHWEST,
                compass::OTHER,
            ],
        )?;
    }
    point_ok("airway_legs", "from", &l.from, n, true)?;
    point_ok("airway_legs", "to", &l.to, n, true)?;

    let pr = &m.procs;
    let np = pr.kind.len();
    let nr = m.routes.route_type.len();
    same_len(
        "procs",
        np,
        &[
            ("airport", pr.airport.len()),
            ("ident", pr.ident.len()),
            ("name", pr.name.len()),
            ("routes", pr.routes.len()),
        ],
    )?;
    for (name, bs) in [
        ("src_nasr", &pr.src_nasr),
        ("src_cifp", &pr.src_cifp),
        ("coded", &pr.coded),
    ] {
        bits_ok("procs", name, bs, np)?;
    }
    code_ok("procs", "kind", &pr.kind, proc_kind::MAX, false)?;
    point_ok("procs", "airport", &pr.airport, n, false)?;
    str_ok("procs", "ident", &pr.ident, n_str, false)?;
    str_ok("procs", "name", &pr.name, n_str, true)?;
    spans_tile("procs", "routes", &pr.routes, |i| i.0, nr)?;
    increasing(
        "procs",
        pr.airport
            .iter()
            .zip(&pr.kind)
            .zip(&pr.ident)
            .map(|((ap, k), id)| {
                let airport = at(&p.ident, ap.0).map_or("", |x| str_or_empty(s, x));
                let region = at(&p.region, ap.0).map_or("", |x| str_or_empty(s, x));
                (airport, region, *k, str_or_empty(s, *id))
            }),
    )?;
    let r = &m.routes;
    let nlg = m.legs.fix.len();
    same_len(
        "routes",
        nr,
        &[("transition", r.transition.len()), ("legs", r.legs.len())],
    )?;
    str_ok("routes", "transition", &r.transition, n_str, true)?;
    spans_tile("routes", "legs", &r.legs, |i| i.0, nlg)?;

    let pfr = &m.preferred_routes;
    let npfr = pfr.origin.len();
    let pfr_segments = &m.preferred_route_segments;
    let n_pfr_segments = pfr_segments.sequence.len();
    same_len(
        "preferred_routes",
        npfr,
        &[
            ("eff_date", pfr.eff_date.len()),
            ("destination", pfr.destination.len()),
            ("route_type", pfr.route_type.len()),
            ("route_no", pfr.route_no.len()),
            ("area_description", pfr.area_description.len()),
            ("altitude_description", pfr.altitude_description.len()),
            ("aircraft_description", pfr.aircraft_description.len()),
            ("hours_description", pfr.hours_description.len()),
            ("direction_description", pfr.direction_description.len()),
            ("route_string", pfr.route_string.len()),
            ("segments", pfr.segments.len()),
        ],
    )?;
    if m.preferred_route_state > preferred_route_state::MAX {
        return Err(bad("preferred_routes", "unknown source availability state"));
    }
    if m.preferred_route_state == preferred_route_state::UNAVAILABLE && npfr != 0 {
        return Err(bad(
            "preferred_routes",
            "unavailable source state carries route rows",
        ));
    }
    for (name, col) in [
        ("eff_date", &pfr.eff_date),
        ("origin", &pfr.origin),
        ("destination", &pfr.destination),
        ("route_type", &pfr.route_type),
    ] {
        str_ok("preferred_routes", name, col, n_str, false)?;
    }
    for (name, col) in [
        ("area_description", &pfr.area_description),
        ("altitude_description", &pfr.altitude_description),
        ("aircraft_description", &pfr.aircraft_description),
        ("hours_description", &pfr.hours_description),
        ("direction_description", &pfr.direction_description),
        ("route_string", &pfr.route_string),
    ] {
        str_ok("preferred_routes", name, col, n_str, true)?;
    }
    spans_tile(
        "preferred_routes",
        "segments",
        &pfr.segments,
        |i| i.0,
        n_pfr_segments,
    )?;
    increasing(
        "preferred_routes",
        pfr.origin
            .iter()
            .zip(&pfr.destination)
            .zip(&pfr.route_type)
            .zip(&pfr.route_no)
            .map(|(((origin, destination), kind), number)| {
                (
                    str_or_empty(s, *origin),
                    str_or_empty(s, *destination),
                    str_or_empty(s, *kind),
                    *number,
                )
            }),
    )?;
    same_len(
        "preferred_route_segments",
        n_pfr_segments,
        &[
            ("value", pfr_segments.value.len()),
            ("segment_type", pfr_segments.segment_type.len()),
            ("state_code", pfr_segments.state_code.len()),
            ("country_code", pfr_segments.country_code.len()),
            ("icao_region_code", pfr_segments.icao_region_code.len()),
            ("nav_type", pfr_segments.nav_type.len()),
            ("next_segment", pfr_segments.next_segment.len()),
        ],
    )?;
    for (name, col, none_ok) in [
        ("value", &pfr_segments.value, false),
        ("segment_type", &pfr_segments.segment_type, false),
        ("state_code", &pfr_segments.state_code, true),
        ("country_code", &pfr_segments.country_code, true),
        ("icao_region_code", &pfr_segments.icao_region_code, true),
        ("nav_type", &pfr_segments.nav_type, true),
        ("next_segment", &pfr_segments.next_segment, true),
    ] {
        str_ok("preferred_route_segments", name, col, n_str, none_ok)?;
    }
    for span in &pfr.segments {
        let start = usize::try_from(span.start.get()).map_err(|_| {
            bad(
                "preferred_routes",
                "preferred segment span start does not fit",
            )
        })?;
        let end = start
            .checked_add(usize::try_from(span.len).map_err(|_| {
                bad(
                    "preferred_routes",
                    "preferred segment span length does not fit",
                )
            })?)
            .ok_or_else(|| bad("preferred_routes", "preferred segment span overflows"))?;
        let rows = pfr_segments
            .sequence
            .get(start..end)
            .ok_or_else(|| bad("preferred_routes", "preferred segment span is out of range"))?;
        if rows.windows(2).any(|pair| {
            pair.first()
                .zip(pair.last())
                .is_some_and(|(first, last)| first >= last)
        }) {
            return Err(bad(
                "preferred_route_segments",
                "segments are not strictly increasing by sequence within a route",
            ));
        }
    }
    let lg = &m.legs;
    same_len(
        "legs",
        nlg,
        &[
            ("center", lg.center.len()),
            ("path_term", lg.path_term.len()),
            ("turn", lg.turn.len()),
            ("course_mag_cdeg", lg.course_mag_cdeg.len()),
            ("dist_or_time", lg.dist_or_time.len()),
            ("alt_desc", lg.alt_desc.len()),
            ("alt1_ft", lg.alt1_ft.len()),
            ("alt2_ft", lg.alt2_ft.len()),
            ("speed_kt", lg.speed_kt.len()),
        ],
    )?;
    bits_ok("legs", "dist_is_time", &lg.dist_is_time, nlg)?;
    point_ok("legs", "fix", &lg.fix, n, true)?;
    point_ok("legs", "center", &lg.center, n, true)?;
    code_ok("legs", "path_term", &lg.path_term, path_term::MAX, true)?;
    enum_ok(
        "legs",
        "turn",
        &lg.turn,
        &[turn::LEFT, turn::RIGHT, turn::EITHER],
    )?;
    enum_ok(
        "legs",
        "alt_desc",
        &lg.alt_desc,
        &[
            alt_desc::AT_OR_ABOVE,
            alt_desc::BETWEEN,
            alt_desc::UNVERIFIED,
        ],
    )?;
    bearing_ok("legs", "course_mag_cdeg", &lg.course_mag_cdeg)?;
    alt_ok("legs", "alt1_ft", &lg.alt1_ft)?;
    alt_ok("legs", "alt2_ft", &lg.alt2_ft)?;

    let mo = &m.mora;
    same_len(
        "mora",
        mo.lat.len(),
        &[("lon", mo.lon.len()), ("hundreds_ft", mo.hundreds_ft.len())],
    )?;
    for (row, ((lat, lon), h)) in mo.lat.iter().zip(&mo.lon).zip(&mo.hundreds_ft).enumerate() {
        // A one-degree cell: southwest corner within the globe, value in hundreds of feet.
        if !(-90..90).contains(lat) || !(-180..180).contains(lon) {
            return Err(bad(
                "mora",
                format!("row {row} names a cell outside the globe"),
            ));
        }
        if *h != MORA_UNKNOWN && !(0..=1_000).contains(h) {
            return Err(bad(
                "mora",
                format!("hundreds_ft[{row}] = {h} is outside 0 to 100000 ft"),
            ));
        }
    }
    increasing("mora", mo.lat.iter().zip(&mo.lon).map(|(a, b)| (*a, *b)))?;

    let u = &m.unresolved;
    let nu = u.role.len();
    same_len(
        "unresolved",
        nu,
        &[
            ("owner_kind", u.owner_kind.len()),
            ("owner", u.owner.len()),
            ("scope", u.scope.len()),
            ("ordinal", u.ordinal.len()),
            ("name", u.name.len()),
            ("region", u.region.len()),
            ("wanted", u.wanted.len()),
        ],
    )?;
    code_ok("unresolved", "role", &u.role, unresolved_role::MAX, false)?;
    code_ok(
        "unresolved",
        "owner_kind",
        &u.owner_kind,
        owner_kind::MAX,
        false,
    )?;
    for (name, col, none_ok) in [
        ("owner", &u.owner, false),
        ("scope", &u.scope, true),
        ("name", &u.name, false),
        ("region", &u.region, true),
        ("wanted", &u.wanted, true),
    ] {
        str_ok("unresolved", name, col, n_str, none_ok)?;
    }

    let d = &m.disagreements;
    let nd = d.point.len();
    same_len(
        "disagreements",
        nd,
        &[
            ("nasr_lat", d.nasr_lat.len()),
            ("nasr_lon", d.nasr_lon.len()),
            ("dist_mm", d.dist_mm.len()),
        ],
    )?;
    point_ok("disagreements", "point", &d.point, n, false)?;
    for (lat, lon) in d.nasr_lat.iter().zip(&d.nasr_lon) {
        pos_ok("disagreements", *lat, *lon, false)?;
    }
    Ok(())
}

/// Identity key of a point: (ident, region, scope, class of kind) as strings; the derived
/// ordering is the table's sort order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
// A7.2: cold struct, size guard waived
pub struct PointKey<'a> {
    pub ident: &'a str,
    pub region: &'a str,
    pub scope: &'a str,
    pub class: u8,
}

/// The identity key of point row `ix`.
pub fn point_key(m: &Model, ix: PointIx) -> Option<PointKey<'_>> {
    let i = ix.0;
    Some(PointKey {
        ident: str_at(&m.strings, at(&m.points.ident, i)?)?,
        region: str_or_empty(&m.strings, at(&m.points.region, i)?),
        scope: str_or_empty(&m.strings, at(&m.points.scope, i)?),
        class: point_kind::class(at(&m.points.kind, i)?),
    })
}

/// Finds a point by identity (binary search over the sorted table).
pub fn find_point(m: &Model, key: PointKey<'_>) -> Option<PointIx> {
    let (mut lo, mut hi) = (0usize, m.points.kind.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let k = point_key(m, PointIx(u32::try_from(mid).ok()?))?;
        match k.cmp(&key) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Some(PointIx(u32::try_from(mid).ok()?)),
        }
    }
    None
}

/// Finds an airway by designator and location letter.
pub fn find_airway(m: &Model, designator: &str, location: u8) -> Option<AirwayIx> {
    let a = &m.airways;
    a.designator
        .iter()
        .zip(&a.location)
        .position(|(d, l)| *l == location && str_or_empty(&m.strings, *d) == designator)
        .and_then(|i| u32::try_from(i).ok())
        .map(AirwayIx)
}

/// The sorted PFR records for one exact airport pair. `start..end` is empty when the pair is
/// not published. This is an advisory lookup; absence never rejects a filed route.
pub fn preferred_route_range(m: &Model, origin: &str, destination: &str) -> std::ops::Range<usize> {
    let table = &m.preferred_routes;
    let key_at = |row: usize| {
        Some((
            str_at(&m.strings, *table.origin.get(row)?)?,
            str_at(&m.strings, *table.destination.get(row)?)?,
        ))
    };
    let key = (origin, destination);
    let mut lo = 0usize;
    let mut hi = table.origin.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some(candidate) = key_at(mid) else {
            return 0..0;
        };
        if candidate < key {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let start = lo;
    hi = table.origin.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let Some(candidate) = key_at(mid) else {
            return 0..0;
        };
        if candidate <= key {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    start..lo
}

/// Finds a procedure by airport identity (identifier and ICAO region), kind and identifier.
pub fn find_proc(
    m: &Model,
    airport_ident: &str,
    airport_region: &str,
    kind: u8,
    ident: &str,
) -> Option<ProcIx> {
    let pr = &m.procs;
    pr.kind
        .iter()
        .zip(&pr.ident)
        .zip(&pr.airport)
        .position(|((k, id), ap)| {
            *k == kind
                && str_or_empty(&m.strings, *id) == ident
                && at(&m.points.ident, ap.0)
                    .is_some_and(|a| str_or_empty(&m.strings, a) == airport_ident)
                && at(&m.points.region, ap.0)
                    .is_some_and(|r| str_or_empty(&m.strings, r) == airport_region)
        })
        .and_then(|i| u32::try_from(i).ok())
        .map(ProcIx)
}

/// Finds a boundary by source, stratum, facility and multiple code (first match by name).
pub fn find_boundary(
    m: &Model,
    source: u8,
    stratum: u8,
    facility: &str,
    multiple: u8,
) -> Option<BoundaryIx> {
    let b = &m.boundaries;
    b.source
        .iter()
        .zip(&b.stratum)
        .zip(&b.multiple)
        .zip(&b.facility)
        .position(|(((so, st), mu), f)| {
            *so == source
                && *st == stratum
                && *mu == multiple
                && str_or_empty(&m.strings, *f) == facility
        })
        .and_then(|i| u32::try_from(i).ok())
        .map(BoundaryIx)
}

/// Whether a boundary's outline is complete (OA-ASP-069): `false` for a CIFP volume whose
/// records end without the end marker, so its outline is not precise and a consumer must not
/// treat it as such; `None` when `ix` is not a boundary of the model.
pub fn boundary_is_complete(m: &Model, ix: BoundaryIx) -> Option<bool> {
    let row = ix.row();
    (row < m.boundaries.source.len()).then(|| !bit_get(&m.boundaries.incomplete, row))
}

/// True for the NASR airway pseudo-point naming a border crossing, such as
/// `U.S. CANADIAN BORDER-2`.
pub fn is_border_crossing_name(name: &str) -> bool {
    name.starts_with("U.S. ") && name.contains(" BORDER-")
}

/// Number of unresolved references that are real errors (not border crossings).
pub fn unresolved_error_count(m: &Model) -> usize {
    m.unresolved
        .role
        .iter()
        .filter(|&&r| r != unresolved_role::BORDER_CROSSING)
        .count()
}

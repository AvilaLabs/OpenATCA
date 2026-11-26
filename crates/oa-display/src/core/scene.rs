//! The static airspace map (**core**, B1, B2): a pure function from the airspace model, an index of
//! it, and a view to a display list. Nothing is read from a clock or a file; the same inputs give
//! the same list.
//!
//! # What is drawn
//!
//! Safety over flash: the map shows the data that matters at the range it is set to, and says on
//! the screen what it is not showing.
//!
//! * The own facility's ARTCC HIGH and LOW boundaries (the facility the snapshot was built for),
//!   two pixels wide in the full strata colours; neighbouring facilities' boundaries one pixel wide
//!   in dimmer colours of the same hues.
//! * Airway legs as thin lines: high airways (designators J and Q) and low airways (every other
//!   designator, V and T among them).
//! * Navaids: VOR, VORTAC and VOR-DME (always), NDB, DME and TACAN (within a range threshold), as a
//!   small ring and dot. Fixes as a triangle, terminal waypoints as a smaller one, runway
//!   thresholds as a plus and localizers as a diamond (each within a range threshold). Airports as
//!   a ring and dot: those with at least one coded instrument procedure in the model always, the
//!   others within a range threshold.
//! * Labels, placed without collisions (see below), a range-ring scale with each ring labelled
//!   where it meets the 45 degree north-east ray (so no two ring labels can meet), and a centre
//!   mark.
//! * A legend strip along the top of the screen: facility and cycles, range and ring spacing, the
//!   boundary colours, a `HIDDEN:` line naming every range threshold or layer switch that is hiding
//!   something, the count of airway legs that end at a published border crossing, and, only when
//!   something truly could not be placed, an alert line.
//!
//! Every layer can be switched off ([`Layers`]) and every threshold set ([`View`]); the defaults
//! are in the crate README. Every threshold keys on the display range (the zoom: nautical miles
//! from the centre to the nearest edge of the map area), never on a symbol's distance from the
//! centre: a given zoom shows the same kinds of symbol across the whole screen.
//!
//! # Label placement
//!
//! Labels are candidates first. Each shown symbol with an ident is a candidate with a priority:
//! 1 VOR, VORTAC and VOR-DME, 2 airports with a coded instrument procedure, 3 other airports,
//! 4 fixes and the minor navaids, 5 terminal waypoints. Candidates are sorted by priority, then
//! ident, then position, then table row, which is a total order, so the same input gives the same
//! labels on every machine. Each is placed at the first of four fixed offsets (north-east,
//! south-east, north-west, south-west of its symbol) whose box overlaps no placed label, no ring
//! label, the legend strip nor the screen edge; if none fits it is dropped and counted
//! ([`SceneStats::labels_dropped`], shown in the legend). Placed boxes live in a coarse grid
//! of cells with a bounded list each, allocated once in [`Scratch`]: a full cell drops the label,
//! it never overflows. Because candidates are placed in priority order, a label is only ever
//! dropped for want of room taken by a label of equal or higher priority. VOR and airport
//! labels with a procedure (priorities 1 and 2) show out to a wider range than the rest.
//!
//! # The legend strip
//!
//! The strip is reserved: the projection's map area is the screen below it
//! ([`LEGEND_STRIP_PX`] rows), so the circle of the chosen range always fits whole in the map area
//! and no data inside the range is under the legend. Map primitives beyond that circle are clipped
//! at the strip's lower edge by its opaque backdrop; that is the edge of the view, not hidden data.
//!
//! # The alert colour
//!
//! The alert colour appears only for a condition that needs attention: data the map tried to
//! place and could not. A leg that ends at a border crossing is expected data (the FAA publishes
//! the crossing as a pseudo-point with no position) and is reported in the normal colour.
//!
//! Boundary and airway edges are geodesics, not straight lines on the glass: a long edge is split
//! into short pieces whose end points are interpolated on the sphere, so the drawn curve follows
//! the great circle to well under a pixel. An edge is split to at most 12 NM and 64 pixels a piece,
//! up to 64 pieces for a boundary edge (768 NM) and 16 for an airway leg (192 NM); a longer edge
//! is drawn with longer pieces.
//!
//! Strata other than ARTCC HIGH and LOW (class airspace, special-use airspace, FIR) are in the
//! model but not drawn in this map.

use crate::core::font::{text_width_units, CAP_HEIGHT};
use crate::core::list::{
    begin_polyline_w, check_limit, list_clear, push_circle, push_line, push_line_w, push_rect,
    push_text, push_vertex, DisplayError, DisplayList, ListCaps, ALIGN_LEFT, ALIGN_RIGHT,
    MAX_LIST_ROWS, MAX_LIST_TEXT_BYTES,
};
use crate::core::palette;
use crate::core::project::{
    dlon_e7, in_bbox, project, project_rad, projection_new, ring_radius16, view_bbox, Bbox,
    Projection, MAX_RANGE_NM, MIN_RANGE_NM, NM_M,
};
use oa_airspace::core::model::{
    bit_get, point_kind, str_or_empty, stratum, unresolved_role, Model, PointIx, POS_NONE,
};
use std::fmt::Write;

/// Maximum precharged work in one scene build. The charge covers table scans, declared span
/// visits, edge subdivision, point-label sorting (at most n * ceil(log2 n) comparisons, times the longest identifier) and
/// worst-case label-grid slot inspection.
pub const MAX_SCENE_WORK: u128 = 1 << 40;
pub const MAX_SCENE_PIXELS: u128 = crate::core::raster::MAX_DISPLAY_PIXELS as u128;

/// Layers, drawn first to last.
pub const LAYER_RING: u8 = 0;
pub const LAYER_AIRWAY: u8 = 1;
pub const LAYER_NEIGHBOUR: u8 = 2;
pub const LAYER_ARTCC_LOW: u8 = 3;
pub const LAYER_ARTCC_HIGH: u8 = 4;
pub const LAYER_SYMBOL: u8 = 5;
pub const LAYER_TEXT: u8 = 6;
// Layers 7 to 10 hold the live traffic (`crate::core::traffic`); the legend strip covers it.
pub const LAYER_LEGEND_BG: u8 = 11;
pub const LAYER_LEGEND: u8 = 12;

/// Height of the legend strip, pixels: five rows of text and a rule.
pub const LEGEND_STRIP_PX: u32 = 104;

/// Default range thresholds, nautical miles from the centre to the nearest edge of the map.
pub const DEFAULT_FIX_MAX_NM: f64 = 40.0;
pub const DEFAULT_TERMINAL_WPT_MAX_NM: f64 = 20.0;
pub const DEFAULT_RUNWAY_MAX_NM: f64 = 15.0;
pub const DEFAULT_NON_IFR_AIRPORT_MAX_NM: f64 = 40.0;
pub const DEFAULT_MINOR_NAVAID_MAX_NM: f64 = 40.0;
pub const DEFAULT_LABEL_MAX_NM: f64 = 40.0;
/// Labels of VORs and of airports with a procedure.
pub const DEFAULT_MAJOR_LABEL_MAX_NM: f64 = 100.0;

const SUBDIV_BOUNDARY: u32 = 64;
const SUBDIV_AIRWAY: u32 = 16;
/// An edge piece is at most this long on the ground, metres, and this long on screen, 1/16 pixel.
const PIECE_M: f64 = 12.0 * NM_M;
const PIECE_16: i64 = 64 * 16;
const MAX_RINGS: i64 = 64;
/// Smallest ring spacing on screen, pixels.
const MIN_RING_PX: f64 = 90.0;
/// The coarsest spacing, used when none of the series is wide enough.
const RING_STEP_MAX_NM: f64 = 5000.0;
const RING_STEPS_NM: [f64; 14] = [
    1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0,
];
/// Symbol half-size, 1/16 pixel.
const SYM: i32 = 4 * 16;
/// Off-screen margin inside which a primitive is still kept, 1/16 pixel.
const MARGIN: i32 = 64 * 16;
const LABEL_SIZE: u8 = 12;
const LEGEND_SIZE: u8 = 12;
/// Rows reserved beyond the points for fixed furniture (rings, legend, centre mark).
const FURNITURE: usize = 256;

/// Which layers are drawn. Each can be switched off from the command line (`--hide`).
// A7.2: cold struct, size guard waived (one per display).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layers {
    pub own_boundary: bool,
    pub neighbour_boundaries: bool,
    pub high_airways: bool,
    pub low_airways: bool,
    /// All navaids: VOR-class always (range permitting nothing), the minor ones by threshold.
    pub vors: bool,
    pub airports: bool,
    pub fixes: bool,
    pub labels: bool,
    pub rings: bool,
}

/// Names of the layers, in the order of [`Layers`], as the command line spells them.
pub const LAYER_NAMES: [&str; 9] = [
    "own-boundary",
    "neighbour-boundaries",
    "high-airways",
    "low-airways",
    "vors",
    "airports",
    "fixes",
    "labels",
    "rings",
];

/// Every layer on.
pub fn layers_all() -> Layers {
    Layers {
        own_boundary: true,
        neighbour_boundaries: true,
        high_airways: true,
        low_airways: true,
        vors: true,
        airports: true,
        fixes: true,
        labels: true,
        rings: true,
    }
}

/// Switches the layer called `name` (one of [`LAYER_NAMES`]) on or off. False for an unknown name.
pub fn layer_set(l: &mut Layers, name: &str, on: bool) -> bool {
    let slot = match name {
        "own-boundary" => &mut l.own_boundary,
        "neighbour-boundaries" => &mut l.neighbour_boundaries,
        "high-airways" => &mut l.high_airways,
        "low-airways" => &mut l.low_airways,
        "vors" => &mut l.vors,
        "airports" => &mut l.airports,
        "fixes" => &mut l.fixes,
        "labels" => &mut l.labels,
        "rings" => &mut l.rings,
        _ => return false,
    };
    *slot = on;
    true
}

/// The layer switches as an array in the order of [`LAYER_NAMES`].
pub fn layers_flags(l: &Layers) -> [bool; 9] {
    [
        l.own_boundary,
        l.neighbour_boundaries,
        l.high_airways,
        l.low_airways,
        l.vors,
        l.airports,
        l.fixes,
        l.labels,
        l.rings,
    ]
}

/// A projection, the layer switches and the range thresholds.
// A7.2: cold struct, size guard waived (one per display).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub proj: Projection,
    /// Latitude and longitude box of the map, for rejecting positions before projecting them.
    pub bbox: Option<Bbox>,
    pub layers: Layers,
    /// Every threshold is compared with the display range, not with a distance from the centre.
    /// Fixes are drawn only when the range is at most this many nautical miles.
    pub fix_max_nm: f64,
    /// Terminal waypoints: likewise.
    pub terminal_wpt_max_nm: f64,
    /// Runway thresholds and localizers: likewise.
    pub runway_max_nm: f64,
    /// Airports without a coded instrument procedure: likewise.
    pub non_ifr_airport_max_nm: f64,
    /// NDB, DME and TACAN: likewise.
    pub minor_navaid_max_nm: f64,
    /// Labels of fixes, minor navaids, other airports and terminal waypoints: likewise.
    pub label_max_nm: f64,
    /// Labels of VORs and of airports with a coded instrument procedure: likewise.
    pub major_label_max_nm: f64,
}

/// A view with every layer on and the default thresholds.
pub fn view_new(
    centre_lat_e7: i32,
    centre_lon_e7: i32,
    range_nm: f64,
    width: u32,
    height: u32,
) -> Result<View, DisplayError> {
    view_new_with_top_inset(
        centre_lat_e7,
        centre_lon_e7,
        range_nm,
        width,
        height,
        LEGEND_STRIP_PX,
    )
}

/// A view with every layer on and the default thresholds, reserving `top_inset` screen rows for
/// application-owned header furniture. The inset must include the standard legend strip; the
/// remaining map height is checked by [`projection_new`] before any scene scratch is allocated.
pub fn view_new_with_top_inset(
    centre_lat_e7: i32,
    centre_lon_e7: i32,
    range_nm: f64,
    width: u32,
    height: u32,
    top_inset: u32,
) -> Result<View, DisplayError> {
    if top_inset < LEGEND_STRIP_PX {
        return Err(DisplayError::BadView(
            "top inset must include the standard legend strip",
        ));
    }
    let proj = projection_new(
        centre_lat_e7,
        centre_lon_e7,
        range_nm,
        width,
        height,
        top_inset,
    )?;
    Ok(View {
        bbox: view_bbox(&proj, MARGIN / 16),
        proj,
        layers: layers_all(),
        fix_max_nm: DEFAULT_FIX_MAX_NM,
        terminal_wpt_max_nm: DEFAULT_TERMINAL_WPT_MAX_NM,
        runway_max_nm: DEFAULT_RUNWAY_MAX_NM,
        non_ifr_airport_max_nm: DEFAULT_NON_IFR_AIRPORT_MAX_NM,
        minor_navaid_max_nm: DEFAULT_MINOR_NAVAID_MAX_NM,
        label_max_nm: DEFAULT_LABEL_MAX_NM,
        major_label_max_nm: DEFAULT_MAJOR_LABEL_MAX_NM,
    })
}

/// What a build drew and what it left out.
// A7.2: cold struct, size guard waived (one per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub own_boundaries: u32,
    pub neighbour_boundaries: u32,
    pub airway_legs: u32,
    pub airway_legs_off_screen: u32,
    pub fixes: u32,
    pub terminal_waypoints: u32,
    /// Runway thresholds and localizers.
    pub runway_symbols: u32,
    pub navaids: u32,
    pub airports: u32,
    pub labels: u32,
    /// Labels that had a candidate and found no room: expected on a busy screen, counted.
    pub labels_dropped: u32,
    pub rings: u32,
    /// Airway legs with an end at a published border crossing: expected, not drawable.
    pub legs_at_border: u32,
    /// Fixes, navaids and airports with no position or that cannot be projected.
    pub points_unplaced: u32,
    /// Airway legs with an end that has no position or cannot be projected, not at a border.
    pub legs_unplaced: u32,
    /// Boundary vertices and edges that cannot be projected, and rings that do not index.
    pub edges_unplaced: u32,
    /// Legend texts that run past the right edge of a narrow screen: each is marked at the edge
    /// and the command line prints the whole legend.
    pub legend_cut: u32,
}

/// What the scene needs to know about a model beyond its tables as stored, found once at
/// start-up (allocating) and then read every frame.
// A7.2: cold struct, size guard waived (one per model).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneIndex {
    /// Per airway leg: 1 for a high airway (designator J or Q), else 0.
    pub leg_high: Vec<u8>,
    /// Per airway leg: bit 0 when its start is a border crossing, bit 1 when its end is.
    pub leg_border: Vec<u8>,
    /// Per point: 1 for an airport with at least one coded instrument procedure.
    pub point_ifr: Vec<u8>,
    /// Per boundary: 1 when it belongs to the facility the model was built for.
    pub boundary_own: Vec<u8>,
}

/// Builds the index. A leg end is a border crossing when the model's unresolved table holds a
/// `BORDER_CROSSING` row for that airway at that leg's ordinal (the start) or the next ordinal
/// (the end), and the end is truly absent from the leg.
pub fn scene_index(m: &Model) -> SceneIndex {
    let n_legs = m.airway_legs.from.len();
    let mut leg_high = vec![0u8; n_legs];
    let mut leg_border = vec![0u8; n_legs];
    for (desig, span) in m.airways.designator.iter().zip(&m.airways.legs) {
        let high = matches!(
            str_or_empty(&m.strings, *desig).as_bytes().first(),
            Some(b'J' | b'Q')
        );
        let start = span.start.row();
        for j in 0..span.len as usize {
            if let Some(h) = leg_high.get_mut(start + j) {
                *h = u8::from(high);
            }
        }
    }
    let u = &m.unresolved;
    for i in 0..u.role.len() {
        let border = u.role.get(i) == Some(&unresolved_role::BORDER_CROSSING)
            && u.owner_kind.get(i) == Some(&oa_airspace::core::model::owner_kind::AIRWAY);
        if !border {
            continue;
        }
        let (Some(owner), Some(scope), Some(ord)) =
            (u.owner.get(i), u.scope.get(i), u.ordinal.get(i))
        else {
            continue;
        };
        let (owner, scope) = (
            str_or_empty(&m.strings, *owner),
            str_or_empty(&m.strings, *scope),
        );
        let ord = *ord as usize;
        for ((desig, loc), span) in m
            .airways
            .designator
            .iter()
            .zip(&m.airways.location)
            .zip(&m.airways.legs)
        {
            if str_or_empty(&m.strings, *desig) != owner || scope.as_bytes() != [*loc] {
                continue;
            }
            let start = span.start.row();
            let len = span.len as usize;
            let mut mark = |j: usize, bit: u8| {
                if j < len {
                    if let Some(b) = leg_border.get_mut(start + j) {
                        *b |= bit;
                    }
                }
            };
            mark(ord, 1);
            if let Some(prev) = ord.checked_sub(1) {
                mark(prev, 2);
            }
        }
    }
    // A border bit stands only where the leg really lacks that end.
    for (r, b) in leg_border.iter_mut().enumerate() {
        let from_none = m
            .airway_legs
            .from
            .get(r)
            .is_none_or(|p| *p == PointIx::NONE);
        let to_none = m.airway_legs.to.get(r).is_none_or(|p| *p == PointIx::NONE);
        if !from_none {
            *b &= !1;
        }
        if !to_none {
            *b &= !2;
        }
    }
    let mut point_ifr = vec![0u8; m.points.kind.len()];
    for (i, ap) in m.procs.airport.iter().enumerate() {
        if bit_get(&m.procs.coded, i) {
            if let Some(f) = point_ifr.get_mut(ap.row()) {
                *f = 1;
            }
        }
    }
    let boundary_own = m
        .boundaries
        .facility
        .iter()
        .map(|f| u8::from(str_or_empty(&m.strings, *f) == m.meta.artcc))
        .collect();
    SceneIndex {
        leg_high,
        leg_border,
        point_ifr,
        boundary_own,
    }
}

/// Capacities that cannot overflow for `m`: counted from the model, with the worst-case edge
/// splitting and the fixed furniture (C2). Reaching a capacity is still an error, never a wrap.
/// Every size is derived in 128-bit arithmetic and checked against [`MAX_LIST_ROWS`] (or
/// [`MAX_LIST_TEXT_BYTES`]) before it is returned, so a model that asks for more is refused with
/// [`DisplayError::TooLarge`] and nothing is allocated.
pub fn scene_caps(m: &Model) -> Result<ListCaps, DisplayError> {
    let points = m.points.kind.len() as u128;
    let boundary_vertices: u128 = m
        .boundaries
        .stratum
        .iter()
        .zip(&m.boundaries.vertices)
        .filter(|(s, _)| **s == stratum::ARTCC_HIGH || **s == stratum::ARTCC_LOW)
        .map(|(_, v)| u128::from(v.len) + 1)
        .sum();
    let ident_bytes: u128 = m
        .points
        .ident
        .iter()
        .map(|i| str_or_empty(&m.strings, *i).len() as u128)
        .sum();
    let furniture = FURNITURE as u128;
    let rows = |what: &'static str, n: u128| check_limit(what, n, MAX_LIST_ROWS);
    Ok(ListCaps {
        lines: rows(
            "airway and symbol lines",
            (m.airway_legs.from.len() as u128) * u128::from(SUBDIV_AIRWAY) + points * 4 + furniture,
        )?,
        rects: 1,
        circles: rows("circles", points + furniture)?,
        polylines: rows("polylines", m.boundaries.stratum.len() as u128 + furniture)?,
        poly_vertices: rows(
            "boundary polyline vertices",
            boundary_vertices * u128::from(SUBDIV_BOUNDARY),
        )?,
        texts: rows("texts", points + furniture)?,
        text_bytes: check_limit(
            "text bytes",
            ident_bytes + furniture * 32,
            MAX_LIST_TEXT_BYTES,
        )?,
    })
}

/// Label candidates and the occupancy grid, allocated once by [`scratch_for`] and reused by every
/// frame (C1): a frame allocates nothing for placement.
// A7.2: cold struct, size guard waived (one per display).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scratch {
    /// Candidate capacity: the number of points in the model.
    pub cap: usize,
    /// Candidates the frame has filled so far (at most `cap`); every per-candidate buffer below
    /// has a fixed length of `cap` and is written by index, so a frame never grows one.
    pub used: usize,
    /// Per candidate, filled by the frame: priority (1 first), symbol centre and gap to the label,
    /// label width (all 1/16 pixel), model point row.
    pub prio: Vec<u8>,
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub gap: Vec<i32>,
    pub w: Vec<i32>,
    pub point: Vec<u32>,
    /// Candidate indexes in placement order.
    pub order: Vec<u32>,
    /// The merge sort's one scratch buffer for `order`, bounded by the candidate capacity.
    pub order_tmp: Vec<u32>,
    /// Per candidate: the offset 0 to 3 it was placed at, or [`DROPPED_NO_ROOM`] or
    /// [`DROPPED_CELL_FULL`].
    pub outcome: Vec<u8>,
    /// Candidates beyond `cap` (cannot happen for a scratch made for the model).
    pub overflow: u32,
    /// Grid size in cells of 64 by 64 pixels, and per cell the number of boxes and the boxes
    /// (`CELL_CAP` per cell) as left, top, right, bottom.
    pub cols: usize,
    pub rows: usize,
    pub cell_n: Vec<u8>,
    pub cell_box: Vec<[i32; 4]>,
    /// Where a label may lie: left, top, right, bottom, 1/16 pixel.
    pub bounds: [i32; 4],
}

/// The label found no offset free of other labels, the legend strip and the screen edge.
pub const DROPPED_NO_ROOM: u8 = 254;
/// An offset was free of other labels but a grid cell it touches is full.
pub const DROPPED_CELL_FULL: u8 = 255;
/// Boxes per grid cell; 64 by 64 pixels holds at most a few 12 pixel labels anyway.
pub const CELL_CAP: usize = 24;
const CELL_SHIFT: u32 = 10;
/// Space kept around a label, 1/16 pixel each side.
pub const LABEL_PAD16: i32 = 2 * 16;

/// Most label boxes the occupancy grid of a scratch may hold (a cell of 64 by 64 pixels holds
/// [`CELL_CAP`]): 4 Mi boxes of 16 bytes is 64 MB, enough for a screen over 26 000 pixels a side.
pub const MAX_GRID_BOXES: usize = 1 << 22;

/// A scratch for `m` drawn at the size of `v`. A candidate count above [`MAX_LIST_ROWS`] or a
/// grid above [`MAX_GRID_BOXES`] is refused with [`DisplayError::TooLarge`] before anything is
/// allocated.
pub fn scratch_for(m: &Model, v: &View) -> Result<Scratch, DisplayError> {
    let pixels = u128::from(v.proj.width) * u128::from(v.proj.height);
    let n = check_limit(
        "label candidates",
        m.points.kind.len() as u128,
        MAX_LIST_ROWS,
    )?;
    let cols = (v.proj.width as usize).div_ceil(64).max(1);
    let rows = (v.proj.height as usize).div_ceil(64).max(1);
    let cells = (cols as u128) * (rows as u128);
    check_limit("label grid boxes", cells * CELL_CAP as u128, MAX_GRID_BOXES)?;
    if pixels > MAX_SCENE_PIXELS {
        return Err(DisplayError::TooLarge {
            what: "scene pixels",
            requested: pixels,
            limit: MAX_SCENE_PIXELS,
        });
    }
    Ok(Scratch {
        cap: n,
        used: 0,
        prio: vec![0; n],
        x: vec![0; n],
        y: vec![0; n],
        gap: vec![0; n],
        w: vec![0; n],
        point: vec![0; n],
        order: vec![0; n],
        order_tmp: vec![0; n],
        outcome: vec![DROPPED_NO_ROOM; n],
        overflow: 0,
        cols,
        rows,
        cell_n: vec![0; cols * rows],
        cell_box: vec![[0; 4]; cols * rows * CELL_CAP],
        bounds: [0; 4],
    })
}

/// Empties the candidates and the grid and sets where labels may lie.
fn scratch_clear(s: &mut Scratch, p: &Projection) {
    s.used = 0;
    s.overflow = 0;
    s.cell_n.fill(0);
    let px16 = |v: u32| i32::try_from(i64::from(v) * 16).unwrap_or(i32::MAX);
    s.bounds = [0, px16(p.top_inset), px16(p.width), px16(p.height)];
}

fn boxes_overlap(a: &[i32; 4], b: &[i32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The grid cells a box touches, clamped to the grid.
fn cell_span(s: &Scratch, b: &[i32; 4]) -> (usize, usize, usize, usize) {
    let col = |x: i32| ((x.max(0) >> CELL_SHIFT) as usize).min(s.cols.saturating_sub(1));
    let row = |y: i32| ((y.max(0) >> CELL_SHIFT) as usize).min(s.rows.saturating_sub(1));
    (
        col(b[0]),
        row(b[1]),
        col(b[2].saturating_sub(1)),
        row(b[3].saturating_sub(1)),
    )
}

enum Fit {
    Free,
    Blocked,
    CellFull,
}

/// Whether a padded box is inside the bounds, free of placed boxes and has room in its cells.
fn grid_fit(s: &Scratch, b: &[i32; 4]) -> Fit {
    let bd = &s.bounds;
    if b[0] < bd[0] || b[1] < bd[1] || b[2] > bd[2] || b[3] > bd[3] {
        return Fit::Blocked;
    }
    let (c0, r0, c1, r1) = cell_span(s, b);
    let mut full = false;
    for r in r0..=r1 {
        for c in c0..=c1 {
            let cell = r * s.cols + c;
            let n = usize::from(s.cell_n.get(cell).copied().unwrap_or(CELL_CAP as u8));
            full |= n >= CELL_CAP;
            for k in 0..n.min(CELL_CAP) {
                if s.cell_box
                    .get(cell * CELL_CAP + k)
                    .is_some_and(|o| boxes_overlap(b, o))
                {
                    return Fit::Blocked;
                }
            }
        }
    }
    if full {
        Fit::CellFull
    } else {
        Fit::Free
    }
}

/// Records a padded box in every cell it touches (the caller has checked there is room).
fn grid_insert(s: &mut Scratch, b: &[i32; 4]) {
    let (c0, r0, c1, r1) = cell_span(s, b);
    for r in r0..=r1 {
        for c in c0..=c1 {
            let cell = r * s.cols + c;
            let n = usize::from(s.cell_n.get(cell).copied().unwrap_or(CELL_CAP as u8));
            if n < CELL_CAP {
                if let Some(slot) = s.cell_box.get_mut(cell * CELL_CAP + n) {
                    *slot = *b;
                    if let Some(c) = s.cell_n.get_mut(cell) {
                        *c += 1;
                    }
                }
            }
        }
    }
}

/// Reserves the space of a box the scene itself draws (a ring label), padded, if it fits.
fn grid_reserve(s: &mut Scratch, text_box: &[i32; 4]) {
    let b = pad_box(text_box);
    if matches!(grid_fit(s, &b), Fit::Free) {
        grid_insert(s, &b);
    }
}

fn pad_box(b: &[i32; 4]) -> [i32; 4] {
    [
        b[0].saturating_sub(LABEL_PAD16),
        b[1].saturating_sub(LABEL_PAD16),
        b[2].saturating_add(LABEL_PAD16),
        b[3].saturating_add(LABEL_PAD16),
    ]
}

/// The text anchor, the alignment and the text box of a label of width `w` at offset `k` (0 north
/// east, 1 south east, 2 north west, 3 south west) of a symbol at (`cx`, `cy`) with room `gap`.
pub fn label_geom(gap: i32, w: i32, cx: i32, cy: i32, k: u8) -> ((i32, i32), u8, [i32; 4]) {
    let h = i32::from(LABEL_SIZE) * 16;
    let east = k == 0 || k == 1;
    let north = k == 0 || k == 2;
    let ax = if east {
        cx.saturating_add(gap)
    } else {
        cx.saturating_sub(gap)
    };
    let ay = if north {
        cy.saturating_sub(gap).saturating_sub(h)
    } else {
        cy.saturating_add(gap)
    };
    let x0 = if east { ax } else { ax.saturating_sub(w) };
    let align = if east { ALIGN_LEFT } else { ALIGN_RIGHT };
    (
        (ax, ay),
        align,
        [x0, ay, x0.saturating_add(w), ay.saturating_add(h)],
    )
}

/// Places every candidate in `s.order` at its first free offset and records the outcome per
/// candidate. Returns the number placed.
pub fn place_labels(s: &mut Scratch) -> u32 {
    for o in s.outcome.iter_mut().take(s.used) {
        *o = DROPPED_NO_ROOM;
    }
    let mut placed = 0;
    for oi in 0..s.used {
        let Some(&i) = s.order.get(oi) else { break };
        let i = i as usize;
        let (Some(&cx), Some(&cy), Some(&gap), Some(&w)) =
            (s.x.get(i), s.y.get(i), s.gap.get(i), s.w.get(i))
        else {
            continue;
        };
        let mut result = DROPPED_NO_ROOM;
        let mut full = false;
        for k in 0..4u8 {
            let (_, _, tb) = label_geom(gap, w, cx, cy, k);
            let b = pad_box(&tb);
            match grid_fit(s, &b) {
                Fit::Free => {
                    grid_insert(s, &b);
                    result = k;
                    placed += 1;
                    break;
                }
                Fit::Blocked => {}
                Fit::CellFull => full = true,
            }
        }
        if result == DROPPED_NO_ROOM && full {
            result = DROPPED_CELL_FULL;
        }
        if let Some(o) = s.outcome.get_mut(i) {
            *o = result;
        }
    }
    placed
}

// A7.1/C2: all 512 layer-switch combinations at the largest displayed threshold
// (19999.9 NM, below the 20000 NM view limit) need at most 254 bytes for HIDDEN.
// The exhaustive test below pins that bound. 256 replaces the insufficient 192;
// this is stack-only formatting scratch, not a larger display/input collection.
const LABEL_BYTES: usize = 256;

/// A short string built on the stack, so a frame allocates nothing for its labels.
#[repr(C)]
struct Label {
    buf: [u8; LABEL_BYTES],
    len: usize,
}
const _: () = assert!(std::mem::size_of::<Label>() == LABEL_BYTES + std::mem::size_of::<usize>());

impl Write for Label {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(std::fmt::Error)?;
        let dst = self.buf.get_mut(self.len..end).ok_or(std::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn label() -> Label {
    Label {
        buf: [0; LABEL_BYTES],
        len: 0,
    }
}

fn label_str(l: &Label) -> &str {
    l.buf
        .get(..l.len)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

/// Writes a number of nautical miles: whole when whole, else one decimal.
fn put_nm(l: &mut Label, nm: f64) -> std::fmt::Result {
    if (nm - oa_ieee::round(nm)).abs() < 1e-9 {
        write!(l, "{}", oa_ieee::round(nm) as i64)
    } else {
        write!(l, "{nm:.1}")
    }
}

const DEG_E7_RAD: f64 = 1e-7 * std::f64::consts::PI / 180.0;

fn unit(lat_e7: i32, lon_e7: i32) -> [f64; 3] {
    let (lat, lon) = (
        f64::from(lat_e7) * DEG_E7_RAD,
        f64::from(lon_e7) * DEG_E7_RAD,
    );
    [
        oa_math::cos(lat) * oa_math::cos(lon),
        oa_math::cos(lat) * oa_math::sin(lon),
        oa_math::sin(lat),
    ]
}

/// How an edge between two positions is split, and the screen ends of it.
// A7.2: cold struct, size guard waived (stack scratch).
struct Edge {
    a: [f64; 3],
    b: [f64; 3],
    pa: (i32, i32),
    pb: (i32, i32),
    pieces: u32,
}

fn edge_plan(proj: &Projection, a: (i32, i32), b: (i32, i32), max_pieces: u32) -> Option<Edge> {
    let pa = project(proj, a.0, a.1)?;
    let pb = project(proj, b.0, b.1)?;
    let chord = (i64::from(pb.0) - i64::from(pa.0))
        .abs()
        .max((i64::from(pb.1) - i64::from(pa.1)).abs());
    // The ground length of the chord to within a few per cent over the whole map (the scale is
    // within 1 + c^2 / 4 of 1); an edge clearly short in both measures needs no splitting and
    // costs no trigonometry.
    let ground_est = chord as f64 / proj.scale;
    if chord <= PIECE_16 && ground_est < PIECE_M * 0.9 {
        return Some(Edge {
            a: [0.0; 3],
            b: [0.0; 3],
            pa,
            pb,
            pieces: 1,
        });
    }
    let (ua, ub) = (unit(a.0, a.1), unit(b.0, b.1));
    let dot = oa_math::clamp_f64(ua[0] * ub[0] + ua[1] * ub[1] + ua[2] * ub[2], -1.0, 1.0);
    let ground = oa_math::acos(dot) * proj.radius_m;
    let by_px = (chord + PIECE_16 - 1) / PIECE_16;
    let by_ground = oa_ieee::ceil(ground / PIECE_M);
    let by_ground = if by_ground.is_finite() {
        by_ground as i64
    } else {
        1
    };
    let pieces = by_px.max(by_ground).clamp(1, i64::from(max_pieces)) as u32;
    Some(Edge {
        a: ua,
        b: ub,
        pa,
        pb,
        pieces,
    })
}

/// The screen position of piece boundary `i` (0 to `pieces`) along the great circle.
fn edge_point(proj: &Projection, e: &Edge, i: u32) -> Option<(i32, i32)> {
    if i == 0 {
        return Some(e.pa);
    }
    if i >= e.pieces {
        return Some(e.pb);
    }
    let t = f64::from(i) / f64::from(e.pieces);
    let v = [
        e.a[0] * (1.0 - t) + e.b[0] * t,
        e.a[1] * (1.0 - t) + e.b[1] * t,
        e.a[2] * (1.0 - t) + e.b[2] * t,
    ];
    let n = oa_ieee::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    if n < 1e-9 {
        // The ends are nearly opposite; the great circle is not defined. Draw straight.
        return Some(if t < 0.5 { e.pa } else { e.pb });
    }
    project_rad(
        proj,
        oa_math::asin(oa_math::clamp_f64(v[2] / n, -1.0, 1.0)),
        oa_math::atan2(v[1], v[0]),
    )
}

/// True when both points lie beyond the same side of the screen (with a margin).
fn off_same_side(proj: &Projection, a: (i32, i32), b: (i32, i32)) -> bool {
    let w = i64::from(proj.width) * 16;
    let h = i64::from(proj.height) * 16;
    let t = i64::from(proj.top_inset) * 16;
    let m = i64::from(MARGIN);
    let (ax, ay, bx, by) = (
        i64::from(a.0),
        i64::from(a.1),
        i64::from(b.0),
        i64::from(b.1),
    );
    (ax < -m && bx < -m)
        || (ax > w + m && bx > w + m)
        || (ay < t - m && by < t - m)
        || (ay > h + m && by > h + m)
}

fn on_screen(proj: &Projection, p: (i32, i32)) -> bool {
    let w = i64::from(proj.width) * 16;
    let h = i64::from(proj.height) * 16;
    let t = i64::from(proj.top_inset) * 16;
    let m = i64::from(MARGIN);
    (-m..=w + m).contains(&i64::from(p.0)) && (t - m..=h + m).contains(&i64::from(p.1))
}

/// True when both positions lie beyond the same side of the box.
fn outside_same_side(bb: &Bbox, a: (i32, i32), b: (i32, i32)) -> bool {
    let (da, db) = (dlon_e7(a.1, bb.centre_lon), dlon_e7(b.1, bb.centre_lon));
    (a.0 < bb.lat_min && b.0 < bb.lat_min)
        || (a.0 > bb.lat_max && b.0 > bb.lat_max)
        || (da < bb.dlon_min && db < bb.dlon_min)
        || (da > bb.dlon_max && db > bb.dlon_max)
}

fn add(p: (i32, i32), dx: i32, dy: i32) -> (i32, i32) {
    (p.0.saturating_add(dx), p.1.saturating_add(dy))
}

/// The ring spacing, nautical miles: the first of a fixed series at least [`MIN_RING_PX`] wide.
pub fn ring_step_nm(proj: &Projection) -> f64 {
    let map_h = proj.height - proj.top_inset;
    let px_per_nm = f64::from(proj.width.min(map_h)) / 2.0 / proj.range_nm;
    RING_STEPS_NM
        .iter()
        .copied()
        .find(|s| s * px_per_nm >= MIN_RING_PX)
        .unwrap_or(RING_STEP_MAX_NM)
}

/// Refuses a view whose numbers would make the build silently wrong: a threshold that is NaN,
/// infinite or negative (a comparison with NaN hides every layer it governs), a projection whose
/// range, scale, centre position or radius is not finite or whose screen is empty. A view made
/// by [`view_new`] passes; one assembled by hand may not.
fn view_check(v: &View) -> Result<(), DisplayError> {
    let thresholds = [
        (
            v.fix_max_nm,
            "fix_max_nm is not a finite, non-negative number",
        ),
        (
            v.terminal_wpt_max_nm,
            "terminal_wpt_max_nm is not a finite, non-negative number",
        ),
        (
            v.runway_max_nm,
            "runway_max_nm is not a finite, non-negative number",
        ),
        (
            v.non_ifr_airport_max_nm,
            "non_ifr_airport_max_nm is not a finite, non-negative number",
        ),
        (
            v.minor_navaid_max_nm,
            "minor_navaid_max_nm is not a finite, non-negative number",
        ),
        (
            v.label_max_nm,
            "label_max_nm is not a finite, non-negative number",
        ),
        (
            v.major_label_max_nm,
            "major_label_max_nm is not a finite, non-negative number",
        ),
    ];
    for (x, why) in thresholds {
        if !(x.is_finite() && x >= 0.0) {
            return Err(DisplayError::BadView(why));
        }
    }
    let p = &v.proj;
    if !p.range_nm.is_finite() || !(MIN_RANGE_NM..=MAX_RANGE_NM).contains(&p.range_nm) {
        return Err(DisplayError::BadView("range is outside 0.05 to 20000 NM"));
    }
    let finite = [p.scale, p.cx16, p.cy16, p.radius_m, p.ecc, p.n, p.c]
        .iter()
        .all(|x| x.is_finite());
    if !finite || p.scale <= 0.0 || p.radius_m <= 0.0 {
        return Err(DisplayError::BadView(
            "the projection has a non-finite or non-positive constant",
        ));
    }
    if p.width == 0 || p.height == 0 || p.top_inset >= p.height {
        return Err(DisplayError::BadView("the screen or its map area is empty"));
    }
    Ok(())
}

/// Clears `l` and builds the static map for `m` as seen through `v`. On an error the list is left
/// faulted (it overflowed or was misused) and will not be drawn; show a fault, never the frame.
pub fn build_scene(
    l: &mut DisplayList,
    m: &Model,
    idx: &SceneIndex,
    v: &View,
    sc: &mut Scratch,
) -> Result<SceneStats, DisplayError> {
    view_check(v)?;
    let cols = (v.proj.width as usize).div_ceil(64).max(1);
    let rows = (v.proj.height as usize).div_ceil(64).max(1);
    let cells = cols as u128 * rows as u128;
    let boxes = cells * CELL_CAP as u128;
    if u128::from(v.proj.width) * u128::from(v.proj.height) > MAX_SCENE_PIXELS {
        return Err(DisplayError::TooLarge {
            what: "scene pixels",
            requested: u128::from(v.proj.width) * u128::from(v.proj.height),
            limit: MAX_SCENE_PIXELS,
        });
    }
    if sc.cols != cols
        || sc.rows != rows
        || sc.cell_n.len() as u128 != cells
        || sc.cell_box.len() as u128 != boxes
    {
        return Err(DisplayError::BadView(
            "label scratch does not match the view",
        ));
    }
    let point_rows = m.points.kind.len();
    if point_rows > MAX_LIST_ROWS {
        return Err(DisplayError::TooLarge {
            what: "scene point rows",
            requested: point_rows as u128,
            limit: MAX_LIST_ROWS as u128,
        });
    }
    if sc.cap < point_rows
        || sc.prio.len() < point_rows
        || sc.x.len() < point_rows
        || sc.y.len() < point_rows
        || sc.gap.len() < point_rows
        || sc.w.len() < point_rows
        || sc.point.len() < point_rows
        || sc.order.len() < point_rows
        || sc.order_tmp.len() < point_rows
        || sc.outcome.len() < point_rows
    {
        return Err(DisplayError::BadView(
            "label scratch capacity is below the model point count",
        ));
    }
    // This preflight span summation and the later boundary rendering pass each walk these rows;
    // the row-count gate below caps both passes, including zero-length spans.
    let boundary_rows = m.boundaries.stratum.len();
    if boundary_rows > MAX_LIST_ROWS {
        return Err(DisplayError::TooLarge {
            what: "scene boundary rows",
            requested: boundary_rows as u128,
            limit: MAX_LIST_ROWS as u128,
        });
    }
    let boundary_vertex_visits = m
        .boundaries
        .stratum
        .iter()
        .zip(&m.boundaries.vertices)
        .fold(0u128, |sum, (_, span)| {
            sum.saturating_add(u128::from(span.len))
        });
    let work = scene_work_bound(m, sc, cells, boundary_vertex_visits);
    check_scene_work(work)?;
    list_clear(l);
    scratch_clear(sc, &v.proj);
    let mut st = SceneStats::default();
    if v.layers.rings {
        rings(l, &v.proj, sc, &mut st)?;
    }
    boundaries(l, m, idx, v, &mut st)?;
    airways(l, m, idx, v, &mut st)?;
    points(l, m, idx, v, sc, &mut st)?;
    legend(l, m, v, &mut st)?;
    Ok(st)
}

/// The precharge [`build_scene`] checks against [`MAX_SCENE_WORK`] for `m` seen through `v` with
/// scratch `sc`, for reporting.
pub fn scene_work_charge(m: &Model, sc: &Scratch, v: &View) -> u128 {
    let cells = (v.proj.width as usize).div_ceil(64).max(1) as u128
        * (v.proj.height as usize).div_ceil(64).max(1) as u128;
    let visits = m
        .boundaries
        .vertices
        .iter()
        .fold(0u128, |sum, span| sum.saturating_add(u128::from(span.len)));
    scene_work_bound(m, sc, cells, visits)
}

/// Refuses a charge above [`MAX_SCENE_WORK`]; the charge equal to it is accepted.
fn check_scene_work(work: u128) -> Result<(), DisplayError> {
    if work > MAX_SCENE_WORK {
        return Err(DisplayError::TooLarge {
            what: "scene work units",
            requested: work,
            limit: MAX_SCENE_WORK,
        });
    }
    Ok(())
}

/// `ceil(log2 n)` for `n >= 2`, 0 for `n <= 1`.
fn ceil_log2(n: u128) -> u128 {
    u128::from(u128::BITS - n.saturating_sub(1).leading_zeros())
}

/// Bottom-up merge sort of `data` by `cmp`, using `tmp` (the same length) as its one scratch
/// buffer: deterministic, no recursion, no allocation. A merge of two runs of `m` elements in all
/// makes at most `m - 1` comparisons, and there are `ceil(log2 n)` passes, so it makes at most
/// `n * ceil(log2 n)` comparisons (0 for `n <= 1`). Equal elements keep their order.
fn merge_sort_by<T: Copy>(
    data: &mut [T],
    tmp: &mut [T],
    cmp: &mut dyn FnMut(&T, &T) -> std::cmp::Ordering,
) -> Result<(), DisplayError> {
    let n = data.len();
    if tmp.len() != n {
        return Err(DisplayError::BadView(
            "label sort scratch length differs from the candidates",
        ));
    }
    let mut width = 1usize;
    let mut in_data = true;
    while width < n {
        let (src, dst): (&[T], &mut [T]) = if in_data {
            (&*data, &mut *tmp)
        } else {
            (&*tmp, &mut *data)
        };
        merge_pass(src, dst, width, cmp);
        in_data = !in_data;
        width = width.saturating_mul(2);
    }
    if !in_data {
        data.copy_from_slice(tmp);
    }
    Ok(())
}

/// One pass: merges each pair of adjacent runs of `width` from `src` into `dst`.
fn merge_pass<T: Copy>(
    src: &[T],
    dst: &mut [T],
    width: usize,
    cmp: &mut dyn FnMut(&T, &T) -> std::cmp::Ordering,
) {
    let n = src.len();
    let mut lo = 0usize;
    while lo < n {
        let mid = lo.saturating_add(width).min(n);
        let hi = lo.saturating_add(width.saturating_mul(2)).min(n);
        let (mut i, mut j, mut k) = (lo, mid, lo);
        while i < mid || j < hi {
            let take_left = match (src.get(i), src.get(j)) {
                (Some(a), Some(b)) if i < mid && j < hi => cmp(a, b) != std::cmp::Ordering::Greater,
                _ => i < mid,
            };
            let from = if take_left { i } else { j };
            if let (Some(out), Some(v)) = (dst.get_mut(k), src.get(from)) {
                *out = *v;
            }
            if take_left {
                i += 1;
            } else {
                j += 1;
            }
            k += 1;
        }
        lo = hi;
    }
}

/// Upper bound on the point-label sort: [`merge_sort_by`] makes at most `n * ceil(log2 n)`
/// comparisons, and one comparison reads at most the longest identifier plus the four fixed key
/// fields (priority, latitude, longitude, row).
fn point_sort_work(m: &Model) -> u128 {
    let n = m.points.kind.len() as u128;
    let longest = m
        .points
        .ident
        .iter()
        .map(|i| str_or_empty(&m.strings, *i).len() as u128)
        .max()
        .unwrap_or(0);
    n.saturating_mul(ceil_log2(n)).saturating_mul(longest + 4)
}

/// Upper bound for each input-derived loop in a frame scene. This preflight runs before clearing
/// the caller's list, so a refused view/model leaves the previous complete scene available.
fn scene_work_bound(m: &Model, sc: &Scratch, cells: u128, boundary_vertex_visits: u128) -> u128 {
    let points = m.points.kind.len() as u128;
    let boundaries = m.boundaries.stratum.len() as u128;
    let legs = m.airway_legs.from.len() as u128;
    let point_sort = point_sort_work(m);
    let grid = sc.cell_n.len() as u128;
    points
        .saturating_mul(34)
        .saturating_add(boundaries.saturating_mul(2))
        .saturating_add(boundary_vertex_visits.saturating_mul(u128::from(SUBDIV_BOUNDARY)))
        .saturating_add(legs.saturating_mul(u128::from(SUBDIV_AIRWAY) + 1))
        .saturating_add(point_sort)
        .saturating_add(grid)
        .saturating_add(
            (MAX_RINGS as u128)
                .saturating_mul(cells)
                .saturating_mul(CELL_CAP as u128)
                .saturating_mul(2),
        )
        .saturating_add(
            points
                .saturating_mul(5)
                .saturating_mul(cells)
                .saturating_mul(CELL_CAP as u128),
        )
}

/// The ring labels sit where each ring meets the ray 45 degrees east of north from the centre.
/// Rings are at least [`MIN_RING_PX`] apart along every ray, so along this one successive label
/// anchors are at least 63 pixels apart in both x and y, more than a label is tall or wide.
fn rings(
    l: &mut DisplayList,
    p: &Projection,
    sc: &mut Scratch,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let centre = (oa_ieee::round(p.cx16) as i32, oa_ieee::round(p.cy16) as i32);
    let step = ring_step_nm(p);
    let map_h = f64::from(p.height - p.top_inset);
    let half_diag16 = oa_math::hypot(f64::from(p.width), map_h) * 8.0;
    let (w16, top16) = (i64::from(p.width) * 16, i64::from(p.top_inset) * 16);
    let mut k = 1_i64;
    while k <= MAX_RINGS {
        let nm = step * k as f64;
        let Some(r) = ring_radius16(p, nm * NM_M) else {
            break;
        };
        if f64::from(r) > half_diag16 {
            break;
        }
        push_circle(l, centre, r, palette::RING, LAYER_RING)?;
        st.rings += 1;
        // cos 45 degrees as 181 / 256, exact integer arithmetic.
        let d = i64::from(r) * 181 / 256;
        let (ax, ay) = (i64::from(centre.0) + d, i64::from(centre.1) - d);
        let (lx, ly) = (ax + 5 * 16, ay - 12 * 16);
        if lx < w16 && ly > top16 {
            let mut t = label();
            put_nm(&mut t, nm).map_err(legend_format_full)?;
            // Point labels keep clear of this one.
            let (wd, ht) = (
                legend_width16(label_str(&t).len()),
                i64::from(LABEL_SIZE) * 16,
            );
            grid_reserve(
                sc,
                &[
                    lx as i32,
                    ly as i32,
                    (lx as i32).saturating_add(wd),
                    (ly + ht) as i32,
                ],
            );
            push_text(
                l,
                (lx as i32, ly as i32),
                LABEL_SIZE,
                ALIGN_LEFT,
                palette::DIM_TEXT,
                LAYER_TEXT,
                label_str(&t),
            )?;
        }
        k += 1;
    }
    let arm = 6 * 16;
    push_line(
        l,
        add(centre, -arm, 0),
        add(centre, arm, 0),
        palette::DIM_TEXT,
        LAYER_RING,
    )?;
    push_line(
        l,
        add(centre, 0, -arm),
        add(centre, 0, arm),
        palette::DIM_TEXT,
        LAYER_RING,
    )?;
    Ok(())
}

fn boundaries(
    l: &mut DisplayList,
    m: &Model,
    idx: &SceneIndex,
    v: &View,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let p = &v.proj;
    let b = &m.boundaries;
    for (i, (s, span)) in b.stratum.iter().zip(&b.vertices).enumerate() {
        let own = idx.boundary_own.get(i) == Some(&1);
        if (own && !v.layers.own_boundary) || (!own && !v.layers.neighbour_boundaries) {
            continue;
        }
        let (colour, layer, width) = match (*s, own) {
            (stratum::ARTCC_HIGH, true) => (palette::ARTCC_HIGH, LAYER_ARTCC_HIGH, 2),
            (stratum::ARTCC_LOW, true) => (palette::ARTCC_LOW, LAYER_ARTCC_LOW, 2),
            (stratum::ARTCC_HIGH, false) => (palette::NEIGHBOUR_HIGH, LAYER_NEIGHBOUR, 1),
            (stratum::ARTCC_LOW, false) => (palette::NEIGHBOUR_LOW, LAYER_NEIGHBOUR, 1),
            _ => continue,
        };
        let start = span.start.row();
        let Some(ring) = start
            .checked_add(span.len as usize)
            .and_then(|end| m.vertices.get(start..end))
        else {
            st.edges_unplaced += 1;
            continue;
        };
        let (Some(first), Some(last)) = (ring.first(), ring.last()) else {
            continue;
        };
        begin_polyline_w(l, colour, layer, width)?;
        if own {
            st.own_boundaries += 1;
        } else {
            st.neighbour_boundaries += 1;
        }
        // Closed: a ring whose last vertex is not its first is closed by an edge back.
        let closing = (first != last).then_some(first);
        let mut prev: Option<(i32, i32)> = None;
        for q in ring.iter().chain(closing) {
            let cur = (q.lat, q.lon);
            if let Some(a) = prev {
                match edge_plan(p, a, cur, SUBDIV_BOUNDARY) {
                    Some(e) => {
                        for i in 1..=e.pieces {
                            if let Some((x, y)) = edge_point(p, &e, i) {
                                push_vertex(l, x, y)?;
                            }
                        }
                    }
                    None => st.edges_unplaced += 1,
                }
            } else if let Some((x, y)) = project(p, cur.0, cur.1) {
                push_vertex(l, x, y)?;
            } else {
                st.edges_unplaced += 1;
            }
            prev = Some(cur);
        }
    }
    Ok(())
}

fn airways(
    l: &mut DisplayList,
    m: &Model,
    idx: &SceneIndex,
    v: &View,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let p = &v.proj;
    let pts = &m.points;
    let pos = |ix: PointIx| -> Option<(i32, i32)> {
        let lat = *pts.lat.get(ix.row())?;
        let lon = *pts.lon.get(ix.row())?;
        (lat != POS_NONE && lon != POS_NONE).then_some((lat, lon))
    };
    for (r, (f, t)) in m.airway_legs.from.iter().zip(&m.airway_legs.to).enumerate() {
        let high = idx.leg_high.get(r) == Some(&1);
        if (high && !v.layers.high_airways) || (!high && !v.layers.low_airways) {
            continue;
        }
        let border = idx.leg_border.get(r).copied().unwrap_or(0);
        if border != 0 {
            st.legs_at_border += 1;
            continue;
        }
        let (Some(a), Some(b)) = (pos(*f), pos(*t)) else {
            st.legs_unplaced += 1;
            continue;
        };
        if let Some(bb) = &v.bbox {
            if outside_same_side(bb, a, b) {
                st.airway_legs_off_screen += 1;
                continue;
            }
        }
        let Some(e) = edge_plan(p, a, b, SUBDIV_AIRWAY) else {
            st.legs_unplaced += 1;
            continue;
        };
        if off_same_side(p, e.pa, e.pb) {
            st.airway_legs_off_screen += 1;
            continue;
        }
        st.airway_legs += 1;
        let mut prev = e.pa;
        for i in 1..=e.pieces {
            if let Some(cur) = edge_point(p, &e, i) {
                push_line(l, prev, cur, palette::AIRWAY, LAYER_AIRWAY)?;
                prev = cur;
            }
        }
    }
    Ok(())
}

fn points(
    l: &mut DisplayList,
    m: &Model,
    idx: &SceneIndex,
    v: &View,
    sc: &mut Scratch,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let p = &v.proj;
    let range = p.range_nm;
    let pts = &m.points;
    let rows = pts.kind.iter().zip(&pts.lat).zip(&pts.lon).zip(&pts.ident);
    // Candidates the scratch can hold: its `cap`, and no more than the shortest buffer. The
    // up-front preflight already guarantees every buffer holds the model's point count.
    let lim = [
        sc.prio.len(),
        sc.x.len(),
        sc.y.len(),
        sc.gap.len(),
        sc.w.len(),
        sc.point.len(),
        sc.order.len(),
        sc.order_tmp.len(),
        sc.outcome.len(),
    ]
    .into_iter()
    .fold(sc.cap, usize::min);
    for (i, (((kind, lat), lon), ident)) in rows.enumerate() {
        let ifr = idx.point_ifr.get(i) == Some(&1);
        let shown = match *kind {
            point_kind::FIX => v.layers.fixes && range <= v.fix_max_nm,
            point_kind::TERMINAL_WAYPOINT => v.layers.fixes && range <= v.terminal_wpt_max_nm,
            point_kind::THRESHOLD | point_kind::LOCALIZER => {
                v.layers.airports && range <= v.runway_max_nm
            }
            point_kind::VOR | point_kind::VORTAC | point_kind::VOR_DME => v.layers.vors,
            point_kind::DME | point_kind::TACAN | point_kind::NDB => {
                v.layers.vors && range <= v.minor_navaid_max_nm
            }
            point_kind::AIRPORT => v.layers.airports && (range <= v.non_ifr_airport_max_nm || ifr),
            _ => false,
        };
        if !shown {
            continue;
        }
        if *lat == POS_NONE || *lon == POS_NONE {
            st.points_unplaced += 1;
            continue;
        }
        if let Some(bb) = &v.bbox {
            if !in_bbox(bb, *lat, *lon) {
                continue;
            }
        }
        let Some(c) = project(p, *lat, *lon) else {
            st.points_unplaced += 1;
            continue;
        };
        if !on_screen(p, c) {
            continue;
        }
        // The label's priority and the room it keeps from the symbol; 0 for no label.
        let (prio, gap) = match *kind {
            point_kind::FIX => {
                // Triangle, apex up.
                let top = add(c, 0, -SYM);
                let bl = add(c, -SYM, SYM * 3 / 4);
                let br = add(c, SYM, SYM * 3 / 4);
                push_line(l, top, bl, palette::FIX, LAYER_SYMBOL)?;
                push_line(l, bl, br, palette::FIX, LAYER_SYMBOL)?;
                push_line(l, br, top, palette::FIX, LAYER_SYMBOL)?;
                st.fixes += 1;
                (4, SYM + 4 * 16)
            }
            point_kind::TERMINAL_WAYPOINT => {
                let h = SYM * 3 / 4;
                let top = add(c, 0, -h);
                let bl = add(c, -h, h * 3 / 4);
                let br = add(c, h, h * 3 / 4);
                push_line(l, top, bl, palette::FIX, LAYER_SYMBOL)?;
                push_line(l, bl, br, palette::FIX, LAYER_SYMBOL)?;
                push_line(l, br, top, palette::FIX, LAYER_SYMBOL)?;
                st.terminal_waypoints += 1;
                (5, h + 4 * 16)
            }
            point_kind::THRESHOLD => {
                let h = SYM / 2;
                push_line(
                    l,
                    add(c, -h, 0),
                    add(c, h, 0),
                    palette::DIM_TEXT,
                    LAYER_SYMBOL,
                )?;
                push_line(
                    l,
                    add(c, 0, -h),
                    add(c, 0, h),
                    palette::DIM_TEXT,
                    LAYER_SYMBOL,
                )?;
                st.runway_symbols += 1;
                (0, 0)
            }
            point_kind::LOCALIZER => {
                let h = SYM * 3 / 4;
                let (n, e, s_, w) = (add(c, 0, -h), add(c, h, 0), add(c, 0, h), add(c, -h, 0));
                for (a, b) in [(n, e), (e, s_), (s_, w), (w, n)] {
                    push_line(l, a, b, palette::DIM_TEXT, LAYER_SYMBOL)?;
                }
                st.runway_symbols += 1;
                (0, 0)
            }
            point_kind::AIRPORT => {
                push_circle(l, c, SYM * 5 / 4, palette::AIRPORT, LAYER_SYMBOL)?;
                push_line(l, c, c, palette::AIRPORT, LAYER_SYMBOL)?;
                st.airports += 1;
                (if ifr { 2 } else { 3 }, SYM * 5 / 4 + 4 * 16)
            }
            kind => {
                push_circle(l, c, SYM, palette::FIX, LAYER_SYMBOL)?;
                push_line(l, c, c, palette::FIX, LAYER_SYMBOL)?;
                st.navaids += 1;
                let vor = matches!(
                    kind,
                    point_kind::VOR | point_kind::VORTAC | point_kind::VOR_DME
                );
                (if vor { 1 } else { 4 }, SYM + 4 * 16)
            }
        };
        let wanted = v.layers.labels
            && prio != 0
            && range
                <= if prio <= 2 {
                    v.major_label_max_nm
                } else {
                    v.label_max_nm
                };
        let name = str_or_empty(&m.strings, *ident);
        if wanted && !name.is_empty() {
            if sc.used < lim {
                let width = legend_width16(name.chars().count());
                if set_candidate(sc, (c.0, c.1), gap, width, prio, i as u32).is_none() {
                    return Err(DisplayError::BadView("label scratch write is out of range"));
                }
                sc.used += 1;
            } else {
                sc.overflow += 1;
            }
        }
    }
    labels(l, m, sc, st)
}

/// Writes the next candidate (index `sc.used`); `None` when that is outside any of its buffers.
fn set_candidate(
    sc: &mut Scratch,
    centre: (i32, i32),
    gap: i32,
    width: i32,
    prio: u8,
    row: u32,
) -> Option<()> {
    let k = sc.used;
    *sc.x.get_mut(k)? = centre.0;
    *sc.y.get_mut(k)? = centre.1;
    *sc.gap.get_mut(k)? = gap;
    *sc.w.get_mut(k)? = width;
    *sc.prio.get_mut(k)? = prio;
    *sc.point.get_mut(k)? = row;
    Some(())
}

/// Sorts the candidates (priority, ident, position, row), places them and draws the placed ones.
fn labels(
    l: &mut DisplayList,
    m: &Model,
    sc: &mut Scratch,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let n = sc.used;
    for (i, o) in sc.order.iter_mut().take(n).enumerate() {
        *o = i as u32;
    }
    let key = |s: &Scratch, c: u32| {
        let c = c as usize;
        let row = s.point.get(c).copied().unwrap_or(0) as usize;
        let ident = m
            .points
            .ident
            .get(row)
            .map_or("", |i| str_or_empty(&m.strings, *i));
        (
            s.prio.get(c).copied().unwrap_or(u8::MAX),
            ident,
            m.points.lat.get(row).copied().unwrap_or(0),
            m.points.lon.get(row).copied().unwrap_or(0),
            row,
        )
    };
    // The key ends with the table row, so the order is total and any correct sort gives it.
    let mut order = std::mem::take(&mut sc.order);
    let mut tmp = std::mem::take(&mut sc.order_tmp);
    let sorted = match (order.get_mut(..n), tmp.get_mut(..n)) {
        (Some(data), Some(scratch)) => {
            merge_sort_by(data, scratch, &mut |a, b| key(sc, *a).cmp(&key(sc, *b)))
        }
        _ => Err(DisplayError::BadView(
            "label scratch is shorter than the candidates",
        )),
    };
    sc.order = order;
    sc.order_tmp = tmp;
    sorted?;
    let placed = place_labels(sc);
    st.labels = placed;
    st.labels_dropped = (n as u32 - placed).saturating_add(sc.overflow);
    for oi in 0..sc.used {
        let Some(&c) = sc.order.get(oi) else { break };
        let c = c as usize;
        let Some(&k) = sc.outcome.get(c) else {
            continue;
        };
        if k >= DROPPED_NO_ROOM {
            continue;
        }
        let (Some(&cx), Some(&cy), Some(&gap), Some(&w), Some(&prio), Some(&row)) = (
            sc.x.get(c),
            sc.y.get(c),
            sc.gap.get(c),
            sc.w.get(c),
            sc.prio.get(c),
            sc.point.get(c),
        ) else {
            continue;
        };
        let ((ax, ay), align, _) = label_geom(gap, w, cx, cy, k);
        let name = m
            .points
            .ident
            .get(row as usize)
            .map_or("", |i| str_or_empty(&m.strings, *i));
        let colour = if prio == 2 || prio == 3 {
            palette::AIRPORT
        } else {
            palette::TEXT
        };
        push_text(l, (ax, ay), LABEL_SIZE, align, colour, LAYER_TEXT, name)?;
    }
    Ok(())
}

/// Width of a string of `chars` characters at the legend size, 1/16 pixel.
fn legend_width16(chars: usize) -> i32 {
    let units = i64::from(text_width_units(chars));
    i32::try_from(units * i64::from(LEGEND_SIZE) * 16 / i64::from(CAP_HEIGHT)).unwrap_or(i32::MAX)
}

/// The `HIDDEN:` line: every range threshold or layer switch that is hiding something.
fn hidden_line(v: &View) -> Result<Label, std::fmt::Error> {
    let mut t = label();
    write!(t, "HIDDEN: ")?;
    let mut n = 0;
    let range = v.proj.range_nm;
    let lay = &v.layers;
    let mut item = |t: &mut Label, text: &str, nm: Option<f64>| -> std::fmt::Result {
        if n > 0 {
            write!(t, ", ")?;
        }
        write!(t, "{text}")?;
        if let Some(nm) = nm {
            write!(t, ">")?;
            put_nm(t, nm)?;
            write!(t, "NM")?;
        }
        n += 1;
        Ok(())
    };
    if lay.fixes && range > v.fix_max_nm {
        item(&mut t, "FIXES", Some(v.fix_max_nm))?;
    }
    if lay.fixes && range > v.terminal_wpt_max_nm {
        item(&mut t, "TERMINAL WPTS", Some(v.terminal_wpt_max_nm))?;
    }
    if lay.airports && range > v.runway_max_nm {
        item(&mut t, "RWY/LOC", Some(v.runway_max_nm))?;
    }
    if lay.airports && range > v.non_ifr_airport_max_nm {
        item(&mut t, "NON-IFR APTS", Some(v.non_ifr_airport_max_nm))?;
    }
    if lay.vors && range > v.minor_navaid_max_nm {
        item(&mut t, "NDB/DME/TACAN", Some(v.minor_navaid_max_nm))?;
    }
    if lay.labels && range > v.label_max_nm {
        item(&mut t, "LABELS", Some(v.label_max_nm))?;
    }
    if lay.labels && range > v.major_label_max_nm {
        item(&mut t, "VOR/IFR APT LABELS", Some(v.major_label_max_nm))?;
    }
    for (name, on) in LAYER_NAMES.iter().zip(layers_flags(lay)) {
        if !on {
            let mut up = label();
            for c in name.chars() {
                write!(up, "{}", c.to_ascii_uppercase())?;
            }
            write!(up, " OFF")?;
            item(&mut t, label_str(&up), None)?;
        }
    }
    if n == 0 {
        write!(t, "NONE")?;
    }
    Ok(t)
}

fn legend_format_full(_: std::fmt::Error) -> DisplayError {
    DisplayError::Full {
        table: "legend formatting bytes",
        capacity: LABEL_BYTES,
    }
}

/// One legend text. When it runs past the right edge of the screen it is marked there with a
/// bar in the text colour (the alert colour is for unplaced data) and counted in `legend_cut`, so a narrow screen never cuts it silently.
fn legend_text(
    l: &mut DisplayList,
    st: &mut SceneStats,
    width16: i32,
    at: (i32, i32),
    colour: u8,
    text: &str,
) -> Result<(), DisplayError> {
    push_text(l, at, LEGEND_SIZE, ALIGN_LEFT, colour, LAYER_LEGEND, text)?;
    let end = at.0.saturating_add(legend_width16(text.chars().count()));
    if end > width16 {
        st.legend_cut += 1;
        // A two-pixel bar the height of the text, inside the last pixels of the screen.
        let bx = width16.saturating_sub(16);
        push_line_w(
            l,
            (bx, at.1),
            (bx, at.1.saturating_add(i32::from(LEGEND_SIZE) * 16)),
            palette::TEXT,
            LAYER_LEGEND,
            2,
        )?;
    }
    Ok(())
}

fn legend(
    l: &mut DisplayList,
    m: &Model,
    v: &View,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let p = &v.proj;
    let x = 8 * 16;
    let pitch = 18 * 16;
    let mut y = 8 * 16;
    let strip16 = i32::try_from(LEGEND_STRIP_PX * 16).unwrap_or(i32::MAX);
    let reserved16 = i32::try_from(i64::from(p.top_inset) * 16).unwrap_or(i32::MAX);
    let width16 = i32::try_from(i64::from(p.width) * 16).unwrap_or(i32::MAX);
    // An opaque backdrop the full width of the strip, and a rule under it.
    push_rect(
        l,
        (0, 0),
        (width16, reserved16),
        palette::BACKGROUND,
        LAYER_LEGEND_BG,
    )?;
    push_line(
        l,
        (0, strip16 - 8),
        (width16, strip16 - 8),
        palette::RING,
        LAYER_LEGEND,
    )?;

    let mut t = label();
    write!(
        t,
        "{} NASR {} CIFP {}  NORTH UP  RANGE ",
        m.meta.artcc, m.meta.nasr_effective, m.meta.cifp_cycle
    )
    .map_err(legend_format_full)?;
    put_nm(&mut t, p.range_nm).map_err(legend_format_full)?;
    write!(t, " NM  RINGS ").map_err(legend_format_full)?;
    put_nm(&mut t, ring_step_nm(p)).map_err(legend_format_full)?;
    write!(t, " NM").map_err(legend_format_full)?;
    legend_text(l, st, width16, (x, y), palette::TEXT, label_str(&t))?;
    y += pitch;

    // The boundary key: own facility two pixels, neighbours dimmer.
    let mut kx = x;
    for (name, colour, own) in [
        ("HIGH", palette::ARTCC_HIGH, true),
        ("LOW", palette::ARTCC_LOW, true),
        ("NEIGHBOUR HIGH", palette::NEIGHBOUR_HIGH, false),
        ("NEIGHBOUR LOW", palette::NEIGHBOUR_LOW, false),
    ] {
        let mut k = label();
        if own {
            write!(k, "{} {name}", m.meta.artcc).map_err(legend_format_full)?;
        } else {
            write!(k, "{name}").map_err(legend_format_full)?;
        }
        legend_text(l, st, width16, (kx, y), colour, label_str(&k))?;
        kx = kx
            .saturating_add(legend_width16(k.len))
            .saturating_add(legend_width16(2));
    }
    y += pitch;

    let h = hidden_line(v).map_err(legend_format_full)?;
    legend_text(l, st, width16, (x, y), palette::TEXT, label_str(&h))?;
    y += pitch;

    let mut nx = x;
    if st.legs_at_border > 0 {
        let mut t = label();
        write!(t, "{} LEGS END AT BORDER CROSSINGS", st.legs_at_border)
            .map_err(legend_format_full)?;
        legend_text(l, st, width16, (nx, y), palette::TEXT, label_str(&t))?;
        nx = nx
            .saturating_add(legend_width16(t.len))
            .saturating_add(legend_width16(3));
    }
    // Normal colour: a dropped label is expected on a busy screen, not a fault.
    if st.labels + st.labels_dropped > 0 {
        let mut t = label();
        write!(t, "LABELS DROPPED: {}", st.labels_dropped).map_err(legend_format_full)?;
        legend_text(l, st, width16, (nx, y), palette::TEXT, label_str(&t))?;
    }
    y += pitch;

    // The alert colour is for what needs attention: data the map tried to place and could not.
    if st.points_unplaced + st.legs_unplaced + st.edges_unplaced > 0 {
        let mut t = label();
        write!(
            t,
            "NOT PLACED {} PTS {} LEGS {} EDGES",
            st.points_unplaced, st.legs_unplaced, st.edges_unplaced
        )
        .map_err(legend_format_full)?;
        legend_text(l, st, width16, (x, y), palette::ALERT, label_str(&t))?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod work_budget_tests {
    use super::*;
    use crate::core::list::{list_new, push_line, ListCaps};
    use oa_airspace::model::{empty_model, Meta};

    fn empty_display_model() -> Model {
        empty_model(Meta {
            nasr_effective: 20261118,
            cifp_cycle: 2611,
            artcc: String::from("ZAU"),
            buffer_m: 0,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        })
    }

    fn largest_hidden_view() -> View {
        let mut view = view_new(0, 0, 20_000.0, 1920, 1080).unwrap();
        view.fix_max_nm = 19_999.9;
        view.terminal_wpt_max_nm = 19_999.9;
        view.runway_max_nm = 19_999.9;
        view.non_ifr_airport_max_nm = 19_999.9;
        view.minor_navaid_max_nm = 19_999.9;
        view.label_max_nm = 19_999.9;
        view.major_label_max_nm = 19_999.9;
        view
    }

    // REQ: OA-DSP-025
    #[test]
    fn combined_hidden_range_and_layer_disclosures_are_complete() {
        let mut view = largest_hidden_view();
        for name in [
            "own-boundary",
            "neighbour-boundaries",
            "high-airways",
            "low-airways",
            "rings",
        ] {
            assert!(layer_set(&mut view.layers, name, false));
        }
        let text = hidden_line(&view).unwrap();
        assert_eq!(
            label_str(&text),
            concat!(
                "HIDDEN: FIXES>19999.9NM, TERMINAL WPTS>19999.9NM, RWY/LOC>19999.9NM, ",
                "NON-IFR APTS>19999.9NM, NDB/DME/TACAN>19999.9NM, LABELS>19999.9NM, ",
                "VOR/IFR APT LABELS>19999.9NM, OWN-BOUNDARY OFF, NEIGHBOUR-BOUNDARIES OFF, ",
                "HIGH-AIRWAYS OFF, LOW-AIRWAYS OFF, RINGS OFF"
            )
        );
        assert_eq!(text.len, 254);
    }

    // REQ: OA-DSP-025
    #[test]
    fn every_hidden_layer_combination_fits_the_analyzed_stack_budget() {
        let mut maximum = 0;
        // Nine fixed switches give exactly512 configurations, independent of input size.
        for bits in 0u16..512 {
            let mut view = largest_hidden_view();
            for (bit, name) in LAYER_NAMES.iter().enumerate() {
                assert!(layer_set(&mut view.layers, name, bits & (1 << bit) != 0));
            }
            let text = hidden_line(&view).unwrap();
            maximum = maximum.max(text.len);
            assert!(text.len <= LABEL_BYTES);
        }
        assert_eq!(maximum, 254);
    }

    // REQ: OA-DSP-054
    #[test]
    fn scene_work_refusal_preserves_previous_list_and_retry_builds() {
        let mut model = empty_display_model();
        let view = view_new(0, 0, 50.0, 1920, 1080).unwrap();
        let index = scene_index(&model);
        let mut scratch = scratch_for(&model, &view).unwrap();
        let mut list = list_new(ListCaps {
            lines: 64,
            rects: 8,
            circles: 32,
            polylines: 32,
            poly_vertices: 64,
            texts: 128,
            text_bytes: 4096,
        })
        .unwrap();
        push_line(&mut list, (0, 0), (16, 16), palette::TEXT, LAYER_AIRWAY).unwrap();
        let old_lines = list.lines.x0.clone();

        model.boundaries.stratum.resize(20, stratum::ARTCC_HIGH);
        model
            .boundaries
            .vertices
            .extend((0..20).map(|_| oa_airspace::model::Span {
                start: oa_airspace::model::VertexIx::new(0),
                len: 1_000_000_000,
            }));
        assert!(matches!(
            build_scene(&mut list, &model, &index, &view, &mut scratch),
            Err(DisplayError::TooLarge {
                what: "scene work units",
                ..
            })
        ));
        assert_eq!(list.lines.x0, old_lines);

        model.boundaries.stratum.clear();
        model.boundaries.vertices.clear();
        assert!(build_scene(&mut list, &model, &index, &view, &mut scratch).is_ok());
    }

    // REQ: OA-DSP-054
    #[test]
    fn boundary_scan_charge_includes_zero_length_spans() {
        let mut model = empty_display_model();
        let view = view_new(0, 0, 50.0, 1920, 1080).unwrap();
        let scratch = scratch_for(&model, &view).unwrap();
        let cells = 510;
        let empty_work = scene_work_bound(&model, &scratch, cells, 0);
        model.boundaries.stratum.push(stratum::ARTCC_HIGH);
        let one_boundary = scene_work_bound(&model, &scratch, cells, 0);
        assert_eq!(one_boundary.saturating_sub(empty_work), 2);
    }

    // REQ: OA-DSP-055
    #[test]
    fn scratch_profile_refusal_precedes_allocation_and_smaller_retry_succeeds() {
        let model = empty_display_model();
        let large = view_new(0, 0, 50.0, 8193, 8192).unwrap();
        assert!(matches!(
            scratch_for(&model, &large),
            Err(DisplayError::TooLarge {
                what: "scene pixels",
                ..
            })
        ));
        let accepted = view_new(0, 0, 50.0, 8192, 8192).unwrap();
        assert!(scratch_for(&model, &accepted).is_ok());
    }

    // REQ: OA-DSP-054
    #[test]
    fn scene_work_limit_is_exact_and_one_above_is_refused() {
        assert!(MAX_SCENE_WORK > u128::from(u32::MAX));
        assert!(check_scene_work(MAX_SCENE_WORK).is_ok());
        assert!(matches!(
            check_scene_work(MAX_SCENE_WORK + 1),
            Err(DisplayError::TooLarge {
                what: "scene work units",
                requested,
                limit,
            }) if requested == MAX_SCENE_WORK + 1 && limit == MAX_SCENE_WORK
        ));
    }

    // REQ: OA-DSP-054
    #[test]
    fn each_charged_term_moves_the_bound_by_its_own_cost() {
        let mut model = empty_display_model();
        let view = view_new(0, 0, 50.0, 1920, 1080).unwrap();
        let scratch = scratch_for(&model, &view).unwrap();
        let cells = 510u128;
        let base = scene_work_bound(&model, &scratch, cells, 0);

        // Airway legs: SUBDIV_AIRWAY + 1 per leg.
        model
            .airway_legs
            .from
            .resize(3, oa_airspace::model::PointIx::new(0));
        assert_eq!(
            scene_work_bound(&model, &scratch, cells, 0) - base,
            3 * (u128::from(SUBDIV_AIRWAY) + 1)
        );
        model.airway_legs.from.clear();

        // Boundary vertex visits: SUBDIV_BOUNDARY each.
        assert_eq!(
            scene_work_bound(&model, &scratch, cells, 10) - base,
            10 * u128::from(SUBDIV_BOUNDARY)
        );

        // Grid slots: one unit per cell of the scratch.
        let mut bigger = scratch_for(&model, &view).unwrap();
        bigger.cell_n.push(0);
        assert_eq!(scene_work_bound(&model, &bigger, cells, 0) - base, 1);

        // Point sort: n * ceil(log2 n) comparisons, each the longest identifier plus
        // four key fields, on top of 34 per point and the label-grid inspections.
        let n = 5usize;
        model.points.kind.resize(n, 0);
        model.strings.text = "x".repeat(12);
        model.strings.offsets = vec![0, 12];
        model
            .points
            .ident
            .resize(n, oa_airspace::model::StrIx::new(0));
        let with_points = scene_work_bound(&model, &scratch, cells, 0);
        let sort = n as u128 * 3 * (12 + 4);
        let others = n as u128 * 34 + n as u128 * 5 * cells * CELL_CAP as u128;
        assert_eq!(with_points - base, sort + others);
    }

    // REQ: OA-DSP-054
    #[test]
    fn point_sort_charge_follows_identifier_length_not_the_string_pool() {
        let mut model = empty_display_model();
        let view = view_new(0, 0, 50.0, 1920, 1080).unwrap();
        let scratch = scratch_for(&model, &view).unwrap();
        // A large pool of strings that no point uses must not change the charge.
        model.points.kind.resize(1000, 0);
        model.strings.text = "y".repeat(1 << 20);
        model.strings.offsets = vec![0, 4];
        model
            .points
            .ident
            .resize(1000, oa_airspace::model::StrIx::new(0));
        let small = scene_work_bound(&model, &scratch, 510, 0);
        model.strings.offsets = vec![0, 4, 1 << 20];
        let pool_only = scene_work_bound(&model, &scratch, 510, 0);
        assert_eq!(small, pool_only);
        assert!(small < MAX_SCENE_WORK);
    }

    /// A comparator over `vals` (the key is the value, then the row, so keys are unique) that
    /// counts its calls.
    fn sort_counting(vals: &[u32]) -> (Vec<u32>, u64) {
        let n = vals.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut tmp = vec![0u32; n];
        let mut count = 0u64;
        merge_sort_by(&mut order, &mut tmp, &mut |a, b| {
            count += 1;
            (vals[*a as usize], *a).cmp(&(vals[*b as usize], *b))
        })
        .unwrap();
        (order, count)
    }

    // REQ: OA-DSP-054
    #[test]
    fn merge_sort_refuses_a_scratch_of_the_wrong_length_and_leaves_the_data() {
        for tmp_len in [0usize, 3, 5] {
            let mut data = [3u32, 1, 2, 0];
            let mut tmp = vec![0u32; tmp_len];
            let r = merge_sort_by(&mut data, &mut tmp, &mut |a, b| a.cmp(b));
            assert!(matches!(r, Err(DisplayError::BadView(_))), "{tmp_len}");
            assert_eq!(data, [3, 1, 2, 0]);
        }
    }

    // REQ: OA-DSP-054
    #[test]
    fn merge_sort_matches_std_and_stays_within_n_ceil_log2_n_comparisons() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 33) as u32
        };
        for n in [0usize, 1, 2, 3, 4, 5, 7, 8, 9, 1000, 1024, 4097] {
            let sorted: Vec<u32> = (0..n as u32).collect();
            let reversed: Vec<u32> = (0..n as u32).rev().collect();
            let equal = vec![7u32; n];
            let organ: Vec<u32> = (0..n as u32).map(|i| i.min(n as u32 - 1 - i)).collect();
            let random: Vec<u32> = (0..n).map(|_| rnd() % 50).collect();
            for (name, vals) in [
                ("sorted", sorted),
                ("reversed", reversed),
                ("equal", equal),
                ("organ-pipe", organ),
                ("random", random),
            ] {
                let (got, count) = sort_counting(&vals);
                let mut want: Vec<u32> = (0..n as u32).collect();
                want.sort_unstable_by_key(|r| (vals[*r as usize], *r));
                assert_eq!(got, want, "{name} n={n}");
                let bound = n as u64 * ceil_log2(n as u128) as u64;
                assert!(count <= bound, "{name} n={n}: {count} > {bound}");
                if n <= 1 {
                    assert_eq!(count, 0);
                }
            }
        }
    }
}

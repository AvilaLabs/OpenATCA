//! The static airspace map (**core**, B1, B2): a pure function from the airspace model and a view
//! to a display list. Nothing is read from a clock or a file; the same model and view give the same
//! list.
//!
//! Drawn: ARTCC HIGH and LOW boundaries (distinct colours), every airway leg as a thin line, fixes
//! (triangle), navaids (ring and dot) and airports (large ring), labels for them below a range
//! threshold, a range-ring scale from the centre, a centre mark, and a legend naming the facility,
//! the data cycles, the range and the ring spacing. The legend is on screen so a picture can never
//! be separated from the data it shows.
//!
//! Boundary and airway edges are geodesics, not straight lines on the glass: a long edge is split
//! into short pieces whose end points are interpolated on the sphere, so the drawn curve follows
//! the great circle to well under a pixel. An edge is split to at most 12 NM and 64 pixels a piece,
//! up to 64 pieces for a boundary edge (768 NM) and 16 for an airway leg (192 NM); a longer edge
//! is drawn with longer pieces.
//!
//! Strata other than ARTCC HIGH and LOW (class airspace, special-use airspace, FIR) are in the
//! model but not drawn in this map.

use crate::font::{text_width_units, CAP_HEIGHT};
use crate::list::{
    begin_polyline, list_clear, push_circle, push_line, push_rect, push_text, push_vertex,
    DisplayError, DisplayList, ListCaps, ALIGN_LEFT,
};
use crate::palette as pal;
use crate::project::{project, project_rad, projection_new, ring_radius16, Projection, NM_M};
use oa_airspace::model::{point_kind, str_or_empty, stratum, Model, POS_NONE};
use std::fmt::Write;

/// Layers, drawn first to last.
pub const LAYER_RING: u8 = 0;
pub const LAYER_AIRWAY: u8 = 1;
pub const LAYER_ARTCC_LOW: u8 = 2;
pub const LAYER_ARTCC_HIGH: u8 = 3;
pub const LAYER_SYMBOL: u8 = 4;
pub const LAYER_TEXT: u8 = 5;
pub const LAYER_LEGEND_BG: u8 = 6;
pub const LAYER_LEGEND: u8 = 7;

/// Default label thresholds, nautical miles of range.
pub const DEFAULT_FIX_LABEL_MAX_NM: f64 = 60.0;
pub const DEFAULT_AIRPORT_LABEL_MAX_NM: f64 = 60.0;

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

/// A projection and the label thresholds.
// A7.2: cold struct, size guard waived (one per display).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub proj: Projection,
    /// Fixes and navaids are labelled when the range is at most this many nautical miles.
    pub fix_label_max_nm: f64,
    pub airport_label_max_nm: f64,
}

/// A view with default label thresholds.
pub fn view_new(
    centre_lat_e7: i32,
    centre_lon_e7: i32,
    range_nm: f64,
    width: u32,
    height: u32,
) -> Result<View, DisplayError> {
    Ok(View {
        proj: projection_new(centre_lat_e7, centre_lon_e7, range_nm, width, height)?,
        fix_label_max_nm: DEFAULT_FIX_LABEL_MAX_NM,
        airport_label_max_nm: DEFAULT_AIRPORT_LABEL_MAX_NM,
    })
}

/// What a build drew and what it left out.
// A7.2: cold struct, size guard waived (one per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub boundaries: u32,
    pub airway_legs: u32,
    pub airway_legs_off_screen: u32,
    pub fixes: u32,
    pub navaids: u32,
    pub airports: u32,
    pub labels: u32,
    pub rings: u32,
    /// Fixes, navaids and airports with no position or that cannot be projected.
    pub points_unplaced: u32,
    /// Airway legs with an end that has no position or cannot be projected.
    pub legs_unplaced: u32,
    /// Boundary vertices and edges that cannot be projected, and rings that do not index.
    pub edges_unplaced: u32,
}

/// Capacities that cannot overflow for `m`: counted from the model, with the worst-case edge
/// splitting and the fixed furniture (C2). Reaching a capacity is still an error, never a wrap.
pub fn scene_caps(m: &Model) -> ListCaps {
    let points = m.points.kind.len();
    let boundary_vertices: usize = m
        .boundaries
        .stratum
        .iter()
        .zip(&m.boundaries.vertices)
        .filter(|(s, _)| **s == stratum::ARTCC_HIGH || **s == stratum::ARTCC_LOW)
        .map(|(_, v)| (v.len as usize).saturating_add(1))
        .sum();
    let ident_bytes: usize = m
        .points
        .ident
        .iter()
        .map(|i| str_or_empty(&m.strings, *i).len())
        .sum();
    ListCaps {
        lines: m
            .airway_legs
            .from
            .len()
            .saturating_mul(SUBDIV_AIRWAY as usize)
            .saturating_add(points.saturating_mul(3))
            .saturating_add(FURNITURE),
        rects: 1,
        circles: points.saturating_add(FURNITURE),
        polylines: m.boundaries.stratum.len().saturating_add(FURNITURE),
        poly_vertices: boundary_vertices.saturating_mul(SUBDIV_BOUNDARY as usize),
        texts: points.saturating_add(FURNITURE),
        text_bytes: ident_bytes.saturating_add(FURNITURE * 32),
    }
}

/// A short string built on the stack, so a frame allocates nothing for its labels.
struct Label {
    buf: [u8; 96],
    len: usize,
}

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
        buf: [0; 96],
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
fn put_nm(l: &mut Label, nm: f64) {
    if (nm - nm.round()).abs() < 1e-9 {
        let _ = write!(l, "{}", nm.round() as i64);
    } else {
        let _ = write!(l, "{nm:.1}");
    }
}

const DEG_E7_RAD: f64 = 1e-7 * std::f64::consts::PI / 180.0;

fn unit(lat_e7: i32, lon_e7: i32) -> [f64; 3] {
    let (lat, lon) = (
        f64::from(lat_e7) * DEG_E7_RAD,
        f64::from(lon_e7) * DEG_E7_RAD,
    );
    [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]
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
    let dot = (ua[0] * ub[0] + ua[1] * ub[1] + ua[2] * ub[2]).clamp(-1.0, 1.0);
    let ground = dot.acos() * proj.radius_m;
    let by_px = (chord + PIECE_16 - 1) / PIECE_16;
    let by_ground = (ground / PIECE_M).ceil();
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
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n < 1e-9 {
        // The ends are nearly opposite; the great circle is not defined. Draw straight.
        return Some(if t < 0.5 { e.pa } else { e.pb });
    }
    project_rad(proj, (v[2] / n).clamp(-1.0, 1.0).asin(), v[1].atan2(v[0]))
}

/// True when both points lie beyond the same side of the screen (with a margin).
fn off_same_side(proj: &Projection, a: (i32, i32), b: (i32, i32)) -> bool {
    let w = i64::from(proj.width) * 16;
    let h = i64::from(proj.height) * 16;
    let m = i64::from(MARGIN);
    let (ax, ay, bx, by) = (
        i64::from(a.0),
        i64::from(a.1),
        i64::from(b.0),
        i64::from(b.1),
    );
    (ax < -m && bx < -m)
        || (ax > w + m && bx > w + m)
        || (ay < -m && by < -m)
        || (ay > h + m && by > h + m)
}

fn on_screen(proj: &Projection, p: (i32, i32)) -> bool {
    let w = i64::from(proj.width) * 16;
    let h = i64::from(proj.height) * 16;
    let m = i64::from(MARGIN);
    (-m..=w + m).contains(&i64::from(p.0)) && (-m..=h + m).contains(&i64::from(p.1))
}

fn add(p: (i32, i32), dx: i32, dy: i32) -> (i32, i32) {
    (p.0.saturating_add(dx), p.1.saturating_add(dy))
}

/// The ring spacing, nautical miles: the first of a fixed series at least [`MIN_RING_PX`] wide.
pub fn ring_step_nm(proj: &Projection) -> f64 {
    let px_per_nm = f64::from(proj.width.min(proj.height)) / 2.0 / proj.range_nm;
    RING_STEPS_NM
        .iter()
        .copied()
        .find(|s| s * px_per_nm >= MIN_RING_PX)
        .unwrap_or(RING_STEP_MAX_NM)
}

/// Clears `l` and builds the static map for `m` as seen through `v`. On an error the list is left
/// faulted (it overflowed or was misused) and will not be drawn; show a fault, never the frame.
pub fn build_scene(l: &mut DisplayList, m: &Model, v: &View) -> Result<SceneStats, DisplayError> {
    list_clear(l);
    let mut st = SceneStats::default();
    let p = &v.proj;
    rings(l, p, &mut st)?;
    boundaries(l, m, p, &mut st)?;
    airways(l, m, p, &mut st)?;
    points(l, m, v, &mut st)?;
    legend(l, m, p, &st)?;
    Ok(st)
}

fn rings(l: &mut DisplayList, p: &Projection, st: &mut SceneStats) -> Result<(), DisplayError> {
    let centre = (p.cx16.round() as i32, p.cy16.round() as i32);
    let step = ring_step_nm(p);
    let half_diag16 = f64::from(p.width).hypot(f64::from(p.height)) * 8.0;
    let mut k = 1_i64;
    while k <= MAX_RINGS {
        let nm = step * k as f64;
        let Some(r) = ring_radius16(p, nm * NM_M) else {
            break;
        };
        if f64::from(r) > half_diag16 {
            break;
        }
        push_circle(l, centre, r, pal::RING, LAYER_RING)?;
        st.rings += 1;
        // The label sits on the east axis, above the ring, when that is on screen.
        let lx = centre.0.saturating_add(r).saturating_add(4 * 16);
        if i64::from(lx) < i64::from(p.width) * 16 {
            let mut t = label();
            put_nm(&mut t, nm);
            push_text(
                l,
                (lx, centre.1.saturating_sub(15 * 16)),
                LABEL_SIZE,
                ALIGN_LEFT,
                pal::DIM_TEXT,
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
        pal::DIM_TEXT,
        LAYER_RING,
    )?;
    push_line(
        l,
        add(centre, 0, -arm),
        add(centre, 0, arm),
        pal::DIM_TEXT,
        LAYER_RING,
    )?;
    Ok(())
}

fn boundaries(
    l: &mut DisplayList,
    m: &Model,
    p: &Projection,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let b = &m.boundaries;
    for (s, span) in b.stratum.iter().zip(&b.vertices) {
        let (colour, layer) = match *s {
            stratum::ARTCC_HIGH => (pal::ARTCC_HIGH, LAYER_ARTCC_HIGH),
            stratum::ARTCC_LOW => (pal::ARTCC_LOW, LAYER_ARTCC_LOW),
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
        begin_polyline(l, colour, layer)?;
        st.boundaries += 1;
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
    p: &Projection,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let pts = &m.points;
    let pos = |ix: oa_airspace::model::PointIx| -> Option<(i32, i32)> {
        let lat = *pts.lat.get(ix.row())?;
        let lon = *pts.lon.get(ix.row())?;
        (lat != POS_NONE && lon != POS_NONE).then_some((lat, lon))
    };
    for (f, t) in m.airway_legs.from.iter().zip(&m.airway_legs.to) {
        let (Some(a), Some(b)) = (pos(*f), pos(*t)) else {
            st.legs_unplaced += 1;
            continue;
        };
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
                push_line(l, prev, cur, pal::AIRWAY, LAYER_AIRWAY)?;
                prev = cur;
            }
        }
    }
    Ok(())
}

fn points(
    l: &mut DisplayList,
    m: &Model,
    v: &View,
    st: &mut SceneStats,
) -> Result<(), DisplayError> {
    let p = &v.proj;
    let pts = &m.points;
    let rows = pts.kind.iter().zip(&pts.lat).zip(&pts.lon).zip(&pts.ident);
    for (((kind, lat), lon), ident) in rows {
        let drawn = matches!(
            *kind,
            point_kind::FIX
                | point_kind::VOR
                | point_kind::VORTAC
                | point_kind::VOR_DME
                | point_kind::DME
                | point_kind::TACAN
                | point_kind::NDB
                | point_kind::AIRPORT
        );
        if !drawn {
            continue;
        }
        if *lat == POS_NONE || *lon == POS_NONE {
            st.points_unplaced += 1;
            continue;
        }
        let Some(c) = project(p, *lat, *lon) else {
            st.points_unplaced += 1;
            continue;
        };
        if !on_screen(p, c) {
            continue;
        }
        let label_max = match *kind {
            point_kind::FIX => {
                // Triangle, apex up.
                let top = add(c, 0, -SYM);
                let bl = add(c, -SYM, SYM * 3 / 4);
                let br = add(c, SYM, SYM * 3 / 4);
                push_line(l, top, bl, pal::FIX, LAYER_SYMBOL)?;
                push_line(l, bl, br, pal::FIX, LAYER_SYMBOL)?;
                push_line(l, br, top, pal::FIX, LAYER_SYMBOL)?;
                st.fixes += 1;
                v.fix_label_max_nm
            }
            point_kind::AIRPORT => {
                push_circle(l, c, SYM * 5 / 4, pal::AIRPORT, LAYER_SYMBOL)?;
                push_line(l, c, c, pal::AIRPORT, LAYER_SYMBOL)?;
                st.airports += 1;
                v.airport_label_max_nm
            }
            _ => {
                push_circle(l, c, SYM, pal::FIX, LAYER_SYMBOL)?;
                push_line(l, c, c, pal::FIX, LAYER_SYMBOL)?;
                st.navaids += 1;
                v.fix_label_max_nm
            }
        };
        if p.range_nm <= label_max {
            let name = str_or_empty(&m.strings, *ident);
            if !name.is_empty() {
                let colour = if *kind == point_kind::AIRPORT {
                    pal::AIRPORT
                } else {
                    pal::TEXT
                };
                let dx = if *kind == point_kind::AIRPORT {
                    SYM * 5 / 4 + 4 * 16
                } else {
                    SYM + 4 * 16
                };
                push_text(
                    l,
                    add(c, dx, -6 * 16),
                    LABEL_SIZE,
                    ALIGN_LEFT,
                    colour,
                    LAYER_TEXT,
                    name,
                )?;
                st.labels += 1;
            }
        }
    }
    Ok(())
}

fn legend(
    l: &mut DisplayList,
    m: &Model,
    p: &Projection,
    st: &SceneStats,
) -> Result<(), DisplayError> {
    let mut rows: [(Label, u8); 5] = [
        (label(), pal::TEXT),
        (label(), pal::TEXT),
        (label(), pal::ARTCC_HIGH),
        (label(), pal::ARTCC_LOW),
        (label(), pal::ALERT),
    ];
    let [r0, r1, r2, r3, r4] = &mut rows;
    let _ = write!(
        r0.0,
        "{} NASR {} CIFP {}",
        m.meta.artcc, m.meta.nasr_effective, m.meta.cifp_cycle
    );
    let _ = write!(r1.0, "NORTH UP  RANGE ");
    put_nm(&mut r1.0, p.range_nm);
    let _ = write!(r1.0, " NM  RINGS ");
    put_nm(&mut r1.0, ring_step_nm(p));
    let _ = write!(r1.0, " NM");
    let _ = write!(r2.0, "ARTCC HIGH");
    let _ = write!(r3.0, "ARTCC LOW");
    let unplaced = st.points_unplaced + st.legs_unplaced + st.edges_unplaced;
    if unplaced > 0 {
        // Data the map could not place is said on screen, not dropped silently.
        let _ = write!(
            r4.0,
            "NOT PLACED {} PTS {} LEGS {} EDGES",
            st.points_unplaced, st.legs_unplaced, st.edges_unplaced
        );
    }
    let x = 8 * 16;
    let pitch = 18 * 16;
    let widest = rows.iter().map(|(t, _)| t.len).max().unwrap_or(0);
    let wide16 =
        i64::from(text_width_units(widest)) * i64::from(LEGEND_SIZE) * 16 / i64::from(CAP_HEIGHT);
    let right = i32::try_from(wide16).unwrap_or(0).saturating_add(2 * x);
    // A backdrop, so map lines never cross the text.
    push_rect(
        l,
        (0, 0),
        (right, 8 * 16 + pitch * 5),
        pal::BACKGROUND,
        LAYER_LEGEND_BG,
    )?;
    let mut y = 8 * 16;
    for (t, colour) in &rows {
        if t.len > 0 {
            push_text(
                l,
                (x, y),
                LEGEND_SIZE,
                ALIGN_LEFT,
                *colour,
                LAYER_LEGEND,
                label_str(t),
            )?;
        }
        y += pitch;
    }
    Ok(())
}

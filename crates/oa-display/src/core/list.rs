//! The display list (**core**, B1, A3): a flat, struct-of-arrays list of drawing primitives, built
//! fresh each frame by pure functions and drawn by [`crate::core::raster`].
//!
//! Primitives: line segment, polyline (a span of a shared vertex array), filled rectangle, circle
//! outline and text. Every primitive carries a palette index and a layer. Coordinates are `i32`
//! in 1/16 pixel.
//!
//! Capacity is fixed at creation (C1, C2): [`list_new`] is the only allocation, and every push
//! checks its table against the named capacity in [`ListCaps`]. A push that does not fit returns
//! [`DisplayError::Full`] and marks the list *faulted*; the rasterizer refuses a faulted list, so
//! a partial frame is never drawn by mistake. [`list_clear`] resets the list for the next frame
//! and keeps all capacity.

use crate::core::palette::PALETTE_LEN;
use std::fmt;

/// Most rows any one table of a display list may be given. A table of 2^24 rows of 4-byte
/// values is 64 MB and a full list stays near 1 GB; the real ZAU model asks for a small fraction
/// of this, so a request above it is a malformed or hostile model, not a map.
pub const MAX_LIST_ROWS: usize = 1 << 24;
/// Most bytes the string table of a display list may be given (64 MB).
pub const MAX_LIST_TEXT_BYTES: usize = 1 << 26;
/// Number of layers. Layers draw in ascending order.
pub const MAX_LAYERS: usize = 16;
/// Largest text size, cap height in pixels.
pub const MAX_TEXT_SIZE: u8 = 255;

/// Text alignment about the anchor x: the anchor is the left end, the middle, or the right end.
pub const ALIGN_LEFT: u8 = 0;
pub const ALIGN_CENTRE: u8 = 1;
pub const ALIGN_RIGHT: u8 = 2;

/// Why a display list could not be built.
// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayError {
    /// A table reached its fixed capacity. The frame is incomplete and must not be shown.
    Full {
        table: &'static str,
        capacity: usize,
    },
    BadColour(u8),
    BadLayer(u8),
    BadSize(u8),
    BadRadius(i32),
    /// Line width in pixels must be 1 or 2.
    BadWidth(u8),
    /// A polyline vertex was added with no polyline open.
    NoPolyline,
    /// The view configuration is not usable; the text says which part.
    BadView(&'static str),
    /// A table or buffer derived from the input would be larger than its display limit. Nothing
    /// was allocated.
    TooLarge {
        what: &'static str,
        /// The size asked for (saturated at `u128::MAX`).
        requested: u128,
        limit: usize,
    },
}

impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DisplayError::Full { table, capacity } => write!(
                f,
                "display list full: {table} reached its capacity of {capacity}; the frame is incomplete"
            ),
            DisplayError::BadColour(c) => write!(f, "colour index {c} is not in the palette"),
            DisplayError::BadLayer(l) => write!(f, "layer {l} is not below {MAX_LAYERS}"),
            DisplayError::BadSize(s) => write!(f, "text size {s} is not 1 to {MAX_TEXT_SIZE}"),
            DisplayError::BadRadius(r) => write!(f, "circle radius {r} is negative"),
            DisplayError::BadWidth(w) => write!(f, "line width {w} is not 1 or 2"),
            DisplayError::NoPolyline => write!(f, "polyline vertex added with no polyline open"),
            DisplayError::BadView(what) => write!(f, "bad view: {what}"),
            DisplayError::TooLarge {
                what,
                requested,
                limit,
            } => write!(
                f,
                "{what} would need {requested}, more than the display limit of {limit}"
            ),
        }
    }
}

impl std::error::Error for DisplayError {}

/// Capacity of each table, in rows (bytes for `text_bytes`).
// A7.2: cold struct, size guard waived (one per list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListCaps {
    pub lines: usize,
    pub rects: usize,
    pub circles: usize,
    pub polylines: usize,
    pub poly_vertices: usize,
    pub texts: usize,
    pub text_bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Lines {
    pub x0: Vec<i32>,
    pub y0: Vec<i32>,
    pub x1: Vec<i32>,
    pub y1: Vec<i32>,
    pub colour: Vec<u8>,
    pub layer: Vec<u8>,
    /// Width in pixels, 1 or 2.
    pub width: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Rects {
    pub x0: Vec<i32>,
    pub y0: Vec<i32>,
    pub x1: Vec<i32>,
    pub y1: Vec<i32>,
    pub colour: Vec<u8>,
    pub layer: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Circles {
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub r: Vec<i32>,
    pub colour: Vec<u8>,
    pub layer: Vec<u8>,
}

/// A polyline is `len` consecutive vertices of the list's shared vertex arrays from `start`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Polylines {
    pub start: Vec<u32>,
    pub len: Vec<u32>,
    pub colour: Vec<u8>,
    pub layer: Vec<u8>,
    /// Width in pixels, 1 or 2.
    pub width: Vec<u8>,
}

/// Text: `len` bytes of the list's string table from `start`. `x`, `y` anchor the string: `y` is
/// the top of the capital letters, `x` the left, middle or right (`align`). `size` is the cap
/// height in pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Texts {
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub size: Vec<u8>,
    pub align: Vec<u8>,
    pub start: Vec<u32>,
    pub len: Vec<u32>,
    pub colour: Vec<u8>,
    pub layer: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct DisplayList {
    pub caps: ListCaps,
    pub lines: Lines,
    pub rects: Rects,
    pub circles: Circles,
    pub polylines: Polylines,
    pub poly_x: Vec<i32>,
    pub poly_y: Vec<i32>,
    pub texts: Texts,
    /// The string table all text spans point into.
    pub text: String,
    /// Bit `n` set when layer `n` holds a primitive.
    pub layers_used: u16,
    /// Set by a failed push; cleared by [`list_clear`]. A faulted list is never drawn.
    pub faulted: bool,
}

/// Checks one derived size against its display limit.
pub fn check_limit(
    what: &'static str,
    requested: u128,
    limit: usize,
) -> Result<usize, DisplayError> {
    match usize::try_from(requested) {
        Ok(n) if n <= limit => Ok(n),
        _ => Err(DisplayError::TooLarge {
            what,
            requested,
            limit,
        }),
    }
}

/// An empty list with all its capacity allocated. A capacity above [`MAX_LIST_ROWS`] (or
/// [`MAX_LIST_TEXT_BYTES`] for the string table) is refused before anything is allocated.
pub fn list_new(caps: ListCaps) -> Result<DisplayList, DisplayError> {
    for (what, n) in [
        ("lines", caps.lines),
        ("rects", caps.rects),
        ("circles", caps.circles),
        ("polylines", caps.polylines),
        ("polyline vertices", caps.poly_vertices),
        ("texts", caps.texts),
    ] {
        check_limit(what, n as u128, MAX_LIST_ROWS)?;
    }
    check_limit("text bytes", caps.text_bytes as u128, MAX_LIST_TEXT_BYTES)?;
    Ok(DisplayList {
        caps,
        lines: Lines {
            x0: Vec::with_capacity(caps.lines),
            y0: Vec::with_capacity(caps.lines),
            x1: Vec::with_capacity(caps.lines),
            y1: Vec::with_capacity(caps.lines),
            colour: Vec::with_capacity(caps.lines),
            layer: Vec::with_capacity(caps.lines),
            width: Vec::with_capacity(caps.lines),
        },
        rects: Rects {
            x0: Vec::with_capacity(caps.rects),
            y0: Vec::with_capacity(caps.rects),
            x1: Vec::with_capacity(caps.rects),
            y1: Vec::with_capacity(caps.rects),
            colour: Vec::with_capacity(caps.rects),
            layer: Vec::with_capacity(caps.rects),
        },
        circles: Circles {
            x: Vec::with_capacity(caps.circles),
            y: Vec::with_capacity(caps.circles),
            r: Vec::with_capacity(caps.circles),
            colour: Vec::with_capacity(caps.circles),
            layer: Vec::with_capacity(caps.circles),
        },
        polylines: Polylines {
            start: Vec::with_capacity(caps.polylines),
            len: Vec::with_capacity(caps.polylines),
            colour: Vec::with_capacity(caps.polylines),
            layer: Vec::with_capacity(caps.polylines),
            width: Vec::with_capacity(caps.polylines),
        },
        poly_x: Vec::with_capacity(caps.poly_vertices),
        poly_y: Vec::with_capacity(caps.poly_vertices),
        texts: Texts {
            x: Vec::with_capacity(caps.texts),
            y: Vec::with_capacity(caps.texts),
            size: Vec::with_capacity(caps.texts),
            align: Vec::with_capacity(caps.texts),
            start: Vec::with_capacity(caps.texts),
            len: Vec::with_capacity(caps.texts),
            colour: Vec::with_capacity(caps.texts),
            layer: Vec::with_capacity(caps.texts),
        },
        text: String::with_capacity(caps.text_bytes),
        layers_used: 0,
        faulted: false,
    })
}

/// Empties the list for the next frame. Every capacity is kept; nothing is freed or allocated.
pub fn list_clear(l: &mut DisplayList) {
    l.lines.x0.clear();
    l.lines.y0.clear();
    l.lines.x1.clear();
    l.lines.y1.clear();
    l.lines.colour.clear();
    l.lines.layer.clear();
    l.lines.width.clear();
    l.rects.x0.clear();
    l.rects.y0.clear();
    l.rects.x1.clear();
    l.rects.y1.clear();
    l.rects.colour.clear();
    l.rects.layer.clear();
    l.circles.x.clear();
    l.circles.y.clear();
    l.circles.r.clear();
    l.circles.colour.clear();
    l.circles.layer.clear();
    l.polylines.start.clear();
    l.polylines.len.clear();
    l.polylines.colour.clear();
    l.polylines.layer.clear();
    l.polylines.width.clear();
    l.poly_x.clear();
    l.poly_y.clear();
    l.texts.x.clear();
    l.texts.y.clear();
    l.texts.size.clear();
    l.texts.align.clear();
    l.texts.start.clear();
    l.texts.len.clear();
    l.texts.colour.clear();
    l.texts.layer.clear();
    l.text.clear();
    l.layers_used = 0;
    l.faulted = false;
}

/// Marks the list faulted and passes the error on.
fn fault(l: &mut DisplayList, e: DisplayError) -> Result<(), DisplayError> {
    l.faulted = true;
    Err(e)
}

fn check_style(l: &mut DisplayList, colour: u8, layer: u8) -> Result<(), DisplayError> {
    if usize::from(colour) >= PALETTE_LEN {
        return fault(l, DisplayError::BadColour(colour));
    }
    if usize::from(layer) >= MAX_LAYERS {
        return fault(l, DisplayError::BadLayer(layer));
    }
    Ok(())
}

fn check_width(l: &mut DisplayList, width: u8) -> Result<(), DisplayError> {
    if !(1..=2).contains(&width) {
        return fault(l, DisplayError::BadWidth(width));
    }
    Ok(())
}

fn check_room(
    l: &mut DisplayList,
    table: &'static str,
    used: usize,
    capacity: usize,
) -> Result<(), DisplayError> {
    if used >= capacity {
        return fault(l, DisplayError::Full { table, capacity });
    }
    Ok(())
}

fn mark(l: &mut DisplayList, layer: u8) {
    l.layers_used |= 1 << layer;
}

/// Adds a one pixel line segment.
pub fn push_line(
    l: &mut DisplayList,
    p0: (i32, i32),
    p1: (i32, i32),
    colour: u8,
    layer: u8,
) -> Result<(), DisplayError> {
    push_line_w(l, p0, p1, colour, layer, 1)
}

/// Adds a line segment `width` pixels wide, 1 or 2 (see [`crate::core::raster`] for how 2 is drawn).
pub fn push_line_w(
    l: &mut DisplayList,
    (x0, y0): (i32, i32),
    (x1, y1): (i32, i32),
    colour: u8,
    layer: u8,
    width: u8,
) -> Result<(), DisplayError> {
    check_style(l, colour, layer)?;
    check_width(l, width)?;
    check_room(l, "lines", l.lines.x0.len(), l.caps.lines)?;
    l.lines.x0.push(x0);
    l.lines.y0.push(y0);
    l.lines.x1.push(x1);
    l.lines.y1.push(y1);
    l.lines.colour.push(colour);
    l.lines.layer.push(layer);
    l.lines.width.push(width);
    mark(l, layer);
    Ok(())
}

/// Adds a filled rectangle between two opposite corners, in either order.
pub fn push_rect(
    l: &mut DisplayList,
    (x0, y0): (i32, i32),
    (x1, y1): (i32, i32),
    colour: u8,
    layer: u8,
) -> Result<(), DisplayError> {
    check_style(l, colour, layer)?;
    check_room(l, "rects", l.rects.x0.len(), l.caps.rects)?;
    l.rects.x0.push(x0);
    l.rects.y0.push(y0);
    l.rects.x1.push(x1);
    l.rects.y1.push(y1);
    l.rects.colour.push(colour);
    l.rects.layer.push(layer);
    mark(l, layer);
    Ok(())
}

/// Adds a circle outline of radius `r` (1/16 pixel, not negative) about `(x, y)`.
pub fn push_circle(
    l: &mut DisplayList,
    (x, y): (i32, i32),
    r: i32,
    colour: u8,
    layer: u8,
) -> Result<(), DisplayError> {
    check_style(l, colour, layer)?;
    if r < 0 {
        return fault(l, DisplayError::BadRadius(r));
    }
    check_room(l, "circles", l.circles.x.len(), l.caps.circles)?;
    l.circles.x.push(x);
    l.circles.y.push(y);
    l.circles.r.push(r);
    l.circles.colour.push(colour);
    l.circles.layer.push(layer);
    mark(l, layer);
    Ok(())
}

/// Opens a one pixel polyline; the vertices that follow, from [`push_vertex`], belong to it until
/// the next polyline is opened.
pub fn begin_polyline(l: &mut DisplayList, colour: u8, layer: u8) -> Result<(), DisplayError> {
    begin_polyline_w(l, colour, layer, 1)
}

/// Opens a polyline `width` pixels wide, 1 or 2.
pub fn begin_polyline_w(
    l: &mut DisplayList,
    colour: u8,
    layer: u8,
    width: u8,
) -> Result<(), DisplayError> {
    check_style(l, colour, layer)?;
    check_width(l, width)?;
    check_room(l, "polylines", l.polylines.start.len(), l.caps.polylines)?;
    // The vertex count is bounded by the capacity, which is a usize built from a u32-sized
    // allocation in practice; a list too large for u32 rows is refused as full.
    let start = u32::try_from(l.poly_x.len()).unwrap_or(u32::MAX);
    l.polylines.start.push(start);
    l.polylines.len.push(0);
    l.polylines.colour.push(colour);
    l.polylines.layer.push(layer);
    l.polylines.width.push(width);
    mark(l, layer);
    Ok(())
}

/// Adds a vertex to the open polyline.
pub fn push_vertex(l: &mut DisplayList, x: i32, y: i32) -> Result<(), DisplayError> {
    check_room(l, "polyline vertices", l.poly_x.len(), l.caps.poly_vertices)?;
    let Some(len) = l.polylines.len.last_mut() else {
        return fault(l, DisplayError::NoPolyline);
    };
    *len = len.saturating_add(1);
    l.poly_x.push(x);
    l.poly_y.push(y);
    Ok(())
}

/// Adds text. `size` is the cap height in pixels, 1 to 255. Characters the font lacks are drawn
/// as the missing-glyph box (see [`crate::core::font`]), never skipped.
pub fn push_text(
    l: &mut DisplayList,
    (x, y): (i32, i32),
    size: u8,
    align: u8,
    colour: u8,
    layer: u8,
    s: &str,
) -> Result<(), DisplayError> {
    check_style(l, colour, layer)?;
    if size == 0 {
        return fault(l, DisplayError::BadSize(size));
    }
    check_room(l, "texts", l.texts.x.len(), l.caps.texts)?;
    if l.text.len().saturating_add(s.len()) > l.caps.text_bytes {
        return fault(
            l,
            DisplayError::Full {
                table: "text bytes",
                capacity: l.caps.text_bytes,
            },
        );
    }
    let start = u32::try_from(l.text.len()).unwrap_or(u32::MAX);
    let len = u32::try_from(s.len()).unwrap_or(u32::MAX);
    l.text.push_str(s);
    l.texts.x.push(x);
    l.texts.y.push(y);
    l.texts.size.push(size);
    l.texts.align.push(align);
    l.texts.start.push(start);
    l.texts.len.push(len);
    l.texts.colour.push(colour);
    l.texts.layer.push(layer);
    mark(l, layer);
    Ok(())
}

/// The capacity of every column, in a fixed order. Equal before and after a frame means the
/// frame allocated nothing in the list (C1).
pub fn list_capacities(l: &DisplayList) -> [usize; 34] {
    [
        l.lines.x0.capacity(),
        l.lines.y0.capacity(),
        l.lines.x1.capacity(),
        l.lines.y1.capacity(),
        l.lines.colour.capacity(),
        l.lines.layer.capacity(),
        l.lines.width.capacity(),
        l.rects.x0.capacity(),
        l.rects.y0.capacity(),
        l.rects.x1.capacity(),
        l.rects.y1.capacity(),
        l.rects.colour.capacity(),
        l.rects.layer.capacity(),
        l.circles.x.capacity(),
        l.circles.y.capacity(),
        l.circles.r.capacity(),
        l.circles.colour.capacity(),
        l.circles.layer.capacity(),
        l.polylines.start.capacity(),
        l.polylines.len.capacity(),
        l.polylines.colour.capacity(),
        l.polylines.layer.capacity(),
        l.polylines.width.capacity(),
        l.poly_x.capacity(),
        l.poly_y.capacity(),
        l.texts.x.capacity(),
        l.texts.y.capacity(),
        l.texts.size.capacity(),
        l.texts.align.capacity(),
        l.texts.start.capacity(),
        l.texts.len.capacity(),
        l.texts.colour.capacity(),
        l.texts.layer.capacity(),
        l.text.capacity(),
    ]
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn caps(n: usize) -> ListCaps {
        ListCaps {
            lines: n,
            rects: n,
            circles: n,
            polylines: n,
            poly_vertices: n,
            texts: n,
            text_bytes: 8,
        }
    }

    // REQ: OA-DSP-001
    #[test]
    fn every_primitive_carries_a_colour_and_a_layer() {
        let mut l = list_new(caps(4)).unwrap();
        push_line(&mut l, (0, 1), (2, 3), 4, 5).unwrap();
        push_rect(&mut l, (1, 1), (9, 9), 6, 7).unwrap();
        push_circle(&mut l, (5, 5), 32, 2, 3).unwrap();
        begin_polyline(&mut l, 3, 1).unwrap();
        push_vertex(&mut l, 10, 10).unwrap();
        push_vertex(&mut l, 20, 20).unwrap();
        push_text(&mut l, (7, 8), 12, ALIGN_CENTRE, 1, 9, "AB").unwrap();
        assert_eq!((l.lines.colour[0], l.lines.layer[0]), (4, 5));
        assert_eq!((l.rects.colour[0], l.rects.layer[0]), (6, 7));
        assert_eq!((l.circles.colour[0], l.circles.layer[0]), (2, 3));
        assert_eq!((l.polylines.colour[0], l.polylines.layer[0]), (3, 1));
        assert_eq!((l.polylines.start[0], l.polylines.len[0]), (0, 2));
        assert_eq!(
            (l.texts.colour[0], l.texts.layer[0], l.texts.size[0]),
            (1, 9, 12)
        );
        assert_eq!(
            &l.text[l.texts.start[0] as usize..][..l.texts.len[0] as usize],
            "AB"
        );
        assert_eq!(
            l.layers_used,
            (1 << 5) | (1 << 7) | (1 << 3) | (1 << 1) | (1 << 9)
        );
        assert!(!l.faulted);
    }

    // REQ: OA-DSP-001
    #[test]
    fn colour_layer_size_and_radius_are_checked() {
        let mut l = list_new(caps(4)).unwrap();
        assert_eq!(
            push_line(&mut l, (0, 0), (1, 1), PALETTE_LEN as u8, 0),
            Err(DisplayError::BadColour(PALETTE_LEN as u8))
        );
        assert_eq!(
            push_line(&mut l, (0, 0), (1, 1), 0, MAX_LAYERS as u8),
            Err(DisplayError::BadLayer(MAX_LAYERS as u8))
        );
        assert_eq!(
            push_text(&mut l, (0, 0), 0, 0, 0, 0, "A"),
            Err(DisplayError::BadSize(0))
        );
        assert_eq!(
            push_circle(&mut l, (0, 0), -1, 0, 0),
            Err(DisplayError::BadRadius(-1))
        );
        assert_eq!(push_vertex(&mut l, 1, 1), Err(DisplayError::NoPolyline));
        assert!(l.faulted);
        assert!(l.lines.x0.is_empty() && l.texts.x.is_empty());
    }

    // REQ: OA-DSP-002
    #[test]
    fn overflow_of_every_table_is_an_error_that_says_so() {
        let full = |table: &'static str, capacity: usize| DisplayError::Full { table, capacity };
        let mut l = list_new(caps(1)).unwrap();
        push_line(&mut l, (0, 0), (1, 1), 0, 0).unwrap();
        assert_eq!(
            push_line(&mut l, (0, 0), (1, 1), 0, 0),
            Err(full("lines", 1))
        );
        assert!(l.faulted);
        list_clear(&mut l);
        assert!(!l.faulted);
        push_rect(&mut l, (0, 0), (1, 1), 0, 0).unwrap();
        assert_eq!(
            push_rect(&mut l, (0, 0), (1, 1), 0, 0),
            Err(full("rects", 1))
        );
        list_clear(&mut l);
        push_circle(&mut l, (0, 0), 1, 0, 0).unwrap();
        assert_eq!(
            push_circle(&mut l, (0, 0), 1, 0, 0),
            Err(full("circles", 1))
        );
        list_clear(&mut l);
        begin_polyline(&mut l, 0, 0).unwrap();
        assert_eq!(begin_polyline(&mut l, 0, 0), Err(full("polylines", 1)));
        list_clear(&mut l);
        begin_polyline(&mut l, 0, 0).unwrap();
        push_vertex(&mut l, 0, 0).unwrap();
        assert_eq!(push_vertex(&mut l, 0, 0), Err(full("polyline vertices", 1)));
        list_clear(&mut l);
        push_text(&mut l, (0, 0), 6, 0, 0, 0, "ABCD").unwrap();
        assert_eq!(
            push_text(&mut l, (0, 0), 6, 0, 0, 0, "X"),
            Err(full("texts", 1))
        );
        list_clear(&mut l);
        assert_eq!(
            push_text(&mut l, (0, 0), 6, 0, 0, 0, "TOO LONG TEXT"),
            Err(full("text bytes", 8))
        );
        let msg = DisplayError::Full {
            table: "lines",
            capacity: 1,
        }
        .to_string();
        assert!(msg.contains("lines") && msg.contains("full") && msg.contains("incomplete"));
    }

    // REQ: OA-DSP-003
    #[test]
    fn clearing_keeps_every_capacity() {
        let mut l = list_new(caps(100)).unwrap();
        let before = list_capacities(&l);
        for round in 0..5 {
            list_clear(&mut l);
            for i in 0..100 {
                push_line(&mut l, (i, round), (i, i), 1, 0).unwrap();
                push_text(&mut l, (i, 0), 6, 0, 1, 0, "").unwrap();
            }
            begin_polyline(&mut l, 1, 0).unwrap();
            for i in 0..100 {
                push_vertex(&mut l, i, i).unwrap();
            }
        }
        assert_eq!(list_capacities(&l), before);
    }
}

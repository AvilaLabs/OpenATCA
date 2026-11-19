//! The software rasterizer (**core**, B1): draws a [`DisplayList`] into a caller-owned XRGB8888
//! pixel buffer, the format of a KMS dumb buffer (four bytes per pixel, blue first, the fourth
//! byte unused and written as zero). It allocates nothing, uses no floating point, and gives the
//! same bytes on every machine.
//!
//! Lines are exact integer midpoint (Bresenham) lines: one pixel wide, no anti-aliasing. Reason:
//! every pixel on screen is then a palette colour, never a blend that could read as a different
//! colour, and the result is a pure function of the endpoints. A line is clipped analytically: the
//! pixel at step `i` along the major axis is `n0 + sign * ((2 * |dn| * i + dm) / (2 * dm))`, so the
//! part inside the buffer is found by binary search on that closed form and drawn with the
//! standard incremental error term started at the right remainder. The pixels drawn are exactly
//! the pixels of the unclipped line that fall inside the buffer, however far outside the
//! endpoints lie. A line is drawn the same whichever way its endpoints are given.
//!
//! Bounds (E3): every pixel write goes through [`put`], which checks the position against the
//! frame and the buffer length. The clip makes that check always true for lines; it stays as the
//! last defence. All arithmetic is `i64` on coordinates saturated to 2^30 (1/16 pixel), so
//! nothing overflows.

use crate::core::font::{strokes_of, text_width_units, ADVANCE, CAP_HEIGHT};
use crate::core::list::{DisplayList, ALIGN_CENTRE, ALIGN_RIGHT, MAX_LAYERS};
use crate::core::palette::{bytes_of, BACKGROUND, PALETTE_LEN};
use oa_sha256::{Digest, Sha256};
use std::fmt;

/// Coordinate magnitude limit, 1/16 pixel.
const LIMIT: i64 = 1 << 30;

/// The geometry of a pixel buffer. `stride` is the length of a row in bytes (a KMS "pitch"),
/// at least `width * 4` and a multiple of four.
// A7.2: cold struct, size guard waived (one per buffer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSpec {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

/// Bytes a buffer of this geometry must hold.
pub fn frame_len(spec: &FrameSpec) -> usize {
    (spec.stride as usize).saturating_mul(spec.height as usize)
}

/// Why a frame could not be drawn.
// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// Geometry or buffer unusable; the text says how.
    BadFrame(&'static str),
    /// The list overflowed while it was built and is incomplete.
    FaultedList,
    /// The list is inconsistent: a column length, span, colour or layer is wrong.
    BadList(&'static str),
    Hash,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::BadFrame(w) => write!(f, "bad frame: {w}"),
            RenderError::FaultedList => write!(f, "the display list overflowed; frame refused"),
            RenderError::BadList(w) => write!(f, "inconsistent display list: {w}"),
            RenderError::Hash => write!(f, "frame hash failed"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Checks the geometry against the buffer length.
pub fn check_frame(spec: &FrameSpec, buf_len: usize) -> Result<(), RenderError> {
    if spec.width == 0 || spec.height == 0 {
        return Err(RenderError::BadFrame("width and height must be at least 1"));
    }
    if spec.width > 1 << 16 || spec.height > 1 << 16 {
        return Err(RenderError::BadFrame(
            "width and height must be at most 65536",
        ));
    }
    if !spec.stride.is_multiple_of(4) || u64::from(spec.stride) < u64::from(spec.width) * 4 {
        return Err(RenderError::BadFrame(
            "stride must be a multiple of 4 and at least width * 4",
        ));
    }
    if buf_len < frame_len(spec) {
        return Err(RenderError::BadFrame(
            "buffer is shorter than stride * height",
        ));
    }
    Ok(())
}

/// The geometry and bytes a primitive draws into.
struct Canvas<'a> {
    buf: &'a mut [u8],
    w: i64,
    h: i64,
    stride: i64,
}

/// Writes one pixel; a position outside the frame writes nothing.
fn put(c: &mut Canvas<'_>, bytes: [u8; 4], x: i64, y: i64) {
    if x < 0 || y < 0 || x >= c.w || y >= c.h {
        return;
    }
    let off = y * c.stride + x * 4;
    let Ok(off) = usize::try_from(off) else {
        return;
    };
    if let Some(px) = c.buf.get_mut(off..).and_then(|s| s.first_chunk_mut::<4>()) {
        *px = bytes;
    }
}

fn clamp(v: i32) -> i64 {
    i64::from(v).clamp(-LIMIT, LIMIT)
}

/// The pixel containing a 1/16 pixel coordinate.
fn px(v: i64) -> i64 {
    v >> 4
}

/// Smallest `i` in `lo..=hi` with `pred(i)`, for a predicate false-then-true along the range.
fn first_true(lo: i64, hi: i64, pred: impl Fn(i64) -> bool) -> Option<i64> {
    if lo > hi || !pred(hi) {
        return None;
    }
    let (mut a, mut b) = (lo, hi);
    while a < b {
        let mid = a + (b - a) / 2;
        if pred(mid) {
            b = mid;
        } else {
            a = mid + 1;
        }
    }
    Some(a)
}

/// Draws a line between two points given in 1/16 pixel.
fn draw_line(c: &mut Canvas<'_>, bytes: [u8; 4], p0: (i64, i64), p1: (i64, i64)) {
    let (px0, py0) = (px(p0.0.clamp(-LIMIT, LIMIT)), px(p0.1.clamp(-LIMIT, LIMIT)));
    let (px1, py1) = (px(p1.0.clamp(-LIMIT, LIMIT)), px(p1.1.clamp(-LIMIT, LIMIT)));
    let steep = (py1 - py0).abs() > (px1 - px0).abs();
    // Major axis m (the one that steps by one each pixel) and minor axis n.
    let (m0, n0, m1, n1) = if steep {
        (py0, px0, py1, px1)
    } else {
        (px0, py0, px1, py1)
    };
    // Canonical direction: the same pixels whichever end is given first.
    let (m0, n0, m1, n1) = if m0 <= m1 {
        (m0, n0, m1, n1)
    } else {
        (m1, n1, m0, n0)
    };
    let dm = m1 - m0;
    let dn = n1 - n0;
    let adn = dn.abs();
    let sgn = dn.signum();
    let (m_size, n_size) = if steep { (c.h, c.w) } else { (c.w, c.h) };
    // Steps along the major axis whose major coordinate is inside the frame.
    let lo = (-m0).max(0);
    let hi = dm.min(m_size - 1 - m0);
    if lo > hi {
        return;
    }
    // Offset along the minor axis after `i` steps; not decreasing in `i`.
    let k_at = |i: i64| -> i64 {
        if dm == 0 {
            0
        } else {
            (2 * adn * i + dm) / (2 * dm)
        }
    };
    // The minor coordinate is n0 + sgn * k; keep the steps where it lies in 0..n_size.
    let inside = |n: i64| (0..n_size).contains(&n);
    let (lo, hi) = if sgn == 0 {
        if !inside(n0) {
            return;
        }
        (lo, hi)
    } else if inside(n0) && inside(n1) {
        // The minor coordinate never leaves the range between two ends inside it.
        (lo, hi)
    } else {
        let (k_min, k_max) = if sgn > 0 {
            (-n0, n_size - 1 - n0)
        } else {
            (n0 - (n_size - 1), n0)
        };
        let Some(first) = first_true(lo, hi, |i| k_at(i) >= k_min) else {
            return;
        };
        // Last step with k <= k_max: one before the first step with k > k_max.
        let last = match first_true(first, hi, |i| k_at(i) > k_max) {
            Some(i) => i - 1,
            None => hi,
        };
        if first > last {
            return;
        }
        (first, last)
    };
    let two_dm = 2 * dm;
    let (mut k, mut r) = if dm == 0 {
        (0, 0)
    } else {
        let t = 2 * adn * lo + dm;
        (t / two_dm, t % two_dm)
    };
    let mut i = lo;
    while i <= hi {
        let (m, n) = (m0 + i, n0 + sgn * k);
        if steep {
            put(c, bytes, n, m);
        } else {
            put(c, bytes, m, n);
        }
        r += 2 * adn;
        if r >= two_dm && dm != 0 {
            r -= two_dm;
            k += 1;
        }
        i += 1;
    }
}

/// Draws a line `width` pixels wide, 1 or 2. A two pixel line is the one pixel line and a copy of
/// it moved one pixel along its minor axis (down for a shallow line, right for a steep one), so it
/// is two pixels thick measured along that axis and at least 1.41 pixels thick across the line.
/// Both copies are clipped exactly like any line, so the result is deterministic and exact.
fn draw_line_w(c: &mut Canvas<'_>, bytes: [u8; 4], p0: (i64, i64), p1: (i64, i64), width: u8) {
    draw_line(c, bytes, p0, p1);
    if width >= 2 {
        let shallow = (p1.0 - p0.0).abs() >= (p1.1 - p0.1).abs();
        let (ox, oy) = if shallow { (0, 16) } else { (16, 0) };
        draw_line(c, bytes, (p0.0 + ox, p0.1 + oy), (p1.0 + ox, p1.1 + oy));
    }
}

/// Fills the pixels whose centres lie inside the rectangle between two corners.
fn fill_rect(c: &mut Canvas<'_>, bytes: [u8; 4], x0: i64, y0: i64, x1: i64, y1: i64) {
    let (xa, xb) = (x0.min(x1), x0.max(x1));
    let (ya, yb) = (y0.min(y1), y0.max(y1));
    let left = ((xa + 8) >> 4).max(0);
    let right = ((xb + 8) >> 4).min(c.w);
    let top = ((ya + 8) >> 4).max(0);
    let bottom = ((yb + 8) >> 4).min(c.h);
    if left >= right || top >= bottom {
        return;
    }
    for y in top..bottom {
        let (Ok(a), Ok(b)) = (
            usize::try_from(y * c.stride + left * 4),
            usize::try_from(y * c.stride + right * 4),
        ) else {
            continue;
        };
        if let Some(row) = c.buf.get_mut(a..b) {
            for p in row.as_chunks_mut::<4>().0 {
                *p = bytes;
            }
        }
    }
}

/// Square root rounded to nearest, for `v < 2^62`.
fn round_sqrt(v: i64) -> i64 {
    let s = u64::try_from(v.max(0))
        .unwrap_or(0)
        .saturating_mul(4)
        .isqrt();
    i64::try_from(s.div_ceil(2)).unwrap_or(0)
}

/// Draws a circle outline. Each row of the circle that is on screen contributes its two end
/// pixels and each column its two end pixels, which together leave no gaps; the work is bounded by
/// the frame size, not the radius, so a circle far larger than the screen costs no more.
fn draw_circle(c: &mut Canvas<'_>, bytes: [u8; 4], x16: i64, y16: i64, r16: i64) {
    let cx = px(x16.clamp(-LIMIT, LIMIT));
    let cy = px(y16.clamp(-LIMIT, LIMIT));
    let r = (r16.clamp(0, LIMIT) + 8) >> 4;
    if cx + r < 0 || cx - r >= c.w || cy + r < 0 || cy - r >= c.h {
        return;
    }
    let r2 = r * r;
    for dy in (-r).max(-cy)..=r.min(c.h - 1 - cy) {
        let x = round_sqrt(r2 - dy * dy);
        put(c, bytes, cx + x, cy + dy);
        put(c, bytes, cx - x, cy + dy);
    }
    for dx in (-r).max(-cx)..=r.min(c.w - 1 - cx) {
        let y = round_sqrt(r2 - dx * dx);
        put(c, bytes, cx + dx, cy + y);
        put(c, bytes, cx + dx, cy - y);
    }
}

/// Draws a string. `(x16, y16)` is the anchor, 1/16 pixel; `size` is the cap height in pixels.
fn draw_text(
    c: &mut Canvas<'_>,
    bytes: [u8; 4],
    x16: i64,
    y16: i64,
    size: i64,
    align: u8,
    s: &str,
) {
    // A grid unit is size / CAP_HEIGHT pixels, so u units are u * size * 16 / CAP_HEIGHT in 1/16.
    let unit16 = |u: i64| (u * size * 16) / i64::from(CAP_HEIGHT);
    let width16 = unit16(i64::from(text_width_units(s.chars().count())));
    let left = match align {
        ALIGN_CENTRE => x16.clamp(-LIMIT, LIMIT) - width16 / 2,
        ALIGN_RIGHT => x16.clamp(-LIMIT, LIMIT) - width16,
        _ => x16.clamp(-LIMIT, LIMIT),
    };
    let top = y16.clamp(-LIMIT, LIMIT);
    let mut origin = left;
    for ch in s.chars() {
        for st in strokes_of(ch) {
            let [a, b, cc, d] = *st;
            draw_line(
                c,
                bytes,
                (origin + unit16(i64::from(a)), top + unit16(i64::from(b))),
                (origin + unit16(i64::from(cc)), top + unit16(i64::from(d))),
            );
        }
        origin += unit16(i64::from(ADVANCE));
    }
}

/// Checks a list's internal consistency. The fields are public, so the rasterizer does not trust
/// that [`crate::core::list`]'s pushes built it.
pub fn validate_list(l: &DisplayList) -> Result<(), RenderError> {
    if l.faulted {
        return Err(RenderError::FaultedList);
    }
    let n = l.lines.x0.len();
    if [
        l.lines.y0.len(),
        l.lines.x1.len(),
        l.lines.y1.len(),
        l.lines.colour.len(),
        l.lines.layer.len(),
        l.lines.width.len(),
    ]
    .iter()
    .any(|&k| k != n)
    {
        return Err(RenderError::BadList("line columns differ in length"));
    }
    let n = l.rects.x0.len();
    if [
        l.rects.y0.len(),
        l.rects.x1.len(),
        l.rects.y1.len(),
        l.rects.colour.len(),
        l.rects.layer.len(),
    ]
    .iter()
    .any(|&k| k != n)
    {
        return Err(RenderError::BadList("rect columns differ in length"));
    }
    let n = l.circles.x.len();
    if [
        l.circles.y.len(),
        l.circles.r.len(),
        l.circles.colour.len(),
        l.circles.layer.len(),
    ]
    .iter()
    .any(|&k| k != n)
    {
        return Err(RenderError::BadList("circle columns differ in length"));
    }
    let n = l.polylines.start.len();
    if [
        l.polylines.len.len(),
        l.polylines.colour.len(),
        l.polylines.layer.len(),
        l.polylines.width.len(),
    ]
    .iter()
    .any(|&k| k != n)
        || l.poly_x.len() != l.poly_y.len()
    {
        return Err(RenderError::BadList("polyline columns differ in length"));
    }
    let n = l.texts.x.len();
    if [
        l.texts.y.len(),
        l.texts.size.len(),
        l.texts.align.len(),
        l.texts.start.len(),
        l.texts.len.len(),
        l.texts.colour.len(),
        l.texts.layer.len(),
    ]
    .iter()
    .any(|&k| k != n)
    {
        return Err(RenderError::BadList("text columns differ in length"));
    }
    let styled = |colour: &[u8], layer: &[u8]| {
        colour.iter().all(|&c| usize::from(c) < PALETTE_LEN)
            && layer.iter().all(|&y| usize::from(y) < MAX_LAYERS)
    };
    if !styled(&l.lines.colour, &l.lines.layer)
        || !styled(&l.rects.colour, &l.rects.layer)
        || !styled(&l.circles.colour, &l.circles.layer)
        || !styled(&l.polylines.colour, &l.polylines.layer)
        || !styled(&l.texts.colour, &l.texts.layer)
    {
        return Err(RenderError::BadList(
            "a colour or layer index is out of range",
        ));
    }
    if !l
        .lines
        .width
        .iter()
        .chain(&l.polylines.width)
        .all(|w| (1..=2).contains(w))
    {
        return Err(RenderError::BadList("a line width is not 1 or 2"));
    }
    if l.circles.r.iter().any(|&r| r < 0) {
        return Err(RenderError::BadList("a circle radius is negative"));
    }
    for (s, n) in l.polylines.start.iter().zip(&l.polylines.len) {
        let end = u64::from(*s) + u64::from(*n);
        if end > l.poly_x.len() as u64 {
            return Err(RenderError::BadList(
                "a polyline runs past the vertex array",
            ));
        }
    }
    for (i, (s, n)) in l.texts.start.iter().zip(&l.texts.len).enumerate() {
        let (s, n) = (*s as usize, *n as usize);
        let ok = s.checked_add(n).and_then(|e| l.text.get(s..e)).is_some();
        if !ok {
            return Err(RenderError::BadList(
                "a text span is not a string of the table",
            ));
        }
        if l.texts.size.get(i).copied().unwrap_or(0) == 0 {
            return Err(RenderError::BadList("a text size is zero"));
        }
    }
    Ok(())
}

/// Clears the frame to palette entry 0 and draws the list. An error leaves the buffer untouched:
/// the list and the frame are validated before any pixel is written. The padding at the end of
/// each row (stride beyond width) is never written.
pub fn rasterize(l: &DisplayList, spec: &FrameSpec, buf: &mut [u8]) -> Result<(), RenderError> {
    check_frame(spec, buf.len())?;
    validate_list(l)?;
    let (width, stride, height) = (
        spec.width as usize,
        spec.stride as usize,
        spec.height as usize,
    );
    let bg = bytes_of(BACKGROUND).unwrap_or([0; 4]);
    for row in buf.chunks_exact_mut(stride).take(height) {
        if let Some(visible) = row.get_mut(..width * 4) {
            for p in visible.as_chunks_mut::<4>().0 {
                *p = bg;
            }
        }
    }
    let mut c = Canvas {
        buf,
        w: i64::from(spec.width),
        h: i64::from(spec.height),
        stride: i64::from(spec.stride),
    };
    for layer in 0..MAX_LAYERS as u8 {
        if l.layers_used & (1 << layer) != 0 {
            draw_layer(&mut c, l, layer);
        }
    }
    Ok(())
}

/// Draws one layer: rectangles, circles, polylines, lines, text, each in list order.
fn draw_layer(c: &mut Canvas<'_>, l: &DisplayList, layer: u8) {
    let colour = |i: u8| bytes_of(i).unwrap_or([0; 4]);
    let r = &l.rects;
    for (((((x0, y0), x1), y1), k), ly) in
        r.x0.iter()
            .zip(&r.y0)
            .zip(&r.x1)
            .zip(&r.y1)
            .zip(&r.colour)
            .zip(&r.layer)
    {
        if *ly == layer {
            fill_rect(
                c,
                colour(*k),
                clamp(*x0),
                clamp(*y0),
                clamp(*x1),
                clamp(*y1),
            );
        }
    }
    let ci = &l.circles;
    for ((((x, y), rad), k), ly) in
        ci.x.iter()
            .zip(&ci.y)
            .zip(&ci.r)
            .zip(&ci.colour)
            .zip(&ci.layer)
    {
        if *ly == layer {
            draw_circle(c, colour(*k), i64::from(*x), i64::from(*y), i64::from(*rad));
        }
    }
    let p = &l.polylines;
    for ((((s, n), k), ly), w) in p
        .start
        .iter()
        .zip(&p.len)
        .zip(&p.colour)
        .zip(&p.layer)
        .zip(&p.width)
    {
        if *ly != layer {
            continue;
        }
        let (s, n) = (*s as usize, *n as usize);
        let (Some(xs), Some(ys)) = (l.poly_x.get(s..s + n), l.poly_y.get(s..s + n)) else {
            continue;
        };
        let b = colour(*k);
        let mut prev: Option<(i64, i64)> = None;
        for (x, y) in xs.iter().zip(ys) {
            let cur = (i64::from(*x), i64::from(*y));
            // A one-vertex polyline is a point.
            draw_line_w(c, b, prev.unwrap_or(cur), cur, *w);
            prev = Some(cur);
        }
    }
    let ln = &l.lines;
    for ((((((x0, y0), x1), y1), k), ly), w) in ln
        .x0
        .iter()
        .zip(&ln.y0)
        .zip(&ln.x1)
        .zip(&ln.y1)
        .zip(&ln.colour)
        .zip(&ln.layer)
        .zip(&ln.width)
    {
        if *ly == layer {
            draw_line_w(
                c,
                colour(*k),
                (i64::from(*x0), i64::from(*y0)),
                (i64::from(*x1), i64::from(*y1)),
                *w,
            );
        }
    }
    let t = &l.texts;
    for (i, (((((x, y), size), align), k), ly)) in
        t.x.iter()
            .zip(&t.y)
            .zip(&t.size)
            .zip(&t.align)
            .zip(&t.colour)
            .zip(&t.layer)
            .enumerate()
    {
        if *ly != layer {
            continue;
        }
        let (Some(s), Some(n)) = (t.start.get(i), t.len.get(i)) else {
            continue;
        };
        let (s, n) = (*s as usize, *n as usize);
        if let Some(text) = l.text.get(s..s + n) {
            draw_text(
                c,
                colour(*k),
                i64::from(*x),
                i64::from(*y),
                i64::from(*size),
                *align,
                text,
            );
        }
    }
}

/// SHA-256 of the visible pixels: each row's `width * 4` bytes in order, so the padding at the end
/// of a row and the stride do not matter. The same picture has the same digest in any buffer.
pub fn frame_digest(spec: &FrameSpec, buf: &[u8]) -> Result<Digest, RenderError> {
    check_frame(spec, buf.len())?;
    let (width, stride, height) = (
        spec.width as usize,
        spec.stride as usize,
        spec.height as usize,
    );
    let mut h = Sha256::new();
    for row in buf.chunks_exact(stride).take(height) {
        let visible = row
            .get(..width * 4)
            .ok_or(RenderError::BadFrame("short row"))?;
        h.update(visible).map_err(|_| RenderError::Hash)?;
    }
    Ok(h.finalize())
}

/// Draws the list and returns the SHA-256 of the frame.
pub fn render(l: &DisplayList, spec: &FrameSpec, buf: &mut [u8]) -> Result<Digest, RenderError> {
    rasterize(l, spec, buf)?;
    frame_digest(spec, buf)
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
    use crate::core::list::*;
    use crate::core::palette::{PALETTE, TEXT};
    use oa_sha256::to_hex;

    const GUARD: usize = 64;
    const PAD: u8 = 0xAB;

    /// A frame buffer with padding at the end of every row and a guard after the last row, all
    /// filled with a marker the rasterizer must never overwrite.
    struct Fb {
        spec: FrameSpec,
        buf: Vec<u8>,
    }

    fn fb(w: u32, h: u32, extra_px: u32) -> Fb {
        let spec = FrameSpec {
            width: w,
            height: h,
            stride: (w + extra_px) * 4,
        };
        Fb {
            spec,
            buf: vec![PAD; frame_len(&spec) + GUARD],
        }
    }

    fn big() -> ListCaps {
        ListCaps {
            lines: 64,
            rects: 64,
            circles: 64,
            polylines: 64,
            poly_vertices: 256,
            texts: 64,
            text_bytes: 4096,
        }
    }

    /// Nothing outside the visible pixels changed, and every visible pixel is a palette colour.
    fn assert_intact(f: &Fb) {
        let (w, stride) = (f.spec.width as usize * 4, f.spec.stride as usize);
        for y in 0..f.spec.height as usize {
            assert!(
                f.buf[y * stride + w..(y + 1) * stride]
                    .iter()
                    .all(|&b| b == PAD),
                "row {y} padding was written"
            );
            for x in 0..f.spec.width as usize {
                let o = y * stride + x * 4;
                let px = u32::from_le_bytes(f.buf[o..o + 4].try_into().unwrap());
                assert!(
                    PALETTE.contains(&px),
                    "pixel ({x},{y}) is {px:#x}, not a palette colour"
                );
            }
        }
        assert!(
            f.buf[frame_len(&f.spec)..].iter().all(|&b| b == PAD),
            "the guard was written"
        );
    }

    fn at(f: &Fb, x: i64, y: i64) -> u32 {
        let o = y as usize * f.spec.stride as usize + x as usize * 4;
        u32::from_le_bytes(f.buf[o..o + 4].try_into().unwrap())
    }

    /// The set of pixels not drawn in the background colour.
    fn lit(f: &Fb) -> Vec<(i64, i64)> {
        let mut v = Vec::new();
        for y in 0..i64::from(f.spec.height) {
            for x in 0..i64::from(f.spec.width) {
                if at(f, x, y) != PALETTE[0] {
                    v.push((x, y));
                }
            }
        }
        v
    }

    fn draw(f: &mut Fb, l: &DisplayList) -> Digest {
        render(l, &f.spec, &mut f.buf).unwrap()
    }

    fn xorshift(s: &mut u64) -> u64 {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        *s
    }

    fn range(s: &mut u64, lo: i64, hi: i64) -> i64 {
        lo + (xorshift(s) % (hi - lo + 1) as u64) as i64
    }

    fn line_list(lines: &[(i32, i32, i32, i32)]) -> DisplayList {
        line_list_w(lines, 1)
    }

    fn line_list_w(lines: &[(i32, i32, i32, i32)], width: u8) -> DisplayList {
        let mut l = list_new(ListCaps {
            lines: lines.len(),
            ..big()
        })
        .unwrap();
        for &(x0, y0, x1, y1) in lines {
            push_line_w(&mut l, (x0, y0), (x1, y1), TEXT, 0, width).unwrap();
        }
        l
    }

    // REQ: OA-DSP-009
    #[test]
    fn a_clipped_line_is_exactly_the_part_of_the_whole_line_inside() {
        // Draw each line on a small frame, and on a frame with a 320 pixel margin on every side
        // where nothing is clipped; the small frame must equal the window of the large one.
        const M: i64 = 320;
        let mut s = 0x1234_5678_9ABC_DEF1_u64;
        let (w, h) = (40_i64, 30_i64);
        for round in 0..2500 {
            let mut p = [0_i64; 4];
            for v in &mut p {
                // Mostly outside, sometimes inside; sub-pixel positions.
                *v = range(&mut s, -(M - 2) * 16, (M - 2) * 16 + w * 16);
            }
            if round % 5 == 0 {
                p[0] = range(&mut s, 0, w * 16 - 1);
                p[1] = range(&mut s, 0, h * 16 - 1);
            }
            let width = if round % 4 == 3 { 2 } else { 1 };
            let small_list = line_list_w(
                &[(p[0] as i32, p[1] as i32, p[2] as i32, p[3] as i32)],
                width,
            );
            let mut small = fb(w as u32, h as u32, 3);
            rasterize(&small_list, &small.spec, &mut small.buf).unwrap();
            assert_intact(&small);
            let shift = (M * 16) as i32;
            let big_list = line_list_w(
                &[(
                    p[0] as i32 + shift,
                    p[1] as i32 + shift,
                    p[2] as i32 + shift,
                    p[3] as i32 + shift,
                )],
                width,
            );
            let mut large = fb((w + 2 * M) as u32, (h + 2 * M) as u32, 0);
            rasterize(&big_list, &large.spec, &mut large.buf).unwrap();
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(
                        at(&small, x, y),
                        at(&large, x + M, y + M),
                        "line {p:?} differs at ({x},{y})"
                    );
                }
            }
        }
    }

    // REQ: OA-DSP-009
    #[test]
    fn lines_outside_crossing_every_edge_and_on_the_corners() {
        let (w, h) = (64_i32, 48_i32);
        let px = |v: i32| v * 16;
        let case = |lines: &[(i32, i32, i32, i32)]| {
            let mut f = fb(w as u32, h as u32, 2);
            draw(&mut f, &line_list(lines));
            assert_intact(&f);
            lit(&f)
        };
        // Entirely outside, on every side and diagonal, near and very far.
        for (x0, y0, x1, y1) in [
            (-50, -50, -10, -5),
            (w + 5, 0, w + 90, h),
            (0, -9, w, -9),
            (0, h + 9, w, h + 9),
            (-30, 10, -30, 40),
            (w + 30, 10, w + 30, 40),
            (-400, 100, 100, -400),
            (-5000, -5000, -2000, -4000),
            (i32::MIN / 2, i32::MIN / 2, i32::MIN / 4, i32::MIN / 2),
            (i32::MAX, i32::MAX, i32::MAX - 100, i32::MAX - 7),
        ] {
            let l = if x0.abs() < 100_000
                && y0.abs() < 100_000
                && x1.abs() < 100_000
                && y1.abs() < 100_000
            {
                case(&[(px(x0), px(y0), px(x1), px(y1))])
            } else {
                case(&[(x0, y0, x1, y1)])
            };
            assert!(l.is_empty(), "({x0},{y0})-({x1},{y1}) drew {l:?}");
        }
        // Crossing each edge from inside and from outside to outside: something is drawn.
        for (x0, y0, x1, y1) in [
            (10, 10, -40, 10),
            (10, 10, w + 40, 12),
            (10, 10, 12, -40),
            (10, 10, 12, h + 40),
            (-20, 20, w + 20, 25),
            (30, -20, 32, h + 20),
            (-10, -10, w + 10, h + 10),
            (w + 10, -10, -10, h + 10),
        ] {
            let l = case(&[(px(x0), px(y0), px(x1), px(y1))]);
            assert!(
                l.len() >= 10,
                "({x0},{y0})-({x1},{y1}) drew only {} pixels",
                l.len()
            );
        }
        // On the corners: a line that ends exactly on a corner pixel lights it, and one that
        // passes diagonally through two opposite corners lights both.
        for (cx, cy) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            let dx = if cx == 0 { -1 } else { 1 };
            let dy = if cy == 0 { -1 } else { 1 };
            let far = 70;
            let l = case(&[(
                px(cx + dx * far) + 8,
                px(cy + dy * far) + 8,
                px(cx) + 8,
                px(cy) + 8,
            )]);
            assert!(
                l.contains(&(i64::from(cx), i64::from(cy))),
                "corner ({cx},{cy}) not lit"
            );
            assert!(l
                .iter()
                .all(|&(x, y)| x >= 0 && y >= 0 && x < i64::from(w) && y < i64::from(h)));
        }
        let l = case(&[(px(-1) + 8, px(-1) + 8, px(w) + 8, px(h) + 8)]);
        assert!(l.contains(&(0, 0)));
        // A point (a zero-length line) on each corner, and just outside.
        for (x, y, on) in [
            (0, 0, true),
            (w - 1, h - 1, true),
            (-1, 0, false),
            (w, h - 1, false),
        ] {
            let l = case(&[(px(x) + 3, px(y) + 3, px(x) + 3, px(y) + 3)]);
            assert_eq!(l.len(), usize::from(on), "point ({x},{y})");
        }
    }

    // REQ: OA-DSP-009
    #[test]
    fn a_line_is_the_same_whichever_end_comes_first() {
        let mut s = 99_u64;
        for _ in 0..500 {
            let p: Vec<i32> = (0..4).map(|_| range(&mut s, -3000, 3000) as i32).collect();
            let mut a = fb(50, 40, 0);
            let mut b = fb(50, 40, 0);
            let da = draw(&mut a, &line_list(&[(p[0], p[1], p[2], p[3])]));
            let db = draw(&mut b, &line_list(&[(p[2], p[3], p[0], p[1])]));
            assert_eq!(da, db, "{p:?}");
        }
    }

    // REQ: OA-DSP-009
    #[test]
    fn circles_clip_exactly_and_cost_nothing_for_huge_radii() {
        const M: i64 = 200;
        let mut s = 7_u64;
        let (w, h) = (48_i64, 36_i64);
        for _ in 0..300 {
            let cx = range(&mut s, -150 * 16, (w + 150) * 16) as i32;
            let cy = range(&mut s, -150 * 16, (h + 150) * 16) as i32;
            let r = range(&mut s, 0, 220 * 16) as i32;
            let mk = |dx: i32, dy: i32| {
                let mut l = list_new(big()).unwrap();
                push_circle(&mut l, (cx + dx, cy + dy), r, TEXT, 0).unwrap();
                l
            };
            let mut small = fb(w as u32, h as u32, 1);
            draw(&mut small, &mk(0, 0));
            assert_intact(&small);
            let mut large = fb((w + 2 * M) as u32, (h + 2 * M) as u32, 0);
            let shift = (M * 16) as i32;
            draw(&mut large, &mk(shift, shift));
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(
                        at(&small, x, y),
                        at(&large, x + M, y + M),
                        "circle {cx},{cy},{r}"
                    );
                }
            }
        }
        // Huge radii: an arc crossing the frame, and circles that miss it entirely.
        let mut l = list_new(big()).unwrap();
        push_circle(
            &mut l,
            (24 * 16, 18 * 16 + i32::MAX / 4),
            i32::MAX / 4,
            TEXT,
            0,
        )
        .unwrap();
        push_circle(&mut l, (0, 0), i32::MAX, TEXT, 0).unwrap();
        push_circle(&mut l, (i32::MAX, i32::MAX), 16, TEXT, 0).unwrap();
        push_circle(&mut l, (i32::MIN, i32::MIN), i32::MAX, TEXT, 0).unwrap();
        let mut f = fb(48, 36, 0);
        draw(&mut f, &l);
        assert_intact(&f);
        assert!(!lit(&f).is_empty());
    }

    // REQ: OA-DSP-009
    #[test]
    fn rectangles_fill_pixels_whose_centres_are_inside_and_clip() {
        let mut l = list_new(big()).unwrap();
        push_rect(&mut l, (8 * 16, 3 * 16), (12 * 16, 5 * 16), TEXT, 0).unwrap();
        // Corners given in the other order, and half a pixel in: pixel centres decide.
        push_rect(
            &mut l,
            (20 * 16 + 8, 10 * 16 + 8),
            (15 * 16 + 8, 7 * 16 + 8),
            2,
            0,
        )
        .unwrap();
        // Far outside and covering the whole frame.
        push_rect(&mut l, (i32::MIN, i32::MIN), (-1, i32::MAX), 3, 0).unwrap();
        push_rect(&mut l, (-100, -100), (100, 100), 4, 0).unwrap();
        let mut f = fb(30, 20, 2);
        draw(&mut f, &l);
        assert_intact(&f);
        assert_eq!(at(&f, 8, 3), PALETTE[usize::from(TEXT)]);
        assert_eq!(at(&f, 11, 4), PALETTE[usize::from(TEXT)]);
        assert_ne!(at(&f, 12, 4), PALETTE[usize::from(TEXT)]);
        assert_ne!(at(&f, 8, 5), PALETTE[usize::from(TEXT)]);
        // x from 15.5 to 20.5 covers pixel centres 15.5.. so pixels 16 to 20; y 7.5 to 10.5.
        assert_eq!(at(&f, 16, 8), PALETTE[2]);
        assert_eq!(at(&f, 20, 10), PALETTE[2]);
        assert_ne!(at(&f, 15, 8), PALETTE[2]);
        assert_ne!(at(&f, 21, 8), PALETTE[2]);
        // The small covering rectangle (-100..100 px at 1/16 pixel is -6.25 to 6.25 pixels).
        assert_eq!(at(&f, 0, 0), PALETTE[4]);
        assert_eq!(at(&f, 5, 5), PALETTE[4]);
    }

    // REQ: OA-DSP-011
    #[test]
    fn random_primitives_including_extreme_coordinates_never_write_out_of_bounds_or_panic() {
        let words = [
            "",
            "A",
            "ORD",
            "AaZz09 .,/-+:()*#",
            "\u{2191}\u{2193}",
            "~\u{00e9}\u{1F600}",
            "NO PANIC AT ALL TODAY",
        ];
        let mut digests = Vec::new();
        for run in 0..2 {
            let mut s = 0xDEAD_BEEF_CAFE_F00D_u64;
            let mut all = Vec::new();
            for frame in 0..120 {
                let mut l = list_new(big()).unwrap();
                let coord = |s: &mut u64| -> i32 {
                    match xorshift(s) % 6 {
                        0 => i32::MIN,
                        1 => i32::MAX,
                        2 => range(s, -64, 64 * 16) as i32,
                        3 => range(s, -(1 << 20), 1 << 20) as i32,
                        _ => xorshift(s) as i32,
                    }
                };
                for _ in 0..30 {
                    let k = xorshift(&mut s);
                    let (colour, layer) = ((k % 10) as u8, ((k >> 8) % 16) as u8);
                    match (k >> 16) % 6 {
                        0 => {
                            let (a, b, c, d) =
                                (coord(&mut s), coord(&mut s), coord(&mut s), coord(&mut s));
                            push_line_w(
                                &mut l,
                                (a, b),
                                (c, d),
                                colour,
                                layer,
                                1 + (xorshift(&mut s) % 2) as u8,
                            )
                            .unwrap();
                        }
                        1 => {
                            let (a, b, c, d) =
                                (coord(&mut s), coord(&mut s), coord(&mut s), coord(&mut s));
                            push_rect(&mut l, (a, b), (c, d), colour, layer).unwrap();
                        }
                        2 => {
                            let (a, b) = (coord(&mut s), coord(&mut s));
                            let r = coord(&mut s).saturating_abs();
                            push_circle(&mut l, (a, b), r, colour, layer).unwrap();
                        }
                        3 => {
                            begin_polyline_w(
                                &mut l,
                                colour,
                                layer,
                                1 + (xorshift(&mut s) % 2) as u8,
                            )
                            .unwrap();
                            for _ in 0..(xorshift(&mut s) % 7) {
                                let (a, b) = (coord(&mut s), coord(&mut s));
                                push_vertex(&mut l, a, b).unwrap();
                            }
                        }
                        _ => {
                            let (a, b) = (coord(&mut s), coord(&mut s));
                            let size = (1 + xorshift(&mut s) % 255) as u8;
                            let align = (xorshift(&mut s) % 4) as u8;
                            let w = words[(xorshift(&mut s) % words.len() as u64) as usize];
                            push_text(&mut l, (a, b), size, align, colour, layer, w).unwrap();
                        }
                    }
                }
                let mut f = fb(37 + frame % 11, 23 + frame % 7, frame % 3);
                let d = draw(&mut f, &l);
                assert_intact(&f);
                all.extend_from_slice(&d);
            }
            digests.push(to_hex(&oa_sha256::sha256(&all)));
            let _ = run;
        }
        assert_eq!(digests[0], digests[1], "the sweep is not deterministic");
    }

    // REQ: OA-DSP-012
    #[test]
    fn small_scenes_render_to_pinned_hashes() {
        let h = |l: &DisplayList, w: u32, hh: u32| {
            let mut f = fb(w, hh, 0);
            let d = render(l, &f.spec, &mut f.buf).unwrap();
            assert_intact(&f);
            if let Some(dir) = std::env::var_os("OA_DISPLAY_DUMP") {
                let path = std::path::Path::new(&dir)
                    .join(format!("golden-{w}x{hh}-{}.ppm", to_hex(&d[..4])));
                let mut out = Vec::new();
                crate::ppm::write_ppm(&mut out, &f.spec, &f.buf).unwrap();
                std::fs::write(path, out).unwrap();
            }
            to_hex(&d)
        };
        // A fan of lines in sixteen directions, a rectangle, a circle and a polyline.
        let mut l = list_new(big()).unwrap();
        let (cx, cy) = (32 * 16, 24 * 16);
        for (i, (dx, dy)) in [
            (30, 0),
            (28, 11),
            (21, 21),
            (11, 28),
            (0, 30),
            (-11, 28),
            (-21, 21),
            (-28, 11),
            (-30, 0),
            (-28, -11),
            (-21, -21),
            (-11, -28),
            (0, -22),
            (11, -28),
            (21, -21),
            (28, -11),
        ]
        .into_iter()
        .enumerate()
        {
            push_line(
                &mut l,
                (cx, cy),
                (cx + dx * 16, cy + dy * 16),
                2 + (i % 3) as u8,
                1,
            )
            .unwrap();
        }
        push_rect(&mut l, (4 * 16, 4 * 16), (12 * 16, 9 * 16), 3, 0).unwrap();
        push_circle(&mut l, (cx, cy), 20 * 16, 5, 2).unwrap();
        begin_polyline(&mut l, 6, 2).unwrap();
        for (x, y) in [(2, 44), (10, 30), (20, 40), (30, 44), (50, 46), (62, 30)] {
            push_vertex(&mut l, x * 16 + 5, y * 16 + 9).unwrap();
        }
        assert_eq!(
            h(&l, 64, 48),
            "42328fff6fd1a21230b6dc56e10db03cf7e4b28037c0d06a28b87755e97e445a"
        );
        // Text: every glyph class, three sizes, three alignments.
        let mut t = list_new(big()).unwrap();
        push_text(
            &mut t,
            (4 * 16, 4 * 16),
            12,
            ALIGN_LEFT,
            1,
            0,
            "ABCDEFGHIJKLM",
        )
        .unwrap();
        push_text(
            &mut t,
            (4 * 16, 22 * 16),
            12,
            ALIGN_LEFT,
            1,
            0,
            "NOPQRSTUVWXYZ",
        )
        .unwrap();
        push_text(
            &mut t,
            (4 * 16, 40 * 16),
            12,
            ALIGN_LEFT,
            1,
            0,
            "0123456789",
        )
        .unwrap();
        push_text(
            &mut t,
            (4 * 16, 58 * 16),
            12,
            ALIGN_LEFT,
            5,
            0,
            "O0 I1 S5 B8 Z2",
        )
        .unwrap();
        push_text(
            &mut t,
            (4 * 16, 76 * 16),
            12,
            ALIGN_LEFT,
            6,
            0,
            ".,/-+:()*#\u{2191}\u{2193}~",
        )
        .unwrap();
        push_text(
            &mut t,
            (100 * 16, 100 * 16),
            18,
            ALIGN_CENTRE,
            2,
            0,
            "CENTRE",
        )
        .unwrap();
        push_text(
            &mut t,
            (196 * 16, 128 * 16),
            24,
            ALIGN_RIGHT,
            3,
            0,
            "RIGHT 41",
        )
        .unwrap();
        assert_eq!(
            h(&t, 200, 160),
            "bb3de42bf3290d28e58605a3484f7d92aa603e732f0a60bb21ad902689fd74d0"
        );
        // Layers: a later layer covers an earlier one whatever the push order; clipped shapes.
        let mut c = list_new(big()).unwrap();
        push_rect(&mut c, (8 * 16, 8 * 16), (40 * 16, 40 * 16), 2, 3).unwrap();
        push_rect(&mut c, (0, 0), (30 * 16, 30 * 16), 3, 1).unwrap();
        push_line(&mut c, (-20 * 16, -20 * 16), (70 * 16, 60 * 16), 1, 2).unwrap();
        push_circle(&mut c, (50 * 16, 10 * 16), 25 * 16, 5, 4).unwrap();
        push_text(&mut c, (-8 * 16, 20 * 16), 12, ALIGN_LEFT, 1, 5, "CLIPPED").unwrap();
        assert_eq!(
            h(&c, 64, 48),
            "e59d8040562bfe137fe7220b99c455be7f1390f3c7d624ee3b0c0c78cad96ab0"
        );
    }

    // REQ: OA-DSP-010
    #[test]
    fn the_picture_depends_only_on_the_list_not_on_what_the_buffer_held() {
        let mut l = list_new(big()).unwrap();
        push_text(&mut l, (16, 16), 12, ALIGN_LEFT, 1, 2, "DETERMINISM 0123").unwrap();
        push_line(&mut l, (0, 0), (60 * 16, 30 * 16), 3, 1).unwrap();
        push_circle(&mut l, (30 * 16, 15 * 16), 12 * 16, 4, 0).unwrap();
        let mut want: Option<Digest> = None;
        let mut s = 5_u64;
        for fill in [0x00_u8, 0xFF, 0x5A, 0xAB] {
            for stride_extra in [0, 5] {
                let mut f = fb(64, 40, stride_extra);
                f.buf.iter_mut().enumerate().for_each(|(i, b)| {
                    *b = if fill == 0xAB {
                        xorshift(&mut s) as u8
                    } else {
                        fill ^ (i as u8 & 1)
                    }
                });
                let d = rasterize(&l, &f.spec, &mut f.buf)
                    .and_then(|()| frame_digest(&f.spec, &f.buf))
                    .unwrap();
                assert_eq!(
                    *want.get_or_insert(d),
                    d,
                    "fill {fill:#x}, extra {stride_extra}"
                );
            }
        }
    }

    // REQ: OA-DSP-024
    #[test]
    fn a_two_pixel_line_is_the_line_and_its_copy_one_pixel_along_the_minor_axis() {
        let mut s = 31_u64;
        for _ in 0..400 {
            let p: Vec<i32> = (0..4).map(|_| range(&mut s, 0, 60 * 16) as i32).collect();
            let mut one = fb(64, 64, 0);
            let mut two = fb(64, 64, 0);
            draw(&mut one, &line_list(&[(p[0], p[1], p[2], p[3])]));
            draw(&mut two, &line_list_w(&[(p[0], p[1], p[2], p[3])], 2));
            assert_intact(&two);
            let shallow = (p[2] - p[0]).abs() >= (p[3] - p[1]).abs();
            let (ox, oy) = if shallow { (0, 1) } else { (1, 0) };
            let base = lit(&one);
            // Exactly the base pixels and the base pixels moved by one, inside the frame.
            let mut want: Vec<(i64, i64)> = base
                .iter()
                .flat_map(|&(x, y)| [(x, y), (x + ox, y + oy)])
                .filter(|&(x, y)| x < 64 && y < 64)
                .collect();
            want.sort_unstable();
            want.dedup();
            let mut got = lit(&two);
            got.sort_unstable();
            assert_eq!(got, want, "{p:?}");
        }
        // Axis-aligned lines are exactly two pixels thick.
        let mut f = fb(40, 40, 0);
        draw(
            &mut f,
            &line_list_w(&[(5 * 16, 10 * 16, 25 * 16, 10 * 16)], 2),
        );
        assert_eq!(lit(&f).len(), 2 * 21);
        let mut f = fb(40, 40, 0);
        draw(
            &mut f,
            &line_list_w(&[(10 * 16, 5 * 16, 10 * 16, 25 * 16)], 2),
        );
        assert_eq!(lit(&f).len(), 2 * 21);
    }

    // REQ: OA-DSP-024
    #[test]
    fn two_pixel_polylines_and_goldens_and_refused_widths() {
        let mut l = list_new(big()).unwrap();
        begin_polyline_w(&mut l, 2, 1, 2).unwrap();
        for (x, y) in [(3, 3), (30, 8), (50, 40), (10, 44), (3, 3)] {
            push_vertex(&mut l, x * 16 + 3, y * 16 + 11).unwrap();
        }
        for (i, (dx, dy)) in [
            (28, 0),
            (20, 20),
            (0, 28),
            (-20, 20),
            (-28, 0),
            (-20, -20),
            (0, -28),
            (20, -20),
            (28, 9),
            (9, 28),
            (-9, 28),
            (-28, 9),
        ]
        .into_iter()
        .enumerate()
        {
            push_line_w(
                &mut l,
                (32 * 16, 24 * 16),
                ((32 + dx) * 16 + 7, (24 + dy) * 16 + 5),
                3 + (i % 2) as u8,
                2,
                2,
            )
            .unwrap();
        }
        let mut f = fb(64, 48, 0);
        let d = draw(&mut f, &l);
        assert_intact(&f);
        assert_eq!(
            to_hex(&d),
            "0af409950f76dc896b0fd5a15e2d753af1df8348b7aae218b5b3fb292d481e9e"
        );
        // A width other than 1 or 2 is refused when pushed and when found in a list.
        let mut bad = list_new(big()).unwrap();
        assert_eq!(
            push_line_w(&mut bad, (0, 0), (1, 1), 1, 0, 3),
            Err(DisplayError::BadWidth(3))
        );
        assert_eq!(
            push_line_w(&mut bad, (0, 0), (1, 1), 1, 0, 0),
            Err(DisplayError::BadWidth(0))
        );
        let mut bad = list_new(big()).unwrap();
        begin_polyline_w(&mut bad, 1, 0, 2).unwrap();
        bad.polylines.width[0] = 9;
        assert!(matches!(
            render(&bad, &f.spec, &mut f.buf),
            Err(RenderError::BadList(_))
        ));
    }

    // REQ: OA-DSP-012
    #[test]
    fn layers_draw_in_ascending_order_whatever_the_push_order() {
        let mut l = list_new(big()).unwrap();
        push_rect(&mut l, (0, 0), (10 * 16, 10 * 16), 2, 5).unwrap();
        push_rect(&mut l, (0, 0), (10 * 16, 10 * 16), 3, 1).unwrap();
        let mut f = fb(12, 12, 0);
        draw(&mut f, &l);
        assert_eq!(at(&f, 5, 5), PALETTE[2]);
    }

    // REQ: OA-DSP-013
    #[test]
    fn the_frame_hash_ignores_stride_and_changes_with_any_pixel() {
        let mut l = list_new(big()).unwrap();
        push_text(&mut l, (16, 16), 12, ALIGN_LEFT, 1, 0, "HASH").unwrap();
        push_circle(&mut l, (20 * 16, 20 * 16), 8 * 16, 4, 0).unwrap();
        let mut a = fb(48, 32, 0);
        let mut b = fb(48, 32, 9);
        let da = draw(&mut a, &l);
        let db = draw(&mut b, &l);
        assert_eq!(da, db);
        assert_eq!(frame_digest(&a.spec, &a.buf).unwrap(), da);
        // The digest is over exactly the visible bytes.
        let mut raw = Vec::new();
        for row in a.buf.chunks_exact(a.spec.stride as usize).take(32) {
            raw.extend_from_slice(&row[..48 * 4]);
        }
        assert_eq!(oa_sha256::sha256(&raw), da);
        // One changed pixel, one changed byte of the unused fourth byte: both change it.
        a.buf[5 * 4 * 48 + 7 * 4 + 1] ^= 1;
        assert_ne!(frame_digest(&a.spec, &a.buf).unwrap(), da);
    }

    // REQ: OA-DSP-013
    #[test]
    fn unusable_frames_are_refused() {
        let l = list_new(big()).unwrap();
        let ok = FrameSpec {
            width: 8,
            height: 8,
            stride: 32,
        };
        let mut buf = vec![0u8; 256];
        assert!(rasterize(&l, &ok, &mut buf).is_ok());
        for spec in [
            FrameSpec { width: 0, ..ok },
            FrameSpec { height: 0, ..ok },
            FrameSpec { stride: 28, ..ok },
            FrameSpec { stride: 34, ..ok },
            FrameSpec {
                width: 100_000,
                stride: 400_000,
                ..ok
            },
        ] {
            assert!(
                matches!(
                    rasterize(&l, &spec, &mut buf),
                    Err(RenderError::BadFrame(_))
                ),
                "{spec:?}"
            );
        }
        assert!(matches!(
            rasterize(&l, &ok, &mut buf[..255]),
            Err(RenderError::BadFrame(_))
        ));
    }

    // REQ: OA-DSP-014
    #[test]
    fn a_faulted_or_inconsistent_list_is_refused_and_the_buffer_left_alone() {
        let mut f = fb(16, 16, 0);
        let mut l = list_new(ListCaps { lines: 1, ..big() }).unwrap();
        push_line(&mut l, (0, 0), (100, 100), 1, 0).unwrap();
        assert!(push_line(&mut l, (0, 0), (100, 100), 1, 0).is_err());
        assert_eq!(
            render(&l, &f.spec, &mut f.buf),
            Err(RenderError::FaultedList)
        );
        assert!(
            f.buf.iter().all(|&b| b == PAD),
            "a refused frame wrote pixels"
        );
        // Public columns edited by hand are validated, not trusted.
        let mut bad = list_new(big()).unwrap();
        push_line(&mut bad, (0, 0), (1, 1), 1, 0).unwrap();
        bad.lines.colour[0] = 200;
        assert!(matches!(
            render(&bad, &f.spec, &mut f.buf),
            Err(RenderError::BadList(_))
        ));
        let mut bad = list_new(big()).unwrap();
        push_line(&mut bad, (0, 0), (1, 1), 1, 0).unwrap();
        bad.lines.x1.pop();
        assert!(matches!(
            render(&bad, &f.spec, &mut f.buf),
            Err(RenderError::BadList(_))
        ));
        let mut bad = list_new(big()).unwrap();
        begin_polyline(&mut bad, 1, 0).unwrap();
        bad.polylines.len[0] = 5;
        assert!(matches!(
            render(&bad, &f.spec, &mut f.buf),
            Err(RenderError::BadList(_))
        ));
        let mut bad = list_new(big()).unwrap();
        push_text(&mut bad, (0, 0), 12, 0, 1, 0, "\u{2191}").unwrap();
        bad.texts.len[0] = 1;
        assert!(matches!(
            render(&bad, &f.spec, &mut f.buf),
            Err(RenderError::BadList(_))
        ));
        let mut bad = list_new(big()).unwrap();
        push_circle(&mut bad, (0, 0), 5, 1, 0).unwrap();
        bad.circles.r[0] = -1;
        assert!(matches!(
            render(&bad, &f.spec, &mut f.buf),
            Err(RenderError::BadList(_))
        ));
        assert!(f.buf.iter().all(|&b| b == PAD));
    }

    // REQ: OA-DSP-015
    #[test]
    fn text_is_anchored_by_its_alignment_and_unknown_characters_show_a_box() {
        let extent = |align: u8, s: &str| {
            let mut l = list_new(big()).unwrap();
            push_text(&mut l, (100 * 16, 20 * 16), 12, align, 1, 0, s).unwrap();
            let mut f = fb(200, 40, 0);
            draw(&mut f, &l);
            let px = lit(&f);
            let xs = (
                px.iter().map(|p| p.0).min().unwrap(),
                px.iter().map(|p| p.0).max().unwrap(),
            );
            let ys = (
                px.iter().map(|p| p.1).min().unwrap(),
                px.iter().map(|p| p.1).max().unwrap(),
            );
            (xs, ys)
        };
        let (left, ys) = extent(ALIGN_LEFT, "AB");
        let (centre, _) = extent(ALIGN_CENTRE, "AB");
        let (right, _) = extent(ALIGN_RIGHT, "AB");
        assert_eq!(left.0, 100);
        // "AB" is 10 units wide at 2 pixels a unit: 20 pixels, and 12 pixels tall from the anchor.
        assert_eq!(left.1 - left.0, 20);
        assert_eq!(centre.0, 90);
        assert_eq!(right.1, 100);
        assert_eq!((ys.0, ys.1), (20, 32));
        // An unknown character is drawn as the crossed box, not skipped.
        let (xs, _) = extent(ALIGN_LEFT, "~");
        assert_eq!(xs.1 - xs.0, 8);
    }
}

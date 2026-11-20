//! Allocation probe for the tests of the display list (C1). Shell code: the capacity of a `Vec`
//! is whatever the allocator returned, so core code does not read it (RULESET B8).

/// The capacity of every column, in a fixed order. Equal before and after a frame means the
/// frame allocated nothing in the list (C1).
pub fn list_capacities(l: &crate::list::DisplayList) -> [usize; 34] {
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

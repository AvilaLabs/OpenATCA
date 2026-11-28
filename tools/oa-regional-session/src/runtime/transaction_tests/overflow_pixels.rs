use super::*;

// oa-display raster pixels are XRGB8888; FrameSpec.stride supplies the native byte pitch.
const XRGB8888_BYTES_PER_PIXEL: usize = 4;

pub(super) fn assert_all_text_disjoint(runtime: &RegionalRuntime) {
    let rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("actual runtime text rows are valid");
    for (left_index, left) in rows.iter().enumerate() {
        for right in rows.iter().skip(left_index + 1) {
            let disjoint = left.bounds.x1 <= right.bounds.x0
                || right.bounds.x1 <= left.bounds.x0
                || left.bounds.y1 <= right.bounds.y0
                || right.bounds.y1 <= left.bounds.y0;
            assert!(
                disjoint,
                "overlapping rendered rows: {:?} {:?} and {:?} {:?}",
                left.text, left.bounds, right.text, right.bounds
            );
        }
    }
}

pub(super) fn assert_notice_isolated(
    runtime: &RegionalRuntime,
    expected: &str,
    row_kind: oa_display::status_strip::StatusStripRow,
) {
    assert!(!expected.is_empty(), "expected status text is nonempty");
    let rows = surveillance::surveillance_text_rows(&runtime.surveillance)
        .expect("actual runtime text rows are valid");
    assert_eq!(
        rows.iter().filter(|row| row.text == expected).count(),
        1,
        "expected notice occurs once: {expected:?}"
    );

    let plan =
        oa_display::status_strip::regional_status_plan().expect("shared status layout is valid");
    let placement = plan.row(row_kind).expect("requested status row exists");
    let reserved = placement.reserved_bounds();
    let actual_text = placement
        .bounds_for(expected)
        .expect("expected notice fits the reserved row");
    let spec = regional_frame_spec(runtime).expect("completed frame geometry is available");
    let actual = regional_pixels(runtime).expect("completed frame pixels are available");
    let width = i32::try_from(spec.width).expect("frame width is representable");
    let height = i32::try_from(spec.height).expect("frame height is representable");
    let x0 = reserved.x0;
    let x1 = actual_text.x1;
    assert!(
        x0 >= 0 && x0 < x1 && x1 <= width && reserved.y0 >= 0 && reserved.y0 < reserved.y1,
        "crop bounds are valid: reserved={reserved:?}, text={actual_text:?}, frame={spec:?}"
    );
    assert!(
        reserved.y1 <= height,
        "reserved row is inside the frame: {reserved:?}"
    );

    let mut isolated = oa_display::list::list_new(oa_display::list::ListCaps {
        lines: 0,
        rects: 0,
        circles: 0,
        polylines: 0,
        poly_vertices: 0,
        texts: 1,
        text_bytes: expected.len(),
    })
    .expect("one-text display list fits");
    oa_display::list::push_text(
        &mut isolated,
        (placement.x_px() * 16, placement.y_px() * 16),
        placement.size(),
        oa_display::core::list::ALIGN_LEFT,
        placement.colour(),
        placement.layer(),
        expected,
    )
    .expect("expected text fits the isolated display list");
    let mut alone = vec![0; oa_display::raster::frame_len(&spec)];
    oa_display::raster::rasterize(&isolated, &spec, &mut alone)
        .expect("isolated expected notice rasterizes");
    let background = oa_display::palette::bytes_of(oa_display::palette::BACKGROUND)
        .expect("background palette entry exists");

    let stride = usize::try_from(spec.stride).expect("native row stride is representable");
    let x0 = usize::try_from(x0).expect("crop begins inside the frame");
    let x1 = usize::try_from(x1).expect("crop ends inside the frame");
    let y0 = usize::try_from(reserved.y0).expect("crop begins inside the frame");
    let y1 = usize::try_from(reserved.y1).expect("crop ends inside the frame");
    let mut actual_glyph_pixels = 0usize;
    let mut isolated_glyph_pixels = 0usize;
    for y in y0..y1 {
        let start = y
            .checked_mul(stride)
            .and_then(|offset| offset.checked_add(x0.checked_mul(XRGB8888_BYTES_PER_PIXEL)?))
            .expect("row crop offset is representable");
        let end = y
            .checked_mul(stride)
            .and_then(|offset| offset.checked_add(x1.checked_mul(XRGB8888_BYTES_PER_PIXEL)?))
            .expect("row crop end is representable");
        let actual_row = actual.get(start..end).expect("actual crop is in bounds");
        let isolated_row = alone.get(start..end).expect("isolated crop is in bounds");
        assert_eq!(
            actual_row, isolated_row,
            "runtime crop differs from isolated {expected:?} at y={y}"
        );
        actual_glyph_pixels += actual_row
            .chunks_exact(XRGB8888_BYTES_PER_PIXEL)
            .filter(|pixel| *pixel != background.as_slice())
            .count();
        isolated_glyph_pixels += isolated_row
            .chunks_exact(XRGB8888_BYTES_PER_PIXEL)
            .filter(|pixel| *pixel != background.as_slice())
            .count();
    }
    assert!(actual_glyph_pixels > 0, "runtime notice glyphs are visible");
    assert!(
        isolated_glyph_pixels > 0,
        "isolated notice glyphs are visible"
    );
}

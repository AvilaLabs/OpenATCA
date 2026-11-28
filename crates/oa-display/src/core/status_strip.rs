//! Shared renderer-owned plan for the regional top status strip (**core**, B1/B2).
//!
//! Rows are stable across frames, including when a family or operator console is empty. Every
//! row uses the stroke renderer's padded text bounds; the strip inset is the bottom of the final
//! reserved row rather than a separately copied baseline.

use crate::core::font::{text_block_bounds_px, TextBounds};
use crate::core::interaction::preflight_text_room;
use crate::core::list::{push_text, DisplayError, DisplayList, ALIGN_LEFT};
use crate::core::palette;
use crate::core::text::{text_clipped, text_new, text_push_str, text_str, TextBuf};

const STRIP_FIRST_ANCHOR_PX: i32 = crate::core::scene::LEGEND_STRIP_PX as i32 + 2;
const STRIP_X_PX: i32 = 8;
const STRIP_LAYER: u8 = 15;
const MODE_ROW_SIZE: u8 = 7;
const ALERT_ROW_SIZE: u8 = 10;
const SAFETY_ROW_SIZE: u8 = 8;
const STRIP_ROW_PITCH_PX: u32 = 14;
const MODE_ROW_MAX_CHARS: usize = 64;
const CONSOLE_ROW_CHARS: usize = 64;
const MAX_CONSOLE_BYTES: usize = CONSOLE_ROW_CHARS * 2;
const HEADING_ROW_MAX_CHARS: usize = crate::core::text::TEXT_BYTES;
const SAFETY_ROW_MAX_CHARS: usize = crate::core::text::TEXT_BYTES;
const OVERFLOW_ROW_MAX_CHARS: usize = 64;

/// One stable row in the composed regional status band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusStripRow {
    Mode,
    MsawHeading,
    FinalHeading,
    Safety(usize),
    MsawOverflow,
    FinalOverflow,
    Console(usize),
}

/// Pixel placement and draw style for one status row. Coordinates use whole canvas pixels.
// D5: prepared renderer geometry; methods query the privately constructed row reservation.
// A7.2: cold struct, size guard waived (one of nine fixed renderer configuration rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusStripPlacement {
    x_px: i32,
    y_px: i32,
    size: u8,
    pitch_px: u32,
    align: u8,
    colour: u8,
    layer: u8,
    bounds: TextBounds,
    max_chars: usize,
}

impl StatusStripPlacement {
    pub const fn x_px(&self) -> i32 {
        self.x_px
    }

    pub const fn y_px(&self) -> i32 {
        self.y_px
    }

    pub const fn size(&self) -> u8 {
        self.size
    }

    pub const fn pitch_px(&self) -> u32 {
        self.pitch_px
    }

    pub const fn align(&self) -> u8 {
        self.align
    }

    pub const fn colour(&self) -> u8 {
        self.colour
    }

    pub const fn layer(&self) -> u8 {
        self.layer
    }

    pub const fn reserved_bounds(&self) -> TextBounds {
        self.bounds
    }

    /// Conservative bounds for the actual row text, using the renderer's font and padding.
    pub fn bounds_for(&self, text: &str) -> Result<TextBounds, DisplayError> {
        if text.len() > self.max_chars || !text.is_ascii() {
            return Err(DisplayError::BadView(
                "status row text exceeds its renderer reservation",
            ));
        }
        Ok(text_block_bounds_px(
            (self.x_px * 16, self.y_px * 16),
            self.size,
            self.align,
            text.chars().count(),
            1,
            self.pitch_px,
        ))
    }
}

/// Fixed row geometry for a complete regional status band.
// D5: renderer layout subsystem; methods query its precomputed, privately constructed geometry.
// A7.2: cold struct, size guard waived (one bounded layout configuration per presentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalStatusPlan {
    mode: StatusStripPlacement,
    msaw_heading: StatusStripPlacement,
    final_heading: StatusStripPlacement,
    safety: [StatusStripPlacement; 2],
    msaw_overflow: StatusStripPlacement,
    final_overflow: StatusStripPlacement,
    console: [StatusStripPlacement; 2],
    top_inset_px: u32,
}

impl RegionalStatusPlan {
    /// Look up a row placement. Safety and console indices outside the fixed row count return None.
    pub fn row(&self, row: StatusStripRow) -> Option<StatusStripPlacement> {
        match row {
            StatusStripRow::Mode => Some(self.mode),
            StatusStripRow::MsawHeading => Some(self.msaw_heading),
            StatusStripRow::FinalHeading => Some(self.final_heading),
            StatusStripRow::Safety(index) => self.safety.get(index).copied(),
            StatusStripRow::MsawOverflow => Some(self.msaw_overflow),
            StatusStripRow::FinalOverflow => Some(self.final_overflow),
            StatusStripRow::Console(index) => self.console.get(index).copied(),
        }
    }

    /// Bottom of the complete reserved strip, in canvas pixels.
    pub const fn top_inset_px(&self) -> u32 {
        self.top_inset_px
    }

    /// Minimum frame height that permits selected-target status below this strip.
    pub fn minimum_frame_height_with_selection(&self) -> Result<u32, DisplayError> {
        crate::core::interaction::minimum_frame_height_with_selection(self.top_inset_px)
    }

    /// Minimum canvas width for the supplied mode and console text, including the right margin.
    pub fn minimum_width_for_operator_rows(
        &self,
        mode: &str,
        console: &str,
    ) -> Result<u32, DisplayError> {
        let rows = regional_operator_rows(mode, console)?;
        let mut right = 0i32;
        let mode_row = self
            .row(StatusStripRow::Mode)
            .ok_or(DisplayError::BadView("regional mode row is unavailable"))?;
        right = right.max(mode_row.bounds_for(text_str(&rows.mode))?.x1);
        let console_rows = rows
            .console
            .get(..rows.console_count)
            .ok_or(DisplayError::BadView(
                "regional console row count is invalid",
            ))?;
        for (index, row_text) in console_rows.iter().enumerate() {
            let placement = self
                .row(StatusStripRow::Console(index))
                .ok_or(DisplayError::BadView("regional console row is unavailable"))?;
            right = right.max(placement.bounds_for(text_str(row_text))?.x1);
        }
        u32::try_from(
            right
                .checked_add(8)
                .ok_or(DisplayError::BadView("regional operator width overflows"))?,
        )
        .map_err(|_| DisplayError::BadView("regional operator width is unrepresentable"))
    }
}

/// Mode and optional console lines prepared by the same plan that positions them.
// A7.2: cold struct, size guard waived (one bounded operator-text packet per presentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionalOperatorRows {
    pub mode: TextBuf,
    pub console: [TextBuf; 2],
    pub console_count: usize,
}

// A7.2: cold struct, size guard waived (fixed style parameters used while constructing the plan).
#[derive(Clone, Copy)]
struct RowStyle {
    size: u8,
    pitch_px: u32,
    colour: u8,
    max_chars: usize,
}

fn placement_at(y_px: i32, style: RowStyle) -> Result<StatusStripPlacement, DisplayError> {
    let anchor = (STRIP_X_PX * 16, y_px * 16);
    let bounds = text_block_bounds_px(
        anchor,
        style.size,
        ALIGN_LEFT,
        style.max_chars,
        1,
        style.pitch_px,
    );
    Ok(StatusStripPlacement {
        x_px: STRIP_X_PX,
        y_px,
        size: style.size,
        pitch_px: style.pitch_px,
        align: ALIGN_LEFT,
        colour: style.colour,
        layer: STRIP_LAYER,
        bounds,
        max_chars: style.max_chars,
    })
}

fn next_anchor(placement: StatusStripPlacement) -> Result<i32, DisplayError> {
    placement
        .bounds
        .y1
        .checked_add(2)
        .ok_or(DisplayError::BadView("status strip row position overflows"))
}

/// Build the stable regional strip plan and derive its map inset from padded text rectangles.
pub fn regional_status_plan() -> Result<RegionalStatusPlan, DisplayError> {
    let mode_style = RowStyle {
        size: MODE_ROW_SIZE,
        pitch_px: STRIP_ROW_PITCH_PX,
        colour: palette::TEXT,
        max_chars: MODE_ROW_MAX_CHARS,
    };
    let alert_style = RowStyle {
        size: ALERT_ROW_SIZE,
        pitch_px: STRIP_ROW_PITCH_PX,
        colour: palette::ALERT,
        max_chars: HEADING_ROW_MAX_CHARS,
    };
    let safety_style = RowStyle {
        size: SAFETY_ROW_SIZE,
        pitch_px: STRIP_ROW_PITCH_PX,
        colour: palette::ALERT,
        max_chars: SAFETY_ROW_MAX_CHARS,
    };
    let overflow_style = RowStyle {
        max_chars: OVERFLOW_ROW_MAX_CHARS,
        ..alert_style
    };
    let mut y = STRIP_FIRST_ANCHOR_PX;
    let mode = placement_at(y, mode_style)?;
    y = next_anchor(mode)?;
    let msaw_heading = placement_at(y, alert_style)?;
    y = next_anchor(msaw_heading)?;
    let final_heading = placement_at(y, alert_style)?;
    y = next_anchor(final_heading)?;
    let safety_first = placement_at(y, safety_style)?;
    y = next_anchor(safety_first)?;
    let safety_second = placement_at(y, safety_style)?;
    y = next_anchor(safety_second)?;
    let msaw_overflow = placement_at(y, overflow_style)?;
    y = next_anchor(msaw_overflow)?;
    let final_overflow = placement_at(y, overflow_style)?;
    y = next_anchor(final_overflow)?;
    let console_first = placement_at(y, mode_style)?;
    y = next_anchor(console_first)?;
    let console_second = placement_at(y, mode_style)?;
    let top_inset_px = u32::try_from(console_second.bounds.y1)
        .map_err(|_| DisplayError::BadView("status strip inset is unrepresentable"))?;
    Ok(RegionalStatusPlan {
        mode,
        msaw_heading,
        final_heading,
        safety: [safety_first, safety_second],
        msaw_overflow,
        final_overflow,
        console: [console_first, console_second],
        top_inset_px,
    })
}

/// Validate and split operator text according to the reserved mode/console row widths.
pub fn regional_operator_rows(
    mode: &str,
    console: &str,
) -> Result<RegionalOperatorRows, DisplayError> {
    if mode.is_empty()
        || mode.len() > MODE_ROW_MAX_CHARS
        || !mode.bytes().all(|byte| (32..=126).contains(&byte))
        || console.len() > MAX_CONSOLE_BYTES
        || !console.bytes().all(|byte| (32..=126).contains(&byte))
    {
        return Err(DisplayError::BadView("regional operator text is invalid"));
    }
    let mut mode_text = text_new();
    text_push_str(&mut mode_text, mode);
    if text_clipped(&mode_text) {
        return Err(DisplayError::BadView("regional mode row is too long"));
    }
    let mut rows = [text_new(); 2];
    let mut count = 0usize;
    for (index, bytes) in console.as_bytes().chunks(CONSOLE_ROW_CHARS).enumerate() {
        let row = rows
            .get_mut(index)
            .ok_or(DisplayError::BadView("regional console row count exceeded"))?;
        let line = core::str::from_utf8(bytes)
            .map_err(|_| DisplayError::BadView("regional console encoding is invalid"))?;
        text_push_str(row, line);
        if text_clipped(row) {
            return Err(DisplayError::BadView("regional console row is too long"));
        }
        count += 1;
    }
    Ok(RegionalOperatorRows {
        mode: mode_text,
        console: rows,
        console_count: count,
    })
}

/// Append the actual operator rows with the plan's sole style and coordinate source.
pub fn append_regional_operator_rows(
    list: &mut DisplayList,
    plan: &RegionalStatusPlan,
    mode: &str,
    console: &str,
) -> Result<(), DisplayError> {
    let rows = regional_operator_rows(mode, console)?;
    let selected_rows = rows
        .console
        .get(..rows.console_count)
        .ok_or(DisplayError::BadView(
            "regional console row count is invalid",
        ))?;
    let byte_count = selected_rows
        .iter()
        .try_fold(text_str(&rows.mode).len(), |sum, row| {
            sum.checked_add(text_str(row).len())
        });
    let byte_count = byte_count.ok_or(DisplayError::TooLarge {
        what: "regional operator text bytes",
        requested: u128::MAX,
        limit: list.caps.text_bytes as u128,
    })?;
    preflight_text_room(list, rows.console_count + 1, byte_count)?;
    push_placement_text(list, plan.row(StatusStripRow::Mode), text_str(&rows.mode))?;
    for (index, row) in selected_rows.iter().enumerate() {
        push_placement_text(
            list,
            plan.row(StatusStripRow::Console(index)),
            text_str(row),
        )?;
    }
    Ok(())
}

fn push_placement_text(
    list: &mut DisplayList,
    placement: Option<StatusStripPlacement>,
    text: &str,
) -> Result<(), DisplayError> {
    let placement = placement.ok_or(DisplayError::BadView("regional status row is unavailable"))?;
    placement.bounds_for(text)?;
    push_text(
        list,
        (placement.x_px * 16, placement.y_px * 16),
        placement.size,
        placement.align,
        placement.colour,
        placement.layer,
        text,
    )
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // Mutation: moving rows or corrupting their public style/bounds getters must fail the geometry oracle.
    // REQ: OA-REG-020
    #[test]
    fn reserved_rows_have_independent_numeric_bounds_and_derived_inset() {
        let plan = regional_status_plan().expect("status plan is valid");
        let rows: [(StatusStripRow, i32, u8, usize, u8); 9] = [
            (StatusStripRow::Mode, 106, 7, 64, palette::TEXT),
            (StatusStripRow::MsawHeading, 124, 10, 128, palette::ALERT),
            (StatusStripRow::FinalHeading, 142, 10, 128, palette::ALERT),
            (StatusStripRow::Safety(0), 160, 8, 128, palette::ALERT),
            (StatusStripRow::Safety(1), 178, 8, 128, palette::ALERT),
            (StatusStripRow::MsawOverflow, 196, 10, 64, palette::ALERT),
            (StatusStripRow::FinalOverflow, 214, 10, 64, palette::ALERT),
            (StatusStripRow::Console(0), 232, 7, 64, palette::TEXT),
            (StatusStripRow::Console(1), 250, 7, 64, palette::TEXT),
        ];
        let mut bottoms = 0i32;
        for (index, (row_kind, anchor, size, chars, colour)) in rows.iter().enumerate() {
            let row = plan.row(*row_kind).expect("reserved row exists");
            assert_eq!(row.x_px(), 8);
            assert_eq!(row.y_px(), *anchor);
            assert_eq!(row.size(), *size);
            assert_eq!(row.pitch_px(), 14);
            assert_eq!(row.align(), ALIGN_LEFT);
            assert_eq!(row.colour(), *colour);
            assert_eq!(row.layer(), 15);
            let width_px = i32::try_from(
                (*chars)
                    .checked_mul(usize::from(*size))
                    .expect("bounded ASCII cell width"),
            )
            .expect("bounded ASCII cell width")
                + 4;
            let expected = TextBounds {
                x0: 6,
                y0: *anchor - 2,
                x1: 6 + width_px,
                y1: *anchor + 16,
            };
            assert_eq!(row.reserved_bounds(), expected);
            assert_eq!(
                row.bounds_for(&"A".repeat(*chars))
                    .expect("reserved text fits"),
                expected
            );
            assert!(row.bounds_for(&"A".repeat(chars + 1)).is_err());
            assert!(row.bounds_for("é").is_err());
            bottoms = bottoms.max(expected.y1);
            for (prior_kind, ..) in rows.iter().take(index) {
                let previous = plan.row(*prior_kind).expect("prior row exists");
                assert!(previous.reserved_bounds().y1 <= expected.y0);
            }
        }
        let heading = plan.row(StatusStripRow::MsawHeading).unwrap();
        assert!(heading.bounds_for(&"A".repeat(129)).is_err());
        assert!(heading.bounds_for("é").is_err());
        assert!(plan.row(StatusStripRow::Safety(2)).is_none());
        assert!(plan.row(StatusStripRow::Console(2)).is_none());
        assert_eq!(
            plan.top_inset_px(),
            u32::try_from(bottoms).expect("positive inset")
        );
    }

    // Mutation: clipping the maximum operator console row must fail the independently derived width.
    // REQ: OA-REG-020
    #[test]
    fn operator_width_uses_actual_mode_and_console_row_metrics() {
        let plan = regional_status_plan().expect("status plan is valid");
        let mode = "M".repeat(MODE_ROW_MAX_CHARS);
        let console = "C".repeat(MAX_CONSOLE_BYTES);
        let minimum = plan
            .minimum_width_for_operator_rows(&mode, &console)
            .expect("maximum admitted operator rows fit a finite width");
        let rows = regional_operator_rows(&mode, &console).expect("maximum rows validate");
        // Font advance is 6/6 of size: 64*7px, x=8, 2px padding, 8px margin.
        let independently_derived_width = 8 + (64 * (7 * 6 / 6)) + 2 + 8;
        assert_eq!(minimum, independently_derived_width);
        assert_eq!(rows.console_count, 2);
    }

    // Mutation: weaken any byte, length, or indexed-row boundary and an invalid value is accepted.
    // REQ: OA-REG-020
    #[test]
    fn operator_rows_refuse_invalid_text_and_unknown_row_slots() {
        for mode in [
            "".to_owned(),
            "M".repeat(65),
            "é".into(),
            "\u{1f}".into(),
            "\u{7f}".into(),
        ] {
            assert!(regional_operator_rows(&mode, "").is_err());
        }
        for console in [
            "C".repeat(129),
            "é".into(),
            "\u{1f}".into(),
            "\u{7f}".into(),
        ] {
            assert!(regional_operator_rows("M", &console).is_err());
        }
        let plan = regional_status_plan().expect("valid renderer plan");
        assert!(plan.row(StatusStripRow::Safety(2)).is_none());
        assert!(plan.row(StatusStripRow::Console(2)).is_none());
    }
}

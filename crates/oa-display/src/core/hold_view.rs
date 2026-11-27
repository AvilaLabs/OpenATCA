//! Bounded presentation adapter for explicit project hold-view records.
//!
//! It displays only caller-supplied instructions. It does not compute a holding trajectory or
//! assert that a clearance was transmitted or accepted.

use crate::core::list::{push_text, DisplayError, DisplayList, ALIGN_LEFT};
use crate::core::palette;
use crate::core::project::Projection;
use crate::core::text::{text_new, text_push_dec, text_push_str, text_str};

pub const MAX_HOLD_VIEW_ROWS: usize = 8192;
pub const MAX_HOLD_VIEW_VISIBLE_ROWS: usize = 24;
const HEADER_X: i32 = 8;
const HEADER_Y: i32 = 168;
const ROW_Y: i32 = 184;
const ROW_STEP: i32 = 16;
const TEXT_SIZE: u8 = 8;
const MAX_ROW_BYTES: usize = 128;

/// Renderer-facing copy of the complete flight key and explicit hold fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldViewEntry {
    pub gufi: [u8; 16],
    pub aircraft_address: u32,
    pub centre: [u8; 4],
    pub plan_id: [u8; 12],
    pub fix_name: [u8; 8],
    pub direction_from_fix_deg: u16,
    pub inbound_course_deg: u16,
    /// 0 absent, 1 left, 2 right.
    pub turn: u8,
    pub leg_value: Option<u16>,
    /// 0 absent, 1 miles, 2 minutes.
    pub leg_unit: u8,
    /// Blank remains `None`; midnight is `Some(0)`.
    pub efc_utc_minute: Option<u16>,
    pub updated_receipt_ns: i64,
    pub revision: u64,
    pub lifecycle_generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HoldViewStats {
    pub total: u16,
    pub drawn: u16,
    pub off_panel: u16,
}

fn fail(reason: &'static str) -> DisplayError {
    DisplayError::BadView(reason)
}

fn valid_padded_ascii<const N: usize>(value: &[u8; N]) -> bool {
    let mut padding = false;
    let mut present = false;
    for byte in value {
        if *byte == 0 {
            padding = true;
        } else if padding || !byte.is_ascii_graphic() {
            return false;
        } else {
            present = true;
        }
    }
    present
}

/// Append a stable Hold View summary and as many rows as fit the bounded panel.
/// The complete packet remains available to the caller; `off_panel` makes visual truncation
/// explicit. All records and capacity are validated before mutating the display list.
pub fn append_hold_view(
    list: &mut DisplayList,
    projection: &Projection,
    entries: &[HoldViewEntry],
) -> Result<HoldViewStats, DisplayError> {
    if entries.len() > MAX_HOLD_VIEW_ROWS {
        return Err(DisplayError::TooLarge {
            what: "hold view records",
            requested: entries.len() as u128,
            limit: MAX_HOLD_VIEW_ROWS as u128,
        });
    }
    if projection.width < 500 || projection.height < 200 || list.faulted {
        return Err(fail("hold view requires a 500 by 200 healthy display"));
    }
    if entries.iter().any(|entry| {
        entry.gufi.iter().all(|byte| *byte == 0)
            || !(1..=0x01ff_ffff).contains(&entry.aircraft_address)
            || !valid_padded_ascii(&entry.centre)
            || !valid_padded_ascii(&entry.plan_id)
            || !valid_padded_ascii(&entry.fix_name)
            || entry.direction_from_fix_deg >= 360
            || entry.inbound_course_deg >= 360
            || entry.turn > 2
            || entry.leg_unit > 2
            || entry.leg_value.is_some_and(|value| value == 0)
            || entry.leg_value.is_some() != (entry.leg_unit != 0)
            || entry.efc_utc_minute.is_some_and(|minute| minute >= 1440)
            || entry.updated_receipt_ns < 0
            || entry.revision == 0
            || entry.lifecycle_generation == 0
    }) {
        return Err(fail(
            "hold view contains an invalid key or instruction field",
        ));
    }
    let total = u16::try_from(entries.len()).map_err(|_| DisplayError::TooLarge {
        what: "hold view count",
        requested: entries.len() as u128,
        limit: u16::MAX as u128,
    })?;
    let drawn = entries.len().min(MAX_HOLD_VIEW_VISIBLE_ROWS);
    let header = {
        let mut value = text_new();
        text_push_str(&mut value, "HOLD VIEW ");
        text_push_dec(&mut value, entries.len() as u64, 0);
        if entries.len() > drawn {
            text_push_str(&mut value, " OFF PANEL ");
            text_push_dec(&mut value, (entries.len() - drawn) as u64, 0);
        }
        value
    };
    if text_str(&header).len() > MAX_ROW_BYTES {
        return Err(fail("hold view header exceeded its fixed bound"));
    }
    let needed_texts = drawn.checked_add(1).ok_or(DisplayError::TooLarge {
        what: "hold view text rows",
        requested: u128::MAX,
        limit: list.caps.texts as u128,
    })?;
    if list
        .texts
        .x
        .len()
        .checked_add(needed_texts)
        .is_none_or(|n| n > list.caps.texts)
        || list
            .text
            .len()
            .checked_add(needed_texts * MAX_ROW_BYTES)
            .is_none_or(|n| n > list.caps.text_bytes)
    {
        return Err(DisplayError::Full {
            table: "hold view text",
            capacity: list.caps.texts.min(list.caps.text_bytes),
        });
    }
    push_text(
        list,
        (HEADER_X * 16, HEADER_Y * 16),
        TEXT_SIZE,
        ALIGN_LEFT,
        palette::TEXT,
        15,
        text_str(&header),
    )?;
    for (index, entry) in entries.iter().take(drawn).enumerate() {
        let fix_end = entry
            .fix_name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(entry.fix_name.len());
        let fix_bytes = entry
            .fix_name
            .get(..fix_end)
            .ok_or_else(|| fail("hold fix name extent"))?;
        let fix_name =
            core::str::from_utf8(fix_bytes).map_err(|_| fail("hold fix name is not ASCII"))?;
        let mut line = text_new();
        text_push_str(&mut line, "HOLD ");
        text_push_dec(&mut line, u64::from(entry.aircraft_address), 0);
        text_push_str(&mut line, " ");
        text_push_str(&mut line, fix_name);
        text_push_str(&mut line, " DIR ");
        text_push_dec(&mut line, u64::from(entry.direction_from_fix_deg), 3);
        text_push_str(&mut line, " IN ");
        text_push_dec(&mut line, u64::from(entry.inbound_course_deg), 3);
        text_push_str(&mut line, " ");
        text_push_str(
            &mut line,
            match entry.turn {
                0 => "--",
                1 => "L",
                2 => "R",
                _ => return Err(fail("hold turn escaped validation")),
            },
        );
        text_push_str(&mut line, " LEG ");
        if let Some(value) = entry.leg_value {
            text_push_dec(&mut line, u64::from(value), 0);
            text_push_str(
                &mut line,
                match entry.leg_unit {
                    1 => "MI",
                    2 => "MIN",
                    _ => return Err(fail("hold leg unit escaped validation")),
                },
            );
        } else {
            text_push_str(&mut line, "--");
        }
        text_push_str(&mut line, " EFC ");
        if let Some(minute) = entry.efc_utc_minute {
            text_push_dec(&mut line, u64::from(minute / 60), 2);
            text_push_dec(&mut line, u64::from(minute % 60), 2);
        } else {
            text_push_str(&mut line, "----");
        }
        text_push_str(&mut line, " REV ");
        text_push_dec(&mut line, entry.revision, 0);
        text_push_str(&mut line, " GEN ");
        text_push_dec(&mut line, entry.lifecycle_generation, 0);
        if text_str(&line).len() > MAX_ROW_BYTES {
            return Err(fail("hold view row exceeded its fixed bound"));
        }
        let y = ROW_Y + i32::try_from(index).map_err(|_| fail("hold view row index"))? * ROW_STEP;
        push_text(
            list,
            (HEADER_X * 16, y * 16),
            TEXT_SIZE,
            ALIGN_LEFT,
            palette::TEXT,
            15,
            text_str(&line),
        )?;
    }
    Ok(HoldViewStats {
        total,
        drawn: drawn as u16,
        off_panel: (entries.len() - drawn) as u16,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use crate::core::list::{list_new, ListCaps};
    use crate::core::project::projection_new;

    fn entry(id: u8) -> HoldViewEntry {
        let mut plan_id = [0; 12];
        plan_id[0] = b'P';
        plan_id[1] = b'0' + id;
        let mut fix_name = [0; 8];
        fix_name[..3].copy_from_slice(b"FIX");
        HoldViewEntry {
            gufi: [id; 16],
            aircraft_address: 0xabc100 + u32::from(id),
            centre: *b"KORD",
            plan_id,
            fix_name,
            direction_from_fix_deg: 90,
            inbound_course_deg: 270,
            turn: 2,
            leg_value: Some(4),
            leg_unit: 1,
            efc_utc_minute: Some(0),
            updated_receipt_ns: 1,
            revision: 1,
            lifecycle_generation: 1,
        }
    }

    fn view() -> Projection {
        projection_new(410_000_000, -870_000_000, 100.0, 800, 600, 160).unwrap()
    }

    fn list(texts: usize, bytes: usize) -> DisplayList {
        list_new(ListCaps {
            lines: 8,
            rects: 8,
            circles: 8,
            polylines: 0,
            poly_vertices: 0,
            texts,
            text_bytes: bytes,
        })
        .unwrap()
    }

    // REQ: OA-ENC-023
    #[test]
    fn packet_keeps_full_key_and_renders_bounded_counted_rows() {
        let mut rows = (1..=26).map(entry).collect::<Vec<_>>();
        rows[1].turn = 0;
        rows[1].leg_value = None;
        rows[1].leg_unit = 0;
        rows[1].efc_utc_minute = None;
        rows[2].direction_from_fix_deg = 359;
        rows[2].turn = 1;
        rows[2].leg_value = Some(u16::MAX);
        rows[2].leg_unit = 2;
        rows[2].efc_utc_minute = Some(1_439);
        rows[2].revision = u64::MAX;
        rows[2].lifecycle_generation = u64::MAX;
        let mut output = list(32, 32 * MAX_ROW_BYTES);
        let stats = append_hold_view(&mut output, &view(), &rows).unwrap();
        assert_eq!(
            stats,
            HoldViewStats {
                total: 26,
                drawn: 24,
                off_panel: 2
            }
        );
        assert_eq!(rows[0].centre, *b"KORD");
        assert_eq!(output.texts.start.len(), 25);
        let spans = output
            .texts
            .start
            .iter()
            .zip(&output.texts.len)
            .map(|(&start, &length)| {
                let start = usize::try_from(start).unwrap();
                let end = start + usize::try_from(length).unwrap();
                output.text.get(start..end).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(spans.first(), Some(&"HOLD VIEW 26 OFF PANEL 2"));
        assert_eq!(
            spans.get(1),
            Some(&"HOLD 11256065 FIX DIR 090 IN 270 R LEG 4MI EFC 0000 REV 1 GEN 1")
        );
        assert_eq!(
            spans.get(2),
            Some(&"HOLD 11256066 FIX DIR 090 IN 270 -- LEG -- EFC ---- REV 1 GEN 1")
        );
        assert_eq!(
            spans.get(3),
            Some(
                &"HOLD 11256067 FIX DIR 359 IN 270 L LEG 65535MIN EFC 2359 REV 18446744073709551615 GEN 18446744073709551615"
            )
        );
    }

    #[test]
    fn invalid_key_or_capacity_refusal_is_atomic() {
        let mut bad = entry(1);
        bad.plan_id = *b"A\0BCDEFGHIJK";
        let mut output = list(32, 32 * MAX_ROW_BYTES);
        assert!(append_hold_view(&mut output, &view(), &[bad]).is_err());
        assert!(output.texts.start.is_empty());

        let mut output = list(1, MAX_ROW_BYTES);
        assert!(append_hold_view(&mut output, &view(), &[entry(1)]).is_err());
        assert!(output.texts.start.is_empty());
    }
}

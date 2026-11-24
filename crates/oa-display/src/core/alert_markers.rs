//! Current MSAW alert markers (**core**, B1, A3): bounded plain observations to display-list
//! primitives. This presenter does not decide alert state or move a marker to an alert origin.
//!
//! At most [`MAX_ALERT_MARKERS`] whole 16-byte records (2,048 bytes) are read per call; the
//! regional bridge currently supplies at most 32. Records must be ordered by unique positive
//! aircraft address. Validation, view-width checks and list-capacity checks finish before any
//! display-list mutation. The caller reserves the worst-case table budget with the exported
//! per-marker and header constants: two circles, one text and one text byte per marker, plus one
//! header text and 64 header bytes.

use crate::core::font::{text_width_units, CAP_HEIGHT};
use crate::core::list::{push_circle, push_text, DisplayError, DisplayList, ALIGN_CENTRE, ALIGN_LEFT};
use crate::core::palette;
use crate::core::project::{project, Projection};
use crate::core::text::{text_new, text_push_dec, text_push_str, text_str};

/// Fixed records read by one presentation call. The regional bridge uses a smaller limit of 32.
pub const MAX_ALERT_MARKERS: usize = 128;
/// Worst case circle table reservation for one marker (outer and inner rings).
pub const ALERT_CIRCLES_PER_MARKER: usize = 2;
/// Each marker reserves room for one short state label, whether or not it is visible.
pub const ALERT_TEXTS_PER_MARKER: usize = 1;
/// Header reservation independent of its current decimal values.
pub const ALERT_HEADER_TEXTS: usize = 1;
pub const ALERT_HEADER_TEXT_BYTES: usize = 64;
pub const ALERT_TEXT_BYTES_PER_MARKER: usize = 1;

/// Sentinel used when `position_known` is zero.
pub const NO_POSITION_E7: i32 = i32::MIN;
const ALERT_LAYER: u8 = 13;
const ALERT_TEXT_SIZE: u8 = 10;
const ALERT_HEADER_X_PX: i32 = 8;
const ALERT_HEADER_Y_PX: i32 = 72;
const ALERT_OUTER_RADIUS_PX: i32 = 16;
const ALERT_INNER_RADIUS_PX: i32 = 12;
const MIN_ALERT_VIEW_WIDTH: u32 = 500;
const MIN_ALERT_TOP_INSET: u32 = 104;

const fn px(value: i32) -> i32 {
    value * 16
}

/// MSAW state, shown with shape and a one-letter label as well as alert colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MarkerState {
    Caution = 0,
    Warning = 1,
    ProtectionLost = 2,
}

/// One current MSAW alert and its current observed position. Unknown position never becomes a
/// guessed point; a known position is projected at the observation coordinates supplied here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AlertMarker {
    pub address: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub state: MarkerState,
    pub position_known: u8,
    pub reserved: [u8; 2],
}
const _: () = assert!(core::mem::size_of::<AlertMarker>() == 16);

/// Counts for one bounded presentation call. State counts include alerts off the frame or without
/// a known position; `drawn` counts only alerts with a visible current point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct MarkerStats {
    pub examined: u16,
    pub drawn: u16,
    pub off_map: u16,
    pub unlocated: u16,
    pub caution: u16,
    pub warning: u16,
    pub protection_lost: u16,
    pub reserved: [u8; 2],
}
const _: () = assert!(core::mem::size_of::<MarkerStats>() == 16);

fn bad_alert(what: &'static str) -> DisplayError {
    DisplayError::BadView(what)
}

fn ensure_room(
    table: &'static str,
    used: usize,
    needed: usize,
    capacity: usize,
) -> Result<(), DisplayError> {
    if used
        .checked_add(needed)
        .is_some_and(|requested| requested <= capacity)
    {
        Ok(())
    } else {
        Err(DisplayError::Full { table, capacity })
    }
}

fn inside_frame(p: &Projection, (x, y): (i32, i32)) -> bool {
    let x = i64::from(x);
    let y = i64::from(y);
    x >= 0
        && x < i64::from(p.width) * 16
        && y >= i64::from(p.top_inset) * 16
        && y < i64::from(p.height) * 16
}

fn increment(counter: &mut u16) {
    *counter = counter.saturating_add(1);
}

fn header(stats: MarkerStats) -> crate::core::text::TextBuf {
    let mut text = text_new();
    text_push_str(&mut text, "MSAW WARNING ");
    text_push_dec(&mut text, u64::from(stats.warning), 0);
    text_push_str(&mut text, " CAUTION ");
    text_push_dec(&mut text, u64::from(stats.caution), 0);
    text_push_str(&mut text, " LOST ");
    text_push_dec(&mut text, u64::from(stats.protection_lost), 0);
    text_push_str(&mut text, " OFF ");
    text_push_dec(&mut text, u64::from(stats.off_map), 0);
    text_push_str(&mut text, " UNLOCATED ");
    text_push_dec(&mut text, u64::from(stats.unlocated), 0);
    text
}

/// Append current alert markers and their bounded count header to an existing display list.
///
/// The view must provide a 104 px legend strip and at least 500 px of width. Exact header width
/// and all worst-case list capacity requirements are checked before writing anything.
pub fn append_alert_markers(
    list: &mut DisplayList,
    projection: &Projection,
    markers: &[AlertMarker],
) -> Result<MarkerStats, DisplayError> {
    if projection.width < MIN_ALERT_VIEW_WIDTH
        || projection.top_inset < MIN_ALERT_TOP_INSET
        || projection.height <= projection.top_inset
    {
        return Err(bad_alert("MSAW markers require width 500 and a 104 px top strip"));
    }
    if list.faulted {
        return Err(bad_alert("MSAW markers cannot append to a faulted list"));
    }
    if markers.len() > MAX_ALERT_MARKERS {
        return Err(DisplayError::TooLarge {
            what: "MSAW alert marker records",
            requested: markers.len() as u128,
            limit: MAX_ALERT_MARKERS,
        });
    }

    // Validate the complete record set and compute every placement before list mutation.
    let examined = match u16::try_from(markers.len()) {
        Ok(value) => value,
        Err(_) => {
            return Err(DisplayError::TooLarge {
                what: "MSAW alert marker count",
                requested: markers.len() as u128,
                limit: MAX_ALERT_MARKERS,
            })
        }
    };
    let mut stats = MarkerStats {
        examined,
        ..MarkerStats::default()
    };
    let mut projected = [None; MAX_ALERT_MARKERS];
    let mut previous_address = 0u32;
    for (index, marker) in markers.iter().enumerate() {
        if marker.address == 0 || marker.address <= previous_address {
            return Err(bad_alert("MSAW addresses must be positive, unique and increasing"));
        }
        previous_address = marker.address;
        if marker.reserved != [0; 2] {
            return Err(bad_alert("MSAW marker reserved bytes are nonzero"));
        }
        match marker.state {
            MarkerState::Caution => increment(&mut stats.caution),
            MarkerState::Warning => increment(&mut stats.warning),
            MarkerState::ProtectionLost => increment(&mut stats.protection_lost),
        }
        match marker.position_known {
            0 => {
                if marker.lat_e7 != NO_POSITION_E7 || marker.lon_e7 != NO_POSITION_E7 {
                    return Err(bad_alert("unknown MSAW position must use both sentinels"));
                }
                increment(&mut stats.unlocated);
            }
            1 => {
                if !(-900_000_000..=900_000_000).contains(&marker.lat_e7)
                    || !(-1_800_000_000..=1_800_000_000).contains(&marker.lon_e7)
                {
                    return Err(bad_alert("known MSAW position is outside coordinate bounds"));
                }
                match project(projection, marker.lat_e7, marker.lon_e7) {
                    Some(point) if inside_frame(projection, point) => {
                        projected[index] = Some(point);
                        increment(&mut stats.drawn);
                    }
                    _ => increment(&mut stats.off_map),
                }
            }
            _ => return Err(bad_alert("MSAW position-known flag must be zero or one")),
        }
    }

    let header_text = header(stats);
    if crate::core::text::text_clipped(&header_text) {
        return Err(bad_alert("MSAW count header exceeds its fixed text buffer"));
    }
    let header_str = text_str(&header_text);
    let header_width16 = i64::from(text_width_units(header_str.chars().count()))
        .checked_mul(i64::from(ALERT_TEXT_SIZE))
        .and_then(|width| width.checked_mul(16))
        .and_then(|width| width.checked_div(i64::from(CAP_HEIGHT)))
        .ok_or(DisplayError::TooLarge {
            what: "MSAW header width",
            requested: u128::MAX,
            limit: projection.width as usize * 16,
        })?;
    if i64::from(ALERT_HEADER_X_PX) * 16 + header_width16 > i64::from(projection.width) * 16 {
        return Err(bad_alert("MSAW count header does not fit the view width"));
    }

    let needed_circles = markers
        .len()
        .checked_mul(ALERT_CIRCLES_PER_MARKER)
        .ok_or(DisplayError::TooLarge {
            what: "MSAW circle reservation",
            requested: u128::MAX,
            limit: list.caps.circles,
        })?;
    let needed_texts = markers
        .len()
        .checked_mul(ALERT_TEXTS_PER_MARKER)
        .and_then(|count| count.checked_add(ALERT_HEADER_TEXTS))
        .ok_or(DisplayError::TooLarge {
            what: "MSAW text reservation",
            requested: u128::MAX,
            limit: list.caps.texts,
        })?;
    let needed_text_bytes = markers
        .len()
        .checked_mul(ALERT_TEXT_BYTES_PER_MARKER)
        .and_then(|count| count.checked_add(ALERT_HEADER_TEXT_BYTES))
        .ok_or(DisplayError::TooLarge {
            what: "MSAW text byte reservation",
            requested: u128::MAX,
            limit: list.caps.text_bytes,
        })?;
    ensure_room("circles", list.circles.x.len(), needed_circles, list.caps.circles)?;
    ensure_room("texts", list.texts.x.len(), needed_texts, list.caps.texts)?;
    ensure_room("text bytes", list.text.len(), needed_text_bytes, list.caps.text_bytes)?;

    for (index, marker) in markers.iter().enumerate() {
        let Some(point) = projected.get(index).copied().flatten() else {
            continue;
        };
        match marker.state {
            MarkerState::Caution => {
                push_circle(list, point, px(ALERT_INNER_RADIUS_PX), palette::ALERT, ALERT_LAYER)?;
            }
            MarkerState::Warning | MarkerState::ProtectionLost => {
                push_circle(list, point, px(ALERT_OUTER_RADIUS_PX), palette::ALERT, ALERT_LAYER)?;
                push_circle(list, point, px(ALERT_INNER_RADIUS_PX), palette::ALERT, ALERT_LAYER)?;
            }
        }
        let label = match marker.state {
            MarkerState::Caution => "C",
            MarkerState::Warning => "W",
            MarkerState::ProtectionLost => "L",
        };
        push_text(
            list,
            (point.0, point.1 - px(i32::from(ALERT_TEXT_SIZE) / 2)),
            ALERT_TEXT_SIZE,
            ALIGN_CENTRE,
            palette::ALERT,
            ALERT_LAYER,
            label,
        )?;
    }
    push_text(
        list,
        (px(ALERT_HEADER_X_PX), px(ALERT_HEADER_Y_PX)),
        ALERT_TEXT_SIZE,
        ALIGN_LEFT,
        palette::ALERT,
        ALERT_LAYER,
        header_str,
    )?;
    Ok(stats)
}

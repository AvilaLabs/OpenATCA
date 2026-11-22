//! Controller ownership and handoff evidence (**core**, B1, B2): a caller supplies a typed,
//! already-decided ownership snapshot, and this module draws one fixed-budget status line.
//! Transport acknowledgments are deliberately absent from the input type; the supplied
//! `current_owner` is rendered unchanged in every phase.

use crate::core::font::{text_width_units, CAP_HEIGHT};
use crate::core::list::{push_text, DisplayError, DisplayList, ListCaps, ALIGN_RIGHT};
use crate::core::palette;
use crate::core::project::Projection;
use crate::core::scene::LEGEND_STRIP_PX;
use crate::core::text::{text_clipped, text_new, text_push_char, text_push_str, text_str, TextBuf};

/// Bytes in a flight callsign or unit designator supplied to the ownership display.
pub const OWNERSHIP_ID_BYTES: usize = 8;
/// Text budget for the longest ownership status line: the stale phase with two eight-byte IDs.
/// A7.1: 64 bytes covers its 56 ASCII characters plus margin; the line is always one list text.
pub const OWNERSHIP_TEXT_BYTES: usize = 64;
/// Ownership status text, above map and traffic data.
pub const LAYER_OWNERSHIP: u8 = 13;

const STATUS_SIZE: u8 = 10;
const RIGHT_PAD_PX: i32 = 8;
const STATUS_TEXT_FROM_BOTTOM_PX: i32 = 42;

/// The transfer state that belongs to a flight, with its current owner supplied separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold display input, one per rendered scope frame.
pub enum OwnershipPhase {
    /// No handoff is pending.
    Current,
    /// The owner offered the flight to the receiving unit; ownership has not moved.
    Offered { to: [u8; OWNERSHIP_ID_BYTES] },
    /// The receiving unit accepted, but has not yet taken control.
    Accepted { by: [u8; OWNERSHIP_ID_BYTES] },
    /// A stale offer was refused; the supplied current owner remains authoritative.
    StaleOffer { to: [u8; OWNERSHIP_ID_BYTES] },
}

/// A single flight's controller-facing ownership line. IDs are uppercase ASCII letters/digits,
/// zero padded on the right; all three IDs must be nonempty. `current_owner` always comes from the
/// ownership record, including in offered, accepted, and stale states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold display input, one per rendered scope frame.
pub struct OwnershipStatus {
    pub callsign: [u8; OWNERSHIP_ID_BYTES],
    pub current_owner: [u8; OWNERSHIP_ID_BYTES],
    pub phase: OwnershipPhase,
}

/// Capacity for one ownership line; add this to the map's existing caps.
pub const fn ownership_caps() -> ListCaps {
    ListCaps {
        lines: 0,
        rects: 0,
        circles: 0,
        polylines: 0,
        poly_vertices: 0,
        texts: 1,
        text_bytes: OWNERSHIP_TEXT_BYTES,
    }
}

fn valid_identifier(id: &[u8; OWNERSHIP_ID_BYTES]) -> bool {
    let mut ended = false;
    let mut present = false;
    for &byte in id {
        if byte == 0 {
            ended = true;
        } else if ended || !(byte.is_ascii_uppercase() || byte.is_ascii_digit()) {
            return false;
        } else {
            present = true;
        }
    }
    present
}

fn add_identifier(b: &mut TextBuf, id: &[u8; OWNERSHIP_ID_BYTES]) {
    for &byte in id {
        if byte == 0 {
            break;
        }
        text_push_char(b, byte);
    }
}

fn status_text(s: &OwnershipStatus, b: &mut TextBuf) -> u8 {
    text_push_str(b, "FLIGHT ");
    add_identifier(b, &s.callsign);
    text_push_str(b, " OWNER ");
    add_identifier(b, &s.current_owner);
    match s.phase {
        OwnershipPhase::Current => {
            text_push_str(b, " | CURRENT");
            palette::TARGET
        }
        OwnershipPhase::Offered { to } => {
            text_push_str(b, " | OFFERED TO ");
            add_identifier(b, &to);
            palette::ARTCC_HIGH
        }
        OwnershipPhase::Accepted { by } => {
            text_push_str(b, " | ACCEPTED BY ");
            add_identifier(b, &by);
            palette::FIX
        }
        OwnershipPhase::StaleOffer { to } => {
            text_push_str(b, " | STALE OFFER TO ");
            add_identifier(b, &to);
            palette::ALERT
        }
    }
}

/// Appends the ownership state as one right-aligned status line above traffic notices.
/// A malformed identifier, a frame too short for the reserved row, or a line too wide is refused
/// before this function changes the list. The caller supplies capacities from [`ownership_caps`].
pub fn build_ownership_status(
    l: &mut DisplayList,
    proj: &Projection,
    status: &OwnershipStatus,
) -> Result<(), DisplayError> {
    let phase_id = match status.phase {
        OwnershipPhase::Current => None,
        OwnershipPhase::Offered { to } | OwnershipPhase::StaleOffer { to } => Some(to),
        OwnershipPhase::Accepted { by } => Some(by),
    };
    if !valid_identifier(&status.callsign)
        || !valid_identifier(&status.current_owner)
        || phase_id.is_some_and(|id| !valid_identifier(&id))
    {
        return Err(DisplayError::BadView(
            "ownership identifiers must be uppercase ASCII",
        ));
    }
    if i64::from(proj.height) - i64::from(STATUS_TEXT_FROM_BOTTOM_PX)
        < i64::from(proj.top_inset) + 8
    {
        return Err(DisplayError::BadView(
            "frame leaves no room for ownership status row",
        ));
    }

    let mut text = text_new();
    let colour = status_text(status, &mut text);
    let content = text_str(&text);
    if text_clipped(&text) || content.len() > OWNERSHIP_TEXT_BYTES {
        return Err(DisplayError::BadView(
            "ownership status exceeds its text budget",
        ));
    }
    let text_width16 = text_width_units(content.len())
        .saturating_mul(i32::from(STATUS_SIZE))
        .saturating_mul(16)
        / CAP_HEIGHT;
    let right16 = i32::try_from(proj.width)
        .unwrap_or(i32::MAX)
        .saturating_mul(16)
        .saturating_sub(RIGHT_PAD_PX * 16);
    let left16 = right16.saturating_sub(text_width16);
    if left16 < 0 {
        return Err(DisplayError::BadView(
            "frame is too narrow for ownership status line",
        ));
    }

    let text_y = i32::try_from(proj.height)
        .unwrap_or(i32::MAX)
        .saturating_sub(STATUS_TEXT_FROM_BOTTOM_PX)
        .saturating_mul(16);
    push_text(
        l,
        (right16, text_y),
        STATUS_SIZE,
        ALIGN_RIGHT,
        colour,
        LAYER_OWNERSHIP,
        content,
    )
}

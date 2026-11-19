//! Live traffic (**core**, B1, B2): a pure function from a set of displayed targets and a
//! projection to display-list commands, appended to the list the static map built. Nothing here
//! reads a clock or a file, and the crate does not know the tracker: the shell turns tracker rows
//! into [`Target`]s (SCOPE design principle 7, RULESET D3).
//!
//! Every public item of this module is core. Nothing allocates after [`targets_new`] and
//! [`crate::core::list::list_new`] (C1, C5): the data block text is written into a stack
//! [`crate::core::text::TextBuf`], and the display list's own tables have their capacity from the start.
//!
//! # Symbols
//!
//! Plain, and never carried by colour alone. Sizes are pixels; every colour is a palette entry.
//!
//! | state                  | drawing                                                          |
//! |------------------------|------------------------------------------------------------------|
//! | tracking               | 8 px square outline and a centre dot, [`palette::TARGET`]            |
//! | coasting               | the square with an X through it; data block tag `CST`            |
//! | uncertain              | a 12 px diamond and a centre dot, and the 95% radius drawn as a circle of true scale; tag `UNC` (and `R95?` when the radius is unknown or cannot be drawn) |
//! | coasting and uncertain | the diamond with an X through it, the circle; tags `CST UNC`     |
//! | reacquired             | any of the above plus a 9 px radius circle in [`palette::ALERT`] for [`TrafficStyle::reacquired_s`] seconds after the reacquisition; tag `REACQ` |
//! | callsign conflict      | the hex address in place of the callsign and the tag `CS?` ([`T_CS_CONFLICT`]) |
//!
//! Every target also has a velocity leader (a line from the symbol along the ground track, as long
//! as the target flies in [`TrafficStyle::leader_min`] minutes, absent without a velocity), a
//! history trail of small [`palette::DIM_TEXT`] dots at the last [`TrafficStyle::trail_n`] positions,
//! and a data block at a fixed offset: line 1 the callsign (the hex address without one, or with
//! two callsigns heard for the address at one instant), line 2 the altitude in hundreds of feet
//! and the ground speed in knots over ten (`---` or `--` when absent), line 3 the state tags
//! above when there are any, in the order `CST UNC CS? R95? REACQ`.
//!
//! Data blocks are placed at one fixed offset from the symbol.
// TODO(LS-401): automatic data block placement without overlap in holding and on final (M8);
// until then blocks of close targets overlap.
//!
//! # Capacity, and what is never silent
//!
//! A [`TargetSet`] has a fixed capacity. A push past it is refused, counted ([`targets_refused`])
//! and answered with [`TrafficError::Full`]; a target that is not valid (a position off the globe,
//! a trail longer than [`MAX_TRAIL`], unknown flag bits) is refused the same way
//! ([`TrafficError::Malformed`], [`targets_malformed`]). Nothing is truncated silently. The frame
//! then carries `TARGETS NOT DRAWN: N` (and `TARGETS MALFORMED: N`) in the alert colour at the
//! bottom left. A target whose position is off the map area (or under the legend strip) is counted
//! in [`TrafficStats::off_screen`], as a scope does at its edge. Every part of a target that could
//! not be drawn (a leader, a 95% circle, a history dot, a text) is counted in [`TrafficStats`] by
//! reason; [`traffic_loss`] sums the ones that are work lost (E6, G6).
//!
//! The set's columns are private, so every target in it went through [`targets_push`] and has
//! the same number of entries in every column: no default stands in for a missing column.

use crate::core::list::{
    check_limit, push_circle, push_line, push_rect, push_text, DisplayError, DisplayList, ListCaps,
    ALIGN_LEFT, MAX_LIST_ROWS, MAX_LIST_TEXT_BYTES,
};
use crate::core::palette;
use crate::core::project::{project, Projection};
use crate::core::text::{
    text_clear, text_clipped, text_is_empty, text_new, text_push_char, text_push_dec,
    text_push_hex, text_push_str, text_str, TextBuf,
};
use oa_geo::{Ellipsoid, LatLon};
use std::fmt;

/// Layers of the traffic, between the map and the legend strip.
pub const LAYER_TRAIL: u8 = 7;
pub const LAYER_LEADER: u8 = 8;
pub const LAYER_SYMBOL: u8 = 9;
pub const LAYER_BLOCK: u8 = 10;

/// Most history positions a target carries.
pub const MAX_TRAIL: usize = 16;
/// Absent altitude. A real altitude is always above this: the shell clamps real values to
/// `NO_ALT + 1` and counts the clamp, so the sentinel cannot be reached from data.
pub const NO_ALT: i32 = i32::MIN;
/// Absent ground speed or track.
pub const NO_U16: u16 = u16::MAX;
/// Absent reacquisition: the target has not been reacquired lately. A real age is always below
/// this: the shell limits the window to [`MAX_REACQ_AGE_S`].
pub const NO_REACQ: u16 = u16::MAX;
/// Largest real reacquisition age, seconds (one below the sentinel).
pub const MAX_REACQ_AGE_S: u16 = NO_REACQ - 1;

/// [`Target::flags`]: the track is coasting.
pub const T_COAST: u8 = 1;
/// The position is uncertain (its 95% radius is over the tracker's limit).
pub const T_UNCERTAIN: u8 = 2;
/// Two different callsigns were heard for this address at the same instant: the callsign is not
/// shown (it would be a choice by sort order, E6) and the data block says so.
pub const T_CS_CONFLICT: u8 = 4;
/// Every flag bit this module knows; a target with another bit is refused.
const FLAGS_KNOWN: u8 = T_COAST | T_UNCERTAIN | T_CS_CONFLICT;

/// Largest leader, minutes.
pub const MAX_LEADER_MIN: f64 = 30.0;
/// Largest number of targets one set may be given.
pub const MAX_TARGETS: usize = 1 << 20;

/// Valid position range, 1e-7 degree.
const LAT_E7_MAX: i32 = 900_000_000;
const LON_E7_MAX: i32 = 1_800_000_000;

// Geometry, in pixels.
const SQUARE_HALF: i32 = 4;
const DIAMOND_HALF: i32 = 6;
const REACQ_RADIUS: i32 = 9;
const TRAIL_HALF: i32 = 1;
const BLOCK_DX: i32 = 14;
const BLOCK_DY: i32 = -30;
const BLOCK_SIZE: u8 = 10;
const BLOCK_PITCH: i32 = 14;
const NOTICE_SIZE: u8 = 12;
const NOTICE_X: i32 = 8;
const NOTICE_FROM_BOTTOM: i32 = 22;
const NOTICE_PITCH: i32 = 14;
/// Notices a frame can carry: targets not drawn, targets malformed.
const NOTICES: usize = 2;
const MAX_BLOCK_LINES: usize = 3;
/// Text bytes one target can need: line 1 at most 8 (a callsign) or 7 (a hex address), line 2 at
/// most 8 (`999+ 655`), line 3 at most 22 (`CST UNC CS? R95? REACQ`); 37 in all, 64 allowed.
const BYTES_PER_TARGET: usize = 64;
const NOTICE_BYTES: usize = 96;

const fn px(n: i32) -> i32 {
    n * 16
}

/// Why a target could not be added.
// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrafficError {
    /// The set is at capacity; the target was refused and counted.
    Full { capacity: usize },
    /// The target is not valid (E5); it was refused and counted.
    Malformed { what: &'static str },
}

impl fmt::Display for TrafficError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrafficError::Full { capacity } => write!(
                f,
                "target set full at {capacity} targets; the target was refused and counted"
            ),
            TrafficError::Malformed { what } => {
                write!(f, "target refused and counted: {what}")
            }
        }
    }
}

impl std::error::Error for TrafficError {}

/// One displayed target. Plain data (A1).
///
/// A9 exception: `r95_m` is an `f32` in metres. It is the tracker's own output precision, it is
/// used once, to size a drawn circle that is then rounded to 1/16 pixel, and it is never state
/// that a later frame reads back, so the machine-to-machine reproducibility A9 protects is not at
/// risk. Every other column is fixed point.
// Budget: built once per target per frame to be pushed into the set (the set holds the columns, so
// this record is not itself held in quantity). 164 bytes is the packed size: the 16-entry trail is
// 128 of them. Fields are ordered 4-byte first so there is no padding.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Target {
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Altitude, feet, or [`NO_ALT`].
    pub alt_ft: i32,
    /// Aircraft address (bit 24 set for a non-ICAO one); shown as hex without a callsign.
    pub addr: u32,
    /// 95% horizontal position radius, metres; NaN or not positive when unknown.
    pub r95_m: f32,
    /// Positions before the current one, newest first; the first `trail_len` are valid.
    pub trail: [(i32, i32); MAX_TRAIL],
    /// Callsign, ASCII, NUL or space padded; all NUL when absent.
    pub callsign: [u8; 8],
    /// Ground track, 0.1 degree true in [0, 3600), or [`NO_U16`].
    pub track_ddeg: u16,
    /// Ground speed, 0.1 knot, or [`NO_U16`]. A leader needs both this and the track.
    pub gs_dkt: u16,
    /// Seconds since the track was last reacquired, or [`NO_REACQ`].
    pub reacq_age_s: u16,
    /// [`T_COAST`], [`T_UNCERTAIN`] and [`T_CS_CONFLICT`].
    pub flags: u8,
    pub trail_len: u8,
}
const _: () = assert!(core::mem::size_of::<Target>() == 164);

/// The targets of one frame, struct-of-arrays (A3), of fixed capacity (C1, C2). Trail positions
/// are `MAX_TRAIL` rows per target in the flat `trail_*` columns. The columns are private: the
/// only way in is [`targets_push`], so every column has one entry per target (A1.1, D4).
// A7.2: cold struct (a collection of columns), size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct TargetSet {
    capacity: usize,
    refused_full: u32,
    refused_malformed: u32,
    lat_e7: Vec<i32>,
    lon_e7: Vec<i32>,
    track_ddeg: Vec<u16>,
    gs_dkt: Vec<u16>,
    alt_ft: Vec<i32>,
    addr: Vec<u32>,
    callsign: Vec<[u8; 8]>,
    flags: Vec<u8>,
    reacq_age_s: Vec<u16>,
    r95_m: Vec<f32>,
    trail_len: Vec<u8>,
    trail_lat_e7: Vec<i32>,
    trail_lon_e7: Vec<i32>,
}

/// Read access to the columns of a [`TargetSet`], all of one length (the number of targets), the
/// trail columns [`MAX_TRAIL`] times that. A view of plain slices; A7.2: cold struct.
#[derive(Clone, Copy, Debug)]
pub struct TargetColumns<'a> {
    pub lat_e7: &'a [i32],
    pub lon_e7: &'a [i32],
    pub track_ddeg: &'a [u16],
    pub gs_dkt: &'a [u16],
    pub alt_ft: &'a [i32],
    pub addr: &'a [u32],
    pub callsign: &'a [[u8; 8]],
    pub flags: &'a [u8],
    pub reacq_age_s: &'a [u16],
    pub r95_m: &'a [f32],
    pub trail_len: &'a [u8],
    pub trail_lat_e7: &'a [i32],
    pub trail_lon_e7: &'a [i32],
}

/// An empty set with room for `capacity` targets; the only allocation. A capacity over
/// [`MAX_TARGETS`] is refused before anything is allocated.
pub fn targets_new(capacity: usize) -> Result<TargetSet, DisplayError> {
    check_limit("targets", capacity as u128, MAX_TARGETS)?;
    let trail = capacity * MAX_TRAIL;
    Ok(TargetSet {
        capacity,
        refused_full: 0,
        refused_malformed: 0,
        lat_e7: Vec::with_capacity(capacity),
        lon_e7: Vec::with_capacity(capacity),
        track_ddeg: Vec::with_capacity(capacity),
        gs_dkt: Vec::with_capacity(capacity),
        alt_ft: Vec::with_capacity(capacity),
        addr: Vec::with_capacity(capacity),
        callsign: Vec::with_capacity(capacity),
        flags: Vec::with_capacity(capacity),
        reacq_age_s: Vec::with_capacity(capacity),
        r95_m: Vec::with_capacity(capacity),
        trail_len: Vec::with_capacity(capacity),
        trail_lat_e7: Vec::with_capacity(trail),
        trail_lon_e7: Vec::with_capacity(trail),
    })
}

/// Number of targets in the set.
pub fn targets_len(s: &TargetSet) -> usize {
    s.lat_e7.len()
}

/// The capacity the set was made with.
pub fn targets_capacity(s: &TargetSet) -> usize {
    s.capacity
}

/// Targets refused because the set was full, since the last [`targets_clear`].
pub fn targets_refused(s: &TargetSet) -> u32 {
    s.refused_full
}

/// Targets refused because they were not valid, since the last [`targets_clear`].
pub fn targets_malformed(s: &TargetSet) -> u32 {
    s.refused_malformed
}

/// The columns of the set, for reading.
pub fn targets_columns(s: &TargetSet) -> TargetColumns<'_> {
    TargetColumns {
        lat_e7: &s.lat_e7,
        lon_e7: &s.lon_e7,
        track_ddeg: &s.track_ddeg,
        gs_dkt: &s.gs_dkt,
        alt_ft: &s.alt_ft,
        addr: &s.addr,
        callsign: &s.callsign,
        flags: &s.flags,
        reacq_age_s: &s.reacq_age_s,
        r95_m: &s.r95_m,
        trail_len: &s.trail_len,
        trail_lat_e7: &s.trail_lat_e7,
        trail_lon_e7: &s.trail_lon_e7,
    }
}

/// Empties the set and its refusal counts for the next frame, keeping every capacity.
pub fn targets_clear(s: &mut TargetSet) {
    s.refused_full = 0;
    s.refused_malformed = 0;
    s.lat_e7.clear();
    s.lon_e7.clear();
    s.track_ddeg.clear();
    s.gs_dkt.clear();
    s.alt_ft.clear();
    s.addr.clear();
    s.callsign.clear();
    s.flags.clear();
    s.reacq_age_s.clear();
    s.r95_m.clear();
    s.trail_len.clear();
    s.trail_lat_e7.clear();
    s.trail_lon_e7.clear();
}

fn position_ok(lat_e7: i32, lon_e7: i32) -> bool {
    (-LAT_E7_MAX..=LAT_E7_MAX).contains(&lat_e7) && (-LON_E7_MAX..=LON_E7_MAX).contains(&lon_e7)
}

/// What is wrong with a target, if anything (E5).
fn malformed(t: &Target) -> Option<&'static str> {
    let n = usize::from(t.trail_len);
    if n > MAX_TRAIL {
        return Some("trail length above the maximum");
    }
    if !position_ok(t.lat_e7, t.lon_e7) {
        return Some("position outside the globe");
    }
    if t.trail.iter().take(n).any(|p| !position_ok(p.0, p.1)) {
        return Some("history position outside the globe");
    }
    if t.flags & !FLAGS_KNOWN != 0 {
        return Some("unknown flag bits");
    }
    None
}

/// Adds a target, or refuses it and counts it: at capacity the call returns
/// [`TrafficError::Full`], a target that is not valid [`TrafficError::Malformed`].
pub fn targets_push(s: &mut TargetSet, t: &Target) -> Result<(), TrafficError> {
    if s.lat_e7.len() >= s.capacity {
        s.refused_full = s.refused_full.saturating_add(1);
        return Err(TrafficError::Full {
            capacity: s.capacity,
        });
    }
    if let Some(what) = malformed(t) {
        s.refused_malformed = s.refused_malformed.saturating_add(1);
        return Err(TrafficError::Malformed { what });
    }
    s.lat_e7.push(t.lat_e7);
    s.lon_e7.push(t.lon_e7);
    s.track_ddeg.push(t.track_ddeg);
    s.gs_dkt.push(t.gs_dkt);
    s.alt_ft.push(t.alt_ft);
    s.addr.push(t.addr);
    s.callsign.push(t.callsign);
    s.flags.push(t.flags);
    s.reacq_age_s.push(t.reacq_age_s);
    s.r95_m.push(t.r95_m);
    // Checked above: at most MAX_TRAIL (16).
    let n = usize::from(t.trail_len);
    s.trail_len.push(t.trail_len);
    for k in 0..MAX_TRAIL {
        let (la, lo) = if k < n {
            t.trail.get(k).copied().unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        s.trail_lat_e7.push(la);
        s.trail_lon_e7.push(lo);
    }
    Ok(())
}

/// How traffic is drawn. Plain data; A7.2: cold struct (one per display).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrafficStyle {
    /// Velocity leader, minutes of flight.
    pub leader_min: f64,
    /// History positions drawn, at most [`MAX_TRAIL`].
    pub trail_n: u8,
    /// A reacquired mark is drawn for this many seconds.
    pub reacquired_s: u16,
}

/// One minute of leader, five history dots, a reacquired mark for ten seconds.
pub const DEFAULT_STYLE: TrafficStyle = TrafficStyle {
    leader_min: 1.0,
    trail_n: 5,
    reacquired_s: 10,
};

impl Default for TrafficStyle {
    fn default() -> Self {
        DEFAULT_STYLE
    }
}

/// What a traffic build drew and left out, by reason (E6). A7.2: cold struct, size guard waived
/// (one per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrafficStats {
    pub drawn: u32,
    /// Targets whose position is placed but off the map area or under the legend strip. Not a
    /// loss: a scope's edge.
    pub off_screen: u32,
    /// Targets the set refused at capacity. Shown on the frame.
    pub refused: u32,
    /// Targets the set refused as not valid. Shown on the frame.
    pub malformed: u32,
    /// Targets whose columns could not be read (cannot happen through [`targets_push`]); counted
    /// so that an impossible state is not an invisible one.
    pub inconsistent: u32,
    pub with_leader: u32,
    pub coasting: u32,
    pub uncertain: u32,
    pub reacquired: u32,
    /// Targets shown with the `CS?` tag: two callsigns for one address at one instant.
    pub cs_conflict: u32,
    /// Uncertain targets whose 95% radius is unknown (NaN or not positive): the diamond and the
    /// tag `R95?` are drawn, no circle. Shown on the data block, so not a loss.
    pub r95_unknown: u32,
    /// Leaders not drawn because the track was not in [0, 3600).
    pub leader_bad_track: u32,
    /// Leaders not drawn because the geodesic or the projection failed.
    pub leader_failed: u32,
    /// 95% circles not drawn although the radius is known (the geodesic or the projection failed).
    pub circle_failed: u32,
    /// History dots not drawn because the position did not project.
    pub trail_dropped: u32,
    /// Altitudes beyond what the data block can show, written `999+` or `-99-`: shown, not lost.
    pub alt_display_clamped: u32,
    /// LOSS: targets whose position is not on the globe (cannot happen through [`targets_push`]);
    /// not the scope's edge.
    pub invalid_position: u32,
    /// LOSS: targets with a valid position that does not project (a pole, a failed projection);
    /// not the scope's edge.
    pub unprojectable: u32,
    /// Texts the frame could not hold (a text buffer clip).
    pub text_clipped: u32,
}

/// The sum of the counts in `s` that are work lost, as opposed to a scope's edge (`off_screen`) or
/// something the screen shows (`r95_unknown`, `cs_conflict`): G6.
pub fn traffic_loss(s: &TrafficStats) -> u64 {
    [
        s.refused,
        s.malformed,
        s.inconsistent,
        s.invalid_position,
        s.unprojectable,
        s.leader_bad_track,
        s.leader_failed,
        s.circle_failed,
        s.trail_dropped,
        s.text_clipped,
    ]
    .iter()
    .fold(0u64, |a, &n| a.saturating_add(u64::from(n)))
}

fn style_check(st: &TrafficStyle) -> Result<(), DisplayError> {
    if !st.leader_min.is_finite() || !(0.0..=MAX_LEADER_MIN).contains(&st.leader_min) {
        return Err(DisplayError::BadView("leader minutes outside 0 to 30"));
    }
    if usize::from(st.trail_n) > MAX_TRAIL {
        return Err(DisplayError::BadView("trail length above the maximum"));
    }
    Ok(())
}

/// Capacities that cannot overflow for up to `max_targets` targets, to be added to the map's
/// ([`caps_add`]). Checked in 128-bit arithmetic against the display limits.
pub fn traffic_caps(max_targets: usize, st: &TrafficStyle) -> Result<ListCaps, DisplayError> {
    style_check(st)?;
    let n = max_targets as u128;
    let rows = |what: &'static str, per: u128, extra: u128| {
        check_limit(what, n * per + extra, MAX_LIST_ROWS)
    };
    Ok(ListCaps {
        // Leader, diamond (4) and X (2).
        lines: rows("traffic lines", 8, 0)?,
        // Centre dot and the trail.
        rects: rows("traffic rects", 1 + MAX_TRAIL as u128, 0)?,
        // The 95% circle and the reacquired mark.
        circles: rows("traffic circles", 2, 0)?,
        polylines: 0,
        poly_vertices: 0,
        texts: rows("traffic texts", MAX_BLOCK_LINES as u128, NOTICES as u128)?,
        text_bytes: check_limit(
            "traffic text bytes",
            n * BYTES_PER_TARGET as u128 + (NOTICES * NOTICE_BYTES) as u128,
            MAX_LIST_TEXT_BYTES,
        )?,
    })
}

/// The sum of two capacity sets, each table checked against its display limit.
pub fn caps_add(a: &ListCaps, b: &ListCaps) -> Result<ListCaps, DisplayError> {
    let rows = |what: &'static str, x: usize, y: usize| {
        check_limit(what, x as u128 + y as u128, MAX_LIST_ROWS)
    };
    Ok(ListCaps {
        lines: rows("lines", a.lines, b.lines)?,
        rects: rows("rects", a.rects, b.rects)?,
        circles: rows("circles", a.circles, b.circles)?,
        polylines: rows("polylines", a.polylines, b.polylines)?,
        poly_vertices: rows("polyline vertices", a.poly_vertices, b.poly_vertices)?,
        texts: rows("texts", a.texts, b.texts)?,
        text_bytes: check_limit(
            "text bytes",
            a.text_bytes as u128 + b.text_bytes as u128,
            MAX_LIST_TEXT_BYTES,
        )?,
    })
}

/// Writes the altitude as the data block shows it into `b`: hundreds of feet, rounded to the
/// nearest (halves away from zero), at least two digits, three for flight levels; `---` when
/// absent. An altitude beyond what three digits (`999`, 99 950 ft) or `-99` (-9 950 ft) can show
/// is written with a marker, `999+` or `-99-`, and the call returns true so that it is counted:
/// a garbage altitude is never a plausible flight level (E6).
pub fn alt_text(b: &mut TextBuf, alt_ft: i32) -> bool {
    if alt_ft == NO_ALT {
        text_push_str(b, "---");
        return false;
    }
    let a = i64::from(alt_ft);
    let h = if a >= 0 {
        (a + 50) / 100
    } else {
        -((-a + 50) / 100)
    };
    if h < 0 {
        text_push_char(b, b'-');
        text_push_dec(b, (-h).min(99) as u64, 2);
        if h < -99 {
            text_push_char(b, b'-');
            return true;
        }
    } else {
        text_push_dec(b, h.min(999) as u64, 3);
        if h > 999 {
            text_push_char(b, b'+');
            return true;
        }
    }
    false
}

/// Writes the ground speed as the data block shows it into `b`: knots over ten, rounded to the
/// nearest, two digits; `--` when absent.
pub fn speed_text(b: &mut TextBuf, gs_dkt: u16) {
    if gs_dkt == NO_U16 {
        text_push_str(b, "--");
        return;
    }
    text_push_dec(b, (u64::from(gs_dkt) + 50) / 100, 2);
}

/// Writes line 1 of the data block into `b`: the callsign without its padding, or the hex address
/// when there is no callsign or `conflict` says two were heard at one instant.
pub fn ident_text(b: &mut TextBuf, callsign: &[u8; 8], addr: u32, conflict: bool) {
    let n = if conflict {
        0
    } else {
        callsign
            .iter()
            .rposition(|&c| c != 0 && c != b' ')
            .map_or(0, |i| i + 1)
    };
    for &c in callsign.iter().take(n) {
        // A character the font lacks draws as its missing-glyph box, so it is seen.
        text_push_char(b, if (0x20..0x7f).contains(&c) { c } else { b'#' });
    }
    if n == 0 {
        text_push_hex(b, addr, 6);
    }
}

/// Writes the state tags of line 3 into `b`, space separated; nothing for a plain tracking
/// target. `r95_unknown` is the uncertain target whose circle could not be drawn.
pub fn tags_text(b: &mut TextBuf, flags: u8, reacquired: bool, r95_unknown: bool) {
    let mut first = true;
    for (on, tag) in [
        (flags & T_COAST != 0, "CST"),
        (flags & T_UNCERTAIN != 0, "UNC"),
        (flags & T_CS_CONFLICT != 0, "CS?"),
        (r95_unknown, "R95?"),
        (reacquired, "REACQ"),
    ] {
        if on {
            if !first {
                text_push_char(b, b' ');
            }
            text_push_str(b, tag);
            first = false;
        }
    }
}

fn add(p: (i32, i32), dx: i32, dy: i32) -> (i32, i32) {
    (p.0.saturating_add(dx), p.1.saturating_add(dy))
}

/// Pixel radius (1/16) of a circle of true radius `metres` about the target: the screen distance
/// from the target to the point `metres` east of it.
fn true_radius16(
    proj: &Projection,
    ell: &Ellipsoid,
    here: (i32, i32),
    at: LatLon,
    metres: f64,
) -> Option<i32> {
    let east = ell.direct(at, 90.0, metres).ok()?.position;
    let (lat, lon) = (to_e7(east.lat())?, to_e7(east.lon())?);
    let p = project(proj, lat, lon)?;
    let d = oa_math::hypot(f64::from(p.0 - here.0), f64::from(p.1 - here.1));
    d.is_finite()
        .then_some(oa_ieee::round(d).min(f64::from(1 << 28)) as i32)
}

fn to_e7(deg: f64) -> Option<i32> {
    let v = oa_ieee::round(deg * 1e7);
    (v.is_finite() && v.abs() <= 1.8e9).then_some(v as i32)
}

/// Where a position is on the map (N6): a position that is not on the globe, or does not project,
/// is its own answer and never the edge of the scope.
// A7.2: cold struct, size guard waived (one per target, on the stack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapPos {
    /// Not a latitude and longitude (outside 90 and 180 degrees).
    Invalid,
    /// A valid position the projection cannot place.
    Unprojectable,
    /// Placed, but off the screen or under the legend strip.
    Off,
    /// On the map area, 1/16 pixel.
    On((i32, i32)),
}

/// Classifies a position against the map area (see [`MapPos`]).
pub fn map_position(proj: &Projection, lat_e7: i32, lon_e7: i32) -> MapPos {
    if !position_ok(lat_e7, lon_e7) {
        return MapPos::Invalid;
    }
    let Some((x, y)) = project(proj, lat_e7, lon_e7) else {
        return MapPos::Unprojectable;
    };
    let (xl, yl) = (i64::from(x), i64::from(y));
    let on = xl >= 0
        && xl <= i64::from(proj.width) * 16
        && yl >= i64::from(proj.top_inset) * 16
        && yl <= i64::from(proj.height) * 16;
    if on {
        MapPos::On((x, y))
    } else {
        MapPos::Off
    }
}

/// True when a position projects onto the map area: on the screen and not under the legend
/// strip. What [`build_traffic`] counts as drawn.
pub fn on_map(proj: &Projection, lat_e7: i32, lon_e7: i32) -> bool {
    matches!(map_position(proj, lat_e7, lon_e7), MapPos::On(_))
}

/// What became of a target's velocity leader.
// A7.2: cold struct, size guard waived (one per target, on the stack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leader {
    /// No velocity, no speed or no leader minutes: nothing to draw, nothing lost.
    Absent,
    /// The track is outside [0, 3600).
    BadTrack,
    /// The geodesic or the projection failed.
    Failed,
    /// The screen end, 1/16 pixel.
    End((i32, i32)),
}

fn leader_end(
    proj: &Projection,
    ell: &Ellipsoid,
    at: Option<LatLon>,
    track_ddeg: u16,
    gs_dkt: u16,
    minutes: f64,
) -> Leader {
    if track_ddeg == NO_U16 || gs_dkt == NO_U16 {
        return Leader::Absent;
    }
    if track_ddeg >= 3600 {
        return Leader::BadTrack;
    }
    // Knots times minutes over sixty is nautical miles.
    let metres = f64::from(gs_dkt) / 10.0 * minutes / 60.0 * 1852.0;
    if metres <= 0.0 {
        return Leader::Absent;
    }
    let end = at
        .and_then(|a| ell.direct(a, f64::from(track_ddeg) / 10.0, metres).ok())
        .map(|d| d.position)
        .and_then(|p| project(proj, to_e7(p.lat())?, to_e7(p.lon())?));
    end.map_or(Leader::Failed, Leader::End)
}

/// One square outline of half-size `h` pixels about `c`.
fn square(l: &mut DisplayList, c: (i32, i32), h: i32) -> Result<(), DisplayError> {
    let (a, b, d, e) = (
        add(c, -px(h), -px(h)),
        add(c, px(h), -px(h)),
        add(c, px(h), px(h)),
        add(c, -px(h), px(h)),
    );
    for (p, q) in [(a, b), (b, d), (d, e), (e, a)] {
        push_line(l, p, q, palette::TARGET, LAYER_SYMBOL)?;
    }
    Ok(())
}

/// One diamond outline of half-size `h` pixels about `c`.
fn diamond(l: &mut DisplayList, c: (i32, i32), h: i32) -> Result<(), DisplayError> {
    let (n, e, s, w) = (
        add(c, 0, -px(h)),
        add(c, px(h), 0),
        add(c, 0, px(h)),
        add(c, -px(h), 0),
    );
    for (p, q) in [(n, e), (e, s), (s, w), (w, n)] {
        push_line(l, p, q, palette::TARGET, LAYER_SYMBOL)?;
    }
    Ok(())
}

/// The scalar columns of target `i`; `None` when a column is shorter than the others (which the
/// private columns make impossible, and which is counted if it ever happens).
// A7.2: cold struct, size guard waived (one per target on the stack, not held in quantity).
#[derive(Clone, Copy)]
struct Scalars {
    lat: i32,
    lon: i32,
    alt: i32,
    addr: u32,
    r95: f32,
    callsign: [u8; 8],
    track: u16,
    gs: u16,
    reacq_age: u16,
    flags: u8,
    trail_len: u8,
}

fn scalars_at(s: &TargetSet, i: usize) -> Option<Scalars> {
    Some(Scalars {
        lat: *s.lat_e7.get(i)?,
        lon: *s.lon_e7.get(i)?,
        alt: *s.alt_ft.get(i)?,
        addr: *s.addr.get(i)?,
        r95: *s.r95_m.get(i)?,
        callsign: *s.callsign.get(i)?,
        track: *s.track_ddeg.get(i)?,
        gs: *s.gs_dkt.get(i)?,
        reacq_age: *s.reacq_age_s.get(i)?,
        flags: *s.flags.get(i)?,
        trail_len: *s.trail_len.get(i)?,
    })
}

/// Pushes the text in `tb` as one line of a block at `at_b` and moves `at_b` down a line; an empty
/// text is skipped, and a clipped one is counted.
fn block_line(
    l: &mut DisplayList,
    at_b: &mut (i32, i32),
    tb: &TextBuf,
    stats: &mut TrafficStats,
) -> Result<(), DisplayError> {
    if text_clipped(tb) {
        stats.text_clipped += 1;
    }
    if text_is_empty(tb) {
        return Ok(());
    }
    push_text(
        l,
        *at_b,
        BLOCK_SIZE,
        ALIGN_LEFT,
        palette::TARGET,
        LAYER_BLOCK,
        text_str(tb),
    )?;
    *at_b = add(*at_b, 0, px(BLOCK_PITCH));
    Ok(())
}

/// Appends the traffic of `set` to `l` (it does not clear the list). `ell` is the WGS-84
/// ellipsoid, built once by the caller. Targets are drawn in set order. On an error the list is
/// left faulted and will not be drawn. Allocates nothing.
pub fn build_traffic(
    l: &mut DisplayList,
    proj: &Projection,
    ell: &Ellipsoid,
    set: &TargetSet,
    st: &TrafficStyle,
) -> Result<TrafficStats, DisplayError> {
    style_check(st)?;
    let mut stats = TrafficStats {
        refused: set.refused_full,
        malformed: set.refused_malformed,
        ..TrafficStats::default()
    };
    let mut tb = text_new();
    for i in 0..set.lat_e7.len() {
        let Some(t) = scalars_at(set, i) else {
            stats.inconsistent += 1;
            continue;
        };
        let c = match map_position(proj, t.lat, t.lon) {
            MapPos::On(c) => c,
            MapPos::Off => {
                stats.off_screen += 1;
                continue;
            }
            MapPos::Invalid => {
                stats.invalid_position += 1;
                continue;
            }
            MapPos::Unprojectable => {
                stats.unprojectable += 1;
                continue;
            }
        };
        let coast = t.flags & T_COAST != 0;
        let uncertain = t.flags & T_UNCERTAIN != 0;
        let conflict = t.flags & T_CS_CONFLICT != 0;
        let reacq = t.reacq_age != NO_REACQ && t.reacq_age < st.reacquired_s;
        let at = LatLon::new(f64::from(t.lat) * 1e-7, f64::from(t.lon) * 1e-7).ok();

        // History dots, 2 px squares, in the dim colour.
        let n = usize::from(t.trail_len).min(usize::from(st.trail_n));
        for k in 0..n {
            let at_k = i * MAX_TRAIL + k;
            let pos = set
                .trail_lat_e7
                .get(at_k)
                .zip(set.trail_lon_e7.get(at_k))
                .and_then(|(&tla, &tlo)| project(proj, tla, tlo));
            match pos {
                Some(p) => push_rect(
                    l,
                    add(p, -px(TRAIL_HALF), -px(TRAIL_HALF)),
                    add(p, px(TRAIL_HALF), px(TRAIL_HALF)),
                    palette::DIM_TEXT,
                    LAYER_TRAIL,
                )?,
                None => stats.trail_dropped += 1,
            }
        }

        // The velocity leader.
        match leader_end(proj, ell, at, t.track, t.gs, st.leader_min) {
            Leader::End(end) => {
                push_line(l, c, end, palette::TARGET, LAYER_LEADER)?;
                stats.with_leader += 1;
            }
            Leader::BadTrack => stats.leader_bad_track += 1,
            Leader::Failed => stats.leader_failed += 1,
            Leader::Absent => {}
        }

        // The position symbol.
        let half = if uncertain {
            diamond(l, c, DIAMOND_HALF)?;
            DIAMOND_HALF
        } else {
            square(l, c, SQUARE_HALF)?;
            SQUARE_HALF
        };
        if coast {
            push_line(
                l,
                add(c, -px(half), -px(half)),
                add(c, px(half), px(half)),
                palette::TARGET,
                LAYER_SYMBOL,
            )?;
            push_line(
                l,
                add(c, -px(half), px(half)),
                add(c, px(half), -px(half)),
                palette::TARGET,
                LAYER_SYMBOL,
            )?;
            stats.coasting += 1;
        } else {
            push_rect(
                l,
                add(c, -8, -8),
                add(c, 8, 8),
                palette::TARGET,
                LAYER_SYMBOL,
            )?;
        }
        // An uncertain target without its circle says so on the data block (`R95?`), whether
        // the radius was unknown or the circle could not be placed.
        let mut no_circle = false;
        if uncertain {
            stats.uncertain += 1;
            if t.r95.is_finite() && t.r95 > 0.0 {
                let r = at.and_then(|a| true_radius16(proj, ell, c, a, f64::from(t.r95)));
                match r {
                    Some(r) => push_circle(l, c, r, palette::TARGET, LAYER_SYMBOL)?,
                    None => {
                        stats.circle_failed += 1;
                        no_circle = true;
                    }
                }
            } else {
                stats.r95_unknown += 1;
                no_circle = true;
            }
        }
        if reacq {
            push_circle(l, c, px(REACQ_RADIUS), palette::ALERT, LAYER_SYMBOL)?;
            stats.reacquired += 1;
        }

        // The data block, at a fixed offset (LS-401: no overlap avoidance yet).
        let mut at_b = add(c, px(BLOCK_DX), px(BLOCK_DY));
        text_clear(&mut tb);
        ident_text(&mut tb, &t.callsign, t.addr, conflict);
        block_line(l, &mut at_b, &tb, &mut stats)?;
        text_clear(&mut tb);
        if alt_text(&mut tb, t.alt) {
            stats.alt_display_clamped += 1;
        }
        text_push_char(&mut tb, b' ');
        speed_text(&mut tb, t.gs);
        block_line(l, &mut at_b, &tb, &mut stats)?;
        text_clear(&mut tb);
        tags_text(&mut tb, t.flags, reacq, no_circle);
        block_line(l, &mut at_b, &tb, &mut stats)?;
        if conflict {
            stats.cs_conflict += 1;
        }
        stats.drawn += 1;
    }
    let h16 = i32::try_from(i64::from(proj.height) * 16).unwrap_or(i32::MAX);
    if set.refused_full > 0 {
        text_clear(&mut tb);
        text_push_str(&mut tb, "TARGETS NOT DRAWN: ");
        text_push_dec(&mut tb, u64::from(set.refused_full), 0);
        text_push_str(&mut tb, " (DISPLAY FULL AT ");
        text_push_dec(&mut tb, set.capacity as u64, 0);
        text_push_char(&mut tb, b')');
        notice(l, h16, 0, &tb, &mut stats)?;
    }
    if set.refused_malformed > 0 {
        text_clear(&mut tb);
        text_push_str(&mut tb, "TARGETS MALFORMED: ");
        text_push_dec(&mut tb, u64::from(set.refused_malformed), 0);
        text_push_str(&mut tb, " (REFUSED)");
        notice(l, h16, 1, &tb, &mut stats)?;
    }
    Ok(stats)
}

/// One notice line, `row` lines above the first, at the bottom left in the alert colour.
fn notice(
    l: &mut DisplayList,
    h16: i32,
    row: i32,
    tb: &TextBuf,
    stats: &mut TrafficStats,
) -> Result<(), DisplayError> {
    if text_clipped(tb) {
        stats.text_clipped += 1;
    }
    let at_n = (
        px(NOTICE_X),
        h16.saturating_sub(px(NOTICE_FROM_BOTTOM + row * NOTICE_PITCH)),
    );
    push_text(
        l,
        at_n,
        NOTICE_SIZE,
        ALIGN_LEFT,
        palette::ALERT,
        LAYER_BLOCK,
        text_str(tb),
    )
}

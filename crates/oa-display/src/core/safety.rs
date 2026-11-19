//! The safety-net indications (**core**, B1, B2): what the screen says about the state of the
//! nets, the inhibitions in force, the surveillance feeds and the freshness of the picture. A pure
//! function of read-only inputs (M5 design rules 1 and 2, ND-04); it appends to the list a map and
//! traffic build already made and does not clear it.
//!
//! # The rule that keeps every existing frame unchanged
//!
//! The banner is drawn whenever something is not well, and **nothing at all is drawn when every
//! net is running, no inhibition is in force, every feed is live and the picture is fresh**: no
//! primitive, no empty box. A frame with all well is the frame the map and traffic alone make,
//! byte for byte.
//!
//! # The banner
//!
//! One line per condition, in the alert colour, in a box with an outline at the top left under
//! the legend strip, above everything else. It cannot be dismissed: it is a function of the state
//! and nothing else, so it lasts exactly as long as the condition.
//!
//! * `PICTURE FROZEN <age>` when the newest track update is older than the configured limit (age
//!   at or over the limit; one nanosecond under it, nothing), or when there has been none.
//! * One line per net that is not plainly running: name, state, reason, area, count, age of the
//!   state, and the number of inhibitions in force.
//! * One line per feed that is not live: `FEED <name> SLOW <age>` or `FEED <name> LOST <age>`.
//! * `INHIBITION AREAS NOT DRAWN: N` when an area has a vertex that does not project.
//!
//! # Inhibition areas
//!
//! Every inhibition in force is drawn as a closed outline in the alert colour with a label at its
//! highest vertex: the net, the end time in UTC, the author and the reason cut to 16 characters.
//! Edges are straight on the glass between the projected vertices (the same straight edges in
//! latitude and longitude that suppression tests, see `oa_safety::geom`). Labels do not avoid
//! one another or the data blocks (LS-401).
//!
//! # Budgets (A7.1)
//!
//! The list capacities are [`safety_caps`], added to the map's and the traffic's with
//! `traffic::caps_add`. The work of a build is charged beforehand by [`safety_work_charge`]
//! against [`MAX_SAFETY_WORK`]. A scene's own charge (`scene::MAX_SCENE_WORK`, 2^40) is not raised:
//! the safety layer adds a few tens of thousands at most.

use crate::core::font::{text_width_units, CAP_HEIGHT};
use crate::core::list::{
    begin_polyline_w, push_line, push_rect, push_text, push_vertex, DisplayError, DisplayList,
    ListCaps, ALIGN_LEFT,
};
use crate::core::palette;
use crate::core::project::{project, Projection};
use crate::core::scene::LEGEND_STRIP_PX;
use crate::core::text::{
    text_clear, text_clipped, text_new, text_push_char, text_push_dec, text_push_str, text_str,
    TextBuf, TEXT_BYTES,
};
use oa_safety::geom::MAX_VERTICES;
use oa_safety::inhibit::{
    in_force, inhibits_slice, text_of, InhibitTable, Inhibition, INHIBIT_CAP,
};
use oa_safety::net::{
    net_name, net_statuses, reason_text, status_needs_banner, Area, NetState, NetStatus, StateTable,
};

/// Inhibition outlines and labels share the map's text layer: above the map, below the traffic
/// and the legend strip, so the strip still covers them and a target is never hidden by one.
pub const LAYER_INHIBIT: u8 = 6;
/// The banner's box and its text, above every other layer.
pub const LAYER_BANNER_BG: u8 = 14;
pub const LAYER_BANNER: u8 = 15;

/// Most surveillance feeds the display lists. A7.1: ND-04 names the surveillance feeds of a
/// facility (primary and secondary radar, ADS-B, a second site); eight is more than a facility
/// has, and a configuration with more is refused rather than cut.
pub const MAX_FEEDS: usize = 8;
/// Bytes of a feed's name.
pub const FEED_NAME_BYTES: usize = 8;
/// Banner lines: the frozen picture, the four nets, the feeds and the unplaced-areas notice.
pub const MAX_BANNER_LINES: usize = 1 + oa_safety::net::NET_COUNT + MAX_FEEDS + 1;

const BANNER_SIZE: u8 = 12;
const BANNER_PITCH: i32 = 16;
const BANNER_X: i32 = 8;
const BANNER_PAD: i32 = 4;
const LABEL_SIZE: u8 = 10;
/// Longest reason text in an area's label, characters.
const LABEL_REASON_CHARS: usize = 16;

/// Most the work charge of one build can be: every inhibition's vertices projected and drawn
/// once, and every banner line and label written once. A7.1: the figure is the sum of these
/// bounds, 64 * (32 + 1) vertices plus 78 texts of 96 bytes, about 10 thousand, not a measured
/// time; the build refuses a state whose charge is over it, which no valid state is.
pub const MAX_SAFETY_WORK: usize =
    INHIBIT_CAP * (MAX_VERTICES + 1) + (INHIBIT_CAP + MAX_BANNER_LINES) * TEXT_BYTES;

/// The state of one surveillance feed as the display is told it (the shell classifies it from the
/// feed's own monitoring; this is plain data).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedState {
    Live,
    /// Late: no message for `age_ms`, but not yet lost.
    Slow {
        age_ms: u32,
    },
    /// Lost: no message for `age_ms`.
    Lost {
        age_ms: u32,
    },
}

/// One feed: its name (ASCII, zero padded) and state.
// A7.2: cold struct, size guard waived (at most eight per frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedStatus {
    pub name: [u8; FEED_NAME_BYTES],
    pub state: FeedState,
}

/// Everything the safety indications read. Plain data, borrowed, read only.
// A7.2: cold struct, size guard waived (one per frame).
#[derive(Clone, Copy, Debug)]
pub struct SafetyScene<'a> {
    pub states: &'a StateTable,
    pub inhibits: &'a InhibitTable,
    pub now_ns: i64,
    pub feeds: &'a [FeedStatus],
    /// Time of the newest track update, or `None` if there has been none.
    pub newest_track_ns: Option<i64>,
    /// The picture is frozen when the newest update is at least this old. From configuration, no
    /// default; positive.
    pub frozen_limit_ns: i64,
}

/// What a safety build drew. A7.2: cold struct, size guard waived (one per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SafetyStats {
    pub banner_lines: u32,
    pub nets_shown: u32,
    pub feeds_shown: u32,
    pub frozen: bool,
    pub inhibitions_drawn: u32,
    /// Areas not drawn because a vertex does not project. Said on the banner.
    pub inhibitions_unplaced: u32,
    pub text_clipped: u32,
}

/// List capacities for every safety primitive the build can add, to be added to the map's and the
/// traffic's (`traffic::caps_add`).
pub fn safety_caps() -> ListCaps {
    let texts = INHIBIT_CAP + MAX_BANNER_LINES;
    ListCaps {
        // The outline of the banner box.
        lines: 4,
        // The banner's backdrop.
        rects: 1,
        circles: 0,
        polylines: INHIBIT_CAP,
        // Each outline repeats its first vertex to close.
        poly_vertices: INHIBIT_CAP * (MAX_VERTICES + 1),
        texts,
        text_bytes: texts * TEXT_BYTES,
    }
}

/// The work a build of `s` is charged: vertices of the inhibitions held, and the texts it can
/// write, bounded by what the tables hold (C7), before any is done.
pub fn safety_work_charge(s: &SafetyScene<'_>) -> u128 {
    let held = inhibits_slice(s.inhibits).len();
    let verts: usize = inhibits_slice(s.inhibits)
        .iter()
        .map(|i| usize::from(i.area.n) + 1)
        .sum();
    (verts + (held + MAX_BANNER_LINES) * TEXT_BYTES) as u128
}

/// Civil date and time of day (UTC) of a time in nanoseconds since the epoch: year, month, day,
/// hour, minute. Integer arithmetic (the days-from-civil method).
pub fn utc_parts(ns: i64) -> (i64, u8, u8, u8, u8) {
    let secs = i64::div_euclid(ns, 1_000_000_000);
    let days = i64::div_euclid(secs, 86_400);
    let rem = i64::rem_euclid(secs, 86_400);
    let z = days + 719_468;
    let era = i64::div_euclid(z, 146_097);
    let doe = i64::rem_euclid(z, 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        u8::try_from(m).unwrap_or(0),
        u8::try_from(d).unwrap_or(0),
        u8::try_from(rem / 3600).unwrap_or(0),
        u8::try_from(rem % 3600 / 60).unwrap_or(0),
    )
}

fn push_signed(b: &mut TextBuf, v: i64) {
    if v < 0 {
        text_push_char(b, b'-');
    }
    text_push_dec(b, v.unsigned_abs(), 0);
}

fn push_dec2(b: &mut TextBuf, v: u8) {
    text_push_dec(b, u64::from(v), 2);
}

/// Largest number a banner line writes in full: beyond it the line shows the cap and a `+`, so
/// that the longest line (the longest reason, an area, a count, an age and the inhibitions) always
/// fits the 96-byte text buffer and a line is never cut mid-way.
const SHOW_MAX: u64 = 999_999;

fn push_capped(b: &mut TextBuf, v: u64, cap: u64) {
    text_push_dec(b, v.min(cap), 0);
    if v > cap {
        text_push_char(b, b'+');
    }
}

/// Appends `ns` as seconds with one decimal and an `S` (negative reads as zero).
fn push_tenths(b: &mut TextBuf, ns: i64) {
    let ns = ns.max(0);
    text_push_dec(b, (ns / 1_000_000_000) as u64, 0);
    text_push_char(b, b'.');
    text_push_dec(b, (ns % 1_000_000_000 / 100_000_000) as u64, 0);
    text_push_char(b, b'S');
}

/// Appends an age as whole seconds and an `S`, or `--` when unknown.
fn push_age(b: &mut TextBuf, age_ns: Option<i64>) {
    match age_ns {
        Some(a) => {
            push_capped(b, (a.max(0) / 1_000_000_000) as u64, SHOW_MAX);
            text_push_char(b, b'S');
        }
        None => text_push_str(b, "--"),
    }
}

/// The banner line of a net that is not plainly running.
pub fn net_line(b: &mut TextBuf, s: &NetStatus) {
    text_clear(b);
    text_push_str(b, net_name(s.net));
    match s.state {
        NetState::Running => text_push_str(b, " RUNNING"),
        NetState::Inhibited => {
            text_push_str(b, " INHIBITED: ");
            push_capped(b, u64::from(s.inhibitions), SHOW_MAX);
            text_push_str(b, " IN FORCE");
        }
        NetState::Degraded {
            reason,
            area,
            count,
        } => {
            text_push_str(b, " DEGRADED: ");
            text_push_str(b, reason_text(reason));
            match area {
                Area::Whole => text_push_str(b, ", WHOLE"),
                Area::Polygon(id) => {
                    text_push_str(b, ", AREA ");
                    push_capped(b, u64::from(id), SHOW_MAX);
                }
            }
            text_push_str(b, ", COUNT ");
            push_capped(b, u64::from(count), SHOW_MAX);
        }
        NetState::Unavailable { reason } => {
            text_push_str(b, " UNAVAILABLE: ");
            text_push_str(b, reason_text(reason));
        }
    }
    text_push_str(b, ", AGE ");
    push_age(b, s.age_ns);
    if s.inhibitions > 0 && s.state != NetState::Inhibited {
        text_push_str(b, ", INH ");
        push_capped(b, u64::from(s.inhibitions), 999);
    }
}

/// The banner line of a feed that is not live (empty for a live one).
pub fn feed_line(b: &mut TextBuf, f: &FeedStatus) {
    text_clear(b);
    let (word, age_ms) = match f.state {
        FeedState::Live => return,
        FeedState::Slow { age_ms } => (" SLOW ", age_ms),
        FeedState::Lost { age_ms } => (" LOST ", age_ms),
    };
    text_push_str(b, "FEED ");
    text_push_str(b, text_of(&f.name));
    text_push_str(b, word);
    push_tenths(b, i64::from(age_ms) * 1_000_000);
}

/// The banner line of a frozen picture.
pub fn frozen_line(b: &mut TextBuf, age_ns: Option<i64>) {
    text_clear(b);
    text_push_str(b, "PICTURE FROZEN");
    match age_ns {
        Some(a) => {
            text_push_char(b, b' ');
            push_tenths(b, a);
        }
        None => text_push_str(b, ", NO TRACK UPDATE"),
    }
}

/// The label of an inhibition area.
pub fn inhibit_label(b: &mut TextBuf, i: &Inhibition) {
    text_clear(b);
    text_push_str(b, net_name(i.net));
    text_push_str(b, " UNTIL ");
    let (y, mo, d, h, mi) = utc_parts(i.end_ns);
    push_signed(b, y);
    text_push_char(b, b'-');
    push_dec2(b, mo);
    text_push_char(b, b'-');
    push_dec2(b, d);
    text_push_char(b, b' ');
    push_dec2(b, h);
    text_push_char(b, b':');
    push_dec2(b, mi);
    text_push_str(b, "Z BY ");
    text_push_str(b, text_of(&i.author));
    text_push_str(b, ": ");
    let reason = text_of(&i.reason);
    text_push_str(b, reason.get(..LABEL_REASON_CHARS).unwrap_or(reason));
}

fn px(n: i32) -> i32 {
    n.saturating_mul(16)
}

/// Appends the safety indications to `l`: the inhibition areas in force, then the banner when
/// anything is not well. Draws nothing when all is well. On an error the list is left faulted.
/// Allocates nothing.
pub fn build_safety(
    l: &mut DisplayList,
    proj: &Projection,
    s: &SafetyScene<'_>,
) -> Result<SafetyStats, DisplayError> {
    if s.frozen_limit_ns <= 0 {
        return Err(DisplayError::BadView("frozen limit must be positive"));
    }
    if s.feeds.len() > MAX_FEEDS {
        return Err(DisplayError::BadView("more feeds than the display lists"));
    }
    let charge = safety_work_charge(s);
    if charge > MAX_SAFETY_WORK as u128 {
        return Err(DisplayError::TooLarge {
            what: "safety work",
            requested: charge,
            limit: MAX_SAFETY_WORK,
        });
    }
    let mut stats = SafetyStats::default();
    let mut tb = text_new();

    // Inhibition areas in force.
    for i in inhibits_slice(s.inhibits) {
        if !in_force(i.start_ns, i.end_ns, s.now_ns) {
            continue;
        }
        let n = usize::from(i.area.n);
        let mut pts = [(0i32, 0i32); MAX_VERTICES];
        let mut placed = n > 0;
        for (k, slot) in pts.iter_mut().enumerate().take(n) {
            let p = i
                .area
                .lat_e7
                .get(k)
                .zip(i.area.lon_e7.get(k))
                .and_then(|(&la, &lo)| project(proj, la, lo));
            match p {
                Some(p) => *slot = p,
                None => placed = false,
            }
        }
        if !placed {
            stats.inhibitions_unplaced += 1;
            continue;
        }
        begin_polyline_w(l, palette::ALERT, LAYER_INHIBIT, 1)?;
        for &(x, y) in pts.iter().take(n) {
            push_vertex(l, x, y)?;
        }
        if let Some(&(x, y)) = pts.first() {
            push_vertex(l, x, y)?;
        }
        // The label at the highest vertex (smallest y, then smallest x).
        let top = pts
            .iter()
            .take(n)
            .min_by_key(|&&(x, y)| (y, x))
            .copied()
            .unwrap_or((0, 0));
        inhibit_label(&mut tb, i);
        if text_clipped(&tb) {
            stats.text_clipped += 1;
        }
        push_text(
            l,
            (top.0.saturating_add(px(4)), top.1.saturating_add(px(4))),
            LABEL_SIZE,
            ALIGN_LEFT,
            palette::ALERT,
            LAYER_INHIBIT,
            text_str(&tb),
        )?;
        stats.inhibitions_drawn += 1;
    }

    // The banner lines, in order: frozen picture, nets, feeds, unplaced areas.
    let mut lines = [text_new(); MAX_BANNER_LINES];
    let mut n_lines = 0usize;
    let mut add = |lines: &mut [TextBuf; MAX_BANNER_LINES], b: &TextBuf, st: &mut SafetyStats| {
        if let Some(slot) = lines.get_mut(n_lines) {
            *slot = *b;
            n_lines += 1;
            if text_clipped(b) {
                st.text_clipped += 1;
            }
        }
    };
    let age = s.newest_track_ns.map(|t| s.now_ns.saturating_sub(t).max(0));
    let frozen = age.is_none_or(|a| a >= s.frozen_limit_ns);
    if frozen {
        frozen_line(&mut tb, age);
        add(&mut lines, &tb, &mut stats);
        stats.frozen = true;
    }
    for ns in net_statuses(s.states, s.inhibits, s.now_ns) {
        if status_needs_banner(&ns) {
            net_line(&mut tb, &ns);
            add(&mut lines, &tb, &mut stats);
            stats.nets_shown += 1;
        }
    }
    for f in s.feeds {
        if f.state != FeedState::Live {
            feed_line(&mut tb, f);
            add(&mut lines, &tb, &mut stats);
            stats.feeds_shown += 1;
        }
    }
    if stats.inhibitions_unplaced > 0 {
        text_clear(&mut tb);
        text_push_str(&mut tb, "INHIBITION AREAS NOT DRAWN: ");
        text_push_dec(&mut tb, u64::from(stats.inhibitions_unplaced), 0);
        add(&mut lines, &tb, &mut stats);
    }
    stats.banner_lines = u32::try_from(n_lines).unwrap_or(u32::MAX);
    if n_lines == 0 {
        return Ok(stats);
    }

    // The box: as wide as its widest line, an outline in the alert colour on the background.
    let size = i64::from(BANNER_SIZE);
    let widest = lines
        .iter()
        .take(n_lines)
        .map(|b| text_width_units(text_str(b).len()))
        .max()
        .unwrap_or(0);
    let w = i32::try_from(i64::from(widest) * size / i64::from(CAP_HEIGHT)).unwrap_or(i32::MAX);
    let rows = i32::try_from(n_lines).unwrap_or(i32::MAX);
    let x0 = px(BANNER_X - BANNER_PAD);
    let y0 = px(LEGEND_STRIP_PX as i32 + 8 - BANNER_PAD);
    let x1 = x0.saturating_add(px(w.saturating_add(2 * BANNER_PAD)));
    let y1 = y0.saturating_add(px(rows
        .saturating_mul(BANNER_PITCH)
        .saturating_add(BANNER_PAD)));
    push_rect(l, (x0, y0), (x1, y1), palette::BACKGROUND, LAYER_BANNER_BG)?;
    for (p, q) in [
        ((x0, y0), (x1, y0)),
        ((x1, y0), (x1, y1)),
        ((x1, y1), (x0, y1)),
        ((x0, y1), (x0, y0)),
    ] {
        push_line(l, p, q, palette::ALERT, LAYER_BANNER)?;
    }
    let mut y = px(LEGEND_STRIP_PX as i32 + 8);
    for b in lines.iter().take(n_lines) {
        push_text(
            l,
            (px(BANNER_X), y),
            BANNER_SIZE,
            ALIGN_LEFT,
            palette::ALERT,
            LAYER_BANNER,
            text_str(b),
        )?;
        y = y.saturating_add(px(BANNER_PITCH));
    }
    Ok(stats)
}

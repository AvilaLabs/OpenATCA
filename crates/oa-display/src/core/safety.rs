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
//! and nothing else, so it lasts exactly as long as the condition. The box has an **opaque**
//! background (a safety banner must be legible over any map), so it hides what is under it; the
//! simpler of the two remedies is taken: targets whose symbol is under the box are counted on the
//! banner, `TARGETS UNDER BANNER: n`. The target set is a required input (an empty set is a real
//! value that says there are none), so the count never silently reads nothing.
//!
//! Lines, in priority order:
//! * `PICTURE FROZEN <age>` when the newest track update is at least the configured limit old
//!   (an age one nanosecond under the limit shows nothing), or when there has been none;
//!   `PICTURE TIME INCONSISTENT` when the newest update is dated after now.
//! * `SAFETY RECORD INCOMPLETE: ...` when the record has lost something: event log overflow,
//!   suppressions lost to a full log, expired inhibitions waiting for their review event,
//!   refused state reports or clipped texts (counts of each that is not zero).
//! * One line per net that is not plainly running: name, state, reason, area, count, age of the
//!   state, and the number of inhibitions in force. A net whose suppressions were lost to a full
//!   log reads degraded (`SUPPRESSION LOG FULL`).
//! * One line per feed that is not live: `FEED <name> SLOW <age>` or `FEED <name> LOST <age>`.
//! * `INHIBITION AREAS NOT DRAWN: N` when an area has a vertex that does not project.
//!
//! If the lines do not fit the frame, the last visible line reads `+N MORE STATES NOT SHOWN`.
//! A build that fails is an alarm (see [`build_safety`]). A line is longer than a small frame is
//! wide on some screens; the rasterizer clips it at the frame edge, and the frame is meant for
//! the facility display sizes.
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
use crate::core::traffic::{targets_columns, TargetSet};
use oa_safety::geom::MAX_VERTICES;
use oa_safety::inhibit::{
    events_overflow, in_force, inhibits_expired_waiting, inhibits_slice, suppressed_overflow,
    text_of, InhibitTable, Inhibition, INHIBIT_CAP,
};
use oa_safety::net::{
    net_name, net_statuses, reason_text, state_rejected, status_needs_banner, Area, NetState,
    NetStatus, StateTable,
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
/// Banner lines: the picture, the record, the four nets, the feeds, the unplaced-areas notice, the
/// targets under the banner and the "more states" line that replaces a last one.
pub const MAX_BANNER_LINES: usize = 1 + 1 + oa_safety::net::NET_COUNT + MAX_FEEDS + 1 + 1;

const BANNER_SIZE: u8 = 12;
/// Space left at the bottom of the frame for the traffic notices (`TARGETS NOT DRAWN` and the like).
const BOTTOM_RESERVE: i32 = 48;
const BANNER_PITCH: i32 = 16;
const BANNER_X: i32 = 8;
const BANNER_PAD: i32 = 4;
const LABEL_SIZE: u8 = 10;
/// Longest reason text in an area's label, characters.
const LABEL_REASON_CHARS: usize = 16;

/// Most the work charge of one build can be: every inhibition's vertices projected and drawn
/// once, and every banner line and label written once. A7.1: the figure is the sum of these
/// bounds, 64 * (32 + 1) vertices plus 80 texts of 128 bytes, about 10 thousand, not a measured
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
    /// The targets drawn: those under the opaque banner are counted on it. Required.
    pub targets: &'a TargetSet,
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
    /// The newest track update is dated after now.
    pub time_inconsistent: bool,
    /// A loss of the safety record is on the banner.
    pub record_incomplete: bool,
    /// Banner lines left out for want of room (said on the last line).
    pub banner_hidden: u32,
    /// Targets whose symbol is under the opaque banner box.
    pub targets_under_banner: u32,
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
/// fits the text buffer and a line is never cut mid-way.
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

/// The banner's lines in priority order, and how many there are.
// A7.2: cold struct, size guard waived (one per frame, on the stack).
struct Banner {
    lines: [TextBuf; MAX_BANNER_LINES],
    n: usize,
}

fn banner_add(b: &mut Banner, tb: &TextBuf, st: &mut SafetyStats) {
    if let Some(slot) = b.lines.get_mut(b.n) {
        *slot = *tb;
        b.n += 1;
        if text_clipped(tb) {
            st.text_clipped += 1;
        }
    }
}

/// Inserts `tb` at line `at` (or at the end), shifting the rest down.
fn banner_insert(b: &mut Banner, at: usize, tb: &TextBuf, st: &mut SafetyStats) {
    if b.n >= MAX_BANNER_LINES {
        return;
    }
    let at = usize::min(at, b.n);
    b.lines.copy_within(at..b.n, at + 1);
    if let Some(slot) = b.lines.get_mut(at) {
        *slot = *tb;
        b.n += 1;
        if text_clipped(tb) {
            st.text_clipped += 1;
        }
    }
}

/// The net lines, in net order, for every net that is not plainly running.
fn collect_nets(b: &mut Banner, tb: &mut TextBuf, s: &SafetyScene<'_>, st: &mut SafetyStats) {
    for ns in net_statuses(s.states, s.inhibits, s.now_ns) {
        if status_needs_banner(&ns) {
            net_line(tb, &ns);
            banner_add(b, tb, st);
            st.nets_shown += 1;
        }
    }
}

/// Rows of banner text that fit the frame between the legend strip and the bottom notices.
fn rows_that_fit(proj: &Projection) -> usize {
    let free = i64::from(proj.height) - i64::from(LEGEND_STRIP_PX) - 8 - i64::from(BOTTOM_RESERVE);
    usize::try_from((free / i64::from(BANNER_PITCH)).max(1)).unwrap_or(1)
}

/// The box of `n` lines whose widest has `widest_units` grid units, 1/16 pixel: left, top, right,
/// bottom.
fn box_of(n: usize, widest_units: i32) -> (i32, i32, i32, i32) {
    let w = i32::try_from(i64::from(widest_units) * i64::from(BANNER_SIZE) / i64::from(CAP_HEIGHT))
        .unwrap_or(i32::MAX);
    let rows = i32::try_from(n).unwrap_or(i32::MAX);
    let x0 = px(BANNER_X - BANNER_PAD);
    let y0 = px(LEGEND_STRIP_PX as i32 + 8 - BANNER_PAD);
    (
        x0,
        y0,
        x0.saturating_add(px(w.saturating_add(2 * BANNER_PAD))),
        y0.saturating_add(px(rows
            .saturating_mul(BANNER_PITCH)
            .saturating_add(BANNER_PAD))),
    )
}

fn widest(b: &Banner) -> i32 {
    b.lines
        .iter()
        .take(b.n)
        .map(|t| text_width_units(text_str(t).len()))
        .max()
        .unwrap_or(0)
}

/// Targets whose symbol is under the box (opaque, so they are hidden by it).
fn targets_under(set: &TargetSet, proj: &Projection, bx: (i32, i32, i32, i32)) -> u32 {
    let c = targets_columns(set);
    let n = c
        .lat_e7
        .iter()
        .zip(c.lon_e7)
        .filter_map(|(&la, &lo)| project(proj, la, lo))
        .filter(|&(x, y)| x >= bx.0 && x <= bx.2 && y >= bx.1 && y <= bx.3)
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Clamps the banner to the frame, counts what the box hides and draws it.
fn banner_draw(
    l: &mut DisplayList,
    proj: &Projection,
    b: &mut Banner,
    tb: &mut TextBuf,
    targets: &TargetSet,
    st: &mut SafetyStats,
) -> Result<(), DisplayError> {
    let fit = rows_that_fit(proj);
    if b.n > fit {
        // The last visible line says how many states are not shown: never silent.
        let hidden = b.n - (fit - 1);
        b.n = fit - 1;
        text_clear(tb);
        text_push_char(tb, b'+');
        text_push_dec(tb, hidden as u64, 0);
        text_push_str(tb, " MORE STATES NOT SHOWN");
        banner_add(b, tb, st);
        st.banner_hidden = u32::try_from(hidden).unwrap_or(u32::MAX);
    }
    if b.n == 0 {
        return Ok(());
    }
    // Targets under the box, counted against the box with the counting line itself.
    let under = targets_under(
        targets,
        proj,
        box_of(b.n + 1, widest(b).max(text_width_units(30))),
    );
    if under > 0 && b.n < fit {
        text_clear(tb);
        text_push_str(tb, "TARGETS UNDER BANNER: ");
        push_capped(tb, u64::from(under), SHOW_MAX);
        banner_add(b, tb, st);
    }
    st.targets_under_banner = targets_under(targets, proj, box_of(b.n, widest(b)));
    st.banner_lines = u32::try_from(b.n).unwrap_or(u32::MAX);
    let (x0, y0, x1, y1) = box_of(b.n, widest(b));
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
    for t in b.lines.iter().take(b.n) {
        push_text(
            l,
            (px(BANNER_X), y),
            BANNER_SIZE,
            ALIGN_LEFT,
            palette::ALERT,
            LAYER_BANNER,
            text_str(t),
        )?;
        y = y.saturating_add(px(BANNER_PITCH));
    }
    Ok(())
}

/// The line for the losses the safety record has suffered: events, suppressions, expiries waiting,
/// refused reports and clipped texts. Empty (and false) when there are none.
fn record_line(tb: &mut TextBuf, s: &SafetyScene<'_>, clipped: u32) -> bool {
    text_clear(tb);
    text_push_str(tb, "SAFETY RECORD INCOMPLETE:");
    let parts = [
        (" EVT ", events_overflow(s.inhibits)),
        (" SUPP ", suppressed_overflow(s.inhibits)),
        (
            " EXP ",
            u64::from(inhibits_expired_waiting(s.inhibits, s.now_ns)),
        ),
        (" REJ ", u64::from(state_rejected(s.states))),
        (" CLIP ", u64::from(clipped)),
    ];
    let mut any = false;
    for (name, n) in parts {
        if n > 0 {
            text_push_str(tb, if any { "," } else { "" });
            text_push_str(tb, name);
            push_capped(tb, n, SHOW_MAX);
            any = true;
        }
    }
    any
}

/// A code for each reason a build can fail, shown on the banner.
fn error_code(e: &DisplayError) -> u64 {
    match e {
        DisplayError::BadView(_) => 1,
        DisplayError::TooLarge { .. } => 2,
        DisplayError::Full { .. } => 3,
        _ => 4,
    }
}

fn build_inner(
    l: &mut DisplayList,
    proj: &Projection,
    s: &SafetyScene<'_>,
    stats: &mut SafetyStats,
) -> Result<(), DisplayError> {
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

    // The banner lines, in priority order: the picture, the record, nets, feeds, unplaced areas.
    let mut b = Banner {
        lines: [text_new(); MAX_BANNER_LINES],
        n: 0,
    };
    match s.newest_track_ns {
        // A track update dated after now is never "fresh": the clock or the data is wrong.
        Some(t) if t > s.now_ns => {
            text_clear(&mut tb);
            text_push_str(&mut tb, "PICTURE TIME INCONSISTENT");
            banner_add(&mut b, &tb, stats);
            stats.time_inconsistent = true;
        }
        newest => {
            let age = newest.map(|t| s.now_ns.saturating_sub(t));
            // An age equal to the limit is frozen; one nanosecond under it is not.
            if age.is_none_or(|a| a >= s.frozen_limit_ns) {
                frozen_line(&mut tb, age);
                banner_add(&mut b, &tb, stats);
                stats.frozen = true;
            }
        }
    }
    let picture_lines = b.n;
    collect_nets(&mut b, &mut tb, s, stats);
    for f in s.feeds {
        if f.state != FeedState::Live {
            feed_line(&mut tb, f);
            banner_add(&mut b, &tb, stats);
            stats.feeds_shown += 1;
        }
    }
    if stats.inhibitions_unplaced > 0 {
        text_clear(&mut tb);
        text_push_str(&mut tb, "INHIBITION AREAS NOT DRAWN: ");
        text_push_dec(&mut tb, u64::from(stats.inhibitions_unplaced), 0);
        banner_add(&mut b, &tb, stats);
    }
    // The record line comes last in the build (it counts the clips of every other line) and goes
    // second in the banner.
    if record_line(&mut tb, s, stats.text_clipped) {
        banner_insert(&mut b, picture_lines, &tb, stats);
        stats.record_incomplete = true;
    }
    banner_draw(l, proj, &mut b, &mut tb, s.targets, stats)
}

/// Appends the safety indications to `l`: the inhibition areas in force, then the banner when
/// anything is not well. Draws nothing when all is well. Allocates nothing.
///
/// **An `Err` is an alarm, not a missing frame.** If the build fails before it has drawn anything
/// into the list (a bad configuration, too much work), the net lines that can be drawn are drawn
/// with a `SAFETY DISPLAY ERROR <code>` line, and the error is still returned. If the list itself
/// is faulted nothing can be drawn and the caller's frame is not shown. In both cases the shell
/// must treat `Err` as an alarm and say so by another route (aural alert, the supervisor).
pub fn build_safety(
    l: &mut DisplayList,
    proj: &Projection,
    s: &SafetyScene<'_>,
) -> Result<SafetyStats, DisplayError> {
    let mut stats = SafetyStats::default();
    match build_inner(l, proj, s, &mut stats) {
        Ok(()) => Ok(stats),
        Err(e) => {
            if !l.faulted {
                let mut st = SafetyStats::default();
                let mut tb = text_new();
                let mut b = Banner {
                    lines: [text_new(); MAX_BANNER_LINES],
                    n: 0,
                };
                collect_nets(&mut b, &mut tb, s, &mut st);
                text_clear(&mut tb);
                text_push_str(&mut tb, "SAFETY DISPLAY ERROR ");
                text_push_dec(&mut tb, error_code(&e), 0);
                banner_insert(&mut b, 0, &tb, &mut st);
                // Best effort: a failure here leaves the list faulted, which the caller sees.
                let _ = banner_draw(l, proj, &mut b, &mut tb, s.targets, &mut st);
            }
            Err(e)
        }
    }
}

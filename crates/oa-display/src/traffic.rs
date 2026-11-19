//! Live traffic (**core**, B1, B2): a pure function from a set of displayed targets and a
//! projection to display-list commands, appended to the list the static map built. Nothing here
//! reads a clock or a file, and the crate does not know the tracker: the shell turns tracker rows
//! into [`Target`]s (SCOPE design principle 7, RULESET D3).
//!
//! # Symbols
//!
//! Plain, and never carried by colour alone. Sizes are pixels; every colour is a palette entry.
//!
//! | state                  | drawing                                                          |
//! |------------------------|------------------------------------------------------------------|
//! | tracking               | 8 px square outline and a centre dot, [`pal::TARGET`]            |
//! | coasting               | the square with an X through it; data block tag `CST`            |
//! | uncertain              | a 12 px diamond and a centre dot, and the 95% radius drawn as a circle of true scale; tag `UNC` |
//! | coasting and uncertain | the diamond with an X through it, the circle; tags `CST UNC`     |
//! | reacquired             | any of the above plus a 9 px radius circle in [`pal::ALERT`] for [`TrafficStyle::reacquired_s`] seconds after the reacquisition; tag `REACQ` |
//!
//! Every target also has a velocity leader (a line from the symbol along the ground track, as long
//! as the target flies in [`TrafficStyle::leader_min`] minutes, absent without a velocity), a
//! history trail of small [`pal::DIM_TEXT`] dots at the last [`TrafficStyle::trail_n`] positions,
//! and a data block at a fixed offset: line 1 the callsign (the hex address without one), line 2
//! the altitude in hundreds of feet and the ground speed in knots over ten (`---` or `--` when
//! absent), line 3 the state tags above when there are any.
//!
//! Data blocks are placed at one fixed offset from the symbol.
// TODO(LS-401): automatic data block placement without overlap in holding and on final (M8);
// until then blocks of close targets overlap.
//!
//! # Capacity
//!
//! A [`TargetSet`] has a fixed capacity. A push past it is refused, counted in
//! [`TargetSet::refused`] and answered with [`TrafficError::Full`]; it never truncates silently.
//! The frame then carries `TARGETS NOT DRAWN: N` in the alert colour at the bottom left. A target
//! whose position is off the map area (or under the legend strip) is counted in
//! [`TrafficStats::off_screen`], as a scope does at its edge.

use crate::list::{
    check_limit, push_circle, push_line, push_rect, push_text, DisplayError, DisplayList, ListCaps,
    ALIGN_LEFT, MAX_LIST_ROWS, MAX_LIST_TEXT_BYTES,
};
use crate::palette as pal;
use crate::project::{project, Projection};
use oa_geo::{Ellipsoid, LatLon};
use std::fmt;
use std::fmt::Write;

/// Layers of the traffic, between the map and the legend strip.
pub const LAYER_TRAIL: u8 = 7;
pub const LAYER_LEADER: u8 = 8;
pub const LAYER_SYMBOL: u8 = 9;
pub const LAYER_BLOCK: u8 = 10;

/// Most history positions a target carries.
pub const MAX_TRAIL: usize = 16;
/// Absent altitude.
pub const NO_ALT: i32 = i32::MIN;
/// Absent ground speed or track.
pub const NO_U16: u16 = u16::MAX;
/// Absent reacquisition: the target has not been reacquired lately.
pub const NO_REACQ: u16 = u16::MAX;

/// [`Target::flags`]: the track is coasting.
pub const T_COAST: u8 = 1;
/// The position is uncertain (its 95% radius is over the tracker's limit).
pub const T_UNCERTAIN: u8 = 2;

/// Largest leader, minutes.
pub const MAX_LEADER_MIN: f64 = 30.0;
/// Largest number of targets one set may be given.
pub const MAX_TARGETS: usize = 1 << 20;

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
const MAX_BLOCK_LINES: usize = 3;
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
}

impl fmt::Display for TrafficError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrafficError::Full { capacity } => write!(
                f,
                "target set full at {capacity} targets; the target was refused and counted"
            ),
        }
    }
}

impl std::error::Error for TrafficError {}

/// One displayed target. Plain data (A1); A7.2: transient, one per push.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Ground track, 0.1 degree true in [0, 3600), or [`NO_U16`].
    pub track_ddeg: u16,
    /// Ground speed, 0.1 knot, or [`NO_U16`]. A leader needs both this and the track.
    pub gs_dkt: u16,
    /// Altitude, feet, or [`NO_ALT`].
    pub alt_ft: i32,
    /// Aircraft address (bit 24 set for a non-ICAO one); shown as hex without a callsign.
    pub addr: u32,
    /// Callsign, ASCII, NUL or space padded; all NUL when absent.
    pub callsign: [u8; 8],
    /// [`T_COAST`] and [`T_UNCERTAIN`].
    pub flags: u8,
    /// Seconds since the track was last reacquired, or [`NO_REACQ`].
    pub reacq_age_s: u16,
    /// 95% horizontal position radius, metres; NaN or not positive when unknown.
    pub r95_m: f32,
    /// Positions before the current one, newest first; the first `trail_len` are valid.
    pub trail: [(i32, i32); MAX_TRAIL],
    pub trail_len: u8,
}

/// The targets of one frame, struct-of-arrays (A3), of fixed capacity (C1, C2). Trail positions
/// are `MAX_TRAIL` rows per target in the flat `trail_*` columns.
// A7.2: cold struct (a collection of columns), size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct TargetSet {
    pub capacity: usize,
    /// Targets refused because the set was full, since the last [`targets_clear`].
    pub refused: u32,
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    pub track_ddeg: Vec<u16>,
    pub gs_dkt: Vec<u16>,
    pub alt_ft: Vec<i32>,
    pub addr: Vec<u32>,
    pub callsign: Vec<[u8; 8]>,
    pub flags: Vec<u8>,
    pub reacq_age_s: Vec<u16>,
    pub r95_m: Vec<f32>,
    pub trail_len: Vec<u8>,
    pub trail_lat_e7: Vec<i32>,
    pub trail_lon_e7: Vec<i32>,
}

/// An empty set with room for `capacity` targets; the only allocation. A capacity over
/// [`MAX_TARGETS`] is refused before anything is allocated.
pub fn targets_new(capacity: usize) -> Result<TargetSet, DisplayError> {
    check_limit("targets", capacity as u128, MAX_TARGETS)?;
    let trail = capacity * MAX_TRAIL;
    Ok(TargetSet {
        capacity,
        refused: 0,
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

/// Empties the set and its refusal count for the next frame, keeping every capacity.
pub fn targets_clear(s: &mut TargetSet) {
    s.refused = 0;
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

/// Adds a target, or refuses it: at capacity the target is counted in `refused` and the call
/// returns [`TrafficError::Full`].
pub fn targets_push(s: &mut TargetSet, t: &Target) -> Result<(), TrafficError> {
    if s.lat_e7.len() >= s.capacity {
        s.refused = s.refused.saturating_add(1);
        return Err(TrafficError::Full {
            capacity: s.capacity,
        });
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
    let n = usize::from(t.trail_len).min(MAX_TRAIL);
    s.trail_len.push(n as u8);
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

/// What a traffic build drew and left out.
// A7.2: cold struct, size guard waived (one per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrafficStats {
    pub drawn: u32,
    /// Targets whose position is off the map area, under the legend strip or not projectable.
    pub off_screen: u32,
    /// Targets the set refused at capacity. Shown on the frame.
    pub refused: u32,
    pub with_leader: u32,
    pub coasting: u32,
    pub uncertain: u32,
    pub reacquired: u32,
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
        texts: rows("traffic texts", MAX_BLOCK_LINES as u128, 1)?,
        text_bytes: check_limit(
            "traffic text bytes",
            n * BYTES_PER_TARGET as u128 + NOTICE_BYTES as u128,
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

/// Altitude as the data block shows it: hundreds of feet, rounded to the nearest (halves away
/// from zero), at least two digits, three for flight levels; `---` when absent.
pub fn alt_text(alt_ft: i32) -> String {
    if alt_ft == NO_ALT {
        return "---".to_string();
    }
    let a = i64::from(alt_ft);
    let h = if a >= 0 {
        (a + 50) / 100
    } else {
        -((-a + 50) / 100)
    };
    if h < 0 {
        format!("-{:02}", (-h).min(99))
    } else {
        format!("{:03}", h.min(999))
    }
}

/// Ground speed as the data block shows it: knots over ten, rounded to the nearest, two digits;
/// `--` when absent.
pub fn speed_text(gs_dkt: u16) -> String {
    if gs_dkt == NO_U16 {
        return "--".to_string();
    }
    format!("{:02}", (u32::from(gs_dkt) + 50) / 100)
}

/// Line 1 of the data block: the callsign without its padding, or the hex address.
pub fn ident_text(callsign: &[u8; 8], addr: u32) -> String {
    let n = callsign
        .iter()
        .rposition(|&b| b != 0 && b != b' ')
        .map_or(0, |i| i + 1);
    let mut s = String::new();
    for &b in callsign.iter().take(n) {
        // A character the font lacks draws as its missing-glyph box, so it is seen.
        s.push(if (0x20..0x7f).contains(&b) {
            b as char
        } else {
            '#'
        });
    }
    if s.is_empty() {
        let _ = write!(s, "{addr:06X}");
    }
    s
}

/// The state tags of line 3, space separated; empty for a plain tracking target.
pub fn tags_text(flags: u8, reacquired: bool) -> String {
    let mut s = String::new();
    for (on, tag) in [
        (flags & T_COAST != 0, "CST"),
        (flags & T_UNCERTAIN != 0, "UNC"),
        (reacquired, "REACQ"),
    ] {
        if on {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(tag);
        }
    }
    s
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
    let d = f64::from(p.0 - here.0).hypot(f64::from(p.1 - here.1));
    d.is_finite()
        .then_some(d.round().min(f64::from(1 << 28)) as i32)
}

fn to_e7(deg: f64) -> Option<i32> {
    let v = (deg * 1e7).round();
    (v.is_finite() && v.abs() <= 1.8e9).then_some(v as i32)
}

/// True when a position projects onto the map area: on the screen and not under the legend
/// strip. What [`build_traffic`] counts as drawn; a shell uses it to keep targets that are off
/// the map out of a capacity-limited set.
pub fn on_map(proj: &Projection, lat_e7: i32, lon_e7: i32) -> bool {
    let Some((x, y)) = project(proj, lat_e7, lon_e7) else {
        return false;
    };
    let (x, y) = (i64::from(x), i64::from(y));
    x >= 0
        && x <= i64::from(proj.width) * 16
        && y >= i64::from(proj.top_inset) * 16
        && y <= i64::from(proj.height) * 16
}

/// The screen end of the leader, 1/16 pixel.
fn leader_end(
    proj: &Projection,
    ell: &Ellipsoid,
    at: LatLon,
    track_ddeg: u16,
    gs_dkt: u16,
    minutes: f64,
) -> Option<(i32, i32)> {
    if track_ddeg == NO_U16 || gs_dkt == NO_U16 || track_ddeg >= 3600 {
        return None;
    }
    // Knots times minutes over sixty is nautical miles.
    let metres = f64::from(gs_dkt) / 10.0 * minutes / 60.0 * 1852.0;
    if metres <= 0.0 {
        return None;
    }
    let end = ell
        .direct(at, f64::from(track_ddeg) / 10.0, metres)
        .ok()?
        .position;
    project(proj, to_e7(end.lat())?, to_e7(end.lon())?)
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
        push_line(l, p, q, pal::TARGET, LAYER_SYMBOL)?;
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
        push_line(l, p, q, pal::TARGET, LAYER_SYMBOL)?;
    }
    Ok(())
}

/// Appends the traffic of `set` to `l` (it does not clear the list). `ell` is the WGS-84
/// ellipsoid, built once by the caller. Targets are drawn in set order. On an error the list is
/// left faulted and will not be drawn.
pub fn build_traffic(
    l: &mut DisplayList,
    proj: &Projection,
    ell: &Ellipsoid,
    set: &TargetSet,
    st: &TrafficStyle,
) -> Result<TrafficStats, DisplayError> {
    style_check(st)?;
    let mut stats = TrafficStats {
        refused: set.refused,
        ..TrafficStats::default()
    };
    let h16 = i64::from(proj.height) * 16;
    for i in 0..set.lat_e7.len() {
        let (Some(&lat), Some(&lon)) = (set.lat_e7.get(i), set.lon_e7.get(i)) else {
            continue;
        };
        let Some(c) = project(proj, lat, lon).filter(|_| on_map(proj, lat, lon)) else {
            stats.off_screen += 1;
            continue;
        };
        let flags = set.flags.get(i).copied().unwrap_or(0);
        let coast = flags & T_COAST != 0;
        let uncertain = flags & T_UNCERTAIN != 0;
        let age = set.reacq_age_s.get(i).copied().unwrap_or(NO_REACQ);
        let reacq = age != NO_REACQ && age < st.reacquired_s;
        let at = LatLon::new(f64::from(lat) * 1e-7, f64::from(lon) * 1e-7).ok();

        // History dots, 2 px squares, in the dim colour.
        let n =
            usize::from(set.trail_len.get(i).copied().unwrap_or(0)).min(usize::from(st.trail_n));
        for k in 0..n {
            let at_k = i * MAX_TRAIL + k;
            if let (Some(&tla), Some(&tlo)) =
                (set.trail_lat_e7.get(at_k), set.trail_lon_e7.get(at_k))
            {
                if let Some(p) = project(proj, tla, tlo) {
                    push_rect(
                        l,
                        add(p, -px(TRAIL_HALF), -px(TRAIL_HALF)),
                        add(p, px(TRAIL_HALF), px(TRAIL_HALF)),
                        pal::DIM_TEXT,
                        LAYER_TRAIL,
                    )?;
                }
            }
        }

        // The velocity leader.
        let gs = set.gs_dkt.get(i).copied().unwrap_or(NO_U16);
        let trk = set.track_ddeg.get(i).copied().unwrap_or(NO_U16);
        if let Some(end) = at.and_then(|a| leader_end(proj, ell, a, trk, gs, st.leader_min)) {
            push_line(l, c, end, pal::TARGET, LAYER_LEADER)?;
            stats.with_leader += 1;
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
                pal::TARGET,
                LAYER_SYMBOL,
            )?;
            push_line(
                l,
                add(c, -px(half), px(half)),
                add(c, px(half), -px(half)),
                pal::TARGET,
                LAYER_SYMBOL,
            )?;
            stats.coasting += 1;
        } else {
            push_rect(l, add(c, -8, -8), add(c, 8, 8), pal::TARGET, LAYER_SYMBOL)?;
        }
        if uncertain {
            stats.uncertain += 1;
            let r95 = set.r95_m.get(i).copied().unwrap_or(f32::NAN);
            if r95.is_finite() && r95 > 0.0 {
                if let Some(r) = at.and_then(|a| true_radius16(proj, ell, c, a, f64::from(r95))) {
                    push_circle(l, c, r, pal::TARGET, LAYER_SYMBOL)?;
                }
            }
        }
        if reacq {
            push_circle(l, c, px(REACQ_RADIUS), pal::ALERT, LAYER_SYMBOL)?;
            stats.reacquired += 1;
        }

        // The data block, at a fixed offset (LS-401: no overlap avoidance yet).
        let mut at_b = add(c, px(BLOCK_DX), px(BLOCK_DY));
        let callsign = set.callsign.get(i).copied().unwrap_or([0; 8]);
        let addr = set.addr.get(i).copied().unwrap_or(0);
        let alt = set.alt_ft.get(i).copied().unwrap_or(NO_ALT);
        let line2 = format!("{} {}", alt_text(alt), speed_text(gs));
        let tags = tags_text(flags, reacq);
        for text in [ident_text(&callsign, addr), line2, tags] {
            if text.is_empty() {
                continue;
            }
            push_text(
                l,
                at_b,
                BLOCK_SIZE,
                ALIGN_LEFT,
                pal::TARGET,
                LAYER_BLOCK,
                &text,
            )?;
            at_b = add(at_b, 0, px(BLOCK_PITCH));
        }
        stats.drawn += 1;
    }
    if set.refused > 0 {
        let at_n = (
            px(NOTICE_X),
            i32::try_from(h16)
                .unwrap_or(i32::MAX)
                .saturating_sub(px(NOTICE_FROM_BOTTOM)),
        );
        let text = format!(
            "TARGETS NOT DRAWN: {} (DISPLAY FULL AT {})",
            set.refused, set.capacity
        );
        push_text(
            l,
            at_n,
            NOTICE_SIZE,
            ALIGN_LEFT,
            pal::ALERT,
            LAYER_BLOCK,
            &text,
        )?;
    }
    Ok(stats)
}

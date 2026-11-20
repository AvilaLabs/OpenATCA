//! Inhibition objects (**core**, B1, B2), M5 design rule 2: an inhibition is an object, never a
//! setting. It has an area, an altitude band, a start, an end no later than the maximum the
//! configuration allows for its net, a reason and the name of whoever set it. It is logged when
//! made, when it expires and when it is removed early, and every alert it suppresses is recorded
//! so a replay can show what it hid. There is no permanent inhibition and no way to switch a net
//! off for an area except by such an object. (Guam, 1997: the MSAW inhibition and its poor
//! management were named by the NTSB.)
//!
//! Rules of the table:
//! * In force at `t` means `start <= t < end`: the start instant is in, the end instant is out.
//! * The band is inclusive at both ends. An alert candidate whose altitude is unknown is never
//!   suppressed: a band cannot be checked against a missing altitude (rule 4).
//! * A request that is not valid, or that finds the table full, is refused and counted; no
//!   inhibition is ever evicted to make room.
//! * An expired inhibition leaves the table once its "expired, review" event is in the event log.
//!   If the log is full the inhibition stays (not in force, counted by [`inhibits_expired_waiting`])
//!   until the shell drains the log, so no expiry is ever unreported.
//! * The suppressed-alert log is bounded; an entry that does not fit is counted in
//!   [`suppressed_overflow`], never silently lost.

use crate::geom::{polygon_contains, polygon_extent, polygon_new, Polygon, Vertex, POLYGON_EMPTY};
use crate::net::{net_index, NetId, NET_COUNT};

/// Most inhibitions held at once, all nets. C2: a facility runs a few deliberate inhibitions at a
/// time (a closed range, a helicopter route, a noisy approach); 64 is more than 15 per net, so
/// reaching it means the inhibitions themselves need review. A request past it is refused.
pub const INHIBIT_CAP: usize = 64;
/// Bytes of the reason text. 32 holds a plain sentence fragment ("RANGE 3 TEST FIRING"); longer
/// text is refused, not cut.
pub const REASON_BYTES: usize = 32;
/// Bytes of the author text: a controller or supervisor identifier.
pub const AUTHOR_BYTES: usize = 24;
/// Entries of the suppressed-alert log. C2: one entry per suppressed alert candidate per cycle;
/// 4096 is about 68 minutes of one persistently suppressed pair at 1 Hz (24 bytes each, 96 KiB),
/// and the shell drains the log into the replay record far sooner. Full means counted overflow.
pub const SUPPRESSED_CAP: usize = 4096;
/// Entries of the event log. C2: about four events per inhibition over its life (made, expired
/// or removed) for the whole table.
pub const EVENT_CAP: usize = 256;

/// Time in nanoseconds since the epoch.
pub type TimeNs = i64;

/// One inhibition. Plain data; only [`inhibit_create`] makes a valid one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Inhibition {
    pub start_ns: TimeNs,
    pub end_ns: TimeNs,
    pub id: u32,
    pub alt_low_ft: i32,
    pub alt_high_ft: i32,
    pub area: Polygon,
    /// ASCII, zero padded.
    pub reason: [u8; REASON_BYTES],
    pub author: [u8; AUTHOR_BYTES],
    pub net: NetId,
}
// Budget: scanned for every alert candidate; the polygon (260), two times (16), id and band (12),
// the two texts (56) and the net, padded to the 8-byte alignment of the times.
const _: () = assert!(core::mem::size_of::<Inhibition>() == 352);

const EMPTY: Inhibition = Inhibition {
    start_ns: 0,
    end_ns: 0,
    id: 0,
    alt_low_ft: 0,
    alt_high_ft: 0,
    area: POLYGON_EMPTY,
    reason: [0; REASON_BYTES],
    author: [0; AUTHOR_BYTES],
    net: NetId::ConflictAlert,
};

/// The maximum duration of an inhibition on each net, from the configuration. There is no default:
/// a net without a maximum accepts no inhibition.
// A7.2: cold struct, size guard waived (one per facility).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InhibitConfig {
    pub max_duration_ns: [Option<i64>; NET_COUNT],
    /// An inhibition may start at most this far after the time of the request. No default.
    pub max_start_ahead_ns: Option<i64>,
    /// The latitude and the longitude extent of an inhibition area, 1e-7 degree, at most. No
    /// default: an area is as large as the facility says and no larger.
    pub max_extent_e7: Option<i32>,
}

/// A configuration that allows no inhibition on any net.
pub const fn inhibit_config_none() -> InhibitConfig {
    InhibitConfig {
        max_duration_ns: [None; NET_COUNT],
        max_start_ahead_ns: None,
        max_extent_e7: None,
    }
}

/// What a controller or supervisor asks for. Plain data, borrowed.
// A7.2: cold struct, size guard waived (one per request).
#[derive(Clone, Copy, Debug)]
pub struct InhibitRequest<'a> {
    pub net: NetId,
    pub vertices: &'a [Vertex],
    pub alt_low_ft: i32,
    pub alt_high_ft: i32,
    pub start_ns: TimeNs,
    pub end_ns: TimeNs,
    pub reason: &'a str,
    pub author: &'a str,
}

/// Why a request was refused. Each refusal is counted in the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InhibitError {
    /// The configuration has no maximum duration for the net (or not a positive one).
    NoMaxConfigured,
    /// The configuration has no start horizon (or not a non-negative one).
    NoStartHorizonConfigured,
    /// The configuration has no maximum extent (or not a positive one).
    NoExtentConfigured,
    EndNotAfterStart,
    /// The start is further ahead than the configured horizon.
    StartTooFar,
    /// The area is wider or taller than the configured maximum extent.
    AreaTooLarge,
    /// The event log has no room to record the creation: the request is refused, never made
    /// without its record.
    EventLogFull,
    /// Every id has been used.
    IdsExhausted,
    /// The end is already past at the time of the request.
    AlreadyExpired,
    TooLong,
    BadPolygon(&'static str),
    /// The band's low limit is above its high limit.
    BadBand,
    EmptyReason,
    EmptyAuthor,
    /// The text is longer than its field or has a character outside printable ASCII.
    BadText(&'static str),
    TableFull,
}

/// Why a removal was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoveError {
    NotFound,
    /// The event log has no room for the record of the removal (two slots for an expired
    /// inhibition still waiting): the inhibition stays, and stays visible.
    EventLogFull,
    /// The author text is empty, too long or not printable ASCII: a removal names who did it.
    BadAuthor,
}

/// What happened to an inhibition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EventKind {
    Created = 0,
    /// The end passed: the inhibition left the table and must be reviewed.
    Expired = 1,
    /// Removed before its end by an explicit action; `by` names who.
    Removed = 2,
}

/// One entry of the event log. For `Created` and `Expired`, `by` is the inhibition's author; for
/// `Removed`, whoever removed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Event {
    pub time_ns: TimeNs,
    pub id: u32,
    pub kind: EventKind,
    pub net: NetId,
    pub by: [u8; AUTHOR_BYTES],
}
// Budget: held 256, written on a rare action; the time, id, two bytes and the author.
const _: () = assert!(core::mem::size_of::<Event>() == 40);

const EVENT_EMPTY: Event = Event {
    time_ns: 0,
    id: 0,
    kind: EventKind::Created,
    net: NetId::ConflictAlert,
    by: [0; AUTHOR_BYTES],
};

/// One alert candidate that an inhibition hid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Suppressed {
    pub time_ns: TimeNs,
    pub inhibition_id: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub net: NetId,
}
// Budget: 4096 held; the time, three 4-byte fields and the net, padded to 8.
const _: () = assert!(core::mem::size_of::<Suppressed>() == 24);

const SUPPRESSED_EMPTY: Suppressed = Suppressed {
    time_ns: 0,
    inhibition_id: 0,
    lat_e7: 0,
    lon_e7: 0,
    net: NetId::ConflictAlert,
};

/// The inhibitions in force at one position and time: `count` ids in `ids`, in table order.
// A7.2: cold struct, size guard waived (a result on the stack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Suppressors {
    pub count: u32,
    pub ids: [u32; INHIBIT_CAP],
}

/// The table, its logs and its counters. Fixed arrays, allocated once (C1); fields are private,
/// the free functions below are the only way in.
// A7.2: cold struct, size guard waived (one per facility).
#[derive(Clone, Debug)]
pub struct InhibitTable {
    items: [Inhibition; INHIBIT_CAP],
    len: usize,
    next_id: u32,
    events: [Event; EVENT_CAP],
    events_len: usize,
    events_overflow: u64,
    suppressed: [Suppressed; SUPPRESSED_CAP],
    suppressed_len: usize,
    suppressed_overflow: u64,
    suppressed_overflow_net: [u64; NET_COUNT],
    refused_full: u32,
    refused_event_full: u32,
    refused_invalid: u32,
}

/// An empty table. The one allocation of the subsystem is the caller's, for the value itself
/// (about 110 KiB): box it once at start.
pub fn inhibits_new() -> InhibitTable {
    InhibitTable {
        items: [EMPTY; INHIBIT_CAP],
        len: 0,
        next_id: 1,
        events: [EVENT_EMPTY; EVENT_CAP],
        events_len: 0,
        events_overflow: 0,
        suppressed: [SUPPRESSED_EMPTY; SUPPRESSED_CAP],
        suppressed_len: 0,
        suppressed_overflow: 0,
        suppressed_overflow_net: [0; NET_COUNT],
        refused_full: 0,
        refused_event_full: 0,
        refused_invalid: 0,
    }
}

/// The text of a zero-padded field: the bytes before the first zero. Empty if it is not valid
/// UTF-8 (it is ASCII by construction).
pub fn text_of(field: &[u8]) -> &str {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    field
        .get(..end)
        .and_then(|b| core::str::from_utf8(b).ok())
        .unwrap_or("")
}

fn text_put<const N: usize>(s: &str, empty: InhibitError) -> Result<[u8; N], InhibitError> {
    if s.is_empty() {
        return Err(empty);
    }
    if s.len() > N {
        return Err(InhibitError::BadText("text longer than its field"));
    }
    if !s.bytes().all(|b| (0x20..0x7F).contains(&b)) {
        return Err(InhibitError::BadText("text is not printable ASCII"));
    }
    let mut out = [0u8; N];
    for (d, b) in out.iter_mut().zip(s.bytes()) {
        *d = b;
    }
    Ok(out)
}

/// True if the inhibition is in force at `t`: `start <= t < end`.
pub fn in_force(start_ns: TimeNs, end_ns: TimeNs, t: TimeNs) -> bool {
    start_ns <= t && t < end_ns
}

/// True if `alt_ft` is in the band, both limits included.
pub fn in_band(low_ft: i32, high_ft: i32, alt_ft: i32) -> bool {
    low_ft <= alt_ft && alt_ft <= high_ft
}

fn log_event(t: &mut InhibitTable, e: Event) -> bool {
    match t.events.get_mut(t.events_len) {
        Some(slot) => {
            *slot = e;
            t.events_len += 1;
            true
        }
        None => {
            t.events_overflow = t.events_overflow.saturating_add(1);
            false
        }
    }
}

fn validate(
    cfg: &InhibitConfig,
    r: &InhibitRequest<'_>,
    now_ns: TimeNs,
) -> Result<(Polygon, [u8; REASON_BYTES], [u8; AUTHOR_BYTES]), InhibitError> {
    let max = cfg
        .max_duration_ns
        .get(net_index(r.net))
        .copied()
        .flatten()
        .filter(|&m| m > 0)
        .ok_or(InhibitError::NoMaxConfigured)?;
    let ahead = cfg
        .max_start_ahead_ns
        .filter(|&a| a >= 0)
        .ok_or(InhibitError::NoStartHorizonConfigured)?;
    let extent = cfg
        .max_extent_e7
        .filter(|&e| e > 0)
        .ok_or(InhibitError::NoExtentConfigured)?;
    if r.end_ns <= r.start_ns {
        return Err(InhibitError::EndNotAfterStart);
    }
    if r.end_ns <= now_ns {
        return Err(InhibitError::AlreadyExpired);
    }
    if r.start_ns > now_ns.saturating_add(ahead) {
        return Err(InhibitError::StartTooFar);
    }
    // A span that overflows i64 is longer than any maximum.
    match r.end_ns.checked_sub(r.start_ns) {
        Some(d) if d <= max => {}
        _ => return Err(InhibitError::TooLong),
    }
    if r.alt_low_ft > r.alt_high_ft {
        return Err(InhibitError::BadBand);
    }
    let area = polygon_new(r.vertices).map_err(InhibitError::BadPolygon)?;
    if polygon_extent(&area) > i64::from(extent) {
        return Err(InhibitError::AreaTooLarge);
    }
    let reason = text_put::<REASON_BYTES>(r.reason, InhibitError::EmptyReason)?;
    let author = text_put::<AUTHOR_BYTES>(r.author, InhibitError::EmptyAuthor)?;
    Ok((area, reason, author))
}

/// Makes an inhibition and returns its id (ids start at 1 and are never reused). Checks, in
/// order: a maximum duration configured for the net (the configuration must supply one), end after
/// start, end after `now_ns`, duration within the maximum, band, polygon, texts, then room. A
/// refusal is counted and changes nothing else. The creation is logged at `now_ns`.
pub fn inhibit_create(
    t: &mut InhibitTable,
    cfg: &InhibitConfig,
    r: &InhibitRequest<'_>,
    now_ns: TimeNs,
) -> Result<u32, InhibitError> {
    let (area, reason, author) = match validate(cfg, r, now_ns) {
        Ok(v) => v,
        Err(e) => {
            t.refused_invalid = t.refused_invalid.saturating_add(1);
            return Err(e);
        }
    };
    if t.len >= INHIBIT_CAP {
        t.refused_full = t.refused_full.saturating_add(1);
        return Err(InhibitError::TableFull);
    }
    if t.events_len >= EVENT_CAP {
        t.refused_event_full = t.refused_event_full.saturating_add(1);
        return Err(InhibitError::EventLogFull);
    }
    if t.next_id == u32::MAX {
        t.refused_invalid = t.refused_invalid.saturating_add(1);
        return Err(InhibitError::IdsExhausted);
    }
    let id = t.next_id;
    let Some(slot) = t.items.get_mut(t.len) else {
        t.refused_full = t.refused_full.saturating_add(1);
        return Err(InhibitError::TableFull);
    };
    *slot = Inhibition {
        start_ns: r.start_ns,
        end_ns: r.end_ns,
        id,
        alt_low_ft: r.alt_low_ft,
        alt_high_ft: r.alt_high_ft,
        area,
        reason,
        author,
        net: r.net,
    };
    t.len += 1;
    t.next_id += 1;
    log_event(
        t,
        Event {
            time_ns: now_ns,
            id,
            kind: EventKind::Created,
            net: r.net,
            by: author,
        },
    );
    Ok(id)
}

/// The inhibitions held, in creation order (including any that have expired and wait to be
/// reported).
pub fn inhibits_slice(t: &InhibitTable) -> &[Inhibition] {
    t.items.get(..t.len).unwrap_or(&[])
}

/// Number of inhibitions on `net` in force at `now_ns`.
pub fn inhibits_in_force(t: &InhibitTable, net: NetId, now_ns: TimeNs) -> u32 {
    let n = inhibits_slice(t)
        .iter()
        .filter(|i| i.net == net && in_force(i.start_ns, i.end_ns, now_ns))
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The inhibitions on `net` that hide an alert at the position, altitude and time: in force, the
/// point strictly inside the area, the altitude in the band. An unknown altitude (`None`) is
/// hidden by none. Pure: nothing is logged (see [`inhibit_suppress`]). Work: at most
/// [`INHIBIT_CAP`] inhibitions of at most 32 edges each.
pub fn inhibit_suppressors(
    t: &InhibitTable,
    net: NetId,
    lat_e7: i32,
    lon_e7: i32,
    alt_ft: Option<i32>,
    now_ns: TimeNs,
) -> Suppressors {
    let mut out = Suppressors {
        count: 0,
        ids: [0; INHIBIT_CAP],
    };
    let Some(alt) = alt_ft else {
        return out;
    };
    for i in inhibits_slice(t) {
        if i.net == net
            && in_force(i.start_ns, i.end_ns, now_ns)
            && in_band(i.alt_low_ft, i.alt_high_ft, alt)
            && polygon_contains(&i.area, lat_e7, lon_e7)
        {
            if let Some(slot) = out.ids.get_mut(out.count as usize) {
                *slot = i.id;
                out.count += 1;
            }
        }
    }
    out
}

/// [`inhibit_suppressors`], and every inhibition that hides the candidate is recorded in the
/// suppressed-alert log. Call it when an alert would be raised and the answer decides whether it
/// is. A full log counts the entry in [`suppressed_overflow`]; the suppression still happens (the
/// alternative, showing alerts a controller inhibited on purpose, would be the nuisance the
/// inhibition exists to remove), and the overflow is visible.
pub fn inhibit_suppress(
    t: &mut InhibitTable,
    net: NetId,
    lat_e7: i32,
    lon_e7: i32,
    alt_ft: Option<i32>,
    now_ns: TimeNs,
) -> Suppressors {
    let s = inhibit_suppressors(t, net, lat_e7, lon_e7, alt_ft, now_ns);
    for &id in s.ids.iter().take(s.count as usize) {
        let entry = Suppressed {
            time_ns: now_ns,
            inhibition_id: id,
            lat_e7,
            lon_e7,
            net,
        };
        match t.suppressed.get_mut(t.suppressed_len) {
            Some(slot) => {
                *slot = entry;
                t.suppressed_len += 1;
            }
            None => {
                t.suppressed_overflow = t.suppressed_overflow.saturating_add(1);
                if let Some(n) = t.suppressed_overflow_net.get_mut(net_index(net)) {
                    *n = n.saturating_add(1);
                }
            }
        }
    }
    s
}

/// The suppressed-alert log, oldest first.
pub fn suppressed_log(t: &InhibitTable) -> &[Suppressed] {
    t.suppressed.get(..t.suppressed_len).unwrap_or(&[])
}

/// Suppressions that did not fit in the log.
pub fn suppressed_overflow(t: &InhibitTable) -> u64 {
    t.suppressed_overflow
}

/// Suppressions on `net` lost to a full log since the log was last cleared; while it is above
/// zero the net's status reads degraded (`SuppressionLogFull`).
pub fn suppressed_overflow_net(t: &InhibitTable, net: NetId) -> u64 {
    t.suppressed_overflow_net
        .get(net_index(net))
        .copied()
        .unwrap_or(0)
}

/// Empties the suppressed-alert log once the shell has recorded it, and the per-net marks that
/// made the nets read degraded. The total overflow count stays: the loss happened.
pub fn suppressed_clear(t: &mut InhibitTable) {
    t.suppressed_len = 0;
    t.suppressed_overflow_net = [0; NET_COUNT];
}

/// Removes an inhibition before its end, by an explicit action that names who did it. Logged.
pub fn inhibit_remove(
    t: &mut InhibitTable,
    id: u32,
    by: &str,
    now_ns: TimeNs,
) -> Result<(), RemoveError> {
    let by = text_put::<AUTHOR_BYTES>(by, InhibitError::EmptyAuthor)
        .map_err(|_| RemoveError::BadAuthor)?;
    let Some(at) = inhibits_slice(t).iter().position(|i| i.id == id) else {
        return Err(RemoveError::NotFound);
    };
    let Some(it) = t.items.get(at).copied() else {
        return Err(RemoveError::NotFound);
    };
    // One slot for the removal; an inhibition already past its end that still waits for its
    // review event needs a second, for that event first.
    let waiting = it.end_ns <= now_ns;
    let need = if waiting { 2 } else { 1 };
    if t.events_len.saturating_add(need) > EVENT_CAP {
        return Err(RemoveError::EventLogFull);
    }
    let net = it.net;
    if waiting {
        log_event(
            t,
            Event {
                time_ns: now_ns,
                id,
                kind: EventKind::Expired,
                net,
                by: it.author,
            },
        );
    }
    remove_at(t, at);
    log_event(
        t,
        Event {
            time_ns: now_ns,
            id,
            kind: EventKind::Removed,
            net,
            by,
        },
    );
    Ok(())
}

/// Removes slot `at`, keeping the order of the rest (so replay is deterministic).
fn remove_at(t: &mut InhibitTable, at: usize) {
    if at >= t.len {
        return;
    }
    t.items.copy_within(at + 1..t.len, at);
    t.len -= 1;
    if let Some(last) = t.items.get_mut(t.len) {
        *last = EMPTY;
    }
}

/// Removes every inhibition whose end is at or before `now_ns`, each with one `Expired` event
/// ("inhibition expired, review"), and returns how many. An inhibition is reported once and
/// leaves the table. If the event log is full the inhibition stays, counted by
/// [`inhibits_expired_waiting`], and is reported on a later call after the log is drained.
pub fn inhibit_expire(t: &mut InhibitTable, now_ns: TimeNs) -> u32 {
    let mut removed = 0u32;
    let mut i = 0usize;
    while i < t.len {
        let Some(it) = t.items.get(i).copied() else {
            break;
        };
        if it.end_ns <= now_ns {
            if t.events_len >= EVENT_CAP {
                // Keep it: the review event cannot be recorded yet.
                i += 1;
                continue;
            }
            log_event(
                t,
                Event {
                    time_ns: now_ns,
                    id: it.id,
                    kind: EventKind::Expired,
                    net: it.net,
                    by: it.author,
                },
            );
            remove_at(t, i);
            removed += 1;
        } else {
            i += 1;
        }
    }
    removed
}

/// Inhibitions past their end that are still held because the event log was full.
pub fn inhibits_expired_waiting(t: &InhibitTable, now_ns: TimeNs) -> u32 {
    let n = inhibits_slice(t)
        .iter()
        .filter(|i| i.end_ns <= now_ns)
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Moves up to `out.len()` events, oldest first, out of the log and returns how many.
pub fn events_take(t: &mut InhibitTable, out: &mut [Event]) -> usize {
    let n = t.events_len.min(out.len());
    for (d, s) in out.iter_mut().zip(t.events.iter()).take(n) {
        *d = *s;
    }
    t.events.copy_within(n..t.events_len, 0);
    t.events_len -= n;
    n
}

/// Events waiting in the log.
pub fn events_len(t: &InhibitTable) -> usize {
    t.events_len
}

/// Events that did not fit in the log and were lost (an expiry is never lost this way: see
/// [`inhibit_expire`]; creation and removal events can be).
pub fn events_overflow(t: &InhibitTable) -> u64 {
    t.events_overflow
}

/// Requests refused as not valid.
pub fn inhibits_refused_invalid(t: &InhibitTable) -> u32 {
    t.refused_invalid
}

/// Requests refused because the event log had no room for their record.
pub fn inhibits_refused_event_full(t: &InhibitTable) -> u32 {
    t.refused_event_full
}

/// Requests refused because the table was full.
pub fn inhibits_refused_full(t: &InhibitTable) -> u32 {
    t.refused_full
}

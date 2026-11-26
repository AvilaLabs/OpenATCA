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

/// Most inhibitions retained at once, all nets. C2: a facility runs a few deliberate inhibitions at a
/// time (a closed range, a helicopter route, a noisy approach); 64 is more than 15 per net, so
/// reaching it means the inhibitions themselves need review. A request past it is refused.
pub const INHIBIT_CAP: usize = 64;
/// Bytes of the reason text. 32 holds a plain sentence fragment ("RANGE 3 TEST FIRING"); longer
/// text is refused, not cut.
pub const REASON_BYTES: usize = 32;
/// Bytes of the author text: a controller or supervisor identifier.
pub const AUTHOR_BYTES: usize = 24;
/// Entries of the suppressed-alert log. C2: one entry per suppressed alert candidate per cycle;
/// 4096 is about 68 minutes of one persistently suppressed pair at 1 Hz (40 bytes each, 160 KiB),
/// and the shell drains the log into the replay record far sooner. Full means counted overflow.
pub const SUPPRESSED_CAP: usize = 4096;
/// Exact-aircraft controls retained in the facility table. A request past this bound is refused.
pub const PAIR_SUPPRESS_CAP: usize = 128;
/// Set-of-aircraft controls retained in the facility table.
pub const GROUP_SUPPRESS_CAP: usize = 16;
/// Maximum aircraft in one group control. Membership is checked as a set; duplicates are refused.
pub const GROUP_TRACK_CAP: usize = 64;
/// Maximum work needed to check every pair and group control once.
pub const CONTROL_LOOKUP_WORK_MAX: u32 =
    (PAIR_SUPPRESS_CAP + GROUP_SUPPRESS_CAP * GROUP_TRACK_CAP) as u32;
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
    /// Maximum finite duration for exact-pair/group CA alert controls. No default.
    pub max_alert_suppression_ns: Option<i64>,
    /// Issued facility policy authorizing military special-operation group suppression.
    pub allow_military_group_suppression: bool,
}

/// A configuration that allows no inhibition on any net.
pub const fn inhibit_config_none() -> InhibitConfig {
    InhibitConfig {
        max_duration_ns: [None; NET_COUNT],
        max_start_ahead_ns: None,
        max_extent_e7: None,
        max_alert_suppression_ns: None,
        allow_military_group_suppression: false,
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
    /// Invalid explicit alert-control key, time window, member set, or text.
    BadAlertControl,
    /// Pair or group control table is full.
    AlertControlFull,
    /// Control IDs have been exhausted.
    AlertControlIdsExhausted,
    /// Lookup budget was too small to inspect all bounded control records.
    AlertControlWorkLimit,
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
    /// The exact alert instance; zero for legacy area-only callers without pair identity.
    pub alert_start_ns: TimeNs,
    /// Inhibition ID when `source == 0`, otherwise the explicit pair/group control ID.
    pub inhibition_id: u32,
    pub addr_a: u32,
    pub addr_b: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub net: NetId,
    /// 0 area inhibition, 1 pair control, 2 group control.
    pub source: u8,
}
// Budget: one bounded acknowledgement/audit record per suppressed alert instance.
const _: () = assert!(core::mem::size_of::<Suppressed>() == 40);

const SUPPRESSED_EMPTY: Suppressed = Suppressed {
    time_ns: 0,
    alert_start_ns: 0,
    inhibition_id: 0,
    addr_a: 0,
    addr_b: 0,
    lat_e7: 0,
    lon_e7: 0,
    net: NetId::ConflictAlert,
    source: 0,
};

const PAIR_SUPPRESSION_EMPTY: PairSuppression = PairSuppression {
    start_ns: 0,
    end_ns: 0,
    alert_start_ns: 0,
    id: 0,
    addr_a: 0,
    addr_b: 0,
    reason: [0; REASON_BYTES],
    author: [0; AUTHOR_BYTES],
};

const GROUP_SUPPRESSION_EMPTY: GroupSuppression = GroupSuppression {
    start_ns: 0,
    end_ns: 0,
    id: 0,
    member_count: 0,
    members: [0; GROUP_TRACK_CAP],
    reason: [0; REASON_BYTES],
    author: [0; AUTHOR_BYTES],
};

/// A suppression for one currently identified alert instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PairSuppression {
    pub start_ns: TimeNs,
    pub end_ns: TimeNs,
    pub alert_start_ns: TimeNs,
    pub id: u32,
    pub addr_a: u32,
    pub addr_b: u32,
    pub reason: [u8; REASON_BYTES],
    pub author: [u8; AUTHOR_BYTES],
}
// Budget: 128 active records, checked on each pair; exact size catches record growth.
const _: () = assert!(core::mem::size_of::<PairSuppression>() == 96);

/// A suppression among members of an explicit aircraft set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct GroupSuppression {
    pub start_ns: TimeNs,
    pub end_ns: TimeNs,
    pub id: u32,
    pub member_count: u8,
    pub members: [u32; GROUP_TRACK_CAP],
    pub reason: [u8; REASON_BYTES],
    pub author: [u8; AUTHOR_BYTES],
}
// Budget: 16 active records, checked on each pair; exact size catches record growth.
const _: () = assert!(core::mem::size_of::<GroupSuppression>() == 336);

/// Reason and author are required for every explicit alert suppression.
#[derive(Clone, Copy, Debug)]
pub struct AlertControlText<'a> {
    pub reason: &'a str,
    pub author: &'a str,
}

/// One specific pair alert to suppress for a finite window.
#[derive(Clone, Copy, Debug)]
pub struct PairSuppressRequest<'a> {
    pub addr_a: u32,
    pub addr_b: u32,
    pub alert_start_ns: TimeNs,
    pub start_ns: TimeNs,
    pub end_ns: TimeNs,
    pub text: AlertControlText<'a>,
}

/// Alerts between any two members of `members` are suppressed during this finite window.
#[derive(Clone, Copy, Debug)]
pub struct GroupSuppressRequest<'a> {
    pub members: &'a [u32],
    pub start_ns: TimeNs,
    pub end_ns: TimeNs,
    pub purpose: GroupSuppressPurpose,
    pub text: AlertControlText<'a>,
}

/// Group-suppression purpose. Only the FAA's specific military-special-operation case is
/// accepted; `Other` exists so a caller's wrong-purpose request is refused explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GroupSuppressPurpose {
    MilitarySpecialOperation = 1,
    Other = 255,
}

/// One alert candidate shared by the area and explicit pair/group control queries.
#[derive(Clone, Copy, Debug)]
pub struct AlertCandidate {
    pub net: NetId,
    pub addr_a: u32,
    pub addr_b: u32,
    pub alert_start_ns: TimeNs,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub alt_ft: Option<i32>,
    pub now_ns: TimeNs,
    pub control_work_budget: u32,
}

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
    pair_controls: [PairSuppression; PAIR_SUPPRESS_CAP],
    pair_controls_len: usize,
    group_controls: [GroupSuppression; GROUP_SUPPRESS_CAP],
    group_controls_len: usize,
    refused_control: u32,
    control_work_refused: u64,
    refused_full: u32,
    refused_event_full: u32,
    refused_invalid: u32,
}

/// An empty table. The one allocation of the subsystem is the caller's, for the value itself
/// (about 200 KiB with all bounded logs/control tables): box it once at start.
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
        pair_controls: [PAIR_SUPPRESSION_EMPTY; PAIR_SUPPRESS_CAP],
        pair_controls_len: 0,
        group_controls: [GROUP_SUPPRESSION_EMPTY; GROUP_SUPPRESS_CAP],
        group_controls_len: 0,
        refused_control: 0,
        control_work_refused: 0,
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

fn control_text_put(
    text: AlertControlText<'_>,
) -> Result<([u8; REASON_BYTES], [u8; AUTHOR_BYTES]), InhibitError> {
    let reason = text_put(text.reason, InhibitError::EmptyReason)?;
    let author = text_put(text.author, InhibitError::EmptyAuthor)?;
    Ok((reason, author))
}

fn control_window(start_ns: TimeNs, end_ns: TimeNs, now_ns: TimeNs, max_duration_ns: i64) -> bool {
    max_duration_ns > 0
        && start_ns < end_ns
        && end_ns > now_ns
        && end_ns
            .checked_sub(start_ns)
            .is_some_and(|span| span <= max_duration_ns)
}

/// Adds a finite control for one exact pair alert. The pair and alert start identify one instance;
/// a later alert between the same aircraft is unaffected. Expired controls remain bounded records
/// until the operator drains/rebuilds the facility table; capacity refusals are counted.
pub fn pair_suppress_create(
    t: &mut InhibitTable,
    cfg: &InhibitConfig,
    r: &PairSuppressRequest<'_>,
    now_ns: TimeNs,
) -> Result<u32, InhibitError> {
    let (addr_a, addr_b) = if r.addr_a < r.addr_b {
        (r.addr_a, r.addr_b)
    } else {
        (r.addr_b, r.addr_a)
    };
    let result = (|| {
        if addr_a == 0
            || addr_b == 0
            || addr_a == addr_b
            || r.alert_start_ns > now_ns
            || !cfg
                .max_alert_suppression_ns
                .is_some_and(|limit| control_window(r.start_ns, r.end_ns, now_ns, limit))
        {
            return Err(InhibitError::BadAlertControl);
        }
        let (reason, author) = control_text_put(r.text)?;
        if t.pair_controls_len >= PAIR_SUPPRESS_CAP {
            return Err(InhibitError::AlertControlFull);
        }
        if t.next_id == u32::MAX {
            return Err(InhibitError::AlertControlIdsExhausted);
        }
        let id = t.next_id;
        if t.pair_controls.iter().take(t.pair_controls_len).any(|old| {
            old.addr_a == addr_a
                && old.addr_b == addr_b
                && old.alert_start_ns == r.alert_start_ns
                && r.start_ns < old.end_ns
                && old.start_ns < r.end_ns
        }) {
            return Err(InhibitError::BadAlertControl);
        }
        let Some(slot) = t.pair_controls.get_mut(t.pair_controls_len) else {
            return Err(InhibitError::AlertControlFull);
        };
        *slot = PairSuppression {
            start_ns: r.start_ns,
            end_ns: r.end_ns,
            alert_start_ns: r.alert_start_ns,
            id,
            addr_a,
            addr_b,
            reason,
            author,
        };
        t.pair_controls_len += 1;
        t.next_id += 1;
        Ok(id)
    })();
    if result.is_err() {
        t.refused_control = t.refused_control.saturating_add(1);
    }
    result
}

/// Adds a finite group control. Members are stored in canonical address order and must be distinct.
pub fn group_suppress_create(
    t: &mut InhibitTable,
    cfg: &InhibitConfig,
    r: &GroupSuppressRequest<'_>,
    now_ns: TimeNs,
) -> Result<u32, InhibitError> {
    let result = (|| {
        if r.members.len() < 2
            || r.members.len() > GROUP_TRACK_CAP
            || r.purpose != GroupSuppressPurpose::MilitarySpecialOperation
            || !cfg.allow_military_group_suppression
            || !cfg
                .max_alert_suppression_ns
                .is_some_and(|limit| control_window(r.start_ns, r.end_ns, now_ns, limit))
        {
            return Err(InhibitError::BadAlertControl);
        }
        let (reason, author) = control_text_put(r.text)?;
        let mut members = [0u32; GROUP_TRACK_CAP];
        for (i, &addr) in r.members.iter().enumerate() {
            if addr == 0 {
                return Err(InhibitError::BadAlertControl);
            }
            let mut j = i;
            while j > 0 {
                let previous = members
                    .get(j - 1)
                    .copied()
                    .ok_or(InhibitError::BadAlertControl)?;
                if previous <= addr {
                    break;
                }
                *members.get_mut(j).ok_or(InhibitError::BadAlertControl)? = previous;
                j -= 1;
            }
            if j > 0 && members.get(j - 1).copied() == Some(addr) {
                return Err(InhibitError::BadAlertControl);
            }
            *members.get_mut(j).ok_or(InhibitError::BadAlertControl)? = addr;
        }
        if t.group_controls
            .iter()
            .take(t.group_controls_len)
            .any(|old| {
                let old_members = old
                    .members
                    .get(..usize::from(old.member_count))
                    .unwrap_or(&[]);
                old_members == members.get(..r.members.len()).unwrap_or(&[])
                    && r.start_ns < old.end_ns
                    && old.start_ns < r.end_ns
            })
        {
            return Err(InhibitError::BadAlertControl);
        }
        if t.group_controls_len >= GROUP_SUPPRESS_CAP {
            return Err(InhibitError::AlertControlFull);
        }
        if t.next_id == u32::MAX {
            return Err(InhibitError::AlertControlIdsExhausted);
        }
        let id = t.next_id;
        let Some(slot) = t.group_controls.get_mut(t.group_controls_len) else {
            return Err(InhibitError::AlertControlFull);
        };
        *slot = GroupSuppression {
            start_ns: r.start_ns,
            end_ns: r.end_ns,
            id,
            member_count: r.members.len() as u8,
            members,
            reason,
            author,
        };
        t.group_controls_len += 1;
        t.next_id += 1;
        Ok(id)
    })();
    if result.is_err() {
        t.refused_control = t.refused_control.saturating_add(1);
    }
    result
}

/// Reports whether a pair is explicitly controlled and records a suppression only for the actual
/// alert instance. Both aircraft must belong to a group; a single member does not suppress outsiders.
/// `work_budget` must cover the fixed worst case before any lookup begins.
pub fn alert_control_suppress(
    t: &mut InhibitTable,
    candidate: &AlertCandidate,
) -> Result<bool, InhibitError> {
    if candidate.control_work_budget < CONTROL_LOOKUP_WORK_MAX {
        t.control_work_refused = t.control_work_refused.saturating_add(1);
        return Err(InhibitError::AlertControlWorkLimit);
    }
    let (a, b) = if candidate.addr_a < candidate.addr_b {
        (candidate.addr_a, candidate.addr_b)
    } else {
        (candidate.addr_b, candidate.addr_a)
    };
    let mut matched = false;
    for i in 0..t.pair_controls_len {
        let Some(control) = t.pair_controls.get(i).copied() else {
            break;
        };
        if control.addr_a == a
            && control.addr_b == b
            && control.alert_start_ns == candidate.alert_start_ns
            && in_force(control.start_ns, control.end_ns, candidate.now_ns)
        {
            matched = true;
            log_control_suppression(t, candidate, a, b, control.id, 1);
        }
    }
    for i in 0..t.group_controls_len {
        let Some(control) = t.group_controls.get(i).copied() else {
            break;
        };
        if !in_force(control.start_ns, control.end_ns, candidate.now_ns) {
            continue;
        }
        let members = control
            .members
            .get(..usize::from(control.member_count))
            .unwrap_or(&[]);
        if members.contains(&a) && members.contains(&b) {
            matched = true;
            log_control_suppression(t, candidate, a, b, control.id, 2);
        }
    }
    Ok(matched)
}

fn log_control_suppression(
    t: &mut InhibitTable,
    candidate: &AlertCandidate,
    addr_a: u32,
    addr_b: u32,
    id: u32,
    source: u8,
) {
    let entry = Suppressed {
        time_ns: candidate.now_ns,
        alert_start_ns: candidate.alert_start_ns,
        inhibition_id: id,
        addr_a,
        addr_b,
        lat_e7: candidate.lat_e7,
        lon_e7: candidate.lon_e7,
        net: candidate.net,
        source,
    };
    if let Some(slot) = t.suppressed.get_mut(t.suppressed_len) {
        *slot = entry;
        t.suppressed_len += 1;
    } else {
        t.suppressed_overflow = t.suppressed_overflow.saturating_add(1);
        if let Some(n) = t.suppressed_overflow_net.get_mut(net_index(candidate.net)) {
            *n = n.saturating_add(1);
        }
    }
}

/// Number of refused explicit control requests.
pub fn alert_control_refused(t: &InhibitTable) -> u32 {
    t.refused_control
}

/// Number of control lookups refused because the caller supplied too little work budget.
pub fn alert_control_work_refused(t: &InhibitTable) -> u64 {
    t.control_work_refused
}

/// Pair-control records, creation order, including expired entries retained for audit.
pub fn pair_suppressions(t: &InhibitTable) -> &[PairSuppression] {
    t.pair_controls.get(..t.pair_controls_len).unwrap_or(&[])
}

/// Group-control records, creation order, including expired entries retained for audit.
pub fn group_suppressions(t: &InhibitTable) -> &[GroupSuppression] {
    t.group_controls.get(..t.group_controls_len).unwrap_or(&[])
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
            alert_start_ns: 0,
            inhibition_id: id,
            addr_a: 0,
            addr_b: 0,
            lat_e7,
            lon_e7,
            net,
            source: 0,
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

/// Area inhibition query and audit for one identified conflict alert. Each matching area
/// inhibition yields one acknowledgement record bound to the alert's canonical pair and start.
pub fn inhibit_suppress_pair(t: &mut InhibitTable, candidate: &AlertCandidate) -> Suppressors {
    let s = inhibit_suppressors(
        t,
        candidate.net,
        candidate.lat_e7,
        candidate.lon_e7,
        candidate.alt_ft,
        candidate.now_ns,
    );
    let (a, b) = if candidate.addr_a < candidate.addr_b {
        (candidate.addr_a, candidate.addr_b)
    } else {
        (candidate.addr_b, candidate.addr_a)
    };
    for &id in s.ids.iter().take(s.count as usize) {
        let entry = Suppressed {
            time_ns: candidate.now_ns,
            alert_start_ns: candidate.alert_start_ns,
            inhibition_id: id,
            addr_a: a,
            addr_b: b,
            lat_e7: candidate.lat_e7,
            lon_e7: candidate.lon_e7,
            net: candidate.net,
            source: 0,
        };
        if let Some(slot) = t.suppressed.get_mut(t.suppressed_len) {
            *slot = entry;
            t.suppressed_len += 1;
        } else {
            t.suppressed_overflow = t.suppressed_overflow.saturating_add(1);
            if let Some(n) = t.suppressed_overflow_net.get_mut(net_index(candidate.net)) {
                *n = n.saturating_add(1);
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

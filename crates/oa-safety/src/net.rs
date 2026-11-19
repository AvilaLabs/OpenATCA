//! The state of each safety net (**core**, B1, B2), M5 design rule 1: a net always says what state
//! it is in. Each net reports its state every cycle with the time; the table keeps the last report.
//! A net that stops reporting is unavailable, never silently running: a report older than the
//! staleness limit reads as `Unavailable(StateStale)`, and a net that has never reported as
//! `Unavailable(NeverReported)`. (Ueberlingen, 2002: conflict alert off in a fallback mode and
//! nothing said so; BFU recommendation 10/2004.)
//!
//! "Inhibited" is not reported by a net: it is derived from the inhibition table (at least one
//! inhibition in force for the net), so a net cannot claim it and an inhibition cannot be hidden.

use crate::inhibit::{inhibits_in_force, InhibitTable, TimeNs};

/// Number of safety nets.
pub const NET_COUNT: usize = 4;

/// The safety nets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NetId {
    ConflictAlert = 0,
    Msaw = 1,
    AreaProximity = 2,
    ApproachPath = 3,
}

/// Every net, in display order.
pub const NETS: [NetId; NET_COUNT] = [
    NetId::ConflictAlert,
    NetId::Msaw,
    NetId::AreaProximity,
    NetId::ApproachPath,
];

/// The net's position in tables indexed by net.
pub fn net_index(n: NetId) -> usize {
    n as usize
}

/// The name shown to a controller (the font has capitals, digits and a few marks).
pub fn net_name(n: NetId) -> &'static str {
    match n {
        NetId::ConflictAlert => "CONFLICT ALERT",
        NetId::Msaw => "MSAW",
        NetId::AreaProximity => "AREA PROXIMITY",
        NetId::ApproachPath => "APPROACH PATH",
    }
}

/// Why a net is degraded or unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    /// The cycle's work budget ran out: `count` is what was not evaluated (rule 5).
    BudgetExceeded = 1,
    /// The altimeter setting was refused as stale or missing (MSAW below the transition level).
    AltimeterRefused = 2,
    /// Surveillance for the area is lost or too old.
    SurveillanceLost = 3,
    /// Altitude data are missing for `count` tracks (rule 4).
    AltitudeMissing = 4,
    /// A required adaptation or terrain input is missing or refused.
    InputMissing = 5,
    /// Maintenance or a fallback mode.
    Maintenance = 6,
    /// The net's last report is older than the staleness limit. Set by the table, not by a net.
    StateStale = 7,
    /// The net has never reported. Set by the table.
    NeverReported = 8,
    /// The report is dated after the time of the query. Set by the table.
    TimeInconsistent = 9,
}

/// The text of a reason: at most 17 characters, so that the longest banner line fits its buffer.
pub fn reason_text(r: Reason) -> &'static str {
    match r {
        Reason::BudgetExceeded => "BUDGET EXCEEDED",
        Reason::AltimeterRefused => "ALTIMETER REFUSED",
        Reason::SurveillanceLost => "SURVEILLANCE LOST",
        Reason::AltitudeMissing => "ALTITUDE MISSING",
        Reason::InputMissing => "INPUT MISSING",
        Reason::Maintenance => "MAINTENANCE",
        Reason::StateStale => "STATE STALE",
        Reason::NeverReported => "NEVER REPORTED",
        Reason::TimeInconsistent => "STATE DATED AHEAD",
    }
}

/// The area a degradation affects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Whole,
    /// A polygon id of the adaptation.
    Polygon(u32),
}

/// What a net reports each cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetReport {
    Running,
    /// Running with reduced coverage: why, where, and how many items (pairs, tracks) were not
    /// evaluated.
    Degraded {
        reason: Reason,
        area: Area,
        count: u32,
    },
    Unavailable {
        reason: Reason,
    },
}

/// A net's state as shown: its report (after the staleness rule) or, for a running net with an
/// inhibition in force, `Inhibited`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetState {
    Running,
    Degraded {
        reason: Reason,
        area: Area,
        count: u32,
    },
    Inhibited,
    Unavailable {
        reason: Reason,
    },
}

/// One net at one time: its state, the inhibitions in force on it (also shown when it is degraded
/// or unavailable) and the age of the state (`None` when it has never reported or is dated in the
/// future).
// A7.2: cold struct, size guard waived (four per frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetStatus {
    pub net: NetId,
    pub state: NetState,
    pub inhibitions: u32,
    pub age_ns: Option<i64>,
}

/// True if the net is anything but plainly running: the display must say so.
pub fn status_needs_banner(s: &NetStatus) -> bool {
    s.state != NetState::Running
}

#[derive(Clone, Copy, Debug)]
// A7.2: cold struct, size guard waived (four per facility).
struct Slot {
    report: NetReport,
    at_ns: TimeNs,
    reported: bool,
}

/// The last report of each net and the staleness limit. Fixed size; no allocation.
// A7.2: cold struct, size guard waived (one per facility).
#[derive(Clone, Debug)]
pub struct StateTable {
    slots: [Slot; NET_COUNT],
    stale_limit_ns: i64,
    rejected: u32,
}

/// Why a state table or report was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// The staleness limit must be positive and is supplied by the configuration (no default).
    BadStaleLimit,
    /// The report is dated before the net's previous report.
    TimeBackwards,
}

/// A table with the staleness limit from the configuration. Every net starts as never reported.
pub fn state_table_new(stale_limit_ns: i64) -> Result<StateTable, StateError> {
    if stale_limit_ns <= 0 {
        return Err(StateError::BadStaleLimit);
    }
    Ok(StateTable {
        slots: [Slot {
            report: NetReport::Running,
            at_ns: 0,
            reported: false,
        }; NET_COUNT],
        stale_limit_ns,
        rejected: 0,
    })
}

/// Records a net's report at `at_ns`. A report dated before the net's previous one is refused and
/// counted; the previous report stands (and goes stale in time).
pub fn state_report(
    t: &mut StateTable,
    net: NetId,
    report: NetReport,
    at_ns: TimeNs,
) -> Result<(), StateError> {
    let Some(slot) = t.slots.get_mut(net_index(net)) else {
        return Ok(());
    };
    if slot.reported && at_ns < slot.at_ns {
        t.rejected = t.rejected.saturating_add(1);
        return Err(StateError::TimeBackwards);
    }
    *slot = Slot {
        report,
        at_ns,
        reported: true,
    };
    Ok(())
}

/// Reports refused as dated backwards.
pub fn state_rejected(t: &StateTable) -> u32 {
    t.rejected
}

/// The staleness limit, nanoseconds.
pub fn state_stale_limit(t: &StateTable) -> i64 {
    t.stale_limit_ns
}

/// The status of `net` at `now_ns`. A report older than the limit (strictly: age over the limit)
/// is `Unavailable(StateStale)`; never reported is `Unavailable(NeverReported)`; a report dated
/// after `now_ns` is `Unavailable(TimeInconsistent)`. A running net with an inhibition in force is
/// `Inhibited`. Work: one pass over the inhibition table.
pub fn net_status(st: &StateTable, inh: &InhibitTable, net: NetId, now_ns: TimeNs) -> NetStatus {
    let inhibitions = inhibits_in_force(inh, net, now_ns);
    let unavailable = |reason, age_ns| NetStatus {
        net,
        state: NetState::Unavailable { reason },
        inhibitions,
        age_ns,
    };
    let Some(slot) = st.slots.get(net_index(net)) else {
        return unavailable(Reason::NeverReported, None);
    };
    if !slot.reported {
        return unavailable(Reason::NeverReported, None);
    }
    let Some(age) = now_ns.checked_sub(slot.at_ns) else {
        return unavailable(Reason::TimeInconsistent, None);
    };
    if age < 0 {
        return unavailable(Reason::TimeInconsistent, None);
    }
    if age > st.stale_limit_ns {
        return unavailable(Reason::StateStale, Some(age));
    }
    let state = match slot.report {
        NetReport::Running if inhibitions > 0 => NetState::Inhibited,
        NetReport::Running => NetState::Running,
        NetReport::Degraded {
            reason,
            area,
            count,
        } => NetState::Degraded {
            reason,
            area,
            count,
        },
        NetReport::Unavailable { reason } => NetState::Unavailable { reason },
    };
    NetStatus {
        net,
        state,
        inhibitions,
        age_ns: Some(age),
    }
}

/// The status of every net, in [`NETS`] order.
pub fn net_statuses(st: &StateTable, inh: &InhibitTable, now_ns: TimeNs) -> [NetStatus; NET_COUNT] {
    NETS.map(|n| net_status(st, inh, n, now_ns))
}

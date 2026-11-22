//! The state of each safety net (**core**, B1, B2), M5 design rule 1: a net always says what state
//! it is in. Each net reports its state every cycle with the time; the table keeps the last report.
//! A net that stops reporting is unavailable, never silently running: a report older than the
//! staleness limit reads as `Unavailable(StateStale)`, and a net that has never reported as
//! `Unavailable(NeverReported)`. (Ueberlingen, 2002: conflict alert off in a fallback mode and
//! nothing said so; BFU recommendation 10/2004.)
//!
//! "Inhibited" is not reported by a net: it is derived from the inhibition table (at least one
//! inhibition in force for the net), so a net cannot claim it and an inhibition cannot be hidden.

use crate::inhibit::{inhibits_in_force, suppressed_overflow_net, InhibitTable, TimeNs};

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
    /// Suppressions on the net were lost because the suppressed-alert log was full: what an
    /// inhibition hid is no longer fully recorded. Set by the table, not by a net.
    SuppressionLogFull = 10,
    /// A track's horizontal uncertainty radius was invalid or exceeded the configured cap and was
    /// held to that cap.
    RadiusCapped = 11,
}

/// The text of a reason: at most 20 characters, so that the longest banner line fits its buffer.
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
        Reason::SuppressionLogFull => "SUPPRESSION LOG FULL",
        Reason::RadiusCapped => "UNCERTAINTY CAPPED",
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
    /// Running with a shortfall: reason, area, and count of affected items. The count may be work
    /// left unevaluated or input items that were missing, rejected, or altered.
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
    /// The last report offered was refused as dated too far ahead; cleared by the next accepted one.
    bad_time: bool,
}

/// The last report of each net and the staleness limit. Fixed size; no allocation.
// A7.2: cold struct, size guard waived (one per facility).
#[derive(Clone, Debug)]
pub struct StateTable {
    slots: [Slot; NET_COUNT],
    stale_limit_ns: i64,
    /// A report may be dated at most this far after the report-time clock.
    future_horizon_ns: i64,
    rejected: u32,
    future_rejected: u32,
    backdated_applied: u32,
}

/// Why a state table or report was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// The staleness limit must be positive and is supplied by the configuration (no default).
    BadStaleLimit,
    /// The future horizon must not be negative and is supplied by the configuration.
    BadHorizon,
    /// The report is dated before the net's previous report and is not worse than it.
    TimeBackwards,
    /// The report is dated further ahead of the report-time clock than the horizon allows.
    TimeAhead,
}

/// A table with the staleness limit and the future horizon from the configuration (neither has a
/// default). Every net starts as never reported.
pub fn state_table_new(
    stale_limit_ns: i64,
    future_horizon_ns: i64,
) -> Result<StateTable, StateError> {
    if stale_limit_ns <= 0 {
        return Err(StateError::BadStaleLimit);
    }
    if future_horizon_ns < 0 {
        return Err(StateError::BadHorizon);
    }
    Ok(StateTable {
        slots: [Slot {
            report: NetReport::Running,
            at_ns: 0,
            reported: false,
            bad_time: false,
        }; NET_COUNT],
        stale_limit_ns,
        future_horizon_ns,
        rejected: 0,
        future_rejected: 0,
        backdated_applied: 0,
    })
}

/// How bad a report is: running 0, degraded 1, unavailable 2.
fn severity(r: &NetReport) -> u8 {
    match r {
        NetReport::Running => 0,
        NetReport::Degraded { .. } => 1,
        NetReport::Unavailable { .. } => 2,
    }
}

/// Records a net's report dated `at_ns`, offered at `now_ns` on the report-time clock.
///
/// * Dated more than the horizon after `now_ns`: refused and counted, and the net reads
///   `Unavailable(TimeInconsistent)` until a valid report arrives. The stored report is untouched,
///   so one bad timestamp cannot lock the net out.
/// * Dated before the stored report: a report that is worse than the stored one (unavailable over
///   degraded over running) replaces its state at once and keeps the later timestamp, so a late
///   bad news is never lost; any other backdated report is refused and counted.
/// * Otherwise it replaces the stored report.
pub fn state_report(
    t: &mut StateTable,
    net: NetId,
    report: NetReport,
    at_ns: TimeNs,
    now_ns: TimeNs,
) -> Result<(), StateError> {
    let horizon = t.future_horizon_ns;
    let Some(slot) = t.slots.get_mut(net_index(net)) else {
        return Ok(());
    };
    if at_ns > now_ns.saturating_add(horizon) {
        slot.bad_time = true;
        t.future_rejected = t.future_rejected.saturating_add(1);
        return Err(StateError::TimeAhead);
    }
    if slot.reported && at_ns < slot.at_ns {
        if severity(&report) > severity(&slot.report) {
            slot.report = report;
            slot.bad_time = false;
            t.backdated_applied = t.backdated_applied.saturating_add(1);
            return Ok(());
        }
        t.rejected = t.rejected.saturating_add(1);
        return Err(StateError::TimeBackwards);
    }
    *slot = Slot {
        report,
        at_ns,
        reported: true,
        bad_time: false,
    };
    Ok(())
}

/// Reports refused, dated before the stored one without being worse, or too far ahead.
pub fn state_rejected(t: &StateTable) -> u32 {
    t.rejected.saturating_add(t.future_rejected)
}

/// Reports refused as dated too far ahead of the report-time clock.
pub fn state_future_rejected(t: &StateTable) -> u32 {
    t.future_rejected
}

/// Backdated reports that were worse than the stored one and replaced its state.
pub fn state_backdated_applied(t: &StateTable) -> u32 {
    t.backdated_applied
}

/// The staleness limit, nanoseconds.
pub fn state_stale_limit(t: &StateTable) -> i64 {
    t.stale_limit_ns
}

/// The status of `net` at `now_ns`. A report older than the limit (strictly: an age equal to the
/// limit is still current) is `Unavailable(StateStale)`; a net whose last offered report was
/// refused as dated too far ahead is `Unavailable(TimeInconsistent)`; never reported is `Unavailable(NeverReported)`; a report dated
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
    if slot.bad_time {
        return unavailable(Reason::TimeInconsistent, None);
    }
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
    // Suppressions were lost on this net since the log was last cleared: say so, whatever else.
    let lost = suppressed_overflow_net(inh, net);
    let state = match slot.report {
        NetReport::Running if lost > 0 => NetState::Degraded {
            reason: Reason::SuppressionLogFull,
            area: Area::Whole,
            count: u32::try_from(lost).unwrap_or(u32::MAX),
        },
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

#[cfg(kani)]
mod proofs {
    use super::*;
    use crate::inhibit::inhibits_new;

    // REQ: OA-SAF-016
    #[kani::proof]
    fn future_timestamp_refusal_is_unavailable_until_valid_recovery() {
        let stale_limit_ns: i64 = kani::any();
        let future_horizon_ns: i64 = kani::any();
        let now_ns: i64 = kani::any();
        kani::assume(stale_limit_ns > 0);
        kani::assume(future_horizon_ns >= 0);

        let last_acceptable_ns = now_ns.saturating_add(future_horizon_ns);
        let mut states = state_table_new(stale_limit_ns, future_horizon_ns).unwrap();
        let inhibits = inhibits_new();
        let before = net_status(&states, &inhibits, NetId::Msaw, now_ns);
        assert_eq!(
            before.state,
            NetState::Unavailable {
                reason: Reason::NeverReported
            }
        );

        if last_acceptable_ns < i64::MAX {
            let future_ns = last_acceptable_ns + 1;
            kani::cover!(
                last_acceptable_ns < i64::MAX,
                "representable future timestamp refused"
            );
            kani::cover!(future_ns == i64::MAX, "last representable future refused");
            assert_eq!(
                state_report(
                    &mut states,
                    NetId::Msaw,
                    NetReport::Running,
                    future_ns,
                    now_ns
                ),
                Err(StateError::TimeAhead)
            );
            assert_eq!(state_future_rejected(&states), 1);
            assert_eq!(state_rejected(&states), 1);
            kani::cover!(
                state_future_rejected(&states) == 1,
                "future refusal counted"
            );
            assert_eq!(
                net_status(&states, &inhibits, NetId::Msaw, now_ns).state,
                NetState::Unavailable {
                    reason: Reason::TimeInconsistent
                }
            );

            assert_eq!(
                state_report(&mut states, NetId::Msaw, NetReport::Running, now_ns, now_ns),
                Ok(())
            );
            let recovered = net_status(&states, &inhibits, NetId::Msaw, now_ns);
            assert_eq!(recovered.state, NetState::Running);
            assert_eq!(recovered.age_ns, Some(0));
            assert_eq!(state_future_rejected(&states), 1);
            assert_eq!(state_rejected(&states), 1);
            kani::cover!(
                recovered.state == NetState::Running,
                "valid report restores running state"
            );
        } else {
            // A saturated cutoff admits every representable timestamp, including i64::MAX.
            assert_eq!(last_acceptable_ns, i64::MAX);
            kani::cover!(
                last_acceptable_ns == i64::MAX,
                "saturated horizon admits maximum timestamp"
            );
            assert_eq!(
                state_report(
                    &mut states,
                    NetId::Msaw,
                    NetReport::Running,
                    i64::MAX,
                    now_ns
                ),
                Ok(())
            );
            let accepted = net_status(&states, &inhibits, NetId::Msaw, i64::MAX);
            assert_eq!(accepted.state, NetState::Running);
            assert_eq!(accepted.age_ns, Some(0));
            assert_eq!(state_future_rejected(&states), 0);
            assert_eq!(state_rejected(&states), 0);
        }
    }
}

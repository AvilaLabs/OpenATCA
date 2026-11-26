//! The net state table and the inhibition table: staleness, validation, in-force boundaries,
//! suppression, the suppressed-alert log, removal and expiry.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_safety::geom::{polygon_contains, polygon_new, Vertex, MAX_VERTICES};
use oa_safety::inhibit::*;
use oa_safety::net::*;

const S: i64 = 1_000_000_000;
const H: i64 = 3600 * S;

/// A square 0.2 degrees on a side about 42 N, 88 W, in 1e-7 degree.
const SQUARE: [Vertex; 4] = [
    (419_000_000, -881_000_000),
    (421_000_000, -881_000_000),
    (421_000_000, -879_000_000),
    (419_000_000, -879_000_000),
];
const INSIDE: (i32, i32) = (420_000_000, -880_000_000);
const OUTSIDE: (i32, i32) = (420_000_000, -870_000_000);

/// A report dated `at`, offered at `at` (the report-time clock reads the same instant).
fn report(t: &mut StateTable, net: NetId, r: NetReport, at: i64) -> Result<(), StateError> {
    state_report(t, net, r, at, at)
}

fn table_new(limit: i64) -> Result<StateTable, StateError> {
    state_table_new(limit, 5 * S)
}

fn cfg(hours: i64) -> InhibitConfig {
    InhibitConfig {
        max_duration_ns: [Some(hours * H); NET_COUNT],
        max_start_ahead_ns: Some(24 * H),
        max_extent_e7: Some(20_000_000),
        max_alert_suppression_ns: Some(hours * H),
        allow_military_group_suppression: true,
    }
}

fn req<'a>(net: NetId, start: i64, end: i64) -> InhibitRequest<'a> {
    InhibitRequest {
        net,
        vertices: &SQUARE,
        alt_low_ft: 1000,
        alt_high_ft: 5000,
        start_ns: start,
        end_ns: end,
        reason: "RANGE 3 TEST FIRING",
        author: "SUP J SMITH",
    }
}

fn alert_candidate(
    addr_a: u32,
    addr_b: u32,
    alert_start_ns: i64,
    now_ns: i64,
    work: u32,
) -> AlertCandidate {
    AlertCandidate {
        net: NetId::ConflictAlert,
        addr_a,
        addr_b,
        alert_start_ns,
        lat_e7: INSIDE.0,
        lon_e7: INSIDE.1,
        alt_ft: Some(2000),
        now_ns,
        control_work_budget: work,
    }
}

fn table() -> Box<InhibitTable> {
    Box::new(inhibits_new())
}

// REQ: OA-SAF-001
#[test]
fn a_net_that_never_reported_or_stopped_reporting_is_unavailable() {
    let inh = table();
    let mut st = table_new(5 * S).unwrap();
    // Never reported.
    let s = net_status(&st, &inh, NetId::Msaw, 100 * S);
    assert_eq!(
        s.state,
        NetState::Unavailable {
            reason: Reason::NeverReported
        }
    );
    assert_eq!(s.age_ns, None);
    assert!(status_needs_banner(&s));
    report(&mut st, NetId::Msaw, NetReport::Running, 100 * S).unwrap();
    // Fresh, at the limit (not over it) and one ns over it.
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 100 * S).state,
        NetState::Running
    );
    let at = net_status(&st, &inh, NetId::Msaw, 105 * S);
    assert_eq!(at.state, NetState::Running);
    assert_eq!(at.age_ns, Some(5 * S));
    let over = net_status(&st, &inh, NetId::Msaw, 105 * S + 1);
    assert_eq!(
        over.state,
        NetState::Unavailable {
            reason: Reason::StateStale
        }
    );
    assert_eq!(over.age_ns, Some(5 * S + 1));
    // Another net is untouched by it.
    assert_eq!(
        net_status(&st, &inh, NetId::ConflictAlert, 100 * S).state,
        NetState::Unavailable {
            reason: Reason::NeverReported
        }
    );
    // A report dated after the query is not "running".
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 99 * S).state,
        NetState::Unavailable {
            reason: Reason::TimeInconsistent
        }
    );
    // Extreme times do not panic.
    let _ = net_status(&st, &inh, NetId::Msaw, i64::MIN);
    let _ = net_status(&st, &inh, NetId::Msaw, i64::MAX);
}

// REQ: OA-SAF-002
#[test]
fn the_staleness_limit_comes_from_configuration_and_reports_cannot_go_back() {
    assert_eq!(
        state_table_new(0, S).unwrap_err(),
        StateError::BadStaleLimit
    );
    assert_eq!(
        state_table_new(-1, S).unwrap_err(),
        StateError::BadStaleLimit
    );
    assert_eq!(state_table_new(S, -1).unwrap_err(), StateError::BadHorizon);
    let mut st = table_new(S).unwrap();
    assert_eq!(state_stale_limit(&st), S);
    report(&mut st, NetId::Msaw, NetReport::Running, 10 * S).unwrap();
    assert_eq!(
        report(&mut st, NetId::Msaw, NetReport::Running, 9 * S).unwrap_err(),
        StateError::TimeBackwards
    );
    assert_eq!(state_rejected(&st), 1);
    // The refused report did not replace the stored one.
    let inh = table();
    assert_eq!(net_status(&st, &inh, NetId::Msaw, 10 * S).age_ns, Some(0));
}

// REQ: OA-SAF-003
#[test]
fn each_state_is_reported_with_its_reason_area_count_and_age() {
    let inh = table();
    let mut st = table_new(10 * S).unwrap();
    report(
        &mut st,
        NetId::ConflictAlert,
        NetReport::Degraded {
            reason: Reason::BudgetExceeded,
            area: Area::Polygon(7),
            count: 12,
        },
        50 * S,
    )
    .unwrap();
    report(
        &mut st,
        NetId::Msaw,
        NetReport::Unavailable {
            reason: Reason::AltimeterRefused,
        },
        50 * S,
    )
    .unwrap();
    report(&mut st, NetId::AreaProximity, NetReport::Running, 50 * S).unwrap();
    let all = net_statuses(&st, &inh, 53 * S);
    assert_eq!(all.map(|s| s.net), NETS);
    assert_eq!(
        all[0].state,
        NetState::Degraded {
            reason: Reason::BudgetExceeded,
            area: Area::Polygon(7),
            count: 12
        }
    );
    assert_eq!(all[0].age_ns, Some(3 * S));
    assert_eq!(
        all[1].state,
        NetState::Unavailable {
            reason: Reason::AltimeterRefused
        }
    );
    assert_eq!(all[2].state, NetState::Running);
    assert!(!status_needs_banner(&all[2]));
    // The approach path net never reported.
    assert!(status_needs_banner(&all[3]));
    for n in NETS {
        assert!(!net_name(n).is_empty());
    }
    assert!(!reason_text(Reason::StateStale).is_empty());
}

// REQ: OA-SAF-004
#[test]
fn a_running_net_with_an_inhibition_in_force_is_inhibited_and_a_degraded_one_says_both() {
    let mut inh = table();
    let mut st = table_new(10 * S).unwrap();
    report(&mut st, NetId::Msaw, NetReport::Running, 100 * S).unwrap();
    report(
        &mut st,
        NetId::ConflictAlert,
        NetReport::Degraded {
            reason: Reason::SurveillanceLost,
            area: Area::Whole,
            count: 1,
        },
        100 * S,
    )
    .unwrap();
    let id = inhibit_create(
        &mut inh,
        &cfg(8),
        &req(NetId::Msaw, 100 * S, 100 * S + H),
        100 * S,
    )
    .unwrap();
    inhibit_create(
        &mut inh,
        &cfg(8),
        &req(NetId::ConflictAlert, 100 * S, 100 * S + H),
        100 * S,
    )
    .unwrap();
    let m = net_status(&st, &inh, NetId::Msaw, 101 * S);
    assert_eq!(m.state, NetState::Inhibited);
    assert_eq!(m.inhibitions, 1);
    let c = net_status(&st, &inh, NetId::ConflictAlert, 101 * S);
    assert!(matches!(c.state, NetState::Degraded { .. }));
    assert_eq!(c.inhibitions, 1);
    // After the inhibition ends the net reads as running again (its report is still fresh).
    report(&mut st, NetId::Msaw, NetReport::Running, 100 * S + H).unwrap();
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 100 * S + H).state,
        NetState::Running
    );
    assert!(id > 0);
}

// REQ: OA-SAF-005
#[test]
fn an_invalid_inhibition_is_refused_and_counted() {
    let mut inh = table();
    let now = 1000 * S;
    let ok = req(NetId::Msaw, now, now + H);
    // No maximum configured: the configuration must supply one.
    assert_eq!(
        inhibit_create(&mut inh, &inhibit_config_none(), &ok, now).unwrap_err(),
        InhibitError::NoMaxConfigured
    );
    // Absent for this net only.
    let mut c = cfg(8);
    c.max_duration_ns[net_index(NetId::Msaw)] = None;
    assert_eq!(
        inhibit_create(&mut inh, &c, &ok, now).unwrap_err(),
        InhibitError::NoMaxConfigured
    );
    // Not a positive maximum.
    c.max_duration_ns[net_index(NetId::Msaw)] = Some(0);
    assert_eq!(
        inhibit_create(&mut inh, &c, &ok, now).unwrap_err(),
        InhibitError::NoMaxConfigured
    );
    let c = cfg(2);
    // End equal to or before the start.
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, now, now), now).unwrap_err(),
        InhibitError::EndNotAfterStart
    );
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, now + 2, now + 1), now).unwrap_err(),
        InhibitError::EndNotAfterStart
    );
    // Already over.
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, now - H, now), now).unwrap_err(),
        InhibitError::AlreadyExpired
    );
    // One ns over the maximum is too long; exactly the maximum is accepted.
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, now, now + 2 * H + 1), now).unwrap_err(),
        InhibitError::TooLong
    );
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, i64::MIN + 1, i64::MAX), now).unwrap_err(),
        InhibitError::TooLong
    );
    // Bad polygon.
    let line = [(0, 0), (10, 10), (20, 20)];
    let mut r = req(NetId::Msaw, now, now + H);
    r.vertices = &line;
    assert!(matches!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::BadPolygon(_)
    ));
    // Band reversed.
    let mut r = req(NetId::Msaw, now, now + H);
    r.alt_low_ft = 6000;
    assert_eq!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::BadBand
    );
    // Text.
    let mut r = req(NetId::Msaw, now, now + H);
    r.reason = "";
    assert_eq!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::EmptyReason
    );
    r.reason = "OK";
    r.author = "";
    assert_eq!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::EmptyAuthor
    );
    r.author = "A NAME WAY TOO LONG FOR THE FIELD";
    assert!(matches!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::BadText(_)
    ));
    r.author = "CAF\u{e9}";
    assert!(matches!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::BadText(_)
    ));
    assert_eq!(inhibits_refused_invalid(&inh), 14);
    assert!(inhibits_slice(&inh).is_empty());
    // The accepted boundary.
    let id = inhibit_create(&mut inh, &c, &req(NetId::Msaw, now, now + 2 * H), now).unwrap();
    assert_eq!(id, 1);
    let i = inhibits_slice(&inh)[0];
    assert_eq!(text_of(&i.reason), "RANGE 3 TEST FIRING");
    assert_eq!(text_of(&i.author), "SUP J SMITH");
}

// REQ: OA-SAF-006
#[test]
fn a_full_table_refuses_and_counts_and_never_evicts() {
    let mut inh = table();
    let now = 10 * S;
    let c = cfg(1);
    for k in 0..INHIBIT_CAP {
        let id = inhibit_create(&mut inh, &c, &req(NetId::Msaw, now, now + H), now).unwrap();
        assert_eq!(id as usize, k + 1);
    }
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, now, now + H), now).unwrap_err(),
        InhibitError::TableFull
    );
    assert_eq!(inhibits_refused_full(&inh), 1);
    assert_eq!(inhibits_slice(&inh).len(), INHIBIT_CAP);
    assert_eq!(inhibits_slice(&inh)[0].id, 1);
    // Room again after a removal; the next id is new, never reused.
    inhibit_remove(&mut inh, 5, "SUP J SMITH", now).unwrap();
    let id = inhibit_create(&mut inh, &c, &req(NetId::Msaw, now, now + H), now).unwrap();
    assert_eq!(id as usize, INHIBIT_CAP + 1);
}

// REQ: OA-SAF-007
#[test]
fn an_inhibition_is_in_force_from_its_start_to_just_before_its_end() {
    assert!(!in_force(100, 200, 99));
    assert!(in_force(100, 200, 100));
    assert!(in_force(100, 200, 199));
    assert!(!in_force(100, 200, 200));
    assert!(!in_force(100, 100, 100));
    assert!(!in_force(200, 100, 150));
    let mut inh = table();
    inhibit_create(&mut inh, &cfg(1), &req(NetId::Msaw, 100 * S, 200 * S), 0).unwrap();
    for (t, n) in [
        (100 * S - 1, 0),
        (100 * S, 1),
        (200 * S - 1, 1),
        (200 * S, 0),
    ] {
        assert_eq!(inhibits_in_force(&inh, NetId::Msaw, t), n, "t = {t}");
    }
    assert_eq!(inhibits_in_force(&inh, NetId::ConflictAlert, 150 * S), 0);
}

// REQ: OA-SAF-008
#[test]
fn the_altitude_band_is_inclusive_at_both_ends() {
    assert!(!in_band(1000, 5000, 999));
    assert!(in_band(1000, 5000, 1000));
    assert!(in_band(1000, 5000, 5000));
    assert!(!in_band(1000, 5000, 5001));
    assert!(!in_band(5000, 1000, 3000));
}

// REQ: OA-SAF-009
#[test]
fn the_query_names_the_inhibitions_that_hide_a_position_altitude_and_time() {
    let mut inh = table();
    let c = cfg(1);
    let a = inhibit_create(&mut inh, &c, &req(NetId::Msaw, 0, H), 0).unwrap();
    let mut r2 = req(NetId::Msaw, 0, H);
    r2.alt_low_ft = 4000;
    r2.alt_high_ft = 9000;
    let b = inhibit_create(&mut inh, &c, &r2, 0).unwrap();
    let t = 10 * S;
    let q = |net, pos: (i32, i32), alt, t| inhibit_suppressors(&inh, net, pos.0, pos.1, alt, t);
    let s = q(NetId::Msaw, INSIDE, Some(3000), t);
    assert_eq!((s.count, s.ids[0]), (1, a));
    let s = q(NetId::Msaw, INSIDE, Some(4500), t);
    assert_eq!((s.count, s.ids[0], s.ids[1]), (2, a, b));
    // Band edges are inclusive.
    assert_eq!(q(NetId::Msaw, INSIDE, Some(1000), t).count, 1);
    assert_eq!(q(NetId::Msaw, INSIDE, Some(5000), t).count, 2);
    assert_eq!(q(NetId::Msaw, INSIDE, Some(9000), t).count, 1);
    assert_eq!(q(NetId::Msaw, INSIDE, Some(9001), t).count, 0);
    assert_eq!(q(NetId::Msaw, INSIDE, Some(999), t).count, 0);
    // Outside the area, another net, outside the time.
    assert_eq!(q(NetId::Msaw, OUTSIDE, Some(3000), t).count, 0);
    assert_eq!(q(NetId::ConflictAlert, INSIDE, Some(3000), t).count, 0);
    assert_eq!(q(NetId::Msaw, INSIDE, Some(3000), H).count, 0);
    assert_eq!(q(NetId::Msaw, INSIDE, Some(3000), -1).count, 0);
    // An unknown altitude is never hidden.
    assert_eq!(q(NetId::Msaw, INSIDE, None, t).count, 0);
    // A point on the boundary or on a vertex is not hidden.
    assert_eq!(
        q(NetId::Msaw, (419_000_000, -880_000_000), Some(3000), t).count,
        0
    );
    assert_eq!(q(NetId::Msaw, SQUARE[0], Some(3000), t).count, 0);
    // The pure query logged nothing.
    assert!(suppressed_log(&inh).is_empty());
}

// REQ: OA-SAF-010
#[test]
fn every_suppression_is_logged_and_a_full_log_counts_the_overflow() {
    let mut inh = table();
    let a = inhibit_create(&mut inh, &cfg(1), &req(NetId::Msaw, 0, H), 0).unwrap();
    let s = inhibit_suppress(&mut inh, NetId::Msaw, INSIDE.0, INSIDE.1, Some(2000), 5 * S);
    assert_eq!(s.count, 1);
    // A candidate nothing hides is not logged.
    let s = inhibit_suppress(
        &mut inh,
        NetId::Msaw,
        OUTSIDE.0,
        OUTSIDE.1,
        Some(2000),
        6 * S,
    );
    assert_eq!(s.count, 0);
    assert_eq!(
        suppressed_log(&inh),
        &[Suppressed {
            time_ns: 5 * S,
            alert_start_ns: 0,
            inhibition_id: a,
            addr_a: 0,
            addr_b: 0,
            lat_e7: INSIDE.0,
            lon_e7: INSIDE.1,
            net: NetId::Msaw,
            source: 0,
        }]
    );
    for k in 1..SUPPRESSED_CAP {
        inhibit_suppress(
            &mut inh,
            NetId::Msaw,
            INSIDE.0,
            INSIDE.1,
            Some(2000),
            7 * S + k as i64,
        );
    }
    assert_eq!(suppressed_log(&inh).len(), SUPPRESSED_CAP);
    assert_eq!(suppressed_overflow(&inh), 0);
    for _ in 0..3 {
        let s = inhibit_suppress(&mut inh, NetId::Msaw, INSIDE.0, INSIDE.1, Some(2000), 9 * S);
        // The suppression still applies; the lost entry is counted.
        assert_eq!(s.count, 1);
    }
    assert_eq!(suppressed_overflow(&inh), 3);
    assert_eq!(suppressed_log(&inh).len(), SUPPRESSED_CAP);
    assert_eq!(suppressed_log(&inh)[0].time_ns, 5 * S);
    // The loss shows on the net: running reads degraded while the lost entries stand.
    let mut st = table_new(10 * S).unwrap();
    report(&mut st, NetId::Msaw, NetReport::Running, 9 * S).unwrap();
    let ns = net_status(&st, &inh, NetId::Msaw, 9 * S);
    assert_eq!(
        ns.state,
        NetState::Degraded {
            reason: Reason::SuppressionLogFull,
            area: Area::Whole,
            count: 3
        }
    );
    assert_eq!(suppressed_overflow_net(&inh, NetId::Msaw), 3);
    assert_eq!(suppressed_overflow_net(&inh, NetId::ConflictAlert), 0);
    suppressed_clear(&mut inh);
    assert!(suppressed_log(&inh).is_empty());
    assert_eq!(suppressed_overflow(&inh), 3);
    assert_eq!(suppressed_overflow_net(&inh, NetId::Msaw), 0);
}

// REQ: OA-SAF-021, OA-SAF-023
#[test]
fn pair_suppression_acknowledges_only_the_named_alert_instance() {
    let mut inh = table();
    let id = pair_suppress_create(
        &mut inh,
        &cfg(1),
        &PairSuppressRequest {
            addr_a: 22,
            addr_b: 11,
            alert_start_ns: 4 * S,
            start_ns: 5 * S,
            end_ns: 10 * S,
            text: AlertControlText {
                reason: "COORDINATED",
                author: "SUP A",
            },
        },
        5 * S,
    )
    .unwrap();
    assert!(alert_control_suppress(
        &mut inh,
        &alert_candidate(11, 22, 4 * S, 5 * S, CONTROL_LOOKUP_WORK_MAX)
    )
    .unwrap());
    let receipt = suppressed_log(&inh).last().unwrap();
    assert_eq!(
        (
            receipt.inhibition_id,
            receipt.alert_start_ns,
            receipt.addr_a,
            receipt.addr_b,
            receipt.source
        ),
        (id, 4 * S, 11, 22, 1)
    );
    assert!(
        !alert_control_suppress(
            &mut inh,
            &alert_candidate(11, 22, 11 * S, 6 * S, CONTROL_LOOKUP_WORK_MAX)
        )
        .unwrap(),
        "a later alert is not acknowledged by the old entry"
    );
    assert!(!alert_control_suppress(
        &mut inh,
        &alert_candidate(11, 23, 4 * S, 6 * S, CONTROL_LOOKUP_WORK_MAX)
    )
    .unwrap());
    assert_eq!(
        alert_control_suppress(&mut inh, &alert_candidate(11, 22, 4 * S, 6 * S, 0)),
        Err(InhibitError::AlertControlWorkLimit)
    );
    assert_eq!(alert_control_work_refused(&inh), 1);
    let bad = PairSuppressRequest {
        addr_a: 7,
        addr_b: 7,
        alert_start_ns: 4 * S,
        start_ns: 5 * S,
        end_ns: 10 * S,
        text: AlertControlText {
            reason: "COORDINATED",
            author: "SUP A",
        },
    };
    assert_eq!(
        pair_suppress_create(&mut inh, &cfg(1), &bad, 5 * S),
        Err(InhibitError::BadAlertControl)
    );
    assert_eq!(alert_control_refused(&inh), 1);
}

// REQ: OA-SAF-022, OA-SAF-023
#[test]
fn group_suppression_requires_both_aircraft_to_belong_to_the_selected_set() {
    let mut inh = table();
    group_suppress_create(
        &mut inh,
        &cfg(1),
        &GroupSuppressRequest {
            members: &[30, 10, 20],
            start_ns: 5 * S,
            end_ns: 10 * S,
            purpose: GroupSuppressPurpose::MilitarySpecialOperation,
            text: AlertControlText {
                reason: "MIL SPECIAL OPS",
                author: "SUP A",
            },
        },
        5 * S,
    )
    .unwrap();
    assert!(alert_control_suppress(
        &mut inh,
        &alert_candidate(10, 30, 5 * S, 6 * S, CONTROL_LOOKUP_WORK_MAX)
    )
    .unwrap());
    assert!(!alert_control_suppress(
        &mut inh,
        &alert_candidate(10, 40, 5 * S, 6 * S, CONTROL_LOOKUP_WORK_MAX)
    )
    .unwrap());
    assert!(!alert_control_suppress(
        &mut inh,
        &alert_candidate(40, 50, 5 * S, 6 * S, CONTROL_LOOKUP_WORK_MAX)
    )
    .unwrap());
    assert_eq!(suppressed_log(&inh).len(), 1);
    assert_eq!(suppressed_log(&inh)[0].source, 2);
}

// REQ: OA-SAF-022
#[test]
fn group_suppression_refuses_disabled_facility_policy_and_nonmilitary_purpose() {
    let mut inh = table();
    let request = GroupSuppressRequest {
        members: &[10, 20],
        start_ns: 5 * S,
        end_ns: 10 * S,
        purpose: GroupSuppressPurpose::MilitarySpecialOperation,
        text: AlertControlText {
            reason: "MIL SPECIAL OPS",
            author: "SUP A",
        },
    };
    let mut disabled = cfg(1);
    disabled.allow_military_group_suppression = false;
    assert_eq!(
        group_suppress_create(&mut inh, &disabled, &request, 5 * S),
        Err(InhibitError::BadAlertControl)
    );
    let mut wrong = request;
    wrong.purpose = GroupSuppressPurpose::Other;
    assert_eq!(
        group_suppress_create(&mut inh, &cfg(1), &wrong, 5 * S),
        Err(InhibitError::BadAlertControl)
    );
    assert_eq!(alert_control_refused(&inh), 2);
}

// REQ: OA-SAF-011
#[test]
fn an_explicit_removal_is_logged_with_who_did_it() {
    let mut inh = table();
    let id = inhibit_create(&mut inh, &cfg(1), &req(NetId::Msaw, 0, H), 0).unwrap();
    assert_eq!(
        inhibit_remove(&mut inh, id + 1, "SUP B JONES", S).unwrap_err(),
        RemoveError::NotFound
    );
    assert_eq!(
        inhibit_remove(&mut inh, id, "", S).unwrap_err(),
        RemoveError::BadAuthor
    );
    assert_eq!(inhibits_slice(&inh).len(), 1);
    inhibit_remove(&mut inh, id, "SUP B JONES", 2 * S).unwrap();
    assert!(inhibits_slice(&inh).is_empty());
    assert_eq!(
        inhibit_suppressors(&inh, NetId::Msaw, INSIDE.0, INSIDE.1, Some(2000), 3 * S).count,
        0
    );
    let mut ev = [EVENT_EMPTY_FOR_TEST; 8];
    let n = events_take(&mut inh, &mut ev);
    assert_eq!(n, 2);
    assert_eq!(
        (ev[0].kind, ev[0].id, ev[0].time_ns),
        (EventKind::Created, id, 0)
    );
    assert_eq!(text_of(&ev[0].by), "SUP J SMITH");
    assert_eq!(
        (ev[1].kind, ev[1].id, ev[1].time_ns),
        (EventKind::Removed, id, 2 * S)
    );
    assert_eq!(text_of(&ev[1].by), "SUP B JONES");
    assert_eq!(events_len(&inh), 0);
}

const EVENT_EMPTY_FOR_TEST: Event = Event {
    time_ns: 0,
    id: 0,
    kind: EventKind::Created,
    net: NetId::ConflictAlert,
    by: [0; AUTHOR_BYTES],
};

// REQ: OA-SAF-012
#[test]
fn an_expired_inhibition_is_reported_once_and_leaves_the_table() {
    let mut inh = table();
    let c = cfg(1);
    let a = inhibit_create(&mut inh, &c, &req(NetId::Msaw, 0, 10 * S), 0).unwrap();
    let b = inhibit_create(&mut inh, &c, &req(NetId::Msaw, 0, 20 * S), 0).unwrap();
    let mut ev = [EVENT_EMPTY_FOR_TEST; 8];
    events_take(&mut inh, &mut ev);
    assert_eq!(inhibit_expire(&mut inh, 10 * S - 1), 0);
    assert_eq!(inhibit_expire(&mut inh, 10 * S), 1);
    assert_eq!(inhibit_expire(&mut inh, 11 * S), 0);
    assert_eq!(inhibits_slice(&inh).len(), 1);
    assert_eq!(inhibits_slice(&inh)[0].id, b);
    let n = events_take(&mut inh, &mut ev);
    assert_eq!(n, 1);
    assert_eq!(
        (ev[0].kind, ev[0].id, ev[0].time_ns),
        (EventKind::Expired, a, 10 * S)
    );
    // It no longer suppresses.
    assert_eq!(
        inhibit_suppressors(&inh, NetId::Msaw, INSIDE.0, INSIDE.1, Some(2000), 10 * S).count,
        1
    );
    inhibit_expire(&mut inh, 30 * S);
    assert!(inhibits_slice(&inh).is_empty());
    assert_eq!(events_take(&mut inh, &mut ev), 1);
    assert_eq!(events_take(&mut inh, &mut ev), 0);
}

// REQ: OA-SAF-013
#[test]
fn an_expiry_that_cannot_be_logged_waits_in_the_table_and_is_never_lost() {
    let mut inh = table();
    let c = cfg(1);
    // Fill the event log: create while there is room in the table, else remove the oldest.
    while events_len(&inh) < EVENT_CAP {
        if inhibits_slice(&inh).len() < INHIBIT_CAP {
            inhibit_create(&mut inh, &c, &req(NetId::Msaw, 0, 10 * S), 0).unwrap();
        } else {
            let first = inhibits_slice(&inh)[0].id;
            inhibit_remove(&mut inh, first, "SUP J SMITH", 0).unwrap();
        }
    }
    assert!(inhibits_slice(&inh).len() > 1);
    let held = inhibits_slice(&inh).len();
    assert_eq!(inhibit_expire(&mut inh, 20 * S), 0);
    assert_eq!(inhibits_slice(&inh).len(), held);
    assert_eq!(inhibits_expired_waiting(&inh, 20 * S) as usize, held);
    // Draining the log lets them be reported, each once.
    let mut ev = vec![EVENT_EMPTY_FOR_TEST; EVENT_CAP];
    assert_eq!(events_take(&mut inh, &mut ev), EVENT_CAP);
    assert_eq!(inhibit_expire(&mut inh, 20 * S) as usize, held);
    assert!(inhibits_slice(&inh).is_empty());
    assert_eq!(inhibits_expired_waiting(&inh, 20 * S), 0);
}

// REQ: OA-SAF-014
#[test]
fn polygons_are_validated_and_tested_without_floating_point() {
    let v = |p: &[(i32, i32)]| polygon_new(p);
    assert!(v(&SQUARE).is_ok());
    assert!(v(&[(0, 0), (1, 1)]).is_err());
    // Bowtie, collinear points and repeated vertices.
    assert!(v(&[(0, 0), (10, 10), (10, 0), (0, 10)]).is_err());
    assert_eq!(v(&[(0, 0), (0, 10), (0, 20)]).unwrap_err(), "zero area");
    assert!(v(&[(0, 0), (0, 0), (10, 0), (0, 10)]).is_err());
    // An edge that folds back over its neighbour.
    assert_eq!(
        v(&[(0, 0), (0, 20), (0, 10), (10, 10)]).unwrap_err(),
        "edge doubles back"
    );
    // A vertex in the middle of a straight edge is fine.
    assert!(v(&[(0, 0), (0, 10), (0, 20), (20, 20), (20, 0)]).is_ok());
    // Poles, off the globe, antimeridian.
    assert!(v(&[(900_000_000, 0), (0, 10), (10, 0)]).is_err());
    assert!(v(&[(0, 0), (0, 1_800_000_001), (10, 0)]).is_err());
    assert!(v(&[(0, -1_700_000_000), (100, 1_700_000_000), (10, 0)]).is_err());
    // Too many vertices.
    let many: Vec<Vertex> = (0..=MAX_VERTICES as i32).map(|k| (k, k * k)).collect();
    assert_eq!(v(&many).unwrap_err(), "more than 32 vertices");
    // A concave polygon (an L): the notch is outside.
    let l = polygon_new(&[(0, 0), (0, 100), (50, 100), (50, 50), (100, 50), (100, 0)]).unwrap();
    assert!(polygon_contains(&l, 25, 25));
    assert!(polygon_contains(&l, 25, 75));
    assert!(polygon_contains(&l, 75, 25));
    assert!(!polygon_contains(&l, 75, 75));
    assert!(!polygon_contains(&l, 25, 101));
    assert!(!polygon_contains(&l, 50, 75));
    assert!(!polygon_contains(&l, 0, 0));
    // The extremes of the coordinate range do not overflow.
    let big = polygon_new(&[
        (-899_999_999, -800_000_000),
        (899_999_999, -800_000_000),
        (899_999_999, 800_000_000),
        (-899_999_999, 800_000_000),
    ])
    .unwrap();
    assert!(polygon_contains(&big, 0, 0));
    assert!(!polygon_contains(&big, 0, 800_000_001));
}

// REQ: OA-SAF-015
#[test]
fn a_backdated_report_that_is_worse_replaces_the_state_and_any_other_is_refused() {
    let inh = table();
    let unavailable = NetReport::Unavailable {
        reason: Reason::InputMissing,
    };
    let degraded = NetReport::Degraded {
        reason: Reason::BudgetExceeded,
        area: Area::Whole,
        count: 1,
    };
    let mut st = table_new(60 * S).unwrap();
    report(&mut st, NetId::Msaw, NetReport::Running, 100 * S).unwrap();
    // Running, then a backdated unavailable: the bad news is not lost; the later time stands.
    state_report(&mut st, NetId::Msaw, unavailable, 90 * S, 101 * S).unwrap();
    let s = net_status(&st, &inh, NetId::Msaw, 102 * S);
    assert_eq!(
        s.state,
        NetState::Unavailable {
            reason: Reason::InputMissing
        }
    );
    assert_eq!(s.age_ns, Some(2 * S));
    assert_eq!(state_backdated_applied(&st), 1);
    // A backdated running or degraded over the unavailable one is refused.
    for r in [NetReport::Running, degraded, unavailable] {
        assert_eq!(
            state_report(&mut st, NetId::Msaw, r, 80 * S, 101 * S).unwrap_err(),
            StateError::TimeBackwards
        );
    }
    assert_eq!(state_rejected(&st), 3);
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 102 * S).state,
        NetState::Unavailable {
            reason: Reason::InputMissing
        }
    );
    // Running then a backdated degraded replaces; a later fresh running clears it again.
    report(&mut st, NetId::ConflictAlert, NetReport::Running, 100 * S).unwrap();
    state_report(&mut st, NetId::ConflictAlert, degraded, 99 * S, 100 * S).unwrap();
    assert!(matches!(
        net_status(&st, &inh, NetId::ConflictAlert, 100 * S).state,
        NetState::Degraded { .. }
    ));
    report(&mut st, NetId::ConflictAlert, NetReport::Running, 101 * S).unwrap();
    assert_eq!(
        net_status(&st, &inh, NetId::ConflictAlert, 101 * S).state,
        NetState::Running
    );
    // A backdated running over a stored degraded is refused.
    report(&mut st, NetId::ApproachPath, degraded, 100 * S).unwrap();
    assert!(state_report(
        &mut st,
        NetId::ApproachPath,
        NetReport::Running,
        50 * S,
        100 * S
    )
    .is_err());
}

// REQ: OA-SAF-016
#[test]
fn a_report_dated_beyond_the_horizon_is_refused_and_cannot_lock_the_net_out() {
    let inh = table();
    let mut st = table_new(10 * S).unwrap();
    // Never reported, then one bad timestamp: unavailable, inconsistent time.
    assert_eq!(
        state_report(&mut st, NetId::Msaw, NetReport::Running, i64::MAX, 100 * S).unwrap_err(),
        StateError::TimeAhead
    );
    assert_eq!(state_future_rejected(&st), 1);
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 100 * S).state,
        NetState::Unavailable {
            reason: Reason::TimeInconsistent
        }
    );
    // A valid report ends it at once.
    report(&mut st, NetId::Msaw, NetReport::Running, 100 * S).unwrap();
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 100 * S).state,
        NetState::Running
    );
    // After a good one, a bad timestamp makes it unavailable but stores nothing; the next good
    // report (even older than the bad one) is accepted.
    assert!(state_report(&mut st, NetId::Msaw, NetReport::Running, i64::MAX, 101 * S).is_err());
    assert!(matches!(
        net_status(&st, &inh, NetId::Msaw, 101 * S).state,
        NetState::Unavailable {
            reason: Reason::TimeInconsistent
        }
    ));
    report(&mut st, NetId::Msaw, NetReport::Running, 102 * S).unwrap();
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 102 * S).state,
        NetState::Running
    );
    // Exactly the horizon is accepted, one nanosecond more is not; extremes do not overflow.
    state_report(&mut st, NetId::Msaw, NetReport::Running, 107 * S, 102 * S).unwrap();
    assert!(state_report(
        &mut st,
        NetId::Msaw,
        NetReport::Running,
        107 * S + 1,
        102 * S
    )
    .is_err());
    // Extreme clocks do not overflow.
    let _ = state_report(&mut st, NetId::Msaw, NetReport::Running, 0, i64::MAX);
    let _ = state_report(&mut st, NetId::Msaw, NetReport::Running, i64::MAX, i64::MAX);
    let _ = state_report(&mut st, NetId::Msaw, NetReport::Running, i64::MIN, i64::MIN);
}

// REQ: OA-SAF-017
#[test]
fn a_full_event_log_refuses_creation_and_removal_and_loses_no_record() {
    let mut inh = table();
    let c = cfg(1);
    let held = 32;
    for _ in 0..held {
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, 0, 10 * S), 0).unwrap();
    }
    // Replace the oldest, two events at a time, until the log is exactly full.
    while events_len(&inh) + 2 <= EVENT_CAP {
        let first = inhibits_slice(&inh)[0].id;
        inhibit_remove(&mut inh, first, "SUP J SMITH", 0).unwrap();
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, 0, 10 * S), 0).unwrap();
    }
    assert_eq!(events_len(&inh), EVENT_CAP);
    assert!(held > 1);
    // No room: creation is refused (counted), removal is refused and the inhibition stays.
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, 0, 10 * S), 0).unwrap_err(),
        InhibitError::EventLogFull
    );
    assert_eq!(inhibits_refused_event_full(&inh), 1);
    let id = inhibits_slice(&inh)[0].id;
    assert_eq!(
        inhibit_remove(&mut inh, id, "SUP J SMITH", 1).unwrap_err(),
        RemoveError::EventLogFull
    );
    assert_eq!(inhibits_slice(&inh).len(), held);
    assert_eq!(events_overflow(&inh), 0);
    // One slot free: an inhibition in force can be removed; one that already ended waits for
    // two (its review event, then the removal).
    let mut ev = vec![EVENT_EMPTY_FOR_TEST; EVENT_CAP];
    let mut one = [EVENT_EMPTY_FOR_TEST; 1];
    assert_eq!(events_take(&mut inh, &mut one), 1);
    assert_eq!(
        inhibit_remove(&mut inh, id, "SUP J SMITH", 20 * S).unwrap_err(),
        RemoveError::EventLogFull
    );
    assert_eq!(events_take(&mut inh, &mut one), 1);
    inhibit_remove(&mut inh, id, "SUP J SMITH", 20 * S).unwrap();
    let n = events_take(&mut inh, &mut ev);
    let last: Vec<_> = ev[..n].iter().rev().take(2).map(|e| e.kind).collect();
    assert_eq!(last, [EventKind::Removed, EventKind::Expired]);
}

// REQ: OA-SAF-018
#[test]
fn a_start_horizon_a_maximum_extent_and_ids_are_required_and_enforced() {
    let mut inh = table();
    let now = 100 * S;
    let mut c = cfg(2);
    let r = req(NetId::Msaw, now, now + H);
    c.max_start_ahead_ns = None;
    assert_eq!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::NoStartHorizonConfigured
    );
    let mut c = cfg(2);
    c.max_extent_e7 = None;
    assert_eq!(
        inhibit_create(&mut inh, &c, &r, now).unwrap_err(),
        InhibitError::NoExtentConfigured
    );
    let c = cfg(2);
    // The start may be at most the horizon ahead.
    let ahead = now + 24 * H;
    assert!(inhibit_create(&mut inh, &c, &req(NetId::Msaw, ahead, ahead + H), now).is_ok());
    assert_eq!(
        inhibit_create(&mut inh, &c, &req(NetId::Msaw, ahead + 1, ahead + H), now).unwrap_err(),
        InhibitError::StartTooFar
    );
    // The square spans 2 000 000 in each direction.
    let mut small = cfg(2);
    small.max_extent_e7 = Some(2_000_000);
    assert!(inhibit_create(&mut inh, &small, &r, now).is_ok());
    small.max_extent_e7 = Some(1_999_999);
    assert_eq!(
        inhibit_create(&mut inh, &small, &r, now).unwrap_err(),
        InhibitError::AreaTooLarge
    );
}

// REQ: OA-SAF-019
#[test]
fn lost_suppressions_mark_only_their_net_degraded_until_the_log_is_cleared() {
    let mut inh = table();
    let mut st = table_new(10 * S).unwrap();
    for n in NETS {
        report(&mut st, n, NetReport::Running, 5 * S).unwrap();
    }
    inhibit_create(&mut inh, &cfg(1), &req(NetId::Msaw, 0, H), 0).unwrap();
    for k in 0..(SUPPRESSED_CAP + 1) {
        inhibit_suppress(
            &mut inh,
            NetId::Msaw,
            INSIDE.0,
            INSIDE.1,
            Some(2000),
            5 * S + k as i64,
        );
    }
    let all = net_statuses(&st, &inh, 5 * S);
    assert!(matches!(
        all[net_index(NetId::Msaw)].state,
        NetState::Degraded {
            reason: Reason::SuppressionLogFull,
            count: 1,
            ..
        }
    ));
    assert_eq!(
        all[net_index(NetId::ConflictAlert)].state,
        NetState::Running
    );
    suppressed_clear(&mut inh);
    assert_eq!(
        net_status(&st, &inh, NetId::Msaw, 5 * S).state,
        NetState::Inhibited
    );
    assert_eq!(suppressed_overflow(&inh), 1);
}

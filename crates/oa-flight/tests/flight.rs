//! The flight record: keys, altitude forms, stamps, the lag, the lifecycle, handoffs, conflicts,
//! removals, and properties over operation sequences.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_flight::*;

const FLOOR: i64 = 60_000;

fn cfg() -> FlightConfig {
    FlightConfig {
        capacity: 16,
        lag_floor_ms: FLOOR,
        max_future_ms: 5_000,
        refusal_log: 64,
        conflict_log: 64,
        removal_log: 64,
    }
}

fn table() -> FlightTable {
    flights_new(&cfg()).unwrap()
}

fn gk(n: u128) -> FlightKey {
    key_gufi(n)
}

fn code<const N: usize>(s: &str) -> [u8; N] {
    let mut o = [0u8; N];
    for (d, b) in o.iter_mut().zip(s.bytes()) {
        *d = b;
    }
    o
}

const ZAU: [u8; 4] = *b"ZAU\0";
const ZMP: [u8; 4] = *b"ZMP\0";

/// An operation stamped `ts` and seen `ts + 61 s` later.
fn op(kind: u8, key: FlightKey, centre: [u8; 4], ts: i64) -> Op {
    op_new(kind, key, centre, ts, ts + 61_000, ts as u64)
}

fn set_plan(key: FlightKey, centre: [u8; 4], ts: i64, state: u8) -> Op {
    let mut o = op(OP_SET_PLAN, key, centre, ts);
    o.state = state;
    o
}

fn assign(key: FlightKey, ts: i64, ft: i32) -> Op {
    let mut o = op(OP_ASSIGN, key, ZAU, ts);
    o.alt = Alt {
        form: ALT_SIMPLE,
        a_ft: ft,
        b_ft: 0,
    };
    o
}

fn owner(key: FlightKey, ts: i64, unit: &str) -> Op {
    let mut o = op(OP_OWNER, key, ZAU, ts);
    o.unit_a = code(unit);
    o
}

fn ho(kind: u8, key: FlightKey, ts: i64, a: &str, b: &str) -> Op {
    let mut o = op(kind, key, ZAU, ts);
    o.unit_a = code(a);
    o.unit_b = code(b);
    o
}

fn apply(t: &mut FlightTable, o: &Op) -> Outcome {
    let r = flights_apply(t, o);
    flights_check(t).unwrap();
    r
}

fn live(t: &mut FlightTable, key: FlightKey) {
    assert_eq!(
        apply(t, &set_plan(key, ZAU, 1_000, ST_ACTIVE)),
        Outcome::Applied
    );
}

// REQ: OA-FLT-001
#[test]
fn keys_are_gufi_or_the_tfms_triple_and_nothing_less() {
    let mut t = table();
    let tf = key_tfms(code("TST123"), code("KAAA"), code("KBBB"));
    for k in [gk(1), tf] {
        assert_eq!(
            apply(&mut t, &set_plan(k, ZAU, 1_000, ST_ACTIVE)),
            Outcome::Applied
        );
        assert!(flight_get(&t, &k).is_some());
    }
    assert_eq!(flights_len(&t), 2);
    // The same triple is the same flight; a GUFI flight is not a TFMS flight.
    assert_eq!(
        apply(&mut t, &set_plan(tf, ZAU, 2_000, ST_ACTIVE)),
        Outcome::Unchanged
    );
    assert_eq!(flights_len(&t), 2);
    // A TFMS-keyed flight is found by its identity too: a second flight of that triple is a conflict.
    let f = flight_get(&t, &tf).unwrap();
    assert_eq!(
        (f.callsign, f.dep, f.arr),
        (code("TST123"), code("KAAA"), code("KBBB"))
    );
    // Keys with a missing part are no key.
    let mut bad = tf;
    bad.arr = [0; 12];
    let nil_gufi = gk(0);
    let mut mixed = gk(5);
    mixed.callsign = code("X");
    for k in [bad, nil_gufi, mixed] {
        let r = apply(&mut t, &set_plan(k, ZAU, 3_000, ST_ACTIVE));
        assert_eq!(r, Outcome::Refused(R_BAD_KEY));
    }
    let mut unknown = op(OP_ASSIGN, gk(1), ZAU, 4_000);
    unknown.kind = 99;
    assert_eq!(apply(&mut t, &unknown), Outcome::Refused(R_BAD_KEY));
    assert_eq!(flights_len(&t), 2);
}

// REQ: OA-FLT-002
#[test]
fn the_three_altitudes_are_separate_fields_with_their_own_forms() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    let mut a = assign(k, 2_000, 36_000);
    assert_eq!(apply(&mut t, &a), Outcome::Applied);
    let mut i = op(OP_INTERIM, k, ZAU, 3_000);
    i.alt = Alt {
        form: ALT_SIMPLE,
        a_ft: 24_000,
        b_ft: 0,
    };
    assert_eq!(apply(&mut t, &i), Outcome::Applied);
    let mut r = op(OP_REQUEST, k, ZAU, 4_000);
    r.alt = Alt {
        form: ALT_VFR,
        a_ft: 0,
        b_ft: 0,
    };
    assert_eq!(apply(&mut t, &r), Outcome::Applied);
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(f.assigned.a_ft, 36_000);
    assert_eq!((f.interim.form, f.interim.a_ft), (ALT_SIMPLE, 24_000));
    assert_eq!(f.requested.form, ALT_VFR);
    assert_eq!(f.requested.a_ft, 0, "VFR is a state, not a number");

    // The interim altitude can be cleared, which is stated, not absent.
    let mut nil = op(OP_INTERIM, k, ZAU, 5_000);
    nil.alt = Alt {
        form: ALT_NIL,
        a_ft: 0,
        b_ft: 0,
    };
    assert_eq!(apply(&mut t, &nil), Outcome::Applied);
    assert_eq!(flight_get(&t, &k).unwrap().interim.form, ALT_NIL);
    assert_eq!(flight_get(&t, &k).unwrap().assigned.a_ft, 36_000);

    // Block and altitude at a fix are assigned forms.
    a.ts_ms = 6_000;
    a.seen_ms = 67_000;
    a.alt = Alt {
        form: ALT_BLOCK,
        a_ft: 45_000,
        b_ft: 47_000,
    };
    assert_eq!(apply(&mut t, &a), Outcome::Applied);
    assert_eq!(flight_get(&t, &k).unwrap().assigned.form, ALT_BLOCK);
    let mut fix = op(OP_ASSIGN, k, ZAU, 7_000);
    fix.alt = Alt {
        form: ALT_AT_FIX,
        a_ft: 22_000,
        b_ft: 7_000,
    };
    fix.fix = code("ABCDE");
    assert_eq!(apply(&mut t, &fix), Outcome::Applied);
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(
        (f.assigned.form, f.assigned_fix),
        (ALT_AT_FIX, code("ABCDE"))
    );
    // A later plain altitude clears the fix.
    assert_eq!(apply(&mut t, &assign(k, 8_000, 30_000)), Outcome::Applied);
    assert_eq!(flight_get(&t, &k).unwrap().assigned_fix, [0; 12]);
}

// REQ: OA-FLT-042
#[test]
fn an_explicit_assigned_altitude_clear_is_nil_not_vfr_or_unstated() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    assert_eq!(apply(&mut t, &assign(k, 2_000, 36_000)), Outcome::Applied);

    let mut clear = op(OP_ASSIGN, k, ZAU, 3_000);
    clear.alt = Alt {
        form: ALT_NIL,
        a_ft: 0,
        b_ft: 0,
    };
    assert_eq!(apply(&mut t, &clear), Outcome::Applied);
    let cleared = flight_get(&t, &k).unwrap();
    assert_eq!(cleared.assigned.form, ALT_NIL);
    assert_eq!(cleared.assigned.a_ft, 0);
    assert_eq!(cleared.assigned.b_ft, 0);
    assert_eq!(cleared.stamps[F_ASSIGNED].ts_ms, 3_000);

    clear.ts_ms = 4_000;
    clear.seen_ms = 65_000;
    assert_eq!(apply(&mut t, &clear), Outcome::Unchanged);
    assert_eq!(flight_get(&t, &k).unwrap().stamps[F_ASSIGNED].ts_ms, 3_000);

    let mut requested_nil = op(OP_REQUEST, k, ZAU, 5_000);
    requested_nil.alt = clear.alt;
    assert_eq!(apply(&mut t, &requested_nil), Outcome::Refused(R_BAD_VALUE));
}

// REQ: OA-FLT-002
#[test]
fn an_altitude_the_field_cannot_hold_is_refused_and_changes_nothing() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    assert_eq!(apply(&mut t, &assign(k, 2_000, 30_000)), Outcome::Applied);
    let before = flights_digest(&t);
    let bad = |kind: u8, alt: Alt, fix: [u8; 12]| {
        let mut o = op(kind, k, ZAU, 3_000);
        o.alt = alt;
        o.fix = fix;
        o
    };
    let simple = |a| Alt {
        form: ALT_SIMPLE,
        a_ft: a,
        b_ft: 0,
    };
    for o in [
        // No altitude at all: the U-2 plan with none is refused, not processed.
        bad(OP_ASSIGN, Alt::default(), [0; 12]),
        bad(OP_ASSIGN, simple(100_001), [0; 12]),
        bad(OP_ASSIGN, simple(-2_001), [0; 12]),
        bad(
            OP_ASSIGN,
            Alt {
                form: ALT_SIMPLE,
                a_ft: 100,
                b_ft: 7,
            },
            [0; 12],
        ),
        bad(
            OP_ASSIGN,
            Alt {
                form: ALT_BLOCK,
                a_ft: 47_000,
                b_ft: 45_000,
            },
            [0; 12],
        ),
        bad(
            OP_ASSIGN,
            Alt {
                form: ALT_VFR,
                a_ft: 5,
                b_ft: 0,
            },
            [0; 12],
        ),
        bad(
            OP_ASSIGN,
            Alt {
                form: ALT_NIL,
                a_ft: 1,
                b_ft: 0,
            },
            [0; 12],
        ),
        bad(
            OP_ASSIGN,
            Alt {
                form: ALT_AT_FIX,
                a_ft: 1,
                b_ft: 2,
            },
            [0; 12],
        ),
        bad(OP_ASSIGN, simple(100), code("FIX")),
        bad(
            OP_INTERIM,
            Alt {
                form: ALT_VFR,
                a_ft: 0,
                b_ft: 0,
            },
            [0; 12],
        ),
        bad(
            OP_INTERIM,
            Alt {
                form: ALT_BLOCK,
                a_ft: 1,
                b_ft: 2,
            },
            [0; 12],
        ),
        bad(
            OP_REQUEST,
            Alt {
                form: ALT_NIL,
                a_ft: 0,
                b_ft: 0,
            },
            [0; 12],
        ),
        bad(
            OP_REQUEST,
            Alt {
                form: ALT_BLOCK,
                a_ft: 1,
                b_ft: 2,
            },
            [0; 12],
        ),
        bad(
            OP_REQUEST,
            Alt {
                form: 77,
                a_ft: 0,
                b_ft: 0,
            },
            [0; 12],
        ),
    ] {
        assert_eq!(apply(&mut t, &o), Outcome::Refused(R_BAD_VALUE), "{o:?}");
    }
    let mut set_bad = set_plan(k, ZAU, 3_000, ST_ACTIVE);
    set_bad.state = ST_COMPLETED;
    assert_eq!(apply(&mut t, &set_bad), Outcome::Refused(R_BAD_VALUE));
    // The refusals changed no flight; only the counts moved.
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(f.assigned.a_ft, 30_000);
    assert_eq!(f.stamps[F_ASSIGNED].ts_ms, 2_000);
    assert_ne!(before, flights_digest(&t));
    assert_eq!(t.refusals.len(), 15);
}

// REQ: OA-FLT-003
#[test]
fn a_field_keeps_when_it_last_changed_and_the_message_it_came_from() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    let mut a = assign(k, 2_000, 36_000);
    a.msg = 777;
    assert_eq!(apply(&mut t, &a), Outcome::Applied);
    let s = flight_get(&t, &k).unwrap().stamps[F_ASSIGNED];
    assert_eq!((s.ts_ms, s.avail_ms, s.msg), (2_000, 63_000, 777));

    // The same value stated again changes nothing, not even the stamp.
    let mut again = assign(k, 9_000, 36_000);
    again.msg = 888;
    assert_eq!(apply(&mut t, &again), Outcome::Unchanged);
    assert_eq!(flight_get(&t, &k).unwrap().stamps[F_ASSIGNED], s);

    // A new value moves it, and only it.
    let mut b = assign(k, 10_000, 38_000);
    b.msg = 999;
    assert_eq!(apply(&mut t, &b), Outcome::Applied);
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(f.stamps[F_ASSIGNED].msg, 999);
    assert_eq!(f.stamps[F_ASSIGNED].ts_ms, 10_000);
    assert_eq!(f.stamps[F_INTERIM], Stamp::default());
    assert_eq!(f.ops_applied, 3, "the plan, then two assignments");
}

// REQ: OA-FLT-004
#[test]
fn an_operation_is_available_no_earlier_than_stamp_plus_lag_and_the_clock_never_goes_back() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    assert_eq!(t.clock_ms, 1_000 + 61_000);
    // Seen only 10 s after its stamp: the floor of 60 s holds it until stamp + 60 s.
    let mut early = assign(k, 100_000, 30_000);
    early.seen_ms = 110_000;
    assert_eq!(apply(&mut t, &early), Outcome::Applied);
    let s = flight_get(&t, &k).unwrap().stamps[F_ASSIGNED];
    assert_eq!(s.avail_ms, 160_000);
    assert_eq!(t.stats.lag_raised, 1);
    // A following operation seen earlier than that is held at the clock, not before it.
    let mut next = owner(k, 101_000, "ZAU12");
    next.seen_ms = 120_000;
    assert_eq!(apply(&mut t, &next), Outcome::Applied);
    let o = flight_get(&t, &k).unwrap().stamps[F_OWNER];
    assert_eq!(o.avail_ms, 161_000, "stamp plus floor again");
    let mut held = assign(k, 90_000, 31_000);
    held.seen_ms = 125_000;
    assert_eq!(apply(&mut t, &held), Outcome::Refused(R_STALE));
    assert_eq!(
        t.clock_ms, 161_000,
        "an operation never moves the clock back"
    );
    assert!(t.stats.clock_held >= 1);
    // An operation stamped after its receipt by more than the allowed clock difference is refused.
    let mut future = assign(k, 500_000, 32_000);
    future.seen_ms = 400_000;
    assert_eq!(apply(&mut t, &future), Outcome::Refused(R_BAD_TIME));
    assert_eq!(t.clock_ms, 161_000);
    // Within the allowed difference it is accepted.
    let mut near = assign(k, 404_000, 32_000);
    near.seen_ms = 400_000;
    assert_eq!(apply(&mut t, &near), Outcome::Applied);
    assert_eq!(
        flight_get(&t, &k).unwrap().stamps[F_ASSIGNED].avail_ms,
        464_000
    );
}

// REQ: OA-FLT-005
#[test]
fn the_lifecycle_allows_exactly_the_stated_steps() {
    // The machine: from, operation (set-plan asking active unless noted), result.
    let table: &[(u8, u8, u8, Option<u8>)] = &[
        // (state, op, wanted state of a set-plan, resulting state; None = refused)
        (ST_PROPOSED, OP_SET_PLAN, ST_PROPOSED, Some(ST_PROPOSED)),
        (ST_PROPOSED, OP_SET_PLAN, ST_ACTIVE, Some(ST_ACTIVE)),
        (ST_PROPOSED, OP_DEPART, 0, None),
        (ST_PROPOSED, OP_COMPLETE, 0, None),
        (ST_PROPOSED, OP_CANCEL, 0, Some(ST_CANCELLED)),
        (ST_PROPOSED, OP_DROP, 0, Some(ST_DROPPED)),
        (ST_ACTIVE, OP_SET_PLAN, ST_PROPOSED, None),
        (ST_ACTIVE, OP_SET_PLAN, ST_ACTIVE, Some(ST_ACTIVE)),
        (ST_ACTIVE, OP_DEPART, 0, Some(ST_DEPARTED)),
        (ST_ACTIVE, OP_COMPLETE, 0, Some(ST_COMPLETED)),
        (ST_ACTIVE, OP_CANCEL, 0, Some(ST_CANCELLED)),
        (ST_ACTIVE, OP_DROP, 0, Some(ST_DROPPED)),
        (ST_DEPARTED, OP_SET_PLAN, ST_PROPOSED, None),
        (ST_DEPARTED, OP_SET_PLAN, ST_ACTIVE, Some(ST_DEPARTED)),
        (ST_DEPARTED, OP_DEPART, 0, Some(ST_DEPARTED)),
        (ST_DEPARTED, OP_COMPLETE, 0, Some(ST_COMPLETED)),
        (ST_DEPARTED, OP_CANCEL, 0, None),
        (ST_DEPARTED, OP_DROP, 0, Some(ST_DROPPED)),
        (ST_DROPPED, OP_SET_PLAN, ST_PROPOSED, None),
        (ST_DROPPED, OP_SET_PLAN, ST_ACTIVE, Some(ST_ACTIVE)),
        (ST_DROPPED, OP_DEPART, 0, None),
        (ST_DROPPED, OP_COMPLETE, 0, Some(ST_COMPLETED)),
        (ST_DROPPED, OP_CANCEL, 0, Some(ST_CANCELLED)),
        (ST_DROPPED, OP_DROP, 0, Some(ST_DROPPED)),
        (ST_COMPLETED, OP_SET_PLAN, ST_ACTIVE, None),
        (ST_COMPLETED, OP_DEPART, 0, None),
        (ST_COMPLETED, OP_COMPLETE, 0, Some(ST_COMPLETED)),
        (ST_COMPLETED, OP_CANCEL, 0, None),
        (ST_COMPLETED, OP_DROP, 0, None),
        (ST_CANCELLED, OP_SET_PLAN, ST_ACTIVE, None),
        (ST_CANCELLED, OP_DEPART, 0, None),
        (ST_CANCELLED, OP_COMPLETE, 0, None),
        (ST_CANCELLED, OP_CANCEL, 0, Some(ST_CANCELLED)),
        (ST_CANCELLED, OP_DROP, 0, None),
    ];
    // Reach each state by a legal path, then try the operation.
    let path = |to: u8| -> Vec<(u8, u8)> {
        match to {
            ST_PROPOSED => vec![(OP_SET_PLAN, ST_PROPOSED)],
            ST_ACTIVE => vec![(OP_SET_PLAN, ST_ACTIVE)],
            ST_DEPARTED => vec![(OP_SET_PLAN, ST_ACTIVE), (OP_DEPART, 0)],
            ST_DROPPED => vec![(OP_SET_PLAN, ST_ACTIVE), (OP_DROP, 0)],
            ST_COMPLETED => vec![(OP_SET_PLAN, ST_ACTIVE), (OP_COMPLETE, 0)],
            _ => vec![(OP_SET_PLAN, ST_ACTIVE), (OP_CANCEL, 0)],
        }
    };
    for &(from, kind, want, result) in table {
        let mut t = table_new();
        let k = gk(1);
        let mut ts = 1_000;
        for (pk, ps) in path(from) {
            let mut o = op(pk, k, ZAU, ts);
            o.state = ps;
            assert_ne!(apply(&mut t, &o), Outcome::Refused(R_BAD_TRANSITION));
            ts += 1_000;
        }
        let f = flight_get(&t, &k).unwrap();
        assert_eq!(view_of(&f, &ZAU).unwrap().state, from, "path to {from}");
        let mut o = op(kind, k, ZAU, ts);
        o.state = want;
        let out = apply(&mut t, &o);
        let f = flight_get(&t, &k).unwrap();
        let now = view_of(&f, &ZAU).unwrap().state;
        match result {
            Some(s) => {
                assert!(
                    !matches!(out, Outcome::Refused(_)),
                    "{from} {kind} {want}: {out:?}"
                );
                assert_eq!(now, s, "{from} {kind} {want}");
            }
            None => {
                assert!(
                    matches!(out, Outcome::Refused(_)),
                    "{from} {kind} {want}: {out:?}"
                );
                assert_eq!(now, from, "a refused operation leaves the state");
            }
        }
    }
}

fn table_new() -> FlightTable {
    table()
}

// REQ: OA-FLT-005
#[test]
fn an_operation_on_a_flight_or_plan_that_is_not_there_is_refused() {
    let mut t = table();
    let k = gk(1);
    for kind in [OP_DEPART, OP_COMPLETE, OP_CANCEL, OP_DROP] {
        assert_eq!(
            apply(&mut t, &op(kind, k, ZAU, 1_000)),
            Outcome::Refused(R_UNKNOWN_FLIGHT)
        );
    }
    assert_eq!(
        apply(&mut t, &assign(k, 1_000, 5_000)),
        Outcome::Refused(R_UNKNOWN_FLIGHT)
    );
    assert_eq!(
        apply(&mut t, &owner(k, 1_000, "ZAU12")),
        Outcome::Refused(R_UNKNOWN_FLIGHT)
    );
    assert_eq!(flights_len(&t), 0, "only a plan makes a flight");
    // A flight another centre reports has no plan view at this one.
    live(&mut t, k);
    assert_eq!(
        apply(&mut t, &op(OP_COMPLETE, k, ZMP, 2_000)),
        Outcome::Refused(R_NO_VIEW)
    );
    // An amendment to a flight whose plans have ended.
    assert_eq!(
        apply(&mut t, &op(OP_CANCEL, k, ZAU, 3_000)),
        Outcome::Applied
    );
    for o in [assign(k, 4_000, 5_000), owner(k, 4_000, "ZAU12")] {
        assert_eq!(apply(&mut t, &o), Outcome::Refused(R_TERMINAL));
    }
    assert_eq!(flight_get(&t, &k).unwrap().assigned.form, ALT_NONE);
}

// REQ: OA-FLT-006
#[test]
fn every_refusal_is_counted_by_operation_and_reason_and_listed() {
    let mut t = table();
    let k = gk(1);
    assert_eq!(
        apply(&mut t, &op(OP_COMPLETE, k, ZAU, 1_000)),
        Outcome::Refused(R_UNKNOWN_FLIGHT)
    );
    live(&mut t, k);
    assert_eq!(
        apply(&mut t, &op(OP_DEPART, k, ZAU, 2_000)),
        Outcome::Applied
    );
    assert_eq!(
        apply(&mut t, &op(OP_CANCEL, k, ZAU, 3_000)),
        Outcome::Refused(R_BAD_TRANSITION)
    );
    assert_eq!(
        apply(&mut t, &op(OP_CANCEL, k, ZAU, 4_000)),
        Outcome::Refused(R_BAD_TRANSITION)
    );
    let count =
        |t: &FlightTable, o: u8, r: u8| t.stats.refused[usize::from(o) * R_COUNT + usize::from(r)];
    assert_eq!(count(&t, OP_COMPLETE, R_UNKNOWN_FLIGHT), 1);
    assert_eq!(count(&t, OP_CANCEL, R_BAD_TRANSITION), 2);
    assert_eq!(t.refusals.len(), 3);
    let last = t.refusals[2];
    assert_eq!(
        (last.op, last.reason, last.state),
        (OP_CANCEL, R_BAD_TRANSITION, ST_DEPARTED)
    );
    assert_eq!((last.key, last.ts_ms, last.msg), (k, 4_000, 4_000));
    let total: u64 = t.stats.refused.iter().sum();
    assert_eq!(total + t.stats.applied + t.stats.unchanged, t.stats.ops);

    // A full log counts what it cannot list; the counts stay exact.
    let mut small = flights_new(&FlightConfig {
        refusal_log: 2,
        ..cfg()
    })
    .unwrap();
    for n in 0..5 {
        flights_apply(&mut small, &op(OP_DROP, gk(9), ZAU, 1_000 + n));
    }
    assert_eq!(small.refusals.len(), 2);
    assert_eq!(small.refusals_unlisted(), 3);
    assert_eq!(small.stats.refused.iter().sum::<u64>(), 5);
    flights_log_clear(&mut small);
    assert!(small.refusals.is_empty());
    assert_eq!(
        small.stats.refused.iter().sum::<u64>(),
        5,
        "clearing the log keeps the counts"
    );
}

trait Unlisted {
    fn refusals_unlisted(&self) -> u64;
}
impl Unlisted for FlightTable {
    fn refusals_unlisted(&self) -> u64 {
        self.stats.refusals_unlisted
    }
}

// REQ: OA-FLT-007
#[test]
fn each_centre_has_its_own_plan_view_of_one_flight() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    // The next centre proposes the flight after the first activated it: not a step backward.
    assert_eq!(
        apply(&mut t, &set_plan(k, ZMP, 2_000, ST_PROPOSED)),
        Outcome::Applied
    );
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(f.n_views, 2);
    assert_eq!(view_of(&f, &ZAU).unwrap().state, ST_ACTIVE);
    assert_eq!(view_of(&f, &ZMP).unwrap().state, ST_PROPOSED);
    assert_eq!(
        apply(&mut t, &set_plan(k, ZMP, 3_000, ST_ACTIVE)),
        Outcome::Applied
    );
    // The first drops it when it leaves its airspace: the second still has it.
    assert_eq!(apply(&mut t, &op(OP_DROP, k, ZAU, 4_000)), Outcome::Applied);
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(view_of(&f, &ZAU).unwrap().state, ST_DROPPED);
    assert!(flight_is_live(&f));
    // A dropped plan can be reinstated, and that is counted.
    assert_eq!(
        apply(&mut t, &set_plan(k, ZAU, 5_000, ST_ACTIVE)),
        Outcome::Applied
    );
    assert_eq!(t.stats.reinstated, 1);

    // More centres than views are refused, never dropped silently or evicted.
    let mut n = 0u8;
    loop {
        let mut c = *b"Z00\0";
        c[2] = b'A' + n;
        let r = apply(&mut t, &set_plan(k, c, 6_000 + i64::from(n), ST_ACTIVE));
        if r != Outcome::Applied {
            assert_eq!(r, Outcome::Refused(R_VIEWS_FULL));
            break;
        }
        n += 1;
        assert!(n < 20);
    }
    assert_eq!(flight_get(&t, &k).unwrap().n_views as usize, MAX_VIEWS);
    let blank = apply(&mut t, &set_plan(gk(2), [0; 4], 7_000, ST_ACTIVE));
    assert_eq!(blank, Outcome::Refused(R_BAD_VALUE));
}

// REQ: OA-FLT-008
#[test]
fn two_plans_for_one_flight_are_listed_as_conflicts() {
    let mut t = table();
    let k = gk(1);
    let mut p1 = set_plan(k, ZAU, 1_000, ST_ACTIVE);
    p1.plan_id = code("PLAN000001");
    assert_eq!(apply(&mut t, &p1), Outcome::Applied);
    // The same plan restated is nothing.
    let mut again = set_plan(k, ZAU, 2_000, ST_ACTIVE);
    again.plan_id = code("PLAN000001");
    assert_eq!(apply(&mut t, &again), Outcome::Unchanged);
    assert!(t.conflicts.is_empty());
    // A different plan identifier on a live plan.
    let mut p2 = set_plan(k, ZAU, 3_000, ST_ACTIVE);
    p2.plan_id = code("PLAN000002");
    assert_eq!(apply(&mut t, &p2), Outcome::Applied);
    assert_eq!(t.conflicts.len(), 1);
    assert_eq!(t.conflicts[0].kind, C_PLAN_CHANGED);
    assert_eq!(
        view_of(&flight_get(&t, &k).unwrap(), &ZAU).unwrap().plan_id,
        code("PLAN000002")
    );
    // The plan ends, and the same key is filed again: the old plan is replaced, and listed.
    assert_eq!(
        apply(&mut t, &op(OP_CANCEL, k, ZAU, 4_000)),
        Outcome::Applied
    );
    let mut same = set_plan(k, ZAU, 5_000, ST_ACTIVE);
    same.plan_id = code("PLAN000002");
    assert_eq!(apply(&mut t, &same), Outcome::Refused(R_TERMINAL));
    let mut p3 = set_plan(k, ZAU, 6_000, ST_ACTIVE);
    p3.plan_id = code("PLAN000003");
    assert_eq!(apply(&mut t, &p3), Outcome::Applied);
    assert_eq!(t.conflicts[1].kind, C_PLAN_REFILED);
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(view_of(&f, &ZAU).unwrap().state, ST_ACTIVE);
    assert_eq!(f.n_views, 1, "still one plan for the flight at this centre");
    assert_eq!(t.stats.conflicts[usize::from(C_PLAN_CHANGED)], 1);
    assert_eq!(t.stats.conflicts[usize::from(C_PLAN_REFILED)], 1);
}

// REQ: OA-FLT-009
#[test]
fn an_address_or_callsign_route_on_two_live_flights_is_a_conflict() {
    let mut t = table();
    let (a, b) = (gk(1), gk(2));
    live(&mut t, a);
    live(&mut t, b);
    let mut ad = op(OP_ADDRESS, a, ZAU, 2_000);
    ad.address = 0xA1B2C3;
    assert_eq!(apply(&mut t, &ad), Outcome::Applied);
    assert_eq!(flight_by_address(&t, 0xA1B2C3).unwrap().key, a);
    assert!(t.conflicts.is_empty());
    // The same address on a second live flight: both are kept, and it is listed.
    let mut ad2 = op(OP_ADDRESS, b, ZAU, 3_000);
    ad2.address = 0xA1B2C3;
    assert_eq!(apply(&mut t, &ad2), Outcome::Applied);
    assert_eq!(t.conflicts.len(), 1);
    assert_eq!(
        (t.conflicts[0].kind, t.conflicts[0].a, t.conflicts[0].b),
        (C_ADDRESS_TWICE, b, a)
    );
    assert_eq!(
        flight_by_address(&t, 0xA1B2C3).unwrap().key,
        b,
        "the later claim is looked up"
    );
    assert_eq!(flights_len(&t), 2);
    // An address on a flight whose plans ended is not a conflict.
    let c = gk(3);
    live(&mut t, c);
    assert_eq!(
        apply(&mut t, &op(OP_CANCEL, c, ZAU, 4_000)),
        Outcome::Applied
    );
    let mut ad3 = op(OP_ADDRESS, a, ZAU, 5_000);
    ad3.address = 0x123456;
    assert_eq!(apply(&mut t, &ad3), Outcome::Applied);
    assert_eq!(t.conflicts.len(), 1);
    // Bad addresses.
    for bad in [0u32, 0x0100_0000] {
        let mut o = op(OP_ADDRESS, a, ZAU, 6_000);
        o.address = bad;
        assert_eq!(apply(&mut t, &o), Outcome::Refused(R_BAD_VALUE));
    }
    assert!(flight_by_address(&t, 0).is_none());

    // Callsign, departure and arrival on two live flights.
    let mut id = op(OP_IDENTITY, a, ZAU, 7_000);
    id.callsign = code("TST123");
    id.dep = code("KAAA");
    id.arr = code("KBBB");
    assert_eq!(apply(&mut t, &id), Outcome::Applied);
    assert_eq!(
        t.conflicts.len(),
        1,
        "a partial key is not a key until complete"
    );
    let mut id2 = op(OP_IDENTITY, b, ZAU, 8_000);
    id2.callsign = code("TST123");
    id2.dep = code("KAAA");
    assert_eq!(apply(&mut t, &id2), Outcome::Applied);
    assert_eq!(t.conflicts.len(), 1);
    let mut id3 = op(OP_IDENTITY, b, ZAU, 9_000);
    id3.arr = code("KBBB");
    assert_eq!(apply(&mut t, &id3), Outcome::Applied);
    assert_eq!(t.conflicts.len(), 2);
    assert_eq!(
        (t.conflicts[1].kind, t.conflicts[1].a, t.conflicts[1].b),
        (C_TFMS_KEY_TWICE, b, a)
    );
    assert_eq!(
        apply(&mut t, &op(OP_IDENTITY, b, ZAU, 9_500)),
        Outcome::Refused(R_BAD_VALUE)
    );
}

// REQ: OA-FLT-010
#[test]
fn a_handoff_is_initiated_accepted_and_completed_by_the_new_owner() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    assert_eq!(apply(&mut t, &owner(k, 2_000, "ZAU12")), Outcome::Applied);
    assert_eq!(
        apply(&mut t, &ho(OP_HO_INITIATE, k, 3_000, "ZAU12", "ZAU34")),
        Outcome::Applied
    );
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(
        (f.ho_state, f.ho_from, f.ho_to),
        (HO_OFFERED, code("ZAU12"), code("ZAU34"))
    );
    assert_eq!(f.owner, code("ZAU12"), "an offer does not move ownership");
    // The offer restated is nothing; a different one is refused while it is pending.
    assert_eq!(
        apply(&mut t, &ho(OP_HO_INITIATE, k, 3_500, "ZAU12", "ZAU34")),
        Outcome::Unchanged
    );
    assert_eq!(
        apply(&mut t, &ho(OP_HO_INITIATE, k, 3_600, "ZAU12", "ZAU56")),
        Outcome::Refused(R_HANDOFF_PENDING)
    );
    assert_eq!(
        apply(&mut t, &ho(OP_HO_UPDATE, k, 4_000, "ZAU12", "ZAU56")),
        Outcome::Applied
    );
    assert_eq!(flight_get(&t, &k).unwrap().ho_to, code("ZAU56"));
    assert_eq!(
        apply(&mut t, &ho(OP_HO_ACCEPT, k, 5_000, "ZAU12", "ZAU56")),
        Outcome::Applied
    );
    let f = flight_get(&t, &k).unwrap();
    assert_eq!((f.ho_state, f.ho_accepted_by), (HO_ACCEPTED, code("ZAU56")));
    assert_eq!(
        apply(&mut t, &ho(OP_HO_UPDATE, k, 5_500, "ZAU12", "ZAU78")),
        Outcome::Refused(R_BAD_TRANSITION)
    );
    assert_eq!(
        apply(&mut t, &ho(OP_HO_ACCEPT, k, 5_600, "ZAU12", "ZAU78")),
        Outcome::Refused(R_BAD_TRANSITION)
    );
    // The owner reported by the feed becomes the accepting unit: the handoff is complete.
    assert_eq!(apply(&mut t, &owner(k, 6_000, "ZAU56")), Outcome::Applied);
    let f = flight_get(&t, &k).unwrap();
    assert_eq!((f.ho_state, f.owner), (HO_NONE, code("ZAU56")));
    assert_eq!(t.stats.handoffs_completed, 1);
    // No handoff is pending for any further step.
    for kind in [
        OP_HO_UPDATE,
        OP_HO_ACCEPT,
        OP_HO_RETRACT,
        OP_HO_FAIL,
        OP_HO_TAKE,
    ] {
        assert_eq!(
            apply(&mut t, &ho(kind, k, 7_000, "ZAU56", "ZAU78")),
            Outcome::Refused(R_NO_HANDOFF)
        );
    }
}

// REQ: OA-FLT-010
#[test]
fn a_handoff_can_be_retracted_fail_or_end_in_taking_control() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    assert_eq!(apply(&mut t, &owner(k, 2_000, "ZAU12")), Outcome::Applied);
    for (kind, ts) in [(OP_HO_RETRACT, 4_000), (OP_HO_FAIL, 6_000)] {
        assert_eq!(
            apply(&mut t, &ho(OP_HO_INITIATE, k, ts - 1_000, "ZAU12", "ZAU34")),
            Outcome::Applied
        );
        assert_eq!(
            apply(&mut t, &ho(kind, k, ts, "ZAU12", "ZAU34")),
            Outcome::Applied
        );
        assert_eq!(flight_get(&t, &k).unwrap().ho_state, HO_NONE);
    }
    assert_eq!(
        (t.stats.handoffs_retracted, t.stats.handoffs_failed),
        (1, 1)
    );
    assert_eq!(
        apply(&mut t, &ho(OP_HO_INITIATE, k, 7_000, "ZAU12", "ZAU34")),
        Outcome::Applied
    );
    assert_eq!(
        apply(&mut t, &ho(OP_HO_TAKE, k, 8_000, "ZAU12", "ZAU90")),
        Outcome::Applied
    );
    let f = flight_get(&t, &k).unwrap();
    assert_eq!((f.ho_state, f.owner), (HO_NONE, code("ZAU90")));
    assert_eq!(f.stamps[F_OWNER].ts_ms, 8_000);
    assert_eq!(t.stats.handoffs_completed, 1);
    // An owner that changes under an offer to the offered unit ends it, and that is counted.
    assert_eq!(
        apply(&mut t, &ho(OP_HO_INITIATE, k, 9_000, "ZAU90", "ZAU34")),
        Outcome::Applied
    );
    assert_eq!(apply(&mut t, &owner(k, 10_000, "ZAU34")), Outcome::Applied);
    assert_eq!(flight_get(&t, &k).unwrap().ho_state, HO_NONE);
    assert_eq!(t.stats.handoffs_implicit, 1);
    // Units are required.
    for o in [
        ho(OP_HO_INITIATE, k, 11_000, "", "ZAU34"),
        ho(OP_HO_INITIATE, k, 11_000, "ZAU34", ""),
        owner(k, 11_000, ""),
    ] {
        assert_eq!(apply(&mut t, &o), Outcome::Refused(R_BAD_VALUE));
    }
}

// REQ: OA-FLT-011
#[test]
fn an_older_change_is_stale_and_a_tie_with_another_value_is_refused() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    assert_eq!(apply(&mut t, &assign(k, 10_000, 30_000)), Outcome::Applied);
    assert_eq!(
        apply(&mut t, &assign(k, 9_000, 20_000)),
        Outcome::Refused(R_STALE)
    );
    // Another message at the same time with another value is a tie.
    let mut tie = assign(k, 10_000, 40_000);
    tie.msg = 77;
    assert_eq!(
        apply(&mut t, &tie),
        Outcome::Refused(R_SAME_STAMP_DIFFERENT)
    );
    assert_eq!(flight_get(&t, &k).unwrap().assigned.a_ft, 30_000);
    // An older restatement of what is held is not an error.
    assert_eq!(apply(&mut t, &assign(k, 9_000, 30_000)), Outcome::Unchanged);
    // The same rule holds for the plan state and for ownership.
    assert_eq!(
        apply(&mut t, &op(OP_DEPART, k, ZAU, 20_000)),
        Outcome::Applied
    );
    assert_eq!(
        apply(&mut t, &op(OP_DROP, k, ZAU, 19_000)),
        Outcome::Refused(R_STALE)
    );
    assert_eq!(apply(&mut t, &owner(k, 30_000, "ZAU12")), Outcome::Applied);
    assert_eq!(
        apply(&mut t, &owner(k, 29_000, "ZAU34")),
        Outcome::Refused(R_STALE)
    );
    assert_eq!(flight_get(&t, &k).unwrap().owner, code("ZAU12"));
}

// REQ: OA-FLT-012
#[test]
fn a_flight_leaves_only_by_a_logged_removal_and_a_full_table_refuses_instead_of_evicting() {
    let mut t = flights_new(&FlightConfig {
        capacity: 4,
        removal_log: 2,
        ..cfg()
    })
    .unwrap();
    for n in 1..=4u128 {
        live(&mut t, gk(n));
    }
    assert_eq!(
        apply(&mut t, &set_plan(gk(5), ZAU, 2_000, ST_ACTIVE)),
        Outcome::Refused(R_TABLE_FULL)
    );
    assert_eq!(flights_len(&t), 4);
    assert!(flight_get(&t, &gk(1)).is_some(), "nothing was evicted");
    assert!(
        !flights_remove(&mut t, &gk(99), 3_000, REM_EXPLICIT),
        "no such flight"
    );
    assert!(flights_remove(&mut t, &gk(1), 3_000, REM_EXPLICIT));
    assert_eq!(t.removals.len(), 1);
    assert_eq!(
        (t.removals[0].key, t.removals[0].reason, t.removals[0].state),
        (gk(1), REM_EXPLICIT, ST_ACTIVE)
    );
    flights_check(&t).unwrap();
    assert_eq!(
        apply(&mut t, &set_plan(gk(5), ZAU, 4_000, ST_ACTIVE)),
        Outcome::Applied
    );

    // Sweep: a flight whose plans ended is retired after the retention, a silent live one after
    // the silence limit; each is logged, and none is removed when the log has no room.
    assert_eq!(
        apply(&mut t, &op(OP_COMPLETE, gk(2), ZAU, 5_000)),
        Outcome::Applied
    );
    assert_eq!(flights_sweep(&mut t, 10_000, 100_000, 1_000_000), 0);
    let removed = flights_sweep(&mut t, 70_000 + 100_001, 100_000, 1_000_000_000);
    assert_eq!(
        removed, 1,
        "only the finished flight; the log has room for one more"
    );
    assert_eq!(t.removals[1].reason, REM_RETIRED);
    assert_eq!(t.removals[1].key, gk(2));
    let blocked = flights_sweep(&mut t, 10_000_000, 100_000, 1_000);
    assert_eq!(blocked, 0, "the removal log is full, so nothing is removed");
    assert!(t.stats.removals_blocked > 0);
    assert_eq!(t.stats.removed, 2);
    flights_log_clear(&mut t);
    let silent = flights_sweep(&mut t, 10_000_000, 100_000, 1_000);
    assert_eq!(silent, 2, "capped by the log: two entries of room");
    assert!(t.removals.iter().all(|r| r.reason == REM_SILENT));
    flights_check(&t).unwrap();
    assert_eq!(t.stats.created - t.stats.removed, flights_len(&t) as u64);
}

// REQ: OA-FLT-016
#[test]
fn the_table_allocates_nothing_after_it_is_made() {
    let mut t = table();
    let caps = |t: &FlightTable| {
        (
            t.flights.capacity(),
            t.free.capacity(),
            t.refusals.capacity(),
            t.conflicts.capacity(),
            t.removals.capacity(),
        )
    };
    let before = caps(&t);
    let mut rng = Rng(1);
    for _ in 0..20_000 {
        let o = random_op(&mut rng);
        flights_apply(&mut t, &o);
        if rng.below(50) == 0 {
            let now = t.clock_ms;
            flights_sweep(&mut t, now, 5_000, 200_000);
            flights_log_clear(&mut t);
        }
    }
    assert_eq!(caps(&t), before);
    assert!(t.refusals.len() <= t.cfg.refusal_log);
    flights_check(&t).unwrap();
}

// ---- properties -------------------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const UNITS: [&str; 3] = ["ZAU12", "ZAU34", "ZMP05"];
const CENTRES: [[u8; 4]; 4] = [*b"ZAU\0", *b"ZMP\0", *b"ZKC\0", [0; 4]];

/// A random operation over a small domain, so flights, centres and values collide often. Times
/// move about, so staleness and ties occur.
fn random_op(r: &mut Rng) -> Op {
    let key = match r.below(8) {
        0 => key_tfms(code("TST1"), code("KAAA"), code("KBBB")),
        1 => key_tfms(code("TST1"), code("KAAA"), code("KCCC")),
        n => gk(u128::from(n)),
    };
    let centre = CENTRES[if r.below(12) == 0 {
        3
    } else {
        r.below(3) as usize
    }];
    let ts = 100_000 + (r.below(400) as i64) * 1_000;
    let seen = ts + (r.below(130) as i64) * 1_000 - 10_000;
    // A third of the operations are plan operations, so plans end and are filed again.
    let kind = if r.below(3) == 0 {
        1 + r.below(5)
    } else {
        1 + r.below(17)
    };
    let mut o = op_new(kind as u8, key, centre, ts, seen, r.next() >> 8);
    let unit = |r: &mut Rng| code::<8>(UNITS[r.below(3) as usize]);
    match o.kind {
        OP_SET_PLAN => {
            o.state = if r.below(3) == 0 {
                ST_PROPOSED
            } else {
                ST_ACTIVE
            };
            if r.below(2) == 0 {
                o.plan_id = code(["P1", "P2"][r.below(2) as usize]);
            }
        }
        OP_ASSIGN | OP_INTERIM | OP_REQUEST => {
            let form = match r.below(8) {
                0 => ALT_NONE,
                1 => ALT_VFR,
                2 => ALT_BLOCK,
                3 => ALT_AT_FIX,
                4 => ALT_NIL,
                5 => ALT_VFR_ON_TOP_PLUS,
                _ => ALT_SIMPLE,
            };
            let a = (r.below(4) as i32) * 1_000;
            o.alt = Alt {
                form,
                a_ft: if matches!(form, ALT_VFR | ALT_NIL | ALT_NONE) {
                    0
                } else {
                    a
                },
                b_ft: match form {
                    ALT_BLOCK | ALT_AT_FIX => a + (r.below(3) as i32 - 1) * 1_000,
                    _ => 0,
                },
            };
            if form == ALT_AT_FIX || r.below(20) == 0 {
                o.fix = code("FIX");
            }
        }
        OP_OWNER => o.unit_a = unit(r),
        OP_HO_INITIATE => {
            o.unit_a = unit(r);
            o.unit_b = unit(r);
        }
        OP_HO_UPDATE | OP_HO_ACCEPT | OP_HO_TAKE => o.unit_b = unit(r),
        OP_ADDRESS => o.address = 1 + r.below(3) as u32,
        OP_IDENTITY => {
            if r.below(2) == 0 {
                o.callsign = code("TST1");
            }
            if r.below(2) == 0 {
                o.dep = code("KAAA");
            }
            if r.below(2) == 0 {
                o.arr = code(["KBBB", "KCCC"][r.below(2) as usize]);
            }
        }
        _ => {}
    }
    o
}

/// The invariants the table states beyond [`flights_check`]: every flight has a plan view, each
/// centre has at most one view of it (so at most one plan per flight and centre, active or
/// not), every view is in a defined state, the counts add up and every flight that left is in
/// the log.
fn audit(t: &FlightTable, logged_removals: u64) {
    flights_check(t).unwrap();
    for f in t.flights.iter().filter(|f| f.key.kind != KEY_FREE) {
        let n = usize::from(f.n_views);
        assert!((1..=MAX_VIEWS).contains(&n), "a flight has a plan view");
        for (i, v) in f.views[..n].iter().enumerate() {
            assert!((1..ST_COUNT as u8).contains(&v.state));
            assert!(
                f.views[..i].iter().all(|w| w.centre != v.centre),
                "one plan per centre"
            );
        }
    }
    assert_eq!(t.stats.created, flights_len(t) as u64 + t.stats.removed);
    assert_eq!(t.stats.removed, logged_removals, "every removal is logged");
    let refused: u64 = t.stats.refused.iter().sum();
    assert_eq!(t.stats.ops, t.stats.applied + t.stats.unchanged + refused);
    assert!(t.refusals.len() as u64 <= refused);
}

// REQ: OA-FLT-013
#[test]
fn every_short_sequence_of_operations_leaves_a_valid_record() {
    // Every sequence of up to four operations drawn from a fixed alphabet that covers every
    // operation kind, the interesting values and two centres, checked after every step.
    let k = gk(1);
    let mut alphabet: Vec<Op> = Vec::new();
    for (kind, state) in [(OP_SET_PLAN, ST_PROPOSED), (OP_SET_PLAN, ST_ACTIVE)] {
        let mut o = op(kind, k, ZAU, 0);
        o.state = state;
        alphabet.push(o);
    }
    alphabet.push(set_plan(k, ZMP, 0, ST_ACTIVE));
    for kind in [OP_DEPART, OP_COMPLETE, OP_CANCEL, OP_DROP] {
        alphabet.push(op(kind, k, ZAU, 0));
    }
    alphabet.push(assign(k, 0, 30_000));
    let mut blk = op(OP_ASSIGN, k, ZAU, 0);
    blk.alt = Alt {
        form: ALT_BLOCK,
        a_ft: 1_000,
        b_ft: 2_000,
    };
    alphabet.push(blk);
    let mut intr = op(OP_INTERIM, k, ZAU, 0);
    intr.alt = Alt {
        form: ALT_NIL,
        a_ft: 0,
        b_ft: 0,
    };
    alphabet.push(intr);
    alphabet.push(owner(k, 0, "ZAU12"));
    alphabet.push(ho(OP_HO_INITIATE, k, 0, "ZAU12", "ZAU34"));
    alphabet.push(ho(OP_HO_ACCEPT, k, 0, "ZAU12", "ZAU34"));
    alphabet.push(ho(OP_HO_RETRACT, k, 0, "ZAU12", "ZAU34"));
    alphabet.push(ho(OP_HO_TAKE, k, 0, "ZAU12", "ZAU34"));
    let mut ad = op(OP_ADDRESS, k, ZAU, 0);
    ad.address = 0xABCDEF;
    alphabet.push(ad);
    let mut req = op(OP_REQUEST, k, ZAU, 0);
    req.alt = Alt {
        form: ALT_VFR,
        a_ft: 0,
        b_ft: 0,
    };
    alphabet.push(req);
    alphabet.push(ho(OP_HO_UPDATE, k, 0, "ZAU12", "ZAU56"));
    alphabet.push(ho(OP_HO_FAIL, k, 0, "ZAU12", "ZAU34"));
    let mut id = op(OP_IDENTITY, k, ZAU, 0);
    id.callsign = code("TST1");
    id.dep = code("KAAA");
    id.arr = code("KBBB");
    alphabet.push(id);
    // Out of order: stamped before the step that came before it (marked by a time of -1).
    let older = |mut o: Op| {
        o.ts_ms = -1;
        o
    };
    for o in [
        assign(k, 0, 20_000),
        owner(k, 0, "ZAU34"),
        op(OP_DROP, k, ZAU, 0),
        ho(OP_HO_ACCEPT, k, 0, "ZAU12", "ZAU34"),
    ] {
        alphabet.push(older(o));
    }
    let n = alphabet.len();
    let mut count = 0u64;
    for len in 1..=4usize {
        let mut idx = vec![0usize; len];
        loop {
            let mut t = table();
            for (step, &i) in idx.iter().enumerate() {
                let mut o = alphabet[i];
                // Strictly increasing stamps, seen after the floor.
                let at = 1_000 * (step as i64 + 1);
                o.ts_ms = if o.ts_ms == -1 { at - 1_500 } else { at };
                o.seen_ms = at + 61_000;
                o.msg = step as u64;
                flights_apply(&mut t, &o);
                audit(&t, 0);
            }
            count += 1;
            // Next sequence (odometer).
            let mut p = len;
            loop {
                if p == 0 {
                    break;
                }
                p -= 1;
                idx[p] += 1;
                if idx[p] < n {
                    break;
                }
                idx[p] = 0;
                if p == 0 {
                    p = usize::MAX;
                    break;
                }
            }
            if p == usize::MAX {
                break;
            }
        }
    }
    assert_eq!(count, (1..=4u32).map(|l| (n as u64).pow(l)).sum::<u64>());
}

// REQ: OA-FLT-013, OA-FLT-014
#[test]
fn random_long_sequences_keep_every_invariant_and_every_removal_is_logged() {
    let mut reasons = [0u64; R_COUNT];
    let mut conflicts = [0u64; C_COUNT];
    let (mut applied, mut completed, mut removed) = (0u64, 0u64, 0u64);
    for seed in 1..=2000u64 {
        let mut r = Rng(seed * 0x9e37_79b9 + 1);
        let mut t = flights_new(&FlightConfig {
            capacity: 6,
            ..cfg()
        })
        .unwrap();
        let mut logged = 0u64;
        for step in 0..60 {
            let o = random_op(&mut r);
            flights_apply(&mut t, &o);
            if step % 9 == 8 {
                let now = t.clock_ms;
                flights_sweep(&mut t, now, 400_000, 150_000);
            }
            if r.below(12) == 0 {
                let k = gk(1 + r.below(7) as u128);
                let now = t.clock_ms;
                flights_remove(&mut t, &k, now, REM_EXPLICIT);
            }
            logged += t.removals.len() as u64;
            flights_log_clear(&mut t);
            audit(&t, logged);
        }
        for (k, c) in t.stats.refused.iter().enumerate() {
            reasons[k % R_COUNT] += c;
        }
        for (k, c) in t.stats.conflicts.iter().enumerate() {
            conflicts[k] += c;
        }
        applied += t.stats.applied;
        completed += t.stats.handoffs_completed;
        removed += t.stats.removed;
    }
    // The generator reaches what it claims to: most refusal reasons, every conflict kind,
    // handoffs and removals.
    assert!(
        reasons.iter().filter(|&&n| n > 0).count() >= 9,
        "{reasons:?}"
    );
    assert!(conflicts.iter().all(|&n| n > 0), "{conflicts:?}");
    assert!(applied > 5_000 && completed > 0 && removed > 0);
}

// REQ: OA-FLT-015
#[test]
fn the_same_operations_give_the_same_record_bit_for_bit() {
    for seed in 1..=50u64 {
        let run = |seed: u64| {
            let mut r = Rng(seed);
            let mut t = flights_new(&FlightConfig {
                capacity: 8,
                ..cfg()
            })
            .unwrap();
            let mut digests = Vec::new();
            for step in 0..200 {
                flights_apply(&mut t, &random_op(&mut r));
                if step % 25 == 24 {
                    let now = t.clock_ms;
                    flights_sweep(&mut t, now, 30_000, 200_000);
                }
                digests.push(flights_digest(&t));
            }
            (t, digests)
        };
        let (a, da) = run(seed);
        let (b, db) = run(seed);
        assert_eq!(a, b, "the whole table, indexes and logs");
        assert_eq!(da, db, "the digest after every step");
        let (c, dc) = run(seed + 1000);
        assert_ne!(da, dc);
        assert_ne!(a, c);
    }
}

/// The operations of one message: one time, one message number.
fn in_msg(mut o: Op, msg: u64) -> Op {
    o.msg = msg;
    o.ts_ms = 5_000;
    o.seen_ms = 5_000 + 61_000;
    o
}

// REQ: OA-FLT-022
#[test]
fn the_operations_of_one_message_share_a_stamp_without_tying() {
    let mut t = table();
    let k = gk(1);
    // A flight first seen airborne: active with an actual departure in its first message.
    let a = in_msg(set_plan(k, ZAU, 0, ST_ACTIVE), 1);
    let d = in_msg(op(OP_DEPART, k, ZAU, 0), 1);
    assert_eq!(apply(&mut t, &a), Outcome::Applied);
    assert_eq!(
        apply(&mut t, &d),
        Outcome::Applied,
        "a forward step of the same message"
    );
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(view_of(&f, &ZAU).unwrap().state, ST_DEPARTED);
    // The owner clearing a handoff and the handoff op in one message.
    let init = in_msg(ho(OP_HO_INITIATE, k, 0, "ZAU12", "ZMP34"), 2);
    let own = in_msg(owner(k, 0, "ZMP34"), 2);
    assert_eq!(apply(&mut t, &init), Outcome::Applied);
    assert_eq!(
        apply(&mut t, &own),
        Outcome::Applied,
        "the owner of the same message"
    );
    assert_eq!(flight_get(&t, &k).unwrap().ho_state, HO_NONE);
    // Another message at the same time with a different value is still a tie.
    let other = in_msg(op(OP_DROP, k, ZAU, 0), 3);
    assert_eq!(
        apply(&mut t, &other),
        Outcome::Refused(R_SAME_STAMP_DIFFERENT)
    );
}

// REQ: OA-FLT-022
#[test]
fn a_refused_plan_operation_never_makes_a_flight() {
    let mut t = table();
    let k = gk(1);
    assert_eq!(
        apply(&mut t, &set_plan(k, [0; 4], 1_000, ST_ACTIVE)),
        Outcome::Refused(R_BAD_VALUE)
    );
    assert_eq!(flights_len(&t), 0);
    assert_eq!(t.stats.created, 0);
}

// REQ: OA-FLT-008
#[test]
fn a_new_centre_for_a_flight_whose_plans_ended_is_refused_unless_it_is_a_new_plan() {
    let mut t = table();
    let k = gk(1);
    let mut first = set_plan(k, ZAU, 1_000, ST_ACTIVE);
    first.plan_id = code("P1");
    assert_eq!(apply(&mut t, &first), Outcome::Applied);
    assert_eq!(
        apply(&mut t, &op(OP_COMPLETE, k, ZAU, 2_000)),
        Outcome::Applied
    );
    // The same plan reported by another centre after it ended.
    let mut same = set_plan(k, ZMP, 3_000, ST_ACTIVE);
    same.plan_id = code("P1");
    assert_eq!(apply(&mut t, &same), Outcome::Refused(R_TERMINAL));
    let blank = set_plan(k, ZMP, 3_000, ST_ACTIVE);
    assert_eq!(apply(&mut t, &blank), Outcome::Refused(R_TERMINAL));
    assert_eq!(flight_get(&t, &k).unwrap().n_views, 1);
    // A different plan identifier is the key filed again.
    let mut new = set_plan(k, ZMP, 4_000, ST_ACTIVE);
    new.plan_id = code("P2");
    assert_eq!(apply(&mut t, &new), Outcome::Applied);
    assert_eq!(t.stats.conflicts[C_PLAN_REFILED as usize], 1);
    assert_eq!(flight_get(&t, &k).unwrap().n_views, 2);
}

// REQ: OA-FLT-010
#[test]
fn an_owner_older_than_the_handoff_it_would_end_leaves_the_handoff_and_is_counted() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    assert_eq!(apply(&mut t, &owner(k, 2_000, "ZAU12")), Outcome::Applied);
    assert_eq!(
        apply(&mut t, &ho(OP_HO_INITIATE, k, 9_000, "ZAU12", "ZMP34")),
        Outcome::Applied
    );
    let before = flight_get(&t, &k).unwrap().stamps[F_HANDOFF];
    // The owner is older than the offer: it is applied, the handoff stays and its stamp is kept.
    assert_eq!(apply(&mut t, &owner(k, 5_000, "ZMP34")), Outcome::Applied);
    let f = flight_get(&t, &k).unwrap();
    assert_eq!(f.ho_state, HO_OFFERED);
    assert_eq!(f.stamps[F_HANDOFF], before);
    assert_eq!(t.stats.handoff_clears_skipped, 1);
}

// REQ: OA-FLT-010
#[test]
fn taking_control_is_by_the_unit_that_accepted() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    apply(&mut t, &ho(OP_HO_INITIATE, k, 2_000, "ZAU12", "ZMP34"));
    apply(&mut t, &ho(OP_HO_ACCEPT, k, 3_000, "", "ZMP34"));
    assert_eq!(
        apply(&mut t, &ho(OP_HO_TAKE, k, 4_000, "", "ZMP99")),
        Outcome::Refused(R_BAD_TRANSITION)
    );
    assert_eq!(
        apply(&mut t, &ho(OP_HO_TAKE, k, 4_000, "", "ZMP34")),
        Outcome::Applied
    );
}

// REQ: OA-FLT-004
#[test]
fn a_question_is_not_advanced_past_an_operation_that_will_be_refused_for_its_key_or_time() {
    let mut t = table();
    let k = gk(1);
    live(&mut t, k);
    let clock = t.clock_ms;
    let mut bad = set_plan(key_gufi(0), ZAU, 5_000_000, ST_ACTIVE);
    bad.seen_ms = 5_000_000 + 61_000;
    assert_eq!(flights_op_avail(&t, &bad), clock, "a bad key");
    let mut future = op(OP_ASSIGN, k, ZAU, 9_000_000);
    future.seen_ms = 1_000;
    assert_eq!(flights_op_avail(&t, &future), clock, "a bad time");
    let good = assign(k, 5_000_000, 30_000);
    assert_eq!(flights_op_avail(&t, &good), 5_000_000 + 61_000);
}

// REQ: OA-FLT-025
#[test]
fn flights_flying_with_a_callsign_are_counted_without_the_ended_and_the_dropped() {
    let mut t = table();
    let id = |k: FlightKey, cs: &str, ts: i64| {
        let mut o = op(OP_IDENTITY, k, ZAU, ts);
        o.callsign = code(cs);
        o
    };
    for n in 1..=4u128 {
        live(&mut t, gk(n));
        assert_eq!(apply(&mut t, &id(gk(n), "ABC1", 2_000)), Outcome::Applied);
    }
    assert_eq!(flights_flying_by_callsign(&t, &code("ABC1")).count, 4);
    assert_eq!(flights_flying_by_callsign(&t, &code("ZZZ9")).count, 0);
    assert_eq!(
        flights_flying_by_callsign(&t, &[0; 8]).count,
        0,
        "no callsign names no flight"
    );
    apply(&mut t, &op(OP_COMPLETE, gk(1), ZAU, 3_000));
    apply(&mut t, &op(OP_DROP, gk(2), ZAU, 3_000));
    let r = flights_flying_by_callsign(&t, &code("ABC1"));
    assert_eq!(r.count, 2);
    assert!(flight_is_flying(&r.first.unwrap()));
    assert!(
        flight_is_live(&flight_get(&t, &gk(2)).unwrap()),
        "dropped is live but not flying"
    );
    assert!(!flight_is_flying(&flight_get(&t, &gk(2)).unwrap()));
}

// REQ: OA-FLT-030
#[test]
fn of_several_flying_flights_with_one_callsign_the_newest_plan_is_found_the_same_whatever_the_order(
) {
    let build = |order: &[u128]| {
        let mut t = table();
        for &n in order {
            // Flight n is stamped n seconds after the start; all state one route.
            let ts = (n as i64) * 1_000;
            assert_eq!(
                apply(&mut t, &set_plan(gk(n), ZAU, ts, ST_ACTIVE)),
                Outcome::Applied
            );
            let mut id = op(OP_IDENTITY, gk(n), ZAU, ts);
            id.callsign = code("ABC1");
            id.dep = code("KAAA");
            id.arr = code("KBBB");
            assert_eq!(apply(&mut t, &id), Outcome::Applied);
        }
        t
    };
    for order in [[1u128, 2, 3], [3, 1, 2], [2, 3, 1]] {
        let t = build(&order);
        let r = flights_flying_by_callsign(&t, &code("ABC1"));
        assert_eq!(r.count, 3);
        assert!(r.same_route);
        assert_eq!(r.newest.unwrap().key, gk(3), "the newest plan stamp");
    }
    // One flight with another arrival: the flights no longer share a route.
    let mut t = build(&[1, 2]);
    live(&mut t, gk(9));
    let mut id = op(OP_IDENTITY, gk(9), ZAU, 9_000);
    id.callsign = code("ABC1");
    id.dep = code("KAAA");
    id.arr = code("KCCC");
    apply(&mut t, &id);
    assert!(!flights_flying_by_callsign(&t, &code("ABC1")).same_route);
    // A route that is not stated is not shared.
    let mut t = table();
    for n in 1..=2u128 {
        live(&mut t, gk(n));
        let mut id = op(OP_IDENTITY, gk(n), ZAU, 2_000);
        id.callsign = code("XYZ9");
        apply(&mut t, &id);
    }
    assert!(!flights_flying_by_callsign(&t, &code("XYZ9")).same_route);
}

// REQ: OA-FLT-033
#[test]
fn every_flight_that_claims_an_address_is_counted_and_the_flying_ones_copied_newest_first() {
    let mut t = table();
    let claim = |t: &mut FlightTable, n: u128, ts: i64| {
        live(t, gk(n));
        let mut o = op(OP_ADDRESS, gk(n), ZAU, ts);
        o.address = 0xABCDEF;
        apply(t, &o);
    };
    for (n, ts) in [(1u128, 2_000), (2, 3_000), (3, 4_000)] {
        claim(&mut t, n, ts);
    }
    let mut out = Vec::new();
    let c = flights_claiming_address(&t, 0xABCDEF, &mut out, 8);
    assert_eq!((c.total, c.flying, c.sole_dropped), (3, 3, false));
    let keys: Vec<FlightKey> = out.iter().map(|f| f.key).collect();
    assert_eq!(keys, [gk(3), gk(2), gk(1)]);
    // The most asked for is copied; the counts are of all of them.
    let mut few = Vec::new();
    let c = flights_claiming_address(&t, 0xABCDEF, &mut few, 2);
    assert_eq!((c.total, c.flying, few.len()), (3, 3, 2));
    assert_eq!(
        flights_claiming_address(&t, 0x123456, &mut Vec::new(), 8),
        Claimants::default()
    );
    assert_eq!(flights_claiming_address(&t, 0, &mut Vec::new(), 8).total, 0);
    // A flight that takes another address leaves the chain.
    let mut o = op(OP_ADDRESS, gk(2), ZAU, 9_000);
    o.address = 0x111111;
    apply(&mut t, &o);
    let mut out = Vec::new();
    assert_eq!(flights_claiming_address(&t, 0xABCDEF, &mut out, 8).total, 2);
    assert_eq!(
        out.iter().map(|f| f.key).collect::<Vec<_>>(),
        [gk(3), gk(1)]
    );
    // A claimant that is dropped is counted and not copied; alone it is flagged.
    apply(&mut t, &op(OP_DROP, gk(1), ZAU, 10_000));
    let mut out = Vec::new();
    let c = flights_claiming_address(&t, 0xABCDEF, &mut out, 8);
    assert_eq!(
        (c.total, c.flying, c.sole_dropped, out.len()),
        (2, 1, false, 1)
    );
    apply(&mut t, &op(OP_DROP, gk(3), ZAU, 11_000));
    let mut out = Vec::new();
    let c = flights_claiming_address(&t, 0xABCDEF, &mut out, 8);
    assert_eq!(
        (c.total, c.flying, c.sole_dropped, out.len()),
        (2, 0, false, 0)
    );
    let mut lone = table();
    live(&mut lone, gk(7));
    let mut o = op(OP_ADDRESS, gk(7), ZAU, 2_000);
    o.address = 0x777777;
    apply(&mut lone, &o);
    apply(&mut lone, &op(OP_DROP, gk(7), ZAU, 3_000));
    let c = flights_claiming_address(&lone, 0x777777, &mut Vec::new(), 8);
    assert_eq!((c.total, c.flying, c.sole_dropped), (1, 0, true));
}

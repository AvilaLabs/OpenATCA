//! Offline, scripted two-sector handoff workflow.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_flight::*;

const ZAU: [u8; 4] = *b"ZAU\0";

fn code<const N: usize>(text: &str) -> [u8; N] {
    let mut out = [0; N];
    for (to, from) in out.iter_mut().zip(text.bytes()) {
        *to = from;
    }
    out
}

fn op(kind: u8, key: FlightKey, ts_ms: i64, msg: u64) -> Op {
    op_new(kind, key, ZAU, ts_ms, ts_ms + 61_000, msg)
}

fn handoff(kind: u8, key: FlightKey, ts_ms: i64, msg: u64) -> Op {
    let mut operation = op(kind, key, ts_ms, msg);
    operation.unit_a = code("ZAU12");
    operation.unit_b = code("ZMP34");
    operation
}

fn table() -> FlightTable {
    flights_new(&FlightConfig {
        capacity: 4,
        lag_floor_ms: 60_000,
        max_future_ms: 5_000,
        refusal_log: 16,
        conflict_log: 4,
        removal_log: 4,
    })
    .unwrap()
}

fn start_flight(table: &mut FlightTable, key: FlightKey, msg: u64) {
    let mut plan = op(OP_SET_PLAN, key, 1_000, msg);
    plan.state = ST_ACTIVE;
    assert_eq!(flights_apply(table, &plan), Outcome::Applied);
    let mut owner = op(OP_OWNER, key, 2_000, msg + 1);
    owner.unit_a = code("ZAU12");
    assert_eq!(flights_apply(table, &owner), Outcome::Applied);
}

#[test]
fn two_sector_offer_accept_timeout_and_stale_duplicate_delivery() {
    let accepted_key = key_gufi(901);
    let timed_out_key = key_gufi(902);
    let mut table = table();
    start_flight(&mut table, accepted_key, 1);
    start_flight(&mut table, timed_out_key, 10);

    // Offer and accept do not transfer ownership. A duplicate acceptance is idempotent.
    let offer = handoff(OP_HO_INITIATE, accepted_key, 3_000, 3);
    assert_eq!(flights_apply(&mut table, &offer), Outcome::Applied);
    let accept = handoff(OP_HO_ACCEPT, accepted_key, 4_000, 4);
    assert_eq!(flights_apply(&mut table, &accept), Outcome::Applied);
    assert_eq!(flights_apply(&mut table, &accept), Outcome::Unchanged);
    let accepted = flight_get(&table, &accepted_key).unwrap();
    assert_eq!(accepted.owner, code("ZAU12"));
    assert_eq!(accepted.ho_state, HO_ACCEPTED);
    assert_eq!(accepted.ho_accepted_by, code("ZMP34"));

    // Only the named accepting sector taking control changes the single owner.
    let take = handoff(OP_HO_TAKE, accepted_key, 5_000, 5);
    assert_eq!(flights_apply(&mut table, &take), Outcome::Applied);
    let transferred = flight_get(&table, &accepted_key).unwrap();
    assert_eq!(transferred.owner, code("ZMP34"));
    assert_eq!(transferred.ho_state, HO_NONE);

    // A delivery older than the accepted event is refused and cannot roll the record back.
    let stale_accept = handoff(OP_HO_ACCEPT, accepted_key, 3_500, 40);
    assert_eq!(
        flights_apply(&mut table, &stale_accept),
        Outcome::Refused(R_STALE)
    );
    assert_eq!(
        flight_get(&table, &accepted_key).unwrap().owner,
        code("ZMP34")
    );

    // The timeout is represented by the existing explicit failure transition. It clears the
    // pending offer while retaining the prior owner; a later accept is refused as no handoff.
    let pending = handoff(OP_HO_INITIATE, timed_out_key, 3_000, 12);
    assert_eq!(flights_apply(&mut table, &pending), Outcome::Applied);
    let timeout = handoff(OP_HO_FAIL, timed_out_key, 63_000, 13);
    assert_eq!(flights_apply(&mut table, &timeout), Outcome::Applied);
    let timed_out = flight_get(&table, &timed_out_key).unwrap();
    assert_eq!(timed_out.owner, code("ZAU12"));
    assert_eq!(timed_out.ho_state, HO_NONE);
    assert_eq!(
        flights_apply(
            &mut table,
            &handoff(OP_HO_ACCEPT, timed_out_key, 64_000, 14)
        ),
        Outcome::Refused(R_NO_HANDOFF)
    );

    flights_check(&table).unwrap();
    assert_eq!(table.stats.handoffs_completed, 1);
    assert_eq!(table.stats.handoffs_failed, 1);
    assert_eq!(
        table.stats.refused[usize::from(OP_HO_ACCEPT) * R_COUNT + usize::from(R_STALE)],
        1
    );
    assert_eq!(
        table.stats.refused[usize::from(OP_HO_ACCEPT) * R_COUNT + usize::from(R_NO_HANDOFF)],
        1
    );
}

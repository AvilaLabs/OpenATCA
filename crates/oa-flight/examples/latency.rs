#![forbid(unsafe_code)]
//! Native shell measurement of direct oa-flight operation latency; clock and reporting stay here.

use std::time::Instant;

use oa_flight::{
    flights_apply, flights_check, flights_new, key_gufi, op_new, FlightConfig, Outcome,
    OP_HO_ACCEPT, OP_HO_INITIATE, OP_HO_TAKE, OP_OWNER, OP_SET_PLAN, ST_ACTIVE,
};

const LIMIT: usize = 100_000;
const UNIT_A: [u8; 8] = *b"UNIT-A  ";
const UNIT_B: [u8; 8] = *b"UNIT-B  ";
const CENTRE: [u8; 4] = *b"ZNY ";

fn main() {
    if let Err(error) = run() {
        eprintln!("oa-flight latency: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let n = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "10000".to_owned())
        .parse::<usize>()
        .map_err(|_| "sample count must be an integer")?;
    if !(100..=LIMIT).contains(&n) {
        return Err(format!("sample count must be 100..={LIMIT}"));
    }
    let mut table = flights_new(&FlightConfig {
        capacity: n,
        lag_floor_ms: 0,
        max_future_ms: 0,
        refusal_log: n,
        conflict_log: n,
        removal_log: n,
    })
    .map_err(|e| format!("flight table: {e:?}"))?;
    let mut handoff_ns = Vec::with_capacity(n);
    let mut amendment_ns = Vec::with_capacity(n);

    for i in 0..n {
        let key = key_gufi((i + 1) as u128);
        let t = (i as i64).saturating_mul(10);
        apply_ok(&mut table, plan(key, t, i as u64, ST_ACTIVE))?;
        let mut owner = op_new(OP_OWNER, key, CENTRE, t + 1, t + 1, i as u64 + 1);
        owner.unit_a = UNIT_A;
        apply_ok(&mut table, owner)?;

        let handoff_start = Instant::now();
        let mut initiate = op_new(OP_HO_INITIATE, key, CENTRE, t + 2, t + 2, i as u64 + 2);
        initiate.unit_a = UNIT_A;
        initiate.unit_b = UNIT_B;
        apply_ok(&mut table, initiate)?;
        let mut accept = op_new(OP_HO_ACCEPT, key, CENTRE, t + 3, t + 3, i as u64 + 3);
        accept.unit_b = UNIT_B;
        apply_ok(&mut table, accept)?;

        let mut take = op_new(OP_HO_TAKE, key, CENTRE, t + 4, t + 4, i as u64 + 4);
        take.unit_b = UNIT_B;
        let outcome = flights_apply(&mut table, &take);
        require_applied(outcome, "handoff take")?;
        handoff_ns.push(handoff_start.elapsed().as_nanos().min(u64::MAX as u128) as u64);

        // Changing a live view's plan id records the core's explicit plan-change conflict.
        let mut amendment = plan(key, t + 5, i as u64 + 5, ST_ACTIVE);
        amendment.plan_id = [(i % 251 + 2) as u8; 12];
        let start = Instant::now();
        let outcome = flights_apply(&mut table, &amendment);
        amendment_ns.push(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
        require_applied(outcome, "plan amendment")?;
    }
    flights_check(&table).map_err(|e| format!("final invariant audit failed: {e:?}"))?;
    if table.stats.refused.iter().any(|&count| count != 0)
        || table.stats.removals_blocked != 0
        || table.stats.refusals_unlisted != 0
        || table.stats.conflicts_unlisted != 0
        || table.stats.ops != (n as u64).saturating_mul(6)
        || table.stats.applied != table.stats.ops
        || table.stats.conflicts.iter().sum::<u64>() != n as u64
    {
        return Err("run refused: one or more operations were refused or lost from a log".into());
    }
    let handoff = distribution(&mut handoff_ns);
    let amendment = distribution(&mut amendment_ns);
    let handoff_p99_pass = percentile(&handoff_ns, 99) < 10_000_000;
    let amendment_p99_pass = percentile(&amendment_ns, 99) < 10_000_000;

    println!(
        "{{\"schema\":\"openatca-latency-v1\",\"source\":\"oa-flight-public-api\",\"samples\":{n},\"targets\":{{\"display_p99_limit_ms\":100,\"alert_same_scan\":true,\"handoff_p99_limit_ms\":10,\"handoff_p99_pass\":{},\"amendment_p99_limit_ms\":10,\"amendment_p99_pass\":{}}},\"handoff_take\":{},\"plan_amendment\":{},\"counts\":{{\"ops\":{},\"applied\":{},\"unchanged\":{},\"refused_by_op_reason\":{:?},\"conflicts_by_kind\":{:?},\"conflicts_unlisted\":{},\"created\":{},\"removed\":{},\"removals_blocked\":{},\"handoffs_completed\":{},\"invariant_audit\":\"passed\"}},\"scope_note\":\"Engineering targets for direct in-process oa-flight core operations; no FAA or published external latency target is claimed. Message receipt and end-to-end flight propagation are outside this timer.\"}}",
        handoff_p99_pass,
        amendment_p99_pass,
        handoff,
        amendment,
        table.stats.ops,
        table.stats.applied,
        table.stats.unchanged,
        table.stats.refused,
        table.stats.conflicts,
        table.stats.conflicts_unlisted,
        table.stats.created,
        table.stats.removed,
        table.stats.removals_blocked,
        table.stats.handoffs_completed,
    );
    Ok(())
}

fn plan(key: oa_flight::FlightKey, t: i64, msg: u64, state: u8) -> oa_flight::Op {
    let mut op = op_new(OP_SET_PLAN, key, CENTRE, t, t, msg);
    op.state = state;
    op.plan_id = [1; 12];
    op
}

fn apply_ok(table: &mut oa_flight::FlightTable, op: oa_flight::Op) -> Result<(), String> {
    require_applied(flights_apply(table, &op), "setup operation")
}

fn require_applied(outcome: Outcome, what: &str) -> Result<(), String> {
    match outcome {
        Outcome::Applied => Ok(()),
        Outcome::Unchanged => Err(format!("{what} was unchanged")),
        Outcome::Refused(reason) => Err(format!("{what} refused with reason {reason}")),
    }
}

fn distribution(samples: &mut [u64]) -> String {
    samples.sort_unstable();
    let at = |percent: usize| -> u64 {
        let rank = (samples.len() * percent).div_ceil(100).max(1);
        samples.get(rank - 1).copied().unwrap_or(0)
    };
    format!(
        "{{\"unit\":\"ns\",\"p50\":{},\"p95\":{},\"p99\":{},\"max\":{}}}",
        at(50),
        at(95),
        at(99),
        samples.last().copied().unwrap_or(0)
    )
}

fn percentile(samples: &[u64], percent: usize) -> u64 {
    let rank = (samples.len() * percent).div_ceil(100).max(1);
    samples.get(rank - 1).copied().unwrap_or(0)
}

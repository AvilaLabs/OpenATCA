#![forbid(unsafe_code)]
//! `oa-flight <recording-dir> [--from <unix ns>] [--to <unix ns>]`: replays SFDPS recordings into
//! the flight record and prints counts: decoded forms, lag, refusals, conflicts, removals.

use std::path::PathBuf;
use std::process::ExitCode;

use oa_flight::{FlightConfig, R_COUNT};
use oa_flight_sfdps::{lag_quantile_s, recordings_in, replay_sfdps, ReplayConfig};

fn main() -> ExitCode {
    let mut dir: Option<PathBuf> = None;
    let (mut from, mut to) = (i64::MIN, i64::MAX);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--from" | "--to" => {
                let Some(v) = args.next().and_then(|v| v.parse::<i64>().ok()) else {
                    eprintln!("{a} needs a number");
                    return ExitCode::from(2);
                };
                if a == "--from" {
                    from = v;
                } else {
                    to = v;
                }
            }
            _ if dir.is_none() => dir = Some(PathBuf::from(a)),
            _ => {
                eprintln!("usage: oa-flight <recording-dir> [--from <ns>] [--to <ns>]");
                return ExitCode::from(2);
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("usage: oa-flight <recording-dir> [--from <ns>] [--to <ns>]");
        return ExitCode::from(2);
    };
    // The sealed hold-out guard sees the directory first; every file is opened through it too.
    if let Err(e) = oa_sealed::refuse(&dir) {
        eprintln!("oa-flight: {e}");
        return ExitCode::from(2);
    }
    let paths = match recordings_in(&dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let cfg = ReplayConfig {
        flight: FlightConfig {
            capacity: 120_000,
            lag_floor_ms: 58_000,
            max_future_ms: 5_000,
            refusal_log: 64,
            conflict_log: 64,
            removal_log: 4096,
        },
        from_ns: from,
        to_ns: to,
        sweep_every_ms: 60_000,
        retain_ms: 3_600_000,
        silence_ms: 6 * 3_600_000,
    };
    let rep = match replay_sfdps(paths, &cfg, &mut |_, _| {}) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "frames {} skipped_before {} after_end {}",
        rep.frames, rep.frames_skipped_before, rep.frames_after_end
    );
    println!(
        "docs_unparsed {} docs_refused {}",
        rep.docs_unparsed, rep.docs_refused
    );
    println!(
        "rows {} lag_ms min {} max {}",
        rep.lag.rows, rep.lag.min_ms, rep.lag.max_ms
    );
    for (n, d) in [(50, 100), (95, 100), (99, 100)] {
        println!("lag p{n} s {:?}", lag_quantile_s(&rep.lag, n, d));
    }
    println!("assigned_forms {:?}", rep.assigned_forms);
    println!("requested_forms {:?}", rep.requested_forms);
    println!("handoff_events {:?}", rep.handoff_events);
    println!(
        "interim value {} nil {}",
        rep.interim_value, rep.interim_nil
    );
    println!(
        "refused_by (root, elements, mismatch, rows, strings) {:?}",
        rep.refused_by
    );
    for (k, n) in rep.present.iter().enumerate() {
        println!(
            "present {} {n}",
            oa_fixm::FIELD_NAMES.get(k).copied().unwrap_or("?")
        );
    }
    for (k, n) in rep
        .fixm
        .field_errors
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
    {
        println!(
            "field_error {} {n}",
            oa_fixm::FIELD_NAMES.get(k).copied().unwrap_or("?")
        );
    }
    println!(
        "fixm messages {} rows {} rejected {:?} unknown_elements {} unknown_attributes {}",
        rep.fixm.messages,
        rep.fixm.rows,
        rep.fixm.rejected,
        rep.fixm.unknown_elements,
        rep.fixm.unknown_attributes
    );
    println!("adapter {:?}", rep.adapter);
    let s = &rep.table.stats;
    println!(
        "ops {} applied {} unchanged {} lag_raised {} clock_held {} created {} removed {} blocked {}",
        s.ops, s.applied, s.unchanged, s.lag_raised, s.clock_held, s.created, s.removed, s.removals_blocked
    );
    println!(
        "views_added {} reinstated {} ho_completed {} ho_implicit {} ho_retracted {} ho_failed {}",
        s.views_added,
        s.reinstated,
        s.handoffs_completed,
        s.handoffs_implicit,
        s.handoffs_retracted,
        s.handoffs_failed
    );
    println!(
        "conflicts {:?} unlisted {}",
        s.conflicts, s.conflicts_unlisted
    );
    println!("refused total {}", s.refused.iter().sum::<u64>());
    println!("handoff_clears_skipped {}", s.handoff_clears_skipped);
    for (k, n) in s.refused.iter().enumerate().filter(|(_, n)| **n > 0) {
        println!("refused op {} reason {} {n}", k / R_COUNT, k % R_COUNT);
    }
    for ((op, r, st), n) in &rep.refusals {
        println!("refusal op {op} reason {r} state {st} {n}");
    }
    println!("view_states {:?}", rep.view_states);
    println!("removed_by_reason {:?}", rep.removed_by_reason);
    println!("flights_held {}", oa_flight::flights_len(&rep.table));
    ExitCode::SUCCESS
}

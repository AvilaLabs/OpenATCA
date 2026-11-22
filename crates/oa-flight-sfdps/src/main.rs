#![forbid(unsafe_code)]
//! `oa-flight <recording-dir> [--from <unix ns>] [--to <unix ns>]
//! [--measure-from <unix ns>]`: replays SFDPS recordings into the flight record. The optional
//! timing window reports bounded per-received-second frame-processing work.

use std::path::PathBuf;
use std::process::ExitCode;

use oa_flight::{FlightConfig, R_COUNT};
use oa_flight_sfdps::{
    lag_quantile_s, recordings_in, replay_sfdps_counted, replay_sfdps_timed_counted, ReplayConfig,
    ReplayTimingReport,
};

fn print_timing(t: &ReplayTimingReport) {
    println!(
        "sfdps_timing scope=frame_callback_decode_adapter_table_sweep read_decompression_merge_and_receive_gates_excluded concurrent_replay_loops_unsynchronized=true measure_from_ns={} measure_to_ns_inclusive={} first_received_ns={:?} last_received_ns={:?} seconds_expected={} seconds_observed={} seconds_empty={} timed_frames={} processing_wall_ms={:.3} p95_observed_second_wall_ms={:.3} max_observed_second_wall_ms={:.3}",
        t.measure_from_ns,
        t.measure_to_ns,
        t.first_received_ns,
        t.last_received_ns,
        t.expected_seconds,
        t.observed_seconds,
        t.empty_seconds,
        t.timed_frames,
        t.processing_wall_ns as f64 / 1_000_000.0,
        t.p95_observed_second_ns as f64 / 1_000_000.0,
        t.max_observed_second_ns as f64 / 1_000_000.0,
    );
}

fn main() -> ExitCode {
    // Printed when `main` returns: on every exit, an error or a refusal included. Frames by
    // receive time and messages by their own time.
    let mut drops = oa_sealed::DropReport::new();
    let mut dir: Option<PathBuf> = None;
    let (mut from, mut to, mut measure_from) = (i64::MIN, i64::MAX, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--from" | "--to" | "--measure-from" => {
                let Some(v) = args.next().and_then(|v| v.parse::<i64>().ok()) else {
                    eprintln!("{a} needs a number");
                    return ExitCode::from(2);
                };
                match a.as_str() {
                    "--from" => from = v,
                    "--to" => to = v,
                    "--measure-from" => measure_from = Some(v),
                    _ => unreachable!(),
                }
            }
            _ if dir.is_none() => dir = Some(PathBuf::from(a)),
            _ => {
                eprintln!("usage: oa-flight <recording-dir> [--from <ns>] [--to <ns>] [--measure-from <ns>]");
                return ExitCode::from(2);
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!(
            "usage: oa-flight <recording-dir> [--from <ns>] [--to <ns>] [--measure-from <ns>]"
        );
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
    let mut sealed = 0u64;
    let replayed = match measure_from {
        Some(start) => {
            replay_sfdps_timed_counted(paths, &cfg, &mut |_, _, _| {}, &mut sealed, start)
                .map(|(report, timing)| (report, Some(timing)))
        }
        None => replay_sfdps_counted(paths, &cfg, &mut |_, _, _| {}, &mut sealed)
            .map(|report| (report, None)),
    };
    drops.add(sealed);
    let (rep, timing) = match replayed {
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
    if let Some(timing) = timing {
        print_timing(&timing);
    }
    println!(
        "docs_unparsed {} docs_refused {}",
        rep.docs_unparsed, rep.docs_refused
    );
    if let Some(scan) = &rep.merge {
        println!(
            "input_scan files={} decoded_bytes={} frame_and_gzip_errors={} sealed_frames={} sealed_message_rows={}",
            scan.files,
            scan.bytes,
            oa_replay::errors_total(&scan.errors),
            scan.sealed_window_dropped,
            rep.sealed_window_dropped.saturating_sub(scan.sealed_window_dropped),
        );
    }
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

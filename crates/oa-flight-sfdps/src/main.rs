#![forbid(unsafe_code)]
//! `oa-flight <recording-dir> [--from <unix ns>] [--to <unix ns>]
//! [--measure-from <unix ns>]`: replays SFDPS recordings into the flight record. The optional
//! timing window reports bounded per-received-second frame-processing work.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oa_flight::{FlightConfig, R_COUNT};
use oa_flight_sfdps::{
    lag_quantile_s, recordings_in, replay_sfdps_audited_counted, replay_sfdps_counted,
    replay_sfdps_timed_counted, validate_replay_window, ReplayConfig, ReplayReport,
    ReplayTimingReport,
};

// C6/C7: local trace is at most 1 GiB, with an 8 KiB fixed-schema record cap and
// 2 MiB reserved for bounded input receipts and terminal counters/status. Limits cannot be raised by input.
const AUDIT_BYTES: u64 = 1 << 30;
const AUDIT_RECORD_BYTES: usize = 8192;
const AUDIT_FOOTER_BYTES: u64 = 2 * 1024 * 1024;
const COMPATIBILITY_BYTES: usize = 1 << 20;

fn output_key(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "output path must name a file")
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(std::fs::canonicalize(parent)?.join(name))
}

fn preflight_compatibility_path(path: &Path, audit_path: &Path) -> io::Result<()> {
    oa_sealed::refuse_io(path)?;
    if output_key(path)? == output_key(audit_path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "H2 audit and compatibility outputs must be distinct",
        ));
    }
    // seal-allow: sealed-path guard and distinct-output check ran above; this metadata probe only
    // refuses an existing destination, whose contents are never read.
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "H2 compatibility output must be new",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_compatibility(path: &Path, records: &str, complete: bool) -> io::Result<()> {
    oa_sealed::refuse_io(path)?;
    let status = format!(
        "STATUS\tstatus={}\n",
        if complete {
            "measurement-complete"
        } else {
            "INCOMPLETE"
        }
    );
    let total = records
        .len()
        .checked_add(status.len())
        .ok_or_else(|| io::Error::other("H2 compatibility output length overflow; INCOMPLETE"))?;
    if total > COMPATIBILITY_BYTES {
        return Err(io::Error::other(
            "H2 compatibility sidecar exceeds fixed output bound; INCOMPLETE",
        ));
    }
    // seal-allow: private mode-0600 output is create_new-only after the sealed-path check above.
    let mut output = std::fs::OpenOptions::new();
    output.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        output.mode(0o600);
    }
    let mut file = output.open(path)?;
    file.write_all(records.as_bytes())?;
    file.write_all(status.as_bytes())?;
    file.flush()?;
    file.sync_all()
}

fn compatibility_run_records(
    compatibility: &oa_flight_sfdps::compat::CompatibilityAudit,
    build: &str,
    cfg: &ReplayConfig,
    stats: &oa_flight_sfdps::audit::AuditStats,
    inputs: &[oa_replay::source::ObservedInput],
) -> io::Result<String> {
    use std::fmt::Write as _;
    let records = compatibility.records()?;
    let (header, body) = records
        .split_once('\n')
        .ok_or_else(|| io::Error::other("H2 compatibility header missing"))?;
    let mut out = String::with_capacity(
        records
            .len()
            .saturating_add(inputs.len().saturating_mul(180))
            .saturating_add(1024),
    );
    writeln!(out, "{header}").map_err(|_| io::Error::other("formatting compatibility header"))?;
    writeln!(out, "CONFIG\tfrom_ns={}\tto_ns_inclusive={}\tcapacity={}\tlag_floor_ms={}\tmax_future_ms={}\trefusal_log={}\tconflict_log={}\tremoval_log={}\tsweep_every_ms={}\tretain_ms={}\tsilence_ms={}", cfg.from_ns,cfg.to_ns,cfg.flight.capacity,cfg.flight.lag_floor_ms,cfg.flight.max_future_ms,cfg.flight.refusal_log,cfg.flight.conflict_log,cfg.flight.removal_log,cfg.sweep_every_ms,cfg.retain_ms,cfg.silence_ms)
        .map_err(|_| io::Error::other("formatting compatibility config"))?;
    out.push_str(build);
    writeln!(out, "RUN_BINDING\tinput_count={}\tframes_visited={}\trows_delivered={}\treader_errors={}\tcap_refused={}\tsink_failed={}\tqualification=not-assessed", inputs.len(), stats.frames_visited, stats.rows_delivered, oa_replay::errors_total(&stats.merge_errors), u8::from(stats.cap_refused), u8::from(stats.sink_failed))
        .map_err(|_| io::Error::other("formatting compatibility run binding"))?;
    for (index, input) in inputs.iter().enumerate() {
        let receipt = input.receipt.get();
        let digest = if receipt.opened {
            let mut hex = String::with_capacity(64);
            for byte in receipt.sha256 {
                let _ = write!(hex, "{byte:02x}");
            }
            hex
        } else {
            "none".into()
        };
        writeln!(out, "COMPAT_INPUT\tindex={index}\topened={}\teof={}\tbytes={}\treserved_bytes={}\tsha256={digest}", u8::from(receipt.opened),u8::from(receipt.eof),receipt.bytes,input.reserved_bytes)
            .map_err(|_| io::Error::other("formatting compatibility input binding"))?;
    }
    out.push_str(body);
    Ok(out)
}

fn audited_replay(
    paths: Vec<PathBuf>,
    cfg: &ReplayConfig,
    path: &Path,
    compatibility_path: Option<&Path>,
    sealed: &mut u64,
) -> io::Result<ReplayReport> {
    use oa_flight_sfdps::audit::{AuditLimits, AuditStats, ReplayAudit};
    oa_sealed::refuse_io(path)?;
    if let Some(compatibility_path) = compatibility_path {
        preflight_compatibility_path(compatibility_path, path)?;
    }
    let build = oa_flight_sfdps::audit::build_record(
        option_env!("OA_BUILD_COMMIT"),
        option_env!("OA_BUILD_DIRTY"),
    )?;
    // seal-allow: private mode-0600 output is create_new-only after the sealed-path check above.
    let mut output = std::fs::OpenOptions::new();
    output.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        output.mode(0o600);
    }
    let file = output.open(path)?;
    let mut writer = io::BufWriter::new(file);
    writeln!(writer, "H2-OBSERVED-STATE-1\tscope=SFDPS-replay-compatibility\tindependent_predictions=not-measured\tindependent_agreement_denominator=not-observable")?;
    writeln!(writer, "CONFIG\tfrom_ns={}\tto_ns_inclusive={}\tcapacity={}\tlag_floor_ms={}\tmax_future_ms={}\trefusal_log={}\tconflict_log={}\tremoval_log={}\tsweep_every_ms={}\tretain_ms={}\tsilence_ms={}",cfg.from_ns,cfg.to_ns,cfg.flight.capacity,cfg.flight.lag_floor_ms,cfg.flight.max_future_ms,cfg.flight.refusal_log,cfg.flight.conflict_log,cfg.flight.removal_log,cfg.sweep_every_ms,cfg.retain_ms,cfg.silence_ms)?;
    writer.write_all(build.as_bytes())?;
    let mut bytes = 2048u64; // conservative header charge, separate from the reserved footer
    let mut sink = |event: &oa_flight_sfdps::audit::Observation| -> io::Result<()> {
        let record = oa_flight_sfdps::audit::observation_text(event);
        if record.len() > AUDIT_RECORD_BYTES {
            return Err(io::Error::other(
                "H2 audit record exceeds fixed schema bound; INCOMPLETE",
            ));
        }
        let len =
            u64::try_from(record.len()).map_err(|_| io::Error::other("audit length overflow"))?;
        let next = bytes
            .checked_add(len)
            .ok_or_else(|| io::Error::other("audit byte count overflow"))?;
        if next > AUDIT_BYTES - AUDIT_FOOTER_BYTES {
            return Err(io::Error::other(
                "H2 audit output byte cap reached; INCOMPLETE",
            ));
        }
        writer.write_all(record.as_bytes())?;
        bytes = next;
        Ok(())
    };
    let (mut result, stats, inputs, compatibility) = {
        let mut audit = ReplayAudit {
            limits: AuditLimits {
                frames: 2_000_000,
                rows: 4_000_000,
                events: 40_000_000,
            },
            stats: AuditStats::default(),
            inputs: Vec::new(),
            sink: &mut sink,
        };
        let mut compatibility = oa_flight_sfdps::compat::CompatibilityAudit::default();
        let result = if compatibility_path.is_some() {
            oa_flight_sfdps::replay_sfdps_compat_counted(
                paths,
                cfg,
                &mut |_, _, _| {},
                sealed,
                &mut audit,
                &mut compatibility,
            )
        } else {
            replay_sfdps_audited_counted(paths, cfg, &mut |_, _, _| {}, sealed, &mut audit)
        };
        (result, audit.stats, audit.inputs, compatibility)
    };
    if result.is_ok() && compatibility_path.is_some() {
        if let Err(error) = compatibility.validate_completed() {
            result = Err(error);
        }
    }
    let mut input_records = String::new();
    for (index, input) in inputs.iter().enumerate() {
        input_records.push_str(&oa_replay::source::observed_input_line(index, input)?);
    }
    use std::fmt::Write as _;
    let mut footer = match &result {
        Ok(report) => format!("RESULT\tstatus=measurement-complete\tqualification=not-assessed\tframes={}\tdecoded_rows={}\tdocs_unparsed={}\tdocs_refused={}\treader_loss={}\n", report.frames, report.lag.rows, report.docs_unparsed, report.docs_refused, u8::from(oa_replay::errors_total(&stats.merge_errors)>0)),
        Err(error) => {
            let message = error.to_string();
            let mut escaped = String::new();
            for byte in message.as_bytes().iter().take(8192) { let _ = write!(escaped,"{byte:02x}"); }
            format!("RESULT\tstatus=INCOMPLETE\terror_hex={escaped}\n")
        }
    };
    let _ = writeln!(footer,"COUNTS\tframes_visited={}\trows_delivered={}\tevents_delivered={}\tmissing_key_or_time={}\tzero_op_rows={}\tcap_refused={}\tsink_failed={}\tsealed_drops={}\tmerge_files={}\tmerge_bytes={}\treader_steps={}\tdecoded_bytes_charged={}\traw_input_bytes_reserved={}\traw_input_cap_refused={}",stats.frames_visited,stats.rows_delivered,stats.events_delivered,stats.rows_missing_key_or_time,stats.rows_without_operation,u8::from(stats.cap_refused),u8::from(stats.sink_failed),stats.sealed_drops,stats.merge_files,stats.merge_bytes,stats.reader_steps_used,stats.decoded_bytes_charged,stats.raw_input_bytes_reserved,u8::from(stats.raw_input_cap_refused));
    for (reason, count) in stats.merge_errors.iter().enumerate() {
        let _ = writeln!(footer, "READER_ERROR\treason={reason}\tcount={count}");
    }
    for (kind, ((applied, unchanged), refused)) in stats
        .operations_applied
        .iter()
        .zip(&stats.operations_unchanged)
        .zip(&stats.operations_refused)
        .enumerate()
    {
        let _ = writeln!(
            footer,
            "OP_COUNTS\tkind={kind}\tapplied={applied}\tunchanged={unchanged}"
        );
        for (reason, count) in refused.iter().enumerate() {
            let _ = writeln!(
                footer,
                "OP_REFUSAL\tkind={kind}\treason={reason}\tcount={count}"
            );
        }
    }
    footer.push_str(&input_records);
    if u64::try_from(footer.len()).map_or(true, |len| len > AUDIT_FOOTER_BYTES) {
        return Err(io::Error::other(
            "H2 terminal audit metadata exceeds bound; INCOMPLETE",
        ));
    }
    writer.write_all(footer.as_bytes())?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    if let Some(compatibility_path) = compatibility_path {
        let records = compatibility_run_records(&compatibility, &build, cfg, &stats, &inputs)?;
        write_compatibility(compatibility_path, &records, result.is_ok())?;
    }
    result
}

fn print_timing(t: &ReplayTimingReport) {
    println!(
        "sfdps_timing scope=message_processing source_io_excluded=true sealed_messages_excluded=true measure_from_ns={} measure_to_ns_inclusive={} first_received_ns={:?} last_received_ns={:?} seconds_expected={} seconds_observed={} seconds_empty={} timed_frames={} processing_wall_ms={:.3} p95_observed_second_wall_ms={:.3} max_observed_second_wall_ms={:.3}",
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
    let mut audit_to: Option<PathBuf> = None;
    let mut compatibility_to: Option<PathBuf> = None;
    let (mut from, mut to, mut measure_from) = (i64::MIN, i64::MAX, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--audit-to" => {
                let Some(value) = args.next() else {
                    eprintln!("--audit-to needs a new private output path");
                    return ExitCode::from(2);
                };
                if audit_to.replace(PathBuf::from(value)).is_some() {
                    eprintln!("duplicate --audit-to");
                    return ExitCode::from(2);
                }
            }
            "--compatibility-to" => {
                let Some(value) = args.next() else {
                    eprintln!("--compatibility-to needs a new private output path");
                    return ExitCode::from(2);
                };
                if compatibility_to.replace(PathBuf::from(value)).is_some() {
                    eprintln!("duplicate --compatibility-to");
                    return ExitCode::from(2);
                }
            }
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
                eprintln!("usage: oa-flight <recording-dir> [--from <ns>] [--to <ns>] [--measure-from <ns>] [--audit-to <new-private-file> [--compatibility-to <new-private-file>]]");
                return ExitCode::from(2);
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!(
            "usage: oa-flight <recording-dir> [--from <ns>] [--to <ns>] [--measure-from <ns>] [--audit-to <new-private-file> [--compatibility-to <new-private-file>]]"
        );
        return ExitCode::from(2);
    };
    if audit_to.is_some() && measure_from.is_some() {
        eprintln!("--audit-to and --measure-from are distinct diagnostics; run one at a time");
        return ExitCode::from(2);
    }
    if compatibility_to.is_some() && audit_to.is_none() {
        eprintln!("--compatibility-to requires --audit-to");
        return ExitCode::from(2);
    }
    if audit_to.is_some() {
        if let Err(error) = validate_replay_window(from, to) {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    }
    // The sealed hold-out guard sees the directory first; every file is opened through it too.
    if let Err(e) = oa_sealed::refuse(&dir) {
        eprintln!("oa-flight: {e}");
        return ExitCode::from(2);
    }
    let listing = if audit_to.is_some() {
        oa_flight_sfdps::replay::recordings_for_audit(&dir)
    } else {
        recordings_in(&dir)
    };
    let paths = match listing {
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
    let replayed = if let Some(path) = audit_to {
        audited_replay(paths, &cfg, &path, compatibility_to.as_deref(), &mut sealed)
            .map(|report| (report, None))
    } else {
        match measure_from {
            Some(start) => {
                replay_sfdps_timed_counted(paths, &cfg, &mut |_, _, _| {}, &mut sealed, start)
                    .map(|(report, timing)| (report, Some(timing)))
            }
            None => replay_sfdps_counted(paths, &cfg, &mut |_, _, _| {}, &mut sealed)
                .map(|report| (report, None)),
        }
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
    println!(
        "position_evidence held={} updates={} capacity_refusals={} out_of_order_ignored={} future_message_time_ignored={} future_position_time_ignored={} refiled_held={} refile_capacity_refusals={}",
        rep.positions.len(),
        rep.positions.updated,
        rep.positions.refused_full,
        rep.positions.out_of_order,
        rep.positions.bad_time,
        rep.positions.bad_position_time,
        rep.positions.refiled_len(),
        rep.positions.refiled_refused,
    );
    println!("flights_held {}", oa_flight::flights_len(&rep.table));
    ExitCode::SUCCESS
}

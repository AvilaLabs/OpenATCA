//! Replay normalized OAF observations through a separate ACK consumer and report M2 refusal
//! when required paired system state is unavailable. This is an offline diagnostic, not a switch.
#![forbid(unsafe_code)]

use oa_migration_switch::EndpointClass;
use oa_regional_session::migration_drill::{
    consumer_worker, run_recorded_stream, RecordedDrillReport, RecordedRunOptions,
    MAX_CONSUMER_FRAME_BYTES, MAX_NORMALIZED_EVENTS, MAX_ZAU_BOX_ADDRESSES,
    MAX_ZAU_DISTINCT_ADDRESSES, ZAU_POSITION_ASSOCIATION_MAX_AGE_NS,
};
use oa_regional_session::recordings_reader::RecordingSource;
use oa_regional_session::recordings_stream::{
    StreamInput, MAX_H2_INPUT_FRAMES, MAX_H2_INPUT_STREAMS, MAX_H2_PHYSICAL_INPUT_BYTES,
};
use std::env;
use std::path::PathBuf;

type ParsedArgs = (
    Vec<StreamInput>,
    [u8; 4],
    EndpointClass,
    PathBuf,
    f64,
    bool,
);

fn parse_args() -> Result<ParsedArgs, String> {
    let mut args = env::args_os().skip(1);
    let mut inputs = Vec::new();
    let mut centre = None;
    let mut endpoint_class = None;
    let mut model_snapshot = None;
    let mut map_range_nm = None;
    let mut local_authority_fixture = false;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--adsb") | Some("--sfdps") => {
                if inputs.len() >= MAX_H2_INPUT_STREAMS { return Err("input stream cap exceeded".into()); }
                let source = if flag.to_str() == Some("--adsb") { RecordingSource::Adsb } else { RecordingSource::Sfdps };
                let path = args.next().map(PathBuf::from).ok_or("missing path after source flag")?;
                inputs.push(StreamInput { source, path });
            }
            Some("--centre") if centre.is_none() => {
                let value = args.next().ok_or("missing value after --centre")?;
                let text = value.to_str().ok_or("centre must be UTF-8")?;
                if text.len() != 4 || !text.as_bytes().iter().all(u8::is_ascii_alphanumeric) {
                    return Err("centre must be four ASCII alphanumeric bytes".into());
                }
                let mut bytes = [0u8; 4];
                bytes.copy_from_slice(text.as_bytes());
                centre = Some(bytes);
            }
            Some("--endpoint-class") if endpoint_class.is_none() => {
                let value = args.next().ok_or("missing value after --endpoint-class")?;
                endpoint_class = Some(match value.to_str() {
                    Some("terminal") => EndpointClass::Terminal,
                    Some("enroute") => EndpointClass::EnrouteLongRange,
                    _ => return Err("endpoint class must be terminal or enroute".into()),
                });
            }
            Some("--model-snapshot") if model_snapshot.is_none() => {
                model_snapshot = Some(PathBuf::from(args.next().ok_or("missing path after --model-snapshot")?));
            }
            Some("--map-range-nm") if map_range_nm.is_none() => {
                let value = args.next().ok_or("missing value after --map-range-nm")?;
                map_range_nm = Some(
                    value.to_str().ok_or("map range must be UTF-8")?
                        .parse::<f64>()
                        .ok()
                        .filter(|range| range.is_finite())
                        .ok_or("map range must be a finite decimal")?,
                );
            }
            Some("--local-authority-fixture") if !local_authority_fixture => {
                local_authority_fixture = true;
            }
            _ => return Err("usage: regional-m3-recorded-drill (--adsb FILE | --sfdps FILE)+ --centre ZAU1 --endpoint-class terminal|enroute --model-snapshot FILE --map-range-nm NM [--local-authority-fixture] (fixture is unauthenticated and never represents FAA legacy/manual state)".into()),
        }
    }
    if inputs.is_empty() {
        return Err("at least one recorded input is required".into());
    }
    Ok((
        inputs,
        centre.ok_or("--centre is required")?,
        endpoint_class.ok_or("--endpoint-class is required")?,
        model_snapshot.ok_or("--model-snapshot is required")?,
        map_range_nm.ok_or("--map-range-nm is required")?,
        local_authority_fixture,
    ))
}

fn emit(report: &RecordedDrillReport) {
    let alert_rows = report
        .replica_cycles
        .iter()
        .flat_map(|cycles| cycles.iter())
        .map(|cycle| cycle.active_ca_alerts)
        .sum::<u64>();
    println!("H5RECORDED\tversion=6\tmode=two_independent_same_implementation_replays\tprivileged_action_audit=UNAVAILABLE_PUBLISHED_M2_API\tauthority_mode={}\tqualification={}\tcentre={}\tendpoint_class={}\tmeasurement_start_ns={}\tmeasurement_end_ns={}\tcapture_start_ns={}\twarmup_events={}\tupdate_interval_ns={}\tdecision_limit_ns={}\tcycles_defined=100\tm2_ingest_attempts={}\tm2_cycles_refused={}\tflip_rollback_cycles_attempted={}\tflip_rollback_cycles_completed={}\taudit_events_written=0\tmax_retained_snapshot_bytes_per_replica={}\tdrill_runtime_cap_s=300\tphysical_input_byte_cap={}\tinput_frame_cap={}\tinput_stream_cap={}\tnormalized_event_cap={}\tconsumer_event_frame_cap_bytes={}\tnormalized_events={}\tposition_inputs={}\tflight_inputs={}\thandoff_operations={}\tactive_ca_alert_frame_rows={}\tquarantine_records={}\tmanual_handling_required={}\tinput_worker_hash_echo_acks={}\twarmup_worker_hash_echo_acks={}\tselected_state_apply_ack_rows={}\treplay_projection_equal_cycles={}\tmodel_sha256={}\tmap_centre_lat_e7={}\tmap_centre_lon_e7={}\tmap_range_nm={}\tprofile_max_frames={}\tprofile_max_tracks={}\tprofile_max_flights={}\tprofile_max_total_events={}\tprofile_max_baro_evidence={}\tprofile_max_drain_rows={}\tprofile_max_ca_candidates={}\tprofile_max_open_alerts={}\tprofile_max_ca_work_per_frame={}\tworker_input_gaps=0\tworker_input_duplicates=0\tworker_input_out_of_order=0\tworker_output_gaps=0\tworker_output_duplicates=0\tworker_output_out_of_order=0\tconsumer_scope=bounded_local_worker_applies_only_transmitted_selected_rows_to_typed_tables_no_external_endpoint_or_source_completeness_claim\tdecision_to_selected_output_ack_ns=PER_CYCLE\ttrack_output_loss=UNOBSERVABLE\tactive_alert_loss=UNOBSERVABLE\tdata_block_loss=UNOBSERVABLE\thandoff_state_change=UNOBSERVABLE\tinterface_gap_duplicate_order=UNOBSERVABLE\tstate_families_available={},{},{},{}",
        report.authority_mode,
        report.qualification, report.centre, report.endpoint_class,
        report.window_start_ns, report.window_end_ns,
        report.capture_start_ns, report.warmup_event_count,
        report.update_interval_ns, report.decision_limit_ns,
        report.m2_attempt_count,
        report.m2_refusal_count,
        report.flip_rollback_attempt_count,
        report.flip_rollback_complete_count,
        report.max_retained_snapshot_bytes_per_replica,
        MAX_H2_PHYSICAL_INPUT_BYTES, MAX_H2_INPUT_FRAMES, MAX_H2_INPUT_STREAMS,
        MAX_NORMALIZED_EVENTS, MAX_CONSUMER_FRAME_BYTES,
        report.normalized_event_count, report.position_input_count, report.flight_input_count,
        report.handoff_operation_count, alert_rows, report.quarantine_record_count,
        report.manual_handling_required_count,
        report.input_ack_count, report.warmup_input_ack_count, report.replay_output_ack_count,
        report.cycles.iter().filter(|cycle| cycle.replay_projection_equal).count(),
        report.model_sha256, report.map_centre_lat_e7, report.map_centre_lon_e7, report.map_range_nm,
        report.run_profile.surveillance.max_frames, report.run_profile.surveillance.max_tracks,
        report.run_profile.max_flights, report.run_profile.max_total_events,
        report.run_profile.surveillance.max_baro_evidence,
        report.run_profile.surveillance.max_drain_rows,
        report.run_profile.surveillance.max_candidate_pairs,
        report.run_profile.surveillance.max_open_alerts,
        report.run_profile.surveillance.max_ca_work_per_frame,
        u8::from(report.source_state_families_available[0]), u8::from(report.source_state_families_available[1]),
        u8::from(report.source_state_families_available[2]), u8::from(report.source_state_families_available[3]));
    let profile = report.run_profile.surveillance;
    let ca_candidate_bytes = (profile.max_candidate_pairs as u128) * 16;
    let ca_open_alert_bytes = (profile.max_open_alerts as u128) * 96;
    let ca_closed_alert_bytes = (profile.max_open_alerts as u128) * 96;
    let ca_persistent_bytes = ca_candidate_bytes + ca_open_alert_bytes + ca_closed_alert_bytes;
    let ca_frame_alert_copy_bytes = ca_open_alert_bytes + ca_closed_alert_bytes;
    println!("REPLAY_BOUNDS\tprofile=recorded_peak\tmax_tracks={}\tmax_flights={}\tmax_frames={}\tmax_events={}\tmax_baro_evidence={}\tmax_drain_rows={}\tca_candidates={}\tca_candidate_bytes={}\tca_open_alert_bytes={}\tca_closed_alert_bytes={}\tca_persistent_bytes={}\tca_frame_alert_copy_bytes={}\tca_grid_work_units_per_frame={}\tca_work_unit=band_step_or_cell_probe_or_track_in_cell\tca_eval_budget_per_frame={}\tframe_history_retained=0\tcompleteness_requires_zero_work_short_zero_input_short_zero_tracks_unscanned=1\tmemory_accounting=dominant_CA_tables_only; excludes decoded model, tracker/flight tables, source buffers, BTreeMap allocator overhead, transient frame copies, and process allocator metadata",
        report.run_profile.surveillance.max_tracks, report.run_profile.max_flights,
        report.run_profile.surveillance.max_frames, report.run_profile.max_total_events,
        report.run_profile.surveillance.max_baro_evidence,
        report.run_profile.surveillance.max_drain_rows,
        report.run_profile.surveillance.max_candidate_pairs,
        ca_candidate_bytes, ca_open_alert_bytes, ca_closed_alert_bytes,
        ca_persistent_bytes, ca_frame_alert_copy_bytes,
        report.run_profile.surveillance.max_ca_work_per_frame,
        report.run_profile.surveillance.max_candidate_pairs);
    println!("INPUT_TOTALS\tframes={}\tmessages={}\tstatus_frames={}\treject_frames={}\tphysical_bytes={}\texpanded_bytes={}\tframe_work_units={}",
        report.stream_counts.frames, report.stream_counts.messages,
        report.stream_counts.status_frames, report.stream_counts.reject_frames,
        report.stream_counts.physical_bytes, report.stream_counts.expanded_bytes,
        report.stream_counts.frame_work_units);
    let region = &report.region_filter;
    println!("SPATIAL_FILTER\tsource={}\tclassification=coarse_box_not_ARTCC_boundary\tlat_min_e7={}\tlat_max_e7={}\tlon_min_e7={}\tlon_max_e7={}\tposition_rows_seen={}\tposition_rows_in_box={}\tposition_rows_outside_box={}\tposition_rows_without_address={}\tactive_addresses_at_end={}\tdistinct_addresses_seen={}\tactive_address_cap={}\tdistinct_address_cap={}\tposition_association_max_age_ns={}\tflight_rows_seen={}\tflight_rows_with_stale_in_box_position={}\tflight_rows_without_in_box_position={}\texpired_address_memberships={}\tother_rows_seen={}\tadmitted_events={}\tcoverage_claim=none; ADS-B/source thinning and missing rows prevent total-aircraft or complete-traffic inference",
        region.source, region.lat_min_e7, region.lat_max_e7, region.lon_min_e7, region.lon_max_e7,
        region.position_rows_seen, region.position_rows_in_box, region.position_rows_outside_box,
        region.position_rows_without_address, region.active_in_box_addresses_at_end,
        region.distinct_addresses_seen, MAX_ZAU_BOX_ADDRESSES, MAX_ZAU_DISTINCT_ADDRESSES,
        ZAU_POSITION_ASSOCIATION_MAX_AGE_NS, region.flight_rows_seen,
        region.flight_rows_with_stale_in_box_position, region.flight_rows_without_in_box_position,
        region.expired_address_memberships,
        region.other_rows_seen, region.admitted_events);
    for stream in &report.input_streams {
        println!(
            "STREAM\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            stream.source,
            stream.sha256,
            stream.bytes,
            stream.messages,
            stream
                .first_frame_receive_ns
                .map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            stream
                .last_frame_receive_ns
                .map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            stream.max_frame_gap_ns,
            u8::from(stream.source_full_window_covered),
            stream.source_largest_gap_ns
        );
    }
    for cycle in &report.cycles {
        println!("CYCLE\tindex={}\tstart_ns={}\tend_ns={}\tposition_inputs={}\tflight_inputs={}\thandoff_operations={}\tinput_worker_hash_echo_acks={}\tinput_max_hash_echo_ns={}\tselected_output={}\tselected_output_state_apply_ack_rows={}\tselected_output_apply_batch_ack_ns={}\tdecision_to_selected_output_ack_ns={}\trollback_output_apply_ack_ns={}\tflip_rollback_attempted={}\tselected_state_generation={}\tselected_state_family_counts={:?}\tselected_state_sha256={}\tprojections_equal={}\tbaseline_frames={}\tcandidate_frames={}\tbaseline_tracks={}\tcandidate_tracks={}\tbaseline_flights={}\tcandidate_flights={}\tbaseline_handoffs={}\tcandidate_handoffs={}\tbaseline_alerts={}\tcandidate_alerts={}\tbaseline_export_dropped={:?}\tcandidate_export_dropped={:?}\tbaseline_projection_sha256={}\tcandidate_projection_sha256={}\tm2_ingest_refusal={}\tm2_validation_ns={}\tm1_transition_refusal={}\tm1_validation_ns={}\tflip_rollback_status={}\tunavailable={}",
            cycle.index, cycle.start_ns, cycle.end_ns, cycle.position_inputs,
            cycle.flight_inputs, cycle.handoff_operations, cycle.consumer_acks,
            cycle.consumer_max_ack_ns.map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            cycle.selected_output, cycle.replay_output_acks, cycle.replay_output_max_ack_ns,
            cycle.decision_to_selected_output_ack_ns.map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            cycle.rollback_output_apply_ack_ns.map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            u8::from(cycle.flip_rollback_attempted),
            cycle.selected_state_applied_generation.map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            cycle.selected_state_applied_family_counts.map_or_else(|| "UNAVAILABLE".into(), |v| format!("{v:?}")),
            cycle.selected_state_applied_sha256.as_deref().unwrap_or("UNAVAILABLE"),
            u8::from(cycle.replay_projection_equal),
            report.replica_cycles[0].get(cycle.index).map_or(0, |r| r.frames),
            report.replica_cycles[1].get(cycle.index).map_or(0, |r| r.frames),
            cycle.baseline_track_rows, cycle.second_track_rows,
            cycle.baseline_flight_views, cycle.second_flight_views,
            cycle.baseline_handoff_views, cycle.second_handoff_views,
            cycle.baseline_active_ca_alerts, cycle.second_active_ca_alerts,
            cycle.selector_export_dropped_baseline, cycle.selector_export_dropped_candidate,
            cycle.baseline_projection_sha256, cycle.second_projection_sha256,
            cycle.m2_ingest_refusal.as_deref().unwrap_or("NOT_ATTEMPTED"),
            cycle.m2_validation_ns.map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            cycle.m1_transition_refusal.as_deref().unwrap_or("NOT_ATTEMPTED"),
            cycle.m1_validation_ns.map_or_else(|| "UNAVAILABLE".into(), |v| v.to_string()),
            cycle.flip_rollback_status, cycle.unavailable);
    }
    println!("REPLAY_CAPACITY_FIELDS\tcycle\tpass\tca_work_short\tca_input_short\tca_tracks_unscanned\tca_scan_complete");
    for (pass, rows) in report.replica_cycles.iter().enumerate() {
        for (cycle, row) in rows.iter().enumerate() {
            let complete =
                row.ca_work_short == 0 && row.ca_input_short == 0 && row.ca_tracks_unscanned == 0;
            println!(
                "REPLAY_CAPACITY\t{cycle}\t{pass}\t{}\t{}\t{}\t{}",
                row.ca_work_short,
                row.ca_input_short,
                row.ca_tracks_unscanned,
                u8::from(complete),
            );
        }
    }
    println!("SUMMARY\tcycles_defined=100\tm2_ingest_attempts={}\tm2_refused_cycles={}\tflip_rollback_cycles_attempted={}\tflip_rollback_cycles_completed={}\tnormalized_input_worker_hash_echo_acks={}\twarmup_worker_hash_echo_acks={}\tselected_state_apply_ack_rows={}\tprojection_equal_cycles={}\tqualification={}",
        report.m2_attempt_count,
        report.m2_refusal_count,
        report.flip_rollback_attempt_count,
        report.flip_rollback_complete_count,
        report.input_ack_count, report.warmup_input_ack_count, report.replay_output_ack_count,
        report.cycles.iter().filter(|c| c.replay_projection_equal).count(), report.qualification);
}

fn main() {
    let mut args = env::args_os().skip(1);
    if args.next().as_deref().and_then(|value| value.to_str()) == Some("--h5-consumer-worker") {
        if let Err(error) = consumer_worker() {
            eprintln!("consumer worker: {error}");
            std::process::exit(2);
        }
        return;
    }
    let (
        inputs,
        centre,
        endpoint_class,
        model_path,
        map_range_nm,
        local_authority_fixture,
    ) = match parse_args() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let model_bytes = match oa_regional_session::cli::read_guarded(
        &model_path,
        oa_airspace::core::snapshot::MAX_SNAPSHOT_BYTES,
    ) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("guarded model snapshot refused: {error}");
            std::process::exit(1);
        }
    };
    let executable = match env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("cannot locate drill executable: {error}");
            std::process::exit(2);
        }
    };
    match run_recorded_stream(
        &inputs,
        centre,
        endpoint_class,
        &model_bytes,
        map_range_nm,
        RecordedRunOptions {
            worker_executable: &executable,
            local_authority_fixture,
        },
    ) {
        Ok(report) => emit(&report),
        Err(error) => {
            eprintln!("recorded drill refused: {error}");
            std::process::exit(1);
        }
    }
}

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use oa_airspace::core::{model, snapshot};
use oa_migration_switch::EndpointClass;
use oa_regional_session::migration_drill::{
    regional_replay_finish_observed, regional_replay_new_with_warmup,
    regional_replay_push_observed, run_recorded_stream, RecordedRunOptions, H5_CAPTURE_START_NS,
    H5_MEASUREMENT_END_NS, H5_MEASUREMENT_START_NS,
};
use oa_regional_session::recordings_reader::RecordingSource;
use oa_regional_session::recordings_stream::StreamInput;
use oa_regional_session::runtime::RegionalRunProfile;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
const SYNTHETIC_STEP_NS: i64 = 1_000_000_000;
const SYNTHETIC_BIN_NS: i64 = 36_000_000_000;

struct FixtureFile(PathBuf);

impl Drop for FixtureFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn synthetic_model() -> Vec<u8> {
    let mut m = model::empty_model(model::Meta {
        nasr_effective: 20261101,
        cifp_cycle: 2611,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [0; 32],
        cifp_sha256: [0; 32],
    });
    m.strings.text.push_str("KORD");
    m.strings.offsets.push(4);
    m.points.kind.push(model::point_kind::AIRPORT);
    m.points.lat.push(419_769_403);
    m.points.lon.push(-879_081_497);
    m.points.elev_ft.push(680);
    m.points.ident.push(model::StrIx::new(0));
    m.points.alt_ident.push(model::StrIx::NONE);
    m.points.region.push(model::StrIx::new(1));
    m.points.scope.push(model::StrIx::NONE);
    m.points.name.push(model::StrIx::NONE);
    m.strings.text.push_str("K5");
    m.strings.offsets.push(6);
    m.points.src_nasr = model::bit_zeros(1);
    m.points.src_cifp = model::bit_zeros(1);
    m.points.in_area = model::bit_zeros(1);
    m.points.cls_b = model::bit_zeros(1);
    m.points.cls_c = model::bit_zeros(1);
    m.points.cls_d = model::bit_zeros(1);
    m.points.cls_e = model::bit_zeros(1);
    model::bit_set(&mut m.points.src_nasr, 0, true);
    model::bit_set(&mut m.points.in_area, 0, true);
    snapshot::encode(&m).expect("valid synthetic ZAU model")
}

fn write_synthetic_adsb(path: &Path) {
    // seal-allow: test writes only a newly-created public synthetic OAF fixture under temp_dir.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("create synthetic OAF fixture");
    let mut sequence = 0u64;
    let mut receive_ns = H5_CAPTURE_START_NS;
    while receive_ns < H5_MEASUREMENT_END_NS {
        sequence += 1;
        let lon_delta = i64::try_from(sequence - 1)
            .expect("synthetic row index")
            .checked_mul(21_750)
            .expect("synthetic longitude progression");
        let lon_e7 = i32::try_from(i64::from(-934_000_000) + lon_delta)
            .expect("synthetic longitude coordinate");
        let row = oa_adsb::Row {
            time_ns: receive_ns,
            icao: 0x00ab_cdef,
            lat_e7: 419_000_000,
            lon_e7,
            baro_alt_ft: 35_000,
            gs_dkt: 3_500,
            track_ddeg: 900,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            source: 0,
            ..oa_adsb::ROW_EMPTY
        };
        let mut reports = oa_adsb::Reports::default();
        oa_adsb::reports_push(&mut reports, &row);
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &reports, 0, 1)
            .expect("encode synthetic ADS-B payload");
        let mut frame = vec![0; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let bytes = oa_feed::encode_frame(
            &mut frame,
            oa_feed::Kind::Message as u8,
            0,
            sequence,
            receive_ns,
            receive_ns / 1_000_000,
            &payload,
        )
        .expect("encode synthetic OAF frame");
        file.write_all(frame.get(..bytes).expect("encoded frame slice"))
            .expect("write synthetic frame");
        receive_ns += SYNTHETIC_STEP_NS;
    }
    file.sync_all().expect("sync synthetic OAF fixture");
}

fn write_synthetic_sfdps(path: &Path) {
    // seal-allow: test writes only a newly-created public synthetic OAF/FIXM fixture under temp_dir.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("create synthetic SFDPS OAF fixture");
    let mut sequence = 0u64;
    let mut receive_ns = H5_CAPTURE_START_NS;
    while receive_ns < H5_MEASUREMENT_END_NS {
        sequence += 1;
        let elapsed_seconds = (receive_ns - H5_CAPTURE_START_NS) / 1_000_000_000;
        let is_flight_update = elapsed_seconds % (SYNTHETIC_BIN_NS / 1_000_000_000)
            == SYNTHETIC_STEP_NS / 1_000_000_000;
        let (kind, payload) = if is_flight_update {
            let second_of_day = 20 * 3_600 + 50 * 60 + elapsed_seconds;
            let hour = (second_of_day / 3_600) % 24;
            let minute = (second_of_day / 60) % 60;
            let second = second_of_day % 60;
            let xml = format!(
                r#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZAU1" source="TH" system="SLC" timestamp="2026-11-19T{hour:02}:{minute:02}:{second:02}.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZAU1"/><aircraftDescription xsi:type="ns5:NasAircraftType" equipmentQualifier="L" aircraftAddress="ABCDEF" registration="N1TST" wakeTurbulence="M"/></flight></message></ns5:MessageCollection>"#
            );
            let payload = xml.into_bytes();
            (oa_feed::Kind::Message as u8, payload)
        } else {
            (
                oa_feed::Kind::Status as u8,
                b"synthetic status heartbeat".to_vec(),
            )
        };
        let frame_receive_ns = if is_flight_update {
            receive_ns
                .checked_add(1)
                .expect("synthetic receipt-time increment")
        } else {
            receive_ns
        };
        let mut frame = vec![0; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
        let bytes = oa_feed::encode_frame(
            &mut frame,
            kind,
            0,
            sequence,
            frame_receive_ns,
            frame_receive_ns / 1_000_000,
            &payload,
        )
        .expect("encode synthetic SFDPS OAF frame");
        file.write_all(frame.get(..bytes).expect("encoded frame slice"))
            .expect("write synthetic SFDPS frame");
        receive_ns += SYNTHETIC_STEP_NS;
    }
    file.sync_all().expect("sync synthetic SFDPS OAF fixture");
}

fn write_tracker_smoke_inputs(adsb_path: &Path, sfdps_path: &Path) {
    // Match recording_import_cli's tracker-confirmation row values and pack its ordered reports
    // into one OAF message, exercising the same decoded receipt batch in the production runtime.
    let mut adsb_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(adsb_path)
        .expect("create tracker smoke ADS-B fixture");
    let mut reports = oa_adsb::Reports::default();
    for sequence in 0..12u64 {
        let source_ns = H5_CAPTURE_START_NS
            .checked_add(i64::try_from(sequence).expect("smoke index") * 1_000_000_000)
            .expect("smoke ADS-B time");
        let lon_e7 = i32::try_from(-883_000_000i64 + i64::try_from(sequence).unwrap() * 21_750)
            .expect("smoke longitude");
        let row = oa_adsb::Row {
            time_ns: source_ns,
            icao: 0x00ab_cdef,
            lat_e7: 419_000_000,
            lon_e7,
            baro_alt_ft: 35_000,
            gs_dkt: 3_500,
            track_ddeg: 900,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
            source: 0,
            ..oa_adsb::ROW_EMPTY
        };
        oa_adsb::reports_push(&mut reports, &row);
    }
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &reports, 0, 12).expect("encode smoke ADS-B payload");
    let receipt_ns = H5_CAPTURE_START_NS + 11_000_000_000;
    let mut frame = vec![0; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
    let bytes = oa_feed::encode_frame(
        &mut frame,
        oa_feed::Kind::Message as u8,
        0,
        1,
        receipt_ns,
        receipt_ns / 1_000_000,
        &payload,
    )
    .expect("encode smoke ADS-B OAF frame");
    adsb_file
        .write_all(frame.get(..bytes).expect("smoke ADS-B frame slice"))
        .expect("write smoke ADS-B frame");
    adsb_file.sync_all().expect("sync smoke ADS-B file");

    let mut sfdps_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(sfdps_path)
        .expect("create tracker smoke SFDPS fixture");
    let receipt_ns = H5_CAPTURE_START_NS + 5_000_000_000;
    let xml = br#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZAU1" source="TH" system="SLC" timestamp="2026-11-19T20:50:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZAU1"/><aircraftDescription xsi:type="ns5:NasAircraftType" equipmentQualifier="L" aircraftAddress="ABCDEF" registration="N1TST" wakeTurbulence="M"/></flight></message></ns5:MessageCollection>"#;
    let mut frame = vec![0; oa_feed::HEADER_LEN + xml.len() + oa_feed::TRAILER_LEN];
    let bytes = oa_feed::encode_frame(
        &mut frame,
        oa_feed::Kind::Message as u8,
        0,
        1,
        receipt_ns,
        receipt_ns / 1_000_000,
        xml,
    )
    .expect("encode smoke SFDPS OAF frame");
    sfdps_file
        .write_all(frame.get(..bytes).expect("smoke SFDPS frame slice"))
        .expect("write smoke SFDPS frame");
    sfdps_file.sync_all().expect("sync smoke SFDPS file");
}

// REQ: OA-MIG-415
#[test]
fn known_good_adsb_and_fixm_smoke_reaches_runtime_and_rendered_snapshot() {
    let scratch = std::env::temp_dir().join(format!(
        "openatca-h5-runtime-smoke-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&scratch).expect("create runtime smoke temp directory");
    let adsb_path = scratch.join("smoke-adsb.oaf");
    let sfdps_path = scratch.join("smoke-sfdps.oaf");
    let _adsb_fixture = FixtureFile(adsb_path.clone());
    let _sfdps_fixture = FixtureFile(sfdps_path.clone());
    write_tracker_smoke_inputs(&adsb_path, &sfdps_path);
    let inputs = [
        StreamInput {
            source: RecordingSource::Adsb,
            path: adsb_path,
        },
        StreamInput {
            source: RecordingSource::Sfdps,
            path: sfdps_path,
        },
    ];
    let model_bytes = synthetic_model();
    let mut replay = regional_replay_new_with_warmup(
        &model_bytes,
        oa_sha256::sha256(&model_bytes),
        300.0,
        H5_CAPTURE_START_NS,
        H5_CAPTURE_START_NS,
        H5_CAPTURE_START_NS + 48_000_000_000,
        RegionalRunProfile::recorded_peak_rendered(),
    )
    .expect("construct real runtime smoke");
    let mut positions = 0u64;
    let mut flights = 0u64;
    let mut normalized = 0u64;
    let mut visited_frames = 0u64;
    let mut max_tracks = 0usize;
    let mut max_display_targets = 0usize;
    let summary = oa_regional_session::recordings_stream::stream_normalized_events(
        &inputs,
        Some((H5_CAPTURE_START_NS, H5_CAPTURE_START_NS + 48_000_000_000)),
        *b"ZAU1",
        |event, _provenance| {
            normalized += 1;
            match event.kind {
                oa_regional_session::EventKind::Position { .. } => positions += 1,
                oa_regional_session::EventKind::Flight(_) => flights += 1,
                _ => {}
            }
            regional_replay_push_observed(&mut replay, event, |_, frame| {
                visited_frames += 1;
                max_tracks = max_tracks.max(frame.surveillance.tracks.len());
                max_display_targets =
                    max_display_targets.max(frame.surveillance.display_targets.len());
                Ok(())
            })
        },
    )
    .expect("decode and normalize synthetic source files");
    regional_replay_finish_observed(replay, |_, frame| {
        visited_frames += 1;
        max_tracks = max_tracks.max(frame.surveillance.tracks.len());
        max_display_targets = max_display_targets.max(frame.surveillance.display_targets.len());
        Ok(())
    })
    .expect("finish runtime smoke replay");
    assert_eq!(positions, 12, "accepted normalized ADS-B positions");
    assert!(flights > 0, "no normalized FIXM flight events");
    assert_eq!(summary.normalized_events, normalized);
    assert_eq!(summary.normalized_surveillance_events, positions);
    assert_eq!(summary.normalized_flight_events, flights);
    assert_eq!(summary.normalized_other_events, 0);
    assert_eq!(summary.adapter_counts.adsb_frames, 1);
    assert_eq!(summary.adapter_counts.sfdps_frames, 1);
    assert_eq!(
        summary.adapter_counts.adsb_rows_in, 12,
        "all synthetic ADS-B rows reach the adapter counter"
    );
    assert!(
        summary.adapter_counts.sfdps_rows_decoded > 0,
        "synthetic FIXM source reaches the SFDPS row decoder"
    );
    assert!(visited_frames > 0, "runtime produced no frames");
    assert!(
        max_tracks > 0,
        "confirmed tracker output absent from RegionalFrame"
    );
    assert!(
        max_display_targets > 0,
        "rendered display target absent from RegionalFrame"
    );
}

// REQ: OA-MIG-414
#[test]
fn recorded_stream_runs_100_paired_m2_flip_rollback_bins_and_consumer_applies_each_update() {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "oa_h5_recorded_synthetic_{}_{}.oaf",
        std::process::id(),
        id
    ));
    let sfdps_path = std::env::temp_dir().join(format!(
        "oa_h5_recorded_synthetic_sfdps_{}_{}.oaf",
        std::process::id(),
        id
    ));
    let _adsb_fixture = FixtureFile(path.clone());
    let _sfdps_fixture = FixtureFile(sfdps_path.clone());
    write_synthetic_adsb(&path);
    write_synthetic_sfdps(&sfdps_path);
    let input = [
        StreamInput {
            source: RecordingSource::Adsb,
            path,
        },
        StreamInput {
            source: RecordingSource::Sfdps,
            path: sfdps_path,
        },
    ];
    let model_bytes = synthetic_model();
    let worker = Path::new(env!("CARGO_BIN_EXE_regional-m3-recorded-drill"));
    let report = run_recorded_stream(
        &input,
        *b"ZAU1",
        EndpointClass::Terminal,
        &model_bytes,
        300.0,
        RecordedRunOptions {
            worker_executable: worker,
            local_authority_fixture: true,
        },
    )
    .expect("paired synthetic recorded stream completes");

    // `gs_dkt=3,500` is 350 knots and the 21,750 e-7-degree eastward step is about 180 m
    // at this latitude per one-second ADS-B interval.
    let latitude_rad = 41.9_f64.to_radians();
    let measured_step_m =
        21_750.0 / 10_000_000.0 * std::f64::consts::PI / 180.0 * 6_371_000.0 * latitude_rad.cos();
    let expected_step_m = 350.0 * 1_852.0 / 3_600.0;
    assert!((measured_step_m - expected_step_m).abs() < 1.0);
    let m2_refusals = report
        .cycles
        .iter()
        .filter_map(|cycle| {
            cycle
                .m2_ingest_refusal
                .as_deref()
                .map(|refusal| (cycle.index, refusal))
        })
        .collect::<Vec<_>>();
    assert_eq!(report.cycles.len(), 100);
    assert_eq!(report.m2_attempt_count, 200);
    assert_eq!(
        report.m2_refusal_count, 0,
        "M2 refusal details: {m2_refusals:?}"
    );
    assert_eq!(report.flip_rollback_attempt_count, 100);
    assert_eq!(report.flip_rollback_complete_count, 100);
    assert!(report
        .cycles
        .iter()
        .all(|cycle| cycle.flip_rollback_attempted && cycle.consumer_frame_updates > 0));
    assert!(report.per_update_consumer.updates > 100);
    assert!(report.flight_input_count > 0);
    assert!(report.flight_input_count < 500);
    assert!(report.cycles.iter().any(|cycle| cycle.flight_inputs > 0));
    assert!(report.per_update_consumer.virtual_gap_samples > 0);
    assert_eq!(report.per_update_consumer.virtual_gap_exceeded, 0);
    assert_eq!(
        report.per_update_consumer.decision_to_first_output_samples,
        100
    );
    assert_eq!(
        report.per_update_consumer.decision_to_first_output_exceeded,
        0
    );
    assert!(
        report.replay_output_ack_count > 0,
        "selected-state ACK count={}, per-cycle row counts={:?}, display rows={:?}, positions={}, flights={}",
        report.replay_output_ack_count,
        report
            .cycles
            .iter()
            .map(|cycle| cycle.selected_state_applied_family_counts)
            .collect::<Vec<_>>(),
        report
            .cycles
            .iter()
            .map(|cycle| cycle.selected_display_block_rows)
            .collect::<Vec<_>>(),
        report.position_input_count,
        report.flight_input_count
    );
    assert!(report.cycles.iter().any(|cycle| {
        cycle.selected_display_block_rows.unwrap_or(0) > 0
            && cycle.selected_state_applied_sha256.is_some()
    }));
    assert_eq!(report.invalid_source_frame_count, None);
    assert_eq!(report.truncated_source_frame_count, None);
    assert_eq!(report.skipped_source_bytes, None);
    assert_eq!(report.source_files_refused, None);
    assert_eq!(report.stream_counts.reject_frames, 0);
    assert!(report.warmup_event_count > 0);
    assert_eq!(report.input_streams.len(), 2);
    assert!(report.stream_counts.frames >= 2_000);
    assert_eq!(report.handoff_operation_count, 0);
    assert_eq!(
        report.source_state_families_available,
        [true, true, false, false]
    );
    assert_eq!(report.window_start_ns, H5_MEASUREMENT_START_NS);
    assert_eq!(report.window_end_ns, H5_MEASUREMENT_END_NS);
    assert_eq!(report.capture_start_ns, H5_CAPTURE_START_NS);
    assert_eq!(
        report.qualification,
        "LOCAL_FIXTURE_M2_AND_PER_UPDATE_MECHANISM_ONLY"
    );
    assert_eq!(
        report.authority_mode,
        "LOCAL_SYNTHETIC_M1_AND_EMPTY_CONTROL_INPUT_FIXTURE_NOT_AUTHENTICATED"
    );
}

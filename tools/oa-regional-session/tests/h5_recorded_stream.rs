#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

// REQ: OA-MIG-414
use oa_airspace::core::{model, snapshot};
use oa_migration_switch::EndpointClass;
use oa_regional_session::migration_drill::{
    run_recorded_stream, RecordedRunOptions, H5_CAPTURE_START_NS, H5_MEASUREMENT_END_NS,
    H5_MEASUREMENT_START_NS,
};
use oa_regional_session::recordings_reader::RecordingSource;
use oa_regional_session::recordings_stream::StreamInput;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
const SYNTHETIC_STEP_NS: i64 = 4_000_000_000;

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
        let row = oa_adsb::Row {
            time_ns: receive_ns,
            icao: 0x00ab_cdef,
            lat_e7: 419_000_000,
            lon_e7: -873_000_000,
            baro_alt_ft: 35_000,
            gs_dkt: 4_500,
            track_ddeg: 900,
            nic: 8,
            nac_p: 9,
            nac_v: 2,
            sil: 3,
            version: 2,
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

#[test]
fn recorded_stream_runs_100_paired_m2_flip_rollback_bins_and_consumer_applies_each_update() {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "oa_h5_recorded_synthetic_{}_{}.oaf",
        std::process::id(),
        id
    ));
    let _fixture = FixtureFile(path.clone());
    write_synthetic_adsb(&path);
    let input = [StreamInput {
        source: RecordingSource::Adsb,
        path,
    }];
    let model_bytes = synthetic_model();
    let worker = Path::new(env!("CARGO_BIN_EXE_regional-m3-recorded-drill"));
    let report = run_recorded_stream(
        &input,
        *b"ZAU1",
        EndpointClass::Terminal,
        &model_bytes,
        100.0,
        RecordedRunOptions {
            worker_executable: worker,
            local_authority_fixture: true,
        },
    )
    .expect("paired synthetic recorded stream completes");

    assert_eq!(report.cycles.len(), 100);
    assert_eq!(report.m2_attempt_count, 200);
    assert_eq!(report.m2_refusal_count, 0);
    assert_eq!(report.flip_rollback_attempt_count, 100);
    assert_eq!(report.flip_rollback_complete_count, 100);
    assert!(report
        .cycles
        .iter()
        .all(|cycle| cycle.flip_rollback_attempted && cycle.consumer_frame_updates > 0));
    assert!(report.per_update_consumer.updates > 100);
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
    assert!(report.replay_output_ack_count > 0);
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

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use oa_regional_session::recordings_cli::convert_recordings_window;
use oa_regional_session::recordings_reader::{RecordingInput, RecordingSource};
use oa_regional_session::runtime::{regional_new, regional_pixels, regional_step};
use oa_regional_session::session::{
    flush_receipt_group, push_event, session_cursor_new, ReceiptBatch,
};

const OBSERVATIONS: usize = 5;
const SECOND_NS: i64 = 1_000_000_000;
const ADDRESS: u32 = 0x00ab_cdef;
const SOURCE_LATITUDE_E7: i32 = 419_000_000;

fn scratch_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("oa-adsb-runtime-{}-{nonce}", std::process::id()))
}

fn valid_airborne_row(time_ns: i64, longitude_e7: i32) -> oa_adsb::Row {
    oa_adsb::Row {
        time_ns,
        icao: ADDRESS,
        lat_e7: SOURCE_LATITUDE_E7,
        lon_e7: longitude_e7,
        baro_alt_ft: 35_000,
        gs_dkt: 4_500,
        track_ddeg: 900,
        vrate_fpm: 0,
        flags: 0,
        nic: 8,
        nac_p: 9,
        nac_v: 2,
        sil: 3,
        version: 2,
        source: 0,
        ..oa_adsb::ROW_EMPTY
    }
}

fn append_adsb_frame(path: &Path, sequence: u64, row: oa_adsb::Row) {
    // OAF1 ADS-B history uses the first report time because no receive wall clock exists.
    let first_report_ns = row.time_ns;
    let mut reports = oa_adsb::Reports::default();
    oa_adsb::reports_push(&mut reports, &row);
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &reports, 0, 1).expect("encode synthetic ADS-B row");

    let mut frame = vec![0; oa_feed::HEADER_LEN + payload.len() + oa_feed::TRAILER_LEN];
    let frame_len = oa_feed::encode_frame(
        &mut frame,
        oa_feed::Kind::Message as u8,
        oa_feed::FLAG_BYTES,
        sequence,
        first_report_ns,
        first_report_ns / 1_000_000,
        &payload,
    )
    .expect("encode synthetic OAF message");
    frame.truncate(frame_len);
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open synthetic OAF input")
        .write_all(&frame)
        .expect("append synthetic OAF message");
}

fn write_sfdps_status(path: &Path, timestamp_ns: i64) {
    let mut frame = vec![0; oa_feed::HEADER_LEN + oa_feed::TRAILER_LEN + 16];
    let frame_len = oa_feed::encode_frame(
        &mut frame,
        oa_feed::Kind::Status as u8,
        0,
        0,
        timestamp_ns,
        timestamp_ns / 1_000_000,
        b"synthetic status",
    )
    .expect("encode synthetic SFDPS status frame");
    frame.truncate(frame_len);
    fs::write(path, frame).expect("write synthetic SFDPS OAF stream");
}

fn synthetic_runtime() -> oa_regional_session::runtime::RegionalRuntime {
    let model = oa_regional_session::cli::synthetic_model().expect("public synthetic model");
    let encoded = oa_airspace::core::snapshot::encode(&model).expect("encode synthetic model");
    regional_new(
        model,
        oa_sha256::sha256(&encoded),
        *b"ZAU1",
        oa_regional_session::surveillance::SurveillanceConfig {
            centre_lat_e7: 419_000_000,
            centre_lon_e7: -883_000_000,
            range_nm: 180.0,
        },
    )
    .expect("construct synthetic regional runtime")
}

fn apply_normalized_tsv(
    runtime: &mut oa_regional_session::runtime::RegionalRuntime,
    normalized: &str,
) -> Vec<oa_regional_session::runtime::RegionalFrame> {
    let mut cursor = session_cursor_new();
    let mut frames = Vec::new();
    for line in normalized.lines().skip(1) {
        let event = oa_regional_session::parse_event_line(line).expect("reparse normalized row");
        assert_eq!(event.source_ns % SECOND_NS, 0);
        // Reports are on exact-second boundaries, so the conservative ceil cutoff is unchanged.
        assert_eq!(event.receipt_ns, event.source_ns);
        assert_eq!(event.source, oa_regional_session::SourceKind::Surveillance);
        if let Some(batch) = push_event(&mut cursor, event).expect("stage normalized row") {
            frames.push(regional_step(runtime, &batch).expect("execute regional receipt"));
        }
    }
    if let Some(batch) = flush_receipt_group(&mut cursor).expect("flush final receipt") {
        frames.push(regional_step(runtime, &batch).expect("execute final receipt"));
    }
    frames
}

// REQ: OA-REG-013
#[test]
fn recorded_hour_adsb_adapter_roundtrip_reaches_regional_raster() {
    let scratch = scratch_dir();
    fs::create_dir(&scratch).expect("create synthetic scratch directory");
    let adsb_path = scratch.join("synthetic-adsb.oaf");
    let sfdps_path = scratch.join("synthetic-sfdps.oaf");
    let base_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:04.000Z").expect("parse epoch fixture");
    let base_ns = base_ms.checked_mul(1_000_000).expect("convert epoch to ns");
    write_sfdps_status(&sfdps_path, base_ns);

    let mut source_longitudes = Vec::with_capacity(OBSERVATIONS);
    for index in 0..OBSERVATIONS {
        let step = i64::try_from(index).expect("small fixture index") * SECOND_NS;
        let time_ns = base_ns + step;
        let longitude_e7 = -883_000_000 + i32::try_from(index).expect("small index") * 27_900;
        source_longitudes.push(longitude_e7);
        append_adsb_frame(
            &adsb_path,
            u64::try_from(index).expect("small OAF sequence"),
            valid_airborne_row(time_ns, longitude_e7),
        );
    }

    let window_end = base_ns + 3_600 * SECOND_NS;
    let converted = convert_recordings_window(
        &[
            RecordingInput {
                source: RecordingSource::Adsb,
                path: adsb_path,
            },
            RecordingInput {
                source: RecordingSource::Sfdps,
                path: sfdps_path,
            },
        ],
        *b"ZAU1",
        (base_ns, window_end),
    )
    .expect("adapt the synthetic recorded-hour ADS-B stream");
    assert_eq!(converted.window_ns, (base_ns, window_end));
    assert_eq!(converted.summary.normalized_events, OBSERVATIONS as u64);

    let mut runtime = synthetic_runtime();
    let frames = apply_normalized_tsv(&mut runtime, &converted.normalized_tsv);
    assert_eq!(frames.len(), OBSERVATIONS);
    let last = frames.last().expect("normalized timeline reached runtime");
    assert!(frames.iter().all(|frame| frame.input.source_events[0] == 1));
    assert!(last.surveillance.displayed > 0);
    // Kills replacing each report longitude with one constant in-view position.
    let last_track = last
        .surveillance
        .tracks
        .iter()
        .find(|track| track.address == ADDRESS)
        .expect("confirmed synthetic track is present");
    let expected_last_longitude = source_longitudes
        .last()
        .copied()
        .expect("synthetic reports include a final source position");
    assert_eq!(last_track.lon_e7, expected_last_longitude);
    assert_eq!(last_track.lat_e7, SOURCE_LATITUDE_E7);
    let first_confirmed = frames
        .iter()
        .skip(1)
        .find_map(|frame| {
            frame
                .surveillance
                .tracks
                .iter()
                .find(|track| track.address == ADDRESS)
        })
        .expect("confirmation begins after the initial report");
    let expected_first_confirmed_longitude = source_longitudes
        .get(1)
        .copied()
        .expect("synthetic reports include a confirmation position");
    assert_eq!(first_confirmed.lon_e7, expected_first_confirmed_longitude);
    assert_ne!(first_confirmed.lon_e7, last_track.lon_e7);

    let rendered = regional_pixels(&runtime).expect("regional pixels are available");
    assert_eq!(rendered.len(), 800 * 600 * 4);
    let target_colour = oa_display::core::palette::bytes_of(oa_display::core::palette::TARGET)
        .expect("target palette entry exists");
    assert!(rendered
        .chunks_exact(4)
        .any(|pixel| pixel == target_colour.as_slice()));

    let mut blank = synthetic_runtime();
    regional_step(
        &mut blank,
        &ReceiptBatch {
            receipt_ns: last.input.receipt_ns,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .expect("render empty comparison frame at the same recorded cutoff");
    assert_ne!(
        rendered,
        regional_pixels(&blank).expect("comparison pixels are available")
    );

    let _ = fs::remove_dir_all(scratch);
}

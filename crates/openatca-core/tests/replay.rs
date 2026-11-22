#![allow(clippy::unwrap_used)]

use openatca_core::{ReplayError, ReplaySettings, SurveillanceReplay};

const CLIP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/synthetic-adsb.oaf"
);
const CLIP_BYTES: &[u8] = include_bytes!("fixtures/synthetic-adsb.oaf");
const START_NS: i64 = 1_750_000_000_000_000_000;
const SECOND_NS: i64 = 1_000_000_000;
const FIRST_AIRCRAFT: u32 = 0xA00001;
const SECOND_AIRCRAFT: u32 = 0xA00002;

fn write_clip(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

// REQ: OA-CORE-001
#[test]
fn synthetic_dropout_is_visible_and_repeats() {
    let mut replay = SurveillanceReplay::open(CLIP, ReplaySettings::default()).unwrap();
    let mut repeated = SurveillanceReplay::open(CLIP, ReplaySettings::default()).unwrap();
    let input = replay.input_health();
    assert_eq!(input.damaged_or_undecodable_frames, 0);
    assert_eq!(input.records_dropped, 0);
    assert_eq!(input.quality_rejected, 0);
    assert_eq!(input.sealed_window_dropped, 0);
    assert_eq!(input.measurements, 50);
    let mut saw_coast = false;
    let mut saw_recovery = false;
    let mut saw_second_aircraft = false;

    for second in 0..30 {
        let frame = replay.next_frame().unwrap();
        let again = repeated.next_frame().unwrap();
        assert_eq!(frame.time_ns, START_NS + second * SECOND_NS);
        assert_eq!(frame.time_ns, again.time_ns);
        assert_eq!(frame.health.rows_lost, 0);
        let tracks: Vec<_> = frame.tracks().collect();
        assert_eq!(tracks, again.tracks().collect::<Vec<_>>());
        for track in tracks {
            assert!(track.time_ns <= frame.time_ns);
            if track.address == FIRST_AIRCRAFT && track.coasting {
                assert!(
                    (START_NS + 10 * SECOND_NS..START_NS + 20 * SECOND_NS).contains(&track.time_ns)
                );
                saw_coast = true;
            }
            saw_recovery |= track.address == FIRST_AIRCRAFT
                && track.time_ns >= START_NS + 20 * SECOND_NS
                && !track.coasting;
            saw_second_aircraft |= track.address == SECOND_AIRCRAFT;
        }
    }
    assert!(saw_coast && saw_recovery && saw_second_aircraft);
    assert!(replay.next_frame().is_none());
    assert!(replay.next_frame().is_none());
    assert!(repeated.next_frame().is_none());
}

// REQ: OA-CORE-002
#[test]
fn clip_byte_and_row_limits_are_enforced() {
    let byte_limit = ReplaySettings {
        max_clip_bytes: CLIP_BYTES.len() as u64 - 1,
        ..ReplaySettings::default()
    };
    let row_limit = ReplaySettings {
        max_clip_rows: 49,
        ..ReplaySettings::default()
    };
    assert!(matches!(
        SurveillanceReplay::open(CLIP, byte_limit),
        Err(ReplayError::ClipTooLarge { bytes, limit })
            if bytes == CLIP_BYTES.len() as u64 && limit == byte_limit.max_clip_bytes
    ));
    assert!(matches!(
        SurveillanceReplay::open(CLIP, row_limit),
        Err(ReplayError::TooManyReports {
            rows: 50,
            limit: 49
        })
    ));
}

// REQ: OA-CORE-002
#[test]
fn a_truncated_last_frame_is_counted() {
    let bytes = CLIP_BYTES.get(..CLIP_BYTES.len() - 1).unwrap();
    let path = write_clip("openatca-core-truncated-adsb.oaf", bytes);
    let mut replay =
        SurveillanceReplay::open(path.to_str().unwrap(), ReplaySettings::default()).unwrap();
    assert_eq!(replay.input_health().damaged_or_undecodable_frames, 1);
    let mut steps = 0;
    while replay.next_frame().is_some() {
        steps += 1;
    }
    assert_eq!(steps, 29);
}

// REQ: OA-CORE-003
#[test]
fn dropped_output_is_counted_across_frames() {
    let settings = ReplaySettings {
        output_capacity: 1,
        ..ReplaySettings::default()
    };
    let mut replay = SurveillanceReplay::open(CLIP, settings).unwrap();
    let mut lost = 0;
    while let Some(frame) = replay.next_frame() {
        assert!(frame.tracks().count() <= settings.output_capacity);
        assert!(frame.health.rows_lost >= lost);
        lost = frame.health.rows_lost;
    }
    assert!(lost > 0);
}

// REQ: OA-CORE-004
#[test]
fn replay_uses_the_recordings_report_interval() {
    // The first six frames all contain two ADS3 reports. Keep seconds zero and five.
    const FRAME_BYTES: usize = 36 + 16 + 2 * 80 + 4;
    let mut bytes = CLIP_BYTES.get(..FRAME_BYTES).unwrap().to_vec();
    bytes.extend_from_slice(CLIP_BYTES.get(5 * FRAME_BYTES..6 * FRAME_BYTES).unwrap());
    let path = write_clip("openatca-core-slow-adsb.oaf", &bytes);
    let mut replay =
        SurveillanceReplay::open(path.to_str().unwrap(), ReplaySettings::default()).unwrap();
    assert_eq!(replay.next_frame().unwrap().health.confirmed_tracks, 0);
    let last = replay.next_frame().unwrap();
    assert_eq!(last.time_ns, START_NS + 5 * SECOND_NS);
    assert_eq!(last.health.confirmed_tracks, 2);
    assert!(replay.next_frame().is_none());
}

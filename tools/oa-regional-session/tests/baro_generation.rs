#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_regional_session::cli::synthetic_model;
use oa_regional_session::runtime::{
    regional_enable_synthetic_msaw, regional_new, regional_step, RegionalRuntime,
};
use oa_regional_session::surveillance::SurveillanceConfig;
use oa_regional_session::{EventKind, Identity, NormalizedEvent, ReceiptBatch, SourceKind};

const SECOND: i64 = 1_000_000_000;
const ADDRESS: u32 = 0xabc101;

fn runtime() -> RegionalRuntime {
    let model = synthetic_model().expect("synthetic airspace model");
    let snapshot = oa_airspace::core::snapshot::encode(&model).expect("encode synthetic model");
    let mut runtime = regional_new(
        model,
        oa_sha256::sha256(&snapshot),
        *b"ZAU1",
        SurveillanceConfig {
            centre_lat_e7: 410_000_000,
            centre_lon_e7: -870_000_000,
            range_nm: 180.0,
        },
    )
    .expect("regional runtime");
    regional_enable_synthetic_msaw(&mut runtime).expect("synthetic MSAW service");
    runtime
}

fn batch(receipt_s: i64, source_times_s: &[i64], altitudes_ft: &[Option<i32>]) -> ReceiptBatch {
    assert_eq!(source_times_s.len(), altitudes_ft.len());
    let events = source_times_s
        .iter()
        .zip(altitudes_ft)
        .enumerate()
        .map(|(index, (&source_s, &altitude_ft))| NormalizedEvent {
            receipt_ns: receipt_s * SECOND,
            source_ns: source_s * SECOND,
            sequence: u64::try_from(index + 1).expect("bounded fixture sequence"),
            source_message_id: None,
            source: SourceKind::Surveillance,
            identity: Identity {
                flight_gufi: 0,
                aircraft_address: ADDRESS,
            },
            centre: *b"ZAU1",
            kind: EventKind::Position {
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
                altitude_ft,
                east_mmps: 0,
                north_mmps: 0,
                vertical_rate_fpm: Some(0),
                track_callsign: None,
            },
        })
        .collect();
    let source_clock_regressions = source_times_s
        .windows(2)
        .filter(|pair| pair[1] < pair[0])
        .count() as u64;
    ReceiptBatch {
        receipt_ns: receipt_s * SECOND,
        source_clock_regressions,
        events,
    }
}

// REQ: OA-REG-011
#[test]
fn clock_stepback_new_track_does_not_inherit_previous_baro_epoch() {
    let mut runtime = runtime();

    // The first generation learns barometric altitude at 200s, confirms at 201s, then
    // continues horizontally through 210s. Receipt time advances after each batch.
    regional_step(&mut runtime, &batch(200, &[200], &[Some(1_500)])).unwrap();
    regional_step(&mut runtime, &batch(201, &[201], &[None])).unwrap();
    regional_step(&mut runtime, &batch(210, &[210], &[None])).unwrap();

    // Six ascending late reports span the 10s out-of-order window. The sixth makes tracker_update
    // step its clock back to 191s, retire the old track whose latest state was 210s, and start a
    // fresh generation for this address from its 191s barometric report. The 192s row confirms
    // that new track. A final altitude-absent report at 222s keeps the aggregate surveillance
    // feed live at the 223s issue cutoff without refreshing the 191s barometric evidence. Keeping
    // the reports in one receipt avoids advancing the tracker clock between the back-clock reports.
    let frame = regional_step(
        &mut runtime,
        &batch(
            223,
            &[181, 183, 185, 187, 189, 191, 192, 222],
            &[None, None, None, None, None, Some(1_500), None, None],
        ),
    )
    .unwrap();

    assert!(frame.surveillance.tracker_stats.time_steps_back > 0);
    assert_eq!(
        frame.surveillance.tracks.len(),
        1,
        "tracker stats at issue cutoff: {:?}",
        frame.surveillance.tracker_stats
    );
    assert_eq!(frame.surveillance.tracks[0].address, ADDRESS);
    let msaw = frame.surveillance.msaw.as_ref().expect("MSAW evidence");
    let stats = msaw.stats.expect("MSAW cycle stats");
    // At the 223s issue cutoff, the new epoch's 191s barometric sample is 32s old, beyond
    // synthetic stale_hold_s=30. Reusing the prior 200s map value would report age 23s and
    // incorrectly process this altitude as extrapolatable.
    assert!(stats.alt_stale > 0, "{stats:?}");
    assert_eq!(stats.alt_extrapolated, 0, "{stats:?}");
}

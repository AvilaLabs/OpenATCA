#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_display::core::traffic::MAX_TARGET_SYMBOL_RADIUS_PX;
use oa_display::palette::{bytes_of, TARGET};
use oa_display::project::{projection_new, unproject};
use oa_regional_session::cli::synthetic_model;
use oa_regional_session::surveillance::{
    surveillance_begin, surveillance_event, surveillance_finish, surveillance_new,
    surveillance_pixels, SurveillanceConfig, FRAME_HEIGHT, FRAME_WIDTH,
};
use oa_regional_session::{EventKind, Identity, NormalizedEvent, SourceKind};

const FIRST_FRAME_NS: i64 = 1_000_000_000;
const SECOND_FRAME_NS: i64 = 2_000_000_000;
const CENTRE_LAT_E7: i32 = 410_000_000;
const CENTRE_LON_E7: i32 = -870_000_000;
const RANGE_NM: f64 = 180.0;
const TOP_INSET_PX: u32 = 224;
const EDGE_SCREENS: [(i32, i32); 4] = [(20, 380), (780, 380), (400, 225), (400, 584)];

fn surveillance() -> oa_regional_session::surveillance::Surveillance {
    surveillance_new(
        synthetic_model().expect("synthetic model builds"),
        SurveillanceConfig {
            centre_lat_e7: CENTRE_LAT_E7,
            centre_lon_e7: CENTRE_LON_E7,
            range_nm: RANGE_NM,
        },
    )
    .expect("synthetic regional surveillance initializes")
}

fn position(
    receipt_ns: i64,
    source_ns: i64,
    sequence: u64,
    address: u32,
    latitude_e7: i32,
    longitude_e7: i32,
) -> NormalizedEvent {
    NormalizedEvent {
        receipt_ns,
        input_receipt_ns: receipt_ns,
        source_ns,
        sequence,
        source_message_id: None,
        source: SourceKind::Surveillance,
        identity: Identity {
            flight_gufi: u128::from(address),
            aircraft_address: address,
        },
        centre: *b"TEST",
        kind: EventKind::Position {
            latitude_e7,
            longitude_e7,
            altitude_ft: Some(20_000),
            east_mmps: 10_000,
            north_mmps: 0,
            vertical_rate_fpm: Some(0),
            track_callsign: None,
        },
    }
}

fn feed_fault(receipt_ns: i64, sequence: u64, source: SourceKind) -> NormalizedEvent {
    NormalizedEvent {
        receipt_ns,
        input_receipt_ns: receipt_ns,
        source_ns: receipt_ns,
        sequence,
        source_message_id: None,
        source,
        identity: Identity {
            flight_gufi: 0,
            aircraft_address: 0,
        },
        centre: *b"TEST",
        kind: EventKind::FeedState {
            available: false,
            reason: 1,
        },
    }
}

fn has_target_pixel(pixels: &[u8], screen: (i32, i32)) -> bool {
    let Some(target) = bytes_of(TARGET) else {
        return false;
    };
    let stride = usize::try_from(FRAME_WIDTH).unwrap_or(0) * 4;
    for y in (screen.1 - MAX_TARGET_SYMBOL_RADIUS_PX)..=(screen.1 + MAX_TARGET_SYMBOL_RADIUS_PX) {
        for x in (screen.0 - MAX_TARGET_SYMBOL_RADIUS_PX)..=(screen.0 + MAX_TARGET_SYMBOL_RADIUS_PX)
        {
            let (Ok(y), Ok(x)) = (usize::try_from(y), usize::try_from(x)) else {
                continue;
            };
            let offset = y * stride + x * 4;
            if pixels.get(offset..offset + 4) == Some(target.as_slice()) {
                return true;
            }
        }
    }
    false
}

#[test]
fn regional_reserved_status_strip_reports_faults_without_covering_edge_targets() {
    let projection = projection_new(
        CENTRE_LAT_E7,
        CENTRE_LON_E7,
        RANGE_NM,
        FRAME_WIDTH,
        FRAME_HEIGHT,
        TOP_INSET_PX,
    )
    .expect("synthetic top-inset projection is valid");
    // The fifth source lies in the reserved header and is correctly off-map, not hidden traffic.
    let mut source_screens = EDGE_SCREENS.to_vec();
    source_screens.push((400, 175));
    let positions: Vec<_> = source_screens
        .iter()
        .map(|&(x, y)| unproject(&projection, x * 16, y * 16).expect("edge point is invertible"))
        .collect();
    let mut session = surveillance();

    surveillance_begin(&mut session, FIRST_FRAME_NS).expect("first scan begins");
    for (index, &(lat, lon)) in positions.iter().enumerate() {
        let address = 0x00ab_c001 + u32::try_from(index).unwrap_or(0);
        let event = position(
            FIRST_FRAME_NS,
            FIRST_FRAME_NS,
            u64::try_from(index + 1).unwrap_or(1),
            address,
            lat,
            lon,
        );
        surveillance_event(&mut session, &event).expect("first edge report is admitted");
    }
    surveillance_finish(&mut session, None).expect("first scan finishes");

    surveillance_begin(&mut session, SECOND_FRAME_NS).expect("second scan begins");
    for (index, &(lat, lon)) in positions.iter().enumerate() {
        let address = 0x00ab_c001 + u32::try_from(index).unwrap_or(0);
        let event = position(
            SECOND_FRAME_NS,
            SECOND_FRAME_NS,
            u64::try_from(index + 1).unwrap_or(1),
            address,
            lat,
            lon,
        );
        surveillance_event(&mut session, &event).expect("second edge report is admitted");
    }
    for (index, source) in [
        SourceKind::Surveillance,
        SourceKind::Flight,
        SourceKind::Route,
        SourceKind::Weather,
        SourceKind::Adaptation,
    ]
    .into_iter()
    .enumerate()
    {
        surveillance_event(
            &mut session,
            &feed_fault(
                SECOND_FRAME_NS,
                10 + u64::try_from(index).unwrap_or(0),
                source,
            ),
        )
        .expect("feed fault is admitted as synthetic health input");
    }
    let frame = surveillance_finish(&mut session, None).expect("second scan finishes");

    let stats = frame
        .safety_stats
        .expect("rendered regional frame retains typed safety accounting");
    assert_eq!(
        usize::try_from(stats.nets_shown).unwrap_or(0),
        3,
        "CA has just run; the other three unconfigured nets need a banner"
    );
    assert_eq!(usize::try_from(stats.feeds_shown).unwrap_or(0), 5);
    assert!(
        stats.banner_hidden > 0,
        "compact overflow summary remains visible"
    );
    assert_eq!(stats.targets_under_banner, 0);
    assert_eq!(
        usize::try_from(frame.displayed).unwrap_or(0),
        EDGE_SCREENS.len()
    );
    assert_eq!(
        usize::try_from(frame.off_map).unwrap_or(usize::MAX),
        source_screens.len() - EDGE_SCREENS.len(),
        "the header-positioned fifth report is off-map"
    );

    let pixels = surveillance_pixels(&session).expect("completed frame has pixels");
    for edge in EDGE_SCREENS {
        assert!(
            has_target_pixel(pixels, edge),
            "status strip obscured target at independent screen coordinate {edge:?}"
        );
    }
}

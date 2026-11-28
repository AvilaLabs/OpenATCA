#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_display::core::font::{text_width_units, ADVANCE, CAP_HEIGHT};
use oa_display::list::{list_new, push_rect, push_text, ListCaps, ALIGN_LEFT};
use oa_display::palette::{ALERT, BACKGROUND};
use oa_display::project::{projection_new, unproject, SUBPIXEL};
use oa_display::raster::{frame_len, rasterize, FrameSpec};
use oa_regional_session::cli::synthetic_model;
use oa_regional_session::runtime::{
    regional_new, regional_new_with_runtime_final_monitor, regional_pixels, regional_step,
    FinalApproachAssignment, FinalMonitorDegradation, FinalMonitorStatus, RegionalFrame,
    RuntimeFinalMonitorConfig, DEFAULT_REGIONAL_RUN_PROFILE,
};
use oa_regional_session::surveillance::{SurveillanceConfig, FRAME_HEIGHT, FRAME_WIDTH};
use oa_regional_session::{parse_event_line, ReceiptBatch};
use oa_runway::final_monitor::{FinalPair, FinalPairId, FinalSeverity};
use oa_runway::plane::{Geo, Local};

const CENTRE_LAT_E7: i32 = 410_000_000;
const CENTRE_LON_E7: i32 = -870_000_000;
const RANGE_NM: f64 = 180.0;
const TOP_INSET_PX: u32 = 224;
const EDGE_MARGIN_PX: i32 = 8;
const TARGET_NEIGHBORHOOD_RADIUS_PX: i32 = 24;
const PIXEL_BYTES: usize = 4;
const TARGET_ADDRESS: u32 = 0x000abc;
const TARGET_GUFI: u128 = 1;
const FIRST_RUNWAY_ID: u32 = 27_001;
const SECOND_RUNWAY_ID: u32 = 27_002;
// Keep edge rendering fixtures on the unconfigured tracker's global whole-second tick grid.
// Mutation killed: fractional-second scan setup makes the unconfigured baseline refuse its
// tracker output before the configured-vs-plain pixel comparison can be exercised.
const FIRST_SCAN_NS: i64 = 3_000_000_000;
const LAST_SCAN_NS: i64 = 6_000_000_000;
// The configured 4.8-second input interval is exact; source age is independently 4.799 seconds.
const NEAR_LIMIT_RECEIPT_NS: i64 = 10_800_000_000;
const NEAR_LIMIT_SOURCE_NS: i64 = 6_001_000_000;
const STALE_RECEIPT_NS: i64 = 16_000_000_000;
const STALE_SOURCE_NS: i64 = 10_999_000_000;
const HEADER_X_PX: i32 = 8;
const HEADER_Y_PX: i32 = 135;
const HEADER_TEXT_SIZE: u8 = 10;
const HEADER_PANEL_TOP_PX: i32 = 104;
const HEADER_PANEL_BOTTOM_PX: i32 = 224;
const DEGRADED_HEADER_PREFIX: &str = "FINAL DEGRADED WARNING 0 CAUTION 0 ";
const PROTECTION_LOST_PHRASE: &str = "PROTECTION LOST";

#[derive(Clone, Copy)]
struct EdgeTarget {
    name: &'static str,
    x_px: i32,
    y_px: i32,
}

fn display_config() -> SurveillanceConfig {
    SurveillanceConfig {
        centre_lat_e7: CENTRE_LAT_E7,
        centre_lon_e7: CENTRE_LON_E7,
        range_nm: RANGE_NM,
    }
}

fn projection() -> oa_display::project::Projection {
    projection_new(
        CENTRE_LAT_E7,
        CENTRE_LON_E7,
        RANGE_NM,
        FRAME_WIDTH,
        FRAME_HEIGHT,
        TOP_INSET_PX,
    )
    .unwrap()
}

fn edge_targets() -> [EdgeTarget; 4] {
    let width = i32::try_from(FRAME_WIDTH).unwrap();
    let height = i32::try_from(FRAME_HEIGHT).unwrap();
    let map_top = i32::try_from(TOP_INSET_PX).unwrap();
    let map_mid_y = (map_top + height) / 2;
    let middle_x = width / 2;
    [
        EdgeTarget {
            name: "left",
            x_px: EDGE_MARGIN_PX,
            y_px: map_mid_y,
        },
        EdgeTarget {
            name: "right",
            x_px: width - EDGE_MARGIN_PX,
            y_px: map_mid_y,
        },
        EdgeTarget {
            name: "top map edge",
            x_px: middle_x,
            y_px: map_top + EDGE_MARGIN_PX,
        },
        EdgeTarget {
            name: "bottom",
            x_px: middle_x,
            y_px: height - EDGE_MARGIN_PX,
        },
    ]
}

fn edge_target_position(edge: EdgeTarget) -> (i32, i32) {
    unproject(&projection(), edge.x_px * SUBPIXEL, edge.y_px * SUBPIXEL).unwrap()
}

fn final_monitor_config(position: (i32, i32)) -> RuntimeFinalMonitorConfig {
    RuntimeFinalMonitorConfig {
        pair: FinalPair {
            id: FinalPairId {
                first_runway_id: FIRST_RUNWAY_ID,
                first_end_name: *b"27R",
                second_runway_id: SECOND_RUNWAY_ID,
                second_end_name: *b"27L",
            },
            projection_origin: Some(Geo {
                lat_deg: f64::from(position.0) / 10_000_000.0,
                lon_deg: f64::from(position.1) / 10_000_000.0,
            }),
            origin: Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            axis_east: 1.0,
            axis_north: 0.0,
            normal_east: 0.0,
            normal_north: 1.0,
            course_length_m: 10_000.0,
            ntz_half_width_m: 10.0,
            max_r95_m: 100.0,
            lookahead_ns: 60_000_000_000,
            update_ns: 4_800_000_000,
        },
        assignments: vec![FinalApproachAssignment {
            flight: oa_flight::key_gufi(TARGET_GUFI),
            runway_id: FIRST_RUNWAY_ID,
            end_name: *b"27R",
        }],
    }
}

fn plain_runtime() -> oa_regional_session::runtime::RegionalRuntime {
    let model = synthetic_model().unwrap();
    let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
    regional_new(
        model,
        oa_sha256::sha256(&snapshot),
        *b"ZAU1",
        display_config(),
    )
    .unwrap()
}

fn configured_runtime(position: (i32, i32)) -> oa_regional_session::runtime::RegionalRuntime {
    let model = synthetic_model().unwrap();
    let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
    regional_new_with_runtime_final_monitor(
        model,
        oa_sha256::sha256(&snapshot),
        *b"ZAU1",
        display_config(),
        DEFAULT_REGIONAL_RUN_PROFILE,
        final_monitor_config(position),
    )
    .unwrap()
}

fn warning_batch(
    receipt_ns: i64,
    source_ns: i64,
    position: (i32, i32),
    include_flight_setup: bool,
) -> ReceiptBatch {
    let mut lines = Vec::with_capacity(if include_flight_setup { 4 } else { 1 });
    lines.push(format!(
        "v1\t{receipt_ns}\t{source_ns}\t{source_ns}\tSURVEILLANCE\t00000000000000000000000000000000\t000abc\tZAU1\tPOS\t{}\t{}\t10000\t0\t0\t0\tCALLSIGN\tN123AB",
        position.0, position.1,
    ));
    if include_flight_setup {
        lines.push(format!(
            "v1\t{receipt_ns}\t1000000000\t1\tFLIGHT\t{TARGET_GUFI:032x}\t000abc\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2"
        ));
        lines.push(format!(
            "v1\t{receipt_ns}\t2000000000\t2\tFLIGHT\t{TARGET_GUFI:032x}\t000abc\tZAU1\tFLIGHT\tADDRESS"
        ));
        lines.push(format!(
            "v1\t{receipt_ns}\t{receipt_ns}\t3\tFLIGHT\t{TARGET_GUFI:032x}\t000abc\tZAU1\tFLIGHT\tIDENTITY_PARTIAL\tN123AB\t-\t-"
        ));
    }
    ReceiptBatch {
        receipt_ns,
        source_clock_regressions: 0,
        events: lines
            .into_iter()
            .map(|line| parse_event_line(&line).unwrap())
            .collect(),
    }
}

fn run_confirming_scans(
    runtime: &mut oa_regional_session::runtime::RegionalRuntime,
    position: (i32, i32),
) -> RegionalFrame {
    let mut final_frame = None;
    for source_ns in (FIRST_SCAN_NS..=LAST_SCAN_NS).step_by(1_000_000_000_usize) {
        final_frame = Some(
            regional_step(
                runtime,
                &warning_batch(source_ns, source_ns, position, source_ns == FIRST_SCAN_NS),
            )
            .unwrap(),
        );
    }
    final_frame.unwrap()
}

fn assert_confirmed_target_and_flight_link(frame: &RegionalFrame, source_ns: i64) {
    assert!(
        frame.surveillance.tracks.iter().any(|track| {
            track.address == TARGET_ADDRESS && track.newest_source_ns == source_ns
        }),
        "target tracker record missing: tracks={:?} stats={:?}",
        frame.surveillance.tracks,
        frame.surveillance.tracker_stats
    );
    assert!(
        frame
            .flight_data
            .iter()
            .any(|flight| { flight.address == TARGET_ADDRESS && flight.gufi == TARGET_GUFI }),
        "target flight address was not linked: flight_data={:?} operations={:?}",
        frame.flight_data,
        frame.flight_operations
    );
}

fn protection_lost_suffix_pixels() -> (Vec<u8>, (usize, usize, usize, usize)) {
    let spec = FrameSpec {
        width: FRAME_WIDTH,
        height: FRAME_HEIGHT,
        stride: FRAME_WIDTH * u32::try_from(PIXEL_BYTES).unwrap(),
    };
    let mut list = list_new(ListCaps {
        lines: 0,
        rects: 1,
        circles: 0,
        polylines: 0,
        poly_vertices: 0,
        texts: 1,
        text_bytes: PROTECTION_LOST_PHRASE.len(),
    })
    .unwrap();
    push_rect(
        &mut list,
        (0, HEADER_PANEL_TOP_PX * SUBPIXEL),
        (
            i32::try_from(FRAME_WIDTH).unwrap() * SUBPIXEL,
            HEADER_PANEL_BOTTOM_PX * SUBPIXEL,
        ),
        BACKGROUND,
        14,
    )
    .unwrap();

    let advance_16 =
        (i64::from(ADVANCE) * i64::from(HEADER_TEXT_SIZE) * 16) / i64::from(CAP_HEIGHT);
    let prefix_width_16 =
        i64::try_from(DEGRADED_HEADER_PREFIX.chars().count()).unwrap() * advance_16;
    let phrase_x_16 = i64::from(HEADER_X_PX * SUBPIXEL) + prefix_width_16;
    let phrase_x_px = usize::try_from(phrase_x_16.div_euclid(i64::from(SUBPIXEL))).unwrap();
    let phrase_width_16 = i64::from(text_width_units(PROTECTION_LOST_PHRASE.chars().count()))
        * i64::from(HEADER_TEXT_SIZE)
        * i64::from(SUBPIXEL)
        / i64::from(CAP_HEIGHT);
    let phrase_width_px =
        usize::try_from((phrase_width_16 + i64::from(SUBPIXEL) - 1) / i64::from(SUBPIXEL)).unwrap();
    let phrase_y_px = usize::try_from(HEADER_Y_PX).unwrap();
    let phrase_bottom_px = phrase_y_px + usize::from(HEADER_TEXT_SIZE) + 1;

    // Build only the independently positioned suffix glyphs with the existing font primitive;
    // do not call the production header formatter for this expected pixel region.
    push_text(
        &mut list,
        (i32::try_from(phrase_x_16).unwrap(), HEADER_Y_PX * SUBPIXEL),
        HEADER_TEXT_SIZE,
        ALIGN_LEFT,
        ALERT,
        15,
        PROTECTION_LOST_PHRASE,
    )
    .unwrap();
    let mut expected = vec![0; frame_len(&spec)];
    rasterize(&list, &spec, &mut expected).unwrap();
    (
        expected,
        (
            phrase_x_px,
            phrase_x_px + phrase_width_px,
            phrase_y_px,
            phrase_bottom_px,
        ),
    )
}

fn consumer_suffix_matches(
    actual: &[u8],
    expected: &[u8],
    region: (usize, usize, usize, usize),
) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    let stride = usize::try_from(FRAME_WIDTH).unwrap() * PIXEL_BYTES;
    let (x_start, x_end, y_start, y_end) = region;
    (y_start..y_end).all(|y| {
        let start = y * stride + x_start * PIXEL_BYTES;
        let end = y * stride + x_end * PIXEL_BYTES;
        actual.get(start..end) == expected.get(start..end)
    })
}

fn differs_near_target(with_warning: &[u8], without_warning: &[u8], target: (i32, i32)) -> bool {
    if with_warning.len() != without_warning.len() {
        return false;
    }
    let stride = usize::try_from(FRAME_WIDTH).unwrap() * PIXEL_BYTES;
    let map_top = i32::try_from(TOP_INSET_PX).unwrap();
    let width = i32::try_from(FRAME_WIDTH).unwrap();
    let height = i32::try_from(FRAME_HEIGHT).unwrap();
    let x_start = usize::try_from((target.0 - TARGET_NEIGHBORHOOD_RADIUS_PX).max(0)).unwrap();
    let x_end = usize::try_from((target.0 + TARGET_NEIGHBORHOOD_RADIUS_PX).min(width - 1)).unwrap();
    let y_start = usize::try_from((target.1 - TARGET_NEIGHBORHOOD_RADIUS_PX).max(map_top)).unwrap();
    let y_end =
        usize::try_from((target.1 + TARGET_NEIGHBORHOOD_RADIUS_PX).min(height - 1)).unwrap();
    (y_start..=y_end).any(|y| {
        (x_start..=x_end).any(|x| {
            let start = y * stride + x * PIXEL_BYTES;
            let end = start + PIXEL_BYTES;
            with_warning.get(start..end) != without_warning.get(start..end)
        })
    })
}

// Mutation killed: skip final-monitor markers at the regional presentation boundary, or classify
// an on-screen edge warning as OFF when its halo is clipped; its target neighborhood then matches
// the same-input runtime without the configured final monitor.
// REQ: OA-RWY-015, OA-DSP-077
#[test]
fn genuine_final_warning_is_visible_on_each_scope_edge_and_keeps_its_target() {
    for edge in edge_targets() {
        let position = edge_target_position(edge);
        // These independently pinned screen coordinates define the geographic edge fixtures.
        let target_px = (edge.x_px, edge.y_px);
        let mut with_monitor = configured_runtime(position);
        let warning_frame = run_confirming_scans(&mut with_monitor, position);
        assert_confirmed_target_and_flight_link(&warning_frame, LAST_SCAN_NS);
        assert!(
            matches!(
                warning_frame.final_monitor.status,
                FinalMonitorStatus::Available
            ),
            "{edge_name} edge final-monitor status={:?} alerts={:?} tracker={:?}",
            warning_frame.final_monitor.status,
            warning_frame.final_monitor.alerts,
            warning_frame.surveillance.tracker_stats,
            edge_name = edge.name,
        );
        assert!(
            warning_frame.final_monitor.alerts.iter().any(|alert| {
                alert.address == TARGET_ADDRESS && alert.severity == FinalSeverity::Warning
            }),
            "{edge_name} edge did not retain the real warning: status={:?} alerts={:?} tracker={:?}",
            warning_frame.final_monitor.status,
            warning_frame.final_monitor.alerts,
            warning_frame.surveillance.tracker_stats,
            edge_name = edge.name,
        );
        assert!(
            warning_frame.surveillance.displayed > 0
                && warning_frame
                    .surveillance
                    .tracks
                    .iter()
                    .any(|track| track.address == TARGET_ADDRESS),
            "{edge_name} edge warning removed its target: tracks={:?} tracker={:?}",
            warning_frame.surveillance.tracks,
            warning_frame.surveillance.tracker_stats,
            edge_name = edge.name,
        );

        let with_warning_pixels = regional_pixels(&with_monitor).unwrap();
        let mut without_monitor = plain_runtime();
        let baseline_frame = run_confirming_scans(&mut without_monitor, position);
        assert_confirmed_target_and_flight_link(&baseline_frame, LAST_SCAN_NS);
        let baseline_pixels = regional_pixels(&without_monitor).unwrap();
        assert!(
            differs_near_target(with_warning_pixels, baseline_pixels, target_px),
            "{edge_name} edge has no changed alert ink in the target neighborhood",
            edge_name = edge.name
        );
    }
}

// Mutation killed: use the generic regional display title for the degraded monitor header instead
// of the typed final-monitor heading, which changes the consumer's PROTECTION LOST suffix pixels.
// REQ: OA-RWY-015, OA-DSP-077
#[test]
fn degraded_final_monitor_protection_lost_heading_survives_regional_display_title() {
    let position = (CENTRE_LAT_E7, CENTRE_LON_E7);
    let mut runtime = configured_runtime(position);
    let warning = run_confirming_scans(&mut runtime, position);
    assert_confirmed_target_and_flight_link(&warning, LAST_SCAN_NS);
    assert!(
        matches!(warning.final_monitor.status, FinalMonitorStatus::Available)
            && warning.final_monitor.alerts.iter().any(|alert| {
                alert.address == TARGET_ADDRESS && alert.severity == FinalSeverity::Warning
            }),
        "confirmed prediction did not warn: status={:?} alerts={:?} tracker={:?}",
        warning.final_monitor.status,
        warning.final_monitor.alerts,
        warning.surveillance.tracker_stats
    );
    let near_limit = regional_step(
        &mut runtime,
        &warning_batch(NEAR_LIMIT_RECEIPT_NS, NEAR_LIMIT_SOURCE_NS, position, false),
    )
    .unwrap();
    assert_confirmed_target_and_flight_link(&near_limit, NEAR_LIMIT_SOURCE_NS);
    assert!(near_limit.surveillance.tracks.iter().any(|track| {
        track.address == TARGET_ADDRESS
            && track.state_ns == near_limit.surveillance.time_ns
            && track.newest_source_ns == NEAR_LIMIT_SOURCE_NS
    }));
    assert!(
        matches!(
            near_limit.final_monitor.status,
            FinalMonitorStatus::Available
        ),
        "near-limit prediction status={:?} timing={:?} tracker={:?}",
        near_limit.final_monitor.status,
        near_limit.final_monitor.timing,
        near_limit.surveillance.tracker_stats
    );
    let near_limit_timing = near_limit.final_monitor.timing.unwrap();
    let near_limit_now_ns = u64::try_from(near_limit.surveillance.time_ns).unwrap();
    let prior_frame_ns = u64::try_from(warning.surveillance.time_ns).unwrap();
    assert_eq!(
        near_limit_timing.observed_input_interval_ns,
        near_limit_now_ns - prior_frame_ns,
        "the input interval is exactly the configured cadence"
    );
    assert_eq!(
        near_limit_timing.prediction_age_ns,
        near_limit_now_ns - u64::try_from(NEAR_LIMIT_SOURCE_NS).unwrap()
    );
    assert_eq!(
        near_limit
            .surveillance
            .feeds
            .iter()
            .find(|feed| feed.name == *b"TRACK\0\0\0")
            .map(|feed| feed.state),
        Some(oa_display::core::safety::FeedState::Live),
        "the explicit monitor period informs feed cadence without changing source timestamps"
    );
    let frame = regional_step(
        &mut runtime,
        &warning_batch(STALE_RECEIPT_NS, STALE_SOURCE_NS, position, false),
    )
    .unwrap();
    assert_confirmed_target_and_flight_link(&frame, STALE_SOURCE_NS);
    assert!(
        matches!(
            frame.final_monitor.status,
            FinalMonitorStatus::Degraded(FinalMonitorDegradation::StalePredictionAndInputCadence)
        ),
        "stale source did not degrade: status={:?} timing={:?} alerts={:?} tracker={:?}",
        frame.final_monitor.status,
        frame.final_monitor.timing,
        frame.final_monitor.alerts,
        frame.surveillance.tracker_stats
    );
    assert!(frame.final_monitor.alerts.is_empty());
    assert_eq!(
        frame.final_monitor.timing.unwrap().prediction_age_ns,
        5_001_000_000,
        "monitor prediction age remains based on the actual source timestamp"
    );
    assert_eq!(
        frame
            .surveillance
            .feeds
            .iter()
            .find(|feed| feed.name == *b"TRACK\0\0\0")
            .map(|feed| feed.state),
        Some(oa_display::core::safety::FeedState::Live),
        "TRACK feed cadence health and final-monitor prediction age are separate clocks"
    );

    // The regional base view is titled "REGIONAL RESEARCH"; the typed monitor heading must still
    // retain its protection-loss phrase in the actual composed consumer raster.
    let actual = regional_pixels(&runtime).unwrap();
    let (expected, phrase_region) = protection_lost_suffix_pixels();
    assert!(consumer_suffix_matches(actual, &expected, phrase_region));
}

// Mutation killed: keep issuing final-monitor warnings after the configured TRACK source is Lost.
// REQ: OA-RWY-015
#[test]
fn configured_final_monitor_refuses_lost_track_source() {
    let position = (CENTRE_LAT_E7, CENTRE_LON_E7);
    let mut runtime = configured_runtime(position);
    let warning = run_confirming_scans(&mut runtime, position);
    assert!(warning
        .final_monitor
        .alerts
        .iter()
        .any(|alert| alert.severity == FinalSeverity::Warning));

    let lost = regional_step(
        &mut runtime,
        &ReceiptBatch {
            receipt_ns: 30_000_000_000,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        lost.surveillance
            .feeds
            .iter()
            .find(|feed| feed.name == *b"TRACK\0\0\0")
            .map(|feed| feed.state),
        Some(oa_display::core::safety::FeedState::Lost { age_ms: 24_000 })
    );
    assert!(lost.surveillance.surveillance_withheld);
    assert!(lost.final_monitor.alerts.is_empty());
    assert!(matches!(
        lost.final_monitor.status,
        FinalMonitorStatus::Degraded(FinalMonitorDegradation::MissingAssignedTrack)
    ));
}

// Mutation killed: make an empty first non-grid monitor frame fatal instead of presenting the
// configured assignment as missing and withholding final-monitor protection.
// REQ: OA-RWY-015
#[test]
fn empty_non_grid_startup_presents_missing_assignment_protection_lost() {
    let mut runtime = configured_runtime((CENTRE_LAT_E7, CENTRE_LON_E7));
    let frame = regional_step(
        &mut runtime,
        &ReceiptBatch {
            receipt_ns: 1_200_000_000,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .expect("empty non-grid startup frame remains reportable");

    assert!(frame.surveillance.tracks.is_empty());
    assert!(frame.final_monitor.alerts.is_empty());
    assert!(matches!(
        frame.final_monitor.status,
        FinalMonitorStatus::Degraded(FinalMonitorDegradation::MissingAssignedTrack)
    ));
}

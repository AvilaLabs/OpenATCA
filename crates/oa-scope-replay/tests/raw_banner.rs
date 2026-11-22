//! Raw-only banner geometry against the map's drawable target area.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use oa_airspace::model::{empty_model, Meta};
use oa_display::core::palette;
use oa_display::project::{project, unproject};
use oa_display::scene::LEGEND_STRIP_PX;
use oa_display::traffic::DEFAULT_STYLE;
use oa_display::traffic::{map_position, MapPos, MAX_TARGET_SYMBOL_RADIUS_PX};
use oa_scope_replay::frame::{
    replay_build_raw, replay_new, replay_raster, Replay, ReplayConfig, ReplayError,
};
use oa_scope_replay::raw::{RawObservationColumns, RawStats};
use oa_scope_replay::rows::DEFAULT_CONVERT;

const NOW: i64 = 1_700_000_000_000_000_000;
const ORD: (i32, i32) = (419_786_000, -879_048_000);
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const RANGE_NM: f64 = 30.0;
const BANNER_TOP_PX: i32 = 10;
const BANNER_SIZE_PX: i32 = 16;
const BANNER_PADDING_PX: i32 = 4;

fn replay(width: u32, height: u32) -> Replay {
    replay_with_capacity(width, height, 4)
}

fn replay_with_capacity(width: u32, height: u32, max_targets: usize) -> Replay {
    let model = empty_model(Meta {
        nasr_effective: 0,
        cifp_cycle: 0,
        artcc: "ZAU".into(),
        buffer_m: 0,
        nasr_sha256: [0; 32],
        cifp_sha256: [0; 32],
    });
    replay_new(
        model,
        &ReplayConfig {
            centre_lat_e7: ORD.0,
            centre_lon_e7: ORD.1,
            range_nm: RANGE_NM,
            width,
            height,
            max_targets,
            style: DEFAULT_STYLE,
            convert: DEFAULT_CONVERT,
        },
    )
    .unwrap()
}

fn build_raw(replay: &mut Replay, lat_e7: i32, lon_e7: i32) -> Result<RawStats, ReplayError> {
    let times = [NOW];
    let addresses = [0xA00001];
    let latitudes = [lat_e7];
    let longitudes = [lon_e7];
    let altitudes = [12_000];
    let speeds = [oa_display::traffic::NO_U16];
    let tracks = [oa_display::traffic::NO_U16];
    let observations = RawObservationColumns {
        t_ns: &times,
        addr: &addresses,
        lat_e7: &latitudes,
        lon_e7: &longitudes,
        alt_ft: &altitudes,
        gs_dkt: &speeds,
        track_ddeg: &tracks,
    };
    replay_build_raw(replay, &observations, NOW, "RAW GEOMETRY").map(|(_, stats)| stats)
}

fn build_columns(
    replay: &mut Replay,
    times: &[i64],
    addresses: &[u32],
    latitudes: &[i32],
    longitudes: &[i32],
) -> (RawStats, Vec<u8>) {
    let altitudes = vec![12_000; times.len()];
    let speeds = vec![oa_display::traffic::NO_U16; times.len()];
    let tracks = vec![oa_display::traffic::NO_U16; times.len()];
    let observations = RawObservationColumns {
        t_ns: times,
        addr: addresses,
        lat_e7: latitudes,
        lon_e7: longitudes,
        alt_ft: &altitudes,
        gs_dkt: &speeds,
        track_ddeg: &tracks,
    };
    let (_, stats) = replay_build_raw(replay, &observations, NOW, "RAW EVIDENCE").unwrap();
    replay_raster(replay).unwrap();
    (stats, replay.buf.clone())
}

fn has_text(replay: &Replay, wanted: &str) -> bool {
    (0..replay.list.texts.x.len()).any(|i| {
        let start = replay.list.texts.start[i] as usize;
        let len = replay.list.texts.len[i] as usize;
        &replay.list.text[start..start + len] == wanted
    })
}

// REQ: OA-DSP-071, OA-DSP-072
#[test]
fn raw_banner_is_outside_admitted_symbol_extent_and_bad_layout_refuses() {
    let mut replay = replay(WIDTH, HEIGHT);
    let proj = replay.view.proj;
    let banner_bottom_px = BANNER_TOP_PX + BANNER_SIZE_PX + BANNER_PADDING_PX;

    // The raw banner stays in the non-map top inset. Put one synthetic target at the very first
    // drawable map row, then verify its full centered marker extent clears the opaque banner.
    let target_y_px = i32::try_from(LEGEND_STRIP_PX).unwrap() + 1;
    let (lat_e7, lon_e7) = unproject(&proj, (WIDTH as i32 / 2) * 16, target_y_px * 16).unwrap();
    let Some((target_x16, target_y16)) = project(&proj, lat_e7, lon_e7) else {
        panic!("synthetic map-edge target must project");
    };
    assert!(matches!(map_position(&proj, lat_e7, lon_e7), MapPos::On(_)));
    let marker_top16 = target_y16 - MAX_TARGET_SYMBOL_RADIUS_PX * 16;
    assert!(
        marker_top16 > banner_bottom_px * 16,
        "marker at ({target_x16}, {target_y16}) must clear banner bottom {banner_bottom_px}px"
    );
    let stats = build_raw(&mut replay, lat_e7, lon_e7).unwrap();
    assert_eq!(stats.targets_admitted, 1);
    assert_eq!(stats.off_map_positions, 0);

    // A projected point inside the header band is not admitted by the map-area reducer; it is
    // counted as off-map, so it cannot silently become an obscured displayed target.
    let (header_lat, header_lon) = unproject(&proj, (WIDTH as i32 / 2) * 16, 20 * 16).unwrap();
    let Some((_, header_y16)) = project(&proj, header_lat, header_lon) else {
        panic!("synthetic header-band position must project");
    };
    assert!(header_y16 < i32::try_from(LEGEND_STRIP_PX).unwrap() * 16);
    let stats = build_raw(&mut replay, header_lat, header_lon).unwrap();
    assert_eq!(stats.targets_admitted, 0);
    assert_eq!(stats.off_map_positions, 1);

    // The existing raw-build refusal protects against a future header/inset edit that lets a
    // maximum-radius marker reach the opaque banner.
    replay.view.proj.top_inset = (banner_bottom_px + MAX_TARGET_SYMBOL_RADIUS_PX) as u32;
    assert!(matches!(
        build_raw(&mut replay, lat_e7, lon_e7),
        Err(ReplayError::RawBannerDoesNotFit { .. })
    ));
}

// REQ: OA-DSP-072
#[test]
fn raw_banner_is_visible_on_a_320_pixel_scope() {
    let mut narrow = replay(320, 240);
    assert_eq!(
        build_raw(&mut narrow, ORD.0, ORD.1)
            .unwrap()
            .targets_admitted,
        1
    );
    replay_raster(&mut narrow).unwrap();
    let ink = palette::PALETTE[usize::from(palette::ALERT)].to_le_bytes();
    assert!(narrow
        .buf
        .chunks_exact(narrow.spec.stride as usize)
        .skip(10)
        .take(12)
        .flat_map(|line| line.chunks_exact(4))
        .any(|pixel| pixel == ink));
}

// REQ: OA-DSP-071, OA-DSP-072
#[test]
fn raw_banner_distinguishes_no_fresh_position_from_valid_off_map_evidence() {
    let mut empty = replay(320, 240);
    let (empty_stats, empty_pixels) = build_columns(&mut empty, &[], &[], &[], &[]);
    assert_eq!(empty_stats.fresh_valid_positions, 0);
    assert!(has_text(&empty, "NO FRESH RAW POSITIONS"));

    let mut stale = replay(320, 240);
    let stale_times = [NOW - oa_scope_replay::raw::RAW_STALE_CUTOFF_NS - 1];
    let addresses = [0xA00001];
    let latitudes = [ORD.0];
    let longitudes = [ORD.1];
    let (stale_stats, stale_pixels) = build_columns(
        &mut stale,
        &stale_times,
        &addresses,
        &latitudes,
        &longitudes,
    );
    assert_eq!(stale_stats.stale_addresses, 1);
    assert_eq!(stale_stats.fresh_valid_positions, 0);
    assert!(has_text(&stale, "NO FRESH RAW POSITIONS"));
    assert_eq!(
        empty_pixels, stale_pixels,
        "empty and stale both show the notice"
    );

    // Valid but outside the projection still proves that fresh raw position evidence exists.
    let mut off_map = replay(320, 240);
    let fresh_times = [NOW];
    let off_map_lon = [ORD.1 + 10_000_000];
    let (off_map_stats, off_map_pixels) = build_columns(
        &mut off_map,
        &fresh_times,
        &addresses,
        &latitudes,
        &off_map_lon,
    );
    assert_eq!(off_map_stats.off_map_positions, 1);
    assert_eq!(off_map_stats.fresh_valid_positions, 1);
    assert!(!has_text(&off_map, "NO FRESH RAW POSITIONS"));
    assert_ne!(
        empty_pixels, off_map_pixels,
        "the second line changes the raster"
    );

    // Fresh in-map evidence remains countable even when target-slot capacity refuses admission.
    let mut limited = replay_with_capacity(320, 240, 1);
    let two_times = [NOW, NOW];
    let two_addresses = [0xA00001, 0xA00002];
    let two_lats = [ORD.0, ORD.0 + 100];
    let two_lons = [ORD.1, ORD.1 + 100];
    let (limited_stats, _) = build_columns(
        &mut limited,
        &two_times,
        &two_addresses,
        &two_lats,
        &two_lons,
    );
    assert_eq!(limited_stats.fresh_valid_positions, 2);
    assert_eq!(limited_stats.targets_admitted, 1);
    assert_eq!(limited_stats.refused_capacity, 1);
    assert!(!has_text(&limited, "NO FRESH RAW POSITIONS"));
}

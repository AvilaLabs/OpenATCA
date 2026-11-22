//! Raw-only banner geometry against the map's drawable target area.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use oa_airspace::model::{empty_model, Meta};
use oa_display::project::{project, unproject};
use oa_display::scene::LEGEND_STRIP_PX;
use oa_display::traffic::DEFAULT_STYLE;
use oa_display::traffic::{map_position, MapPos, MAX_TARGET_SYMBOL_RADIUS_PX};
use oa_scope_replay::frame::{replay_build_raw, replay_new, Replay, ReplayConfig, ReplayError};
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

fn replay() -> oa_scope_replay::frame::Replay {
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
            width: WIDTH,
            height: HEIGHT,
            max_targets: 4,
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

// REQ: OA-DSP-071, OA-DSP-072
#[test]
fn raw_banner_is_outside_admitted_symbol_extent_and_bad_layout_refuses() {
    let mut replay = replay();
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

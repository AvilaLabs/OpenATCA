//! The 2^26-pixel display profile through every public entry point: `check_frame`,
//! `scratch_for`, `build_scene` and `rasterize`, square and non-square, at the limit and at
//! the smallest product above it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::model::{empty_model, Meta, Model};
use oa_display::list::{list_new, DisplayError, ListCaps};
use oa_display::raster::{check_frame, frame_len, rasterize, FrameSpec, RenderError};
use oa_display::scene::{build_scene, scene_index, scratch_for, view_new};
use oa_display::MAX_DISPLAY_PIXELS;

fn model() -> Model {
    empty_model(Meta {
        nasr_effective: 20261118,
        cifp_cycle: 2611,
        artcc: String::from("ZAU"),
        buffer_m: 0,
        nasr_sha256: [0; 32],
        cifp_sha256: [0; 32],
    })
}

fn caps() -> ListCaps {
    ListCaps {
        lines: 512,
        rects: 8,
        circles: 512,
        polylines: 32,
        poly_vertices: 64,
        texts: 512,
        text_bytes: 16384,
    }
}

fn spec(w: u32, h: u32) -> FrameSpec {
    FrameSpec {
        width: w,
        height: h,
        stride: w * 4,
    }
}

// REQ: OA-DSP-053
#[test]
fn the_profile_is_exactly_two_to_the_26_pixels() {
    assert_eq!(MAX_DISPLAY_PIXELS, 1 << 26);
    for (w, h) in [(8192u32, 8192u32), (16384, 4096), (65536, 1024)] {
        assert_eq!(u64::from(w) * u64::from(h), MAX_DISPLAY_PIXELS);
    }
}

// REQ: OA-DSP-053
#[test]
fn check_frame_accepts_the_limit_and_refuses_the_next_product() {
    for (w, h) in [(8192, 8192), (16384, 4096), (65536, 1024)] {
        let s = spec(w, h);
        assert_eq!(check_frame(&s, frame_len(&s)), Ok(()), "{w}x{h}");
    }
    let over = spec(65536, 1025);
    assert!(matches!(
        check_frame(&over, frame_len(&over)),
        Err(RenderError::WorkLimit { requested, limit })
            if requested == u128::from(MAX_DISPLAY_PIXELS) + 65536
                && limit == u128::from(MAX_DISPLAY_PIXELS)
    ));
    let over = spec(8193, 8192);
    assert!(matches!(
        check_frame(&over, frame_len(&over)),
        Err(RenderError::WorkLimit { .. })
    ));
}

// REQ: OA-DSP-055
#[test]
fn scratch_for_accepts_the_limit_and_refuses_the_next_product() {
    let m = model();
    for (w, h) in [(8192, 8192), (16384, 4096), (65536, 1024)] {
        let v = view_new(0, 0, 50.0, w, h).unwrap();
        assert!(scratch_for(&m, &v).is_ok(), "{w}x{h}");
    }
    let v = view_new(0, 0, 50.0, 65536, 1025).unwrap();
    assert!(matches!(
        scratch_for(&m, &v),
        Err(DisplayError::TooLarge {
            what: "scene pixels",
            ..
        })
    ));
}

// REQ: OA-DSP-053
#[test]
fn build_scene_and_rasterize_accept_the_limit_and_refuse_the_next_product() {
    let m = model();
    let idx = scene_index(&m);
    for (w, h) in [(8192, 8192), (16384, 4096), (65536, 1024)] {
        let v = view_new(0, 0, 50.0, w, h).unwrap();
        let mut sc = scratch_for(&m, &v).unwrap();
        let mut list = list_new(caps()).unwrap();
        build_scene(&mut list, &m, &idx, &v, &mut sc).unwrap();
        let s = spec(w, h);
        let mut buf = vec![0u8; frame_len(&s)];
        assert_eq!(rasterize(&list, &s, &mut buf), Ok(()), "{w}x{h}");
    }

    // One row above the profile: both refuse before touching anything.
    let small = view_new(0, 0, 50.0, 640, 480).unwrap();
    let mut sc = scratch_for(&m, &small).unwrap();
    let mut list = list_new(caps()).unwrap();
    let over_view = view_new(0, 0, 50.0, 65536, 1025).unwrap();
    assert!(matches!(
        build_scene(&mut list, &m, &idx, &over_view, &mut sc),
        Err(DisplayError::TooLarge {
            what: "scene pixels",
            ..
        })
    ));
    let over = spec(65536, 1025);
    let mut buf = Vec::new();
    assert!(matches!(
        rasterize(&list, &over, &mut buf),
        Err(RenderError::WorkLimit { .. })
    ));
}

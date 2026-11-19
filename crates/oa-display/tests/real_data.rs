//! The map over a real ZAU snapshot. Reads the snapshot path from `OPENATCA_SNAPSHOT` (build one
//! with `oa-adapt build --nasr <zip> --cifp <zip> --artcc ZAU --out <file>`) and skips when it is
//! unset. Reports the time per frame at 1920 by 1080, at 250 NM and 60 NM.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::model::{stratum, validate_model};
use oa_airspace::snapshot::decode;
use oa_display::list::{list_clear, list_new};
use oa_display::probe::list_capacities;
use oa_display::raster::{
    frame_digest, frame_len, rasterize, render_work_charge, FrameSpec, MAX_RENDER_WORK,
};
use oa_display::safety::{build_safety, FeedState, FeedStatus, SafetyScene};
use oa_display::scene::{
    build_scene, scene_caps, scene_index, scene_work_charge, scratch_for, view_new, MAX_SCENE_WORK,
};
use oa_safety::inhibit::inhibits_new;
use oa_safety::net::{state_report, state_table_new, NetReport, NETS};
use oa_sha256::to_hex;
use std::time::Instant;

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

// REQ: OA-DSP-021
#[test]
fn a_real_zau_snapshot_renders_the_map_and_reports_frame_time() {
    let Some(path) = std::env::var_os("OPENATCA_SNAPSHOT") else {
        eprintln!("OPENATCA_SNAPSHOT is not set; skipping real-data test");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let model = decode(&bytes).unwrap();
    validate_model(&model).unwrap();
    assert_eq!(model.meta.artcc, "ZAU");
    let artcc_high = model
        .boundaries
        .stratum
        .iter()
        .filter(|&&s| s == stratum::ARTCC_HIGH)
        .count();
    let artcc_low = model
        .boundaries
        .stratum
        .iter()
        .filter(|&&s| s == stratum::ARTCC_LOW)
        .count();
    assert!(
        artcc_high > 0 && artcc_low > 0,
        "snapshot has no ARTCC boundaries"
    );

    // Chicago, as a controller sets a scope: 250 NM, and 60 NM about O'Hare, from the centre to
    // the shorter edge.
    let spec = FrameSpec {
        width: 1920,
        height: 1080,
        stride: 1920 * 4,
    };
    let index = scene_index(&model);
    let mut list = list_new(scene_caps(&model).unwrap()).unwrap();
    let mut buf = vec![0u8; frame_len(&spec)];
    let caps = list_capacities(&list);
    let mut scratch = scratch_for(
        &model,
        &view_new(419_800_000, -879_000_000, 250.0, 1920, 1080).unwrap(),
    )
    .unwrap();
    let mut wide = None;
    for (lat, lon, range) in [
        (419_800_000, -879_000_000, 250.0),
        (419_786_000, -879_048_000, 60.0),
        (419_786_000, -879_048_000, 20.0),
    ] {
        let view = view_new(lat, lon, range, 1920, 1080).unwrap();
        let stats = build_scene(&mut list, &model, &index, &view, &mut scratch).unwrap();
        // Nothing the data cannot place; the border legs are expected data, not a fault.
        assert_eq!(
            (
                stats.points_unplaced,
                stats.legs_unplaced,
                stats.edges_unplaced
            ),
            (0, 0, 0),
            "{stats:?}"
        );
        assert!(stats.own_boundaries > 0 && stats.rings >= 2 && stats.airway_legs > 10);
        if range > 100.0 {
            assert!(stats.legs_at_border > 0, "{stats:?}");
            assert!(stats.neighbour_boundaries > 0 && stats.airports > 10);
            wide = Some(view);
        } else if range > 40.0 {
            // The zoom hides fixes and every label but the VORs' and the instrument airports'.
            assert!(
                stats.fixes == 0 && stats.terminal_waypoints == 0,
                "{stats:?}"
            );
            assert!(stats.navaids > 10 && stats.labels > 5, "{stats:?}");
        } else {
            assert!(stats.fixes > 100 && stats.labels > 100, "{stats:?}");
            assert!(stats.terminal_waypoints > 0, "{stats:?}");
            assert!(stats.labels_dropped > 0 || stats.labels > 100);
        }
        rasterize(&list, &spec, &mut buf).unwrap();
        let first = frame_digest(&spec, &buf).unwrap();

        // The safety indications with every net running, no inhibition, every feed live and a
        // fresh picture add nothing: the frame is the same bytes.
        {
            let now = 1_000_000_000_000i64;
            let mut states = state_table_new(10_000_000_000).unwrap();
            for n in NETS {
                state_report(&mut states, n, NetReport::Running, now).unwrap();
            }
            let inhibits = Box::new(inhibits_new());
            let feeds = [FeedStatus {
                name: *b"ADSB\0\0\0\0",
                state: FeedState::Live,
            }];
            let sc = SafetyScene {
                states: &states,
                inhibits: &inhibits,
                now_ns: now,
                feeds: &feeds,
                newest_track_ns: Some(now - 1_000_000_000),
                frozen_limit_ns: 5_000_000_000,
            };
            let proj =
                oa_display::project::projection_new(lat, lon, range, 1920, 1080, 104).unwrap();
            let st = build_safety(&mut list, &proj, &sc).unwrap();
            assert_eq!(st.banner_lines, 0);
            let mut again = vec![0u8; frame_len(&spec)];
            rasterize(&list, &spec, &mut again).unwrap();
            assert_eq!(frame_digest(&spec, &again).unwrap(), first);
        }

        let (mut build_ms, mut raster_ms) = (Vec::new(), Vec::new());
        for _ in 0..30 {
            let t0 = Instant::now();
            build_scene(&mut list, &model, &index, &view, &mut scratch).unwrap();
            let t1 = Instant::now();
            rasterize(&list, &spec, &mut buf).unwrap();
            let t2 = Instant::now();
            // Every frame is the same bytes: the picture is a pure function of model and view.
            // (The hash is taken after the timed section: it is not part of a frame.)
            assert_eq!(frame_digest(&spec, &buf).unwrap(), first, "frames differ");
            build_ms.push((t1 - t0).as_secs_f64() * 1e3);
            raster_ms.push((t2 - t1).as_secs_f64() * 1e3);
        }
        assert_eq!(
            list_capacities(&list),
            caps,
            "a frame changed the list's capacity"
        );
        eprintln!("ZAU {range} NM frame sha256 {}", to_hex(&first));
        eprintln!("drawn: {stats:?}");
        eprintln!(
            "per frame at 1920x1080, {range} NM (median of 30): build {:.2} ms, raster {:.2} ms",
            median(&mut build_ms),
            median(&mut raster_ms)
        );
    }
    let view = wide.unwrap();
    build_scene(&mut list, &model, &index, &view, &mut scratch).unwrap();

    // Where the raster time goes: the same list with one kind of primitive removed.
    let mut probe = |name: &str, f: &dyn Fn(&mut oa_display::list::DisplayList)| {
        let mut l = list.clone();
        f(&mut l);
        let mut t = Vec::new();
        for _ in 0..15 {
            let t0 = Instant::now();
            rasterize(&l, &spec, &mut buf).unwrap();
            t.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!("  raster {name}: {:.2} ms", median(&mut t));
    };
    probe("full", &|_| {});
    probe("empty (clear only)", &|l| list_clear(l));
    probe("no lines", &|l| {
        l.lines = oa_display::list::Lines::default();
    });
    probe("no text", &|l| {
        l.texts = oa_display::list::Texts::default();
        l.text.clear();
    });
    probe("no circles", &|l| {
        l.circles = oa_display::list::Circles::default();
    });
    probe("no polylines", &|l| {
        l.polylines = oa_display::list::Polylines::default();
        l.poly_x.clear();
        l.poly_y.clear();
    });
}

// REQ: OA-DSP-054
#[test]
fn a_real_zau_snapshot_fits_the_work_limits_at_8192_by_8192() {
    let Some(path) = std::env::var_os("OPENATCA_SNAPSHOT") else {
        eprintln!("OPENATCA_SNAPSHOT is not set; skipping real-data test");
        return;
    };
    let model = decode(&std::fs::read(&path).unwrap()).unwrap();
    let spec = FrameSpec {
        width: 8192,
        height: 8192,
        stride: 8192 * 4,
    };
    let index = scene_index(&model);
    let mut list = list_new(scene_caps(&model).unwrap()).unwrap();
    let view = view_new(419_800_000, -879_000_000, 250.0, 8192, 8192).unwrap();
    let mut scratch = scratch_for(&model, &view).unwrap();
    let scene_charge = scene_work_charge(&model, &scratch, &view);
    build_scene(&mut list, &model, &index, &view, &mut scratch).unwrap();
    let render_charge = render_work_charge(&list, &spec).unwrap();
    let mut buf = vec![0u8; frame_len(&spec)];
    rasterize(&list, &spec, &mut buf).unwrap();
    eprintln!(
        "ZAU 8192x8192: points={} legs={} strings={} B; scene charge {scene_charge} of {MAX_SCENE_WORK}; render charge {render_charge} of {MAX_RENDER_WORK}",
        model.points.kind.len(),
        model.airway_legs.from.len(),
        model.strings.text.len(),
    );
    assert!(scene_charge <= MAX_SCENE_WORK && render_charge <= MAX_RENDER_WORK);
}

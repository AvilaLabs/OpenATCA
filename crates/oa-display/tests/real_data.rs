//! The map over a real ZAU snapshot. Reads the snapshot path from `OPENATCA_SNAPSHOT` (build one
//! with `oa-adapt build --nasr <zip> --cifp <zip> --artcc ZAU --out <file>`) and skips when it is
//! unset. Reports the time per frame at 1920 by 1080.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::model::{stratum, validate_model};
use oa_airspace::snapshot::decode;
use oa_display::list::{list_capacities, list_clear, list_new};
use oa_display::raster::{frame_digest, frame_len, rasterize, FrameSpec};
use oa_display::scene::{build_scene, scene_caps, view_new};
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

    // Chicago, 250 NM from the centre to the shorter edge, as a controller sets a scope.
    let view = view_new(419_800_000, -879_000_000, 250.0, 1920, 1080).unwrap();
    let spec = FrameSpec {
        width: 1920,
        height: 1080,
        stride: 1920 * 4,
    };
    let mut list = list_new(scene_caps(&model));
    let mut buf = vec![0u8; frame_len(&spec)];
    let caps = list_capacities(&list);

    let stats = build_scene(&mut list, &model, &view).unwrap();
    assert!(
        stats.boundaries > 0 && stats.airway_legs > 100 && stats.fixes > 100 && stats.airports > 10
    );
    assert!(stats.rings >= 4);
    rasterize(&list, &spec, &mut buf).unwrap();
    let first = frame_digest(&spec, &buf).unwrap();

    let (mut build_ms, mut raster_ms, mut hash_ms) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..30 {
        let t0 = Instant::now();
        build_scene(&mut list, &model, &view).unwrap();
        let t1 = Instant::now();
        rasterize(&list, &spec, &mut buf).unwrap();
        let t2 = Instant::now();
        let d = frame_digest(&spec, &buf).unwrap();
        let t3 = Instant::now();
        // Every frame is the same bytes: the picture is a pure function of model and view.
        assert_eq!(d, first, "frames differ");
        build_ms.push((t1 - t0).as_secs_f64() * 1e3);
        raster_ms.push((t2 - t1).as_secs_f64() * 1e3);
        hash_ms.push((t3 - t2).as_secs_f64() * 1e3);
    }
    assert_eq!(
        list_capacities(&list),
        caps,
        "a frame changed the list's capacity"
    );
    eprintln!("ZAU map frame sha256 {}", to_hex(&first));
    eprintln!("drawn: {stats:?}");
    eprintln!(
        "per frame at 1920x1080 (median of 30): build {:.2} ms, raster {:.2} ms, sha-256 {:.2} ms",
        median(&mut build_ms),
        median(&mut raster_ms),
        median(&mut hash_ms)
    );

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

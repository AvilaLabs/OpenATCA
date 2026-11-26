//! Cross-check tests over a hand-built airspace model. Every identifier here is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_airspace::model::{
    boundary_source, empty_model, point_kind, stratum, Meta, Model, PosE7, Span, StrIx, VertexIx,
};
use oa_notam::*;

/// A model for ARTCC `ZQQ`: airports `KQQA` (alternate `QQA`, runway ends 09L and 27R, a
/// localizer) and `QQB` (runway end 18 only), a fix `FIXA`, and a boundary square for `ZQQ`
/// (29.9 to 30.15 N, 90.5 to 90.15 W) and one for `ZQR` elsewhere.
fn model() -> Model {
    let meta = Meta {
        nasr_effective: 20261029,
        cifp_cycle: 2611,
        artcc: "ZQQ".to_string(),
        buffer_m: 0,
        nasr_sha256: [0; 32],
        cifp_sha256: [0; 32],
    };
    let mut m = empty_model(meta);
    let s = |m: &mut Model, text: &str| {
        let ix = StrIx::new((m.strings.offsets.len() - 1) as u32);
        m.strings.text.push_str(text);
        m.strings.offsets.push(m.strings.text.len() as u32);
        ix
    };
    let kqqa = s(&mut m, "KQQA");
    let qqa = s(&mut m, "QQA");
    let qqb = s(&mut m, "QQB");
    let r09 = s(&mut m, "09L");
    let r27 = s(&mut m, "27R");
    let r18 = s(&mut m, "18");
    let fixa = s(&mut m, "FIXA");
    let loc = s(&mut m, "IQQA");
    let zqq = s(&mut m, "ZQQ");
    let zqr = s(&mut m, "ZQR");
    for (kind, ident, alt, scope) in [
        (point_kind::AIRPORT, kqqa, qqa, StrIx::NONE),
        (point_kind::AIRPORT, qqb, StrIx::NONE, StrIx::NONE),
        (point_kind::THRESHOLD, r09, StrIx::NONE, kqqa),
        (point_kind::THRESHOLD, r27, StrIx::NONE, kqqa),
        (point_kind::THRESHOLD, r18, StrIx::NONE, qqb),
        (point_kind::LOCALIZER, loc, StrIx::NONE, kqqa),
        (point_kind::FIX, fixa, StrIx::NONE, StrIx::NONE),
    ] {
        m.points.kind.push(kind);
        m.points.ident.push(ident);
        m.points.alt_ident.push(alt);
        m.points.scope.push(scope);
    }
    let e7 = |d: f64| (d * 1e7).round() as i32;
    let square = |m: &mut Model, fac: StrIx, strat: u8, lat: (f64, f64), lon: (f64, f64)| {
        let start = m.vertices.len() as u32;
        for (la, lo) in [
            (lat.0, lon.0),
            (lat.0, lon.1),
            (lat.1, lon.1),
            (lat.1, lon.0),
        ] {
            m.vertices.push(PosE7 {
                lat: e7(la),
                lon: e7(lo),
            });
        }
        m.boundaries.source.push(boundary_source::NASR_ARB);
        m.boundaries.stratum.push(strat);
        m.boundaries.facility.push(fac);
        m.boundaries.vertices.push(Span {
            start: VertexIx::new(start),
            len: 4,
        });
    };
    square(
        &mut m,
        zqq,
        stratum::ARTCC_HIGH,
        (29.9, 30.15),
        (-90.5, -90.15),
    );
    square(
        &mut m,
        zqr,
        stratum::ARTCC_HIGH,
        (39.0, 41.0),
        (-81.0, -79.0),
    );
    m
}

struct Case {
    fir: &'static str,
    loc: &'static str,
    apt: Option<(&'static str, &'static str)>,
    rwy: Option<&'static str>,
    navaid: bool,
    ring: Option<&'static str>,
    coords: &'static str,
}

fn build(cases: &[Case]) -> Notams {
    let mut out = new_notams(16, 1 << 16, 16, 4096);
    let mut st = default_stats();
    for c in cases {
        let mut members = Vec::new();
        if let Some((d, i)) = c.apt {
            members.push(
                airport()
                    .replace("<aixm:designator>QQA<", &format!("<aixm:designator>{d}<"))
                    .replace("KQQA", i),
            );
        }
        if let Some(r) = c.rwy {
            members.push(runway().replace("09L/27R", r));
        }
        if c.navaid {
            members.push(navaid());
        }
        if let Some(r) = c.ring {
            members.push(runway_element(r));
        }
        let refs: Vec<&str> = members.iter().map(String::as_str).collect();
        let x = msg(&refs)
            .replace(
                "<event:affectedFIR>ZQQ<",
                &format!("<event:affectedFIR>{}<", c.fir),
            )
            .replace(
                "<event:location>QQA<",
                &format!("<event:location>{}<", c.loc),
            )
            .replace("3012N09045W", c.coords);
        run_into(x.as_bytes(), &mut out, &mut st).unwrap();
    }
    check_invariants(&out);
    out
}

const NEAR: &str = "30.10 -90.20 30.10 -90.30 30.20 -90.30 30.20 -90.20 30.10 -90.20";
const AROUND: &str = "29.0 -91.0 29.0 -90.0 31.0 -90.0 31.0 -91.0 29.0 -91.0";
const FAR: &str = "40.0 -80.0 40.0 -79.9 40.1 -79.9 40.1 -80.0 40.0 -80.0";

fn cases() -> Vec<Case> {
    let c = |fir, loc, apt, rwy, navaid, ring, coords| Case {
        fir,
        loc,
        apt,
        rwy,
        navaid,
        ring,
        coords,
    };
    vec![
        c(
            "ZQQ",
            "QQA",
            Some(("QQA", "KQQA")),
            Some("09L/27R"),
            true,
            Some(NEAR),
            "3012N09045W",
        ),
        c(
            "ZQR",
            "KQQA",
            Some(("QQB", "KQQB")),
            Some("18/36"),
            false,
            Some(AROUND),
            "3012N09045W",
        ),
        c(
            "KZQQ",
            "ZZZ",
            Some(("ZZZ", "KZZZ")),
            Some("09/27"),
            true,
            Some(FAR),
            "3012N09045W",
        ),
        c("ZQR", "FIXA", None, None, false, None, "3000N09020W"),
        c("ZQQ", "ZQQ", None, None, false, None, "3012N09045W"),
    ]
}

// REQ: OA-NOTAM-020
#[test]
fn locations_airports_runways_and_navaids_resolve_against_the_model() {
    let m = model();
    let n = build(&cases());
    let x = cross_check(&m, &n);
    let tl = |d, r, rr, rrr| Tally {
        distinct: d,
        resolved: r,
        rows: rr,
        rows_resolved: rrr,
    };
    // Locations: QQA and KQQA (an airport's own and alternate identifier), FIXA (a fix) and ZQQ
    // (an ARTCC) resolve; ZZZ does not.
    assert_eq!(x.all.location, tl(5, 4, 5, 4));
    assert_eq!(x.own.location, tl(3, 2, 3, 2));
    assert_eq!(x.all.unresolved_locations, vec!["ZZZ".to_string()]);
    // Airports: QQA, QQB resolve; ZZZ does not.
    assert_eq!(x.all.airport, tl(3, 2, 3, 2));
    assert_eq!(x.own.airport, tl(2, 1, 2, 1));
    assert_eq!(x.all.unresolved_airports, vec!["ZZZ".to_string()]);
    // Runways: both ends of 09L/27R are thresholds; 18/36 has no 36; ZZZ has no airport.
    assert_eq!(x.all.runway, tl(3, 1, 3, 1));
    assert_eq!(x.own.runway, tl(2, 1, 2, 1));
    assert_eq!(
        x.all.unresolved_runways,
        vec!["QQB 18/36".to_string(), "ZZZ 09/27".to_string()]
    );
    // Navaids: an ILS at an airport that has a localizer.
    assert_eq!(x.all.navaid, tl(2, 1, 2, 1));
    assert_eq!(x.own.navaid, tl(2, 1, 2, 1));
    assert_eq!(x.own_rows, 3);
    let text = format_cross_check(&x);
    assert!(text.contains("location: 4/5 distinct resolve, 4/5 rows"));
    assert!(text.contains("model ARTCC only"));
    // No rows, no counts.
    let none = cross_check(&m, &new_notams(1, 16, 1, 4));
    assert_eq!(none.all, Scope::default());
    assert_eq!(none.polygons.boundaries, 1);
}

// REQ: OA-NOTAM-021
#[test]
fn polygons_are_tested_against_the_models_own_artcc_boundary() {
    let m = model();
    let n = build(&cases());
    let x = cross_check(&m, &n);
    let p = x.polygons;
    // Only the model's own facility counts: one boundary, the neighbour's is ignored.
    assert_eq!((p.boundaries, p.boundaries_refused), (1, 0));
    assert_eq!((p.rows_with_polygons, p.polygons, p.unusable), (3, 3, 0));
    // NEAR has a vertex inside; AROUND has the whole boundary inside it; FAR touches nothing
    // (it lies in the neighbour's facility).
    assert_eq!((p.touching, p.rows_touching), (2, 2));
    // Circles: every row has one; the fourth row's centre is inside the boundary.
    assert_eq!((p.circles, p.circles_centre_inside), (5, 1));
    assert!(format_cross_check(&x).contains("2 touch the ARTCC boundary in 2 rows"));
    // A model without the facility's boundary: nothing touches, nothing is an error.
    let mut other = model();
    other.meta.artcc = "ZNO".to_string();
    let y = cross_check(&other, &n);
    assert_eq!(y.polygons.boundaries, 0);
    assert_eq!(y.polygons.touching, 0);
    assert_eq!(y.own_rows, 0);
    // A boundary of fewer than three vertices is counted as refused, not a panic.
    let mut bad = model();
    bad.boundaries.vertices[0].len = 2;
    let z = cross_check(&bad, &n);
    assert_eq!(
        (z.polygons.boundaries, z.polygons.boundaries_refused),
        (0, 1)
    );
    // A polygon oa-geo cannot build (two vertices after the ring is closed) is unusable.
    let mut dup = new_notams(2, 16, 4, 16);
    dup.poly_vstart.push(0);
    dup.poly_vlen.push(3);
    dup.poly_kind.push(FEAT_RUNWAY_ELEMENT);
    dup.poly_feature.push(u32::MAX);
    for _ in 0..3 {
        dup.vtx_lat.push(301_000_000);
        dup.vtx_lon.push(-901_000_000);
    }
    let mut r = BLANK_ROW;
    r.poly_count = 1;
    assert!(push_row(&mut dup, &r));
    let w = cross_check(&m, &dup);
    assert_eq!(
        (
            w.polygons.polygons,
            w.polygons.unusable,
            w.polygons.touching
        ),
        (1, 1, 0)
    );
}

/// The model with its own boundary (the first four vertices) replaced by a box.
fn model_with_boundary(lat: (f64, f64), lon: (f64, f64)) -> Model {
    let mut m = model();
    let e7 = |d: f64| (d * 1e7).round() as i32;
    for (v, (la, lo)) in m.vertices.iter_mut().zip([
        (lat.0, lon.0),
        (lat.0, lon.1),
        (lat.1, lon.1),
        (lat.1, lon.0),
    ]) {
        v.lat = e7(la);
        v.lon = e7(lo);
    }
    m
}

fn one_ring(ring: &'static str) -> Notams {
    build(&[Case {
        fir: "ZQQ",
        loc: "QQA",
        apt: None,
        rwy: None,
        navaid: false,
        ring: Some(ring),
        coords: "3012N09045W",
    }])
}

// REQ: OA-NOTAM-021
#[test]
fn a_boundary_point_between_a_long_polygon_edge_and_its_vertex_box_still_touches() {
    // The polygon's north edge runs 100 NM along 45 N, and the geodesic bulges about 670 m north
    // of the parallel at its middle (89.325 W). The boundary box lies in that sliver: no
    // vertex of the polygon is in it, and its corners are north of every polygon vertex.
    let m = model_with_boundary((45.002, 45.004), (-89.345, -89.305));
    let n = one_ring("44.0 -90.5 44.0 -88.15 45.0 -88.15 45.0 -90.5 44.0 -90.5");
    let p = cross_check(&m, &n).polygons;
    assert_eq!((p.polygons, p.unusable, p.boundaries), (1, 0, 1));
    assert_eq!((p.touching, p.rows_touching), (1, 1));
}

// REQ: OA-NOTAM-021
#[test]
fn a_polygon_across_the_antimeridian_touches_a_boundary_inside_it() {
    let m = model_with_boundary((30.4, 30.6), (179.9, -179.9));
    let n = one_ring("30.0 179.5 30.0 -179.5 31.0 -179.5 31.0 179.5 30.0 179.5");
    let p = cross_check(&m, &n).polygons;
    assert_eq!((p.polygons, p.unusable, p.boundaries), (1, 0, 1));
    assert_eq!((p.touching, p.rows_touching), (1, 1));
}

//! Geometry: fixed-point conversion, the selection area, the spherical estimate against the exact
//! distance, and the geodesic-versus-rhumb separation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::geom::*;
use oa_geo::{wgs84, LatLon};

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).expect("valid coordinate")
}

fn square() -> Vec<LatLon> {
    vec![
        ll(41.0, -89.0),
        ll(41.0, -87.0),
        ll(43.0, -87.0),
        ll(43.0, -89.0),
    ]
}

// REQ: OA-ASP-001
#[test]
fn fixed_point_conversion_rounds_to_nearest_and_rejects_the_unstorable() {
    for d in [
        0.0,
        41.978_611_1,
        -87.904_722_2,
        89.999_999_95,
        -179.999_999_9,
        12.345_678_91,
    ] {
        let v = deg_to_e7(d).expect("in range");
        assert!((e7_to_deg(v) - d).abs() <= 0.5e-7 + 1e-12, "{d}");
    }
    assert_eq!(deg_to_e7(41.978_611_1), Some(419_786_111));
    assert_eq!(deg_to_e7(f64::NAN), None);
    assert_eq!(deg_to_e7(f64::INFINITY), None);
    assert_eq!(deg_to_e7(300.0), None);
    assert_eq!(deg_to_e7(-300.0), None);
    // The worst rounding error is half a unit, about 5.6 mm of latitude.
    let ell = wgs84();
    let a = ll(42.0, -88.0);
    let b = ll(42.0 + 0.5e-7, -88.0);
    let d = distance_m(&ell, a, b).expect("distance");
    assert!(d < 0.006, "{d}");
}

// REQ: OA-ASP-002
#[test]
fn exact_distance_to_a_segment_matches_a_known_geometry() {
    let ell = wgs84();
    // One degree of longitude on the equator is 111319.49 m on WGS84; the segment runs north-south
    // through the origin, so the nearest point to (0, 1) is (0, 0).
    let d = exact_edge_m(&ell, ll(-1.0, 0.0), ll(1.0, 0.0), ll(0.0, 1.0)).expect("distance");
    assert!((d - 111_319.49).abs() < 1.0, "{d}");
    // Beyond the end of the segment the nearest point is the end.
    let end = exact_edge_m(&ell, ll(-1.0, 0.0), ll(1.0, 0.0), ll(2.0, 0.0)).expect("distance");
    let direct = distance_m(&ell, ll(1.0, 0.0), ll(2.0, 0.0)).expect("distance");
    assert!((end - direct).abs() < 0.01, "{end} {direct}");
    // A degenerate segment is a point.
    let p = exact_edge_m(&ell, ll(5.0, 5.0), ll(5.0, 5.0), ll(5.0, 6.0)).expect("distance");
    let q = distance_m(&ell, ll(5.0, 5.0), ll(5.0, 6.0)).expect("distance");
    assert!((p - q).abs() < 0.01);
}

// REQ: OA-ASP-002
#[test]
fn area_membership_agrees_with_the_exact_distance_around_the_buffer() {
    // The spherical estimate decides far points and the exact distance the points near the
    // buffer; the answer must equal "inside, or exact distance within the buffer" everywhere,
    // including points a few metres either side of the buffer.
    let buffer = 92_600.0;
    let area = build_area(vec![square()], buffer).expect("area");
    let mut checked = 0;
    let mut near_buffer = 0;
    for ilat in 0..=40 {
        for ilon in 0..=40 {
            let p = ll(39.5 + f64::from(ilat) * 0.1, -90.5 + f64::from(ilon) * 0.1);
            let exact = area_edge_distance_m(&area, p).expect("distance");
            if area_inside(&area, p) {
                assert!(area_contains(&area, p).expect("contains"), "{p:?} inside");
                continue;
            }
            if (exact - buffer).abs() < 2.0 {
                continue; // too close to the boundary of the test itself
            }
            assert_eq!(
                area_contains(&area, p).expect("contains"),
                exact <= buffer,
                "{p:?} exact {exact}"
            );
            checked += 1;
            if (exact - buffer).abs() < 0.02 * buffer {
                near_buffer += 1;
            }
        }
    }
    assert!(checked > 500, "{checked}");
    assert!(near_buffer > 10, "{near_buffer}");
    // Points placed at known distances from the middle of the northern edge's meridian.
    let ell = wgs84();
    let edge_mid = ell
        .direct(ll(43.0, -89.0), 90.0, 100_000.0)
        .expect("direct")
        .position;
    for (metres, want) in [(91_000.0, true), (94_000.0, false)] {
        let p = ell.direct(edge_mid, 0.0, metres).expect("direct").position;
        // Distance to the geodesic edge differs from `metres` by the edge's bulge (under 1 km here).
        let exact = area_edge_distance_m(&area, p).expect("distance");
        assert!((exact - metres).abs() < 1_500.0, "{exact}");
        assert_eq!(area_contains(&area, p).expect("contains"), want, "{metres}");
    }
}

// REQ: OA-ASP-002
#[test]
fn zero_buffer_is_the_polygon_and_a_wider_buffer_only_adds() {
    let tight = build_area(vec![square()], 0.0).expect("area");
    let wide = build_area(vec![square()], 50_000.0).expect("area");
    let inside = ll(42.0, -88.0);
    let outside_near = ll(43.2, -88.0);
    let outside_far = ll(45.0, -88.0);
    assert!(area_inside(&tight, inside) && area_contains(&tight, inside).expect("c"));
    assert!(!area_contains(&tight, outside_near).expect("c"));
    assert!(area_contains(&wide, outside_near).expect("c"));
    assert!(!area_contains(&wide, outside_far).expect("c"));
    assert!(
        !area_inside(&wide, outside_near),
        "the buffer is not the polygon"
    );
}

// REQ: OA-ASP-002
#[test]
fn one_degree_cells_overlap_the_area_by_sample_or_vertex() {
    let area = build_area(vec![square()], 0.0).expect("area");
    assert!(cell_overlaps_area(&area, 42, -89).expect("cell"));
    assert!(
        cell_overlaps_area(&area, 40, -88).expect("cell"),
        "vertex 41N lies in the cell 40..41"
    );
    assert!(!cell_overlaps_area(&area, 10, -89).expect("cell"));
    assert!(!cell_overlaps_area(&area, 42, -60).expect("cell"));
}

// REQ: OA-ASP-002
#[test]
fn a_volume_that_encloses_the_area_touches_it() {
    let area = build_area(vec![square()], 0.0).expect("area");
    let big: Vec<LatLon> = vec![
        ll(35.0, -95.0),
        ll(35.0, -80.0),
        ll(50.0, -80.0),
        ll(50.0, -95.0),
    ];
    let samples = ring_samples(&wgs84(), &big).expect("samples");
    assert!(samples_touch_area(&area, &samples).expect("touch"));
    let far: Vec<LatLon> = vec![
        ll(10.0, 10.0),
        ll(10.0, 12.0),
        ll(12.0, 12.0),
        ll(12.0, 10.0),
    ];
    let samples = ring_samples(&wgs84(), &far).expect("samples");
    assert!(!samples_touch_area(&area, &samples).expect("touch"));
    assert!(!samples_touch_area(&area, &[]).expect("touch"));
}

// REQ: OA-ASP-005
#[test]
fn geodesic_and_rhumb_separate_by_the_known_amount_on_a_high_latitude_parallel() {
    // The edge along 60 N from 0 to 30 E: the rhumb line follows the parallel, the great circle
    // bulges poleward to about 60.85 N at the middle (spherical: atan(tan 60 / cos 15)), which is
    // about 94 km.
    let ring = vec![ll(60.0, 0.0), ll(60.0, 30.0), ll(59.0, 15.0)];
    let sep = edge_separations(&wgs84(), &ring).expect("separation");
    assert_eq!(sep.edge, 0);
    assert!(
        sep.max_m > 90_000.0 && sep.max_m < 99_000.0,
        "{}",
        sep.max_m
    );
    assert!((sep.at_fraction - 0.5).abs() < 0.05, "{}", sep.at_fraction);
    assert!(
        (sep.edge_len_m - 1_660_000.0).abs() < 40_000.0,
        "{}",
        sep.edge_len_m
    );
}

// REQ: OA-ASP-005
#[test]
fn a_meridian_or_the_equator_has_no_separation_and_a_point_edge_is_skipped() {
    let ell = wgs84();
    let meridian = vec![ll(10.0, 5.0), ll(20.0, 5.0), ll(20.0, 6.0)];
    let sep = edge_separations(&ell, &meridian).expect("separation");
    // The meridian edge is exact under both; the others contribute, but not the meridian one.
    assert_ne!(sep.edge, 0);
    let degenerate = vec![ll(10.0, 5.0), ll(10.0, 5.0)];
    let none = edge_separations(&ell, &degenerate).expect("separation");
    assert_eq!(none.max_m, 0.0);
}

// REQ: OA-ASP-002
#[test]
fn segment_samples_follow_circles_arcs_and_edges() {
    use oa_airspace::model::via;
    let ell = wgs84();
    let centre = ll(42.0, -88.0);
    let circle = [SegIn {
        via: via::CIRCLE,
        pos: None,
        centre: Some(centre),
        radius_m: 10_000.0,
    }];
    let pts = segment_samples(&ell, &circle).expect("samples");
    assert!(pts.len() >= 36);
    for p in &pts {
        let d = distance_m(&ell, centre, *p).expect("distance");
        assert!((d - 10_000.0).abs() < 1.0, "{d}");
    }
    let edge = [
        SegIn {
            via: via::GEODESIC,
            pos: Some(ll(42.0, -88.0)),
            centre: None,
            radius_m: 0.0,
        },
        SegIn {
            via: via::GEODESIC,
            pos: Some(ll(42.0, -87.0)),
            centre: None,
            radius_m: 0.0,
        },
    ];
    let pts = segment_samples(&ell, &edge).expect("samples");
    assert!(pts.len() >= 5, "{}", pts.len());
}

// REQ: OA-ASP-008
#[test]
fn the_bounding_filter_keeps_points_on_both_sides_of_the_antimeridian() {
    let ring = vec![
        ll(40.0, 179.0),
        ll(40.0, -179.0),
        ll(42.0, -179.0),
        ll(42.0, 179.0),
    ];
    let area = build_area(vec![ring], 0.0).expect("area");
    for lon in [179.5, 180.0, -180.0, -179.5] {
        let p = ll(41.0, lon);
        assert!(area_inside(&area, p), "inside at {lon}");
        assert!(area_contains(&area, p).expect("contains"), "contains {lon}");
    }
    assert!(!area_inside(&area, ll(41.0, 170.0)));
    assert!(!area_inside(&area, ll(41.0, -170.0)));
    assert!(!area_inside(&area, ll(41.0, 0.0)));
    // A volume outline crossing the antimeridian touches the area; one on the far side does not.
    let crossing = [
        ll(41.0, 179.5),
        ll(41.1, 179.9),
        ll(41.0, -179.9),
        ll(41.1, -179.5),
    ];
    assert!(samples_touch_area(&area, &crossing).expect("touch"));
    let far = [ll(41.0, 10.0), ll(41.1, 11.0), ll(41.2, 10.5)];
    assert!(!samples_touch_area(&area, &far).expect("touch"));
    // Cells on either side of the antimeridian overlap.
    assert!(cell_overlaps_area(&area, 40, 179).expect("cell"));
    assert!(cell_overlaps_area(&area, 40, -180).expect("cell"));
    assert!(!cell_overlaps_area(&area, 40, 100).expect("cell"));
}

// REQ: OA-ASP-008
#[test]
fn the_bounding_filter_includes_the_poleward_bulge_of_a_long_edge() {
    // The geodesic from 65N 30W to 65N 30E rises to about 68N at the meridian 0, three degrees
    // above both of its vertices.
    let ring = vec![
        ll(65.0, -30.0),
        ll(65.0, 30.0),
        ll(50.0, 30.0),
        ll(50.0, -30.0),
    ];
    let area = build_area(vec![ring], 0.0).expect("area");
    let p = ll(67.5, 0.0);
    assert!(area_inside(&area, p), "inside the polygon, below the bulge");
    assert!(area_contains(&area, p).expect("contains"));
    let b = area.bbox.expect("narrow enough for a box");
    assert!(b.max_lat > 67.9, "{}", b.max_lat);
    // The same ring mirrored south bulges toward the south pole.
    let south = vec![
        ll(-65.0, -30.0),
        ll(-50.0, -30.0),
        ll(-50.0, 30.0),
        ll(-65.0, 30.0),
    ];
    let area = build_area(vec![south], 0.0).expect("area");
    assert!(area_inside(&area, ll(-67.5, 0.0)));
}

// REQ: OA-ASP-008
#[test]
fn an_outline_that_cannot_be_formed_is_an_error_not_a_miss() {
    let area = build_area(vec![square()], 0.0).expect("area");
    // The first sample is just outside the area but inside its box; the outline has an edge
    // far longer than any polygon edge may be, so no polygon can be built from it.
    let samples = [ll(43.005, -88.0), ll(10.0, -100.0), ll(10.0, 100.0)];
    let r = samples_touch_area(&area, &samples);
    assert!(r.is_err(), "{r:?}");
    // Two samples enclose nothing; that is an ordinary "not touching".
    let two = [ll(43.005, -88.0), ll(43.006, -88.0)];
    assert_eq!(samples_touch_area(&area, &two), Ok(false));
}

fn rect(lat0: f64, lat1: f64, lon0: f64, lon1: f64) -> Vec<LatLon> {
    vec![
        ll(lat0, lon0),
        ll(lat0, lon1),
        ll(lat1, lon1),
        ll(lat1, lon0),
    ]
}

/// Rotates every vertex about the origin by `deg` in the (lon, lat) plane and then shifts it;
/// near the equator one degree is about the same length either way.
fn moved(ring: &[LatLon], deg: f64, dlat: f64, dlon: f64) -> Vec<LatLon> {
    let (s, c) = deg.to_radians().sin_cos();
    ring.iter()
        .map(|p| {
            ll(
                p.lon() * s + p.lat() * c + dlat,
                p.lon() * c - p.lat() * s + dlon,
            )
        })
        .collect()
}

/// True when the closed outline `ring`, with its edges sampled as the build samples them,
/// touches the zero-buffer area `area_ring`.
fn volume_selected(area_ring: &[LatLon], volume: &[LatLon], buffer_m: f64) -> bool {
    let area = build_area(vec![area_ring.to_vec()], buffer_m).expect("area");
    let samples = dense_ring_samples(&wgs84(), volume).expect("samples");
    samples_touch_area(&area, &samples).expect("touch")
}

// REQ: OA-ASP-065
#[test]
fn a_volume_that_crosses_the_area_without_a_vertex_in_either_is_selected() {
    // Area: a tall thin rectangle; volume: a wide flat one across it. They overlap by about
    // one square kilometre, neither holds a vertex of the other, and the samples of either
    // outline are up to 20 km apart.
    let area = rect(-0.18, 0.18, 0.0405, 0.0495);
    let vol = rect(0.009, 0.018, -0.18, 0.18);
    assert!(volume_selected(&area, &vol, 0.0));
    assert!(volume_selected(&vol, &area, 0.0), "symmetric");
    // Moved and turned together, the pair still overlaps.
    for (deg, dlat, dlon) in [
        (0.0, 0.0, 0.0),
        (0.0, 3.0, -7.0),
        (0.0, -12.0, 40.0),
        (30.0, 0.0, 0.0),
        (90.0, 5.0, 5.0),
        (123.0, -4.0, 60.0),
        (-45.0, 20.0, -100.0),
    ] {
        let a = moved(&area, deg, dlat, dlon);
        let v = moved(&vol, deg, dlat, dlon);
        assert!(volume_selected(&a, &v, 0.0), "{deg} {dlat} {dlon}");
    }
    // The same volume lifted 10 m clear of the area's northern edge is not selected at zero
    // buffer, is at a buffer larger than the gap, and is not at one smaller.
    let lifted = rect(0.18 + 0.0000905, 0.18 + 0.009, -0.18, 0.18);
    assert!(!volume_selected(&area, &lifted, 0.0));
    assert!(!volume_selected(&area, &lifted, 5.0));
    assert!(volume_selected(&area, &lifted, 20.0));
    // A normal buffer: a volume 30 km north of the area is in at 50 km and out at 10 km.
    let far = rect(0.18 + 0.27, 0.18 + 0.30, -0.18, 0.18);
    assert!(volume_selected(&area, &far, 50_000.0));
    assert!(!volume_selected(&area, &far, 10_000.0));
}

// REQ: OA-ASP-066
#[test]
fn a_cell_crossed_by_a_thin_area_is_selected_wherever_the_strip_lies() {
    // The strip is 0.01 degrees (about 1.1 km) tall and lies between the rows of the old
    // 81-point grid of the cell (every 0.125 degrees).
    let strip = rect(0.060, 0.070, -0.10, 1.10);
    let area = build_area(vec![strip], 0.0).expect("area");
    assert!(cell_overlaps_area(&area, 0, 0).expect("cell"));
    for k in 0..8 {
        let la = f64::from(k) * 0.125 + 0.05;
        let area = build_area(vec![rect(la, la + 0.01, -0.10, 1.10)], 0.0).expect("area");
        assert!(cell_overlaps_area(&area, 0, 0).expect("cell"), "row {k}");
        // The same strip in longitude, between the columns of the grid.
        let area = build_area(vec![rect(-0.10, 1.10, la, la + 0.01)], 0.0).expect("area");
        assert!(cell_overlaps_area(&area, 0, 0).expect("cell"), "column {k}");
    }
    // A strip that does not reach the cell is not selected, and a normal buffer still adds.
    let off = build_area(vec![rect(-0.30, -0.20, -0.10, 1.10)], 0.0).expect("area");
    assert!(!cell_overlaps_area(&off, 0, 0).expect("cell"));
    let near = build_area(vec![rect(-0.30, -0.20, -0.10, 1.10)], 30_000.0).expect("area");
    assert!(cell_overlaps_area(&near, 0, 0).expect("cell"));
    let wide = build_area(vec![rect(-0.30, -0.20, -0.10, 1.10)], 10_000.0).expect("area");
    assert!(!cell_overlaps_area(&wide, 0, 0).expect("cell"));
}

fn arc_pair(ell: &oa_geo::Ellipsoid, centre: LatLon, ends_m: f64, coded_m: f64) -> [SegIn; 2] {
    use oa_airspace::model::via;
    let at = |brg: f64| ell.direct(centre, brg, ends_m).expect("direct").position;
    [
        SegIn {
            via: via::ARC_CW,
            pos: Some(at(0.0)),
            centre: Some(centre),
            radius_m: coded_m,
        },
        SegIn {
            via: via::GEODESIC,
            pos: Some(at(90.0)),
            centre: None,
            radius_m: 0.0,
        },
    ]
}

/// A 200 m square `r_m` from `centre` on bearing 45.
fn square_at(ell: &oa_geo::Ellipsoid, centre: LatLon, r_m: f64) -> Vec<LatLon> {
    let c = ell.direct(centre, 45.0, r_m).expect("direct").position;
    let (dlat, dlon) = (
        100.0 / 111_000.0,
        100.0 / (111_320.0 * c.lat().to_radians().cos()),
    );
    rect(
        c.lat() - dlat,
        c.lat() + dlat,
        c.lon() - dlon,
        c.lon() + dlon,
    )
}

// REQ: OA-ASP-068
#[test]
fn arc_selection_uses_the_coded_radius_not_the_distance_to_the_start() {
    let ell = wgs84();
    let centre = ll(42.0, -88.0);
    let touches = |segs: &[SegIn], r_m: f64| {
        let area = build_area(vec![square_at(&ell, centre, r_m)], 0.0).expect("area");
        let samples = segment_samples(&ell, segs).expect("samples");
        samples_touch_area(&area, &samples).expect("touch")
    };
    // Ends 5 km out, coded 10 km: the stored arc is the 10 km one, so an area on it is hit
    // (the old derived radius, 5 km, missed it) and one beyond it is not.
    let wide = arc_pair(&ell, centre, 5_000.0, 10_000.0);
    assert!(touches(&wide, 10_000.0), "area on the coded radius");
    assert!(!touches(&wide, 12_500.0), "area beyond the coded radius");
    // Ends 10 km out, coded 5 km: the other way round. An area at 10 km, where the derived
    // radius put the arc, is clear of the stored one; an area on the coded radius is hit.
    let narrow = arc_pair(&ell, centre, 10_000.0, 5_000.0);
    assert!(touches(&narrow, 5_000.0), "area on the coded radius");
    assert!(!touches(&narrow, 10_000.0), "area on the derived radius");
    assert!(!touches(&narrow, 7_500.0), "area between the two radii");
    // The gap the build reports is the one between the coded radius and the end points.
    assert!((arc_radius_gap_m(&ell, &wide).expect("gap") - 5_000.0).abs() < 1.0);
    assert!((arc_radius_gap_m(&ell, &narrow).expect("gap") - 5_000.0).abs() < 1.0);
    let sound = arc_pair(&ell, centre, 7_000.0, 7_000.0);
    assert!(arc_radius_gap_m(&ell, &sound).expect("gap") < 1.0);
    // A coded radius one tenth of a nautical mile off is within the tolerance, 0.2 NM is not.
    let near = arc_pair(&ell, centre, 7_000.0, 7_000.0 + 92.6);
    assert!(arc_radius_gap_m(&ell, &near).expect("gap") <= ARC_RADIUS_TOLERANCE_M);
    let off = arc_pair(&ell, centre, 7_000.0, 7_000.0 + 185.2);
    assert!(arc_radius_gap_m(&ell, &off).expect("gap") > ARC_RADIUS_TOLERANCE_M);
}

// REQ: OA-ASP-065
#[test]
fn a_rhumb_edge_is_sampled_as_the_rhumb_line_it_is() {
    use oa_airspace::model::via;
    let ell = wgs84();
    // 60 N, 30 degrees of longitude: the rhumb line stays on the parallel, the geodesic rises
    // about 0.85 degrees (95 km) above it at mid-way.
    let seg = |v: u8, lon: f64| SegIn {
        via: v,
        pos: Some(ll(60.0, lon)),
        centre: None,
        radius_m: 0.0,
    };
    let rhumb = segment_samples(&ell, &[seg(via::RHUMB, 0.0), seg(via::RHUMB, 30.0)]).expect("s");
    let geod =
        segment_samples(&ell, &[seg(via::GEODESIC, 0.0), seg(via::GEODESIC, 30.0)]).expect("s");
    // The samples run to the second vertex and back along the closing edge, so look at the
    // first half: the outbound edge.
    let mid_rhumb = rhumb.iter().map(|p| p.lat()).fold(f64::MIN, f64::max);
    let mid_geod = geod.iter().map(|p| p.lat()).fold(f64::MIN, f64::max);
    assert!((mid_rhumb - 60.0).abs() < 1e-6, "{mid_rhumb}");
    assert!(mid_geod > 60.8, "{mid_geod}");
    assert!(rhumb.len() > 600, "{}", rhumb.len());
}

// REQ: OA-ASP-002
#[test]
fn the_selection_never_disagrees_with_the_exact_distance_near_the_buffer_of_a_long_edge() {
    let ell = wgs84();
    for lat in [24.0, 50.0] {
        for az0 in [90.0, 60.0] {
            for len_nm in [300.0, 600.0] {
                let a = ll(lat, -100.0);
                let b = ell.direct(a, az0, len_nm * 1852.0).unwrap().position;
                // The third vertex is north of the edge, so points south of it are outside.
                let ring = vec![a, b, ll(lat + 3.0, -100.0)];
                let inv = ell.inverse(a, b).unwrap();
                for buf in [1000.0, 3000.0, 10_000.0] {
                    let area = build_area(vec![ring.clone()], buf).unwrap();
                    for ti in (1..24).step_by(2) {
                        let t = inv.distance_m * f64::from(ti) / 24.0;
                        let q = ell.direct(a, inv.azimuth1_deg, t).unwrap();
                        for k in (-30..=30).step_by(3).filter(|k| *k != 0) {
                            let d = buf * (1.0 + f64::from(k) * 0.01);
                            let p = ell
                                .direct(q.position, q.azimuth2_deg + 90.0, d)
                                .unwrap()
                                .position;
                            let truth = area_inside(&area, p)
                                || area_edge_distance_m(&area, p).unwrap() <= buf;
                            assert_eq!(
                                area_contains(&area, p).unwrap(),
                                truth,
                                "lat {lat} az {az0} len {len_nm} NM buffer {buf} m, point {d} m from the edge at {t} m"
                            );
                        }
                    }
                }
            }
        }
    }
}

// REQ: OA-ASP-008
#[test]
fn a_ring_box_holds_the_bulge_of_its_edges_and_wraps_over_the_antimeridian() {
    let ell = wgs84();
    // A 100 NM edge along 45 N bulges about 670 m poleward: the box is taller than the vertices.
    let ring = [
        ll(44.0, -90.5),
        ll(44.0, -88.15),
        ll(45.0, -88.15),
        ll(45.0, -90.5),
    ];
    let b = ring_bbox(&ell, &ring).unwrap();
    assert!(b.max_lat > 45.005 && b.max_lat < 45.01, "{}", b.max_lat);
    assert!(b.min_lat < 44.0 && b.min_lat > 43.99);
    // Across the antimeridian the interval is unwrapped: 179.5 east to 180.5.
    let w = [
        ll(30.0, 179.5),
        ll(30.0, -179.5),
        ll(31.0, -179.5),
        ll(31.0, 179.5),
    ];
    let b = ring_bbox(&ell, &w).unwrap();
    assert!(lon_overlaps(179.9, 179.9, b.min_lon, b.max_lon));
    assert!(lon_overlaps(-179.9, -179.9, b.min_lon, b.max_lon));
    assert!(!lon_overlaps(0.0, 0.0, b.min_lon, b.max_lon));
    // Too wide or too far north for a box to be a safe filter.
    assert!(ring_bbox(&ell, &[ll(0.0, 0.0), ll(0.0, 100.0), ll(10.0, -160.0)]).is_none());
    assert!(ring_bbox(&ell, &[ll(81.0, 0.0), ll(81.0, 10.0), ll(82.0, 5.0)]).is_none());
}

// REQ: OA-ASP-074
#[test]
fn a_boundary_needing_more_samples_than_the_budget_is_refused() {
    use oa_airspace::model::via;
    let ell = wgs84();
    // 300 edges of about 20,000 km each: 4,000 dense points apiece, over a million in all.
    let ring: Vec<LatLon> = (0..300)
        .map(|i| {
            ll(
                if i % 2 == 0 { 10.0 } else { -10.0 },
                if i % 2 == 0 { 0.0 } else { 179.0 },
            )
        })
        .collect();
    assert!(dense_ring_samples(&ell, &ring).is_err());
    assert!(ring_samples(&ell, &ring).is_ok());
    // 400 circles of 400 km radius: about 3,200 points apiece.
    let circle = SegIn {
        via: via::CIRCLE,
        pos: None,
        centre: Some(ll(42.0, -88.0)),
        radius_m: 400_000.0,
    };
    assert!(segment_samples(&ell, &vec![circle; 400]).is_err());
    assert!(segment_samples(&ell, &vec![circle; 10]).is_ok());
}

// REQ: OA-ASP-074
#[test]
fn boundary_tests_past_the_work_budget_are_refused_before_any_test() {
    let area = build_area(vec![square()], 0.0).expect("area");
    let pts = vec![ll(50.0, 0.0); 10];
    // Ten samples against the square's edges, dense edges and vertices is more than 100 tests.
    assert!(samples_touch_area_within(&area, &pts, 100).is_err());
    assert_eq!(samples_touch_area_within(&area, &pts, 1 << 20), Ok(false));
}

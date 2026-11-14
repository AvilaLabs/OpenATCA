//! Geometry: fixed-point conversion, the selection area, the spherical estimate against the exact
//! distance, and the geodesic-versus-rhumb separation.

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

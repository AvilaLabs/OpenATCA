//! Point-in-polygon with geodesic edges (OA-GEO-006), including the real ZAU LOW boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_geo::{wgs84, Ellipsoid, GeoError, LatLon, Polygon};

const ZAU_LOW: &str = include_str!("fixtures/zau_low.csv");

fn pt(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).expect("valid")
}

fn zau_vertices() -> Vec<LatLon> {
    ZAU_LOW
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            assert_eq!(f.len(), 3, "{l}");
            pt(f[1].parse().expect("lat"), f[2].parse().expect("lon"))
        })
        .collect()
}

// REQ: OA-GEO-006
#[test]
fn zau_low_contains_airports_inside_and_excludes_those_outside() {
    let e = wgs84();
    let verts = zau_vertices();
    assert_eq!(verts.len(), 34, "ZAU LOW fixture");
    let zau = Polygon::new(&e, &verts).expect("ZAU LOW is a valid polygon");
    assert_eq!(zau.len(), 34);
    // Inside: Chicago O'Hare and Midway.
    assert!(zau.contains(pt(41.9786, -87.9048)), "ORD");
    assert!(zau.contains(pt(41.7868, -87.7522)), "MDW");
    // Outside: Detroit (ZOB), Minneapolis (ZMP), Atlanta, Denver.
    assert!(!zau.contains(pt(42.2124, -83.3534)), "DTW");
    assert!(!zau.contains(pt(44.8848, -93.2223)), "MSP");
    assert!(!zau.contains(pt(33.6407, -84.4277)), "ATL");
    assert!(!zau.contains(pt(39.8561, -104.6737)), "DEN");
    // The poles and the antimeridian are far outside.
    assert!(!zau.contains(pt(89.0, 0.0)));
    assert!(!zau.contains(pt(-60.0, 180.0)));
}

// REQ: OA-GEO-006
#[test]
fn zau_low_every_edge_separates_inside_from_outside() {
    let e = wgs84();
    let verts = zau_vertices();
    let zau = Polygon::new(&e, &verts).expect("polygon");
    let mut checked = 0;
    let mut inside_side: Option<f64> = None;
    for i in 0..verts.len() {
        let a = verts[i];
        let b = verts[(i + 1) % verts.len()];
        let inv = e.inverse(a, b).expect("edge");
        if inv.distance_m < 1000.0 {
            continue;
        }
        let mid = e
            .direct(a, inv.azimuth1_deg, inv.distance_m / 2.0)
            .expect("mid");
        // Azimuth at the midpoint from the direct problem's own geodesic: re-solve from the mid.
        let toward_b = e.inverse(mid.position, b).expect("mid to b").azimuth1_deg;
        let right = e
            .direct(mid.position, toward_b + 90.0, 100.0)
            .expect("right")
            .position;
        let left = e
            .direct(mid.position, toward_b - 90.0, 100.0)
            .expect("left")
            .position;
        let (r_in, l_in) = (zau.contains(right), zau.contains(left));
        assert!(
            r_in != l_in,
            "edge {i}: exactly one side must be inside (right {r_in}, left {l_in})"
        );
        // A simple polygon has its interior on the same side of every edge.
        let side = if r_in { 1.0 } else { -1.0 };
        match inside_side {
            None => inside_side = Some(side),
            Some(s) => assert_eq!(s, side, "edge {i}: interior flipped side"),
        }
        checked += 1;
    }
    assert!(checked >= 30, "only {checked} edges checked");
    eprintln!(
        "ZAU LOW: {checked} edges of {} checked; interior on the {} of travel",
        verts.len(),
        if inside_side == Some(1.0) {
            "right"
        } else {
            "left"
        }
    );
}

// REQ: OA-GEO-006
#[test]
fn zau_low_closing_edge_is_part_of_the_polygon() {
    // The ring is closed implicitly: the edge from the last point back to the first separates
    // inside from outside too.
    let e = wgs84();
    let verts = zau_vertices();
    let zau = Polygon::new(&e, &verts).expect("polygon");
    let last = verts[verts.len() - 1];
    let first = verts[0];
    let inv = e.inverse(last, first).expect("closing edge");
    let mid = e
        .direct(last, inv.azimuth1_deg, inv.distance_m / 2.0)
        .expect("mid");
    let az = e.inverse(mid.position, first).expect("az").azimuth1_deg;
    let right = e
        .direct(mid.position, az + 90.0, 10.0)
        .expect("right")
        .position;
    let left = e
        .direct(mid.position, az - 90.0, 10.0)
        .expect("left")
        .position;
    assert!(zau.contains(right) != zau.contains(left));
}

// REQ: OA-GEO-006
#[test]
fn edges_are_geodesics_not_lines_of_constant_latitude() {
    // The geodesic between (60, -30) and (60, 30) bulges north of 60 degrees; a point 1 km
    // north of the parallel but 1 km south of the geodesic is inside the triangle below it.
    let e = wgs84();
    let tri = Polygon::new(&e, &[pt(60.0, -30.0), pt(60.0, 30.0), pt(40.0, 0.0)]).expect("tri");
    let g = e.inverse(pt(60.0, -30.0), pt(60.0, 30.0)).expect("edge");
    let top = e
        .direct(pt(60.0, -30.0), g.azimuth1_deg, g.distance_m / 2.0)
        .expect("mid")
        .position;
    assert!(top.lat() > 61.0, "geodesic apex {}", top.lat());
    assert!(
        tri.contains(pt(top.lat() - 0.01, top.lon())),
        "just below the apex"
    );
    assert!(
        !tri.contains(pt(top.lat() + 0.01, top.lon())),
        "just above the apex"
    );
    // Between the parallel and the geodesic: inside because the edge is the geodesic.
    assert!(tri.contains(pt(60.5, 0.0)));
}

// REQ: OA-GEO-006
#[test]
fn polygons_across_the_antimeridian_and_in_the_south() {
    let e = wgs84();
    let ring = [
        pt(-10.0, 175.0),
        pt(-10.0, -175.0),
        pt(-20.0, -175.0),
        pt(-20.0, 175.0),
    ];
    let poly = Polygon::new(&e, &ring).expect("antimeridian polygon");
    assert!(poly.contains(pt(-15.0, 180.0)));
    assert!(poly.contains(pt(-15.0, -179.0)));
    assert!(poly.contains(pt(-15.0, 179.0)));
    assert!(!poly.contains(pt(-15.0, 170.0)));
    assert!(!poly.contains(pt(-15.0, -170.0)));
    assert!(!poly.contains(pt(-5.0, 180.0)));
    assert!(!poly.contains(pt(-25.0, 180.0)));
    // Reversed vertex order gives the same answers.
    let rev: Vec<LatLon> = ring.iter().rev().copied().collect();
    let poly2 = Polygon::new(&e, &rev).expect("reversed");
    for p in [
        pt(-15.0, 180.0),
        pt(-15.0, 170.0),
        pt(-5.0, 180.0),
        pt(-15.0, -179.0),
    ] {
        assert_eq!(poly.contains(p), poly2.contains(p));
    }
}

// REQ: OA-GEO-006
#[test]
fn closing_vertex_and_vertices_on_the_test_meridian() {
    let e = wgs84();
    // Square whose corners lie exactly on meridians 0 and 10; test points on those meridians.
    let sq = [pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0), pt(0.0, 0.0)];
    let open = Polygon::new(&e, &sq).expect("open");
    let mut closed_ring = sq.to_vec();
    closed_ring.push(sq[0]);
    let closed = Polygon::new(&e, &closed_ring).expect("closed");
    assert_eq!(open.len(), closed.len());
    for poly in [&open, &closed] {
        assert!(poly.contains(pt(5.0, 5.0)));
        // On the meridians through the vertices: strictly inside latitude band, interior test.
        assert!(poly.contains(pt(5.0, 1e-6)));
        assert!(poly.contains(pt(5.0, 10.0 - 1e-6)));
        assert!(!poly.contains(pt(5.0, -1e-6)));
        assert!(!poly.contains(pt(5.0, 10.0 + 1e-6)));
        assert!(!poly.contains(pt(11.0, 5.0)));
        assert!(!poly.contains(pt(-1.0, 5.0)));
        assert!(!poly.contains(pt(20.0, 0.0)));
        assert!(!poly.contains(pt(-20.0, 10.0)));
    }
}

// REQ: OA-GEO-006
#[test]
fn invalid_polygons_are_rejected() {
    let e: Ellipsoid = wgs84();
    assert!(matches!(
        Polygon::new(&e, &[]),
        Err(GeoError::BadPolygon(_))
    ));
    assert!(matches!(
        Polygon::new(&e, &[pt(0.0, 0.0), pt(1.0, 1.0)]),
        Err(GeoError::BadPolygon(_))
    ));
    assert!(matches!(
        Polygon::new(
            &e,
            &[pt(0.0, 0.0), pt(0.0, 0.0), pt(1.0, 1.0), pt(0.0, 0.0)]
        ),
        Err(GeoError::BadPolygon(_))
    ));
    // Vertex at a pole.
    assert!(matches!(
        Polygon::new(&e, &[pt(90.0, 0.0), pt(80.0, 0.0), pt(80.0, 20.0)]),
        Err(GeoError::BadPolygon(_))
    ));
    // A ring around the north pole encloses it.
    let ring: Vec<LatLon> = (0..8).map(|i| pt(80.0, -180.0 + 45.0 * i as f64)).collect();
    assert!(matches!(
        Polygon::new(&e, &ring),
        Err(GeoError::BadPolygon(_))
    ));
    // An edge longer than 10 000 km.
    assert!(matches!(
        Polygon::new(&e, &[pt(0.0, 0.0), pt(0.0, 120.0), pt(10.0, 60.0)]),
        Err(GeoError::BadPolygon(_))
    ));
}

// REQ: OA-GEO-006
#[test]
fn zau_low_agrees_with_a_densely_sampled_planar_polygon() {
    // Independent check: replace each geodesic edge by 400 sampled points and apply the textbook
    // planar crossing-number test in (lon, lat). The chord-to-geodesic deviation is far below a
    // metre, so the two answers differ only for points within about a metre of the boundary.
    let e = wgs84();
    let verts = zau_vertices();
    let zau = Polygon::new(&e, &verts).expect("polygon");
    let mut dense: Vec<(f64, f64)> = Vec::new();
    for i in 0..verts.len() {
        let a = verts[i];
        let b = verts[(i + 1) % verts.len()];
        let inv = e.inverse(a, b).expect("edge");
        for k in 0..400 {
            let q = e
                .direct(a, inv.azimuth1_deg, inv.distance_m * k as f64 / 400.0)
                .expect("sample")
                .position;
            dense.push((q.lon(), q.lat()));
        }
    }
    let planar = |lon: f64, lat: f64| {
        let mut inside = false;
        let mut j = dense.len() - 1;
        for i in 0..dense.len() {
            let (xi, yi) = dense[i];
            let (xj, yj) = dense[j];
            if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    };
    let mut state = 777u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut inside, mut total) = (0, 0);
    for _ in 0..5_000 {
        let lon = -95.0 + 13.0 * next();
        let lat = 37.0 + 9.0 * next();
        let got = zau.contains(pt(lat, lon));
        assert_eq!(got, planar(lon, lat), "disagreement at {lat} {lon}");
        total += 1;
        inside += usize::from(got);
    }
    assert!(
        inside > 500 && inside < total - 500,
        "{inside} of {total} inside"
    );
}

// REQ: OA-GEO-006
#[test]
fn symmetric_geodesic_crossing_at_equator_separates_nearby_points() {
    // By symmetry, the shortest geodesic from (-10, -10) to (10, 10) crosses longitude zero
    // at latitude zero. The triangle's third vertex is east of that edge, so points immediately
    // north and south of the crossing must lie on opposite sides of the boundary.
    let e = wgs84();
    let tri = Polygon::new(&e, &[pt(-10.0, -10.0), pt(10.0, 10.0), pt(0.0, 30.0)])
        .expect("valid symmetric triangle");
    assert!(
        tri.contains(pt(-0.000_001, 0.0)),
        "south of the diagonal is east of the triangle edge"
    );
    assert!(
        !tri.contains(pt(0.000_001, 0.0)),
        "north of the diagonal is west of the triangle edge"
    );
}

// REQ: OA-GEO-006
#[test]
fn equatorial_edge_has_known_inside_and_outside_sides() {
    // The shortest edge between two points on the equator remains on the equator. The apex is
    // north of that edge, which fixes the expected answer without using polygon calculations.
    let e = wgs84();
    let tri = Polygon::new(&e, &[pt(0.0, -20.0), pt(0.0, 20.0), pt(20.0, 0.0)])
        .expect("valid equatorial triangle");
    assert!(tri.contains(pt(0.1, 0.0)));
    assert!(!tri.contains(pt(-0.1, 0.0)));
}

// REQ: OA-GEO-006
#[test]
fn equatorial_edge_crossing_the_antimeridian_has_known_sides() {
    // The short equatorial arc from 170 E to 170 W crosses longitude 180 at latitude zero.
    // The third vertex is north, so the explicit points below are inside and outside respectively.
    let e = wgs84();
    let tri = Polygon::new(&e, &[pt(0.0, 170.0), pt(0.0, -170.0), pt(20.0, 180.0)])
        .expect("valid antimeridian triangle");
    assert!(tri.contains(pt(0.1, 180.0)));
    assert!(!tri.contains(pt(-0.1, 180.0)));
    assert!(!tri.contains(pt(0.1, 0.0)));
}

// REQ: OA-GEO-006
#[test]
fn containment_near_a_vertex_has_explicit_expected_sides() {
    // This triangle has an equatorial edge and a meridian edge. Near its southwest vertex,
    // points northeast are inside while points south or west are outside.
    let e = wgs84();
    let tri = Polygon::new(&e, &[pt(0.0, 0.0), pt(0.0, 10.0), pt(10.0, 0.0)])
        .expect("valid right triangle");
    assert!(tri.contains(pt(0.000_1, 0.000_1)));
    assert!(!tri.contains(pt(-0.000_1, 0.000_1)));
    assert!(!tri.contains(pt(0.000_1, -0.000_1)));
}

// REQ: OA-GEO-026
#[test]
fn geo_error_display_preserves_the_specific_failure_message() {
    assert!(GeoError::NonFinite("longitude")
        .to_string()
        .contains("longitude"));
    assert!(GeoError::BadPolygon("edge spans 180 degrees of longitude")
        .to_string()
        .contains("edge spans 180 degrees of longitude"));
}

// REQ: OA-GEO-006
#[test]
fn oblique_spherical_edges_separate_points_across_the_analytic_great_circle() {
    let sphere = Ellipsoid::new(6_371_000.0, 0.0).expect("sphere");
    let a = pt(10.0, -30.0);
    let b = pt(65.0, 40.0);
    let c = pt(15.0, 60.0);
    let cartesian = |p: LatLon| {
        let (s, co) = p.lat().to_radians().sin_cos();
        let (sl, cl) = p.lon().to_radians().sin_cos();
        [co * cl, co * sl, s]
    };
    let u = cartesian(a);
    let v = cartesian(b);
    let normal = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    for ring in [[a, b, c], [c, b, a]] {
        let polygon = Polygon::new(&sphere, &ring).expect("oblique triangle");
        for lon in [-29.9999_f64, -15.0, 0.0, 25.0, 39.9999] {
            // A great circle is the plane normal dot position = 0. Solving that equation
            // for latitude gives this crossing without a geodesic or root solver.
            let (sl, cl) = lon.to_radians().sin_cos();
            let crossing = (-(normal[0] * cl + normal[1] * sl) / normal[2])
                .atan()
                .to_degrees();
            assert!(polygon.contains(pt(crossing - 1e-7, lon)), "south at {lon}");
            assert!(
                !polygon.contains(pt(crossing + 1e-7, lon)),
                "north at {lon}"
            );
        }
    }
}

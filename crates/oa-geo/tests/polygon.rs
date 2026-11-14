//! Point-in-polygon with geodesic edges (OA-GEO-006), including the real ZAU LOW boundary.

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

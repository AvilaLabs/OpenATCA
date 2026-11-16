//! Rhumb lines and the geodesic-versus-rhumb separation (OA-GEO-007).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_geo::{wgs84, Ellipsoid, GeoError, LatLon};

fn pt(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).expect("valid")
}

// REQ: OA-GEO-007
#[test]
fn rhumb_along_a_meridian_equals_the_geodesic() {
    let e = wgs84();
    let a = pt(10.0, 20.0);
    let b = pt(65.0, 20.0);
    let r = e.rhumb_inverse(a, b).expect("rhumb");
    let g = e.inverse(a, b).expect("geodesic");
    assert!(
        (r.distance_m - g.distance_m).abs() < 1e-6,
        "{} {}",
        r.distance_m,
        g.distance_m
    );
    assert_eq!(r.course_deg, 0.0);
    let back = e.rhumb_inverse(b, a).expect("rhumb");
    assert_eq!(back.course_deg.abs(), 180.0);
    let sep = e.geodesic_rhumb_separation(a, b, 20).expect("separation");
    assert!(sep.max_m < 1e-6, "{}", sep.max_m);
}

// REQ: OA-GEO-007
#[test]
fn rhumb_along_a_parallel_has_the_parallel_length() {
    let e = wgs84();
    for lat in [0.0, 30.0, 45.0, -60.0] {
        let r = e
            .rhumb_inverse(pt(lat, -10.0), pt(lat, 15.0))
            .expect("rhumb");
        let phi = f64::to_radians(lat);
        let e2 = 0.006_694_379_990_141_317;
        let radius = 6_378_137.0 * phi.cos() / (1.0 - e2 * phi.sin().powi(2)).sqrt();
        let expect = radius * 25f64.to_radians();
        assert!(
            (r.distance_m - expect).abs() < 1e-6,
            "{lat}: {} {expect}",
            r.distance_m
        );
        assert_eq!(r.course_deg, 90.0);
        let w = e
            .rhumb_inverse(pt(lat, 15.0), pt(lat, -10.0))
            .expect("rhumb");
        assert_eq!(w.course_deg, -90.0);
    }
}

// REQ: OA-GEO-007
#[test]
fn rhumb_runs_the_short_way_across_the_antimeridian() {
    let e = wgs84();
    let r = e
        .rhumb_inverse(pt(20.0, 179.0), pt(20.0, -179.0))
        .expect("rhumb");
    let one_way = e
        .rhumb_inverse(pt(20.0, 0.0), pt(20.0, 2.0))
        .expect("rhumb");
    assert!((r.distance_m - one_way.distance_m).abs() < 1e-6);
    assert_eq!(r.course_deg, 90.0);
    let mid = e
        .rhumb_point_at_fraction(pt(20.0, 179.0), pt(20.0, -179.0), 0.5)
        .expect("midpoint");
    assert!((mid.lon().abs() - 180.0).abs() < 1e-9 && (mid.lat() - 20.0).abs() < 1e-9);
}

// REQ: OA-GEO-007
#[test]
fn rhumb_direct_and_inverse_are_consistent() {
    let e = wgs84();
    let cases = [
        (pt(41.9786, -87.9048), pt(51.4775, -0.4614)),
        (pt(-33.9, 151.2), pt(-41.3, 174.8)),
        (pt(0.5, 0.0), pt(-0.5, 0.2)),
        (pt(70.0, 160.0), pt(68.0, -170.0)),
        (pt(35.0, 140.0), pt(35.001, 140.001)),
    ];
    for (a, b) in cases {
        let r = e.rhumb_inverse(a, b).expect("rhumb");
        let d = e
            .rhumb_direct(a, r.course_deg, r.distance_m)
            .expect("direct");
        let miss = e.inverse(d, b).expect("miss").distance_m;
        assert!(miss < 1e-6, "{a:?} {b:?} miss {miss}");
        let half = e.rhumb_point_at_fraction(a, b, 0.5).expect("half");
        let d1 = e.rhumb_inverse(a, half).expect("first half").distance_m;
        let d2 = e.rhumb_inverse(half, b).expect("second half").distance_m;
        assert!((d1 + d2 - r.distance_m).abs() < 1e-6, "{a:?} {b:?}");
        assert!((d1 - d2).abs() < 1e-6, "{a:?} {b:?}");
        // Constant course along the line.
        let c1 = e.rhumb_inverse(a, half).expect("c1").course_deg;
        assert!((c1 - r.course_deg).abs() < 1e-9, "{a:?} {b:?}");
    }
    let a = pt(10.0, 10.0);
    let b = pt(20.0, 30.0);
    assert_eq!(e.rhumb_point_at_fraction(a, b, 0.0).expect("start"), a);
    assert_eq!(e.rhumb_point_at_fraction(a, b, 1.0).expect("end"), b);
    assert!(e.rhumb_point_at_fraction(a, b, 1.5).is_err());
    assert!(e.rhumb_point_at_fraction(a, b, f64::NAN).is_err());
}

// REQ: OA-GEO-007
#[test]
fn rhumb_matches_the_geodesic_locally() {
    // Over a short segment a rhumb line and the geodesic coincide to second order: the course
    // agrees with the geodesic azimuth and the lengths agree to relative 1e-10 over 100 m.
    let e = wgs84();
    for (lat, lon, course) in [
        (41.0, -88.0, 37.0),
        (-20.0, 130.0, 250.0),
        (60.0, 10.0, 91.0),
        (5.0, 0.0, 12.5),
    ] {
        let a = pt(lat, lon);
        let b = e.rhumb_direct(a, course, 100.0).expect("rhumb direct");
        let g = e.inverse(a, b).expect("geodesic");
        assert!(
            (g.distance_m - 100.0).abs() < 1e-7,
            "{lat}: {}",
            g.distance_m
        );
        // The chord's mean azimuth equals the rhumb course to third order (the end azimuths
        // differ from it by half the meridian convergence).
        let mid = g.azimuth1_deg
            + 0.5 * ((g.azimuth2_deg - g.azimuth1_deg + 180.0).rem_euclid(360.0) - 180.0);
        let dc = ((mid - course + 180.0).rem_euclid(360.0) - 180.0).abs();
        assert!(dc < 1e-6, "{lat}: course differs by {dc} deg");
    }
}

// REQ: OA-GEO-007
#[test]
fn rhumb_rejects_poles_and_bad_input() {
    let e = wgs84();
    assert_eq!(
        e.rhumb_inverse(pt(90.0, 0.0), pt(10.0, 0.0)),
        Err(GeoError::RhumbPole)
    );
    assert_eq!(
        e.rhumb_inverse(pt(10.0, 0.0), pt(-90.0, 5.0)),
        Err(GeoError::RhumbPole)
    );
    // Due north far enough to pass the pole.
    assert_eq!(
        e.rhumb_direct(pt(80.0, 0.0), 0.0, 2_000_000.0),
        Err(GeoError::RhumbPole)
    );
    assert!(e.rhumb_direct(pt(80.0, 0.0), f64::NAN, 1.0).is_err());
    assert!(e.rhumb_direct(pt(80.0, 0.0), 0.0, f64::INFINITY).is_err());
    assert!(e
        .geodesic_rhumb_separation(pt(0.0, 0.0), pt(1.0, 1.0), 1)
        .is_err());
    assert!(e
        .geodesic_rhumb_separation(pt(0.0, 0.0), pt(1.0, 1.0), 100_001)
        .is_err());
    // Near the pole it still works and the result is finite.
    let d = e
        .rhumb_direct(pt(89.0, 0.0), 80.0, 50_000.0)
        .expect("near pole");
    assert!(d.lat() > 89.0 && d.lon().is_finite());
}

// REQ: OA-GEO-007
#[test]
fn spherical_oblique_rhumb_matches_closed_form_and_reverses_signs() {
    let radius = 6_371_000.0;
    let sphere = Ellipsoid::new(radius, 0.0).expect("sphere");
    let a = pt(12.0, -35.0);
    let b = pt(47.0, 18.0);
    let (phi1, phi2) = (12.0f64.to_radians(), 47.0f64.to_radians());
    let dphi = phi2 - phi1;
    let dlambda = 53.0f64.to_radians();
    let psi = |phi: f64| (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln();
    let dpsi = psi(phi2) - psi(phi1);
    let expected_distance = radius * (dphi * dphi + (dphi / dpsi * dlambda).powi(2)).sqrt();
    let expected_course = dlambda.atan2(dpsi).to_degrees();

    let forward = sphere.rhumb_inverse(a, b).expect("forward");
    let reverse = sphere.rhumb_inverse(b, a).expect("reverse");
    assert!((forward.distance_m - expected_distance).abs() < 1e-7);
    assert!((forward.course_deg - expected_course).abs() < 1e-12);
    assert!((reverse.distance_m - expected_distance).abs() < 1e-7);
    assert!((reverse.course_deg - (expected_course - 180.0)).abs() < 1e-12);
}

// REQ: OA-GEO-007
#[test]
fn rhumb_exact_antipodal_longitude_tie_is_eastward() {
    let e = wgs84();
    let a = pt(25.0, -40.0);
    let b = pt(25.0, 140.0);
    let r = e.rhumb_inverse(a, b).expect("eastward tie");
    let reverse_tie = e.rhumb_inverse(b, a).expect("reverse tie");
    assert_eq!(r.course_deg, 90.0);
    assert_eq!(reverse_tie.course_deg, 90.0);
    assert_eq!(r.distance_m, reverse_tie.distance_m);
    assert!(r.distance_m > 0.0);
}

// REQ: OA-GEO-027
#[test]
fn oblique_ellipsoidal_rhumbs_match_independent_integrals() {
    // mpmath, 100 decimal digits: integrate M(phi) = a(1-e2)/(1-e2 sin(phi)^2)^(3/2),
    // psi = asinh(tan(phi)) - e atanh(e sin(phi)), course = atan2(dlon, dpsi),
    // length = |dm| sqrt(1 + (dlon/dpsi)^2). No geodesic implementation supplies the answers.
    let cases = [
        (
            (-75.0, -120.0),
            (83.0, 35.0),
            20_133_289.958_898_693,
            29.361_226_572_331_564,
        ),
        (
            (89.0, -170.0),
            (89.8, 160.0),
            93_964.691_603_311_25,
            -18.021_027_254_210_786,
        ),
        (
            (10.0, -70.0),
            (65.0, 40.0),
            10_742_776.338_751_214,
            55.365_872_921_422_174,
        ),
    ];
    let e = wgs84();
    for (a, b, distance, course) in cases {
        let start = pt(a.0, a.1);
        let result = e.rhumb_inverse(start, pt(b.0, b.1)).expect("inverse");
        assert!((result.distance_m - distance).abs() < 1e-3, "{result:?}");
        assert!((result.course_deg - course).abs() < 1e-10, "{result:?}");
        let endpoint = e.rhumb_direct(start, course, distance).expect("direct");
        assert!((endpoint.lat() - b.0).abs() < 1e-8, "{endpoint:?}");
        assert!((endpoint.lon() - b.1).abs() < 1e-8, "{endpoint:?}");
    }
}

/// Closed-form separation on a sphere between the great circle through (phi, -delta) and
/// (phi, +delta) and the parallel (which is the rhumb line there): the parallel is farthest from
/// the great circle at its midpoint, where sin(d / R) = |n . P| with n the unit normal of the
/// great-circle plane.
fn sphere_parallel_separation(radius: f64, lat_deg: f64, half_lon_deg: f64) -> f64 {
    let (phi, delta) = (lat_deg.to_radians(), half_lon_deg.to_radians());
    let nx = -phi.sin();
    let nz = phi.cos() * delta.cos();
    let norm = (nx * nx + nz * nz).sqrt();
    let dot = (nx * phi.cos() + nz * phi.sin()) / norm;
    radius * dot.abs().asin()
}

// REQ: OA-GEO-007
#[test]
fn separation_matches_the_closed_form_on_a_sphere() {
    let r = 6_371_000.0;
    let sphere = Ellipsoid::new(r, 0.0).expect("sphere");
    let mut worst = 0.0f64;
    for (lat, half) in [
        (40.0, 20.0),
        (60.0, 30.0),
        (-25.0, 5.0),
        (10.0, 60.0),
        (75.0, 40.0),
    ] {
        let sep = sphere
            .geodesic_rhumb_separation(pt(lat, -half), pt(lat, half), 24)
            .expect("separation");
        let expect = sphere_parallel_separation(r, lat, half);
        let err = (sep.max_m - expect).abs();
        worst = worst.max(err);
        assert!(
            err < 1e-5,
            "lat {lat} half {half}: got {} want {expect}",
            sep.max_m
        );
        assert!((sep.at_fraction - 0.5).abs() < 1e-6, "{}", sep.at_fraction);
    }
    eprintln!("sphere closed-form separation worst error {worst:e} m");
}

// REQ: OA-GEO-007
#[test]
fn separation_on_wgs84_is_stable_and_symmetric() {
    let e = wgs84();
    // Chicago to London: the rhumb line is far from the geodesic.
    let a = pt(41.9786, -87.9048);
    let b = pt(51.4775, -0.4614);
    let coarse = e.geodesic_rhumb_separation(a, b, 16).expect("coarse");
    let fine = e.geodesic_rhumb_separation(a, b, 160).expect("fine");
    let rev = e.geodesic_rhumb_separation(b, a, 64).expect("reverse");
    eprintln!(
        "ORD-LHR: separation {:.6} m at fraction {:.6}; geodesic {:.3} m, rhumb {:.3} m",
        fine.max_m, fine.at_fraction, fine.geodesic_m, fine.rhumb_m
    );
    assert!(
        fine.max_m > 10_000.0,
        "a transatlantic rhumb is tens of km off the geodesic"
    );
    assert!(fine.rhumb_m > fine.geodesic_m);
    assert!(
        (coarse.max_m - fine.max_m).abs() < 1e-4,
        "{} {}",
        coarse.max_m,
        fine.max_m
    );
    assert!(
        (rev.max_m - fine.max_m).abs() < 1e-3,
        "{} {}",
        rev.max_m,
        fine.max_m
    );
    // A 360 km edge: a couple of kilometres.
    let s = e
        .geodesic_rhumb_separation(pt(42.0, -93.0), pt(43.5, -89.0), 32)
        .expect("short");
    assert!(s.max_m > 100.0 && s.max_m < 5_000.0, "{}", s.max_m);
    // Coincident endpoints.
    let z = e.geodesic_rhumb_separation(a, a, 8).expect("zero");
    assert_eq!(z.max_m, 0.0);
}

// REQ: OA-GEO-007
#[test]
fn rhumb_length_equals_the_sum_of_geodesic_chords() {
    // Independent check of the rhumb length: points along the line from rhumb_point_at_fraction
    // joined by geodesics from the (separately validated) geodesic solver.
    let e = wgs84();
    for (a, b) in [
        (pt(41.9786, -87.9048), pt(51.4775, -0.4614)),
        (pt(-33.9, 151.2), pt(-41.3, 174.8)),
        (pt(64.0, -22.0), pt(64.0, 28.0)),
    ] {
        let r = e.rhumb_inverse(a, b).expect("rhumb");
        let n = 4000;
        let mut prev = a;
        let mut sum = 0.0;
        for i in 1..=n {
            let q = e
                .rhumb_point_at_fraction(a, b, i as f64 / n as f64)
                .expect("point");
            sum += e.inverse(prev, q).expect("chord").distance_m;
            prev = q;
        }
        assert!(
            (sum - r.distance_m).abs() < 0.1,
            "{a:?} {b:?}: chords {sum} rhumb {}",
            r.distance_m
        );
    }
}

// REQ: OA-GEO-007
#[test]
fn the_zau_high_edge_12_separation_is_unchanged_to_a_millimetre() {
    // The edge of the ZAU HIGH boundary with the largest separation. The value is the one the
    // 64-interval scan with golden-section refinement gave before the Newton-type solver
    // replaced it (592.4280445927 m); the two agree to under a nanometre.
    let e = wgs84();
    let sep = e
        .geodesic_rhumb_separation(pt(43.69167, -87.16944), pt(43.58667, -84.96361), 64)
        .expect("separation");
    assert!(
        (sep.max_m - 592.428_044_592_7).abs() < 1e-3,
        "{}",
        sep.max_m
    );
    assert!((sep.geodesic_m - 178_366.501_858_1).abs() < 1e-3);
    assert!((sep.at_fraction - 0.4998).abs() < 1e-3);
}

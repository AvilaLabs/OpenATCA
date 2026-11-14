//! Geodesic inverse and direct problems against GeographicLib's GeodTest data (OA-GEO-003/4/5).
//!
//! `fixtures/geodtest_sample.dat` is a 31-line excerpt that always runs. If the environment
//! variable OPENATCA_GEODTEST names `GeodTest-short.dat` (10,000 lines) the whole set runs too.

use oa_geo::{wgs84, Ellipsoid, LatLon};

const SAMPLE: &str = include_str!("fixtures/geodtest_sample.dat");

struct Case {
    p1: LatLon,
    p2: LatLon,
    azi1: f64,
    azi2: f64,
    s12: f64,
    a12: f64,
}

fn parse(text: &str) -> Vec<Case> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Vec<f64> = l
                .split_whitespace()
                .map(|x| x.parse().expect("number"))
                .collect();
            assert_eq!(v.len(), 10, "GeodTest line has 10 columns");
            Case {
                p1: LatLon::new(v[0], v[1]).expect("lat1 lon1"),
                p2: LatLon::new(v[3], v[4]).expect("lat2 lon2"),
                azi1: v[2],
                azi2: v[5],
                s12: v[6],
                a12: v[7],
            }
        })
        .collect()
}

fn angle_err(a: f64, b: f64) -> f64 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

#[derive(Default, Debug)]
struct Maxima {
    inv_s: f64,
    inv_az_raw: f64,
    inv_az_excess: f64,
    inv_az_well_conditioned: f64,
    dir_pos: f64,
    dir_az2: f64,
}

/// Azimuth tolerance for the inverse problem. Near the end of the geodesic's conjugate point
/// (arc length on the auxiliary sphere close to 180 degrees) the azimuth of a geodesic between
/// two points given as doubles is ill-conditioned: a position rounding of `noise` metres moves
/// the azimuth by about noise / gap, where gap is the distance short of the conjugate point.
/// The tolerance is 1e-8 degrees plus that term with noise = 5e-9 m (the ulp of a longitude
/// near 180 degrees is 3.2e-9 m), with the gap floored at 1 micrometre.
fn azimuth_tolerance(e: &Ellipsoid, c: &Case) -> f64 {
    if c.a12 >= 180.0 {
        // Printed as exactly 180: the symmetric family (azimuths 90 and 90), which is not
        // ill-conditioned and must meet the plain tolerance.
        return 1e-8;
    }
    let gap = ((180.0 - c.a12).to_radians() * e.equatorial_radius()).max(1e-6);
    1e-8 + (5e-9 / gap).to_degrees()
}

fn run(e: &Ellipsoid, cases: &[Case]) -> Maxima {
    let mut m = Maxima::default();
    for (i, c) in cases.iter().enumerate() {
        let r = e.inverse(c.p1, c.p2).expect("inverse converges");
        let tol = azimuth_tolerance(e, c);
        let ds = (r.distance_m - c.s12).abs();
        let da1 = angle_err(r.azimuth1_deg, c.azi1);
        let da2 = angle_err(r.azimuth2_deg, c.azi2);
        m.inv_s = m.inv_s.max(ds);
        m.inv_az_raw = m.inv_az_raw.max(da1).max(da2);
        m.inv_az_excess = m.inv_az_excess.max(da1 - tol).max(da2 - tol);
        if tol < 2e-8 {
            m.inv_az_well_conditioned = m.inv_az_well_conditioned.max(da1).max(da2);
        }
        assert!(ds < 1e-3, "line {i}: distance error {ds} m");
        assert!(
            da1 <= tol && da2 <= tol,
            "line {i}: azimuth errors {da1} {da2} (tol {tol})"
        );

        let d = e.direct(c.p1, c.azi1, c.s12).expect("direct");
        let dpos = e
            .inverse(d.position, c.p2)
            .expect("position error")
            .distance_m;
        let daz = angle_err(d.azimuth2_deg, c.azi2);
        m.dir_pos = m.dir_pos.max(dpos);
        m.dir_az2 = m.dir_az2.max(daz);
        assert!(dpos < 1e-3, "line {i}: direct position error {dpos} m");
        assert!(daz < 1e-8, "line {i}: direct azimuth error {daz} deg");
    }
    m
}

fn full_set() -> Option<Vec<Case>> {
    match std::env::var("OPENATCA_GEODTEST") {
        Ok(path) => {
            let text = std::fs::read_to_string(&path).expect("read OPENATCA_GEODTEST file");
            Some(parse(&text))
        }
        Err(_) => {
            eprintln!("OPENATCA_GEODTEST not set: skipping the full 10,000-line GeodTest run");
            None
        }
    }
}

// REQ: OA-GEO-003
#[test]
fn inverse_matches_geodtest_sample() {
    let e = wgs84();
    let cases = parse(SAMPLE);
    assert!(cases.len() >= 30);
    let m = run(&e, &cases);
    eprintln!("sample maxima: {m:?}");
}

// REQ: OA-GEO-004
#[test]
fn direct_matches_geodtest_sample() {
    let e = wgs84();
    for (i, c) in parse(SAMPLE).iter().enumerate() {
        let d = e.direct(c.p1, c.azi1, c.s12).expect("direct");
        let pos = e.inverse(d.position, c.p2).expect("inverse").distance_m;
        assert!(pos < 1e-3, "line {i}: position error {pos} m");
        assert!(
            angle_err(d.azimuth2_deg, c.azi2) < 1e-8,
            "line {i}: azimuth 2"
        );
    }
}

// REQ: OA-GEO-003, OA-GEO-004, OA-GEO-005
#[test]
fn full_geodtest_short() {
    let Some(cases) = full_set() else { return };
    assert_eq!(cases.len(), 10_000);
    let e = wgs84();
    let m = run(&e, &cases);
    eprintln!("GeodTest-short maxima over {} lines: {m:?}", cases.len());
    assert!(m.inv_s < 1e-3 && m.dir_pos < 1e-3);
    assert!(m.inv_az_well_conditioned < 1e-8);
}

// REQ: OA-GEO-005
#[test]
fn coincident_and_nearly_coincident_points() {
    let e = wgs84();
    for (lat, lon) in [
        (0.0, 0.0),
        (41.9786, -87.9048),
        (-90.0, 0.0),
        (90.0, 10.0),
        (12.3, 180.0),
    ] {
        let p = LatLon::new(lat, lon).expect("valid");
        let r = e.inverse(p, p).expect("coincident");
        assert_eq!(r.distance_m, 0.0, "{lat} {lon}");
        assert!(r.azimuth1_deg.is_finite() && r.azimuth2_deg.is_finite());
    }
    // Separations from 1e-15 to 1e-3 degrees: distance tracks the metric to nanometres.
    let a = LatLon::new(45.0, 10.0).expect("valid");
    for k in 3..16 {
        let dlon = 10f64.powi(-k);
        let b = LatLon::new(45.0, 10.0 + dlon).expect("valid");
        let r = e.inverse(a, b).expect("tiny");
        // a cos(phi)/sqrt(1 - e2 sin^2 phi) per radian of longitude
        let expect = dlon.to_radians() * 6_378_137.0 * 45f64.to_radians().cos()
            / (1.0 - 0.006_694_379_990_14 * 0.5f64).sqrt();
        assert!(
            (r.distance_m - expect).abs() < 1e-9 + expect * 1e-9,
            "dlon 1e-{k}"
        );
        assert!((r.azimuth1_deg - 90.0).abs() < 1e-3);
    }
}

// REQ: OA-GEO-005
#[test]
fn antipodal_and_nearly_antipodal_points_converge() {
    let e = wgs84();
    let pole_to_pole = 20_003_931.458_623;
    for (a, b) in [
        ((0.0, 0.0), (0.0, 180.0)),
        ((90.0, 0.0), (-90.0, 0.0)),
        ((30.0, 10.0), (-30.0, -170.0)),
        ((-45.5, 170.0), (45.5, -10.0)),
        ((0.0, 90.0), (0.0, -90.0)),
    ] {
        let p = LatLon::new(a.0, a.1).expect("valid");
        let q = LatLon::new(b.0, b.1).expect("valid");
        let r = e.inverse(p, q).expect("antipodal converges");
        assert!(
            r.distance_m > 19_900_000.0 && r.distance_m <= 20_037_509.0,
            "{a:?} {b:?} {}",
            r.distance_m
        );
        if a == (0.0, 0.0) || a == (90.0, 0.0) {
            assert!(
                (r.distance_m - pole_to_pole).abs() < 1e-3,
                "{}",
                r.distance_m
            );
        }
    }
    // A sweep of small offsets from the antipode; every pair converges and the geodesic
    // returned actually reaches the second point.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut worst = 0.0f64;
    for _ in 0..400 {
        let lat = next() * 178.0 - 89.0;
        let lon = next() * 360.0 - 180.0;
        let scale = 10f64.powf(-12.0 + 9.0 * next());
        let p = LatLon::new(lat, lon).expect("valid");
        let anti_lon = if lon > 0.0 { lon - 180.0 } else { lon + 180.0 };
        let dlat = (next() - 0.5) * 2.0 * scale;
        let dlon = (next() - 0.5) * 2.0 * scale;
        let q = LatLon::new(
            (-lat + dlat).clamp(-90.0, 90.0),
            (anti_lon + dlon).clamp(-180.0, 180.0),
        )
        .expect("valid");
        let r = e.inverse(p, q).expect("near-antipodal converges");
        let d = e.direct(p, r.azimuth1_deg, r.distance_m).expect("direct");
        let miss = e.inverse(d.position, q).expect("miss").distance_m;
        worst = worst.max(miss);
    }
    eprintln!("near-antipodal round-trip worst miss {worst:e} m");
    assert!(worst < 1e-3, "round trip missed by {worst} m");
}

// REQ: OA-GEO-003, OA-GEO-004
#[test]
fn random_pairs_round_trip_and_reverse_consistently() {
    let e = wgs84();
    let mut state = 12345u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut worst_fwd, mut worst_rev, mut worst_sym) = (0.0f64, 0.0f64, 0.0f64);
    for _ in 0..3000 {
        let p = LatLon::new(next() * 180.0 - 90.0, next() * 360.0 - 180.0).expect("valid");
        let q = LatLon::new(next() * 180.0 - 90.0, next() * 360.0 - 180.0).expect("valid");
        let r = e.inverse(p, q).expect("inverse");
        let back = e.inverse(q, p).expect("reverse inverse");
        worst_sym = worst_sym.max((r.distance_m - back.distance_m).abs());
        let fwd = e.direct(p, r.azimuth1_deg, r.distance_m).expect("direct");
        worst_fwd = worst_fwd.max(e.inverse(fwd.position, q).expect("i").distance_m);
        let rev = e
            .direct(q, r.azimuth2_deg + 180.0, r.distance_m)
            .expect("direct back");
        worst_rev = worst_rev.max(e.inverse(rev.position, p).expect("i").distance_m);
    }
    eprintln!(
        "random pairs: forward {worst_fwd:e} m, reverse {worst_rev:e} m, symmetry {worst_sym:e} m"
    );
    assert!(worst_fwd < 1e-4 && worst_rev < 1e-4 && worst_sym < 1e-6);
}

// REQ: OA-GEO-004
#[test]
fn direct_known_cases() {
    let e = wgs84();
    let origin = LatLon::new(0.0, 0.0).expect("valid");
    // Along the equator the geodesic is the equator: lon = s / a.
    let d = e.direct(origin, 90.0, 1_000_000.0).expect("direct");
    assert!(d.position.lat().abs() < 1e-12);
    assert!((d.position.lon() - (1_000_000.0 / 6_378_137.0f64).to_degrees()).abs() < 1e-11);
    assert!((d.azimuth2_deg - 90.0).abs() < 1e-12);
    // The quarter meridian is 10001965.7293 m.
    let d = e.direct(origin, 0.0, 10_001_965.729_3).expect("direct");
    assert!((d.position.lat() - 90.0).abs() < 1e-9);
    // Negative distance runs backwards; a full-circumference-plus offset wraps.
    let f = e.direct(origin, 45.0, 500_000.0).expect("direct");
    let b = e.direct(origin, 225.0, -500_000.0).expect("direct");
    assert!((f.position.lat() - b.position.lat()).abs() < 1e-12);
    assert!((f.position.lon() - b.position.lon()).abs() < 1e-12);
    let wrapped = e
        .direct(origin, 90.0, 40_075_016.685_578_5 + 1_000_000.0)
        .expect("direct");
    assert!((wrapped.position.lon() - (1_000_000.0 / 6_378_137.0f64).to_degrees()).abs() < 1e-7);
    // Azimuth is reduced modulo 360.
    let g = e.direct(origin, 45.0 + 720.0, 500_000.0).expect("direct");
    assert!((g.position.lat() - f.position.lat()).abs() < 1e-12);
    // Crossing the antimeridian.
    let h = e
        .direct(LatLon::new(0.0, 179.5).expect("valid"), 90.0, 200_000.0)
        .expect("direct");
    assert!(h.position.lon() < -178.0 && h.position.lon() > -179.0);
}

// REQ: OA-GEO-003
#[test]
fn inverse_known_cases_and_azimuth_conventions() {
    let e = wgs84();
    let a = LatLon::new(0.0, 0.0).expect("valid");
    let b = LatLon::new(0.0, 1.0).expect("valid");
    let r = e.inverse(a, b).expect("equator");
    assert!((r.distance_m - 6_378_137.0f64 * 1f64.to_radians()).abs() < 1e-6);
    assert_eq!(r.azimuth1_deg, 90.0);
    assert_eq!(r.azimuth2_deg, 90.0);
    let n = LatLon::new(1.0, 0.0).expect("valid");
    let r = e.inverse(a, n).expect("meridian");
    assert!(
        (r.distance_m - 110_574.388_6).abs() < 1e-3,
        "{}",
        r.distance_m
    );
    assert_eq!(r.azimuth1_deg, 0.0);
    let r = e.inverse(n, a).expect("meridian south");
    assert_eq!(r.azimuth1_deg.abs(), 180.0);
}

// REQ: OA-GEO-003
#[test]
fn ellipsoid_parameters_are_validated() {
    assert!(Ellipsoid::new(6_378_137.0, 1.0 / 298.257_223_563).is_ok());
    assert!(Ellipsoid::new(6_371_000.0, 0.0).is_ok());
    for (a, f) in [
        (0.0, 0.003),
        (-1.0, 0.003),
        (f64::NAN, 0.003),
        (6.4e6, f64::NAN),
        (6.4e6, -0.003),
        (6.4e6, 0.5),
        (f64::INFINITY, 0.003),
    ] {
        assert!(Ellipsoid::new(a, f).is_err(), "{a} {f}");
    }
    let e = wgs84();
    assert_eq!(e.equatorial_radius(), 6_378_137.0);
    assert!((e.flattening() - 1.0 / 298.257_223_563).abs() < 1e-18);
}

//! Runway building, refusal and the occupancy test, on hand-worked runways.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_geo::{wgs84, LatLon};
use oa_runway::occupy::{frame_coords, is_occupied, occupies, Occupancy};
use oa_runway::plane::{plane_new, to_local, Geo, Local, Plane, EXTENT_M, PLANE_ERROR_BOUND_M};
use oa_runway::runway::*;

fn g(lat: f64, lon: f64) -> Geo {
    Geo {
        lat_deg: lat,
        lon_deg: lon,
    }
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn end<'a>(name: &'a str, align: f64, pos: Geo) -> EndInput<'a> {
    EndInput {
        name,
        true_alignment_deg: Some(align),
        position: Some(pos),
        displaced_position: None,
        displaced_len_ft: None,
        elev_ft: None,
        displaced_elev_ft: None,
        declared: DeclaredFt::default(),
        cifp_threshold: None,
    }
}

/// Midway Airport (KMDW) runway 04L/22R, real FAA NASR values from the 2026/10/29 cycle
/// (APT_RWY_END; typed from the public airport data in the repository's NASR fixture
/// `oa-airspace/tests/fixtures/nasr/APT_RWY_END.csv`). Both ends are displaced. The length 5,507 ft
/// equals the published TORA and the end-to-end distance; the width 150 ft is the published
/// APT_RWY width, typed here.
fn mdw() -> (Plane, RunwayInput<'static>) {
    let plane = plane_new(41.7868, -87.7524).unwrap();
    let a = EndInput {
        name: "04L",
        true_alignment_deg: Some(43.0),
        position: Some(g(41.7813685, -87.76119458)),
        displaced_position: Some(g(41.78287669, -87.75928211)),
        displaced_len_ft: Some(758.0),
        elev_ft: Some(618.5),
        displaced_elev_ft: Some(617.1),
        declared: DeclaredFt {
            tora: Some(5507.0),
            toda: Some(5507.0),
            asda: Some(5507.0),
            lda: Some(4749.0),
        },
        cifp_threshold: None,
    };
    let b = EndInput {
        name: "22R",
        true_alignment_deg: Some(223.0),
        position: Some(g(41.79233711, -87.74730236)),
        displaced_position: Some(g(41.79058855, -87.74951872)),
        displaced_len_ft: Some(878.0),
        elev_ft: Some(605.2),
        displaced_elev_ft: Some(605.6),
        declared: DeclaredFt {
            tora: Some(5507.0),
            toda: Some(5507.0),
            asda: Some(5507.0),
            lda: Some(4629.0),
        },
        cifp_threshold: None,
    };
    (
        plane,
        RunwayInput {
            length_ft: Some(5507.0),
            width_ft: Some(150.0),
            a,
            b,
        },
    )
}

/// Austin-Bergstrom (KAUS) runway 18L/36R, real FAA NASR values, cycle 2026/10/29 (APT_RWY and
/// APT_RWY_END, from the public airport data): 9,000 x 150 ft, no displacement. 18L is the runway
/// of the 4 February 2023 incident.
fn kaus() -> (Plane, RunwayInput<'static>) {
    let plane = plane_new(30.1945, -97.6699).unwrap();
    let mut a = end("18L", 179.0, g(30.20383005, -97.65789105));
    a.elev_ft = Some(491.6);
    let mut b = end("36R", 359.0, g(30.17909102, -97.65724333));
    b.elev_ft = Some(473.6);
    (
        plane,
        RunwayInput {
            length_ft: Some(9000.0),
            width_ft: Some(150.0),
            a,
            b,
        },
    )
}

/// San Francisco (KSFO) runway 10L/28R, real FAA NASR values, cycle 2026/10/29 (APT_RWY and
/// APT_RWY_END, from the public airport data): 11,870 x 200 ft. 28R has a displaced threshold
/// of 300 ft; 10L has none.
fn ksfo() -> (Plane, RunwayInput<'static>) {
    let plane = plane_new(37.6213, -122.3790).unwrap();
    let mut a = end("10L", 118.0, g(37.62873872, -122.39339186));
    a.elev_ft = Some(5.5);
    let mut b = end("28R", 298.0, g(37.61353361, -122.35714111));
    b.elev_ft = Some(13.0);
    b.displaced_position = Some(g(37.61391813, -122.35805769));
    b.displaced_len_ft = Some(300.0);
    (
        plane,
        RunwayInput {
            length_ft: Some(11_870.0),
            width_ft: Some(200.0),
            a,
            b,
        },
    )
}

// REQ: OA-RWY-002
#[test]
fn midway_displaced_thresholds_keep_the_physical_ends_apart() {
    let (plane, input) = mdw();
    let rw = build_runway(&plane, &input).unwrap();
    // Physical ends 5,507 ft apart; thresholds displaced 758 and 878 ft inside them.
    assert!(near(rw.length_m, 5507.0 * FT_TO_M, 1e-9));
    assert!(near(rw.half_len_m * 2.0, 5507.0 * FT_TO_M, 5.0));
    assert!(near(rw.ends[0].displacement_m, 758.0 * FT_TO_M, 5.0));
    assert!(near(rw.ends[1].displacement_m, 878.0 * FT_TO_M, 5.0));
    assert!(rw.ends[0].physical != rw.ends[0].threshold);
    assert_eq!(rw.ends[0].physical, g(41.7813685, -87.76119458));
    assert_eq!(rw.ends[0].threshold, g(41.78287669, -87.75928211));
    assert!(!rw.ends[0].threshold_from_cifp);
    assert_eq!(&rw.ends[0].name, b"04L");
    assert_eq!(rw.ends[0].elev_ft, Some(618.5));
    // Declared distances in metres: LDA 4,749 ft = 5,507 - 758.
    assert!(near(
        rw.ends[0].declared.lda.unwrap(),
        4749.0 * FT_TO_M,
        1e-9
    ));
    assert!(near(rw.width_m, 150.0 * FT_TO_M, 1e-9));
    // The areas start at the landing threshold, not the physical end: end a's approach area
    // ends 758 ft inside the physical end.
    let thr_a = -rw.half_len_m + rw.ends[0].displacement_m;
    assert!(near(rw.approach_along[0][1], thr_a, 1e-6));
    assert!(near(
        rw.approach_along[0][0],
        thr_a - APPROACH_LENGTH_M,
        1e-6
    ));
    // The departure area starts at the physical end.
    assert!(near(rw.departure_along[0][1], -rw.half_len_m, 1e-6));
}

// REQ: OA-RWY-002
#[test]
fn midway_geometry_is_hand_worked() {
    // 04L physical end to 22R physical end: 41.7813685,-87.76119458 to 41.79233711,-87.74730236:
    // 1219.7 m north, 1153.7 m east at this latitude: 1678.6 m, azimuth 43.4 degrees.
    let (plane, input) = mdw();
    let rw = build_runway(&plane, &input).unwrap();
    assert!(near(rw.half_len_m, 839.3, 1.0));
    assert!(near(rw.ux, 0.6875, 0.005));
    assert!(near(rw.uy, 0.7262, 0.005));
    assert!(near(rw.ux * rw.ux + rw.uy * rw.uy, 1.0, 1e-12));
    assert!(near(rw.half_width_m, 22.86, 1e-9));
}

// REQ: OA-RWY-002
#[test]
fn austin_and_san_francisco_geometry() {
    for (plane, input, len_ft, width_ft, displaced_b) in [
        {
            let (p, i) = kaus();
            (p, i, 9_000.0, 150.0, false)
        },
        {
            let (p, i) = ksfo();
            (p, i, 11_870.0, 200.0, true)
        },
    ] {
        let rw = build_runway(&plane, &input).unwrap();
        assert!(near(rw.length_m, len_ft * FT_TO_M, 1e-9));
        assert!(near(rw.width_m, width_ft * FT_TO_M, 1e-9));
        // Physical ends are the thresholds where nothing is displaced.
        assert_eq!(rw.ends[0].physical, rw.ends[0].threshold);
        assert!(rw.ends[0].displacement_m.abs() < 1.0);
        if displaced_b {
            assert!(rw.ends[1].physical != rw.ends[1].threshold);
            assert!(near(rw.ends[1].displacement_m, 300.0 * FT_TO_M, 5.0));
        } else {
            assert_eq!(rw.ends[1].physical, rw.ends[1].threshold);
        }
        assert!(near(rw.half_len_m * 2.0, len_ft * FT_TO_M, 30.0));
        let o = occupies(&rw, rw.center, 0.0);
        assert!(o.on_runway);
        let past = Local {
            east_m: rw.center.east_m + rw.ux * (rw.half_len_m + 5.0),
            north_m: rw.center.north_m + rw.uy * (rw.half_len_m + 5.0),
        };
        let o = occupies(&rw, past, 0.0);
        assert!(!o.on_runway && o.departure[1]);
        // End b's approach area starts at its landing threshold: inside the physical end by the
        // displacement.
        assert!(near(
            rw.approach_along[1][0],
            rw.half_len_m - rw.ends[1].displacement_m,
            1e-6
        ));
        assert!(o.approach[1]);
    }
}

// REQ: OA-RWY-003
#[test]
fn cifp_threshold_takes_the_place_of_a_missing_displaced_coordinate() {
    // OA-ASP-067: a displacement and no NASR displaced coordinate; the CIFP threshold is used.
    let (plane, mut input) = mdw();
    let cifp = input.a.displaced_position.take().unwrap();
    input.a.cifp_threshold = Some(cifp);
    let rw = build_runway(&plane, &input).unwrap();
    assert!(rw.ends[0].threshold_from_cifp);
    assert_eq!(rw.ends[0].threshold, cifp);
    assert!(near(rw.ends[0].displacement_m, 758.0 * FT_TO_M, 5.0));
    // With no CIFP threshold either, the runway is refused: never the physical end as a guess.
    input.a.cifp_threshold = None;
    assert_eq!(build_runway(&plane, &input), Err(Refusal::ThresholdUnknown));
}

// REQ: OA-RWY-003
#[test]
fn cifp_threshold_fills_an_end_with_no_nasr_coordinate() {
    let (plane, mut input) = mdw();
    input.a.displaced_position = None;
    input.a.displaced_len_ft = Some(0.0);
    input.a.cifp_threshold = input.a.position;
    input.a.position = None;
    // The CIFP point is the physical end here (explicitly not displaced): accepted, marked as
    // CIFP.
    let rw = build_runway(&plane, &input).unwrap();
    assert!(rw.ends[0].threshold_from_cifp);
    assert_eq!(rw.ends[0].physical, rw.ends[0].threshold);
    // Without the explicit zero the displacement is not known, and the CIFP point does not
    // stand as the end.
    input.a.displaced_len_ft = None;
    assert_eq!(
        build_runway(&plane, &input),
        Err(Refusal::PhysicalEndMissing)
    );
    // Nor does a displacement above zero.
    input.a.displaced_len_ft = Some(758.0);
    assert_eq!(
        build_runway(&plane, &input),
        Err(Refusal::PhysicalEndMissing)
    );
}

// REQ: OA-RWY-004
#[test]
fn a_runway_with_a_missing_or_inconsistent_value_is_refused_and_counted() {
    let (plane, good) = mdw();
    let mut counts = RefusalCounts::default();
    assert!(build_counted(&plane, &good, &mut counts).is_some());

    let mut cases: Vec<(RunwayInput<'static>, Refusal)> = Vec::new();
    let mut c = good;
    c.width_ft = None;
    cases.push((c, Refusal::BadWidth));
    let mut c = good;
    c.width_ft = Some(0.0);
    cases.push((c, Refusal::BadWidth));
    let mut c = good;
    c.width_ft = Some(f64::NAN);
    cases.push((c, Refusal::BadWidth));
    let mut c = good;
    c.width_ft = Some(900.0);
    cases.push((c, Refusal::WidthImplausible));
    let mut c = good;
    c.length_ft = None;
    cases.push((c, Refusal::BadLength));
    let mut c = good;
    c.a.name = "";
    cases.push((c, Refusal::BadEndName));
    let mut c = good;
    c.a.displaced_len_ft = Some(-5.0);
    cases.push((c, Refusal::BadDisplacement));
    let mut c = good;
    c.b.position = None;
    cases.push((c, Refusal::PhysicalEndMissing));
    let mut c = good;
    c.a.displaced_position = Some(g(95.0, 0.0));
    cases.push((c, Refusal::BadCoordinate));
    let mut c = good;
    c.a.position = Some(g(41.9, -87.75));
    c.a.displaced_position = None;
    c.a.displaced_len_ft = None;
    cases.push((c, Refusal::LengthMismatch));
    let mut c = good;
    c.a.true_alignment_deg = Some(46.0);
    cases.push((c, Refusal::AlignmentMismatch));
    let mut c = good;
    c.b.true_alignment_deg = Some(f64::NAN);
    cases.push((c, Refusal::AlignmentMismatch));
    let mut c = good;
    c.a.displaced_len_ft = Some(1_200.0);
    cases.push((c, Refusal::DisplacementMismatch));
    let mut c = good;
    // A threshold 60 m off the line.
    c.a.displaced_position = Some(g(41.78287669, -87.75858211));
    cases.push((c, Refusal::ThresholdOffRunway));
    let mut c = good;
    c.b.position = c.a.position;
    c.b.displaced_position = c.a.position;
    c.b.displaced_len_ft = Some(0.0);
    cases.push((c, Refusal::Degenerate));
    let mut c = good;
    c.b.position = Some(g(41.9, -87.0));
    cases.push((c, Refusal::OutsideExtent));

    for (input, why) in &cases {
        assert_eq!(build_runway(&plane, input), Err(*why), "{why:?}");
        assert!(build_counted(&plane, input, &mut counts).is_none());
    }
    assert_eq!(counts.built, 1);
    for r in REFUSALS {
        let n = cases.iter().filter(|(_, w)| *w == r).count() as u32;
        assert_eq!(refusal_count(&counts, r), n, "{r:?}");
    }
    assert_eq!(
        counts.refused.iter().sum::<u32>() as usize,
        cases.len(),
        "every refusal is counted once"
    );
}

// REQ: OA-RWY-004
#[test]
fn displacement_inputs_that_cannot_be_checked_are_refused() {
    let (plane, good) = mdw();
    let mut counts = RefusalCounts::default();
    // (a) A displaced coordinate with no displacement length.
    let mut c = good;
    c.a.displaced_len_ft = None;
    assert_eq!(
        build_counted_err(&plane, &c, &mut counts),
        Refusal::DisplacementMissing
    );
    // (b) Physical end missing, displacement unknown.
    let mut c = good;
    c.a.displaced_position = None;
    c.a.displaced_len_ft = None;
    c.a.cifp_threshold = c.a.position;
    c.a.position = None;
    assert_eq!(
        build_counted_err(&plane, &c, &mut counts),
        Refusal::PhysicalEndMissing
    );
    // (c) A NASR displaced coordinate and a CIFP threshold 60 m apart: refused. 10 m apart:
    // accepted, the NASR coordinate stands.
    let nasr = good.a.displaced_position.unwrap();
    let mut c = good;
    c.a.cifp_threshold = Some(g(nasr.lat_deg + 0.00054, nasr.lon_deg));
    assert_eq!(
        build_counted_err(&plane, &c, &mut counts),
        Refusal::ThresholdDisagreement
    );
    let mut c = good;
    c.a.cifp_threshold = Some(g(nasr.lat_deg + 0.00009, nasr.lon_deg));
    let rw = build_runway(&plane, &c).unwrap();
    assert_eq!(rw.ends[0].threshold, nasr);
    assert!(!rw.ends[0].threshold_from_cifp);
    assert_eq!(refusal_count(&counts, Refusal::DisplacementMissing), 1);
    assert_eq!(refusal_count(&counts, Refusal::PhysicalEndMissing), 1);
    assert_eq!(refusal_count(&counts, Refusal::ThresholdDisagreement), 1);
}

fn build_counted_err(
    plane: &Plane,
    input: &RunwayInput<'_>,
    counts: &mut RefusalCounts,
) -> Refusal {
    assert!(build_counted(plane, input, counts).is_none());
    build_runway(plane, input).unwrap_err()
}

// REQ: OA-RWY-004
#[test]
fn a_threshold_beyond_the_far_end_or_outside_the_near_end_is_refused() {
    let (plane, good) = mdw();
    // The runway axis runs about (0.6875 east, 0.7262 north): 83,000 m per degree of longitude
    // and 111,000 m per degree of latitude here.
    let a = good.a.position.unwrap();
    let along = |m: f64| {
        g(
            a.lat_deg + 0.7262 * m / 111_000.0,
            a.lon_deg + 0.6875 * m / 83_000.0,
        )
    };
    // 1,800 m along from end a: past end b, 1,678 m away.
    let mut c = good;
    c.a.displaced_position = Some(along(1_800.0));
    assert_eq!(build_runway(&plane, &c), Err(Refusal::ThresholdOffRunway));
    // 25 m behind end a: more than 15 m outside the near end.
    let mut c = good;
    c.a.displaced_position = Some(along(-25.0));
    assert_eq!(build_runway(&plane, &c), Err(Refusal::ThresholdOffRunway));
    // 10 m behind is within the allowance (the displacement check then decides).
    let mut c = good;
    c.a.displaced_position = Some(along(-10.0));
    c.a.displaced_len_ft = Some(-10.0 / FT_TO_M * 0.0);
    assert_ne!(build_runway(&plane, &c), Err(Refusal::ThresholdOffRunway));
}

// REQ: OA-RWY-005
#[test]
fn the_tolerances_are_the_stated_constants() {
    assert_eq!(LENGTH_TOLERANCE_M, 60.0);
    assert_eq!(ALIGNMENT_TOLERANCE_DEG, 2.0);
    assert_eq!(DISPLACEMENT_TOLERANCE_M, 30.0);
    assert_eq!(THRESHOLD_OFF_AXIS_M, 15.0);
    let (plane, good) = mdw();
    // A published length 55 m short is accepted, 65 m short is refused (5,507 ft = 1678.5 m).
    let mut c = good;
    c.length_ft = Some(5507.0 - 55.0 / FT_TO_M);
    assert!(build_runway(&plane, &c).is_ok());
    c.length_ft = Some(5507.0 - 65.0 / FT_TO_M);
    assert_eq!(build_runway(&plane, &c), Err(Refusal::LengthMismatch));
    // Alignment 1.5 degrees off the 43.4 degree azimuth is accepted, 3 degrees is refused.
    let mut c = good;
    c.a.true_alignment_deg = Some(44.8);
    assert!(build_runway(&plane, &c).is_ok());
    c.a.true_alignment_deg = Some(47.0);
    assert_eq!(build_runway(&plane, &c), Err(Refusal::AlignmentMismatch));
    // A displacement 20 m off is accepted, 40 m off is refused.
    let mut c = good;
    c.a.displaced_len_ft = Some(758.0 + 20.0 / FT_TO_M);
    assert!(build_runway(&plane, &c).is_ok());
    c.a.displaced_len_ft = Some(758.0 + 40.0 / FT_TO_M);
    assert_eq!(build_runway(&plane, &c), Err(Refusal::DisplacementMismatch));
}

// REQ: OA-RWY-004
#[test]
fn refusal_index_is_the_position_in_the_reason_list() {
    for (i, r) in REFUSALS.iter().enumerate() {
        assert_eq!(refusal_index(*r), i);
    }
}

#[test]
fn end_names() {
    assert_eq!(end_name("10L"), Some(*b"10L"));
    assert_eq!(end_name("N"), Some([b'N', 0, 0]));
    assert_eq!(end_name(""), None);
    assert_eq!(end_name("10LL"), None);
    assert_eq!(end_name("1 L"), None);
}

// REQ: OA-RWY-001
#[test]
fn plane_new_refuses_what_it_cannot_represent() {
    assert!(plane_new(f64::NAN, 0.0).is_none());
    assert!(plane_new(81.0, 0.0).is_none());
    assert!(plane_new(-80.5, 0.0).is_none());
    assert!(plane_new(40.0, 181.0).is_none());
    assert!(plane_new(80.0, 180.0).is_some());
    let p = plane_new(40.0, -100.0).unwrap();
    assert_eq!(
        to_local(&p, g(40.0, -100.0)),
        Some(Local {
            east_m: 0.0,
            north_m: 0.0
        })
    );
    assert!(to_local(&p, g(f64::NAN, -100.0)).is_none());
    assert!(to_local(&p, g(40.5, -100.0)).is_none());
    // Across the antimeridian the short way round.
    let q = plane_new(52.0, 179.95).unwrap();
    let l = to_local(&q, g(52.0, -179.95)).unwrap();
    assert!(near(l.east_m, 0.1 * 68_500.0, 100.0));
}

// REQ: OA-RWY-001
#[test]
fn plane_error_against_geodesic_is_within_the_stated_bound() {
    let ell = wgs84();
    let mut worst = (0.0f64, 0.0, 0.0, 0.0);
    for lat0 in [
        0.0, 25.0, 40.0, 52.0, 64.0, 72.0, 78.0, 80.0, -34.0, -55.0, -80.0,
    ] {
        let plane = plane_new(lat0, 10.0).unwrap();
        let o = LatLon::new(lat0, 10.0).unwrap();
        // Every 5 degrees, so the 45 degree diagonals are in, out to the edge of the disc.
        for k in 0..72 {
            let az = k as f64 * 5.0;
            for dist in [
                1_000.0,
                5_000.0,
                10_000.0,
                15_000.0,
                19_000.0,
                EXTENT_M - 5.0,
            ] {
                let q = ell.direct(o, az, dist).unwrap().position;
                let l = to_local(&plane, g(q.lat(), q.lon())).unwrap();
                let d = (l.east_m * l.east_m + l.north_m * l.north_m).sqrt();
                if (d - dist).abs() > worst.0 {
                    worst = ((d - dist).abs(), lat0, az, dist);
                }
            }
        }
    }
    eprintln!("plane worst distance-from-origin error {worst:?}");
    assert!(
        worst.0 <= PLANE_ERROR_BOUND_M,
        "worst distance error {worst:?}"
    );
    // Past the disc: the corner of the old square is refused.
    let plane = plane_new(40.0, 10.0).unwrap();
    let o = LatLon::new(40.0, 10.0).unwrap();
    let q = ell.direct(o, 45.0, EXTENT_M + 100.0).unwrap().position;
    assert!(to_local(&plane, g(q.lat(), q.lon())).is_none());
    let q = ell.direct(o, 45.0, EXTENT_M - 100.0).unwrap().position;
    assert!(to_local(&plane, g(q.lat(), q.lon())).is_some());
}

// REQ: OA-RWY-009
#[test]
fn every_region_flag_at_a_smaller_radius_stays_set_at_a_larger_one() {
    let rw = mdw_runway();
    let radii = [0.0, 0.5, 3.0, 40.0, 400.0, 2_000.0];
    for i in 0..5 {
        let (lo, hi, w) = region_box(&rw, i);
        // Points inside, on and outside every edge of the region, and far away.
        for along in [
            lo - 900.0,
            lo - 1.0,
            lo,
            (lo + hi) / 2.0,
            hi,
            hi + 1.0,
            hi + 900.0,
        ] {
            for across in [-w - 900.0, -w - 1.0, -w, 0.0, w, w + 1.0, w + 900.0] {
                let p = at(&rw, along, across);
                for pair in radii.windows(2) {
                    let (small, large) = (occupies(&rw, p, pair[0]), occupies(&rw, p, pair[1]));
                    assert!(!flag(&small, i) || flag(&large, i), "{i} {along} {across}");
                }
            }
        }
    }
}

// REQ: OA-RWY-010
#[test]
fn a_position_radius_or_runway_that_is_not_known_occupies_every_region_whatever_the_rest() {
    let rw = mdw_runway();
    let beyond = 1.0e9 * 1.000_001;
    let bad_positions = [
        (f64::NAN, 0.0),
        (0.0, f64::NEG_INFINITY),
        (beyond, 0.0),
        (0.0, -beyond),
        (f64::MAX, f64::MIN),
    ];
    for r in [0.0, 5.0, 1.0e6] {
        for (e, n) in bad_positions {
            let o = occupies(
                &rw,
                Local {
                    east_m: e,
                    north_m: n,
                },
                r,
            );
            assert!(
                o.unknown && o.on_runway && o.approach == [true; 2] && o.departure == [true; 2]
            );
        }
    }
    let inside = at(&rw, 0.0, 0.0);
    for r in [f64::NAN, f64::INFINITY, -f64::MIN_POSITIVE, -3.0] {
        let o = occupies(&rw, inside, r);
        assert!(o.unknown && o.on_runway && o.approach == [true; 2] && o.departure == [true; 2]);
    }
    // A runway with a non-finite number, at every kind of position and radius.
    let mut broken = rw;
    broken.half_len_m = f64::NAN;
    for p in [
        inside,
        at(&rw, 9_000.0, 0.0),
        Local {
            east_m: f64::NAN,
            north_m: 0.0,
        },
    ] {
        for r in [0.0, 10.0, f64::NAN] {
            let o = occupies(&broken, p, r);
            assert!(
                o.unknown && o.on_runway && o.approach == [true; 2] && o.departure == [true; 2]
            );
        }
    }
}

// REQ: OA-RWY-011
#[test]
fn a_target_inside_the_runway_rectangle_touches_it_at_every_radius() {
    let rw = mdw_runway();
    let (lo, hi, w) = region_box(&rw, 0);
    for along in [lo, lo + 0.001, 0.0, hi - 0.001, hi] {
        for across in [-w, -w + 0.001, 0.0, w - 0.001, w] {
            for r in [0.0, 1.0, 50.0, 5_000.0] {
                let o = occupies(&rw, at(&rw, along, across), r);
                assert!(o.on_runway && !o.unknown, "{along} {across} {r}");
            }
        }
    }
}

// REQ: OA-RWY-013
#[test]
fn a_point_in_the_local_plane_is_finite_and_inside_the_extent_or_refused() {
    for (lat0, lon0) in [
        (0.0, 0.0),
        (40.0, -100.0),
        (52.0, 179.95),
        (-34.0, 151.0),
        (80.0, 10.0),
    ] {
        let plane = plane_new(lat0, lon0).unwrap();
        let (mut some, mut none) = (0u32, 0u32);
        for i in -40..=40 {
            for j in -40..=40 {
                let p = g(lat0 + f64::from(i) * 0.01, lon0 + f64::from(j) * 0.01);
                match to_local(&plane, p) {
                    Some(l) => {
                        some += 1;
                        assert!(l.east_m.is_finite() && l.north_m.is_finite());
                        assert!(l.east_m.abs() <= EXTENT_M && l.north_m.abs() <= EXTENT_M);
                    }
                    None => none += 1,
                }
            }
        }
        assert!(some > 100 && none > 100, "{lat0} {some} {none}");
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(to_local(&plane, g(bad, lon0)).is_none());
            assert!(to_local(&plane, g(lat0, bad)).is_none());
        }
    }
}

// REQ: OA-RWY-001
#[test]
fn plane_distance_between_two_points_against_geodesic_is_within_twice_the_bound() {
    // End to end: two points anywhere in the disc (so up to twice the extent apart), the plane's
    // distance between them against the geodesic distance. A fixed pseudo-random sequence.
    let ell = wgs84();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut worst = (0.0f64, 0.0, 0.0);
    for lat0 in [0.0, 30.0, 45.0, 60.0, 70.0, 80.0, -45.0] {
        let plane = plane_new(lat0, -100.0).unwrap();
        let o = LatLon::new(lat0, -100.0).unwrap();
        for _ in 0..400 {
            let mut pts = [Local {
                east_m: 0.0,
                north_m: 0.0,
            }; 2];
            let mut lls = [o; 2];
            for i in 0..2 {
                let az = next() * 360.0;
                let dist = EXTENT_M * next().sqrt();
                let q = ell.direct(o, az, dist).unwrap().position;
                lls[i] = q;
                pts[i] = to_local(&plane, g(q.lat(), q.lon())).unwrap();
            }
            let truth = ell.inverse(lls[0], lls[1]).unwrap().distance_m;
            let d = ((pts[0].east_m - pts[1].east_m).powi(2)
                + (pts[0].north_m - pts[1].north_m).powi(2))
            .sqrt();
            if (d - truth).abs() > worst.0 {
                worst = ((d - truth).abs(), lat0, truth);
            }
        }
    }
    eprintln!("plane worst pair-distance error {worst:?}");
    assert!(worst.0 <= 2.0 * PLANE_ERROR_BOUND_M, "{worst:?}");
}

// Occupancy, on Midway: the runway runs along (ux, uy) from 04L to 22R.
fn at(rw: &Runway, along: f64, across: f64) -> Local {
    // `across` is to the left of the direction of travel, as in the crate: across = dy ux - dx uy.
    Local {
        east_m: rw.center.east_m + along * rw.ux - across * rw.uy,
        north_m: rw.center.north_m + along * rw.uy + across * rw.ux,
    }
}

fn mdw_runway() -> Runway {
    let (plane, input) = mdw();
    build_runway(&plane, &input).unwrap()
}

#[test]
fn at_to_frame_round_trips() {
    let rw = mdw_runway();
    let (a, c) = frame_coords(&rw, at(&rw, 123.0, -45.0)).unwrap();
    assert!(near(a, 123.0, 1e-9) && near(c, -45.0, 1e-9));
}

type EdgeFn<'a> = Box<dyn Fn(f64) -> Local + 'a>;
type Breaker = fn(&mut Runway);

/// The flag of region `i` (0 runway, 1 and 2 the approach areas, 3 and 4 the departure areas).
fn flag(o: &Occupancy, i: usize) -> bool {
    match i {
        0 => o.on_runway,
        1 => o.approach[0],
        2 => o.approach[1],
        3 => o.departure[0],
        _ => o.departure[1],
    }
}

/// `(lo, hi, half_width)` of region `i` from the runway's own numbers.
fn region_box(rw: &Runway, i: usize) -> (f64, f64, f64) {
    match i {
        0 => (-rw.half_len_m, rw.half_len_m, rw.half_width_m),
        1 => (
            rw.approach_along[0][0],
            rw.approach_along[0][1],
            rw.approach_half_width_m,
        ),
        2 => (
            rw.approach_along[1][0],
            rw.approach_along[1][1],
            rw.approach_half_width_m,
        ),
        3 => (
            rw.departure_along[0][0],
            rw.departure_along[0][1],
            rw.departure_half_width_m,
        ),
        _ => (
            rw.departure_along[1][0],
            rw.departure_along[1][1],
            rw.departure_half_width_m,
        ),
    }
}

// REQ: OA-RWY-006
#[test]
fn the_region_intervals_are_the_hand_worked_ones_on_a_runway_displaced_at_both_ends() {
    // Midway: half length 839.3 m; 04L (end a) displaced 758 ft = 231.04 m, 22R (end b)
    // displaced 878 ft = 267.61 m. Along coordinates run from a (negative) to b (positive).
    let rw = mdw_runway();
    let hl = rw.half_len_m;
    assert!(near(hl, 839.3, 1.0));
    let thr_a = -hl + 758.0 * FT_TO_M;
    let thr_b = hl - 878.0 * FT_TO_M;
    assert!(near(thr_a, -608.3, 1.5) && near(thr_b, 571.7, 1.5));
    let eps = 5.0;
    // Approach areas: from each landing threshold outward by the approach length.
    assert!(near(rw.approach_along[0][1], thr_a, eps));
    assert!(near(
        rw.approach_along[0][0],
        thr_a - APPROACH_LENGTH_M,
        eps
    ));
    assert!(near(rw.approach_along[1][0], thr_b, eps));
    assert!(near(
        rw.approach_along[1][1],
        thr_b + APPROACH_LENGTH_M,
        eps
    ));
    // Departure areas: from each physical end outward by the departure length.
    assert!(near(rw.departure_along[0][1], -hl, 1e-9));
    assert!(near(
        rw.departure_along[0][0],
        -hl - DEPARTURE_LENGTH_M,
        1e-9
    ));
    assert!(near(rw.departure_along[1][0], hl, 1e-9));
    assert!(near(
        rw.departure_along[1][1],
        hl + DEPARTURE_LENGTH_M,
        1e-9
    ));
    assert!(near(
        rw.approach_half_width_m,
        rw.half_width_m + APPROACH_SIDE_MARGIN_M,
        1e-9
    ));
    assert!(near(
        rw.departure_half_width_m,
        rw.half_width_m + DEPARTURE_SIDE_MARGIN_M,
        1e-9
    ));
}

// REQ: OA-RWY-006
#[test]
fn every_edge_of_every_region_at_the_interval_ends() {
    let rw = mdw_runway();
    // The radius the test adds to r95 itself: the plane error bound and the rounding pad.
    let pad = PLANE_ERROR_BOUND_M + 1e-6;
    let gap = 0.5;
    for i in 0..5 {
        let (lo, hi, w) = region_box(&rw, i);
        let mid = (lo + hi) / 2.0;
        // Each edge as (point on the edge's outward side at distance `d`, where d = 0 is on it).
        let edges: [(&str, EdgeFn); 4] = [
            ("lo", Box::new(|d| at(&rw, lo - d, 0.0))),
            ("hi", Box::new(|d| at(&rw, hi + d, 0.0))),
            ("left", Box::new(|d| at(&rw, mid, w + d))),
            ("right", Box::new(|d| at(&rw, mid, -w - d))),
        ];
        for (name, p) in &edges {
            let what = format!("region {i} edge {name}");
            // On the edge and just inside it: touched at r95 = 0.
            assert!(flag(&occupies(&rw, p(0.0), 0.0), i), "{what} on");
            assert!(flag(&occupies(&rw, p(-0.01), 0.0), i), "{what} inside");
            // Outside by the pad and a gap: clear at r95 = 0 and at a radius that does not
            // reach (gap - 0.1), touched at gap + 0.1.
            let out = p(pad + gap);
            assert!(!flag(&occupies(&rw, out, 0.0), i), "{what} outside r=0");
            assert!(
                !flag(&occupies(&rw, out, gap - 0.1), i),
                "{what} outside short"
            );
            assert!(
                flag(&occupies(&rw, out, gap + 0.1), i),
                "{what} outside reached"
            );
            // Inside the pad only: touched even at r95 = 0 (the plane's error is added).
            assert!(
                flag(&occupies(&rw, p(pad - 0.1), 0.0), i),
                "{what} within the pad"
            );
        }
    }
}

// REQ: OA-RWY-007
#[test]
fn unknown_position_or_radius_occupies_everything() {
    let rw = mdw_runway();
    let bad = [
        (
            Local {
                east_m: f64::NAN,
                north_m: 0.0,
            },
            5.0,
        ),
        (
            Local {
                east_m: 0.0,
                north_m: f64::INFINITY,
            },
            5.0,
        ),
        (
            Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            f64::NAN,
        ),
        (
            Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            f64::INFINITY,
        ),
        (
            Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            -1.0,
        ),
        (
            Local {
                east_m: f64::MAX,
                north_m: f64::MAX,
            },
            5.0,
        ),
    ];
    for (p, r) in bad {
        let o = occupies(&rw, p, r);
        assert!(o.unknown && o.on_runway && is_occupied(&o), "{p:?} {r}");
        assert!(o.approach == [true, true] && o.departure == [true, true]);
    }
    // A known, far-away target is clear and not unknown.
    let o = occupies(&rw, at(&rw, 0.0, 9_000.0), 10.0);
    assert!(!o.unknown && !is_occupied(&o));
    assert_eq!(o, Occupancy::default());
}

// REQ: OA-RWY-008
#[test]
fn a_larger_radius_only_adds_occupancy() {
    let rw = mdw_runway();
    let mut prev = [Occupancy::default(); 1];
    for (a, c) in [
        (0.0, 0.0),
        (900.0, 30.0),
        (-1500.0, 60.0),
        (2500.0, 0.0),
        (300.0, 400.0),
        (-2200.0, 200.0),
    ] {
        prev[0] = occupies(&rw, at(&rw, a, c), 0.0);
        let mut r = 0.0;
        while r < 3_000.0 {
            r += 7.0;
            let o = occupies(&rw, at(&rw, a, c), r);
            let p = prev[0];
            assert!(!p.on_runway || o.on_runway);
            for i in 0..2 {
                assert!(!p.approach[i] || o.approach[i]);
                assert!(!p.departure[i] || o.departure[i]);
            }
            prev[0] = o;
        }
    }
}

// REQ: OA-RWY-007
#[test]
fn a_runway_with_a_non_finite_number_never_clears_a_target() {
    let far = Local {
        east_m: 0.0,
        north_m: 9_000.0,
    };
    let base = mdw_runway();
    assert!(!is_occupied(&occupies(&base, far, 0.0)));
    let breaks: [(&str, Breaker); 9] = [
        ("half_len", |r| r.half_len_m = f64::NAN),
        ("half_width", |r| r.half_width_m = f64::NAN),
        ("ux", |r| r.ux = f64::NAN),
        ("center", |r| r.center.east_m = f64::INFINITY),
        ("approach", |r| r.approach_along[1][1] = f64::NAN),
        ("approach width", |r| r.approach_half_width_m = f64::NAN),
        ("departure", |r| r.departure_along[0][0] = f64::NEG_INFINITY),
        ("departure width", |r| r.departure_half_width_m = f64::NAN),
        ("uy", |r| r.uy = f64::INFINITY),
    ];
    for (name, f) in breaks {
        let mut rw = base;
        f(&mut rw);
        let o = occupies(&rw, far, 0.0);
        assert!(
            o.unknown && o.on_runway && o.approach == [true; 2] && o.departure == [true; 2],
            "{name}"
        );
    }
}

// REQ: OA-RWY-007
#[test]
fn real_coordinates_run_end_to_end_through_the_plane_and_the_test() {
    let (plane, input) = mdw();
    let rw = build_runway(&plane, &input).unwrap();
    let a = input.a.position.unwrap();
    let b = input.b.position.unwrap();
    let local = |lat: f64, lon: f64| to_local(&plane, g(lat, lon)).unwrap();
    // The midpoint of the two NASR end coordinates is on the runway, and nowhere else.
    let mid = local((a.lat_deg + b.lat_deg) / 2.0, (a.lon_deg + b.lon_deg) / 2.0);
    let o = occupies(&rw, mid, 0.0);
    assert!(o.on_runway && !o.unknown);
    assert!(!o.approach[0] && !o.approach[1] && !o.departure[0] && !o.departure[1]);
    // The 22R displaced threshold is on the runway and at the start of end b's approach area.
    let t = input.b.displaced_position.unwrap();
    let o = occupies(&rw, local(t.lat_deg, t.lon_deg), 0.0);
    assert!(o.on_runway && o.approach[1] && !o.approach[0]);
    // The physical ends are on the runway, with the departure area beyond them.
    let o = occupies(&rw, local(a.lat_deg, a.lon_deg), 0.0);
    assert!(o.on_runway && o.departure[0] && o.approach[0]);
    // 300 m abeam the midpoint (to the left of the direction a to b, 0.7262 north per metre
    // of the perpendicular's 0.6875 and 0.7262 components): clear.
    let (ux, uy) = (rw.ux, rw.uy);
    let abeam = local(
        (a.lat_deg + b.lat_deg) / 2.0 + ux * 300.0 / 111_000.0,
        (a.lon_deg + b.lon_deg) / 2.0 - uy * 300.0 / 83_000.0,
    );
    let o = occupies(&rw, abeam, 0.0);
    assert!(!is_occupied(&o) && !o.unknown, "{o:?}");
    // The same point is occupied by a target whose radius covers the 300 m.
    assert!(occupies(&rw, abeam, 300.0).on_runway);
}

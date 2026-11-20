//! Runway building, refusal and the occupancy test, on hand-worked runways.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_geo::{wgs84, LatLon};
use oa_runway::occupy::{frame_coords, is_occupied, occupies, Occupancy};
use oa_runway::plane::{plane_new, to_local, Geo, Local, Plane, PLANE_ERROR_BOUND_M};
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
            east_m: rw.center.east_m + rw.ux * (rw.half_len_m + 1.0),
            north_m: rw.center.north_m + rw.uy * (rw.half_len_m + 1.0),
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
    input.a.displaced_len_ft = None;
    input.a.cifp_threshold = input.a.position;
    input.a.position = None;
    // The CIFP point is the physical end here (not displaced): accepted, marked as CIFP.
    let rw = build_runway(&plane, &input).unwrap();
    assert!(rw.ends[0].threshold_from_cifp);
    assert_eq!(rw.ends[0].physical, rw.ends[0].threshold);
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
    let mut worst = 0.0f64;
    for lat0 in [0.0, 25.0, 40.0, 52.0, 64.0, 72.0, -34.0, -55.0, 80.0, -80.0] {
        let plane = plane_new(lat0, 10.0).unwrap();
        let o = LatLon::new(lat0, 10.0).unwrap();
        for k in 0..72 {
            let az = k as f64 * 5.0;
            for dist in [1_000.0, 5_000.0, 10_000.0, 14_000.0, 19_000.0] {
                let q = ell.direct(o, az, dist).unwrap().position;
                let l = to_local(&plane, g(q.lat(), q.lon())).unwrap();
                let d = (l.east_m * l.east_m + l.north_m * l.north_m).sqrt();
                worst = worst.max((d - dist).abs());
            }
        }
    }
    assert!(
        worst <= PLANE_ERROR_BOUND_M,
        "worst distance error {worst} m"
    );
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

// REQ: OA-RWY-006
#[test]
fn occupancy_at_the_rectangle_edges() {
    let rw = mdw_runway();
    let hl = rw.half_len_m;
    let hw = rw.half_width_m;
    let o = |a: f64, c: f64, r: f64| occupies(&rw, at(&rw, a, c), r);
    // Centre, and just inside each edge.
    assert!(o(0.0, 0.0, 0.0).on_runway);
    assert!(o(hl - 0.01, 0.0, 0.0).on_runway);
    assert!(o(-hl + 0.01, 0.0, 0.0).on_runway);
    assert!(o(0.0, hw - 0.01, 0.0).on_runway);
    assert!(o(0.0, -hw + 0.01, 0.0).on_runway);
    // Just outside the side edge: clear at radius zero, touching at a radius that reaches it.
    assert!(!is_occupied(&o(0.0, hw + 0.5, 0.0)));
    assert!(o(0.0, hw + 0.5, 0.6).on_runway);
    assert!(!is_occupied(&o(0.0, hw + 0.5, 0.4)));
    assert!(!is_occupied(&o(0.0, -hw - 0.5, 0.4)));
    assert!(o(0.0, -hw - 0.5, 0.6).on_runway);
    // Just past the end: not on the runway but in the departure area of that end.
    let past = o(hl + 0.5, 0.0, 0.0);
    assert!(!past.on_runway && past.departure[1] && past.approach[1] && !past.departure[0]);
    assert!(o(hl + 0.5, 0.0, 0.6).on_runway);
    // The corner: the square widening reaches it at r = 0.5 on both axes.
    assert!(o(hl + 0.4, hw + 0.4, 0.5).on_runway);
    assert!(!o(hl + 0.6, hw + 0.6, 0.5).on_runway);
}

// REQ: OA-RWY-006
#[test]
fn occupancy_of_the_approach_and_departure_areas() {
    let rw = mdw_runway();
    let hl = rw.half_len_m;
    let hw = rw.half_width_m;
    // End b's approach area starts at its landing threshold, 878 ft inside the physical end, and
    // reaches APPROACH_LENGTH_M beyond it.
    let thr_b = rw.approach_along[1][0];
    assert!(near(thr_b, hl - rw.ends[1].displacement_m, 1e-6));
    let o = occupies(&rw, at(&rw, thr_b + 100.0, 0.0), 0.0);
    assert!(o.on_runway && o.approach[1] && !o.approach[0]);
    let o = occupies(&rw, at(&rw, thr_b + APPROACH_LENGTH_M - 1.0, 0.0), 0.0);
    assert!(o.approach[1] && !o.on_runway);
    assert!(!is_occupied(&occupies(
        &rw,
        at(&rw, thr_b + APPROACH_LENGTH_M + 1.0, 0.0),
        0.0
    )));
    // The area is wider than the runway by its side margin.
    let side = hw + APPROACH_SIDE_MARGIN_M;
    assert!(occupies(&rw, at(&rw, hl + 10.0, side - 1.0), 0.0).approach[1]);
    assert!(!occupies(&rw, at(&rw, hl + 10.0, side + 1.0), 0.0).approach[1]);
    // Departure area beyond end a, and its own, narrower, width.
    let d = occupies(&rw, at(&rw, -hl - DEPARTURE_LENGTH_M + 1.0, 0.0), 0.0);
    assert!(d.departure[0] && !d.on_runway);
    assert!(!is_occupied(&occupies(
        &rw,
        at(&rw, -hl - 1_600.0, 0.0),
        0.0
    )));
    let dside = hw + DEPARTURE_SIDE_MARGIN_M;
    assert!(occupies(&rw, at(&rw, -hl - 10.0, dside - 1.0), 0.0).departure[0]);
    assert!(!occupies(&rw, at(&rw, -hl - 10.0, dside + 1.0), 0.0).departure[0]);
    // Far from everything.
    assert!(!is_occupied(&occupies(&rw, at(&rw, 0.0, 5_000.0), 100.0)));
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

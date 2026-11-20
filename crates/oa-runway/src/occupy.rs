//! The occupancy test. Core (pure, allocation-free).
//!
//! [`occupies`] says which of a runway's regions a target touches: the runway rectangle, the
//! approach area of each end and the departure area of each end. The target is its reported
//! position widened by its 95 % radius `r95_m`, and the widening is a square of half side `r95_m`
//! in the runway's own axes, which contains the circle of that radius: the test can say
//! "occupied" for a target the circle would clear, never the reverse. It is conservative by
//! construction:
//!
//! * every region test compares the target's distance from the region with `r`, so a larger radius
//!   adds occupancy and removes none (proof `occupancy_is_monotone_in_r95`);
//! * a position or radius that is not finite, a negative radius, or a position
//!   beyond [`COORD_LIMIT_M`], is unknown and occupies every region (proof
//!   `non_finite_input_occupies`): missing data never clears a runway (design rule 4).

use crate::plane::Local;
use crate::runway::Runway;

/// The largest coordinate, metres, taken as known: a position past it (the earth is 2e7 m round)
/// is not a position, and is unknown. Our own choice. It keeps every product and sum below in the
/// test far from overflow, so no intermediate is infinite or NaN for any runway [`crate::runway`]
/// builds (its numbers are all within the plane's extent and the areas' lengths).
pub const COORD_LIMIT_M: f64 = 1.0e9;

/// The regions a target touches. Plain data. All five flags are set with `unknown` for a target
/// whose position or radius is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
// A7.2: cold struct, size guard waived
pub struct Occupancy {
    /// The rectangle between the physical ends at the published width.
    pub on_runway: bool,
    /// The approach area of end `a` and of end `b`.
    pub approach: [bool; 2],
    /// The departure area beyond end `a` and beyond end `b`.
    pub departure: [bool; 2],
    /// The position or radius was unknown, so everything is set.
    pub unknown: bool,
}

/// True when the target touches the runway, either approach area or either departure area.
pub fn is_occupied(o: &Occupancy) -> bool {
    o.on_runway || o.approach[0] || o.approach[1] || o.departure[0] || o.departure[1]
}

/// The target's coordinates in the runway's axes, `(along, across)` metres from its centre, or
/// `None` where they are not finite.
pub fn frame_coords(rw: &Runway, pos: Local) -> Option<(f64, f64)> {
    let dx = pos.east_m - rw.center.east_m;
    let dy = pos.north_m - rw.center.north_m;
    let along = dx * rw.ux + dy * rw.uy;
    let across = dy * rw.ux - dx * rw.uy;
    if along.is_finite() && across.is_finite() {
        Some((along, across))
    } else {
        None
    }
}

/// One of the five regions of a runway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// The rectangle between the physical ends.
    Runway,
    /// The approach area of end `a`.
    ApproachA,
    /// The approach area of end `b`.
    ApproachB,
    /// The departure area beyond end `a`.
    DepartureA,
    /// The departure area beyond end `b`.
    DepartureB,
}

/// Every region, in the order the flags of [`Occupancy`] are filled.
pub const REGIONS: [Region; 5] = [
    Region::Runway,
    Region::ApproachA,
    Region::ApproachB,
    Region::DepartureA,
    Region::DepartureB,
];

/// A region's `along` interval and half width, `(lo, hi, half_width)`.
fn box_of(rw: &Runway, region: Region) -> (f64, f64, f64) {
    match region {
        Region::Runway => (-rw.half_len_m, rw.half_len_m, rw.half_width_m),
        Region::ApproachA => (
            rw.approach_along[0][0],
            rw.approach_along[0][1],
            rw.approach_half_width_m,
        ),
        Region::ApproachB => (
            rw.approach_along[1][0],
            rw.approach_along[1][1],
            rw.approach_half_width_m,
        ),
        Region::DepartureA => (
            rw.departure_along[0][0],
            rw.departure_along[0][1],
            rw.departure_half_width_m,
        ),
        Region::DepartureB => (
            rw.departure_along[1][0],
            rw.departure_along[1][1],
            rw.departure_half_width_m,
        ),
    }
}

/// True when a target at `(along, across)` with radius `r` (finite, at least zero) touches the
/// box `[lo, hi] x [-w, w]`: it is within `r` of the box on each axis, `lo - along <= r`,
/// `along - hi <= r`, `-w - across <= r` and `across - w <= r`. Each distance does not depend on
/// `r` and is compared with it, so the test is monotone in `r` however the subtractions round
/// (they are of numbers within 1e9 m, so a rounding is under 1e-6 m).
fn in_box(along: f64, across: f64, b: (f64, f64, f64), r: f64) -> bool {
    b.0 - along <= r && along - b.1 <= r && -b.2 - across <= r && across - b.2 <= r
}

fn known(pos: Local, r95_m: f64) -> bool {
    (-COORD_LIMIT_M..=COORD_LIMIT_M).contains(&pos.east_m)
        && (-COORD_LIMIT_M..=COORD_LIMIT_M).contains(&pos.north_m)
        && r95_m.is_finite()
        && r95_m >= 0.0
}

/// Whether a target at `pos` with 95 % radius `r95_m` touches one region of `rw`. An unknown
/// position or radius touches every region.
pub fn touches(rw: &Runway, pos: Local, r95_m: f64, region: Region) -> bool {
    if !known(pos, r95_m) {
        return true;
    }
    match frame_coords(rw, pos) {
        Some((along, across)) => in_box(along, across, box_of(rw, region), r95_m),
        None => true,
    }
}

const UNKNOWN: Occupancy = Occupancy {
    on_runway: true,
    approach: [true, true],
    departure: [true, true],
    unknown: true,
};

/// Which regions of `rw` a target at `pos` (airport plane, metres) with 95 % radius `r95_m`
/// touches. An unknown position or radius occupies everything.
pub fn occupies(rw: &Runway, pos: Local, r95_m: f64) -> Occupancy {
    if !known(pos, r95_m) {
        return UNKNOWN;
    }
    match frame_coords(rw, pos) {
        Some((along, across)) => {
            let hit = |region| in_box(along, across, box_of(rw, region), r95_m);
            Occupancy {
                on_runway: hit(Region::Runway),
                approach: [hit(Region::ApproachA), hit(Region::ApproachB)],
                departure: [hit(Region::DepartureA), hit(Region::DepartureB)],
                unknown: false,
            }
        }
        None => UNKNOWN,
    }
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;
    use crate::plane::{plane_from_parts, to_local, Geo, EXTENT_M};
    use crate::runway::RunwayEnd;

    fn any_geo() -> Geo {
        Geo {
            lat_deg: kani::any(),
            lon_deg: kani::any(),
        }
    }

    fn any_local() -> Local {
        Local {
            east_m: kani::any(),
            north_m: kani::any(),
        }
    }

    fn any_end() -> RunwayEnd {
        RunwayEnd {
            name: [0; 3],
            physical: any_geo(),
            threshold: any_geo(),
            physical_local: any_local(),
            threshold_local: any_local(),
            displacement_m: 0.0,
            threshold_from_cifp: false,
            true_alignment_deg: None,
            elev_ft: None,
            displaced_elev_ft: None,
            declared: Default::default(),
        }
    }

    /// A runway with every geometry number any `f64`, NaN and infinity included.
    fn any_runway() -> Runway {
        Runway {
            ends: [any_end(), any_end()],
            length_m: kani::any(),
            width_m: kani::any(),
            center: any_local(),
            ux: kani::any(),
            uy: kani::any(),
            half_len_m: kani::any(),
            half_width_m: kani::any(),
            approach_along: [[kani::any(), kani::any()], [kani::any(), kani::any()]],
            approach_half_width_m: kani::any(),
            departure_along: [[kani::any(), kani::any()], [kani::any(), kani::any()]],
            departure_half_width_m: kani::any(),
        }
    }

    fn small(x: f64) -> bool {
        (-COORD_LIMIT_M..=COORD_LIMIT_M).contains(&x)
    }

    /// A runway of the kind [`crate::runway::build_runway`] makes: every geometry number finite
    /// and within [`COORD_LIMIT_M`] (they are within tens of kilometres), the axis a vector of at
    /// most length 2.
    fn finite_runway() -> Runway {
        let rw = any_runway();
        kani::assume(
            small(rw.center.east_m)
                && small(rw.center.north_m)
                && (-2.0..=2.0).contains(&rw.ux)
                && (-2.0..=2.0).contains(&rw.uy)
                && small(rw.half_len_m)
                && small(rw.half_width_m)
                && small(rw.approach_along[0][0])
                && small(rw.approach_along[0][1])
                && small(rw.approach_along[1][0])
                && small(rw.approach_along[1][1])
                && small(rw.approach_half_width_m)
                && small(rw.departure_along[0][0])
                && small(rw.departure_along[0][1])
                && small(rw.departure_along[1][0])
                && small(rw.departure_along[1][1])
                && small(rw.departure_half_width_m),
        );
        rw
    }

    fn implies(a: bool, b: bool) -> bool {
        !a || b
    }

    // REQ: OA-RWY-009
    #[kani::proof]
    fn occupancy_is_monotone_in_r95() {
        // Every flag of `occupies`, and `touches`, is `in_box` of the target's runway-axis
        // coordinates (which do not depend on the radius) with one region's box, so monotone
        // `in_box` over every finite box and coordinate pair is monotone occupancy.
        let (along, across): (f64, f64) = (kani::any(), kani::any());
        let b: (f64, f64, f64) = (kani::any(), kani::any(), kani::any());
        let (r1, r2): (f64, f64) = (kani::any(), kani::any());
        kani::assume(small(along) && small(across));
        kani::assume(small(b.0) && small(b.1) && small(b.2));
        kani::assume(r1.is_finite() && r2.is_finite() && 0.0 <= r1 && r1 <= r2);
        let (t1, t2) = (in_box(along, across, b, r1), in_box(along, across, b, r2));
        assert!(implies(t1, t2));
        kani::cover!(t1, "touched at the smaller radius");
        kani::cover!(!t1 && t2, "a larger radius adds occupancy");
        kani::cover!(!t2, "clear at the larger radius");
    }

    // REQ: OA-RWY-010
    #[kani::proof]
    fn non_finite_input_occupies() {
        let rw = any_runway();
        let (e, n, r): (f64, f64, f64) = (kani::any(), kani::any(), kani::any());
        kani::assume(!(small(e) && small(n) && r.is_finite() && r >= 0.0));
        let o = occupies(
            &rw,
            Local {
                east_m: e,
                north_m: n,
            },
            r,
        );
        assert!(o.unknown && o.on_runway && is_occupied(&o));
        assert!(o.approach[0] && o.approach[1] && o.departure[0] && o.departure[1]);
        kani::cover!(!e.is_finite(), "non-finite east");
        kani::cover!(e.is_finite() && !small(e), "east beyond the limit");
        kani::cover!(!r.is_finite(), "non-finite radius");
        kani::cover!(r < 0.0, "negative radius");
    }

    // REQ: OA-RWY-011
    #[kani::proof]
    fn point_in_rectangle_occupies() {
        // `occupies` and `touches` take the runway region from `box_of` and test it with
        // `in_box` on the target's runway-axis coordinates (`frame_coords`, a rotation that the
        // unit tests check), so: a target whose coordinates are inside the rectangle touches it.
        let rw = finite_runway();
        let (along, across): (f64, f64) = (kani::any(), kani::any());
        let r: f64 = kani::any();
        kani::assume(r.is_finite() && r >= 0.0);
        let b = box_of(&rw, Region::Runway);
        kani::assume(along >= b.0 && along <= b.1 && across >= -b.2 && across <= b.2);
        assert!(in_box(along, across, b, r));
        kani::cover!(r > 0.0, "inside with a positive radius");
        kani::cover!(r == 0.0, "inside with a zero radius");
        kani::cover!(
            b.0 < along && along < b.1,
            "strictly inside along the runway"
        );
    }

    // REQ: OA-RWY-012
    #[kani::proof]
    fn occupies_never_panics() {
        let rw = finite_runway();
        let o = occupies(&rw, any_local(), kani::any());
        kani::cover!(is_occupied(&o), "an input that occupies");
        kani::cover!(!is_occupied(&o), "an input that is clear");
    }

    // REQ: OA-RWY-013
    #[kani::proof]
    fn local_plane_is_bounded() {
        // Every scale a `plane_new` result can have: M and N cos(lat0) per degree for latitudes
        // within `MAX_ORIGIN_LAT_DEG` (110.5 to 111.7 km, and 19 to 111.4 km), and tan(lat0)
        // within 5.8, and tan(lat0) / 2R within 5.8 / 12.7e6.
        let (m, k, t, h): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        kani::assume(m >= 110_000.0 && m <= 112_000.0);
        kani::assume(k >= 10_000.0 && k <= 112_000.0);
        kani::assume(t >= -6.0 && t <= 6.0);
        kani::assume(h >= -1e-6 && h <= 1e-6);
        let (lat0, lon0): (f64, f64) = (kani::any(), kani::any());
        kani::assume(lat0 >= -80.0 && lat0 <= 80.0 && lon0 >= -180.0 && lon0 <= 180.0);
        let p = plane_from_parts(lat0, lon0, m, k, t, h);
        let g = any_geo();
        let l = to_local(&p, g);
        if !(g.lat_deg.is_finite() && g.lon_deg.is_finite()) {
            assert!(l.is_none());
        }
        if let Some(l) = l {
            assert!(l.east_m.is_finite() && l.north_m.is_finite());
            assert!(l.east_m <= EXTENT_M && l.east_m >= -EXTENT_M);
            assert!(l.north_m <= EXTENT_M && l.north_m >= -EXTENT_M);
        }
        kani::cover!(l.is_some(), "a point inside the extent");
        kani::cover!(l.is_none(), "a point refused");
    }
}

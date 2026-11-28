//! The occupancy test. Core (pure, allocation-free).
//!
//! [`occupies`] says which of a runway's regions a target touches: the runway rectangle, the
//! approach area of each end and the departure area of each end. The target is its reported
//! position widened by its 95 % radius `r95_m` plus the plane's error bound and a rounding pad,
//! and the widening is a square of that half side in the runway's own axes, which contains the circle of that radius: the test can say
//! "occupied" for a target the circle would clear, never the reverse. It is conservative by
//! construction:
//!
//! * every region test compares the target's distance from the region with `r`, so a larger radius
//!   adds occupancy and removes none (tested over random runways, positions and radii, not proved);
//! * a position or radius that is not finite, a negative radius, a position
//!   beyond [`COORD_LIMIT_M`], a runway with non-finite occupancy geometry, or an inconsistent
//!   width tuple is unknown and occupies every region (tested over random runways, not proved):
//!   missing or torn width data never clears a runway (design rule 4).

use crate::plane::{Local, PLANE_ERROR_BOUND_M};
use crate::runway::{Runway, APPROACH_SIDE_MARGIN_M, DEPARTURE_SIDE_MARGIN_M};

/// The largest coordinate, metres, taken as known: a position past it (the earth is 2e7 m round)
/// is not a position, and is unknown. Our own choice. It keeps every product and sum below in the
/// test far from overflow, so no intermediate is infinite or NaN for any runway [`crate::runway`]
/// builds (its numbers are all within the plane's extent and the areas' lengths).
pub const COORD_LIMIT_M: f64 = 1.0e9;

/// Added to the radius for the rounding of the subtractions in [`in_box`] (numbers within
/// [`COORD_LIMIT_M`], so a rounding is under 1e-6 m). Our own choice.
pub const ROUNDING_PAD_M: f64 = 1.0e-6;

/// Tolerance for the same width values carried as a full width and as its half-width plus margin.
const DIMENSION_TOLERANCE_M: f64 = 1.0e-6;

/// The radius the test uses: the target's 95 % radius, the plane's error bound
/// ([`PLANE_ERROR_BOUND_M`]: the position the plane gives may be that far from the true one)
/// and the rounding pad. A larger `r95_m` gives a larger value, so a poor position makes a
/// runway look occupied more often, never less.
fn widened(r95_m: f64) -> f64 {
    r95_m + PLANE_ERROR_BOUND_M + ROUNDING_PAD_M
}

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
// A NaN must read as touching, so the comparisons are negated "farther than" tests.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn in_box(along: f64, across: f64, b: (f64, f64, f64), r: f64) -> bool {
    // Written as the negation of "farther than r", so that a NaN anywhere reads as touching:
    // missing data never clears a runway.
    !(b.0 - along > r) && !(along - b.1 > r) && !(-b.2 - across > r) && !(across - b.2 > r)
}

/// True when every number the test reads from the runway is finite.
fn runway_finite(rw: &Runway) -> bool {
    rw.length_m.is_finite()
        && rw.width_m.is_finite()
        && rw.center.east_m.is_finite()
        && rw.center.north_m.is_finite()
        && rw.ux.is_finite()
        && rw.uy.is_finite()
        && rw.half_len_m.is_finite()
        && rw.half_width_m.is_finite()
        && rw.approach_half_width_m.is_finite()
        && rw.departure_half_width_m.is_finite()
        && rw.approach_along[0][0].is_finite()
        && rw.approach_along[0][1].is_finite()
        && rw.approach_along[1][0].is_finite()
        && rw.approach_along[1][1].is_finite()
        && rw.departure_along[0][0].is_finite()
        && rw.departure_along[0][1].is_finite()
        && rw.departure_along[1][0].is_finite()
        && rw.departure_along[1][1].is_finite()
}

/// A finite but partially updated width tuple is not a usable runway snapshot. Occupancy uses
/// half widths directly, while `width_m` is the published width; accepting a mismatch could
/// shrink the tested perimeter and clear a target that the published runway contains.
fn dimensions_coherent(rw: &Runway) -> bool {
    let close = |a: f64, b: f64| (a - b).abs() <= DIMENSION_TOLERANCE_M;
    rw.length_m > 0.0
        && rw.width_m > 0.0
        && rw.half_len_m > 0.0
        && rw.half_width_m > 0.0
        && close(rw.width_m, 2.0 * rw.half_width_m)
        && close(
            rw.approach_half_width_m,
            rw.half_width_m + APPROACH_SIDE_MARGIN_M,
        )
        && close(
            rw.departure_half_width_m,
            rw.half_width_m + DEPARTURE_SIDE_MARGIN_M,
        )
}

fn known(rw: &Runway, pos: Local, r95_m: f64) -> bool {
    runway_finite(rw)
        && dimensions_coherent(rw)
        && (-COORD_LIMIT_M..=COORD_LIMIT_M).contains(&pos.east_m)
        && (-COORD_LIMIT_M..=COORD_LIMIT_M).contains(&pos.north_m)
        && r95_m.is_finite()
        && r95_m >= 0.0
}

/// Whether a target at `pos` with 95 % radius `r95_m` touches one region of `rw`. An unknown
/// position or radius touches every region.
pub fn touches(rw: &Runway, pos: Local, r95_m: f64, region: Region) -> bool {
    if !known(rw, pos, r95_m) {
        return true;
    }
    match frame_coords(rw, pos) {
        Some((along, across)) => in_box(along, across, box_of(rw, region), widened(r95_m)),
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
    if !known(rw, pos, r95_m) {
        return UNKNOWN;
    }
    match frame_coords(rw, pos) {
        Some((along, across)) => {
            let hit = |region| in_box(along, across, box_of(rw, region), widened(r95_m));
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
    use crate::plane::Geo;
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
            plane_origin: Geo {
                lat_deg: kani::any(),
                lon_deg: kani::any(),
            },
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
    /// and within [`COORD_LIMIT_M`] (they are within tens of kilometres), the axis components within [-1, 1].
    fn finite_runway() -> Runway {
        let rw = any_runway();
        kani::assume(
            small(rw.center.east_m)
                && small(rw.center.north_m)
                && (-1.0..=1.0).contains(&rw.ux)
                && (-1.0..=1.0).contains(&rw.uy)
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

    // REQ: OA-RWY-012
    #[kani::proof]
    fn occupies_never_panics() {
        let rw = finite_runway();
        let pos = any_local();
        let o = occupies(&rw, pos, kani::any());
        // The intermediates of a known position are finite and small: `frame_coords` is the
        // only arithmetic on the position, and it cannot overflow or produce NaN there.
        if small(pos.east_m) && small(pos.north_m) {
            let c = frame_coords(&rw, pos);
            assert!(c.is_some());
            if let Some((along, across)) = c {
                assert!(along > -1.0e10 && along < 1.0e10);
                assert!(across > -1.0e10 && across < 1.0e10);
            }
        }
        kani::cover!(is_occupied(&o), "an input that occupies");
        kani::cover!(!is_occupied(&o), "an input that is clear");
    }
}

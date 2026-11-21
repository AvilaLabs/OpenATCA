//! Choosing among the places a name has. Core (pure).
//!
//! A name that NASR gives several points (the same identifier in more than one region, or a fix
//! and a navaid) takes the one nearest the flight's route: the distance from a candidate to the
//! nearest of the reference places of the flight (the places on its route that resolve
//! uniquely, or the airports it flies between), by great circle. It is refused when the
//! candidates are more than 500 NM from each other, or when the flight has no reference place.

use crate::core::cross::great_circle_m;
use crate::core::types::Place;

/// Candidates farther apart than this are not chosen between, metres (500 NM).
pub const MAX_SPREAD_M: f64 = 500.0 * 1852.0;

/// What a name resolved to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resolution {
    /// The name has no place.
    NotFound,
    /// The name has one place.
    Unique(Place),
    /// The name has several places and the one nearest the flight's route was taken.
    Picked(Place),
    /// The name has several places and none was taken.
    Refused,
}

/// Resolves a name with the places `cands` (distinct places) for a flight whose reference places
/// are `refs`. Ties go to the earlier candidate.
pub fn resolve(cands: &[Place], refs: &[Place]) -> Resolution {
    match cands {
        [] => return Resolution::NotFound,
        [one] => return Resolution::Unique(*one),
        _ => {}
    }
    for (i, a) in cands.iter().enumerate() {
        for b in cands.iter().skip(i + 1) {
            if great_circle_m(*a, *b) > MAX_SPREAD_M {
                return Resolution::Refused;
            }
        }
    }
    let mut best: Option<(f64, Place)> = None;
    for c in cands {
        for r in refs {
            let d = great_circle_m(*c, *r);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, *c));
            }
        }
    }
    best.map_or(Resolution::Refused, |b| Resolution::Picked(b.1))
}

/// How a name stands against the table, for the match rates.
pub const CLASS_NAMES: [&str; 4] = [
    "not found",
    "one place",
    "several places within 500 NM of each other (resolvable)",
    "several places over 500 NM apart (refused)",
];

/// The class of a name with the places `cands`: an index into [`CLASS_NAMES`].
pub fn classify(cands: &[Place]) -> usize {
    match cands {
        [] => 0,
        [_] => 1,
        _ => {
            for (i, a) in cands.iter().enumerate() {
                for b in cands.iter().skip(i + 1) {
                    if great_circle_m(*a, *b) > MAX_SPREAD_M {
                        return 3;
                    }
                }
            }
            2
        }
    }
}

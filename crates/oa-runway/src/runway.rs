//! A runway built from published values, or refused. Core (pure).
//!
//! The inputs are plain values the shell copies out of NASR (`APT_RWY`, `APT_RWY_END`) and the
//! CIFP runway record (`oa-arinc424`); this module reads no file. A runway is built only when its
//! thresholds are all known and agree with the published length, alignment and displacement. When
//! they do not, [`build_counted`] returns `None` and counts the reason: a runway with doubtful
//! geometry is refused, never guessed (design rule 1: the airport then shows unavailable).
//!
//! Threshold sources (OA-ASP-067): a NASR displaced-threshold coordinate is the landing
//! threshold. A NASR end with a displacement above zero and no displaced coordinate has no NASR
//! threshold, and the CIFP runway threshold (the landing threshold) takes its place; with neither
//! the runway is refused. An end with no displacement uses the NASR physical end, and the CIFP
//! threshold only where NASR has no end coordinate at all. The physical end is always kept apart
//! from the landing threshold.

use crate::plane::{to_local, Geo, Local, Plane};
use oa_geo::{wgs84, LatLon};

/// Metres in a foot, exactly.
pub const FT_TO_M: f64 = 0.3048;

/// The most a published length may differ from the geodesic distance between the physical ends,
/// metres. Our own choice, about 200 ft: NASR gives the length in whole feet and the end
/// coordinates from a separate survey, so some difference is normal; more means one is wrong.
pub const LENGTH_TOLERANCE_M: f64 = 60.0;

/// The most a published true alignment may differ from the geodesic azimuth between the physical
/// ends, degrees. Our own choice: NASR gives alignment in whole degrees (half a degree of
/// rounding), and we allow the rest for survey and rounding of the coordinates.
pub const ALIGNMENT_TOLERANCE_DEG: f64 = 2.0;

/// The most a published displaced-threshold length may differ from the distance along the runway
/// from the physical end to the threshold coordinate, metres. Our own choice, about 100 ft.
pub const DISPLACEMENT_TOLERANCE_M: f64 = 30.0;

/// The most a threshold may lie off the line between the physical ends, metres. Our own choice:
/// a threshold is on the centreline, so anything over half a runway width is a wrong coordinate.
pub const THRESHOLD_OFF_AXIS_M: f64 = 15.0;

/// The widest runway accepted, feet. Our own sanity bound, above any published paved runway
/// (the widest are 300 ft): a larger width is a data error.
pub const MAX_WIDTH_FT: f64 = 500.0;

/// The shortest distance between the physical ends, metres, below which the runway is
/// degenerate (no direction). Our own choice.
pub const MIN_SEPARATION_M: f64 = 1.0;

/// Length of the approach area, measured outward from the landing threshold, metres.
/// DEVELOPMENT VALUE: our own choice, not an FAA value.
pub const APPROACH_LENGTH_M: f64 = 1_500.0;

/// How far past each runway edge the approach area reaches sideways, metres.
/// DEVELOPMENT VALUE: our own choice, not an FAA value.
pub const APPROACH_SIDE_MARGIN_M: f64 = 100.0;

/// Length of the departure area, measured outward from the physical end, metres.
/// DEVELOPMENT VALUE: our own choice, not an FAA value.
pub const DEPARTURE_LENGTH_M: f64 = 1_000.0;

/// How far past each runway edge the departure area reaches sideways, metres.
/// DEVELOPMENT VALUE: our own choice, not an FAA value.
pub const DEPARTURE_SIDE_MARGIN_M: f64 = 50.0;

/// Longest runway-end name kept (`10L`, `NE`, `N`).
pub const END_NAME_LEN: usize = 3;

/// A runway-end name, ASCII, zero padded.
pub type EndName = [u8; END_NAME_LEN];

/// Declared distances as published, feet.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
// A7.2: cold struct, size guard waived
pub struct DeclaredFt {
    /// `TKOF_RUN_AVBL` (TORA).
    pub tora: Option<f64>,
    /// `TKOF_DIST_AVBL` (TODA).
    pub toda: Option<f64>,
    /// `ACLT_STOP_DIST_AVBL` (ASDA).
    pub asda: Option<f64>,
    /// `LNDG_DIST_AVBL` (LDA).
    pub lda: Option<f64>,
}

/// One end of a runway as the shell read it. Plain data; every field may be absent.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct EndInput<'a> {
    /// `RWY_END_ID`, for example `10L`.
    pub name: &'a str,
    /// `TRUE_ALIGNMENT`, degrees true: the direction of landing or take-off on this end.
    pub true_alignment_deg: Option<f64>,
    /// NASR runway end coordinate: the physical end.
    pub position: Option<Geo>,
    /// NASR displaced-threshold coordinate.
    pub displaced_position: Option<Geo>,
    /// `DISPLACED_THR_LEN`, feet.
    pub displaced_len_ft: Option<f64>,
    /// `RWY_END_ELEV`, feet.
    pub elev_ft: Option<f64>,
    /// `DISPLACED_THR_ELEV`, feet.
    pub displaced_elev_ft: Option<f64>,
    /// The declared distances.
    pub declared: DeclaredFt,
    /// The CIFP runway record's threshold (the landing threshold), where one exists.
    pub cifp_threshold: Option<Geo>,
}

/// A runway as the shell read it: `APT_RWY` and the two `APT_RWY_END` rows. `a` is the end the
/// runway's direction runs from, `b` the other.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct RunwayInput<'a> {
    /// `RWY_LEN`, feet.
    pub length_ft: Option<f64>,
    /// `RWY_WIDTH`, feet.
    pub width_ft: Option<f64>,
    pub a: EndInput<'a>,
    pub b: EndInput<'a>,
}

/// Declared distances in metres.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
// A7.2: cold struct, size guard waived
pub struct DeclaredM {
    pub tora: Option<f64>,
    pub toda: Option<f64>,
    pub asda: Option<f64>,
    pub lda: Option<f64>,
}

/// One end of a built runway.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct RunwayEnd {
    pub name: EndName,
    /// The physical end of the pavement.
    pub physical: Geo,
    /// The landing threshold: the physical end unless displaced.
    pub threshold: Geo,
    pub physical_local: Local,
    pub threshold_local: Local,
    /// Distance along the runway from the physical end to the landing threshold, metres, as
    /// measured from the coordinates.
    pub displacement_m: f64,
    /// True where the threshold came from the CIFP record (OA-ASP-067).
    pub threshold_from_cifp: bool,
    pub true_alignment_deg: Option<f64>,
    pub elev_ft: Option<f64>,
    pub displaced_elev_ft: Option<f64>,
    pub declared: DeclaredM,
}

/// A built runway, in its airport's plane. The rectangle is `|along| <= half_len_m` and
/// `|across| <= half_width_m`, where `along` runs from end `a` to end `b` through `center` and
/// `across` is to its left. The four areas are intervals of `along` with the half widths that go
/// with them. Every number is finite: [`build_runway`] is the only way to make one.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Runway {
    pub ends: [RunwayEnd; 2],
    /// Published length and width, metres.
    pub length_m: f64,
    pub width_m: f64,
    pub center: Local,
    /// Unit vector from end `a` to end `b`.
    pub ux: f64,
    pub uy: f64,
    pub half_len_m: f64,
    pub half_width_m: f64,
    /// `along` interval of the approach area of each end, `[lo, hi]`, measured from its landing
    /// threshold outward ([`APPROACH_LENGTH_M`]).
    pub approach_along: [[f64; 2]; 2],
    pub approach_half_width_m: f64,
    /// `along` interval beyond each physical end ([`DEPARTURE_LENGTH_M`]).
    pub departure_along: [[f64; 2]; 2],
    pub departure_half_width_m: f64,
}

/// Why a runway was refused. Each has a slot in [`RefusalCounts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Width absent, not finite or not above zero.
    BadWidth,
    /// Width above [`MAX_WIDTH_FT`].
    WidthImplausible,
    /// Length absent, not finite or not above zero.
    BadLength,
    /// An end name empty, longer than [`END_NAME_LEN`] or not ASCII letters and digits.
    BadEndName,
    /// A displacement length that is not finite or is negative.
    BadDisplacement,
    /// No coordinate for a physical end (NASR or CIFP).
    PhysicalEndMissing,
    /// No landing threshold coordinate: an end with a displacement and neither a NASR displaced
    /// coordinate nor a CIFP threshold (OA-ASP-067).
    ThresholdUnknown,
    /// A coordinate that is not a valid latitude and longitude.
    BadCoordinate,
    /// A coordinate past the plane's extent (the plane's extent).
    OutsideExtent,
    /// The physical ends are less than [`MIN_SEPARATION_M`] apart.
    Degenerate,
    /// Published length against end distance off by more than [`LENGTH_TOLERANCE_M`].
    LengthMismatch,
    /// Published alignment against end azimuth off by more than [`ALIGNMENT_TOLERANCE_DEG`].
    AlignmentMismatch,
    /// Published displacement against measured off by more than [`DISPLACEMENT_TOLERANCE_M`].
    DisplacementMismatch,
    /// A threshold off the line between the ends by more than [`THRESHOLD_OFF_AXIS_M`], or
    /// outside the runway.
    ThresholdOffRunway,
}

/// Every reason, in the order of [`RefusalCounts`].
pub const REFUSALS: [Refusal; 14] = [
    Refusal::BadWidth,
    Refusal::WidthImplausible,
    Refusal::BadLength,
    Refusal::BadEndName,
    Refusal::BadDisplacement,
    Refusal::PhysicalEndMissing,
    Refusal::ThresholdUnknown,
    Refusal::BadCoordinate,
    Refusal::OutsideExtent,
    Refusal::Degenerate,
    Refusal::LengthMismatch,
    Refusal::AlignmentMismatch,
    Refusal::DisplacementMismatch,
    Refusal::ThresholdOffRunway,
];

/// How many runways were refused for each reason, and how many were built. Counts saturate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
// A7.2: cold struct, size guard waived
pub struct RefusalCounts {
    pub built: u32,
    pub refused: [u32; REFUSALS.len()],
}

/// The slot of a reason in [`RefusalCounts::refused`].
pub fn refusal_index(r: Refusal) -> usize {
    match r {
        Refusal::BadWidth => 0,
        Refusal::WidthImplausible => 1,
        Refusal::BadLength => 2,
        Refusal::BadEndName => 3,
        Refusal::BadDisplacement => 4,
        Refusal::PhysicalEndMissing => 5,
        Refusal::ThresholdUnknown => 6,
        Refusal::BadCoordinate => 7,
        Refusal::OutsideExtent => 8,
        Refusal::Degenerate => 9,
        Refusal::LengthMismatch => 10,
        Refusal::AlignmentMismatch => 11,
        Refusal::DisplacementMismatch => 12,
        Refusal::ThresholdOffRunway => 13,
    }
}

/// The count for a reason.
pub fn refusal_count(c: &RefusalCounts, r: Refusal) -> u32 {
    c.refused.get(refusal_index(r)).copied().unwrap_or(0)
}

/// Builds a runway and counts the outcome: `built`, or the reason it was refused.
pub fn build_counted(
    plane: &Plane,
    input: &RunwayInput<'_>,
    counts: &mut RefusalCounts,
) -> Option<Runway> {
    match build_runway(plane, input) {
        Ok(r) => {
            counts.built = counts.built.saturating_add(1);
            Some(r)
        }
        Err(why) => {
            if let Some(slot) = counts.refused.get_mut(refusal_index(why)) {
                *slot = slot.saturating_add(1);
            }
            None
        }
    }
}

/// The name as fixed bytes, or `None` if it is empty, too long or not ASCII letters and digits.
pub fn end_name(s: &str) -> Option<EndName> {
    let mut out = [0u8; END_NAME_LEN];
    let mut n = 0usize;
    for b in s.bytes() {
        if !b.is_ascii_alphanumeric() {
            return None;
        }
        *out.get_mut(n)? = b;
        n += 1;
    }
    if n == 0 {
        None
    } else {
        Some(out)
    }
}

struct Resolved {
    physical: Geo,
    threshold: Geo,
    from_cifp: bool,
}

fn positive_finite(v: Option<f64>) -> Option<f64> {
    match v {
        Some(x) if x.is_finite() && x > 0.0 => Some(x),
        _ => None,
    }
}

/// The physical end and landing threshold of one end, by the OA-ASP-067 order.
fn resolve_end(e: &EndInput<'_>) -> Result<Resolved, Refusal> {
    let disp_ft = match e.displaced_len_ft {
        None => 0.0,
        Some(x) if x.is_finite() && x >= 0.0 => x,
        Some(_) => return Err(Refusal::BadDisplacement),
    };
    if let Some(t) = e.displaced_position {
        let physical = e.position.ok_or(Refusal::PhysicalEndMissing)?;
        return Ok(Resolved {
            physical,
            threshold: t,
            from_cifp: false,
        });
    }
    if disp_ft > 0.0 {
        // Displaced, with no NASR coordinate for the threshold: the CIFP threshold or nothing.
        let physical = e.position.ok_or(Refusal::PhysicalEndMissing)?;
        let threshold = e.cifp_threshold.ok_or(Refusal::ThresholdUnknown)?;
        return Ok(Resolved {
            physical,
            threshold,
            from_cifp: true,
        });
    }
    match (e.position, e.cifp_threshold) {
        // No displacement: the physical end is the threshold.
        (Some(p), _) => Ok(Resolved {
            physical: p,
            threshold: p,
            from_cifp: false,
        }),
        // NASR has no coordinate at all and says the end is not displaced, so the CIFP
        // threshold is the physical end too.
        (None, Some(c)) => Ok(Resolved {
            physical: c,
            threshold: c,
            from_cifp: true,
        }),
        (None, None) => Err(Refusal::PhysicalEndMissing),
    }
}

fn latlon(g: Geo) -> Result<LatLon, Refusal> {
    LatLon::new(g.lat_deg, g.lon_deg).map_err(|_| Refusal::BadCoordinate)
}

/// True when `a` and `b` differ by more than `tol` degrees round the circle.
fn angle_off(a: f64, b: f64, tol: f64) -> bool {
    let mut d = (a - b) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    !(d <= tol && d >= -tol)
}

fn declared_m(d: &DeclaredFt) -> DeclaredM {
    let m = |v: Option<f64>| v.filter(|x| x.is_finite()).map(|x| x * FT_TO_M);
    DeclaredM {
        tora: m(d.tora),
        toda: m(d.toda),
        asda: m(d.asda),
        lda: m(d.lda),
    }
}

fn abs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// Builds a runway in `plane`, or says why it cannot.
pub fn build_runway(plane: &Plane, input: &RunwayInput<'_>) -> Result<Runway, Refusal> {
    let width_ft = positive_finite(input.width_ft).ok_or(Refusal::BadWidth)?;
    if width_ft > MAX_WIDTH_FT {
        return Err(Refusal::WidthImplausible);
    }
    let length_ft = positive_finite(input.length_ft).ok_or(Refusal::BadLength)?;
    let name_a = end_name(input.a.name).ok_or(Refusal::BadEndName)?;
    let name_b = end_name(input.b.name).ok_or(Refusal::BadEndName)?;
    let ra = resolve_end(&input.a)?;
    let rb = resolve_end(&input.b)?;
    let local = |g: Geo| to_local(plane, g).ok_or(Refusal::OutsideExtent);
    // Validate every coordinate before projecting (a latitude of 91 is a bad coordinate, not an
    // outside-extent one).
    for g in [ra.physical, ra.threshold, rb.physical, rb.threshold] {
        latlon(g)?;
    }
    let (pa, ta) = (local(ra.physical)?, local(ra.threshold)?);
    let (pb, tb) = (local(rb.physical)?, local(rb.threshold)?);

    let (dx, dy) = (pb.east_m - pa.east_m, pb.north_m - pa.north_m);
    let d = oa_ieee::sqrt(dx * dx + dy * dy);
    if d.is_nan() || d < MIN_SEPARATION_M {
        return Err(Refusal::Degenerate);
    }
    let (ux, uy) = (dx / d, dy / d);

    // Length and alignment against the geodesic between the physical ends.
    let ell = wgs84();
    let inv = ell
        .inverse(latlon(ra.physical)?, latlon(rb.physical)?)
        .map_err(|_| Refusal::BadCoordinate)?;
    if abs(inv.distance_m - length_ft * FT_TO_M) > LENGTH_TOLERANCE_M {
        return Err(Refusal::LengthMismatch);
    }
    if let Some(al) = input.a.true_alignment_deg {
        if !al.is_finite() || angle_off(al, inv.azimuth1_deg, ALIGNMENT_TOLERANCE_DEG) {
            return Err(Refusal::AlignmentMismatch);
        }
    }
    if let Some(al) = input.b.true_alignment_deg {
        if !al.is_finite() || angle_off(al, inv.azimuth2_deg + 180.0, ALIGNMENT_TOLERANCE_DEG) {
            return Err(Refusal::AlignmentMismatch);
        }
    }

    // Thresholds: on the line, inside the runway, at the published displacement.
    let along_from =
        |from: Local, p: Local| (p.east_m - from.east_m) * ux + (p.north_m - from.north_m) * uy;
    let across_of =
        |from: Local, p: Local| (p.north_m - from.north_m) * ux - (p.east_m - from.east_m) * uy;
    // End `a` measures inward along +u, end `b` along -u.
    let disp_a = along_from(pa, ta);
    let disp_b = -along_from(pb, tb);
    for (disp, across) in [(disp_a, across_of(pa, ta)), (disp_b, across_of(pb, tb))] {
        if !(abs(across) <= THRESHOLD_OFF_AXIS_M && disp >= -THRESHOLD_OFF_AXIS_M && disp <= d) {
            return Err(Refusal::ThresholdOffRunway);
        }
    }
    for (e, disp) in [(&input.a, disp_a), (&input.b, disp_b)] {
        if let Some(l) = e.displaced_len_ft {
            if abs(disp - l * FT_TO_M) > DISPLACEMENT_TOLERANCE_M {
                return Err(Refusal::DisplacementMismatch);
            }
        }
    }

    let center = Local {
        east_m: pa.east_m + dx * 0.5,
        north_m: pa.north_m + dy * 0.5,
    };
    let half_len = d * 0.5;
    let half_w = width_ft * FT_TO_M * 0.5;
    // Along coordinates of the landing thresholds, from the centre.
    let thr_a = -half_len + disp_a;
    let thr_b = half_len - disp_b;
    let make_end = |name, r: &Resolved, e: &EndInput<'_>, pl, tl, disp| RunwayEnd {
        name,
        physical: r.physical,
        threshold: r.threshold,
        physical_local: pl,
        threshold_local: tl,
        displacement_m: disp,
        threshold_from_cifp: r.from_cifp,
        true_alignment_deg: e.true_alignment_deg,
        elev_ft: e.elev_ft,
        displaced_elev_ft: e.displaced_elev_ft,
        declared: declared_m(&e.declared),
    };
    let out = Runway {
        ends: [
            make_end(name_a, &ra, &input.a, pa, ta, disp_a),
            make_end(name_b, &rb, &input.b, pb, tb, disp_b),
        ],
        length_m: length_ft * FT_TO_M,
        width_m: width_ft * FT_TO_M,
        center,
        ux,
        uy,
        half_len_m: half_len,
        half_width_m: half_w,
        approach_along: [
            [thr_a - APPROACH_LENGTH_M, thr_a],
            [thr_b, thr_b + APPROACH_LENGTH_M],
        ],
        approach_half_width_m: half_w + APPROACH_SIDE_MARGIN_M,
        departure_along: [
            [-half_len - DEPARTURE_LENGTH_M, -half_len],
            [half_len, half_len + DEPARTURE_LENGTH_M],
        ],
        departure_half_width_m: half_w + DEPARTURE_SIDE_MARGIN_M,
    };
    Ok(out)
}

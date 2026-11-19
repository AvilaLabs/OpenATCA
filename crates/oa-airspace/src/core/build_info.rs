//! What a build learns beside the model, and the thresholds it applies: plain data the report
//! reads (OA-ASP-003..005). **Core** (B1, B7): data only, no behaviour (A1). Split out of `build`
//! (shell) so the report, a pure function, names nothing in the shell.

/// Metres per nautical mile.
pub const NM_M: f64 = 1852.0;
/// A NASR and a CIFP position closer than this agree; farther apart is reported (OA-ASP-003).
pub const DISAGREE_M: f64 = 1.0;
/// A NASR navaid and a CIFP navaid with the same identifier and class pair up only when within
/// this distance, 1 nautical mile like airports (short NDB identifiers recur across the
/// country, so a farther same-identifier navaid is another station).
pub const MAX_PAIR_M: f64 = MAX_AIRPORT_PAIR_M;
/// A NASR airport and a CIFP airport with the same identifier pair up only when within this
/// distance, 1 nautical mile (OA-ASP-011). A farther same-identifier pair is two different
/// places that share a code: it is not merged, and the report lists it as a disagreement.
pub const MAX_AIRPORT_PAIR_M: f64 = NM_M;
/// A NASR record whose own decimal and DMS positions differ by more than this (degrees, one
/// model unit) is reported as a source defect.
pub const POSITION_DEFECT_DEG: f64 = 1e-7;

/// A NASR record whose published decimal position disagrees with its own DMS position.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct PositionDefect {
    pub file: &'static str,
    pub line: u64,
    pub ident: String,
    pub diff_deg: f64,
}

/// Largest error of the fixed-point conversion (A9). Cold struct (A7.2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rounding {
    /// Geodesic distance between a source coordinate and its stored 1e-7 degree value, largest
    /// over every stored coordinate, metres.
    pub pos_max_m: f64,
    /// Which value produced it.
    pub pos_at: String,
    /// Coordinates converted.
    pub pos_values: u64,
    /// Largest error converting a leg distance from nautical miles to whole metres.
    pub dist_max_m: f64,
    /// Coordinates whose round trip could not be measured (a failed geodesic or an unusable
    /// coordinate): their error is unknown, not zero, so they are left out of `pos_max_m`.
    pub pos_unverified: u64,
}

/// What the build learned that the model tables do not hold; the report prints it.
#[derive(Clone, Debug, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct BuildInfo {
    pub artcc_name: String,
    /// Every distinct NASR `EFF_DATE` in the consumed files; the model keeps the latest.
    pub nasr_dates: Vec<String>,
    /// The boundaries that define the selection area, with vertex counts.
    pub area_rings: Vec<String>,
    pub rounding: Rounding,
    /// NASR positions whose decimal and DMS disagree, for objects in the model.
    pub position_defects: Vec<PositionDefect>,
    /// Keys that occurred twice within a source (second dropped), and positions that differ.
    pub duplicates: Vec<String>,
    /// NASR navaids that were not paired because two CIFP navaids lie within the pairing cap, or
    /// because two NASR navaids want the same CIFP navaid.
    pub navaid_ambiguous: Vec<String>,
    /// NASR navaids with a same-identifier CIFP navaid beyond the pairing cap: `(metres, text)`,
    /// nearest first.
    pub navaid_over_cap: Vec<(f64, String)>,
    /// Navaids whose NASR type and CIFP class disagree.
    pub kind_mismatches: Vec<String>,
    /// NASR-only procedures (not coded in CIFP), as `airport kind ident`.
    pub uncoded_procedures: Vec<String>,
    /// Airports whose NASR class B/C/D flag and CIFP class airspace disagree.
    pub class_mismatches: Vec<String>,
    /// Source records skipped, with the reason and count.
    pub skipped: Vec<(String, usize)>,
    /// Procedure legs whose altitude description code is not decoded (UK-002).
    pub unverified_alt_desc: usize,
    /// Boundaries that could not be used for the area test, with the reason.
    pub boundary_notes: Vec<String>,
    /// Boundary records refused because another record has their key and different content.
    pub boundary_conflicts: usize,
    /// Selected CIFP volumes with an arc whose coded radius differs from the distance between
    /// its centre and an end point by more than [`ARC_RADIUS_TOLERANCE_M`], one line per volume.
    pub arc_radius_mismatches: Vec<String>,
    /// NASR runway ends with a displaced threshold but no coordinate for it and no CIFP
    /// threshold: no threshold point is made (OA-ASP-067); one entry per runway end.
    pub threshold_unknown: Vec<String>,
    /// Runway ends where NASR gives a displaced threshold without a coordinate and the CIFP
    /// threshold, which is taken, lies at the NASR physical end (within [`DISAGREE_M`]): the
    /// displacement is probably missing from the CIFP position as well.
    pub threshold_doubtful: Vec<String>,
    /// Airway legs whose end point is not the next row of the same airway (OA-ASP-010): the leg
    /// is kept with no end point, and the reason is listed here.
    pub airway_breaks: Vec<String>,
    /// Candidate counts: (label, count).
    pub candidates: Vec<(String, usize)>,
    /// NASR/CIFP pairs compared over the whole cycle (not only the selection).
    pub pairs_compared: usize,
    /// Of those, pairs more than [`DISAGREE_M`] apart.
    pub pairs_over_limit: usize,
    /// The largest distance among them, metres.
    pub pairs_max_m: f64,
    /// Those pairs, largest distance first: `(metres, label)`.
    pub pairs_over: Vec<(f64, String)>,
}

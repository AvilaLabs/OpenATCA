//! Plain data shared by the core modules. Cold, offline code (A7.2, C6).

/// Absent time, Unix ms (the value the legacy tables use).
pub const NO_TIME: i64 = i64::MIN;
/// Absent address, as in the legacy tables.
pub const NO_ADDR: u32 = u32::MAX;

/// The settings of a run, stated by the tool (the core has none).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// A truth path counts as having reached a point only if its closest approach is within this,
    /// metres.
    pub cut_m: f64,
    /// A landing counts only if the last airborne report is within this of the airport
    /// reference point, metres.
    pub runway_radius_m: f64,
    /// A landing whose last airborne report is more than this before the first ground report is
    /// not timed, ms: the aircraft may have been out of coverage for the last part.
    pub landing_gap_max_ms: i64,
    /// The longest look-ahead scored, ms (the end of the last bin).
    pub max_look_ahead_ms: i64,
}

/// A point on the earth, degrees.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub lat_deg: f64,
    pub lon_deg: f64,
}

/// One sample of a truth path. `gap_before` is true when truth had no answer between the previous
/// sample and this one (the path is not continuous across it); the first sample's value is
/// ignored.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathPoint {
    pub t_ms: i64,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub gap_before: bool,
}

/// One time the truth path was within the cut distance of a point.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Passage {
    /// Time of closest approach within the stretch, Unix ms.
    pub t_ms: i64,
    /// The closest approach, metres.
    pub dist_m: f64,
    /// The stretch begins or ends at the start or end of the path, or at a hole in the truth, so
    /// its closest approach is not known (the aircraft may have been nearer where truth has no
    /// answer).
    pub at_edge: bool,
}

/// The truth path against one point.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Approach {
    /// The closest approach of the whole path, metres; infinite when the path has no sample.
    pub min_dist_m: f64,
    /// Every stretch within the cut distance, in time order.
    pub passages: Vec<Passage>,
}

/// One report of an aircraft that states a ground state.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirState {
    pub t_ms: i64,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub ground: bool,
}

/// A change from airborne to ground in the reports of an aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    /// The last airborne report before the first ground one, Unix ms: the landing time.
    pub t_ms: i64,
    /// From that report to the first ground report, ms.
    pub gap_ms: i64,
    /// The last airborne report is within the runway radius of the airport reference point.
    pub at_airport: bool,
}

/// Why an estimate is not scored. Every estimate lands in one reason or in a bin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exclusion {
    /// The message gave no time for the point.
    NoEstimate,
    /// The message carries no issue time.
    NoIssueTime,
    /// The flight was not joined to an ADS-B aircraft (see the join counts for why).
    NotJoined,
    /// The point has no known position.
    PlaceUnknown,
    /// The point's name has more than one position in the recordings.
    PlaceAmbiguous,
    /// The airport has no known reference point.
    AirportUnknown,
    /// The joined aircraft has no truth sample in the flight's interval.
    NoTruthPath,
    /// The truth path never came within the cut distance of the point.
    BeyondCut,
    /// Every passage was before the estimate was issued.
    PassedBeforeIssue,
    /// The first passage after issue begins or ends at a path end or a hole in the truth.
    PassageAtEdge,
    /// The aircraft has no airborne-to-ground change near the airport after issue.
    NoLanding,
    /// The first landing near the airport after issue has too long a gap in the reports.
    LandingGap,
    /// The look-ahead is past the last bin.
    LookAheadBeyondRange,
}

/// Number of [`Exclusion`] values.
pub const EXCLUSION_COUNT: usize = 13;
/// Display names, indexed by [`exclusion_index`].
pub const EXCLUSION_NAMES: [&str; EXCLUSION_COUNT] = [
    "message gave no time for the point",
    "message carries no issue time",
    "flight not joined to an ADS-B aircraft",
    "point has no known position",
    "point name has several positions",
    "airport has no known reference point",
    "no truth sample of the aircraft in the flight's interval",
    "truth path never within the cut distance",
    "every passage before the estimate was issued",
    "first passage after issue at a path end or hole in the truth",
    "no airborne-to-ground change near the airport after issue",
    "landing gap in the reports too long",
    "look-ahead beyond the last bin",
];

/// The index of an exclusion in [`EXCLUSION_NAMES`].
pub fn exclusion_index(e: Exclusion) -> usize {
    match e {
        Exclusion::NoEstimate => 0,
        Exclusion::NoIssueTime => 1,
        Exclusion::NotJoined => 2,
        Exclusion::PlaceUnknown => 3,
        Exclusion::PlaceAmbiguous => 4,
        Exclusion::AirportUnknown => 5,
        Exclusion::NoTruthPath => 6,
        Exclusion::BeyondCut => 7,
        Exclusion::PassedBeforeIssue => 8,
        Exclusion::PassageAtEdge => 9,
        Exclusion::NoLanding => 10,
        Exclusion::LandingGap => 11,
        Exclusion::LookAheadBeyondRange => 12,
    }
}

/// One scored estimate.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Estimate minus actual, seconds: positive when the estimate is later than what happened.
    pub error_s: f64,
    /// Actual time minus the time the message was issued, seconds.
    pub look_ahead_s: f64,
    /// The look-ahead bin, an index into [`crate::core::bins::BIN_NAMES`].
    pub bin: usize,
}

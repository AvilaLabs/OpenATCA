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
    /// A landing needs ground reports and no airborne report for this long after the change, ms.
    pub landing_persist_ms: i64,
    /// A landing's last airborne report is below the airport elevation plus this, feet, or ...
    pub landing_alt_ft: f64,
    /// ... below this ground speed, metres per second.
    pub landing_speed_ms: f64,
    /// A truth leg takes off from (lands at) an airport when its first (last) place is within
    /// this of the airport reference point, metres.
    pub leg_airport_m: f64,
    /// A message binds a leg starting after it only within this, ms.
    pub leg_window_ms: i64,
    /// The clip box, degrees: south, north, west and east edges. A truth leg whose last place is
    /// within `edge_margin_m` of an edge ended where the aircraft left the box.
    pub box_deg: [f64; 4],
    /// How near the box edge a leg's end counts as at the edge, metres.
    pub edge_margin_m: f64,
}

impl Config {
    /// The settings that the tool states besides the four it takes from the command line.
    pub fn new(
        cut_m: f64,
        runway_radius_m: f64,
        landing_gap_max_ms: i64,
        max_look_ahead_ms: i64,
    ) -> Config {
        Config {
            cut_m,
            runway_radius_m,
            landing_gap_max_ms,
            max_look_ahead_ms,
            landing_persist_ms: 30_000,
            landing_alt_ft: 1000.0,
            landing_speed_ms: 80.0,
            leg_airport_m: 10.0 * 1852.0,
            leg_window_ms: 3 * 3_600_000,
            box_deg: [38.5, 46.5, -93.5, -83.5],
            edge_margin_m: 10.0 * 1852.0,
        }
    }
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
    /// The same stretches with no split at a return to the point (the rule of the earlier round).
    pub unsplit: Vec<Passage>,
}

/// One report of an aircraft that states a ground state.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirState {
    pub t_ms: i64,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub ground: bool,
    /// Geometric altitude, else barometric, feet; [`NO_ALT`] when neither.
    pub alt_ft: i32,
    /// Ground speed, 0.1 kt; [`NO_SPEED`] when absent.
    pub gs_dkt: u16,
}

/// No altitude in a report.
pub const NO_ALT: i32 = i32::MIN;
/// No ground speed in a report.
pub const NO_SPEED: u16 = u16::MAX;

/// A change from airborne to ground in the reports of an aircraft that held up as a landing.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    /// The touchdown time: the middle of the gap between the last airborne and the first ground
    /// report, Unix ms.
    pub t_ms: i64,
    /// The last airborne report, Unix ms.
    pub t_air_ms: i64,
    /// The first ground report, Unix ms.
    pub t_gnd_ms: i64,
    /// From the last airborne to the first ground report, ms.
    pub gap_ms: i64,
    /// The last airborne report is within the runway radius of the airport reference point.
    pub at_airport: bool,
}

/// The landings of an aircraft, and the changes near the airport that were refused.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LandingSet {
    pub landings: Vec<Landing>,
    /// Changes near the airport followed by an airborne report within the persistence time
    /// (touch and go, or a ground flag blip).
    pub refused_persist: u64,
    /// Changes near the airport whose last airborne report was too high and too fast.
    pub refused_altitude: u64,
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
    /// The point's name is not in NASR or the places file.
    PlaceUnknown,
    /// The point's name has several places and none could be chosen (no reference to choose by,
    /// or the places are over 500 NM apart).
    PlaceAmbiguous,
    /// The airport is not in NASR or the places file.
    AirportUnknown,
    /// No truth leg of the aircraft is in progress at the issue time or starts after it.
    NoLeg,
    /// Legs of the aircraft are in progress or start after the issue, but none is the flight's own.
    LegNotOwn,
    /// The flight's own next leg starts more than the window after the issue time.
    DepartureTooFar,
    /// The bound leg has no truth sample.
    NoTruthPath,
    /// The truth path never came within the cut distance of the point.
    BeyondCut,
    /// The chosen passage begins or ends at a path end or a hole in the truth.
    PassageAtEdge,
    /// The bound leg has no ground report.
    NoGroundCoverage,
    /// The bound leg has no airborne-to-ground change near the airport.
    NoLanding,
    /// The changes near the airport were touch-and-go or blips, or too high and fast.
    TouchAndGo,
    /// The bound leg landed, but not at the airport (a diversion, or another flight's leg).
    Diversion,
    /// The landing near the airport has too long a gap in the reports.
    LandingGap,
    /// The estimate is not after the time of the message that carried it.
    EstimateBeforeIssue,
    /// The estimated look-ahead is past the last bin.
    LookAheadBeyondRange,
}

/// Number of [`Exclusion`] values.
pub const EXCLUSION_COUNT: usize = 19;
/// Display names, indexed by [`exclusion_index`].
pub const EXCLUSION_NAMES: [&str; EXCLUSION_COUNT] = [
    "message gave no time for the point",
    "message carries no issue time",
    "flight not joined to an ADS-B aircraft",
    "point name not in NASR or the places file",
    "point name has several places, none chosen",
    "airport not in NASR or the places file",
    "no leg (none of the aircraft in progress at issue or starting after it)",
    "no leg of the flight's own (the aircraft's legs there are another flight's)",
    "departure more than the window after the issue (pre-departure, dropped as no leg)",
    "no truth sample of the bound leg",
    "truth path never within the cut distance",
    "passage at a path end or hole in the truth",
    "bound leg has no ground report",
    "no airborne-to-ground change near the airport on the bound leg",
    "only touch-and-go, blip or too-high changes near the airport",
    "bound leg landed elsewhere (diversion or another flight's leg)",
    "landing gap in the reports too long",
    "estimate not after the issue time",
    "estimated look-ahead beyond the last bin",
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
        Exclusion::NoLeg => 6,
        Exclusion::LegNotOwn => 7,
        Exclusion::DepartureTooFar => 8,
        Exclusion::NoTruthPath => 9,
        Exclusion::BeyondCut => 10,
        Exclusion::PassageAtEdge => 11,
        Exclusion::NoGroundCoverage => 12,
        Exclusion::NoLanding => 13,
        Exclusion::TouchAndGo => 14,
        Exclusion::Diversion => 15,
        Exclusion::LandingGap => 16,
        Exclusion::EstimateBeforeIssue => 17,
        Exclusion::LookAheadBeyondRange => 18,
    }
}

/// One scored estimate.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Estimate minus actual, seconds: positive when the estimate is later than what happened.
    pub error_s: f64,
    /// The estimate minus the time the message was issued, seconds: the stated lead.
    pub lead_s: f64,
    /// The bin of the stated lead, an index into [`crate::core::bins::BIN_NAMES`].
    pub bin: usize,
    /// The bin of the actual time minus the issue time, if in range.
    pub act_bin: Option<usize>,
    /// The event happened before the message was issued.
    pub before_issue: bool,
    /// The closest approach of the passage chosen, metres (crossings; zero for landings).
    pub dist_m: f64,
}

//! The plain values of the scoring: one STDDS record, one position fix of a reference, why a
//! record could not be paired, and the independence class.

use std::fmt;

/// Longest gap between the two rows a fix is interpolated from, ms. A longer gap is a break in
/// the track (the tracker coasts or drops it), never bridged.
pub const GAP_MS: i64 = 5_000;
/// Furthest past the first or last row a fix is extrapolated, ms. Beyond it there is no fix.
pub const EXTRAP_MS: i64 = 1_000;
/// Slowest truth ground speed that still gives a track direction, m/s. Below it the along/cross
/// split of that sample is absent (the horizontal error is kept).
pub const MIN_SPEED_MPS: f64 = 2.0;

/// How independent of ADS-B a STDDS record is. STARS in fusion mode uses ADS-B as one of its
/// sensors, so a STDDS track may follow the same aircraft reports the truth is built from: no
/// metric may blend the classes. Only [`Class::Unknown`] is ever assigned until the real feed
/// shows which field separates them (see [`classify_record`]).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// No field known to say how the track was made.
    Unknown,
    /// Reserved: a track that real data shows is made from radar sensors only.
    RadarOnly,
    /// Reserved: a track that real data shows used ADS-B in the fusion.
    FusedAdsb,
}

/// Every class, in report order.
pub const CLASSES: [Class; 3] = [Class::Unknown, Class::RadarOnly, Class::FusedAdsb];

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Unknown => "unknown",
            Class::RadarOnly => "radar-only",
            Class::FusedAdsb => "fused-adsb",
        }
    }
}

/// The independence class of one STDDS record. One function, so the rule is in one place.
///
/// Today it returns [`Class::Unknown`] for every record: the parser has no field confirmed to
/// say which sensors made a track. Fields to check on the first real day, in this order:
/// * `track/status` (`TaisRecord::track_status`): does it carry a source, sensor or fusion state
///   (for example coasting, radar only, ADS-B, mixed)?
/// * `track/coast` and the other element paths the parser counts as unknown (`Stats::unknown`,
///   printed by the `stdds_summary` example): any `sensor`, `source`, `surveillance`, `fusion`,
///   `adsb`, `mode` or `radar` element below `track`, `record` or the root.
/// * `track/reportedBeaconCode` present with no `acAddress` (a radar-only Mode 3/A track) against
///   `acAddress` present (a Mode S or ADS-B derived identity).
/// * the message `src` (the producing TRACON) as a proxy: a facility known to run fusion mode.
/// * `flightPlan/rawFlightRules` and equipment suffix as a proxy for ADS-B equipage.
///
/// The Mode S address alone does NOT separate the classes: a radar Mode S reply carries it too.
pub fn classify_record(_record: &oa_stdds::TaisRecord, _message: &oa_stdds::TaisMessage) -> Class {
    Class::Unknown
}

/// One STDDS record reduced to what the scoring uses. A missing field is `None`, never 0.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rec {
    /// Index of the input file.
    pub file: u32,
    /// Index of the record among the records of its file.
    pub index: u32,
    pub class: Class,
    /// Mode S address, 24 bits.
    pub addr: Option<u32>,
    /// The producing TRACON, zero padded; all zero when the message has none.
    pub src: [u8; 8],
    pub track_num: Option<u32>,
    /// `mrtTime`, Unix ms.
    pub t_ms: Option<i64>,
    /// Latitude, longitude, degrees.
    pub pos: Option<(f64, f64)>,
    /// `reportedAltitude`, feet (ASSUMED feet).
    pub alt_ft: Option<f64>,
    /// East and north velocity, m/s (ASSUMED m/s).
    pub vel: Option<(f64, f64)>,
}

/// A position (and what else is known) of one aircraft at one time.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub alt_ft: Option<f64>,
    /// East and north velocity, m/s.
    pub vel: Option<(f64, f64)>,
    /// The fix lies outside the rows it came from, by at most [`EXTRAP_MS`].
    pub extrapolated: bool,
}

/// Why a reference has no fix for an aircraft at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Miss {
    NoAircraft,
    OutsideSpan,
    Gap,
    NoNearbyReport,
    TooUncertain,
    NotFinite,
}

impl Miss {
    pub fn name(self) -> &'static str {
        match self {
            Miss::NoAircraft => "no such aircraft",
            Miss::OutsideSpan => "outside the track (over 1 s)",
            Miss::Gap => "gap in the track",
            Miss::NoNearbyReport => "no nearby report",
            Miss::TooUncertain => "too uncertain",
            Miss::NotFinite => "non-finite value",
        }
    }
}

/// A position reference, the tracker or the ADS-B truth.
pub trait Reference {
    fn at(&self, addr: u32, t_ms: i64) -> Result<Fix, Miss>;
}

/// Why a STDDS record is not a paired sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unpaired {
    NoAddress,
    NoTimeOrPosition,
    NoTracker(Miss),
    NoTruth(Miss),
    NotFinite,
    /// Fitted-offset mode only: the record's track has no fitted time offset.
    NotFitted,
}

impl fmt::Display for Unpaired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unpaired::NoAddress => f.write_str("no Mode S address"),
            Unpaired::NoTimeOrPosition => f.write_str("no time or position"),
            Unpaired::NoTracker(m) => write!(f, "no tracker fix: {}", m.name()),
            Unpaired::NoTruth(m) => write!(f, "no ADS-B truth: {}", m.name()),
            Unpaired::NotFinite => f.write_str("non-finite error"),
            Unpaired::NotFitted => f.write_str("track has no fitted time offset"),
        }
    }
}

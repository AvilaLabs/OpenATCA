//! The plain values of the scoring: one STDDS record, one position fix of a reference, why a
//! record could not be paired, and the class.

use std::fmt;

/// Longest gap between the two rows a fix is interpolated from, ms. A longer gap is a break in
/// the track (the tracker coasts or drops it), never bridged.
pub const GAP_MS: i64 = 5_000;
/// Furthest past the first or last row a fix is extrapolated, ms. Beyond it there is no fix.
pub const EXTRAP_MS: i64 = 1_000;
/// Slowest truth ground speed that still gives a track direction, m/s. Below it the along/cross
/// split of that sample is absent (the horizontal error is kept).
pub const MIN_SPEED_MPS: f64 = 2.0;

/// The class of a STDDS record, from its own flags. STARS in fusion mode may use ADS-B as one of its
/// sensors, so a STDDS track may follow the same aircraft reports the truth is built from, and no
/// metric may blend the classes. The class names state the value of the flag, not a meaning: what
/// `track/adsb` means is not yet confirmed (the census, `tools/oa-stdds-census`, measures it).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// `track/pseudo`, `track/frozen` or `track/coast` is present and true. Reported on its own and
    /// never pooled with the others, whatever `track/adsb` says.
    PseudoFrozenCoast,
    /// None of those, and `track/adsb` is present and false.
    AdsbFalse,
    /// None of those, and `track/adsb` is present and true.
    AdsbTrue,
    /// None of those, and `track/adsb` is absent.
    AdsbAbsent,
}

/// Every class, in report order.
pub const CLASSES: [Class; 4] = [
    Class::PseudoFrozenCoast,
    Class::AdsbFalse,
    Class::AdsbTrue,
    Class::AdsbAbsent,
];

impl Class {
    /// The name in the report.
    pub fn name(self) -> &'static str {
        match self {
            Class::PseudoFrozenCoast => "pseudo, frozen or coasting",
            Class::AdsbFalse => "adsb=false",
            Class::AdsbTrue => "adsb=true",
            Class::AdsbAbsent => "adsb absent",
        }
    }

    /// The name in the CSV (no comma, no space).
    pub fn code(self) -> &'static str {
        match self {
            Class::PseudoFrozenCoast => "pseudo-frozen-coast",
            Class::AdsbFalse => "adsb-false",
            Class::AdsbTrue => "adsb-true",
            Class::AdsbAbsent => "adsb-absent",
        }
    }
}

/// The class of one STDDS record. One function, so the rule is in one place, and the same as the
/// census's: a flag is read only when its bit is in `present` (an absent flag is never false).
/// The first of `pseudo`, `frozen`, `coast` that is present and true wins over `adsb`.
pub fn classify_record(record: &oa_stdds::TaisRecord, _message: &oa_stdds::TaisMessage) -> Class {
    use oa_stdds::tais::tais_bit;
    use oa_stdds::TaisField;
    let has = |f: TaisField| record.present & tais_bit(f) != 0;
    if (has(TaisField::TrackPseudo) && record.pseudo)
        || (has(TaisField::TrackFrozen) && record.frozen)
        || (has(TaisField::TrackCoast) && record.coast)
    {
        Class::PseudoFrozenCoast
    } else if !has(TaisField::TrackAdsb) {
        Class::AdsbAbsent
    } else if record.adsb {
        Class::AdsbTrue
    } else {
        Class::AdsbFalse
    }
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

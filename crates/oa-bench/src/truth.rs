//! The truth interface (M2_DESIGN section 1) as this crate consumes it. Core (pure).
//!
//! `oa-truth` builds reference trajectories from ADS-B; this crate does not. It sees truth only
//! through a [`Truth`]: a query function value and a list of legs. The names and field types
//! here are the ones `oa-truth` uses, so wiring it in later changes no metric code. The
//! synthetic self-test builds a [`Truth`] from closed-form paths instead.
//!
//! [`Env`] is the same idea for the airspace model: function values that look up fixes,
//! airports, the zone and the controlling unit at a point, so the metrics need no M0 model.

/// A truth state at one time (M2_DESIGN section 1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TruthSample {
    /// Latitude, degrees.
    pub lat: f64,
    /// Longitude, degrees.
    pub lon: f64,
    /// ADS-B barometric altitude, feet; `None` when the report carries none.
    pub baro_alt_ft: Option<i32>,
    /// Smoothed east velocity, metres per second.
    pub vel_east_mps: f64,
    /// Smoothed north velocity, metres per second.
    pub vel_north_mps: f64,
    /// Barometric vertical rate, feet per minute; `None` when absent.
    pub vert_rate_fpm: Option<i32>,
    /// 95% horizontal uncertainty of the smoothed position, metres.
    pub sigma95_h_m: f64,
    /// Index of the leg in [`Truth::legs`] this sample belongs to.
    pub leg: u32,
}

/// Why there is no truth at a time. Nothing is ever filled in (M2_DESIGN section 1.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoTruth {
    /// No leg of this aircraft address exists.
    UnknownAircraft,
    /// Inside a leg, but no real report lies within the bracket on one side of the time.
    NoNearbyReport,
    /// The smoothed position is less certain than the gate.
    TooUncertain,
    /// No leg of the aircraft covers the time: before its first report, after its last, or between legs.
    OutsideLegs,
}

/// Number of [`NoTruth`] values.
pub const NO_TRUTH_COUNT: usize = 4;
/// Display names, indexed by `NoTruth as usize`.
pub const NO_TRUTH_NAMES: [&str; NO_TRUTH_COUNT] = [
    "unknown aircraft",
    "no nearby report",
    "truth too uncertain",
    "outside legs",
];

/// One truth leg: a continuous stretch of one aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TruthLeg {
    /// 24-bit aircraft address.
    pub addr: u32,
    /// First report time of the leg, Unix ms.
    pub start_ms: i64,
    /// Last report time of the leg, Unix ms.
    pub end_ms: i64,
    /// Callsign when known: ASCII, zero or space padded.
    pub callsign: Option<[u8; 8]>,
}

/// The truth query: `(addr, t_ms)` to a sample or the reason there is none.
pub type TruthFn<'a> = &'a dyn Fn(u32, i64) -> Result<TruthSample, NoTruth>;

/// Truth as the metrics see it: a query and the legs.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
pub struct Truth<'a> {
    /// The truth query.
    pub query: TruthFn<'a>,
    /// Every leg, in any order.
    pub legs: &'a [TruthLeg],
}

/// An airport as the benchmark needs it.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct AirportRef {
    /// Field elevation, feet.
    pub elev_ft: i32,
    /// Runway thresholds `(lat, lon)` in degrees.
    pub thresholds: Vec<(f64, f64)>,
}

/// Airspace lookups as function values (cold: filled from the M0 model by the shell).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
pub struct Env<'a> {
    /// Fix or navaid identifier to `(lat, lon)`; `None` when the identifier is unknown or names
    /// more than one place (use [`Env::fix_all`] and a context to choose).
    pub fix: &'a dyn Fn(&str) -> Option<(f64, f64)>,
    /// Every distinct place the identifier names, in a fixed order; empty when unknown.
    pub fix_all: &'a dyn Fn(&str) -> Vec<(f64, f64)>,
    /// Airport identifier to its reference data.
    pub airport: &'a dyn Fn(&str) -> Option<AirportRef>,
    /// Whether a point is inside the zone the presence and initiation metrics cover (ZAU).
    pub in_zone: &'a dyn Fn(f64, f64) -> bool,
    /// The unit and sector owning a point and altitude, in the `oa-fixm` format (unit in bytes 0
    /// to 3, sector in bytes 4 to 7, zero padded); `None` outside every modelled unit.
    pub unit_at: &'a dyn Fn(f64, f64, Option<i32>) -> Option<[u8; 8]>,
}

/// An [`Env`] that knows nothing: every lookup is empty and no point is in the zone.
pub fn empty_env() -> Env<'static> {
    Env {
        fix: &|_| None,
        fix_all: &|_| Vec::new(),
        airport: &|_| None,
        in_zone: &|_, _| false,
        unit_at: &|_, _, _| None,
    }
}

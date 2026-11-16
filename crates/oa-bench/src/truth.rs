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
    /// The state carries an altitude or vertical rate that is not a finite number in range.
    NotFinite,
}

/// Number of [`NoTruth`] values.
pub const NO_TRUTH_COUNT: usize = 5;
/// Display names, indexed by `NoTruth as usize`.
pub const NO_TRUTH_NAMES: [&str; NO_TRUTH_COUNT] = [
    "unknown aircraft",
    "no nearby report",
    "truth too uncertain",
    "outside legs",
    "non-finite truth value",
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
    /// How the leg began, from the ADS-B ground indication (T8).
    pub start: LegStart,
}

/// How a truth leg began, from the ADS-B on-ground indication of its reports. A takeoff is an
/// observed event: a ground report followed by an airborne one. Nothing is inferred from altitude.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LegStart {
    /// No report of the leg states ground or airborne (or the source carries no such indication).
    #[default]
    Unknown,
    /// The first report that states a ground state is airborne: the leg has no takeoff.
    FirstSeenAirborne,
    /// Every report that states a ground state is on the ground: the aircraft never took off.
    NeverAirborne,
    /// A ground report was followed by an airborne one. `t_ms` is the first airborne report; the
    /// takeoff itself lay between the last ground report and it, `gap_ms` earlier at most.
    Takeoff { t_ms: i64, gap_ms: i64 },
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
    /// Altimeter settings observed at the field: `(observation time Unix ms, inches of
    /// mercury)`, in any order. Empty when none is known.
    pub altimeters: Vec<(i64, f64)>,
}

/// An altimeter setting further than this from the event is not used, milliseconds (60 min).
pub const ALTIMETER_WINDOW_MS: i64 = 3_600_000;
/// Standard altimeter setting, inches of mercury: pressure altitude is indicated altitude here.
pub const STANDARD_INHG: f64 = 29.92;

/// The setting of `obs` (`(observation time Unix ms, inches of mercury)`, any order) observed
/// nearest in time to `t_ms` and within [`ALTIMETER_WINDOW_MS`] of it (the earlier of two equally
/// near); `None` when there is none. A setting outside 27 to 32 inHg is not an altimeter setting
/// and is skipped.
pub fn nearest_setting(obs: &[(i64, f64)], t_ms: i64) -> Option<f64> {
    obs.iter()
        .filter(|&&(_, v)| v.is_finite() && (27.0..=32.0).contains(&v))
        .map(|&(o, v)| ((o - t_ms).abs(), o, v))
        .filter(|&(d, _, _)| d <= ALTIMETER_WINDOW_MS)
        .min_by_key(|&(d, o, _)| (d, o))
        .map(|(_, _, v)| v)
}

/// Indicated altitude (feet above mean sea level) of an aircraft at pressure altitude `pa_ft`
/// with altimeter setting `inhg`: `pa + (inHg - 29.92) * 1000`, the relation the ground test uses.
pub fn indicated_ft(pa_ft: f64, inhg: f64) -> f64 {
    pa_ft + (inhg - STANDARD_INHG) * 1000.0
}

impl AirportRef {
    /// The altimeter setting, inches of mercury, observed nearest in time to `t_ms`
    /// ([`nearest_setting`]); `None` when there is none within [`ALTIMETER_WINDOW_MS`].
    pub fn altimeter_near(&self, t_ms: i64) -> Option<f64> {
        nearest_setting(&self.altimeters, t_ms)
    }
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
    /// Whether a point lies in the areas of more than one unit, so `unit_at` has none for it
    /// although the point is inside the model; asked only when `unit_at` answers `None`.
    pub unit_ambiguous: &'a dyn Fn(f64, f64) -> bool,
    /// The altimeter setting, inches of mercury, for a point `(lat, lon)` in degrees at a time
    /// (Unix ms): that of the nearest station with a setting within 60 minutes of the time;
    /// `None` when there is none.
    pub altimeter_at: &'a dyn Fn(f64, f64, i64) -> Option<f64>,
}

/// An [`Env`] that knows nothing: every lookup is empty and no point is in the zone.
pub fn empty_env() -> Env<'static> {
    Env {
        fix: &|_| None,
        fix_all: &|_| Vec::new(),
        airport: &|_| None,
        in_zone: &|_, _| false,
        unit_at: &|_, _, _| None,
        unit_ambiguous: &|_, _| false,
        altimeter_at: &|_, _, _| None,
    }
}

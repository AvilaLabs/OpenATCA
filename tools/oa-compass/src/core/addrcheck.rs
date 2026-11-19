//! Core (pure, B1): does an ERAM flight's track follow the aircraft address it carries? The flight
//! is compared with the ADS-B reports of the same address: every sample of the flight is paired
//! with the report of that address nearest in time (within [`MAX_PAIR_GAP_S`] seconds), and the
//! median distance over the pairs decides. The same rule applies to every flight.
//!
//! Threshold ([`MAX_MEDIAN_KM`], 25 km) and its reason. A pair is at most 10 s apart in time, so
//! the aircraft itself has moved at most 2.6 km between the two (1000 km/h over 10 s); the two
//! sources add their own position errors, a few km for a radar track. The median of the pairs
//! of one flight is therefore a few km when the flight is the aircraft of that address, and the
//! distance between two aircraft (tens to hundreds of km) when it is not. 25 km is about ten
//! times the first and well under the second. At least [`MIN_PAIRS`] pairs are needed: fewer is
//! "not checked", never "agrees". A flight with an address and no ADS-B reports of that address
//! is also not checked. A non-ICAO address (above 0xFFFFFF) has its own outcome, `NonIcao`.

use oa_ieee::sqrt;
use oa_math::sin_cos;

/// Largest time between a sample and the report it is paired with, seconds.
pub const MAX_PAIR_GAP_S: i64 = 10;
/// Fewest pairs for a verdict.
pub const MIN_PAIRS: usize = 10;
/// Median distance above which the track contradicts the address, km.
pub const MAX_MEDIAN_KM: f64 = 25.0;

/// One ADS-B report as the check uses it: address, Unix ns, latitude and longitude in 1e-7 degree.
/// A7.2: transient, one report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
// A7.2: cold struct, size guard waived
pub struct RefPoint {
    pub address: u32,
    pub t_ns: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
}

/// What the check found for one flight. A7.2: cold.
#[derive(Clone, Copy, Debug, PartialEq)]
// A7.2: cold enum, size guard waived
pub enum Verdict {
    /// The address is not a 24-bit ICAO address (a non-ICAO address, above 0xFFFFFF): there are no
    /// ADS-B reports under it to compare with, and nothing is claimed.
    NonIcao,
    /// Too few pairs (or no reports of the address): nothing is claimed.
    NotChecked { pairs: usize },
    /// The median distance is within the threshold.
    Agrees { pairs: usize, median_km: f64 },
    /// The median distance is over the threshold: the track is not that aircraft's.
    Contradicts { pairs: usize, median_km: f64 },
}

/// Distance in km between two positions of 1e-7 degree, on the sphere of the WGS-84 mean radius,
/// by the equirectangular formula at the mean latitude: exact enough (under 1% below 500 km) for a
/// threshold of tens of km.
pub fn distance_km(a: (i32, i32), b: (i32, i32)) -> f64 {
    const R_KM: f64 = 6371.0088;
    const DEG: f64 = core::f64::consts::PI / 180.0;
    let (la, lb) = (f64::from(a.0) * 1.0e-7, f64::from(b.0) * 1.0e-7);
    let dlat = (lb - la) * DEG;
    // The shorter way round the antimeridian.
    let mut dlon = (f64::from(b.1) - f64::from(a.1)) * 1.0e-7;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let (_, c) = sin_cos((la + lb) * 0.5 * DEG);
    let dx = dlon * DEG * c;
    R_KM * sqrt(dlat * dlat + dx * dx)
}

/// The slice of `refs` (sorted) that holds `address`.
fn of_address(refs: &[RefPoint], address: u32) -> &[RefPoint] {
    let lo = refs.partition_point(|r| r.address < address);
    let hi = refs.partition_point(|r| r.address <= address);
    refs.get(lo..hi).unwrap_or(&[])
}

/// The report of `mine` (the reports of one address, sorted by time) that a sample at `t_ns` is
/// paired with: the nearest in time, the earlier one when the two are equally near, and none when
/// the nearest is more than [`MAX_PAIR_GAP_S`] seconds away (exactly that far is still paired).
pub fn nearest(mine: &[RefPoint], t_ns: i64) -> Option<&RefPoint> {
    let at = mine.partition_point(|r| r.t_ns < t_ns);
    let before = at.checked_sub(1).and_then(|i| mine.get(i));
    let after = mine.get(at);
    let best = match (before, after) {
        // `abs_diff` cannot overflow where a subtraction could.
        (Some(b), Some(a)) => {
            if b.t_ns.abs_diff(t_ns) <= a.t_ns.abs_diff(t_ns) {
                Some(b)
            } else {
                Some(a)
            }
        }
        (Some(r), None) | (None, Some(r)) => Some(r),
        (None, None) => None,
    };
    let gap = MAX_PAIR_GAP_S.unsigned_abs().saturating_mul(1_000_000_000);
    best.filter(|r| r.t_ns.abs_diff(t_ns) <= gap)
}

/// Whether a median distance contradicts the address: strictly over [`MAX_MEDIAN_KM`] (exactly
/// 25 km does not).
pub fn contradicts(median_km: f64) -> bool {
    median_km > MAX_MEDIAN_KM
}

/// The check of one flight: `samples` are (Unix ns, latitude, longitude) of its track, one per
/// position time (the caller keeps the first of any several at one time and leaves out samples
/// that are not exported), `refs` all ADS-B reports sorted by (address, time). The median is the
/// lower of the two middle elements when the count is even, the nearest-rank 1/2 percentile that
/// `stats::percentile` gives.
pub fn check_flight(address: u32, samples: &[(i64, i32, i32)], refs: &[RefPoint]) -> Verdict {
    if address > 0x00FF_FFFF {
        return Verdict::NonIcao;
    }
    let mine = of_address(refs, address);
    let mut km: Vec<f64> = Vec::new();
    for &(t, lat, lon) in samples {
        if let Some(r) = nearest(mine, t) {
            km.push(distance_km((lat, lon), (r.lat_e7, r.lon_e7)));
        }
    }
    let pairs = km.len();
    if pairs < MIN_PAIRS {
        return Verdict::NotChecked { pairs };
    }
    km.sort_by(f64::total_cmp);
    // Lower middle element: index (n - 1) / 2, in range because n >= MIN_PAIRS.
    let median_km = km
        .get(pairs.saturating_sub(1) / 2)
        .copied()
        .unwrap_or(f64::NAN);
    if contradicts(median_km) {
        Verdict::Contradicts { pairs, median_km }
    } else {
        Verdict::Agrees { pairs, median_km }
    }
}

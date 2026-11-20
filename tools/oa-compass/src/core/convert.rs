//! Core (pure, B1, B7): one row to the values of one CAT062 Ed 1.21 record. A row is an OpenATCA
//! tracker tick row ([`TrackRow`]) or an ERAM track sample ([`EramRow`]); the result
//! ([`RecordValues`]) holds the counts of each item's LSB, exactly as the specification defines
//! them, so that the shell only has to lay them into a record.
//!
//! Nothing is invented (E6). An item the row does not have is left out of the record and named
//! in [`Converted::notes`], one bit per reason ([`NOTE_NAMES`]); a value that does not fit its
//! item is left out the same way, never clipped; the two places a value is clipped on purpose (the
//! I500 accuracy components) are named too. The shell counts the notes.
//!
//! # I500 accuracy: covariance to APW
//!
//! The tracker's position covariance `[ee, en, nn]` (m^2, east-north axes at the row's position)
//! is turned into the two components of I062/500 subfield 3, APW, the standard deviation of the
//! position in latitude and in longitude, LSB 180/2^25 degrees (5.2.26):
//!
//! * `sigma_north = sqrt(nn)` and `sigma_east = sqrt(ee)`, the one-sigma marginals of the
//!   covariance. This is exact for each axis separately; the correlation `en` has no place in
//!   APW (subfield 2, the XY covariance, is Cartesian) and is dropped, which is the one
//!   approximation, and it is the right one for a consumer that reads each axis alone.
//! * Metres to angle with the WGS-84 radii at the row's latitude `phi`: north by the meridian
//!   radius `M = a (1 - e^2) / (1 - e^2 sin^2 phi)^(3/2)`, east by `N cos(phi)` with the prime
//!   vertical radius `N = a / sqrt(1 - e^2 sin^2 phi)`; then to counts of `180/2^25` degrees.
//!   Counts round to nearest, a component at 0 counts is raised to 1 (a value of 0 would claim a
//!   perfect position) and one above 65 535 counts (about 39 km north) is the 16-bit maximum,
//!   which the specification reads as "maximum or above". Both are noted.
//! * The 95% horizontal radius `r95` the tracker publishes (`oa_track::r95_m_of`, which sets
//!   `O_UNCERTAIN`) is `sqrt(5.99146) * sqrt(lambda_max)` with `lambda_max` the larger
//!   eigenvalue of the covariance; for a circular error it is 2.4477 sigma, and each APW
//!   component is at most `r95 / 2.4477` (the axis marginals never exceed `lambda_max`). A
//!   consumer that wants `r95` back for an isotropic track multiplies the larger component by
//!   2.4477 and converts to metres. It is not stored as such: CAT062 has no 95% radius.
//! * Assumed: the covariance is a Gaussian position covariance in the east-north plane at the
//!   row's position (OA-TRK-263), and the row's position is the mean it belongs to. A covariance
//!   that is negative or not a number is not used (the item is left out).

use core::f64::consts::PI;

use oa_ieee::{round, sqrt};
use oa_math::sin_cos;

use super::time::{day_time_of_ns, DayTime};

/// Position LSB: 180/2^25 degrees, as a count per radian (2^25 / pi).
const COUNTS_PER_RAD: f64 = 33_554_432.0 / PI;
/// Largest latitude and longitude of a row, 1e-7 degree.
const MAX_LAT_E7: i32 = 900_000_000;
const MAX_LON_E7: i32 = 1_800_000_000;
/// WGS-84 semi-major axis, m, and first eccentricity squared.
const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 0.006_694_379_990_14;
/// I062/185 LSB, m/s (5.2.14).
const VEL_LSB_MPS: f64 = 0.25;
/// One knot in m/s (exact).
const KT_MPS: f64 = 1852.0 / 3600.0;
/// I062/135 and I062/136 LSB, ft (1/4 FL, 5.2.12, 5.2.13) and range in counts: -15 FL to 1500 FL.
const ALT_LSB_FT: f64 = 25.0;
const ALT_MIN_COUNTS: f64 = -60.0;
const ALT_MAX_COUNTS: f64 = 6000.0;
/// I062/220 LSB, ft/min (5.2.17).
const CLIMB_LSB_FPM: f64 = 6.25;
/// ERAM altitudes at or above this many feet are read as flight levels (see [`eram_record`]).
const ERAM_FLIGHT_LEVEL_FT: i32 = 18_000;
/// Largest I062/500 APW component (16 bits).
const APW_MAX: f64 = 65_535.0;

/// Reasons a row is not converted at all; counted by the shell, never passed on (E5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The latitude or longitude is outside the range of a position.
    Position,
    /// The time does not convert to a UTC day and time of day.
    Time,
}

/// Names of the refusals, indexed by `Refusal as usize`.
pub const REFUSAL_NAMES: [&str; 2] = [
    "position outside the range of a latitude and longitude",
    "time not representable",
];

/// Bits of [`Converted::notes`]: what the row did not have or did not fit.
pub const NOTE_MODE3A: usize = 0;
pub const NOTE_ADDRESS: usize = 1;
pub const NOTE_IDENT: usize = 2;
pub const NOTE_ALT_ABSENT: usize = 3;
pub const NOTE_ALT_RANGE: usize = 4;
pub const NOTE_CLIMB_ABSENT: usize = 5;
pub const NOTE_CLIMB_RANGE: usize = 6;
pub const NOTE_VEL_ABSENT: usize = 7;
pub const NOTE_VEL_RANGE: usize = 8;
pub const NOTE_AGES: usize = 9;
pub const NOTE_MOVEMENT: usize = 10;
pub const NOTE_ACCURACY_ABSENT: usize = 11;
pub const NOTE_ACCURACY_MAX: usize = 12;
pub const NOTE_ACCURACY_MIN: usize = 13;
pub const NOTE_UNCERTAIN: usize = 14;
pub const NOTE_REACQUIRED: usize = 15;
pub const NOTE_SERVICE: usize = 16;
pub const NOTE_ALT_ROUNDED: usize = 17;
pub const NOTE_LON_WRAPPED: usize = 18;
pub const NOTE_ALT_QNH: usize = 19;
pub const NOTE_CLIMB_NO_ALT: usize = 20;
pub const NOTE_IDENTITY_UNCONFIRMED: usize = 21;
pub const NOTE_STATUS_ASSUMED: usize = 22;
pub const NOTE_ADDRESS_WITHHELD: usize = 23;
/// Number of notes.
pub const NOTES: usize = 24;

/// The report line of each note, indexed by the constants above.
pub const NOTE_NAMES: [&str; NOTES] = [
    "I060 Mode 3/A code: not in the source row (item omitted)",
    "I380 target address: row has none or it is not a 24-bit Mode S address (subitem omitted)",
    "I380 target identification: row has none or it cannot be encoded (subitem omitted)",
    "I135/I136 altitude: row has none (item omitted)",
    "I135/I136 altitude: outside -15 FL to 1500 FL or not a number (item omitted)",
    "I220 rate of climb/descent: row has none (item omitted)",
    "I220 rate of climb/descent: outside the range of the item or not a number (item omitted)",
    "I185 velocity: row has none (item omitted)",
    "I185 velocity: outside the range of the item or not a number (item omitted)",
    "I290/I295 update ages: not in the source (items omitted)",
    "I200 mode of movement: the source decides no mode (item omitted)",
    "I500 accuracy: no covariance in the source, or not usable (item omitted)",
    "I500 accuracy: a component at the 16-bit maximum (encoded as the maximum, 'maximum or above')",
    "I500 accuracy: a component under 1 LSB (encoded as 1 LSB, never 0)",
    "O_UNCERTAIN row flag: no CAT062 field, not encoded (the I500 accuracy carries the size)",
    "O_REACQUIRED row flag: no CAT062 field, not encoded",
    "I015 service identification: not in the source (item omitted)",
    "I135/I136 altitude off the 25 ft grid: rounded to the nearest 25 ft (item kept)",
    "longitude of exactly +180 degrees (or rounding to it): written as -180, the same meridian (CAT062 range is -180 up to, not including, +180)",
    "ERAM altitude under 18 000 ft: written as I135 with QNH = 1 (altimeter-corrected; ASSUMED, UNCONFIRMED, inferred from this project's own comparison; see README), not as a flight level",
    "I220 rate of climb/descent: exported on a row without an altitude (item kept)",
    "O_IDENTITY_UNCONFIRMED row flag (the key is a local radar handle, not an address): written with CNF = 0 all the same (ASSUMED, UNCONFIRMED: the specification defines CNF for the track, not for its identity; see README)",
    "I080 MON = 0 and CNF = 0: not in the feed; written as defaults (ASSUMED, UNCONFIRMED), not facts about ERAM",
    "I380 target address withheld: the flight's track contradicts the address it carries (see the address check below)",
];

/// The bit of note `i` in [`Converted::notes`].
pub const fn note_bit(i: usize) -> u32 {
    1u32 << i
}

/// Adds one to the counter of every note set in `mask`.
pub fn count_notes(counts: &mut [u64; NOTES], mask: u32) {
    for (i, c) in counts.iter_mut().enumerate() {
        if mask & note_bit(i) != 0 {
            *c = c.saturating_add(1);
        }
    }
}

/// One OpenATCA tracker tick row, in the units of `oa_track::TrackOutput`, with the flags opened
/// out and the aircraft address decided by the shell (the key of a track without a confirmed
/// address is not an address). A7.2: transient, one row.
#[derive(Clone, Copy, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct TrackRow {
    /// Unix nanoseconds.
    pub t_ns: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// True east and north velocity, m/s; NaN if absent.
    pub vel_east_mps: f32,
    pub vel_north_mps: f32,
    /// Position covariance `[ee, en, nn]`, m^2.
    pub pos_cov_m2: [f32; 3],
    /// Barometric altitude, ft, and whether the row has one (`O_HAS_ALT`).
    pub alt_ft: f32,
    pub has_alt: bool,
    /// Vertical rate, ft/min; NaN if absent.
    pub vrate_fpm: f32,
    /// `O_COAST`.
    pub coast: bool,
    /// `O_RADAR_DISAGREES`.
    pub radar_disagrees: bool,
    /// `O_UNCERTAIN` and `O_REACQUIRED`: known, but CAT062 has no field for them.
    pub uncertain: bool,
    pub reacquired: bool,
    /// `O_IDENTITY_UNCONFIRMED`: the key is a local radar handle, not an address. Known, not
    /// encoded: it is not the CNF (tentative track) state, which this export never writes as 1.
    pub identity_unconfirmed: bool,
    /// The aircraft address of the track as the key holds it (bit 24 marks a non-ICAO address),
    /// or `None` when the key is not an address.
    pub address: Option<u32>,
    /// The tracker was fed one kind of sensor only (ADS-B): I062/080 MON = 1.
    pub monosensor: bool,
}

/// One ERAM track sample (`TH` message with a position), in the units of the legacy tables, with
/// the absent values opened out. A7.2: transient, one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct EramRow {
    /// Position time, Unix milliseconds.
    pub pos_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Reported altitude, ft.
    pub alt_ft: Option<i32>,
    /// Track velocity east and north, 0.1 kt.
    pub vel_dkt: Option<(i16, i16)>,
    pub coast: bool,
    /// The flight's `aircraftAddress`, if any message carried it.
    pub address: Option<u32>,
    /// The address is withheld: the flight's track contradicts it (see `addrcheck`).
    pub address_withheld: bool,
    /// The flight's callsign, NUL padded; `None` if no message carried it.
    pub callsign: Option<[u8; 8]>,
}

/// The values of one record, in the LSBs of the items. A7.2: transient, one record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct RecordValues {
    /// I062/015 service identification.
    pub service: Option<u8>,
    /// I062/070, 1/128 s since midnight UTC.
    pub time: u32,
    /// I062/105, counts of 180/2^25 degrees.
    pub lat: i32,
    pub lon: i32,
    /// I062/185, counts of 0.25 m/s, east then north (Vx, Vy).
    pub velocity: Option<(i16, i16)>,
    /// I062/040; set by the shell (it owns the numbering of the tracks).
    pub track_number: u16,
    /// I062/080 CST (third extent).
    pub coasting: bool,
    /// I062/080 MON (monosensor track).
    pub monosensor: bool,
    /// I062/080 AFF (first extent): ADS-B data inconsistent with other surveillance information.
    pub aff: bool,
    /// I062/380 subitem 1.
    pub address: Option<u32>,
    /// I062/380 subitem 2, eight characters of `A-Z`, `0-9`, space.
    pub ident: Option<[u8; 8]>,
    /// I062/135 calculated barometric altitude, counts of 1/4 FL, no QNH correction.
    pub baro_alt: Option<i16>,
    /// QNH flag of I062/135 (altimeter correction applied).
    pub baro_qnh: bool,
    /// I062/136 measured flight level, counts of 1/4 FL.
    pub measured_fl: Option<i16>,
    /// I062/220, counts of 6.25 ft/min.
    pub climb: Option<i16>,
    /// I062/500 subitem 3 APW, latitude and longitude, counts of 180/2^25 degrees.
    pub accuracy: Option<(u16, u16)>,
}

/// A converted row: the UTC day, the values and the notes. A7.2: transient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Converted {
    pub day: i64,
    pub values: RecordValues,
    /// Bit `i` set: note `i` of [`NOTE_NAMES`] applies to this row.
    pub notes: u32,
}

/// Rounds to nearest and checks the result against `lo..=hi`; `None` for NaN or out of range.
fn count_in(x: f64, lo: f64, hi: f64) -> Option<f64> {
    if x.is_nan() {
        return None;
    }
    let r = round(x);
    (r >= lo && r <= hi).then_some(r)
}

/// Position counts and whether the longitude was wrapped. A longitude of exactly +180 degrees
/// (count 2^25, also reached by rounding) is written as -180: the range of I062/105 is
/// -180 <= lon < 180 (5.2.8) and it is the same meridian.
fn position(lat_e7: i32, lon_e7: i32) -> Option<(i32, i32, bool)> {
    if lat_e7.unsigned_abs() > MAX_LAT_E7.unsigned_abs()
        || lon_e7.unsigned_abs() > MAX_LON_E7.unsigned_abs()
    {
        return None;
    }
    // 2^25 / 1.8e9 counts per 1e-7 degree; at most 2^25 counts, so the cast cannot truncate.
    let f = |e7: i32| round(f64::from(e7) * (33_554_432.0 / 1_800_000_000.0)) as i32;
    let (lat, lon) = (f(lat_e7), f(lon_e7));
    if lon >= 1 << 25 {
        Some((lat, -(1 << 25), true))
    } else {
        Some((lat, lon, false))
    }
}

/// The two APW components for a covariance at latitude `lat_e7`, and the notes. `None` if the
/// covariance is not usable.
fn accuracy(cov: &[f32; 3], lat_e7: i32) -> Option<((u16, u16), u32)> {
    let (ee, nn) = (f64::from(cov[0]), f64::from(cov[2]));
    if !(ee >= 0.0 && nn >= 0.0 && ee.is_finite() && nn.is_finite()) {
        return None;
    }
    let phi = f64::from(lat_e7) * 1.0e-7 * (PI / 180.0);
    let (s, c) = sin_cos(phi);
    let w = 1.0 - WGS84_E2 * s * s;
    let sw = sqrt(w);
    let prime_vertical = WGS84_A / sw;
    let meridian = WGS84_A * (1.0 - WGS84_E2) / (w * sw);
    let lat = sqrt(nn) / meridian * COUNTS_PER_RAD;
    let lon = if c > 1.0e-9 {
        sqrt(ee) / (prime_vertical * c) * COUNTS_PER_RAD
    } else {
        f64::INFINITY
    };
    let mut notes = 0u32;
    let mut one = |v: f64| -> u16 {
        let r = round(v);
        if r >= APW_MAX {
            notes |= note_bit(NOTE_ACCURACY_MAX);
            u16::MAX
        } else if r < 1.0 {
            notes |= note_bit(NOTE_ACCURACY_MIN);
            1
        } else {
            r as u16
        }
    };
    let pair = (one(lat), one(lon));
    Some((pair, notes))
}

/// Velocity counts of a pair of m/s values; `Err(true)` if a value is absent (NaN), `Err(false)`
/// if one is out of range.
fn velocity_counts(east: f64, north: f64) -> Result<(i16, i16), bool> {
    if east.is_nan() || north.is_nan() {
        return Err(true);
    }
    let lim = f64::from(i16::MAX);
    let f = |v: f64| count_in(v / VEL_LSB_MPS, -lim - 1.0, lim).map(|r| r as i16);
    match (f(east), f(north)) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(false),
    }
}

/// Why an ERAM sample would be refused by [`eram_record`] (position outside the range of a
/// latitude and longitude, or a time that does not convert), without converting it; `None` if it
/// would be converted. The address check uses it to leave out the samples that are not exported.
pub fn eram_refusal(pos_ms: i64, lat_e7: i32, lon_e7: i32) -> Option<Refusal> {
    let Some(t_ns) = pos_ms.checked_mul(1_000_000) else {
        return Some(Refusal::Time);
    };
    if day_time_of_ns(t_ns).is_none() {
        return Some(Refusal::Time);
    }
    position(lat_e7, lon_e7)
        .is_none()
        .then_some(Refusal::Position)
}

/// Converts one OpenATCA tracker tick row.
pub fn track_record(r: &TrackRow) -> Result<Converted, Refusal> {
    let DayTime { day, units } = day_time_of_ns(r.t_ns).ok_or(Refusal::Time)?;
    let (lat, lon, wrapped) = position(r.lat_e7, r.lon_e7).ok_or(Refusal::Position)?;
    let mut v = RecordValues {
        // The service identification is allocated by the system (5.2.2): this export allocates 1
        // to the OpenATCA tracker output, which is the one value it can state truthfully.
        service: Some(1),
        time: units,
        lat,
        lon,
        coasting: r.coast,
        monosensor: r.monosensor,
        aff: r.radar_disagrees,
        ..RecordValues::default()
    };
    let mut notes = note_bit(NOTE_MODE3A) | note_bit(NOTE_AGES) | note_bit(NOTE_MOVEMENT);
    notes |= note_bit(NOTE_IDENT);
    if wrapped {
        notes |= note_bit(NOTE_LON_WRAPPED);
    }
    if r.uncertain {
        notes |= note_bit(NOTE_UNCERTAIN);
    }
    if r.reacquired {
        notes |= note_bit(NOTE_REACQUIRED);
    }
    if r.identity_unconfirmed {
        notes |= note_bit(NOTE_IDENTITY_UNCONFIRMED);
    }
    match r.address {
        Some(a) if a <= 0x00FF_FFFF => v.address = Some(a),
        _ => notes |= note_bit(NOTE_ADDRESS),
    }
    match velocity_counts(f64::from(r.vel_east_mps), f64::from(r.vel_north_mps)) {
        Ok(c) => v.velocity = Some(c),
        Err(true) => notes |= note_bit(NOTE_VEL_ABSENT),
        Err(false) => notes |= note_bit(NOTE_VEL_RANGE),
    }
    if r.has_alt {
        match count_in(
            f64::from(r.alt_ft) / ALT_LSB_FT,
            ALT_MIN_COUNTS,
            ALT_MAX_COUNTS,
        ) {
            Some(c) => v.baro_alt = Some(c as i16),
            None => notes |= note_bit(NOTE_ALT_RANGE),
        }
    } else {
        notes |= note_bit(NOTE_ALT_ABSENT);
    }
    // The rate of climb does not depend on the altitude item: a valid rate is exported whether or
    // not the row has an altitude.
    let climb = f64::from(r.vrate_fpm);
    if climb.is_nan() {
        notes |= note_bit(NOTE_CLIMB_ABSENT);
    } else {
        let lim = f64::from(i16::MAX);
        match count_in(climb / CLIMB_LSB_FPM, -lim - 1.0, lim) {
            Some(c) => {
                v.climb = Some(c as i16);
                if !r.has_alt {
                    notes |= note_bit(NOTE_CLIMB_NO_ALT);
                }
            }
            None => notes |= note_bit(NOTE_CLIMB_RANGE),
        }
    }
    match accuracy(&r.pos_cov_m2, r.lat_e7) {
        Some((a, n)) => {
            v.accuracy = Some(a);
            notes |= n;
        }
        None => notes |= note_bit(NOTE_ACCURACY_ABSENT),
    }
    Ok(Converted {
        day,
        values: v,
        notes,
    })
}

/// A callsign as eight CAT062 characters: NUL padding becomes spaces; anything outside `A-Z`,
/// `0-9` and space makes it unencodable (`None`), never a changed character.
fn ident_of(c: &[u8; 8]) -> Option<[u8; 8]> {
    let mut out = [b' '; 8];
    for (o, &b) in out.iter_mut().zip(c.iter()) {
        *o = match b {
            0 => b' ',
            b'A'..=b'Z' | b'0'..=b'9' | b' ' => b,
            _ => return None,
        };
    }
    // Eight spaces is no identification.
    (out != [b' '; 8]).then_some(out)
}

/// Converts one ERAM track sample. ERAM gives no covariance, no update ages, no Mode 3/A code and
/// no mode of movement, and no service identification to take over, so those are noted on every
/// row.
pub fn eram_record(r: &EramRow) -> Result<Converted, Refusal> {
    let t_ns = r.pos_ms.checked_mul(1_000_000).ok_or(Refusal::Time)?;
    let DayTime { day, units } = day_time_of_ns(t_ns).ok_or(Refusal::Time)?;
    let (lat, lon, wrapped) = position(r.lat_e7, r.lon_e7).ok_or(Refusal::Position)?;
    let mut v = RecordValues {
        time: units,
        lat,
        lon,
        coasting: r.coast,
        ..RecordValues::default()
    };
    let mut notes = note_bit(NOTE_MODE3A)
        | note_bit(NOTE_AGES)
        | note_bit(NOTE_MOVEMENT)
        | note_bit(NOTE_ACCURACY_ABSENT)
        | note_bit(NOTE_CLIMB_ABSENT)
        | note_bit(NOTE_SERVICE)
        | note_bit(NOTE_STATUS_ASSUMED);
    if wrapped {
        notes |= note_bit(NOTE_LON_WRAPPED);
    }
    match r.address {
        Some(_) if r.address_withheld => {
            notes |= note_bit(NOTE_ADDRESS_WITHHELD);
        }
        Some(a) if a <= 0x00FF_FFFF => v.address = Some(a),
        _ => notes |= note_bit(NOTE_ADDRESS),
    }
    match r.callsign.as_ref().and_then(ident_of) {
        Some(i) => v.ident = Some(i),
        None => notes |= note_bit(NOTE_IDENT),
    }
    match r.vel_dkt {
        // 0.1 kt to counts of 0.25 m/s.
        Some((x, y)) => {
            let f = |d: i16| f64::from(d) * 0.1 * KT_MPS;
            match velocity_counts(f(x), f(y)) {
                Ok(c) => v.velocity = Some(c),
                Err(_) => notes |= note_bit(NOTE_VEL_RANGE),
            }
        }
        None => notes |= note_bit(NOTE_VEL_ABSENT),
    }
    match r.alt_ft {
        // The feed gives feet without saying what they are referenced to. The project's own
        // comparison (docs/M2_DESIGN.md, "Altitude reference for T4") found values under 18 000 ft
        // altimeter-corrected and values at or above it flight levels. The same rule is used
        // here, and it is an inference, not something the feed states: at or above 18 000 ft the
        // value is a measured flight level (I136); below it, I135 with QNH = 1.
        Some(a) => match count_in(f64::from(a) / ALT_LSB_FT, ALT_MIN_COUNTS, ALT_MAX_COUNTS) {
            Some(c) => {
                if a >= ERAM_FLIGHT_LEVEL_FT {
                    v.measured_fl = Some(c as i16);
                } else {
                    v.baro_alt = Some(c as i16);
                    v.baro_qnh = true;
                    notes |= note_bit(NOTE_ALT_QNH);
                }
                if a % 25 != 0 {
                    notes |= note_bit(NOTE_ALT_ROUNDED);
                }
            }
            None => notes |= note_bit(NOTE_ALT_RANGE),
        },
        None => notes |= note_bit(NOTE_ALT_ABSENT),
    }
    Ok(Converted {
        day,
        values: v,
        notes,
    })
}

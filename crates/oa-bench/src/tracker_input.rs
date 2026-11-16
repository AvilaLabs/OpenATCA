//! Adapts a tracker's output rows so they are scored exactly like ERAM's published samples.
//! Core (pure).
//!
//! The metric code reads a [`Legacy`] table of track samples, one flight per aircraft. This
//! module fills that table from plain columns (a tracker's output at a fixed rate), so every
//! metric, the association and the pairing run unchanged: the tracker is "the legacy stream" of
//! the benchmark (M3_DESIGN section 2). Each distinct address becomes one flight (ascending, so
//! the result does not depend on row order), keyed by the address, with no callsign: association
//! is by address, the same first rule as for ERAM. Positions keep their 1e-7 degree units,
//! altitudes feet, velocity is converted to the table's 0.1 knot east and north, and the coast
//! flag is carried so the coast split of the metrics applies.

use std::collections::{BTreeMap, BTreeSet};

use crate::legacy::{flight_ix, new_legacy, push_th, set_addr, Legacy, NO_ALT, NO_TIME};
use crate::truth::TruthLeg;

const MPS_PER_KT: f64 = 0.514_444_444_444;

/// Columns of a tracker's output, one entry per row. Plain data (A1); A7.2: transient view.
#[derive(Clone, Copy, Debug)]
pub struct TrackSamples<'a> {
    /// 24-bit aircraft address (bit 24 set for a non-ICAO address, as in `oa-truth`).
    pub addr: &'a [u32],
    /// Time of the state, Unix ns.
    pub t_ns: &'a [i64],
    pub lat_e7: &'a [i32],
    pub lon_e7: &'a [i32],
    /// Altitude, feet, or [`NO_ALT`].
    pub alt_ft: &'a [i32],
    /// True east and north velocity, m/s; NaN where absent.
    pub vel_east_mps: &'a [f32],
    pub vel_north_mps: &'a [f32],
    pub coast: &'a [bool],
}

/// Why the columns could not be adapted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplesError {
    /// The columns have different lengths.
    ColumnLength,
}

/// The legacy table of a tracker's output. Refuses columns of different lengths (E5). All rows
/// of an address are one flight; see [`legacy_from_track_samples_by_leg`] for one flight per trip.
pub fn legacy_from_track_samples(s: &TrackSamples<'_>) -> Result<Legacy, SamplesError> {
    adapt(s, &BTreeMap::new(), new_legacy())
}

/// As [`legacy_from_track_samples`], filling `leg` (which sets the table limits).
pub fn legacy_from_track_samples_into(
    s: &TrackSamples<'_>,
    leg: Legacy,
) -> Result<Legacy, SamplesError> {
    adapt(s, &BTreeMap::new(), leg)
}

/// Report label of the rows left out because their flight did not fit the flight table.
pub const DROP_FLIGHT_FULL: &str = "tracker rows dropped, reason flight table full";
/// Report label of the rows left out because a column had no entry for them.
pub const DROP_NO_TRIP: &str = "tracker rows dropped, reason no trip (column entry missing)";

/// As [`legacy_from_track_samples`], but one flight per truth leg of an address: a row belongs
/// to the latest leg of its address that starts at or before its time (to the first leg if it
/// precedes them all; an address with no leg is one flight). Intervals between rows are then
/// never taken across two trips of one aircraft. Flights are ordered by (address, leg).
pub fn legacy_from_track_samples_by_leg(
    s: &TrackSamples<'_>,
    legs: &[TruthLeg],
) -> Result<Legacy, SamplesError> {
    let mut starts: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
    for l in legs {
        starts.entry(l.addr).or_default().push(l.start_ms);
    }
    for v in starts.values_mut() {
        v.sort_unstable();
    }
    adapt(s, &starts, new_legacy())
}

/// The trip of a row: the number of leg starts of its address at or before `t_ms`, less one.
fn trip_of(starts: &BTreeMap<u32, Vec<i64>>, addr: u32, t_ms: i64) -> u32 {
    starts.get(&addr).map_or(0, |v| {
        u32::try_from(v.partition_point(|&x| x <= t_ms).saturating_sub(1)).unwrap_or(u32::MAX)
    })
}

/// The flight key of an address and trip.
fn flight_key(addr: u32, trip: u32) -> u128 {
    u128::from(addr) | (1u128 << 100) | (u128::from(trip) << 40)
}

fn adapt(
    s: &TrackSamples<'_>,
    starts: &BTreeMap<u32, Vec<i64>>,
    mut leg: Legacy,
) -> Result<Legacy, SamplesError> {
    let n = s.addr.len();
    if [
        s.t_ns.len(),
        s.lat_e7.len(),
        s.lon_e7.len(),
        s.alt_ft.len(),
        s.vel_east_mps.len(),
        s.vel_north_mps.len(),
        s.coast.len(),
    ]
    .iter()
    .any(|&l| l != n)
    {
        return Err(SamplesError::ColumnLength);
    }
    let trip_at = |i: usize| -> Option<(u32, u32)> {
        let (&a, &t) = (s.addr.get(i)?, s.t_ns.get(i)?);
        Some((a, trip_of(starts, a, t.div_euclid(1_000_000))))
    };
    let keys: BTreeSet<(u32, u32)> = (0..n).filter_map(trip_at).collect();
    let mut flight_of = BTreeMap::new();
    for (a, trip) in keys {
        if let Some(f) = flight_ix(&mut leg, flight_key(a, trip)) {
            set_addr(&mut leg, f, a);
            flight_of.insert((a, trip), f);
        }
    }
    for i in 0..n {
        let (Some(&t), Some(&la), Some(&lo), Some(key)) =
            (s.t_ns.get(i), s.lat_e7.get(i), s.lon_e7.get(i), trip_at(i))
        else {
            leg.counts.note(DROP_NO_TRIP.into(), 1);
            continue;
        };
        let Some(&f) = flight_of.get(&key) else {
            leg.counts.note(DROP_FLIGHT_FULL.into(), 1);
            continue;
        };
        let alt = s.alt_ft.get(i).copied().unwrap_or(NO_ALT);
        let (ve, vn) = (
            s.vel_east_mps.get(i).copied().unwrap_or(f32::NAN),
            s.vel_north_mps.get(i).copied().unwrap_or(f32::NAN),
        );
        let vel = (ve.is_finite() && vn.is_finite()).then(|| {
            let dkt = |v: f32| {
                (f64::from(v) / MPS_PER_KT * 10.0)
                    .round()
                    .clamp(-32_000.0, 32_000.0) as i16
            };
            (dkt(ve), dkt(vn))
        });
        push_th(
            &mut leg,
            f,
            t.div_euclid(1_000_000),
            NO_TIME,
            la,
            lo,
            alt,
            vel,
            s.coast.get(i).copied().unwrap_or(false),
        );
    }
    Ok(leg)
}

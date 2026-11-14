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

use std::collections::BTreeSet;

use crate::legacy::{flight_ix, new_legacy, push_th, set_addr, Legacy, NO_ALT, NO_TIME};

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

/// The legacy table of a tracker's output. Refuses columns of different lengths (E5).
pub fn legacy_from_track_samples(s: &TrackSamples<'_>) -> Result<Legacy, SamplesError> {
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
    let mut leg = new_legacy();
    let addrs: BTreeSet<u32> = s.addr.iter().copied().collect();
    let mut flight_of = std::collections::BTreeMap::new();
    for a in addrs {
        if let Some(f) = flight_ix(&mut leg, u128::from(a) | (1u128 << 100)) {
            set_addr(&mut leg, f, a);
            flight_of.insert(a, f);
        }
    }
    for i in 0..n {
        let (Some(&a), Some(&t), Some(&la), Some(&lo)) = (
            s.addr.get(i),
            s.t_ns.get(i),
            s.lat_e7.get(i),
            s.lon_e7.get(i),
        ) else {
            continue;
        };
        let Some(&f) = flight_of.get(&a) else {
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

//! A synthetic report stream at a chosen rate per aircraft, from the points of a trace archive.
//! Shell (B3): no I/O, but a measurement tool's input, not a core function.
//!
//! The archive keeps about one point per aircraft every few seconds; a live feed gives up to two
//! position messages a second. Between two consecutive points of one aircraft, no more than
//! [`MAX_GAP_NS`] apart, points are added at `rate_hz` by linear interpolation of position,
//! altitude, vertical rate and velocity; every field that is not interpolated (the quality
//! fields, covariances, identity, sensor, flags) is the earlier point's. The kept points
//! themselves are passed on unchanged. Nothing is added after an aircraft's last point or across
//! a longer gap. Times are in order; every row is "synthetic rate from interpolated trace points".

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use oa_track::{Measurement, M_HAS_VEL, NONE_I32};

/// The longest interval between two points that is bridged, ns.
pub const MAX_GAP_NS: i64 = 60_000_000_000;

/// The label of every figure measured on this stream.
pub const LABEL: &str = "synthetic rate from interpolated trace points";

/// The iterator over the stream, in time order.
// A7.2: cold struct, size guard waived
pub struct Synth<'a> {
    meas: &'a [Measurement],
    /// Indexes into `meas` of each aircraft's points, in time order.
    per: Vec<Vec<u32>>,
    /// Each aircraft's position: the segment, and the step within it (0 is the point itself).
    cur: Vec<(u32, i64)>,
    heap: BinaryHeap<Reverse<(i64, u32)>>,
    step_ns: i64,
}

fn lerp_i32(a: i32, b: i32, num: i64, den: i64) -> i32 {
    let d = (i128::from(b) - i128::from(a)) * i128::from(num) / i128::from(den);
    (i128::from(a) + d) as i32
}

fn lerp_f32(a: f32, b: f32, f: f64) -> f32 {
    (f64::from(a) + (f64::from(b) - f64::from(a)) * f) as f32
}

/// `meas` in time order at `rate_hz` (positive and finite) points per aircraft per second.
pub fn synth(meas: &[Measurement], rate_hz: f64) -> Result<Synth<'_>, String> {
    if !(rate_hz.is_finite() && rate_hz > 0.0 && rate_hz <= 1000.0) {
        return Err(format!("--rate-hz {rate_hz}: not in (0, 1000]"));
    }
    let step_ns = ((1e9 / rate_hz).round() as i64).max(1);
    let mut index: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut per: Vec<Vec<u32>> = Vec::new();
    for (i, m) in meas.iter().enumerate() {
        let k = *index.entry(m.addr).or_insert_with(|| {
            per.push(Vec::new());
            per.len() - 1
        });
        if let Some(v) = per.get_mut(k) {
            v.push(i as u32);
        }
    }
    let mut heap = BinaryHeap::new();
    for (k, v) in per.iter().enumerate() {
        if let Some(m) = v.first().and_then(|&i| meas.get(i as usize)) {
            heap.push(Reverse((m.toa_ns, k as u32)));
        }
    }
    let cur = vec![(0u32, 0i64); per.len()];
    Ok(Synth {
        meas,
        per,
        cur,
        heap,
        step_ns,
    })
}

impl Synth<'_> {
    fn point(&self, k: usize, seg: u32) -> Option<&Measurement> {
        let i = *self.per.get(k)?.get(seg as usize)?;
        self.meas.get(i as usize)
    }
}

impl Iterator for Synth<'_> {
    type Item = Measurement;

    fn next(&mut self) -> Option<Measurement> {
        let Reverse((_, k32)) = self.heap.pop()?;
        let k = k32 as usize;
        let (seg, step) = *self.cur.get(k)?;
        let a = *self.point(k, seg)?;
        let next = self.point(k, seg + 1).copied();
        let bridged = |b: &Measurement| {
            let gap = b.toa_ns - a.toa_ns;
            gap > 0 && gap <= MAX_GAP_NS
        };
        let out = if step == 0 {
            a
        } else {
            let b = next?;
            let (num, den) = (step * self.step_ns, b.toa_ns - a.toa_ns);
            let mut m = a;
            m.toa_ns = a.toa_ns + num;
            m.lat_e7 = lerp_i32(a.lat_e7, b.lat_e7, num, den);
            m.lon_e7 = lerp_i32(a.lon_e7, b.lon_e7, num, den);
            if a.alt_ft != NONE_I32 && b.alt_ft != NONE_I32 {
                m.alt_ft = lerp_i32(a.alt_ft, b.alt_ft, num, den);
            }
            if a.vrate_fpm != NONE_I32 && b.vrate_fpm != NONE_I32 {
                m.vrate_fpm = lerp_i32(a.vrate_fpm, b.vrate_fpm, num, den);
            }
            if a.flags & M_HAS_VEL != 0 && b.flags & M_HAS_VEL != 0 {
                let f = num as f64 / den as f64;
                m.vel_east_mps = lerp_f32(a.vel_east_mps, b.vel_east_mps, f);
                m.vel_north_mps = lerp_f32(a.vel_north_mps, b.vel_north_mps, f);
            }
            m
        };
        // The next event of this aircraft: another synthetic point in the segment if one fits
        // strictly before the next kept point, else the next kept point.
        let more =
            next.is_some_and(|b| bridged(&b) && a.toa_ns + (step + 1) * self.step_ns < b.toa_ns);
        let (ns, nstep, nt) = if more {
            (seg, step + 1, Some(a.toa_ns + (step + 1) * self.step_ns))
        } else {
            (seg + 1, 0, next.map(|b| b.toa_ns))
        };
        if let (Some(c), Some(t)) = (self.cur.get_mut(k), nt) {
            *c = (ns, nstep);
            self.heap.push(Reverse((t, k32)));
        }
        Some(out)
    }
}

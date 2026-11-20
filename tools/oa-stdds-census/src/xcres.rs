//! The cross-check's results: outcome counts and difference distributions, by flag, for all
//! updates, by track class and by facility. Counts and distributions only.

use crate::dist::{summarize, Summary};
use crate::xcheck::{Params, MIN_GS_FOR_TIME_MPS, NM_M};

/// One exactly-one match at the primary gate; a field is NaN when it could not be formed.
#[derive(Clone, Copy, Debug)]
pub struct MatchRec {
    pub flag: bool,
    pub dh: f32,
    pub dv: f32,
    pub along: f32,
    pub cross: f32,
    pub along_s: f32,
}

/// Index of (flag, gate, outcome) in the 18 outcome counters.
pub fn cidx(flag: usize, gate: usize, out: usize) -> usize {
    flag * 9 + gate * 3 + out
}

/// The five difference quantities, in the order of [`XcGroup::sums`] within a flag.
pub const Q_NAMES: [&str; 5] = [
    "horizontal difference, metres",
    "vertical difference STDDS minus ADS-B, feet",
    "along-track difference, metres (positive: STDDS ahead)",
    "cross-track difference, metres (positive: STDDS right of the ADS-B track)",
    "along-track difference as time, seconds (aircraft at 100 kt or more)",
];

/// The histogram range of quantity `q` and of the vertical differences of horizontal-only
/// matches: wide enough to show an altitude unit error.
pub fn q_range(q: usize, p: &Params) -> (f64, f64) {
    let h = p.gates_nm.get(p.primary).map_or(1.0, |g| g * NM_M);
    match q {
        0 => (0.0, h),
        1 => (-p.v_gate_ft, p.v_gate_ft),
        2 | 3 => (-h, h),
        _ => (-h / MIN_GS_FOR_TIME_MPS, h / MIN_GS_FOR_TIME_MPS),
    }
}

/// Range of the horizontal-only vertical differences, feet.
pub const HDV_RANGE_FT: f64 = 5000.0;

/// Accumulator for one group while the tracks are walked.
pub struct XcAcc {
    counts: [u64; 18],
    hcounts: [u64; 6],
    vals: Vec<Vec<f32>>,
    hdv: [Vec<f32>; 2],
}

impl Default for XcAcc {
    fn default() -> XcAcc {
        XcAcc {
            counts: [0; 18],
            hcounts: [0; 6],
            vals: vec![Vec::new(); 10],
            hdv: [Vec::new(), Vec::new()],
        }
    }
}

impl XcAcc {
    pub fn add_track(
        &mut self,
        cnt: &[u32; 18],
        hcnt: &[u32; 6],
        recs: &[MatchRec],
        hdv: &[(bool, f32)],
    ) {
        for (a, &b) in self.counts.iter_mut().zip(cnt) {
            *a += u64::from(b);
        }
        for (a, &b) in self.hcounts.iter_mut().zip(hcnt) {
            *a += u64::from(b);
        }
        for r in recs {
            let f = usize::from(r.flag);
            for (q, v) in [r.dh, r.dv, r.along, r.cross, r.along_s]
                .into_iter()
                .enumerate()
            {
                if !v.is_nan() {
                    if let Some(vs) = self.vals.get_mut(f * 5 + q) {
                        vs.push(v);
                    }
                }
            }
        }
        for &(flag, v) in hdv {
            if let Some(vs) = self.hdv.get_mut(usize::from(flag)) {
                vs.push(v);
            }
        }
    }

    pub fn finish(mut self, p: &Params) -> XcGroup {
        let sums = self
            .vals
            .iter_mut()
            .enumerate()
            .map(|(i, v)| {
                let (lo, hi) = q_range(i % 5, p);
                summarize(v, lo, hi)
            })
            .collect();
        let [h0, h1] = &mut self.hdv;
        XcGroup {
            counts: self.counts,
            hcounts: self.hcounts,
            sums,
            hdv: [
                summarize(h0, -HDV_RANGE_FT, HDV_RANGE_FT),
                summarize(h1, -HDV_RANGE_FT, HDV_RANGE_FT),
            ],
        }
    }
}

/// The cross-check of one set of updates.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct XcGroup {
    /// Outcomes by flag, gate, outcome; see [`cidx`].
    pub counts: [u64; 18],
    /// Outcomes by flag (false, true) and outcome (none, one, ambiguous) with the horizontal gate
    /// of the primary gate and no altitude gate.
    pub hcounts: [u64; 6],
    /// Difference distributions of the exactly-one matches at the primary gate, `flag * 5 + q`.
    pub sums: Vec<Summary>,
    /// Vertical differences of the exactly-one horizontal-only matches, by flag.
    pub hdv: [Summary; 2],
}

/// The whole cross-check.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct XcResults {
    pub params: Params,
    pub points: u64,
    pub aircraft: u64,
    pub segments: u64,
    /// Updates not checked: no position or time, by flag; and no flag.
    pub no_position: [u64; 2],
    pub no_flag: u64,
    pub global: XcGroup,
    pub classes: Vec<XcGroup>,
    /// In the order of the census facilities.
    pub facs: Vec<XcGroup>,
}

//! The cross-check's results: outcome counts and difference distributions, by flag, for all
//! updates, by track class and by facility. Counts and distributions only.

use crate::dist::{summarize, Summary};
use crate::xcheck::{ClipStats, Params, Window, MIN_GS_FOR_TIME_MPS, NM_M};

/// Seconds the placebo passes shift the STDDS time (positions unchanged).
pub const PLACEBO_S: i64 = 60;
/// A track's median difference is taken only from this many matches.
pub const MIN_TRACK_MATCHES: usize = 3;
/// A cell with fewer matches than this prints "n<30" instead of medians.
pub const MIN_CELL_N: u64 = 30;

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

/// The histogram range of quantity `q`: the gate, inclusive at its upper edge.
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

/// Updates not cross-checked, by reason (flag index 0 false, 1 true).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotChecked {
    /// No position or no `mrtTime`.
    pub no_position: [u64; 2],
    /// Latitude above 85 degrees.
    pub polar: [u64; 2],
    /// No `trackNum` (so no track to attach the result to).
    pub no_track: [u64; 2],
    /// An exact repeat of the track's previous update (time and position).
    pub duplicate: [u64; 2],
    /// No `adsb` element at all.
    pub no_flag: u64,
}

/// What the cross-check keeps per track until the end.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct TrackXc {
    pub cnt: [u32; 18],
    pub hcnt: [u32; 6],
    /// Updates with any indexed ADS-B aircraft within 30 NM, by flag.
    pub cov: [u32; 2],
    /// Placebo outcomes at the primary gate: (shift index * 2 + flag) * 3 + outcome.
    pub pcnt: [u32; 12],
    pub pdh: Vec<(u8, f32)>,
    pub recs: Vec<MatchRec>,
    pub hdv: Vec<(bool, f32)>,
}

fn median(v: &mut [f32]) -> Option<f32> {
    v.sort_by(|a, b| oa_math::cmp_canon_f32(*a, *b));
    let n = v.len();
    v.get(n.div_ceil(2).saturating_sub(1)).copied()
}

/// Accumulator for one group while the tracks are walked.
pub struct XcAcc {
    counts: [u64; 18],
    hcounts: [u64; 6],
    covered: [u64; 2],
    pcounts: [u64; 12],
    pvals: Vec<Vec<f32>>,
    vals: Vec<Vec<f32>>,
    tvals: Vec<Vec<f32>>,
    hdv: [Vec<f32>; 2],
    tracks_matched: [u64; 2],
}

impl Default for XcAcc {
    fn default() -> XcAcc {
        XcAcc {
            counts: [0; 18],
            hcounts: [0; 6],
            covered: [0; 2],
            pcounts: [0; 12],
            pvals: vec![Vec::new(); 4],
            vals: vec![Vec::new(); 10],
            tvals: vec![Vec::new(); 10],
            hdv: [Vec::new(), Vec::new()],
            tracks_matched: [0; 2],
        }
    }
}

impl XcAcc {
    pub fn add_track(&mut self, t: &TrackXc) {
        for (a, &b) in self.counts.iter_mut().zip(&t.cnt) {
            *a += u64::from(b);
        }
        for (a, &b) in self.hcounts.iter_mut().zip(&t.hcnt) {
            *a += u64::from(b);
        }
        for (a, &b) in self.covered.iter_mut().zip(&t.cov) {
            *a += u64::from(b);
        }
        for (a, &b) in self.pcounts.iter_mut().zip(&t.pcnt) {
            *a += u64::from(b);
        }
        for &(k, v) in &t.pdh {
            if let Some(vs) = self.pvals.get_mut(usize::from(k)) {
                vs.push(v);
            }
        }
        let mut per: Vec<Vec<f32>> = vec![Vec::new(); 10];
        let mut n_flag = [0usize; 2];
        for r in &t.recs {
            let f = usize::from(r.flag);
            if let Some(c) = n_flag.get_mut(f) {
                *c += 1;
            }
            for (q, v) in [r.dh, r.dv, r.along, r.cross, r.along_s]
                .into_iter()
                .enumerate()
            {
                if !v.is_nan() {
                    if let Some(vs) = self.vals.get_mut(f * 5 + q) {
                        vs.push(v);
                    }
                    if let Some(vs) = per.get_mut(f * 5 + q) {
                        vs.push(v);
                    }
                }
            }
        }
        for (f, &n) in n_flag.iter().enumerate() {
            if n == 0 {
                continue;
            }
            if let Some(c) = self.tracks_matched.get_mut(f) {
                *c += 1;
            }
            if n < MIN_TRACK_MATCHES {
                continue;
            }
            for q in 0..5 {
                let m = per.get_mut(f * 5 + q).and_then(|v| median(v));
                if let (Some(m), Some(tv)) = (m, self.tvals.get_mut(f * 5 + q)) {
                    tv.push(m);
                }
            }
        }
        for &(flag, v) in &t.hdv {
            if let Some(vs) = self.hdv.get_mut(usize::from(flag)) {
                vs.push(v);
            }
        }
    }

    pub fn finish(mut self, p: &Params) -> XcGroup {
        let range = |i: usize| q_range(i % 5, p);
        let sums = self
            .vals
            .iter_mut()
            .enumerate()
            .map(|(i, v)| {
                let (lo, hi) = range(i);
                summarize(v, lo, hi)
            })
            .collect();
        let tsums = self
            .tvals
            .iter_mut()
            .enumerate()
            .map(|(i, v)| {
                let (lo, hi) = range(i);
                summarize(v, lo, hi)
            })
            .collect();
        let (hlo, hhi) = q_range(0, p);
        let psums = self
            .pvals
            .iter_mut()
            .map(|v| summarize(v, hlo, hhi))
            .collect();
        let [h0, h1] = &mut self.hdv;
        XcGroup {
            counts: self.counts,
            hcounts: self.hcounts,
            covered: self.covered,
            pcounts: self.pcounts,
            psums,
            sums,
            tsums,
            tracks_matched: self.tracks_matched,
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
    /// Updates with any indexed ADS-B aircraft within 30 NM at their time, by flag.
    pub covered: [u64; 2],
    /// Placebo outcomes at the primary gate with the time shifted +60 s then -60 s:
    /// (shift * 2 + flag) * 3 + outcome.
    pub pcounts: [u64; 12],
    /// Horizontal differences of the placebo exactly-one matches, `shift * 2 + flag`.
    pub psums: Vec<Summary>,
    /// Difference distributions of the exactly-one matches at the primary gate, `flag * 5 + q`.
    pub sums: Vec<Summary>,
    /// The same over tracks: each track's median (tracks with 3 or more matches).
    pub tsums: Vec<Summary>,
    /// Tracks with at least one match, by flag.
    pub tracks_matched: [u64; 2],
    /// Vertical differences of the exactly-one horizontal-only matches, by flag.
    pub hdv: [Summary; 2],
}

/// The whole cross-check.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct XcResults {
    pub params: Params,
    pub window: Option<Window>,
    pub clip: ClipStats,
    pub points: u64,
    pub aircraft: u64,
    pub segments: u64,
    pub segments_fast: u64,
    pub segments_polar: u64,
    pub not_checked: NotChecked,
    /// Distinct ADS-B aircraft that were an exactly-one match, by flag.
    pub matched_aircraft: [u64; 2],
    /// Bytes held by the index and the cross-check (an estimate).
    pub index_bytes: u64,
    pub held_bytes: u64,
    pub global: XcGroup,
    pub classes: Vec<XcGroup>,
    /// In the order of the census facilities.
    pub facs: Vec<XcGroup>,
}

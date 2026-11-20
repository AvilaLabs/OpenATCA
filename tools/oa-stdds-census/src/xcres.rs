//! The cross-check's results: outcome counts and difference distributions, by flag, for all
//! updates, by track class and by facility. Counts and distributions only.

use crate::dist::{summarize, Summary};
use crate::xcheck::{ClipStats, Params, Window, MIN_GS_FOR_TIME_MPS, NM_M};

/// Seconds the placebo passes shift the STDDS time (positions unchanged).
pub const PLACEBO_S: i64 = 60;
/// A UTC time as `YYYY-MM-DD hh:mm:ss` (an aggregate time of the data, never of one aircraft).
pub fn utc(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let sec = ms.rem_euclid(86_400_000) / 1000;
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        sec / 3600,
        sec % 3600 / 60,
        sec % 60
    )
}
/// The span of a recording as minutes: the start rounded down and the end rounded up to the
/// minute, the span of the recording and not the time of any aircraft.
pub fn minute_span(s: Option<(i64, i64)>) -> String {
    const MIN: i64 = 60_000;
    s.map_or_else(
        || "none".to_string(),
        |(a, b)| {
            let lo = a.div_euclid(MIN) * MIN;
            let hi = (b + MIN - 1).div_euclid(MIN) * MIN;
            format!(
                "{} to {} UTC",
                utc(lo).get(..16).unwrap_or_default(),
                utc(hi).get(..16).unwrap_or_default()
            )
        },
    )
}

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
    /// Time between the two ADS-B reports that bracket the update, ms.
    pub gap_ms: u32,
}

/// Upper edges of the bracket-gap classes, ms; a last class holds the rest.
pub const GAP_EDGES_MS: [u32; 3] = [2000, 5000, 10_000];
/// Names of the bracket-gap classes.
pub const GAP_NAMES: [&str; 4] = ["up to 2 s", "2 to 5 s", "5 to 10 s", "over 10 s"];

/// Index of the bracket-gap class of `gap_ms`.
pub fn gap_class(gap_ms: u32) -> usize {
    GAP_EDGES_MS
        .iter()
        .position(|&e| gap_ms <= e)
        .unwrap_or(GAP_EDGES_MS.len())
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

/// The histogram range of quantity `q` for the address-associated differences: wide, so the
/// gate does not cut them.
pub fn addr_range(q: usize, p: &Params) -> (f64, f64) {
    match q {
        0 => (0.0, ADDR_RANGE_M),
        1 => (-p.v_gate_ft * 10.0, p.v_gate_ft * 10.0),
        2 | 3 => (-ADDR_RANGE_M, ADDR_RANGE_M),
        _ => (
            -ADDR_RANGE_M / MIN_GS_FOR_TIME_MPS,
            ADDR_RANGE_M / MIN_GS_FOR_TIME_MPS,
        ),
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
    /// A pseudo, frozen or coasting track update (not a measured position).
    pub status: [u64; 2],
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
    /// adsb=true updates with an address: the aircraft of that address, found by address.
    pub addr_n: u32,
    pub addr: Vec<MatchRec>,
}

impl TrackXc {
    /// Whether nothing was kept (a segment with nothing to add).
    pub fn is_empty(&self) -> bool {
        self.cnt.iter().all(|&c| c == 0)
            && self.pdh.is_empty()
            && self.recs.is_empty()
            && self.hdv.is_empty()
            && self.addr_n == 0
    }
}

/// Range of the address-associated horizontal and along/cross differences, metres: the widest
/// gate, so nothing is cut at the primary gate.
pub const ADDR_RANGE_M: f64 = 10.0 * NM_M;

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
    gap_counts: [[u64; 4]; 2],
    addr_n: u64,
    addr_found: u64,
    avals: Vec<Vec<f32>>,
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
            gap_counts: [[0; 4]; 2],
            addr_n: 0,
            addr_found: 0,
            avals: vec![Vec::new(); 5],
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
        self.addr_n += u64::from(t.addr_n);
        for r in &t.addr {
            self.addr_found += 1;
            for (q, v) in [r.dh, r.dv, r.along, r.cross, r.along_s]
                .into_iter()
                .enumerate()
            {
                if !v.is_nan() {
                    if let Some(vs) = self.avals.get_mut(q) {
                        vs.push(v);
                    }
                }
            }
        }
        for r in &t.recs {
            let f = usize::from(r.flag);
            if let Some(c) = self
                .gap_counts
                .get_mut(f)
                .and_then(|g| g.get_mut(gap_class(r.gap_ms)))
            {
                *c += 1;
            }
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
        let asums = self
            .avals
            .iter_mut()
            .enumerate()
            .map(|(q, v)| {
                let (lo, hi) = addr_range(q, p);
                summarize(v, lo, hi)
            })
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
            gap_counts: self.gap_counts,
            addr_n: self.addr_n,
            addr_found: self.addr_found,
            asums,
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
    /// Track segments with at least one match, by flag. A segment is a run of one track number
    /// that no gap over 120 s, new flag or run of backwards times has cut.
    pub tracks_matched: [u64; 2],
    /// Exactly-one matches by flag and bracket-gap class ([`GAP_NAMES`]).
    pub gap_counts: [[u64; 4]; 2],
    /// adsb=true updates with a Mode S address checked by address, and those whose aircraft
    /// had a bracket at that time.
    pub addr_n: u64,
    pub addr_found: u64,
    /// The differences of those, in the order of [`Q_NAMES`], ranges from [`addr_range`].
    pub asums: Vec<Summary>,
    /// Vertical differences of the exactly-one horizontal-only matches, by flag.
    pub hdv: [Summary; 2],
}

/// The whole cross-check.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct XcResults {
    pub params: Params,
    pub window: Option<Window>,
    /// Earliest and latest STDDS update counted, UTC ms.
    pub stdds_span_ms: Option<(i64, i64)>,
    pub clip: ClipStats,
    pub points: u64,
    pub aircraft: u64,
    pub segments: u64,
    pub segments_fast: u64,
    pub segments_polar: u64,
    pub segments_long: u64,
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

impl XcResults {
    /// `Err` if the STDDS updates and the ADS-B clip do not overlap in time (a wrong day or
    /// clip), or either has none; the text gives the two spans (aggregate times only).
    pub fn check_overlap(&self) -> Result<(), String> {
        match (self.stdds_span_ms, self.clip.span_ms) {
            (Some((a, b)), Some((c, d))) if a <= d && c <= b => Ok(()),
            (None, _) if self.window.is_some() => {
                Err("no STDDS updates inside the window".to_string())
            }
            (None, _) => Err("no STDDS updates with a time".to_string()),
            (_, None) => Err("no ADS-B reports in the clip".to_string()),
            (s, c) => Err(format!(
                "the STDDS updates ({}) and the ADS-B clip ({}) do not overlap in time: wrong day or clip",
                minute_span(s),
                minute_span(c)
            )),
        }
    }
}

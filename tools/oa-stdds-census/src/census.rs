//! The census accumulator: per-update counts live, per-track interval lists kept compactly and
//! reduced to distributions at the end. Pure over the parsed [`TaisRecord`]s it is given; it
//! holds only counts and distributions in its output, never a position, address, code,
//! callsign or timestamp of an aircraft (the per-track interval lists are in memory only, as
//! durations, and are dropped by [`Census::finish`]).

use crate::hist::{Hist, MAX_TICKS, TICK_MS};
use crate::xcheck::{outcome, Cand, Index, MIN_GS_FOR_TIME_MPS, NM_M};
use crate::xcres::{cidx, MatchRec, XcAcc, XcGroup, XcResults};
use oa_stdds::tais::tais_bit;
use oa_stdds::{TaisField, TaisRecord};
use std::collections::{HashMap, HashSet};

/// Stored interval entry: no interval for this update (first after a break, no time, backwards
/// time, longer than 120 s, or no flag).
const GAP: u16 = 0xFFFF;
/// The ADS-B flag of the update that ends the interval.
const FLAG_BIT: u16 = 0x8000;
/// The interval spans a flag change (the previous update had the other flag).
const ACROSS_BIT: u16 = 0x4000;
const TICK_MASK: u16 = 0x3FFF;

/// Longest interval kept, in milliseconds; a longer gap is a reset (a track number reused).
const MAX_INTERVAL_MS: i64 = MAX_TICKS as i64 * TICK_MS;

/// Flag index: `false`, `true`, absent.
pub const FLAG_NAMES: [&str; 3] = ["adsb=false", "adsb=true", "adsb absent"];

/// Track classes by the flags seen on the track's updates.
pub const CLASS_NAMES: [&str; 4] = [
    "fewer than 2 flagged updates",
    "all adsb=false",
    "all adsb=true",
    "mixed (flag changes)",
];

/// Bins of flag changes per track: 0, 1, 2, 3-4, 5-9, 10 and more.
pub const CHANGE_BIN_NAMES: [&str; 6] = ["0", "1", "2", "3-4", "5-9", "10+"];

/// Counts over the updates of one flag.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdShare {
    pub n: u64,
    /// Updates with an address field (any value), of those the ones that read zero (a
    /// placeholder, not an address), and the ones with a non-zero address.
    pub icao_field: u64,
    pub icao_zero: u64,
    pub icao: u64,
    pub beacon: u64,
    pub beacon_1200: u64,
    pub acid: u64,
    pub plan: u64,
    pub icao_and_beacon: u64,
    pub none_of_icao_beacon_acid: u64,
    pub pseudo: u64,
    pub frozen: u64,
    pub is_new: u64,
}

fn has(r: &TaisRecord, f: TaisField) -> bool {
    r.present & tais_bit(f) != 0
}

/// Bits of every `flightPlan/*` field.
fn plan_mask() -> u64 {
    (tais_bit(TaisField::PlanStatus) << 1) - tais_bit(TaisField::Acid)
}

impl IdShare {
    fn add(&mut self, r: &TaisRecord) {
        let field = has(r, TaisField::AcAddress);
        let icao = field && r.mode_s_addr != 0;
        let beacon = has(r, TaisField::ReportedBeacon);
        let acid = has(r, TaisField::Acid);
        self.n += 1;
        self.icao_field += u64::from(field);
        self.icao_zero += u64::from(field && !icao);
        self.icao += u64::from(icao);
        self.beacon += u64::from(beacon);
        self.beacon_1200 += u64::from(beacon && r.reported_beacon == 1200);
        self.acid += u64::from(acid);
        self.plan += u64::from(r.present & plan_mask() != 0);
        self.icao_and_beacon += u64::from(icao && beacon);
        self.none_of_icao_beacon_acid += u64::from(!icao && !beacon && !acid);
        self.pseudo += u64::from(has(r, TaisField::TrackPseudo) && r.pseudo);
        self.frozen += u64::from(has(r, TaisField::TrackFrozen) && r.frozen);
        self.is_new += u64::from(has(r, TaisField::TrackNew) && r.is_new);
    }
}

/// The counts and distributions of one set of updates (all, or one facility).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct Group {
    /// Updates with the flag false, true, absent.
    pub updates: [u64; 3],
    pub ids: [IdShare; 2],
    /// Interval distributions by the flag of the update that ends the interval; intervals that
    /// span a flag change are kept apart (see [`Results::dirs`]).
    pub iv: [Hist; 2],
    /// Updates with no `mrtTime`, with a time before the track's previous one, and with a gap
    /// over 120 s from it (a reset).
    pub no_mrt: u64,
    pub backwards: u64,
    pub resets: u64,
    /// Tracks seen, tracks with at least one adsb=false update, and of those the tracks whose
    /// adsb=false updates ever carried a non-zero Mode S address.
    pub tracks: u64,
    pub tracks_with_false: u64,
    pub tracks_false_with_icao: u64,
}

impl Group {
    fn observe(&mut self, r: &TaisRecord, flag: Option<bool>) {
        let i = flag_index(flag);
        if let Some(c) = self.updates.get_mut(i) {
            *c += 1;
        }
        if let Some(f) = flag {
            if let Some(s) = self.ids.get_mut(usize::from(f)) {
                s.add(r);
            }
        }
    }
}

fn flag_index(flag: Option<bool>) -> usize {
    flag.map_or(2, usize::from)
}

/// A track's identity: facility index, `recSTARSSrcID`, track number.
type TrackKey = (u32, [u8; 8], u32);

struct Track {
    fac: u32,
    updates: u32,
    last_ms: Option<i64>,
    last_flag: Option<bool>,
    changes: u32,
    n: [u32; 2],
    false_icao: bool,
    false_nonzero: u32,
    ivs: Vec<u16>,
    /// Position cross-check: outcomes by flag, gate, outcome (see [`cidx`]), by flag and
    /// outcome for the horizontal-only check, the exactly-one matches, and the vertical
    /// differences of the horizontal-only matches. Empty or zero when the check is off.
    xc_cnt: [u32; 18],
    xc_hcnt: [u32; 6],
    xc_recs: Vec<MatchRec>,
    xc_hdv: Vec<(bool, f32)>,
}

fn nan_or(v: Option<f64>) -> f32 {
    v.map_or(f32::NAN, |x| x as f32)
}

/// Counts the outcomes of one update's candidates at every gate and keeps the match.
fn xc_observe(x: &Index, t: &mut Track, flag: bool, r: &TaisRecord) {
    let p = x.params;
    let alt = has(r, TaisField::ReportedAltitude).then_some(r.reported_alt_ft);
    let cands = x.candidates(r.mrt_time_ms, r.lat_e7, r.lon_e7, alt);
    let f = usize::from(flag);
    for (g, &nm) in p.gates_nm.iter().enumerate() {
        let n = cands
            .iter()
            .filter(|c| c.passes(nm * NM_M, p.v_gate_ft))
            .count();
        if let Some(c) = t.xc_cnt.get_mut(cidx(f, g, outcome(n))) {
            *c += 1;
        }
    }
    let h_m = p.gates_nm.get(p.primary).map_or(0.0, |g| g * NM_M);
    let h_only: Vec<&Cand> = cands.iter().filter(|c| c.dh_m <= h_m).collect();
    if let Some(c) = t.xc_hcnt.get_mut(f * 3 + outcome(h_only.len())) {
        *c += 1;
    }
    if let [c] = h_only.as_slice() {
        if let Some(v) = c.dv_ft {
            t.xc_hdv.push((flag, v as f32));
        }
    }
    let gated: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.passes(h_m, p.v_gate_ft))
        .collect();
    if let [c] = gated.as_slice() {
        let along_s = c
            .along_m
            .zip(c.gs_mps)
            .filter(|&(_, gs)| gs >= MIN_GS_FOR_TIME_MPS)
            .map(|(a, gs)| a / gs);
        t.xc_recs.push(MatchRec {
            flag,
            dh: c.dh_m as f32,
            dv: nan_or(c.dv_ft),
            along: nan_or(c.along_m),
            cross: nan_or(c.cross_m),
            along_s: nan_or(along_s),
        });
    }
}

/// The running census.
pub struct Census {
    fac_ix: HashMap<[u8; 8], u32>,
    fac_keys: Vec<[u8; 8]>,
    global: Group,
    facs: Vec<Group>,
    tracks: HashMap<TrackKey, Track>,
    src_ids: HashSet<[u8; 8]>,
    no_track_num: u64,
    xc: Option<Index>,
    /// Updates not cross-checked: no position or time (by flag), and no flag.
    xc_nopos: [u64; 2],
    xc_noflag: u64,
}

impl Default for Census {
    fn default() -> Census {
        Census::new()
    }
}

/// Per-class distributions.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct ClassStats {
    pub tracks: u64,
    pub updates: [u64; 2],
    /// adsb=false updates of the class's tracks that carry a non-zero address.
    pub false_nonzero: u64,
    pub iv: [Hist; 2],
}

/// Flag-change intervals for one direction (0: false to true, 1: true to false): the interval
/// across the change, the one before it (ending at the last update of the old flag), and the
/// one after it (the next interval after the change update).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct DirStats {
    pub changes: u64,
    pub across: Hist,
    pub before: Hist,
    pub after: Hist,
}

/// The finished census.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Results {
    pub global: Group,
    /// Facilities as the source id the data carries (zero padded; all zero is "no src"), with
    /// their groups.
    pub facs: Vec<([u8; 8], Group)>,
    pub classes: [ClassStats; 4],
    pub dirs: [DirStats; 2],
    /// Flag changes in all, and tracks with at least one.
    pub changes: u64,
    pub tracks_with_change: u64,
    /// Over tracks with two or more flagged updates, the tracks by number of changes.
    pub change_bins: [u64; 6],
    /// Consecutive flagged update pairs on tracks (the chances for a change).
    pub flagged_pairs: u64,
    pub distinct_src_ids: u64,
    pub no_track_num: u64,
    /// The position cross-check, if an ADS-B index was given.
    pub xc: Option<XcResults>,
}

impl Census {
    pub fn new() -> Census {
        Census {
            fac_ix: HashMap::new(),
            fac_keys: Vec::new(),
            global: Group::default(),
            facs: Vec::new(),
            tracks: HashMap::new(),
            src_ids: HashSet::new(),
            no_track_num: 0,
            xc: None,
            xc_nopos: [0; 2],
            xc_noflag: 0,
        }
    }

    /// A census that also cross-checks every tracked update against the ADS-B `index`.
    pub fn with_xcheck(index: Index) -> Census {
        Census {
            xc: Some(index),
            ..Census::new()
        }
    }

    /// Updates counted so far.
    pub fn updates(&self) -> u64 {
        self.global.updates.iter().sum()
    }

    /// Takes one TAIS record of the message whose producing facility is `src` (all zero when the
    /// message carries none).
    pub fn observe(&mut self, src: [u8; 8], r: &TaisRecord) {
        let flag = has(r, TaisField::TrackAdsb).then_some(r.adsb);
        let fac = match self.fac_ix.get(&src) {
            Some(&f) => f,
            None => {
                let f = u32::try_from(self.fac_keys.len()).unwrap_or(u32::MAX);
                self.fac_ix.insert(src, f);
                self.fac_keys.push(src);
                self.facs.push(Group::default());
                f
            }
        };
        self.global.observe(r, flag);
        if let Some(g) = self.facs.get_mut(fac as usize) {
            g.observe(r, flag);
        }
        self.src_ids.insert(r.rec_stars_src_id);
        if !has(r, TaisField::TrackNum) {
            self.no_track_num += 1;
            return;
        }
        let key = (fac, r.rec_stars_src_id, r.track_num);
        let t = self.tracks.entry(key).or_insert_with(|| Track {
            fac,
            updates: 0,
            last_ms: None,
            last_flag: None,
            changes: 0,
            n: [0; 2],
            false_icao: false,
            false_nonzero: 0,
            ivs: Vec::new(),
            xc_cnt: [0; 18],
            xc_hcnt: [0; 6],
            xc_recs: Vec::new(),
            xc_hdv: Vec::new(),
        });
        match (self.xc.as_ref(), flag) {
            (Some(x), Some(f)) => {
                if has(r, TaisField::Lat) && has(r, TaisField::Lon) && has(r, TaisField::MrtTime) {
                    xc_observe(x, t, f, r);
                } else if let Some(c) = self.xc_nopos.get_mut(usize::from(f)) {
                    *c += 1;
                }
            }
            (Some(_), None) => self.xc_noflag += 1,
            _ => {}
        }
        let mrt = has(r, TaisField::MrtTime).then_some(r.mrt_time_ms);
        let mut entry = GAP;
        let mut counter: Option<fn(&mut Group)> = None;
        match (mrt, flag) {
            (None, _) => {
                counter = Some(|g| g.no_mrt += 1);
                t.last_ms = None;
            }
            (Some(m), None) => t.last_ms = Some(m),
            (Some(m), Some(f)) => match t.last_ms {
                Some(l) if m < l => counter = Some(|g| g.backwards += 1),
                Some(l) if m - l > MAX_INTERVAL_MS => {
                    counter = Some(|g| g.resets += 1);
                    t.last_ms = Some(m);
                }
                Some(l) => {
                    let ticks = u16::try_from((m - l) / TICK_MS).unwrap_or(TICK_MASK);
                    let across = t.last_flag.is_some_and(|p| p != f);
                    entry = ticks.min(TICK_MASK)
                        | if f { FLAG_BIT } else { 0 }
                        | if across { ACROSS_BIT } else { 0 };
                    t.last_ms = Some(m);
                }
                None => t.last_ms = Some(m),
            },
        }
        if let Some(c) = counter {
            c(&mut self.global);
            if let Some(g) = self.facs.get_mut(fac as usize) {
                c(g);
            }
        }
        if t.updates > 0 {
            t.ivs.push(entry);
        }
        t.updates = t.updates.saturating_add(1);
        if let Some(f) = flag {
            if t.last_flag.is_some_and(|p| p != f) {
                t.changes += 1;
            }
            t.last_flag = Some(f);
            if let Some(c) = t.n.get_mut(usize::from(f)) {
                *c += 1;
            }
            let real = !f && has(r, TaisField::AcAddress) && r.mode_s_addr != 0;
            t.false_icao |= real;
            t.false_nonzero += u32::from(real);
        }
    }

    /// Reduces the per-track lists to the distributions and drops them.
    pub fn finish(self) -> Results {
        let Census {
            fac_keys,
            mut global,
            mut facs,
            tracks,
            src_ids,
            no_track_num,
            xc,
            xc_nopos,
            xc_noflag,
            ..
        } = self;
        let xc_params = xc.as_ref().map(|x| x.params);
        let mut accs: Vec<XcAcc> = Vec::new();
        if xc_params.is_some() {
            accs.resize_with(5 + fac_keys.len(), XcAcc::default);
        }
        let mut classes: [ClassStats; 4] = Default::default();
        let mut dirs: [DirStats; 2] = Default::default();
        let mut changes = 0u64;
        let mut tracks_with_change = 0u64;
        let mut change_bins = [0u64; 6];
        let mut flagged_pairs = 0u64;
        for t in tracks.values() {
            let [nf, nt] = t.n;
            let flagged = u64::from(nf) + u64::from(nt);
            let class = match (nf, nt) {
                _ if flagged < 2 => 0,
                (_, 0) => 1,
                (0, _) => 2,
                _ => 3,
            };
            if let Some(c) = classes.get_mut(class) {
                c.tracks += 1;
                c.updates[0] += u64::from(nf);
                c.updates[1] += u64::from(nt);
                c.false_nonzero += u64::from(t.false_nonzero);
            }
            if flagged >= 2 {
                flagged_pairs += flagged - 1;
                changes += u64::from(t.changes);
                tracks_with_change += u64::from(t.changes > 0);
                let bin = match t.changes {
                    0 => 0,
                    1 => 1,
                    2 => 2,
                    3..=4 => 3,
                    5..=9 => 4,
                    _ => 5,
                };
                if let Some(b) = change_bins.get_mut(bin) {
                    *b += 1;
                }
            }
            let fac = t.fac as usize;
            if xc_params.is_some() {
                for k in [0, 1 + class, 5 + fac] {
                    if let Some(a) = accs.get_mut(k) {
                        a.add_track(&t.xc_cnt, &t.xc_hcnt, &t.xc_recs, &t.xc_hdv);
                    }
                }
            }
            for g in std::iter::once(&mut global).chain(facs.get_mut(fac)) {
                g.tracks += 1;
                g.tracks_with_false += u64::from(nf > 0);
                g.tracks_false_with_icao += u64::from(t.false_icao);
            }
            for (i, &e) in t.ivs.iter().enumerate() {
                if e == GAP {
                    continue;
                }
                let ticks = usize::from(e & TICK_MASK);
                let flag = usize::from(e & FLAG_BIT != 0);
                if e & ACROSS_BIT != 0 {
                    // flag is the new flag: true means false to true (direction 0).
                    let d = 1 - flag;
                    let prev = i
                        .checked_sub(1)
                        .and_then(|p| t.ivs.get(p))
                        .filter(|&&p| p != GAP && p & ACROSS_BIT == 0);
                    let next = t
                        .ivs
                        .get(i + 1)
                        .filter(|&&p| p != GAP && p & ACROSS_BIT == 0);
                    if let Some(ds) = dirs.get_mut(d) {
                        ds.changes += 1;
                        ds.across.add(ticks);
                        if let Some(&p) = prev {
                            ds.before.add(usize::from(p & TICK_MASK));
                        }
                        if let Some(&n) = next {
                            ds.after.add(usize::from(n & TICK_MASK));
                        }
                    }
                    continue;
                }
                for h in [
                    global.iv.get_mut(flag),
                    facs.get_mut(fac).and_then(|g| g.iv.get_mut(flag)),
                    classes.get_mut(class).and_then(|c| c.iv.get_mut(flag)),
                ]
                .into_iter()
                .flatten()
                {
                    h.add(ticks);
                }
            }
        }
        let xc_res = xc.zip(xc_params).and_then(|(x, p)| {
            let mut it = accs.into_iter().map(|a| a.finish(&p));
            let global = it.next()?;
            let classes: Vec<XcGroup> = it.by_ref().take(4).collect();
            Some(XcResults {
                params: p,
                points: x.points() as u64,
                aircraft: x.aircraft,
                segments: x.segments,
                no_position: xc_nopos,
                no_flag: xc_noflag,
                global,
                classes,
                facs: it.collect(),
            })
        });
        let facs = fac_keys.into_iter().zip(facs).collect();
        Results {
            global,
            facs,
            classes,
            dirs,
            changes,
            tracks_with_change,
            change_bins,
            flagged_pairs,
            distinct_src_ids: src_ids.len() as u64,
            no_track_num,
            xc: xc_res,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
pub(crate) mod tests {
    use super::*;

    pub fn base() -> TaisRecord {
        TaisRecord {
            present: 0,
            errors: 0,
            invalidated: 0,
            present_empty: 0,
            offset: 0,
            mrt_time_ms: 0,
            lat_e7: 0,
            lon_e7: 0,
            reported_alt_ft: 0,
            requested_alt_ft: 0,
            assigned_alt_ft: 0,
            vx_cmps: 0,
            vy_cmps: 0,
            vvert_ftmin: 0,
            track_num: 0,
            sfpn: 0,
            mode_s_addr: 0,
            reported_beacon: 0,
            assigned_beacon: 0,
            track_status: [0; 8],
            acid: [0; 8],
            ac_type: [0; 8],
            airport: [0; 8],
            runway: [0; 8],
            flight_rules: [0; 8],
            raw_flight_rules: [0; 8],
            ecid: [0; 8],
            ptd_time: [0; 8],
            entry_fix: [0; 8],
            exit_fix: [0; 8],
            plan_type: [0; 8],
            plan_status: [0; 16],
            pseudo: false,
            frozen: false,
            is_new: false,
            adsb: false,
            rec_type: [0; 8],
            rec_src: [0; 8],
            rec_stars_src_id: [0; 8],
            rec_stars_tod_ms: 0,
            rec_safa_receipt_ms: 0,
        }
    }

    /// A record of track `n` at `t_ms` with the flag `adsb` (`None`: no flag element).
    pub fn rec(n: u32, t_ms: i64, adsb: Option<bool>) -> TaisRecord {
        let mut r = base();
        r.track_num = n;
        r.mrt_time_ms = t_ms;
        r.present = tais_bit(TaisField::TrackNum) | tais_bit(TaisField::MrtTime);
        if let Some(a) = adsb {
            r.adsb = a;
            r.present |= tais_bit(TaisField::TrackAdsb);
        }
        r
    }

    pub fn src(s: &str) -> [u8; 8] {
        let mut a = [0u8; 8];
        a[..s.len()].copy_from_slice(s.as_bytes());
        a
    }

    // REQ: OA-CEN-001
    #[test]
    fn intervals_split_by_flag() {
        let mut c = Census::new();
        // Track 1 (false): 4.6 s steps; track 2 (true): 1.0 s steps.
        for i in 0..5 {
            c.observe(src("AAA"), &rec(1, i * 4600, Some(false)));
            c.observe(src("AAA"), &rec(2, i * 1000, Some(true)));
        }
        let r = c.finish();
        assert_eq!(r.global.updates, [5, 5, 0]);
        assert_eq!(r.global.iv[0].n, 4);
        assert_eq!(r.global.iv[1].n, 4);
        assert!((r.global.iv[0].quantile(1, 2).unwrap() - 4.605).abs() < 1e-9);
        assert!((r.global.iv[1].quantile(1, 2).unwrap() - 1.005).abs() < 1e-9);
        assert_eq!(r.global.tracks, 2);
        assert_eq!(r.global.tracks_with_false, 1);
    }

    // REQ: OA-CEN-005
    #[test]
    fn absent_flag_is_not_false() {
        let mut c = Census::new();
        c.observe(src("AAA"), &rec(1, 0, None));
        c.observe(src("AAA"), &rec(1, 4000, None));
        c.observe(src("AAA"), &rec(1, 8000, Some(false)));
        let r = c.finish();
        assert_eq!(r.global.updates, [1, 0, 2]);
        assert_eq!(r.global.ids[0].n, 1);
        // Only the update with a flag has an interval; the absent-flag ones leave none.
        assert_eq!(r.global.iv[0].n, 1);
    }

    // REQ: OA-CEN-005
    #[test]
    fn missing_time_backwards_and_reset_are_counted() {
        let mut c = Census::new();
        let mut no_time = rec(1, 0, Some(false));
        no_time.present &= !tais_bit(TaisField::MrtTime);
        c.observe(src("AAA"), &rec(1, 100_000, Some(false)));
        c.observe(src("AAA"), &rec(1, 90_000, Some(false)));
        c.observe(src("AAA"), &no_time);
        c.observe(src("AAA"), &rec(1, 500_000, Some(false)));
        c.observe(src("AAA"), &rec(1, 700_000, Some(false)));
        c.observe(src("AAA"), &rec(1, 704_700, Some(false)));
        let r = c.finish();
        assert_eq!(r.global.no_mrt, 1);
        assert_eq!(r.global.backwards, 1);
        // 500_000 follows a missing time (no reference), 700_000 is 200 s later (reset).
        assert_eq!(r.global.resets, 1);
        assert_eq!(r.global.iv[0].n, 1);
        assert!((r.global.iv[0].quantile(1, 2).unwrap() - 4.705).abs() < 1e-9);
    }

    // REQ: OA-CEN-002
    #[test]
    fn flag_changes_and_either_side_intervals() {
        let mut c = Census::new();
        // false at 0, 4.6, 9.2; true at 10.2 (change, across 1.0); true at 11.2 (after 1.0);
        // false at 15.8 (change back, across 4.6, before 1.0).
        for (t, f) in [
            (0, false),
            (4600, false),
            (9200, false),
            (10_200, true),
            (11_200, true),
            (15_800, false),
        ] {
            c.observe(src("AAA"), &rec(7, t, Some(f)));
        }
        // A pure-false track and a single-update track.
        c.observe(src("AAA"), &rec(8, 0, Some(false)));
        c.observe(src("AAA"), &rec(8, 4700, Some(false)));
        c.observe(src("AAA"), &rec(9, 0, Some(true)));
        let r = c.finish();
        assert_eq!(r.changes, 2);
        assert_eq!(r.tracks_with_change, 1);
        assert_eq!(r.change_bins, [1, 0, 1, 0, 0, 0]);
        assert_eq!(r.flagged_pairs, 5 + 1);
        assert_eq!(r.dirs[0].changes, 1);
        assert_eq!(r.dirs[1].changes, 1);
        // false to true: before = 4.6 s (ends at the last false), across 1.0 s, after 1.0 s.
        assert!((r.dirs[0].before.quantile(1, 2).unwrap() - 4.605).abs() < 1e-9);
        assert!((r.dirs[0].across.quantile(1, 2).unwrap() - 1.005).abs() < 1e-9);
        assert!((r.dirs[0].after.quantile(1, 2).unwrap() - 1.005).abs() < 1e-9);
        // true to false: across 4.6 s, before 1.0 s, nothing after.
        assert!((r.dirs[1].across.quantile(1, 2).unwrap() - 4.605).abs() < 1e-9);
        assert!((r.dirs[1].before.quantile(1, 2).unwrap() - 1.005).abs() < 1e-9);
        assert_eq!(r.dirs[1].after.n, 0);
        // Across intervals stay out of the steady per-flag distributions.
        assert_eq!(r.global.iv[0].n, 2 + 1);
        assert_eq!(r.global.iv[1].n, 1);
    }

    // REQ: OA-CEN-008
    #[test]
    fn track_classes() {
        let mut c = Census::new();
        for (n, flags) in [
            (1u32, vec![false, false]),
            (2, vec![true, true, true]),
            (3, vec![false, true]),
            (4, vec![false]),
        ] {
            for (i, f) in flags.into_iter().enumerate() {
                c.observe(src("AAA"), &rec(n, i as i64 * 4000, Some(f)));
            }
        }
        let r = c.finish();
        assert_eq!(r.classes[0].tracks, 1);
        assert_eq!(r.classes[1].tracks, 1);
        assert_eq!(r.classes[2].tracks, 1);
        assert_eq!(r.classes[3].tracks, 1);
        assert_eq!(r.classes[1].updates, [2, 0]);
        assert_eq!(r.classes[2].updates, [0, 3]);
        assert_eq!(r.classes[3].updates, [1, 1]);
        assert_eq!(r.classes[1].iv[0].n, 1);
        assert_eq!(r.classes[2].iv[1].n, 2);
        assert_eq!(r.classes[3].iv[0].n + r.classes[3].iv[1].n, 0);
    }

    // REQ: OA-CEN-003
    #[test]
    fn identity_shares_of_false_updates() {
        let mut c = Census::new();
        let mut a = rec(1, 0, Some(false));
        a.present |= tais_bit(TaisField::AcAddress) | tais_bit(TaisField::ReportedBeacon);
        a.reported_beacon = 1200;
        a.mode_s_addr = 0x00AB_CDEF;
        let mut b = rec(2, 0, Some(false));
        b.present |= tais_bit(TaisField::ReportedBeacon) | tais_bit(TaisField::Acid);
        b.reported_beacon = 4521;
        let mut p = rec(3, 0, Some(false));
        p.present |= tais_bit(TaisField::Sfpn) | tais_bit(TaisField::TrackPseudo);
        p.pseudo = true;
        let n = rec(4, 0, Some(false));
        let mut z = rec(6, 0, Some(false));
        z.present |= tais_bit(TaisField::AcAddress);
        let mut t = rec(5, 0, Some(true));
        t.present |= tais_bit(TaisField::AcAddress);
        t.mode_s_addr = 1;
        for r in [a, b, p, n, t, z] {
            c.observe(src("AAA"), &r);
        }
        let s = c.finish().global.ids;
        let f = &s[0];
        assert_eq!(f.n, 5);
        assert_eq!(f.icao, 1);
        assert_eq!(f.icao_field, 2);
        assert_eq!(f.icao_zero, 1);
        assert_eq!(f.beacon, 2);
        assert_eq!(f.beacon_1200, 1);
        assert_eq!(f.acid, 1);
        assert_eq!(f.plan, 2);
        assert_eq!(f.icao_and_beacon, 1);
        assert_eq!(f.none_of_icao_beacon_acid, 3);
        assert_eq!(f.pseudo, 1);
        assert_eq!(s[1].n, 1);
        assert_eq!(s[1].icao, 1);
    }

    // REQ: OA-CEN-003
    #[test]
    fn tracks_whose_false_updates_carry_an_address() {
        let mut c = Census::new();
        let mut a = rec(1, 0, Some(false));
        a.present |= tais_bit(TaisField::AcAddress);
        a.mode_s_addr = 0x12_3456;
        c.observe(src("AAA"), &a);
        // A zero address field is no address.
        let mut z = rec(3, 0, Some(false));
        z.present |= tais_bit(TaisField::AcAddress);
        c.observe(src("AAA"), &z);
        c.observe(src("AAA"), &rec(1, 4600, Some(false)));
        // An address on an adsb=true update does not count.
        let mut b = rec(2, 0, Some(true));
        b.present |= tais_bit(TaisField::AcAddress);
        c.observe(src("AAA"), &b);
        c.observe(src("AAA"), &rec(2, 4600, Some(false)));
        let r = c.finish();
        assert_eq!(
            r.classes[0].false_nonzero + r.classes[1].false_nonzero + r.classes[3].false_nonzero,
            1
        );
        let g = r.global;
        assert_eq!(g.tracks, 3);
        assert_eq!(g.tracks_with_false, 3);
        assert_eq!(g.tracks_false_with_icao, 1);
    }

    // REQ: OA-CEN-004
    #[test]
    fn facilities_are_split_and_tracks_do_not_mix() {
        let mut c = Census::new();
        // Same track number at two facilities is two tracks.
        for i in 0..3 {
            c.observe(src("AAA"), &rec(1, i * 4600, Some(false)));
            c.observe(src("BBB"), &rec(1, i * 1000, Some(true)));
        }
        let mut other = rec(1, 0, Some(true));
        other.rec_stars_src_id = src("X1");
        c.observe(src("AAA"), &other);
        assert_eq!(c.updates(), 7);
        let r = c.finish();
        assert_eq!(r.facs.len(), 2);
        assert_eq!(r.distinct_src_ids, 2);
        let a = &r.facs.iter().find(|(k, _)| *k == src("AAA")).unwrap().1;
        let b = &r.facs.iter().find(|(k, _)| *k == src("BBB")).unwrap().1;
        assert_eq!(a.updates, [3, 1, 0]);
        assert_eq!(b.updates, [0, 3, 0]);
        assert_eq!(a.iv[0].n, 2);
        assert_eq!(b.iv[1].n, 2);
        assert_eq!(b.iv[0].n, 0);
        assert!((a.iv[0].quantile(1, 2).unwrap() - 4.605).abs() < 1e-9);
        // Track 1 at AAA with the two source ids, and track 1 at BBB.
        assert_eq!(r.global.tracks, 3);
    }

    // REQ: OA-CEN-005
    #[test]
    fn record_without_a_track_number_is_counted() {
        let mut c = Census::new();
        let mut r = rec(1, 0, Some(false));
        r.present &= !tais_bit(TaisField::TrackNum);
        c.observe(src("AAA"), &r);
        let r = c.finish();
        assert_eq!(r.no_track_num, 1);
        assert_eq!(r.global.updates[0], 1);
        assert_eq!(r.global.tracks, 0);
    }

    // --- position cross-check ---------------------------------------------------------------

    use crate::xcheck::tests::{deg_lat_per_m, deg_lon_per_m, pt};
    use crate::xcheck::{build_from_points, Params, Pt, NM_M};

    const V: f64 = 400.0 * NM_M / 3600.0;

    /// One aircraft flying east at 400 kt from `lon0` at 40 N, reporting at 1 Hz for 0 to 120 s.
    fn fly(icao: u32, lon0: f64, lat: f64, alt: i32) -> Vec<Pt> {
        (0..=120)
            .map(|s| {
                pt(
                    icao,
                    s * 1000,
                    lat,
                    lon0 + s as f64 * V * deg_lon_per_m(40.0),
                    alt,
                )
            })
            .collect()
    }

    /// An STDDS update of track `n` at `t_ms` at the position of an aircraft that left `lon0` at
    /// `lat` at 400 kt, moved `east_m` and `north_m` and with the clock `shift_s` seconds late
    /// (the position is where the aircraft is at `t + shift`).
    #[allow(clippy::too_many_arguments)]
    fn upd(
        n: u32,
        t_ms: i64,
        flag: Option<bool>,
        lon0: f64,
        lat: f64,
        alt: i32,
        east_m: f64,
        north_m: f64,
        shift_s: f64,
    ) -> TaisRecord {
        let mut r = rec(n, t_ms, flag);
        let ts = t_ms as f64 / 1000.0 + shift_s;
        r.lat_e7 = ((lat + north_m * deg_lat_per_m(40.0)) * 1e7).round() as i32;
        r.lon_e7 = ((lon0 + (ts * V + east_m) * deg_lon_per_m(40.0)) * 1e7).round() as i32;
        r.reported_alt_ft = alt;
        r.present |= tais_bit(TaisField::Lat)
            | tais_bit(TaisField::Lon)
            | tais_bit(TaisField::ReportedAltitude);
        r
    }

    fn xcensus(pts: Vec<Pt>) -> Census {
        Census::with_xcheck(build_from_points(pts, Params::default()).unwrap())
    }

    // REQ: OA-CEN-009
    #[test]
    fn counts_none_one_and_ambiguous_at_each_gate() {
        // Aircraft 1 is 0.7 NM from the update; aircraft 2 flies 1.5 NM north of it, 300 ft higher.
        let lat = 40.0;
        let mut pts = fly(1, -75.0, lat, 10_000);
        pts.extend(fly(
            2,
            -75.0,
            lat + 1.5 * NM_M * deg_lat_per_m(40.0),
            10_300,
        ));
        let mut c = xcensus(pts);
        // Update on aircraft 1's path, 0.7 NM north of it; altitude 10_000 ft.
        c.observe(
            src("AAA"),
            &upd(
                1,
                30_500,
                Some(false),
                -75.0,
                lat,
                10_000,
                0.0,
                0.7 * NM_M,
                0.0,
            ),
        );
        let r = c.finish();
        let x = r.xc.unwrap();
        let at = |f: usize, g: usize| -> Vec<u64> {
            (0..3)
                .map(|o| x.global.counts[crate::xcres::cidx(f, g, o)])
                .collect()
        };
        // 0.5 NM: nobody. 1 NM and 2 NM: aircraft 1 (0.7 NM away) and aircraft 2 (0.8 NM away,
        // 300 ft off, inside the 500 ft gate) are both candidates: ambiguous.
        assert_eq!(at(0, 0), [1, 0, 0]);
        assert_eq!(at(0, 1), [0, 0, 1]);
        assert_eq!(at(0, 2), [0, 0, 1]);
        assert_eq!(at(1, 1), [0, 0, 0]);
    }

    // REQ: OA-CEN-009
    #[test]
    fn the_altitude_gate_separates_two_candidates() {
        let lat = 40.0;
        let mut pts = fly(1, -75.0, lat, 10_000);
        pts.extend(fly(
            2,
            -75.0,
            lat + 0.5 * NM_M * deg_lat_per_m(40.0),
            11_000,
        ));
        let mut c = xcensus(pts);
        // Both are inside 1 NM; only aircraft 1 is inside 500 ft of the update's altitude.
        c.observe(
            src("AAA"),
            &upd(1, 30_000, Some(true), -75.0, lat, 10_100, 0.0, 0.0, 0.0),
        );
        // No altitude on the update: the altitude gate is not applied, so both are candidates.
        let mut no_alt = upd(2, 31_000, Some(true), -75.0, lat, 0, 0.0, 0.0, 0.0);
        no_alt.present &= !tais_bit(TaisField::ReportedAltitude);
        c.observe(src("AAA"), &no_alt);
        let x = c.finish().xc.unwrap();
        let one = |g: usize| {
            (0..3)
                .map(|o| x.global.counts[crate::xcres::cidx(1, g, o)])
                .collect::<Vec<_>>()
        };
        assert_eq!(one(1), [0, 1, 1]);
        // Horizontal only at the primary gate: both updates see two aircraft.
        assert_eq!(&x.global.hcounts[3..6], [0, 0, 2]);
    }

    // REQ: OA-CEN-009
    #[test]
    fn updates_without_position_or_flag_are_counted_not_checked() {
        let mut c = xcensus(fly(1, -75.0, 40.0, 10_000));
        c.observe(src("AAA"), &rec(1, 10_000, Some(false)));
        let mut nof = upd(1, 14_600, None, -75.0, 40.0, 10_000, 0.0, 0.0, 0.0);
        nof.present &= !tais_bit(TaisField::TrackAdsb);
        c.observe(src("AAA"), &nof);
        let x = c.finish().xc.unwrap();
        assert_eq!(x.no_position, [1, 0]);
        assert_eq!(x.no_flag, 1);
        assert_eq!(x.global.counts.iter().sum::<u64>(), 0);
    }

    // REQ: OA-CEN-010
    #[test]
    fn an_injected_bias_shows_in_the_matched_distribution_and_the_control_does_not() {
        // adsb=false updates on aircraft 1 sit 300 m ahead and 100 m left of it; the control
        // (adsb=true) updates on aircraft 2 sit exactly on it. 40 updates each, at times with
        // a fraction of a second so the interpolation is used.
        let mut pts = fly(1, -75.0, 40.0, 10_000);
        pts.extend(fly(2, -75.0, 40.2, 20_000));
        let mut c = xcensus(pts);
        for k in 0..40 {
            let t = 5_300 + k * 2_000;
            c.observe(
                src("AAA"),
                &upd(1, t, Some(false), -75.0, 40.0, 10_000, 300.0, 100.0, 0.0),
            );
            c.observe(
                src("AAA"),
                &upd(2, t, Some(true), -75.0, 40.2, 20_000, 0.0, 0.0, 0.0),
            );
        }
        let x = c.finish().xc.unwrap();
        let g = &x.global;
        let m = |f: usize, q: usize| g.sums[f * 5 + q].p50.unwrap();
        assert_eq!(g.sums[0].n, 40);
        assert_eq!(g.sums[5].n, 40);
        // Bias: along +300 m, cross is minus the 100 m north offset (east track, so left).
        assert!((m(0, 2) - 300.0).abs() < 1.0, "{}", m(0, 2));
        assert!((m(0, 3) + 100.0).abs() < 1.0, "{}", m(0, 3));
        assert!((m(0, 0) - 316.2).abs() < 1.5, "{}", m(0, 0));
        assert!(m(1, 0) < 1.0 && m(1, 2).abs() < 1.0 && m(1, 3).abs() < 1.0);
        // The histogram holds the 40 matches: the bias bin is far from the control's.
        assert_eq!(g.sums[0].bins.iter().sum::<u64>(), 40);
        assert_eq!(g.sums[5].bins[0], 40);
        assert!(g.sums[0].bins[0] == 0);
        // Both flags' updates were exactly-one at every gate (aircraft 1 is 316 m away).
        assert_eq!(g.counts[crate::xcres::cidx(0, 0, 1)], 40);
    }

    // REQ: OA-CEN-011
    #[test]
    fn a_known_time_offset_is_recovered_as_the_along_track_time() {
        // The position belongs to 1.5 s after the stated time: along-track ahead by 1.5 s.
        let mut c = xcensus(fly(1, -75.0, 40.0, 10_000));
        for k in 0..30 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    5_300 + k * 3_000,
                    Some(false),
                    -75.0,
                    40.0,
                    10_000,
                    0.0,
                    0.0,
                    1.5,
                ),
            );
        }
        let x = c.finish().xc.unwrap();
        let along = x.global.sums[2].p50.unwrap();
        let secs = x.global.sums[4].p50.unwrap();
        assert!((secs - 1.5).abs() < 0.01, "{secs}");
        assert!((along - 1.5 * V).abs() < 1.0, "{along}");
        assert_eq!(x.global.sums[4].n, 30);
        // A late label (position of 2 s before) gives a negative value.
        let mut c = xcensus(fly(1, -75.0, 40.0, 10_000));
        c.observe(
            src("AAA"),
            &upd(1, 20_300, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, -2.0),
        );
        let x = c.finish().xc.unwrap();
        assert!((x.global.sums[4].p50.unwrap() + 2.0).abs() < 0.01);
    }

    // REQ: OA-CEN-012
    #[test]
    fn matches_are_split_by_track_class_and_facility() {
        let mut pts = fly(1, -75.0, 40.0, 10_000);
        pts.extend(fly(2, -75.0, 40.3, 10_000));
        pts.extend(fly(3, -75.0, 40.6, 10_000));
        let mut c = xcensus(pts);
        // Facility AAA: track 1 all false (aircraft 1), track 2 mixed (aircraft 2).
        for k in 0..4 {
            let t = 5_300 + k * 4_000;
            c.observe(
                src("AAA"),
                &upd(1, t, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0),
            );
            c.observe(
                src("AAA"),
                &upd(2, t, Some(k % 2 == 0), -75.0, 40.3, 10_000, 0.0, 0.0, 0.0),
            );
        }
        // Facility BBB: track 1 (the same number) has one update only.
        c.observe(
            src("BBB"),
            &upd(1, 9_300, Some(false), -75.0, 40.6, 10_000, 0.0, 0.0, 0.0),
        );
        let r = c.finish();
        let x = r.xc.unwrap();
        let ones = |g: &crate::xcres::XcGroup, f: usize| g.counts[crate::xcres::cidx(f, 1, 1)];
        assert_eq!(ones(&x.global, 0), 4 + 2 + 1);
        assert_eq!(ones(&x.global, 1), 2);
        // Classes: [single, all false, all true, mixed].
        assert_eq!(ones(&x.classes[0], 0), 1);
        assert_eq!(ones(&x.classes[1], 0), 4);
        assert_eq!(ones(&x.classes[2], 0), 0);
        assert_eq!(ones(&x.classes[3], 0), 2);
        assert_eq!(ones(&x.classes[3], 1), 2);
        // Facilities, in the census order.
        let a = r.facs.iter().position(|(k, _)| *k == src("AAA")).unwrap();
        let b = r.facs.iter().position(|(k, _)| *k == src("BBB")).unwrap();
        assert_eq!(ones(&x.facs[a], 0), 6);
        assert_eq!(ones(&x.facs[b], 0), 1);
        assert_eq!(x.facs[a].sums[0].n, 6);
    }

    // REQ: OA-CEN-012
    #[test]
    fn without_a_clip_there_is_no_cross_check() {
        let mut c = Census::new();
        c.observe(
            src("AAA"),
            &upd(1, 5_300, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0),
        );
        assert!(c.finish().xc.is_none());
    }
}

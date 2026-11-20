//! The census accumulator: per-update counts live, per-track interval lists kept compactly and
//! reduced to distributions at the end. Pure over the parsed [`TaisRecord`]s it is given; it
//! holds only counts and distributions in its output, never a position, address, code,
//! callsign or timestamp of an aircraft (the per-track interval lists are in memory only, as
//! durations, and are dropped by [`Census::finish`]).

use crate::hist::{Hist, MAX_TICKS, TICK_MS};
use crate::xcheck::Params;
use crate::xcheck::{outcome, Cand, Index, Window, COVERAGE_M, MIN_GS_FOR_TIME_MPS, NM_M};
use crate::xcres::{
    addr_range, cidx, q_range, MatchRec, NotChecked, TrackXc, XcAcc, XcGroup, XcResults, PLACEBO_S,
};
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
    "one flagged update",
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
    pub is_new: u64,
}

/// A 128-bit fingerprint of what makes two updates the same: facility, source id, track number,
/// `mrtTime`, position and flags. Two hashes of the unspecified std hasher with different
/// prefixes; a collision would drop a distinct update, and the collision odds are heuristic (the
/// hasher is not specified to be uniform).
fn fingerprint(src: [u8; 8], r: &TaisRecord, mrt: i64) -> u128 {
    use std::hash::{Hash, Hasher};
    let flags = [
        has(r, TaisField::TrackAdsb).then_some(r.adsb),
        has(r, TaisField::TrackPseudo).then_some(r.pseudo),
        has(r, TaisField::TrackFrozen).then_some(r.frozen),
        has(r, TaisField::TrackCoast).then_some(r.coast),
        has(r, TaisField::TrackNew).then_some(r.is_new),
    ];
    let pos = (has(r, TaisField::Lat) && has(r, TaisField::Lon)).then_some((r.lat_e7, r.lon_e7));
    let h = |salt: u8| {
        let mut s = std::collections::hash_map::DefaultHasher::new();
        (salt, src, r.rec_stars_src_id, r.track_num, mrt, pos, flags).hash(&mut s);
        u128::from(s.finish())
    };
    (h(1) << 64) | h(2)
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
        self.is_new += u64::from(has(r, TaisField::TrackNew) && r.is_new);
    }
}

/// The counts and distributions of one set of updates (all, or one facility).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct Group {
    /// Updates with the flag false, true, absent.
    pub updates: [u64; 3],
    /// Identity counts of the updates with a flag, leaving out pseudo, frozen and coasting
    /// ones (so `ids[f].n` counts what the class lines count).
    pub ids: [IdShare; 2],
    /// Interval distributions by the flag of the update that ends the interval; intervals that
    /// span a flag change are kept apart (see [`Results::dirs`]).
    pub iv: [Hist; 2],
    /// Updates with no `mrtTime`, with a time before the track's previous one, and with a gap
    /// over 120 s from it (a reset).
    pub no_mrt: u64,
    pub backwards: u64,
    pub resets: u64,
    /// Restarts of a track after [`RESYNC_AFTER`] backwards updates in a row, and restarts at an
    /// update flagged new (a new track on a track number that had state).
    pub resyncs: u64,
    pub new_resets: u64,
    /// Track segments seen (a track cut at every restart of its interval state), segments with
    /// at least one adsb=false update, and of those the segments whose adsb=false updates ever
    /// carried a non-zero Mode S address.
    pub segments: u64,
    pub segments_with_false: u64,
    pub segments_false_with_icao: u64,
    /// Updates flagged pseudo, frozen or coasting (each read only when present and true), by
    /// kind ([`STATUS_NAMES`]) and flag (false, true, absent). They are in `updates` and in
    /// `ids`, and in no flag tally, class, change count or interval.
    pub status: [[u64; 3]; 3],
    /// Updates with any of the three, by flag.
    pub status_any: [u64; 3],
}

/// Kinds of updates that are not a measured position of the aircraft.
pub const STATUS_NAMES: [&str; 3] = ["pseudo", "frozen", "coasting"];

/// The pseudo, frozen and coasting kinds of an update, each only when present and true.
fn status_kinds(r: &TaisRecord) -> [bool; 3] {
    [
        has(r, TaisField::TrackPseudo) && r.pseudo,
        has(r, TaisField::TrackFrozen) && r.frozen,
        has(r, TaisField::TrackCoast) && r.coast,
    ]
}

impl Group {
    fn observe(&mut self, r: &TaisRecord, flag: Option<bool>) {
        let i = flag_index(flag);
        if let Some(c) = self.updates.get_mut(i) {
            *c += 1;
        }
        let kinds = status_kinds(r);
        for (k, _) in kinds.iter().enumerate().filter(|(_, &on)| on) {
            if let Some(c) = self.status.get_mut(k).and_then(|row| row.get_mut(i)) {
                *c += 1;
            }
        }
        if let Some(c) = self.status_any.get_mut(i) {
            *c += u64::from(kinds.contains(&true));
        }
        // The identity shares, like the tallies, leave out pseudo, frozen and coasting updates.
        if let Some(f) = flag.filter(|_| !kinds.contains(&true)) {
            if let Some(s) = self.ids.get_mut(usize::from(f)) {
                s.add(r);
            }
        }
    }
}

/// Applies `f` to the global group and to the facility's.
fn bump(global: &mut Group, facs: &mut [Group], fac: u32, f: fn(&mut Group)) {
    f(global);
    if let Some(g) = facs.get_mut(fac as usize) {
        f(g);
    }
}

fn flag_index(flag: Option<bool>) -> usize {
    flag.map_or(2, usize::from)
}

/// A track's identity: facility index, `recSTARSSrcID`, track number.
type TrackKey = (u32, [u8; 8], u32);

/// Backwards updates in a row after which a track restarts from the latest update.
pub const RESYNC_AFTER: u8 = 3;

/// Bytes counted per kept interval entry, match record and similar small item (estimates).
/// Every figure carries the 2x capacity slack of a `Vec` filled by pushes.
const SLACK: u64 = 2;
const IV_BYTES: u64 = SLACK * 2;
const REC_BYTES: u64 = SLACK * std::mem::size_of::<MatchRec>() as u64;
const SMALL_BYTES: u64 = SLACK * 8;
/// What [`Census::finish`] allocates on top of what is held while reading, per item: a match
/// record's five values are copied into three accumulators (all, its class, its facility); a
/// placebo or vertical-only value once into each of the three; a track segment's per-track
/// medians (ten at most) once into each of the three.
/// (An address match is counted the same, as three copies.)
const FINISH_REC_BYTES: u64 = SLACK * 3 * 5 * 4;
const FINISH_SMALL_BYTES: u64 = SLACK * 3 * 4;
const FINISH_SEG_BYTES: u64 = SLACK * 3 * 10 * 4;
/// A segment held after its track moved on, and a track in the map (key, state and the map's own
/// slack).
const SEG_BYTES: u64 = SLACK * std::mem::size_of::<Seg>() as u64;
const TRACK_BYTES: u64 =
    SLACK * (std::mem::size_of::<TrackKey>() + std::mem::size_of::<Track>() + 8) as u64 + SEG_BYTES;
/// A fingerprint in the set of seen updates (16 bytes, with the set's slack).
const DEDUP_BYTES: u64 = SLACK * 16;
/// The cross-check record of a segment, and what `finish` copies of its medians.
const XC_SEG_BYTES: u64 = SLACK * std::mem::size_of::<TrackXc>() as u64 + FINISH_SEG_BYTES;

/// One segment of a track: the updates since the last restart of the interval state. Class,
/// flag counts, changes and intervals are taken per segment, because STARS reuses track numbers
/// and one key holds many aircraft.
#[derive(Default)]
struct Seg {
    /// Updates of the segment (all, for the interval list's alignment).
    updates: u32,
    changes: u32,
    /// Updates by flag; pseudo, frozen and coasting updates are not in it.
    n: [u32; 2],
    false_icao: bool,
    false_nonzero: u32,
    ivs: Vec<u16>,
    /// The cross-check of the segment, made when its first update is checked.
    xc: Option<Box<TrackXc>>,
}

struct Track {
    fac: u32,
    last_ms: Option<i64>,
    last_flag: Option<bool>,
    /// Time, latitude and longitude of the previous update with a position (duplicate check).
    last_key: Option<(i64, i32, i32)>,
    /// Backwards updates in a row.
    backs: u8,
    /// The previous update that moved the time was a pseudo, frozen or coasting one: the next
    /// interval is not counted.
    after_status: bool,
    cur: Seg,
    done: Vec<Seg>,
}

/// `v` as f32, kept inside `[lo, hi]` (rounding to f32 must not push a value that passed the
/// gate in f64 outside the histogram range); NaN if there is none.
fn f32_in(v: Option<f64>, lo: f64, hi: f64) -> f32 {
    let Some(x) = v else {
        return f32::NAN;
    };
    let mut f = x as f32;
    if f64::from(f) > hi {
        f = f.next_down();
    }
    if f64::from(f) < lo {
        f = f.next_up();
    }
    f
}

/// The record of a candidate. Along and cross metres, like the time, need a ground speed of
/// 100 kt or more (the track angle of a slow aircraft is noise).
fn match_rec(c: &Cand, flag: bool, range: impl Fn(usize) -> (f64, f64)) -> MatchRec {
    let fast = c.gs_mps.is_some_and(|g| g >= MIN_GS_FOR_TIME_MPS);
    let along_s = c
        .along_m
        .zip(c.gs_mps)
        .filter(|&(_, gs)| gs >= MIN_GS_FOR_TIME_MPS)
        .map(|(a, gs)| a / gs);
    let f = |q: usize, v: Option<f64>| {
        let (lo, hi) = range(q);
        f32_in(v, lo, hi)
    };
    MatchRec {
        flag,
        dh: f(0, Some(c.dh_m)),
        dv: f(1, c.dv_ft),
        along: f(2, c.along_m.filter(|_| fast)),
        cross: f(3, c.cross_m.filter(|_| fast)),
        along_s: f(4, along_s),
        gap_ms: u32::try_from(c.gap_ms).unwrap_or(u32::MAX),
    }
}

/// Counts the outcomes of one update's candidates at every gate, keeps the match, the
/// coverage and the two placebo passes (the same positions with the time shifted by plus and
/// minus [`PLACEBO_S`]). Returns the bytes added and the matched aircraft, if exactly one.
fn xc_observe(x: &Index, t: &mut TrackXc, flag: bool, r: &TaisRecord) -> (u64, Option<u32>) {
    let p: Params = x.params;
    let alt = has(r, TaisField::ReportedAltitude).then_some(r.reported_alt_ft);
    let cands = x.candidates(r.mrt_time_ms, r.lat_e7, r.lon_e7, alt);
    let f = usize::from(flag);
    let mut bytes = 0;
    for (g, &nm) in p.gates_nm.iter().enumerate() {
        let n = cands
            .iter()
            .filter(|c| c.passes(nm * NM_M, p.v_gate_ft))
            .count();
        if let Some(c) = t.cnt.get_mut(cidx(f, g, outcome(n))) {
            *c += 1;
        }
    }
    let covered = !cands.is_empty() || x.covered(r.mrt_time_ms, r.lat_e7, r.lon_e7, COVERAGE_M);
    if let Some(c) = t.cov.get_mut(f) {
        *c += u32::from(covered);
    }
    let h_m = p.gates_nm.get(p.primary).map_or(0.0, |g| g * NM_M);
    let h_only: Vec<&Cand> = cands.iter().filter(|c| c.dh_m <= h_m).collect();
    if let Some(c) = t.hcnt.get_mut(f * 3 + outcome(h_only.len())) {
        *c += 1;
    }
    if let [c] = h_only.as_slice() {
        if let Some(v) = c.dv_ft {
            t.hdv.push((flag, v as f32));
            bytes += SMALL_BYTES + FINISH_SMALL_BYTES;
        }
    }
    let gated: Vec<&Cand> = cands
        .iter()
        .filter(|c| c.passes(h_m, p.v_gate_ft))
        .collect();
    let mut matched = None;
    if let [c] = gated.as_slice() {
        t.recs.push(match_rec(c, flag, |q| q_range(q, &p)));
        bytes += REC_BYTES + FINISH_REC_BYTES;
        matched = Some(c.icao);
    }
    for (k, shift) in [PLACEBO_S, -PLACEBO_S].into_iter().enumerate() {
        let code = k * 2 + f;
        // A shifted time outside the clip's span has no ADS-B to find: counted apart, not as a
        // non-match.
        if x.outside_clip(r.mrt_time_ms + shift * 1000) {
            if let Some(c) = t.pout.get_mut(code) {
                *c += 1;
            }
            continue;
        }
        let pc = x.candidates(r.mrt_time_ms + shift * 1000, r.lat_e7, r.lon_e7, alt);
        let ok: Vec<&Cand> = pc.iter().filter(|c| c.passes(h_m, p.v_gate_ft)).collect();
        if let Some(c) = t.pcnt.get_mut(code * 3 + outcome(ok.len())) {
            *c += 1;
        }
        if let [c] = ok.as_slice() {
            t.pdh.push((code as u8, f32_in(Some(c.dh_m), 0.0, h_m)));
            bytes += SMALL_BYTES + FINISH_SMALL_BYTES;
        }
    }
    // An adsb=true update with an address: the aircraft of that address, whatever its distance.
    if flag && has(r, TaisField::AcAddress) && r.mode_s_addr != 0 {
        t.addr_n += 1;
        if let Some(c) = x.for_icao(r.mrt_time_ms, r.lat_e7, r.lon_e7, alt, r.mode_s_addr) {
            t.addr.push(match_rec(&c, flag, |q| addr_range(q, &p)));
            bytes += REC_BYTES + FINISH_REC_BYTES;
        }
    }
    (bytes, matched)
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
    xc_nc: NotChecked,
    /// Distinct ADS-B aircraft that were an exactly-one match, by flag.
    xc_ac: [HashSet<u32>; 2],
    window: Option<Window>,
    outside_window: u64,
    /// With a window, updates with no `mrtTime` (outside every window).
    no_time_outside: u64,
    /// Fingerprints of the updates seen, and the exact repeats dropped.
    seen: HashSet<u128>,
    duplicates: u64,
    /// Earliest and latest `mrtTime` of the updates counted, UTC ms.
    span_ms: Option<(i64, i64)>,
    /// Estimate of the bytes held, and the budget past which the run is stopped.
    held_bytes: u64,
    budget: u64,
}

/// Default memory budget, bytes.
pub const DEFAULT_BUDGET: u64 = 4_000_000_000;

impl Default for Census {
    fn default() -> Census {
        Census::new()
    }
}

/// Per-class distributions.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct ClassStats {
    /// Track segments of the class.
    pub segments: u64,
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
    pub segments_with_change: u64,
    /// Segments with no flagged update (only status updates, or only updates with no flag); in
    /// no class.
    pub segments_no_flag: u64,
    /// Over tracks with two or more flagged updates, the tracks by number of changes.
    pub change_bins: [u64; 6],
    /// Consecutive flagged update pairs on tracks (the chances for a change).
    pub flagged_pairs: u64,
    pub distinct_src_ids: u64,
    pub no_track_num: u64,
    /// Updates left out by the time window, and (with a window) updates with no `mrtTime`.
    pub outside_window: u64,
    pub no_time_outside: u64,
    /// Updates with the same key, time, position and flags as one seen, dropped. Updates without
    /// a time or a track number are not checked.
    pub duplicates: u64,
    /// Distinct track keys (facility, recSTARSSrcID, trackNum).
    pub track_keys: u64,
    pub window: Option<Window>,
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
            xc_nc: NotChecked::default(),
            xc_ac: [HashSet::new(), HashSet::new()],
            window: None,
            outside_window: 0,
            no_time_outside: 0,
            seen: HashSet::new(),
            duplicates: 0,
            span_ms: None,
            held_bytes: 0,
            budget: DEFAULT_BUDGET,
        }
    }

    /// Restricts the census to updates whose `mrtTime` is in the window (time of day).
    pub fn with_window(mut self, w: Option<Window>) -> Census {
        self.window = w;
        self
    }

    /// Sets the memory budget, bytes.
    pub fn with_budget(mut self, bytes: u64) -> Census {
        self.budget = bytes;
        self
    }

    /// The estimate of the bytes held so far, with what [`finish`](Self::finish) will allocate
    /// for them (the index is not included).
    pub fn held_bytes(&self) -> u64 {
        self.held_bytes
    }

    /// Whether the estimate of the bytes held is past the budget.
    pub fn over_budget(&self) -> bool {
        self.held_bytes + self.xc.as_ref().map_or(0, Index::bytes) > self.budget
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
    /// message carries none; the record's own `recSTARSSrcID` then names the facility).
    pub fn observe(&mut self, src: [u8; 8], r: &TaisRecord) {
        let src = if src == [0; 8] {
            r.rec_stars_src_id
        } else {
            src
        };
        let flag = has(r, TaisField::TrackAdsb).then_some(r.adsb);
        let mrt = has(r, TaisField::MrtTime).then_some(r.mrt_time_ms);
        if let Some(w) = self.window {
            match mrt {
                // With a window, an update with no time is outside every window.
                None => {
                    self.no_time_outside += 1;
                    return;
                }
                Some(m) if !w.contains(m) => {
                    self.outside_window += 1;
                    return;
                }
                Some(_) => {}
            }
        }
        // An update with the same key, time, position and flags as one seen, across files or
        // within one, is dropped and counted. Updates without a time or track number are not
        // checked.
        if let (Some(m), true) = (mrt, has(r, TaisField::TrackNum)) {
            if !self.seen.insert(fingerprint(src, r, m)) {
                self.duplicates += 1;
                return;
            }
            self.held_bytes += DEDUP_BYTES;
        }
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
        if let Some(m) = mrt {
            self.span_ms = Some(self.span_ms.map_or((m, m), |(a, b)| (a.min(m), b.max(m))));
        }
        self.global.observe(r, flag);
        if let Some(g) = self.facs.get_mut(fac as usize) {
            g.observe(r, flag);
        }
        self.src_ids.insert(r.rec_stars_src_id);
        if !has(r, TaisField::TrackNum) {
            self.no_track_num += 1;
            if self.xc.is_some() {
                match flag {
                    Some(f) => {
                        if let Some(c) = self.xc_nc.no_track.get_mut(usize::from(f)) {
                            *c += 1;
                        }
                    }
                    None => self.xc_nc.no_flag += 1,
                }
            }
            return;
        }
        let key = (fac, r.rec_stars_src_id, r.track_num);
        let held = &mut self.held_bytes;
        let t = self.tracks.entry(key).or_insert_with(|| {
            *held += TRACK_BYTES;
            Track {
                fac,
                last_ms: None,
                last_flag: None,
                last_key: None,
                backs: 0,
                after_status: false,
                cur: Seg::default(),
                done: Vec::new(),
            }
        });
        let pos = has(r, TaisField::Lat) && has(r, TaisField::Lon);
        let key_now = mrt.filter(|_| pos).map(|m| (m, r.lat_e7, r.lon_e7));
        let is_new = has(r, TaisField::TrackNew) && r.is_new;
        // Pseudo, frozen and coasting updates take part in the time state (so the segments are
        // cut by track time alone) and in no flag tally, class, change or interval.
        let status = status_kinds(r).contains(&true);
        // The interval state restarts here (the first update after a break, an update flagged
        // new, a resync after backwards times, a gap over 120 s): a new segment starts, so a
        // reused track number is not one aircraft.
        let restart = match (mrt, t.last_ms) {
            (Some(_), None) => true,
            (Some(m), Some(l)) => {
                is_new
                    || (m < l && t.backs + 1 >= RESYNC_AFTER)
                    || (m >= l && m - l > MAX_INTERVAL_MS)
            }
            (None, _) => false,
        };
        if restart && t.cur.updates > 0 {
            t.done.push(std::mem::take(&mut t.cur));
            self.held_bytes += SEG_BYTES;
        }
        if let Some(x) = self.xc.as_ref() {
            match (flag, key_now) {
                (None, _) => self.xc_nc.no_flag += 1,
                (Some(f), None) => {
                    if let Some(c) = self.xc_nc.no_position.get_mut(usize::from(f)) {
                        *c += 1;
                    }
                }
                (Some(f), Some(k)) => {
                    let fi = usize::from(f);
                    if status {
                        if let Some(c) = self.xc_nc.status.get_mut(fi) {
                            *c += 1;
                        }
                    } else if !x.supports(r.lat_e7) {
                        if let Some(c) = self.xc_nc.polar.get_mut(fi) {
                            *c += 1;
                        }
                    } else if t.last_key == Some(k) {
                        if let Some(c) = self.xc_nc.duplicate.get_mut(fi) {
                            *c += 1;
                        }
                    } else if x.outside_clip(k.0) {
                        // Past the ADS-B clip's span: no ADS-B could be found, so it is neither
                        // "no candidate" nor "no ADS-B".
                        if let Some(c) = self.xc_nc.outside_clip.get_mut(fi) {
                            *c += 1;
                        }
                    } else {
                        let xc = t.cur.xc.get_or_insert_with(|| {
                            self.held_bytes += XC_SEG_BYTES;
                            Box::default()
                        });
                        let (bytes, matched) = xc_observe(x, xc, f, r);
                        self.held_bytes += bytes;
                        if let (Some(a), Some(set)) = (matched, self.xc_ac.get_mut(fi)) {
                            set.insert(a);
                        }
                    }
                }
            }
        }
        if key_now.is_some() {
            t.last_key = key_now;
        }
        let tally = flag.filter(|_| !status);
        let mut entry = GAP;
        // Whether the update moved the track's time state.
        let mut moved = true;
        match mrt {
            None => {
                bump(&mut self.global, &mut self.facs, fac, |g| g.no_mrt += 1);
                t.last_ms = None;
                t.last_flag = None;
                t.backs = 0;
                moved = false;
            }
            Some(m) => match t.last_ms {
                Some(_) if is_new => {
                    bump(&mut self.global, &mut self.facs, fac, |g| g.new_resets += 1);
                    t.last_ms = Some(m);
                    t.last_flag = tally;
                    t.backs = 0;
                }
                None => {
                    t.last_ms = Some(m);
                    t.last_flag = tally;
                    t.backs = 0;
                }
                Some(l) if m < l => {
                    bump(&mut self.global, &mut self.facs, fac, |g| g.backwards += 1);
                    t.backs += 1;
                    if t.backs >= RESYNC_AFTER {
                        bump(&mut self.global, &mut self.facs, fac, |g| g.resyncs += 1);
                        t.last_ms = Some(m);
                        t.last_flag = tally;
                        t.backs = 0;
                    } else {
                        moved = false;
                    }
                }
                Some(l) if m - l > MAX_INTERVAL_MS => {
                    bump(&mut self.global, &mut self.facs, fac, |g| g.resets += 1);
                    t.last_ms = Some(m);
                    t.last_flag = tally;
                    t.backs = 0;
                }
                Some(l) => {
                    t.backs = 0;
                    if let Some(f) = tally {
                        if t.after_status {
                            // The interval starts at a coasting update: not counted.
                            t.last_flag = Some(f);
                        } else {
                            let ticks = u16::try_from((m - l) / TICK_MS).unwrap_or(TICK_MASK);
                            // A change is counted only where an across interval exists.
                            let across = t.last_flag.is_some_and(|p| p != f);
                            t.cur.changes += u32::from(across);
                            entry = ticks.min(TICK_MASK)
                                | if f { FLAG_BIT } else { 0 }
                                | if across { ACROSS_BIT } else { 0 };
                            t.last_flag = Some(f);
                        }
                    }
                    t.last_ms = Some(m);
                }
            },
        }
        t.after_status = if moved { status } else { t.after_status };
        let seg = &mut t.cur;
        if seg.updates > 0 {
            seg.ivs.push(entry);
            self.held_bytes += IV_BYTES;
        }
        seg.updates = seg.updates.saturating_add(1);
        if let Some(f) = tally {
            if let Some(c) = seg.n.get_mut(usize::from(f)) {
                *c += 1;
            }
            let real = !f && has(r, TaisField::AcAddress) && r.mode_s_addr != 0;
            seg.false_icao |= real;
            seg.false_nonzero += u32::from(real);
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
            xc_nc,
            xc_ac,
            window,
            outside_window,
            no_time_outside,
            duplicates,
            span_ms,
            held_bytes,
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
        let mut segments_with_change = 0u64;
        let mut segments_no_flag = 0u64;
        let mut change_bins = [0u64; 6];
        let mut flagged_pairs = 0u64;
        let track_keys = tracks.len() as u64;
        for t in tracks.values() {
            for seg in t.done.iter().chain(std::iter::once(&t.cur)) {
                if seg.updates == 0 {
                    continue;
                }
                let [nf, nt] = seg.n;
                let flagged = u64::from(nf) + u64::from(nt);
                let class = match (nf, nt) {
                    _ if flagged < 2 => 0,
                    (_, 0) => 1,
                    (0, _) => 2,
                    _ => 3,
                };
                segments_no_flag += u64::from(flagged == 0);
                if let Some(c) = classes.get_mut(class).filter(|_| flagged > 0) {
                    c.segments += 1;
                    c.updates[0] += u64::from(nf);
                    c.updates[1] += u64::from(nt);
                    c.false_nonzero += u64::from(seg.false_nonzero);
                }
                if flagged >= 2 {
                    flagged_pairs += flagged - 1;
                    changes += u64::from(seg.changes);
                    segments_with_change += u64::from(seg.changes > 0);
                    let bin = match seg.changes {
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
                    if let Some(x) = &seg.xc {
                        for k in [0, 1 + class, 5 + fac] {
                            if let Some(a) = accs.get_mut(k) {
                                a.add_track(x);
                            }
                        }
                    }
                }
                for g in std::iter::once(&mut global).chain(facs.get_mut(fac)) {
                    g.segments += 1;
                    g.segments_with_false += u64::from(nf > 0);
                    g.segments_false_with_icao += u64::from(seg.false_icao);
                }
                for (i, &e) in seg.ivs.iter().enumerate() {
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
                            .and_then(|p| seg.ivs.get(p))
                            .filter(|&&p| p != GAP && p & ACROSS_BIT == 0);
                        let next = seg
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
        }
        let xc_res = xc.zip(xc_params).and_then(|(x, p)| {
            let mut it = accs.into_iter().map(|a| a.finish(&p));
            let global = it.next()?;
            let classes: Vec<XcGroup> = it.by_ref().take(4).collect();
            Some(XcResults {
                params: p,
                window,
                stdds_span_ms: span_ms,
                clip: x.clip.clone(),
                points: x.points() as u64,
                aircraft: x.aircraft,
                segments: x.segments,
                segments_fast: x.segments_fast,
                segments_polar: x.segments_polar,
                segments_long: x.segments_long,
                not_checked: xc_nc,
                matched_aircraft: {
                    let [a, b] = &xc_ac;
                    [a.len() as u64, b.len() as u64]
                },
                index_bytes: x.bytes(),
                held_bytes,
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
            segments_with_change,
            segments_no_flag,
            change_bins,
            flagged_pairs,
            distinct_src_ids: src_ids.len() as u64,
            no_track_num,
            outside_window,
            no_time_outside,
            duplicates,
            track_keys,
            window,
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
            coast: false,
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
        assert_eq!(r.global.segments, 2);
        assert_eq!(r.global.segments_with_false, 1);
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
        assert_eq!(r.segments_with_change, 1);
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
        assert_eq!(r.classes[0].segments, 1);
        assert_eq!(r.classes[1].segments, 1);
        assert_eq!(r.classes[2].segments, 1);
        assert_eq!(r.classes[3].segments, 1);
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
        // The pseudo update is left out of the identity shares.
        assert_eq!(f.n, 4);
        assert_eq!(f.icao, 1);
        assert_eq!(f.icao_field, 2);
        assert_eq!(f.icao_zero, 1);
        assert_eq!(f.beacon, 2);
        assert_eq!(f.beacon_1200, 1);
        assert_eq!(f.acid, 1);
        assert_eq!(f.plan, 1);
        assert_eq!(f.icao_and_beacon, 1);
        assert_eq!(f.none_of_icao_beacon_acid, 2);
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
        assert_eq!(g.segments, 3);
        assert_eq!(g.segments_with_false, 3);
        assert_eq!(g.segments_false_with_icao, 1);
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
        assert_eq!(r.global.segments, 3);
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
        assert_eq!(r.global.segments, 0);
    }

    // --- position cross-check ---------------------------------------------------------------

    use crate::xcheck::tests::{deg_lat_per_m, deg_lon_per_m, pt, pt_full};
    use crate::xcheck::{build_from_points, Params, Pt, Window, NM_M};

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
        assert_eq!(x.not_checked.no_position, [1, 0]);
        assert_eq!(x.not_checked.no_flag, 1);
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

    // REQ: OA-CEN-016
    #[test]
    fn outcome_totals_reconcile_with_the_update_counts_per_flag() {
        let lat = 40.0;
        let mut c = xcensus(fly(1, -75.0, lat, 10_000));
        // Checked: two updates (one false, one true).
        c.observe(
            src("AAA"),
            &upd(1, 10_300, Some(false), -75.0, lat, 10_000, 0.0, 0.0, 0.0),
        );
        c.observe(
            src("AAA"),
            &upd(2, 10_300, Some(true), -75.0, lat, 10_000, 0.0, 0.0, 0.0),
        );
        // The previous update of track 1 repeated in time and position, with a coast flag that
        // is present and false (not an exact repeat, so not dropped): a duplicate of the
        // position check.
        let mut again = upd(1, 10_300, Some(false), -75.0, lat, 10_000, 0.0, 0.0, 0.0);
        again.present |= tais_bit(TaisField::TrackCoast);
        c.observe(src("AAA"), &again);
        // No trackNum (false and true), no position (true), polar (false), no flag.
        let mut nt = upd(3, 11_300, Some(false), -75.0, lat, 10_000, 0.0, 0.0, 0.0);
        nt.present &= !tais_bit(TaisField::TrackNum);
        c.observe(src("AAA"), &nt);
        let mut nt2 = upd(3, 11_300, Some(true), -75.0, lat, 10_000, 0.0, 0.0, 0.0);
        nt2.present &= !tais_bit(TaisField::TrackNum);
        c.observe(src("AAA"), &nt2);
        c.observe(src("AAA"), &rec(4, 12_000, Some(true)));
        let mut polar = upd(5, 12_000, Some(false), -75.0, lat, 10_000, 0.0, 0.0, 0.0);
        polar.lat_e7 = 880_000_000;
        c.observe(src("AAA"), &polar);
        let mut nof = upd(6, 12_000, None, -75.0, lat, 10_000, 0.0, 0.0, 0.0);
        nof.present &= !tais_bit(TaisField::TrackAdsb);
        c.observe(src("AAA"), &nof);
        let r = c.finish();
        let x = r.xc.clone().unwrap();
        let nc = x.not_checked;
        assert_eq!(nc.duplicate, [1, 0]);
        assert_eq!(nc.no_track, [1, 1]);
        assert_eq!(nc.no_position, [0, 1]);
        assert_eq!(nc.polar, [1, 0]);
        assert_eq!(nc.no_flag, 1);
        for f in 0..2 {
            let checked: u64 = (0..3).map(|k| x.global.counts[cidx(f, 0, k)]).sum();
            let total = checked
                + nc.no_position[f]
                + nc.polar[f]
                + nc.no_track[f]
                + nc.duplicate[f]
                + nc.outside_clip[f];
            assert_eq!(total, r.global.updates[f], "flag {f}");
        }
        assert_eq!(r.global.updates[2], nc.no_flag);
        // And the report says so (eight records were observed).
        let text = crate::report::render(&with_records(8), &r);
        assert!(text.contains("(reconciles)"));
        assert!(!text.contains("DOES NOT RECONCILE"));
    }

    // REQ: OA-CEN-017
    #[test]
    fn placebo_passes_do_not_match_and_coverage_is_counted() {
        let lat = 40.0;
        let mut c = xcensus(fly(1, -75.0, lat, 10_000));
        for k in 0..5 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    20_300 + k * 2_000,
                    Some(false),
                    -75.0,
                    lat,
                    10_000,
                    0.0,
                    0.0,
                    0.0,
                ),
            );
        }
        // An update far from any aircraft (100 NM north): no candidate and not covered; one
        // 20 NM north: no candidate but covered.
        let far = |nm: f64| {
            upd(
                2,
                30_300,
                Some(true),
                -75.0,
                lat + nm * NM_M * deg_lat_per_m(lat),
                10_000,
                0.0,
                0.0,
                0.0,
            )
        };
        let mut f100 = far(100.0);
        f100.track_num = 3;
        c.observe(src("AAA"), &f100);
        c.observe(src("AAA"), &far(20.0));
        let x = c.finish().xc.unwrap();
        let g = &x.global;
        assert_eq!(g.counts[cidx(0, 1, 1)], 5);
        // Shifted by +60 s the aircraft is 12 km away: never within the 1 NM gate. Shifted by
        // -60 s the time (about 20 s) is before the clip's first report: counted as outside the
        // clip, not as a non-match.
        assert_eq!(g.pcounts[1], 0);
        assert_eq!(g.pcounts[0], 5);
        assert_eq!(g.psums[0].n, 0);
        assert_eq!(g.pcounts[2 * 3..2 * 3 + 3], [0, 0, 0]);
        assert_eq!(g.pout[2], 5);
        assert_eq!(g.pout[0], 0);
        assert_eq!(g.covered, [5, 1]);
        assert_eq!(g.counts[cidx(1, 1, 0)], 2);
    }

    // REQ: OA-CEN-017
    #[test]
    fn a_stationary_target_matches_at_the_placebo_times_too() {
        // The control shows what chance looks like: a target that does not move matches at any
        // time shift, so the placebo exactly-one rate is as high as the real one.
        let mut pts = Vec::new();
        for s in 0..=240 {
            pts.push(pt_full(1, s * 1000, 40.0, -75.0, 9000, 900, 0));
        }
        let mut c = xcensus(pts);
        for k in 0..3 {
            c.observe(
                src("AAA"),
                &upd_at(1, 100_300 + k * 2_000, Some(false), 40.0, -75.0, 9000),
            );
        }
        let g = c.finish().xc.unwrap().global;
        assert_eq!(g.counts[cidx(0, 1, 1)], 3);
        assert_eq!(g.pcounts[1], 3);
        assert_eq!(g.pcounts[2 * 3 + 1], 3);
    }

    /// An update at a given position (degrees), no motion model.
    fn upd_at(n: u32, t_ms: i64, flag: Option<bool>, lat: f64, lon: f64, alt: i32) -> TaisRecord {
        let mut r = rec(n, t_ms, flag);
        r.lat_e7 = (lat * 1e7).round() as i32;
        r.lon_e7 = (lon * 1e7).round() as i32;
        r.reported_alt_ft = alt;
        r.present |= tais_bit(TaisField::Lat)
            | tais_bit(TaisField::Lon)
            | tais_bit(TaisField::ReportedAltitude);
        r
    }

    // REQ: OA-CEN-018
    #[test]
    fn distinct_aircraft_tracks_per_track_medians_and_no_vertical_counts() {
        let lat = 40.0;
        let mut pts = fly(1, -75.0, lat, 10_000);
        pts.extend(fly(2, -75.0, lat + 0.2, 10_000));
        // Aircraft 3 has no barometric altitude.
        pts.extend(fly(3, -75.0, lat + 0.4, i32::MIN));
        let mut c = xcensus(pts);
        // Track 1 (false): 4 matches on aircraft 1, along offsets 100, 200, 300, 400 m.
        for k in 0..4 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    5_300 + k * 3_000,
                    Some(false),
                    -75.0,
                    lat,
                    10_000,
                    100.0 * (k + 1) as f64,
                    0.0,
                    0.0,
                ),
            );
        }
        // Track 2 (false): 2 matches on aircraft 2: too few for a track median.
        for k in 0..2 {
            c.observe(
                src("AAA"),
                &upd(
                    2,
                    5_300 + k * 3_000,
                    Some(false),
                    -75.0,
                    lat + 0.2,
                    10_000,
                    50.0,
                    0.0,
                    0.0,
                ),
            );
        }
        // Track 3 (true): 3 matches on aircraft 3, which has no altitude: no vertical difference.
        for k in 0..3 {
            c.observe(
                src("AAA"),
                &upd(
                    3,
                    5_300 + k * 3_000,
                    Some(true),
                    -75.0,
                    lat + 0.4,
                    10_000,
                    0.0,
                    0.0,
                    0.0,
                ),
            );
        }
        let x = c.finish().xc.unwrap();
        let g = &x.global;
        assert_eq!(x.matched_aircraft, [2, 1]);
        assert_eq!(g.tracks_matched, [2, 1]);
        assert_eq!(g.sums[0].n, 6);
        assert_eq!(g.sums[1].n, 6);
        assert_eq!(g.sums[5].n, 3);
        assert_eq!(
            g.sums[6].n, 0,
            "no vertical difference without an ADS-B altitude"
        );
        // Per-track medians: only track 1 (4 matches) for false; its median along is 200 m.
        assert_eq!(g.tsums[2].n, 1);
        assert!((g.tsums[2].p50.unwrap() - 200.0).abs() < 1.5);
        assert_eq!(g.tsums[7].n, 1);
        assert!(x.held_bytes > 0 && x.index_bytes > 0);
    }

    // REQ: OA-CEN-018
    #[test]
    fn the_100_kt_cut_leaves_slow_aircraft_out_of_the_time_figure() {
        let pts: Vec<Pt> = (0..=20)
            .map(|s| pt_full(1, s * 1000, 40.0, -75.0, 9000, 900, 300))
            .collect();
        let mut c = xcensus(pts);
        c.observe(
            src("AAA"),
            &upd_at(
                1,
                10_300,
                Some(false),
                40.0,
                -75.0 + 100.0 * deg_lon_per_m(40.0),
                9000,
            ),
        );
        let x = c.finish().xc.unwrap();
        assert_eq!(x.global.sums[0].n, 1);
        // A slow aircraft's track angle is noise: no along or cross metres, no time.
        assert_eq!(x.global.sums[2].n, 0);
        assert_eq!(x.global.sums[3].n, 0);
        assert_eq!(x.global.sums[4].n, 0);
    }

    // REQ: OA-CEN-018
    #[test]
    fn small_cells_print_n_below_30_instead_of_medians() {
        let mut c = xcensus(fly(1, -75.0, 40.0, 10_000));
        for k in 0..5 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    5_300 + k * 2_000,
                    Some(false),
                    -75.0,
                    40.0,
                    10_000,
                    0.0,
                    0.0,
                    0.0,
                ),
            );
        }
        let text = crate::report::render(&crate::read::ReadStats::new(), &c.finish());
        assert!(text.contains("n<30 (matches 5)"));
        assert!(!text.contains("matches 5, medians"));
    }

    // REQ: OA-CEN-019
    #[test]
    fn flag_changes_equal_the_changes_by_direction_and_resets_cut_the_chain() {
        let mut c = Census::new();
        // Track 1: false, true, then a backwards update with the other flag (no change counted),
        // then false at the right time: one false to true, one true to false.
        for (t, f) in [(0, false), (4600, true), (3000, false), (9200, false)] {
            c.observe(src("AAA"), &rec(1, t, Some(f)));
        }
        // Track 2: a gap over 120 s between a false and a true is a reset, not a change.
        c.observe(src("AAA"), &rec(2, 0, Some(false)));
        c.observe(src("AAA"), &rec(2, 200_000, Some(true)));
        // Track 3: an update flagged new starts a new segment, so its flag is not a change.
        c.observe(src("AAA"), &rec(3, 0, Some(false)));
        let mut n = rec(3, 4600, Some(true));
        n.present |= tais_bit(TaisField::TrackNew);
        n.is_new = true;
        c.observe(src("AAA"), &n);
        let r = c.finish();
        assert_eq!(r.global.backwards, 1);
        assert_eq!(r.global.resets, 1);
        assert_eq!(r.global.new_resets, 1);
        assert_eq!(r.changes, 2);
        assert_eq!(r.changes, r.dirs[0].changes + r.dirs[1].changes);
        assert_eq!(r.dirs[0].changes, 1);
        assert_eq!(r.dirs[1].changes, 1);
    }

    // REQ: OA-CEN-019
    #[test]
    fn a_backwards_absent_flag_update_does_not_rewind_the_track_and_three_resync() {
        let mut c = Census::new();
        c.observe(src("AAA"), &rec(1, 100_000, Some(false)));
        // An absent-flag update before the track's time is backwards, not an anchor.
        c.observe(src("AAA"), &rec(1, 50_000, None));
        c.observe(src("AAA"), &rec(1, 104_600, Some(false)));
        // Three backwards updates in a row restart the track at the latest one.
        for t in [90_000, 80_000, 70_000] {
            c.observe(src("AAA"), &rec(1, t, Some(false)));
        }
        c.observe(src("AAA"), &rec(1, 74_600, Some(false)));
        let r = c.finish();
        assert_eq!(r.global.backwards, 4);
        assert_eq!(r.global.resyncs, 1);
        // Intervals: 4.6 s (before the backwards run) and 4.6 s (after the restart).
        assert_eq!(r.global.iv[0].n, 2);
        assert!((r.global.iv[0].quantile(1, 2).unwrap() - 4.605).abs() < 1e-9);
    }

    // REQ: OA-CEN-020
    #[test]
    fn the_window_and_the_memory_budget_are_applied() {
        let w = Window::new(10, 20).unwrap();
        let mut c = Census::new().with_window(Some(w)).with_budget(1);
        for t in [5_000, 12_000, 25_000] {
            c.observe(src("AAA"), &rec(1, t, Some(false)));
        }
        assert!(c.over_budget());
        let r = c.finish();
        assert_eq!(r.outside_window, 2);
        assert_eq!(r.global.updates, [1, 0, 0]);
        let c = Census::new();
        assert!(!c.over_budget());
    }

    /// Aircraft 1 and 2 (22 km apart) every second for 300 s.
    fn two(lat: f64) -> Vec<Pt> {
        let mut v = Vec::new();
        for (icao, la) in [(1, lat), (2, lat + 0.2)] {
            for s in 0..=300 {
                v.push(pt(
                    icao,
                    s * 1000,
                    la,
                    -75.0 + s as f64 * V * deg_lon_per_m(40.0),
                    10_000,
                ));
            }
        }
        v
    }

    // REQ: OA-CEN-018
    #[test]
    fn a_recycled_track_number_is_two_segments_with_a_median_each() {
        let lat = 40.0;
        let mut c = xcensus(two(lat));
        // Track 7 follows aircraft 1 (100 m ahead), then, after a gap over 120 s, the number is
        // reused by aircraft 2 (400 m ahead).
        for (t0, la, east) in [(10_300, lat, 100.0), (200_300, lat + 0.2, 400.0)] {
            for k in 0..3 {
                c.observe(
                    src("AAA"),
                    &upd(
                        7,
                        t0 + k * 2_000,
                        Some(false),
                        -75.0,
                        la,
                        10_000,
                        east,
                        0.0,
                        0.0,
                    ),
                );
            }
        }
        let x = c.finish().xc.unwrap();
        assert_eq!(x.global.tracks_matched[0], 2);
        let s = &x.global.tsums[2];
        assert_eq!(s.n, 2, "one median per segment");
        assert!(s.p10.unwrap() < 150.0 && s.p90.unwrap() > 350.0, "{s:?}");
        assert_eq!(x.global.sums[2].n, 6);
    }

    // REQ: OA-CEN-009
    #[test]
    fn pseudo_frozen_and_coasting_updates_are_not_checked_and_reconcile() {
        let mut c = xcensus(two(40.0));
        let mut a = upd(1, 10_300, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0);
        a.present |= tais_bit(TaisField::TrackPseudo);
        a.pseudo = true;
        let mut b = upd(2, 10_300, Some(true), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0);
        b.present |= tais_bit(TaisField::TrackFrozen);
        b.frozen = true;
        let mut k = upd(4, 10_300, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0);
        k.present |= tais_bit(TaisField::TrackCoast);
        k.coast = true;
        // A coast flag that is present and false is a measured position.
        let mut nk = upd(5, 10_300, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0);
        nk.present |= tais_bit(TaisField::TrackCoast);
        let ok = upd(3, 10_300, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0);
        for r in [&a, &b, &k, &nk, &ok] {
            c.observe(src("AAA"), r);
        }
        let r = c.finish();
        let x = r.xc.unwrap();
        assert_eq!(x.not_checked.status, [2, 1]);
        let checked = |f: usize| (0..3).map(|k| x.global.counts[cidx(f, 0, k)]).sum::<u64>();
        assert_eq!((checked(0), checked(1)), (2, 0));
        assert_eq!(r.global.updates[0], 4);
    }

    // REQ: OA-CEN-012
    #[test]
    fn an_adsb_true_update_with_an_address_is_also_checked_by_address() {
        let mut c = xcensus(two(40.0));
        // 3 NM north of aircraft 1: outside the gates, so no proximity match, but its address
        // finds aircraft 1; address 99 is no aircraft in the clip; an adsb=false update with an
        // address is not an address check.
        let near = |n: u32, addr: u32, flag: bool| {
            let mut r = upd(
                n,
                10_300,
                Some(flag),
                -75.0,
                40.0,
                10_000,
                0.0,
                3.0 * NM_M,
                0.0,
            );
            r.present |= tais_bit(TaisField::AcAddress);
            r.mode_s_addr = addr;
            r
        };
        for r in [near(1, 1, true), near(2, 99, true), near(3, 1, false)] {
            c.observe(src("AAA"), &r);
        }
        let g = c.finish().xc.unwrap().global;
        assert_eq!((g.addr_n, g.addr_found), (2, 1));
        assert_eq!(g.counts[cidx(1, 1, 0)], 2);
        assert_eq!(g.asums[0].n, 1);
        assert!((g.asums[0].p50.unwrap() - 3.0 * NM_M).abs() < 30.0);
        assert_eq!(g.sums[5].n, 0, "no proximity match for it");
    }

    // REQ: OA-CEN-018
    #[test]
    fn the_bracket_width_of_matches_is_counted() {
        // Reports 4 s apart: a 2 to 5 s bracket.
        let pts: Vec<Pt> = (0..=60)
            .map(|s| {
                pt(
                    1,
                    s * 4000,
                    40.0,
                    -75.0 + s as f64 * 4.0 * V * deg_lon_per_m(40.0),
                    9000,
                )
            })
            .collect();
        let mut c = xcensus(pts);
        for k in 0..3 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    50_000 + k * 1_000,
                    Some(false),
                    -75.0,
                    40.0,
                    9000,
                    0.0,
                    0.0,
                    0.0,
                ),
            );
        }
        let g = c.finish().xc.unwrap().global;
        assert_eq!(g.gap_counts[0], [0, 3, 0, 0]);
    }

    // REQ: OA-CEN-017
    #[test]
    fn the_report_labels_the_fused_flag_and_gives_the_chance_share() {
        // A target that does not move matches at the placebo times as much as at the real one.
        let pts: Vec<Pt> = (0..=240)
            .map(|s| pt_full(1, s * 1000, 40.0, -75.0, 9000, 900, 0))
            .collect();
        let mut c = xcensus(pts);
        for k in 0..3 {
            c.observe(
                src("AAA"),
                &upd_at(1, 100_300 + k * 2_000, Some(false), 40.0, -75.0, 9000),
            );
        }
        let out = crate::report::render(&crate::read::ReadStats::new(), &c.finish());
        for want in [
            "adsb=true (ADS-B-fused, not independent of the truth)",
            "placebo-to-real ratio (rough indication, unknown direction, not a chance share) 100.0%",
            "headline, adsb=false updates by track class",
            "not ADS-B-free",
            "ADS-B bracket width of the matches",
            "track segments",
            "time spans (UTC, to the second",
            "a rough indication of unknown direction, not an estimate of the chance share",
            "not suppressed by the single-candidate rule",
            "placebo outside the clip",
            "an estimate under stated assumptions, not a bound",
            "duplicate updates dropped",
            "outside the ADS-B clip (time span)",
            "not of any aircraft",
        ] {
            assert!(out.contains(want), "{want}");
        }
    }

    // REQ: OA-CEN-020
    #[test]
    fn the_estimate_counts_what_finish_allocates() {
        let mut c = xcensus(two(40.0));
        let before = c.held_bytes();
        for k in 0..10 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    10_300 + k * 1_000,
                    Some(false),
                    -75.0,
                    40.0,
                    10_000,
                    100.0,
                    0.0,
                    0.0,
                ),
            );
        }
        // Counted from the numbers, not from the constants: per match, the record twice over
        // (Vec slack), and its five f32 values in each of three accumulators, twice over.
        let rec = std::mem::size_of::<MatchRec>() as u64;
        let per = 2 * rec + 2 * 3 * 5 * 4;
        assert!(c.held_bytes() - before >= 10 * per, "{}", c.held_bytes());
        // An address match costs the same.
        let mid = c.held_bytes();
        let mut a = upd(2, 10_300, Some(true), -75.0, 40.0, 10_000, 100.0, 0.0, 0.0);
        a.present |= tais_bit(TaisField::AcAddress);
        a.mode_s_addr = 1;
        c.observe(src("AAA"), &a);
        // That update is a proximity match too (one record) and an address record (another).
        assert!(c.held_bytes() - mid >= 2 * per, "{}", c.held_bytes() - mid);
    }

    // REQ: OA-CEN-009
    #[test]
    fn a_clip_of_another_day_does_not_overlap_and_the_check_says_so() {
        let day = 86_400_000;
        let mut c = xcensus(two(40.0));
        c.observe(
            src("AAA"),
            &upd(1, 10_300, Some(false), -75.0, 40.0, 10_000, 0.0, 0.0, 0.0),
        );
        assert!(c.finish().xc.unwrap().check_overlap().is_ok());
        let mut c = xcensus(two(40.0));
        c.observe(
            src("AAA"),
            &upd(
                1,
                day + 10_300,
                Some(false),
                -75.0,
                40.0,
                10_000,
                0.0,
                0.0,
                0.0,
            ),
        );
        let e = c.finish().xc.unwrap().check_overlap().unwrap_err();
        assert!(
            e.contains("do not overlap") && e.contains("1970-01-02"),
            "{e}"
        );
    }

    // REQ: OA-CEN-010
    #[test]
    fn rounding_to_f32_never_pushes_a_value_past_its_range() {
        for hi in [0.1_f64, 2407.6, 1852.0, 3_000.123] {
            let f = f32_in(Some(hi - 1e-12), 0.0, hi);
            assert!(f64::from(f) <= hi, "{hi} {f}");
            let g = f32_in(Some(-hi + 1e-12), -hi, hi);
            assert!(f64::from(g) >= -hi, "{hi} {g}");
        }
        assert!(f32_in(None, 0.0, 1.0).is_nan());
    }

    /// A record of track `n` with a coasting flag.
    fn coast(n: u32, t_ms: i64, flag: bool) -> TaisRecord {
        let mut r = rec(n, t_ms, Some(flag));
        r.present |= tais_bit(TaisField::TrackCoast);
        r.coast = true;
        r
    }

    // REQ: OA-CEN-008
    #[test]
    fn a_reused_track_number_is_classed_per_segment_not_as_mixed() {
        let mut c = Census::new();
        // One key: an adsb=true aircraft, then after a gap over 120 s an adsb=false one.
        for k in 0..3 {
            c.observe(src("AAA"), &rec(9, k * 1_000, Some(true)));
        }
        for k in 0..3 {
            c.observe(src("AAA"), &rec(9, 300_000 + k * 4_600, Some(false)));
        }
        let r = c.finish();
        assert_eq!(r.track_keys, 1);
        assert_eq!(r.global.segments, 2);
        assert_eq!(r.classes[2].segments, 1);
        assert_eq!(r.classes[1].segments, 1);
        assert_eq!(r.classes[3].segments, 0, "never mixed");
        assert_eq!(r.changes, 0);
        assert_eq!(r.change_bins, [2, 0, 0, 0, 0, 0]);
        assert_eq!(r.global.segments_with_false, 1);
        assert_eq!(r.classes[1].iv[0].n, 2);
        assert_eq!(r.classes[2].iv[1].n, 2);
    }

    // REQ: OA-CEN-019
    #[test]
    fn a_segment_is_never_joined_across_days() {
        const DAY: i64 = 86_400_000;
        let mut c = Census::new();
        // The same track number at the same time of day on two days, with different flags.
        for d in 0..2 {
            for k in 0..3 {
                c.observe(
                    src("AAA"),
                    &rec(4, d * DAY + 36_000_000 + k * 4_600, Some(d == 1)),
                );
            }
        }
        let r = c.finish();
        assert_eq!(r.global.segments, 2);
        assert_eq!(r.global.resets, 1);
        assert_eq!(r.classes[3].segments, 0);
        assert_eq!(r.changes, 0);
    }

    // REQ: OA-CEN-021
    #[test]
    fn coasting_updates_stay_out_of_the_tallies_and_have_counts_of_their_own() {
        let mut c = Census::new();
        for k in 0..6 {
            c.observe(src("AAA"), &rec(1, k * 1_000, Some(true)));
        }
        // Coasts carrying adsb=false inside the all-true segment, and one with adsb=true.
        c.observe(src("AAA"), &coast(1, 6_500, false));
        c.observe(src("AAA"), &coast(1, 7_000, false));
        c.observe(src("AAA"), &coast(1, 7_500, true));
        for k in 8..11 {
            c.observe(src("AAA"), &rec(1, k * 1_000, Some(true)));
        }
        let r = c.finish();
        assert_eq!(r.classes[2].segments, 1, "stays all adsb=true");
        assert_eq!(r.classes[3].segments, 0);
        assert_eq!(r.classes[2].updates, [0, 9]);
        assert_eq!(r.changes, 0);
        assert_eq!(r.global.segments_with_false, 0);
        // Counted as updates, and by kind and flag.
        assert_eq!(r.global.updates, [2, 10, 0]);
        assert_eq!(r.global.status[2], [2, 1, 0]);
        assert_eq!(r.global.status[0], [0, 0, 0]);
        assert_eq!(r.global.status_any, [2, 1, 0]);
        // The interval census holds the nine normal updates' intervals except the one that
        // starts at a coast: 5 before the coasts, then 8 s to 9 s and 9 s to 10 s.
        assert_eq!(r.global.iv[0].n, 0);
        assert_eq!(r.global.iv[1].n, 5 + 2);
    }

    // REQ: OA-CEN-009
    #[test]
    fn an_update_after_the_clip_is_outside_it_not_a_non_match() {
        let lat = 40.0;
        let mut c = xcensus(fly(1, -75.0, lat, 10_000));
        c.observe(
            src("AAA"),
            &upd(1, 10_300, Some(false), -75.0, lat, 10_000, 0.0, 0.0, 0.0),
        );
        // The clip ends at 120 s: these two are after it.
        c.observe(
            src("AAA"),
            &upd(2, 200_300, Some(false), -75.0, lat, 10_000, 0.0, 0.0, 0.0),
        );
        c.observe(
            src("AAA"),
            &upd(3, 200_300, Some(true), -75.0, lat, 10_000, 0.0, 0.0, 0.0),
        );
        let r = c.finish();
        let x = r.xc.unwrap();
        assert_eq!(x.not_checked.outside_clip, [1, 1]);
        let checked = |f: usize| (0..3).map(|k| x.global.counts[cidx(f, 0, k)]).sum::<u64>();
        assert_eq!((checked(0), checked(1)), (1, 0));
        assert_eq!(x.global.counts[cidx(1, 1, 0)], 0, "not a no-candidate");
        assert_eq!(x.global.covered, [1, 0]);
    }

    // REQ: OA-CEN-017
    #[test]
    fn a_placebo_lookup_off_the_clip_is_counted_apart_with_or_without_a_window() {
        let lat = 40.0;
        // Clip 0 to 120 s; the update at 100 s has +60 s at 160 s (outside) and -60 s at 40 s.
        let mut c = xcensus(fly(1, -75.0, lat, 10_000));
        for k in 0..3 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    100_300 + k * 1_000,
                    Some(false),
                    -75.0,
                    lat,
                    10_000,
                    0.0,
                    0.0,
                    0.0,
                ),
            );
        }
        let g = c.finish().xc.unwrap().global;
        assert_eq!(g.pout[0], 3);
        assert_eq!(g.pcounts[0..3], [0, 0, 0]);
        assert_eq!(g.pout[2], 0);
        assert_eq!(g.pcounts[6..9].iter().sum::<u64>(), 3);
    }

    // REQ: OA-CEN-022
    #[test]
    fn an_update_repeated_across_files_or_within_one_is_dropped_and_counted() {
        let mut c = Census::new();
        let a = |t: i64| rec(1, t, Some(false));
        // File one, then an overlapping file two that repeats its last two updates and adds one.
        for t in [0, 4_600, 9_200] {
            c.observe(src("AAA"), &a(t));
        }
        for t in [4_600, 9_200, 13_800, 13_800] {
            c.observe(src("AAA"), &a(t));
        }
        // Same time with another flag is a different update.
        c.observe(src("AAA"), &rec(1, 13_800, Some(true)));
        let r = c.finish();
        assert_eq!(r.duplicates, 3);
        assert_eq!(r.global.updates, [4, 1, 0]);
        assert_eq!(r.global.iv[0].n, 3);
    }

    // REQ: OA-CEN-022
    #[test]
    fn a_window_leaves_out_an_update_without_a_time() {
        let mut c = Census::new().with_window(Some(Window::new(0, 100).unwrap()));
        let mut nt = rec(1, 0, Some(false));
        nt.present &= !tais_bit(TaisField::MrtTime);
        c.observe(src("AAA"), &nt);
        c.observe(src("AAA"), &rec(1, 5_000, Some(false)));
        let r = c.finish();
        assert_eq!(r.no_time_outside, 1);
        assert_eq!(r.global.no_mrt, 0);
        assert_eq!(r.global.updates, [1, 0, 0]);
        // Without a window it is counted as an update with no time.
        let mut c = Census::new();
        c.observe(src("AAA"), &nt);
        let r = c.finish();
        assert_eq!((r.no_time_outside, r.global.no_mrt), (0, 1));
    }

    // REQ: OA-CEN-004
    #[test]
    fn a_record_with_neither_src_nor_source_id_is_an_unknown_facility() {
        let mut c = Census::new();
        c.observe([0; 8], &rec(1, 0, Some(false)));
        let mut named = rec(2, 0, Some(false));
        named.rec_stars_src_id = src("X1");
        c.observe([0; 8], &named);
        let r = c.finish();
        let keys: Vec<[u8; 8]> = r.facs.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, [[0; 8], src("X1")]);
        assert_eq!(crate::report::fac_label(&keys[0], 0), "facility unknown");
        assert_eq!(crate::report::fac_label(&keys[1], 1), "X1");
    }

    /// Asserts two segments, all false then all true, and no mixed one.
    fn two_pure_segments(r: &Results) {
        assert_eq!(r.global.segments, 2);
        assert_eq!(r.classes[1].segments, 1);
        assert_eq!(r.classes[2].segments, 1);
        assert_eq!(r.classes[3].segments, 0, "a cut, not a mixed track");
    }

    // REQ: OA-CEN-019
    #[test]
    fn an_update_flagged_new_cuts_the_segment() {
        let mut c = Census::new();
        c.observe(src("AAA"), &rec(1, 0, Some(false)));
        c.observe(src("AAA"), &rec(1, 4_600, Some(false)));
        let mut n = rec(1, 9_200, Some(true));
        n.present |= tais_bit(TaisField::TrackNew);
        n.is_new = true;
        c.observe(src("AAA"), &n);
        c.observe(src("AAA"), &rec(1, 13_800, Some(true)));
        two_pure_segments(&c.finish());
    }

    // REQ: OA-CEN-019
    #[test]
    fn three_backward_updates_in_a_row_cut_the_segment() {
        let mut c = Census::new();
        for t in [100_000, 104_600, 90_000, 80_000] {
            c.observe(src("AAA"), &rec(1, t, Some(false)));
        }
        // The third backward update starts the new segment.
        for t in [70_000, 74_600, 79_200] {
            c.observe(src("AAA"), &rec(1, t, Some(true)));
        }
        let r = c.finish();
        assert_eq!(r.global.resyncs, 1);
        two_pure_segments(&r);
    }

    // REQ: OA-CEN-019
    #[test]
    fn a_timed_update_after_an_untimed_one_cuts_the_segment() {
        let mut c = Census::new();
        let mut no_time = rec(1, 0, Some(false));
        no_time.present &= !tais_bit(TaisField::MrtTime);
        c.observe(src("AAA"), &rec(1, 0, Some(false)));
        c.observe(src("AAA"), &rec(1, 4_600, Some(false)));
        c.observe(src("AAA"), &no_time);
        c.observe(src("AAA"), &rec(1, 9_200, Some(true)));
        c.observe(src("AAA"), &rec(1, 13_800, Some(true)));
        two_pure_segments(&c.finish());
    }

    // REQ: OA-CEN-001
    #[test]
    fn a_coast_keeps_a_segment_alive_across_a_gap_and_only_status_segments_are_counted_apart() {
        // Measured updates 200 s apart are two segments; with coasts between them (STARS kept
        // the track) they are one.
        let mut c = Census::new();
        c.observe(src("AAA"), &rec(1, 0, Some(false)));
        c.observe(src("AAA"), &rec(1, 200_000, Some(false)));
        assert_eq!(c.finish().global.segments, 2);
        let mut c = Census::new();
        c.observe(src("AAA"), &rec(1, 0, Some(false)));
        for t in [60_000, 120_000, 180_000] {
            c.observe(src("AAA"), &coast(1, t, true));
        }
        c.observe(src("AAA"), &rec(1, 200_000, Some(false)));
        // A track number with only coasting updates, and one with only untagged flags.
        c.observe(src("AAA"), &coast(2, 0, false));
        c.observe(src("AAA"), &coast(2, 4_000, true));
        c.observe(src("AAA"), &rec(3, 0, None));
        let r = c.finish();
        assert_eq!(r.global.segments, 3);
        assert_eq!(r.classes[1].segments, 1, "the measured track");
        assert_eq!(r.segments_no_flag, 2);
        assert_eq!(r.classes[0].segments, 0, "not in the one-flagged class");
        // The interval that starts at a coast is not counted: none between 0 and 200 s.
        assert_eq!(r.global.iv[0].n, 0);
    }

    // REQ: OA-CEN-021
    #[test]
    fn the_shares_leave_out_status_updates_like_the_class_lines() {
        let mut c = Census::new();
        for k in 0..4 {
            c.observe(src("AAA"), &rec(1, k * 4_000, Some(true)));
        }
        for k in 0..4 {
            c.observe(src("AAA"), &coast(1, 20_000 + k * 1_000, false));
        }
        let r = c.finish();
        // Totals count every update; the flagged and identity counts do not count the coasts.
        assert_eq!(r.global.updates, [4, 4, 0]);
        assert_eq!((r.global.ids[0].n, r.global.ids[1].n), (0, 4));
        assert_eq!(r.classes[2].updates, [0, 4]);
        let text = crate::report::render(&crate::read::ReadStats::new(), &r);
        assert!(text.contains("(as the class lines do): 0.00%"), "{text}");
        assert!(text.contains("false share of the measured flagged updates 0.00%"));
    }

    fn with_records(n: u64) -> crate::read::ReadStats {
        let mut rs = crate::read::ReadStats::new();
        rs.tais_records = n;
        rs
    }

    // REQ: OA-CEN-016
    #[test]
    fn the_input_reconciliation_is_printed_without_a_clip_and_fails_when_it_should() {
        let w = Window::new(0, 100).unwrap();
        let mut c = Census::new().with_window(Some(w));
        let mut nt = rec(1, 0, Some(false));
        nt.present &= !tais_bit(TaisField::MrtTime);
        for r in [
            rec(1, 1_000, Some(false)),
            rec(1, 1_000, Some(false)),
            rec(1, 500_000, Some(false)),
            nt,
        ] {
            c.observe(src("AAA"), &r);
        }
        let r = c.finish();
        let ok = crate::report::render(&with_records(4), &r);
        assert!(
            ok.contains("input TAIS records 4 = updates 1 + duplicate updates dropped 1 + outside the window 1 + no time, outside the window 1 (reconciles)"),
            "{ok}"
        );
        let bad = crate::report::render(&with_records(5), &r);
        assert!(bad.contains("no time, outside the window 1 (DOES NOT RECONCILE)"));
    }

    // REQ: OA-CEN-016
    #[test]
    fn the_no_flag_and_placebo_lines_say_whether_they_reconcile() {
        let lat = 40.0;
        let mut c = xcensus(fly(1, -75.0, lat, 10_000));
        for k in 0..3 {
            c.observe(
                src("AAA"),
                &upd(
                    1,
                    100_300 + k * 1_000,
                    Some(false),
                    -75.0,
                    lat,
                    10_000,
                    0.0,
                    0.0,
                    0.0,
                ),
            );
        }
        c.observe(src("AAA"), &rec(2, 5_000, None));
        let r = c.finish();
        let text = crate::report::render(&with_records(4), &r);
        assert!(text.contains("no flag: updates 1 = not checked 1 (reconciles)"));
        assert!(text.contains(
            "placebo reconciliation: matched 0 + not matched 0 + outside the clip 3 = checked 3 (reconciles)"
        ));
        assert!(!text.contains("DOES NOT RECONCILE"), "{text}");
        // A placebo count that is off by one is reported.
        let mut bad = r.clone();
        if let Some(x) = bad.xc.as_mut() {
            x.global.pout[0] += 1;
        }
        let text = crate::report::render(&with_records(4), &bad);
        assert!(text.contains("outside the clip 4 = checked 3 (DOES NOT RECONCILE)"));
    }
}

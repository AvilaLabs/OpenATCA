//! Scoring protocol P-002: OpenATCA, ERAM and a passthrough baseline against one reference.
//!
//! ```text
//! cargo run --release -p oa-track --example p002 -- \
//!     --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> [--label <text>] [--allow-dirty] [--development] [--frozen-commit <sha>]
//! ```
//!
//! Written from the comparison's scoring protocol P-002 and its amendment P-003, fixed by their
//! hashes before any hold-out data was read; the protocol governs where this code and it differ. The split, the
//! tracker run, the reference, the ERAM load, the association and the pairing are those of
//! `vs_eram`, and the error definitions are `oa-bench`'s (cross-track and along-track in the
//! truth frame, the time offset as `metrics_th::fit_offset` defines it, here per aircraft).
//!
//! P-003 adds, for every system alike, the rounding-tolerant along-track error
//! `a_t = sign(a) * max(0, |a| - 0.5 s * v)` of the time-fitted along-track error `a` and the
//! reference ground speed `v` (M2t), and the horizontal error `hypot(cross, a_t)` (M3t); M4 and
//! M7 are also computed on M3t. The headline position metrics are M1 and M3t; M2 and M3 are
//! reported, but no claim of "better than ERAM" may rest on them. The breakdowns by the tracker's
//! coasting, uncertain (`O_UNCERTAIN`) and reacquired (`O_REACQUIRED`) flags report M1, M3t and M5
//! with sample counts, on the same resamples. The scorer records `git rev-parse HEAD` and
//! refuses a tree that `git status --porcelain` lists as modified unless `--allow-dirty` is
//! given, and then stamps every output file as not valid for hold-out scoring. `--development`
//! marks a run on development data the same way (stamped, not a hold-out score); a run with
//! neither flag is a hold-out run and fails if any OpenATCA sample has no tracker row for its
//! flags, or only an ambiguous one (see "The flag join"). A hold-out run also needs
//! `--frozen-commit <40 hex>` and is refused unless the commit the binary was built from (the
//! build-time variables `OA_BUILD_COMMIT` and `OA_BUILD_DIRTY`, see `tools/p002_build.sh`) is that
//! commit, that build's tree was clean, and the working tree at run time is clean and at it.
//!
//! Writes `p002.txt` (tables M1 to M7 and M2t, M3t, M4t, M7t with bootstrap intervals,
//! breakdowns by phase, altitude and the three flags, exclusions, offsets, digests, commit) and
//! `samples.csv` (one row per grid sample).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use oa_adsb::{reports_len, NONE_I32, NONE_U16};
use oa_bench::adsb_truth;
use oa_bench::assoc::{associate, astatus, TruthIndex};
use oa_bench::chunked::truth_index;
use oa_bench::geo::KT_MPS;
use oa_bench::input::{load, load_adsb, FeedInput};
use oa_bench::legacy::{Legacy, NO_ALT};
use oa_bench::pair::{build_pairs_with, PairTable};
use oa_bench::run::RunConfig;
use oa_bench::stats::{band, phase, Exclusions};
use oa_bench::thindex::build_th_index;
use oa_bench::tracker_input::{legacy_from_track_samples_by_leg, TrackSamples};
use oa_bench::truth::Truth;
use oa_replay::FeedKind;
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    interval_stats, run_tracker_spilled, score_ranges, scoring_config, split_reports,
    IntervalStats, RowSet, RowSource, SCORE_CHUNK_ROWS, SPLIT_TEXT,
};
use oa_track::DEFAULT_CONFIG;
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

/// A sample is matched to a system's row within this, ms (P-002 "Samples").
const TOL_MS: i64 = 500;
/// Samples below this truth ground speed do not enter a time fit, m/s (`OffsetConfig` default).
const FIT_MIN_SPEED_MPS: f64 = 50.0;
/// Bootstrap resamples and seed (P-002 "Uncertainty").
const BOOT_N: usize = 1000;
const BOOT_SEED: u64 = 20_261_119;
/// M7 threshold, metres (0.5 NM).
const LARGE_M: f64 = 926.0;
/// The passthrough rule: within this share of OpenATCA's value.
const FLOOR_SHARE: f64 = 0.10;

/// P-003 rounding tolerance: reference times are whole seconds, so along-track error within this
/// many seconds of travel at the reference ground speed is not charged.
const ROUND_TOL_S: f64 = 0.5;
/// The stamp on every output file of a run on a modified tree (P-003 section 2, item 13).
const DIRTY_STAMP: &str = "DIRTY TREE \u{2014} not valid for hold-out scoring";
/// The time offset of an aircraft with no usable fit: none (scored unfitted, counted).
const UNFITTED_TAU_S: f64 = 0.0;
/// The stamp on every output file of a run marked `--development`.
const DEV_STAMP: &str = "DEVELOPMENT RUN \u{2014} not a hold-out score";

const SYS: [&str; 3] = ["OpenATCA", "ERAM", "Passthrough"];
const OA: usize = 0;
const ER: usize = 1;
const PT: usize = 2;
const STATS: [&str; 3] = ["median", "p95", "rms"];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut sfdps, mut out, mut label) = (None, None, None, String::new());
    let (mut allow_dirty, mut development) = (false, false);
    let mut frozen: Option<String> = None;
    while let Some(a) = args.next() {
        if a == "--allow-dirty" {
            allow_dirty = true;
            continue;
        }
        if a == "--development" {
            development = true;
            continue;
        }
        let v = args.next();
        match a.as_str() {
            "--adsb" => adsb = v.map(PathBuf::from),
            "--sfdps-dir" => sfdps = v.map(PathBuf::from),
            "--out" => out = v.map(PathBuf::from),
            "--label" => label = v.unwrap_or_default(),
            "--frozen-commit" => frozen = v,
            _ => {
                eprintln!("p002: unknown option {a}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(adsb), Some(sfdps), Some(out)) = (adsb, sfdps, out) else {
        eprintln!(
            "usage: p002 --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> [--label <text>] [--allow-dirty] [--development] [--frozen-commit <sha>]"
        );
        return ExitCode::from(2);
    };
    let git = git_state();
    if git_is_dirty(&git) && !allow_dirty {
        eprintln!(
            "p002: refused: the working tree is not clean ({}); a hold-out score is valid only from a clean checkout at the frozen commit (--allow-dirty runs it, stamped as not valid)",
            git_tree_text(&git)
        );
        return ExitCode::from(2);
    }
    // A hold-out run is one that is neither marked development nor allowed on a dirty tree.
    let mode = Mode {
        holdout: !allow_dirty && !development,
        development,
    };
    if let Err(e) = holdout_binding(
        mode,
        frozen.as_deref(),
        Build {
            commit: option_env!("OA_BUILD_COMMIT"),
            dirty: option_env!("OA_BUILD_DIRTY"),
        },
        &git,
    ) {
        eprintln!("p002: refused: {e}");
        return ExitCode::from(2);
    }
    match run(&adsb, &sfdps, &out, &label, &git, mode) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("p002: {e}");
            ExitCode::from(1)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Small numerics: PRNG, binomial test, statistics.
// ---------------------------------------------------------------------------------------------

/// splitmix64 (Steele, Lea, Flood): the bootstrap's generator.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (multiply-shift; the bias is below 2^-40 for the sizes used here).
    fn below(&mut self, n: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }
}

/// Two-sided exact binomial (p = 1/2) p-value for `k` of `n`, in f64 with log factorials:
/// twice the smaller tail, capped at 1.
fn sign_test_p(k: u64, n: u64) -> f64 {
    if n == 0 {
        return f64::NAN;
    }
    let mut lf = Vec::with_capacity(n as usize + 1);
    let mut acc = 0.0f64;
    lf.push(0.0);
    for i in 1..=n {
        acc += (i as f64).ln();
        lf.push(acc);
    }
    let at = |i: u64| lf.get(i as usize).copied().unwrap_or(f64::NAN);
    let lo = k.min(n - k.min(n));
    let ln2n = n as f64 * std::f64::consts::LN_2;
    let tail: f64 = (0..=lo)
        .map(|i| (at(n) - at(i) - at(n - i) - ln2n).exp())
        .sum();
    (2.0 * tail).min(1.0)
}

/// Nearest-rank percentile `p` of `buf` (any order, rearranged): rank `ceil(p n / 100)`.
fn rank_of(buf: &mut [f64], p: u64) -> f64 {
    let n = buf.len() as u64;
    if n == 0 {
        return f64::NAN;
    }
    let k = (p * n).div_ceil(100).clamp(1, n) as usize - 1;
    let (_, v, _) = buf.select_nth_unstable_by(k, |a, b| oa_math::cmp_canon_f64(*a, *b));
    *v
}

/// `[median, p95, rms]` of absolute values (the library's definitions: nearest rank, no
/// interpolation, RMS from the sum of squares). NaN for no values.
fn stat3(buf: &mut [f64]) -> [f64; 3] {
    if buf.is_empty() {
        return [f64::NAN; 3];
    }
    let rms = (buf.iter().map(|v| v * v).sum::<f64>() / buf.len() as f64).sqrt();
    let med = rank_of(buf, 50);
    let p95 = rank_of(buf, 95);
    [med, p95, rms]
}

/// Signed nearest-rank percentile of a sorted copy.
fn signed_rank(v: &[f64], p: u64) -> f64 {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    rank_of(&mut s, p)
}

// ---------------------------------------------------------------------------------------------
// Samples per system.
// ---------------------------------------------------------------------------------------------

/// A flag of a tracker row, or no tracker row to read it from (E6: never a default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Tri {
    /// No tracker row matched the sample, or the rows that matched it disagree.
    Unmatched = 0,
    No = 1,
    Yes = 2,
}

/// `0`, `1` or `unmatched`, for the CSV.
fn tri_text(t: Tri) -> String {
    match t {
        Tri::Unmatched => "unmatched".to_string(),
        Tri::No => "0".to_string(),
        Tri::Yes => "1".to_string(),
    }
}

fn tri_of(b: bool) -> Tri {
    if b {
        Tri::Yes
    } else {
        Tri::No
    }
}

/// One system's paired sample at its own row time.
///
/// A7: held per grid sample and system, so the size is asserted. 56 bytes: six 8-byte numbers,
/// five 1-byte fields, padding to 8. A7.1: `Tri` replaces the two `bool` flags of P-003 at the
/// same width, so the flags added no bytes (they sit in what was padding).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Pt {
    t: i64,
    cross: f64,
    along: f64,
    /// Truth ground speed, m/s.
    v: f64,
    /// Ground-speed error, m/s (NaN when absent).
    spd: f64,
    /// Track-angle error, degrees (NaN when absent).
    hdg: f64,
    coast: u8,
    /// The tracker's `O_UNCERTAIN` and `O_REACQUIRED` flags of the row (OpenATCA only).
    uncertain: Tri,
    reacquired: Tri,
    phase: u8,
    band: u8,
}
const _: () = assert!(core::mem::size_of::<Pt>() == 56);

type PerAc = BTreeMap<u32, Vec<Pt>>;

#[derive(Default, Clone, Copy)]
// A7.2: cold struct, size guard waived
struct Dropped {
    other_flight: u64,
    no_cross_along: u64,
    duplicate_time: u64,
}

/// Each aircraft's primary flight (most finite position pairs, lowest flight on a tie; never the
/// one with the smaller error), its finite position pairs, sorted by time, one per time.
fn points_of(p: &PairTable, per: &mut PerAc, dropped: &mut Dropped) {
    let n = p.t_ms.len();
    let fin = |i: usize| {
        p.cross_m.get(i).is_some_and(|v| v.is_finite())
            && p.along_m.get(i).is_some_and(|v| v.is_finite())
    };
    let mut count: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for i in 0..n {
        if let (Some(&a), true) = (p.addr.get(i), fin(i)) {
            *count
                .entry((a, p.flight.get(i).copied().unwrap_or(0)))
                .or_default() += 1;
        }
    }
    let mut primary: BTreeMap<u32, (u32, u64)> = BTreeMap::new();
    for (&(a, f), &c) in &count {
        let e = primary.entry(a).or_insert((f, c));
        if c > e.1 {
            *e = (f, c);
        }
    }
    let nan = f32::NAN;
    let mut local: PerAc = BTreeMap::new();
    for i in 0..n {
        let (Some(&a), Some(&t)) = (p.addr.get(i), p.t_ms.get(i)) else {
            continue;
        };
        let f = p.flight.get(i).copied().unwrap_or(0);
        if primary.get(&a).map(|e| e.0) != Some(f) {
            if fin(i) {
                dropped.other_flight += 1;
            }
            continue;
        }
        if !fin(i) {
            dropped.no_cross_along += 1;
            continue;
        }
        let g = |v: &[f32], d: f32| f64::from(v.get(i).copied().unwrap_or(d));
        local.entry(a).or_default().push(Pt {
            t,
            cross: g(&p.cross_m, nan),
            along: g(&p.along_m, nan),
            v: g(&p.truth_speed_mps, nan),
            spd: g(&p.speed_err_kt, nan) * KT_MPS,
            hdg: g(&p.hdg_err_deg, nan),
            coast: p.coast.get(i).copied().unwrap_or(0),
            uncertain: Tri::Unmatched,
            reacquired: Tri::Unmatched,
            phase: p.phase.get(i).copied().unwrap_or(0),
            band: p.band.get(i).copied().unwrap_or(0),
        });
    }
    for (a, mut v) in local {
        v.sort_by_key(|s| s.t);
        let before = v.len();
        v.dedup_by_key(|s| s.t);
        dropped.duplicate_time += (before - v.len()) as u64;
        per.entry(a).or_default().extend(v);
    }
}

// ---------------------------------------------------------------------------------------------
// The flag join.
// ---------------------------------------------------------------------------------------------

/// What the tracker rows at one (address, millisecond) say of the two flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlagEntry {
    One {
        uncertain: bool,
        reacquired: bool,
    },
    /// Two or more rows at the key that disagree: the flags of a sample there are unknown.
    Conflict,
}

/// The tracker's flags by (address, row time in whole milliseconds).
///
/// The join key is the finest time both sources carry: the pair table that gives a sample its
/// time holds whole milliseconds (`t_ns` floored, as `legacy_from_track_samples_by_leg` does), so
/// two tracker rows within one millisecond share a key. Such keys are counted, never overwritten:
/// rows that agree stay one entry, rows that disagree make the key a conflict.
// A7.2: cold struct, size guard waived
#[derive(Debug, Default)]
struct FlagJoin {
    map: BTreeMap<(u32, i64), FlagEntry>,
    /// Rows whose key already held a row.
    duplicate_rows: u64,
    /// Keys whose rows disagree on a flag.
    conflict_keys: u64,
}

/// Builds the join from a range's tracker rows; the columns must be as long as `addr`.
fn flag_join(
    addr: &[u32],
    t_ns: &[i64],
    uncertain: &[bool],
    reacquired: &[bool],
) -> Result<FlagJoin, String> {
    let n = addr.len();
    if [t_ns.len(), uncertain.len(), reacquired.len()]
        .iter()
        .any(|&l| l != n)
    {
        return Err("tracker row columns have different lengths".to_string());
    }
    let mut j = FlagJoin::default();
    for (((&a, &t), &u), &re) in addr.iter().zip(t_ns).zip(uncertain).zip(reacquired) {
        let new = FlagEntry::One {
            uncertain: u,
            reacquired: re,
        };
        match j.map.entry((a, t.div_euclid(1_000_000))) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(new);
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                j.duplicate_rows = j.duplicate_rows.saturating_add(1);
                if *e.get() != new && *e.get() != FlagEntry::Conflict {
                    j.conflict_keys = j.conflict_keys.saturating_add(1);
                    e.insert(FlagEntry::Conflict);
                }
            }
        }
    }
    Ok(j)
}

/// The `(uncertain, reacquired)` states of a sample: unmatched when no tracker row has its key or
/// the rows there disagree.
fn flag_lookup(j: &FlagJoin, addr: u32, t_ms: i64) -> (Tri, Tri) {
    match j.map.get(&(addr, t_ms)) {
        Some(FlagEntry::One {
            uncertain,
            reacquired,
        }) => (tri_of(*uncertain), tri_of(*reacquired)),
        Some(FlagEntry::Conflict) | None => (Tri::Unmatched, Tri::Unmatched),
    }
}

/// The sample nearest in time to `t` within [`TOL_MS`] (the earlier on a tie).
fn nearest(v: &[Pt], t: i64) -> Option<&Pt> {
    let i = v.partition_point(|s| s.t < t);
    let before = i.checked_sub(1).and_then(|j| v.get(j));
    let pick = match (before, v.get(i)) {
        (Some(b), Some(a)) => {
            if t - b.t <= a.t - t {
                b
            } else {
                a
            }
        }
        (Some(b), None) => b,
        (None, Some(a)) => a,
        (None, None) => return None,
    };
    ((pick.t - t).abs() <= TOL_MS).then_some(pick)
}

/// Association, pairing and exclusions of one legacy table against the reference.
// A7.2: cold struct, size guard waived
struct Paired {
    pairs: PairTable,
    ex: Exclusions,
    assoc_counts: [u64; astatus::COUNT],
}

fn pair_legacy(leg: &Legacy, truth: &Truth<'_>, tix: &TruthIndex, rcfg: &RunConfig) -> Paired {
    let ix = build_th_index(&leg.th, leg.flights.gufi.len());
    let assoc = associate(leg, &ix, truth, tix, &rcfg.assoc);
    let (pairs, ex) = build_pairs_with(leg, &ix, &assoc, truth, &rcfg.pair, &|_, _, _| None);
    Paired {
        pairs,
        ex,
        assoc_counts: assoc.counts,
    }
}

// ---------------------------------------------------------------------------------------------
// The grid.
// ---------------------------------------------------------------------------------------------

/// One system's value at a grid sample, with its fitted-time quantities.
///
/// A7: held per grid sample and system. 96 bytes: a [`Pt`] (56) and five 8-byte quantities.
/// A7.1: P-003 added `along_tol` and `h_tol` (+16 bytes), the rounding-tolerant errors M2t and M3t
/// the metrics read per sample; they are stored, not recomputed, because every table, bootstrap
/// resample and the CSV read them. This is an offline scorer on a few million samples.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct Val {
    pt: Pt,
    along_fit: f64,
    h_raw: f64,
    h_fit: f64,
    /// P-003 rounding-tolerant along-track error and horizontal error.
    along_tol: f64,
    h_tol: f64,
}
const _: () = assert!(core::mem::size_of::<Val>() == 96);

/// A grid sample: its three systems' values.
///
/// A7: one per grid sample. 312 bytes: `ac` and `t` (16), `addr` and two 1-byte strata (8 with
/// padding), and three `Option<Val>` of 96 (the `Tri` niche leaves no tag). A7.1: grew with `Val`
/// (+48 bytes for three systems), see there.
#[derive(Clone, Debug)]
#[repr(C)]
struct Row {
    /// Index of the aircraft among the grid aircraft.
    ac: usize,
    t: i64,
    addr: u32,
    /// Phase and altitude band of the ERAM sample (the stratum of every system's value).
    phase: u8,
    band: u8,
    val: [Option<Val>; 3],
}
const _: () = assert!(core::mem::size_of::<Row>() == 312);

impl Row {
    fn common(&self) -> bool {
        self.val.iter().all(Option::is_some)
    }
}

/// The rows of grid aircraft `ac` (rows are in aircraft order).
fn of_ac(rows: &[Row], ac: usize) -> &[Row] {
    let a = rows.partition_point(|r| r.ac < ac);
    let b = rows.partition_point(|r| r.ac <= ac);
    rows.get(a..b).unwrap_or(&[])
}

/// The least-squares offset `tau` of `along = tau * v` over `(along, v)` pairs (library fit).
fn fit_tau(it: impl Iterator<Item = (f64, f64)>) -> Option<f64> {
    let (mut sev, mut svv) = (0.0, 0.0);
    for (e, v) in it {
        if e.is_finite() && v.is_finite() && v >= FIT_MIN_SPEED_MPS {
            sev += e * v;
            svv += v * v;
        }
    }
    (svv > 0.0).then(|| sev / svv)
}

/// Per system, the aircraft with a value but no usable time fit, and their samples with a value:
/// `(aircraft, samples)`. Such samples are scored unfitted (P-003 resolution 1).
fn unfitted_counts(rows: &[Row], taus: &[[Option<f64>; 3]]) -> [(u64, u64); 3] {
    let mut acs: [BTreeSet<usize>; 3] = [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()];
    let mut samples = [0u64; 3];
    for r in rows {
        let tau = taus.get(r.ac).copied().unwrap_or([None; 3]);
        for (((v, t), set), n) in r.val.iter().zip(tau).zip(acs.iter_mut()).zip(&mut samples) {
            if v.is_some() && t.is_none() {
                set.insert(r.ac);
                *n += 1;
            }
        }
    }
    let mut out = [(0u64, 0u64); 3];
    for ((o, set), n) in out.iter_mut().zip(&acs).zip(samples) {
        *o = (set.len() as u64, n);
    }
    out
}

/// M4: per system, the samples within `thr` over ALL grid samples (a miss, and a value that is
/// not a number, are failures), and the number of present values that are not a number.
fn m4_counts(rows: &[Row], n_ac: usize, h: fn(&Val) -> f64, thr: f64) -> ([Cnt; 3], [u64; 3]) {
    let mut c = [Cnt::new(n_ac), Cnt::new(n_ac), Cnt::new(n_ac)];
    let mut nan = [0u64; 3];
    for r in rows {
        for ((cs, n), v) in c.iter_mut().zip(nan.iter_mut()).zip(&r.val) {
            if v.as_ref().is_some_and(|v| h(v).is_nan()) {
                *n += 1;
            }
            cs.add(r.ac, v.as_ref().is_some_and(|v| h(v) <= thr));
        }
    }
    (c, nan)
}

/// M7 and M7t: failures over `LARGE_M` of the horizontal error `h`, a miss and a value that is not
/// a number counting as failures.
// A7.2: cold struct, size guard waived
struct Large {
    cnt: [Cnt; 3],
    /// Present values over the limit or not a number.
    over: [u64; 3],
    /// Present values that are not a number (counted in `over`).
    nan: [u64; 3],
    /// Present raw (unfitted) values over the limit.
    raw_over: [u64; 3],
}

fn large_counts(rows: &[Row], n_ac: usize, h: fn(&Val) -> f64) -> Large {
    let mut l = Large {
        cnt: [Cnt::new(n_ac), Cnt::new(n_ac), Cnt::new(n_ac)],
        over: [0; 3],
        nan: [0; 3],
        raw_over: [0; 3],
    };
    for r in rows {
        for (s, v) in r.val.iter().enumerate() {
            let big = v.as_ref().is_some_and(|v| fails_over(h(v), LARGE_M));
            if let (Some(v), Some(n)) = (v, l.nan.get_mut(s)) {
                *n += u64::from(h(v).is_nan());
            }
            if let (true, Some(n)) = (big, l.over.get_mut(s)) {
                *n += 1;
            }
            if let (true, Some(n)) = (
                v.as_ref().is_some_and(|v| v.h_raw > LARGE_M),
                l.raw_over.get_mut(s),
            ) {
                *n += 1;
            }
            if let Some(c) = l.cnt.get_mut(s) {
                c.add(r.ac, v.is_none() || big);
            }
        }
    }
    l
}

/// M6: per aircraft with common samples, whether OpenATCA's RMS of the fitted horizontal error is
/// lower than ERAM's. An aircraft whose RMS is not finite for either system is a failure for the
/// share, is counted per system in `nan`, and is left out of the win, lose, tie counts and the
/// sign test (a comparison with NaN is not a tie).
// A7.2: cold struct, size guard waived
struct M6 {
    win: u64,
    lose: u64,
    tie: u64,
    /// Aircraft with a non-finite RMS: `[OpenATCA, ERAM]`.
    nan: [u64; 2],
    share: Cnt,
}

fn m6_counts(rows: &[Row], n_ac: usize) -> M6 {
    let mut m = M6 {
        win: 0,
        lose: 0,
        tie: 0,
        nan: [0; 2],
        share: Cnt::new(n_ac),
    };
    for ac in 0..n_ac {
        let mut ss = [(0.0f64, 0u64); 2];
        for r in of_ac(rows, ac).iter().filter(|r| r.common()) {
            for (k, s) in [OA, ER].into_iter().enumerate() {
                if let (Some(v), Some(slot)) =
                    (r.val.get(s).and_then(|v| v.as_ref()), ss.get_mut(k))
                {
                    slot.0 += v.h_fit * v.h_fit;
                    slot.1 += 1;
                }
            }
        }
        let (a, b) = (ss[0], ss[1]);
        if a.1 == 0 {
            continue;
        }
        let (ra, rb) = ((a.0 / a.1 as f64).sqrt(), (b.0 / b.1 as f64).sqrt());
        if !(ra.is_finite() && rb.is_finite()) {
            m.nan[0] += u64::from(!ra.is_finite());
            m.nan[1] += u64::from(!rb.is_finite());
            m.share.add(ac, false);
            continue;
        }
        if ra < rb {
            m.win += 1;
        } else if ra > rb {
            m.lose += 1;
        } else {
            m.tie += 1;
        }
        m.share.add(ac, ra < rb);
    }
    m
}

/// Samples of all flagged OpenATCA points still without a tracker row for their flags, counted
/// over every point (not only the addresses the join saw).
fn unmatched_points(pts: &PerAc) -> u64 {
    pts.values()
        .flatten()
        .filter(|p| p.uncertain == Tri::Unmatched || p.reacquired == Tri::Unmatched)
        .count() as u64
}

/// The hold-out gate: a hold-out run with any unmatched sample is refused.
fn flag_gate(mode: Mode, unmatched: u64, dup_rows: u64, conflicts: u64) -> Result<(), String> {
    if mode.holdout && unmatched > 0 {
        return Err(format!(
            "hold-out run refused: {unmatched} OpenATCA samples have no tracker row (or only conflicting rows) for their flags; {conflicts} keys conflict, {dup_rows} duplicate rows (use --development for a non-hold-out run)"
        ));
    }
    Ok(())
}

/// Whether an error counts as a failure over `limit` in a coverage-penalised metric: over it, or
/// not a number. A value that cannot be computed is never a pass (E6, G6); M4 reads `h <= thr` as
/// the pass and so does this.
fn fails_over(h: f64, limit: f64) -> bool {
    h.is_nan() || h > limit
}

/// P-003 rounding-tolerant along-track error: the fitted error `a` less `0.5 s * v` of travel at
/// the reference ground speed `v`, floored at zero and keeping the sign of `a`. NaN when either
/// is not finite.
fn along_tol(a: f64, v: f64) -> f64 {
    if !(a.is_finite() && v.is_finite()) {
        return f64::NAN;
    }
    let m = (a.abs() - ROUND_TOL_S * v).max(0.0);
    if a < 0.0 {
        -m
    } else {
        m
    }
}

/// What `git` said of the tree the scorer was built from.
// A7.2: cold struct, size guard waived
struct GitState {
    head: String,
    /// `git status --porcelain` output; `None` when git could not say.
    porcelain: Option<String>,
}

/// How a run was started: a hold-out run is neither marked development nor allowed dirty.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
struct Mode {
    holdout: bool,
    development: bool,
}

/// What the binary says it was built from: `OA_BUILD_COMMIT` (a commit) and `OA_BUILD_DIRTY`
/// (`0` for a clean tree), set by whoever built it (`tools/p002_build.sh`); `None` when unset.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy)]
struct Build<'a> {
    commit: Option<&'a str>,
    dirty: Option<&'a str>,
}

/// Whether `s` is a full commit id: 40 lowercase hex digits.
fn is_full_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The hold-out gate on provenance (not on what is computed): a hold-out run needs
/// `--frozen-commit`, a binary built from exactly that commit on a clean tree, and a working tree
/// at run time that is at that commit and clean. Any other mode is not checked here.
fn holdout_binding(
    mode: Mode,
    frozen: Option<&str>,
    build: Build<'_>,
    git: &GitState,
) -> Result<(), String> {
    if !mode.holdout {
        return Ok(());
    }
    let Some(frozen) = frozen else {
        return Err("a hold-out run needs --frozen-commit <sha> (or use --development)".into());
    };
    if !is_full_sha(frozen) {
        return Err(format!(
            "--frozen-commit must be 40 lowercase hex digits, got `{frozen}`"
        ));
    }
    let Some(built) = build.commit else {
        return Err("this binary does not say what it was built from (OA_BUILD_COMMIT unset at build time; build with tools/p002_build.sh)".into());
    };
    if built != frozen {
        return Err(format!(
            "the binary was built from {built}, not the frozen commit {frozen}"
        ));
    }
    if build.dirty != Some("0") {
        return Err("the binary was built from a tree that was not clean".into());
    }
    if git.head != frozen {
        return Err(format!(
            "the working tree is at {}, not the frozen commit {frozen}",
            git.head
        ));
    }
    if git_is_dirty(git) {
        return Err(format!(
            "the working tree is not clean ({})",
            git_tree_text(git)
        ));
    }
    Ok(())
}

fn git_is_dirty(g: &GitState) -> bool {
    tree_is_dirty(g.porcelain.as_deref())
}

fn git_tree_text(g: &GitState) -> String {
    match g.porcelain.as_deref() {
        None => "git status unavailable".to_string(),
        Some(p) => {
            let n = p.lines().filter(|l| !l.trim().is_empty()).count();
            if n == 0 {
                "clean".to_string()
            } else {
                format!("{n} paths modified or untracked")
            }
        }
    }
}

/// Whether a tree is unfit for hold-out scoring: any line of `git status --porcelain`, or no
/// answer from git at all.
fn tree_is_dirty(porcelain: Option<&str>) -> bool {
    porcelain.is_none_or(|p| p.lines().any(|l| !l.trim().is_empty()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cross,
    AlongRaw,
    AlongFit,
    HFit,
    AlongTol,
    HTol,
    Speed,
    Track,
}

impl Kind {
    fn get(self, v: &Val) -> f64 {
        match self {
            Kind::Cross => v.pt.cross.abs(),
            Kind::AlongRaw => v.pt.along.abs(),
            Kind::AlongFit => v.along_fit.abs(),
            Kind::HFit => v.h_fit,
            Kind::AlongTol => v.along_tol.abs(),
            Kind::HTol => v.h_tol,
            Kind::Speed => v.pt.spd.abs(),
            Kind::Track => v.pt.hdg.abs(),
        }
    }
}

/// Whether the row is in the common set for `k`: all three systems have a finite value.
fn common_for(r: &Row, k: Kind) -> bool {
    r.val
        .iter()
        .all(|v| v.as_ref().is_some_and(|v| k.get(v).is_finite()))
}

// ---------------------------------------------------------------------------------------------
// Bootstrap over aircraft.
// ---------------------------------------------------------------------------------------------

/// Values per aircraft, concatenated: aircraft `i` owns `data[start[i]..start[i + 1]]`.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct Col {
    start: Vec<usize>,
    data: Vec<f64>,
}

fn build_col(rows: &[Row], n_ac: usize, f: &dyn Fn(&Row) -> Option<f64>) -> Col {
    let mut cnt = vec![0usize; n_ac];
    let mut data = Vec::new();
    for r in rows {
        if let Some(x) = f(r) {
            data.push(x);
            if let Some(c) = cnt.get_mut(r.ac) {
                *c += 1;
            }
        }
    }
    let mut start = Vec::with_capacity(n_ac + 1);
    let mut s = 0;
    start.push(0);
    for c in cnt {
        s += c;
        start.push(s);
    }
    Col { start, data }
}

impl Col {
    fn slice(&self, i: usize) -> &[f64] {
        match (self.start.get(i), self.start.get(i + 1)) {
            (Some(&a), Some(&b)) => self.data.get(a..b).unwrap_or(&[]),
            _ => &[],
        }
    }

    fn point(&self) -> [f64; 3] {
        let mut b = self.data.clone();
        stat3(&mut b)
    }

    fn boot(&self, draws: &[Vec<u32>]) -> Vec<[f64; 3]> {
        let mut buf = Vec::with_capacity(self.data.len() + 16);
        draws
            .iter()
            .map(|d| {
                buf.clear();
                for &i in d {
                    buf.extend_from_slice(self.slice(i as usize));
                }
                stat3(&mut buf)
            })
            .collect()
    }
}

/// Per-aircraft counts of a share: `num` successes of `den` samples.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct Cnt {
    num: Vec<u64>,
    den: Vec<u64>,
}

impl Cnt {
    fn new(n_ac: usize) -> Cnt {
        Cnt {
            num: vec![0; n_ac],
            den: vec![0; n_ac],
        }
    }

    fn add(&mut self, ac: usize, ok: bool) {
        if let (Some(n), Some(d)) = (self.num.get_mut(ac), self.den.get_mut(ac)) {
            *n += u64::from(ok);
            *d += 1;
        }
    }

    fn point(&self) -> (u64, u64) {
        (self.num.iter().sum(), self.den.iter().sum())
    }

    /// The share per resample (NaN for an empty resample).
    fn boot(&self, draws: &[Vec<u32>]) -> Vec<f64> {
        draws
            .iter()
            .map(|d| {
                let (mut n, mut m) = (0u64, 0u64);
                for &i in d {
                    n += self.num.get(i as usize).copied().unwrap_or(0);
                    m += self.den.get(i as usize).copied().unwrap_or(0);
                }
                if m == 0 {
                    f64::NAN
                } else {
                    n as f64 / m as f64
                }
            })
            .collect()
    }
}

/// The 95% interval of resampled values: ranks 25 and 975 of 1,000 sorted values (the 2.5th and
/// 97.5th nearest-rank percentiles); NaN resamples are skipped.
fn interval(v: impl Iterator<Item = f64>) -> (f64, f64) {
    let mut s: Vec<f64> = v.filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let n = s.len() as u64;
    let lo = ((25 * n).div_ceil(1000)).clamp(1, n) as usize - 1;
    let hi = ((975 * n).div_ceil(1000)).clamp(1, n) as usize - 1;
    (
        s.get(lo).copied().unwrap_or(f64::NAN),
        s.get(hi).copied().unwrap_or(f64::NAN),
    )
}

fn draws_of(n_ac: usize) -> Vec<Vec<u32>> {
    let mut rng = SplitMix(BOOT_SEED);
    (0..BOOT_N)
        .map(|_| {
            (0..n_ac)
                .map(|_| rng.below(n_ac as u64) as u32)
                .collect::<Vec<u32>>()
        })
        .collect()
}

/// One metric over the three systems: values, and the resampled statistics.
// A7.2: cold struct (one per metric or aircraft column, not per sample), size guard waived
struct Metric {
    n: [usize; 3],
    /// Samples present in all three systems and kept by the group, left out of the common set
    /// because this system's value is not finite (E6): reported apart from "not common".
    nonfinite: [usize; 3],
    pt: [[f64; 3]; 3],
    boot: [Vec<[f64; 3]>; 3],
}

fn metric(
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
    keep: &dyn Fn(&Row) -> bool,
) -> Metric {
    let col = |s: usize| {
        build_col(rows, n_ac, &|r: &Row| {
            if keep(r) && common_for(r, k) {
                r.val.get(s).and_then(|v| v.as_ref()).map(|v| k.get(v))
            } else {
                None
            }
        })
    };
    let cols = [col(OA), col(ER), col(PT)];
    let mut nonfinite = [0usize; 3];
    for r in rows
        .iter()
        .filter(|r| keep(r) && r.common() && !common_for(r, k))
    {
        for (n, v) in nonfinite.iter_mut().zip(&r.val) {
            if v.as_ref().is_some_and(|v| !k.get(v).is_finite()) {
                *n += 1;
            }
        }
    }
    Metric {
        nonfinite,
        n: [cols[0].data.len(), cols[1].data.len(), cols[2].data.len()],
        pt: [cols[0].point(), cols[1].point(), cols[2].point()],
        boot: [
            cols[0].boot(draws),
            cols[1].boot(draws),
            cols[2].boot(draws),
        ],
    }
}

fn fmt_ci(pt: f64, lo: f64, hi: f64, prec: usize) -> String {
    format!("{pt:.prec$} [{lo:.prec$}, {hi:.prec$}]")
}

/// Point value of statistic `k` of system `s`.
fn pv(m: &Metric, s: usize, k: usize) -> f64 {
    m.pt.get(s)
        .and_then(|a| a.get(k))
        .copied()
        .unwrap_or(f64::NAN)
}

/// Resampled statistics of system `s`.
fn bv(m: &Metric, s: usize) -> &[[f64; 3]] {
    m.boot.get(s).map_or(&[], Vec::as_slice)
}

/// `value [lo, hi]` of statistic `k` of system `s`.
fn cell(m: &Metric, s: usize, k: usize, prec: usize) -> String {
    let (lo, hi) = interval(bv(m, s).iter().filter_map(|b| b.get(k).copied()));
    fmt_ci(pv(m, s, k), lo, hi, prec)
}

/// OpenATCA minus ERAM, statistic `k`, with the interval of the paired resamples.
fn diff_cell(m: &Metric, k: usize, prec: usize) -> String {
    let d = pv(m, OA, k) - pv(m, ER, k);
    let (lo, hi) = interval(
        bv(m, OA)
            .iter()
            .zip(bv(m, ER))
            .filter_map(|(a, b)| Some(a.get(k).copied()? - b.get(k).copied()?)),
    );
    fmt_ci(d, lo, hi, prec)
}

/// The passthrough rule for one value: does the passthrough baseline come within 10 % of
/// OpenATCA's?
fn floor_note(oa: f64, pt: f64) -> String {
    if !(oa.is_finite() && pt.is_finite()) {
        return "undefined (no value)".to_string();
    }
    let rel = if oa != 0.0 {
        (pt - oa).abs() / oa.abs()
    } else if pt == 0.0 {
        0.0
    } else {
        f64::INFINITY
    };
    if rel <= FLOOR_SHARE {
        format!(
            "APPLIES: at the reference floor ({:.1} % apart)",
            rel * 100.0
        )
    } else {
        format!("does not apply ({:.1} % apart)", rel * 100.0)
    }
}

fn write_metric(o: &mut String, title: &str, unit: &str, m: &Metric) {
    let _ = writeln!(
        o,
        "{title} ({unit}; common set; absolute values for median and p95)"
    );
    for (k, name) in STATS.iter().enumerate() {
        let _ = writeln!(o, "  {name}:");
        for (s, sys) in SYS.iter().enumerate() {
            let _ = writeln!(
                o,
                "    {sys:<12} {}   n={}",
                cell(m, s, k, 3),
                m.n.get(s).copied().unwrap_or(0)
            );
        }
        let _ = writeln!(o, "    OpenATCA minus ERAM  {}", diff_cell(m, k, 3));
        let _ = writeln!(
            o,
            "    passthrough within 10%: {}",
            floor_note(pv(m, OA, k), pv(m, PT, k))
        );
    }
    let _ = writeln!(o, "  {}", nonfinite_text(m));
    let _ = writeln!(o);
}

/// The samples a metric left out for a value that is not finite, per system, apart from the
/// samples that were simply not in all three systems.
fn nonfinite_text(m: &Metric) -> String {
    format!(
        "left out of the common set for a non-finite value (present in all three systems): OpenATCA {}, ERAM {}, Passthrough {}",
        m.nonfinite[0], m.nonfinite[1], m.nonfinite[2]
    )
}

/// A breakdown: one block per stratum, medians with intervals and the difference, p95 and RMS as
/// point values.
fn write_strata(
    o: &mut String,
    title: &str,
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
) {
    let _ = writeln!(
        o,
        "{title}: breakdown (common set; median with 95% interval, p95 and rms as points)"
    );
    let mut strata: Vec<(String, Keep)> = Vec::new();
    for (i, name) in phase::NAMES.iter().enumerate() {
        strata.push((
            format!("phase={name}"),
            Box::new(move |r: &Row| usize::from(r.phase) == i),
        ));
    }
    for (i, name) in band::NAMES.iter().enumerate() {
        strata.push((
            format!("altitude={name}"),
            Box::new(move |r: &Row| usize::from(r.band) == i),
        ));
    }
    strata.push((
        "OpenATCA not coasting".to_string(),
        Box::new(|r: &Row| {
            r.val
                .first()
                .and_then(|v| v.as_ref())
                .is_some_and(|v| v.pt.coast == 0)
        }),
    ));
    strata.push((
        "OpenATCA coasting".to_string(),
        Box::new(|r: &Row| {
            r.val
                .first()
                .and_then(|v| v.as_ref())
                .is_some_and(|v| v.pt.coast != 0)
        }),
    ));
    write_groups(o, rows, n_ac, draws, k, &strata);
}

type Keep = Box<dyn Fn(&Row) -> bool>;

/// The flag groups of P-003: OpenATCA's coasting, uncertain and reacquired rows against the rest,
/// each pair on the same common set and resamples as the other tables.
fn flag_groups() -> Vec<(String, Keep)> {
    let oa = |r: &Row| r.val.first().and_then(|v| v.as_ref()).map(|v| v.pt);
    let mut g: Vec<(String, Keep)> = Vec::new();
    type Flag = fn(&Pt) -> Tri;
    let flags: [(&str, Flag); 3] = [
        ("coasting", |p| tri_of(p.coast != 0)),
        ("uncertain", |p| p.uncertain),
        ("reacquired", |p| p.reacquired),
    ];
    for (name, f) in flags {
        g.push((
            format!("OpenATCA not {name}"),
            Box::new(move |r: &Row| oa(r).is_some_and(|p| f(&p) == Tri::No)),
        ));
        g.push((
            format!("OpenATCA {name}"),
            Box::new(move |r: &Row| oa(r).is_some_and(|p| f(&p) == Tri::Yes)),
        ));
        // A sample with no tracker row to read the flag from is in neither group above.
        g.push((
            format!("OpenATCA {name} UNMATCHED"),
            Box::new(move |r: &Row| oa(r).is_some_and(|p| f(&p) == Tri::Unmatched)),
        ));
    }
    g
}

/// P-003 breakdown by the tracker's flags for one metric.
fn write_flag_groups(
    o: &mut String,
    title: &str,
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
) {
    let _ = writeln!(
        o,
        "{title}: breakdown by OpenATCA row flag (common set; groups by OpenATCA's flag on the grid sample; median with 95% interval, p95 and rms as points)"
    );
    write_groups(o, rows, n_ac, draws, k, &flag_groups());
}

fn write_groups(
    o: &mut String,
    rows: &[Row],
    n_ac: usize,
    draws: &[Vec<u32>],
    k: Kind,
    strata: &[(String, Keep)],
) {
    for (name, keep) in strata {
        let m = metric(rows, n_ac, draws, k, keep.as_ref());
        let n = m.n.first().copied().unwrap_or(0);
        if n == 0 {
            let _ = writeln!(o, "  {name:<28} n=0 (no common samples)");
            continue;
        }
        let _ = writeln!(o, "  {name:<28} n={n}");
        if m.nonfinite.iter().any(|&x| x > 0) {
            let _ = writeln!(o, "      {}", nonfinite_text(&m));
        }
        let _ = writeln!(
            o,
            "      median  OpenATCA {}  ERAM {}  Passthrough {}  OpenATCA minus ERAM {}",
            cell(&m, OA, 0, 2),
            cell(&m, ER, 0, 2),
            cell(&m, PT, 0, 2),
            diff_cell(&m, 0, 2)
        );
        for k2 in 1..3 {
            let _ = writeln!(
                o,
                "      {:<7} OpenATCA {:.2}  ERAM {:.2}  Passthrough {:.2}",
                STATS.get(k2).copied().unwrap_or(""),
                pv(&m, OA, k2),
                pv(&m, ER, k2),
                pv(&m, PT, k2)
            );
        }
    }
    let _ = writeln!(o);
}

// ---------------------------------------------------------------------------------------------
// The run.
// ---------------------------------------------------------------------------------------------

fn git_state() -> GitState {
    let dir = env!("CARGO_MANIFEST_DIR");
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    GitState {
        head: git(&["rev-parse", "HEAD"]).map_or("unknown".into(), |s| s.trim().to_string()),
        porcelain: git(&["status", "--porcelain"]),
    }
}

fn peak_rss_mb() -> f64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

fn fmt_opt(v: Option<f64>, prec: usize) -> String {
    v.filter(|x| x.is_finite())
        .map_or(String::new(), |x| format!("{x:.prec$}"))
}

fn run(
    clip_path: &Path,
    sfdps: &Path,
    out_dir: &Path,
    label: &str,
    git: &GitState,
    mode: Mode,
) -> Result<String, String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let t_start = Instant::now();

    // Tracker on set A (as vs_eram).
    let clip = load_adsb(clip_path).map_err(|e| e.to_string())?;
    let clip_hash = clip.hash.clone();
    let (set_a, set_b, split) = split_reports(&clip.reports);
    oa_bench::split_check::check_split(&set_a, &set_b)
        .map_err(|v| format!("split refused: {v:?}"))?;
    drop(clip);
    let (meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let spill = out_dir.join(format!("spill-{}", std::process::id()));
    let t_track = Instant::now();
    let run = run_tracker_spilled(&meas, &cfg, &spill).map_err(|e| e.to_string())?;
    let track_secs = t_track.elapsed().as_secs_f64();
    let n_meas = meas.len();
    let withheld = run.stats.rows_uncertain;
    drop(meas);
    eprintln!("tracker done in {track_secs:.1} s");

    // Reference from set B only.
    let tcfg = TRUTH_DEFAULT;
    let built = adsb_truth::build(&set_b, &tcfg).map_err(|e| format!("truth: {e}"))?;
    drop(set_b);
    let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
    let truth = Truth {
        query: &q,
        legs: &built.legs,
    };
    let mut rcfg = RunConfig::default();
    rcfg.pair.max_sigma_m = tcfg.max_sigma95_h_m;
    // frozen p002 protocol: scored without the hint-over-plan rule (OA-BENCH-132 came later)
    rcfg.assoc.hint_over_plan = false;
    let tix = truth_index(&truth);
    eprintln!("reference built");

    // OpenATCA pairs, one address range at a time (each aircraft is in one range).
    let mut oa_pts: PerAc = BTreeMap::new();
    let mut oa_drop = Dropped::default();
    let mut oa_ex = Exclusions::new();
    let mut oa_assoc = [0u64; astatus::COUNT];
    let mut oa_rows = 0u64;
    let (mut flag_dup_rows, mut flag_conflicts) = (0u64, 0u64);
    let counts = run.row_counts(RowSet::Ticks).map_err(|e| e.0)?;
    for range in score_ranges(&counts, SCORE_CHUNK_ROWS) {
        let r = run
            .rows_in(RowSet::Ticks, range.0, range.1)
            .map_err(|e| e.0)?;
        let legacy = legacy_from_track_samples_by_leg(
            &TrackSamples {
                addr: &r.addr,
                t_ns: &r.t_ns,
                lat_e7: &r.lat_e7,
                lon_e7: &r.lon_e7,
                alt_ft: &r.alt_ft,
                vel_east_mps: &r.vel_east_mps,
                vel_north_mps: &r.vel_north_mps,
                coast: &r.coast,
            },
            truth.legs,
        )
        .map_err(|e| format!("track samples: {e:?}"))?;
        oa_rows += r.addr.len() as u64;
        // The tracker's flags by (address, row time in ms), duplicates counted.
        let flags = flag_join(&r.addr, &r.t_ns, &r.uncertain, &r.reacquired)?;
        drop(r);
        flag_dup_rows += flags.duplicate_rows;
        flag_conflicts += flags.conflict_keys;
        let p = pair_legacy(&legacy, &truth, &tix, &rcfg);
        let first_new: BTreeSet<u32> = flags.map.keys().map(|k| k.0).collect();
        points_of(&p.pairs, &mut oa_pts, &mut oa_drop);
        for a in first_new {
            for pt in oa_pts.get_mut(&a).into_iter().flatten() {
                let (u, re) = flag_lookup(&flags, a, pt.t);
                pt.uncertain = u;
                pt.reacquired = re;
            }
        }
        for (k, v) in p.ex {
            *oa_ex.entry(k).or_default() += v;
        }
        for (a, b) in oa_assoc.iter_mut().zip(p.assoc_counts) {
            *a += b;
        }
    }
    drop(run);
    let _ = fs::remove_dir_all(&spill);
    eprintln!("OpenATCA paired aircraft: {}", oa_pts.len());
    // A hold-out score needs every sample's flags: an unmatched sample is not scored as if its
    // flags were "no" (E6, G6). Every point is counted, whichever way the join reached it.
    let flag_unmatched = unmatched_points(&oa_pts);
    flag_gate(mode, flag_unmatched, flag_dup_rows, flag_conflicts)?;

    // ERAM pairs.
    let loaded = load(&[FeedInput {
        feed: FeedKind::Sfdps,
        dir: sfdps.to_path_buf(),
    }])
    .map_err(|e| e.to_string())?;
    let sfdps_inputs: Vec<String> = loaded
        .inputs
        .iter()
        .map(|h| {
            format!(
                "{} files={} bytes={} sha256={}",
                h.label, h.files, h.bytes, h.sha256
            )
        })
        .collect();
    let ep = pair_legacy(&loaded.legacy, &truth, &tix, &rcfg);
    drop(loaded);
    let mut er_pts: PerAc = BTreeMap::new();
    let mut er_drop = Dropped::default();
    points_of(&ep.pairs, &mut er_pts, &mut er_drop);
    eprintln!("ERAM paired aircraft: {}", er_pts.len());

    // Passthrough: for each grid sample, the nearest set-A report of that aircraft within 0.5 s.
    let by_addr_time: Vec<(u32, i64, u32)> = {
        let mut v: Vec<(u32, i64, u32)> = set_a
            .icao
            .iter()
            .zip(&set_a.time_ns)
            .enumerate()
            .map(|(i, (&a, &t))| (a, t, i as u32))
            .collect();
        v.sort_unstable();
        v
    };
    let tol_ns = TOL_MS * 1_000_000;
    let mut chosen: BTreeSet<u32> = BTreeSet::new();
    for (&a, v) in &er_pts {
        for s in v {
            let t = s.t * 1_000_000;
            let i = by_addr_time.partition_point(|x| (x.0, x.1) < (a, t));
            let before = i
                .checked_sub(1)
                .and_then(|j| by_addr_time.get(j))
                .filter(|x| x.0 == a);
            let after = by_addr_time.get(i).filter(|x| x.0 == a);
            let pick = match (before, after) {
                (Some(b), Some(n)) => Some(if t - b.1 <= n.1 - t { b } else { n }),
                (Some(b), None) => Some(b),
                (None, Some(n)) => Some(n),
                (None, None) => None,
            };
            if let Some(p) = pick.filter(|p| (p.1 - t).abs() <= tol_ns) {
                chosen.insert(p.2);
            }
        }
    }
    drop(by_addr_time);
    let mut order: Vec<(u32, i64, u32)> = chosen
        .iter()
        .filter_map(|&i| {
            Some((
                *set_a.icao.get(i as usize)?,
                *set_a.time_ns.get(i as usize)?,
                i,
            ))
        })
        .collect();
    order.sort_unstable();
    let (mut c_addr, mut c_t, mut c_la, mut c_lo, mut c_alt) =
        (vec![], vec![], vec![], vec![], vec![]);
    let (mut c_ve, mut c_vn, mut c_coast) = (vec![], vec![], vec![]);
    for &(a, t, i) in &order {
        let i = i as usize;
        let (Some(&la), Some(&lo)) = (set_a.lat_e7.get(i), set_a.lon_e7.get(i)) else {
            continue;
        };
        c_addr.push(a);
        c_t.push(t);
        c_la.push(la);
        c_lo.push(lo);
        let alt = set_a.baro_alt_ft.get(i).copied().unwrap_or(NONE_I32);
        c_alt.push(if alt == NONE_I32 { NO_ALT } else { alt });
        let (gs, tr) = (
            set_a.gs_dkt.get(i).copied().unwrap_or(NONE_U16),
            set_a.track_ddeg.get(i).copied().unwrap_or(NONE_U16),
        );
        if gs != NONE_U16 && tr != NONE_U16 {
            let sp = f64::from(gs) * 0.1 * KT_MPS;
            let ang = (f64::from(tr) * 0.1).to_radians();
            c_ve.push((sp * ang.sin()) as f32);
            c_vn.push((sp * ang.cos()) as f32);
        } else {
            c_ve.push(f32::NAN);
            c_vn.push(f32::NAN);
        }
        c_coast.push(false);
    }
    let set_a_reports = reports_len(&set_a);
    drop(set_a);
    let pt_legacy = legacy_from_track_samples_by_leg(
        &TrackSamples {
            addr: &c_addr,
            t_ns: &c_t,
            lat_e7: &c_la,
            lon_e7: &c_lo,
            alt_ft: &c_alt,
            vel_east_mps: &c_ve,
            vel_north_mps: &c_vn,
            coast: &c_coast,
        },
        truth.legs,
    )
    .map_err(|e| format!("passthrough samples: {e:?}"))?;
    let pp = pair_legacy(&pt_legacy, &truth, &tix, &rcfg);
    let mut pt_pts: PerAc = BTreeMap::new();
    let mut pt_drop = Dropped::default();
    points_of(&pp.pairs, &mut pt_pts, &mut pt_drop);
    eprintln!("passthrough paired aircraft: {}", pt_pts.len());

    // The grid.
    let mut rows: Vec<Row> = Vec::new();
    let mut ac_addr: Vec<u32> = Vec::new();
    // Misses per system: [aircraft has no paired samples, no paired sample within 0.5 s].
    let mut miss = [[0u64; 2]; 3];
    for (&addr, ev) in &er_pts {
        let ac = ac_addr.len();
        ac_addr.push(addr);
        let others = [oa_pts.get(&addr), None, pt_pts.get(&addr)];
        for s in ev {
            let mut val: [Option<Val>; 3] = [None; 3];
            for (k, slot) in val.iter_mut().enumerate() {
                let p = if k == ER {
                    Some(*s)
                } else {
                    match others.get(k).copied().flatten() {
                        None => {
                            if let Some(m) = miss.get_mut(k).and_then(|m| m.get_mut(0)) {
                                *m += 1;
                            }
                            None
                        }
                        Some(v) => {
                            let n = nearest(v, s.t).copied();
                            if n.is_none() {
                                if let Some(m) = miss.get_mut(k).and_then(|m| m.get_mut(1)) {
                                    *m += 1;
                                }
                            }
                            n
                        }
                    }
                };
                *slot = p.map(|pt| Val {
                    pt,
                    along_fit: pt.along,
                    h_raw: pt.cross.hypot(pt.along),
                    h_fit: pt.cross.hypot(pt.along),
                    along_tol: along_tol(pt.along, pt.v),
                    h_tol: pt.cross.hypot(along_tol(pt.along, pt.v)),
                });
            }
            rows.push(Row {
                ac,
                addr,
                t: s.t,
                phase: s.phase,
                band: s.band,
                val,
            });
        }
    }
    let n_ac = ac_addr.len();
    let grid = rows.len();
    let n_common = rows.iter().filter(|r| r.common()).count();

    // Per-aircraft, per-system time offsets, fitted on the aircraft's common samples and applied
    // to all of its samples.
    let mut taus: Vec<[Option<f64>; 3]> = vec![[None; 3]; n_ac];
    for (ac, tau) in taus.iter_mut().enumerate() {
        for (s, slot) in tau.iter_mut().enumerate() {
            *slot = fit_tau(
                of_ac(&rows, ac)
                    .iter()
                    .filter(|r| r.common())
                    .filter_map(|r| r.val.get(s).and_then(|v| v.as_ref()))
                    .map(|v| (v.pt.along, v.pt.v)),
            );
        }
    }
    let unfitted = unfitted_counts(&rows, &taus);
    for r in &mut rows {
        let tau = taus.get(r.ac).copied().unwrap_or([None; 3]);
        for (s, v) in r.val.iter_mut().enumerate() {
            if let Some(v) = v {
                // P-003 resolution 1: an aircraft with no usable samples for a system's fit is
                // scored unfitted (offset 0), and counted in `unfitted` and printed below.
                let t = tau.get(s).copied().flatten().unwrap_or(UNFITTED_TAU_S);
                v.along_fit = v.pt.along - t * v.pt.v;
                v.h_fit = v.pt.cross.hypot(v.along_fit);
                v.along_tol = along_tol(v.along_fit, v.pt.v);
                v.h_tol = v.pt.cross.hypot(v.along_tol);
            }
        }
    }
    eprintln!("grid {grid} samples, {n_common} common, {n_ac} aircraft");

    // Bootstrap draws and metrics.
    let draws = draws_of(n_ac);
    let all = |_: &Row| true;
    let m1 = metric(&rows, n_ac, &draws, Kind::Cross, &all);
    let m2r = metric(&rows, n_ac, &draws, Kind::AlongRaw, &all);
    let m2f = metric(&rows, n_ac, &draws, Kind::AlongFit, &all);
    let m3 = metric(&rows, n_ac, &draws, Kind::HFit, &all);
    let m2t = metric(&rows, n_ac, &draws, Kind::AlongTol, &all);
    let m3t = metric(&rows, n_ac, &draws, Kind::HTol, &all);
    let m5s = metric(&rows, n_ac, &draws, Kind::Speed, &all);
    let m5t = metric(&rows, n_ac, &draws, Kind::Track, &all);
    eprintln!("main metrics bootstrapped");

    let mut o = String::new();
    if git_is_dirty(git) {
        let _ = writeln!(o, "{DIRTY_STAMP}");
    }
    if mode.development {
        let _ = writeln!(o, "{DEV_STAMP}");
    }
    let _ = writeln!(
        o,
        "P-002 scoring with the P-003 amendment: OpenATCA, ERAM, passthrough baseline"
    );
    if !label.is_empty() {
        let _ = writeln!(o, "  label: {label}");
    }
    let _ = writeln!(
        o,
        "  git commit: {} (working tree: {})",
        git.head,
        git_tree_text(git)
    );
    let _ = writeln!(
        o,
        "  binary built from: {} (build tree clean: {})",
        option_env!("OA_BUILD_COMMIT").unwrap_or("unknown"),
        option_env!("OA_BUILD_DIRTY").map_or("unknown", |d| if d == "0" { "yes" } else { "no" })
    );
    let _ = writeln!(
        o,
        "  P-003: M2t = sign(a) * max(0, |a| - {ROUND_TOL_S} s * v) of the time-fitted along-track error a and the reference ground speed v; M3t = hypot(cross, M2t); all three systems alike; headline position metrics are M1 and M3t (no 'better than ERAM' claim may rest on M2 or M3); M4 and M7 are also given on M3t"
    );
    let _ = writeln!(
        o,
        "  ADS-B clip: {} sha256={} (files={}, bytes={})",
        clip_path.display(),
        clip_hash.sha256,
        clip_hash.files,
        clip_hash.bytes
    );
    let _ = writeln!(o, "  SFDPS dir: {}", sfdps.display());
    for l in &sfdps_inputs {
        let _ = writeln!(o, "    {l}");
    }
    let _ = writeln!(o, "  split: {SPLIT_TEXT}");
    let _ = writeln!(
        o,
        "  set A {} reports ({} after counting), set B {} reports -> reference ({} legs); tracker measurements {n_meas}",
        split.set_a, set_a_reports, split.set_b, built.legs.len()
    );
    let _ = writeln!(
        o,
        "  bootstrap: {BOOT_N} resamples of whole aircraft, splitmix64 seed {BOOT_SEED}, 95% interval = ranks 25 and 975 of the sorted resampled values; every table uses the same resamples"
    );
    let _ = writeln!(
        o,
        "  statistics: median and p95 are nearest-rank over absolute values, rms over all; cross-track, along-track, horizontal as oa-bench defines them (truth frame); ground-speed error in m/s (system minus truth)"
    );
    let _ = writeln!(
        o,
        "  grid: {grid} samples on {n_ac} aircraft (ERAM sample times, primary flight per aircraft, finite cross/along); common set (all three systems have a value): {n_common}"
    );
    let _ = writeln!(
        o,
        "  inputs: OpenATCA used ADS-B only (set A); ERAM fuses radar and ADS-B; the reference comes from the same ADS-B feed (set B); passthrough is set A as reported"
    );
    let _ = writeln!(o);

    // Coverage and exclusions.
    let _ = writeln!(o, "COVERAGE AND EXCLUSIONS (per system and reason)");
    let _ = writeln!(
        o,
        "  OpenATCA output rows scored: {oa_rows}; rows flagged uncertain by the tracker (95% radius over 926 m), all scored: {withheld}"
    );
    let _ = writeln!(
        o,
        "  passthrough rows paired (one per distinct nearest set-A report of the grid): {}",
        c_addr.len()
    );
    for (s, sys) in SYS.iter().enumerate() {
        let valued = rows
            .iter()
            .filter(|r| r.val.get(s).is_some_and(Option::is_some))
            .count();
        let _ = writeln!(
            o,
            "  {sys}: grid samples with a value {valued} of {grid}; misses {}: [{}] aircraft with no paired samples at all, [{}] no paired sample within 0.5 s of the grid time",
            grid - valued,
            miss.get(s).map_or(0, |m| m[0]),
            miss.get(s).map_or(0, |m| m[1]),
        );
    }
    for (sys, (n_ac_unfit, n_samples)) in SYS.iter().zip(unfitted) {
        let _ = writeln!(
            o,
            "  {sys}: aircraft with no usable time fit (no common sample at {FIT_MIN_SPEED_MPS} m/s or more): {n_ac_unfit}, with {n_samples} samples, scored UNFITTED (offset 0 s) in M2, M3, M2t, M3t, M4, M6 and M7 per P-003 resolution 1"
        );
    }
    let _ = writeln!(
        o,
        "  OpenATCA grid samples that are coasting rows (scored, broken out): {}",
        rows.iter()
            .filter(|r| r
                .val
                .first()
                .and_then(|v| v.as_ref())
                .is_some_and(|v| v.pt.coast != 0))
            .count()
    );
    let sets: [(&str, &Paired, &Dropped); 3] = [
        (
            "OpenATCA",
            &Paired {
                pairs: PairTable::default(),
                ex: oa_ex,
                assoc_counts: oa_assoc,
            },
            &oa_drop,
        ),
        ("ERAM", &ep, &er_drop),
        ("Passthrough", &pp, &pt_drop),
    ];
    for (name, p, d) in sets {
        let _ = writeln!(o, "  {name} flights by association status (the 'no truth' statuses are 'no ADS-B reference for the aircraft'; the implausible statuses are 'association flagged'):");
        for (n, c) in astatus::NAMES.iter().zip(p.assoc_counts) {
            if c > 0 {
                let _ = writeln!(o, "      {n}: {c} flights");
            }
        }
        let _ = writeln!(
            o,
            "  {name} pairing exclusions (samples, by scope and reason):"
        );
        for ((scope, why), c) in &p.ex {
            let _ = writeln!(o, "      {scope}: {why}: {c}");
        }
        let _ = writeln!(
            o,
            "  {name} paired samples left out of the grid: {} on a non-primary flight, {} without finite cross/along (truth nearly stationary), {} at a time already used",
            d.other_flight, d.no_cross_along, d.duplicate_time
        );
    }
    let _ = writeln!(o);

    // Offsets.
    let _ = writeln!(
        o,
        "TIME OFFSETS (per aircraft least squares of along = tau * v on the aircraft's common samples with truth speed >= {FIT_MIN_SPEED_MPS} m/s, applied to every sample of the aircraft; tau > 0: the system is ahead of the reference)"
    );
    for (s, sys) in SYS.iter().enumerate() {
        let t: Vec<f64> = taus
            .iter()
            .filter_map(|t| t.get(s).copied().flatten())
            .collect();
        let nofit = n_ac - t.len();
        let mean = t.iter().sum::<f64>() / t.len().max(1) as f64;
        let _ = writeln!(
            o,
            "  {sys}: aircraft fitted {} ({} left unfitted, raw used); tau s: median {:.3}, p5 {:.3}, p25 {:.3}, p75 {:.3}, p95 {:.3}, min {:.3}, max {:.3}, mean {:.3}; spread (IQR) {:.3}",
            t.len(),
            nofit,
            signed_rank(&t, 50),
            signed_rank(&t, 5),
            signed_rank(&t, 25),
            signed_rank(&t, 75),
            signed_rank(&t, 95),
            t.iter().copied().fold(f64::INFINITY, f64::min),
            t.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            mean,
            signed_rank(&t, 75) - signed_rank(&t, 25)
        );
    }
    let _ = writeln!(o);

    // Signed raw errors, a sanity view (the bench reports ERAM's along-track mean near +100 m).
    let _ = writeln!(
        o,
        "SIGNED RAW ERRORS (m; signed mean and median of the unfitted errors; along > 0: ahead of the reference)"
    );
    for (s, sys) in SYS.iter().enumerate() {
        for (set, only_common) in [("all valued grid samples", false), ("common set", true)] {
            let sel = |f: &dyn Fn(&Val) -> f64| -> Vec<f64> {
                rows.iter()
                    .filter(|r| !only_common || r.common())
                    .filter_map(|r| r.val.get(s).and_then(|v| v.as_ref()))
                    .map(f)
                    .collect()
            };
            let (c, a) = (sel(&|v| v.pt.cross), sel(&|v| v.pt.along));
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
            let _ = writeln!(
                o,
                "  {sys}, {set} (n={}): cross mean {:.2} median {:.2}; along mean {:.2} median {:.2}",
                a.len(),
                mean(&c),
                signed_rank(&c, 50),
                mean(&a),
                signed_rank(&a, 50)
            );
        }
    }
    let _ = writeln!(o);

    // M1 to M3, M5.
    write_metric(&mut o, "M1 cross-track error", "m", &m1);
    write_metric(&mut o, "M2 along-track error, raw", "m", &m2r);
    write_metric(&mut o, "M2 along-track error, time-fitted", "m", &m2f);
    write_metric(&mut o, "M3 horizontal error after time fit", "m", &m3);
    write_metric(
        &mut o,
        "M2t along-track error, rounding-tolerant (P-003)",
        "m",
        &m2t,
    );
    write_metric(
        &mut o,
        "M3t horizontal error, rounding-tolerant (P-003)",
        "m",
        &m3t,
    );
    write_metric(&mut o, "M5 ground-speed error", "m/s", &m5s);
    write_metric(&mut o, "M5 track-angle error", "deg", &m5t);

    // M4.
    for (what, h) in [
        (
            "time-fitted horizontal error (M3)",
            (|v: &Val| v.h_fit) as fn(&Val) -> f64,
        ),
        (
            "rounding-tolerant horizontal error (M3t, P-003)",
            |v: &Val| v.h_tol,
        ),
    ] {
        let _ = writeln!(
            o,
            "M4{} coverage-penalised share of ALL {grid} grid samples with {what} within threshold (a miss is a failure)",
            if what.contains("M3t") { "t" } else { "" }
        );
        for (name, thr) in [("within 100 m", 100.0), ("within 300 m", 300.0)] {
            let (c, nan) = m4_counts(&rows, n_ac, h, thr);
            let _ = writeln!(
                o,
                "  {name} (present values not a number, counted as failures: OpenATCA {}, ERAM {}, Passthrough {}):",
                nan[0], nan[1], nan[2]
            );
            write_share(&mut o, &c, &draws, 1.0);
        }
        let _ = writeln!(o);
    }

    // M6.
    let m6r = m6_counts(&rows, n_ac);
    let (win, lose, tie) = (m6r.win, m6r.lose, m6r.tie);
    let _ = writeln!(
        o,
        "M6 per-aircraft: share of aircraft (with common samples) where OpenATCA's M3 RMS is lower than ERAM's"
    );
    let _ = writeln!(
        o,
        "  OpenATCA lower {win}, ERAM lower {lose}, tied {tie}; aircraft compared {}; aircraft with an RMS that is not a number (a failure for the share, left out of the counts and the sign test): OpenATCA {}, ERAM {}",
        win + lose + tie,
        m6r.nan[0],
        m6r.nan[1]
    );
    write_share(&mut o, &[m6r.share], &draws, 1.0);
    let _ = writeln!(
        o,
        "  sign test (exact binomial, p = 1/2, ties dropped, two-sided): p = {:.3e} on {win} of {}",
        sign_test_p(win, win + lose),
        win + lose
    );
    let _ = writeln!(
        o,
        "  passthrough within 10%: not defined for M6 (it compares OpenATCA with ERAM)"
    );
    let _ = writeln!(o);

    // M7.
    let _ = writeln!(
        o,
        "M7 large errors: samples over {LARGE_M} m (time-fitted horizontal) on ALL {grid} grid samples; a miss is counted as a failure"
    );
    let l7 = large_counts(&rows, n_ac, |v| v.h_fit);
    for (s, sys) in SYS.iter().enumerate() {
        let (f, _) = l7.cnt.get(s).map_or((0, 0), Cnt::point);
        let misses = grid
            - rows
                .iter()
                .filter(|r| r.val.get(s).is_some_and(Option::is_some))
                .count();
        let _ = writeln!(
            o,
            "  {sys}: failures {f} = {} over {LARGE_M} m or not a number (of which {} not a number) + {misses} misses (raw, unfitted over {LARGE_M} m: {})",
            l7.over.get(s).copied().unwrap_or(0),
            l7.nan.get(s).copied().unwrap_or(0),
            l7.raw_over.get(s).copied().unwrap_or(0)
        );
    }
    write_share(&mut o, &l7.cnt, &draws, 10_000.0);
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "M7t large errors on the rounding-tolerant horizontal error (M3t, P-003): samples over {LARGE_M} m on ALL {grid} grid samples; a miss is counted as a failure"
    );
    let l7t = large_counts(&rows, n_ac, |v| v.h_tol);
    for (s, sys) in SYS.iter().enumerate() {
        let (f, _) = l7t.cnt.get(s).map_or((0, 0), Cnt::point);
        let _ = writeln!(
            o,
            "  {sys}: failures {f} (over {LARGE_M} m, or not a number: {}, or missed)",
            l7t.nan.get(s).copied().unwrap_or(0)
        );
    }
    write_share(&mut o, &l7t.cnt, &draws, 10_000.0);
    let _ = writeln!(o);

    // Breakdowns.
    write_strata(
        &mut o,
        "M1 cross-track error (m)",
        &rows,
        n_ac,
        &draws,
        Kind::Cross,
    );
    write_strata(
        &mut o,
        "M3 horizontal error after time fit (m)",
        &rows,
        n_ac,
        &draws,
        Kind::HFit,
    );
    write_strata(
        &mut o,
        "M5 ground-speed error (m/s)",
        &rows,
        n_ac,
        &draws,
        Kind::Speed,
    );
    write_strata(
        &mut o,
        "M5 track-angle error (deg)",
        &rows,
        n_ac,
        &draws,
        Kind::Track,
    );

    // P-003 flag groups (P-002 item 9: the low-confidence breakdown is the uncertain group).
    write_flag_groups(
        &mut o,
        "M1 cross-track error (m)",
        &rows,
        n_ac,
        &draws,
        Kind::Cross,
    );
    write_flag_groups(
        &mut o,
        "M3t rounding-tolerant horizontal error (m)",
        &rows,
        n_ac,
        &draws,
        Kind::HTol,
    );
    write_flag_groups(
        &mut o,
        "M5 ground-speed error (m/s)",
        &rows,
        n_ac,
        &draws,
        Kind::Speed,
    );
    write_flag_groups(
        &mut o,
        "M5 track-angle error (deg)",
        &rows,
        n_ac,
        &draws,
        Kind::Track,
    );

    let secs = t_start.elapsed().as_secs_f64();
    let _ = writeln!(
        o,
        "RUN: tracker {track_secs:.1} s; total {secs:.1} s; peak resident memory {:.0} MB",
        peak_rss_mb()
    );
    let _ = writeln!(
        o,
        "NOTE: OpenATCA's uncertain and reacquired flags are joined to the paired samples by address and row time in whole milliseconds (the finest time the pair table carries; tracker times are floored to the millisecond); paired OpenATCA samples with no tracker row to read the flags from (or only rows that disagree), shown as the UNMATCHED groups and as `unmatched` in samples.csv: {flag_unmatched}; tracker rows sharing a key with an earlier row: {flag_dup_rows}; keys whose rows disagree: {flag_conflicts}; run is a {} run",
        if mode.holdout { "hold-out" } else { "non-hold-out" }
    );

    // samples.csv.
    let mut csv = String::new();
    if git_is_dirty(git) {
        let _ = writeln!(csv, "# {DIRTY_STAMP}");
    }
    if mode.development {
        let _ = writeln!(csv, "# {DEV_STAMP}");
    }
    csv.push_str("addr,t_ms,phase,band,common");
    for p in ["oa", "eram", "pt"] {
        let _ = write!(
            csv,
            ",{p}_miss,{p}_cross_m,{p}_along_raw_m,{p}_along_fit_m,{p}_h_fit_m,{p}_along_tol_m,{p}_h_tol_m,{p}_speed_err_mps,{p}_track_err_deg,{p}_row_dt_ms"
        );
    }
    csv.push_str(",oa_coast,oa_uncertain,oa_reacquired\n");
    for r in &rows {
        let _ = write!(
            csv,
            "{:06x},{},{},{},{}",
            r.addr,
            r.t,
            phase::NAMES
                .get(usize::from(r.phase))
                .copied()
                .unwrap_or(""),
            band::NAMES.get(usize::from(r.band)).copied().unwrap_or(""),
            u8::from(r.common())
        );
        for v in &r.val {
            match v {
                None => csv.push_str(",1,,,,,,,,,"),
                Some(v) => {
                    let _ = write!(
                        csv,
                        ",0,{},{},{},{},{},{},{},{},{}",
                        fmt_opt(Some(v.pt.cross), 2),
                        fmt_opt(Some(v.pt.along), 2),
                        fmt_opt(Some(v.along_fit), 2),
                        fmt_opt(Some(v.h_fit), 2),
                        fmt_opt(Some(v.along_tol), 2),
                        fmt_opt(Some(v.h_tol), 2),
                        fmt_opt(Some(v.pt.spd), 3),
                        fmt_opt(Some(v.pt.hdg), 2),
                        v.pt.t - r.t
                    );
                }
            }
        }
        let oa = r.val.first().and_then(|v| v.as_ref()).map(|v| v.pt);
        let _ = writeln!(
            csv,
            ",{},{},{}",
            oa.map_or(String::new(), |p| p.coast.to_string()),
            oa.map_or(String::new(), |p| tri_text(p.uncertain)),
            oa.map_or(String::new(), |p| tri_text(p.reacquired))
        );
    }
    fs::write(out_dir.join("samples.csv"), &csv).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("p002.txt"), &o).map_err(|e| e.to_string())?;
    Ok(o)
}

/// Shares (scaled by `scale`) of the counts of OpenATCA, ERAM, passthrough with intervals, the
/// OpenATCA-minus-ERAM difference, and the passthrough rule. A single count prints as OpenATCA's.
fn write_share(o: &mut String, c: &[Cnt], draws: &[Vec<u32>], scale: f64) {
    let boots: Vec<Vec<f64>> = c.iter().map(|x| x.boot(draws)).collect();
    let pts: Vec<f64> = c
        .iter()
        .map(|x| {
            let (n, d) = x.point();
            if d == 0 {
                f64::NAN
            } else {
                n as f64 / d as f64 * scale
            }
        })
        .collect();
    let prec = if scale > 1.0 { 1 } else { 4 };
    for (s, (p, b)) in pts.iter().zip(&boots).enumerate() {
        let (lo, hi) = interval(b.iter().map(|v| v * scale));
        let name = if c.len() == 1 {
            "OpenATCA"
        } else {
            SYS.get(s).copied().unwrap_or("")
        };
        let (n, d) = c.get(s).map_or((0, 0), Cnt::point);
        let _ = writeln!(
            o,
            "    {name:<12} {} ({n} of {d})",
            fmt_ci(*p, lo, hi, prec)
        );
    }
    if let (Some(a), Some(b), Some(ba), Some(bb)) =
        (pts.first(), pts.get(1), boots.first(), boots.get(1))
    {
        if c.len() > 1 {
            let (lo, hi) = interval(ba.iter().zip(bb).map(|(x, y)| (x - y) * scale));
            let _ = writeln!(
                o,
                "    OpenATCA minus ERAM  {}",
                fmt_ci(a - b, lo, hi, prec)
            );
        }
    }
    if let (Some(a), Some(p), true) = (pts.first(), pts.get(PT), c.len() > 2) {
        let _ = writeln!(o, "    passthrough within 10%: {}", floor_note(*a, *p));
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_known_values() {
        let mut r = SplitMix(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn below_stays_in_range_and_is_deterministic() {
        let (mut a, mut b) = (SplitMix(BOOT_SEED), SplitMix(BOOT_SEED));
        for _ in 0..1000 {
            let x = a.below(7);
            assert!(x < 7);
            assert_eq!(x, b.below(7));
        }
    }

    #[test]
    fn sign_test_matches_exact_values() {
        assert!((sign_test_p(0, 10) - 2.0 / 1024.0).abs() < 1e-12);
        assert!((sign_test_p(5, 10) - 1.0).abs() < 1e-12);
        // 9 of 10: 2 * (1 + 10) / 1024.
        assert!((sign_test_p(9, 10) - 22.0 / 1024.0).abs() < 1e-12);
        assert!(sign_test_p(0, 0).is_nan());
    }

    #[test]
    fn stat3_uses_nearest_rank_and_rms() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = stat3(&mut v);
        assert_eq!(s[0], 50.0);
        assert_eq!(s[1], 95.0);
        assert!((s[2] - (338_350.0f64 / 100.0).sqrt()).abs() < 1e-9);
        assert!(stat3(&mut []).iter().all(|x| x.is_nan()));
    }

    #[test]
    fn along_tolerance_below_at_and_above_half_second_of_travel() {
        // v = 200 m/s: the tolerance is 100 m.
        assert_eq!(along_tol(60.0, 200.0), 0.0);
        assert_eq!(along_tol(100.0, 200.0), 0.0);
        assert_eq!(along_tol(130.0, 200.0), 30.0);
        assert_eq!(along_tol(0.0, 200.0), 0.0);
        // Zero speed charges nothing.
        assert_eq!(along_tol(7.5, 0.0), 7.5);
    }

    #[test]
    fn along_tolerance_keeps_the_sign() {
        assert_eq!(along_tol(-130.0, 200.0), -30.0);
        assert_eq!(along_tol(-100.0, 200.0), 0.0);
        assert_eq!(along_tol(-60.0, 200.0), 0.0);
        assert!(along_tol(f64::NAN, 200.0).is_nan());
        assert!(along_tol(10.0, f64::NAN).is_nan());
    }

    #[test]
    fn dirty_tree_is_refused_unless_porcelain_is_empty() {
        assert!(!tree_is_dirty(Some("")));
        assert!(!tree_is_dirty(Some("\n")));
        assert!(tree_is_dirty(Some(" M crates/oa-track/src/score.rs\n")));
        assert!(tree_is_dirty(Some("?? new.rs\n")));
        // No answer from git is not a clean tree.
        assert!(tree_is_dirty(None));
    }

    #[test]
    fn tau_recovers_a_constant_offset() {
        let pts: Vec<(f64, f64)> = [60.0, 120.0, 200.0].iter().map(|&v| (0.5 * v, v)).collect();
        assert!((fit_tau(pts.iter().copied()).unwrap() - 0.5).abs() < 1e-12);
        // Slow samples do not enter the fit.
        assert!(fit_tau([(10.0, 10.0)].into_iter()).is_none());
    }

    #[test]
    fn interval_picks_ranks_25_and_975() {
        let (lo, hi) = interval((1..=1000).map(f64::from));
        assert_eq!((lo, hi), (25.0, 975.0));
    }

    #[test]
    fn bootstrap_of_one_aircraft_is_its_own_statistic() {
        let col = Col {
            start: vec![0, 3],
            data: vec![1.0, 2.0, 3.0],
        };
        let draws = draws_of(1);
        assert_eq!(draws.len(), BOOT_N);
        let b = col.boot(&draws);
        assert!(b.iter().all(|x| *x == col.point()));
        assert_eq!(draws_of(5), draws_of(5));
    }

    #[test]
    fn nearest_respects_the_tolerance() {
        let p = |t| Pt {
            t,
            cross: 0.0,
            along: 0.0,
            v: 0.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: Tri::No,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        };
        let v = vec![p(1000), p(2000)];
        assert_eq!(nearest(&v, 1400).unwrap().t, 1000);
        assert_eq!(nearest(&v, 1500).unwrap().t, 1000);
        assert!(nearest(&v, 1501).is_some());
        assert!(nearest(&v, 3000).is_none());
    }

    // REQ: OA-TRK-373
    #[test]
    fn a_duplicate_flag_key_is_counted_and_never_overwritten() {
        let ns = |ms: i64| ms * 1_000_000;
        // Two rows in one millisecond that agree, then two that disagree, then a unique key.
        let j = flag_join(
            &[1, 1, 2, 2, 3],
            &[ns(5), ns(5) + 400_000, ns(7), ns(7) + 1, ns(9)],
            &[true, true, false, true, false],
            &[false, false, false, false, true],
        )
        .unwrap();
        assert_eq!((j.duplicate_rows, j.conflict_keys), (2, 1));
        assert_eq!(flag_lookup(&j, 1, 5), (Tri::Yes, Tri::No));
        // Disagreeing rows: neither wins; the sample is unmatched.
        assert_eq!(flag_lookup(&j, 2, 7), (Tri::Unmatched, Tri::Unmatched));
        assert_eq!(flag_lookup(&j, 3, 9), (Tri::No, Tri::Yes));
        // A key with no tracker row is unmatched, not "no".
        assert_eq!(flag_lookup(&j, 3, 10), (Tri::Unmatched, Tri::Unmatched));
        assert_eq!(flag_lookup(&j, 4, 9), (Tri::Unmatched, Tri::Unmatched));
        // A third row after a conflict does not count the key twice.
        let j = flag_join(
            &[1, 1, 1],
            &[ns(1), ns(1), ns(1)],
            &[true, false, true],
            &[false; 3],
        )
        .unwrap();
        assert_eq!((j.duplicate_rows, j.conflict_keys), (2, 1));
        // Columns of different lengths are refused.
        assert!(flag_join(&[1], &[1, 2], &[true], &[true]).is_err());
    }

    // REQ: OA-TRK-373
    #[test]
    fn an_unmatched_sample_is_in_its_own_group_and_not_in_no() {
        let pt = |u| Pt {
            t: 0,
            cross: 0.0,
            along: 0.0,
            v: 0.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: u,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        };
        let val = |u| Val {
            pt: pt(u),
            along_fit: 0.0,
            h_raw: 0.0,
            h_fit: 0.0,
            along_tol: 0.0,
            h_tol: 0.0,
        };
        let row = |u| Row {
            ac: 0,
            t: 0,
            addr: 1,
            phase: 0,
            band: 0,
            val: [Some(val(u)), None, None],
        };
        let groups = flag_groups();
        let find = |name: &str| groups.iter().find(|g| g.0 == name).unwrap();
        for (u, no, yes, un) in [
            (Tri::No, true, false, false),
            (Tri::Yes, false, true, false),
            (Tri::Unmatched, false, false, true),
        ] {
            let r = row(u);
            assert_eq!((find("OpenATCA not uncertain").1)(&r), no);
            assert_eq!((find("OpenATCA uncertain").1)(&r), yes);
            assert_eq!((find("OpenATCA uncertain UNMATCHED").1)(&r), un);
        }
    }

    // REQ: OA-TRK-374
    #[test]
    fn a_not_a_number_error_is_a_failure_in_the_large_error_metrics() {
        assert!(fails_over(f64::NAN, LARGE_M));
        assert!(fails_over(LARGE_M + 1.0, LARGE_M));
        assert!(fails_over(f64::INFINITY, LARGE_M));
        assert!(!fails_over(LARGE_M, LARGE_M));
        assert!(!fails_over(0.0, LARGE_M));
        // The same sample is a failure of the M4 test as well: `h <= thr` is false for NaN.
        assert!(f64::NAN.partial_cmp(&100.0).is_none());
        // A sample whose truth speed is not known has a not-a-number M3t and fails M7t.
        let h_tol = 5.0f64.hypot(along_tol(3.0, f64::NAN));
        assert!(h_tol.is_nan() && fails_over(h_tol, LARGE_M));
    }

    // REQ: OA-TRK-374
    #[test]
    fn aircraft_with_no_time_fit_are_counted_and_their_samples_are_named() {
        let pt = Pt {
            t: 0,
            cross: 1.0,
            along: 1.0,
            v: 100.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: Tri::No,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        };
        let val = Val {
            pt,
            along_fit: 1.0,
            h_raw: 1.0,
            h_fit: 1.0,
            along_tol: 1.0,
            h_tol: 1.0,
        };
        let row = |ac, vals: [Option<Val>; 3]| Row {
            ac,
            t: 0,
            addr: ac as u32,
            phase: 0,
            band: 0,
            val: vals,
        };
        let rows = vec![
            row(0, [Some(val), Some(val), None]),
            row(0, [Some(val), Some(val), None]),
            row(1, [Some(val), None, None]),
        ];
        // Aircraft 0 is fitted for OpenATCA only; aircraft 1 has no fit at all.
        let taus = vec![[Some(0.1), None, None], [None, None, None]];
        let u = unfitted_counts(&rows, &taus);
        assert_eq!(u[0], (1, 1)); // OpenATCA: aircraft 1, one sample
        assert_eq!(u[1], (1, 2)); // ERAM: aircraft 0, two samples
        assert_eq!(u[2], (0, 0)); // passthrough has no values: nothing to count
    }

    fn fx_pt() -> Pt {
        Pt {
            t: 0,
            cross: 1.0,
            along: 1.0,
            v: 100.0,
            spd: 0.0,
            hdg: 0.0,
            coast: 0,
            uncertain: Tri::No,
            reacquired: Tri::No,
            phase: 0,
            band: 0,
        }
    }

    fn fx_val(h: f64) -> Val {
        Val {
            pt: fx_pt(),
            along_fit: 0.0,
            h_raw: 1.0,
            h_fit: h,
            along_tol: 0.0,
            h_tol: h,
        }
    }

    fn fx_row(ac: usize, h: [Option<f64>; 3]) -> Row {
        Row {
            ac,
            t: 0,
            addr: ac as u32,
            phase: 0,
            band: 0,
            val: h.map(|x| x.map(fx_val)),
        }
    }

    // REQ: OA-TRK-374
    #[test]
    fn the_m4_and_m7_loops_count_a_nan_as_a_failure_and_print_the_count() {
        let rows = vec![
            fx_row(0, [Some(50.0), Some(f64::NAN), Some(500.0)]),
            fx_row(0, [Some(f64::NAN), Some(50.0), None]),
            fx_row(1, [Some(2000.0), Some(50.0), Some(50.0)]),
        ];
        let (c, nan) = m4_counts(&rows, 2, |v| v.h_fit, 100.0);
        assert_eq!(nan, [1, 1, 0]);
        // Within 100 m: OpenATCA 1 of 3 (50; NaN and 2000 fail), ERAM 2 of 3, passthrough 1 of 3.
        assert_eq!(c[0].point(), (1, 3));
        assert_eq!(c[1].point(), (2, 3));
        assert_eq!(c[2].point(), (1, 3));
        let l = large_counts(&rows, 2, |v| v.h_tol);
        assert_eq!(l.nan, [1, 1, 0]);
        assert_eq!(l.over, [2, 1, 0]);
        // Failures: over-or-NaN plus the miss of passthrough in row 2.
        assert_eq!(l.cnt[0].point(), (2, 3));
        assert_eq!(l.cnt[1].point(), (1, 3));
        assert_eq!(l.cnt[2].point(), (1, 3));
    }

    // REQ: OA-TRK-374
    #[test]
    fn m6_does_not_turn_a_nan_into_a_tie() {
        let rows = vec![
            fx_row(0, [Some(10.0), Some(20.0), Some(1.0)]),
            fx_row(1, [Some(f64::NAN), Some(20.0), Some(1.0)]),
            fx_row(2, [Some(30.0), Some(20.0), Some(1.0)]),
            fx_row(3, [Some(5.0), Some(f64::NAN), Some(1.0)]),
        ];
        let m = m6_counts(&rows, 4);
        assert_eq!((m.win, m.lose, m.tie), (1, 1, 0));
        assert_eq!(m.nan, [1, 1]);
        // The two NaN aircraft are failures of the share, in a denominator of four.
        assert_eq!(m.share.point(), (1, 4));
    }

    // REQ: OA-TRK-374
    #[test]
    fn a_non_finite_sample_is_counted_apart_from_not_common() {
        let mut rows = vec![
            fx_row(0, [Some(10.0), Some(20.0), Some(30.0)]),
            fx_row(0, [Some(f64::NAN), Some(20.0), Some(30.0)]),
            fx_row(0, [None, Some(20.0), Some(30.0)]),
        ];
        // Cross-track is finite everywhere; use the fitted horizontal error as the kind.
        let m = metric(&rows, 1, &draws_of(1), Kind::HFit, &|_| true);
        assert_eq!(m.n, [1, 1, 1]);
        assert_eq!(m.nonfinite, [1, 0, 0]);
        assert!(nonfinite_text(&m).contains("OpenATCA 1, ERAM 0"));
        rows.clear();
    }

    // REQ: OA-TRK-373
    #[test]
    fn the_hold_out_gate_refuses_any_unmatched_sample_and_counts_every_point() {
        let pts = |u| {
            let mut p = fx_pt();
            p.uncertain = u;
            p
        };
        let mut per: PerAc = BTreeMap::new();
        per.insert(1, vec![pts(Tri::No), pts(Tri::Yes)]);
        assert_eq!(unmatched_points(&per), 0);
        // An unmatched point on an address the join never saw is still counted.
        per.insert(9, vec![pts(Tri::Unmatched)]);
        assert_eq!(unmatched_points(&per), 1);
        let hold = Mode {
            holdout: true,
            development: false,
        };
        let dev = Mode {
            holdout: false,
            development: true,
        };
        assert!(flag_gate(hold, 0, 3, 1).is_ok());
        let e = flag_gate(hold, 1, 3, 1).unwrap_err();
        assert!(e.contains("hold-out run refused") && e.contains("1 OpenATCA samples"));
        assert!(flag_gate(dev, 5, 0, 0).is_ok());
    }

    // REQ: OA-TRK-377
    #[test]
    fn a_hold_out_run_needs_the_frozen_commit_a_clean_build_and_a_clean_tree_at_it() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let other = "fedcba9876543210fedcba9876543210fedcba98";
        let git = |head: &str, dirty: bool| GitState {
            head: head.into(),
            porcelain: Some(if dirty {
                " M x\n".into()
            } else {
                String::new()
            }),
        };
        let b = |c: Option<&'static str>, d: Option<&'static str>| Build {
            commit: c,
            dirty: d,
        };
        let hold = Mode {
            holdout: true,
            development: false,
        };
        let dev = Mode {
            holdout: false,
            development: true,
        };
        let good = b(Some(sha), Some("0"));
        assert!(holdout_binding(hold, Some(sha), good, &git(sha, false)).is_ok());
        // No --frozen-commit, a short or upper-case one.
        assert!(holdout_binding(hold, None, good, &git(sha, false)).is_err());
        assert!(holdout_binding(hold, Some("0123456"), good, &git(sha, false)).is_err());
        // Embedded sha differs from the argument.
        let e = holdout_binding(hold, Some(other), good, &git(other, false)).unwrap_err();
        assert!(e.contains("built from"), "{e}");
        // Dirty build, unknown build.
        let e = holdout_binding(hold, Some(sha), b(Some(sha), Some("1")), &git(sha, false))
            .unwrap_err();
        assert!(e.contains("not clean"), "{e}");
        assert!(holdout_binding(hold, Some(sha), b(Some(sha), None), &git(sha, false)).is_err());
        assert!(holdout_binding(hold, Some(sha), b(None, None), &git(sha, false)).is_err());
        // Run-time tree at another commit, or dirty.
        assert!(holdout_binding(hold, Some(sha), good, &git(other, false)).is_err());
        assert!(holdout_binding(hold, Some(sha), good, &git(sha, true)).is_err());
        // Development mode is unchanged: no argument, no build information needed.
        assert!(holdout_binding(dev, None, b(None, None), &git(other, true)).is_ok());
    }

    #[test]
    fn share_counts_resample_whole_aircraft() {
        let mut c = Cnt::new(2);
        c.add(0, true);
        c.add(0, true);
        c.add(1, false);
        assert_eq!(c.point(), (2, 3));
        let b = c.boot(&[vec![0, 0], vec![1, 1], vec![0, 1]]);
        assert_eq!(b[0], 1.0);
        assert_eq!(b[1], 0.0);
        assert!((b[2] - 2.0 / 3.0).abs() < 1e-12);
    }
}

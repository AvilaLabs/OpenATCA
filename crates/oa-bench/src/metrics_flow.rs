//! Metrics T5 to T9: publication, staleness, presence, initiation and association errors.
//! Core (pure): free functions over the legacy tables, the pair table and the truth interface.

use std::collections::BTreeMap;

use crate::assoc::Assoc;
use crate::geo::dist_m;
use crate::legacy::{name, Legacy, NONE, NO_TIME};
use crate::pair::PairTable;
use crate::stats::{
    band, group_rows, new_result, sort_cases, KeyedCase, MetricResult, Row, COAST_NAMES,
};
use crate::thindex::{samples_of, ThIndex};
use crate::truth::{Env, Truth};

/// Longest publication interval counted by T5, seconds. A longer silence is a flight that left
/// and re-entered coverage, not a publication interval; those are counted in a scalar.
pub const MAX_INTERVAL_S: f64 = 600.0;

/// T5: seconds between consecutive position times of one flight, over every flight (no truth
/// needed). Equal position times (repeats) are counted, not measured. The intervals are whole
/// milliseconds and are summed as integers ([`crate::chunked::T5Hists`]), the same way the
/// chunked scoring sums them, so the two agree to the last bit.
pub fn t5_publication_interval(leg: &Legacy, ix: &ThIndex) -> MetricResult {
    let mut h = crate::chunked::T5Hists::default();
    crate::chunked::t5_add(&mut h, leg, ix);
    crate::chunked::t5_result(&h)
}

/// T6: receive time minus position time, seconds (UK-004).
pub fn t6_staleness(leg: &Legacy) -> MetricResult {
    let mut vals = Vec::new();
    let (mut coast, mut bands) = (Vec::new(), Vec::new());
    let mut none = 0u64;
    for i in 0..leg.th.pos_ms.len() {
        let recv = leg.th.recv_ms.get(i).copied().unwrap_or(NO_TIME);
        if recv == NO_TIME {
            none += 1;
            continue;
        }
        let pos = leg.th.pos_ms.get(i).copied().unwrap_or(0);
        vals.push((recv - pos) as f64 / 1000.0);
        coast.push(leg.th.coast.get(i).copied().unwrap_or(0));
        bands.push(band::of(
            leg.th
                .alt_ft
                .get(i)
                .copied()
                .filter(|&a| a != crate::legacy::NO_ALT),
        ));
    }
    let mut r = new_result("T6", "staleness (receive time minus position time)", "s");
    if vals.is_empty() {
        r.withheld = Some(
            "the recordings carry no receive time (raw .msg files); use .oaf recordings".into(),
        );
    } else {
        r.rows = group_rows(
            &vals,
            &[
                ("altitude", &bands, &band::NAMES),
                ("coast", &coast, &COAST_NAMES),
            ],
        );
    }
    r.scalars
        .push(("samples_without_receive_time".into(), none as f64));
    r
}

/// Presence and initiation settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresenceConfig {
    /// Truth grid step, ms.
    pub grid_ms: i64,
    /// An ERAM position within this of a truth time counts as present, ms.
    pub window_ms: i64,
    /// A leg that begins inside the zone counts as a takeoff for T8 only if its first truth
    /// altitude is at or below this, feet.
    pub takeoff_alt_ft: i32,
}

impl Default for PresenceConfig {
    fn default() -> PresenceConfig {
        PresenceConfig {
            grid_ms: 5_000,
            window_ms: 30_000,
            takeoff_alt_ft: 2_000,
        }
    }
}

fn has_within(times: &[i64], t: i64, window: i64) -> bool {
    let k = times.partition_point(|&x| x < t - window);
    times.get(k).is_some_and(|&x| x <= t + window)
}

/// The sums T7 and T8 are made of, so legs can be taken in address ranges and added up (the
/// chunked scoring of a tracker's output). Cold record (A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresenceParts {
    pub n_in: u64,
    pub n_present: u64,
    pub n_unassoc: u64,
    /// `(index of the leg in the truth legs, delay in seconds)`, in leg order.
    pub delays: Vec<(usize, f64)>,
    pub started_inside: u64,
    pub never_zone: u64,
    pub never_tracked: u64,
    /// Entries whose first associated position is at or before the zone entry.
    pub established_before_entry: u64,
}

/// Adds `b` to `a`.
pub fn presence_add(a: &mut PresenceParts, b: PresenceParts) {
    a.n_in += b.n_in;
    a.n_present += b.n_present;
    a.n_unassoc += b.n_unassoc;
    a.delays.extend(b.delays);
    a.started_inside += b.started_inside;
    a.never_zone += b.never_zone;
    a.never_tracked += b.never_tracked;
    a.established_before_entry += b.established_before_entry;
}

/// The T7 and T8 sums over the truth legs whose address is in `lo..=hi`, against the flights of
/// `leg` associated with truth aircraft (their times are all the legs of the range can see, so a
/// range must hold every flight associated with an address in it).
pub fn presence_parts(
    leg: &Legacy,
    ix: &ThIndex,
    assoc: &Assoc,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &PresenceConfig,
    range: (u32, u32),
) -> PresenceParts {
    let mut times: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
    for f in 0..leg.flights.gufi.len() {
        let a = assoc.addr.get(f).copied().unwrap_or(NONE);
        if a == NONE {
            continue;
        }
        let v = times.entry(a).or_default();
        for &s in samples_of(ix, f as u32) {
            v.push(leg.th.pos_ms.get(s as usize).copied().unwrap_or(0));
        }
    }
    for v in times.values_mut() {
        v.sort_unstable();
    }
    let mut p = PresenceParts::default();
    for (li, l) in truth.legs.iter().enumerate() {
        if l.addr < range.0 || l.addr > range.1 {
            continue;
        }
        let ts = times.get(&l.addr);
        let mut t_in: Option<(i64, bool)> = None;
        let mut t = l.start_ms;
        while t <= l.end_ms {
            if let Ok(s) = (truth.query)(l.addr, t) {
                if (env.in_zone)(s.lat, s.lon) {
                    p.n_in += 1;
                    match ts {
                        None => p.n_unassoc += 1,
                        Some(v) => {
                            if has_within(v, t, cfg.window_ms) {
                                p.n_present += 1;
                            }
                        }
                    }
                    if t_in.is_none() {
                        let low = s.baro_alt_ft.is_some_and(|a| a <= cfg.takeoff_alt_ft);
                        t_in = Some((t, t == l.start_ms && !low));
                    }
                }
            }
            t += cfg.grid_ms.max(1);
        }
        match t_in {
            None => p.never_zone += 1,
            Some((_, true)) => p.started_inside += 1,
            Some((tin, false)) => {
                let first = ts.and_then(|v| {
                    let k = v.partition_point(|&x| x < l.start_ms);
                    v.get(k).copied().filter(|&x| x <= l.end_ms)
                });
                match first {
                    Some(f) if f <= tin => p.established_before_entry += 1,
                    Some(f) => p.delays.push((li, (f - tin) as f64 / 1000.0)),
                    None => p.never_tracked += 1,
                }
            }
        }
    }
    p
}

/// T7 and T8 from the sums (the delays in truth-leg order).
pub fn presence_results(
    mut p: PresenceParts,
    cfg: &PresenceConfig,
) -> (MetricResult, MetricResult) {
    p.delays.sort_by_key(|d| d.0);
    let delays: Vec<f64> = p.delays.iter().map(|d| d.1).collect();
    let mut r7 = new_result("T7", "presence in the zone", "share");
    r7.scalars
        .push(("truth_grid_points_in_zone".into(), p.n_in as f64));
    r7.scalars
        .push(("with_eram_position".into(), p.n_present as f64));
    r7.scalars.push((
        "presence".into(),
        if p.n_in == 0 {
            f64::NAN
        } else {
            p.n_present as f64 / p.n_in as f64
        },
    ));
    r7.scalars.push((
        "points_of_legs_with_no_associated_flight".into(),
        p.n_unassoc as f64,
    ));
    r7.notes.push(format!(
        "grid {} s, window {} s",
        cfg.grid_ms / 1000,
        cfg.window_ms / 1000
    ));
    let mut r8 = new_result(
        "T8",
        "initiation delay (first ERAM position minus zone entry)",
        "s",
    );
    r8.rows = vec![Row {
        label: "all".into(),
        stats: crate::stats::stats_of(&delays),
    }];
    r8.scalars.push((
        "legs_starting_inside_zone_not_a_takeoff".into(),
        p.started_inside as f64,
    ));
    r8.scalars
        .push(("legs_never_in_zone".into(), p.never_zone as f64));
    r8.scalars
        .push(("entries_never_tracked".into(), p.never_tracked as f64));
    r8.scalars.push((
        "established_before_entry".into(),
        p.established_before_entry as f64,
    ));
    (r7, r8)
}

/// T7 presence and T8 initiation delay. T7: over a grid on every truth leg, the share of truth
/// flight time inside the zone for which an ERAM position of the associated flight lies within
/// the window; a leg with no associated flight counts as absent. T8: time of the first
/// associated ERAM position of the leg minus the truth entry into the zone (or takeoff), seconds.
/// A value of zero or less means the track was established before entry: counted as
/// "established before entry", not in the delay distribution.
pub fn t7_t8_presence(
    leg: &Legacy,
    ix: &ThIndex,
    assoc: &Assoc,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &PresenceConfig,
) -> (MetricResult, MetricResult) {
    let p = presence_parts(leg, ix, assoc, truth, env, cfg, (0, u32::MAX));
    presence_results(p, cfg)
}

/// T9 settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwapConfig {
    /// Every `stride`-th paired sample of a flight is tested.
    pub stride: usize,
    /// Consecutive tested samples that make an association error.
    pub min_run: usize,
    /// A sample can only be a mismatch if its own error exceeds this, metres.
    pub min_own_error_m: f64,
}

impl Default for SwapConfig {
    fn default() -> SwapConfig {
        SwapConfig {
            stride: 4,
            min_run: 3,
            min_own_error_m: 300.0,
        }
    }
}

/// T9: association errors. A tested sample is a mismatch when the ERAM position is closer to
/// another aircraft's truth than to its own truth (and its own error exceeds
/// [`SwapConfig::min_own_error_m`]); `min_run` consecutive mismatches among a flight's tested
/// samples are one association error, listed as a case.
pub fn t9_association_errors(
    leg: &Legacy,
    p: &PairTable,
    truth: &Truth<'_>,
    cfg: &SwapConfig,
) -> MetricResult {
    let (mut r, mut cases) = t9_keyed(leg, p, truth, cfg);
    sort_cases(&mut cases);
    r.cases = cases.into_iter().map(|c| c.1).collect();
    r
}

/// [`t9_association_errors`] with its case lines unsorted and uncapped, each with its place in
/// the list (truth address, then time of the first mismatch), and none in the result. The
/// chunked scoring merges the lines of its ranges and sorts them once.
pub fn t9_keyed(
    leg: &Legacy,
    p: &PairTable,
    truth: &Truth<'_>,
    cfg: &SwapConfig,
) -> (MetricResult, Vec<KeyedCase>) {
    let mut cases: Vec<KeyedCase> = Vec::new();
    let mut r = new_result("T9", "association errors (track swaps)", "count");
    let (mut tested, mut mism, mut events, mut flights) = (0u64, 0u64, 0u64, 0u64);
    let mut i = 0usize;
    let n = p.t_ms.len();
    while i < n {
        let f = p.flight.get(i).copied().unwrap_or(0);
        let mut j = i;
        while j < n && p.flight.get(j).copied() == Some(f) {
            j += 1;
        }
        flights += 1;
        let mut run: Vec<(i64, u32, f64)> = Vec::new();
        let flush = |run: &mut Vec<(i64, u32, f64)>,
                     events: &mut u64,
                     cases: &mut Vec<KeyedCase>| {
            if run.len() >= cfg.min_run {
                *events += 1;
                let cs = leg.flights.callsign.get(f as usize).map_or("", |c| name(c));
                let (t0, other, d) = run.first().copied().unwrap_or((0, 0, 0.0));
                let own = p.addr.get(i).copied().unwrap_or(0);
                cases.push((
                    (0, own, t0),
                    format!(
                        "flight {cs} own {own:06X} follows {other:06X} from {t0} ms for {} samples, {d:.0} m from the other",
                        run.len()
                    ),
                ));
            }
            run.clear();
        };
        for k in (i..j).step_by(cfg.stride.max(1)) {
            let (Some(&t), Some(&own), Some(&th)) = (p.t_ms.get(k), p.addr.get(k), p.th.get(k))
            else {
                continue;
            };
            let (lat, lon) = (
                f64::from(leg.th.lat_e7.get(th as usize).copied().unwrap_or(0)) * 1e-7,
                f64::from(leg.th.lon_e7.get(th as usize).copied().unwrap_or(0)) * 1e-7,
            );
            let d_own = match (
                p.cross_m.get(k).copied().filter(|v| v.is_finite()),
                p.along_m.get(k).copied().filter(|v| v.is_finite()),
            ) {
                (Some(c), Some(a)) => f64::from(c).hypot(f64::from(a)),
                _ => match (truth.query)(own, t) {
                    Ok(s) => dist_m(s.lat, s.lon, lat, lon),
                    Err(_) => continue,
                },
            };
            tested += 1;
            let mut best: Option<(u32, f64)> = None;
            if d_own > cfg.min_own_error_m {
                for l in truth.legs {
                    if l.addr == own || t < l.start_ms || t > l.end_ms {
                        continue;
                    }
                    if let Ok(s) = (truth.query)(l.addr, t) {
                        let d = dist_m(s.lat, s.lon, lat, lon);
                        if best.is_none_or(|(_, bd)| d < bd) {
                            best = Some((l.addr, d));
                        }
                    }
                }
            }
            match best {
                Some((a, d)) if d < d_own => {
                    mism += 1;
                    run.push((t, a, d));
                }
                _ => flush(&mut run, &mut events, &mut cases),
            }
        }
        flush(&mut run, &mut events, &mut cases);
        i = j;
    }
    r.scalars.push(("flights_tested".into(), flights as f64));
    r.scalars.push(("samples_tested".into(), tested as f64));
    r.scalars.push(("mismatch_samples".into(), mism as f64));
    r.scalars.push(("association_errors".into(), events as f64));
    r.notes.push(format!(
        "every {}th paired sample; {} consecutive mismatches make an error; own error above {} m",
        cfg.stride, cfg.min_run, cfg.min_own_error_m
    ));
    (r, cases)
}

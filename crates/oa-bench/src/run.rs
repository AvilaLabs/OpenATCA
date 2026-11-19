//! One benchmark run: association, pairing and every metric over the legacy tables. Core (pure):
//! the same legacy tables, truth and settings give the same results.

use crate::assoc::{associate, astatus, index_truth, Assoc, AssocConfig};
use crate::legacy::{plan_index, Legacy, NONE};
use crate::metrics_event::{
    f1_f2_tbfm, f3_tfms, h1_handoffs, r1_route_conformance, EventConfig, EventCtx,
};
use crate::metrics_flow::{
    t5_publication_interval, t6_staleness, t7_t8_presence, t9_association_errors, PresenceConfig,
    SwapConfig,
};
use crate::metrics_th::add_flagged_view;
use crate::metrics_th::{
    t1_cross_track, t2_along_corrected, t2_along_raw, t3_heading, t3_heading_grid, t3_speed,
    t4_altitude, OffsetConfig,
};
use crate::pair::{build_pairs_with, PairConfig};
use crate::stats::{sort_cases, Exclusions, KeyedCase, MetricResult};
use crate::thindex::build_th_index;
use crate::truth::{Env, Truth};

/// Every setting of a run (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default)]
pub struct RunConfig {
    pub assoc: AssocConfig,
    pub pair: PairConfig,
    pub offset: OffsetConfig,
    pub presence: PresenceConfig,
    pub swap: SwapConfig,
    pub event: EventConfig,
}

/// What a run produced.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct RunOutput {
    pub metrics: Vec<MetricResult>,
    pub exclusions: Exclusions,
    pub assoc: Assoc,
    /// Paired samples.
    pub pairs: usize,
}

/// Runs the whole benchmark.
pub fn run_benchmark(leg: &Legacy, truth: &Truth<'_>, env: &Env<'_>, cfg: &RunConfig) -> RunOutput {
    let ix = build_th_index(&leg.th, leg.flights.gufi.len());
    let tix = index_truth(truth.legs);
    let assoc = associate(leg, &ix, truth, &tix, &cfg.assoc);
    let (pairs, mut ex) = build_pairs_with(leg, &ix, &assoc, truth, &cfg.pair, env.altimeter_at);
    let plan = plan_index(leg);
    let ctx = EventCtx {
        assoc: &assoc,
        truth,
        tix: &tix,
        env,
        cfg: &cfg.event,
    };
    let flagged = flagged_assoc(&assoc);
    let (fpairs, _) = build_pairs_with(leg, &ix, &flagged, truth, &cfg.pair, env.altimeter_at);
    let mut m = vec![
        t1_cross_track(&pairs),
        t2_along_raw(&pairs),
        t2_along_corrected(&pairs, &cfg.offset),
        t3_speed(&pairs),
        t3_heading(&pairs),
        t3_heading_grid(&pairs),
        t4_altitude(&pairs),
        t5_publication_interval(leg, &ix),
        t6_staleness(leg),
    ];
    flagged_views(&mut m, &pairs, &fpairs, &assoc.counts);
    let (t7, t8) = t7_t8_presence(leg, &ix, &assoc, truth, env, &cfg.presence);
    m.push(t7);
    m.push(t8);
    m.push(t9_association_errors(leg, &pairs, truth, &cfg.swap));
    let (f1, f2) = f1_f2_tbfm(leg, &plan, &ctx, &mut ex);
    m.push(f1);
    m.push(f2);
    let (f3a, f3d) = f3_tfms(leg, &plan, &ctx, &mut ex);
    m.push(f3a);
    m.push(f3d);
    m.push(r1_route_conformance(
        leg, &pairs, truth, env, &cfg.event, &mut ex,
    ));
    m.push(h1_handoffs(leg, &assoc, truth, env, &cfg.event, &mut ex));
    // Flights whose address association is implausible (or whose only callsign candidate is far
    // away) are not in the primary rows; their errors are published here, never dropped.
    m.extend(flagged_metrics(&fpairs));
    let mut reasons = [0u64; astatus::REASON_NAMES.len()];
    for &r in &assoc.flag_reason {
        if let Some(c) = reasons.get_mut(usize::from(r)) {
            *c += 1;
        }
    }
    let mut a = assoc_result(&assoc.counts, &reasons, assoc.unchecked, &cfg.assoc);
    let total = association_cases(leg, &ix, &assoc, &mut a);
    case_scalars(
        &mut a,
        duplicate_addr_groups(leg, &ix, &assoc).len() as u64,
        total,
    );
    m.push(a);
    RunOutput {
        metrics: m,
        exclusions: ex,
        assoc,
        pairs: pairs.t_ms.len(),
    }
}

/// Flights flagged as implausible (address, address hint or callsign), from the status counts.
pub(crate) fn flagged_flights(counts: &[u64]) -> u64 {
    [
        astatus::ADDRESS_IMPLAUSIBLE,
        astatus::HINT_IMPLAUSIBLE,
        astatus::CALLSIGN_IMPLAUSIBLE,
    ]
    .iter()
    .map(|&s| counts.get(usize::from(s)).copied().unwrap_or(0))
    .sum()
}

/// The column of a pair table a headline table is of.
type Pick = fn(&crate::pair::PairTable) -> &[f32];

/// Adds the flagged-flight counts and the "all associated including flagged" row to the headline
/// tables T1, T2-raw and T4 of `m`.
pub(crate) fn flagged_views(
    m: &mut [MetricResult],
    pairs: &crate::pair::PairTable,
    fpairs: &crate::pair::PairTable,
    counts: &[u64],
) {
    let n = flagged_flights(counts);
    let picks: [(&str, Pick); 3] = [
        ("T1", |p| &p.cross_m),
        ("T2-raw", |p| &p.along_m),
        ("T4", |p| &p.alt_err_ft),
    ];
    for (id, pick) in picks {
        if let Some(r) = m.iter_mut().find(|r| r.id == id) {
            add_flagged_view(r, pairs, fpairs, n, pick);
        }
    }
}

/// The flagged tables (T1, T2-raw, T4) over the pairs of the flagged flights.
pub(crate) fn flagged_metrics(fpairs: &crate::pair::PairTable) -> Vec<MetricResult> {
    vec![
        flagged_table(
            t1_cross_track(fpairs),
            "T1-flagged",
            "cross-track position error, flagged associations (not in T1)",
        ),
        flagged_table(
            t2_along_raw(fpairs),
            "T2-raw-flagged",
            "along-track error, raw, flagged associations (not in T2-raw)",
        ),
        flagged_table(
            t4_altitude(fpairs),
            "T4-flagged",
            "altitude error, flagged associations (not in T4)",
        ),
    ]
}

/// The association table as scalars of a pseudo-metric, so the report carries it. `reasons` is
/// the number of flights per [`astatus::REASON_NAMES`] entry.
pub(crate) fn assoc_result(
    counts: &[u64],
    reasons: &[u64],
    unchecked: u64,
    cfg: &AssocConfig,
) -> MetricResult {
    let mut a =
        crate::stats::new_result("ASSOC", "association of ERAM flights with truth", "flights");
    for (i, n) in astatus::NAMES.iter().enumerate() {
        a.scalars
            .push(((*n).to_string(), counts.get(i).copied().unwrap_or(0) as f64));
    }
    for (i, n) in astatus::REASON_NAMES.iter().enumerate().skip(1) {
        let c = reasons.get(i).copied().unwrap_or(0);
        a.scalars.push((format!("flagged: {n}"), c as f64));
    }
    a.scalars.push((
        "address associations not checked (too few shared times, trusted)".to_string(),
        unchecked as f64,
    ));
    a.notes.push(format!(
        "selection never uses ERAM accuracy: callsign flights are kept when truth overlaps in time and lies within {:.0} NM; the {:.0} NM / {:.0} ft check only splits the callsign method",
        cfg.region_m / crate::geo::NM_M,
        cfg.gate_h_m / crate::geo::NM_M,
        cfg.gate_v_ft
    ));
    a
}

/// The flagged flights as an association of their own: the primary tables never see them.
pub(crate) fn flagged_assoc(a: &Assoc) -> Assoc {
    let mut f = a.clone();
    f.addr.clone_from(&a.flag_addr);
    f.method.clone_from(&a.flag_method);
    f
}

/// Renames a metric computed over the flagged pairs.
fn flagged_table(mut r: MetricResult, id: &'static str, name: &'static str) -> MetricResult {
    r.id = id;
    r.name = name;
    r.notes.push(
        "pairs of flights whose identity check failed (callsigns disagree, or ERAM positions far from the truth aircraft); published separately so nothing is dropped"
            .to_string(),
    );
    r
}

/// The case lines of the association: the flights kept despite failing the tight check and the
/// flagged flights, each with its place in the list (flagged first, then by truth address, then
/// by the time of the flight's first sample), so the list does not depend on the order of the
/// flights in the table.
pub(crate) fn association_case_lines(
    leg: &Legacy,
    ix: &crate::thindex::ThIndex,
    a: &Assoc,
) -> Vec<KeyedCase> {
    let mut v = Vec::new();
    for f in 0..a.status.len() {
        let st = a.status.get(f).copied().unwrap_or(0);
        let cs = leg
            .flights
            .callsign
            .get(f)
            .map_or_else(String::new, |c| crate::legacy::name(c).to_string());
        let (sev, addr, line) = match st {
            astatus::GATE_FAILED => (
                1u8,
                a.addr.get(f).copied().unwrap_or(NONE),
                format!(
                    "flight {cs}: kept (callsign, {} shared times), {:.0}% inside the 3 NM / 1000 ft check; scored under method 'callsign (failed 3 NM / 1000 ft check)'",
                    a.shared.get(f).copied().unwrap_or(0),
                    a.gate_share.get(f).copied().unwrap_or(f64::NAN) * 100.0
                ),
            ),
            astatus::HINT_OVER_PLAN => (
                1u8,
                a.addr.get(f).copied().unwrap_or(NONE),
                format!(
                    "flight {cs}: plan address {}, track address hint {}; used the hint, scored under method 'address hint (plan address implausible or unknown to truth)'; plan address failed: {}",
                    hex_addr(leg.flights.addr.get(f).copied().unwrap_or(NONE)),
                    hex_addr(leg.flights.addr_hint.get(f).copied().unwrap_or(NONE)),
                    astatus::PLAN_FAIL_NAMES
                        .get(usize::from(a.plan_fail.get(f).copied().unwrap_or(0)))
                        .copied()
                        .unwrap_or("")
                ),
            ),
            astatus::ADDRESS_IMPLAUSIBLE
            | astatus::HINT_IMPLAUSIBLE
            | astatus::CALLSIGN_IMPLAUSIBLE => (
                0u8,
                a.flag_addr.get(f).copied().unwrap_or(NONE),
                format!(
                    "flight {cs}: flagged ({}): {}",
                    astatus::NAMES.get(usize::from(st)).copied().unwrap_or(""),
                    astatus::REASON_NAMES
                        .get(usize::from(a.flag_reason.get(f).copied().unwrap_or(0)))
                        .copied()
                        .unwrap_or("")
                ),
            ),
            _ => continue,
        };
        let t = crate::thindex::samples_of(ix, f as u32)
            .first()
            .and_then(|&s| leg.th.pos_ms.get(s as usize).copied())
            .unwrap_or(0);
        v.push(((sev, addr, t), line));
    }
    for (addr, fs) in duplicate_addr_groups(leg, ix, a) {
        let names: Vec<String> = fs
            .iter()
            .map(|&f| {
                leg.flights
                    .callsign
                    .get(f)
                    .map_or_else(String::new, |c| crate::legacy::name(c).to_string())
            })
            .collect();
        let t = fs
            .iter()
            .filter_map(|&f| {
                crate::thindex::samples_of(ix, f as u32)
                    .first()
                    .and_then(|&s| leg.th.pos_ms.get(s as usize).copied())
            })
            .min()
            .unwrap_or(0);
        v.push((
            (1u8, addr, t),
            format!(
                "truth address {} associated with {} ERAM flights at the same time: {}",
                hex_addr(addr),
                fs.len(),
                names.join(", ")
            ),
        ));
    }
    v
}

/// An address as six hex digits, `none` when absent.
fn hex_addr(a: u32) -> String {
    if a == NONE {
        "none".to_string()
    } else {
        format!("{a:06X}")
    }
}

/// Lists the flights kept despite failing the tight check and the flagged flights, in the order
/// of [`association_case_lines`], at most [`crate::stats::MAX_CASES`].
/// Returns the number of case lines before the list was cut.
pub(crate) fn association_cases(
    leg: &Legacy,
    ix: &crate::thindex::ThIndex,
    a: &Assoc,
    r: &mut MetricResult,
) -> u64 {
    let mut v = association_case_lines(leg, ix, a);
    let total = v.len() as u64;
    sort_cases(&mut v);
    r.cases = v.into_iter().map(|c| c.1).collect();
    total
}

/// Truth addresses associated (in the primary rows) with flights whose time spans (first to last
/// sample time) overlap, with those flights. Consecutive legs of one aircraft do not overlap and
/// are not listed.
pub(crate) fn duplicate_addr_groups(
    leg: &Legacy,
    ix: &crate::thindex::ThIndex,
    a: &Assoc,
) -> std::collections::BTreeMap<u32, Vec<usize>> {
    let span = |f: usize| {
        let mut it = crate::thindex::samples_of(ix, f as u32)
            .iter()
            .filter_map(|&s| leg.th.pos_ms.get(s as usize).copied());
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), t| (lo.min(t), hi.max(t))))
    };
    let mut by: std::collections::BTreeMap<u32, Vec<(usize, i64, i64)>> =
        std::collections::BTreeMap::new();
    for (f, &addr) in a.addr.iter().enumerate() {
        if let (true, Some((lo, hi))) = (addr != NONE, span(f)) {
            by.entry(addr).or_default().push((f, lo, hi));
        }
    }
    let mut m = std::collections::BTreeMap::new();
    for (addr, v) in by {
        let fs: Vec<usize> = v
            .iter()
            .filter(|x| v.iter().any(|y| y.0 != x.0 && x.1.max(y.1) <= x.2.min(y.2)))
            .map(|x| x.0)
            .collect();
        if fs.len() > 1 {
            m.insert(addr, fs);
        }
    }
    m
}

/// The scalars about the case list: addresses shared by several flights, and the lines the cut
/// list left out (`total` lines were found, `r.cases` holds what was kept).
pub(crate) fn case_scalars(r: &mut MetricResult, duplicates: u64, total: u64) {
    r.scalars.push((
        "truth addresses associated with more than one ERAM flight at the same time".to_string(),
        duplicates as f64,
    ));
    let left = total.saturating_sub(r.cases.len() as u64);
    r.scalars.push((
        "more cases not listed (the list is cut at 200)".to_string(),
        left as f64,
    ));
}

/// Scores a tracker's output columns: the columns become the legacy table
/// ([`crate::tracker_input`], one flight per truth leg) and go through [`run_benchmark`], the one
/// code path ERAM's samples take, so association, pairing, the along-track offset treatment and every metric are shared.
pub fn score_track_samples(
    s: &crate::tracker_input::TrackSamples<'_>,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &RunConfig,
) -> Result<RunOutput, crate::tracker_input::SamplesError> {
    let leg = crate::tracker_input::legacy_from_track_samples_by_leg(s, truth.legs)?;
    Ok(run_benchmark(&leg, truth, env, cfg))
}

/// The result of metric `id`, if the run has it.
pub fn find<'a>(out: &'a RunOutput, id: &str) -> Option<&'a MetricResult> {
    out.metrics.iter().find(|m| m.id == id)
}

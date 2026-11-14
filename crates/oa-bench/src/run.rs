//! One benchmark run: association, pairing and every metric over the legacy tables. Core (pure):
//! the same legacy tables, truth and settings give the same results.

use crate::assoc::{associate, astatus, index_truth, Assoc, AssocConfig};
use crate::legacy::{plan_index, Legacy};
use crate::metrics_event::{
    f1_f2_tbfm, f3_tfms, h1_handoffs, r1_route_conformance, EventConfig, EventCtx,
};
use crate::metrics_flow::{
    t5_publication_interval, t6_staleness, t7_t8_presence, t9_association_errors, PresenceConfig,
    SwapConfig,
};
use crate::metrics_th::{
    t1_cross_track, t2_along_corrected, t2_along_raw, t3_heading, t3_speed, t4_altitude,
    OffsetConfig,
};
use crate::pair::{build_pairs, PairConfig};
use crate::stats::{Exclusions, MetricResult};
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
    let (pairs, mut ex) = build_pairs(leg, &ix, &assoc, truth, &cfg.pair);
    let plan = plan_index(leg);
    let ctx = EventCtx {
        assoc: &assoc,
        truth,
        tix: &tix,
        env,
        cfg: &cfg.event,
    };
    let mut m = vec![
        t1_cross_track(&pairs),
        t2_along_raw(&pairs),
        t2_along_corrected(&pairs, &cfg.offset),
        t3_speed(&pairs),
        t3_heading(&pairs),
        t4_altitude(&pairs),
        t5_publication_interval(leg, &ix),
        t6_staleness(leg),
    ];
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
    let flagged = flagged_assoc(&assoc);
    let (fpairs, _) = build_pairs(leg, &ix, &flagged, truth, &cfg.pair);
    m.push(flagged_table(
        t1_cross_track(&fpairs),
        "T1-flagged",
        "cross-track position error, flagged associations (not in T1)",
    ));
    m.push(flagged_table(
        t2_along_raw(&fpairs),
        "T2-raw-flagged",
        "along-track error, raw, flagged associations (not in T2-raw)",
    ));
    m.push(flagged_table(
        t4_altitude(&fpairs),
        "T4-flagged",
        "altitude error, flagged associations (not in T4)",
    ));
    // The association table as scalars of a pseudo-metric, so the report carries it.
    let mut a =
        crate::stats::new_result("ASSOC", "association of ERAM flights with truth", "flights");
    for (i, n) in astatus::NAMES.iter().enumerate() {
        a.scalars.push((
            (*n).to_string(),
            assoc.counts.get(i).copied().unwrap_or(0) as f64,
        ));
    }
    for (i, n) in astatus::REASON_NAMES.iter().enumerate().skip(1) {
        let c = assoc
            .flag_reason
            .iter()
            .filter(|&&r| usize::from(r) == i)
            .count();
        a.scalars.push((format!("flagged: {n}"), c as f64));
    }
    a.scalars.push((
        "address associations not checked (too few shared times, trusted)".to_string(),
        assoc.unchecked as f64,
    ));
    a.notes.push(format!(
        "selection never uses ERAM accuracy: callsign flights are kept when truth overlaps in time and lies within {:.0} NM; the {:.0} NM / {:.0} ft check only splits the callsign method",
        cfg.assoc.region_m / crate::geo::NM_M,
        cfg.assoc.gate_h_m / crate::geo::NM_M,
        cfg.assoc.gate_v_ft
    ));
    association_cases(leg, &assoc, &mut a);
    m.push(a);
    RunOutput {
        metrics: m,
        exclusions: ex,
        assoc,
        pairs: pairs.t_ms.len(),
    }
}

/// The flagged flights as an association of their own: the primary tables never see them.
fn flagged_assoc(a: &Assoc) -> Assoc {
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

/// Lists the flights kept despite failing the tight check and the flagged flights.
fn association_cases(leg: &Legacy, a: &Assoc, r: &mut MetricResult) {
    for f in 0..a.status.len() {
        let st = a.status.get(f).copied().unwrap_or(0);
        let cs = leg
            .flights
            .callsign
            .get(f)
            .map_or_else(String::new, |c| crate::legacy::name(c).to_string());
        let line = match st {
            astatus::GATE_FAILED => format!(
                "flight {cs}: kept (callsign, {} shared times), {:.0}% inside the 3 NM / 1000 ft check; scored under method 'callsign (failed 3 NM / 1000 ft check)'",
                a.shared.get(f).copied().unwrap_or(0),
                a.gate_share.get(f).copied().unwrap_or(f64::NAN) * 100.0
            ),
            astatus::ADDRESS_IMPLAUSIBLE
            | astatus::HINT_IMPLAUSIBLE
            | astatus::CALLSIGN_IMPLAUSIBLE => format!(
                "flight {cs}: flagged ({}): {}",
                astatus::NAMES.get(usize::from(st)).copied().unwrap_or(""),
                astatus::REASON_NAMES
                    .get(usize::from(a.flag_reason.get(f).copied().unwrap_or(0)))
                    .copied()
                    .unwrap_or("")
            ),
            _ => continue,
        };
        if r.cases.len() < crate::stats::MAX_CASES {
            r.cases.push(line);
        }
    }
}

/// Scores a tracker's output columns: the columns become the legacy table
/// ([`crate::tracker_input`]) and go through [`run_benchmark`], the one code path ERAM's samples
/// take, so association, pairing, the along-track offset treatment and every metric are shared.
pub fn score_track_samples(
    s: &crate::tracker_input::TrackSamples<'_>,
    truth: &Truth<'_>,
    env: &Env<'_>,
    cfg: &RunConfig,
) -> Result<RunOutput, crate::tracker_input::SamplesError> {
    let leg = crate::tracker_input::legacy_from_track_samples(s)?;
    Ok(run_benchmark(&leg, truth, env, cfg))
}

/// The result of metric `id`, if the run has it.
pub fn find<'a>(out: &'a RunOutput, id: &str) -> Option<&'a MetricResult> {
    out.metrics.iter().find(|m| m.id == id)
}

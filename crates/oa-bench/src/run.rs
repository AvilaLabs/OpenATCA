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
    // The association table as scalars of a pseudo-metric, so the report carries it.
    let mut a =
        crate::stats::new_result("ASSOC", "association of ERAM flights with truth", "flights");
    for (i, n) in astatus::NAMES.iter().enumerate() {
        a.scalars.push((
            (*n).to_string(),
            assoc.counts.get(i).copied().unwrap_or(0) as f64,
        ));
    }
    m.push(a);
    RunOutput {
        metrics: m,
        exclusions: ex,
        assoc,
        pairs: pairs.t_ms.len(),
    }
}

/// The result of metric `id`, if the run has it.
pub fn find<'a>(out: &'a RunOutput, id: &str) -> Option<&'a MetricResult> {
    out.metrics.iter().find(|m| m.id == id)
}

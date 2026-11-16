//! Adapts `oa-truth` to the [`Truth`] interface of this crate. Core (pure).
//!
//! The metric code sees truth only as a query and a list of legs (see [`crate::truth`]); this
//! module is the whole of what connects the two.
//!
//! * **Query.** `oa_truth::truth_at` is mapped reason for reason: `Ok` to a sample (altitude
//!   and vertical rate rounded to whole feet and feet per minute), `UnknownAircraft` to
//!   [`NoTruth::UnknownAircraft`], `OutsideLegs` to [`NoTruth::OutsideLegs`], `NoNearbyReport`
//!   to [`NoTruth::NoNearbyReport`] and `TooUncertain` to [`NoTruth::TooUncertain`]: the same
//!   names with the same meanings in both crates.
//! * **Legs.** One [`TruthLeg`] per `oa-truth` leg, in `LegIx` order, so
//!   [`TruthSample::leg`] indexes the list. The address is the truth's address word: a
//!   non-ICAO address has bit 24 set and so never equals a 24-bit SFDPS address. Start and end
//!   are the first and last whole milliseconds inside the leg.
//! * **Callsign.** `oa-truth` does not carry callsigns. A leg's callsign is the most frequent
//!   non-empty callsign among the decoded reports of its address whose time lies in the leg's
//!   span (ties go to the smaller bytes). Reports that truth removed still count: the callsign
//!   is identity, not a position. `None` when no report in the span carries one.
//! * **Start.** [`TruthLeg::start`] comes from the ADS-B on-ground indication of the reports of
//!   the address inside the leg's span, in time order (see [`leg_start`]).

use std::collections::BTreeMap;

use oa_adsb::{Reports, F_GROUND, NONE_I32};
use oa_sha256::{to_hex, Sha256};
use oa_truth::{
    build_truth, leg_count, leg_end_ms, leg_end_ns, leg_start_ms, leg_start_ns, truth_at,
    truth_hash_into, LegIx, TruthConfig, TruthError, TruthQuery, TruthStats, DEFAULT_CONFIG,
    REMOVE_NAMES, SPLIT_NAMES,
};

use crate::truth::{LegStart, NoTruth, TruthLeg, TruthSample};

/// Maps one `oa-truth` answer to this crate's.
pub fn map_query(q: TruthQuery) -> Result<TruthSample, NoTruth> {
    match q {
        TruthQuery::Ok(s) => Ok(TruthSample {
            lat: s.lat_deg,
            lon: s.lon_deg,
            baro_alt_ft: s.baro_alt_ft.map(|a| a.round() as i32),
            vel_east_mps: s.vel_east_mps,
            vel_north_mps: s.vel_north_mps,
            vert_rate_fpm: s.vert_rate_fpm.map(|v| v.round() as i32),
            sigma95_h_m: s.sigma95_h_m,
            leg: s.leg.0,
        }),
        TruthQuery::UnknownAircraft => Err(NoTruth::UnknownAircraft),
        TruthQuery::OutsideLegs => Err(NoTruth::OutsideLegs),
        TruthQuery::NoNearbyReport => Err(NoTruth::NoNearbyReport),
        TruthQuery::TooUncertain => Err(NoTruth::TooUncertain),
    }
}

/// The truth query of `truth` in this crate's terms.
pub fn query(truth: &oa_truth::Truth, addr: u32, t_ms: i64) -> Result<TruthSample, NoTruth> {
    map_query(truth_at(truth, addr, t_ms))
}

/// Whether a report is on the ground (`Some(true)`), airborne (`Some(false)`: not flagged on the
/// ground and carrying an altitude) or says neither (`None`).
fn ground_state(reports: &Reports, i: usize) -> Option<bool> {
    if reports.flags.get(i).is_some_and(|&f| f & F_GROUND != 0) {
        return Some(true);
    }
    let has_alt = reports.baro_alt_ft.get(i).is_some_and(|&a| a != NONE_I32)
        || reports.geom_alt_ft.get(i).is_some_and(|&a| a != NONE_I32);
    has_alt.then_some(false)
}

/// How a leg began, from the reports `states` (time in ns, on the ground or not) of its address
/// inside its span, in time order. A takeoff is the first airborne report after a ground report;
/// the gap back to the last ground report bounds when it happened. A leg whose first stated
/// state is airborne has no takeoff; one that is never airborne never took off.
pub fn leg_start(states: &[(i64, bool)]) -> LegStart {
    let mut last_ground: Option<i64> = None;
    for &(t_ns, ground) in states {
        match (ground, last_ground) {
            (true, _) => last_ground = Some(t_ns),
            (false, None) => return LegStart::FirstSeenAirborne,
            (false, Some(g)) => {
                return LegStart::Takeoff {
                    t_ms: t_ns.div_euclid(1_000_000),
                    gap_ms: (t_ns - g).div_euclid(1_000_000),
                }
            }
        }
    }
    if last_ground.is_some() {
        LegStart::NeverAirborne
    } else {
        LegStart::Unknown
    }
}

/// The legs of `truth` with callsigns taken from `reports` (see the module docs).
pub fn legs(truth: &oa_truth::Truth, reports: &Reports) -> Vec<TruthLeg> {
    // Rows that carry a callsign, ordered by (address, time).
    let mut rows: Vec<(u32, i64, u32)> = reports
        .icao
        .iter()
        .zip(&reports.time_ns)
        .zip(&reports.callsign)
        .enumerate()
        .filter(|(_, (_, c))| c.iter().any(|&b| b != 0 && b != b' '))
        .map(|(i, ((&a, &t), _))| (a, t, i as u32))
        .collect();
    rows.sort_unstable();
    // Rows that state a ground state, ordered by (address, time).
    let mut states: Vec<(u32, i64, bool)> = (0..reports.icao.len())
        .filter_map(|i| {
            let g = ground_state(reports, i)?;
            Some((*reports.icao.get(i)?, *reports.time_ns.get(i)?, g))
        })
        .collect();
    states.sort_unstable();
    (0..leg_count(truth))
        .map(|i| {
            let ix = LegIx(i as u32);
            let addr = oa_truth::leg_addr(truth, ix).unwrap_or(0);
            let (s_ns, e_ns) = (
                leg_start_ns(truth, ix).unwrap_or(0),
                leg_end_ns(truth, ix).unwrap_or(-1),
            );
            let first = rows.partition_point(|&(a, t, _)| (a, t) < (addr, s_ns));
            let mut count: BTreeMap<[u8; 8], u32> = BTreeMap::new();
            for &(a, t, r) in rows.get(first..).unwrap_or(&[]) {
                if a != addr || t > e_ns {
                    break;
                }
                if let Some(c) = reports.callsign.get(r as usize) {
                    *count.entry(*c).or_insert(0) += 1;
                }
            }
            let mut best: Option<([u8; 8], u32)> = None;
            for (c, n) in count {
                if best.is_none_or(|(_, bn)| n > bn) {
                    best = Some((c, n));
                }
            }
            let sf = states.partition_point(|&(a, t, _)| (a, t) < (addr, s_ns));
            let span: Vec<(i64, bool)> = states
                .get(sf..)
                .unwrap_or(&[])
                .iter()
                .take_while(|&&(a, t, _)| a == addr && t <= e_ns)
                .map(|&(_, t, g)| (t, g))
                .collect();
            TruthLeg {
                start: leg_start(&span),
                addr,
                start_ms: leg_start_ms(truth, ix).unwrap_or(0),
                end_ms: leg_end_ms(truth, ix).unwrap_or(-1),
                callsign: best.map(|(c, _)| c),
            }
        })
        .collect()
}

/// A built truth with what the report needs from it.
// A7.2: cold struct, size guard waived
pub struct BuiltTruth {
    pub truth: oa_truth::Truth,
    pub stats: TruthStats,
    pub legs: Vec<TruthLeg>,
    /// The truth digest (`oa_truth::truth_hash_into`) in hex.
    pub digest: String,
}

/// Builds the truth of `reports` and adapts it.
pub fn build(reports: &Reports, cfg: &TruthConfig) -> Result<BuiltTruth, TruthError> {
    let (truth, stats) = build_truth(reports, cfg)?;
    let mut h = Sha256::new();
    truth_hash_into(&mut h, &truth);
    let digest = to_hex(&h.finalize());
    let legs = legs(&truth, reports);
    Ok(BuiltTruth {
        truth,
        stats,
        legs,
        digest,
    })
}

/// The design's truth settings with the two gate overrides applied.
pub fn config_with(bracket_s: Option<f64>, max_sigma_m: Option<f64>) -> TruthConfig {
    let mut c = DEFAULT_CONFIG;
    if let Some(b) = bracket_s {
        c.bracket_ms = (b * 1000.0).round() as i64;
    }
    if let Some(s) = max_sigma_m {
        c.max_sigma95_h_m = s;
    }
    c
}

fn pct(n: u64, hit: u64) -> String {
    if n == 0 {
        "n/a".to_string()
    } else {
        format!("{:.2} % ({hit} of {n})", 100.0 * hit as f64 / n as f64)
    }
}

fn dist_row(d: &oa_truth::Dist) -> String {
    format!(
        "n={} mean={:.4} rms={:.4} median={:.4} p95={:.4} p99={:.4} max={:.4}",
        d.n, d.mean, d.rms, d.median, d.p95, d.p99, d.max
    )
}

/// The truth section of the report as `(name, value)` rows: gates, statistics, removals by
/// reason, the leave-one-out table and the digest.
pub fn truth_section(b: &BuiltTruth, cfg: &TruthConfig) -> Vec<(String, String)> {
    let s = &b.stats;
    let mut v: Vec<(String, String)> = Vec::new();
    let mut add = |k: &str, x: String| v.push((k.to_string(), x));
    add("bracket_s", format!("{}", cfg.bracket_ms as f64 / 1000.0));
    add("max_sigma95_h_m", format!("{}", cfg.max_sigma95_h_m));
    add(
        "quality_gates",
        format!(
            "NACp >= {}, NIC >= {}, SIL {}, version >= {}",
            cfg.min_nacp, cfg.min_nic, cfg.required_sil, cfg.min_version
        ),
    );
    add("rows_in", s.rows_in.to_string());
    add("aircraft_in", s.aircraft_in.to_string());
    add("rows_kept", s.rows_kept.to_string());
    add("aircraft_with_truth", s.aircraft_out.to_string());
    add("legs", s.legs.to_string());
    for (n, c) in SPLIT_NAMES.iter().zip(s.splits.iter()) {
        add(&format!("leg_starts_{n}"), c.to_string());
    }
    let mut removed = 0u64;
    for (n, c) in REMOVE_NAMES.iter().zip(s.removed.iter()) {
        add(&format!("rows_removed_{n}"), c.to_string());
        removed += c;
    }
    add("rows_removed_total", removed.to_string());
    add(
        "rows_passed_on_carried_quality",
        s.quality_carried.to_string(),
    );
    add(
        "uncertainty_within_gate_at_reports",
        pct(s.unc_at_reports.n, s.unc_at_reports.hit),
    );
    add(
        "uncertainty_within_gate_at_midpoints",
        pct(s.unc_at_midpoints.n, s.unc_at_midpoints.hit),
    );
    let l = &s.loo;
    if cfg.loo {
        add(
            "loo",
            format!(
                "every {}-th report hidden: {} hidden, {} not predicted (no real report within the bracket on both sides)",
                l.stride, l.hidden, l.no_bracket
            ),
        );
        add("loo_horizontal_m", dist_row(&l.horizontal));
        add("loo_cross_track_m", dist_row(&l.cross));
        add("loo_along_track_m", dist_row(&l.along));
        add("loo_altitude_ft", dist_row(&l.alt));
        add(
            "loo_predicted_radius_within_gate",
            pct(l.under_gate.n, l.under_gate.hit),
        );
        add("loo_coverage95", pct(l.coverage95.n, l.coverage95.hit));
    } else {
        add("loo", "not run".to_string());
    }
    add("truth_digest", b.digest.clone());
    v
}

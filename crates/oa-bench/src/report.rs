//! The report (M2_DESIGN section 5): plain text and CSV. Core (pure): a function of the header,
//! the metric results and the exclusions, so identical inputs give identical bytes.
//!
//! The header carries the input file hashes, the code version, the replay state hash (when the
//! inputs are recordings the replay can run on) and the extraction counts. Every metric is
//! printed with its N; the exclusions table lists every (scope, reason) count so nothing is
//! dropped silently. Floating point values print with six decimals.

use std::fmt::Write as _;

use crate::run::RunOutput;
use crate::stats::{MetricResult, Stats};

/// The version string of the metric code. Bump when a definition changes.
pub const METRICS_VERSION: &str = "oa-bench metrics v1";

/// What the report says before any number (M2_DESIGN "What SFDPS lets us measure").
pub const SCOPE_TEXT: &str = "SFDPS publishes ERAM's published track, not its internal track: it may decimate or smooth the \
track and it arrives about 60 s late (UK-004). This report measures the published position, \
velocity and altitude error at each published position time, how often a position is published \
and how stale it is, track presence and association errors. It cannot measure ERAM's internal \
probability of update or internal track error.";

/// What the statistics columns mean.
pub const STATS_TEXT: &str = "n is the number of samples. mean is signed; rms is the root mean square; median, p95, p99 and \
max are taken over absolute values with the nearest-rank method (sort ascending, take rank \
ceil(p*n/100), 1-based, no interpolation).";

/// One hashed input.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputHash {
    pub label: String,
    pub files: u64,
    pub bytes: u64,
    /// SHA-256 as lower-case hex: of the file for one file, of the sorted `name:hash` manifest
    /// for a directory.
    pub sha256: String,
}

/// The report header (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// `synthetic` or `recorded`.
    pub mode: String,
    pub code_version: String,
    pub inputs: Vec<InputHash>,
    /// Replay state hash in hex, or the reason there is none.
    pub replay_hash: String,
    /// Extraction counts, printed in order.
    pub counts: Vec<(String, u64)>,
    /// Free notes (truth source, limits).
    pub notes: Vec<String>,
}

/// The code version string for this build.
pub fn code_version() -> String {
    format!("{METRICS_VERSION}; crate {}", env!("CARGO_PKG_VERSION"))
}

fn stats_line(label: &str, s: &Stats) -> String {
    format!(
        "  {label:<34} n={:<9} mean={:>12.4} rms={:>12.4} median={:>12.4} p95={:>12.4} p99={:>12.4} max={:>12.4}\n",
        s.n, s.mean, s.rms, s.median, s.p95, s.p99, s.max
    )
}

fn metric_text(m: &MetricResult, out: &mut String) {
    let _ = writeln!(out, "\n[{}] {} (unit: {})", m.id, m.name, m.unit);
    if let Some(w) = &m.withheld {
        let _ = writeln!(out, "  WITHHELD: {w}");
    }
    for r in &m.rows {
        out.push_str(&stats_line(&r.label, &r.stats));
    }
    for (k, v) in &m.scalars {
        let _ = writeln!(out, "  {k} = {v:.6}");
    }
    for n in &m.notes {
        let _ = writeln!(out, "  note: {n}");
    }
    for (i, c) in m.cases.iter().enumerate() {
        let _ = writeln!(out, "  case {}: {c}", i + 1);
    }
}

/// The plain-text report.
pub fn render_text(h: &Header, run: &RunOutput) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "OpenATCA benchmark report ({})", h.mode);
    let _ = writeln!(o, "code version: {}", h.code_version);
    let _ = writeln!(o, "replay state hash: {}", h.replay_hash);
    let _ = writeln!(o, "\nInputs");
    for i in &h.inputs {
        let _ = writeln!(
            o,
            "  {}: {} file(s), {} bytes, sha256 {}",
            i.label, i.files, i.bytes, i.sha256
        );
    }
    let _ = writeln!(o, "\nExtraction counts");
    for (k, v) in &h.counts {
        let _ = writeln!(o, "  {k}: {v}");
    }
    let _ = writeln!(o, "\nScope\n  {SCOPE_TEXT}");
    let _ = writeln!(o, "\nStatistics\n  {STATS_TEXT}");
    for n in &h.notes {
        let _ = writeln!(o, "note: {n}");
    }
    let _ = writeln!(o, "\nPaired samples: {}", run.pairs);
    for m in &run.metrics {
        metric_text(m, &mut o);
    }
    let _ = writeln!(o, "\nExclusions (scope, reason, count)");
    for ((scope, reason), n) in &run.exclusions {
        let _ = writeln!(o, "  {scope}: {reason}: {n}");
    }
    o
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The machine-readable report: one record per line, the first field names the record kind
/// (`header`, `input`, `count`, `stat`, `scalar`, `withheld`, `note`, `case`, `exclusion`).
pub fn render_csv(h: &Header, run: &RunOutput) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "kind,metric,label,n,mean,rms,median,p95,p99,max");
    let hv = |o: &mut String, k: &str, v: &str| {
        let _ = writeln!(o, "header,,{},{}", csv_field(k), csv_field(v));
    };
    hv(&mut o, "mode", &h.mode);
    hv(&mut o, "code_version", &h.code_version);
    hv(&mut o, "replay_state_hash", &h.replay_hash);
    hv(&mut o, "paired_samples", &run.pairs.to_string());
    for i in &h.inputs {
        let _ = writeln!(
            o,
            "input,,{},{},{},{}",
            csv_field(&i.label),
            i.files,
            i.bytes,
            i.sha256
        );
    }
    for (k, v) in &h.counts {
        let _ = writeln!(o, "count,,{},{v}", csv_field(k));
    }
    for n in &h.notes {
        let _ = writeln!(o, "note,,{}", csv_field(n));
    }
    for m in &run.metrics {
        if let Some(w) = &m.withheld {
            let _ = writeln!(o, "withheld,{},{}", m.id, csv_field(w));
        }
        for r in &m.rows {
            let s = &r.stats;
            let _ = writeln!(
                o,
                "stat,{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
                m.id,
                csv_field(&r.label),
                s.n,
                s.mean,
                s.rms,
                s.median,
                s.p95,
                s.p99,
                s.max
            );
        }
        for (k, v) in &m.scalars {
            let _ = writeln!(o, "scalar,{},{},{v:.6}", m.id, csv_field(k));
        }
        for n in &m.notes {
            let _ = writeln!(o, "note,{},{}", m.id, csv_field(n));
        }
        for (i, c) in m.cases.iter().enumerate() {
            let _ = writeln!(o, "case,{},{},{}", m.id, i + 1, csv_field(c));
        }
    }
    for ((scope, reason), n) in &run.exclusions {
        let _ = writeln!(
            o,
            "exclusion,{},{},{n}",
            csv_field(scope),
            csv_field(reason)
        );
    }
    o
}

/// The extraction counts of a legacy set, as report lines.
pub fn extraction_counts(l: &crate::legacy::Legacy) -> Vec<(String, u64)> {
    use crate::legacy::NONE;
    let c = &l.counts;
    let with_addr = l.flights.addr.iter().filter(|&&a| a != NONE).count() as u64;
    let with_th = {
        let mut seen = vec![false; l.flights.gufi.len()];
        for &f in &l.th.flight {
            if let Some(s) = seen.get_mut(f as usize) {
                *s = true;
            }
        }
        seen.iter().filter(|&&s| s).count() as u64
    };
    let with_th_addr = {
        let mut seen = vec![false; l.flights.gufi.len()];
        for &f in &l.th.flight {
            if let Some(s) = seen.get_mut(f as usize) {
                *s = true;
            }
        }
        seen.iter()
            .zip(l.flights.addr.iter())
            .filter(|(&s, &a)| s && a != NONE)
            .count() as u64
    };
    let with_hint = l.flights.addr_hint.iter().filter(|&&a| a != NONE).count() as u64;
    let pairs = || l.flights.addr.iter().zip(l.flights.addr_hint.iter());
    let both_eq = pairs().filter(|(&a, &h)| a != NONE && a == h).count() as u64;
    let both_ne = pairs()
        .filter(|(&a, &h)| a != NONE && h != NONE && a != h)
        .count() as u64;
    let with_th_any = {
        let mut seen = vec![false; l.flights.gufi.len()];
        for &f in &l.th.flight {
            if let Some(s) = seen.get_mut(f as usize) {
                *s = true;
            }
        }
        seen.iter()
            .zip(pairs())
            .filter(|(&s, (&a, &h))| s && (a != NONE || h != NONE))
            .count() as u64
    };
    let plan = crate::legacy::plan_index(l);
    let tbfm_match = l
        .tbfm
        .plan_id
        .iter()
        .filter(|p| plan.contains_key(*p))
        .count() as u64;
    let tfms_match = l
        .tfms
        .plan_id
        .iter()
        .filter(|p| {
            let k: [u8; 12] = crate::legacy::pad(crate::legacy::name(*p).as_bytes());
            plan.contains_key(&k)
        })
        .count() as u64;
    let n = |v: usize| v as u64;
    vec![
        ("SFDPS documents".into(), c.docs[0]),
        ("TBFM documents".into(), c.docs[1]),
        ("TFMS documents".into(), c.docs[2]),
        (
            "SFDPS documents refused (non-flight-data roots, e.g. FDPS status messages)".into(),
            c.doc_errors[0],
        ),
        ("TBFM documents refused".into(), c.doc_errors[1]),
        ("TFMS documents refused".into(), c.doc_errors[2]),
        (
            "SFDPS messages rejected by the decoder".into(),
            c.sfdps_rejected,
        ),
        ("flights".into(), n(l.flights.gufi.len())),
        ("flights with a track sample".into(), with_th),
        ("flights with aircraftAddress".into(), with_addr),
        (
            "flights with a track sample and aircraftAddress".into(),
            with_th_addr,
        ),
        ("flights with an ADSB_02M address hint".into(), with_hint),
        ("flights with both address and hint, equal".into(), both_eq),
        (
            "flights with both address and hint, different".into(),
            both_ne,
        ),
        (
            "flights with a track sample and any address (aircraftAddress or hint)".into(),
            with_th_any,
        ),
        ("ERAM TH samples".into(), n(l.th.flight.len())),
        ("TH samples with the coast indicator".into(), c.th_coast),
        ("TH samples without a reported altitude".into(), c.th_no_alt),
        (
            "SFDPS messages with an ADSB_02M name-value pair".into(),
            c.th_adsb_hint,
        ),
        ("handoff events".into(), n(l.handoffs.flight.len())),
        ("expanded routes".into(), n(l.routes.flight.len())),
        ("route points".into(), n(l.route_points.fix.len())),
        (
            "route points relative to a fix".into(),
            c.route_relative_points,
        ),
        (
            "TBFM estimate and schedule values".into(),
            n(l.tbfm.kind.len()),
        ),
        (
            "TBFM values whose flight plan id matches an SFDPS flight".into(),
            tbfm_match,
        ),
        (
            "TFMS estimated arrival and departure times".into(),
            n(l.tfms.kind.len()),
        ),
        (
            "TFMS values whose flight plan id matches an SFDPS flight".into(),
            tfms_match,
        ),
        (
            "rows dropped at capacity (flights, TH, handoffs, routes, TBFM, TFMS)".into(),
            c.overflow_flights
                + c.overflow_th
                + c.overflow_handoffs
                + c.overflow_routes
                + c.overflow_tbfm
                + c.overflow_tfms,
        ),
    ]
}

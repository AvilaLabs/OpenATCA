//! What a run counts, and the text and CSV it is written as. Core (pure): the same counts give
//! the same bytes. Every number is printed with its n; an exclusion is counted under one reason.

use crate::core::bins::{stats, BIN_COUNT, BIN_NAMES};
use crate::core::join::{OUTCOME_COUNT, OUTCOME_NAMES};
use crate::core::types::{
    exclusion_index, Config, Exclusion, Sample, EXCLUSION_COUNT, EXCLUSION_NAMES,
};

/// Upper edges, nautical miles, of the closest-approach distance buckets; the last bucket is
/// everything farther.
pub const DIST_EDGES_NM: [f64; 8] = [0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0, 50.0];
/// Number of closest-approach buckets.
pub const DIST_BUCKETS: usize = 9;
const NM_M: f64 = 1852.0;

/// The scored errors and the exclusions of one kind of estimate.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct KindTable {
    pub name: String,
    /// Estimates offered, scored or not.
    pub estimates: u64,
    /// Error in seconds per bin.
    pub errors: Vec<Vec<f64>>,
    /// The flight each error belongs to, per bin, in the same order.
    pub units: Vec<Vec<u32>>,
    pub exclusions: [u64; EXCLUSION_COUNT],
    /// Error in seconds per bin of the estimated look-ahead (the estimate minus the issue time).
    pub errors_est: Vec<Vec<f64>>,
    /// Error against the TFMS actual arrival time instead of the landing, per look-ahead bin of
    /// the landing (a diagnostic; only where the flight has one).
    pub errors_alt: Vec<Vec<f64>>,
    /// Seconds from the last airborne report to the first ground report of each landing used.
    pub gaps_s: Vec<f64>,
    /// The landing minus the TFMS actual arrival, seconds, for each sample that has one.
    pub landing_minus_actual_s: Vec<f64>,
    /// Estimates whose point or airport name had several places and one was chosen.
    pub resolved_ambiguous: u64,
    /// Error in seconds by how the flight joined to the truth: by address, by callsign.
    pub errors_join: Vec<Vec<f64>>,
}

/// What a scored sample carries besides its error (all diagnostics).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extra {
    /// The gap of the landing used, seconds.
    pub gap_s: Option<f64>,
    /// The error against the TFMS actual arrival, and the landing minus that arrival, seconds.
    pub alt: Option<(f64, f64)>,
    /// The flight joined to the truth by callsign (not by address).
    pub by_callsign: Option<bool>,
}

/// An empty table.
pub fn kind_new(name: &str) -> KindTable {
    KindTable {
        name: name.to_string(),
        estimates: 0,
        errors: vec![Vec::new(); BIN_COUNT],
        units: vec![Vec::new(); BIN_COUNT],
        exclusions: [0; EXCLUSION_COUNT],
        errors_est: vec![Vec::new(); BIN_COUNT],
        errors_alt: vec![Vec::new(); BIN_COUNT],
        gaps_s: Vec::new(),
        landing_minus_actual_s: Vec::new(),
        resolved_ambiguous: 0,
        errors_join: vec![Vec::new(); 2],
    }
}

/// Counts one estimate of flight `unit`: into its bin, or under its exclusion.
pub fn kind_add(t: &mut KindTable, unit: u32, r: Result<Sample, Exclusion>) {
    kind_add_with(t, unit, r, Extra::default());
}

/// [`kind_add`] with the diagnostics of the sample.
pub fn kind_add_with(t: &mut KindTable, unit: u32, r: Result<Sample, Exclusion>, x: Extra) {
    t.estimates += 1;
    match r {
        Ok(s) => {
            if let (Some(e), Some(u)) = (t.errors.get_mut(s.bin), t.units.get_mut(s.bin)) {
                e.push(s.error_s);
                u.push(unit);
            }
            if let Some(e) = s.est_bin.and_then(|b| t.errors_est.get_mut(b)) {
                e.push(s.error_s);
            }
            if let Some(e) = x
                .by_callsign
                .and_then(|c| t.errors_join.get_mut(usize::from(c)))
            {
                e.push(s.error_s);
            }
            if let Some(g) = x.gap_s {
                t.gaps_s.push(g);
            }
            if let Some((err, diff)) = x.alt {
                if let Some(e) = t.errors_alt.get_mut(s.bin) {
                    e.push(err);
                }
                t.landing_minus_actual_s.push(diff);
            }
        }
        Err(x) => {
            if let Some(c) = t.exclusions.get_mut(exclusion_index(x)) {
                *c += 1;
            }
        }
    }
}

/// Counts `n` estimates under one exclusion (a flight that was not joined, for all its estimates).
pub fn kind_exclude(t: &mut KindTable, x: Exclusion, n: u64) {
    t.estimates += n;
    if let Some(c) = t.exclusions.get_mut(exclusion_index(x)) {
        *c += n;
    }
}

/// One system: its kinds, how its flights joined and how close the truth came to its points.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct SystemReport {
    pub name: String,
    pub kinds: Vec<KindTable>,
    /// Flights by join outcome ([`OUTCOME_NAMES`]).
    pub outcomes: [u64; OUTCOME_COUNT],
    /// Closest approach of the truth path to each (flight, point) searched, by bucket
    /// ([`DIST_EDGES_NM`]).
    pub closest: [u64; DIST_BUCKETS],
}

/// An empty system report with the given kinds.
pub fn system_new(name: &str, kinds: &[&str]) -> SystemReport {
    SystemReport {
        name: name.to_string(),
        kinds: kinds.iter().map(|k| kind_new(k)).collect(),
        outcomes: [0; OUTCOME_COUNT],
        closest: [0; DIST_BUCKETS],
    }
}

/// Counts one closest approach, metres. An infinite one (no truth sample) is not counted.
pub fn closest_add(s: &mut SystemReport, dist_m: f64) {
    if dist_m.is_infinite() || dist_m.is_nan() {
        return;
    }
    let d = dist_m / NM_M;
    let k = DIST_EDGES_NM.iter().take_while(|&&e| d > e).count();
    if let Some(c) = s.closest.get_mut(k) {
        *c += 1;
    }
}

fn units_in(units: &[u32]) -> u64 {
    let mut v = units.to_vec();
    v.sort_unstable();
    v.dedup();
    v.len() as u64
}

/// The bins of a kind as rows: the name, the errors and the flights of each bin, then the rows of
/// all bins together.
fn bin_rows(t: &KindTable) -> Vec<(String, Vec<f64>, u64)> {
    let mut rows: Vec<(String, Vec<f64>, u64)> = Vec::new();
    let mut all: Vec<f64> = Vec::new();
    let mut all_units: Vec<u32> = Vec::new();
    for k in 0..BIN_COUNT {
        let e = t.errors.get(k).cloned().unwrap_or_default();
        let u = t.units.get(k).cloned().unwrap_or_default();
        all.extend_from_slice(&e);
        all_units.extend_from_slice(&u);
        rows.push((
            BIN_NAMES.get(k).copied().unwrap_or("?").to_string(),
            e,
            units_in(&u),
        ));
    }
    rows.push((
        "all bins (0 to 120 min)".to_string(),
        all,
        units_in(&all_units),
    ));
    rows
}

fn cell(v: f64) -> String {
    format!("{v:.1}")
}

/// Errors larger than this in absolute value are counted as large, seconds.
pub const LARGE_ERROR_S: f64 = 1800.0;
const HIST_EDGES_S: [f64; 8] = [-1800.0, -600.0, -300.0, -60.0, 60.0, 300.0, 600.0, 1800.0];

fn large(e: &[f64]) -> u64 {
    e.iter().filter(|x| x.abs() > LARGE_ERROR_S).count() as u64
}

fn stat_line(label: &str, e: &[f64], flights: Option<u64>) -> String {
    match stats(e) {
        Some(st) => {
            let f = flights.map_or(String::new(), |f| format!(" flights={f}"));
            let big = large(e);
            format!(
                "  {label}: n={}{f} median={} mean={} p5={} p95={} rms={} over {}s: n={big} ({:.1}%)\n",
                st.n,
                cell(st.median),
                cell(st.mean),
                cell(st.p5),
                cell(st.p95),
                cell(st.rms),
                LARGE_ERROR_S as i64,
                100.0 * big as f64 / st.n as f64
            )
        }
        None => format!("  {label}: n=0\n"),
    }
}

fn est_rows(t: &KindTable) -> String {
    let mut o = String::new();
    for k in 0..BIN_COUNT {
        let e = t.errors_est.get(k).map_or(&[][..], Vec::as_slice);
        o.push_str(&stat_line(
            BIN_NAMES.get(k).copied().unwrap_or("?"),
            e,
            None,
        ));
    }
    o
}

/// Counts of errors per bucket: under -1800, up to -600, -300, -60, within 60, up to 300, 600,
/// 1800, over 1800 (seconds), as `n1/n2/...`.
fn histogram(e: &[f64]) -> String {
    let mut c = [0u64; 9];
    for x in e {
        let k = HIST_EDGES_S.iter().take_while(|&&h| *x >= h).count();
        if let Some(v) = c.get_mut(k) {
            *v += 1;
        }
    }
    let parts: Vec<String> = c.iter().map(u64::to_string).collect();
    format!("[<-1800, -1800..-600, -600..-300, -300..-60, -60..60, 60..300, 300..600, 600..1800, >=1800] = {}", parts.join("/"))
}

/// The statistics rows of a system as CSV: one row per kind and bin, and a last row per kind for
/// all bins. `n` is the number of scored estimates, `flights` the number of flights they came from.
pub fn render_csv(s: &SystemReport) -> String {
    let mut o =
        String::from("system,kind,bin,n,flights,median_s,mean_s,p5_s,p95_s,rms_s,over_1800s\n");
    for t in &s.kinds {
        for (bin, e, flights) in bin_rows(t) {
            match stats(&e) {
                Some(st) => o.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{},{}\n",
                    s.name,
                    t.name,
                    bin,
                    st.n,
                    flights,
                    cell(st.median),
                    cell(st.mean),
                    cell(st.p5),
                    cell(st.p95),
                    cell(st.rms),
                    large(&e)
                )),
                None => o.push_str(&format!("{},{},{},0,0,,,,,,\n", s.name, t.name, bin)),
            }
        }
    }
    o
}

/// The counts of a system as CSV: `table,key,n` rows for the join outcomes, the exclusions of each
/// kind and the closest-approach buckets.
pub fn render_counts_csv(s: &SystemReport) -> String {
    let mut o = String::from("system,table,key,n\n");
    for (name, n) in OUTCOME_NAMES.iter().zip(s.outcomes) {
        o.push_str(&format!("{},join,{},{}\n", s.name, name, n));
    }
    for t in &s.kinds {
        o.push_str(&format!(
            "{},estimates,{},{}\n",
            s.name, t.name, t.estimates
        ));
        for (name, n) in EXCLUSION_NAMES.iter().zip(t.exclusions) {
            o.push_str(&format!("{},excluded {},{},{}\n", s.name, t.name, name, n));
        }
    }
    for (k, n) in s.closest.iter().enumerate() {
        o.push_str(&format!(
            "{},closest approach,{},{}\n",
            s.name,
            bucket_name(k),
            n
        ));
    }
    o
}

fn bucket_name(k: usize) -> String {
    match (
        k.checked_sub(1).and_then(|p| DIST_EDGES_NM.get(p)),
        DIST_EDGES_NM.get(k),
    ) {
        (None, Some(hi)) => format!("up to {hi} NM"),
        (Some(lo), Some(hi)) => format!("over {lo} up to {hi} NM"),
        (Some(lo), None) => format!("over {lo} NM"),
        (None, None) => "?".to_string(),
    }
}

/// A titled block of lines the shell computed (match rates, diagnostics).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct Section {
    pub title: String,
    pub lines: Vec<String>,
}

/// The whole report as text. `header` is the run's own lines (inputs, hashes, loading counts),
/// printed first as `name: value`; `sections` follow the settings.
pub fn render_text(
    header: &[(String, String)],
    cfg: &Config,
    sections: &[Section],
    systems: &[SystemReport],
) -> String {
    let mut o =
        String::from("oa-legacy-traj: legacy trajectory prediction against ADS-B truth\n\n");
    for (k, v) in header {
        o.push_str(&format!("{k}: {v}\n"));
    }
    o.push_str(&format!(
        "\nsettings\n  crossing: time of closest approach of the truth path to the point, counted only within {:.2} NM\n  landing: last airborne report before the first ground report, within {:.1} NM of the airport reference point, gap to the ground report at most {} s\n  error: estimate minus actual, seconds (positive: the estimate is later than what happened)\n  look-ahead: actual time minus the time of the message that carried the estimate, bins {}\n  statistics: median, mean, 5th and 95th percentile (nearest rank, as oa-bench and p002 define it, on the signed errors) and root mean square of the error, in seconds; n is scored estimates, flights the distinct flights they came from\n",
        cfg.cut_m / NM_M,
        cfg.runway_radius_m / NM_M,
        cfg.landing_gap_max_ms / 1000,
        BIN_NAMES.join(", ")
    ));
    for sec in sections {
        o.push_str(&format!("\n== {} ==\n", sec.title));
        for l in &sec.lines {
            o.push_str(l);
            o.push('\n');
        }
    }
    for s in systems {
        o.push_str(&format!("\n== {} ==\n", s.name));
        o.push_str("\nflights by join outcome\n");
        let total: u64 = s.outcomes.iter().sum();
        for (name, n) in OUTCOME_NAMES.iter().zip(s.outcomes) {
            o.push_str(&format!("  {name}: n={n}\n"));
        }
        o.push_str(&format!("  all flights: n={total}\n"));
        o.push_str("\nclosest approach of the truth path to each point searched (joined flights with truth)\n");
        let searched: u64 = s.closest.iter().sum();
        for (k, n) in s.closest.iter().enumerate() {
            o.push_str(&format!("  {}: n={n}\n", bucket_name(k)));
        }
        o.push_str(&format!("  all searched: n={searched}\n"));
        for t in &s.kinds {
            o.push_str(&format!("\n-- {} --\n", t.name));
            o.push_str(&format!("estimates offered: n={}\n", t.estimates));
            o.push_str(&format!(
                "estimates whose name had several places, nearest the route taken: n={}\n",
                t.resolved_ambiguous
            ));
            o.push_str("error by look-ahead (actual time minus issue time)\n");
            for (bin, e, flights) in bin_rows(t) {
                o.push_str(&stat_line(&bin, &e, Some(flights)));
            }
            o.push_str("error by estimated look-ahead (estimate minus issue time)\n");
            o.push_str(&est_rows(t));
            o.push_str("error by how the flight joined to the truth\n");
            for (k, label) in ["joined by address", "joined by callsign"]
                .iter()
                .enumerate()
            {
                let e = t.errors_join.get(k).map_or(&[][..], Vec::as_slice);
                o.push_str(&stat_line(label, e, None));
            }
            o.push_str("error histogram by look-ahead, counts per bucket of error in seconds\n");
            for k in 0..BIN_COUNT {
                let e = t.errors.get(k).map_or(&[][..], Vec::as_slice);
                o.push_str(&format!(
                    "  {}: n={} {}\n",
                    BIN_NAMES.get(k).copied().unwrap_or("?"),
                    e.len(),
                    histogram(e)
                ));
            }
            if t.errors_alt.iter().any(|e| !e.is_empty()) {
                o.push_str("error against the TFMS actual arrival time instead of the landing, by look-ahead (diagnostic)\n");
                for k in 0..BIN_COUNT {
                    let e = t.errors_alt.get(k).map_or(&[][..], Vec::as_slice);
                    o.push_str(&stat_line(
                        BIN_NAMES.get(k).copied().unwrap_or("?"),
                        e,
                        None,
                    ));
                }
            }
            if !t.gaps_s.is_empty() {
                o.push_str("gap from the last airborne report to the first ground report of the landings used, seconds\n");
                o.push_str(&stat_line("all landings used", &t.gaps_s, None));
            }
            if !t.landing_minus_actual_s.is_empty() {
                o.push_str(
                    "landing (last airborne report) minus the TFMS actual arrival time, seconds\n",
                );
                o.push_str(&stat_line(
                    "samples with an actual arrival",
                    &t.landing_minus_actual_s,
                    None,
                ));
            }
            o.push_str("excluded, by reason\n");
            let excluded: u64 = t.exclusions.iter().sum();
            for (name, n) in EXCLUSION_NAMES.iter().zip(t.exclusions) {
                if n > 0 {
                    o.push_str(&format!("  {name}: n={n}\n"));
                }
            }
            let scored: u64 = t.errors.iter().map(|e| e.len() as u64).sum();
            o.push_str(&format!(
                "  excluded in all: n={excluded}; scored: n={scored}; offered: n={}\n",
                t.estimates
            ));
        }
    }
    o
}

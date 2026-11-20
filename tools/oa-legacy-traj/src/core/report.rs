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
}

/// An empty table.
pub fn kind_new(name: &str) -> KindTable {
    KindTable {
        name: name.to_string(),
        estimates: 0,
        errors: vec![Vec::new(); BIN_COUNT],
        units: vec![Vec::new(); BIN_COUNT],
        exclusions: [0; EXCLUSION_COUNT],
    }
}

/// Counts one estimate of flight `unit`: into its bin, or under its exclusion.
pub fn kind_add(t: &mut KindTable, unit: u32, r: Result<Sample, Exclusion>) {
    t.estimates += 1;
    match r {
        Ok(s) => {
            if let (Some(e), Some(u)) = (t.errors.get_mut(s.bin), t.units.get_mut(s.bin)) {
                e.push(s.error_s);
                u.push(unit);
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

/// The statistics rows of a system as CSV: one row per kind and bin, and a last row per kind for
/// all bins. `n` is the number of scored estimates, `flights` the number of flights they came from.
pub fn render_csv(s: &SystemReport) -> String {
    let mut o = String::from("system,kind,bin,n,flights,median_s,mean_s,p5_s,p95_s,rms_s\n");
    for t in &s.kinds {
        for (bin, e, flights) in bin_rows(t) {
            match stats(&e) {
                Some(st) => o.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{}\n",
                    s.name,
                    t.name,
                    bin,
                    st.n,
                    flights,
                    cell(st.median),
                    cell(st.mean),
                    cell(st.p5),
                    cell(st.p95),
                    cell(st.rms)
                )),
                None => o.push_str(&format!("{},{},{},0,0,,,,,\n", s.name, t.name, bin)),
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

/// The whole report as text. `header` is the run's own lines (inputs, hashes, loading counts),
/// printed first as `name: value`.
pub fn render_text(header: &[(String, String)], cfg: &Config, systems: &[SystemReport]) -> String {
    let mut o =
        String::from("oa-legacy-traj: legacy trajectory prediction against ADS-B truth\n\n");
    for (k, v) in header {
        o.push_str(&format!("{k}: {v}\n"));
    }
    o.push_str(&format!(
        "\nsettings\n  crossing: time of closest approach of the truth path to the point, counted only within {:.2} NM\n  landing: last airborne report before the first ground report, within {:.1} NM of the airport reference point, gap to the ground report at most {} s\n  error: estimate minus actual, seconds (positive: the estimate is later than what happened)\n  look-ahead: actual time minus the time of the message that carried the estimate, bins {}\n  statistics: median, mean, 5th and 95th percentile (linear interpolation between ranks) and root mean square of the error, in seconds; n is scored estimates, flights the distinct flights they came from\n",
        cfg.cut_m / NM_M,
        cfg.runway_radius_m / NM_M,
        cfg.landing_gap_max_ms / 1000,
        BIN_NAMES.join(", ")
    ));
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
            o.push_str("error by look-ahead\n");
            for (bin, e, flights) in bin_rows(t) {
                match stats(&e) {
                    Some(st) => o.push_str(&format!(
                        "  {bin}: n={} flights={flights} median={} mean={} p5={} p95={} rms={}\n",
                        st.n,
                        cell(st.median),
                        cell(st.mean),
                        cell(st.p5),
                        cell(st.p95),
                        cell(st.rms)
                    )),
                    None => o.push_str(&format!("  {bin}: n=0\n")),
                }
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

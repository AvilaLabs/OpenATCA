//! What a run counts, and the text and CSV it is written as. Core (pure): the same counts give
//! the same bytes. Every number is printed with its n; an exclusion is counted under one reason.

use std::collections::BTreeMap;

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

/// Number of row classes: address or callsign joined, airborne at issue or not.
pub const GROUPS: usize = 4;
/// Titles of the row classes, indexed by [`group_of`]. The first is the headline.
pub const GROUP_NAMES: [&str; GROUPS] = [
    "HEADLINE: address-joined, airborne at issue",
    "PRE-DEPARTURE: address-joined, not airborne at issue (includes departure delay)",
    "CALLSIGN-JOINED, airborne at issue (not in the headline)",
    "CALLSIGN-JOINED, not airborne at issue (includes departure delay; not in the headline)",
];

/// The class of a row: 0 headline, 1 pre-departure, 2 callsign airborne, 3 callsign pre-departure.
pub fn group_of(by_callsign: bool, airborne: bool) -> usize {
    usize::from(by_callsign) * 2 + usize::from(!airborne)
}

/// The scored rows of one class, by the bin of the stated lead.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Group {
    /// Error in seconds per bin of the stated lead (the estimate minus the issue time).
    pub errors: Vec<Vec<f64>>,
    /// The flight each error belongs to, per bin, in the same order.
    pub units: Vec<Vec<u32>>,
    /// Error in seconds per bin of the actual time minus the issue time (a sensitivity).
    pub errors_act: Vec<Vec<f64>>,
    /// Rows per bin whose event happened before the message was issued (kept in `errors`).
    pub before_issue: Vec<u64>,
}

/// The scored errors and the exclusions of one kind of estimate.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct KindTable {
    pub name: String,
    /// Estimates offered, scored or not.
    pub estimates: u64,
    pub exclusions: [u64; EXCLUSION_COUNT],
    /// The rows by class ([`GROUP_NAMES`]).
    pub groups: Vec<Group>,
    /// Estimates whose point or airport name had several places and one was chosen.
    pub resolved_ambiguous: u64,
    /// Headline rows only from here: error against the TFMS actual arrival time instead of the
    /// landing, per bin of the stated lead.
    pub errors_alt: Vec<Vec<f64>>,
    /// Seconds from the last airborne report to the first ground report of each landing used.
    pub gaps_s: Vec<f64>,
    /// The touchdown time used minus the TFMS actual arrival, seconds.
    pub landing_minus_actual_s: Vec<f64>,
    /// Error with touchdown at the last airborne report, and at the first ground report, per bin.
    pub td_air: Vec<Vec<f64>>,
    pub td_gnd: Vec<Vec<f64>>,
    /// Keep the last message of each (flight, bin): headline rows only.
    pub track_last: bool,
    /// (flight, bin) to the issue time of the last message and the errors of its rows.
    pub last_in_bin: BTreeMap<(u32, usize), (i64, Vec<f64>)>,
    /// Crossing-cut sensitivity: label to the headline errors per bin.
    pub sens: BTreeMap<String, Vec<Vec<f64>>>,
    /// Headline-class crossings whose closest approach was from 2 NM up to the largest cut: the
    /// errors per bin.
    pub abeam: Vec<Vec<f64>>,
    /// Estimates offered per bin of the stated lead; the last slot is those with no lead in range
    /// (no time, before the issue, or past the last bin).
    pub offered_bin: Vec<u64>,
    /// Of them, per bin and by why there was no leg: none of the aircraft's, none the flight's
    /// own, and the departure past the window.
    pub no_leg_bin: Vec<u64>,
    pub not_own_bin: Vec<u64>,
    pub too_far_bin: Vec<u64>,
    /// Exclusions whose bound leg ended at the clip box edge, by reason ([`EXCLUSION_NAMES`]).
    pub at_edge: [u64; EXCLUSION_COUNT],
    /// Exclusions by reason ([`EXCLUSION_NAMES`]) and bin of the stated lead (the last slot: no
    /// lead in range).
    pub excl_bin: Vec<Vec<u64>>,
    /// Headline errors per bin, without the rows whose place was resolved by the flown path.
    pub no_flown: Vec<Vec<f64>>,
    /// Per arrival airport: estimates offered and scored.
    pub by_airport: BTreeMap<String, (u64, u64)>,
}

/// What a scored sample carries besides its error (all diagnostics, for headline rows).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extra {
    /// The gap of the landing used, seconds.
    pub gap_s: Option<f64>,
    /// The error against the TFMS actual arrival, and the touchdown minus that arrival, seconds.
    pub alt: Option<(f64, f64)>,
    /// The error with touchdown at the last airborne report, and at the first ground report.
    pub td: Option<(f64, f64)>,
    /// The issue time of the message, Unix ms.
    pub issue_ms: i64,
    /// The bin of the stated lead, when the estimate is excluded (a scored one carries it).
    pub lead_bin: Option<usize>,
    /// The bound leg ended at the clip box edge, not inside it (for the landing exclusions).
    pub end_at_edge: bool,
    /// The place was one of several, resolved against the flown path.
    pub flown: bool,
}

/// The exclusions that say the bound leg did not land at the airport, split by where the leg ended.
const LANDING_REASONS: [Exclusion; 4] = [
    Exclusion::NoGroundCoverage,
    Exclusion::NoLanding,
    Exclusion::TouchAndGo,
    Exclusion::Diversion,
];

/// An empty table.
pub fn kind_new(name: &str) -> KindTable {
    let per_bin = || vec![Vec::<f64>::new(); BIN_COUNT];
    KindTable {
        name: name.to_string(),
        estimates: 0,
        exclusions: [0; EXCLUSION_COUNT],
        groups: (0..GROUPS)
            .map(|_| Group {
                errors: per_bin(),
                units: vec![Vec::new(); BIN_COUNT],
                errors_act: per_bin(),
                before_issue: vec![0; BIN_COUNT],
            })
            .collect(),
        resolved_ambiguous: 0,
        errors_alt: per_bin(),
        gaps_s: Vec::new(),
        landing_minus_actual_s: Vec::new(),
        td_air: per_bin(),
        td_gnd: per_bin(),
        track_last: true,
        last_in_bin: BTreeMap::new(),
        sens: BTreeMap::new(),
        abeam: per_bin(),
        offered_bin: vec![0; BIN_COUNT + 1],
        no_leg_bin: vec![0; BIN_COUNT + 1],
        not_own_bin: vec![0; BIN_COUNT + 1],
        too_far_bin: vec![0; BIN_COUNT + 1],
        at_edge: [0; EXCLUSION_COUNT],
        excl_bin: vec![vec![0; BIN_COUNT + 1]; EXCLUSION_COUNT],
        no_flown: per_bin(),
        by_airport: BTreeMap::new(),
    }
}

/// Counts one estimate of flight `unit` in class `group`: into its bin, or under its exclusion.
pub fn kind_record(
    t: &mut KindTable,
    unit: u32,
    group: usize,
    r: Result<Sample, Exclusion>,
    x: Extra,
) {
    t.estimates += 1;
    let slot = match &r {
        Ok(s) => s.bin,
        Err(_) => x.lead_bin.unwrap_or(BIN_COUNT),
    };
    if let Some(c) = t.offered_bin.get_mut(slot) {
        *c += 1;
    }
    let s = match r {
        Ok(s) => s,
        Err(e) => {
            if let Some(c) = t.exclusions.get_mut(exclusion_index(e)) {
                *c += 1;
            }
            if let Some(c) = t
                .excl_bin
                .get_mut(exclusion_index(e))
                .and_then(|v| v.get_mut(slot))
            {
                *c += 1;
            }
            let miss = match e {
                Exclusion::NoLeg => Some(&mut t.no_leg_bin),
                Exclusion::LegNotOwn => Some(&mut t.not_own_bin),
                Exclusion::DepartureTooFar => Some(&mut t.too_far_bin),
                _ => None,
            };
            if let Some(c) = miss.and_then(|v| v.get_mut(slot)) {
                *c += 1;
            }
            if x.end_at_edge && LANDING_REASONS.contains(&e) {
                if let Some(c) = t.at_edge.get_mut(exclusion_index(e)) {
                    *c += 1;
                }
            }
            return;
        }
    };
    if let Some(g) = t.groups.get_mut(group) {
        if let (Some(e), Some(u)) = (g.errors.get_mut(s.bin), g.units.get_mut(s.bin)) {
            e.push(s.error_s);
            u.push(unit);
        }
        if let Some(e) = s.act_bin.and_then(|b| g.errors_act.get_mut(b)) {
            e.push(s.error_s);
        }
        if s.before_issue {
            if let Some(c) = g.before_issue.get_mut(s.bin) {
                *c += 1;
            }
        }
    }
    if group != 0 {
        return;
    }
    if !x.flown {
        if let Some(e) = t.no_flown.get_mut(s.bin) {
            e.push(s.error_s);
        }
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
    if let Some((air, gnd)) = x.td {
        if let Some(e) = t.td_air.get_mut(s.bin) {
            e.push(air);
        }
        if let Some(e) = t.td_gnd.get_mut(s.bin) {
            e.push(gnd);
        }
    }
    if t.track_last {
        let slot = t
            .last_in_bin
            .entry((unit, s.bin))
            .or_insert((x.issue_ms, Vec::new()));
        if x.issue_ms > slot.0 {
            *slot = (x.issue_ms, Vec::new());
        }
        if x.issue_ms == slot.0 {
            slot.1.push(s.error_s);
        }
    }
}

/// Counts one estimate offered against the airport it lands at.
pub fn airport_offer(t: &mut KindTable, airport: &str) {
    t.by_airport.entry(airport.to_string()).or_default().0 += 1;
}

/// Counts one estimate scored against the airport it lands at.
pub fn airport_scored(t: &mut KindTable, airport: &str) {
    t.by_airport.entry(airport.to_string()).or_default().1 += 1;
}

/// Adds a headline crossing whose closest approach was beyond the headline cut to the abeam table.
pub fn abeam_add(t: &mut KindTable, s: &Sample) {
    if let Some(v) = t.abeam.get_mut(s.bin) {
        v.push(s.error_s);
    }
}

/// Adds a headline error to a sensitivity table (for example, one crossing cut).
pub fn sens_add(t: &mut KindTable, label: &str, r: &Result<Sample, Exclusion>) {
    if let Ok(s) = r {
        let e = t
            .sens
            .entry(label.to_string())
            .or_insert_with(|| vec![Vec::new(); BIN_COUNT]);
        if let Some(v) = e.get_mut(s.bin) {
            v.push(s.error_s);
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
    /// Named counts of the run (joins refused, legs, landings), in key order.
    pub counters: BTreeMap<String, u64>,
}

/// Adds `n` to the named count of a system.
pub fn count(s: &mut SystemReport, key: &str, n: u64) {
    *s.counters.entry(key.to_string()).or_default() += n;
}

/// The distinct flights with at least one scored row in any class or kind.
pub fn scored_flights(s: &SystemReport) -> u64 {
    let mut u: Vec<u32> = s
        .kinds
        .iter()
        .flat_map(|k| k.groups.iter())
        .flat_map(|g| g.units.iter().flatten().copied())
        .collect();
    u.sort_unstable();
    u.dedup();
    u.len() as u64
}

/// An empty system report with the given kinds.
pub fn system_new(name: &str, kinds: &[&str]) -> SystemReport {
    SystemReport {
        name: name.to_string(),
        kinds: kinds.iter().map(|k| kind_new(k)).collect(),
        outcomes: [0; OUTCOME_COUNT],
        closest: [0; DIST_BUCKETS],
        counters: BTreeMap::new(),
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

/// The bins of a class as rows: the name, the errors and the flights of each bin, then the rows
/// of all bins together.
fn bin_rows(g: &Group) -> Vec<(String, Vec<f64>, u64)> {
    let mut rows: Vec<(String, Vec<f64>, u64)> = Vec::new();
    let mut all: Vec<f64> = Vec::new();
    let mut all_units: Vec<u32> = Vec::new();
    for k in 0..BIN_COUNT {
        let e = g.errors.get(k).cloned().unwrap_or_default();
        let u = g.units.get(k).cloned().unwrap_or_default();
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

/// The smallest sample whose tail percentiles (p95, p99) are printed.
pub const TAIL_MIN_N: u64 = 100;

fn tail(st: &crate::core::bins::Stats, v: f64) -> String {
    if st.n >= TAIL_MIN_N {
        cell(v)
    } else {
        "n<100".to_string()
    }
}

fn stat_line(label: &str, e: &[f64], flights: Option<u64>) -> String {
    match stats(e) {
        Some(st) => {
            let f = flights.map_or(String::new(), |f| format!(" flights={f}"));
            let big = large(e);
            format!(
                "  {label}: n={}{f} |e| median={} p95={} p99={}; bias median={} mean={}; spread p5={} p95={}; rms={}; over {}s: n={big} ({:.1}%)\n",
                st.n,
                cell(st.abs_median),
                tail(&st, st.abs_p95),
                tail(&st, st.abs_p99),
                cell(st.median),
                cell(st.mean),
                tail(&st, st.p5),
                tail(&st, st.p95),
                cell(st.rms),
                LARGE_ERROR_S as i64,
                100.0 * big as f64 / st.n as f64
            )
        }
        None => format!("  {label}: n=0\n"),
    }
}

/// The per-bin lines of a list of errors by bin.
fn bins_text(by_bin: &[Vec<f64>]) -> String {
    let mut o = String::new();
    for k in 0..BIN_COUNT {
        let e = by_bin.get(k).map_or(&[][..], Vec::as_slice);
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

/// A tail percentile for the CSV: empty below [`TAIL_MIN_N`] values.
fn csv_tail(st: &crate::core::bins::Stats, v: f64) -> String {
    if st.n >= TAIL_MIN_N {
        cell(v)
    } else {
        String::new()
    }
}

/// The statistics rows of a system as CSV: one row per kind, class and bin, and a last row per
/// kind and class for all bins. `n` is the number of scored estimates, `flights` the number of
/// flights they came from.
pub fn render_csv(s: &SystemReport) -> String {
    let mut o = String::from(
        "system,kind,block,bin,n,flights,abs_median_s,abs_p95_s,abs_p99_s,bias_median_s,bias_mean_s,p5_s,p95_s,rms_s,over_1800s\n",
    );
    for t in &s.kinds {
        for (gi, g) in t.groups.iter().enumerate() {
            for (bin, e, flights) in bin_rows(g) {
                match stats(&e) {
                    Some(st) => o.push_str(&format!(
                        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                        s.name,
                        t.name,
                        gi,
                        bin,
                        st.n,
                        flights,
                        cell(st.abs_median),
                        csv_tail(&st, st.abs_p95),
                        csv_tail(&st, st.abs_p99),
                        cell(st.median),
                        cell(st.mean),
                        csv_tail(&st, st.p5),
                        csv_tail(&st, st.p95),
                        cell(st.rms),
                        large(&e)
                    )),
                    None => o.push_str(&format!(
                        "{},{},{},{},0,0,,,,,,,,,\n",
                        s.name, t.name, gi, bin
                    )),
                }
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
    for (k, n) in &s.counters {
        o.push_str(&format!("{},count,{},{}\n", s.name, k, n));
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
        "\nsettings\n  crossing: time of closest approach of the truth path to the point, counted only within {:.2} NM (sensitivity at the cuts listed per kind)\n  landing: touchdown is the middle of the gap between the last airborne and the first ground report (gap at most {} s); the last airborne report is within {:.1} NM of the airport reference point, below the airport elevation plus {:.0} ft or below {:.0} m/s, and no airborne report follows within {} s of the last airborne report\n  leg: each message binds one truth leg of the flight's own. In progress at the issue: it binds only if the aircraft is airborne on it (the issue is between its first airborne report and its landing, or its last report), it lands after the issue, and it took off within {:.0} NM of the departure airport or lands within {:.0} NM of the arrival airport, or, with neither airport known, carries the flight's callsign; a leg that had landed before the issue never binds. Else the first leg not yet airborne at the issue starting within {} h after it whose take-off airport is the departure airport, or, with the departure airport unknown or not in NASR, whose callsign is the flight's\n  airborne at issue: the issue time is between the bound leg's first airborne report and its landing (or last report)\n  error: estimate minus actual, seconds (positive: the estimate is later than what happened)\n  bins: by the stated lead, the estimate minus the time of the message that carried it: {}\n  statistics: |e| median, p95, p99 are nearest rank over absolute errors, as oa-bench defines them; bias is the signed median and mean; spread is the signed 5th and 95th percentile (nearest rank); rms and the share over {} s follow; n is scored estimates, flights the distinct flights they came from\n",
        cfg.cut_m / NM_M,
        cfg.landing_gap_max_ms / 1000,
        cfg.runway_radius_m / NM_M,
        cfg.landing_alt_ft,
        cfg.landing_speed_ms,
        cfg.landing_persist_ms / 1000,
        cfg.leg_airport_m / NM_M,
        cfg.leg_airport_m / NM_M,
        cfg.leg_window_ms / 3_600_000,
        BIN_NAMES.join(", "),
        LARGE_ERROR_S as i64
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
        if !s.counters.is_empty() {
            o.push_str("\ncounts\n");
            for (k, n) in &s.counters {
                o.push_str(&format!("  {k}: n={n}\n"));
            }
        }
        o.push_str("\nclosest approach of the truth path to each point searched (joined flights with truth)\n");
        let searched: u64 = s.closest.iter().sum();
        for (k, n) in s.closest.iter().enumerate() {
            o.push_str(&format!("  {}: n={n}\n", bucket_name(k)));
        }
        o.push_str(&format!("  all searched: n={searched}\n"));
        for t in &s.kinds {
            o.push_str(&format!("\n-- {} --\n", t.name));
            if t.name.starts_with("sta_") {
                o.push_str("schedule target (metering), not a forecast: scored against what happened for reference only\n");
            }
            o.push_str(&format!("estimates offered: n={}\n", t.estimates));
            o.push_str("estimates by bin of the stated lead: offered, with no leg (of which no leg of the flight's own; departure past the window), scored\n");
            for k in 0..=BIN_COUNT {
                let at = |v: &[u64]| v.get(k).copied().unwrap_or(0);
                let scored: u64 = t
                    .groups
                    .iter()
                    .map(|g| g.errors.get(k).map_or(0, |e| e.len() as u64))
                    .sum();
                o.push_str(&format!(
                    "  {}: offered={} no leg={} (no leg of the flight's own={}; departure past the window={}) scored={}\n",
                    BIN_NAMES.get(k).copied().unwrap_or("no stated lead in range"),
                    at(&t.offered_bin),
                    at(&t.no_leg_bin) + at(&t.not_own_bin) + at(&t.too_far_bin),
                    at(&t.not_own_bin),
                    at(&t.too_far_bin),
                    scored
                ));
            }
            o.push_str(&format!(
                "estimates whose name had several places, nearest the route taken: n={}\n",
                t.resolved_ambiguous
            ));
            o.push_str("disclosure: share of the offered rows of each bin excluded as truth path never within the cut, passage at an edge, and no ground report\n");
            for k in 0..=BIN_COUNT {
                let offered = t.offered_bin.get(k).copied().unwrap_or(0);
                let share = |x: Exclusion| {
                    let n = t
                        .excl_bin
                        .get(exclusion_index(x))
                        .and_then(|v| v.get(k))
                        .copied()
                        .unwrap_or(0);
                    let pct = if offered == 0 {
                        0.0
                    } else {
                        100.0 * n as f64 / offered as f64
                    };
                    format!("{n} ({pct:.1}%)")
                };
                o.push_str(&format!(
                    "  {}: offered={offered} never within the cut={} at an edge={} no ground report={}\n",
                    BIN_NAMES.get(k).copied().unwrap_or("no stated lead in range"),
                    share(Exclusion::BeyondCut),
                    share(Exclusion::PassageAtEdge),
                    share(Exclusion::NoGroundCoverage)
                ));
            }
            for (gi, title) in GROUP_NAMES.iter().enumerate() {
                let Some(g) = t.groups.get(gi) else { continue };
                o.push_str(&format!(
                    "{title}\n  by the estimate's stated lead (estimate minus issue time); rows whose event was before the issue are kept\n"
                ));
                for (k, (bin, e, flights)) in bin_rows(g).into_iter().enumerate() {
                    let kept = g.before_issue.get(k).copied();
                    let tail = kept.map_or_else(
                        || {
                            format!(
                                " before issue kept: n={}",
                                g.before_issue.iter().sum::<u64>()
                            )
                        },
                        |n| format!(" before issue kept: n={n}"),
                    );
                    let line = stat_line(&bin, &e, Some(flights));
                    o.push_str(line.trim_end_matches('\n'));
                    o.push_str(&tail);
                    o.push('\n');
                }
            }
            if let Some(g) = t.groups.first() {
                o.push_str("sensitivity of the headline: binned by the actual time minus the issue time instead\n");
                o.push_str(&bins_text(&g.errors_act));
                o.push_str("headline error histogram by stated lead, counts per bucket of error in seconds\n");
                for k in 0..BIN_COUNT {
                    let e = g.errors.get(k).map_or(&[][..], Vec::as_slice);
                    o.push_str(&format!(
                        "  {}: n={} {}\n",
                        BIN_NAMES.get(k).copied().unwrap_or("?"),
                        e.len(),
                        histogram(e)
                    ));
                }
            }
            if t.track_last {
                o.push_str("headline, one message per flight and stated-lead bin: the last message of the flight in the bin (all its rows)\n");
                let mut by_bin: Vec<Vec<f64>> = vec![Vec::new(); BIN_COUNT];
                let mut flights: Vec<Vec<u32>> = vec![Vec::new(); BIN_COUNT];
                for ((unit, bin), (_, errs)) in &t.last_in_bin {
                    if let Some(v) = by_bin.get_mut(*bin) {
                        v.extend_from_slice(errs);
                    }
                    if let Some(v) = flights.get_mut(*bin) {
                        v.push(*unit);
                    }
                }
                for k in 0..BIN_COUNT {
                    let e = by_bin.get(k).map_or(&[][..], Vec::as_slice);
                    let f = flights.get(k).map_or(0, |u| units_in(u));
                    o.push_str(&stat_line(
                        BIN_NAMES.get(k).copied().unwrap_or("?"),
                        e,
                        Some(f),
                    ));
                }
            }
            if t.groups
                .first()
                .is_some_and(|g| g.errors.iter().any(|e| !e.is_empty()))
            {
                o.push_str("sensitivity of the headline: without the rows whose place was one of several resolved against the flown path\n");
                o.push_str(&bins_text(&t.no_flown));
            }
            if t.abeam.iter().any(|e| !e.is_empty()) {
                o.push_str("headline crossings abeam: closest approach from the headline cut up to the largest cut, by stated lead (not in the headline)\n");
                o.push_str(&bins_text(&t.abeam));
            }
            for (label, by_bin) in &t.sens {
                o.push_str(&format!("sensitivity of the headline: {label}\n"));
                o.push_str(&bins_text(by_bin));
            }
            if t.errors_alt.iter().any(|e| !e.is_empty()) {
                o.push_str("headline error against the TFMS actual arrival time instead of the touchdown, by stated lead\n");
                o.push_str(&bins_text(&t.errors_alt));
            }
            if t.td_air.iter().any(|e| !e.is_empty()) {
                o.push_str("sensitivity of the headline: touchdown at the last airborne report\n");
                o.push_str(&bins_text(&t.td_air));
                o.push_str("sensitivity of the headline: touchdown at the first ground report\n");
                o.push_str(&bins_text(&t.td_gnd));
            }
            if !t.gaps_s.is_empty() {
                o.push_str("gap from the last airborne report to the first ground report of the landings used (headline), seconds\n");
                o.push_str(&stat_line("all landings used", &t.gaps_s, None));
            }
            if !t.landing_minus_actual_s.is_empty() {
                o.push_str(
                    "touchdown (middle of the gap) minus the TFMS actual arrival time (headline), seconds\n",
                );
                o.push_str(&stat_line(
                    "samples with an actual arrival",
                    &t.landing_minus_actual_s,
                    None,
                ));
            }
            o.push_str("excluded, by reason\n");
            let excluded: u64 = t.exclusions.iter().sum();
            for (k, (name, n)) in EXCLUSION_NAMES.iter().zip(t.exclusions).enumerate() {
                if n > 0 {
                    let edge = t.at_edge.get(k).copied().unwrap_or(0);
                    if LANDING_REASONS.iter().any(|&x| exclusion_index(x) == k) {
                        o.push_str(&format!(
                            "  {name}: n={n} (bound leg ends inside the box: {}; at its edge: {edge})\n",
                            n - edge
                        ));
                    } else {
                        o.push_str(&format!("  {name}: n={n}\n"));
                    }
                }
            }
            let scored: u64 = t
                .groups
                .iter()
                .map(|g| g.errors.iter().map(|e| e.len() as u64).sum::<u64>())
                .sum();
            o.push_str(&format!(
                "  excluded in all: n={excluded}; scored: n={scored}; offered: n={}\n",
                t.estimates
            ));
            if !t.by_airport.is_empty() {
                o.push_str("scored share per arrival airport, the 30 with the most estimates offered (offered, scored, share)\n");
                let mut v: Vec<(&String, &(u64, u64))> = t.by_airport.iter().collect();
                v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(b.0)));
                for (apt, (offered, ok)) in v.into_iter().take(30) {
                    o.push_str(&format!(
                        "  {apt}: offered={offered} scored={ok} share={:.1}%\n",
                        100.0 * *ok as f64 / *offered as f64
                    ));
                }
            }
        }
    }
    o
}

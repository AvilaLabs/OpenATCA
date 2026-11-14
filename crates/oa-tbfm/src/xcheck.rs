//! Cross-check of decoded rows against the M0 airspace model: which meter fix and airport
//! identifiers in the feed name something the model has. Shell/offline code (it allocates
//! freely, RULESET C6); a pure function of the rows and the model.
//!
//! The model is for one ARTCC, and the TBFM feed carries every centre, so two scopes are
//! reported: every row, and only the rows whose centre (`envSrce`) is the model's ARTCC.
//! Only the second is expected to resolve nearly completely.
//!
//! * A meter fix (`mfx`) resolves when the model has a fix, navaid or terminal waypoint with
//!   that identifier (any region).
//! * An airport (`apt`, the metered/arrival airport, and `dap`, the departure airport)
//!   resolves when the model has an airport whose identifier or alternate identifier is the
//!   value (the feed uses the FAA identifier for most airports and ICAO for some).

use crate::rows::{name_str, rows_len, Name4, Rows};
use oa_airspace::model::{point_kind, str_or_empty, Model};
use std::collections::BTreeMap;

/// Counts for one identifier role in one scope.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Distinct non-empty identifiers.
    pub distinct: u32,
    /// Of those, how many the model has.
    pub resolved: u32,
    /// Rows with a non-empty value.
    pub rows: u32,
    /// Of those, rows whose value the model has.
    pub rows_resolved: u32,
}

/// The three roles in one scope, with the distinct identifiers that did not resolve.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub meter_fix: Tally,
    pub arrival_airport: Tally,
    pub departure_airport: Tally,
    pub unresolved_fixes: Vec<String>,
    pub unresolved_arrival: Vec<String>,
    pub unresolved_departure: Vec<String>,
}

/// The result of [`cross_check`].
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrossCheck {
    /// Every row.
    pub all: Scope,
    /// Rows from the model's own ARTCC only.
    pub own: Scope,
}

fn has(sorted: &[&str], want: &str) -> bool {
    sorted.binary_search(&want).is_ok()
}

/// Counts for one role. `values` maps an identifier to (rows in all scope, rows in own scope).
fn tally(values: &BTreeMap<String, (u32, u32)>, known: &[&str], own: bool) -> (Tally, Vec<String>) {
    let mut t = Tally::default();
    let mut missing = Vec::new();
    for (name, &(all_rows, own_rows)) in values {
        let n = if own { own_rows } else { all_rows };
        if n == 0 {
            continue;
        }
        t.distinct += 1;
        t.rows += n;
        if has(known, name) {
            t.resolved += 1;
            t.rows_resolved += n;
        } else {
            missing.push(name.clone());
        }
    }
    (t, missing)
}

fn collect<const N: usize>(
    col: &[[u8; N]],
    own_center: &[Name4],
    artcc: &Name4,
) -> BTreeMap<String, (u32, u32)> {
    let mut m: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for (v, c) in col.iter().zip(own_center) {
        let s = name_str(v);
        if s.is_empty() {
            continue;
        }
        let e = m.entry(s.to_string()).or_insert((0, 0));
        e.0 += 1;
        if c == artcc {
            e.1 += 1;
        }
    }
    m
}

/// Resolves the rows' meter fixes and airports against `model`.
pub fn cross_check(model: &Model, rows: &Rows) -> CrossCheck {
    let p = &model.points;
    let st = &model.strings;
    let mut fixes: Vec<&str> = Vec::new();
    let mut airports: Vec<&str> = Vec::new();
    for (i, &k) in p.kind.iter().enumerate() {
        let id = p.ident.get(i).map_or("", |&s| str_or_empty(st, s));
        match k {
            point_kind::FIX..=point_kind::NDB | point_kind::TERMINAL_WAYPOINT => fixes.push(id),
            point_kind::AIRPORT => {
                airports.push(id);
                let alt = p.alt_ident.get(i).map_or("", |&s| str_or_empty(st, s));
                if !alt.is_empty() {
                    airports.push(alt);
                }
            }
            _ => {}
        }
    }
    fixes.sort_unstable();
    airports.sort_unstable();

    let mut artcc: Name4 = [0; 4];
    for (d, &b) in artcc.iter_mut().zip(model.meta.artcc.as_bytes()) {
        *d = b;
    }
    debug_assert_eq!(rows.center.len(), rows_len(rows));
    let mfx = collect::<8>(&rows.mfx, &rows.center, &artcc);
    let apt = collect::<4>(&rows.apt, &rows.center, &artcc);
    let dap = collect::<4>(&rows.dap, &rows.center, &artcc);

    let mut out = CrossCheck::default();
    for (own, scope) in [(false, &mut out.all), (true, &mut out.own)] {
        (scope.meter_fix, scope.unresolved_fixes) = tally(&mfx, &fixes, own);
        (scope.arrival_airport, scope.unresolved_arrival) = tally(&apt, &airports, own);
        (scope.departure_airport, scope.unresolved_departure) = tally(&dap, &airports, own);
    }
    out
}

/// One line per role for a report.
pub fn format_cross_check(c: &CrossCheck) -> String {
    fn line(label: &str, t: &Tally) -> String {
        format!(
            "  {label}: {}/{} distinct resolve, {}/{} rows\n",
            t.resolved, t.distinct, t.rows_resolved, t.rows
        )
    }
    let mut s = String::new();
    for (name, sc) in [("all centres", &c.all), ("model ARTCC only", &c.own)] {
        s.push_str(name);
        s.push_str(":\n");
        s.push_str(&line("meter fix", &sc.meter_fix));
        s.push_str(&line("arrival airport", &sc.arrival_airport));
        s.push_str(&line("departure airport", &sc.departure_airport));
    }
    s
}

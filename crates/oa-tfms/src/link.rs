//! Linkage between SFDPS rows (oa-fixm) and TFMS rows. Shell/offline (it allocates its working
//! sets, RULESET C6); a pure function of the two row sets.
//!
//! What links the two feeds (docs/TFMS_SURVEY.md): the SFDPS `gufi` is a `urn:uuid` UUID and TFMS
//! has no UUID, so the two `gufi` elements cannot be compared. The 10-character TFMS `gufi`
//! equals the SFDPS `flightPlan@identifier` (the plan identifier, [`oa_fixm::Field::FlightPlanId`]):
//! that is the "GUFI" match reported here. The fallback for TFMS flights that never carry a
//! `gufi` is aircraft identification plus departure plus arrival. A TFMS flight is one distinct
//! `flightRef`; an SFDPS flight is one distinct UUID `gufi`.
//!
//! The report also checks the plan-identifier hypothesis against the fallback: for flights found
//! both ways, the SFDPS flight must be the same one.

use crate::rows::{flights_len, name_str, Rows};
use oa_fixm::{code_str, has_field, Events, Field};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The result of [`link_flights`].
// A7.2: cold struct, size guard waived (one per report)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkReport {
    pub sfdps_rows: u64,
    /// Distinct SFDPS UUID `gufi`s.
    pub sfdps_flights: u64,
    /// SFDPS flights that carry a flight plan identifier.
    pub sfdps_with_plan_id: u64,
    pub tfms_rows: u64,
    /// Distinct TFMS flight reference numbers.
    pub tfms_flights: u64,
    pub tfms_with_gufi: u64,
    pub tfms_without_gufi: u64,
    /// TFMS `gufi`s shaped like a UUID (the only form that could equal an SFDPS `gufi`).
    pub tfms_uuid_shaped: u64,
    /// Of those, how many equal an SFDPS `gufi`.
    pub uuid_matched: u64,
    /// TFMS flights with a `gufi` that equals an SFDPS flight plan identifier.
    pub by_gufi: u64,
    /// TFMS flights without a `gufi` found by acid, departure and arrival.
    pub by_acid_no_gufi: u64,
    /// TFMS flights without a `gufi` that lack a departure or an arrival (no fallback key).
    pub no_gufi_no_key: u64,
    /// TFMS flights with a `gufi` that are also found by acid, departure and arrival.
    pub gufi_flights_also_by_acid: u64,
    /// Of those found both ways, the SFDPS flight is the same one.
    pub agree: u64,
    /// Found both ways, different SFDPS flights.
    pub disagree: u64,
    /// TFMS flights with a `gufi` that no plan identifier matches but acid, departure and
    /// arrival do.
    pub acid_only_among_gufi: u64,
    /// Acid, departure and arrival keys that name more than one SFDPS flight.
    pub ambiguous_acid_keys: u64,
}

/// Aircraft identification, departure and arrival: the fallback key.
type Key = (Vec<u8>, Vec<u8>, Vec<u8>);

/// What one TFMS flight is known by, folded over its rows.
#[derive(Default)]
struct Flight {
    gufi: Vec<u8>,
    acid: Vec<u8>,
    dep: Vec<u8>,
    arr: Vec<u8>,
}

fn name_vec(n: &[u8]) -> Vec<u8> {
    name_str(n).as_bytes().to_vec()
}

fn is_uuid_shape(s: &[u8]) -> bool {
    s.len() == 36
        && s.iter().enumerate().all(|(i, &b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

/// Links the TFMS flights in `rows` to the SFDPS flights in `ev`. See the module comment.
pub fn link_flights(ev: &Events, rows: &Rows) -> LinkReport {
    let mut rep = LinkReport {
        sfdps_rows: ev.len as u64,
        tfms_rows: flights_len(&rows.flights) as u64,
        ..LinkReport::default()
    };

    // SFDPS indexes: plan identifier -> flights, acid+dep+arr -> flights, all UUIDs.
    let mut sf_all: HashSet<u128> = HashSet::new();
    let mut by_plan: HashMap<Vec<u8>, BTreeSet<u128>> = HashMap::new();
    let mut by_key: HashMap<Key, BTreeSet<u128>> = HashMap::new();
    let mut with_plan: HashSet<u128> = HashSet::new();
    for i in 0..ev.len {
        let Some(&g) = ev.gufi.get(i) else { continue };
        sf_all.insert(g);
        if has_field(ev, i, Field::FlightPlanId) {
            if let Some(p) = ev.plan_id.get(i) {
                by_plan
                    .entry(code_str(p).as_bytes().to_vec())
                    .or_default()
                    .insert(g);
                with_plan.insert(g);
            }
        }
        if has_field(ev, i, Field::Callsign)
            && has_field(ev, i, Field::Departure)
            && has_field(ev, i, Field::Arrival)
        {
            if let (Some(c), Some(d), Some(a)) =
                (ev.callsign.get(i), ev.departure.get(i), ev.arrival.get(i))
            {
                by_key
                    .entry((
                        code_str(c).as_bytes().to_vec(),
                        code_str(d).as_bytes().to_vec(),
                        code_str(a).as_bytes().to_vec(),
                    ))
                    .or_default()
                    .insert(g);
            }
        }
    }
    rep.sfdps_flights = sf_all.len() as u64;
    rep.sfdps_with_plan_id = with_plan.len() as u64;
    rep.ambiguous_acid_keys = by_key.values().filter(|s| s.len() > 1).count() as u64;

    // TFMS flights, one per flight reference, folded over their rows.
    let fl = &rows.flights;
    let mut flights: BTreeMap<u64, Flight> = BTreeMap::new();
    for i in 0..flights_len(fl) {
        let (Some(&r), Some(acid), Some(gufi), Some(dep), Some(arr)) = (
            fl.flight_ref.get(i),
            fl.acid.get(i),
            fl.gufi.get(i),
            fl.dep.get(i),
            fl.arr.get(i),
        ) else {
            continue;
        };
        let f = flights.entry(r).or_default();
        for (dst, src) in [
            (&mut f.gufi, gufi.as_slice()),
            (&mut f.acid, acid.as_slice()),
            (&mut f.dep, dep.as_slice()),
            (&mut f.arr, arr.as_slice()),
        ] {
            if dst.is_empty() {
                *dst = name_vec(src);
            }
        }
    }
    rep.tfms_flights = flights.len() as u64;

    for f in flights.values() {
        let key_ok = !f.acid.is_empty() && !f.dep.is_empty() && !f.arr.is_empty();
        let key_hit = if key_ok {
            by_key.get(&(f.acid.clone(), f.dep.clone(), f.arr.clone()))
        } else {
            None
        };
        if f.gufi.is_empty() {
            rep.tfms_without_gufi += 1;
            if !key_ok {
                rep.no_gufi_no_key += 1;
            } else if key_hit.is_some() {
                rep.by_acid_no_gufi += 1;
            }
            continue;
        }
        rep.tfms_with_gufi += 1;
        if is_uuid_shape(&f.gufi) {
            rep.tfms_uuid_shaped += 1;
            let hit = oa_fixm::parse_uuid(&f.gufi).is_some_and(|u| sf_all.contains(&u));
            if hit {
                rep.uuid_matched += 1;
            }
        }
        let by_id = by_plan.get(&f.gufi);
        if by_id.is_some() {
            rep.by_gufi += 1;
        }
        if let Some(k) = key_hit {
            rep.gufi_flights_also_by_acid += u64::from(by_id.is_some());
            match by_id {
                Some(g) if g.iter().any(|x| k.contains(x)) => rep.agree += 1,
                Some(_) => rep.disagree += 1,
                None => rep.acid_only_among_gufi += 1,
            }
        }
    }
    rep
}

/// A multi-line text rendering of a [`LinkReport`], for logs and test output.
pub fn format_link_report(r: &LinkReport) -> String {
    let pct = |n: u64, d: u64| {
        if d == 0 {
            0.0
        } else {
            100.0 * n as f64 / d as f64
        }
    };
    let matched = r.by_gufi + r.by_acid_no_gufi;
    let mut s = String::new();
    s.push_str(&format!(
        "SFDPS: {} rows, {} flights ({} with a flight plan identifier)\n",
        r.sfdps_rows, r.sfdps_flights, r.sfdps_with_plan_id
    ));
    s.push_str(&format!(
        "TFMS: {} rows, {} flights ({} with a gufi, {} without)\n",
        r.tfms_rows, r.tfms_flights, r.tfms_with_gufi, r.tfms_without_gufi
    ));
    s.push_str(&format!(
        "  gufi shaped like a UUID: {} (equal to an SFDPS gufi: {})\n",
        r.tfms_uuid_shaped, r.uuid_matched
    ));
    s.push_str(&format!(
        "  matched by gufi = SFDPS plan identifier: {} of {} ({:.1} %)\n",
        r.by_gufi,
        r.tfms_with_gufi,
        pct(r.by_gufi, r.tfms_with_gufi)
    ));
    s.push_str(&format!(
        "  no gufi, matched by acid+departure+arrival: {} of {} ({:.1} %; {} lack a key)\n",
        r.by_acid_no_gufi,
        r.tfms_without_gufi,
        pct(r.by_acid_no_gufi, r.tfms_without_gufi),
        r.no_gufi_no_key
    ));
    s.push_str(&format!(
        "  total matched: {} of {} TFMS flights ({:.1} %)\n",
        matched,
        r.tfms_flights,
        pct(matched, r.tfms_flights)
    ));
    s.push_str(&format!(
        "  cross-check on flights with a gufi: found both ways {}: same SFDPS flight {}, \
         different {}; found only by acid+departure+arrival {}; ambiguous SFDPS keys {}\n",
        r.gufi_flights_also_by_acid,
        r.agree,
        r.disagree,
        r.acid_only_among_gufi,
        r.ambiguous_acid_keys
    ));
    s
}

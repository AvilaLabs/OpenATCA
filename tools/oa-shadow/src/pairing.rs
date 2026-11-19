//! Pairing ERAM flights with OpenATCA aircraft by one rule.
//!
//! The rule: an ERAM flight that carries a Mode S address OpenATCA has an aircraft for is paired
//! with that aircraft. Any other ERAM flight with a beacon code (the code in force at its first
//! report) is paired with the one OpenATCA aircraft that (a) is not already paired to an ERAM
//! flight whose span overlaps this one's, (b) was offered that squawk
//! within the flight's span (plus one segment gap either side), and (c) is nearest to ERAM's
//! first report of the flight, within the beacon pairing radius at that time; ties go to the
//! lowest address. Anything else is unpaired. Several ERAM flights (legs, duplicate tracks) may
//! pair with one aircraft; they form one group.

use std::collections::BTreeMap;

use crate::geo::{deg, enu_m, oa_at};
use crate::model::{EramFlight, OaAircraft};
use crate::thresholds::THRESHOLDS;

/// How a flight was paired.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    Address,
    BeaconProximity,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::Address => "address",
            Rule::BeaconProximity => "beacon+proximity",
        }
    }
}

/// One OpenATCA aircraft and the ERAM flights paired with it, `(index into the flights, rule)`.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub oa_addr: u32,
    pub eram: Vec<(usize, Rule)>,
}

/// The result of pairing.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pairing {
    /// By OpenATCA address.
    pub groups: Vec<Group>,
    /// ERAM flights with no OpenATCA aircraft, in flight order.
    pub unpaired_eram: Vec<usize>,
    /// OpenATCA aircraft with no ERAM flight, by address.
    pub unpaired_oa: Vec<u32>,
}

/// Pairs `eram` (in a fixed order: it fixes group order and which flight claims first) with `oa`.
pub fn pair(eram: &[EramFlight], oa: &BTreeMap<u32, OaAircraft>) -> Pairing {
    let mut groups: BTreeMap<u32, Vec<(usize, Rule)>> = BTreeMap::new();
    // The spans already paired to each aircraft, for the exclusivity of beacon claims.
    let mut claimed: BTreeMap<u32, Vec<(i64, i64)>> = BTreeMap::new();
    let mut unpaired_eram = Vec::new();
    let mut later: Vec<usize> = Vec::new();
    for (i, f) in eram.iter().enumerate() {
        match (f.addr.filter(|a| oa.contains_key(a)), f.span()) {
            (Some(a), Some(span)) => {
                groups.entry(a).or_default().push((i, Rule::Address));
                claimed.entry(a).or_default().push(span);
            }
            (Some(a), None) => groups.entry(a).or_default().push((i, Rule::Address)),
            // No address, or one OpenATCA has no aircraft for: the beacon rule.
            (None, _) => later.push(i),
        }
    }
    for i in later {
        let Some(f) = eram.get(i) else { continue };
        match beacon_match(f, oa, &claimed) {
            Some(a) => {
                groups
                    .entry(a)
                    .or_default()
                    .push((i, Rule::BeaconProximity));
                if let Some(span) = f.span() {
                    claimed.entry(a).or_default().push(span);
                }
            }
            None => unpaired_eram.push(i),
        }
    }
    unpaired_eram.sort_unstable();
    let unpaired_oa = oa
        .keys()
        .copied()
        .filter(|a| !groups.contains_key(a))
        .collect();
    Pairing {
        groups: groups
            .into_iter()
            .map(|(oa_addr, mut eram)| {
                eram.sort_unstable();
                Group { oa_addr, eram }
            })
            .collect(),
        unpaired_eram,
        unpaired_oa,
    }
}

fn beacon_match(
    f: &EramFlight,
    oa: &BTreeMap<u32, OaAircraft>,
    claimed: &BTreeMap<u32, Vec<(i64, i64)>>,
) -> Option<u32> {
    let first = f.samples.first()?;
    let last = f.samples.last()?;
    let beacon = f.beacon_for_pairing(first.t_ms)?;
    let gap = THRESHOLDS.segment_gap_ms;
    let mut best: Option<(f64, u32)> = None;
    for (&addr, ac) in oa {
        // Exclusive within overlapping spans: an aircraft already paired to a flight that is
        // there at the same time is not a candidate.
        if claimed
            .get(&addr)
            .is_some_and(|v| v.iter().any(|&(a, b)| a <= last.t_ms && first.t_ms <= b))
        {
            continue;
        }
        let offered = ac
            .idents_in(first.t_ms - gap, last.t_ms + gap)
            .iter()
            .any(|p| p.squawk == Some(beacon));
        if !offered {
            continue;
        }
        let Some(p) = oa_at(&ac.samples, first.t_ms, &THRESHOLDS) else {
            continue;
        };
        let (e, n) = enu_m(p.lat_deg, p.lon_deg, deg(first.lat_e7), deg(first.lon_e7));
        let d = e.hypot(n);
        // Ascending address order, so a strict comparison keeps the lowest on a tie.
        if d <= THRESHOLDS.pair_radius_m && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, addr));
        }
    }
    best.map(|(_, a)| a)
}

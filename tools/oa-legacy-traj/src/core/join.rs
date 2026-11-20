//! Which ADS-B aircraft a flight is. Core (pure).
//!
//! A flight that carries an aircraft address joins by it: the truth legs of that address that
//! overlap the flight's interval. A flight without one joins by callsign: the truth legs with
//! the callsign that overlap the interval, which must all be one aircraft. Every flight lands in
//! one [`Outcome`]; none is dropped.

use std::collections::BTreeMap;

use crate::core::types::NO_ADDR;

/// One truth leg: a continuous stretch of one aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leg {
    pub addr: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Upper case, zero padded; all zero when the leg has no callsign.
    pub callsign: [u8; 8],
}

/// A flight to join.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    /// The aircraft address a message carried, [`NO_ADDR`] when none did.
    pub addr: u32,
    /// The flight's records name more than one address (the plan identifier is shared by flights
    /// that carry different ones).
    pub addr_conflict: bool,
    /// Upper case, zero padded; all zero when none.
    pub callsign: [u8; 8],
    /// The flight's interval, Unix ms.
    pub from_ms: i64,
    pub to_ms: i64,
}

/// How a flight was joined, or why it was not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// By the address; the address.
    ByAddress(u32),
    /// By the callsign; the address of the aircraft that has it.
    ByCallsign(u32),
    /// No address and no callsign to join by.
    NoAddress,
    /// An identity, but no truth leg of it overlaps the flight's interval.
    NoTruth,
    /// More than one aircraft (or more than one address) could be the flight.
    Ambiguous,
}

/// Number of [`Outcome`] kinds.
pub const OUTCOME_COUNT: usize = 5;
/// Display names, indexed by [`outcome_index`].
pub const OUTCOME_NAMES: [&str; OUTCOME_COUNT] = [
    "joined by address",
    "joined by callsign",
    "no address and no callsign",
    "no truth (an identity, but no ADS-B leg of it in the interval)",
    "ambiguous (several addresses or several aircraft)",
];

/// The index of an outcome in [`OUTCOME_NAMES`].
pub fn outcome_index(o: Outcome) -> usize {
    match o {
        Outcome::ByAddress(_) => 0,
        Outcome::ByCallsign(_) => 1,
        Outcome::NoAddress => 2,
        Outcome::NoTruth => 3,
        Outcome::Ambiguous => 4,
    }
}

/// The truth legs by address and by callsign.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct LegIndex {
    pub legs: Vec<Leg>,
    pub by_addr: BTreeMap<u32, Vec<u32>>,
    pub by_callsign: BTreeMap<[u8; 8], Vec<u32>>,
}

/// Indexes the legs.
pub fn index_legs(legs: &[Leg]) -> LegIndex {
    let mut ix = LegIndex {
        legs: legs.to_vec(),
        ..LegIndex::default()
    };
    for (i, l) in legs.iter().enumerate() {
        ix.by_addr.entry(l.addr).or_default().push(i as u32);
        if l.callsign.iter().any(|&b| b != 0) {
            ix.by_callsign.entry(l.callsign).or_default().push(i as u32);
        }
    }
    ix
}

fn overlaps(l: &Leg, u: &Unit) -> bool {
    l.start_ms <= u.to_ms && l.end_ms >= u.from_ms
}

/// The indexes of the legs of `addr` that overlap the unit's interval.
pub fn legs_of(ix: &LegIndex, addr: u32, u: &Unit) -> Vec<u32> {
    ix.by_addr
        .get(&addr)
        .into_iter()
        .flatten()
        .copied()
        .filter(|&i| ix.legs.get(i as usize).is_some_and(|l| overlaps(l, u)))
        .collect()
}

/// Joins one flight.
pub fn join(ix: &LegIndex, u: &Unit) -> Outcome {
    if u.addr_conflict {
        return Outcome::Ambiguous;
    }
    if u.addr != NO_ADDR {
        return if legs_of(ix, u.addr, u).is_empty() {
            Outcome::NoTruth
        } else {
            Outcome::ByAddress(u.addr)
        };
    }
    if u.callsign.iter().all(|&b| b == 0) {
        return Outcome::NoAddress;
    }
    let mut addrs: Vec<u32> = ix
        .by_callsign
        .get(&u.callsign)
        .into_iter()
        .flatten()
        .filter_map(|&i| ix.legs.get(i as usize))
        .filter(|l| overlaps(l, u))
        .map(|l| l.addr)
        .collect();
    addrs.sort_unstable();
    addrs.dedup();
    match addrs.as_slice() {
        [] => Outcome::NoTruth,
        [a] => Outcome::ByCallsign(*a),
        _ => Outcome::Ambiguous,
    }
}

/// Every leg of `addr`, by start time.
pub fn all_legs_of(ix: &LegIndex, addr: u32) -> Vec<u32> {
    let mut v: Vec<u32> = ix.by_addr.get(&addr).cloned().unwrap_or_default();
    v.sort_by_key(|&i| {
        ix.legs
            .get(i as usize)
            .map_or((i64::MAX, i64::MAX), |l| (l.start_ms, l.end_ms))
    });
    v
}

/// The one truth leg a message binds to, among `cands` (leg indexes by start time): the leg in
/// progress at the issue time; else the first leg starting after it, within `window_ms`, whose
/// entry in `takes_off_ok` (parallel to `cands`) is true: the take-off airport is the flight's
/// departure airport, when known.
pub fn bind_leg(
    ix: &LegIndex,
    cands: &[u32],
    takes_off_ok: &[bool],
    issue_ms: i64,
    window_ms: i64,
) -> Option<u32> {
    let get = |i: u32| ix.legs.get(i as usize);
    if let Some(&i) = cands
        .iter()
        .find(|&&i| get(i).is_some_and(|l| l.start_ms <= issue_ms && l.end_ms >= issue_ms))
    {
        return Some(i);
    }
    cands
        .iter()
        .zip(takes_off_ok)
        .find(|(&i, &ok)| {
            ok && get(i)
                .is_some_and(|l| l.start_ms > issue_ms && l.start_ms - issue_ms <= window_ms)
        })
        .map(|(&i, _)| i)
}

/// Whether a leg may be the callsign-joined flight's: it starts no later than `window_ms` after the
/// flight's first issue time (it may be in progress at it).
pub fn leg_in_window(l: &Leg, first_issue_ms: i64, window_ms: i64) -> bool {
    l.end_ms >= first_issue_ms && l.start_ms <= first_issue_ms + window_ms
}

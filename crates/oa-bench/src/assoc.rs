//! Association (M2_DESIGN section 2): which truth aircraft is which ERAM flight. Core (pure).
//!
//! Selection never depends on the error being measured: a flight is kept or dropped on identity
//! and time overlap, never on how accurate ERAM's position is. Where a coarse position test is
//! used, its radius ([`AssocConfig::region_m`], 50 NM) is far beyond any ERAM tracking error, so
//! it separates different aircraft, not better from worse tracks.
//!
//! 1. The flight's `aircraftAddress`, where SFDPS carried it, names the truth aircraft when
//!    truth has that address (method address). Failing that, the address hint some track
//!    messages carry in an `ADSB_02M_*` name-value pair (method address hint; it equals
//!    `aircraftAddress` in 112 of 115 flights on the first sample, so it is its own method).
//! 2. An address is then checked for plausibility, with checks that do not use ERAM's accuracy:
//!    the callsigns must not contradict (ERAM's against the truth legs' callsigns, when both are
//!    known), and the ERAM positions must lie within the region of the truth for at least 80% of
//!    the shared times (when there are enough shared times to decide; otherwise the address is
//!    trusted and counted as unchecked). An implausible pair is not scored in the primary rows:
//!    it is flagged and scored in a separate table ([`Assoc::flag_addr`]).
//! 3. Otherwise the callsign (ERAM aircraft identification against the truth leg callsign, ASCII
//!    case-insensitive) gives candidate aircraft. A candidate needs enough shared times (truth
//!    and ERAM overlap in time) and the region test. Exactly one candidate is associated (method
//!    callsign); two or more is ambiguous and counted. A single candidate whose positions
//!    also pass the tight gate (3 NM, 1,000 ft for 80% of shared times) is the ordinary case; one
//!    that fails it is associated all the same under its own method (callsign, failed the
//!    check) so its errors stay in the headline numbers and are visible as a separate split, and
//!    each such flight is listed.
//! 4. A candidate outside the region is not associated: it is flagged, with its pairs in the
//!    separate table when there is exactly one such candidate.
//!
//! A flight with an address that truth does not know is not retried by callsign: the address
//! says it is a different aircraft than any truth callsign could prove.

use std::collections::BTreeMap;

use crate::geo::{dist_m, NM_M};
use crate::legacy::{Legacy, NONE, NO_ALT};
use crate::stats::method;
use crate::thindex::{samples_of, ThIndex};
use crate::truth::{Truth, TruthLeg};

/// Association outcome codes per flight.
pub mod astatus {
    pub const ADDRESS: u8 = 0;
    pub const CALLSIGN: u8 = 1;
    pub const AMBIGUOUS: u8 = 2;
    pub const ADDRESS_NO_TRUTH: u8 = 3;
    pub const GATE_FAILED: u8 = 4;
    pub const NO_CALLSIGN_MATCH: u8 = 5;
    pub const NO_KEY: u8 = 6;
    pub const TOO_FEW_SHARED: u8 = 7;
    pub const ADDRESS_HINT: u8 = 8;
    pub const ADDRESS_IMPLAUSIBLE: u8 = 9;
    pub const HINT_IMPLAUSIBLE: u8 = 10;
    pub const CALLSIGN_IMPLAUSIBLE: u8 = 11;
    pub const COUNT: usize = 12;
    pub const NAMES: [&str; COUNT] = [
        "associated by address",
        "associated by callsign",
        "ambiguous (two aircraft pass the gate)",
        "address has no truth",
        "associated by callsign, failed the 3 NM / 1000 ft check (scored as its own method)",
        "no truth aircraft with this callsign",
        "no address and no callsign",
        "too few shared times to gate",
        "associated by address hint (ADSB_02M)",
        "address implausible against truth (scored separately)",
        "address hint implausible against truth (scored separately)",
        "callsign candidate far from truth (scored separately)",
    ];
    /// Why a flagged flight is flagged.
    pub const REASON_NAMES: [&str; 3] = [
        "not flagged",
        "ERAM positions far from the truth aircraft",
        "callsigns disagree",
    ];
}

/// Association settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssocConfig {
    /// Horizontal gate, metres.
    pub gate_h_m: f64,
    /// Vertical gate, feet.
    pub gate_v_ft: f64,
    /// Share of shared times that must pass.
    pub min_share: f64,
    /// Plausibility region, metres: ERAM positions farther than this from the truth aircraft do
    /// not belong to it (a different aircraft, not a worse track).
    pub region_m: f64,
    /// Fewest shared times a callsign gate may be decided on.
    pub min_shared: u32,
    /// Most samples of a flight tested against one candidate (a deterministic stride is used).
    pub max_samples: usize,
}

impl Default for AssocConfig {
    fn default() -> AssocConfig {
        AssocConfig {
            gate_h_m: 3.0 * NM_M,
            gate_v_ft: 1000.0,
            min_share: 0.8,
            region_m: 50.0 * NM_M,
            min_shared: 5,
            max_samples: 200,
        }
    }
}

/// The association of every flight.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct Assoc {
    /// Truth aircraft address per flight, [`NONE`] when not associated.
    pub addr: Vec<u32>,
    /// [`method`] per flight, 0 when not associated.
    pub method: Vec<u8>,
    /// [`astatus`] per flight.
    pub status: Vec<u8>,
    /// Flights per status.
    pub counts: [u64; astatus::COUNT],
    /// Truth aircraft address of a flagged (implausible) flight, [`NONE`] otherwise. Its pairs
    /// are scored in the separate table.
    pub flag_addr: Vec<u32>,
    /// [`method`] of a flagged flight, 0 otherwise.
    pub flag_method: Vec<u8>,
    /// Index into [`astatus::REASON_NAMES`] per flight.
    pub flag_reason: Vec<u8>,
    /// Share of shared times inside the tight gate, NaN when not computed.
    pub gate_share: Vec<f64>,
    /// Shared times the plausibility or gate test was decided on.
    pub shared: Vec<u32>,
    /// Address associations that could not be checked (too few shared times); trusted.
    pub unchecked: u64,
}

/// Truth lookups by address and by callsign.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct TruthIndex {
    /// Leg indexes per address.
    pub by_addr: BTreeMap<u32, Vec<u32>>,
    /// Addresses per normalised callsign.
    pub by_callsign: BTreeMap<[u8; 8], Vec<u32>>,
}

/// A callsign normalised for comparison: cut at the first zero or space, upper case, zero padded.
pub fn norm_callsign(c: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (o, &b) in out
        .iter_mut()
        .zip(c.iter().take_while(|&&b| b != 0 && b != b' '))
    {
        *o = b.to_ascii_uppercase();
    }
    out
}

/// Indexes the truth legs.
pub fn index_truth(legs: &[TruthLeg]) -> TruthIndex {
    let mut ix = TruthIndex::default();
    for (i, l) in legs.iter().enumerate() {
        ix.by_addr.entry(l.addr).or_default().push(i as u32);
        if let Some(c) = l.callsign {
            let key = norm_callsign(&c);
            if key.iter().any(|&b| b != 0) {
                let v = ix.by_callsign.entry(key).or_default();
                if !v.contains(&l.addr) {
                    v.push(l.addr);
                }
            }
        }
    }
    ix
}

/// Shared times of a candidate and how many lie within `radius_m` (and `vert_ft`, when given).
fn shared_within(
    leg: &Legacy,
    samples: &[u32],
    truth: &Truth<'_>,
    addr: u32,
    radius_m: f64,
    vert_ft: Option<f64>,
    cfg: &AssocConfig,
) -> (u32, u32) {
    let stride = samples.len().div_ceil(cfg.max_samples.max(1)).max(1);
    let (mut shared, mut passed) = (0u32, 0u32);
    for &s in samples.iter().step_by(stride) {
        let i = s as usize;
        let (Some(&t), Some(&lat), Some(&lon)) = (
            leg.th.pos_ms.get(i),
            leg.th.lat_e7.get(i),
            leg.th.lon_e7.get(i),
        ) else {
            continue;
        };
        let Ok(ts) = (truth.query)(addr, t) else {
            continue;
        };
        shared += 1;
        let d = dist_m(ts.lat, ts.lon, f64::from(lat) * 1e-7, f64::from(lon) * 1e-7);
        let alt = leg.th.alt_ft.get(i).copied().unwrap_or(NO_ALT);
        let v_ok = match (vert_ft, ts.baro_alt_ft, alt) {
            (Some(g), Some(ta), a) if a != NO_ALT => f64::from((a - ta).abs()) <= g,
            _ => true,
        };
        if d <= radius_m && v_ok {
            passed += 1;
        }
    }
    (shared, passed)
}

/// Whether the truth legs of `addr` carry a callsign and none equals `cs`.
fn callsign_contradicts(tix: &TruthIndex, legs: &[TruthLeg], addr: u32, cs: &[u8; 8]) -> bool {
    if cs.iter().all(|&b| b == 0) {
        return false;
    }
    let mut known = false;
    for &l in tix.by_addr.get(&addr).map_or(&[][..], |v| &v[..]) {
        let Some(Some(c)) = legs.get(l as usize).map(|l| l.callsign) else {
            continue;
        };
        let k = norm_callsign(&c);
        if k.iter().all(|&b| b == 0) {
            continue;
        }
        known = true;
        if &k == cs {
            return false;
        }
    }
    known
}

/// The verdict of the plausibility test of an address association.
enum Plausible {
    Yes,
    /// Too few shared times to decide: trusted.
    Unchecked,
    No(u8),
}

/// The checks of method 2: callsign agreement, then the share of shared times within the region.
#[allow(clippy::too_many_arguments)]
fn address_plausible(
    leg: &Legacy,
    samples: &[u32],
    truth: &Truth<'_>,
    tix: &TruthIndex,
    addr: u32,
    cs: &[u8; 8],
    cfg: &AssocConfig,
    shared_out: &mut u32,
) -> Plausible {
    if callsign_contradicts(tix, truth.legs, addr, cs) {
        return Plausible::No(2);
    }
    let (shared, passed) = shared_within(leg, samples, truth, addr, cfg.region_m, None, cfg);
    *shared_out = shared;
    if shared < cfg.min_shared {
        Plausible::Unchecked
    } else if f64::from(passed) >= cfg.min_share * f64::from(shared) {
        Plausible::Yes
    } else {
        Plausible::No(1)
    }
}

/// Associates every flight with a truth aircraft.
pub fn associate(
    leg: &Legacy,
    ix: &ThIndex,
    truth: &Truth<'_>,
    tix: &TruthIndex,
    cfg: &AssocConfig,
) -> Assoc {
    let n = leg.flights.gufi.len();
    let mut out = Assoc {
        addr: vec![NONE; n],
        method: vec![0; n],
        status: vec![astatus::NO_KEY; n],
        counts: [0; astatus::COUNT],
        flag_addr: vec![NONE; n],
        flag_method: vec![0; n],
        flag_reason: vec![0; n],
        gate_share: vec![f64::NAN; n],
        shared: vec![0; n],
        unchecked: 0,
    };
    for f in 0..n {
        let fa = leg.flights.addr.get(f).copied().unwrap_or(NONE);
        let cs = norm_callsign(leg.flights.callsign.get(f).map_or(&[][..], |c| &c[..]));
        let samples = samples_of(ix, f as u32);
        let mut status = astatus::NO_KEY;
        let (mut addr, mut meth) = (NONE, 0u8);
        let (mut flag_addr, mut flag_meth, mut reason) = (NONE, 0u8, 0u8);
        let mut shared = 0u32;
        let by_address = if fa != NONE {
            Some((
                fa,
                method::ADDRESS,
                astatus::ADDRESS,
                astatus::ADDRESS_IMPLAUSIBLE,
            ))
        } else {
            leg.flights
                .addr_hint
                .get(f)
                .copied()
                .filter(|&a| a != NONE)
                .map(|a| {
                    (
                        a,
                        method::ADDRESS_HINT,
                        astatus::ADDRESS_HINT,
                        astatus::HINT_IMPLAUSIBLE,
                    )
                })
        };
        if let Some((a, m, ok, bad)) = by_address {
            if tix.by_addr.contains_key(&a) {
                match address_plausible(leg, samples, truth, tix, a, &cs, cfg, &mut shared) {
                    Plausible::Yes => (status, addr, meth) = (ok, a, m),
                    Plausible::Unchecked => {
                        (status, addr, meth) = (ok, a, m);
                        out.unchecked += 1;
                    }
                    Plausible::No(r) => (status, flag_addr, flag_meth, reason) = (bad, a, m, r),
                }
            } else {
                status = astatus::ADDRESS_NO_TRUTH;
            }
        } else if cs.iter().any(|&b| b != 0) {
            match tix.by_callsign.get(&cs) {
                None => status = astatus::NO_CALLSIGN_MATCH,
                Some(cands) => {
                    // (address, shared times, share inside the tight gate)
                    let mut enough: Vec<(u32, u32, f64)> = Vec::new();
                    let mut inside: Vec<(u32, u32, f64)> = Vec::new();
                    for &a in cands {
                        let (sh, reg) =
                            shared_within(leg, samples, truth, a, cfg.region_m, None, cfg);
                        if sh < cfg.min_shared {
                            continue;
                        }
                        let (_, tight) = shared_within(
                            leg,
                            samples,
                            truth,
                            a,
                            cfg.gate_h_m,
                            Some(cfg.gate_v_ft),
                            cfg,
                        );
                        let share = f64::from(tight) / f64::from(sh);
                        enough.push((a, sh, share));
                        if f64::from(reg) >= cfg.min_share * f64::from(sh) {
                            inside.push((a, sh, share));
                        }
                    }
                    if enough.is_empty() {
                        status = astatus::TOO_FEW_SHARED;
                    } else if let [(a, sh, share)] = inside[..] {
                        addr = a;
                        shared = sh;
                        if let Some(g) = out.gate_share.get_mut(f) {
                            *g = share;
                        }
                        if share >= cfg.min_share {
                            (status, meth) = (astatus::CALLSIGN, method::CALLSIGN);
                        } else {
                            (status, meth) = (astatus::GATE_FAILED, method::CALLSIGN_GATE_FAILED);
                        }
                    } else if inside.len() > 1 {
                        status = astatus::AMBIGUOUS;
                    } else {
                        status = astatus::CALLSIGN_IMPLAUSIBLE;
                        reason = 1;
                        if let [(a, sh, _)] = enough[..] {
                            (flag_addr, flag_meth, shared) = (a, method::CALLSIGN, sh);
                        }
                    }
                }
            }
        }
        if let Some(c) = out.counts.get_mut(usize::from(status)) {
            *c += 1;
        }
        if let (Some(a), Some(m), Some(s), Some(fa), Some(fm), Some(fr), Some(sh)) = (
            out.addr.get_mut(f),
            out.method.get_mut(f),
            out.status.get_mut(f),
            out.flag_addr.get_mut(f),
            out.flag_method.get_mut(f),
            out.flag_reason.get_mut(f),
            out.shared.get_mut(f),
        ) {
            (*a, *m, *s, *fa, *fm, *fr, *sh) =
                (addr, meth, status, flag_addr, flag_meth, reason, shared);
        }
    }
    out
}

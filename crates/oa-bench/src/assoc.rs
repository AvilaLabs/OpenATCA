//! Association (M2_DESIGN section 2): which truth aircraft is which ERAM flight. Core (pure).
//!
//! 1. The flight's `aircraftAddress`, where SFDPS carried it, names the truth aircraft when
//!    truth has that address (method address). No gate is applied: the address is the identity.
//!    Failing that, the address hint some track messages carry in an `ADSB_02M_*` name-value
//!    pair (method address hint; it equals `aircraftAddress` in 112 of 115 flights on the first
//!    sample, so it is stored as its own method and every metric can be split by it).
//! 2. Otherwise the callsign (ERAM aircraft identification against the truth leg callsign, ASCII
//!    case-insensitive) gives candidate aircraft, and a candidate is kept only if the ERAM
//!    positions lie within the gate (3 NM, 1,000 ft) of its truth for at least 80% of the shared
//!    times (method callsign).
//! 3. Two or more candidates passing the gate is ambiguous: the flight is not associated and the
//!    case is counted.
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
    pub const COUNT: usize = 9;
    pub const NAMES: [&str; COUNT] = [
        "associated by address",
        "associated by callsign",
        "ambiguous (two aircraft pass the gate)",
        "address has no truth",
        "callsign candidates failed the gate",
        "no truth aircraft with this callsign",
        "no address and no callsign",
        "too few shared times to gate",
        "associated by address hint (ADSB_02M)",
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

/// The share of shared times a candidate passes: `(shared, passed)`.
fn gate_counts(
    leg: &Legacy,
    samples: &[u32],
    truth: &Truth<'_>,
    addr: u32,
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
        let v_ok = match (ts.baro_alt_ft, alt) {
            (Some(ta), a) if a != NO_ALT => f64::from((a - ta).abs()) <= cfg.gate_v_ft,
            _ => true,
        };
        if d <= cfg.gate_h_m && v_ok {
            passed += 1;
        }
    }
    (shared, passed)
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
    };
    for f in 0..n {
        let fa = leg.flights.addr.get(f).copied().unwrap_or(NONE);
        let cs = norm_callsign(leg.flights.callsign.get(f).map_or(&[][..], |c| &c[..]));
        let mut status = astatus::NO_KEY;
        let mut addr = NONE;
        let mut meth = 0u8;
        if fa != NONE {
            if tix.by_addr.contains_key(&fa) {
                status = astatus::ADDRESS;
                addr = fa;
                meth = method::ADDRESS;
            } else {
                status = astatus::ADDRESS_NO_TRUTH;
            }
        } else if let Some(ha) = leg.flights.addr_hint.get(f).copied().filter(|&a| a != NONE) {
            if tix.by_addr.contains_key(&ha) {
                status = astatus::ADDRESS_HINT;
                addr = ha;
                meth = method::ADDRESS_HINT;
            } else {
                status = astatus::ADDRESS_NO_TRUTH;
            }
        } else if cs.iter().any(|&b| b != 0) {
            match tix.by_callsign.get(&cs) {
                None => status = astatus::NO_CALLSIGN_MATCH,
                Some(cands) => {
                    let samples = samples_of(ix, f as u32);
                    let mut pass: Vec<u32> = Vec::new();
                    let mut enough = false;
                    for &a in cands {
                        let (shared, passed) = gate_counts(leg, samples, truth, a, cfg);
                        if shared >= cfg.min_shared {
                            enough = true;
                            if f64::from(passed) >= cfg.min_share * f64::from(shared) {
                                pass.push(a);
                            }
                        }
                    }
                    status = if !enough {
                        astatus::TOO_FEW_SHARED
                    } else {
                        match pass.len() {
                            0 => astatus::GATE_FAILED,
                            1 => {
                                addr = pass.first().copied().unwrap_or(NONE);
                                meth = method::CALLSIGN;
                                astatus::CALLSIGN
                            }
                            _ => astatus::AMBIGUOUS,
                        }
                    };
                }
            }
        }
        if let Some(c) = out.counts.get_mut(usize::from(status)) {
            *c += 1;
        }
        if let (Some(a), Some(m), Some(s)) = (
            out.addr.get_mut(f),
            out.method.get_mut(f),
            out.status.get_mut(f),
        ) {
            *a = addr;
            *m = meth;
            *s = status;
        }
    }
    out
}

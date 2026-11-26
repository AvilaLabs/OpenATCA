//! Who is alerted. The practice in the field is that MSAW is inhibited for VFR traffic and for an
//! aircraft that has cancelled IFR unless the pilot asks for it; this tool does the same with what
//! the recordings hold: a track is alerted when the flight record (SFDPS) has a flight for it that
//! is active under an IFR plan, found by the ICAO address first and then by the callsign, or when
//! an adaptation `msaw_request` line names it for that time. Every other track is still evaluated;
//! its violations are written to `advisory.csv` and counted, never alerted and never dropped.
//! Shell (B3): it reads the recordings through the encounter tool's join of `oa-flight-sfdps`.
//!
//! A flight is VFR when its assigned altitude is the VFR form (VFR on top is an IFR clearance and
//! counts as IFR). A flight whose plan is only proposed is not yet active. A track whose address
//! or callsign names several flights that cannot be told apart is refused and counted as an
//! advisory, and so is a track with no flight.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use oa_encounter_run::join::{
    answer_queries_counted, observe, timeline, AddrResult, Answer, CallsignObs, CsReject, CsResult,
    Key, Query, Sfdps,
};
use oa_flight_sfdps::norm_callsign;
use oa_msaw::Alert;

use crate::config::{RequestSpec, Requester};

/// The lag floor of SFDPS applied to the recordings, milliseconds (UK-004: 58 to 61 s).
pub const SFDPS_LAG_FLOOR_MS: i64 = 58_000;

/// Why a track is or is not alerted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Basis {
    /// An active IFR flight found by the ICAO address.
    Address,
    /// An active IFR flight found by the callsign.
    Callsign,
    /// An adaptation `msaw_request`.
    Request,
    /// The flight record has a VFR flight for it.
    VfrPlan,
    /// The flight is not active (proposed).
    NotActive,
    /// Several flights fit and cannot be told apart.
    Ambiguous,
    /// No flight for it.
    NoFlight,
    /// No flight is found for it, or it is not active or ambiguous, and its beacon code is a VFR
    /// code of the adaptation: not alerted.
    VfrBeacon,
    /// No flight data was given: every track is alerted, and the summary says it was not judged.
    NotJudged,
}

pub const BASIS_COUNT: usize = 9;

impl Basis {
    pub fn text(self) -> &'static str {
        match self {
            Basis::Address => "ifr_by_address",
            Basis::Callsign => "ifr_by_callsign",
            Basis::Request => "pilot_request",
            Basis::VfrPlan => "vfr_plan",
            Basis::NotActive => "plan_not_active",
            Basis::Ambiguous => "ambiguous",
            Basis::NoFlight => "no_flight",
            Basis::VfrBeacon => "vfr_beacon",
            Basis::NotJudged => "not_judged",
        }
    }

    /// Whether a track with this basis is alerted: every one but a VFR flight plan and a VFR
    /// beacon code. A track with no flight, an ambiguous one or a plan not yet active is alerted:
    /// no flight record is not a reason to suppress a warning.
    pub fn alerted(self) -> bool {
        !matches!(self, Basis::VfrPlan | Basis::VfrBeacon)
    }
}

/// The counts of a run. A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EligibilityReport {
    /// Flight data was given (else nothing was judged).
    pub judged: bool,
    /// The alerts that stand (the basis of each, keyed by address and start) and the advisories.
    pub basis: BTreeMap<(u32, i64), Basis>,
    pub advisories: Vec<(Alert, Basis)>,
    /// Alerts and tracks by basis (indexed by `Basis as usize`).
    pub alerts_by: [u64; BASIS_COUNT],
    pub tracks_by: [u64; BASIS_COUNT],
    pub tracks: u64,
    /// SFDPS operations the replay applied and flights it held.
    pub sfdps_ops: u64,
    pub sfdps_flights: usize,
}

/// The basis of one track from the join's answer and the requests.
pub fn basis_of(a: &Answer, requested: bool) -> Basis {
    if requested {
        return Basis::Request;
    }
    if a.flight {
        return live_flight_basis(a.active, a.assigned.form, a.key == Key::Address);
    }
    let several_addr = matches!(
        a.addr,
        AddrResult::SeveralUnresolved | AddrResult::SeveralOverCap
    );
    let several_cs = a.cs == CsResult::Rejected(CsReject::SeveralLive);
    if several_addr || several_cs {
        Basis::Ambiguous
    } else {
        Basis::NoFlight
    }
}

/// The shared alert-basis policy for a flight view joined by the live regional caller.
/// `ALT_VFR_ON_TOP_PLUS` and every non-VFR altitude form remain IFR bases.
pub fn live_flight_basis(active: bool, assigned_form: u8, by_address: bool) -> Basis {
    if assigned_form == oa_flight::ALT_VFR {
        Basis::VfrPlan
    } else if !active {
        Basis::NotActive
    } else if by_address {
        Basis::Address
    } else {
        Basis::Callsign
    }
}

/// Apply the same explicit pilot-request override and VFR-beacon policy to a live basis.
pub fn live_source_basis(basis: Basis, requested: bool, vfr_code: bool) -> Basis {
    if requested {
        Basis::Request
    } else {
        with_beacon(basis, vfr_code)
    }
}

/// The basis once the beacon code is known: a track with no flight, an ambiguous one or a plan not
/// yet active is not alerted only if its code is a VFR code of the adaptation. Nothing else
/// suppresses it (the SFDPS lag floor included: a flight filed within the lag is no flight yet,
/// and the track is alerted).
pub fn with_beacon(b: Basis, vfr_code: bool) -> Basis {
    if vfr_code && matches!(b, Basis::NoFlight | Basis::Ambiguous | Basis::NotActive) {
        Basis::VfrBeacon
    } else {
        b
    }
}

/// Whether a `msaw_request` line names the track at `t_s` seconds after the clip's first
/// measurement, by its address or by its callsign (as the ADS-B reports carry it).
pub fn requested(reqs: &[RequestSpec], addr: u32, cs: CallsignObs, t_s: f64) -> bool {
    reqs.iter().any(|r| {
        t_s >= r.start_s
            && t_s < r.end_s
            && match &r.who {
                Requester::Address(a) => *a == addr,
                Requester::Callsign(c) => match cs {
                    CallsignObs::Stable(x) => x == norm_callsign(pad(c)),
                    _ => false,
                },
            }
    })
}

fn pad(c: &str) -> [u8; 8] {
    let mut b = [0u8; 8];
    for (d, s) in b.iter_mut().zip(c.bytes()) {
        *d = s;
    }
    b
}

/// What the judgement needs of the run: the alerts, the tracks (address, first time ns, position)
/// and the callsigns the ADS-B reports carried.
// A7.2: cold struct, size guard waived
pub struct Inputs<'a> {
    pub alerts: &'a [Alert],
    pub tracks: &'a [(u32, i64, i32, i32)],
    pub callsigns: &'a oa_encounter_run::join::Timeline,
    pub requests: &'a [RequestSpec],
    /// The Mode A codes of each aircraft by time (nanoseconds), in time order, and the codes of VFR
    /// traffic.
    pub idents: &'a BTreeMap<u32, Vec<(i64, u16)>>,
    pub vfr_codes: &'a [u16],
    pub t0_ns: i64,
    pub sfdps: Option<&'a Path>,
}

/// The Mode A codes of `meas` by address, with the time of each report, in time order.
pub fn idents_of(meas: &[oa_track::Measurement]) -> BTreeMap<u32, Vec<(i64, u16)>> {
    let mut m: BTreeMap<u32, Vec<(i64, u16)>> = BTreeMap::new();
    for x in meas {
        if x.ident != oa_track::NO_IDENT {
            m.entry(x.addr).or_default().push((x.toa_ns, x.ident));
        }
    }
    m
}

/// The distinct addresses of `meas` with their first time and position, in address order.
pub fn first_seen(meas: &[oa_track::Measurement]) -> Vec<(u32, i64, i32, i32)> {
    let mut m: BTreeMap<u32, (i64, i32, i32)> = BTreeMap::new();
    for x in meas {
        m.entry(x.addr).or_insert((x.toa_ns, x.lat_e7, x.lon_e7));
    }
    m.into_iter()
        .map(|(a, (t, la, lo))| (a, t, la, lo))
        .collect()
}

/// The callsigns of the clip's reports for `addrs`, from `from_ns` to `to_ns`.
pub fn callsign_timeline(
    r: &oa_adsb::Reports,
    addrs: &BTreeSet<u32>,
    from_ns: i64,
    to_ns: i64,
) -> oa_encounter_run::join::Timeline {
    timeline(r, addrs, from_ns, to_ns)
}

/// Judges every alert and every track. With no recordings nothing is judged: every alert stands
/// (basis `NotJudged`), apart from none being an advisory, and the report says so.
pub fn judge(i: &Inputs, sealed: &mut u64) -> Result<EligibilityReport, String> {
    let mut rep = EligibilityReport::default();
    let obs_of = |addr: u32, t: i64| observe(i.callsigns, addr, t);
    let req_of = |addr: u32, t: i64| {
        let (cs, _) = obs_of(addr, t);
        requested(i.requests, addr, cs, (t - i.t0_ns) as f64 / 1e9)
    };
    let answers: Option<Vec<Answer>> = match i.sfdps {
        None => None,
        Some(dir) => {
            let mk = |addr: u32, t: i64, la: i32, lo: i32| {
                let (cs, pos) = obs_of(addr, t);
                let (la, lo) = pos.unwrap_or((la, lo));
                Query {
                    addr,
                    t_ns: t,
                    callsign: cs,
                    lat_e7: la,
                    lon_e7: lo,
                    try_callsign: true,
                }
            };
            let mut q: Vec<Query> = i
                .alerts
                .iter()
                .map(|a| mk(a.addr, a.start_ns, a.lat_e7, a.lon_e7))
                .collect();
            q.extend(i.tracks.iter().map(|&(a, t, la, lo)| mk(a, t, la, lo)));
            let s = Sfdps {
                dir: dir.to_path_buf(),
                lag_floor_ms: SFDPS_LAG_FLOOR_MS,
                pick_newest: false,
            };
            let (a, _, st) = answer_queries_counted(&s, &q, sealed)?;
            rep.sfdps_ops = st.ops;
            rep.sfdps_flights = st.flights_held;
            Some(a)
        }
    };
    rep.judged = answers.is_some();
    let n_alerts = i.alerts.len();
    // The beacon code the aircraft showed last at or before `t`, or the first after it.
    let vfr_code_at = |addr: u32, t: i64| -> bool {
        i.idents.get(&addr).is_some_and(|v| {
            let k = v.partition_point(|&(x, _)| x <= t);
            k.checked_sub(1)
                .and_then(|k| v.get(k))
                .or_else(|| v.first())
                .is_some_and(|&(_, c)| i.vfr_codes.contains(&c))
        })
    };
    let basis_at = |k: usize, addr: u32, t: i64| -> Basis {
        match &answers {
            None => {
                if req_of(addr, t) {
                    Basis::Request
                } else {
                    Basis::NotJudged
                }
            }
            Some(a) => {
                let b = a
                    .get(k)
                    .map_or(Basis::NoFlight, |a| basis_of(a, req_of(addr, t)));
                with_beacon(b, vfr_code_at(addr, t))
            }
        }
    };
    for (k, a) in i.alerts.iter().enumerate() {
        let b = basis_at(k, a.addr, a.start_ns);
        if let Some(c) = rep.alerts_by.get_mut(b as usize) {
            *c += 1;
        }
        if b.alerted() {
            rep.basis.insert((a.addr, a.start_ns), b);
        } else {
            rep.advisories.push((*a, b));
        }
    }
    for (j, &(addr, t, _, _)) in i.tracks.iter().enumerate() {
        let b = basis_at(n_alerts + j, addr, t);
        if let Some(c) = rep.tracks_by.get_mut(b as usize) {
            *c += 1;
        }
        rep.tracks += 1;
    }
    Ok(rep)
}

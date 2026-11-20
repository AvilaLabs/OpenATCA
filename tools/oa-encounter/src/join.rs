//! The join of the record of close encounters with the flight record (M4): for each aircraft of
//! each encounter, the altitudes SFDPS had made available for it at the closest approach. Shell
//! (B3): it reads the recordings through `oa-flight-sfdps`.
//!
//! "As of" is the table's own time: an operation counts only from `max(received, stamped + lag
//! floor)`, so a question at time T is answered before the first operation available after T is
//! applied and sees nothing that arrived or aged in later.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt::Write as _;
use std::path::PathBuf;

use oa_encounter::Encounter;
use oa_flight::{
    flight_by_address, flight_get, flight_is_flying, flight_is_live, flights_claiming_address,
    flights_flying_by_callsign, Alt, Flight, FlightConfig, FlightKey, FlightTable, ALT_AT_FIX,
    ALT_BLOCK, ALT_NIL, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS, KEY_GUFI, ST_ACTIVE, ST_DEPARTED,
};
use oa_flight_sfdps::{
    norm_callsign, recordings_in, replay_sfdps_counted, Position, Positions, ReplayConfig,
};

use crate::report::{at_vertical_minimum, class_name, context_of, Airport, CLIMB_FPM, LEVEL_FT};

/// Vertical separations from this up to the record volume's 1,000 ft are "near 1,000 ft" for the
/// split (the 25 ft altitude quantum puts two levels 1,000 ft apart at 975 to 1,000 ft, and a
/// climb stopping at a level leaves the pair a little under that while it settles).
pub const NEAR_FROM_FT: f32 = 900.0;
/// Two clearances at least this far apart are levels 1,000 ft apart: a level-off.
pub const LEVEL_OFF_FT: i32 = 1000;

/// How the join found the flight of an aircraft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Key {
    /// No flight.
    #[default]
    None,
    /// The address names a flying flight.
    Address,
    /// The callsign names one flying flight and its own position agrees with the ADS-B one.
    CallsignChecked,
    /// The callsign names one flying flight and it has no position within the window.
    CallsignUnchecked,
    /// The address and the callsign named different flights and only the callsign flight's own
    /// position agreed with the ADS-B one.
    CallsignOverAddress,
}

/// What the address join found. Every flight that claims the address is considered, not only
/// the newest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddrResult {
    /// No flight claims the address.
    #[default]
    NoClaimant,
    /// One flight claims it and it is dropped at the query time.
    OnlyClaimantDropped,
    /// Flights claim it and none is flying (all ended or dropped), other than the case above.
    NoneFlying,
    /// One claimant is flying.
    One,
    /// Several are flying and one has the ADS-B callsign.
    SeveralCallsignDecides,
    /// Several are flying and one is active or departed where the others are proposed.
    SeveralStateDecides,
    /// Several are flying and exactly one has its own position within 120 s within 10 NM of the
    /// ADS-B one.
    SeveralByPosition,
    /// Several are flying and none of the steps leaves one.
    SeveralUnresolved,
    /// More than [`FLYING_CAP`] are flying: not looked at, unresolved.
    SeveralOverCap,
}

pub const ADDR_NAMES: [&str; 9] = [
    "no flight claims the address",
    "the only claimant is dropped",
    "address claimants, none flying (other)",
    "one claimant flying",
    "several flying, callsign decides",
    "several flying, state decides",
    "several flying, one passes the position check",
    "several flying claimants, not resolved",
    "more than the cap flying (unresolved, not looked at)",
];

/// How a disagreement between the address flight and the callsign flight was settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Only the callsign flight's position agrees: the callsign flight was taken.
    CallsignTaken,
    /// Only the address flight's position agrees.
    AddressTaken,
    /// Both agree: the address flight stands.
    BothPass,
    /// Neither agrees (or has a position to check): the address flight stands.
    NeitherPass,
}

pub const RESOLUTION_NAMES: [&str; 4] = [
    "callsign flight taken (only its position agrees)",
    "address flight taken (only its position agrees)",
    "both positions agree, address flight stands",
    "neither position agrees, address flight stands",
];

/// Why the callsign join found nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsReject {
    /// The ADS-B reports of the aircraft in the previous 60 s carry no callsign.
    NoCallsign,
    /// They carry more than one.
    Unstable,
    /// No flight that is flying states the callsign.
    NoLiveFlight,
    /// Several do.
    SeveralLive,
    /// The one flight's own position within 120 s is more than 10 NM from the ADS-B one.
    PositionDisagrees,
}

pub const CS_REJECT_NAMES: [&str; 5] = [
    "no callsign",
    "unstable callsign",
    "no live flight with the callsign",
    "several live flights",
    "position disagrees",
];

fn reject_index(r: CsReject) -> usize {
    match r {
        CsReject::NoCallsign => 0,
        CsReject::Unstable => 1,
        CsReject::NoLiveFlight => 2,
        CsReject::SeveralLive => 3,
        CsReject::PositionDisagrees => 4,
    }
}

/// The result of the callsign join for one aircraft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CsResult {
    /// Not tried (the aircraft is not in a context the callsign join is asked for).
    #[default]
    NotTried,
    Accepted {
        checked: bool,
        /// Several flying flights state the callsign, all with one departure and arrival (one
        /// aircraft with several plans), and the one with the newest plan stamp was taken.
        several: bool,
    },
    Rejected(CsReject),
}

/// What the flight record held for one aircraft at one time. A7.2: cold struct, size guard
/// waived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// The flight the join chose (the address one, else the accepted callsign one) is live.
    pub flight: bool,
    /// A flight claims the address but every plan of it had ended.
    pub ended: bool,
    pub assigned: Alt,
    pub interim: Alt,
    /// The chosen flight has a plan view that is active or departed (not only proposed).
    pub active: bool,
    /// How the chosen flight was found.
    pub key: Key,
    pub addr: AddrResult,
    /// The flight the address names, and the flight the accepted callsign names: both kept, so
    /// the two joins can be compared where both are possible.
    pub addr_flight: Option<FlightKey>,
    pub cs_flight: Option<FlightKey>,
    pub cs: CsResult,
    /// The join before the walk (the newest claimant, if not ended, else the callsign) found a
    /// flight: for counting what the walk lost and gained.
    pub old_found: bool,
}

/// The callsign an aircraft's ADS-B reports carry over the 60 s before a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallsignObs {
    /// All the callsigns of the window are this one (upper case, trimmed).
    Stable([u8; 8]),
    Unstable,
    #[default]
    Absent,
}

/// One question: the flight of an aircraft at a time.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default)]
pub struct Query {
    pub addr: u32,
    /// Unix ns.
    pub t_ns: i64,
    pub callsign: CallsignObs,
    /// The ADS-B position at the time, 1e-7 degree.
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Whether the callsign join is asked for as well as the address one.
    pub try_callsign: bool,
}

/// The longest a flight's own position may be from the query time and still check it, ms.
pub const POSITION_AGE_MS: i64 = 120_000;
/// The farthest a flight's own position may be from the ADS-B one, metres (10 NM).
pub const POSITION_DIST_M: f64 = 10.0 * 1852.0;
/// The window over which the ADS-B callsign must be stable, ns.
pub const STABLE_NS: i64 = 60 * 1_000_000_000;

/// The ADS-B reports of the aircraft of interest: time, position and callsign, by address, in
/// time order.
pub type Timeline = BTreeMap<u32, Vec<(i64, i32, i32, [u8; 8])>>;

/// The timeline of the reports from `from_ns` up to `to_ns` of the addresses in `want`.
pub fn timeline(
    r: &oa_adsb::Reports,
    want: &std::collections::BTreeSet<u32>,
    from_ns: i64,
    to_ns: i64,
) -> Timeline {
    let mut t: Timeline = BTreeMap::new();
    for i in 0..oa_adsb::reports_len(r) {
        let (Some(&a), Some(&ns), Some(&la), Some(&lo), Some(&c)) = (
            r.icao.get(i),
            r.time_ns.get(i),
            r.lat_e7.get(i),
            r.lon_e7.get(i),
            r.callsign.get(i),
        ) else {
            continue;
        };
        if ns >= from_ns && ns <= to_ns && want.contains(&a) {
            t.entry(a).or_default().push((ns, la, lo, norm_callsign(c)));
        }
    }
    for v in t.values_mut() {
        v.sort_by_key(|e| e.0);
    }
    t
}

/// How long before a time the latest ADS-B report may be and still give the aircraft's position, ns.
pub const ADSB_POSITION_AGE_NS: i64 = 60 * 1_000_000_000;

/// The callsign state of `addr` over the 60 s up to `t_ns`, and its latest position not after it
/// and within [`ADSB_POSITION_AGE_NS`].
pub fn observe(tl: &Timeline, addr: u32, t_ns: i64) -> (CallsignObs, Option<(i32, i32)>) {
    let Some(v) = tl.get(&addr) else {
        return (CallsignObs::Absent, None);
    };
    let hi = v.partition_point(|e| e.0 <= t_ns);
    let lo = v.partition_point(|e| e.0 <= t_ns - STABLE_NS);
    let window = v.get(lo..hi).unwrap_or(&[]);
    let mut seen: Option<[u8; 8]> = None;
    let mut obs = CallsignObs::Absent;
    for e in window.iter().filter(|e| e.3 != [0; 8]) {
        match seen {
            None => {
                seen = Some(e.3);
                obs = CallsignObs::Stable(e.3);
            }
            Some(c) if c == e.3 => {}
            Some(_) => {
                obs = CallsignObs::Unstable;
                break;
            }
        }
    }
    let pos = hi
        .checked_sub(1)
        .and_then(|k| v.get(k))
        .filter(|e| t_ns - e.0 <= ADSB_POSITION_AGE_NS)
        .map(|e| (e.1, e.2));
    (obs, pos)
}

/// Great-circle distance between two positions of 1e-7 degree, metres.
pub fn distance_m(a: (i32, i32), b: (i32, i32)) -> f64 {
    let (la1, lo1) = (f64::from(a.0) / 1e7, f64::from(a.1) / 1e7);
    let (la2, lo2) = (f64::from(b.0) / 1e7, f64::from(b.1) / 1e7);
    let (p1, p2) = (la1.to_radians(), la2.to_radians());
    let dl = (lo2 - lo1).to_radians();
    let h = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().min(1.0).asin()
}

/// The sfdps input of a run.
// A7.2: cold struct, size guard waived
pub struct Sfdps {
    pub dir: PathBuf,
    pub lag_floor_ms: i64,
    /// Take the newest plan where every flying flight with the callsign shares its departure and
    /// arrival.
    pub pick_newest: bool,
}

/// What the replay behind the answers saw (counts only).
// A7.2: cold struct, size guard waived
pub struct JoinStats {
    pub frames: u64,
    /// SFDPS frames and rows left out because their time is in a sealed window (OA-SEAL-014).
    pub sealed_window_dropped: u64,
    pub ops: u64,
    pub refused: u64,
    pub conflicts: u64,
    pub flights_held: usize,
    /// Flights that reported a position, and positions the bounded table could not take.
    pub positions: usize,
    pub positions_refused: u64,
    /// Flights with a refile time held, refile times the bounded table could not take, and
    /// positions left out for a stamp too far ahead of their receipt.
    pub refiles: usize,
    pub refiles_refused: u64,
    pub positions_bad_time: u64,
}

/// The flight-record configuration of the tool: the national day holds about 45,000 flights, so
/// 120,000 slots leave a margin of more than two.
fn flight_config(lag_floor_ms: i64) -> FlightConfig {
    FlightConfig {
        capacity: 120_000,
        lag_floor_ms,
        max_future_ms: 5_000,
        refusal_log: 64,
        conflict_log: 64,
        removal_log: 4096,
    }
}

fn cs_join(
    t: &FlightTable,
    pos: &Positions,
    q: &Query,
    pick_newest: bool,
) -> (CsResult, Option<Flight>) {
    let c = match q.callsign {
        CallsignObs::Absent => return (CsResult::Rejected(CsReject::NoCallsign), None),
        CallsignObs::Unstable => return (CsResult::Rejected(CsReject::Unstable), None),
        CallsignObs::Stable(c) => c,
    };
    let r = flights_flying_by_callsign(t, &c);
    let (several, chosen) = match (r.count, r.first, r.newest) {
        (0, _, _) | (_, None, _) => return (CsResult::Rejected(CsReject::NoLiveFlight), None),
        (1, Some(f), _) => (false, f),
        (_, Some(_), Some(n)) if pick_newest && r.same_route => (true, n),
        _ => return (CsResult::Rejected(CsReject::SeveralLive), None),
    };
    let t_ms = q.t_ns.div_euclid(1_000_000);
    let own = own_position(pos, &chosen, t_ms);
    match own {
        Some(p) if distance_m((p.lat_e7, p.lon_e7), (q.lat_e7, q.lon_e7)) > POSITION_DIST_M => {
            (CsResult::Rejected(CsReject::PositionDisagrees), None)
        }
        Some(_) => (
            CsResult::Accepted {
                checked: true,
                several,
            },
            Some(chosen),
        ),
        None => (
            CsResult::Accepted {
                checked: false,
                several,
            },
            Some(chosen),
        ),
    }
}

/// The flight's own latest position if it is stamped at or before the time and within
/// [`POSITION_AGE_MS`] of it.
fn own_position(pos: &Positions, f: &Flight, t_ms: i64) -> Option<Position> {
    (f.key.kind == KEY_GUFI)
        .then(|| fresh_position(pos.get(f.key.gufi), t_ms))
        .flatten()
}

/// A position stamped at or before `t_ms` and no more than [`POSITION_AGE_MS`] before it.
pub fn fresh_position(p: Option<&Position>, t_ms: i64) -> Option<Position> {
    p.filter(|p| p.ts_ms <= t_ms && t_ms - p.ts_ms <= POSITION_AGE_MS)
        .copied()
}

/// How two flights that the address and the callsign name differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagClass {
    /// The same callsign and the same departure and arrival: a refile or a second plan.
    SameRoute,
    /// The same callsign with another departure or arrival: another leg or day.
    OtherRoute,
    /// Different callsigns.
    OtherCallsign,
    /// Anything else (a callsign or a route not stated by one of them).
    Other,
}

pub const DIAG_NAMES: [&str; 4] = [
    "same callsign, same departure and arrival",
    "same callsign, other departure or arrival",
    "different callsigns",
    "anything else",
];

/// A disagreement between the address and the callsign, kept for the report. A7.2: cold struct,
/// size guard waived.
#[derive(Clone, Debug)]
pub struct Diag {
    /// The query (the aircraft) it is about.
    pub query: usize,
    pub class: DiagClass,
    pub t_ms: i64,
    pub addr_key: FlightKey,
    pub cs_key: FlightKey,
    /// How it was settled.
    pub resolution: Resolution,
    /// The callsign join was position checked, and took one of several plans.
    pub cs_checked: bool,
    pub cs_several: bool,
    /// Between the two flights' latest SFDPS positions, metres, where both have one.
    pub flights_m: Option<f64>,
    /// From the ADS-B position to each flight's own position within 120 s, metres.
    pub adsb_to_addr_m: Option<f64>,
    pub adsb_to_cs_m: Option<f64>,
    /// Each flight's latest position whatever its age: age at the query (ms) and distance from the
    /// ADS-B position (m).
    pub addr_any: Option<(i64, f64)>,
    pub cs_any: Option<(i64, f64)>,
    /// The address flight was flying (not dropped) at the query.
    pub addr_flying: bool,
    /// The two callsigns start with the same three letters (one operator).
    pub same_operator: bool,
    /// Looked at again 30 min later: whether each flight had stopped flying (ended or dropped).
    pub probed: bool,
    pub addr_ended_later: bool,
    pub cs_ended_later: bool,
    /// A refile conflict listed for either flight within 30 min of the query, either side.
    pub refile_near: bool,
}

/// How long after a question the two flights of a disagreement are looked at again, and the
/// window of a refile around it, ms.
pub const PROBE_MS: i64 = 30 * 60 * 1000;

fn diag_of(
    pos: &Positions,
    q: &Query,
    query: usize,
    (fa, fc): (&Flight, &Flight),
    (cs_checked, cs_several): (bool, bool),
    resolution: Resolution,
) -> Diag {
    let t_ms = q.t_ns.div_euclid(1_000_000);
    let class = if fa.callsign == [0; 8] || fc.callsign == [0; 8] {
        DiagClass::Other
    } else if fa.callsign != fc.callsign {
        DiagClass::OtherCallsign
    } else if fa.dep == [0; 12] || fa.arr == [0; 12] || fc.dep == [0; 12] || fc.arr == [0; 12] {
        DiagClass::Other
    } else if fa.dep == fc.dep && fa.arr == fc.arr {
        DiagClass::SameRoute
    } else {
        DiagClass::OtherRoute
    };
    let last = |f: &Flight| {
        (f.key.kind == KEY_GUFI)
            .then(|| pos.get(f.key.gufi))
            .flatten()
            .copied()
    };
    let flights_m = last(fa)
        .zip(last(fc))
        .map(|(x, y)| distance_m((x.lat_e7, x.lon_e7), (y.lat_e7, y.lon_e7)));
    let adsb = (q.lat_e7, q.lon_e7);
    let to =
        |f: &Flight| own_position(pos, f, t_ms).map(|p| distance_m((p.lat_e7, p.lon_e7), adsb));
    let any =
        |f: &Flight| last(f).map(|p| (t_ms - p.ts_ms, distance_m((p.lat_e7, p.lon_e7), adsb)));
    Diag {
        query,
        class,
        resolution,
        t_ms,
        addr_key: fa.key,
        cs_key: fc.key,
        cs_checked,
        cs_several,
        flights_m,
        adsb_to_addr_m: to(fa),
        adsb_to_cs_m: to(fc),
        addr_any: any(fa),
        cs_any: any(fc),
        addr_flying: flight_is_flying(fa),
        same_operator: fa.callsign.get(..3) == fc.callsign.get(..3),
        probed: false,
        addr_ended_later: false,
        cs_ended_later: false,
        refile_near: false,
    }
}

/// Whether the flight's own position within 120 s is within 10 NM of the ADS-B position.
fn position_passes(pos: &Positions, f: &Flight, q: &Query) -> bool {
    let t_ms = q.t_ns.div_euclid(1_000_000);
    own_position(pos, f, t_ms)
        .is_some_and(|p| distance_m((p.lat_e7, p.lon_e7), (q.lat_e7, q.lon_e7)) <= POSITION_DIST_M)
}

/// The most flying claimants of one address that are copied and looked at. An airframe states its
/// address on a few legs a day; an address with more flying is counted in the report and left
/// unresolved. The walk of the chain is the whole of it, whatever the cap.
pub const FLYING_CAP: usize = 64;

/// Whether some view of the flight is active or departed.
fn is_underway(f: &Flight) -> bool {
    f.views
        .iter()
        .take(usize::from(f.n_views))
        .any(|v| v.state == ST_ACTIVE || v.state == ST_DEPARTED)
}

/// The address join over every claimant of the address that is flying at the query time. Several
/// flying are narrowed in turn, each step only narrowing (a step that would leave none changes
/// nothing): by the ADS-B callsign, by state (active or departed over proposed), by the position
/// check.
fn addr_join(t: &FlightTable, pos: &Positions, q: &Query) -> (AddrResult, Option<Flight>) {
    let mut flying = Vec::new();
    let c = flights_claiming_address(t, q.addr, &mut flying, FLYING_CAP);
    if c.total == 0 {
        return (AddrResult::NoClaimant, None);
    }
    if c.flying == 0 {
        let r = if c.sole_dropped {
            AddrResult::OnlyClaimantDropped
        } else {
            AddrResult::NoneFlying
        };
        return (r, None);
    }
    if c.flying > FLYING_CAP {
        return (AddrResult::SeveralOverCap, None);
    }
    if let [f] = flying.as_slice() {
        return (AddrResult::One, Some(*f));
    }
    if let CallsignObs::Stable(cs) = q.callsign {
        let kept: Vec<Flight> = flying
            .iter()
            .copied()
            .filter(|f| f.callsign == cs)
            .collect();
        if let [f] = kept.as_slice() {
            return (AddrResult::SeveralCallsignDecides, Some(*f));
        }
        if !kept.is_empty() {
            flying = kept;
        }
    }
    let kept: Vec<Flight> = flying.iter().copied().filter(is_underway).collect();
    if let [f] = kept.as_slice() {
        return (AddrResult::SeveralStateDecides, Some(*f));
    }
    if !kept.is_empty() {
        flying = kept;
    }
    let mut passing = flying.iter().filter(|f| position_passes(pos, f, q));
    match (passing.next(), passing.next()) {
        (Some(f), None) => (AddrResult::SeveralByPosition, Some(*f)),
        _ => (AddrResult::SeveralUnresolved, None),
    }
}

fn lookup(
    t: &FlightTable,
    pos: &Positions,
    q: &Query,
    query: usize,
    pick_newest: bool,
) -> (Answer, Option<Diag>) {
    let mut a = Answer::default();
    let mut chosen = None;
    let (addr, addr_f) = addr_join(t, pos, q);
    a.addr = addr;
    a.ended = matches!(
        addr,
        AddrResult::NoneFlying | AddrResult::OnlyClaimantDropped
    );
    let old_addr = flight_by_address(t, q.addr).is_some_and(|f| flight_is_live(&f));
    if let Some(f) = addr_f {
        a.addr_flight = Some(f.key);
        a.key = Key::Address;
        chosen = Some(f);
    }
    let mut diag = None;
    if q.try_callsign {
        let (r, f) = cs_join(t, pos, q, pick_newest);
        a.cs = r;
        a.cs_flight = f.map(|f| f.key);
        if let (CsResult::Accepted { checked, several }, Some(fc)) = (r, f) {
            match addr_f {
                None => {
                    a.key = if checked {
                        Key::CallsignChecked
                    } else {
                        Key::CallsignUnchecked
                    };
                    chosen = Some(fc);
                }
                Some(fa) if fa.key != fc.key => {
                    // Take the flight whose own position agrees with the ADS-B one; if both or
                    // neither does, the address flight stands.
                    let addr_pass = position_passes(pos, &fa, q);
                    let resolution = match (addr_pass, checked) {
                        (false, true) => Resolution::CallsignTaken,
                        (true, false) => Resolution::AddressTaken,
                        (true, true) => Resolution::BothPass,
                        (false, false) => Resolution::NeitherPass,
                    };
                    if resolution == Resolution::CallsignTaken {
                        a.key = Key::CallsignOverAddress;
                        chosen = Some(fc);
                    }
                    diag = Some(diag_of(
                        pos,
                        q,
                        query,
                        (&fa, &fc),
                        (checked, several),
                        resolution,
                    ));
                }
                Some(_) => {}
            }
        }
    }
    a.old_found = old_addr || matches!(a.cs, CsResult::Accepted { .. });
    if let Some(f) = chosen {
        a.flight = true;
        a.assigned = f.assigned;
        a.interim = f.interim;
        a.active = is_underway(&f);
    }
    (a, diag)
}

/// The answers to `queries`, in the order given, and the disagreements between the two keys. A
/// query at time T is answered before the first operation that is available after T is applied,
/// and sees the positions of the same moment: for the address join and the callsign join alike.
/// A disagreement is looked at again [`PROBE_MS`] later, which is for the report only and joins
/// nothing.
pub fn answer_queries(
    s: &Sfdps,
    queries: &[Query],
) -> Result<(Vec<Answer>, Vec<Diag>, JoinStats), String> {
    answer_queries_counted(s, queries, &mut 0)
}

/// [`answer_queries`], with the frames and messages the replay dropped in a sealed window put in
/// `sealed` on every exit.
pub fn answer_queries_counted(
    s: &Sfdps,
    queries: &[Query],
    sealed: &mut u64,
) -> Result<(Vec<Answer>, Vec<Diag>, JoinStats), String> {
    let mut order: Vec<usize> = (0..queries.len()).collect();
    order.sort_by_key(|&k| queries.get(k).map_or(0, |q| q.t_ns));
    let mut answers = vec![Answer::default(); queries.len()];
    let mut diags: Vec<Diag> = Vec::new();
    let mut probes: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();
    let end_ns = queries.iter().map(|q| q.t_ns).max().unwrap_or(i64::MIN);
    let paths = recordings_in(&s.dir).map_err(|e| e.to_string())?;
    let cfg = ReplayConfig {
        flight: flight_config(s.lag_floor_ms),
        from_ns: i64::MIN,
        to_ns: end_ns.saturating_add((PROBE_MS + 60_000).saturating_mul(1_000_000)),
        sweep_every_ms: 60_000,
        retain_ms: 3_600_000,
        silence_ms: 6 * 3_600_000,
    };
    let mut next = 0usize;
    let rep = replay_sfdps_counted(
        paths,
        &cfg,
        &mut |t, pos, avail_ms| loop {
            let q_ms = order
                .get(next)
                .and_then(|&k| queries.get(k))
                .map(|q| q.t_ns.div_euclid(1_000_000));
            let p_ms = probes.peek().map(|r| r.0 .0);
            match (q_ms, p_ms) {
                (Some(q), p) if q < avail_ms && p.is_none_or(|p| q <= p) => {
                    let k = order.get(next).copied().unwrap_or(0);
                    if let (Some(q), Some(slot)) = (queries.get(k), answers.get_mut(k)) {
                        let (a, d) = lookup(t, pos, q, k, s.pick_newest);
                        *slot = a;
                        if let Some(d) = d {
                            probes.push(Reverse((d.t_ms.saturating_add(PROBE_MS), diags.len())));
                            diags.push(d);
                        }
                    }
                    next += 1;
                }
                (_, Some(p)) if p < avail_ms => {
                    if let Some(Reverse((_, i))) = probes.pop() {
                        if let Some(d) = diags.get_mut(i) {
                            probe(t, pos, d);
                        }
                    }
                }
                _ => break,
            }
        },
        sealed,
    )
    .map_err(|e| e.to_string())?;
    let st = &rep.table.stats;
    let stats = JoinStats {
        frames: rep.frames,
        sealed_window_dropped: rep.sealed_window_dropped,
        ops: st.ops,
        refused: st.refused.iter().sum(),
        conflicts: st.conflicts.iter().sum(),
        flights_held: oa_flight::flights_len(&rep.table),
        positions: rep.positions.len(),
        positions_refused: rep.positions.refused_full,
        refiles: rep.positions.refiled_len(),
        refiles_refused: rep.positions.refiled_refused,
        positions_bad_time: rep.positions.bad_time,
    };
    Ok((answers, diags, stats))
}

/// Looks at the two flights of a disagreement again, 30 min on.
fn probe(t: &FlightTable, pos: &Positions, d: &mut Diag) {
    let ended = |k: &FlightKey| flight_get(t, k).is_some_and(|f| !flight_is_flying(&f));
    d.addr_ended_later = ended(&d.addr_key);
    d.cs_ended_later = ended(&d.cs_key);
    let near = |k: &FlightKey| {
        (k.kind == KEY_GUFI)
            .then(|| pos.refiled_at(k.gufi))
            .flatten()
            .is_some_and(|r| (r - d.t_ms).abs() <= PROBE_MS)
    };
    d.refile_near = near(&d.addr_key) || near(&d.cs_key);
    d.probed = true;
}

/// The clearance a pair is judged on: `Some(feet)` for a simple altitude, else why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clearance {
    Feet(i32),
    /// VFR, VFR on top, a block, an altitude at a fix, or none: no single level.
    NotSimple,
    /// No altitude held.
    Missing,
}

fn simple(a: &Alt) -> Clearance {
    match a.form {
        ALT_SIMPLE => Clearance::Feet(a.a_ft),
        ALT_VFR | ALT_VFR_ON_TOP_PLUS | ALT_BLOCK | ALT_AT_FIX => Clearance::NotSimple,
        _ => Clearance::Missing,
    }
}

/// The assigned altitude only.
pub fn assigned_of(a: &Answer) -> Clearance {
    simple(&a.assigned)
}

/// The interim altitude where one is held (a temporary level wins), else the assigned one; an
/// interim stated as none (cleared) leaves the assigned one.
pub fn effective_of(a: &Answer) -> Clearance {
    match a.interim.form {
        ALT_SIMPLE => Clearance::Feet(a.interim.a_ft),
        ALT_NIL => simple(&a.assigned),
        _ => match simple(&a.interim) {
            Clearance::Missing => simple(&a.assigned),
            c => c,
        },
    }
}

/// How one climbing or descending encounter near 1,000 ft resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Cleared to altitudes 1,000 ft or more apart: the aircraft stops 1,000 ft apart.
    LevelOff,
    /// Cleared to altitudes less than 1,000 ft apart (including the same): the climb or descent
    /// does not stop here.
    ClimbThrough,
    NoFlightRecord,
    NoAssigned,
    NotSimple,
}

pub const VERDICT_NAMES: [&str; 5] = [
    "level-off (cleared 1,000 ft or more apart)",
    "climb or descent that does not stop (cleared less than 1,000 ft apart)",
    "no flight record for one or both aircraft",
    "a flight record with no altitude for one or both",
    "an altitude that is not one level (VFR, block, at a fix) for one or both",
];

fn verdict(a: &Answer, b: &Answer, of: fn(&Answer) -> Clearance) -> Verdict {
    if !a.flight || !b.flight {
        return Verdict::NoFlightRecord;
    }
    match (of(a), of(b)) {
        (Clearance::Feet(x), Clearance::Feet(y)) => {
            if (x - y).abs() >= LEVEL_OFF_FT {
                Verdict::LevelOff
            } else {
                Verdict::ClimbThrough
            }
        }
        (Clearance::NotSimple, _) | (_, Clearance::NotSimple) => Verdict::NotSimple,
        _ => Verdict::NoAssigned,
    }
}

fn vindex(v: Verdict) -> usize {
    match v {
        Verdict::LevelOff => 0,
        Verdict::ClimbThrough => 1,
        Verdict::NoFlightRecord => 2,
        Verdict::NoAssigned => 3,
        Verdict::NotSimple => 4,
    }
}

/// Whether an encounter is one of the climbing or descending encounters near 1,000 ft: airborne
/// context, not the at-vertical-minimum subgroup, a vertical separation at the closest approach
/// from [`NEAR_FROM_FT`] up to the volume's 1,000 ft, and either aircraft at [`CLIMB_FPM`] or more.
pub fn is_near_climb(e: &Encounter, airports: &[Airport], volume_v_ft: f32) -> bool {
    context_of(e, airports) == 0
        && !at_vertical_minimum(e, volume_v_ft)
        && e.v_at_closest_ft >= NEAR_FROM_FT
        && e.v_at_closest_ft < volume_v_ft
        && e.max_abs_vrate_fpm >= CLIMB_FPM
}

/// The split of the encounters near 1,000 ft, counts only, as text for the summary.
pub fn join_text(
    encounters: &[Encounter],
    answers: &[Answer],
    airports: &[Airport],
    volume_v_ft: f32,
    stats: &JoinStats,
    diags: &[Diag],
    window: &str,
) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "\nFLIGHT RECORD JOIN\n");
    let _ = writeln!(o, "  window: {window}");
    let _ = writeln!(
        o,
        "  flight record: {} SFDPS frames replayed, {} operations, {} refused, {} conflicts listed, {} flights held at the end",
        stats.frames, stats.ops, stats.refused, stats.conflicts, stats.flights_held
    );
    let _ = writeln!(
        o,
        "  positions held at the end {}, positions refused (table full) {}, refile times held {}, refiles refused (table full) {}, positions left out for a stamp too far ahead {}",
        stats.positions,
        stats.positions_refused,
        stats.refiles,
        stats.refiles_refused,
        stats.positions_bad_time
    );
    let _ = writeln!(
        o,
        "  the flight record is read as of each encounter's closest approach: an operation counts from the later of its receipt and its timestamp plus the lag floor"
    );
    let _ = writeln!(
        o,
        "  the group: AIRBORNE, not AT_VERTICAL_MINIMUM, vertical separation at the closest approach {NEAR_FROM_FT:.0} ft up to {volume_v_ft:.0} ft, either aircraft at {CLIMB_FPM:.0} fpm or more; LEVEL_FT is {LEVEL_FT:.0}"
    );
    let ans = |i: usize| answers.get(i).copied().unwrap_or_default();
    // Encounter k asked 2k (address a) and 2k + 1 (address b).
    let mut before: BTreeMap<u8, u64> = BTreeMap::new();
    let mut by_class: BTreeMap<u8, [[u64; 5]; 2]> = BTreeMap::new();
    let (mut rec_both, mut rec_one, mut rec_none, mut ended_any) = (0u64, 0u64, 0u64, 0u64);
    let (mut asg_both, mut asg_one, mut asg_none) = (0u64, 0u64, 0u64);
    let mut n = 0u64;
    for (k, e) in encounters.iter().enumerate() {
        if !is_near_climb(e, airports, volume_v_ft) {
            continue;
        }
        n += 1;
        *before.entry(e.class).or_default() += 1;
        let (a, b) = (ans(2 * k), ans(2 * k + 1));
        match u8::from(a.flight) + u8::from(b.flight) {
            2 => rec_both += 1,
            1 => rec_one += 1,
            _ => rec_none += 1,
        }
        if a.ended || b.ended {
            ended_any += 1;
        }
        match u8::from(matches!(assigned_of(&a), Clearance::Feet(_)))
            + u8::from(matches!(assigned_of(&b), Clearance::Feet(_)))
        {
            2 => asg_both += 1,
            1 => asg_one += 1,
            _ => asg_none += 1,
        }
        for (v, of) in [assigned_of as fn(&Answer) -> Clearance, effective_of]
            .into_iter()
            .enumerate()
        {
            let r = verdict(&a, &b, of);
            if let Some(c) = by_class
                .entry(e.class)
                .or_default()
                .get_mut(v)
                .and_then(|t| t.get_mut(vindex(r)))
            {
                *c += 1;
            }
        }
    }
    let _ = writeln!(
        o,
        "\n  before (one group): {n} climbing or descending encounters near 1,000 ft"
    );
    for (c, m) in &before {
        let _ = writeln!(o, "    {}: {m}", class_name(*c));
    }
    let _ = writeln!(o, "  flight record found: both aircraft {rec_both}, one {rec_one}, neither {rec_none}; a flight whose plans had all ended for either: {ended_any}");
    let _ = writeln!(o, "  assigned altitude held as one level: both aircraft {asg_both}, one {asg_one}, neither {asg_none}");
    for (v, name) in ["assigned altitude", "interim altitude over assigned"]
        .iter()
        .enumerate()
    {
        let _ = writeln!(o, "\n  after, judged on the {name}, by class:");
        for (c, t) in &by_class {
            let _ = writeln!(o, "    {}:", class_name(*c));
            for (k, label) in VERDICT_NAMES.iter().enumerate() {
                let n = t.get(v).and_then(|t| t.get(k)).copied().unwrap_or(0);
                let _ = writeln!(o, "      {label}: {n}");
            }
        }
        let _ = writeln!(o, "    all classes:");
        for (k, label) in VERDICT_NAMES.iter().enumerate() {
            let n: u64 = by_class
                .values()
                .map(|t| t.get(v).and_then(|t| t.get(k)).copied().unwrap_or(0))
                .sum();
            let _ = writeln!(o, "      {label}: {n}");
        }
    }
    o.push_str(&key_text(encounters, answers, airports, volume_v_ft));
    o.push_str(&diag_text(encounters, diags, airports, volume_v_ft));
    o
}

/// The median: the mean of the middle two where there is an even number.
fn median(v: &mut [f64]) -> Option<f64> {
    v.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let n = v.len();
    match (n.checked_sub(1).map(|k| k / 2), v.get(n / 2)) {
        (Some(lo), Some(hi)) => Some((v.get(lo).copied()? + hi) / 2.0),
        _ => None,
    }
}

/// The disagreements between the two keys, by kind. Counts and distances only.
fn diag_text(
    encounters: &[Encounter],
    diags: &[Diag],
    airports: &[Airport],
    volume_v_ft: f32,
) -> String {
    let mut o = String::new();
    let in_group = |d: &Diag| {
        encounters
            .get(d.query / 2)
            .is_some_and(|e| is_near_climb(e, airports, volume_v_ft))
    };
    let km = |m: f64| m / 1852.0;
    for (label, only_group) in [("group aircraft", true), ("every aircraft tried", false)] {
        let sel: Vec<&Diag> = diags
            .iter()
            .filter(|d| !only_group || in_group(d))
            .collect();
        let _ = writeln!(o, "  disagreements, {label}: {}", sel.len());
        for (k, name) in RESOLUTION_NAMES.iter().enumerate() {
            let n = sel
                .iter()
                .filter(|d| {
                    matches!(
                        (k, d.resolution),
                        (0, Resolution::CallsignTaken)
                            | (1, Resolution::AddressTaken)
                            | (2, Resolution::BothPass)
                            | (3, Resolution::NeitherPass)
                    )
                })
                .count();
            let _ = writeln!(o, "    settled, {name}: {n}");
        }
        for (k, name) in DIAG_NAMES.iter().enumerate() {
            let of_class: Vec<&&Diag> = sel
                .iter()
                .filter(|d| {
                    matches!(
                        (k, d.class),
                        (0, DiagClass::SameRoute)
                            | (1, DiagClass::OtherRoute)
                            | (2, DiagClass::OtherCallsign)
                            | (3, DiagClass::Other)
                    )
                })
                .collect();
            let _ = write!(o, "    {name}: {}", of_class.len());
            {
                let ended = of_class
                    .iter()
                    .filter(|d| d.probed && (d.addr_ended_later || d.cs_ended_later))
                    .count();
                let refiled = of_class.iter().filter(|d| d.refile_near).count();
                let either = of_class
                    .iter()
                    .filter(|d| {
                        d.probed && (d.addr_ended_later || d.cs_ended_later || d.refile_near)
                    })
                    .count();
                let mut dist: Vec<f64> = of_class.iter().filter_map(|d| d.flights_m).collect();
                let n_dist = dist.len();
                let within = dist.iter().filter(|m| **m <= POSITION_DIST_M).count();
                let med = median(&mut dist)
                    .map_or_else(|| "none".to_string(), |m| format!("{:.1} NM", km(m)));
                let _ = write!(
                    o,
                    "; a plan ended or dropped within 30 min after: {ended}; refile conflict within 30 min: {refiled}; either: {either}; last positions of both flights known: {n_dist} (within 10 NM {within}, median {med})"
                );
            }
            let _ = writeln!(o);
        }
        // The position check, over the disagreements the callsign join had checked.
        let chk: Vec<&&Diag> = sel.iter().filter(|d| d.cs_checked).collect();
        let mut to_cs: Vec<f64> = chk.iter().filter_map(|d| d.adsb_to_cs_m).collect();
        let mut to_addr: Vec<f64> = chk.iter().filter_map(|d| d.adsb_to_addr_m).collect();
        let addr_within = to_addr.iter().filter(|m| **m <= POSITION_DIST_M).count();
        let (n_cs, n_addr) = (to_cs.len(), to_addr.len());
        let m_cs =
            median(&mut to_cs).map_or_else(|| "none".to_string(), |m| format!("{:.2} NM", km(m)));
        let m_addr =
            median(&mut to_addr).map_or_else(|| "none".to_string(), |m| format!("{:.2} NM", km(m)));
        let any_addr: Vec<(i64, f64)> = sel.iter().filter_map(|d| d.addr_any).collect();
        let mut age: Vec<f64> = any_addr.iter().map(|a| a.0 as f64 / 1000.0).collect();
        let mut dist: Vec<f64> = any_addr.iter().map(|a| a.1).collect();
        let within = dist.iter().filter(|m| **m <= POSITION_DIST_M).count();
        let (n_any, m_age) = (age.len(), median(&mut age));
        let m_dist =
            median(&mut dist).map_or_else(|| "none".to_string(), |m| format!("{:.1} NM", km(m)));
        let _ = writeln!(
            o,
            "    address flights with any SFDPS position: {n_any} of {} (age at the query: median {} s; distance from ADS-B: median {m_dist}, within 10 NM {within}); address flight flying (not dropped): {}; callsigns of one operator (same first three letters): {}",
            sel.len(),
            m_age.map_or_else(|| "none".to_string(), |a| format!("{a:.0}")),
            sel.iter().filter(|d| d.addr_flying).count(),
            sel.iter().filter(|d| d.same_operator).count()
        );
        let _ = writeln!(
            o,
            "    position-checked disagreements: {}; ADS-B to the callsign flight's position: median {m_cs} (of {n_cs}); to the address flight's: median {m_addr} (of {n_addr} with a position within 120 s, {addr_within} within 10 NM)",
            chk.len()
        );
    }
    o
}

/// The two joins compared where both name a flight.
#[derive(Clone, Copy, Default)]
struct Cross {
    both: u64,
    same: u64,
    diff: u64,
    checked: u64,
    diff_checked: u64,
    diff_unchecked: u64,
}

impl Cross {
    fn add(&mut self, same: bool, checked: bool) {
        self.both += 1;
        self.checked += u64::from(checked);
        if same {
            self.same += 1;
        } else {
            self.diff += 1;
            if checked {
                self.diff_checked += 1;
            } else {
                self.diff_unchecked += 1;
            }
        }
    }
}

/// The weakness of a key, for the weakest of a pair: the address first, a position-checked
/// callsign (alone or over the address) next, then a callsign with no position check.
fn key_rank(k: Key) -> u8 {
    match k {
        Key::Address => 0,
        Key::CallsignChecked | Key::CallsignOverAddress => 1,
        Key::CallsignUnchecked => 2,
        Key::None => 3,
    }
}

/// The slot of a key in the per-key table.
fn key_slot(k: Key) -> usize {
    match k {
        Key::Address => 0,
        Key::CallsignChecked => 1,
        Key::CallsignUnchecked => 2,
        Key::CallsignOverAddress => 3,
        Key::None => 4,
    }
}

const KEY_NAMES: [&str; 5] = [
    "address",
    "callsign, position checked",
    "callsign, no position check",
    "callsign over the address (only its position agreed)",
    "none",
];

/// The join split by the key that found the flight, the callsign rejections, and the two keys
/// compared where both are possible. Counts only.
fn addr_index(r: AddrResult) -> usize {
    match r {
        AddrResult::NoClaimant => 0,
        AddrResult::OnlyClaimantDropped => 1,
        AddrResult::NoneFlying => 2,
        AddrResult::One => 3,
        AddrResult::SeveralCallsignDecides => 4,
        AddrResult::SeveralStateDecides => 5,
        AddrResult::SeveralByPosition => 6,
        AddrResult::SeveralUnresolved => 7,
        AddrResult::SeveralOverCap => 8,
    }
}

fn key_text(
    encounters: &[Encounter],
    answers: &[Answer],
    airports: &[Airport],
    volume_v_ft: f32,
) -> String {
    let mut o = String::new();
    let ans = |i: usize| answers.get(i).copied().unwrap_or_default();
    let _ = writeln!(
        o,
        "\n  join by key (the callsign is tried where the address names no flight, and may override the address flight where its position agrees and the address flight's does not)"
    );
    let _ = writeln!(
        o,
        "  callsign rule: one flying flight with the callsign (not ended, not dropped); its own SFDPS position within {} s, if it has one, within 10 NM of the ADS-B one; the ADS-B callsign stable over the previous 60 s",
        POSITION_AGE_MS / 1000
    );
    let mut per_key = [0u64; 5];
    let mut addr_results = [0u64; 9];
    // Encounters with a record for both before the walk and not after, by cause; and the reverse.
    let (mut lost, mut gained) = ([0u64; 3], 0u64);
    let mut rejects = [0u64; 5];
    let (mut aircraft, mut not_tried, mut several) = (0u64, 0u64, 0u64);
    let (mut addr_both, mut addr_one, mut addr_none) = (0u64, 0u64, 0u64);
    let (mut both, mut one, mut none) = (0u64, 0u64, 0u64);
    let mut weakest = [0u64; 3];
    let mut by_weakest = [[0u64; 5]; 3];
    let mut n = 0u64;
    // [group, all tried]: both possible, same flight, different flight, checked-different, unchecked-different.
    let mut cross = [Cross::default(); 2];
    for (k, e) in encounters.iter().enumerate() {
        let (a, b) = (ans(2 * k), ans(2 * k + 1));
        let group = is_near_climb(e, airports, volume_v_ft);
        for x in [&a, &b] {
            if x.cs == CsResult::NotTried {
                continue;
            }
            if let (Some(af), Some(cf), CsResult::Accepted { checked, .. }) =
                (x.addr_flight, x.cs_flight, x.cs)
            {
                for (c, on) in cross.iter_mut().zip([group, true]) {
                    if on {
                        c.add(af == cf, checked);
                    }
                }
            }
        }
        if !group {
            continue;
        }
        n += 1;
        let v = verdict(&a, &b, assigned_of);
        for x in [&a, &b] {
            aircraft += 1;
            if let Some(c) = per_key.get_mut(key_slot(x.key)) {
                *c += 1;
            }
            if x.cs == CsResult::NotTried {
                not_tried += 1;
            }
            if let Some(c) = addr_results.get_mut(addr_index(x.addr)) {
                *c += 1;
            }
            if x.addr_flight.is_none() && matches!(x.cs, CsResult::Accepted { several: true, .. }) {
                several += 1;
            }
            if x.addr_flight.is_none() {
                if let CsResult::Rejected(r) = x.cs {
                    if let Some(c) = rejects.get_mut(reject_index(r)) {
                        *c += 1;
                    }
                }
            }
        }
        let (old_both, new_both) = (a.old_found && b.old_found, a.flight && b.flight);
        if old_both && !new_both {
            // The cause of the aircraft that lost its record; where both did, the first in
            // this order: only claimant dropped, several flying unresolved, anything else.
            let cause = |x: &Answer| match x.addr {
                _ if x.flight => 3,
                AddrResult::OnlyClaimantDropped => 0,
                AddrResult::SeveralUnresolved => 1,
                _ => 2,
            };
            if let Some(c) = lost.get_mut(cause(&a).min(cause(&b))) {
                *c += 1;
            }
        }
        gained += u64::from(!old_both && new_both);
        match u8::from(a.addr_flight.is_some()) + u8::from(b.addr_flight.is_some()) {
            2 => addr_both += 1,
            1 => addr_one += 1,
            _ => addr_none += 1,
        }
        match u8::from(a.flight) + u8::from(b.flight) {
            2 => {
                both += 1;
                let w = key_rank(a.key).max(key_rank(b.key)).min(2);
                if let Some(c) = weakest.get_mut(usize::from(w)) {
                    *c += 1;
                }
                if let Some(c) = by_weakest
                    .get_mut(usize::from(w))
                    .and_then(|r| r.get_mut(vindex(v)))
                {
                    *c += 1;
                }
            }
            1 => one += 1,
            _ => none += 1,
        }
    }
    let _ = writeln!(
        o,
        "  the group: {n} encounters, {aircraft} aircraft ({not_tried} without the callsign join)"
    );
    let _ = writeln!(o, "  aircraft by the key that found their flight:");
    for (name, c) in KEY_NAMES.iter().zip(per_key) {
        let _ = writeln!(o, "    {name}: {c}");
    }
    let _ = writeln!(
        o,
        "  the address join, over every flight that claims the address:"
    );
    for (name, c) in ADDR_NAMES.iter().zip(addr_results) {
        let _ = writeln!(o, "    {name}: {c}");
    }
    let _ = writeln!(
        o,
        "  compared with a re-derived old join (the newest claimant only, a dropped flight counted), record for both under the old and not now, by cause: only claimant dropped {}, several flying not resolved and the callsign found nothing {}, anything else {}; the reverse (a record for both now and not under the old join): {gained}",
        lost[0], lost[1], lost[2]
    );
    let _ = writeln!(
        o,
        "  of the callsign joins, one aircraft with several plans (the newest taken): {several}"
    );
    let _ = writeln!(
        o,
        "  callsign rejections, aircraft whose address named no live flight:"
    );
    for (name, c) in CS_REJECT_NAMES.iter().zip(rejects) {
        let _ = writeln!(o, "    {name}: {c}");
    }
    let _ = writeln!(o, "  flight record for both aircraft, address only: {addr_both} of {n}; one {addr_one}, neither {addr_none}");
    let _ = writeln!(o, "  flight record for both aircraft, address then callsign: {both} of {n}; one {one}, neither {none}");
    let _ = writeln!(
        o,
        "  encounters with a record for both, by the weakest key of the two:"
    );
    for (k, name) in KEY_NAMES.iter().take(3).enumerate() {
        let r = by_weakest.get(k).copied().unwrap_or([0; 5]);
        let _ = writeln!(
            o,
            "    {name}: {} (level-off {}, does not stop {}, no altitude {}, not one level {})",
            weakest.get(k).copied().unwrap_or(0),
            r[0],
            r[1],
            r[3],
            r[4]
        );
    }
    let _ = writeln!(
        o,
        "  address and callsign compared where both name a flight (the callsign error):"
    );
    for (label, c) in [
        ("group aircraft", cross[0]),
        ("every aircraft the callsign join was tried for", cross[1]),
    ] {
        let _ = writeln!(
            o,
            "    {label}: both possible {}, the same flight {}, different flights {} (callsign checked {} of the both-possible, different with check {}, different without {})",
            c.both, c.same, c.diff, c.checked, c.diff_checked, c.diff_unchecked
        );
    }
    o
}

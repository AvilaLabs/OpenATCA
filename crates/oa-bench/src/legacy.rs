//! The legacy samples: what ERAM, TBFM and TFMS published, as struct-of-arrays tables. Core
//! (pure): the ingest functions take the bytes of one message and a receive time as input.
//!
//! * [`Flights`] holds one row per SFDPS flight (the GUFI UUID is the flight key), with callsign,
//!   `aircraftAddress` where any message carried it, the flight plan identifier (the key TBFM
//!   and TFMS share with SFDPS, `docs/TFMS_SURVEY.md`), departure and arrival.
//! * [`ThTable`] holds ERAM track (`TH`) positions: position time, receive time, position,
//!   reported altitude, track velocity and the coast flag.
//! * [`HandoffTable`] holds `boundaryCrossings` handoff events; [`RouteTable`] with
//!   [`RoutePoints`] holds ERAM's expanded routes (`HX`).
//! * [`TbfmTable`] holds TBFM meter-fix estimates and schedules with their issue time;
//!   [`TfmsTable`] holds TFMS arrival and departure estimates with their issue time.
//!
//! Every table has a stated capacity ([`MAX_TH_SAMPLES`] and the others); a row that does not fit
//! is dropped and counted in [`Counts`], never silently (RULESET C2). Memory grows with the rows
//! kept and is bounded by the capacities.

use std::collections::HashMap;

use oa_fixm::{code_str, has_field, Field};

/// "No index" for flights and addresses (RULESET A4).
pub const NONE: u32 = u32::MAX;
/// Absent time, Unix ms.
pub const NO_TIME: i64 = i64::MIN;
/// Absent altitude, feet.
pub const NO_ALT: i32 = i32::MIN;
/// Absent velocity component, 0.1 knot.
pub const NO_VEL: i16 = i16::MIN;
/// Absent coordinate, 1e-7 degree.
pub const NO_COORD: i32 = i32::MIN;

/// Most flights. A day of national SFDPS is under 100,000 flight plans; 4 million is 40 times.
pub const MAX_FLIGHTS: usize = 1 << 22;
/// Most ERAM track samples (about 60 bytes each, so under 1 GB). The 2026-11-14 sample held
/// 13,268 in 1,917 files; 16 million is about a day at that rate with headroom.
pub const MAX_TH_SAMPLES: usize = 16 << 20;
/// Most handoff events (the sample held 338 per 1,917 files).
pub const MAX_HANDOFFS: usize = 1 << 22;
/// Most expanded routes (265 per 1,917 files in the sample).
pub const MAX_ROUTES: usize = 1 << 21;
/// Most route points over all routes (about 5 per route in the sample; 16 million).
pub const MAX_ROUTE_POINTS: usize = 1 << 24;
/// Most TBFM estimate and schedule entries.
pub const MAX_TBFM: usize = 1 << 23;
/// Most TFMS estimate entries.
pub const MAX_TFMS: usize = 1 << 23;

/// The capacities of the tables (cold: A7.2); the defaults are the `MAX_*` constants. Tests lower
/// them to exercise the overflow counting.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub flights: usize,
    pub th: usize,
    pub handoffs: usize,
    pub routes: usize,
    pub route_points: usize,
    pub tbfm: usize,
    pub tfms: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            flights: MAX_FLIGHTS,
            th: MAX_TH_SAMPLES,
            handoffs: MAX_HANDOFFS,
            routes: MAX_ROUTES,
            route_points: MAX_ROUTE_POINTS,
            tbfm: MAX_TBFM,
            tfms: MAX_TFMS,
        }
    }
}

/// Handoff event codes.
pub mod hev {
    pub const OTHER: u8 = 0;
    pub const INITIATION: u8 = 1;
    pub const ACCEPTANCE: u8 = 2;
    pub const UPDATE: u8 = 3;
    pub const RETRACTION: u8 = 4;
    pub const FAILURE: u8 = 5;
    pub const NAMES: [&str; 6] = [
        "other",
        "initiation",
        "acceptance",
        "update",
        "retraction",
        "failure",
    ];
}

/// TBFM table kinds and points.
pub mod tbfm_kind {
    /// ETA at the meter fix (`eta_mfx`).
    pub const ETA_MFX: u8 = 1;
    /// ETA at the runway (`eta_rwy`).
    pub const ETA_RWY: u8 = 2;
    /// Scheduled time at the scheduling fix (`sta_sfx`); the feed has no `sta_mfx`.
    pub const STA_SFX: u8 = 3;
    /// Scheduled time at the runway (`sta_rwy`).
    pub const STA_RWY: u8 = 4;
    pub const NAMES: [&str; 5] = ["-", "eta_mfx", "eta_rwy", "sta_sfx", "sta_rwy"];
}

/// TFMS table kinds.
pub mod tfms_kind {
    /// Estimated arrival time (`eta`, type `ESTIMATED`).
    pub const ETA: u8 = 1;
    /// Estimated departure time (`etd`, type `ESTIMATED`).
    pub const ETD: u8 = 2;
    pub const NAMES: [&str; 3] = ["-", "eta", "etd"];
}

/// SFDPS flights, one row per GUFI.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct Flights {
    pub gufi: Vec<u128>,
    /// Aircraft identification, zero padded; all zero when no message carried it.
    pub callsign: Vec<[u8; 8]>,
    /// 24-bit aircraft address, [`NONE`] when absent.
    pub addr: Vec<u32>,
    /// 24-bit address from the `ADSB_02M_*` name-value pair of track messages (a leading `-` or
    /// `+` is dropped), [`NONE`] when absent. Unconfirmed in the FAA documentation; on the
    /// 2026-11-14 sample it equals `aircraftAddress` in 112 of the 115 flights that carry both.
    pub addr_hint: Vec<u32>,
    /// `flightPlan@identifier`, zero padded.
    pub plan_id: Vec<[u8; 12]>,
    pub dep: Vec<[u8; 12]>,
    pub arr: Vec<[u8; 12]>,
}

/// ERAM track samples (`TH` messages with a position).
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct ThTable {
    pub flight: Vec<u32>,
    /// Position time, Unix ms.
    pub pos_ms: Vec<i64>,
    /// Receive time, Unix ms; [`NO_TIME`] when the recording carries none (raw `.msg` files).
    pub recv_ms: Vec<i64>,
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    /// Reported altitude, feet; [`NO_ALT`] when absent.
    pub alt_ft: Vec<i32>,
    /// Track velocity east and north, 0.1 knot; [`NO_VEL`] when absent.
    pub vx_dkt: Vec<i16>,
    pub vy_dkt: Vec<i16>,
    /// 1 when the message carried the coast indicator.
    pub coast: Vec<u8>,
}

/// Handoff events.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct HandoffTable {
    pub flight: Vec<u32>,
    /// Message timestamp, Unix ms.
    pub ts_ms: Vec<i64>,
    /// See [`hev`].
    pub event: Vec<u8>,
    /// Transferring and receiving unit, unit in bytes 0 to 3, sector in 4 to 7.
    pub from: Vec<[u8; 8]>,
    pub to: Vec<[u8; 8]>,
}

/// Expanded routes: one row per `HX` route, its points in [`RoutePoints`].
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct RouteTable {
    pub flight: Vec<u32>,
    pub ts_ms: Vec<i64>,
    /// First point in [`RoutePoints`] and the number of points.
    pub first: Vec<u32>,
    pub len: Vec<u32>,
}

/// Route points of every route, in order.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct RoutePoints {
    pub fix: Vec<[u8; 8]>,
    /// Estimated time at the point, Unix ms; [`NO_TIME`] when absent.
    pub est_ms: Vec<i64>,
    /// Coordinates when the message gives them, else [`NO_COORD`].
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
}

/// TBFM estimates and schedules, one row per time value.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct TbfmTable {
    /// The TBFM `gufi`, equal to the SFDPS flight plan identifier.
    pub plan_id: Vec<[u8; 12]>,
    pub aid: Vec<[u8; 8]>,
    /// The meter fix of the section.
    pub fix: Vec<[u8; 8]>,
    /// See [`tbfm_kind`].
    pub kind: Vec<u8>,
    pub time_ms: Vec<i64>,
    /// When TBFM published the value (`msgTime`), Unix ms.
    pub issue_ms: Vec<i64>,
}

/// TFMS arrival and departure estimates, one row per estimate.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct TfmsTable {
    /// The TFMS `gufi` (equal to the SFDPS flight plan identifier), zero padded.
    pub plan_id: Vec<[u8; 16]>,
    pub acid: Vec<[u8; 8]>,
    pub dep: Vec<[u8; 8]>,
    pub arr: Vec<[u8; 8]>,
    /// See [`tfms_kind`].
    pub kind: Vec<u8>,
    pub time_ms: Vec<i64>,
    /// `sourceTimeStamp` of the message, Unix ms.
    pub issue_ms: Vec<i64>,
}

/// What ingest saw and what it dropped.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Documents offered, per feed (SFDPS, TBFM, TFMS).
    pub docs: [u64; 3],
    /// Documents the parser or decoder refused, per feed.
    pub doc_errors: [u64; 3],
    /// SFDPS messages rejected by the decoder (no usable GUFI or timestamp).
    pub sfdps_rejected: u64,
    /// Rows of every table that did not fit, by table.
    pub overflow_flights: u64,
    pub overflow_th: u64,
    pub overflow_handoffs: u64,
    pub overflow_routes: u64,
    pub overflow_tbfm: u64,
    pub overflow_tfms: u64,
    /// `TH` samples with a coast indicator.
    pub th_coast: u64,
    /// `TH` samples without a reported altitude.
    pub th_no_alt: u64,
    /// `TH` messages carrying an `ADSB_02M_*` name-value pair (an address hint the decoder does
    /// not use; reported only).
    pub th_adsb_hint: u64,
    /// Route points that are relative to a fix (radial and distance) rather than a fix itself.
    pub route_relative_points: u64,
}

/// The legacy tables. Cold container (A7.2); the tables inside are the hot data.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct Legacy {
    pub flights: Flights,
    pub th: ThTable,
    pub handoffs: HandoffTable,
    pub routes: RouteTable,
    pub route_points: RoutePoints,
    pub tbfm: TbfmTable,
    pub tfms: TfmsTable,
    pub counts: Counts,
    pub limits: Limits,
    by_gufi: HashMap<u128, u32>,
}

/// An empty set of tables.
pub fn new_legacy() -> Legacy {
    Legacy::default()
}

/// The flight row of `gufi`, created when new; `None` (counted) when the table is full.
pub fn flight_ix(leg: &mut Legacy, gufi: u128) -> Option<u32> {
    if let Some(&i) = leg.by_gufi.get(&gufi) {
        return Some(i);
    }
    if leg.flights.gufi.len() >= leg.limits.flights {
        leg.counts.overflow_flights = leg.counts.overflow_flights.saturating_add(1);
        return None;
    }
    let i = leg.flights.gufi.len() as u32;
    leg.flights.gufi.push(gufi);
    leg.flights.callsign.push([0; 8]);
    leg.flights.addr.push(NONE);
    leg.flights.addr_hint.push(NONE);
    leg.flights.plan_id.push([0; 12]);
    leg.flights.dep.push([0; 12]);
    leg.flights.arr.push([0; 12]);
    leg.by_gufi.insert(gufi, i);
    Some(i)
}

/// Sets the aircraft address of flight `f`.
pub fn set_addr(leg: &mut Legacy, f: u32, addr: u32) {
    if let Some(a) = leg.flights.addr.get_mut(f as usize) {
        *a = addr;
    }
}

/// Sets the address hint of flight `f`.
pub fn set_addr_hint(leg: &mut Legacy, f: u32, addr: u32) {
    if let Some(a) = leg.flights.addr_hint.get_mut(f as usize) {
        *a = addr;
    }
}

/// Appends an ERAM track sample, or counts the overflow.
#[allow(clippy::too_many_arguments)]
pub fn push_th(
    leg: &mut Legacy,
    flight: u32,
    pos_ms: i64,
    recv_ms: i64,
    lat_e7: i32,
    lon_e7: i32,
    alt_ft: i32,
    vel: Option<(i16, i16)>,
    coast: bool,
) {
    if leg.th.flight.len() >= leg.limits.th {
        leg.counts.overflow_th = leg.counts.overflow_th.saturating_add(1);
        return;
    }
    let t = &mut leg.th;
    t.flight.push(flight);
    t.pos_ms.push(pos_ms);
    t.recv_ms.push(recv_ms);
    t.lat_e7.push(lat_e7);
    t.lon_e7.push(lon_e7);
    t.alt_ft.push(alt_ft);
    let (vx, vy) = vel.unwrap_or((NO_VEL, NO_VEL));
    t.vx_dkt.push(vx);
    t.vy_dkt.push(vy);
    t.coast.push(u8::from(coast));
    if coast {
        leg.counts.th_coast += 1;
    }
    if alt_ft == NO_ALT {
        leg.counts.th_no_alt += 1;
    }
}

/// Appends a handoff event, or counts the overflow.
pub fn push_handoff(
    leg: &mut Legacy,
    flight: u32,
    ts_ms: i64,
    event: u8,
    from: [u8; 8],
    to: [u8; 8],
) {
    if leg.handoffs.flight.len() >= leg.limits.handoffs {
        leg.counts.overflow_handoffs = leg.counts.overflow_handoffs.saturating_add(1);
        return;
    }
    let h = &mut leg.handoffs;
    h.flight.push(flight);
    h.ts_ms.push(ts_ms);
    h.event.push(event);
    h.from.push(from);
    h.to.push(to);
}

/// One point of a route being appended.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct RoutePointIn {
    pub fix: [u8; 8],
    pub est_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
}

/// Appends an expanded route with its points, or counts the overflow (nothing is kept then).
pub fn push_route(leg: &mut Legacy, flight: u32, ts_ms: i64, points: &[RoutePointIn]) {
    if leg.routes.flight.len() >= leg.limits.routes
        || leg.route_points.fix.len().saturating_add(points.len()) > leg.limits.route_points
    {
        leg.counts.overflow_routes = leg.counts.overflow_routes.saturating_add(1);
        return;
    }
    let first = leg.route_points.fix.len() as u32;
    for p in points {
        leg.route_points.fix.push(p.fix);
        leg.route_points.est_ms.push(p.est_ms);
        leg.route_points.lat_e7.push(p.lat_e7);
        leg.route_points.lon_e7.push(p.lon_e7);
    }
    leg.routes.flight.push(flight);
    leg.routes.ts_ms.push(ts_ms);
    leg.routes.first.push(first);
    leg.routes.len.push(points.len() as u32);
}

/// Appends a TBFM entry, or counts the overflow.
pub fn push_tbfm(
    leg: &mut Legacy,
    plan_id: [u8; 12],
    aid: [u8; 8],
    fix: [u8; 8],
    kind: u8,
    time_ms: i64,
    issue_ms: i64,
) {
    if leg.tbfm.kind.len() >= leg.limits.tbfm {
        leg.counts.overflow_tbfm = leg.counts.overflow_tbfm.saturating_add(1);
        return;
    }
    let t = &mut leg.tbfm;
    t.plan_id.push(plan_id);
    t.aid.push(aid);
    t.fix.push(fix);
    t.kind.push(kind);
    t.time_ms.push(time_ms);
    t.issue_ms.push(issue_ms);
}

/// One TFMS estimate being appended.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct TfmsIn {
    pub plan_id: [u8; 16],
    pub acid: [u8; 8],
    pub dep: [u8; 8],
    pub arr: [u8; 8],
    pub kind: u8,
    pub time_ms: i64,
    pub issue_ms: i64,
}

/// Appends a TFMS entry, or counts the overflow.
pub fn push_tfms(leg: &mut Legacy, e: &TfmsIn) {
    if leg.tfms.kind.len() >= leg.limits.tfms {
        leg.counts.overflow_tfms = leg.counts.overflow_tfms.saturating_add(1);
        return;
    }
    let t = &mut leg.tfms;
    t.plan_id.push(e.plan_id);
    t.acid.push(e.acid);
    t.dep.push(e.dep);
    t.arr.push(e.arr);
    t.kind.push(e.kind);
    t.time_ms.push(e.time_ms);
    t.issue_ms.push(e.issue_ms);
}

/// The text of a zero-padded name.
pub fn name(b: &[u8]) -> &str {
    code_str(b)
}

/// Pads `s` into a fixed zero-padded array, cutting at the width.
pub fn pad<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    for (o, b) in out.iter_mut().zip(s.iter()) {
        *o = *b;
    }
    out
}

/// A flight's plan identifier to flight index, for joining TBFM and TFMS rows. Flights with no
/// plan identifier are left out; a duplicate identifier keeps the first flight.
pub fn plan_index(leg: &Legacy) -> HashMap<[u8; 12], u32> {
    let mut m = HashMap::new();
    for (i, p) in leg.flights.plan_id.iter().enumerate() {
        if p.iter().any(|&b| b != 0) {
            m.entry(*p).or_insert(i as u32);
        }
    }
    m
}

/// A summary of which `Field`s an `Events` row has, for the SFDPS ingest.
pub(crate) fn row_has(ev: &oa_fixm::Events, row: usize, f: Field) -> bool {
    has_field(ev, row, f) && !oa_fixm::field_error(ev, row, f)
}

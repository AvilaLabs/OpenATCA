//! The legacy samples: what ERAM, TBFM and TFMS published, as struct-of-arrays tables. Shell
//! (B7: it keeps `HashMap` indexes, which B8 bans from core; lookups only, so no iteration order
//! reaches an output): the ingest functions take the bytes of one message and a receive time as input.
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
pub use oa_fixm::{
    ALT_FORM_AT_FIX, ALT_FORM_BLOCK, ALT_FORM_SIMPLE, ALT_FORM_VFR, ALT_FORM_VFR_PLUS,
};

/// Assigned-altitude field was present but the decoder could not accept its form.
pub const ALT_FORM_INVALID: u8 = 0;

/// "No index" for flights and addresses (RULESET A4).
pub const NONE: u32 = u32::MAX;
/// Absent time, Unix ms.
pub const NO_TIME: i64 = i64::MIN;
/// Absent altitude, feet.
pub const NO_ALT: i32 = i32::MIN;
/// Absent velocity component, 0.1 knot.
pub const NO_VEL: i16 = i16::MIN;
/// Absent transponder code.
pub const NO_BEACON: u16 = u16::MAX;
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
/// Most assigned-altitude updates retained for as-of SFDPS snapshots.
pub const MAX_ASSIGNED_ALTITUDE_UPDATES: usize = 1 << 22;
/// Most assigned-altitude updates per flight; excess rows are counted and not stored.
pub const MAX_ASSIGNED_ALTITUDE_UPDATES_PER_FLIGHT: u16 = 256;
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
    pub beacons: usize,
    /// Offline assignment history is collected only when the caller supplies a positive limit.
    pub assigned_altitude_updates: usize,
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
            beacons: MAX_BEACON_ROWS,
            assigned_altitude_updates: 0,
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
    /// The last transponder code a message gave (`currentBeaconCode`, four octal digits read as
    /// a decimal number, as the ADS-B squawk is), [`NO_BEACON`] when no message carried one.
    pub beacon: Vec<u16>,
    /// `flightPlan@identifier`, zero padded.
    pub plan_id: Vec<[u8; 12]>,
    pub dep: Vec<[u8; 12]>,
    pub arr: Vec<[u8; 12]>,
}

/// Typed SFDPS assigned-altitude updates with their message and receive times.
/// `form` uses oa-fixm ALT_FORM_* codes or ALT_FORM_INVALID; `value_ft` is only for SIMPLE.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct AssignedAltitudeTable {
    pub flight: Vec<u32>,
    pub ts_ms: Vec<i64>,
    pub recv_ms: Vec<i64>,
    pub form: Vec<u8>,
    pub value_ft: Vec<i32>,
}

/// Assignment events whose receive time is absent or contradicts their source time.
/// A positive `source_ms` is the earliest cutoff at which the event can affect selection;
/// zero means its timing is wholly unknown.
#[derive(Clone, Debug, Default)]
pub struct UnknownAssignedAltitudeTable {
    pub flight: Vec<u32>,
    pub source_ms: Vec<i64>,
    pub form: Vec<u8>,
    pub value_ft: Vec<i32>,
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

/// Transponder code changes of ERAM flights: one row each time a message gave a flight a code
/// other than the one it had (the first code counts), so the code in force at a time is the last
/// row of the flight at or before it. Capped at `Limits::beacons` ([`MAX_BEACON_ROWS`] by default); a row that does not fit is
/// counted, not stored.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct BeaconTable {
    pub flight: Vec<u32>,
    /// Message timestamp, Unix ms.
    pub ts_ms: Vec<i64>,
    pub code: Vec<u16>,
}

/// Most beacon change rows (a flight changes code a few times).
pub const MAX_BEACON_ROWS: usize = 1 << 22;

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
    /// When this route was available in the recording, or [`NO_TIME`].
    pub recv_ms: Vec<i64>,
    /// Flight identity known when this route was captured.
    pub addr_at_receive: Vec<u32>,
    pub plan_at_receive: Vec<[u8; 12]>,
    pub arrival_at_receive: Vec<[u8; 12]>,
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
    /// When the estimate was available in the recording, or [`NO_TIME`].
    pub recv_ms: Vec<i64>,
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
    /// SFDPS field errors in the rows the decoder kept (a field that was present but malformed or
    /// duplicated; the row stays, the field is treated as absent).
    pub sfdps_field_errors: u64,
    /// TBFM flights refused for a field, with their siblings kept.
    pub tbfm_rejected: u64,
    /// TFMS messages refused for a field, with their siblings kept.
    pub tfms_rejected: u64,
    /// The refusals above by reason and by field, keyed by a report label (see
    /// [`Counts::note`]).
    pub reasons: std::collections::BTreeMap<String, u64>,
    /// Messages or rows of accepted documents past the replay's per-document table size (SFDPS
    /// messages, TBFM rows, TFMS rows); decoded as far as they fit, the rest counted here.
    pub rows_not_decoded: u64,
    /// Rows of every table that did not fit, by table.
    pub overflow_flights: u64,
    pub overflow_th: u64,
    pub overflow_handoffs: u64,
    pub overflow_routes: u64,
    pub overflow_assigned_altitude_updates: u64,
    pub assigned_altitude_unknown_availability: u64,
    pub overflow_tbfm: u64,
    pub overflow_tfms: u64,
    /// TFMS actual-arrival rows that did not fit; not shown in the report (a diagnostic table).
    pub overflow_tfms_actual: u64,
    /// `TH` samples with a coast indicator.
    pub th_coast: u64,
    /// `TH` samples without a reported altitude.
    pub th_no_alt: u64,
    /// `TH` messages carrying an `ADSB_02M_*` name-value pair (an address hint the decoder does
    /// not use; reported only).
    pub th_adsb_hint: u64,
    /// Route points that are relative to a fix (radial and distance) rather than a fix itself.
    pub route_relative_points: u64,
    /// Records left out because their own time (the message timestamp or position time of an
    /// SFDPS row, the message time of a TBFM row or a TFMS flight) is in a sealed window
    /// (OA-SEAL-014).
    pub sealed_window_dropped: u64,
}

impl Counts {
    /// Adds `n` to the refusal count labelled `label`; a zero adds nothing.
    pub fn note(&mut self, label: String, n: u64) {
        if n > 0 {
            *self.reasons.entry(label).or_insert(0) += n;
        }
    }
}

/// The legacy tables. Cold container (A7.2); the tables inside are the hot data.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct Legacy {
    pub flights: Flights,
    pub th: ThTable,
    pub beacons: BeaconTable,
    pub handoffs: HandoffTable,
    pub routes: RouteTable,
    pub route_points: RoutePoints,
    pub assigned_altitudes: AssignedAltitudeTable,
    pub unknown_assigned_altitudes: UnknownAssignedAltitudeTable,
    pub tbfm: TbfmTable,
    pub tfms: TfmsTable,
    /// TFMS arrival times of type `ACTUAL`, one row each (kind [`tfms_kind::ETA`]). Kept apart
    /// from [`Legacy::tfms`], whose rows are all estimates, so no estimate metric reads them.
    pub tfms_actual: TfmsTable,
    pub counts: Counts,
    pub limits: Limits,
    by_gufi: HashMap<u128, u32>,
    assigned_altitudes_by_flight: HashMap<u32, u16>,
    assigned_altitude_overflow_flights: HashMap<u32, i64>,
}

/// Whether assigned-altitude history for a flight is incomplete because a storage bound was hit.
pub fn assigned_altitude_history_overflowed(leg: &Legacy, flight: u32, cutoff_ms: i64) -> bool {
    leg.assigned_altitude_overflow_flights
        .get(&flight)
        .is_some_and(|&available_ms| available_ms > 0 && available_ms <= cutoff_ms)
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
    leg.flights.beacon.push(NO_BEACON);
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

/// Appends a typed assigned-altitude event, enforcing total and per-flight budgets.
pub fn push_assigned_altitude(
    leg: &mut Legacy,
    flight: u32,
    ts_ms: i64,
    recv_ms: i64,
    form: u8,
    value_ft: i32,
) {
    let limit = leg
        .limits
        .assigned_altitude_updates
        .min(MAX_ASSIGNED_ALTITUDE_UPDATES);
    if limit == 0 {
        return;
    }
    let known_availability = ts_ms > 0 && ts_ms <= recv_ms;
    if !known_availability {
        leg.counts.assigned_altitude_unknown_availability = leg
            .counts
            .assigned_altitude_unknown_availability
            .saturating_add(1);
    }
    if leg.assigned_altitudes.flight.len() + leg.unknown_assigned_altitudes.flight.len() >= limit
        || leg
            .assigned_altitudes_by_flight
            .get(&flight)
            .copied()
            .unwrap_or(0)
            >= MAX_ASSIGNED_ALTITUDE_UPDATES_PER_FLIGHT
    {
        leg.counts.overflow_assigned_altitude_updates = leg
            .counts
            .overflow_assigned_altitude_updates
            .saturating_add(1);
        let earliest_possible = if known_availability {
            recv_ms
        } else {
            ts_ms.max(1)
        };
        let first_overflow = leg
            .assigned_altitude_overflow_flights
            .entry(flight)
            .or_insert(earliest_possible);
        *first_overflow = (*first_overflow).min(earliest_possible);
        return;
    }
    if known_availability {
        let table = &mut leg.assigned_altitudes;
        table.flight.push(flight);
        table.ts_ms.push(ts_ms);
        table.recv_ms.push(recv_ms);
        table.form.push(form);
        table.value_ft.push(value_ft);
    } else {
        let table = &mut leg.unknown_assigned_altitudes;
        table.flight.push(flight);
        table.source_ms.push(ts_ms.max(0));
        table.form.push(form);
        table.value_ft.push(value_ft);
    }
    let n = leg.assigned_altitudes_by_flight.entry(flight).or_insert(0);
    *n = (*n).saturating_add(1);
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
    push_route_received(leg, flight, ts_ms, NO_TIME, points);
}

pub(crate) fn push_route_received(
    leg: &mut Legacy,
    flight: u32,
    ts_ms: i64,
    recv_ms: i64,
    points: &[RoutePointIn],
) {
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
    leg.routes.recv_ms.push(recv_ms);
    leg.routes.addr_at_receive.push(
        leg.flights
            .addr
            .get(flight as usize)
            .copied()
            .unwrap_or(NONE),
    );
    leg.routes.plan_at_receive.push(
        leg.flights
            .plan_id
            .get(flight as usize)
            .copied()
            .unwrap_or([0; 12]),
    );
    leg.routes.arrival_at_receive.push(
        leg.flights
            .arr
            .get(flight as usize)
            .copied()
            .unwrap_or([0; 12]),
    );
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
    t.recv_ms.push(NO_TIME);
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

/// Appends a TFMS actual arrival time to [`Legacy::tfms_actual`], or counts the overflow.
pub fn push_tfms_actual(leg: &mut Legacy, e: &TfmsIn) {
    if leg.tfms_actual.kind.len() >= leg.limits.tfms {
        leg.counts.overflow_tfms_actual = leg.counts.overflow_tfms_actual.saturating_add(1);
        return;
    }
    let t = &mut leg.tfms_actual;
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

/// An identifier (callsign, aircraft identification, plan identifier) normalised for comparison:
/// leading white space skipped, cut at the first zero or white space, ASCII upper case, zero
/// padded to `N`. Every join on an identifier compares through this, so the accepted forms of one
/// identifier (any case, padded with spaces or zeros) are the same key.
pub fn norm_id<const N: usize>(c: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    let start = c.iter().take_while(|b| b.is_ascii_whitespace()).count();
    let body = c.get(start..).unwrap_or(&[]);
    for (o, &b) in out.iter_mut().zip(
        body.iter()
            .take_while(|&&b| b != 0 && !b.is_ascii_whitespace()),
    ) {
        *o = b.to_ascii_uppercase();
    }
    out
}

/// Most flights kept per plan identifier. An identifier is reused by a handful of flights at
/// most (a re-filed plan, a recycled number); a flight past this is not indexed and is counted in
/// [`PlanIndex::overflow`].
pub const MAX_FLIGHTS_PER_PLAN: usize = 8;

/// The flights that used one plan identifier, in flight order.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default)]
pub struct PlanSlot {
    pub flights: [u32; MAX_FLIGHTS_PER_PLAN],
    pub len: u8,
    /// More flights used the identifier than fit: the list is not all of them.
    pub overflowed: bool,
}

/// Plan identifier (normalised, [`norm_id`]) to the flights that used it, for joining TBFM and TFMS rows. Built once, sized
/// at init from the flight table; flights with no plan identifier are left out. An identifier
/// can name several flights (it is reused), so a join must choose among them
/// ([`plan_candidates`]) and never take the first.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct PlanIndex {
    slots: HashMap<[u8; 12], PlanSlot>,
    /// Flights past [`MAX_FLIGHTS_PER_PLAN`] for their identifier, left out of the index.
    pub overflow: u64,
}

/// Builds the plan index of the flights.
pub fn plan_index(leg: &Legacy) -> PlanIndex {
    let mut idx = PlanIndex {
        slots: HashMap::with_capacity(leg.flights.plan_id.len()),
        overflow: 0,
    };
    for (i, p) in leg.flights.plan_id.iter().enumerate() {
        if p.iter().all(|&b| b == 0) {
            continue;
        }
        let slot = idx.slots.entry(norm_id(p)).or_default();
        let n = usize::from(slot.len);
        match slot.flights.get_mut(n) {
            Some(s) => {
                *s = i as u32;
                slot.len += 1;
            }
            None => {
                slot.overflowed = true;
                idx.overflow = idx.overflow.saturating_add(1);
            }
        }
    }
    idx
}

/// The flights that used plan identifier `key`, in flight order.
pub fn plan_candidates<'a>(idx: &'a PlanIndex, key: &[u8; 12]) -> &'a [u32] {
    idx.slots
        .get(&norm_id::<12>(key))
        .and_then(|s| s.flights.get(..usize::from(s.len)))
        .unwrap_or(&[])
}

/// Whether more flights used `key` than [`plan_candidates`] lists.
pub fn plan_overflowed(idx: &PlanIndex, key: &[u8; 12]) -> bool {
    idx.slots
        .get(&norm_id::<12>(key))
        .is_some_and(|s| s.overflowed)
}

/// Whether plan identifier `key` names any flight.
pub fn plan_known(idx: &PlanIndex, key: &[u8; 12]) -> bool {
    idx.slots.contains_key(&norm_id::<12>(key))
}

/// A summary of which `Field`s an `Events` row has, for the SFDPS ingest.
pub(crate) fn row_has(ev: &oa_fixm::Events, row: usize, f: Field) -> bool {
    has_field(ev, row, f) && !oa_fixm::field_error(ev, row, f)
}

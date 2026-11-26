//! Applying decoded feed rows to the flight table. Core (pure, B1): free functions over plain
//! data, time passed in (B4), no allocation (C1).
//!
//! Rules (REQUIREMENTS OA-RPL-*):
//!
//! * **Ordering.** An SFDPS row that carries a position older than the flight's last position
//!   time does not overwrite it: the position, altitude, speeds and position time of that row
//!   are dropped and the row is counted as out of order, while its other fields (amendments,
//!   handoffs) still apply, since a plan message often repeats the last known position. (A
//!   position with no `positionTime` is ordered by the message timestamp.) A TBFM row older than
//!   the flight's last TBFM `msgTime` is refused whole and counted the same way; a refused row
//!   is not an update and does not keep the flight alive.
//! * **Position time.** The position time is written only together with a position: a row whose
//!   position is missing or malformed leaves the held position time as it was, whatever
//!   `positionTime` it carries.
//! * **Source time.** Each flight keeps the timestamp of the newest SFDPS message applied to it
//!   (`flight@timestamp`). A message older than that is a stale amendment, counted: it does not
//!   change the flight's status, controlling unit or computer id and does not claim a flight-plan
//!   identifier (identity fields that only fill an empty column still apply, as does a position
//!   by its own rule above). A claim on a flight-plan identifier another flight holds succeeds
//!   only if that flight's newest message is older than the claiming one; otherwise it is refused
//!   and counted, so a late message of a flight that lost a reused identifier cannot take it
//!   back. A message with no usable timestamp is applied as before and counted separately.
//! * **Identity.** Callsign, departure, arrival and aircraft type fill an empty column. A
//!   different non-empty value is a field conflict: counted, and the value held is kept
//!   (INTERFACES IF-R5: refuse and flag, do not quietly change identity). The computer id is not
//!   identity: each centre's host assigns its own, so SFDPS overwrites it with the latest. TBFM
//!   gives callsign, aircraft type (its `typ` without the `/` suffix) and, only into an empty
//!   column, the computer id; its airports are FAA three-letter codes (`LAX`) where SFDPS uses
//!   ICAO (`KLAX`), so they are not merged.
//! * **Linking TBFM to SFDPS.** The TBFM `gufi` attribute is a 10-character identifier that
//!   equals SFDPS `flightPlan@identifier`, not the SFDPS GUFI (a UUID); on the real feeds it
//!   linked 1,012 of the 1,522 distinct TBFM `gufi` values seen to a flight plan in the SFDPS
//!   recording (the rest are flights SFDPS had not yet published or outside its coverage). So
//!   TBFM rows are linked through the flight-plan index. A TBFM `gufi` that matches nothing
//!   makes a new flight (keyed by the identifier under a `TBFM` tag, flagged [`TBFM_ORIGIN`]);
//!   if SFDPS later reports that flight plan, the flight takes its real GUFI. When `gufi` is
//!   absent the row links through `(centre, tmaId)`, and if that matches nothing it is counted
//!   as unlinked and not applied.
//! * **TBFM `DEL`.** A `DEL` change type, on the `air` or on the `mrp` section, clears the
//!   metering columns (meter fix, STA, ETA, runway, frozen). It does not remove the flight.
//! * **Heard again.** A row marked stale by a step of the replay clock stays stale until an
//!   accepted update carries what the row shows. For SFDPS that is an update whose position was
//!   not refused as older and that carries a position or a flight status; a refused older
//!   position, or an amendment with neither, updates the other fields and the update time but does
//!   not clear the mark. For TBFM it is an update that carries metering state (a `DEL`, a meter
//!   fix, a runway, a time, or a freeze flag); a row of identity alone (callsign, type) does not.
//!   A row refused whole as older is not an update at all.
//! * **Full table.** A new flight with no free slot is refused and counted; nothing is evicted
//!   to make room.
//! * **Flight state.** An SFDPS row that carries a flight status sets the flight's state class
//!   (`flags` bits [`CLASS_MASK`]: active, proposed, finished; none until one is seen), which
//!   the sweep reads for how long the flight may stay silent ([`crate::TableConfig`]). A row
//!   without a status leaves the class as it was.
//! * **Update time.** `last_update_ns` is the largest replay time applied and `last_source` the
//!   source at that time (ties broken by the larger source), so the table state does not depend
//!   on the order in which commuting updates arrive.
//!
//! TBFM `cleared` (empty element) bits are not acted on: the survey leaves open whether an empty
//! element clears a value (docs/TBFM_SURVEY.md, open question 2).

use oa_fixm::{has_field, Events, Field};
use oa_tbfm::rows::{change, pt, sch_flag, section};
use oa_tbfm::{row_at, sch_get, Rows, NO_TIME};

use crate::core::table::{
    claim_plan, claim_tma, class_of_status, create_flight, find_gufi, find_plan, find_tma,
    rekey_flight, slot_mut, slot_ref, FlightTable, Slot, SlotMut, CLASS_MASK, FEED_SFDPS,
    FEED_TBFM, FROZEN, HAS_ALT, HAS_GS, HAS_POS, HAS_VEL, TBFM_ORIGIN,
};

/// What applying one row did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The row changed (or confirmed) the flight state.
    Applied,
    /// SFDPS only: the row's position was older than the flight's, so it was not applied, and
    /// counted out of order; the rest of the row was applied.
    AppliedStalePosition,
    /// Refused: older than what the flight already holds. Counted.
    OutOfOrder,
    /// Refused: the table is full and the flight is new. Counted.
    TableFull,
    /// TBFM row with no usable key: not applied. Counted.
    Unlinked,
    /// The row index is out of range, or the table failed an internal check. Not applied; the
    /// caller counts it as an error.
    BadRow,
}

/// The tag that marks a TBFM-first flight's key: ASCII `TBFM` in the top 32 bits, the 12-byte
/// identifier below. A real GUFI is a random UUID, so it carries this tag with probability 2^-32;
/// such a collision would merely make the two flights one, which the plan index would anyway.
const TBFM_TAG: u128 = 0x5442_464d << 96;

fn tbfm_key(id: &[u8; 12]) -> u128 {
    let mut v = 0u128;
    for &b in id {
        v = (v << 8) | u128::from(b);
    }
    TBFM_TAG | v
}

/// `src` copied into an `N`-byte zero-padded array, cut if longer.
fn fit<const N: usize>(src: &[u8]) -> [u8; N] {
    let mut o = [0u8; N];
    let n = src.len().min(N);
    if let (Some(d), Some(s)) = (o.get_mut(..n), src.get(..n)) {
        d.copy_from_slice(s);
    }
    o
}

/// Identity merge: empty fills, equal confirms, different is a conflict (the held value stays).
/// Returns 1 for a conflict, else 0.
fn ident<const N: usize>(held: &mut [u8; N], new: &[u8; N], conflicts: &mut u32) -> u64 {
    if new.iter().all(|&b| b == 0) {
        return 0;
    }
    if held.iter().all(|&b| b == 0) {
        *held = *new;
        return 0;
    }
    if held != new {
        *conflicts = conflicts.saturating_add(1);
        return 1;
    }
    0
}

/// Records an applied update: count, last update time and source (largest time wins). The stale
/// mark is cleared only when the update `heard` the row, that is, carried what the row shows.
fn touch(c: &mut SlotMut<'_>, now_ns: i64, feed: u8, source: [u8; 2], heard: bool) {
    let newer = *c.last_feed == 0
        || now_ns > *c.last_update_ns
        || (now_ns == *c.last_update_ns && (feed, source) > (*c.last_feed, *c.last_source));
    if newer {
        *c.last_update_ns = now_ns;
        *c.last_feed = feed;
        *c.last_source = source;
    }
    *c.updates = c.updates.saturating_add(1);
    if heard {
        *c.stale = false;
    }
}

fn refuse_out_of_order(t: &mut FlightTable, s: Slot) -> Applied {
    if let Some(c) = slot_mut(t, s) {
        *c.out_of_order = c.out_of_order.saturating_add(1);
    }
    t.stats.out_of_order += 1;
    Applied::OutOfOrder
}

/// Applies row `i` of `ev` (an SFDPS flight event) at replay time `now_ns`.
pub fn apply_sfdps(t: &mut FlightTable, ev: &Events, i: usize, now_ns: i64) -> Applied {
    if i >= ev.len {
        return Applied::BadRow;
    }
    let (Some(&gufi), Some(&ts_ms)) = (ev.gufi.get(i), ev.ts_ms.get(i)) else {
        return Applied::BadRow;
    };
    let plan = if has_field(ev, i, Field::FlightPlanId) {
        ev.plan_id.get(i).copied()
    } else {
        None
    };
    let slot = match find_gufi(t, gufi) {
        Some(s) => s,
        None => {
            let first_seen_by_tbfm = plan
                .and_then(|p| find_plan(t, &p))
                .filter(|&s| slot_ref(t, s).is_some_and(|c| c.flags & TBFM_ORIGIN != 0));
            match first_seen_by_tbfm {
                Some(s) => {
                    if !rekey_flight(t, s, gufi) {
                        return Applied::BadRow;
                    }
                    if let Some(c) = slot_mut(t, s) {
                        *c.flags &= !TBFM_ORIGIN;
                    }
                    t.stats.promoted += 1;
                    s
                }
                None => match create_flight(t, gufi) {
                    Some(s) => s,
                    None => {
                        t.stats.refused_full += 1;
                        return Applied::TableFull;
                    }
                },
            }
        }
    };

    let has_pos = has_field(ev, i, Field::Position);
    // The position time travels with a position: a row whose position is missing or malformed
    // has no position time either, so it cannot make a later, older but valid position stale.
    let ptime = if !has_pos {
        None
    } else if has_field(ev, i, Field::PositionTime) {
        ev.pos_time_ms.get(i).copied()
    } else {
        Some(ts_ms)
    };
    // A position older than the one held is not applied; the rest of the row still is.
    let stale = match (ptime, slot_ref(t, slot)) {
        (Some(pt), Some(c)) => *c.pos_time_ms != NO_TIME && pt < *c.pos_time_ms,
        _ => false,
    };
    // The message's own timestamp orders SFDPS messages for one flight. A message older than the
    // newest one applied to the flight must not roll back its status, controlling unit, computer
    // id or flight-plan ownership.
    let src_ms = if ts_ms > 0 { ts_ms } else { NO_TIME };
    let stale_amend = src_ms != NO_TIME
        && slot_ref(t, slot).is_some_and(|c| *c.src_ms != NO_TIME && src_ms < *c.src_ms);
    if src_ms == NO_TIME {
        t.stats.no_source_time += 1;
    }
    if stale_amend {
        t.stats.stale_amendments += 1;
    }
    if let (Some(p), false) = (plan, stale_amend) {
        claim_plan(t, slot, &p, src_ms);
    }

    let mut conflicts = 0u64;
    {
        let Some(mut c) = slot_mut(t, slot) else {
            return Applied::BadRow;
        };
        let mut conf = *c.conflicts;
        if let (true, Some(v)) = (has_field(ev, i, Field::Callsign), ev.callsign.get(i)) {
            conflicts += ident(c.callsign, v, &mut conf);
        }
        if let (true, false, Some(v)) = (
            has_field(ev, i, Field::ComputerId),
            stale_amend,
            ev.computer_id.get(i),
        ) {
            // Each centre's host assigns its own id, so a flight's id changes as it crosses
            // centres: the latest is kept, and a change is not a conflict.
            *c.computer_id = *v;
        }
        if let (true, Some(v)) = (has_field(ev, i, Field::Departure), ev.departure.get(i)) {
            conflicts += ident(c.departure, v, &mut conf);
        }
        if let (true, Some(v)) = (has_field(ev, i, Field::Arrival), ev.arrival.get(i)) {
            conflicts += ident(c.arrival, v, &mut conf);
        }
        if let (true, Some(v)) = (
            has_field(ev, i, Field::AircraftType),
            ev.aircraft_type.get(i),
        ) {
            conflicts += ident(c.aircraft_type, v, &mut conf);
        }
        *c.conflicts = conf;
        if has_pos && !stale {
            if let (Some(&la), Some(&lo)) = (ev.lat_e7.get(i), ev.lon_e7.get(i)) {
                *c.lat_e7 = la;
                *c.lon_e7 = lo;
                *c.flags |= HAS_POS;
                if let Some(pt) = ptime {
                    *c.pos_time_ms = pt;
                }
            }
        }
        if let (true, false, Some(&v)) = (
            has_field(ev, i, Field::ReportedAlt),
            stale,
            ev.reported_ft.get(i),
        ) {
            *c.alt_ft = v;
            *c.flags |= HAS_ALT;
        }
        if let (true, false, Some(&v)) = (
            has_field(ev, i, Field::GroundSpeed),
            stale,
            ev.gs_dkt.get(i),
        ) {
            *c.gs_dkt = v;
            *c.flags |= HAS_GS;
        }
        if let (true, false, Some(&x), Some(&y)) = (
            has_field(ev, i, Field::Velocity),
            stale,
            ev.vx_dkt.get(i),
            ev.vy_dkt.get(i),
        ) {
            *c.vx_dkt = x;
            *c.vy_dkt = y;
            *c.flags |= HAS_VEL;
        }
        if stale {
            *c.out_of_order = c.out_of_order.saturating_add(1);
        }
        if let (true, false, Some(v)) = (
            has_field(ev, i, Field::ControllingUnit),
            stale_amend,
            ev.ctrl.get(i),
        ) {
            *c.ctrl = *v;
        }
        let status = has_field(ev, i, Field::Status) && !stale_amend;
        if let (true, Some(&st)) = (status, ev.status.get(i)) {
            *c.flags = (*c.flags & !CLASS_MASK) | class_of_status(st);
        }
        if src_ms != NO_TIME && src_ms > *c.src_ms {
            *c.src_ms = src_ms;
        }
        let source = if has_field(ev, i, Field::Source) {
            ev.source.get(i).copied().unwrap_or([0; 2])
        } else {
            [0; 2]
        };
        let heard = !stale && (has_pos || status);
        touch(&mut c, now_ns, FEED_SFDPS, source, heard);
    }
    t.stats.conflicts += conflicts;
    t.stats.sfdps_applied += 1;
    if stale {
        t.stats.out_of_order += 1;
        return Applied::AppliedStalePosition;
    }
    Applied::Applied
}

/// Applies row `i` of `rows` (a TBFM metering row) at replay time `now_ns`.
pub fn apply_tbfm(t: &mut FlightTable, rows: &Rows, i: usize, now_ns: i64) -> Applied {
    let Some(row) = row_at(rows, i) else {
        return Applied::BadRow;
    };
    let has_tma = row.tma_id != [0; 8] && row.center != [0; 4];
    let slot = if row.gufi != [0; 12] {
        let key = tbfm_key(&row.gufi);
        let found = find_plan(t, &row.gufi).or_else(|| find_gufi(t, key));
        match found {
            Some(s) => {
                t.stats.tbfm_linked_gufi += 1;
                s
            }
            None => match create_flight(t, key) {
                Some(s) => {
                    if let Some(c) = slot_mut(t, s) {
                        *c.flags |= TBFM_ORIGIN;
                    }
                    claim_plan(t, s, &row.gufi, NO_TIME);
                    t.stats.tbfm_created += 1;
                    s
                }
                None => {
                    t.stats.refused_full += 1;
                    return Applied::TableFull;
                }
            },
        }
    } else if has_tma {
        match find_tma(t, &row.center, &row.tma_id) {
            Some(s) => {
                t.stats.tbfm_linked_tma += 1;
                s
            }
            None => {
                t.stats.tbfm_unlinked += 1;
                return Applied::Unlinked;
            }
        }
    } else {
        t.stats.tbfm_unlinked += 1;
        return Applied::Unlinked;
    };

    if let Some(c) = slot_ref(t, slot) {
        if row.msg_ms != NO_TIME && *c.tbfm_msg_ms != NO_TIME && row.msg_ms < *c.tbfm_msg_ms {
            return refuse_out_of_order(t, slot);
        }
    }
    if has_tma {
        claim_tma(t, slot, &row.center, &row.tma_id);
    }

    let mut conflicts = 0u64;
    {
        let Some(mut c) = slot_mut(t, slot) else {
            return Applied::BadRow;
        };
        let mut conf = *c.conflicts;
        conflicts += ident(c.callsign, &fit::<8>(&row.aid), &mut conf);
        // `cid` is the TBFM centre's host id: it only fills an empty column (SFDPS owns it).
        // `apt` and `dap` are FAA three-letter identifiers (LAX) where SFDPS has ICAO (KLAX), so
        // they are not merged into the airport columns.
        if c.computer_id.iter().all(|&b| b == 0) {
            *c.computer_id = fit::<4>(&row.cid);
        }
        if row.section == section::FLT {
            // `typ` is the type with an equipment suffix (`A320/L`); keep the designator.
            let end = row
                .typ
                .iter()
                .position(|&b| b == b'/')
                .unwrap_or(row.typ.len());
            let ty = row.typ.get(..end).unwrap_or(&[]);
            conflicts += ident(c.aircraft_type, &fit::<4>(ty), &mut conf);
        }
        *c.conflicts = conf;

        let deleted = row.air_type == change::DEL
            || (row.section == section::MRP && row.sect_type == change::DEL);
        // Whether the row carried metering state (a delete, a meter fix, a runway, a time or a
        // freeze flag) and so says the row is current, as opposed to identity alone.
        let mut metering = deleted || row.mfx != [0; 8];
        if deleted {
            *c.meter_fix = [0; 8];
            *c.sta_ms = NO_TIME;
            *c.eta_ms = NO_TIME;
            *c.runway = [0; 4];
            *c.flags &= !FROZEN;
        } else {
            if row.mfx != [0; 8] {
                *c.meter_fix = row.mfx;
            }
            match row.section {
                section::MRP => {
                    if row.rwy != [0; 4] {
                        *c.runway = row.rwy;
                        metering = true;
                    }
                }
                section::ETA => {
                    if let Some(&v) = row.eta_ms.get(pt::MFX).filter(|&&v| v != NO_TIME) {
                        *c.eta_ms = v;
                        metering = true;
                    }
                }
                section::STA => {
                    // Preserve the source point: an STA for SFX is not an MFX STA. The public
                    // FAA example documents this distinction, and no substitution is made when
                    // the MFX value is absent.
                    if let Some(&v) = row.sta_ms.get(pt::MFX).filter(|&&v| v != NO_TIME) {
                        *c.sta_ms = v;
                        metering = true;
                    }
                }
                section::SCH => match sch_get(row.sch, sch_flag::SFZ) {
                    sch_flag::YES => {
                        *c.flags |= FROZEN;
                        metering = true;
                    }
                    sch_flag::NO => {
                        *c.flags &= !FROZEN;
                        metering = true;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if row.msg_ms != NO_TIME && row.msg_ms > *c.tbfm_msg_ms {
            *c.tbfm_msg_ms = row.msg_ms;
        }
        touch(&mut c, now_ns, FEED_TBFM, [0; 2], metering);
    }
    t.stats.conflicts += conflicts;
    t.stats.tbfm_applied += 1;
    Applied::Applied
}

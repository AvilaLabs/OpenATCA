//! The state hash: SHA-256 over a canonical serialisation of the flight table. Core (pure, B1).
//!
//! Flights are visited in GUFI order, not slot order, so the hash does not depend on the order
//! flights were inserted, on which slots they landed in, or on slots recycled by eviction. Every
//! integer is written little-endian at a fixed width, every name column as its fixed bytes, and
//! the layout starts with a version tag, so the hash is the same on every machine and changes
//! only when the serialisation or the state does. What is hashed:
//!
//! 1. the tag `OATS1`, the live count, `next_sweep_ns` and every [`TableStats`] counter;
//! 2. for each live flight in ascending GUFI order, every column (see [`serialise_flight`]).
//!
//! The newest SFDPS source time of each flight and the three stale-amendment counters are written
//! after the flights (and before the stale marks), as the tag `OASR1`, and only when some flight
//! has a source time or a counter is not zero, so a table fed by TBFM alone hashes as before.
//!
//! A flight marked stale by a step of the clock (see [`crate::FlightTable::stale`]) is written
//! after the flights, as the tag `OAST1` and one byte per flight in the same order, and only when
//! some flight is stale, so a table with none hashes as before.
//!
//! `capacity`, `max_age_ns` and `sweep_ns` are settings, not state, and are not hashed (two runs
//! with the same input and different settings differ through the state itself).
//!
//! The sort allocates a slot list: the hash is taken at checkpoints, not per update, so this is
//! a stated exception to C1 (offline harness, C6).

use oa_sha256::core::sha256::{Digest, Sha256};

use crate::core::table::{is_occupied, slot_ref, FlightTable, Slot, SlotRef, TableStats, NO_TIME};

/// Tag and version of the canonical serialisation.
pub const HASH_TAG: &[u8; 5] = b"OATS1";

fn put(h: &mut Sha256, b: &[u8]) {
    // The only failure is a message over 2^61 bytes; a table is a few megabytes.
    let _ = h.update(b);
}

/// Tag of the stale marks, written after a table's rows when any row is stale.
pub const STALE_TAG: &[u8; 5] = b"OAST1";

/// Writes the stale mark of each of `slots`, in the order given (a table's rows in key order),
/// after the tag [`STALE_TAG`]. Callers write it only when some row is stale, so a replay with no
/// stale row hashes as it did before the marks were hashed.
pub(crate) fn stale_hash_into(h: &mut Sha256, stale: &[bool], slots: impl Iterator<Item = usize>) {
    put(h, STALE_TAG);
    for s in slots {
        put(h, &[u8::from(stale.get(s).copied().unwrap_or(false))]);
    }
}

/// Tag of the SFDPS source times and stale-amendment counters.
pub const SRC_TAG: &[u8; 5] = b"OASR1";

fn put_stats(h: &mut Sha256, s: &TableStats) {
    for v in [
        s.sfdps_applied,
        s.tbfm_applied,
        s.out_of_order,
        s.conflicts,
        s.refused_full,
        s.evicted,
        s.sweeps,
        s.tbfm_linked_gufi,
        s.tbfm_linked_tma,
        s.tbfm_created,
        s.tbfm_unlinked,
        s.promoted,
        s.key_reassigned,
    ] {
        put(h, &v.to_le_bytes());
    }
}

/// Writes one flight's columns, in a fixed order, to the hasher.
fn serialise_flight(h: &mut Sha256, c: &SlotRef<'_>) {
    put(h, &c.gufi.to_le_bytes());
    put(h, c.callsign);
    put(h, c.computer_id);
    put(h, c.departure);
    put(h, c.arrival);
    put(h, c.aircraft_type);
    put(h, c.plan_id);
    put(h, &c.lat_e7.to_le_bytes());
    put(h, &c.lon_e7.to_le_bytes());
    put(h, &c.alt_ft.to_le_bytes());
    put(h, &c.gs_dkt.to_le_bytes());
    put(h, &c.vx_dkt.to_le_bytes());
    put(h, &c.vy_dkt.to_le_bytes());
    put(h, &c.pos_time_ms.to_le_bytes());
    put(h, &c.last_update_ns.to_le_bytes());
    put(h, c.last_source);
    put(h, &[*c.last_feed]);
    put(h, c.ctrl);
    put(h, c.tma_center);
    put(h, c.tma_id);
    put(h, c.meter_fix);
    put(h, &c.sta_ms.to_le_bytes());
    put(h, &c.eta_ms.to_le_bytes());
    put(h, c.runway);
    put(h, &c.tbfm_msg_ms.to_le_bytes());
    put(h, &[*c.flags]);
    put(h, &c.updates.to_le_bytes());
    put(h, &c.out_of_order.to_le_bytes());
    put(h, &c.conflicts.to_le_bytes());
}

/// SHA-256 of the table's canonical serialisation. Independent of insertion history.
pub fn state_hash(t: &FlightTable) -> Digest {
    let mut order: Vec<(u128, u32)> = Vec::with_capacity(t.live);
    for i in 0..t.capacity as u32 {
        if is_occupied(t, Slot(i)) {
            if let Some(&g) = t.gufi.get(i as usize) {
                order.push((g, i));
            }
        }
    }
    order.sort_unstable();
    let mut h = Sha256::new();
    put(&mut h, HASH_TAG);
    put(&mut h, &(order.len() as u64).to_le_bytes());
    put(&mut h, &t.next_sweep_ns.to_le_bytes());
    put_stats(&mut h, &t.stats);
    for &(_, i) in &order {
        if let Some(c) = slot_ref(t, Slot(i)) {
            serialise_flight(&mut h, &c);
        }
    }
    let st = &t.stats;
    let any_src = order
        .iter()
        .any(|&(_, i)| t.src_ms.get(i as usize).is_some_and(|&v| v != NO_TIME));
    if any_src || st.stale_amendments + st.plan_refused_old + st.no_source_time > 0 {
        put(&mut h, SRC_TAG);
        for v in [st.stale_amendments, st.plan_refused_old, st.no_source_time] {
            put(&mut h, &v.to_le_bytes());
        }
        for &(_, i) in &order {
            put(
                &mut h,
                &t.src_ms
                    .get(i as usize)
                    .copied()
                    .unwrap_or(NO_TIME)
                    .to_le_bytes(),
            );
        }
    }
    if crate::core::table::stale_count(t) > 0 {
        stale_hash_into(&mut h, &t.stale, order.iter().map(|&(_, i)| i as usize));
    }
    h.finalize()
}

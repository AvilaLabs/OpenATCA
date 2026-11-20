//! The table: construction, lookup, removal, the audit and the digest. Core (pure).

use alloc::vec;
use alloc::vec::Vec;

use crate::index::{
    hash_addr, hash_key, hash_tfms, index_find, index_insert, index_remove, index_replace,
    new_index,
};
use crate::types::{
    Flight, FlightConfig, FlightError, FlightKey, FlightStats, FlightTable, Removal, View,
    ALT_AT_FIX, ALT_BLOCK, ALT_NIL, ALT_NONE, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS, C_COUNT,
    FLIGHT_FREE, F_COUNT, HO_ACCEPTED, HO_NONE, HO_OFFERED, KEY_FREE, KEY_GUFI, KEY_TFMS,
    MAX_VIEWS, OP_COUNT, REM_RETIRED, REM_SILENT, R_COUNT, ST_ACTIVE, ST_CANCELLED, ST_COMPLETED,
    ST_COUNT, ST_DEPARTED, ST_DROPPED, ST_NONE, ST_PROPOSED,
};

/// Flights a table may be asked to hold.
const MAX_CAPACITY: usize = 1 << 22;
/// Entries a log may be asked to hold.
const MAX_LOG: usize = 1 << 24;

/// A table with room for `cfg.capacity` flights, the three indexes and the three logs, all
/// allocated here and never again. Refuses a configuration that is out of range.
pub fn flights_new(cfg: &FlightConfig) -> Result<FlightTable, FlightError> {
    if cfg.capacity == 0 || cfg.capacity > MAX_CAPACITY {
        return Err(FlightError::Config("capacity"));
    }
    if !(0..=3_600_000).contains(&cfg.lag_floor_ms) {
        return Err(FlightError::Config("lag_floor_ms"));
    }
    if !(0..=3_600_000).contains(&cfg.max_future_ms) {
        return Err(FlightError::Config("max_future_ms"));
    }
    for (n, name) in [
        (cfg.refusal_log, "refusal_log"),
        (cfg.conflict_log, "conflict_log"),
        (cfg.removal_log, "removal_log"),
    ] {
        if n == 0 || n > MAX_LOG {
            return Err(FlightError::Config(name));
        }
    }
    let n = u32::try_from(cfg.capacity).map_err(|_| FlightError::Config("capacity"))?;
    // The free stack holds the lowest slot on top, so slots fill in order.
    let free: Vec<u32> = (0..n).rev().collect();
    Ok(FlightTable {
        cfg: *cfg,
        flights: vec![FLIGHT_FREE; cfg.capacity],
        free,
        by_key: new_index(cfg.capacity),
        by_addr: new_index(cfg.capacity),
        by_tfms: new_index(cfg.capacity),
        clock_ms: i64::MIN,
        stats: FlightStats {
            ops: 0,
            applied: 0,
            unchanged: 0,
            refused: [0; OP_COUNT * R_COUNT],
            lag_raised: 0,
            clock_held: 0,
            created: 0,
            removed: 0,
            removals_blocked: 0,
            views_added: 0,
            reinstated: 0,
            handoffs_completed: 0,
            handoffs_implicit: 0,
            handoffs_retracted: 0,
            handoffs_failed: 0,
            handoff_clears_skipped: 0,
            conflicts: [0; C_COUNT],
            refusals_unlisted: 0,
            conflicts_unlisted: 0,
        },
        refusals: Vec::with_capacity(cfg.refusal_log),
        conflicts: Vec::with_capacity(cfg.conflict_log),
        removals: Vec::with_capacity(cfg.removal_log),
    })
}

/// Flights in the table.
pub fn flights_len(t: &FlightTable) -> usize {
    t.cfg.capacity.saturating_sub(t.free.len())
}

/// Empties the three logs after the caller has read them; counts are unchanged.
pub fn flights_log_clear(t: &mut FlightTable) {
    t.refusals.clear();
    t.conflicts.clear();
    t.removals.clear();
}

pub(crate) fn slot_of_key(t: &FlightTable, k: &FlightKey) -> Option<u32> {
    let flights = &t.flights;
    index_find(&t.by_key, hash_key(k), |s| {
        flights.get(s as usize).is_some_and(|f| f.key == *k)
    })
}

/// A copy of the flight with this key.
pub fn flight_get(t: &FlightTable, k: &FlightKey) -> Option<Flight> {
    slot_of_key(t, k).and_then(|s| t.flights.get(s as usize).copied())
}

/// The flight that most recently claimed this aircraft address (a later claim by another flight
/// replaces the earlier one in the index; both are kept in the table and the claim is listed as a
/// conflict when the earlier one was still live).
pub fn flight_by_address(t: &FlightTable, address: u32) -> Option<Flight> {
    if address == 0 {
        return None;
    }
    let flights = &t.flights;
    let s = index_find(&t.by_addr, hash_addr(address), |s| {
        flights
            .get(s as usize)
            .is_some_and(|f| f.address == address)
    })?;
    t.flights.get(s as usize).copied()
}

/// The view of a flight held by `centre`.
pub fn view_of(f: &Flight, centre: &[u8; 4]) -> Option<View> {
    f.views
        .iter()
        .take(usize::from(f.n_views))
        .find(|v| v.centre == *centre)
        .copied()
}

fn is_terminal(s: u8) -> bool {
    s == ST_COMPLETED || s == ST_CANCELLED
}

/// Whether any plan view of the flight is proposed, active, departed or dropped (not every plan
/// has ended).
pub fn flight_is_live(f: &Flight) -> bool {
    f.views
        .iter()
        .take(usize::from(f.n_views))
        .any(|v| v.state != ST_NONE && !is_terminal(v.state))
}

/// Whether any plan view of the flight is proposed, active or departed: not ended and not dropped.
pub fn flight_is_flying(f: &Flight) -> bool {
    f.views
        .iter()
        .take(usize::from(f.n_views))
        .any(|v| v.state == ST_PROPOSED || v.state == ST_ACTIVE || v.state == ST_DEPARTED)
}

/// The flights that are flying ([`flight_is_flying`]) and state this callsign, counted, and the
/// first of them in slot order. One pass over the table, at most its length.
pub fn flights_flying_by_callsign(t: &FlightTable, callsign: &[u8; 8]) -> (usize, Option<Flight>) {
    let mut n = 0usize;
    let mut first = None;
    if *callsign == [0; 8] {
        return (0, None);
    }
    for f in &t.flights {
        if f.key.kind != KEY_FREE && f.callsign == *callsign && flight_is_flying(f) {
            n += 1;
            if first.is_none() {
                first = Some(*f);
            }
        }
    }
    (n, first)
}

/// The most advanced state among the views: departed, active, proposed, dropped, completed,
/// cancelled, in that order; [`ST_NONE`] for a flight with no view.
pub(crate) fn best_state(f: &Flight) -> u8 {
    let mut best = ST_NONE;
    let mut best_rank = 0u8;
    for v in f.views.iter().take(usize::from(f.n_views)) {
        let rank = match v.state {
            ST_DEPARTED => 6,
            ST_ACTIVE => 5,
            ST_PROPOSED => 4,
            ST_DROPPED => 3,
            ST_COMPLETED => 2,
            ST_CANCELLED => 1,
            _ => 0,
        };
        if rank > best_rank {
            best_rank = rank;
            best = v.state;
        }
    }
    best
}

/// Adds a new flight under `key`: a free slot, the key index. `None` if the table is full.
pub(crate) fn flight_add(t: &mut FlightTable, key: &FlightKey) -> Option<u32> {
    let slot = t.free.pop()?;
    if !index_insert(&mut t.by_key, hash_key(key), slot) {
        t.free.push(slot);
        return None;
    }
    if let Some(f) = t.flights.get_mut(slot as usize) {
        *f = FLIGHT_FREE;
        f.key = *key;
    }
    t.stats.created += 1;
    Some(slot)
}

/// Whether a flight of the TFMS key is complete enough to be indexed by it (all three parts).
pub(crate) fn tfms_complete(f: &Flight) -> bool {
    f.callsign != [0; 8] && f.dep != [0; 12] && f.arr != [0; 12]
}

/// Takes a flight out of the chain of the claimants of its address, if it is in one. Call before
/// changing the address. The chain is walked at most as far as the table is long.
pub(crate) fn unindex_address(t: &mut FlightTable, slot: u32) {
    let Some(f) = t.flights.get(slot as usize).copied() else {
        return;
    };
    if f.address == 0 {
        return;
    }
    let address = f.address;
    let h = hash_addr(address);
    let flights = &t.flights;
    let Some(head) = index_find(&t.by_addr, h, |s| {
        flights
            .get(s as usize)
            .is_some_and(|g| g.address == address)
    }) else {
        return;
    };
    if head == slot {
        if f.addr_next == 0 {
            index_remove(&mut t.by_addr, h, slot, |s| {
                flights.get(s as usize).map_or(0, |g| hash_addr(g.address))
            });
        } else {
            index_replace(&mut t.by_addr, h, slot, f.addr_next - 1);
        }
    } else {
        let mut cur = head;
        for _ in 0..t.flights.len() {
            let Some(next) = t.flights.get(cur as usize).map(|g| g.addr_next) else {
                break;
            };
            if next == 0 {
                break;
            }
            if next - 1 == slot {
                if let Some(g) = t.flights.get_mut(cur as usize) {
                    g.addr_next = f.addr_next;
                }
                break;
            }
            cur = next - 1;
        }
    }
    if let Some(g) = t.flights.get_mut(slot as usize) {
        g.addr_next = 0;
    }
}

/// Takes a flight out of the chain of the flights with its callsign, departure and arrival.
/// Call before changing them.
pub(crate) fn unindex_tfms(t: &mut FlightTable, slot: u32) {
    let Some(f) = t.flights.get(slot as usize).copied() else {
        return;
    };
    if !tfms_complete(&f) {
        return;
    }
    let h = hash_tfms(&f.callsign, &f.dep, &f.arr);
    let flights = &t.flights;
    let same = |s: u32| {
        flights
            .get(s as usize)
            .is_some_and(|g| g.callsign == f.callsign && g.dep == f.dep && g.arr == f.arr)
    };
    let Some(head) = index_find(&t.by_tfms, h, same) else {
        return;
    };
    if head == slot {
        if f.tfms_next == 0 {
            index_remove(&mut t.by_tfms, h, slot, |s| {
                flights
                    .get(s as usize)
                    .map_or(0, |g| hash_tfms(&g.callsign, &g.dep, &g.arr))
            });
        } else {
            index_replace(&mut t.by_tfms, h, slot, f.tfms_next - 1);
        }
    } else {
        let mut cur = head;
        for _ in 0..t.flights.len() {
            let Some(next) = t.flights.get(cur as usize).map(|g| g.tfms_next) else {
                break;
            };
            if next == 0 {
                break;
            }
            if next - 1 == slot {
                if let Some(g) = t.flights.get_mut(cur as usize) {
                    g.tfms_next = f.tfms_next;
                }
                break;
            }
            cur = next - 1;
        }
    }
    if let Some(g) = t.flights.get_mut(slot as usize) {
        g.tfms_next = 0;
    }
}

/// Files the flight as the newest claimant of its address, in front of the flight that held it,
/// and returns that flight's slot.
pub(crate) fn index_address(t: &mut FlightTable, slot: u32) -> Option<u32> {
    let address = t.flights.get(slot as usize).map(|f| f.address)?;
    if address == 0 {
        return None;
    }
    let h = hash_addr(address);
    let flights = &t.flights;
    let prev = index_find(&t.by_addr, h, |s| {
        flights
            .get(s as usize)
            .is_some_and(|g| g.address == address)
    });
    match prev {
        Some(old) if old != slot => {
            if let Some(g) = t.flights.get_mut(slot as usize) {
                g.addr_next = old + 1;
            }
            index_replace(&mut t.by_addr, h, old, slot);
        }
        Some(_) => {}
        None => {
            index_insert(&mut t.by_addr, h, slot);
        }
    }
    prev.filter(|&p| p != slot)
}

/// Files the flight as the newest of its callsign, departure and arrival, in front of the flight
/// that held it, and returns that flight's slot.
pub(crate) fn index_tfms(t: &mut FlightTable, slot: u32) -> Option<u32> {
    let f = t.flights.get(slot as usize).copied()?;
    if !tfms_complete(&f) {
        return None;
    }
    let h = hash_tfms(&f.callsign, &f.dep, &f.arr);
    let flights = &t.flights;
    let same = |s: u32| {
        flights
            .get(s as usize)
            .is_some_and(|g| g.callsign == f.callsign && g.dep == f.dep && g.arr == f.arr)
    };
    let prev = index_find(&t.by_tfms, h, same);
    match prev {
        Some(old) if old != slot => {
            if let Some(g) = t.flights.get_mut(slot as usize) {
                g.tfms_next = old + 1;
            }
            index_replace(&mut t.by_tfms, h, old, slot);
        }
        Some(_) => {}
        None => {
            index_insert(&mut t.by_tfms, h, slot);
        }
    }
    prev.filter(|&p| p != slot)
}

/// Removes the flight with this key from the table, writing the removal to the log first.
/// `false` if there is no such flight or the removal log is full (nothing is removed then, and
/// the count `removals_blocked` rises): a flight never leaves unlogged.
pub fn flights_remove(t: &mut FlightTable, key: &FlightKey, at_ms: i64, reason: u8) -> bool {
    match slot_of_key(t, key) {
        Some(s) => remove_slot(t, s, at_ms, reason),
        None => false,
    }
}

fn remove_slot(t: &mut FlightTable, slot: u32, at_ms: i64, reason: u8) -> bool {
    let Some(f) = t.flights.get(slot as usize).copied() else {
        return false;
    };
    if f.key.kind == KEY_FREE {
        return false;
    }
    if t.removals.len() >= t.cfg.removal_log {
        t.stats.removals_blocked += 1;
        return false;
    }
    t.removals.push(Removal {
        key: f.key,
        at_ms,
        last_avail_ms: f.last_avail_ms,
        reason,
        state: best_state(&f),
    });
    unindex_address(t, slot);
    unindex_tfms(t, slot);
    let flights = &t.flights;
    index_remove(&mut t.by_key, hash_key(&f.key), slot, |s| {
        flights.get(s as usize).map_or(0, |g| hash_key(&g.key))
    });
    if let Some(g) = t.flights.get_mut(slot as usize) {
        *g = FLIGHT_FREE;
    }
    t.free.push(slot);
    t.stats.removed += 1;
    true
}

/// Removes, with a log entry each, every flight whose plans have all ended for longer than
/// `retain_ms`, and every live flight with no operation for longer than `silence_ms`, judged
/// at `now_ms`. One pass over the table (its capacity is the bound). A removal the log has no
/// room for is not made. Returns the flights removed.
pub fn flights_sweep(t: &mut FlightTable, now_ms: i64, retain_ms: i64, silence_ms: i64) -> u32 {
    let mut removed = 0u32;
    let n = t.flights.len();
    for slot in 0..n {
        let Some(f) = t.flights.get(slot).copied() else {
            break;
        };
        if f.key.kind == KEY_FREE {
            continue;
        }
        let idle = now_ms.saturating_sub(f.last_avail_ms);
        let reason = if !flight_is_live(&f) && idle > retain_ms {
            REM_RETIRED
        } else if flight_is_live(&f) && idle > silence_ms {
            REM_SILENT
        } else {
            continue;
        };
        if let Ok(s) = u32::try_from(slot) {
            if remove_slot(t, s, now_ms, reason) {
                removed += 1;
            }
        }
    }
    removed
}

/// A 64-bit digest of the whole table (flights in slot order, counts and logs), so two runs can
/// be compared without holding both. FNV-1a over the fields; not a cryptographic hash.
pub fn flights_digest(t: &FlightTable) -> u64 {
    let mut h = Digest(0xcbf2_9ce4_8422_2325);
    h.u64(t.clock_ms as u64);
    h.u64(t.stats.ops);
    h.u64(t.stats.applied);
    h.u64(t.stats.unchanged);
    for &r in &t.stats.refused {
        h.u64(r);
    }
    for &c in &t.stats.conflicts {
        h.u64(c);
    }
    for v in [
        t.stats.created,
        t.stats.removed,
        t.stats.views_added,
        t.stats.reinstated,
        t.stats.handoffs_completed,
        t.stats.handoffs_implicit,
        t.stats.handoffs_retracted,
        t.stats.handoffs_failed,
        t.stats.handoff_clears_skipped,
        t.stats.lag_raised,
        t.stats.clock_held,
    ] {
        h.u64(v);
    }
    for f in &t.flights {
        h.flight(f);
    }
    for r in &t.refusals {
        h.key(&r.key);
        h.u64(r.ts_ms as u64);
        h.u64(r.avail_ms as u64);
        h.u64(r.msg);
        h.bytes(&[r.op, r.reason, r.state]);
    }
    for c in &t.conflicts {
        h.key(&c.a);
        h.key(&c.b);
        h.u64(c.ts_ms as u64);
        h.u64(c.msg);
        h.bytes(&[c.kind]);
    }
    for r in &t.removals {
        h.key(&r.key);
        h.u64(r.at_ms as u64);
        h.u64(r.last_avail_ms as u64);
        h.bytes(&[r.reason, r.state]);
    }
    h.0
}

struct Digest(u64);

impl Digest {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&u64::to_le_bytes(v));
    }
    fn key(&mut self, k: &FlightKey) {
        self.bytes(&u128::to_le_bytes(k.gufi));
        self.bytes(&k.callsign);
        self.bytes(&k.dep);
        self.bytes(&k.arr);
        self.bytes(&[k.kind]);
    }
    fn alt(&mut self, a: &crate::types::Alt) {
        self.bytes(&[a.form]);
        self.bytes(&i32::to_le_bytes(a.a_ft));
        self.bytes(&i32::to_le_bytes(a.b_ft));
    }
    fn stamp(&mut self, s: &crate::types::Stamp) {
        self.u64(s.ts_ms as u64);
        self.u64(s.avail_ms as u64);
        self.u64(s.msg);
    }
    fn flight(&mut self, f: &Flight) {
        self.key(&f.key);
        if f.key.kind == KEY_FREE {
            return;
        }
        self.alt(&f.assigned);
        self.alt(&f.interim);
        self.alt(&f.requested);
        self.bytes(&f.assigned_fix);
        self.bytes(&f.owner);
        self.bytes(&[f.ho_state]);
        self.bytes(&f.ho_from);
        self.bytes(&f.ho_to);
        self.bytes(&f.ho_accepted_by);
        self.bytes(&u32::to_le_bytes(f.address));
        self.bytes(&u32::to_le_bytes(f.addr_next));
        self.bytes(&u32::to_le_bytes(f.tfms_next));
        self.bytes(&f.callsign);
        self.bytes(&f.dep);
        self.bytes(&f.arr);
        self.bytes(&[f.n_views]);
        for v in f.views.iter().take(usize::from(f.n_views)) {
            self.bytes(&v.centre);
            self.bytes(&[v.state]);
            self.bytes(&v.plan_id);
            self.stamp(&v.stamp);
        }
        for s in &f.stamps {
            self.stamp(s);
        }
        self.bytes(&u32::to_le_bytes(f.ops_applied));
        self.u64(f.last_avail_ms as u64);
    }
}

fn alt_allowed(field: usize, form: u8) -> bool {
    match field {
        crate::types::F_ASSIGNED => matches!(
            form,
            ALT_NONE | ALT_SIMPLE | ALT_VFR_ON_TOP_PLUS | ALT_VFR | ALT_BLOCK | ALT_AT_FIX
        ),
        crate::types::F_INTERIM => matches!(form, ALT_NONE | ALT_SIMPLE | ALT_NIL),
        _ => matches!(form, ALT_NONE | ALT_SIMPLE | ALT_VFR_ON_TOP_PLUS | ALT_VFR),
    }
}

/// Whether `slot` is on the chain of claimants of `address`, from the newest, within the length
/// of the table, and every flight on the way has that address.
fn in_chain(t: &FlightTable, slot: u32, address: u32) -> bool {
    let flights = &t.flights;
    let Some(mut cur) = index_find(&t.by_addr, hash_addr(address), |s| {
        flights
            .get(s as usize)
            .is_some_and(|g| g.address == address)
    }) else {
        return false;
    };
    for _ in 0..t.flights.len() {
        if cur == slot {
            return true;
        }
        match t.flights.get(cur as usize) {
            Some(g) if g.address == address && g.addr_next != 0 => cur = g.addr_next - 1,
            _ => return false,
        }
    }
    false
}

fn in_tfms_chain(t: &FlightTable, slot: u32, f: &Flight) -> bool {
    let flights = &t.flights;
    let same = |s: u32| {
        flights
            .get(s as usize)
            .is_some_and(|g| g.callsign == f.callsign && g.dep == f.dep && g.arr == f.arr)
    };
    let Some(mut cur) = index_find(&t.by_tfms, hash_tfms(&f.callsign, &f.dep, &f.arr), same) else {
        return false;
    };
    for _ in 0..t.flights.len() {
        if cur == slot {
            return true;
        }
        match t.flights.get(cur as usize) {
            Some(g) if same(cur) && g.tfms_next != 0 => cur = g.tfms_next - 1,
            _ => return false,
        }
    }
    false
}

/// The chains of the address and TFMS indexes: each cell names the head of the chain of its key,
/// a chain is acyclic with distinct flights that all have the key and end in 0, every flight with
/// the key is on exactly one chain, and a link is 0 where the flight has no address or full key.
fn chain_audit(t: &FlightTable) -> Result<(), FlightError> {
    let inv = FlightError::Invariant;
    let n = t.flights.len();
    let (mut with_addr, mut with_tfms) = (0usize, 0usize);
    for (i, f) in t.flights.iter().enumerate() {
        let used = f.key.kind != KEY_FREE;
        if f.address != 0 && used {
            with_addr += 1;
        }
        if tfms_complete(f) && used {
            with_tfms += 1;
        }
        if (f.addr_next != 0 && (f.address == 0 || !used))
            || (f.tfms_next != 0 && (!tfms_complete(f) || !used))
        {
            return Err(inv(
                "a link is 0 for a flight without an address or full key",
            ));
        }
        if f.addr_next as usize == i + 1 || f.tfms_next as usize == i + 1 {
            return Err(inv("a flight does not link to itself"));
        }
    }
    let mut on_addr = 0usize;
    for &c in &t.by_addr.cells {
        if c == 0 {
            continue;
        }
        let head = c - 1;
        let Some(h) = t.flights.get(head as usize) else {
            return Err(inv("an address index cell names a slot"));
        };
        let address = h.address;
        if address == 0
            || index_find(&t.by_addr, hash_addr(address), |s| {
                t.flights
                    .get(s as usize)
                    .is_some_and(|g| g.address == address)
            }) != Some(head)
        {
            return Err(inv("an address index cell is the head of its address"));
        }
        let mut cur = head;
        let mut seen: Vec<u32> = Vec::new();
        loop {
            let Some(g) = t.flights.get(cur as usize) else {
                return Err(inv("an address chain stays inside the table"));
            };
            if g.key.kind == KEY_FREE || g.address != address || seen.contains(&cur) {
                return Err(inv(
                    "an address chain holds distinct used flights of the address",
                ));
            }
            seen.push(cur);
            on_addr += 1;
            if seen.len() > n {
                return Err(inv("an address chain is acyclic"));
            }
            if g.addr_next == 0 {
                break;
            }
            cur = g.addr_next - 1;
        }
    }
    if on_addr != with_addr {
        return Err(inv("every flight with an address is on exactly one chain"));
    }
    let mut on_tfms = 0usize;
    for &c in &t.by_tfms.cells {
        if c == 0 {
            continue;
        }
        let head = c - 1;
        let Some(h) = t.flights.get(head as usize).copied() else {
            return Err(inv("a TFMS index cell names a slot"));
        };
        let same = |g: &Flight| g.callsign == h.callsign && g.dep == h.dep && g.arr == h.arr;
        if !tfms_complete(&h)
            || index_find(&t.by_tfms, hash_tfms(&h.callsign, &h.dep, &h.arr), |s| {
                t.flights.get(s as usize).is_some_and(same)
            }) != Some(head)
        {
            return Err(inv("a TFMS index cell is the head of its key"));
        }
        let mut cur = head;
        let mut seen: Vec<u32> = Vec::new();
        loop {
            let Some(g) = t.flights.get(cur as usize) else {
                return Err(inv("a TFMS chain stays inside the table"));
            };
            if g.key.kind == KEY_FREE || !same(g) || seen.contains(&cur) {
                return Err(inv("a TFMS chain holds distinct used flights of the key"));
            }
            seen.push(cur);
            on_tfms += 1;
            if seen.len() > n {
                return Err(inv("a TFMS chain is acyclic"));
            }
            if g.tfms_next == 0 {
                break;
            }
            cur = g.tfms_next - 1;
        }
    }
    if on_tfms != with_tfms {
        return Err(inv("every flight with a full key is on exactly one chain"));
    }
    Ok(())
}

/// Audits every invariant of the table and returns the first that does not hold:
/// each used slot has a valid key found by the key index at that slot; the used slots equal the
/// key index entries and `created - removed`; views have distinct centres and valid states; no
/// view is stamped after the clock; altitude forms are allowed for their field and a block is
/// ordered; a pending handoff has its units and none pending has none; each address and TFMS-key
/// index entry names a flight that has that address or key; the logs are within capacity and
/// every removal is in the log or counted as blocked. Core (pure), one pass over the table.
pub fn flights_check(t: &FlightTable) -> Result<(), FlightError> {
    let inv = FlightError::Invariant;
    if t.flights.len() != t.cfg.capacity {
        return Err(inv("flights length is the capacity"));
    }
    let mut used = 0usize;
    for (i, f) in t.flights.iter().enumerate() {
        let slot = u32::try_from(i).map_err(|_| inv("slot number"))?;
        if f.key.kind == KEY_FREE {
            if t.free.iter().filter(|&&s| s == slot).count() != 1 {
                return Err(inv("a free slot is on the free stack once"));
            }
            continue;
        }
        used += 1;
        if t.free.contains(&slot) {
            return Err(inv("a used slot is not on the free stack"));
        }
        match f.key.kind {
            KEY_GUFI => {
                if f.key.callsign != [0; 8] || f.key.dep != [0; 12] || f.key.arr != [0; 12] {
                    return Err(inv("a GUFI key has only the GUFI"));
                }
            }
            KEY_TFMS => {
                if f.key.gufi != 0
                    || f.key.callsign == [0; 8]
                    || f.key.dep == [0; 12]
                    || f.key.arr == [0; 12]
                {
                    return Err(inv("a TFMS key has its three parts and no GUFI"));
                }
            }
            _ => return Err(inv("key kind")),
        }
        if slot_of_key(t, &f.key) != Some(slot) {
            return Err(inv("the key index finds the flight at its slot"));
        }
        if usize::from(f.n_views) > MAX_VIEWS {
            return Err(inv("views within MAX_VIEWS"));
        }
        for (k, v) in f.views.iter().enumerate() {
            if k >= usize::from(f.n_views) {
                if *v != FLIGHT_FREE.views[0] {
                    return Err(inv("an unused view is blank"));
                }
                continue;
            }
            if v.state == ST_NONE || usize::from(v.state) >= ST_COUNT {
                return Err(inv("a view has a valid state"));
            }
            if v.centre == [0; 4] {
                return Err(inv("a view names its centre"));
            }
            if f.views.iter().take(k).any(|w| w.centre == v.centre) {
                return Err(inv(
                    "one view per centre: at most one plan per flight and centre",
                ));
            }
            if v.stamp.avail_ms > t.clock_ms {
                return Err(inv("no view is stamped after the clock"));
            }
        }
        for s in f.stamps.iter().take(F_COUNT) {
            if s.avail_ms > t.clock_ms {
                return Err(inv("no field is stamped after the clock"));
            }
            if s.avail_ms != 0 && s.avail_ms < s.ts_ms.saturating_add(t.cfg.lag_floor_ms) {
                // The availability is never before the stamped time plus the floor.
                return Err(inv(
                    "availability is not before the stamped time plus the lag floor",
                ));
            }
        }
        if f.last_avail_ms > t.clock_ms {
            return Err(inv("last availability is not after the clock"));
        }
        for (field, a) in [
            (crate::types::F_ASSIGNED, &f.assigned),
            (crate::types::F_INTERIM, &f.interim),
            (crate::types::F_REQUESTED, &f.requested),
        ] {
            if !alt_allowed(field, a.form) {
                return Err(inv("an altitude form is allowed for its field"));
            }
            if a.form == ALT_BLOCK && a.a_ft > a.b_ft {
                return Err(inv("a block is ordered"));
            }
            if a.form == ALT_VFR && (a.a_ft != 0 || a.b_ft != 0) {
                return Err(inv("VFR carries no number"));
            }
        }
        if (f.assigned.form == ALT_AT_FIX) != (f.assigned_fix != [0; 12]) {
            return Err(inv(
                "an altitude at a fix names the fix, any other form does not",
            ));
        }
        match f.ho_state {
            HO_NONE => {
                if f.ho_from != [0; 8] || f.ho_to != [0; 8] || f.ho_accepted_by != [0; 8] {
                    return Err(inv("no handoff pending holds no units"));
                }
            }
            HO_OFFERED => {
                if f.ho_from == [0; 8] || f.ho_to == [0; 8] || f.ho_accepted_by != [0; 8] {
                    return Err(inv(
                        "an offered handoff has its two units and no acceptance",
                    ));
                }
            }
            HO_ACCEPTED => {
                if f.ho_from == [0; 8] || f.ho_to == [0; 8] || f.ho_accepted_by == [0; 8] {
                    return Err(inv("an accepted handoff has its three units"));
                }
            }
            _ => return Err(inv("handoff state")),
        }
        if f.address != 0 && !in_chain(t, slot, f.address) {
            return Err(inv(
                "a flight with an address is in the chain of its address",
            ));
        }
        if tfms_complete(f) && !in_tfms_chain(t, slot, f) {
            return Err(inv(
                "a flight with a full TFMS key is in the chain of its key",
            ));
        }
    }
    if used != flights_len(t) {
        return Err(inv("used slots are capacity minus free"));
    }
    if t.by_key.used != used {
        return Err(inv("the key index holds one entry per flight"));
    }
    if (t.stats.created.saturating_sub(t.stats.removed)) != used as u64 {
        return Err(inv("flights created minus removed is the flights held"));
    }
    if t.removals.len() > t.cfg.removal_log
        || t.refusals.len() > t.cfg.refusal_log
        || t.conflicts.len() > t.cfg.conflict_log
    {
        return Err(inv("logs within their capacity"));
    }
    chain_audit(t)?;
    // Every index entry names a flight that has the key it is filed under.
    for &c in &t.by_addr.cells {
        if c == 0 {
            continue;
        }
        match t.flights.get((c - 1) as usize) {
            Some(f) if f.key.kind != KEY_FREE && f.address != 0 => {}
            _ => return Err(inv("an address index entry names a flight with an address")),
        }
    }
    for &c in &t.by_tfms.cells {
        if c == 0 {
            continue;
        }
        match t.flights.get((c - 1) as usize) {
            Some(f) if f.key.kind != KEY_FREE && tfms_complete(f) => {}
            _ => {
                return Err(inv(
                    "a TFMS index entry names a flight with a full TFMS key",
                ))
            }
        }
    }
    Ok(())
}

//! Slot bookkeeping shared by the TFMS and FNS tables: a fixed number of slots, a free stack, an
//! occupancy flag and an open-addressing [`Index`] keyed by a hash the owner computes. Core
//! (pure, B1): no I/O, no clock, no allocation after [`new_slots`].
//!
//! The owner keeps the key columns; this type keeps only each slot's key hash, so removing an
//! entry needs no knowledge of the key layout. Eviction order is slot order, so what is evicted
//! never depends on anything but the table's history.

use crate::core::index::{index_find, index_insert, index_remove, new_index, Index};

/// Slot bookkeeping. Plain data (A1); behaviour is in the free functions below.
// A7.2: cold struct, size guard waived (one per table)
#[derive(Clone, Debug)]
pub struct Slots {
    /// Slots in total.
    pub capacity: usize,
    /// Slots in use.
    pub live: usize,
    /// Whether slot `i` is in use.
    pub occupied: Vec<bool>,
    /// Free slots, a stack: the next to be used is the last; initially the lowest slot on top.
    pub free: Vec<u32>,
    /// The key hash of each slot in use.
    pub hash: Vec<u64>,
    /// Key hash to slot.
    pub index: Index,
}

/// Empty bookkeeping for `capacity` slots. Everything is allocated here.
pub fn new_slots(capacity: usize) -> Slots {
    Slots {
        capacity,
        live: 0,
        occupied: vec![false; capacity],
        free: (0..capacity as u32).rev().collect(),
        hash: vec![0; capacity],
        index: new_index(capacity),
    }
}

/// The slot in use whose key hashes to `hash` and satisfies `eq`.
pub fn slots_find(s: &Slots, hash: u64, eq: impl Fn(usize) -> bool) -> Option<usize> {
    index_find(&s.index, hash, |i| eq(i as usize)).map(|i| i as usize)
}

/// Takes a free slot for a key hashing to `hash` and indexes it. `None` when the table is full.
/// The caller has checked the key is not already present and fills the columns.
pub fn slots_alloc(s: &mut Slots, hash: u64) -> Option<usize> {
    let i = s.free.pop()?;
    if !index_insert(&mut s.index, hash, i) {
        s.free.push(i);
        return None;
    }
    if let Some(o) = s.occupied.get_mut(i as usize) {
        *o = true;
    }
    if let Some(h) = s.hash.get_mut(i as usize) {
        *h = hash;
    }
    s.live += 1;
    Some(i as usize)
}

/// Frees slot `i`: out of the index and back on the free stack. The caller blanks the columns.
pub fn slots_release(s: &mut Slots, i: usize) {
    if !s.occupied.get(i).copied().unwrap_or(false) {
        return;
    }
    let h = s.hash.get(i).copied().unwrap_or(0);
    let hashes = &s.hash;
    index_remove(&mut s.index, h, i as u32, |o| {
        hashes.get(o as usize).copied().unwrap_or(0)
    });
    if let Some(o) = s.occupied.get_mut(i) {
        *o = false;
    }
    s.free.push(i as u32);
    s.live = s.live.saturating_sub(1);
}

/// Whether slot `i` is in use.
pub fn slot_live(s: &Slots, i: usize) -> bool {
    s.occupied.get(i).copied().unwrap_or(false)
}

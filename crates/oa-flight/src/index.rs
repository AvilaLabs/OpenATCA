//! Fixed-size open-addressing hash index from a key to a slot number. Core (pure, B1): no
//! allocation after [`new_index`].
//!
//! Linear probing with backward-shift deletion (no tombstones). The index stores `slot + 1`
//! (0 is empty); the key lives in the table's flights, and the caller supplies the equality test
//! and, for removal, the hash of an entry's key. The index is sized to the next power of two at
//! or above twice the entries it may hold, so the load never exceeds 1/2 and every probe ends
//! within the length of the cell array (C2, C7). The hash is fixed and unkeyed so a replay
//! does not depend on a seed (B6); the keys are assigned by the FAA's systems, not chosen by a
//! remote party.

use alloc::vec;
use alloc::vec::Vec;

use crate::types::{FlightKey, KEY_GUFI};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Index {
    /// `slot + 1`, or 0 for an empty cell. Length is a power of two.
    pub(crate) cells: Vec<u32>,
    pub(crate) mask: usize,
    pub(crate) used: usize,
}

pub(crate) fn new_index(max_entries: usize) -> Index {
    let len = max_entries.saturating_mul(2).max(2).next_power_of_two();
    Index {
        cells: vec![0; len],
        mask: len - 1,
        used: 0,
    }
}

/// The finalizer of SplitMix64: a fixed permutation of `u64`.
pub(crate) fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    let mut h = h;
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

const FNV_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// Hash of a flight key.
pub(crate) fn hash_key(k: &FlightKey) -> u64 {
    if k.kind == KEY_GUFI {
        return mix64((k.gufi as u64) ^ mix64((k.gufi >> 64) as u64));
    }
    mix64(hash_tfms(&k.callsign, &k.dep, &k.arr) ^ u64::from(k.kind))
}

/// Hash of a callsign, departure and arrival.
pub(crate) fn hash_tfms(callsign: &[u8; 8], dep: &[u8; 12], arr: &[u8; 12]) -> u64 {
    mix64(fnv(fnv(fnv(FNV_SEED, callsign), dep), arr))
}

/// Hash of an aircraft address.
pub(crate) fn hash_addr(a: u32) -> u64 {
    mix64(u64::from(a) ^ 0x5bd1_e995_0000_0000)
}

/// The slot whose key satisfies `eq`, starting the probe at `hash`.
pub(crate) fn index_find(idx: &Index, hash: u64, eq: impl Fn(u32) -> bool) -> Option<u32> {
    let mut i = (hash as usize) & idx.mask;
    for _ in 0..idx.cells.len() {
        let c = *idx.cells.get(i)?;
        if c == 0 {
            return None;
        }
        let slot = c - 1;
        if eq(slot) {
            return Some(slot);
        }
        i = (i + 1) & idx.mask;
    }
    None
}

/// Stores `slot` for a key that is not in the index. False only if the index is full, which its
/// sizing makes unreachable while the table holds at most its capacity.
pub(crate) fn index_insert(idx: &mut Index, hash: u64, slot: u32) -> bool {
    if idx.used >= idx.cells.len() / 2 {
        return false;
    }
    let mut i = (hash as usize) & idx.mask;
    for _ in 0..idx.cells.len() {
        match idx.cells.get_mut(i) {
            Some(c) if *c == 0 => {
                *c = slot + 1;
                idx.used += 1;
                return true;
            }
            Some(_) => i = (i + 1) & idx.mask,
            None => return false,
        }
    }
    false
}

/// Removes the entry for `slot`, found by probing from `hash`. `home` gives the hash of the key
/// of any entry (by its slot); entries after the hole shift back so no later lookup breaks.
pub(crate) fn index_remove(
    idx: &mut Index,
    hash: u64,
    slot: u32,
    home: impl Fn(u32) -> u64,
) -> bool {
    let mut i = (hash as usize) & idx.mask;
    let mut found = false;
    for _ in 0..idx.cells.len() {
        match idx.cells.get(i) {
            Some(0) | None => return false,
            Some(&c) if c == slot + 1 => {
                found = true;
                break;
            }
            Some(_) => i = (i + 1) & idx.mask,
        }
    }
    if !found {
        return false;
    }
    let mut hole = i;
    let mut j = i;
    for _ in 0..idx.cells.len() {
        j = (j + 1) & idx.mask;
        let c = idx.cells.get(j).copied().unwrap_or(0);
        if c == 0 {
            break;
        }
        let k = (home(c - 1) as usize) & idx.mask;
        // The entry at `j` may move into the hole unless its home cell lies cyclically in
        // (hole, j], where it would end up before its own probe start.
        let in_range = if hole <= j {
            k > hole && k <= j
        } else {
            k > hole || k <= j
        };
        if !in_range {
            if let Some(v) = idx.cells.get_mut(hole) {
                *v = c;
            }
            hole = j;
        }
    }
    if let Some(v) = idx.cells.get_mut(hole) {
        *v = 0;
    }
    idx.used = idx.used.saturating_sub(1);
    true
}

/// Points the entry that holds `old` at `new` instead, found by probing from `hash`. The key is
/// the same, so no entry moves. False if `old` is not in the index.
pub(crate) fn index_replace(idx: &mut Index, hash: u64, old: u32, new: u32) -> bool {
    let mut i = (hash as usize) & idx.mask;
    for _ in 0..idx.cells.len() {
        match idx.cells.get_mut(i) {
            Some(0) | None => return false,
            Some(c) if *c == old + 1 => {
                *c = new + 1;
                return true;
            }
            Some(_) => i = (i + 1) & idx.mask,
        }
    }
    false
}

//! Fixed-size open-addressing hash index from a key to a slot number. Core (pure, B1): no I/O,
//! no clock, no allocation after [`new_index`].
//!
//! Linear probing with backward-shift deletion (no tombstones, so a long-lived index never
//! degrades). The index stores only `slot + 1` (0 is empty); the key lives in the owning table's
//! columns, and the caller supplies the equality test and, for removal, the hash of an entry's
//! key. That keeps one copy of every key (D4: the layout is the owner's).
//!
//! Capacity (C2): the index is sized to the next power of two at or above twice the number of
//! entries it may ever hold, so the load never exceeds 1/2. At load 1/2 linear probing needs
//! 1.5 probes for a hit and 2.5 for a miss on average for well-spread hashes; an empty cell
//! always exists, so every probe sequence ends within `len` steps.
//!
//! Not adversarial-robust, on purpose. The hash is fixed and unkeyed (a keyed or randomised hash
//! would make a replay depend on a seed, breaking B6). A sender who could choose many keys that
//! share a home cell could lengthen probe runs, but never beyond the bound above: the worst case
//! for one operation is `len` probes (65,536 at the default capacity, tens of microseconds), the
//! table cannot overflow, and the keys here (GUFIs, flight-plan identifiers, TBFM ids) are
//! assigned by the FAA's own systems, not chosen by a remote party.

/// An open-addressing index. Plain data (A1); behaviour is in the free functions below.
// A7.2: cold struct, size guard waived (one per table, not held in quantity)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    /// `slot + 1`, or 0 for an empty cell. Length is a power of two.
    pub cells: Vec<u32>,
    /// `cells.len() - 1`.
    pub mask: usize,
    /// Entries stored.
    pub used: usize,
}

/// An index that can hold `max_entries` at a load of at most 1/2.
pub fn new_index(max_entries: usize) -> Index {
    let len = max_entries.saturating_mul(2).max(2).next_power_of_two();
    Index {
        cells: vec![0; len],
        mask: len - 1,
        used: 0,
    }
}

/// The finalizer of SplitMix64: a fast, well-mixing, fixed permutation of `u64`.
pub fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Hash of a 128-bit key (a GUFI).
pub fn hash_u128(k: u128) -> u64 {
    mix64((k as u64) ^ mix64((k >> 64) as u64))
}

/// Hash of a short byte key: FNV-1a over the bytes, then [`mix64`].
pub fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in b {
        h = (h ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    mix64(h)
}

/// The slot whose key satisfies `eq`, starting the probe at `hash`.
pub fn index_find(idx: &Index, hash: u64, eq: impl Fn(u32) -> bool) -> Option<u32> {
    let mut i = (hash as usize) & idx.mask;
    // At most `len` steps: the load bound leaves an empty cell, so the loop ends before that.
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

/// Stores `slot` for a key that is not in the index. Returns false only if the index is full,
/// which the sizing makes unreachable when the owner never stores more than its capacity.
pub fn index_insert(idx: &mut Index, hash: u64, slot: u32) -> bool {
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
/// of any entry (by its slot); entries after the hole are shifted back so no later lookup
/// breaks. Returns false if the slot was not found.
pub fn index_remove(idx: &mut Index, hash: u64, slot: u32, home: impl Fn(u32) -> u64) -> bool {
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
    loop {
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

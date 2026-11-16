//! A fixed-capacity open-addressing hash table, allocated once (RULESET C1, C2).
//!
//! Linear probing with backward-shift deletion: no tombstones, so a lookup never walks past
//! dead entries and a table that has seen many deletions probes as short as a fresh one. Robin
//! Hood would shorten the worst probe, but at a load of one half or less the plain probe is
//! short already and this keeps the code small. The hash is a fixed integer mixer (the
//! splitmix64 finalizer) with no per-run seed, so the layout, and with it every iteration order,
//! is the same on every run. The table never grows: an insert of a new key into a full table is
//! refused and counted in [`FixedMap::refused`].

/// A key that hashes to 64 bits the same way on every run.
pub trait TableKey: Copy + Eq {
    /// The key as an integer to mix.
    fn bits(self) -> u64;
}

impl TableKey for u32 {
    fn bits(self) -> u64 {
        u64::from(self)
    }
}

impl TableKey for (u32, u8) {
    fn bits(self) -> u64 {
        u64::from(self.0) | u64::from(self.1) << 32
    }
}

/// The splitmix64 finalizer: a bijection on 64 bits with good avalanche.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The table. A7.2: cold, per-key.
pub struct FixedMap<K: TableKey, V: Copy> {
    slots: Vec<Option<(K, V)>>,
    mask: usize,
    len: usize,
    refused: u64,
}

impl<K: TableKey, V: Copy> FixedMap<K, V> {
    /// A table of `capacity` slots, which must be a power of two (a smaller or other value is
    /// rounded up to one). The only allocation of the table's life.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(2).next_power_of_two();
        FixedMap {
            slots: vec![None; cap],
            mask: cap - 1,
            len: 0,
            refused: 0,
        }
    }

    /// Entries held.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no entry is held.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Slots (a power of two; never changes).
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Inserts of a new key refused because every slot was taken.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// The address and slot count of the storage, for a test that the table was never moved.
    pub fn buffer(&self) -> (usize, usize) {
        (self.slots.as_ptr() as usize, self.slots.capacity())
    }

    fn home(&self, k: K) -> usize {
        (mix(k.bits()) as usize) & self.mask
    }

    /// The slot holding `k`, if any.
    fn find(&self, k: K) -> Option<usize> {
        let mut i = self.home(k);
        // At most `capacity` probes: a full table without the key ends the loop.
        for _ in 0..self.slots.len() {
            match self.slots.get(i)? {
                None => return None,
                Some((key, _)) if *key == k => return Some(i),
                Some(_) => i = (i + 1) & self.mask,
            }
        }
        None
    }

    /// The value of `k`.
    pub fn get(&self, k: &K) -> Option<&V> {
        let i = self.find(*k)?;
        self.slots.get(i)?.as_ref().map(|(_, v)| v)
    }

    /// The value of `k`, to change.
    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let i = self.find(*k)?;
        self.slots.get_mut(i)?.as_mut().map(|(_, v)| v)
    }

    /// Whether `k` is held.
    pub fn contains_key(&self, k: &K) -> bool {
        self.find(*k).is_some()
    }

    /// Sets `k` to `v`. `false`, with nothing stored and the refusal counted, when `k` is new
    /// and the table is full.
    pub fn insert(&mut self, k: K, v: V) -> bool {
        if let Some(i) = self.find(k) {
            if let Some(s) = self.slots.get_mut(i) {
                *s = Some((k, v));
            }
            return true;
        }
        if self.len >= self.slots.len() {
            self.refused += 1;
            return false;
        }
        let mut i = self.home(k);
        loop {
            match self.slots.get_mut(i) {
                Some(s @ None) => {
                    *s = Some((k, v));
                    self.len += 1;
                    return true;
                }
                Some(_) => i = (i + 1) & self.mask,
                // Unreachable: the index is masked into range.
                None => return false,
            }
        }
    }

    /// Removes `k`, returning its value.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let i = self.find(*k)?;
        self.remove_at(i)
    }

    /// Empties slot `i` and shifts back the entries of its cluster that probed past it, so no
    /// entry is separated from its home by a hole.
    fn remove_at(&mut self, i: usize) -> Option<V> {
        let (_, v) = self.slots.get_mut(i)?.take()?;
        self.len -= 1;
        let mut hole = i;
        let mut j = i;
        loop {
            j = (j + 1) & self.mask;
            let Some((key, _)) = self.slots.get(j).copied().flatten() else {
                break;
            };
            let home = self.home(key);
            // The entry at j may fill the hole when its home is not in the cyclic interval
            // (hole, j]: its probe path then passes through the hole.
            let dist_home = j.wrapping_sub(home) & self.mask;
            let dist_hole = j.wrapping_sub(hole) & self.mask;
            if dist_home >= dist_hole {
                let moved = self.slots.get_mut(j).and_then(Option::take);
                if let Some(s) = self.slots.get_mut(hole) {
                    *s = moved;
                }
                hole = j;
            }
        }
        Some(v)
    }

    /// Keeps the entries for which `keep` is true. No allocation: the scan stays at a slot a
    /// removal refilled, and an entry that shifts back is never behind the scan before it has
    /// been seen.
    pub fn retain<F: FnMut(&K, &V) -> bool>(&mut self, mut keep: F) {
        let mut i = 0;
        while i < self.slots.len() {
            let drop_it = match self.slots.get(i) {
                Some(Some((k, v))) => !keep(k, v),
                _ => false,
            };
            if drop_it {
                self.remove_at(i);
            } else {
                i += 1;
            }
        }
    }

    /// Every entry, in slot order (fixed by the hash, not by key).
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots.iter().flatten().map(|(k, v)| (k, v))
    }
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    /// The hash, replaced for the proofs by the identity: the table's logic reads a key only
    /// through equality and its home slot, and with free keys every assignment of homes is
    /// reachable already, so the 64-bit multiplications of the real mixer add cost and no case.
    fn mix_identity(x: u64) -> u64 {
        x
    }

    /// The reference: the held (key, value) pairs in no particular order, at most `OPS`.
    struct Model<const OPS: usize> {
        e: [Option<(u32, u8)>; OPS],
        refused: u64,
    }

    impl<const OPS: usize> Model<OPS> {
        fn pos(&self, k: u32) -> Option<usize> {
            for i in 0..OPS {
                if let Some((key, _)) = self.e[i] {
                    if key == k {
                        return Some(i);
                    }
                }
            }
            None
        }
        fn count(&self) -> usize {
            let mut c = 0;
            for i in 0..OPS {
                if self.e[i].is_some() {
                    c += 1;
                }
            }
            c
        }
    }

    /// The keep predicate of a retain: a function of the pair, chosen by `mask`.
    fn keeps(mask: u8, k: u32, v: u8) -> bool {
        (mask >> ((k ^ u32::from(v)) & 7)) & 1 == 1
    }

    /// `OPS` symbolic operations on a table of `CAP` slots, checked against the reference after
    /// each.
    fn run<const CAP: usize, const OPS: usize>() {
        let mut m: FixedMap<u32, u8> = FixedMap::new(CAP);
        let mut r: Model<OPS> = Model {
            e: [None; OPS],
            refused: 0,
        };
        for _ in 0..OPS {
            let op: u8 = kani::any();
            kani::assume(op < 4);
            let k: u32 = kani::any();
            let v: u8 = kani::any();
            let mask: u8 = kani::any();
            match op {
                0 => {
                    let was_new = r.pos(k).is_none();
                    let full = r.count() >= CAP;
                    let ok = m.insert(k, v);
                    // Refused exactly when the key is new and the table is full.
                    assert_eq!(ok, !(was_new && full));
                    if ok {
                        match r.pos(k) {
                            Some(i) => r.e[i] = Some((k, v)),
                            None => {
                                let mut placed = false;
                                for i in 0..OPS {
                                    if !placed && r.e[i].is_none() {
                                        r.e[i] = Some((k, v));
                                        placed = true;
                                    }
                                }
                                assert!(placed);
                            }
                        }
                    } else {
                        r.refused += 1;
                    }
                }
                1 => {
                    let want = r.pos(k).and_then(|i| r.e[i]).map(|(_, x)| x);
                    assert_eq!(m.get(&k).copied(), want);
                    assert_eq!(m.contains_key(&k), want.is_some());
                }
                2 => {
                    let want = r.pos(k).and_then(|i| r.e[i]).map(|(_, x)| x);
                    assert_eq!(m.remove(&k), want);
                    if let Some(i) = r.pos(k) {
                        r.e[i] = None;
                    }
                }
                _ => {
                    m.retain(|key, val| keeps(mask, *key, *val));
                    for i in 0..OPS {
                        if let Some((key, val)) = r.e[i] {
                            if !keeps(mask, key, val) {
                                r.e[i] = None;
                            }
                        }
                    }
                }
            }
            // The invariants after every operation.
            assert_eq!(m.len(), r.count());
            assert_eq!(m.is_empty(), r.count() == 0);
            assert_eq!(m.refused(), r.refused);
            assert_eq!(m.capacity(), CAP);
            // No key lost: every held pair is found with its value.
            for i in 0..OPS {
                if let Some((key, val)) = r.e[i] {
                    assert_eq!(m.get(&key).copied(), Some(val));
                }
            }
            // No key duplicated or invented: the table holds exactly `len` pairs, each in the
            // reference, and no two equal.
            let mut seen = 0;
            let mut keys = [0u32; CAP];
            for (key, val) in m.iter() {
                assert!(seen < CAP);
                for j in 0..seen {
                    assert!(keys[j] != *key);
                }
                keys[seen] = *key;
                seen += 1;
                let i = r.pos(*key);
                assert!(i.is_some());
                assert_eq!(r.e[i.unwrap_or(0)], Some((*key, *val)));
            }
            assert_eq!(seen, m.len());
            // A probe key: absent from the reference means absent from the table.
            let probe: u32 = kani::any();
            assert_eq!(m.contains_key(&probe), r.pos(probe).is_some());
        }
    }

    // REQ: OA-TRK-303
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::solver(cadical)]
    #[kani::stub(mix, mix_identity)]
    fn fixed_map_agrees_with_a_reference_model_2_slots() {
        // Two slots, four operations: a refusal, wrap-around and a shift back are reachable.
        run::<2, 4>();
    }
}

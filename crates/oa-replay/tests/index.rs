//! Tests of the open-addressing index and the deterministic hashes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::Rng;
use oa_replay::index::{
    hash_bytes, hash_u128, index_find, index_insert, index_remove, mix64, new_index,
};
use std::collections::HashMap;

// REQ: OA-RPL-014
#[test]
fn index_matches_a_model_under_random_inserts_and_removes_with_stated_load() {
    // A deliberately poor hash (few distinct homes) forces long probe runs and wraparound, which
    // exercises backward-shift deletion; the model is std's HashMap.
    for (cap, spread) in [(64usize, 4u64), (64, 1 << 20), (7, 3), (1000, 16)] {
        let mut idx = new_index(cap);
        assert!(idx.cells.len() >= 2 * cap && idx.cells.len().is_power_of_two());
        let mut keys: Vec<u64> = vec![0; cap];
        let mut model: HashMap<u64, u32> = HashMap::new();
        let mut free: Vec<u32> = (0..cap as u32).rev().collect();
        let mut rng = Rng(0x1234_5678_9abc_def1 ^ cap as u64);
        let home = |k: u64| mix64(k % spread);
        for step in 0..20_000 {
            let k = rng.next() % (cap as u64 * 3);
            if let Some(&slot) = model.get(&k) {
                if step % 3 != 0 {
                    // Lookup of a present key.
                    assert_eq!(
                        index_find(&idx, home(k), |s| keys[s as usize] == k),
                        Some(slot)
                    );
                } else {
                    assert!(index_remove(&mut idx, home(k), slot, |s| home(
                        keys[s as usize]
                    )));
                    model.remove(&k);
                    free.push(slot);
                }
            } else if let Some(slot) = free.pop() {
                assert_eq!(index_find(&idx, home(k), |s| keys[s as usize] == k), None);
                keys[slot as usize] = k;
                assert!(index_insert(&mut idx, home(k), slot));
                model.insert(k, slot);
            }
            assert_eq!(idx.used, model.len());
            assert!(idx.used * 2 <= idx.cells.len(), "load bound");
        }
        for (&k, &slot) in &model {
            assert_eq!(
                index_find(&idx, home(k), |s| keys[s as usize] == k),
                Some(slot)
            );
        }
        // Removing something absent is reported, not a corruption.
        assert!(!index_remove(&mut idx, home(u64::MAX), u32::MAX - 1, |s| {
            home(keys[s as usize])
        }));
    }
}

// REQ: OA-RPL-014
#[test]
fn index_refuses_past_its_load_bound_and_hashes_are_fixed_functions() {
    let mut idx = new_index(4);
    let n = idx.cells.len() / 2;
    for s in 0..n as u32 {
        assert!(index_insert(&mut idx, u64::from(s), s));
    }
    assert!(!index_insert(&mut idx, 99, 99), "full at load 1/2");
    // The hashes are pure functions of the key: fixed values pin them across machines.
    assert_eq!(mix64(0), 0xe220_a839_7b1d_cdaf);
    assert_eq!(hash_u128(0), mix64(mix64(0)));
    assert_eq!(hash_bytes(b""), mix64(0xcbf2_9ce4_8422_2325));
    assert_ne!(hash_bytes(b"KA000001AB"), hash_bytes(b"KA000002AB"));
    assert_ne!(hash_u128(1), hash_u128(1 << 64));
}

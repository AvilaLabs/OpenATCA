//! The fixed-capacity table behind the fusion layer's per-key bookkeeping.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_track::fixed_map::FixedMap;
use std::collections::BTreeMap;

/// A small xorshift stream for the model test.
fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

// REQ: OA-TRK-292
#[test]
fn insert_lookup_overwrite_and_remove() {
    let mut m: FixedMap<u32, u32> = FixedMap::new(16);
    assert!(m.is_empty());
    for k in 0..8u32 {
        assert!(m.insert(k, k * 10));
    }
    assert_eq!(m.len(), 8);
    assert_eq!(m.get(&3), Some(&30));
    assert!(m.insert(3, 99));
    assert_eq!(m.len(), 8);
    assert_eq!(m.get(&3), Some(&99));
    *m.get_mut(&4).unwrap() = 7;
    assert_eq!(m.remove(&4), Some(7));
    assert_eq!(m.remove(&4), None);
    assert!(!m.contains_key(&4));
    assert_eq!(m.len(), 7);
    assert_eq!(m.get(&100), None);
}

// REQ: OA-TRK-292
#[test]
fn a_full_table_refuses_a_new_key_counts_it_and_keeps_its_storage() {
    let mut m: FixedMap<u32, u8> = FixedMap::new(8);
    let at = m.buffer();
    for k in 0..8u32 {
        assert!(m.insert(k, 1));
    }
    assert_eq!(m.len(), 8);
    assert!(!m.insert(1000, 2));
    assert!(!m.insert(1001, 2));
    assert_eq!(m.refused(), 2);
    assert_eq!(m.len(), 8);
    assert!(!m.contains_key(&1000));
    // A key already there is still updated, and a lookup of a missing key ends in a full table.
    assert!(m.insert(5, 9));
    assert_eq!(m.get(&5), Some(&9));
    assert_eq!(m.get(&2000), None);
    // Room made by a removal is usable again.
    assert_eq!(m.remove(&0), Some(1));
    assert!(m.insert(1000, 2));
    assert_eq!(m.buffer(), at);
    assert_eq!(m.capacity(), 8);
}

// REQ: OA-TRK-292
#[test]
fn deletion_under_collisions_shifts_the_cluster_back() {
    // A table of two slots per key at most is dense in collisions when filled to the brim; fill
    // it, delete in several orders, and check every survivor is still found and nothing else.
    for cap in [8usize, 16, 64] {
        for start in 0..cap as u32 {
            let mut m: FixedMap<u32, u32> = FixedMap::new(cap);
            for k in 0..cap as u32 {
                assert!(m.insert(k.wrapping_mul(0x9e37_79b9) ^ start, k));
            }
            let keys: Vec<u32> = (0..cap as u32)
                .map(|k| k.wrapping_mul(0x9e37_79b9) ^ start)
                .collect();
            for (n, k) in keys.iter().enumerate() {
                if (n + start as usize).is_multiple_of(3) {
                    assert_eq!(m.remove(k), Some(n as u32));
                }
            }
            for (n, k) in keys.iter().enumerate() {
                let gone = (n + start as usize).is_multiple_of(3);
                assert_eq!(m.get(k).copied(), (!gone).then_some(n as u32), "cap {cap}");
            }
        }
    }
}

// REQ: OA-TRK-292
#[test]
fn retain_matches_a_model_through_wraparound() {
    let mut s = 0x1234_5678_9abc_def0u64;
    for round in 0..200 {
        let mut m: FixedMap<(u32, u8), u64> = FixedMap::new(64);
        let mut model = BTreeMap::new();
        for _ in 0..(20 + round % 40) {
            let k = ((next(&mut s) % 90) as u32, (next(&mut s) % 3) as u8);
            let v = next(&mut s);
            if m.insert(k, v) {
                model.insert(k, v);
            }
        }
        let m3 = 2 + round as u64 % 4;
        m.retain(|k, v| (u64::from(k.0) + v) % m3 != 0);
        model.retain(|k, v| (u64::from(k.0) + *v) % m3 != 0);
        assert_eq!(m.len(), model.len());
        for (k, v) in &model {
            assert_eq!(m.get(k), Some(v));
        }
        let mut got: Vec<_> = m.iter().map(|(k, v)| (*k, *v)).collect();
        got.sort_unstable();
        let want: Vec<_> = model.into_iter().collect();
        assert_eq!(got, want);
    }
}

// REQ: OA-TRK-292
#[test]
fn random_operations_agree_with_a_model_and_the_layout_is_fixed() {
    let run = |seed: u64| {
        let mut s = seed;
        let mut m: FixedMap<u32, u64> = FixedMap::new(128);
        let mut model = BTreeMap::new();
        for _ in 0..20_000 {
            let k = (next(&mut s) % 100) as u32;
            if next(&mut s).is_multiple_of(2) {
                let v = next(&mut s);
                assert!(m.insert(k, v));
                model.insert(k, v);
            } else {
                assert_eq!(m.remove(&k), model.remove(&k));
            }
            assert_eq!(m.len(), model.len());
        }
        for k in 0..100u32 {
            assert_eq!(m.get(&k), model.get(&k));
        }
        m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()
    };
    // The slot order is a function of the operations alone: the same twice.
    assert_eq!(run(7), run(7));
}

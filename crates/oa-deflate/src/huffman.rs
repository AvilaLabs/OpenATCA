//! Length-limited Huffman code construction and canonical code assignment. Core (pure).
//!
//! Lengths come from a plain Huffman tree (two-queue merge over sorted leaves), then any length
//! above the limit is clamped and the Kraft sum repaired by lengthening the cheapest codes, the
//! method used by miniz. The result is always a complete prefix code of at most `max_bits` bits;
//! it is within a fraction of a percent of the package-merge optimum on real data and exactly
//! optimal whenever the plain Huffman tree already fits the limit.

/// Most symbols any DEFLATE alphabet has (literal/length: 288 including the two unused).
pub const MAX_SYMBOLS: usize = 288;

/// Set `lens[s]` to the code length of symbol `s` for frequencies `freq` (`lens` and `freq` have
/// equal length, at most [`MAX_SYMBOLS`]). Unused symbols get length 0. Fewer than two used
/// symbols are padded with dummy symbols so the code stays complete (every decoder accepts a
/// complete code; a lone 1-bit code is legal but not universally accepted).
// E3 / H2: every index is below `n <= MAX_SYMBOLS` (the sorted-leaf and node arrays are sized to
// it) or is a symbol number below `freq.len()`; `bl_count` is indexed by lengths in 1..=max_bits.
#[allow(clippy::indexing_slicing)]
pub fn build_lengths(freq: &[u32], max_bits: usize, lens: &mut [u8]) {
    lens.fill(0);
    let mut syms = [(0u32, 0u16); MAX_SYMBOLS];
    let mut n = 0usize;
    for (s, &f) in freq.iter().enumerate().take(MAX_SYMBOLS) {
        if f > 0 {
            syms[n] = (f, s as u16);
            n += 1;
        }
    }
    if n < 2 {
        let used = if n == 1 { usize::from(syms[0].1) } else { 0 };
        let other = if used == 0 { 1 } else { 0 };
        if let Some(l) = lens.get_mut(used) {
            *l = 1;
        }
        if let Some(l) = lens.get_mut(other) {
            *l = 1;
        }
        return;
    }
    syms[..n].sort_unstable();

    // Two-queue Huffman: sorted leaves and the (already sorted) internal nodes.
    let mut weight = [0u32; MAX_SYMBOLS];
    let mut leaf_parent = [0u16; MAX_SYMBOLS];
    let mut node_parent = [0u16; MAX_SYMBOLS];
    let (mut i, mut j) = (0usize, 0usize);
    for m in 0..n - 1 {
        let mut sum = 0u32;
        for _ in 0..2 {
            let take_leaf = i < n && (j >= m || syms[i].0 <= weight[j]);
            if take_leaf {
                sum += syms[i].0;
                leaf_parent[i] = m as u16;
                i += 1;
            } else {
                sum += weight[j];
                node_parent[j] = m as u16;
                j += 1;
            }
        }
        weight[m] = sum;
    }
    // Depths: the root is node n-2 and parents always have a higher index than children.
    let mut depth = [0u16; MAX_SYMBOLS];
    for m in (0..n - 2).rev() {
        depth[m] = depth[usize::from(node_parent[m])] + 1;
    }
    let mut bl_count = [0u32; 64];
    for &p in leaf_parent.iter().take(n) {
        let d = (usize::from(depth[usize::from(p)]) + 1).min(max_bits);
        bl_count[d] += 1;
    }
    // Repair the Kraft sum after clamping: each step removes one code at max_bits (-1) and
    // splits one shorter code into two one longer (net 0), so the sum falls by exactly one unit.
    let full = 1u64 << max_bits;
    let mut total: u64 = (1..=max_bits)
        .map(|l| u64::from(bl_count[l]) << (max_bits - l))
        .sum();
    while total > full {
        if bl_count[max_bits] == 0 {
            break;
        }
        bl_count[max_bits] -= 1;
        for l in (1..max_bits).rev() {
            if bl_count[l] > 0 {
                bl_count[l] -= 1;
                bl_count[l + 1] += 2;
                break;
            }
        }
        total -= 1;
    }
    // Longest codes to the rarest symbols.
    let mut idx = 0usize;
    for l in (1..=max_bits).rev() {
        for _ in 0..bl_count[l] {
            if let Some(&(_, s)) = syms.get(idx) {
                lens[usize::from(s)] = l as u8;
            }
            idx += 1;
        }
    }
}

/// Canonical codes for `lens`, bit-reversed so they can be written least-significant bit first.
// E3 / H2: `bl_count`/`next` are indexed by code lengths of at most 15 (build_lengths limits
// them); `codes` has one entry per length.
#[allow(clippy::indexing_slicing)]
pub fn assign_codes(lens: &[u8], codes: &mut [u16]) {
    let mut bl_count = [0u16; 16];
    for &l in lens {
        bl_count[usize::from(l & 15)] += 1;
    }
    bl_count[0] = 0;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..16 {
        code = code.wrapping_add(bl_count[bits - 1]).wrapping_shl(1);
        next[bits] = code;
    }
    for (c, &l) in codes.iter_mut().zip(lens) {
        if l == 0 {
            *c = 0;
            continue;
        }
        let l = usize::from(l & 15);
        let v = next[l];
        next[l] = v.wrapping_add(1);
        *c = v.reverse_bits() >> (16 - l);
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn kraft(lens: &[u8], max: usize) -> u64 {
        lens.iter()
            .filter(|&&l| l > 0)
            .map(|&l| 1u64 << (max - usize::from(l)))
            .sum()
    }

    // REQ: OA-DEF-006
    #[test]
    fn lengths_are_complete_and_limited_for_pathological_frequencies() {
        // Fibonacci weights force the plain Huffman tree to depth n-1 (here 40 > 15).
        let mut freq = [0u32; 40];
        let (mut a, mut b) = (1u32, 1u32);
        for f in freq.iter_mut() {
            *f = a;
            let c = a.saturating_add(b).min(1 << 30);
            a = b;
            b = c;
        }
        let mut lens = [0u8; 40];
        build_lengths(&freq, 15, &mut lens);
        assert!(lens.iter().all(|&l| (1..=15).contains(&l)));
        assert_eq!(kraft(&lens, 15), 1 << 15);
        // A rarer symbol never has a shorter code than a more frequent one.
        for w in lens.windows(2) {
            assert!(w[0] >= w[1]);
        }
    }

    // REQ: OA-DEF-006
    #[test]
    fn lengths_complete_for_random_frequency_sets() {
        let mut x = 0x1234_5678_9abc_def0u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for round in 0..300 {
            let n = 2 + (next() % 285) as usize;
            let skew = 1 + (round % 20) as u32;
            let freq: Vec<u32> = (0..n)
                .map(|_| {
                    let r = next();
                    if r & 3 == 0 {
                        0
                    } else {
                        1u32 << ((r >> 8) as u32 % skew)
                    }
                })
                .collect();
            let used = freq.iter().filter(|&&f| f > 0).count();
            for max in [7usize, 15] {
                if used > 1 << max {
                    continue;
                }
                let mut lens = vec![0u8; n];
                build_lengths(&freq, max, &mut lens);
                if used >= 2 {
                    assert_eq!(kraft(&lens, max), 1 << max, "round {round} max {max}");
                    for (l, f) in lens.iter().zip(&freq) {
                        assert_eq!(*l > 0, *f > 0);
                        assert!(usize::from(*l) <= max);
                    }
                }
            }
        }
    }

    // REQ: OA-DEF-006
    #[test]
    fn zero_and_one_symbol_alphabets_stay_complete() {
        let mut lens = [9u8; 4];
        build_lengths(&[0, 0, 0, 0], 15, &mut lens);
        assert_eq!(lens, [1, 1, 0, 0]);
        build_lengths(&[0, 0, 5, 0], 15, &mut lens);
        assert_eq!(lens, [1, 0, 1, 0]);
        build_lengths(&[7, 0, 0, 0], 15, &mut lens);
        assert_eq!(lens, [1, 1, 0, 0]);
    }
}

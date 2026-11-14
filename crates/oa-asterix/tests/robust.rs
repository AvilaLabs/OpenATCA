//! Robustness: no input panics the decoder. Truncation at every byte of every corpus block, with
//! and without LEN fixed up to match, and 4000 random mutations. Whatever the decoder accepts
//! must encode back to exactly the bytes it was given.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
use common::*;
use oa_asterix::*;

/// xorshift64*: a fixed seed makes the test reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Decodes and, if accepted, checks that the block re-encodes to the same bytes.
fn probe(bytes: &[u8]) -> bool {
    match decode_block(bytes) {
        Ok((b, n)) => {
            assert!(n <= bytes.len());
            let again = encode_block(&b).unwrap_or_else(|e| {
                panic!("accepted but cannot be encoded: {e}; input {bytes:02X?}")
            });
            assert_eq!(
                &again[..],
                &bytes[..n],
                "round trip of accepted {bytes:02X?}"
            );
            true
        }
        Err(e) => {
            // The error is always usable: it renders, and its offset is inside the input or
            // one past it.
            let _ = e.to_string();
            assert!(e.offset <= bytes.len(), "{e}: {bytes:02X?}");
            false
        }
    }
}

fn with_len(mut v: Vec<u8>) -> Vec<u8> {
    let len = v.len() as u16;
    if v.len() >= 3 {
        v[1..3].copy_from_slice(&len.to_be_bytes());
    }
    v
}

// REQ: OA-AST-015
#[test]
fn truncation_at_every_byte_never_panics() {
    let mut cut_ok = 0;
    for block in corpus() {
        assert!(probe(&block));
        for cut in 0..block.len() {
            // The input is cut: LEN now claims more than there is.
            assert!(!probe(&block[..cut]), "cut {cut} of {}", block.len());
            // The input is cut and LEN made to match: the walker meets the truncation inside
            // an item or the FSPEC.
            if probe(&with_len(block[..cut].to_vec())) {
                cut_ok += 1;
            }
        }
    }
    // A cut that lands on a record boundary of a two-record block is a valid block.
    assert!(cut_ok > 0);
}

// REQ: OA-AST-016
#[test]
fn four_thousand_random_mutations_never_panic_and_accepted_ones_round_trip() {
    let corpus = corpus();
    let mut rng = Rng(0x00A5_7E81_4C0D_EC01);
    let (mut accepted, mut refused) = (0, 0);
    // OA_ASTERIX_FUZZ_N raises the count for a longer local run.
    let n: usize = std::env::var("OA_ASTERIX_FUZZ_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4000);
    for i in 0..n {
        let mut b = corpus[rng.below(corpus.len())].clone();
        for _ in 0..1 + rng.below(3) {
            match rng.below(5) {
                0 => {
                    let at = rng.below(b.len());
                    b[at] = rng.next() as u8;
                }
                1 => {
                    let at = rng.below(b.len());
                    b[at] ^= 1 << rng.below(8);
                }
                2 => {
                    let at = 1 + rng.below(b.len() - 1);
                    b.insert(at, rng.next() as u8);
                }
                3 => {
                    if b.len() > 4 {
                        let at = 3 + rng.below(b.len() - 3);
                        b.remove(at);
                    }
                }
                _ => {
                    // Overwrite a stretch with a copy of another stretch.
                    let n = 1 + rng.below(4);
                    if b.len() > 2 * n + 3 {
                        let from = 3 + rng.below(b.len() - n - 3);
                        let to = 3 + rng.below(b.len() - n - 3);
                        let chunk: Vec<u8> = b[from..from + n].to_vec();
                        b[to..to + n].copy_from_slice(&chunk);
                    }
                }
            }
        }
        // Half of the time make LEN agree with the mutated length so the mutation reaches the
        // item walker instead of stopping at the framing check.
        if i % 2 == 0 {
            b = with_len(b);
        }
        if probe(&b) {
            accepted += 1;
        } else {
            refused += 1;
        }
    }
    // Both outcomes occur: the test exercises acceptance and refusal.
    assert!(
        accepted > 100 && refused > 100,
        "{accepted} accepted, {refused} refused"
    );
}

// REQ: OA-AST-016
#[test]
fn arbitrary_bytes_never_panic() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for _ in 0..4000 {
        let n = rng.below(80);
        let mut b: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        if n >= 3 {
            // Aim at the supported categories, with a consistent LEN.
            b[0] = [21, 48, 62][rng.below(3)];
            b = with_len(b);
        }
        probe(&b);
        let _ = decode_stream(&b);
    }
}

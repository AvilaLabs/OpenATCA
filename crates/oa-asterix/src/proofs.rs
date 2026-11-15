//! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
#![allow(clippy::indexing_slicing)]

use crate::common::{decode_ident, encode_ident, ms_to_time128, rd, sext, take, time128_to_ms};
use crate::wire::parse_block;

// REQ: OA-AST-100
#[kani::proof]
#[kani::unwind(10)]
fn primitives_rd_take_sext() {
    let buf: [u8; 6] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= 6);
    let input = &buf[..len];
    let start: usize = kani::any();
    let n: usize = kani::any();
    kani::assume(n <= 4);

    let mut pos = start;
    let before = pos;
    match rd(input, &mut pos, n) {
        Ok(v) => {
            assert!(before <= len && n <= len - before);
            assert_eq!(pos, before + n);
            assert!(n == 4 || u64::from(v) < 1u64 << (8 * n));
        }
        Err(_) => assert_eq!(pos, before),
    }
    let mut pos = start;
    let n: usize = kani::any();
    match take(input, &mut pos, n) {
        Ok(s) => {
            assert_eq!(s.len(), n);
            assert_eq!(pos, start + n);
            assert!(pos <= len);
        }
        Err(_) => assert_eq!(pos, start),
    }

    let raw: u32 = kani::any();
    let bits: u32 = kani::any();
    kani::assume(bits >= 1 && bits <= 32);
    let v = sext(raw, bits);
    if bits < 32 {
        assert!(i64::from(v) >= -(1i64 << (bits - 1)) && i64::from(v) < 1i64 << (bits - 1));
    }
    // The low bits are kept.
    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };
    assert_eq!(v as u32 & mask, raw & mask);
}

// REQ: OA-AST-104
#[kani::proof]
fn time_roundtrip_24_bits() {
    let raw: u32 = kani::any();
    kani::assume(raw < 1 << 24);
    assert_eq!(ms_to_time128(time128_to_ms(raw)), raw);
    // And no input panics.
    let any: u32 = kani::any();
    let _ = (time128_to_ms(any), ms_to_time128(any));
}

// REQ: OA-AST-104
#[kani::proof]
#[kani::unwind(10)]
fn ident_roundtrip_any_six_octets() {
    let b: [u8; 6] = kani::any();
    if let Ok(id) = decode_ident(&b) {
        let mut out = Vec::new();
        assert!(encode_ident(&id, &mut out).is_ok());
        assert_eq!(out.as_slice(), &b[..]);
    }
}

/// Block framing over every byte string of up to 3 octets: a short header, a LEN below three or
/// past the input, a block with LEN three (no record) and a category the crate does not carry.
/// A block with a record is six octets at least.
// REQ: OA-AST-103
#[kani::proof]
#[kani::unwind(6)]
fn parse_block_any_bytes_up_to_3() {
    let buf: [u8; 3] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= 3);
    let input = &buf[..len];
    if let Ok(raw) = parse_block(input, 0) {
        assert!(raw.len > 0 && raw.len <= len);
    }
}

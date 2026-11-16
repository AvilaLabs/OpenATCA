#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::bits::HeaderBits;
use crate::dwt::{inverse, lift_line, Rect};
use crate::mq::{self, Cx};
use crate::tagtree::{decode as tag_decode, leaf_value, new_tree};
use crate::testenc::{analyze_line, encode, encode_block, BitW, MqEnc, Opts, TagEnc};
use crate::{decode, J2kError};

/// Small deterministic generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 24
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn image(rng: &mut Rng, n: usize, bits: u8, smooth: bool) -> Vec<i32> {
    let max = (1i64 << bits) - 1;
    let mut v = Vec::with_capacity(n);
    let mut cur = max / 2;
    for _ in 0..n {
        if smooth {
            cur = (cur + rng.below(9) as i64 - 4).clamp(0, max);
            v.push(cur as i32);
        } else {
            v.push(rng.below(max as u64 + 1) as i32);
        }
    }
    v
}

// REQ: OA-J2K-001
#[test]
fn mq_decoder_reads_the_jbig2_annex_h_test_sequence() {
    // The MQ coder of JPEG 2000 is the one of JBIG2 (T.88 H.2): one context, initial state 0.
    // The 256-bit input sequence and its encoding are that annex's test vector.
    let plain: [u8; 32] = [
        0x00, 0x02, 0x00, 0x51, 0x00, 0x00, 0x00, 0xC0, 0x03, 0x52, 0x87, 0x2A, 0xAA, 0xAA, 0xAA,
        0xAA, 0x82, 0xC0, 0x20, 0x00, 0xFC, 0xD7, 0x9E, 0xF6, 0xBF, 0x7F, 0xED, 0x90, 0x4F, 0x46,
        0xA3, 0xBF,
    ];
    let coded: [u8; 30] = [
        0x84, 0xC7, 0x3B, 0xFC, 0xE1, 0xA1, 0x43, 0x04, 0x02, 0x20, 0x00, 0x00, 0x41, 0x0D, 0xBB,
        0x86, 0xF4, 0x31, 0x7F, 0xFF, 0x88, 0xFF, 0x37, 0x47, 0x1A, 0xDB, 0x6A, 0xDF, 0xFF, 0xAC,
    ];
    let mut mq = mq::init(&coded);
    let mut cx = Cx { index: 0, mps: 0 };
    for (k, &want) in plain.iter().enumerate() {
        let mut got = 0u8;
        for _ in 0..8 {
            got = (got << 1) | mq::decode(&mut mq, &mut cx);
        }
        assert_eq!(got, want, "byte {k}");
    }
}

// REQ: OA-J2K-001
#[test]
fn mq_encoder_and_decoder_agree_over_many_contexts() {
    let mut rng = Rng(7);
    for round in 0..20 {
        let n = 1 + rng.below(3000) as usize;
        let skew = 1 + rng.below(8);
        let syms: Vec<(usize, u8)> = (0..n)
            .map(|_| {
                let cx = rng.below(19) as usize;
                (cx, u8::from(rng.below(skew + 1) == 0))
            })
            .collect();
        let mut e = MqEnc::new();
        for &(cx, d) in &syms {
            e.encode(cx, d);
        }
        let bytes = e.flush();
        let mut mq = mq::init(&bytes);
        let mut cxs = [Cx { index: 0, mps: 0 }; 19];
        cxs[0].index = 4;
        cxs[17].index = 3;
        cxs[18].index = 46;
        for (k, &(cx, d)) in syms.iter().enumerate() {
            assert_eq!(
                mq::decode(&mut mq, &mut cxs[cx]),
                d,
                "round {round} symbol {k}"
            );
        }
    }
}

// REQ: OA-J2K-002
#[test]
fn tag_tree_decodes_a_hand_written_bit_string() {
    // One leaf of value 3: bits 0 0 0 1 answer thresholds 1, 2, 3 (below?: no) and 4 (yes).
    let data = [0b0001_0000];
    let mut hb = HeaderBits::new(&data);
    let mut t = new_tree(1, 1);
    assert!(!tag_decode(&mut t, 0, 3, &mut hb).unwrap());
    assert!(tag_decode(&mut t, 0, 4, &mut hb).unwrap());
    assert_eq!(leaf_value(&t, 0), Some(3));
    // Two leaves (root over both), values 1 and 0. Encoded by hand: threshold 1 for leaf 0:
    // root min 0 -> bit 1 (root known = 0); leaf 0: low 0, value unknown: bit 0 (low 1).
    // Leaf 0 is not below 1. Then threshold 2 for leaf 0: bit 1 (value 1). Then leaf 1 at
    // threshold 1: root already known, leaf: bit 1 (value 0). Bits 1 0 1 1.
    let data = [0b1011_0000];
    let mut hb = HeaderBits::new(&data);
    let mut t = new_tree(2, 1);
    assert!(!tag_decode(&mut t, 0, 1, &mut hb).unwrap());
    assert!(tag_decode(&mut t, 0, 2, &mut hb).unwrap());
    assert_eq!(leaf_value(&t, 0), Some(1));
    assert!(tag_decode(&mut t, 1, 1, &mut hb).unwrap());
    assert_eq!(leaf_value(&t, 1), Some(0));
}

// REQ: OA-J2K-002
#[test]
fn tag_tree_round_trips_random_trees_and_thresholds() {
    let mut rng = Rng(11);
    for _ in 0..200 {
        let (w, h) = (1 + rng.below(9) as usize, 1 + rng.below(9) as usize);
        let vals: Vec<u32> = (0..w * h).map(|_| rng.below(6) as u32).collect();
        let mut enc = TagEnc::new(w, h, &vals);
        let mut bw = BitW::new();
        // Query leaves in random order, each at rising thresholds like inclusion does.
        let mut queries = Vec::new();
        for _ in 0..40 {
            queries.push((rng.below((w * h) as u64) as usize, 1 + rng.below(7) as u32));
        }
        for &(leaf, thr) in &queries {
            enc.encode(leaf, thr, &mut bw);
        }
        let bytes = bw.finish();
        let mut hb = HeaderBits::new(&bytes);
        let mut t = new_tree(w as u32, h as u32);
        for &(leaf, thr) in &queries {
            let below = tag_decode(&mut t, leaf as u32, thr, &mut hb).unwrap();
            assert_eq!(below, vals[leaf] < thr);
            if below {
                assert_eq!(leaf_value(&t, leaf as u32), Some(vals[leaf]));
            }
        }
    }
}

// REQ: OA-J2K-003
#[test]
fn header_bits_stuff_after_ff_and_end_on_a_byte() {
    // 0xFF then a byte of which only 7 bits count; its top bit must be 0.
    let data = [0xFF, 0x40, 0x80];
    let mut hb = HeaderBits::new(&data);
    assert_eq!(hb.bits(8).unwrap(), 0xFF);
    assert_eq!(hb.bits(3).unwrap(), 0b100);
    assert_eq!(hb.finish().unwrap(), 2);
    // A stuffed bit that is 1 is refused.
    let mut hb = HeaderBits::new(&[0xFF, 0xC0]);
    hb.bits(8).unwrap();
    assert!(matches!(hb.bit(), Err(J2kError::BadPacket(_))));
    // A header that ends on 0xFF takes the stuffed byte after it.
    let mut hb = HeaderBits::new(&[0xFF, 0x00, 0x55]);
    hb.bits(8).unwrap();
    assert_eq!(hb.finish().unwrap(), 2);
    // Writer and reader agree across 0xFF.
    let mut w = BitW::new();
    for v in [0xFFu32, 0xFF, 0x1, 0x7F] {
        w.bits(v, 8);
    }
    let bytes = w.finish();
    let mut r = HeaderBits::new(&bytes);
    for v in [0xFFu32, 0xFF, 0x1, 0x7F] {
        assert_eq!(r.bits(8).unwrap(), v);
    }
}

// REQ: OA-J2K-004
#[test]
fn inverse_5_3_on_known_vectors() {
    // A linear ramp has zero high-pass; the low-pass samples are the even-indexed ones.
    let mut s = [1, 0, 3, 0, 5];
    lift_line(&mut s, 0).unwrap();
    assert_eq!(s, [1, 2, 3, 4, 5]);
    // [5, 1, 5, 1] analyses to [3, -4, 3, -4]: the update step floors -6/4 to -2.
    let mut s = [3, -4, 3, -4];
    lift_line(&mut s, 0).unwrap();
    assert_eq!(s, [5, 1, 5, 1]);
    // Odd origin: the first sample is high-pass.
    let mut s = [0, 3, 0, 5];
    lift_line(&mut s, 1).unwrap();
    let mut t = [0, 3, 0, 5];
    analyze_line(&mut t, 1);
    let mut back = t;
    lift_line(&mut back, 1).unwrap();
    assert_eq!(back, [0, 3, 0, 5]);
    // A single sample: even coordinate unchanged, odd coordinate halved.
    let mut s = [7];
    lift_line(&mut s, 0).unwrap();
    assert_eq!(s, [7]);
    let mut s = [14];
    lift_line(&mut s, 1).unwrap();
    assert_eq!(s, [7]);
}

// REQ: OA-J2K-004
#[test]
fn inverse_5_3_undoes_the_forward_transform_for_every_length_and_parity() {
    let mut rng = Rng(3);
    for n in 1..=40 {
        for p in 0..2 {
            let x: Vec<i32> = (0..n).map(|_| rng.below(4096) as i32 - 2048).collect();
            let mut y = x.clone();
            analyze_line(&mut y, p);
            lift_line(&mut y, p).unwrap();
            assert_eq!(y, x, "n {n} parity {p}");
        }
    }
}

// REQ: OA-J2K-004
#[test]
fn two_dimensional_inverse_matches_the_forward_transform_with_odd_origins() {
    let mut rng = Rng(5);
    for &(w, h, x0, y0, levels) in &[
        (9usize, 7usize, 0usize, 0usize, 2usize),
        (9, 7, 1, 1, 3),
        (8, 5, 3, 2, 3),
        (1, 6, 1, 0, 2),
        (6, 1, 0, 1, 2),
        (17, 33, 5, 7, 4),
    ] {
        let x: Vec<i32> = (0..w * h).map(|_| rng.below(512) as i32 - 256).collect();
        let mut a = x.clone();
        let sizes = crate::testenc::forward(&mut a, w, h, x0, y0, levels);
        // Resolution rectangles from the origin.
        let mut res = Vec::new();
        for r in 0..=levels {
            let d = 1usize << (levels - r);
            res.push(Rect {
                x0: x0.div_ceil(d) as u32,
                y0: y0.div_ceil(d) as u32,
                x1: (x0 + w).div_ceil(d) as u32,
                y1: (y0 + h).div_ceil(d) as u32,
            });
            let (rw, rh) = sizes[r];
            assert_eq!(
                (rw as u32, rh as u32),
                (res[r].x1 - res[r].x0, res[r].y1 - res[r].y0)
            );
        }
        inverse(&mut a, w, &res).unwrap();
        assert_eq!(a, x, "{w}x{h} at ({x0},{y0}) levels {levels}");
    }
}

fn spec(w: usize, h: usize, orient: u8, passes: u32, start_bp: u32) -> crate::t1::BlockSpec {
    crate::t1::BlockSpec {
        w,
        h,
        orient,
        passes,
        start_bp,
    }
}

// REQ: OA-J2K-005
#[test]
fn code_blocks_round_trip_through_all_three_passes() {
    let mut rng = Rng(9);
    for orient in 0..4u8 {
        for &(w, h) in &[
            (1usize, 1usize),
            (1, 5),
            (7, 1),
            (4, 4),
            (5, 9),
            (13, 6),
            (32, 32),
            (64, 64),
            (3, 64),
        ] {
            for density in [1u64, 3, 40] {
                let coefs: Vec<i32> = (0..w * h)
                    .map(|_| {
                        if rng.below(density) != 0 {
                            0
                        } else {
                            let bitsn = 1 + rng.below(12);
                            let m = rng.below(1 << bitsn) as i32;
                            if rng.below(2) == 0 {
                                m
                            } else {
                                -m
                            }
                        }
                    })
                    .collect();
                let Some((zero, passes, bytes)) = encode_block(&coefs, w, h, orient, 14) else {
                    assert!(coefs.iter().all(|&c| c == 0));
                    continue;
                };
                let mut out = vec![0i32; w * h];
                let mut s = crate::t1::Scratch::default();
                crate::t1::decode_block(
                    &mut s,
                    spec(w, h, orient, passes, 13 - zero),
                    &bytes,
                    &mut out,
                )
                .unwrap();
                assert_eq!(out, coefs, "orient {orient} {w}x{h} density {density}");
            }
        }
    }
}

// REQ: OA-J2K-005
#[test]
fn tier_1_refuses_more_passes_than_planes_and_decodes_zero_passes_as_zero() {
    let mut s = crate::t1::Scratch::default();
    let mut out = vec![9i32; 4];
    crate::t1::decode_block(&mut s, spec(2, 2, 0, 0, 5), &[], &mut out).unwrap();
    assert_eq!(out, [0; 4]);
    // Plane 2 allows at most 3 * 3 - 2 = 7 passes.
    assert!(matches!(
        crate::t1::decode_block(&mut s, spec(2, 2, 0, 8, 2), &[0; 8], &mut out),
        Err(J2kError::BadBlock(_))
    ));
    crate::t1::decode_block(&mut s, spec(2, 2, 0, 7, 2), &[0; 8], &mut out).unwrap();
}

fn round_trip(o: &Opts, rng: &mut Rng, smooth: bool) {
    let n = o.w * o.h;
    let samples = if o.signed {
        let max = 1i64 << (o.bits - 1);
        (0..n)
            .map(|_| rng.below(2 * max as u64) as i32 - max as i32)
            .collect()
    } else {
        image(rng, n, o.bits, smooth)
    };
    let cs = encode(&samples, o);
    let img = decode(&cs).unwrap_or_else(|e| panic!("{o:?}: {e}"));
    assert_eq!((img.width as usize, img.height as usize), (o.w, o.h));
    assert_eq!((img.bits, img.signed), (o.bits, o.signed));
    assert_eq!(img.samples, samples, "{o:?}");
}

// REQ: OA-J2K-006
#[test]
fn whole_codestreams_round_trip_exactly() {
    let mut rng = Rng(21);
    for &(w, h, bits, levels) in &[
        (1usize, 1usize, 8u8, 0usize),
        (1, 1, 8, 3),
        (2, 3, 8, 1),
        (5, 4, 8, 2),
        (17, 9, 12, 3),
        (64, 64, 15, 5),
        (451, 33, 10, 5),
        (100, 130, 16, 5),
        (3, 70, 1, 4),
        (70, 3, 9, 6),
    ] {
        for smooth in [false, true] {
            round_trip(&Opts::new(w, h, bits, levels), &mut rng, smooth);
        }
    }
}

// REQ: OA-J2K-006
#[test]
fn round_trip_with_small_code_blocks_signed_samples_offsets_and_layers() {
    let mut rng = Rng(23);
    for &(xcb, ycb) in &[(2, 2), (3, 5), (6, 4), (10, 2), (5, 5)] {
        let mut o = Opts::new(37, 29, 11, 3);
        o.xcb = xcb;
        o.ycb = ycb;
        round_trip(&o, &mut rng, true);
    }
    for &(x0, y0) in &[(1usize, 0usize), (0, 1), (1, 1), (5, 7), (33, 18)] {
        let mut o = Opts::new(23, 31, 10, 3);
        o.x0 = x0;
        o.y0 = y0;
        round_trip(&o, &mut rng, true);
    }
    let mut o = Opts::new(40, 41, 8, 2);
    o.signed = true;
    round_trip(&o, &mut rng, false);
    for layers in [2usize, 3, 5, 11] {
        let mut o = Opts::new(70, 50, 12, 4);
        o.layers = layers;
        o.xcb = 4;
        o.ycb = 4;
        round_trip(&o, &mut rng, true);
    }
}

// REQ: OA-J2K-006
#[test]
fn constant_and_extreme_images_round_trip() {
    let mut rng = Rng(25);
    for v in [0, 1, 127, 128, 255] {
        let cs = encode(&vec![v; 30 * 20], &Opts::new(30, 20, 8, 3));
        assert_eq!(decode(&cs).unwrap().samples, vec![v; 600]);
    }
    let extremes: Vec<i32> = (0..400)
        .map(|k| if (k / 7 + k) % 2 == 0 { 0 } else { 65535 })
        .collect();
    let cs = encode(&extremes, &Opts::new(20, 20, 16, 4));
    assert_eq!(decode(&cs).unwrap().samples, extremes);
    round_trip(&Opts::new(1, 300, 14, 5), &mut rng, false);
}

// REQ: OA-J2K-007
#[test]
fn frozen_codestreams_decode_to_their_known_samples() {
    // Built once with the test encoder and frozen as bytes, so a change to the decoder cannot
    // be hidden by the same change in the encoder: 4 x 3, 8 bits, 1 level, then 3 x 2.
    const A: [u8; 109] = [
        0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00,
        0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x01,
        0xFF, 0x52, 0x00, 0x0C, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x04, 0x04, 0x00, 0x01, 0xFF,
        0x5C, 0x00, 0x07, 0x40, 0x40, 0x48, 0x48, 0x50, 0xFF, 0x64, 0x00, 0x08, 0x00, 0x01, 0x74,
        0x65, 0x73, 0x74, 0xFF, 0x90, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1D, 0x00, 0x01,
        0xFF, 0x93, 0xCF, 0xB4, 0x14, 0x06, 0x48, 0x82, 0x44, 0x99, 0xC0, 0xF9, 0x01, 0x80, 0x0C,
        0xFF, 0x5F, 0xFF, 0xD9,
    ];
    const B: [u8; 115] = [
        0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00,
        0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x01,
        0xFF, 0x52, 0x00, 0x0C, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x04, 0x04, 0x00, 0x01, 0xFF,
        0x5C, 0x00, 0x07, 0x40, 0x40, 0x48, 0x48, 0x50, 0xFF, 0x64, 0x00, 0x08, 0x00, 0x01, 0x74,
        0x65, 0x73, 0x74, 0xFF, 0x90, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x23, 0x00, 0x01,
        0xFF, 0x93, 0xC3, 0xE7, 0x04, 0x07, 0x1A, 0xC1, 0xF3, 0x82, 0x8F, 0xB4, 0x0E, 0x7E, 0x60,
        0x40, 0x03, 0x1F, 0x0B, 0x11, 0x95, 0x01, 0xF7, 0xFF, 0xD9,
    ];
    let a = decode(&A).unwrap();
    assert_eq!((a.width, a.height, a.bits, a.signed), (4, 3, 8, false));
    assert_eq!(
        a.samples,
        [10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]
    );
    assert_eq!(decode(&B).unwrap().samples, [200, 3, 150, 0, 255, 77]);
    // The test encoder still makes the same bytes.
    assert_eq!(encode(&a.samples, &Opts::new(4, 3, 8, 1)), A);
}

fn find(cs: &[u8], marker: u16) -> usize {
    let m = marker.to_be_bytes();
    (2..cs.len() - 1)
        .find(|&i| cs[i] == m[0] && cs[i + 1] == m[1])
        .unwrap()
}

fn base() -> Vec<u8> {
    encode(&[5; 12], &Opts::new(4, 3, 8, 1))
}

// REQ: OA-J2K-008
#[test]
fn markers_outside_the_profile_are_refused_by_name() {
    assert_eq!(decode(&[]), Err(J2kError::NotCodestream));
    assert_eq!(decode(&[0xFF, 0x4E, 0, 0]), Err(J2kError::NotCodestream));
    // Add each unsupported marker segment to the main header.
    for m in [0xFF5Fu16, 0xFF60, 0xFF61, 0xFF5E, 0xFF70] {
        let mut cs = base();
        let at = find(&cs, 0xFF64);
        let seg = [(m >> 8) as u8, m as u8, 0, 4, 0, 0];
        cs.splice(at..at, seg);
        assert_eq!(decode(&cs), Err(J2kError::UnsupportedMarker(m)), "{m:#x}");
    }
    // Informative markers are skipped.
    for m in [0xFF55u16, 0xFF57, 0xFF63] {
        let mut cs = base();
        let at = find(&cs, 0xFF64);
        cs.splice(at..at, [(m >> 8) as u8, m as u8, 0, 4, 0, 0]);
        assert!(decode(&cs).is_ok(), "{m:#x}");
    }
}

// REQ: OA-J2K-008
#[test]
fn coding_parameters_outside_the_profile_are_refused() {
    let cod = find(&base(), 0xFF52);
    // COD: marker(2) length(2) Scod prog layers(2) mct levels cbw cbh style transform.
    let cases: Vec<(usize, u8, J2kError)> = vec![
        (cod + 5, 1, J2kError::UnsupportedProgression(1)),
        (
            cod + 8,
            1,
            J2kError::Unsupported("multiple component transform"),
        ),
        (cod + 12, 0x01, J2kError::UnsupportedBlockStyle(1)),
        (cod + 12, 0x3F, J2kError::UnsupportedBlockStyle(0x3F)),
        (
            cod + 13,
            0,
            J2kError::Unsupported("irreversible 9-7 wavelet"),
        ),
        (cod + 4, 1, J2kError::Unsupported("precinct partitioning")),
        (cod + 4, 2, J2kError::Unsupported("SOP or EPH markers")),
        (cod + 4, 4, J2kError::Unsupported("SOP or EPH markers")),
    ];
    for (at, v, want) in cases {
        let mut cs = base();
        cs[at] = v;
        assert_eq!(decode(&cs), Err(want), "byte {at} = {v}");
    }
    let mut cs = base();
    cs[cod + 9] = 20; // more levels than allowed
    assert!(matches!(decode(&cs), Err(J2kError::BadMarker(_))));
    let mut cs = base();
    cs[cod + 10] = 9; // code-block width exponent
    assert!(matches!(decode(&cs), Err(J2kError::BadMarker(_))));
    // Quantization style 2 (scalar expounded) is refused.
    let mut cs = base();
    let q = find(&cs, 0xFF5C);
    cs[q + 4] = (cs[q + 4] & 0xE0) | 2;
    assert_eq!(decode(&cs), Err(J2kError::UnsupportedQuantization(2)));
}

// REQ: OA-J2K-008
#[test]
fn image_geometry_outside_the_profile_is_refused() {
    let siz = find(&base(), 0xFF51);
    // SIZ body: Rsiz(2) Xsiz Ysiz XO YO XT YT XTO YTO (4 each) Csiz(2) Ssiz XR YR.
    let at = |off: usize| siz + 4 + off;
    let mut cs = base();
    cs[at(37)] = 2; // XRsiz
    assert_eq!(
        decode(&cs),
        Err(J2kError::Unsupported("component subsampling"))
    );
    let mut cs = base();
    cs[at(35)] = 3; // Csiz = 3
    assert_eq!(
        decode(&cs),
        Err(J2kError::Unsupported("more than one component"))
    );
    let mut cs = base();
    cs[at(21)] = 2; // XTsiz = 2 < 4: several tiles
    assert_eq!(
        decode(&cs),
        Err(J2kError::Unsupported("more than one tile"))
    );
    let mut cs = base();
    cs[at(36)] = 20; // 21 bits
    assert_eq!(decode(&cs), Err(J2kError::BadBitDepth(21)));
    let mut cs = base();
    cs[at(1)] = 1; // Rsiz != 0
    assert!(matches!(decode(&cs), Err(J2kError::Unsupported(_))));
    // Bounds on the image size: one side too long, then both sides allowed (2^14 x 2^11 = 32 Mi
    // samples) but too many samples, then both sides too long.
    let mut cs = base();
    cs[at(2)] = 0x10;
    assert!(matches!(decode(&cs), Err(J2kError::ImageTooLarge { .. })));
    let mut cs = base();
    cs[at(2)..at(2) + 4].copy_from_slice(&16384u32.to_be_bytes());
    cs[at(6)..at(6) + 4].copy_from_slice(&2048u32.to_be_bytes());
    cs[at(18)..at(18) + 4].copy_from_slice(&16384u32.to_be_bytes());
    cs[at(22)..at(22) + 4].copy_from_slice(&2048u32.to_be_bytes());
    assert!(matches!(decode(&cs), Err(J2kError::ImageTooLarge { .. })));
    let mut cs = base();
    cs[at(2)..at(2) + 4].copy_from_slice(&20000u32.to_be_bytes());
    cs[at(6)..at(6) + 4].copy_from_slice(&20000u32.to_be_bytes());
    cs[at(18)..at(18) + 4].copy_from_slice(&20000u32.to_be_bytes());
    cs[at(22)..at(22) + 4].copy_from_slice(&20000u32.to_be_bytes());
    assert!(matches!(decode(&cs), Err(J2kError::ImageTooLarge { .. })));
}

// REQ: OA-J2K-008
#[test]
fn tile_part_structure_outside_the_profile_is_refused() {
    let sot = find(&base(), 0xFF90);
    let mut cs = base();
    cs[sot + 5] = 1; // Isot = 1
    assert_eq!(
        decode(&cs),
        Err(J2kError::Unsupported("more than one tile"))
    );
    let mut cs = base();
    cs[sot + 10] = 1; // TPsot = 1
    assert_eq!(
        decode(&cs),
        Err(J2kError::Unsupported("more than one tile-part"))
    );
    let mut cs = base();
    cs[sot + 11] = 2; // TNsot = 2
    assert_eq!(
        decode(&cs),
        Err(J2kError::Unsupported("more than one tile-part"))
    );
    // A second tile-part after the first.
    let mut cs = base();
    let n = cs.len();
    cs.splice(
        n - 2..n - 2,
        [0xFF, 0x90, 0, 10, 0, 0, 0, 0, 0, 14, 1, 2, 0xFF, 0x93],
    );
    assert_eq!(
        decode(&cs),
        Err(J2kError::Unsupported("more than one tile-part"))
    );
    // No EOC, bytes after it, and a Psot that runs off the end.
    let mut cs = base();
    cs.truncate(cs.len() - 2);
    assert!(decode(&cs).is_err());
    let mut cs = base();
    cs.extend([1, 2, 3]);
    assert_eq!(decode(&cs), Err(J2kError::TrailingBytes(3)));
    let mut cs = base();
    cs[sot + 9] = 0xFF;
    assert!(matches!(decode(&cs), Err(J2kError::BadMarker(_))));
    // Psot = 0 means "to the end": the same stream decodes.
    let mut cs = base();
    cs[sot + 6..sot + 10].copy_from_slice(&[0, 0, 0, 0]);
    assert_eq!(decode(&cs).unwrap().samples, vec![5; 12]);
}

// REQ: OA-J2K-009
#[test]
fn packet_damage_is_refused_with_typed_errors() {
    let mut rng = Rng(31);
    let samples = image(&mut rng, 64 * 40, 12, true);
    let cs = encode(&samples, &Opts::new(64, 40, 12, 3));
    let sod = find(&cs, 0xFF93) + 2;
    // Data cut after the headers: a packet runs off the end.
    let mut short = cs[..cs.len() - 2 - 5].to_vec();
    short.extend([0xFF, 0xD9]);
    let psot = u32::from_be_bytes(short[find(&short, 0xFF90) + 6..][..4].try_into().unwrap());
    let _ = psot;
    assert!(decode(&short).is_err());
    // Extra bytes inside the tile after the last packet.
    let mut long = cs.clone();
    let n = long.len();
    long.splice(n - 2..n - 2, [0, 0]);
    let sot = find(&long, 0xFF90);
    let p = u32::from_be_bytes(long[sot + 6..sot + 10].try_into().unwrap()) + 2;
    long[sot + 6..sot + 10].copy_from_slice(&p.to_be_bytes());
    assert_eq!(
        decode(&long),
        Err(J2kError::BadPacket("data after the last packet"))
    );
    // Too many bit-planes for the band: raise the number of passes implied by zero planes.
    let _ = sod;
    // A guard-bit count of 0 and exponents of 0 leave no magnitude bit-planes.
    let mut z = cs.clone();
    let q = find(&z, 0xFF5C);
    for b in &mut z[q + 4..q + 4 + 10] {
        *b = 0;
    }
    assert!(matches!(decode(&z), Err(J2kError::BadMarker(_))));
}

// REQ: OA-J2K-010
#[test]
fn no_truncation_or_mutation_makes_the_decoder_panic() {
    let mut rng = Rng(41);
    let samples = image(&mut rng, 40 * 30, 10, true);
    let mut o = Opts::new(40, 30, 10, 3);
    o.xcb = 4;
    o.ycb = 4;
    o.layers = 3;
    let cs = encode(&samples, &o);
    for n in 0..cs.len() {
        let _ = decode(&cs[..n]);
    }
    for _ in 0..4000 {
        let mut m = cs.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(m.len() as u64) as usize;
            m[i] = rng.next() as u8;
        }
        let _ = decode(&m);
    }
    // Garbage after valid headers.
    let sod = find(&cs, 0xFF93) + 2;
    for _ in 0..500 {
        let mut m = cs[..sod].to_vec();
        let n = rng.below(300) as usize;
        m.extend((0..n).map(|_| rng.next() as u8));
        m.extend([0xFF, 0xD9]);
        let _ = decode(&m);
    }
}

// REQ: OA-J2K-011
#[test]
fn block_and_pass_limits_are_enforced() {
    // 4 x 4 code blocks over a 2048 x 1024 image: 131072 blocks of 16 samples fit under the
    // limit, but a 16 Mi image would not; check the arithmetic through the header instead.
    let cs = encode(&[0; 64 * 64], &Opts::new(64, 64, 8, 0));
    let siz = find(&cs, 0xFF51);
    let mut big = cs.clone();
    // Claim a 4096 x 4096 image with 4 x 4 blocks: 1 Mi blocks plus resolution bands go over.
    big[siz + 4 + 2..siz + 4 + 6].copy_from_slice(&4096u32.to_be_bytes());
    big[siz + 4 + 6..siz + 4 + 10].copy_from_slice(&4096u32.to_be_bytes());
    big[siz + 4 + 18..siz + 4 + 22].copy_from_slice(&4096u32.to_be_bytes());
    big[siz + 4 + 22..siz + 4 + 26].copy_from_slice(&4096u32.to_be_bytes());
    let cod = find(&big, 0xFF52);
    big[cod + 10] = 0;
    big[cod + 11] = 0;
    assert!(matches!(decode(&big), Err(J2kError::TooManyCodeBlocks(_))));
}

// REQ: OA-J2K-012
#[test]
fn coc_and_qcc_override_cod_and_qcd_in_either_order() {
    let mut rng = Rng(51);
    let samples = image(&mut rng, 30 * 20, 9, true);
    let cs = encode(&samples, &Opts::new(30, 20, 9, 2));
    let cod = find(&cs, 0xFF52);
    let q = find(&cs, 0xFF5C);
    let qlen = u16::from_be_bytes([cs[q + 2], cs[q + 3]]) as usize;
    let qbody = cs[q + 4..q + 2 + qlen].to_vec();
    let coc = [0xFF, 0x53, 0, 9, 0, 0, 2, 4, 4, 0, 1];
    let mut qcc = vec![0xFF, 0x5D, 0, (qbody.len() + 3) as u8, 0];
    qcc.extend(&qbody);
    for qcc_first in [false, true] {
        let mut m = cs.clone();
        // COD claims 4 levels and QCD is broken: only the COC and QCC make the stream decode.
        m[cod + 9] = 4;
        m[q + 4] = (m[q + 4] & 0xE0) | 2;
        let at = find(&m, 0xFF64);
        let mut extra = Vec::new();
        if qcc_first {
            extra.extend(&qcc);
            extra.extend(coc);
        } else {
            extra.extend(coc);
            extra.extend(&qcc);
        }
        m.splice(at..at, extra);
        assert_eq!(decode(&m).unwrap().samples, samples);
    }
    // They must name component 0.
    let mut m = cs.clone();
    let at = find(&m, 0xFF64);
    let mut bad = coc;
    bad[4] = 1;
    m.splice(at..at, bad);
    assert!(matches!(decode(&m), Err(J2kError::BadMarker(_))));
    // A second COD is refused.
    let mut m = cs.clone();
    let seg = m[cod..cod + 2 + 12 + 2].to_vec();
    let at = find(&m, 0xFF64);
    m.splice(at..at, seg);
    assert!(matches!(decode(&m), Err(J2kError::BadMarker(_))));
}

// REQ: OA-J2K-013
#[test]
fn tier_1_work_is_counted_and_bounded_before_any_block_is_decoded() {
    use crate::{header, tile, work_budget, WORK_FLOOR, WORK_PER_BYTE};
    let mut rng = Rng(77);
    // One 64 x 64 block: the work is its passes times its samples.
    let samples = image(&mut rng, 64 * 64, 16, false);
    let cs = encode(&samples, &Opts::new(64, 64, 16, 0));
    let p = header::parse(&cs).unwrap();
    let mut t = tile::build(&p).unwrap();
    tile::read_packets(&mut t, p.body, p.layers).unwrap();
    let passes = u64::from(t.blocks.passes[0]);
    assert!(passes > 30);
    assert_eq!(tile::work(&t), passes * 64 * 64);
    assert_eq!(decode(&cs).unwrap().samples, samples);
    // The same stream declaring 1 bit per sample (exponents unchanged) is inconsistent: its
    // quantization exponent is beyond what one bit allows.
    let mut lie = cs.clone();
    let siz = find(&lie, 0xFF51);
    lie[siz + 4 + 36] = 0;
    assert!(matches!(decode(&lie), Err(J2kError::BadMarker(_))));
    // With the exponent made consistent too (one bit, no decomposition) it asks for far more
    // passes than a 1-bit image can have: refused, with the numbers in the error.
    let q = find(&lie, 0xFF5C);
    lie[q + 5] = 1 << 3;
    let Err(J2kError::WorkBudget { work, budget }) = decode(&lie) else {
        panic!("not refused");
    };
    assert_eq!(work, passes * 64 * 64);
    assert_eq!(budget, work_budget(64 * 64, 1, p.body.len()));
    assert!(work > budget);
    // The budget: per sample at most 3 * (bits + 8) passes, and no more than the data backs.
    assert_eq!(work_budget(1000, 8, usize::MAX), 1000 * 48);
    assert_eq!(
        work_budget(u64::MAX, 16, 100),
        WORK_FLOOR + WORK_PER_BYTE * 100
    );
    // 16 Mi samples of 16 bits in a 12 KB stream: about 0.1 G visits, not 1.4 G.
    assert!(work_budget(16 << 20, 16, 12_000) < 110_000_000);
    // A real NOAA field (451 x 337, 15 bits, about 50 KB) has margin of at least 4 times.
    assert!(work_budget(451 * 337, 15, 36_000) > 4 * 2_200_000);
}

// REQ: OA-J2K-050
#[test]
fn lifting_is_widened_and_refuses_a_result_that_overflows_32_bits() {
    // Width two, even origin: the high sample reflects to both neighbours, so the update step
    // sums 2 * 0x3fffffff + 2 = 2^31. That overflowed an i32 sum; widened it lifts exactly.
    let mut s = [0, 0x3fff_ffff];
    lift_line(&mut s, 0).unwrap();
    assert_eq!(s, [-(1 << 29), 0x3fff_ffff - (1 << 29)]);
    // A coefficient pair whose lifted sample itself leaves i32 is refused, not wrapped.
    let mut s = [i32::MAX, i32::MAX];
    assert!(lift_line(&mut s, 0).is_err());
    let mut s = [i32::MIN, i32::MIN];
    assert!(lift_line(&mut s, 1).is_err());
    // And through the whole two-dimensional inverse.
    let res = [
        Rect {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        },
        Rect {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 1,
        },
    ];
    let mut a = [i32::MAX, i32::MAX];
    assert!(inverse(&mut a, 2, &res).is_err());
    // The same line in range lifts exactly as before.
    let mut s = [3, -4, 3, -4];
    lift_line(&mut s, 0).unwrap();
    assert_eq!(s, [5, 1, 5, 1]);
}

// REQ: OA-J2K-050
#[test]
fn a_quantization_exponent_beyond_the_sample_precision_is_refused() {
    // A 16-bit stream whose exponents allow 30 magnitude planes (guard 0, exponent 31).
    let cs = encode(&[1000; 16], &Opts::new(4, 4, 16, 1));
    let q = find(&cs, 0xFF5C);
    let mut bad = cs.clone();
    bad[q + 4] = 0; // guard 0
    for b in &mut bad[q + 5..q + 5 + 4] {
        *b = 31 << 3;
    }
    assert!(matches!(decode(&bad), Err(J2kError::BadMarker(_))));
    // One exponent just past precision plus its sub-band gain is refused; the exact value is the
    // encoder's own and decodes.
    let mut one = cs.clone();
    one[q + 5] = (16 + 1) << 3; // LL gain is 0: precision + 1 is too much
    assert!(matches!(decode(&one), Err(J2kError::BadMarker(_))));
    assert!(decode(&cs).is_ok());
}

/// A header-only stream of `w` x `h` samples with `2^(cb+2)`-sided blocks, no wavelet levels and
/// `layers` layers: the small body of a 64 x 64 image behind a larger claim.
fn claimed_geometry(w: u32, h: u32, cb: u8, layers: u16) -> Vec<u8> {
    let mut cs = encode(&[0; 64 * 64], &Opts::new(64, 64, 8, 0));
    let siz = find(&cs, 0xFF51);
    for (at, v) in [(2, w), (6, h), (18, w), (22, h)] {
        cs[siz + 4 + at..siz + 4 + at + 4].copy_from_slice(&v.to_be_bytes());
    }
    let cod = find(&cs, 0xFF52);
    cs[cod + 6..cod + 8].copy_from_slice(&layers.to_be_bytes());
    cs[cod + 10] = cb;
    cs[cod + 11] = cb;
    cs
}

// REQ: OA-J2K-014
#[test]
fn segment_bound_is_refused_before_any_table_is_built() {
    use crate::{header, tile};
    // 32768 x 512 samples, 8 x 8 blocks (262,144, at the block limit), 64 layers: 16.8 M
    // segments of tables possible from a body of a few bytes.
    let cs = claimed_geometry(32768, 512, 1, 64);
    let p = header::parse(&cs).unwrap();
    assert_eq!((p.siz.x1, p.siz.y1, p.layers), (32768, 512, 64));
    // Refused by the geometry pass itself: no block or segment table exists yet.
    let want = Err(J2kError::TooManySegments(262_144 * 64));
    assert_eq!(tile::build(&p).map(|_| ()), want);
    assert_eq!(decode(&cs).map(|_| ()), want);
    // Fewer layers at the same geometry fit under the segment limit and are only refused
    // later, for the packets they lack.
    let ok = claimed_geometry(32768, 512, 1, 0);
    assert!(!matches!(
        decode(&ok),
        Err(J2kError::TooManySegments(_) | J2kError::ScratchBudget { .. })
    ));
}

// REQ: OA-J2K-014
#[test]
fn scratch_budget_is_refused_and_real_fields_fit() {
    use crate::{scratch_bytes, MAX_SAMPLES, MAX_SCRATCH_BYTES, MAX_SEGMENTS};
    // The coefficient array of the largest image plus the largest tables still fits; one more
    // table byte-worth does not.
    assert!(scratch_bytes(MAX_SAMPLES, 1 << 18, MAX_SEGMENTS) <= MAX_SCRATCH_BYTES);
    assert!(scratch_bytes(MAX_SAMPLES, 1 << 18, 1 << 20) > MAX_SCRATCH_BYTES);
    // The largest real field of the data set: 151,987 samples, 58 blocks, 58 segments.
    assert!(scratch_bytes(151_987, 58, 58) < 1 << 20);
    // A real-size stream decodes under the limits.
    let samples: Vec<i32> = (0..64 * 64).map(|k| k % 200).collect();
    let cs = encode(&samples, &Opts::new(64, 64, 8, 3));
    assert_eq!(decode(&cs).unwrap().samples, samples);
}

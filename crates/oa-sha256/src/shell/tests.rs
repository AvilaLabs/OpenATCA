//! Tests of the hasher and its shell edges. Shell (B7): tests are shell code.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::*;

fn unhex(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let v = |c: u8| (c as char).to_digit(16).unwrap() as u8;
    b.chunks(2).map(|p| v(p[0]) << 4 | v(p[1])).collect()
}

/// Parse a CAVP response file into (Len, Msg, MD) triples.
fn parse_vectors(text: &str) -> Vec<(usize, Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    let (mut len, mut msg) = (0usize, Vec::new());
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Len = ") {
            len = v.trim().parse().unwrap();
        } else if let Some(v) = line.strip_prefix("Msg = ") {
            msg = unhex(v.trim());
            msg.truncate(len / 8);
        } else if let Some(v) = line.strip_prefix("MD = ") {
            out.push((len, msg.clone(), unhex(v.trim())));
        }
    }
    out
}

// REQ: OA-SHA-001
#[test]
fn cavp_short_msg() {
    let v = parse_vectors(include_str!("../../tests/fixtures/SHA256ShortMsg.rsp"));
    assert_eq!(v.len(), 65);
    for (len, msg, md) in &v {
        assert_eq!(msg.len() * 8, *len);
        assert_eq!(sha256(msg).to_vec(), *md, "len {len}");
    }
}

// REQ: OA-SHA-001
#[test]
fn cavp_long_msg() {
    let v = parse_vectors(include_str!("../../tests/fixtures/SHA256LongMsg.rsp"));
    assert_eq!(v.len(), 64);
    for (len, msg, md) in &v {
        assert_eq!(msg.len() * 8, *len);
        assert_eq!(sha256(msg).to_vec(), *md, "len {len}");
    }
}

// REQ: OA-SHA-001
#[test]
fn cavp_monte_carlo() {
    let text = include_str!("../../tests/fixtures/SHA256Monte.rsp");
    let mut seed = Vec::new();
    let mut expected = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Seed = ") {
            seed = unhex(v.trim());
        } else if let Some(v) = line.strip_prefix("MD = ") {
            expected.push(unhex(v.trim()));
        }
    }
    assert_eq!(expected.len(), 100);
    let (mut m0, mut m1, mut m2) = (seed.clone(), seed.clone(), seed);
    for (j, want) in expected.iter().enumerate() {
        for _ in 3..1003 {
            let mut m = Vec::with_capacity(96);
            m.extend_from_slice(&m0);
            m.extend_from_slice(&m1);
            m.extend_from_slice(&m2);
            let md = sha256(&m).to_vec();
            m0 = m1;
            m1 = m2;
            m2 = md;
        }
        assert_eq!(&m2, want, "monte count {j}");
        m0 = m2.clone();
        m1 = m2.clone();
    }
}

// REQ: OA-SHA-002
#[test]
fn short_msg_every_split_point() {
    let v = parse_vectors(include_str!("../../tests/fixtures/SHA256ShortMsg.rsp"));
    for (_, msg, md) in &v {
        for split in 0..=msg.len() {
            let mut h = Sha256::new();
            h.update(&msg[..split]).unwrap();
            h.update(&msg[split..]).unwrap();
            assert_eq!(h.finalize().to_vec(), *md, "split {split} of {}", msg.len());
        }
    }
}

// REQ: OA-SHA-002
#[test]
fn byte_at_a_time_and_reader_match() {
    let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
    let one = sha256(&data);
    let mut h = Sha256::new();
    for b in &data {
        h.update(std::slice::from_ref(b)).unwrap();
    }
    assert_eq!(h.finalize(), one);
    assert_eq!(sha256_reader(&data[..]).unwrap(), one);
}

// REQ: OA-SHA-001
#[test]
fn known_strings_and_hex() {
    assert_eq!(
        to_hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        to_hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(to_hex(&[0x00, 0xff, 0x1a]), "00ff1a");
}

// REQ: OA-SHA-001
#[test]
fn the_core_hex_writer_matches_the_string_form() {
    for msg in [&b""[..], b"abc", &[0xffu8; 200]] {
        let d = sha256(msg);
        let mut out = [0u8; 64];
        to_hex_into(&d, &mut out);
        assert_eq!(std::str::from_utf8(&out).unwrap(), to_hex(&d));
        assert_eq!(Hex(&d).to_string(), to_hex(&d));
    }
    assert_eq!(hex_byte(0x0a), *b"0a");
    assert_eq!(hex_byte(0xf0), *b"f0");
}

// REQ: OA-SHA-001, OA-GEN-004
#[test]
fn padding_boundaries_match_known_digests() {
    // Digests of n bytes of 'a' from coreutils `sha256sum`, an independent implementation.
    // 55 is the longest message whose padding fits the first block, 56 the shortest that
    // needs a second, 63 and 64 straddle the block edge, 119/120 are the same boundaries in
    // the second block.
    let known: [(usize, &str); 11] = [
        (
            0,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            1,
            "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb",
        ),
        (
            55,
            "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
        ),
        (
            56,
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
        ),
        (
            57,
            "f13b2d724659eb3bf47f2dd6af1accc87b81f09f59f2b75e5c0bed6589dfe8c6",
        ),
        (
            63,
            "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34",
        ),
        (
            64,
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
        ),
        (
            65,
            "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
        ),
        (
            119,
            "31eba51c313a5c08226adf18d4a359cfdfd8d2e816b13f4af952f7ea6584dcfb",
        ),
        (
            120,
            "2f3d335432c70b580af0e8e1b3674a7c020d683aa5f73aaaedfdc55af904c21c",
        ),
        (
            128,
            "6836cf13bac400e9105071cd6af47084dfacad4e5e302c94bfed24e013afb73e",
        ),
    ];
    for (n, want) in known {
        let data = vec![b'a'; n];
        assert_eq!(to_hex(&sha256(&data)), want, "one-shot, {n} bytes");
        // The same message fed one byte at a time pads from a partial block.
        let mut h = Sha256::new();
        for b in &data {
            h.update(std::slice::from_ref(b)).unwrap();
        }
        assert_eq!(to_hex(&h.finalize()), want, "streamed, {n} bytes");
    }
}

// REQ: OA-GEN-003
#[test]
fn length_limit_is_an_error() {
    let mut h = Sha256::new();
    h.total = MAX_MESSAGE_BYTES;
    assert!(matches!(h.update(b"x"), Err(MessageTooLong { .. })));
}

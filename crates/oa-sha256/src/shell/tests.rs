//! Tests of the hasher and its shell edges. Shell (B7): tests are shell code.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::*;

const MAX_RSP_BYTES: usize = 512 * 1024;
const MAX_RSP_RECORDS: usize = 100;
const MAX_RSP_MESSAGE_BYTES: usize = 6_400;
const SHA256_DIGEST_BYTES: usize = 32;

type TestVector = (usize, Vec<u8>, [u8; SHA256_DIGEST_BYTES]);
type ParseResult<T> = Result<T, &'static str>;

fn decode_hex(text: &str, max_bytes: usize) -> ParseResult<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err("odd-length hexadecimal field");
    }
    let decoded_len = bytes.len() / 2;
    if decoded_len > max_bytes {
        return Err("hexadecimal field exceeds byte cap");
    }
    let mut decoded = Vec::with_capacity(decoded_len);
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char)
            .to_digit(16)
            .ok_or("invalid hexadecimal digit")?;
        let lo = (pair[1] as char)
            .to_digit(16)
            .ok_or("invalid hexadecimal digit")?;
        decoded.push(((hi << 4) | lo) as u8);
    }
    Ok(decoded)
}

/// Strict bounded parser for byte-oriented SHA-256 ShortMsg/LongMsg response files.
fn parse_vectors(text: &str) -> ParseResult<Vec<TestVector>> {
    if text.len() > MAX_RSP_BYTES {
        return Err("response exceeds file byte cap");
    }

    let mut out = Vec::with_capacity(MAX_RSP_RECORDS);
    let mut digest_len_seen = false;
    let mut pending_len: Option<usize> = None;
    let mut pending_msg: Option<Vec<u8>> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[L = 32]" {
            if digest_len_seen || pending_len.is_some() || pending_msg.is_some() {
                return Err("duplicate or misplaced digest length header");
            }
            digest_len_seen = true;
            continue;
        }
        let (key, value) = line.split_once(" = ").ok_or("malformed response line")?;
        match key {
            "Len" => {
                if !digest_len_seen || pending_len.is_some() || pending_msg.is_some() {
                    return Err("duplicate or misplaced Len field");
                }
                let bits = value.parse::<usize>().map_err(|_| "invalid Len field")?;
                if bits % 8 != 0 || bits / 8 > MAX_RSP_MESSAGE_BYTES {
                    return Err("Len is non-byte-aligned or exceeds message cap");
                }
                pending_len = Some(bits);
            }
            "Msg" => {
                let bits = pending_len.ok_or("Msg field without Len")?;
                if pending_msg.is_some() {
                    return Err("duplicate Msg field");
                }
                let mut message = decode_hex(value, MAX_RSP_MESSAGE_BYTES)?;
                let expected_bytes = bits / 8;
                // CAVS represents the zero-bit message with a single 00 placeholder.
                if expected_bytes == 0 && message.as_slice() == [0] {
                    message.clear();
                }
                if message.len() != expected_bytes {
                    return Err("Msg byte count does not match Len");
                }
                pending_msg = Some(message);
            }
            "MD" => {
                let bits = pending_len.ok_or("MD field without Len")?;
                let message = pending_msg.take().ok_or("MD field without Msg")?;
                if out.len() >= MAX_RSP_RECORDS {
                    return Err("record count exceeds cap");
                }
                let digest = decode_hex(value, SHA256_DIGEST_BYTES)?;
                let digest: [u8; SHA256_DIGEST_BYTES] = digest
                    .try_into()
                    .map_err(|_| "MD must contain exactly 32 bytes")?;
                out.push((bits, message, digest));
                pending_len = None;
            }
            _ => return Err("unsupported response field"),
        }
    }
    if pending_len.is_some() || pending_msg.is_some() {
        return Err("incomplete final response record");
    }
    if !digest_len_seen || out.is_empty() {
        return Err("missing digest header or records");
    }
    Ok(out)
}

struct MonteVectors {
    seed: [u8; SHA256_DIGEST_BYTES],
    checkpoints: Vec<[u8; SHA256_DIGEST_BYTES]>,
}

fn parse_monte_vectors(text: &str) -> ParseResult<MonteVectors> {
    if text.len() > MAX_RSP_BYTES {
        return Err("response exceeds file byte cap");
    }
    let mut digest_len_seen = false;
    let mut seed: Option<[u8; SHA256_DIGEST_BYTES]> = None;
    let mut next_count = 0usize;
    let mut pending_count: Option<usize> = None;
    let mut checkpoints = Vec::with_capacity(MAX_RSP_RECORDS);
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[L = 32]" {
            if digest_len_seen {
                return Err("duplicate digest length header");
            }
            digest_len_seen = true;
            continue;
        }
        let (key, value) = line.split_once(" = ").ok_or("malformed response line")?;
        match key {
            "Seed" => {
                if !digest_len_seen || seed.is_some() || pending_count.is_some() {
                    return Err("duplicate or misplaced Seed field");
                }
                let bytes = decode_hex(value, SHA256_DIGEST_BYTES)?;
                seed = Some(bytes.try_into().map_err(|_| "Seed must contain 32 bytes")?);
            }
            "COUNT" => {
                if seed.is_none() || pending_count.is_some() {
                    return Err("duplicate or misplaced COUNT field");
                }
                let count = value.parse::<usize>().map_err(|_| "invalid COUNT field")?;
                if count != next_count || count >= MAX_RSP_RECORDS {
                    return Err("COUNT is not contiguous or exceeds cap");
                }
                pending_count = Some(count);
            }
            "MD" => {
                pending_count.take().ok_or("MD field without COUNT")?;
                let bytes = decode_hex(value, SHA256_DIGEST_BYTES)?;
                let digest = bytes.try_into().map_err(|_| "MD must contain 32 bytes")?;
                checkpoints.push(digest);
                next_count += 1;
            }
            _ => return Err("unsupported response field"),
        }
    }
    if pending_count.is_some() {
        return Err("COUNT missing MD");
    }
    if next_count != MAX_RSP_RECORDS {
        return Err("expected exactly 100 checkpoints");
    }
    Ok(MonteVectors {
        seed: seed.ok_or("missing Seed")?,
        checkpoints,
    })
}

// REQ: OA-SHA-001
#[test]
fn cavp_short_msg() {
    let v = parse_vectors(include_str!("../../tests/fixtures/SHA256ShortMsg.rsp")).unwrap();
    assert_eq!(v.len(), 65);
    for (len, msg, md) in &v {
        assert_eq!(msg.len() * 8, *len);
        assert_eq!(sha256(msg), *md, "len {len}");
    }
}

// REQ: OA-SHA-001
#[test]
fn cavp_long_msg() {
    let v = parse_vectors(include_str!("../../tests/fixtures/SHA256LongMsg.rsp")).unwrap();
    assert_eq!(v.len(), 64);
    for (len, msg, md) in &v {
        assert_eq!(msg.len() * 8, *len);
        assert_eq!(sha256(msg), *md, "len {len}");
    }
}

// REQ: OA-SHA-001
#[test]
fn cavp_monte_carlo() {
    let vectors =
        parse_monte_vectors(include_str!("../../tests/fixtures/SHA256Monte.rsp")).unwrap();
    let (mut m0, mut m1, mut m2) = (vectors.seed, vectors.seed, vectors.seed);
    for (j, want) in vectors.checkpoints.iter().enumerate() {
        for _ in 3..1003 {
            let mut m = Vec::with_capacity(96);
            m.extend_from_slice(&m0);
            m.extend_from_slice(&m1);
            m.extend_from_slice(&m2);
            let md = sha256(&m);
            m0 = m1;
            m1 = m2;
            m2 = md;
        }
        assert_eq!(&m2, want, "monte count {j}");
        m0 = m2;
        m1 = m2;
    }
}

#[test]
fn response_loader_refuses_malformed_and_unbounded_inputs() {
    const VALID: &str = "[L = 32]\nLen = 0\nMsg = 00\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n";
    let malformed = [
        "[L = 32]\nMsg = 00\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "[L = 32]\nLen = 0\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "[L = 32]\nLen = 0\nMsg = 00\n",
        "[L = 32]\nLen = 0\nMsg = 0\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "[L = 32]\nLen = 8\nMsg = gg\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "[L = 32]\nLen = 0\nLen = 0\nMsg = 00\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "[L = 32]\nLen = 8\nMsg = ff\nMsg = ff\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "[L = 32]\nLen = 0\nMsg = 00\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "[L = 32]\nLen = 0\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nMsg = 00\n",
        "[L = 32]\nLen = 51208\nMsg = 00\nMD = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
    ];
    assert!(parse_vectors(VALID).is_ok());
    for fixture in malformed {
        assert!(
            parse_vectors(fixture).is_err(),
            "accepted malformed fixture: {fixture}"
        );
    }
    assert!(parse_vectors(&" ".repeat(MAX_RSP_BYTES + 1)).is_err());
}

#[test]
fn monte_loader_refuses_malformed_fields_and_order() {
    const DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let valid = format!("[L = 32]\nSeed = {DIGEST}\nCOUNT = 0\nMD = {DIGEST}\n");
    assert!(
        parse_monte_vectors(&valid).is_err(),
        "one checkpoint must not masquerade as full file"
    );
    let malformed = [
        format!("[L = 32]\nCOUNT = 0\nMD = {DIGEST}\n"),
        format!("[L = 32]\nSeed = {DIGEST}\nMD = {DIGEST}\n"),
        format!("[L = 32]\nSeed = {DIGEST}\nCOUNT = 0\nCOUNT = 0\nMD = {DIGEST}\n"),
        format!("[L = 32]\nSeed = {DIGEST}\nCOUNT = 0\nMD = 0\n"),
        format!("[L = 32]\nSeed = {DIGEST}\nCOUNT = 1\nMD = {DIGEST}\n"),
    ];
    for fixture in malformed {
        assert!(
            parse_monte_vectors(&fixture).is_err(),
            "accepted malformed fixture: {fixture}"
        );
    }
}

// REQ: OA-SHA-002
#[test]
fn short_msg_every_split_point() {
    let v = parse_vectors(include_str!("../../tests/fixtures/SHA256ShortMsg.rsp")).unwrap();
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

#![forbid(unsafe_code)]
//! oa-sha256: SHA-256 per FIPS 180-4. Shell/offline code (B1): it does no I/O except in
//! [`sha256_reader`], which only pulls bytes from a caller-supplied reader.
//!
//! The hasher is a subsystem under D5 (it holds precomputed chaining state and a partial block),
//! so it keeps interface methods. Used for content identity of snapshots and raw downloads (A8).

use std::fmt;
use std::io::Read;

/// Block size in bytes (FIPS 180-4 section 1).
pub const BLOCK_LEN: usize = 64;
/// Digest size in bytes.
pub const DIGEST_LEN: usize = 32;
/// Limit: FIPS 180-4 caps a message at 2^64 - 1 bits, so at most this many whole bytes.
pub const MAX_MESSAGE_BYTES: u64 = u64::MAX / 8;
/// Read buffer size used by [`sha256_reader`].
pub const READ_CHUNK_LEN: usize = 64 * 1024;

/// A 32-byte SHA-256 digest.
pub type Digest = [u8; DIGEST_LEN];

/// Hashing errors.
#[derive(Debug)]
pub enum Sha256Error {
    /// The message exceeded [`MAX_MESSAGE_BYTES`].
    MessageTooLong { limit: u64 },
    /// The reader failed.
    Io(std::io::Error),
}

impl fmt::Display for Sha256Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Sha256Error::MessageTooLong { limit } => {
                write!(
                    f,
                    "sha256: message exceeds limit MAX_MESSAGE_BYTES ({limit} bytes)"
                )
            }
            Sha256Error::Io(e) => write!(f, "sha256: read error: {e}"),
        }
    }
}

impl std::error::Error for Sha256Error {}

const INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Streaming SHA-256 state.
#[derive(Clone, Debug)]
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; BLOCK_LEN],
    buf_len: usize,
    total: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// Process one 64-byte block into the chaining state.
fn compress(h: &mut [u32; 8], block: &[u8; BLOCK_LEN]) {
    let mut w = [0u32; 64];
    let (words, _) = block.as_chunks::<4>();
    for (wi, c) in w.iter_mut().zip(words) {
        *wi = u32::from_be_bytes(*c);
    }
    for t in 16..64 {
        let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
        let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
        w[t] = w[t - 16]
            .wrapping_add(s0)
            .wrapping_add(w[t - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for t in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[t])
            .wrapping_add(w[t]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

impl Sha256 {
    /// A fresh hasher.
    pub const fn new() -> Self {
        Sha256 {
            h: INIT,
            buf: [0; BLOCK_LEN],
            buf_len: 0,
            total: 0,
        }
    }

    /// Absorb more message bytes.
    pub fn update(&mut self, mut data: &[u8]) -> Result<(), Sha256Error> {
        let len = data.len() as u64;
        self.total = match self.total.checked_add(len) {
            Some(t) if t <= MAX_MESSAGE_BYTES => t,
            _ => {
                return Err(Sha256Error::MessageTooLong {
                    limit: MAX_MESSAGE_BYTES,
                })
            }
        };
        if self.buf_len > 0 {
            let room = BLOCK_LEN - self.buf_len;
            let take = room.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len < BLOCK_LEN {
                return Ok(());
            }
            let block = self.buf;
            compress(&mut self.h, &block);
            self.buf_len = 0;
        }
        let (blocks, rest) = data.as_chunks::<BLOCK_LEN>();
        for block in blocks {
            compress(&mut self.h, block);
        }
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buf_len = rest.len();
        Ok(())
    }

    /// Pad and return the digest.
    pub fn finalize(mut self) -> Digest {
        let bits = self.total.wrapping_mul(8); // total <= MAX_MESSAGE_BYTES, cannot wrap
        let mut block = [0u8; BLOCK_LEN];
        block[..self.buf_len].copy_from_slice(&self.buf[..self.buf_len]);
        block[self.buf_len] = 0x80;
        if self.buf_len + 1 > BLOCK_LEN - 8 {
            compress(&mut self.h, &block);
            block = [0u8; BLOCK_LEN];
        }
        block[BLOCK_LEN - 8..].copy_from_slice(&bits.to_be_bytes());
        compress(&mut self.h, &block);
        let mut out = [0u8; DIGEST_LEN];
        let (words, _) = out.as_chunks_mut::<4>();
        for (o, w) in words.iter_mut().zip(self.h) {
            *o = w.to_be_bytes();
        }
        out
    }
}

/// One-shot digest of a byte slice. Infallible: a slice cannot exceed [`MAX_MESSAGE_BYTES`].
pub fn sha256(data: &[u8]) -> Digest {
    let mut h = Sha256::new();
    // A slice length is at most isize::MAX, far below MAX_MESSAGE_BYTES, so this cannot fail.
    let _ = h.update(data);
    h.finalize()
}

/// Digest everything a reader yields (for example a file).
pub fn sha256_reader<R: Read>(mut r: R) -> Result<Digest, Sha256Error> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; READ_CHUNK_LEN];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n])?,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Sha256Error::Io(e)),
        }
    }
    Ok(h.finalize())
}

/// Lowercase hex encoding of bytes.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let v = parse_vectors(include_str!("../tests/fixtures/SHA256ShortMsg.rsp"));
        assert_eq!(v.len(), 65);
        for (len, msg, md) in &v {
            assert_eq!(msg.len() * 8, *len);
            assert_eq!(sha256(msg).to_vec(), *md, "len {len}");
        }
    }

    // REQ: OA-SHA-001
    #[test]
    fn cavp_long_msg() {
        let v = parse_vectors(include_str!("../tests/fixtures/SHA256LongMsg.rsp"));
        assert_eq!(v.len(), 64);
        for (len, msg, md) in &v {
            assert_eq!(msg.len() * 8, *len);
            assert_eq!(sha256(msg).to_vec(), *md, "len {len}");
        }
    }

    // REQ: OA-SHA-001
    #[test]
    fn cavp_monte_carlo() {
        let text = include_str!("../tests/fixtures/SHA256Monte.rsp");
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
        let v = parse_vectors(include_str!("../tests/fixtures/SHA256ShortMsg.rsp"));
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

    // REQ: OA-SHA-001, OA-GEN-004
    #[test]
    fn padding_boundaries_do_not_panic() {
        for n in 0..200usize {
            let d = vec![0xa5u8; n];
            let _ = sha256(&d);
        }
    }

    // REQ: OA-GEN-003
    #[test]
    fn length_limit_is_an_error() {
        let mut h = Sha256::new();
        h.total = MAX_MESSAGE_BYTES;
        assert!(matches!(
            h.update(b"x"),
            Err(Sha256Error::MessageTooLong { .. })
        ));
    }

    // REQ: OA-SHA-001
    #[test]
    fn real_nasr_zip_matches_sha256sum() {
        let Ok(raw) = std::env::var("OPENATCA_RAW") else {
            eprintln!("skipped: OPENATCA_RAW not set");
            return;
        };
        let p = std::path::Path::new(&raw).join("nasr-2026-10-29/NASR_CSV_29_Oct_2026.zip");
        let f = std::fs::File::open(p).unwrap();
        let t = std::time::Instant::now();
        let d = sha256_reader(f).unwrap();
        eprintln!("nasr zip hashed in {:?}", t.elapsed());
        assert_eq!(
            to_hex(&d),
            "60b072454319f166797c5f4667520e4cf9c54d2e4e5faffbcdbae846aea276da"
        );
    }
}

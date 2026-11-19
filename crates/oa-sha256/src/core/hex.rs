//! Lowercase hex text of bytes. Core (pure, B1, B7): a function of its bytes, written into
//! storage the caller owns; no allocation, no I/O. The `String`-returning `to_hex` is shell.

const DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The two lowercase hex digits of `b`, high nibble first.
pub fn hex_byte(b: u8) -> [u8; 2] {
    [
        DIGITS.get(usize::from(b >> 4)).copied().unwrap_or(b'0'),
        DIGITS.get(usize::from(b & 15)).copied().unwrap_or(b'0'),
    ]
}

/// Writes the 64 lowercase hex digits of a digest into `out`.
pub fn to_hex_into(digest: &[u8; 32], out: &mut [u8; 64]) {
    let (pairs, _) = out.as_chunks_mut::<2>();
    for (pair, &b) in pairs.iter_mut().zip(digest) {
        *pair = hex_byte(b);
    }
}

/// A digest shown as 64 lowercase hex digits by `Display`; it formats from a stack buffer, so a
/// core caller that builds text gets the digits without a second allocation of its own.
pub struct Hex<'a>(pub &'a [u8; 32]);

impl core::fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut buf = [0u8; 64];
        to_hex_into(self.0, &mut buf);
        f.write_str(core::str::from_utf8(&buf).map_err(|_| core::fmt::Error)?)
    }
}

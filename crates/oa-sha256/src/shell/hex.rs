//! Lowercase hex text as a `String`. Shell (B1, B7): it allocates what it returns, which core
//! does not; the digits come from core's `hex_byte`.

use crate::core::hex::hex_byte;

/// Lowercase hex encoding of bytes.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        for d in hex_byte(b) {
            s.push(char::from(d));
        }
    }
    s
}

//! Lowercase hex text of a byte string. Core (pure, B1, B7): a function of its bytes; it
//! allocates the `String` it returns (allowed in core) and does no I/O.

/// Lowercase hex encoding of bytes.
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        // Writing to a String cannot fail.
        let _ = write!(s, "{b:02x}");
    }
    s
}

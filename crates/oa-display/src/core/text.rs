//! Fixed-size text buffer (**core**, B1, C1): the data block and notice text of a frame is written
//! here, on the stack, instead of into a fresh `String` per target per frame. No function in this
//! module allocates.
//!
//! A [`TextBuf`] is a validated value type (A1.1): its bytes are always ASCII, so it is always a
//! valid `&str`, and its fields are private. Text that does not fit, or is not ASCII, is not
//! written at all and the buffer says so ([`text_clipped`]); it is never silently cut. The callers
//! size their text so it fits, and count the clip if it ever happens (E6).

/// Bytes a buffer holds. The longest text written is a safety banner line of about 100 bytes at
/// its numeric caps; 128 leaves margin so that no line is clipped.
pub const TEXT_BYTES: usize = 128;

/// ASCII text of at most [`TEXT_BYTES`] bytes. Plain data with private fields (A1.1).
// Budget: 128 bytes of text, its length and the clip flag; one per text line being built, on the
// stack, never held in quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct TextBuf {
    bytes: [u8; TEXT_BYTES],
    len: u8,
    clipped: bool,
}
const _: () = assert!(core::mem::size_of::<TextBuf>() == TEXT_BYTES + 2);

/// An empty buffer.
pub const fn text_new() -> TextBuf {
    TextBuf {
        bytes: [0; TEXT_BYTES],
        len: 0,
        clipped: false,
    }
}

/// Empties the buffer and its clip flag.
pub fn text_clear(b: &mut TextBuf) {
    b.len = 0;
    b.clipped = false;
}

/// The text written so far. A buffer holds only ASCII, so the fallback `?` is unreachable; it is
/// there so a broken invariant shows instead of panicking.
pub fn text_str(b: &TextBuf) -> &str {
    b.bytes
        .get(..usize::from(b.len))
        .and_then(|s| core::str::from_utf8(s).ok())
        .unwrap_or("?")
}

/// True when something was refused for lack of room or because it was not ASCII.
pub fn text_clipped(b: &TextBuf) -> bool {
    b.clipped
}

/// True when nothing has been written.
pub fn text_is_empty(b: &TextBuf) -> bool {
    b.len == 0
}

fn put(b: &mut TextBuf, s: &[u8]) {
    let at = usize::from(b.len);
    let end = at.saturating_add(s.len());
    match b.bytes.get_mut(at..end) {
        Some(dst) if s.is_ascii() => {
            dst.copy_from_slice(s);
            // `end` is at most TEXT_BYTES (128), so it fits in a u8.
            b.len = u8::try_from(end).unwrap_or(u8::MAX);
        }
        _ => b.clipped = true,
    }
}

/// Appends `s` whole, or nothing and sets the clip flag when it does not fit or is not ASCII.
pub fn text_push_str(b: &mut TextBuf, s: &str) {
    put(b, s.as_bytes());
}

/// Appends one ASCII character, or sets the clip flag.
pub fn text_push_char(b: &mut TextBuf, c: u8) {
    put(b, &[c]);
}

/// Appends `v` in decimal, left padded with zeros to at least `min_digits` (at most 20).
pub fn text_push_dec(b: &mut TextBuf, v: u64, min_digits: usize) {
    let mut digits = [b'0'; 20];
    let mut n = v;
    let mut used = 0usize;
    for slot in digits.iter_mut().rev() {
        *slot = b'0' + (n % 10) as u8;
        n /= 10;
        used += 1;
        if n == 0 {
            break;
        }
    }
    let width = used.max(min_digits.min(digits.len()));
    let from = digits.len() - width;
    if let Some(s) = digits.get(from..) {
        put(b, s);
    }
}

/// Appends `v` in upper-case hexadecimal, left padded with zeros to at least `min_digits` (at
/// most 8).
pub fn text_push_hex(b: &mut TextBuf, v: u32, min_digits: usize) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut digits = [b'0'; 8];
    let mut n = v;
    let mut used = 0usize;
    for slot in digits.iter_mut().rev() {
        *slot = HEX.get((n & 15) as usize).copied().unwrap_or(b'?');
        n >>= 4;
        used += 1;
        if n == 0 {
            break;
        }
    }
    let width = used.max(min_digits.min(digits.len()));
    let from = digits.len() - width;
    if let Some(s) = digits.get(from..) {
        put(b, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_pad_and_never_overflow_the_buffer() {
        let mut b = text_new();
        text_push_dec(&mut b, 7, 3);
        text_push_char(&mut b, b' ');
        text_push_dec(&mut b, u64::MAX, 0);
        text_push_char(&mut b, b' ');
        text_push_hex(&mut b, 0xABC, 6);
        text_push_char(&mut b, b' ');
        text_push_hex(&mut b, u32::MAX, 0);
        assert_eq!(text_str(&b), "007 18446744073709551615 000ABC FFFFFFFF");
        assert!(!text_clipped(&b));
        let mut z = text_new();
        text_push_dec(&mut z, 0, 0);
        assert_eq!(text_str(&z), "0");
    }

    #[test]
    fn text_that_does_not_fit_or_is_not_ascii_is_refused_whole_and_flagged() {
        let mut b = text_new();
        text_push_str(&mut b, &"A".repeat(TEXT_BYTES - 1));
        assert!(!text_clipped(&b));
        text_push_str(&mut b, "BC");
        assert!(text_clipped(&b), "two bytes do not fit in the one left");
        assert_eq!(
            text_str(&b).len(),
            TEXT_BYTES - 1,
            "nothing was half written"
        );
        text_push_char(&mut b, b'B');
        assert_eq!(text_str(&b).len(), TEXT_BYTES);
        let mut c = text_new();
        text_push_str(&mut c, "caf\u{e9}");
        assert!(text_clipped(&c) && text_is_empty(&c));
        text_clear(&mut c);
        assert!(!text_clipped(&c));
    }
}

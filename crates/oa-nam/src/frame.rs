//! The transport framing of NAS-IC-82422100 3.2.2.3.1: a four byte header of uppercase ASCII
//! hexadecimal characters giving the length of the message that follows (header excluded),
//! then the message. Smallest valid message 17 bytes (an IRQ), largest 1800. Core (pure).

use crate::error::{ErrorKind as K, NamError};
use crate::field::MAX_MESSAGE_LEN;
use crate::message::MIN_MESSAGE_LEN;

pub const HEADER_LEN: usize = 4;

/// Prepends the length header to a message.
pub fn frame(msg: &[u8]) -> Result<Vec<u8>, NamError> {
    if msg.len() < MIN_MESSAGE_LEN || msg.len() > MAX_MESSAGE_LEN {
        return Err(NamError::new(K::Length, 0));
    }
    let mut out = format!("{:04X}", msg.len()).into_bytes();
    out.extend_from_slice(msg);
    Ok(out)
}

/// Reads the length header at the start of `buf`: `Ok(None)` when fewer than four bytes are
/// there, an error for a character that is not an uppercase hexadecimal digit or a length
/// outside 17 to 1800 (a stream that has lost framing cannot be resynchronised: the caller
/// drops the connection, NAS-IC-82422100 3.2.2.6).
pub fn read_header(buf: &[u8]) -> Result<Option<usize>, NamError> {
    let Some(h) = buf.get(..HEADER_LEN) else {
        return Ok(None);
    };
    let mut v = 0usize;
    for (i, &c) in h.iter().enumerate() {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'F' => c - b'A' + 10,
            _ => return Err(NamError::new(K::FrameHeader, 0).at(i)),
        };
        v = v * 16 + usize::from(d);
    }
    if !(MIN_MESSAGE_LEN..=MAX_MESSAGE_LEN).contains(&v) {
        return Err(NamError::new(K::Length, 0).at(0));
    }
    Ok(Some(v))
}

/// The first frame in `buf`: the message bytes and the number of bytes consumed, or `None`
/// when the frame is not all there yet.
pub fn unframe(buf: &[u8]) -> Result<Option<(&[u8], usize)>, NamError> {
    let Some(len) = read_header(buf)? else {
        return Ok(None);
    };
    match buf.get(HEADER_LEN..HEADER_LEN + len) {
        Some(msg) => Ok(Some((msg, HEADER_LEN + len))),
        None => Ok(None),
    }
}

//! The I/O and text edges of the hasher. Shell (B1, B3, B7): [`sha256_reader`] pulls bytes from
//! a caller-supplied reader and [`Sha256Error`] carries the reader's `std::io::Error`.

use std::fmt;
use std::io::Read;

use crate::core::sha256::{Digest, MessageTooLong, Sha256};

/// Read buffer size used by [`sha256_reader`].
pub const READ_CHUNK_LEN: usize = 64 * 1024;

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
            Sha256Error::MessageTooLong { limit } => MessageTooLong { limit: *limit }.fmt(f),
            Sha256Error::Io(e) => write!(f, "sha256: read error: {e}"),
        }
    }
}

impl std::error::Error for Sha256Error {}

impl From<MessageTooLong> for Sha256Error {
    fn from(e: MessageTooLong) -> Self {
        Sha256Error::MessageTooLong { limit: e.limit }
    }
}

/// Digest everything a reader yields (for example a file).
pub fn sha256_reader<R: Read>(mut r: R) -> Result<Digest, Sha256Error> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; READ_CHUNK_LEN];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = buf.get(..n).ok_or_else(|| {
                    Sha256Error::Io(std::io::Error::other(
                        "reader returned more than it was given room for",
                    ))
                })?;
                h.update(chunk)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Sha256Error::Io(e)),
        }
    }
    Ok(h.finalize())
}

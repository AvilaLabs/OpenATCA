#![forbid(unsafe_code)]
//! oa-sha256: SHA-256 per FIPS 180-4, used for content identity of snapshots and raw downloads
//! (A8). Class `split` (B7): the hasher is core (`core::sha256`, pure); reading from a
//! [`std::io::Read`] (`shell::io`, which owns [`Sha256Error`]) and the `String` form of hex
//! text (`shell::hex`) are shell; core writes hex into caller storage ([`to_hex_into`]). The
//! core's own error is [`MessageTooLong`], converted by `From`.

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;

pub use crate::core::hex::{hex_byte, to_hex_into, Hex};
pub use crate::core::sha256::{
    sha256, Digest, MessageTooLong, Sha256, BLOCK_LEN, DIGEST_LEN, MAX_MESSAGE_BYTES,
};
pub use crate::shell::hex::to_hex;
pub use crate::shell::io::{sha256_reader, Sha256Error, READ_CHUNK_LEN};

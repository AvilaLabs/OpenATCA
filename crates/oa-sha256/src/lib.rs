#![forbid(unsafe_code)]
//! oa-sha256: SHA-256 per FIPS 180-4, used for content identity of snapshots and raw downloads
//! (A8). Class `split` (B7): the hasher is core (`core::sha256`, pure); reading from a
//! [`std::io::Read`] and hex text are shell (`shell::io`), which owns [`Sha256Error`]. The
//! core's own error is [`MessageTooLong`], converted by `From`.

pub mod core;
pub mod shell;

pub use crate::core::sha256::{
    sha256, Digest, MessageTooLong, Sha256, BLOCK_LEN, DIGEST_LEN, MAX_MESSAGE_BYTES,
};
pub use crate::shell::io::{sha256_reader, to_hex, Sha256Error, READ_CHUNK_LEN};

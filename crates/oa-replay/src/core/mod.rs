//! Core (pure, B1, B7): no I/O, no clock, no allocation after construction (B1-B5). The fixed
//! capacity flight table, its hash index and slot bookkeeping, the replay clock's step rules,
//! the error counts. Time is each frame's `recv_unix_ns`, passed in (B4). The state hash is not
//! here: it allocates a slot list per checkpoint, so it is shell (C6).
//! The pipeline and the tables that take decoded rows (`apply`, `adsb`, `fns`, `tfms`) name the
//! decoder crates, which core may not (B8), so they are shell: see `shell`.

pub mod clock;
pub mod errors;
pub mod feed;
pub mod index;
pub(crate) mod mono_split;
pub mod slots;
pub mod table;

//! Core (pure, B1, B7): no I/O, no clock, no allocation after construction (B1-B5). The fixed
//! capacity tables, the pipeline that applies frames to them, the replay clock's step rules and
//! the state hash. Time is each frame's `recv_unix_ns`, passed in (B4).

pub mod adsb;
pub mod apply;
pub mod clock;
pub mod errors;
pub mod fns;
pub mod hash;
pub mod index;
pub(crate) mod mono_split;
pub mod pipeline;
pub mod slots;
pub mod table;
pub mod tfms;

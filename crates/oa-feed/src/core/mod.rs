//! Core (pure, B1, B7): no I/O, no clock, no allocation after construction. The frame layout and
//! encoder, the resynchronising decoder over a caller-supplied buffer, and the feed health
//! record; time is always an input (B4).

pub mod frame;
pub mod health;
pub mod reader;

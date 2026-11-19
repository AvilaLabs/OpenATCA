//! Core (pure, B1, B7): the smoother and its linear algebra, the leave-one-out evaluation, the
//! quality rules, the queries on a built truth, and the plain data they share. No I/O, no clock,
//! no randomness, no hash maps. Re-exported at the crate root.

pub(crate) mod frame;
pub(crate) mod linalg;
pub(crate) mod loo;
pub mod quality;
pub mod query;
pub mod smooth;
pub mod types;

//! Core (pure, B1, B7): the crossing search along a truth path, the landing time, the look-ahead
//! bins and their statistics, the join rule and the report text. No I/O, no clock, no hash maps.

pub mod bins;
pub mod cross;
pub mod join;
pub mod places;
pub mod report;
pub mod types;

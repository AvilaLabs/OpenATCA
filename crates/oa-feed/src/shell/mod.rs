//! Shell (B1, B3, B7): recording to rotating files, replay from files, whole-stream reads, the
//! command line, the text form of the health summary, and the tests. The `oa-feed` binary
//! (`src/main.rs`) is shell too.

pub mod cli;
pub mod record;
pub mod replay;
pub mod stream;
pub mod summary;
#[cfg(test)]
mod tests;

//! Shell (B1, B3, B7): hour files on disk with a bounded cache (`archive`), the archive-backed
//! comparison with observations (`compare`), and the tests. The `oa-wind-check` binary
//! (`src/main.rs`) is shell too.

pub mod archive;
pub mod compare;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_index;

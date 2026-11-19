#![forbid(unsafe_code)]
//! Shell (B1): the commit a program was built from, for the files it writes to say which source
//! produced them. `build.rs` reads `git -C <this crate's directory>`, so the value is the
//! repository the program was built from and not whatever repository it happens to run in. It
//! lives in a crate of its own because a build script is not allowed in a core or split crate
//! (B8).

/// `commit <hash>`, followed by ` (tracked files modified)` when the working tree differed from
/// that commit at build time; `commit unknown` only if git was not available at build time.
pub const BUILD: &str = env!("OA_BUILD_INFO");

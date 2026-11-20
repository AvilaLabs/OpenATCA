//! The sealed recordings of 2026-11-19 are never read: the guard is the shared one (`oa-sealed`,
//! list in `crates/oa-sealed/sealed_paths.txt`), which resolves a path (links, `..`, the nearest existing
//! ancestor of a new directory) before it checks it. Nothing is opened, listed or read to decide.

pub use oa_sealed::refuse as refuse_sealed;

/// The opener every input file goes through: the shared guard checks the path and the opened
/// file, so a link inside an unsealed directory is caught too.
pub fn guarded_opener() -> oa_bench::input::Opener {
    oa_bench::input::file_opener()
}

//! Shell (B1): the sealed hold-out data. The rule and the list live in the shared guard
//! (`oa-sealed`, `crates/oa-sealed/sealed_paths.txt`); this is only its error mapping for the export.
//! Every path is resolved and checked there, as given and after links.

use std::path::Path;

use super::error::ExportError;

/// Refuses `path` if it is, is inside, resolves into or holds a sealed recording.
pub fn refuse_holdout(path: &Path) -> Result<(), ExportError> {
    oa_sealed::refuse(path).map_err(ExportError::Input)
}

/// The opener every input file goes through: `oa_bench`'s file opener is itself guarded.
pub fn guarded_opener() -> oa_bench::input::Opener {
    oa_bench::input::file_opener()
}

/// Opens `path` for reading through the shared guard (checked before and after the open).
pub fn open(path: &Path) -> std::io::Result<std::fs::File> {
    oa_sealed::open(path)
}

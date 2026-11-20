//! Shell (B1): the sealed hold-out data. The rule and the list live in the shared guard
//! (`oa-sealed`, `tools/sealed_paths.txt`); this is only its error mapping for the export.
//! Every path is resolved and checked there, as given and after links.

use std::path::Path;

use super::error::ExportError;

/// Refuses `path` if it is, is inside, resolves into or holds a sealed recording.
pub fn refuse_holdout(path: &Path) -> Result<(), ExportError> {
    oa_sealed::refuse(path).map_err(ExportError::Input)
}

/// The opener every input file goes through: a sealed target is refused before the file is
/// opened, so a link inside an unsealed directory is caught too.
pub fn guarded_opener() -> oa_bench::input::Opener {
    let inner = oa_bench::input::file_opener();
    std::rc::Rc::new(move |p: &Path| {
        oa_sealed::refuse_io(p)?;
        inner(p)
    })
}

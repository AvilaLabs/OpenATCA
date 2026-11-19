//! Shell (B1): the directories of a run. Every level a run creates is recorded, and on an error
//! every level it created is removed, deepest first, with `remove_dir` only (a directory that
//! holds anything is left). The output directory is checked before anything is created: it must
//! be new or empty, and a directory that holds only `.partial` files says that an earlier run was
//! interrupted and names them (they are not removed).

use std::fs;
use std::path::{Path, PathBuf};

use super::error::{io_err, ExportError};

/// Creates `path` and every missing parent, recording each level created, top first.
pub fn create_tracked(path: &Path, created: &mut Vec<PathBuf>) -> Result<(), ExportError> {
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut at = Some(path);
    while let Some(p) = at {
        if p.as_os_str().is_empty() || p.exists() {
            break;
        }
        missing.push(p.to_path_buf());
        at = p.parent();
    }
    for p in missing.into_iter().rev() {
        fs::create_dir(&p).map_err(|e| io_err(&p, &e))?;
        created.push(p);
    }
    Ok(())
}

/// Removes the levels in `created`, deepest first; a level that is not empty stays.
pub fn remove_created(created: &[PathBuf]) {
    for p in created.iter().rev() {
        let _ = fs::remove_dir(p);
    }
}

/// The files under `dir` (two levels), as paths relative to it, and whether anything else is
/// there (a non-regular entry or a level deeper).
fn walk(dir: &Path, rel: &Path, depth: u32, files: &mut Vec<PathBuf>, other: &mut bool) {
    let Ok(rd) = fs::read_dir(dir) else {
        *other = true;
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let r = rel.join(e.file_name());
        match e.file_type() {
            Ok(t) if t.is_file() => files.push(r),
            Ok(t) if t.is_dir() && depth < 2 => walk(&e.path(), &r, depth + 1, files, other),
            _ => *other = true,
        }
    }
}

/// Refuses an output directory that exists and is not empty. If all it holds is `.partial` files
/// the message says an earlier run was interrupted and names them.
pub fn check_out(out: &Path) -> Result<(), ExportError> {
    if !out.exists() {
        return Ok(());
    }
    let is_empty = fs::read_dir(out)
        .map_err(|e| io_err(out, &e))?
        .next()
        .is_none();
    if is_empty {
        return Ok(());
    }
    let (mut files, mut other) = (Vec::new(), false);
    walk(out, Path::new(""), 0, &mut files, &mut other);
    files.sort();
    let only_partial = !other
        && !files.is_empty()
        && files
            .iter()
            .all(|f| f.extension().is_some_and(|x| x == "partial"));
    if only_partial {
        let names: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
        return Err(ExportError::Input(format!(
            "{}: an earlier run was interrupted: the directory holds only partial files ({}); remove these files and run again (they are not removed automatically)",
            out.display(),
            names.join(", ")
        )));
    }
    Err(ExportError::Input(format!(
        "{}: the output directory is not empty; the export writes into a new or empty directory only",
        out.display()
    )))
}

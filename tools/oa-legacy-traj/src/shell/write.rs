//! Output files, written whole under a temporary name and then renamed, so a reader never sees a
//! half-written report.

use std::fs;
use std::path::Path;

/// Writes `text` to `dir/name`: to `dir/name.partial` first, then renamed over the final name.
pub fn write_atomic(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let tmp = dir.join(format!("{name}.partial"));
    let fin = dir.join(name);
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, &fin).map_err(|e| format!("{}: {e}", fin.display()))
}

/// Writes every `(name, text)` file under `dir` (created if absent). All are written under their
/// temporary names before any is renamed, so a failure part-way leaves no final file new.
pub fn write_all(dir: &Path, files: &[(String, String)]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, text) in files {
        let tmp = dir.join(format!("{name}.partial"));
        fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    }
    for (name, _) in files {
        let tmp = dir.join(format!("{name}.partial"));
        let fin = dir.join(name);
        fs::rename(&tmp, &fin).map_err(|e| format!("{}: {e}", fin.display()))?;
    }
    Ok(())
}

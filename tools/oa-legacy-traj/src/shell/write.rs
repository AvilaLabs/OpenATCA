//! Output files, written whole under a temporary name and then renamed, so a reader never sees a
//! half-written report.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Writes `text` to `dir/name`: to `dir/name.partial` first, then renamed over the final name.
pub fn write_atomic(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let tmp = dir.join(format!("{name}.partial"));
    let fin = dir.join(name);
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, &fin).map_err(|e| format!("{}: {e}", fin.display()))
}

/// Writes every `(name, text)` file under `dir` (created if absent). All are written under their
/// temporary names before any is renamed. On any failure every temporary file this call created
/// is removed, and the error says which files were already renamed (their new content is in
/// place; the others are as they were).
pub fn write_all(dir: &Path, files: &[(String, String)]) -> Result<(), String> {
    oa_sealed::refuse(dir).map_err(|e| e.to_string())?;
    for (name, _) in files {
        if Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
            return Err("report output needs a single file name".into());
        }
        oa_sealed::refuse(&dir.join(name)).map_err(|e| e.to_string())?;
        oa_sealed::refuse(&dir.join(format!("{name}.partial"))).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp_of = |name: &str| dir.join(format!("{name}.partial"));
    let mut created: Vec<&str> = Vec::new();
    let clean = |created: &[&str]| {
        for name in created {
            let _ = fs::remove_file(tmp_of(name));
        }
    };
    for (name, text) in files {
        let tmp = tmp_of(name);
        let result = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .and_then(|mut file| {
                created.push(name);
                file.write_all(text.as_bytes())
            });
        if let Err(e) = result {
            clean(&created);
            return Err(format!("{}: {e}; nothing was renamed", tmp.display()));
        }
    }
    let mut renamed: Vec<&str> = Vec::new();
    for (name, _) in files {
        if let Err(e) = fs::rename(tmp_of(name), dir.join(name)) {
            let left: Vec<&str> = created
                .iter()
                .copied()
                .filter(|n| !renamed.contains(n))
                .collect();
            clean(&left);
            let done = if renamed.is_empty() {
                "none".to_string()
            } else {
                renamed.join(", ")
            };
            return Err(format!(
                "{}: {e}; already renamed: {done}",
                dir.join(name).display()
            ));
        }
        renamed.push(name);
    }
    Ok(())
}

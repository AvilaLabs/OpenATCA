//! Staged, no-overwrite publication of the comparator's three reports and completion marker.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use oa_sealed::Sealed;

use crate::guard::{create_output, open_guarded, refuse_sealed};
use crate::report::Report;

fn expected_marker(report: &Report) -> String {
    format!(
        "OASRF001\nsummary {}\ncsv {}\njson {}\nprovenance {}\n",
        oa_sha256::to_hex(&oa_sha256::sha256(report.summary.as_bytes())),
        oa_sha256::to_hex(&oa_sha256::sha256(report.csv.as_bytes())),
        oa_sha256::to_hex(&oa_sha256::sha256(report.json.as_bytes())),
        report.provenance_digest,
    )
}

const LOCK_NAME: &str = ".stdds-ref.publish.lock";
const STAGE_ATTEMPTS: u32 = 32;
const OUTPUTS: [(&str, usize); 4] = [
    ("stdds_ref.txt", 0),
    ("stdds_ref.csv", 1),
    ("stdds_ref.json", 2),
    ("stdds_ref.complete", 3),
];

fn lock(g: &Sealed, out: &Path) -> Result<File, String> {
    let path = out.join(LOCK_NAME);
    refuse_sealed(g, &path)?;
    // seal-allow: create-new guarded output lock; existing locks are opened through open_guarded
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let file = open_guarded(g, &path).map_err(|e| format!("{}: {e}", path.display()))?;
            if file
                .metadata()
                .map_err(|e| format!("{}: {e}", path.display()))?
                .len()
                != 0
            {
                return Err(format!("{}: publisher lock is not empty", path.display()));
            }
            file
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    g.verify_open(&path, &file)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(format!(
                "{}: another same-host publisher owns this output directory",
                path.display()
            ))
        }
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(format!("{}: output lock failed: {error}", path.display()))
        }
    }
    Ok(file)
}

fn sync_dir(g: &Sealed, path: &Path) -> Result<(), String> {
    refuse_sealed(g, path)?;
    // seal-allow: open only the guarded output directory for durability sync; verify the opened handle below
    let dir = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    g.verify_open(path, &dir)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    dir.sync_all()
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn stage_dir(g: &Sealed, out: &Path) -> Result<PathBuf, String> {
    for suffix in 0..STAGE_ATTEMPTS {
        let candidate = out.join(format!(".stdds-ref-stage-{}-{suffix}", std::process::id()));
        refuse_sealed(g, &candidate)?;
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{}: {error}", candidate.display())),
        }
    }
    Err("all 32 bounded report staging names already exist; inspect and remove stale stages".into())
}

fn write_staged(g: &Sealed, stage: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let path = stage.join(name);
    let mut file = create_output(g, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.sync_all()
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn publish_one(g: &Sealed, out: &Path, stage: &Path, name: &str) -> Result<(), String> {
    let source = stage.join(name);
    let final_path = out.join(name);
    // Revalidate the staged source before any link can make it visible as a final output.
    let source_file = open_guarded(g, &source).map_err(|e| format!("{}: {e}", source.display()))?;
    refuse_sealed(g, &final_path)?;
    // `hard_link` is an atomic no-replace publication primitive on one filesystem: unlike
    // `rename`, it cannot replace an existing final name. Remove the staging name immediately.
    // seal-allow: atomically link own guarded report staging output to its guarded final name without replacement
    std::fs::hard_link(&source, &final_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "{}: output exists (never overwritten)",
                final_path.display()
            )
        } else {
            format!("{}: {e}", final_path.display())
        }
    })?;
    std::fs::remove_file(&source).map_err(|e| format!("{}: {e}", source.display()))?;
    g.verify_open(&final_path, &source_file)
        .map_err(|e| format!("{}: {e}", final_path.display()))?;
    sync_dir(g, out)?;
    open_guarded(g, &final_path).map_err(|e| format!("{}: {e}", final_path.display()))?;
    Ok(())
}

/// Publishes summary, CSV, JSON and the completion marker last. The marker binds hashes of all
/// three exact report files and a digest of the provenance header. Existing names are never
/// replaced. A crash before the marker leaves an incomplete output set that the next run refuses.
pub fn publish_report(g: &Sealed, out: &Path, report: &Report) -> Result<(), String> {
    if report.completion_marker != expected_marker(report) {
        return Err(
            "completion marker does not bind the exact summary, CSV, JSON, and provenance".into(),
        );
    }
    refuse_sealed(g, out)?;
    let _lock = lock(g, out)?;
    for (name, _) in OUTPUTS {
        let final_path = out.join(name);
        refuse_sealed(g, &final_path)?;
        // This early probe avoids leaving a new stage for an already-known interrupted prefix;
        // atomic hard_link below repeats the no-replace check against races.
        // seal-allow: existence metadata only for a guarded final output name; no report content read
        if std::fs::symlink_metadata(&final_path).is_ok() {
            return Err(format!(
                "{}: output exists (never overwritten)",
                final_path.display()
            ));
        }
    }
    let stage = stage_dir(g, out)?;
    for (name, index) in OUTPUTS {
        let bytes = match index {
            0 => report.summary.as_bytes(),
            1 => report.csv.as_bytes(),
            2 => report.json.as_bytes(),
            _ => report.completion_marker.as_bytes(),
        };
        write_staged(g, &stage, name, bytes)?;
    }
    sync_dir(g, &stage)?;
    for (name, _) in OUTPUTS.iter().take(3) {
        publish_one(g, out, &stage, name)?;
    }
    // Completion marker is the commit record; it becomes visible only after all report files.
    let complete = out.join("stdds_ref.complete");
    // seal-allow: metadata of this attempt's own staged output marker for exact-inode cleanup after a commit error
    let staged_marker = std::fs::symlink_metadata(stage.join("stdds_ref.complete"))
        .map_err(|e| format!("staged completion marker: {e}"))?;
    let committed = publish_one(g, out, &stage, "stdds_ref.complete")
        .and_then(|_| std::fs::remove_dir(&stage).map_err(|e| format!("{}: {e}", stage.display())))
        .and_then(|_| sync_dir(g, out));
    if let Err(error) = committed {
        // An error after the hard-link commit must not leave a successful-looking marker.
        // Remove only the inode created by this attempt, never another existing output.
        // seal-allow: metadata-only check of guarded output marker; remove only this attempt's proven inode
        let cleanup = match std::fs::symlink_metadata(&complete) {
            Ok(meta)
                if meta.file_type().is_file()
                    && meta.dev() == staged_marker.dev()
                    && meta.ino() == staged_marker.ino() =>
            {
                std::fs::remove_file(&complete)
            }
            Ok(_) => Err(std::io::Error::other(
                "completion path changed; it was not removed",
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        let directory_sync = sync_dir(g, out);
        return Err(format!("report completion failed: {error}; marker cleanup={cleanup:?}; directory sync={directory_sync:?}"));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tree(label: &str) -> PathBuf {
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("oa-stdds-ref-publish-{label}-{tick}"));
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn guard(root: &Path) -> Sealed {
        std::fs::write(root.join(".openatca-data-root"), b"openatca-data-root-v1\n").unwrap();
        Sealed::with_list(&format!("root {}\nseal sealed\n", root.display()), None)
    }

    fn report() -> Report {
        let mut report = Report {
            summary: "summary\n".to_string(),
            csv: "file,record\n".to_string(),
            json: "{}\n".to_string(),
            completion_marker: String::new(),
            provenance_digest: "p".to_string(),
        };
        report.completion_marker = expected_marker(&report);
        report
    }

    // REQ: OA-SRF-020
    #[test]
    fn complete_marker_is_published_last_and_existing_reports_are_never_replaced() {
        let root = tree("success");
        let out = root.join("out");
        std::fs::create_dir(&out).unwrap();
        let g = guard(&root);
        let r = report();
        publish_report(&g, &out, &r).unwrap();
        assert_eq!(
            // seal-allow: reads the summary this test generated in its synthetic temporary output.
            std::fs::read(out.join("stdds_ref.txt")).unwrap(),
            r.summary.as_bytes()
        );
        assert_eq!(
            // seal-allow: reads the CSV this test generated in its synthetic temporary output.
            std::fs::read(out.join("stdds_ref.csv")).unwrap(),
            r.csv.as_bytes()
        );
        assert_eq!(
            // seal-allow: reads the JSON this test generated in its synthetic temporary output.
            std::fs::read(out.join("stdds_ref.json")).unwrap(),
            r.json.as_bytes()
        );
        assert_eq!(
            // seal-allow: reads the marker this test generated in its synthetic temporary output.
            std::fs::read(out.join("stdds_ref.complete")).unwrap(),
            r.completion_marker.as_bytes()
        );
        let leftovers =
            // seal-allow: enumerates this test's output directory for leftover staging names only.
            std::fs::read_dir(&out)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".stdds-ref-stage-")
            });
        assert!(!leftovers);
        let err = publish_report(&g, &out, &r).unwrap_err();
        assert!(err.contains("never overwritten"), "{err}");
        assert_eq!(
            // seal-allow: verifies the first publication's synthetic summary remains unchanged.
            std::fs::read(out.join("stdds_ref.txt")).unwrap(),
            r.summary.as_bytes()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_prefix_without_marker_is_refused_and_preserved() {
        let root = tree("prefix");
        let out = root.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(
            out.join("stdds_ref.txt"),
            b"completed prefix from interrupted run",
        )
        .unwrap();
        let g = guard(&root);
        let err = publish_report(&g, &out, &report()).unwrap_err();
        assert!(err.contains("never overwritten"), "{err}");
        assert_eq!(
            // seal-allow: verifies this test's precreated synthetic prefix remains unchanged.
            std::fs::read(out.join("stdds_ref.txt")).unwrap(),
            b"completed prefix from interrupted run"
        );
        assert!(!out.join("stdds_ref.complete").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_rejects_marker_that_does_not_bind_report_bytes() {
        let root = tree("marker");
        let out = root.join("out");
        std::fs::create_dir(&out).unwrap();
        let g = guard(&root);
        let mut r = report();
        r.json.push_str("tampered");
        let err = publish_report(&g, &out, &r).unwrap_err();
        assert!(err.contains("does not bind"), "{err}");
        assert!(!out.join("stdds_ref.complete").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_same_host_publisher_cannot_take_the_lock() {
        let root = tree("lock");
        let out = root.join("out");
        std::fs::create_dir(&out).unwrap();
        let g = guard(&root);
        let first = lock(&g, &out).unwrap();
        assert!(lock(&g, &out).is_err());
        drop(first);
        assert!(lock(&g, &out).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}

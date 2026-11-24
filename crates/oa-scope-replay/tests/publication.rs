//! Restart and concurrent-writer checks for the shell publication API.
#![allow(clippy::unwrap_used, clippy::panic)]

use oa_scope_replay::publication::Publisher;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("oa-scope-publication-{}-{id}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn manifest() -> &'static [u8] {
    b"0000 2026-01-01T00:00:00.000Z aa\n0001 2026-01-01T00:00:01.000Z bb\n0002 2026-01-01T00:00:02.000Z cc\n"
}

// REQ: OA-DSP-075
#[test]
fn aborted_writer_resumes_only_after_exact_prefix_and_finalizes_manifest() {
    let dir = TempDir::new();
    {
        let mut publisher = Publisher::open(&dir.0, 3, false).unwrap();
        publisher.publish_frame(0, b"ppm-frame-0").unwrap();
        publisher.publish_frame(1, b"ppm-frame-1").unwrap();
    } // Simulates a terminated process: completed prefix survives and the OS lock is released.

    let mut resumed = Publisher::open(&dir.0, 3, true).unwrap();
    assert_eq!(resumed.existing_prefix(), 2);
    assert!(resumed
        .publish_frame(2, b"blocked-before-prefix-check")
        .is_err());
    resumed.verify_existing_frame(0, b"ppm-frame-0").unwrap();
    resumed.verify_existing_frame(1, b"ppm-frame-1").unwrap();
    resumed.publish_frame(2, b"ppm-frame-2").unwrap();
    resumed.finish(manifest()).unwrap();
    assert_eq!(std::fs::read(dir.0.join("hashes.txt")).unwrap(), manifest());
}

// REQ: OA-DSP-075
#[test]
fn tampered_or_truncated_prefix_and_partial_manifest_are_refused_without_replacement() {
    let dir = TempDir::new();
    {
        let mut publisher = Publisher::open(&dir.0, 2, false).unwrap();
        publisher.publish_frame(0, b"good-frame").unwrap();
    }
    std::fs::write(dir.0.join("frame-0000.ppm"), b"tampered").unwrap();
    let mut resumed = Publisher::open(&dir.0, 2, true).unwrap();
    assert!(resumed.verify_existing_frame(0, b"good-frame").is_err());
    assert_eq!(
        std::fs::read(dir.0.join("frame-0000.ppm")).unwrap(),
        b"tampered"
    );
    drop(resumed);

    std::fs::write(dir.0.join("frame-0000.ppm"), b"good-frame").unwrap();
    {
        let mut publisher = Publisher::open(&dir.0, 2, true).unwrap();
        publisher.verify_existing_frame(0, b"good-frame").unwrap();
        publisher.publish_frame(1, b"frame-1").unwrap();
    }
    std::fs::write(dir.0.join("hashes.txt"), b"incomplete").unwrap();
    let mut resumed = Publisher::open(&dir.0, 2, true).unwrap();
    assert!(resumed.verify_manifest(manifest()).is_err());
    assert!(resumed.finish(manifest()).is_err());
    assert_eq!(
        std::fs::read(dir.0.join("hashes.txt")).unwrap(),
        b"incomplete"
    );
}

// REQ: OA-DSP-075
#[test]
fn partial_stages_are_discarded_and_linked_frame_stages_recover_atomically() {
    let partial = TempDir::new();
    std::fs::write(
        partial.0.join(".oa-scope-replay.stage-frame-0000.ppm"),
        b"incomplete",
    )
    .unwrap();
    let mut publisher = Publisher::open(&partial.0, 1, true).unwrap();
    assert_eq!(publisher.existing_prefix(), 0);
    publisher.publish_frame(0, b"complete-frame").unwrap();
    publisher.finish(b"frame manifest\n").unwrap();

    let linked = TempDir::new();
    let stage = linked.0.join(".oa-scope-replay.stage-frame-0000.ppm");
    let final_path = linked.0.join("frame-0000.ppm");
    std::fs::write(&stage, b"complete-frame").unwrap();
    std::fs::hard_link(&stage, &final_path).unwrap();
    let mut publisher = Publisher::open(&linked.0, 1, true).unwrap();
    assert!(!stage.exists());
    assert_eq!(publisher.existing_prefix(), 1);
    publisher
        .verify_existing_frame(0, b"complete-frame")
        .unwrap();
    publisher.finish(b"frame manifest\n").unwrap();
}

// REQ: OA-DSP-075
#[test]
fn partial_and_linked_manifest_stages_recover_without_rewriting_final_files() {
    let partial = TempDir::new();
    {
        let mut publisher = Publisher::open(&partial.0, 1, false).unwrap();
        publisher.publish_frame(0, b"complete-frame").unwrap();
    }
    std::fs::write(
        partial.0.join(".oa-scope-replay.stage-hashes.txt"),
        b"partial manifest",
    )
    .unwrap();
    let mut publisher = Publisher::open(&partial.0, 1, true).unwrap();
    publisher
        .verify_existing_frame(0, b"complete-frame")
        .unwrap();
    publisher.finish(b"frame manifest\n").unwrap();

    let linked = TempDir::new();
    {
        let mut publisher = Publisher::open(&linked.0, 1, false).unwrap();
        publisher.publish_frame(0, b"complete-frame").unwrap();
    }
    let stage = linked.0.join(".oa-scope-replay.stage-hashes.txt");
    let final_path = linked.0.join("hashes.txt");
    std::fs::write(&stage, b"frame manifest\n").unwrap();
    std::fs::hard_link(&stage, &final_path).unwrap();
    let mut publisher = Publisher::open(&linked.0, 1, true).unwrap();
    assert!(!stage.exists());
    publisher
        .verify_existing_frame(0, b"complete-frame")
        .unwrap();
    publisher.verify_manifest(b"frame manifest\n").unwrap();
    publisher.finish(b"frame manifest\n").unwrap();
}

// REQ: OA-DSP-075
#[test]
fn unmatched_or_extra_link_staging_files_are_refused_without_cleanup() {
    let dir = TempDir::new();
    let stage = dir.0.join(".oa-scope-replay.stage-frame-0000.ppm");
    let final_path = dir.0.join("frame-0000.ppm");
    std::fs::write(&stage, b"stage").unwrap();
    std::fs::write(&final_path, b"different final inode").unwrap();
    assert!(Publisher::open(&dir.0, 1, true).is_err());
    assert_eq!(std::fs::read(&stage).unwrap(), b"stage");
    assert_eq!(
        std::fs::read(&final_path).unwrap(),
        b"different final inode"
    );

    std::fs::remove_file(&final_path).unwrap();
    std::fs::remove_file(&stage).unwrap();
    std::fs::write(&stage, b"stage").unwrap();
    let extra_link = dir.0.join("extra-link");
    std::fs::hard_link(&stage, &extra_link).unwrap();
    std::fs::hard_link(&stage, &final_path).unwrap();
    assert!(Publisher::open(&dir.0, 1, true).is_err());
    assert_eq!(std::fs::read(&stage).unwrap(), b"stage");
    assert!(final_path.exists() && extra_link.exists());
}

// REQ: OA-DSP-076
#[test]
fn publisher_lock_excludes_a_second_writer_and_releases_after_abort() {
    let dir = TempDir::new();
    let first = Publisher::open(&dir.0, 1, false).unwrap();
    assert!(Publisher::open(&dir.0, 1, true).is_err());
    drop(first);
    let second = Publisher::open(&dir.0, 1, true).unwrap();
    assert_eq!(second.existing_prefix(), 0);
}

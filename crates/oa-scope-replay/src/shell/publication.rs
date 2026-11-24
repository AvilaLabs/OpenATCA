//! Bounded, restartable publication of deterministic frame files (**shell**, B1, B3, C6, C7).
//!
//! A publisher owns one advisory exclusive file lock for its lifetime. Existing output is
//! accepted only as an exact ordered frame prefix (plus, after all frames, an exact complete
//! hashes manifest). The caller supplies recomputed frame bytes before advancing the prefix.
//! This is publication recovery: callers still recompute from their original inputs and verify
//! their provenance witness; this module stores no process or tracker state.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Largest PPM payload accepted by this publisher: three bytes per maximum display pixel plus a
/// bounded header. This mirrors oa-display's raster pixel cap.
pub const MAX_FRAME_BYTES: usize = ((1u64 << 26) * 3 + 64) as usize;
pub const MAX_DIRECTORY_ENTRIES: u64 = 10_003; // MAX_FRAMES + manifest + lock + one stage.
const LOCK_NAME: &str = ".oa-scope-replay.publisher.lock";
const STAGE_PREFIX: &str = ".oa-scope-replay.stage-frame-";
const STAGE_MANIFEST: &str = ".oa-scope-replay.stage-hashes.txt";
const STAGE_HASHES_ONLY: &str = ".oa-scope-replay.stage-hashes-only.txt";

/// Lock shared by every CLI writer, including overwrite and hashes-only publication.
pub struct OutputLock {
    dir: PathBuf,
    file: File,
    frame_count: u64,
    next_frame: u64,
}

impl OutputLock {
    pub fn acquire(dir: &Path, frame_count: u64) -> std::io::Result<Self> {
        Self::acquire_inner(dir, frame_count, true)
    }

    pub(crate) fn acquire_for_overwrite(dir: &Path, frame_count: u64) -> std::io::Result<Self> {
        Self::acquire_inner(dir, frame_count, false)
    }

    fn acquire_inner(dir: &Path, frame_count: u64, recover: bool) -> std::io::Result<Self> {
        if frame_count == 0 || frame_count > 10_000 {
            return Err(fail("frame count is outside the publisher limit"));
        }
        oa_sealed::refuse(dir).map_err(std::io::Error::other)?;
        fs::create_dir_all(dir)?;
        let file = lock_file(dir)?;
        let lock = Self {
            dir: dir.to_path_buf(),
            file,
            frame_count,
            next_frame: 0,
        };
        if recover {
            lock.recover_stages()?;
        }
        Ok(lock)
    }

    pub(crate) fn protects(&self, dir: &Path) -> bool {
        self.dir == dir && self.file.metadata().is_ok()
    }

    pub(crate) fn recover_stages(&self) -> std::io::Result<()> {
        if !self.protects(&self.dir) {
            return Err(fail("stage recovery requires the active output lock"));
        }
        recover_stages(&self.dir, self.frame_count)
    }

    pub(crate) fn publish_frame(&mut self, index: u64, bytes: &[u8]) -> std::io::Result<()> {
        if index != self.next_frame || index >= self.frame_count || bytes.len() > MAX_FRAME_BYTES {
            return Err(fail(
                "frame publication is out of sequence or exceeds its size limit",
            ));
        }
        publish_staged(
            &self.dir,
            &stage_frame_name(index),
            &format!("frame-{index:04}.ppm"),
            bytes,
            MAX_FRAME_BYTES,
        )?;
        self.next_frame += 1;
        Ok(())
    }

    pub(crate) fn finish_frames(&mut self, hashes: &[u8]) -> std::io::Result<()> {
        if self.next_frame != self.frame_count {
            return Err(fail(
                "cannot finalize before all bounded frames are published",
            ));
        }
        publish_manifest_stage(&self.dir, STAGE_MANIFEST, hashes)?;
        Ok(())
    }

    pub(crate) fn finish_hashes_only(&mut self, hashes: &[u8]) -> std::io::Result<()> {
        if self.next_frame != 0 {
            return Err(fail(
                "hashes-only publication cannot include PPM frame writes",
            ));
        }
        publish_manifest_stage(&self.dir, STAGE_HASHES_ONLY, hashes)?;
        Ok(())
    }
}

/// Exclusive output publisher. Drop releases the OS lock, including after process termination.
pub struct Publisher {
    dir: PathBuf,
    frame_count: u64,
    existing_prefix: u64,
    next_suffix: u64,
    lock: OutputLock,
    hashes_present: bool,
    verified_prefix: u64,
}

fn fail(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

fn frame_name(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("frame-")?.strip_suffix(".ppm")?;
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn lock_file(dir: &Path) -> std::io::Result<File> {
    let path = dir.join(LOCK_NAME);
    oa_sealed::refuse(&path).map_err(std::io::Error::other)?;
    // seal-allow: create-new guarded output lock; existing locks use the guarded regular-file reader
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let (file, size) = oa_sealed::open_regular(&path)?;
            if size != 0 {
                return Err(fail("publisher lock file is not empty"));
            }
            file
        }
        Err(error) => return Err(error),
    };
    oa_sealed::Sealed::production().verify_open(&path, &file)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(fail("another publisher owns this output directory"));
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }
    Ok(file)
}

fn stage_frame_name(index: u64) -> String {
    format!("{STAGE_PREFIX}{index:04}.ppm")
}

fn stage_frame_index(name: &str) -> Option<u64> {
    let digits = name.strip_prefix(STAGE_PREFIX)?.strip_suffix(".ppm")?;
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn regular_metadata(path: &Path) -> std::io::Result<Option<Metadata>> {
    // seal-allow: metadata only for derived output staging/final names; proves linked inode identity before guarded reads
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(Some(meta)),
        Ok(_) => Err(fail(format!("{} is not a regular file", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reconciles only this module's exact staging names, using metadata and directory entries only.
/// In particular, linked final/stage pairs are proven to be the same inode before stage unlink;
/// the sealed guarded reader is used later, after the link count returns to one.
fn recover_stages(dir: &Path, frame_count: u64) -> std::io::Result<()> {
    let mut seen = 0u64;
    let mut frames = vec![false; 10_000];
    let mut manifest_present = false;
    let mut stages: Vec<(String, Option<u64>, bool)> = Vec::new();
    // seal-allow: bounded name inventory in the guarded locked output directory; no input content is read
    for entry in fs::read_dir(dir)? {
        seen = seen
            .checked_add(1)
            .ok_or_else(|| fail("output directory entry count overflow"))?;
        if seen > MAX_DIRECTORY_ENTRIES {
            return Err(fail("output directory exceeds its entry limit"));
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| fail("output directory has a non-UTF-8 name"))?;
        if let Some(index) = stage_frame_index(name) {
            if index >= 10_000 {
                return Err(fail("staging frame index is outside the publisher limit"));
            }
            stages.push((name.to_owned(), Some(index), false));
        } else if name == STAGE_MANIFEST {
            stages.push((name.to_owned(), None, false));
        } else if name == STAGE_HASHES_ONLY {
            stages.push((name.to_owned(), None, true));
        } else if name == "hashes.txt" {
            manifest_present = true;
        } else if let Some(index) = frame_name(name) {
            if index >= 10_000 {
                return Err(fail("frame index is outside the publisher limit"));
            }
            let kind = entry.file_type()?;
            if !kind.is_file() {
                return Err(fail("published frame is not a regular file"));
            }
            if let Some(slot) = frames.get_mut(index as usize) {
                *slot = true;
            }
        }
    }
    if stages.len() > 1 {
        return Err(fail("multiple publisher staging files are present"));
    }
    let Some((stage_name, stage_index, hashes_only)) = stages.pop() else {
        return Ok(());
    };
    let stage = dir.join(&stage_name);
    let stage_meta = regular_metadata(&stage)?.ok_or_else(|| fail("staging file disappeared"))?;
    let final_name =
        stage_index.map_or_else(|| "hashes.txt".to_owned(), |i| format!("frame-{i:04}.ppm"));
    let final_path = dir.join(final_name);
    let final_meta = regular_metadata(&final_path)?;
    match stage_index {
        Some(index) => {
            let i = index as usize;
            if frames
                .iter()
                .skip(i.saturating_add(1))
                .any(|present| *present)
                || frames.iter().take(i).any(|present| !*present)
                || manifest_present
            {
                return Err(fail("staging frame is outside the current output prefix"));
            }
            match final_meta.as_ref() {
                None if !frames.get(i).copied().unwrap_or(false) && stage_meta.nlink() == 1 => {}
                Some(final_meta)
                    if frames.get(i).copied().unwrap_or(false)
                        && stage_meta.nlink() == 2
                        && final_meta.nlink() == 2
                        && stage_meta.dev() == final_meta.dev()
                        && stage_meta.ino() == final_meta.ino() => {}
                _ => return Err(fail("staging frame does not have a recoverable link state")),
            }
        }
        None => {
            let prefix = frames.iter().take_while(|present| **present).count();
            if !hashes_only && prefix != frame_count as usize {
                return Err(fail("staging manifest exists before all frames"));
            }
            if hashes_only && frames.iter().any(|present| *present) {
                return Err(fail("hashes-only staging conflicts with PPM frames"));
            }
            match final_meta.as_ref() {
                None if !manifest_present && stage_meta.nlink() == 1 => {}
                Some(final_meta)
                    if manifest_present
                        && stage_meta.nlink() == 2
                        && final_meta.nlink() == 2
                        && stage_meta.dev() == final_meta.dev()
                        && stage_meta.ino() == final_meta.ino() => {}
                _ => {
                    return Err(fail(
                        "staging manifest does not have a recoverable link state",
                    ))
                }
            }
        }
    }
    oa_sealed::refuse(&stage).map_err(std::io::Error::other)?;
    if final_meta.is_some() {
        oa_sealed::refuse(&final_path).map_err(std::io::Error::other)?;
    }
    fs::remove_file(&stage)?;
    oa_sealed::open(dir)?.sync_all()?;
    Ok(())
}

fn same_link_pair(stage: &Path, final_path: &Path) -> std::io::Result<()> {
    let stage_meta = regular_metadata(stage)?.ok_or_else(|| fail("staging file disappeared"))?;
    let final_meta =
        regular_metadata(final_path)?.ok_or_else(|| fail("published file disappeared"))?;
    if stage_meta.nlink() != 2
        || final_meta.nlink() != 2
        || stage_meta.dev() != final_meta.dev()
        || stage_meta.ino() != final_meta.ino()
    {
        return Err(fail(
            "staging and published files are not the same two-link inode",
        ));
    }
    oa_sealed::refuse(stage).map_err(std::io::Error::other)?;
    oa_sealed::refuse(final_path).map_err(std::io::Error::other)?;
    Ok(())
}

fn publish_staged(
    dir: &Path,
    stage_name: &str,
    final_name: &str,
    bytes: &[u8],
    limit: usize,
) -> std::io::Result<()> {
    if bytes.len() > limit {
        return Err(fail("published file exceeds its size limit"));
    }
    let stage = dir.join(stage_name);
    let final_path = dir.join(final_name);
    oa_sealed::refuse(&stage).map_err(std::io::Error::other)?;
    oa_sealed::refuse(&final_path).map_err(std::io::Error::other)?;
    // seal-allow: create-new write to a guarded output staging name, never an input read
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;
    let guard = oa_sealed::Sealed::production();
    guard.verify_open(&stage, &file)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    guard.verify_open(&stage, &file)?;
    // seal-allow: atomic no-replace link between guarded output names; exact inode pair verified before unlink
    fs::hard_link(&stage, &final_path)?;
    oa_sealed::open(dir)?.sync_all()?;
    same_link_pair(&stage, &final_path)?;
    drop(file);
    fs::remove_file(&stage)?;
    oa_sealed::open(dir)?.sync_all()?;
    Ok(())
}

fn publish_manifest_stage(dir: &Path, stage_name: &str, hashes: &[u8]) -> std::io::Result<()> {
    if hashes.len() > 1_000_000 {
        return Err(fail("hashes manifest exceeds its size limit"));
    }
    publish_staged(dir, stage_name, "hashes.txt", hashes, 1_000_000)
}

pub(crate) fn publish_hashes_only_locked(
    lock: &mut OutputLock,
    hashes: &[u8],
) -> std::io::Result<()> {
    if !lock.protects(&lock.dir) {
        return Err(fail("manifest publication is outside the locked output"));
    }
    lock.finish_hashes_only(hashes)
}

impl Publisher {
    /// Opens a publisher. With `resume`, the directory may contain only an exact completed frame
    /// prefix and, if every frame exists, an optional completed manifest. Without `resume`, it
    /// must contain no published output. The lock entry is retained as an empty file and is safe
    /// to reuse after a process exits; the OS lock itself is released on process death.
    pub fn open(dir: &Path, frame_count: u64, resume: bool) -> std::io::Result<Self> {
        let lock = OutputLock::acquire(dir, frame_count)?;
        Self::open_locked(dir, frame_count, resume, lock)
    }

    pub fn open_locked(
        dir: &Path,
        frame_count: u64,
        resume: bool,
        lock: OutputLock,
    ) -> std::io::Result<Self> {
        Self::open_locked_allow_unrelated(dir, frame_count, resume, lock, false)
    }

    pub fn open_locked_allow_unrelated(
        dir: &Path,
        frame_count: u64,
        resume: bool,
        lock: OutputLock,
        allow_unrelated: bool,
    ) -> std::io::Result<Self> {
        if frame_count == 0 || frame_count > 10_000 || !lock.protects(dir) {
            return Err(fail("publisher does not own this bounded output directory"));
        }
        let mut present = vec![false; frame_count as usize];
        let mut hashes_present = false;
        let mut seen = 0u64;
        // seal-allow: bounded output-name inventory under the verified output-directory lock
        for entry in std::fs::read_dir(dir)? {
            seen = seen
                .checked_add(1)
                .ok_or_else(|| fail("output directory entry count overflow"))?;
            if seen > MAX_DIRECTORY_ENTRIES {
                return Err(fail("output directory exceeds its entry limit"));
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| fail("output directory has a non-UTF-8 name"))?;
            if name == LOCK_NAME {
                let kind = entry.file_type()?;
                if !kind.is_file() {
                    return Err(fail("publisher lock path is not a regular file"));
                }
                continue;
            }
            let kind = entry.file_type()?;
            if !kind.is_file() {
                return Err(fail(format!("unexpected non-file output entry {name}")));
            }
            if name == "hashes.txt" {
                hashes_present = true;
                continue;
            }
            let Some(index) = frame_name(name) else {
                if allow_unrelated && !resume {
                    continue;
                }
                return Err(fail(format!("unexpected output entry {name}")));
            };
            if index >= frame_count {
                return Err(fail(format!("frame index {index} is outside this run")));
            }
            let slot = present
                .get_mut(index as usize)
                .ok_or_else(|| fail("frame index is outside the bounded table"))?;
            if *slot {
                return Err(fail(format!("duplicate frame index {index}")));
            }
            *slot = true;
        }
        let prefix = present.iter().take_while(|&&exists| exists).count() as u64;
        if present.iter().skip(prefix as usize).any(|&exists| exists) {
            return Err(fail("published frames are not an exact ordered prefix"));
        }
        if !resume && (prefix != 0 || hashes_present) {
            return Err(fail(
                "output already contains files; resume was not requested",
            ));
        }
        if hashes_present && prefix != frame_count {
            return Err(fail(
                "hashes manifest exists before the full frame sequence",
            ));
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            frame_count,
            existing_prefix: prefix,
            next_suffix: prefix,
            lock,
            hashes_present,
            verified_prefix: 0,
        })
    }

    /// Number of frames already present. The caller must recompute and verify these against its
    /// provenance witness before publishing any suffix.
    pub fn existing_prefix(&self) -> u64 {
        self.existing_prefix
    }

    /// Compares a bounded recomputed frame against an existing prefix member without following
    /// links. No output file is changed.
    pub fn verify_existing_frame(&mut self, index: u64, expected: &[u8]) -> std::io::Result<()> {
        if index != self.verified_prefix
            || index >= self.existing_prefix
            || expected.len() > MAX_FRAME_BYTES
        {
            return Err(fail(
                "existing frame verification is outside the prefix or size limit",
            ));
        }
        let path = self.dir.join(format!("frame-{index:04}.ppm"));
        let (file, size) = oa_sealed::open_regular(&path)?;
        if size != expected.len() as u64 {
            return Err(fail(format!(
                "published frame {index} is truncated or has the wrong size"
            )));
        }
        let mut bytes = Vec::with_capacity(expected.len());
        file.take(expected.len() as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.as_slice() != expected {
            return Err(fail(format!(
                "published frame {index} differs from recomputed bytes"
            )));
        }
        self.verified_prefix += 1;
        Ok(())
    }

    /// Publishes the next missing frame with create-new and durable file contents. Existing
    /// prefix members are verified by `verify_existing_frame`, never overwritten.
    pub fn publish_frame(&mut self, index: u64, bytes: &[u8]) -> std::io::Result<()> {
        if self.verified_prefix != self.existing_prefix
            || index != self.next_suffix
            || index >= self.frame_count
            || bytes.len() > MAX_FRAME_BYTES
        {
            return Err(fail(
                "frame publication is out of sequence or exceeds its size limit",
            ));
        }
        let name = format!("frame-{index:04}.ppm");
        publish_staged(
            &self.dir,
            &stage_frame_name(index),
            &name,
            bytes,
            MAX_FRAME_BYTES,
        )?;
        self.next_suffix += 1;
        Ok(())
    }

    /// Checks a pre-existing manifest before any missing frame is published.
    pub fn verify_manifest(&self, hashes: &[u8]) -> std::io::Result<()> {
        if hashes.len() > 1_000_000 {
            return Err(fail("hashes manifest exceeds its size limit"));
        }
        if !self.hashes_present {
            return Ok(());
        }
        if self.existing_prefix != self.frame_count {
            return Err(fail(
                "hashes manifest exists before the full frame sequence",
            ));
        }
        let path = self.dir.join("hashes.txt");
        let (file, size) = oa_sealed::open_regular(&path)?;
        if size != hashes.len() as u64 {
            return Err(fail(
                "existing hashes manifest is truncated or has the wrong size",
            ));
        }
        let mut bytes = Vec::with_capacity(hashes.len());
        file.take(hashes.len() as u64 + 1).read_to_end(&mut bytes)?;
        if bytes != hashes {
            return Err(fail(
                "existing hashes manifest differs from recomputed sequence",
            ));
        }
        Ok(())
    }

    /// Creates the complete hashes manifest after every suffix frame is durable, or verifies an
    /// already-published complete manifest byte-for-byte. The manifest is never overwritten.
    pub fn finish(&mut self, hashes: &[u8]) -> std::io::Result<()> {
        if self.verified_prefix != self.existing_prefix
            || self.next_suffix != self.frame_count
            || hashes.len() > 1_000_000
        {
            return Err(fail(
                "cannot finalize before all bounded frames are published",
            ));
        }
        if self.hashes_present {
            self.verify_manifest(hashes)?;
        } else {
            publish_staged(&self.dir, STAGE_MANIFEST, "hashes.txt", hashes, 1_000_000)?;
            self.hashes_present = true;
        }
        oa_sealed::open(&self.dir)?.sync_all()?;
        let _ = &self.lock;
        Ok(())
    }
}

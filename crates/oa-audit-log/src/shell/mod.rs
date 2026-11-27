//! Durable bounded file operations. One fixed-size record is appended per action.
use crate::encoding::{chain, checkpoint, decode, encode, MAX_RECORDS, RECORD_BYTES};
use oa_audit_core::{
    parse_principal_registry, receipt_claim, validate_event, AuditError, AuditReceipt, Checkpoint,
    Digest, Event, PrincipalRegistry, Record, RegistryError, MAX_PRINCIPAL_REGISTRY_BYTES,
};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum LogError {
    Io(io::Error),
    Audit(AuditError),
    BrokenRecord { sequence: u64, error: AuditError },
    CheckpointMismatch,
    Truncated { sequence: u64 },
}

#[derive(Debug)]
pub enum CommitError<E> {
    Audit(LogError),
    Action(E),
}

#[derive(Debug)]
pub enum RegistryLoadError {
    Io(io::Error),
    TooLarge,
    DescriptorChanged,
    DigestMismatch,
    InvalidRegistry(RegistryError),
}

impl std::fmt::Display for RegistryLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "principal registry I/O failed: {error}"),
            Self::TooLarge => write!(f, "principal registry exceeds its byte limit"),
            Self::DescriptorChanged => write!(f, "principal registry changed while being read"),
            Self::DigestMismatch => {
                write!(f, "principal registry does not match its pinned digest")
            }
            Self::InvalidRegistry(error) => write!(f, "principal registry is invalid: {error:?}"),
        }
    }
}

impl std::error::Error for RegistryLoadError {}

/// Load a bounded local principal registry from a production-guarded regular file.
///
/// `expected_digest` must come from an independent trusted configuration/pin. It is never read
/// from the registry bytes themselves. The same opened descriptor is size-checked, read, and
/// checked again before its exact bytes are hashed and parsed.
pub fn load_principal_registry(
    path: &Path,
    expected_digest: Digest,
) -> Result<PrincipalRegistry, RegistryLoadError> {
    let (mut file, _) = oa_sealed::Sealed::production()
        .open_regular(path)
        .map_err(RegistryLoadError::Io)?;
    let before = file.metadata().map_err(RegistryLoadError::Io)?;
    let len = usize::try_from(before.len()).map_err(|_| RegistryLoadError::TooLarge)?;
    if len > MAX_PRINCIPAL_REGISTRY_BYTES {
        return Err(RegistryLoadError::TooLarge);
    }

    let mut bytes = [0_u8; MAX_PRINCIPAL_REGISTRY_BYTES];
    let input = bytes.get_mut(..len).ok_or(RegistryLoadError::TooLarge)?;
    file.read_exact(input).map_err(RegistryLoadError::Io)?;
    let mut extra = [0_u8; 1];
    if file.read(&mut extra).map_err(RegistryLoadError::Io)? != 0
        || file.metadata().map_err(RegistryLoadError::Io)?.len() != before.len()
    {
        return Err(RegistryLoadError::DescriptorChanged);
    }
    let exact_bytes = bytes.get(..len).ok_or(RegistryLoadError::TooLarge)?;
    let actual_digest = oa_sha256::core::sha256::sha256(exact_bytes);
    if actual_digest != expected_digest {
        return Err(RegistryLoadError::DigestMismatch);
    }
    parse_principal_registry(exact_bytes).map_err(RegistryLoadError::InvalidRegistry)
}

impl From<io::Error> for LogError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<AuditError> for LogError {
    fn from(value: AuditError) -> Self {
        Self::Audit(value)
    }
}

/// Verify every entry and require the supplied trusted tail/count, including for truncated logs.
pub fn verify(path: &Path, expected: Checkpoint) -> Result<Checkpoint, LogError> {
    if expected.count > MAX_RECORDS {
        return Err(LogError::Audit(AuditError::LogLimitReached));
    }
    let mut file = open_log_read(path)?;
    verify_file(&mut file, expected, &mut |_| {})
}

fn verify_file(
    file: &mut File,
    expected: Checkpoint,
    visitor: &mut impl FnMut(Record),
) -> Result<Checkpoint, LogError> {
    let mut actual = oa_audit_core::genesis();
    loop {
        let mut bytes = [0u8; RECORD_BYTES];
        let mut read = 0usize;
        while read < RECORD_BYTES {
            match file.read(bytes.get_mut(read..).ok_or(LogError::CheckpointMismatch)?)? {
                0 if read == 0 => {
                    return if actual == expected {
                        Ok(actual)
                    } else if actual.count < expected.count {
                        Err(LogError::Truncated {
                            sequence: actual.count.saturating_add(1),
                        })
                    } else {
                        Err(LogError::CheckpointMismatch)
                    };
                }
                0 => {
                    return Err(LogError::Truncated {
                        sequence: actual.count.saturating_add(1),
                    });
                }
                count => {
                    read = read
                        .checked_add(count)
                        .ok_or(LogError::CheckpointMismatch)?
                }
            }
        }
        let sequence = actual.count.saturating_add(1);
        let record = decode(&bytes).map_err(|error| LogError::BrokenRecord { sequence, error })?;
        let next = chain(actual, record.event)
            .map_err(|error| LogError::BrokenRecord { sequence, error })?;
        if record.sequence != next.sequence
            || record.previous != actual.tail
            || record.digest != next.digest
        {
            return Err(LogError::BrokenRecord {
                sequence,
                error: AuditError::InvalidChain,
            });
        }
        actual = checkpoint(&record);
        if actual.count > expected.count {
            return Err(LogError::CheckpointMismatch);
        }
        visitor(record);
    }
}

/// Durably append one validated event only when the current log matches its trusted checkpoint.
pub fn append(path: &Path, expected: Checkpoint, event: Event) -> Result<Checkpoint, LogError> {
    let record = append_record(path, expected, event)?;
    Ok(checkpoint(&record))
}

/// Append an action and return an opaque receipt for a core commit function to consume.
pub fn append_receipt(
    path: &Path,
    expected: Checkpoint,
    event: Event,
) -> Result<(Checkpoint, AuditReceipt), LogError> {
    let record = append_record(path, expected, event)?;
    Ok((
        checkpoint(&record),
        receipt_claim(
            record.sequence,
            record.digest,
            record.previous,
            record.event,
        ),
    ))
}

fn append_record(path: &Path, expected: Checkpoint, event: Event) -> Result<Record, LogError> {
    if expected.count > MAX_RECORDS {
        return Err(LogError::Audit(AuditError::LogLimitReached));
    }
    // Refuse invalid principals/roles/actions before opening or creating any filesystem entry.
    validate_event(&event)?;
    let parent = path
        .parent()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = open_directory(parent)?;
    let mut file = open_log_append_at(path, &directory)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "audit log is locked").into());
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    verify_file(&mut file, expected, &mut |_| {})?;
    let record = chain(expected, event)?;
    let mut bytes = [0u8; RECORD_BYTES];
    encode(&record, &mut bytes)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    directory.sync_all()?;
    Ok(record)
}

/// Durably write the intent before invoking a caller's commit operation. An audit error never
/// invokes `commit`; a later action error leaves the intent in the log for reconciliation.
pub fn append_before_commit<T, E>(
    path: &Path,
    expected: Checkpoint,
    event: Event,
    commit: impl FnOnce() -> Result<T, E>,
) -> Result<(Checkpoint, T), CommitError<E>> {
    let checkpoint = append(path, expected, event).map_err(CommitError::Audit)?;
    let value = commit().map_err(CommitError::Action)?;
    Ok((checkpoint, value))
}

/// Read and verify one next record, returning the first broken/truncated entry as an error.
pub fn read_records(
    path: &Path,
    expected: Checkpoint,
    visitor: &mut impl FnMut(Record),
) -> Result<Checkpoint, LogError> {
    if expected.count > MAX_RECORDS {
        return Err(LogError::Audit(AuditError::LogLimitReached));
    }
    let mut file = open_log_read(path)?;
    verify_file(&mut file, expected, visitor)
}

fn check_seal(guard: &oa_sealed::Sealed, path: &Path) -> io::Result<()> {
    guard
        .check(path)
        .map_err(|refusal| io::Error::other(refusal.why))
}

fn open_log_read(path: &Path) -> io::Result<File> {
    let (file, _) = oa_sealed::Sealed::production().open_regular(path)?;
    Ok(file)
}

fn open_log_append_at(path: &Path, parent: &File) -> io::Result<File> {
    let guard = oa_sealed::Sealed::production();
    check_seal(guard, path)?;
    let anchored = anchored_child(
        parent,
        path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "audit log path needs a file name",
            )
        })?,
    )?;
    check_seal(guard, &anchored)?;
    // seal-allow: guarded anchored create/append; verify the same descriptor before writing.
    let mut options = OpenOptions::new();
    options.read(true).create(true).append(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(linux_o_nofollow_flag()?);
    }
    let file = options.open(&anchored)?;
    guard.verify_open(&anchored, &file)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(
            "audit log destination is not a regular file",
        ));
    }
    Ok(file)
}

/// Returns the Linux UAPI `O_NOFOLLOW` value for the target architecture.
/// The asm-generic value is overridden by several architecture-specific UAPI headers.
/// Rust reports the x32 ABI with `target_arch = "x86_64"`.
#[cfg(target_os = "linux")]
fn linux_o_nofollow_flag() -> io::Result<i32> {
    #[cfg(any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "m68k",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    ))]
    {
        Ok(0o100000)
    }

    #[cfg(any(
        target_arch = "csky",
        target_arch = "hexagon",
        target_arch = "loongarch64",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "riscv32",
        target_arch = "riscv64",
        target_arch = "s390x",
        target_arch = "sparc",
        target_arch = "sparc64",
        target_arch = "x86",
        target_arch = "x86_64"
    ))]
    {
        Ok(0o400000)
    }

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "m68k",
        target_arch = "powerpc",
        target_arch = "powerpc64",
        target_arch = "csky",
        target_arch = "hexagon",
        target_arch = "loongarch64",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "riscv32",
        target_arch = "riscv64",
        target_arch = "s390x",
        target_arch = "sparc",
        target_arch = "sparc64",
        target_arch = "x86",
        target_arch = "x86_64"
    )))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "audit append does not know the target Linux O_NOFOLLOW flag",
        ))
    }
}

#[cfg(target_os = "linux")]
fn anchored_child(parent: &File, name: &std::ffi::OsStr) -> io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    Ok(PathBuf::from(format!("/proc/self/fd/{}/", parent.as_raw_fd())).join(name))
}

#[cfg(not(target_os = "linux"))]
fn anchored_child(_parent: &File, _name: &std::ffi::OsStr) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "durable audit append requires Linux /proc/self/fd directory anchoring",
    ))
}

fn open_directory(path: &Path) -> io::Result<File> {
    let guard = oa_sealed::Sealed::production();
    check_seal(guard, path)?;
    // seal-allow: pin the guarded parent directory descriptor for anchored append and fsync.
    let file = File::open(path)?;
    guard.verify_open(path, &file)?;
    if !file.metadata()?.is_dir() {
        return Err(io::Error::other("audit log parent is not a directory"));
    }
    Ok(file)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use oa_audit_core::{details, target, Action, Role, DETAILS_CAPACITY};

    fn event(action: Action) -> Event {
        let (target, target_len) = target(b"svc-1").unwrap();
        let (details, details_len) = details(b"approved").unwrap();
        Event {
            principal: 4,
            role: Role::Configure,
            action,
            target,
            target_len,
            time_ns: 99,
            details,
            details_len,
        }
    }

    fn path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oa-audit-{tag}-{}", std::process::id()))
    }

    // REQ: OA-AUDIT-008
    #[test]
    fn principal_registry_loader_checks_external_pin_and_parses_exact_opened_bytes() {
        let file = path("principals");
        let _ = std::fs::remove_file(&file);
        let bytes = b"OACERT-LOCAL-PRINCIPALS-V2\n7\tconfigure\tenabled\n11\toperate\tdisabled\n";
        std::fs::write(&file, bytes).unwrap();
        let pin = oa_sha256::core::sha256::sha256(bytes);
        let registry = load_principal_registry(&file, pin).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(matches!(
            load_principal_registry(&file, [0; 32]),
            Err(RegistryLoadError::DigestMismatch)
        ));
        let _ = std::fs::remove_file(file);
    }

    // REQ: OA-AUDIT-008
    #[test]
    fn principal_registry_loader_refuses_oversize_and_sealed_paths() {
        let file = path("principals-oversize");
        let _ = std::fs::remove_file(&file);
        std::fs::write(&file, vec![b'X'; MAX_PRINCIPAL_REGISTRY_BYTES + 1]).unwrap();
        assert!(matches!(
            load_principal_registry(&file, [0; 32]),
            Err(RegistryLoadError::TooLarge)
        ));

        let sealed = std::env::temp_dir()
            .join(format!("oa-audit-sealed-registry-{}", std::process::id()))
            .join("scds")
            .join("rec-20261119")
            .join("day2-synthetic")
            .join("principals.txt");
        assert!(load_principal_registry(&sealed, [0; 32]).is_err());
        assert!(!sealed.exists());
        let _ = std::fs::remove_file(file);
    }

    // REQ: OA-AUDIT-001, OA-AUDIT-003, OA-AUDIT-004
    #[test]
    fn append_verifies_trusted_tail_and_detects_deletion_reorder_and_tamper() {
        let file = path("chain");
        let _ = std::fs::remove_file(&file);
        let first = append(
            &file,
            oa_audit_core::genesis(),
            event(Action::Certification),
        )
        .unwrap();
        let second = append(&file, first, event(Action::QuarantineRelease)).unwrap();
        assert_eq!(verify(&file, second).unwrap(), second);
        let data = std::fs::read(&file).unwrap();
        assert_eq!(data.len(), RECORD_BYTES * 2);

        std::fs::write(&file, data.get(..RECORD_BYTES).unwrap()).unwrap();
        assert!(matches!(
            verify(&file, second),
            Err(LogError::Truncated { sequence: 2 })
        ));

        let mut reordered = Vec::with_capacity(data.len());
        reordered.extend_from_slice(data.get(RECORD_BYTES..).unwrap());
        reordered.extend_from_slice(data.get(..RECORD_BYTES).unwrap());
        std::fs::write(&file, reordered).unwrap();
        assert!(verify(&file, second).is_err());

        let mut tampered = data;
        if let Some(byte) = tampered.get_mut(40) {
            *byte ^= 1;
        }
        std::fs::write(&file, tampered).unwrap();
        assert!(verify(&file, second).is_err());
        let _ = std::fs::remove_file(file);
    }

    // REQ: OA-AUDIT-004
    #[test]
    fn truncated_record_and_missing_expected_tail_fail_closed() {
        let file = path("truncated");
        let _ = std::fs::remove_file(&file);
        let checkpoint = append(
            &file,
            oa_audit_core::genesis(),
            event(Action::Certification),
        )
        .unwrap();
        let data = std::fs::read(&file).unwrap();
        std::fs::write(&file, data.get(..(RECORD_BYTES - 1)).unwrap()).unwrap();
        assert!(matches!(
            verify(&file, checkpoint),
            Err(LogError::Truncated { .. })
        ));
        assert_eq!(DETAILS_CAPACITY, 20_480);
        let _ = std::fs::remove_file(file);
    }

    // REQ: OA-AUDIT-003, OA-AUDIT-005
    #[test]
    fn audit_failure_does_not_run_committed_action() {
        let file = path("failure");
        let _ = std::fs::remove_file(&file);
        let mut denied = event(Action::MigrationFlip);
        denied.role = Role::Operate;
        let called = std::cell::Cell::new(false);
        let result = append_before_commit(&file, oa_audit_core::genesis(), denied, || {
            called.set(true);
            Ok::<(), ()>(())
        });
        assert!(result.is_err());
        assert!(!called.get());
        assert!(!file.exists());
    }

    // REQ: OA-AUDIT-006
    #[test]
    fn protected_aliases_are_refused_before_log_io() {
        let root = std::env::temp_dir().join(format!("oa-audit-seal-{}", std::process::id()));
        let protected = root
            .join("scds")
            .join("rec-20261119")
            .join("day2-synthetic")
            .join("audit.log");
        let _ = std::fs::remove_dir_all(&root);
        assert!(open_log_read(&protected).is_err());
        assert!(append(
            &protected,
            oa_audit_core::genesis(),
            event(Action::Certification)
        )
        .is_err());
        assert!(!protected.exists());
    }

    #[cfg(target_os = "linux")]
    // REQ: OA-AUDIT-006
    #[test]
    fn append_open_stays_anchored_to_verified_parent_after_rename_and_replacement() {
        let root = std::env::temp_dir().join(format!("oa-audit-parent-{}", std::process::id()));
        let moved = root.with_extension("moved");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&moved);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("audit.log");
        let parent = open_directory(&root).unwrap();
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir_all(&root).unwrap();

        let mut file = open_log_append_at(&path, &parent).unwrap();
        file.try_lock().unwrap();
        file.write_all(b"synthetic").unwrap();
        file.sync_all().unwrap();
        parent.sync_all().unwrap();

        assert_eq!(
            std::fs::read(moved.join("audit.log")).unwrap(),
            b"synthetic"
        );
        assert!(!root.join("audit.log").exists());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&moved);
    }

    #[cfg(target_os = "linux")]
    // REQ: OA-AUDIT-006
    #[test]
    fn append_refuses_leaf_symlinks_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("oa-audit-symlink-{}", std::process::id()));
        let target_root = root.with_extension("target");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&target_root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&target_root).unwrap();
        let target = target_root.join("existing.log");
        std::fs::write(&target, b"sentinel").unwrap();
        let path = root.join("audit.log");
        symlink(&target, &path).unwrap();
        let parent = open_directory(&root).unwrap();

        assert!(open_log_append_at(&path, &parent).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&target_root);
    }

    #[cfg(target_os = "linux")]
    // REQ: OA-AUDIT-003
    #[test]
    fn lock_contention_fails_before_commit_without_waiting() {
        let root = std::env::temp_dir().join(format!("oa-audit-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("audit.log");
        let parent = open_directory(&root).unwrap();
        let locked = open_log_append_at(&path, &parent).unwrap();
        locked.try_lock().unwrap();

        let called = std::cell::Cell::new(false);
        let result = append_before_commit(
            &path,
            oa_audit_core::genesis(),
            event(Action::Certification),
            || {
                called.set(true);
                Ok::<(), ()>(())
            },
        );
        assert!(result.is_err());
        assert!(!called.get());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        drop(locked);
        let _ = std::fs::remove_dir_all(root);
    }
}

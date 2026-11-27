#![forbid(unsafe_code)]
//! Shell (B1): the commit a program was built from, for the files it writes to say which source
//! produced them. `build.rs` reads `git -C <this crate's directory>`, so the value is the
//! repository the program was built from and not whatever repository it happens to run in. It
//! lives in a crate of its own because a build script is not allowed in a core or split crate
//! (B8).

/// `commit <hash>`, followed by ` (tracked files modified)` when a tracked file of `crates/`,
/// `tools/` or the workspace manifest or lock differed from
/// that commit at build time; `commit unknown` only if git was not available at build time.
pub const BUILD: &str = env!("OA_BUILD_INFO");

/// Hard ceiling even when a caller requests a larger artifact limit.
pub const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// Result of hashing one processor artifact from an opened local file. The digest identifies
/// bytes read from that path; it is not a signature, running-process attestation, or proof that
/// the file is the image loaded by a processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtifactDigest {
    /// SHA-256 of the exact bytes read.
    pub sha256: oa_sha256::Digest,
    /// Number of bytes hashed.
    pub bytes: u64,
    /// Always true: this is a local file observation without authenticated processor provenance.
    pub provenance_unverified: bool,
}

/// Refusal to identify an oversized, empty, or unreadable processor artifact.
#[derive(Debug)]
pub enum ArtifactError {
    /// The file could not be opened or inspected.
    Io(std::io::Error),
    /// The file was empty or exceeded the caller's explicit bound.
    InvalidLength,
    /// The caller supplied a zero or over-hard-cap limit.
    InvalidLimit,
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "artifact I/O: {error}"),
            Self::InvalidLength => f.write_str("artifact length is empty or exceeds its bound"),
            Self::InvalidLimit => f.write_str("artifact byte limit is outside the fixed bound"),
        }
    }
}

impl std::error::Error for ArtifactError {}

/// Hash a processor software artifact without reading more than `max_bytes + 1` bytes.
///
/// The caller must compare this local on-disk claim with an independently pinned expected
/// artifact digest and must not describe it as a loaded-image attestation.
pub fn digest_artifact(
    path: &std::path::Path,
    max_bytes: u64,
) -> Result<ArtifactDigest, ArtifactError> {
    use std::io::Read;

    // Reject bad limits before the path guard performs metadata or open operations.
    if max_bytes == 0 || max_bytes > MAX_ARTIFACT_BYTES {
        return Err(ArtifactError::InvalidLimit);
    }

    let (file, declared_len) = oa_sealed::open_regular(path).map_err(ArtifactError::Io)?;
    let before = file.metadata().map_err(ArtifactError::Io)?;
    let before_stamp = FileStamp::from_metadata(&before);
    if !before.is_file()
        || declared_len == 0
        || declared_len > max_bytes
        || before.len() != declared_len
    {
        return Err(ArtifactError::InvalidLength);
    }
    let limit = max_bytes
        .checked_add(1)
        .ok_or(ArtifactError::InvalidLength)?;
    let check_file = file.try_clone().map_err(ArtifactError::Io)?;
    let mut bounded = file.take(limit);
    let mut hasher = oa_sha256::Sha256::new();
    let mut buffer = [0u8; oa_sha256::READ_CHUNK_LEN];
    let mut bytes = 0u64;
    loop {
        let read = bounded.read(&mut buffer).map_err(ArtifactError::Io)?;
        if read == 0 {
            break;
        }
        let read_bytes = u64::try_from(read).map_err(|_| ArtifactError::InvalidLength)?;
        bytes = bytes
            .checked_add(read_bytes)
            .ok_or(ArtifactError::InvalidLength)?;
        if bytes > max_bytes {
            return Err(ArtifactError::InvalidLength);
        }
        hasher
            .update(buffer.get(..read).ok_or(ArtifactError::InvalidLength)?)
            .map_err(|_| ArtifactError::InvalidLength)?;
    }
    let after = check_file.metadata().map_err(ArtifactError::Io)?;
    if FileStamp::from_metadata(&after) != before_stamp || bytes != before.len() {
        return Err(ArtifactError::InvalidLength);
    }
    Ok(ArtifactDigest {
        sha256: hasher.finalize(),
        bytes,
        provenance_unverified: true,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    mtime: i64,
    #[cfg(unix)]
    mtime_nsec: i64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

impl FileStamp {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                len: metadata.len(),
                dev: metadata.dev(),
                ino: metadata.ino(),
                mtime: metadata.mtime(),
                mtime_nsec: metadata.mtime_nsec(),
                ctime: metadata.ctime(),
                ctime_nsec: metadata.ctime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: metadata.len(),
                modified: metadata.modified().ok(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::{digest_artifact, ArtifactError, MAX_ARTIFACT_BYTES};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oa-build-info-artifact-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    // REQ: OA-CERT-003, OA-CERT-007
    #[test]
    fn artifact_digest_is_exact_and_oversize_is_refused() {
        let path = path();
        let bytes = b"synthetic software artifact";
        std::fs::write(&path, bytes).expect("write artifact fixture");
        let actual = digest_artifact(&path, 128).expect("bounded artifact digest");
        assert_eq!(actual.sha256, oa_sha256::sha256(bytes));
        assert_eq!(actual.bytes, bytes.len() as u64);
        assert!(actual.provenance_unverified);
        assert!(matches!(
            digest_artifact(&path, 3),
            Err(ArtifactError::InvalidLength)
        ));
        assert!(matches!(
            digest_artifact(&path, MAX_ARTIFACT_BYTES + 1),
            Err(ArtifactError::InvalidLimit)
        ));
        assert!(matches!(
            digest_artifact(&path, 0),
            Err(ArtifactError::InvalidLimit)
        ));
        let _ = std::fs::remove_file(path);
    }
}

//! Bounded adaptation package integrity checks. Shell only: file access and status live here.
//!
//! `OAG2ADPT` v1 is a canonical fixed-layout manifest for one airspace snapshot. Its expected
//! SHA-256 is supplied separately by the caller. Digests detect corruption and disagreement;
//! they do not authenticate an issuer. The verified snapshot bytes are decoded directly, never
//! reopened after checking.

use crate::core::model::Model;
use crate::core::snapshot::{
    decode, encode, FORMAT_VERSION, FORMAT_VERSION_NOTE, MAX_SNAPSHOT_BYTES,
};
use oa_sha256::{sha256, Digest};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"OAG2ADPT";
const MANIFEST_VERSION: u16 = 1;
const READER_API: u16 = 1;
const MANIFEST_FILE: &str = "manifest.oag2";
const SNAPSHOT_FILE: &str = "airspace.snapshot";
const MAX_MANIFEST_BYTES: usize = 512;
const MAX_PACKAGE_BYTES: u64 = MAX_SNAPSHOT_BYTES as u64 + MAX_MANIFEST_BYTES as u64;
const MAX_RELEASE_BYTES: usize = 64;
const MAX_FACILITY_BYTES: usize = 64;
const MAX_CHANGE_REF_BYTES: usize = 128;
const FIXED_HEADER_BYTES: usize = 58;

/// Explicit refusal reason; safe to include in operator-facing status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    Path(String),
    Read(String),
    Oversize(&'static str),
    Truncated,
    BadMagic,
    UnsupportedManifestVersion(u16),
    UnsupportedReader { min: u16, max: u16 },
    InvalidField(&'static str),
    WrongComponentCount(u8),
    NonCanonical,
    ManifestIdentityMismatch { expected: String, actual: String },
    ComponentLengthMismatch { expected: u64, actual: u64 },
    ComponentDigestMismatch { expected: String, actual: String },
    SnapshotDecode(String),
    FacilityMismatch { manifest: String, snapshot: String },
    PackageChanged(&'static str),
    Output(String),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(e) => write!(f, "guarded path refusal: {e}"),
            Self::Read(e) => write!(f, "read refusal: {e}"),
            Self::Oversize(what) => write!(f, "{what} exceeds its fixed byte limit"),
            Self::Truncated => f.write_str("manifest is truncated"),
            Self::BadMagic => f.write_str("manifest magic is invalid"),
            Self::UnsupportedManifestVersion(v) => write!(f, "manifest schema {v} is unsupported"),
            Self::UnsupportedReader { min, max } => write!(
                f,
                "manifest reader range {min}..={max} excludes reader API {READER_API}"
            ),
            Self::InvalidField(name) => write!(f, "manifest field {name} is invalid"),
            Self::WrongComponentCount(n) => {
                write!(f, "manifest must contain exactly one component, found {n}")
            }
            Self::NonCanonical => f.write_str("manifest encoding is not canonical"),
            Self::ManifestIdentityMismatch { expected, actual } => write!(
                f,
                "pinned manifest digest mismatch: expected {expected}, got {actual}"
            ),
            Self::ComponentLengthMismatch { expected, actual } => write!(
                f,
                "snapshot length mismatch: expected {expected}, got {actual}"
            ),
            Self::ComponentDigestMismatch { expected, actual } => write!(
                f,
                "snapshot digest mismatch: expected {expected}, got {actual}"
            ),
            Self::SnapshotDecode(e) => write!(f, "snapshot decode refused: {e}"),
            Self::FacilityMismatch { manifest, snapshot } => write!(
                f,
                "facility mismatch: manifest {manifest}, snapshot {snapshot}"
            ),
            Self::PackageChanged(what) => write!(f, "package changed during verification: {what}"),
            Self::Output(e) => write!(f, "package output refused: {e}"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Manifest identity and public version metadata for a verified package.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold activation metadata; size guard waived.
pub struct PackageIdentity {
    pub manifest_sha256: Digest,
    pub snapshot_sha256: Digest,
    pub release: String,
    pub facility: String,
    pub change_ref: String,
    pub snapshot_bytes: u64,
}

/// A verified immutable model plus identity. The model came from the exact bytes that were hashed.
// A7.2: cold-start verified snapshot; size guard waived.
pub struct VerifiedPackage {
    model: Model,
    identity: PackageIdentity,
}

impl VerifiedPackage {
    /// Borrow the model that was decoded from the bytes whose digest was checked.
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// Borrow the immutable identity recorded by the successful verification.
    pub fn identity(&self) -> &PackageIdentity {
        &self.identity
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold serialized manifest model; size guard waived.
struct Manifest {
    reader_min: u16,
    reader_max: u16,
    release: String,
    facility: String,
    change_ref: String,
    snapshot_len: u64,
    snapshot_sha256: Digest,
}

fn field<'a>(bytes: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], VerifyError> {
    let end = offset
        .checked_add(len)
        .ok_or(VerifyError::InvalidField("length"))?;
    let slice = bytes.get(*offset..end).ok_or(VerifyError::Truncated)?;
    *offset = end;
    Ok(slice)
}

fn u16_at(bytes: &[u8], offset: &mut usize) -> Result<u16, VerifyError> {
    let value = field(bytes, offset, 2)?;
    let array: [u8; 2] = value.try_into().map_err(|_| VerifyError::Truncated)?;
    Ok(u16::from_le_bytes(array))
}

fn u64_at(bytes: &[u8], offset: &mut usize) -> Result<u64, VerifyError> {
    let value = field(bytes, offset, 8)?;
    let array: [u8; 8] = value.try_into().map_err(|_| VerifyError::Truncated)?;
    Ok(u64::from_le_bytes(array))
}

fn valid_token(text: &str, limit: usize, field_name: &'static str) -> Result<(), VerifyError> {
    if text.is_empty()
        || text.len() > limit
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(VerifyError::InvalidField(field_name));
    }
    Ok(())
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, VerifyError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(VerifyError::Oversize("manifest"));
    }
    if bytes.len() < FIXED_HEADER_BYTES {
        return Err(VerifyError::Truncated);
    }
    let mut offset = 0;
    if field(bytes, &mut offset, MAGIC.len())? != MAGIC {
        return Err(VerifyError::BadMagic);
    }
    let version = u16_at(bytes, &mut offset)?;
    if version != MANIFEST_VERSION {
        return Err(VerifyError::UnsupportedManifestVersion(version));
    }
    let reader_min = u16_at(bytes, &mut offset)?;
    let reader_max = u16_at(bytes, &mut offset)?;
    let lengths = field(bytes, &mut offset, 4)?;
    let release_len = usize::from(*lengths.first().ok_or(VerifyError::Truncated)?);
    let facility_len = usize::from(*lengths.get(1).ok_or(VerifyError::Truncated)?);
    let change_len = usize::from(*lengths.get(2).ok_or(VerifyError::Truncated)?);
    let component_count = *lengths.get(3).ok_or(VerifyError::Truncated)?;
    if component_count != 1 {
        return Err(VerifyError::WrongComponentCount(component_count));
    }
    let snapshot_len = u64_at(bytes, &mut offset)?;
    let digest: Digest = field(bytes, &mut offset, 32)?
        .try_into()
        .map_err(|_| VerifyError::Truncated)?;
    if release_len > MAX_RELEASE_BYTES
        || facility_len > MAX_FACILITY_BYTES
        || change_len > MAX_CHANGE_REF_BYTES
    {
        return Err(VerifyError::InvalidField("text length"));
    }
    let release = std::str::from_utf8(field(bytes, &mut offset, release_len)?)
        .map_err(|_| VerifyError::InvalidField("release"))?
        .to_owned();
    let facility = std::str::from_utf8(field(bytes, &mut offset, facility_len)?)
        .map_err(|_| VerifyError::InvalidField("facility"))?
        .to_owned();
    let change_ref = std::str::from_utf8(field(bytes, &mut offset, change_len)?)
        .map_err(|_| VerifyError::InvalidField("change reference"))?
        .to_owned();
    if offset != bytes.len() {
        return Err(VerifyError::NonCanonical);
    }
    valid_token(&release, MAX_RELEASE_BYTES, "release")?;
    valid_token(&facility, MAX_FACILITY_BYTES, "facility")?;
    valid_token(&change_ref, MAX_CHANGE_REF_BYTES, "change reference")?;
    if reader_min > reader_max || reader_min > READER_API || reader_max < READER_API {
        return Err(VerifyError::UnsupportedReader {
            min: reader_min,
            max: reader_max,
        });
    }
    let manifest = Manifest {
        reader_min,
        reader_max,
        release,
        facility,
        change_ref,
        snapshot_len,
        snapshot_sha256: digest,
    };
    if encode_manifest(&manifest)?.as_slice() != bytes {
        return Err(VerifyError::NonCanonical);
    }
    Ok(manifest)
}

fn encode_manifest(manifest: &Manifest) -> Result<Vec<u8>, VerifyError> {
    valid_token(&manifest.release, MAX_RELEASE_BYTES, "release")?;
    valid_token(&manifest.facility, MAX_FACILITY_BYTES, "facility")?;
    valid_token(
        &manifest.change_ref,
        MAX_CHANGE_REF_BYTES,
        "change reference",
    )?;
    if manifest.reader_min > manifest.reader_max
        || manifest.release.len() > MAX_RELEASE_BYTES
        || manifest.facility.len() > MAX_FACILITY_BYTES
        || manifest.change_ref.len() > MAX_CHANGE_REF_BYTES
    {
        return Err(VerifyError::InvalidField("manifest metadata"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(
            FIXED_HEADER_BYTES
                + manifest.release.len()
                + manifest.facility.len()
                + manifest.change_ref.len(),
        )
        .map_err(|_| VerifyError::Oversize("manifest allocation"))?;
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&MANIFEST_VERSION.to_le_bytes());
    bytes.extend_from_slice(&manifest.reader_min.to_le_bytes());
    bytes.extend_from_slice(&manifest.reader_max.to_le_bytes());
    bytes.push(
        u8::try_from(manifest.release.len())
            .map_err(|_| VerifyError::InvalidField("release length"))?,
    );
    bytes.push(
        u8::try_from(manifest.facility.len())
            .map_err(|_| VerifyError::InvalidField("facility length"))?,
    );
    bytes.push(
        u8::try_from(manifest.change_ref.len())
            .map_err(|_| VerifyError::InvalidField("change reference length"))?,
    );
    bytes.push(1); // The sole v1 component role and filename are fixed by this schema.
    bytes.extend_from_slice(&manifest.snapshot_len.to_le_bytes());
    bytes.extend_from_slice(&manifest.snapshot_sha256);
    bytes.extend_from_slice(manifest.release.as_bytes());
    bytes.extend_from_slice(manifest.facility.as_bytes());
    bytes.extend_from_slice(manifest.change_ref.as_bytes());
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(VerifyError::Oversize("manifest"));
    }
    Ok(bytes)
}

/// Parses an independently provisioned, lowercase SHA-256 pin. No manifest value can supply it.
pub fn parse_pin(text: &str) -> Result<Digest, VerifyError> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(VerifyError::InvalidField("expected manifest SHA-256 pin"));
    }
    let mut digest = [0u8; 32];
    for (i, byte) in digest.iter_mut().enumerate() {
        let hi = hex_nibble(text.as_bytes().get(i * 2).copied())
            .ok_or(VerifyError::InvalidField("expected manifest SHA-256 pin"))?;
        let lo = hex_nibble(text.as_bytes().get(i * 2 + 1).copied())
            .ok_or(VerifyError::InvalidField("expected manifest SHA-256 pin"))?;
        *byte = (hi << 4) | lo;
    }
    Ok(digest)
}

fn hex_nibble(byte: Option<u8>) -> Option<u8> {
    match byte? {
        b'0'..=b'9' => Some(byte? - b'0'),
        b'a'..=b'f' => Some(byte? - b'a' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold filesystem identity used only during verification; size guard waived.
struct FileStamp {
    device: u64,
    inode: u64,
    len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold two-entry package inventory; size guard waived.
struct PackageInventory {
    device: u64,
    inode: u64,
    entries: Vec<(String, FileStamp)>,
}

fn stamp(metadata: &std::fs::Metadata) -> FileStamp {
    FileStamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        len: metadata.len(),
    }
}

fn root_stamp(root: &Path) -> Result<(u64, u64), VerifyError> {
    oa_sealed::refuse(root).map_err(VerifyError::Path)?;
    // seal-allow: metadata follows the shared root refusal and validates its directory type.
    let metadata = fs::symlink_metadata(root)
        .map_err(|e| VerifyError::Read(format!("{}: {e}", root.display())))?;
    if !metadata.file_type().is_dir() {
        return Err(VerifyError::InvalidField(
            "package root must be a real directory",
        ));
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn read_guarded(
    path: &Path,
    limit: usize,
    kind: &'static str,
    expected_len: Option<u64>,
    expected_stamp: &FileStamp,
    root: &Path,
    expected_root: (u64, u64),
) -> Result<Vec<u8>, VerifyError> {
    if root_stamp(root)? != expected_root {
        return Err(VerifyError::PackageChanged("root identity before open"));
    }
    let (file, declared_len) = oa_sealed::open_regular(path)
        .map_err(|e| VerifyError::Path(format!("{}: {e}", path.display())))?;
    let opened_metadata = file
        .metadata()
        .map_err(|e| VerifyError::Read(format!("{}: {e}", path.display())))?;
    let opened_stamp = stamp(&opened_metadata);
    if opened_stamp != *expected_stamp {
        return Err(VerifyError::PackageChanged("opened file identity"));
    }
    if let Some(expected) = expected_len {
        if declared_len != expected {
            return Err(VerifyError::ComponentLengthMismatch {
                expected,
                actual: declared_len,
            });
        }
    }
    let limit_u64 = u64::try_from(limit).map_err(|_| VerifyError::Oversize(kind))?;
    if declared_len > limit_u64 {
        return Err(VerifyError::Oversize(kind));
    }
    let bounded_len = expected_len.unwrap_or(limit_u64);
    if bounded_len > limit_u64 {
        return Err(VerifyError::Oversize(kind));
    }
    let read_limit = bounded_len
        .checked_add(1)
        .ok_or(VerifyError::Oversize(kind))?;
    let mut bytes = Vec::new();
    let reserve = usize::try_from(declared_len).map_err(|_| VerifyError::Oversize(kind))?;
    bytes
        .try_reserve_exact(reserve.checked_add(1).ok_or(VerifyError::Oversize(kind))?)
        .map_err(|_| VerifyError::Oversize(kind))?;
    (&file)
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|e| VerifyError::Read(format!("{}: {e}", path.display())))?;
    if bytes.len() > limit {
        return Err(VerifyError::Oversize(kind));
    }
    if let Some(expected) = expected_len {
        let actual = u64::try_from(bytes.len()).map_err(|_| VerifyError::Oversize(kind))?;
        if actual != expected {
            return Err(VerifyError::ComponentLengthMismatch { expected, actual });
        }
    }
    let after_read = file
        .metadata()
        .map_err(|e| VerifyError::Read(format!("{}: {e}", path.display())))?;
    if stamp(&after_read) != opened_stamp {
        return Err(VerifyError::PackageChanged("file metadata during read"));
    }
    if root_stamp(root)? != expected_root {
        return Err(VerifyError::PackageChanged("root identity after read"));
    }
    Ok(bytes)
}

fn decode_verified(
    manifest: Manifest,
    manifest_digest: Digest,
    snapshot_bytes: Vec<u8>,
) -> Result<VerifiedPackage, VerifyError> {
    let actual_len =
        u64::try_from(snapshot_bytes.len()).map_err(|_| VerifyError::Oversize("snapshot"))?;
    if actual_len != manifest.snapshot_len {
        return Err(VerifyError::ComponentLengthMismatch {
            expected: manifest.snapshot_len,
            actual: actual_len,
        });
    }
    let snapshot_sha256 = sha256(&snapshot_bytes);
    if snapshot_sha256 != manifest.snapshot_sha256 {
        return Err(VerifyError::ComponentDigestMismatch {
            expected: oa_sha256::to_hex(&manifest.snapshot_sha256),
            actual: oa_sha256::to_hex(&snapshot_sha256),
        });
    }
    let model = decode(&snapshot_bytes).map_err(|e| VerifyError::SnapshotDecode(e.to_string()))?;
    let snapshot_facility = model.meta.artcc.as_str();
    if manifest.facility != snapshot_facility {
        return Err(VerifyError::FacilityMismatch {
            manifest: manifest.facility,
            snapshot: snapshot_facility.to_owned(),
        });
    }
    Ok(VerifiedPackage {
        model,
        identity: PackageIdentity {
            manifest_sha256: manifest_digest,
            snapshot_sha256,
            release: manifest.release,
            facility: manifest.facility,
            change_ref: manifest.change_ref,
            snapshot_bytes: actual_len,
        },
    })
}

/// Check an on-disk package against a separately provisioned manifest pin and return the model
/// decoded from those exact checked bytes. This is called before route adaptation is admitted.
/// Root and file identities plus a bounded inventory are rechecked, but the filesystem does not
/// provide an atomic multi-file snapshot here; release tooling must keep package directories
/// immutable, and this function does not promise safety against a hostile replace-and-restore race.
pub fn verify_current(
    root: &Path,
    expected_manifest_sha256: Digest,
) -> Result<VerifiedPackage, VerifyError> {
    let before = check_package_entries(root)?;
    let manifest_stamp = inventory_stamp(&before, MANIFEST_FILE)?;
    let manifest_path = root.join(MANIFEST_FILE);
    let manifest_bytes = read_guarded(
        &manifest_path,
        MAX_MANIFEST_BYTES,
        "manifest",
        Some(manifest_stamp.len),
        manifest_stamp,
        root,
        (before.device, before.inode),
    )?;
    let actual_manifest_sha256 = sha256(&manifest_bytes);
    if actual_manifest_sha256 != expected_manifest_sha256 {
        return Err(VerifyError::ManifestIdentityMismatch {
            expected: oa_sha256::to_hex(&expected_manifest_sha256),
            actual: oa_sha256::to_hex(&actual_manifest_sha256),
        });
    }
    let manifest = parse_manifest(&manifest_bytes)?;
    if manifest.snapshot_len > MAX_SNAPSHOT_BYTES as u64 {
        return Err(VerifyError::Oversize("snapshot declared by manifest"));
    }
    let snapshot_stamp = inventory_stamp(&before, SNAPSHOT_FILE)?;
    let snapshot_path = root.join(SNAPSHOT_FILE);
    let snapshot_bytes = read_guarded(
        &snapshot_path,
        MAX_SNAPSHOT_BYTES,
        "snapshot",
        Some(manifest.snapshot_len),
        snapshot_stamp,
        root,
        (before.device, before.inode),
    )?;
    let after = check_package_entries(root)?;
    if after != before {
        return Err(VerifyError::PackageChanged("bounded package inventory"));
    }
    decode_verified(manifest, actual_manifest_sha256, snapshot_bytes)
}

fn inventory_stamp<'a>(
    inventory: &'a PackageInventory,
    name: &str,
) -> Result<&'a FileStamp, VerifyError> {
    inventory
        .entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|(_, stamp)| stamp)
        .ok_or(VerifyError::InvalidField("package component set"))
}

fn check_package_entries(root: &Path) -> Result<PackageInventory, VerifyError> {
    let (device, inode) = root_stamp(root)?;
    let entries =
        // seal-allow: list the root after refusal/identity check; component reads are guarded separately.
        fs::read_dir(root).map_err(|e| VerifyError::Read(format!("{}: {e}", root.display())))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| VerifyError::Read(format!("{}: {e}", root.display())))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| VerifyError::InvalidField("package entry name"))?;
        if !entry
            .file_type()
            .map_err(|e| VerifyError::Read(format!("{}: {e}", entry.path().display())))?
            .is_file()
        {
            return Err(VerifyError::InvalidField(
                "package entries must be regular files",
            ));
        }
        let entry_stamp = stamp(
            &entry
                .metadata()
                .map_err(|e| VerifyError::Read(format!("{}: {e}", entry.path().display())))?,
        );
        if names.iter().any(|(existing, _)| existing == &name) {
            return Err(VerifyError::InvalidField("duplicate package component"));
        }
        names.push((name, entry_stamp));
        if names.len() > 2 {
            return Err(VerifyError::InvalidField("unreferenced package component"));
        }
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    if names
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        != [SNAPSHOT_FILE, MANIFEST_FILE]
    {
        return Err(VerifyError::InvalidField("package component set"));
    }
    if root_stamp(root)? != (device, inode) {
        return Err(VerifyError::PackageChanged(
            "root identity during inventory",
        ));
    }
    let total_bytes = names.iter().try_fold(0u64, |total, (_, stamp)| {
        total
            .checked_add(stamp.len)
            .ok_or(VerifyError::Oversize("package"))
    })?;
    if total_bytes > MAX_PACKAGE_BYTES {
        return Err(VerifyError::Oversize("package"));
    }
    Ok(PackageInventory {
        device,
        inode,
        entries: names,
    })
}

/// Build a canonical manifest from the exact encoded airspace snapshot bytes.
pub fn manifest_for_snapshot(
    snapshot_bytes: &[u8],
    release: &str,
    facility: &str,
    change_ref: &str,
) -> Result<(Vec<u8>, Digest), VerifyError> {
    if snapshot_bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(VerifyError::Oversize("snapshot"));
    }
    let decoded = decode(snapshot_bytes).map_err(|e| VerifyError::SnapshotDecode(e.to_string()))?;
    if encode(&decoded).map_err(|e| VerifyError::SnapshotDecode(e.to_string()))? != snapshot_bytes {
        return Err(VerifyError::NonCanonical);
    }
    if facility != decoded.meta.artcc {
        return Err(VerifyError::FacilityMismatch {
            manifest: facility.to_owned(),
            snapshot: decoded.meta.artcc,
        });
    }
    let manifest = Manifest {
        reader_min: READER_API,
        reader_max: READER_API,
        release: release.to_owned(),
        facility: facility.to_owned(),
        change_ref: change_ref.to_owned(),
        snapshot_len: u64::try_from(snapshot_bytes.len())
            .map_err(|_| VerifyError::Oversize("snapshot"))?,
        snapshot_sha256: sha256(snapshot_bytes),
    };
    let bytes = encode_manifest(&manifest)?;
    let digest = sha256(&bytes);
    Ok((bytes, digest))
}

/// Write a new package directory, refusing overwrite. Snapshot is written first and manifest last;
/// return the pin only after both writes, syncs and package read-back succeed. If an error occurs,
/// the directory is retained for inspection and the caller must not provision its pin. The byte
/// verifier establishes integrity against a separately supplied pin, not publication durability:
/// if the bytes form a complete valid package despite a reported write/sync error, that package
/// could still pass verification against a separately supplied pin.
pub fn write_new_package(
    root: &Path,
    snapshot_bytes: &[u8],
    release: &str,
    facility: &str,
    change_ref: &str,
) -> Result<Digest, VerifyError> {
    let (manifest_bytes, manifest_digest) =
        manifest_for_snapshot(snapshot_bytes, release, facility, change_ref)?;
    oa_sealed::refuse(root).map_err(VerifyError::Path)?;
    fs::create_dir(root).map_err(|e| VerifyError::Output(format!("{}: {e}", root.display())))?;
    let snapshot_path: PathBuf = root.join(SNAPSHOT_FILE);
    let manifest_path = root.join(MANIFEST_FILE);
    write_new_file(&snapshot_path, snapshot_bytes)?;
    write_new_file(&manifest_path, &manifest_bytes)?;
    verify_current(root, manifest_digest).map_err(|e| {
        VerifyError::Output(format!(
            "package read-back verification failed; directory retained; caller must not provision the manifest pin: {e}"
        ))
    })?;
    Ok(manifest_digest)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), VerifyError> {
    oa_sealed::refuse(path).map_err(VerifyError::Path)?;
    // seal-allow: create-new output follows shared path refusal and reads no input.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| VerifyError::Output(format!("{}: {e}", path.display())))?;
    file.write_all(bytes)
        .map_err(|e| VerifyError::Output(format!("{}: {e}", path.display())))?;
    file.sync_all()
        .map_err(|e| VerifyError::Output(format!("{}: {e}", path.display())))
}

/// The accepted snapshot formats in this reader, retained for producer-side diagnostics.
pub fn supported_snapshot_formats() -> (u32, u32) {
    (FORMAT_VERSION, FORMAT_VERSION_NOTE)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::core::model::{empty_model, Meta};
    use crate::core::snapshot::encode;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn sample() -> (Model, Vec<u8>) {
        let model = empty_model(Meta {
            nasr_effective: 20_261_029,
            cifp_cycle: 2611,
            artcc: "ZAU".to_owned(),
            buffer_m: 92_600,
            nasr_sha256: [0x11; 32],
            cifp_sha256: [0x22; 32],
        });
        let bytes = encode(&model).expect("synthetic empty model encodes");
        (model, bytes)
    }

    fn temp_root(label: &str) -> PathBuf {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("oa-g2-{label}-{}-{serial}", std::process::id()))
    }

    fn make_package() -> (PathBuf, Digest) {
        let (_, bytes) = sample();
        let root = temp_root("package");
        let pin = write_new_package(&root, &bytes, "test-1", "ZAU", "CHG-17")
            .expect("synthetic package writes");
        (root, pin)
    }

    // REQ: OA-ADAPT-001, OA-ADAPT-002
    #[test]
    fn verifies_independent_pin_and_decodes_the_hashed_component() {
        let (root, pin) = make_package();
        let verified = verify_current(&root, pin).expect("package verifies");
        assert_eq!(verified.model().meta.artcc, "ZAU");
        assert_eq!(verified.identity().release, "test-1");
        assert_eq!(verified.identity().change_ref, "CHG-17");
        assert_ne!(
            verified.identity().manifest_sha256,
            verified.identity().snapshot_sha256
        );
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-002
    #[test]
    fn manifest_identity_comes_only_from_the_exact_snapshot_bytes() {
        let (model, snapshot) = sample();
        let mut different_same_facility = model.clone();
        different_same_facility.meta.nasr_effective = 20_261_101;
        assert_eq!(different_same_facility.meta.artcc, model.meta.artcc);
        assert_ne!(different_same_facility, model);
        let other_bytes = encode(&different_same_facility).expect("different synthetic model");

        let (manifest_bytes, pin) = manifest_for_snapshot(&snapshot, "test-1", "ZAU", "CHG-17")
            .expect("manifest is derived from the supplied snapshot");
        let manifest = parse_manifest(&manifest_bytes).expect("canonical manifest");
        let verified = decode_verified(manifest, pin, snapshot.clone())
            .expect("verified model is decoded from the exact snapshot bytes");
        assert_eq!(verified.model(), &model);
        assert_ne!(verified.model(), &different_same_facility);
        assert_ne!(sha256(&snapshot), sha256(&other_bytes));
        assert_eq!(verified.identity().snapshot_sha256, sha256(&snapshot));
    }

    // REQ: OA-ADAPT-001
    #[test]
    fn wrong_external_identity_refuses_before_component_activation() {
        let (root, pin) = make_package();
        let mut wrong = pin;
        wrong[0] ^= 1;
        assert!(matches!(
            verify_current(&root, wrong),
            Err(VerifyError::ManifestIdentityMismatch { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-002
    #[test]
    fn changed_component_with_unchanged_manifest_is_refused() {
        let (root, pin) = make_package();
        let path = root.join(SNAPSHOT_FILE);
        let mut bytes = fs::read(&path).expect("read synthetic snapshot");
        let last = bytes.len().checked_sub(1).expect("nonempty snapshot");
        bytes[last] ^= 1;
        fs::write(path, bytes).expect("mutate synthetic snapshot");
        assert!(matches!(
            verify_current(&root, pin),
            Err(VerifyError::ComponentDigestMismatch { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-001
    #[test]
    fn changed_local_manifest_with_unchanged_external_pin_is_refused() {
        let (root, pin) = make_package();
        let path = root.join(MANIFEST_FILE);
        let mut bytes = fs::read(&path).expect("read synthetic manifest");
        let release_start = FIXED_HEADER_BYTES;
        let release_byte = bytes
            .get_mut(release_start)
            .expect("release identifier byte");
        *release_byte = b'x';
        fs::write(path, bytes).expect("mutate local manifest");
        assert!(matches!(
            verify_current(&root, pin),
            Err(VerifyError::ManifestIdentityMismatch { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-001, OA-ADAPT-002
    #[test]
    fn extra_unreferenced_file_and_missing_component_are_refused() {
        let (root, pin) = make_package();
        fs::write(root.join("extra.bin"), b"synthetic").expect("write extra file");
        assert!(matches!(
            verify_current(&root, pin),
            Err(VerifyError::InvalidField("unreferenced package component"))
        ));
        fs::remove_file(root.join("extra.bin")).expect("remove extra file");
        fs::remove_file(root.join(SNAPSHOT_FILE)).expect("remove snapshot component");
        assert!(verify_current(&root, pin).is_err());
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-001
    #[test]
    fn truncation_and_unsupported_schema_are_counted_refusals() {
        let (root, _) = make_package();
        let path = root.join(MANIFEST_FILE);
        let truncated = MAGIC.to_vec();
        fs::write(&path, &truncated).expect("write truncated manifest");
        assert_eq!(parse_manifest(&truncated), Err(VerifyError::Truncated));
        let (_, snapshot) = sample();
        let (mut unsupported, _) = manifest_for_snapshot(&snapshot, "test-1", "ZAU", "CHG-17")
            .expect("valid synthetic manifest");
        unsupported[8..10].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(
            parse_manifest(&unsupported),
            Err(VerifyError::UnsupportedManifestVersion(2))
        ));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-002
    #[test]
    fn oversized_declared_component_is_refused_before_component_read() {
        let (root, _) = make_package();
        let path = root.join(MANIFEST_FILE);
        let mut bytes = fs::read(&path).expect("read synthetic manifest");
        bytes[18..26].copy_from_slice(&u64::MAX.to_le_bytes());
        let pin = sha256(&bytes);
        fs::write(path, bytes).expect("write oversized declaration");
        assert!(matches!(
            verify_current(&root, pin),
            Err(VerifyError::Oversize("snapshot declared by manifest"))
        ));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-002
    #[test]
    fn declared_component_length_must_match_inventory_before_read() {
        let (root, _) = make_package();
        let path = root.join(MANIFEST_FILE);
        let bytes = fs::read(&path).expect("read synthetic manifest");
        let mut manifest = parse_manifest(&bytes).expect("canonical manifest");
        manifest.snapshot_len = manifest
            .snapshot_len
            .checked_add(1)
            .expect("bounded length");
        let changed = encode_manifest(&manifest).expect("encode changed synthetic manifest");
        let pin = sha256(&changed);
        fs::write(path, changed).expect("write synthetic length mismatch");
        assert!(matches!(
            verify_current(&root, pin),
            Err(VerifyError::ComponentLengthMismatch { expected, actual })
                if expected == manifest.snapshot_len && actual.checked_add(1) == Some(expected)
        ));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-001
    #[test]
    fn unsupported_reader_api_range_refuses_even_with_matching_new_pin() {
        let (root, _) = make_package();
        let path = root.join(MANIFEST_FILE);
        let mut bytes = fs::read(&path).expect("read synthetic manifest");
        bytes[10..12].copy_from_slice(&2u16.to_le_bytes());
        bytes[12..14].copy_from_slice(&2u16.to_le_bytes());
        let updated_pin = sha256(&bytes);
        fs::write(path, bytes).expect("write synthetic reader range");
        assert!(matches!(
            verify_current(&root, updated_pin),
            Err(VerifyError::UnsupportedReader { min: 2, max: 2 })
        ));
        let _ = fs::remove_dir_all(root);
    }

    // REQ: OA-ADAPT-001
    #[test]
    fn absent_package_root_is_a_named_read_refusal() {
        let root = temp_root("absent");
        assert!(matches!(
            verify_current(&root, [0; 32]),
            Err(VerifyError::Read(_))
        ));
    }

    // REQ: OA-ADAPT-001
    #[test]
    fn symlinked_component_path_is_refused_before_open() {
        use std::os::unix::fs::symlink;

        let (root, pin) = make_package();
        let source = root.join(SNAPSHOT_FILE);
        let outside = root.with_extension("snapshot-copy");
        fs::rename(&source, &outside).expect("move synthetic component");
        symlink(&outside, &source).expect("link synthetic component");
        assert!(matches!(
            verify_current(&root, pin),
            Err(VerifyError::InvalidField(
                "package entries must be regular files"
            ))
        ));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(outside);
    }

    #[test]
    fn noncanonical_pin_and_facility_mismatch_refuse() {
        assert!(parse_pin(&"A".repeat(64)).is_err());
        let (_, bytes) = sample();
        assert!(matches!(
            manifest_for_snapshot(&bytes, "test-1", "ZLC", "CHG-17"),
            Err(VerifyError::FacilityMismatch { .. })
        ));
        assert!(manifest_for_snapshot(&bytes, "../bad", "ZAU", "CHG-17").is_err());
        assert!(manifest_for_snapshot(&bytes, "test-1", "ZAU", "../outside").is_err());
    }
}

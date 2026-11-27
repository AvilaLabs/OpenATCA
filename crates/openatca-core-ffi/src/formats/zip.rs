//! Safe bounded adaptation of the existing ZIP metadata and selected-entry APIs.
//!
//! No filesystem operations or extraction paths are provided here. Names and data are owned by
//! the caller-facing flat result only for the duration of one FFI call.
#![forbid(unsafe_code)]

use oa_zip::{Archive, ZipError, DEFAULT_LIMITS};

/// One validated ZIP central-directory record. Name spans index the call's UTF-8 byte pool.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaZipEntry {
    pub name_offset: u64,
    pub name_length: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub local_offset: u64,
    pub crc32: u32,
    pub flags: u16,
    pub method: u16,
    pub zip64: u8,
    pub utf8_name_flag: u8,
    pub reserved: [u8; 6],
}

const _: () = assert!(core::mem::size_of::<OaZipEntry>() == 56);
const _: () = assert!(core::mem::align_of::<OaZipEntry>() == 8);

pub(crate) struct ZipMetadata {
    pub entries: Vec<OaZipEntry>,
    pub names: Vec<u8>,
}

pub(crate) enum ZipAdapterError {
    Parse(ZipError),
    Allocation,
}

impl From<ZipError> for ZipAdapterError {
    fn from(error: ZipError) -> Self {
        Self::Parse(error)
    }
}

pub(crate) fn parse_metadata(input: &[u8]) -> Result<ZipMetadata, ZipAdapterError> {
    let archive = Archive::parse_with_limits(input, DEFAULT_LIMITS)?;
    let mut entries = Vec::new();
    let mut names = Vec::new();
    entries
        .try_reserve(archive.entries().len())
        .map_err(|_| ZipAdapterError::Allocation)?;
    for entry in archive.entries() {
        let name = entry.name().as_bytes();
        let name_offset = u64::try_from(names.len()).map_err(|_| ZipAdapterError::Allocation)?;
        let name_length = u64::try_from(name.len()).map_err(|_| ZipAdapterError::Allocation)?;
        names
            .try_reserve(name.len())
            .map_err(|_| ZipAdapterError::Allocation)?;
        names.extend_from_slice(name);
        entries.push(OaZipEntry {
            name_offset,
            name_length,
            compressed_size: entry.compressed_size(),
            uncompressed_size: entry.uncompressed_size(),
            local_offset: entry.local_offset(),
            crc32: entry.crc32(),
            flags: entry.flags(),
            method: entry.method(),
            zip64: u8::from(entry.is_zip64()),
            utf8_name_flag: u8::from(entry.flags() & (1 << 11) != 0),
            reserved: [0; 6],
        });
    }
    Ok(ZipMetadata { entries, names })
}

pub(crate) fn extract_entry(input: &[u8], index: usize) -> Result<Vec<u8>, ZipError> {
    let archive = Archive::parse_with_limits(input, DEFAULT_LIMITS)?;
    let entry = archive
        .entries()
        .get(index)
        .ok_or_else(|| ZipError::NameNotFound(format!("entry index {index}")))?;
    archive.extract_entry(entry)
}

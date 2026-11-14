//! Loading the two archives. **Shell** (B1, B3): reads files, hashes, unzips and parses; the
//! result is plain data ([`Sources`]) for the core. Offline code run once per cycle (C6).
//!
//! The archives are read directly through `oa-zip`; nothing is unpacked to disk. Each archive's
//! SHA-256 is computed over the file's bytes and compared with the `--expect-sha256` value
//! before anything is parsed: a mismatch refuses the load ([`LoadError::HashMismatch`]), and so
//! does an expectation that names no input archive (a typo must not pass silently).
//!
//! Memory: one archive entry is extracted at a time, validated against the FAA structure file,
//! parsed, and dropped, so the peak is the archives plus one entry plus the parsed records.

use crate::source::{ArchiveInfo, Diagnostics, FileDiag, NasrData, Sources};
use oa_arinc424 as cifp_lib;
use oa_nasr::procedure::ProcedureKind;
use oa_nasr::schema::{parse_schemas, validate, Schemas};
use oa_nasr::{apt, arb, awy, cls, fix, nav, procedure, NasrError, Parsed, RowError};
use oa_sha256::{sha256, to_hex};
use oa_zip::{read_archive_file, Archive, ZipError};
use std::fmt;
use std::path::Path;

/// An expected SHA-256 for an input archive, from `--expect-sha256 <file>=<hex>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    /// Archive file name (no directory).
    pub file: String,
    pub sha256: [u8; 32],
}

/// Why loading failed.
#[derive(Debug)]
pub enum LoadError {
    /// A `--expect-sha256` argument is not `<file>=<64 hex digits>`.
    BadExpectation(String),
    /// An expectation names a file that is not one of the two input archives.
    ExpectationUnused(String),
    /// An archive's hash differs from the expected one. The build refuses to run.
    HashMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    /// The archive could not be read or is not a valid ZIP.
    Zip { archive: String, source: ZipError },
    /// The archive has no entry with this name.
    MissingEntry { archive: String, entry: String },
    /// More than one entry matches the name.
    AmbiguousEntry { archive: String, entry: String },
    /// A NASR file failed to parse in a way that stops the stream.
    Nasr(NasrError),
    /// The CIFP file failed to parse in a way that stops the stream.
    Cifp(cifp_lib::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::BadExpectation(s) => {
                write!(
                    f,
                    "bad --expect-sha256 {s:?}: expected <file>=<64 hex digits>"
                )
            }
            LoadError::ExpectationUnused(n) => {
                write!(
                    f,
                    "--expect-sha256 names {n:?}, which is not an input archive"
                )
            }
            LoadError::HashMismatch {
                file,
                expected,
                actual,
            } => write!(
                f,
                "SHA-256 mismatch for {file}: expected {expected}, got {actual}; refusing to build"
            ),
            LoadError::Zip { archive, source } => write!(f, "{archive}: {source}"),
            LoadError::MissingEntry { archive, entry } => {
                write!(f, "{archive}: no entry named {entry}")
            }
            LoadError::AmbiguousEntry { archive, entry } => {
                write!(f, "{archive}: more than one entry matches {entry}")
            }
            LoadError::Nasr(e) => write!(f, "{e}"),
            LoadError::Cifp(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<NasrError> for LoadError {
    fn from(e: NasrError) -> Self {
        LoadError::Nasr(e)
    }
}

/// Parses `<file>=<hex>`; the hex may be upper or lower case.
pub fn parse_expectation(arg: &str) -> Result<Expectation, LoadError> {
    let bad = || LoadError::BadExpectation(arg.to_string());
    let (file, hex) = arg.rsplit_once('=').ok_or_else(bad)?;
    if file.is_empty() || hex.len() != 64 {
        return Err(bad());
    }
    let mut sha256 = [0u8; 32];
    for (i, slot) in sha256.iter_mut().enumerate() {
        let pair = hex.get(2 * i..2 * i + 2).ok_or_else(bad)?;
        *slot = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
    }
    Ok(Expectation {
        file: file.to_string(),
        sha256,
    })
}

/// Reads both archives from disk and loads them; see [`load_bytes`].
pub fn load_files(nasr: &Path, cifp: &Path, expect: &[Expectation]) -> Result<Sources, LoadError> {
    let name_of = |p: &Path| {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    let nasr_name = name_of(nasr);
    let cifp_name = name_of(cifp);
    let nasr_bytes = read_archive_file(nasr).map_err(|source| LoadError::Zip {
        archive: nasr_name.clone(),
        source,
    })?;
    let cifp_bytes = read_archive_file(cifp).map_err(|source| LoadError::Zip {
        archive: cifp_name.clone(),
        source,
    })?;
    load_bytes(&nasr_name, &nasr_bytes, &cifp_name, &cifp_bytes, expect)
}

fn archive_info(
    name: &str,
    bytes: &[u8],
    expect: &[Expectation],
) -> Result<ArchiveInfo, LoadError> {
    let sha = sha256(bytes);
    let expected = expect.iter().find(|e| e.file == name).map(|e| e.sha256);
    if let Some(want) = expected {
        if want != sha {
            return Err(LoadError::HashMismatch {
                file: name.to_string(),
                expected: to_hex(&want),
                actual: to_hex(&sha),
            });
        }
    }
    Ok(ArchiveInfo {
        name: name.to_string(),
        bytes: bytes.len() as u64,
        sha256: sha,
        expected,
    })
}

/// Hashes, checks and parses both archives held in memory. Every expectation must name one of
/// the two archives and match its hash, or the load is refused before any parsing.
pub fn load_bytes(
    nasr_name: &str,
    nasr_zip: &[u8],
    cifp_name: &str,
    cifp_zip: &[u8],
    expect: &[Expectation],
) -> Result<Sources, LoadError> {
    for e in expect {
        if e.file != nasr_name && e.file != cifp_name {
            return Err(LoadError::ExpectationUnused(e.file.clone()));
        }
    }
    let nasr_archive = archive_info(nasr_name, nasr_zip, expect)?;
    let cifp_archive = archive_info(cifp_name, cifp_zip, expect)?;
    let mut diag = Diagnostics::default();
    let nasr = load_nasr(nasr_name, nasr_zip, &mut diag)?;
    let cifp = load_cifp(cifp_name, cifp_zip, &mut diag)?;
    Ok(Sources {
        nasr_archive,
        cifp_archive,
        nasr,
        cifp,
        diag,
    })
}

fn zip_err(archive: &str) -> impl Fn(ZipError) -> LoadError + '_ {
    move |source| LoadError::Zip {
        archive: archive.to_string(),
        source,
    }
}

/// Extracts the one entry named `file` (at the archive root or under any directory).
fn extract(archive_name: &str, zip: &Archive<'_>, file: &str) -> Result<Vec<u8>, LoadError> {
    let suffix = format!("/{file}");
    let mut hits = zip
        .entries()
        .iter()
        .filter(|e| e.name == file || e.name.ends_with(&suffix));
    let first = hits.next().ok_or_else(|| LoadError::MissingEntry {
        archive: archive_name.to_string(),
        entry: file.to_string(),
    })?;
    if hits.next().is_some() {
        return Err(LoadError::AmbiguousEntry {
            archive: archive_name.to_string(),
            entry: file.to_string(),
        });
    }
    zip.extract_entry(first).map_err(zip_err(archive_name))
}

/// The structure-file prefixes whose schemas cover the data files used.
const STRUCTURE_FILES: [&str; 8] = [
    "ARB_CSV_DATA_STRUCTURE.csv",
    "FIX_CSV_DATA_STRUCTURE.csv",
    "NAV_CSV_DATA_STRUCTURE.csv",
    "APT_CSV_DATA_STRUCTURE.csv",
    "AWY_CSV_DATA_STRUCTURE.csv",
    "STAR_CSV_DATA_STRUCTURE.csv",
    "DP_CSV_DATA_STRUCTURE.csv",
    "CLS_ARSP_CSV_DATA_STRUCTURE.csv",
];

/// Reads one data file: extracts it, validates it against its schema, and hands the bytes to
/// `parse`, which returns the parsed records and the rows it rejected. The `FileDiag` is pushed
/// into `diag`.
struct NasrReader<'a> {
    name: &'a str,
    zip: &'a Archive<'a>,
    schemas: Schemas,
}

impl NasrReader<'_> {
    fn bytes(&self, file: &str, diag: &mut Diagnostics) -> Result<Vec<u8>, LoadError> {
        let bytes = extract(self.name, self.zip, file)?;
        let mut d = FileDiag {
            file: file.to_string(),
            ..FileDiag::default()
        };
        let key = file.strip_suffix(".csv").unwrap_or(file);
        match self.schemas.get(key) {
            Some(schema) => {
                let v = validate(schema, file, &bytes[..])?;
                d.rows = v.rows;
                d.violations = v.violations;
                d.dropped = v.dropped;
            }
            None => d.schema_missing = true,
        }
        diag.nasr_files.push(d);
        Ok(bytes)
    }
}

/// Files rejected rows to the diagnostics of the file they came from.
fn attach(diag: &mut Diagnostics, rejected: Vec<RowError>) {
    for r in rejected {
        if let Some(d) = diag.nasr_files.iter_mut().find(|d| d.file == r.file) {
            d.rejected.push(r);
        }
    }
}

fn take<T>(parsed: Parsed<T>, diag: &mut Diagnostics) -> Vec<T> {
    attach(diag, parsed.rejected);
    parsed.records
}

fn load_nasr(name: &str, bytes: &[u8], diag: &mut Diagnostics) -> Result<NasrData, LoadError> {
    let zip = Archive::parse(bytes).map_err(zip_err(name))?;
    let mut schemas = Schemas::default();
    for s in STRUCTURE_FILES {
        let sb = extract(name, &zip, s)?;
        let part = parse_schemas(s, &sb[..])?;
        schemas.files.extend(part.files);
        schemas.warnings.extend(part.warnings);
    }
    diag.schema_warnings = schemas.warnings.clone();
    let rd = NasrReader {
        name,
        zip: &zip,
        schemas,
    };
    let mut out = NasrData::default();

    let b = rd.bytes("ARB_BASE.csv", diag)?;
    out.arb_base = take(arb::parse_base("ARB_BASE.csv", &b[..])?, diag);
    let b = rd.bytes("ARB_SEG.csv", diag)?;
    out.boundaries = take(arb::parse_boundaries("ARB_SEG.csv", &b[..])?, diag);
    let b = rd.bytes("FIX_BASE.csv", diag)?;
    out.fixes = take(fix::parse_fixes("FIX_BASE.csv", &b[..])?, diag);
    let b = rd.bytes("NAV_BASE.csv", diag)?;
    out.navaids = take(nav::parse_navaids("NAV_BASE.csv", &b[..])?, diag);
    let b = rd.bytes("APT_BASE.csv", diag)?;
    out.airports = take(apt::parse_airports("APT_BASE.csv", &b[..])?, diag);
    let b = rd.bytes("APT_RWY_END.csv", diag)?;
    out.runway_ends = take(apt::parse_runway_ends("APT_RWY_END.csv", &b[..])?, diag);
    let base = rd.bytes("AWY_BASE.csv", diag)?;
    let seg = rd.bytes("AWY_SEG_ALT.csv", diag)?;
    out.airways = take(
        awy::parse_airways("AWY_BASE.csv", &base[..], "AWY_SEG_ALT.csv", &seg[..])?,
        diag,
    );
    for (kind, stem) in [(ProcedureKind::Star, "STAR"), (ProcedureKind::Dp, "DP")] {
        let (bn, an, rn) = (
            format!("{stem}_BASE.csv"),
            format!("{stem}_APT.csv"),
            format!("{stem}_RTE.csv"),
        );
        let base = rd.bytes(&bn, diag)?;
        let apt = rd.bytes(&an, diag)?;
        let rte = rd.bytes(&rn, diag)?;
        let parsed =
            procedure::parse_procedures(kind, (&bn, &base[..]), (&an, &apt[..]), (&rn, &rte[..]))?;
        let recs = take(parsed, diag);
        match kind {
            ProcedureKind::Star => out.stars = recs,
            ProcedureKind::Dp => out.dps = recs,
        }
    }
    let b = rd.bytes("CLS_ARSP.csv", diag)?;
    out.class_airspace = take(cls::parse_class_airspace("CLS_ARSP.csv", &b[..])?, diag);
    Ok(out)
}

fn load_cifp(
    name: &str,
    bytes: &[u8],
    diag: &mut Diagnostics,
) -> Result<cifp_lib::Cifp, LoadError> {
    let zip = Archive::parse(bytes).map_err(zip_err(name))?;
    let data = extract(name, &zip, "FAACIFP18")?;
    let parsed = cifp_lib::parse_bytes(&data, "FAACIFP18").map_err(LoadError::Cifp)?;
    diag.cifp_errors = parsed.errors.iter().map(ToString::to_string).collect();
    diag.cifp_errors_dropped = parsed.errors_dropped;
    diag.cifp_counts = parsed.counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    Ok(parsed)
}

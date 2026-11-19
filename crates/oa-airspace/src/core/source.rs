//! The plain values the shell hands the core: what is known about the input archives and what
//! the loaders noticed. **Core** (B1, B7): data only, no behaviour (A1). Built by `load`
//! (shell), consumed by the report (core).
//!
//! The parsed records themselves (`NasrData`, `Sources`) hold the parser crates' types, one
//! allocated record per input row; they live in `shell::sources`. What crosses here is text and
//! numbers only (B5): the schema violations and rejected rows are already worded by the shell.

/// An input archive: name, size, hash, and the hash that was expected of it, if any.
/// Cold struct (A7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveInfo {
    /// File name without directory.
    pub name: String,
    pub bytes: u64,
    pub sha256: [u8; 32],
    /// The `--expect-sha256` value this archive was checked against; `None` when none was
    /// given (the report says so). A mismatch never gets this far: the load is refused.
    pub expected: Option<[u8; 32]>,
}

/// One schema violation, worded by the shell: where it is and what is wrong.
/// Cold struct (A7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViolationNote {
    /// Column name.
    pub column: String,
    /// What is violated, in words (the report groups on this text).
    pub kind: String,
    /// 1-based physical line in the file.
    pub line: u64,
}

/// What the validation of one NASR file found: schema violations and rows the typed parser
/// rejected. Cold struct (A7.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiag {
    /// File name inside the archive (`FIX_BASE.csv`).
    pub file: String,
    /// Data rows checked against the schema.
    pub rows: u64,
    /// True when the structure files had no schema for this file.
    pub schema_missing: bool,
    pub violations: Vec<ViolationNote>,
    /// Violations beyond the validator's cap, counted but not kept.
    pub dropped: u64,
    /// The rejected rows, each as the parser words it.
    pub rejected: Vec<String>,
    /// Rejected rows beyond the parser's cap (`oa_nasr::MAX_REJECTED_PER_FILE`), counted only.
    /// For a parser that joins several files this is booked on the last file it loaded.
    pub rejected_dropped: u64,
}

/// Everything the loaders noticed that is not a model object. Cold struct (A7.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// One entry per NASR data file used, in load order.
    pub nasr_files: Vec<FileDiag>,
    /// Problems in the structure files themselves.
    pub schema_warnings: Vec<String>,
    /// CIFP records that failed to decode (the parser keeps at most 1000).
    pub cifp_errors: Vec<String>,
    pub cifp_errors_dropped: usize,
    /// CIFP physical record counts by section (`PA`, `D`, ...).
    pub cifp_counts: Vec<(String, usize)>,
}

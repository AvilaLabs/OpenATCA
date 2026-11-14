#![forbid(unsafe_code)]
//! oa-csv: a streaming RFC 4180 reader. See docs/M0_DESIGN.md.
//!
//! * The first record is a required header; fields are read by column name.
//! * Quoted fields, doubled quotes, CRLF or LF line endings and embedded newlines in quoted
//!   fields are supported. A record whose field count differs from the header's is an error.
//! * Every error carries the source name and the 1-based physical line on which the offending
//!   record starts. Records are counted in physical lines, so a record with an embedded newline
//!   advances the line counter by more than one.
//! * Every size is bounded by a named constant (see [`Limits`]); exceeding one is an error that
//!   names the limit. No input, however malformed, makes the reader panic or allocate beyond
//!   the limits.
//!
//! Strictness beyond the RFC's grammar, chosen because the data is safety-relevant: a quote in
//! an unquoted field, any character other than a delimiter after a closing quote, a bare CR
//! outside quotes, and bytes that are not valid UTF-8 are all errors rather than being
//! repaired silently. A leading UTF-8 byte order mark is skipped.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead};

/// Largest accepted record, in raw bytes, counting quotes, delimiters and embedded newlines.
/// The longest real NASR record is a few kilobytes; 1 MiB leaves a wide margin while bounding
/// memory per record.
pub const MAX_RECORD_BYTES: usize = 1 << 20;

/// Largest accepted single field, in bytes after unquoting. NASR remark fields reach 1,500
/// characters; 64 KiB is the cap.
pub const MAX_FIELD_BYTES: usize = 64 * 1024;

/// Largest accepted number of fields (columns) in one record, including the header.
/// The widest NASR file has roughly 100 columns.
pub const MAX_FIELDS: usize = 512;

/// Largest accepted number of data records (header excluded) in one file. The largest NASR
/// file in a cycle has about 70,000 rows; 5 million is far above any real file.
pub const MAX_RECORDS: usize = 5_000_000;

/// The size limits applied by a [`Reader`]. `Limits::default()` uses the named constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Limits {
    /// See [`MAX_RECORD_BYTES`].
    pub max_record_bytes: usize,
    /// See [`MAX_FIELD_BYTES`].
    pub max_field_bytes: usize,
    /// See [`MAX_FIELDS`].
    pub max_fields: usize,
    /// See [`MAX_RECORDS`].
    pub max_records: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_record_bytes: MAX_RECORD_BYTES,
            max_field_bytes: MAX_FIELD_BYTES,
            max_fields: MAX_FIELDS,
            max_records: MAX_RECORDS,
        }
    }
}

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CsvErrorKind {
    /// The underlying reader failed.
    Io(String),
    /// The input has no header record.
    MissingHeader,
    /// A header name is empty.
    EmptyHeaderName {
        /// Zero-based column index.
        index: usize,
    },
    /// Two header columns have the same name.
    DuplicateHeader(String),
    /// A requested column is not in the header.
    MissingColumn(String),
    /// A record has a different number of fields than the header.
    FieldCountMismatch {
        /// Header field count.
        expected: usize,
        /// This record's field count.
        found: usize,
    },
    /// A quote appeared inside an unquoted field.
    QuoteInUnquotedField,
    /// A closing quote was followed by something other than a delimiter or line end.
    CharacterAfterClosingQuote,
    /// A carriage return outside quotes was not followed by a line feed.
    BareCarriageReturn,
    /// The input ended inside a quoted field.
    UnterminatedQuote,
    /// A field contains bytes that are not valid UTF-8.
    InvalidUtf8,
    /// A size limit was exceeded.
    LimitExceeded {
        /// Name of the constant that holds the limit, e.g. `MAX_FIELD_BYTES`.
        limit: &'static str,
        /// The limit's value.
        max: usize,
    },
}

/// An error with the source name and the 1-based physical line of the offending record.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct CsvError {
    /// Source name given to the reader (usually a file name).
    pub source: String,
    /// 1-based physical line on which the offending record starts.
    pub line: u64,
    /// What went wrong.
    pub kind: CsvErrorKind,
}

impl fmt::Display for CsvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: ", self.source, self.line)?;
        match &self.kind {
            CsvErrorKind::Io(e) => write!(f, "read error: {e}"),
            CsvErrorKind::MissingHeader => write!(f, "no header row"),
            CsvErrorKind::EmptyHeaderName { index } => {
                write!(f, "header column {index} has an empty name")
            }
            CsvErrorKind::DuplicateHeader(n) => write!(f, "duplicate header column {n:?}"),
            CsvErrorKind::MissingColumn(n) => write!(f, "missing column {n:?}"),
            CsvErrorKind::FieldCountMismatch { expected, found } => {
                write!(f, "record has {found} fields, header has {expected}")
            }
            CsvErrorKind::QuoteInUnquotedField => write!(f, "quote inside an unquoted field"),
            CsvErrorKind::CharacterAfterClosingQuote => {
                write!(f, "unexpected character after closing quote")
            }
            CsvErrorKind::BareCarriageReturn => {
                write!(f, "carriage return not followed by line feed")
            }
            CsvErrorKind::UnterminatedQuote => write!(f, "quoted field is never closed"),
            CsvErrorKind::InvalidUtf8 => write!(f, "field is not valid UTF-8"),
            CsvErrorKind::LimitExceeded { limit, max } => {
                write!(f, "exceeds limit {limit} ({max})")
            }
        }
    }
}

impl std::error::Error for CsvError {}

/// The header row: column names and a name-to-index lookup.
#[derive(Debug)]
// A7.2: cold struct, size guard waived
pub struct Header {
    source: String,
    line: u64,
    names: Vec<String>,
    index: HashMap<String, usize>,
}

impl Header {
    /// The source name this header was read from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Column names in file order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Number of columns.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// True when there are no columns (never the case for a header built by [`Reader`]).
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Whether the header has a column of this name.
    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    /// Index of a column, or a `MissingColumn` error naming it.
    pub fn index_of(&self, name: &str) -> Result<usize, CsvError> {
        self.index.get(name).copied().ok_or_else(|| CsvError {
            source: self.source.clone(),
            line: self.line,
            kind: CsvErrorKind::MissingColumn(name.to_string()),
        })
    }
}

/// One data record. Fields are read by column name.
#[derive(Debug, Clone)]
// A7.2: cold struct, size guard waived
pub struct Record {
    line: u64,
    data: String,
    ends: Vec<usize>,
}

impl Record {
    /// 1-based physical line on which this record starts.
    pub fn line(&self) -> u64 {
        self.line
    }

    /// Number of fields (equal to the header's).
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// True when the record has no fields (never the case for a record read by [`Reader`]).
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// The field at a zero-based index, if there is one.
    pub fn field(&self, index: usize) -> Option<&str> {
        let end = *self.ends.get(index)?;
        let start = match index.checked_sub(1) {
            Some(prev) => *self.ends.get(prev)?,
            None => 0,
        };
        self.data.get(start..end)
    }

    /// The field in the named column of `header` (the reader's, see [`Reader::header`]). A
    /// missing column is an error naming it. The record holds no reference to its header: a
    /// record is plain data (A1, A4), so the caller passes the header it read it under.
    pub fn get(&self, header: &Header, name: &str) -> Result<&str, CsvError> {
        let i = header.index_of(name)?;
        self.field(i).ok_or_else(|| CsvError {
            source: header.source.clone(),
            line: self.line,
            kind: CsvErrorKind::FieldCountMismatch {
                expected: header.len(),
                found: self.ends.len(),
            },
        })
    }

    /// All fields in file order.
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        (0..self.ends.len()).filter_map(move |i| self.field(i))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    FieldStart,
    Unquoted,
    Quoted,
    QuoteInQuoted,
    AfterCr,
}

/// A streaming CSV reader over any `BufRead`.
// A7.2: cold struct, size guard waived
pub struct Reader<R: BufRead> {
    reader: R,
    source: String,
    limits: Limits,
    header: Header,
    next_line: u64,
    records_read: usize,
    failed: bool,
}

// A7.2: cold struct, size guard waived
struct Raw {
    line: u64,
    data: Vec<u8>,
    ends: Vec<usize>,
}

impl<R: BufRead> Reader<R> {
    /// Opens a reader with the default limits and reads the header row.
    pub fn new(source: impl Into<String>, reader: R) -> Result<Self, CsvError> {
        Self::with_limits(source, reader, Limits::default())
    }

    /// Opens a reader with explicit limits and reads the header row.
    pub fn with_limits(
        source: impl Into<String>,
        reader: R,
        limits: Limits,
    ) -> Result<Self, CsvError> {
        let source = source.into();
        let mut reader = reader;
        skip_bom(&mut reader).map_err(|e| io_error(&source, 1, &e))?;
        let mut next_line = 1;
        let raw =
            read_raw(&mut reader, &source, &limits, &mut next_line)?.ok_or_else(|| CsvError {
                source: source.clone(),
                line: 1,
                kind: CsvErrorKind::MissingHeader,
            })?;
        let fields = split_fields(raw, &source)?;
        let (line, data, ends) = fields;
        let mut names = Vec::with_capacity(ends.len());
        let mut index = HashMap::new();
        let mut start = 0;
        for (i, &end) in ends.iter().enumerate() {
            let name = data.get(start..end).unwrap_or("").to_string();
            start = end;
            let err = |kind| CsvError {
                source: source.clone(),
                line,
                kind,
            };
            if name.is_empty() {
                return Err(err(CsvErrorKind::EmptyHeaderName { index: i }));
            }
            if index.insert(name.clone(), i).is_some() {
                return Err(err(CsvErrorKind::DuplicateHeader(name)));
            }
            names.push(name);
        }
        let header = Header {
            source: source.clone(),
            line,
            names,
            index,
        };
        Ok(Reader {
            reader,
            source,
            limits,
            header,
            next_line,
            records_read: 0,
            failed: false,
        })
    }

    /// The header row.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The source name.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Reads the next record; `Ok(None)` at end of input. After an error every further call
    /// returns `Ok(None)`.
    pub fn next_record(&mut self) -> Result<Option<Record>, CsvError> {
        if self.failed {
            return Ok(None);
        }
        let result = self.read_one();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_one(&mut self) -> Result<Option<Record>, CsvError> {
        let raw = match read_raw(
            &mut self.reader,
            &self.source,
            &self.limits,
            &mut self.next_line,
        )? {
            Some(r) => r,
            None => return Ok(None),
        };
        let line = raw.line;
        if self.records_read >= self.limits.max_records {
            return Err(CsvError {
                source: self.source.clone(),
                line,
                kind: CsvErrorKind::LimitExceeded {
                    limit: "MAX_RECORDS",
                    max: self.limits.max_records,
                },
            });
        }
        let (line, data, ends) = split_fields(raw, &self.source)?;
        if ends.len() != self.header.len() {
            return Err(CsvError {
                source: self.source.clone(),
                line,
                kind: CsvErrorKind::FieldCountMismatch {
                    expected: self.header.len(),
                    found: ends.len(),
                },
            });
        }
        self.records_read += 1;
        Ok(Some(Record { line, data, ends }))
    }
}

impl<R: BufRead> Iterator for Reader<R> {
    type Item = Result<Record, CsvError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_record().transpose()
    }
}

fn io_error(source: &str, line: u64, e: &io::Error) -> CsvError {
    CsvError {
        source: source.to_string(),
        line,
        kind: CsvErrorKind::Io(e.to_string()),
    }
}

fn skip_bom<R: BufRead>(reader: &mut R) -> io::Result<()> {
    const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
    loop {
        match reader.fill_buf() {
            Ok(buf) => {
                if buf.len() >= BOM.len() && buf.starts_with(&BOM) {
                    reader.consume(BOM.len());
                }
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Validates UTF-8 per field and returns (start line, text, field end offsets).
fn split_fields(raw: Raw, source: &str) -> Result<(u64, String, Vec<usize>), CsvError> {
    let line = raw.line;
    let bad = || CsvError {
        source: source.to_string(),
        line,
        kind: CsvErrorKind::InvalidUtf8,
    };
    let data = String::from_utf8(raw.data).map_err(|_| bad())?;
    // Fields end at ASCII delimiters, so a split inside a multi-byte sequence means the
    // sequence straddled two fields and neither field is valid UTF-8 on its own.
    if raw.ends.iter().any(|&e| !data.is_char_boundary(e)) {
        return Err(bad());
    }
    Ok((line, data, raw.ends))
}

/// Reads one logical record's raw bytes. `next_line` is the physical line about to be read and
/// is advanced past every line feed consumed.
fn read_raw<R: BufRead>(
    reader: &mut R,
    source: &str,
    limits: &Limits,
    next_line: &mut u64,
) -> Result<Option<Raw>, CsvError> {
    let start_line = *next_line;
    let err = |kind| CsvError {
        source: source.to_string(),
        line: start_line,
        kind,
    };
    let limit =
        |name: &'static str, max: usize| err(CsvErrorKind::LimitExceeded { limit: name, max });
    let mut data: Vec<u8> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    let mut field_start = 0usize;
    let mut state = State::FieldStart;
    let mut record_bytes = 0usize;

    loop {
        let buf = match reader.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(source, start_line, &e)),
        };
        if buf.is_empty() {
            // End of input.
            if record_bytes == 0 {
                return Ok(None);
            }
            return match state {
                State::Quoted => Err(err(CsvErrorKind::UnterminatedQuote)),
                State::AfterCr => Err(err(CsvErrorKind::BareCarriageReturn)),
                _ => {
                    if ends.len() >= limits.max_fields {
                        return Err(limit("MAX_FIELDS", limits.max_fields));
                    }
                    ends.push(data.len());
                    Ok(Some(Raw {
                        line: start_line,
                        data,
                        ends,
                    }))
                }
            };
        }
        let mut used = 0usize;
        let mut finished = false;
        for &b in buf {
            used += 1;
            record_bytes += 1;
            if record_bytes > limits.max_record_bytes {
                return Err(limit("MAX_RECORD_BYTES", limits.max_record_bytes));
            }
            if b == b'\n' {
                *next_line = next_line.saturating_add(1);
            }
            // `end_field` records the end of the current field; `end_record` also stops.
            let mut end_field = false;
            let mut end_record = false;
            let mut push = false;
            match state {
                State::FieldStart => match b {
                    b'"' => state = State::Quoted,
                    b',' => end_field = true,
                    b'\r' => state = State::AfterCr,
                    b'\n' => {
                        end_field = true;
                        end_record = true;
                    }
                    _ => {
                        push = true;
                        state = State::Unquoted;
                    }
                },
                State::Unquoted => match b {
                    b',' => end_field = true,
                    b'\r' => state = State::AfterCr,
                    b'\n' => {
                        end_field = true;
                        end_record = true;
                    }
                    b'"' => return Err(err(CsvErrorKind::QuoteInUnquotedField)),
                    _ => push = true,
                },
                State::AfterCr => {
                    if b == b'\n' {
                        end_field = true;
                        end_record = true;
                    } else {
                        return Err(err(CsvErrorKind::BareCarriageReturn));
                    }
                }
                State::Quoted => {
                    if b == b'"' {
                        state = State::QuoteInQuoted;
                    } else {
                        push = true;
                    }
                }
                State::QuoteInQuoted => match b {
                    b'"' => {
                        push = true;
                        state = State::Quoted;
                    }
                    b',' => end_field = true,
                    b'\r' => state = State::AfterCr,
                    b'\n' => {
                        end_field = true;
                        end_record = true;
                    }
                    _ => return Err(err(CsvErrorKind::CharacterAfterClosingQuote)),
                },
            }
            if push {
                if data.len() - field_start >= limits.max_field_bytes {
                    return Err(limit("MAX_FIELD_BYTES", limits.max_field_bytes));
                }
                data.push(b);
            }
            if end_field {
                if ends.len() >= limits.max_fields {
                    return Err(limit("MAX_FIELDS", limits.max_fields));
                }
                ends.push(data.len());
                field_start = data.len();
                state = State::FieldStart;
            }
            if end_record {
                finished = true;
                break;
            }
        }
        reader.consume(used);
        if finished {
            return Ok(Some(Raw {
                line: start_line,
                data,
                ends,
            }));
        }
    }
}

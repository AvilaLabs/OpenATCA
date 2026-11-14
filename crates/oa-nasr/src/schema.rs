//! The FAA's `*_CSV_DATA_STRUCTURE.csv` files and validation of data files against them.
//!
//! A structure file has the columns `CSV File`, `Column Name`, `Max Length`, `Data Type` and
//! `Nullable`, one row per column of each data file it covers. `Max Length` is a number for
//! `VARCHAR` and `(precision,scale)` for `NUMBER`.
//!
//! Validation rules, applied to every row of a data file:
//! * every schema column is in the header (`MissingColumn`) and every header column is in the
//!   schema (`UnexpectedColumn`); order is not checked, columns are matched by name;
//! * `VARCHAR` text is at most `Max Length` characters (counted as Unicode scalar values, on the
//!   raw untrimmed text);
//! * `NUMBER(p,s)` is a plain decimal literal with at most `p - s` integer digits and at most `s`
//!   fraction digits (leading zeros and trailing fraction zeros are not counted);
//! * a non-nullable column is never the empty string.

use crate::row::decimal_shape;
use crate::{NasrError, MAX_VIOLATIONS_PER_FILE};
use oa_csv::Reader;
use std::collections::BTreeMap;
use std::io::BufRead;

/// A column's declared type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    /// Text.
    Varchar,
    /// Fixed-point number `NUMBER(precision, scale)`.
    Number {
        /// Total significant digits.
        precision: u32,
        /// Digits after the decimal point.
        scale: u32,
    },
}

/// One column of a data file's schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSpec {
    /// Column name.
    pub name: String,
    /// Maximum length in characters (`VARCHAR`), or 0 for `NUMBER`, whose size is in the type.
    pub max_len: u32,
    /// Declared type.
    pub data_type: DataType,
    /// Whether an empty value is allowed.
    pub nullable: bool,
}

/// The schema of one data file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSchema {
    /// File name without extension, e.g. `FIX_BASE`.
    pub file: String,
    /// Columns in structure-file order.
    pub columns: Vec<ColumnSpec>,
}

/// All schemas in one structure file, keyed by data file name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schemas {
    /// Schemas by data file name (e.g. `FIX_BASE`), in sorted order.
    pub files: BTreeMap<String, FileSchema>,
    /// Defects found in the structure file itself. A column name with surrounding whitespace
    /// is trimmed and reported here (the cycle-2026-10-29 FIX structure file has
    /// `COUNTRY_CODE  ` for `FIX_BASE`).
    pub warnings: Vec<String>,
}

impl Schemas {
    /// The schema for a data file, if the structure file covers it.
    pub fn get(&self, file: &str) -> Option<&FileSchema> {
        self.files.get(file)
    }
}

fn schema_err(file: &str, line: u64, message: String) -> NasrError {
    NasrError::Schema {
        file: file.to_string(),
        line,
        message,
    }
}

/// Parses `(precision,scale)`.
fn parse_precision(s: &str) -> Option<(u32, u32)> {
    let inner = s.trim().strip_prefix('(')?.strip_suffix(')')?;
    let (p, sc) = inner.split_once(',')?;
    let p = p.trim().parse::<u32>().ok()?;
    let sc = sc.trim().parse::<u32>().ok()?;
    (sc <= p).then_some((p, sc))
}

/// Reads a `*_CSV_DATA_STRUCTURE.csv` file.
pub fn parse_schemas<R: BufRead>(source: &str, reader: R) -> Result<Schemas, NasrError> {
    let mut csv = Reader::new(source, reader)?;
    let mut out = Schemas::default();
    while let Some(rec) = csv.next_record()? {
        let line = rec.line();
        let file = rec.get("CSV File")?.trim().to_string();
        let raw_name = rec.get("Column Name")?;
        let name = raw_name.trim().to_string();
        let max = rec.get("Max Length")?.trim();
        let ty = rec.get("Data Type")?.trim();
        let nullable = match rec.get("Nullable")?.trim() {
            "Yes" => true,
            "No" => false,
            other => return Err(schema_err(source, line, format!("Nullable {other:?}"))),
        };
        if file.is_empty() || name.is_empty() {
            return Err(schema_err(source, line, "empty file or column name".into()));
        }
        if name != raw_name {
            out.warnings.push(format!(
                "{source}:{line}: column name {raw_name:?} has surrounding whitespace (trimmed)"
            ));
        }
        let (max_len, data_type) = match ty {
            "VARCHAR" => match max.parse::<u32>() {
                Ok(n) => (n, DataType::Varchar),
                Err(_) => return Err(schema_err(source, line, format!("Max Length {max:?}"))),
            },
            "NUMBER" => match parse_precision(max) {
                Some((precision, scale)) => (0, DataType::Number { precision, scale }),
                None => return Err(schema_err(source, line, format!("NUMBER size {max:?}"))),
            },
            other => return Err(schema_err(source, line, format!("Data Type {other:?}"))),
        };
        let schema = out.files.entry(file.clone()).or_insert_with(|| FileSchema {
            file,
            columns: Vec::new(),
        });
        if schema.columns.iter().any(|c| c.name == name) {
            return Err(schema_err(
                source,
                line,
                format!("column {name:?} listed twice for {}", schema.file),
            ));
        }
        schema.columns.push(ColumnSpec {
            name,
            max_len,
            data_type,
            nullable,
        });
    }
    Ok(out)
}

/// What a row or the header violates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViolationKind {
    /// The schema has this column but the file's header does not.
    MissingColumn,
    /// The file's header has this column but the schema does not.
    UnexpectedColumn,
    /// A text value is longer than `Max Length`.
    TooLong {
        /// Declared maximum.
        max: u32,
        /// Actual length in characters.
        len: usize,
    },
    /// A `NUMBER` value is not a plain decimal literal.
    NotNumeric(String),
    /// A `NUMBER` value has more integer digits than `precision - scale`.
    TooManyIntegerDigits {
        /// Allowed integer digits.
        max: u32,
        /// Actual integer digits.
        digits: usize,
    },
    /// A `NUMBER` value has more fraction digits than `scale`.
    TooManyFractionDigits {
        /// Allowed fraction digits.
        max: u32,
        /// Actual fraction digits.
        digits: usize,
    },
    /// An empty value in a non-nullable column.
    NullViolation,
}

/// One schema violation: file, 1-based line (1 for header problems), column and kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaViolation {
    /// Data file name.
    pub file: String,
    /// 1-based physical line.
    pub line: u64,
    /// Column name.
    pub column: String,
    /// What is violated.
    pub kind: ViolationKind,
}

/// The outcome of validating one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    /// Data file name.
    pub file: String,
    /// Number of data rows checked.
    pub rows: u64,
    /// Violations found, in file order, at most [`MAX_VIOLATIONS_PER_FILE`].
    pub violations: Vec<SchemaViolation>,
    /// Violations found beyond the cap, counted but not stored.
    pub dropped: u64,
}

/// Total violations in a validation, stored or not.
pub fn total_violations(v: &Validation) -> u64 {
    v.violations.len() as u64 + v.dropped
}

struct Collector {
    file: String,
    violations: Vec<SchemaViolation>,
    dropped: u64,
}

impl Collector {
    fn push(&mut self, line: u64, column: &str, kind: ViolationKind) {
        if self.violations.len() >= MAX_VIOLATIONS_PER_FILE {
            self.dropped += 1;
            return;
        }
        self.violations.push(SchemaViolation {
            file: self.file.clone(),
            line,
            column: column.to_string(),
            kind,
        });
    }
}

/// Checks one value against its column spec.
fn check_value(spec: &ColumnSpec, value: &str) -> Option<ViolationKind> {
    if value.is_empty() {
        return (!spec.nullable).then_some(ViolationKind::NullViolation);
    }
    match spec.data_type {
        DataType::Varchar => {
            let len = value.chars().count();
            (len > spec.max_len as usize).then_some(ViolationKind::TooLong {
                max: spec.max_len,
                len,
            })
        }
        DataType::Number { precision, scale } => {
            let Some((int_digits, frac_digits)) = decimal_shape(value) else {
                return Some(ViolationKind::NotNumeric(value.to_string()));
            };
            let int_max = precision - scale;
            if int_digits > int_max as usize {
                return Some(ViolationKind::TooManyIntegerDigits {
                    max: int_max,
                    digits: int_digits,
                });
            }
            if frac_digits > scale as usize {
                return Some(ViolationKind::TooManyFractionDigits {
                    max: scale,
                    digits: frac_digits,
                });
            }
            None
        }
    }
}

/// Validates every row of a data file against its schema. Violations are collected; CSV-level
/// problems (syntax, field count, size limits) stop the read and are returned as errors.
pub fn validate<R: BufRead>(
    schema: &FileSchema,
    source: &str,
    reader: R,
) -> Result<Validation, NasrError> {
    let mut csv = Reader::new(source, reader)?;
    let mut out = Collector {
        file: schema.file.clone(),
        violations: Vec::new(),
        dropped: 0,
    };
    let header_line = 1;
    for spec in &schema.columns {
        if !csv.header().contains(&spec.name) {
            out.push(header_line, &spec.name, ViolationKind::MissingColumn);
        }
    }
    // Spec for each header column by position, resolved once by name.
    let specs: Vec<Option<&ColumnSpec>> = csv
        .header()
        .names()
        .iter()
        .map(|n| schema.columns.iter().find(|c| &c.name == n))
        .collect();
    for (name, spec) in csv.header().names().iter().zip(&specs) {
        if spec.is_none() {
            out.push(header_line, name, ViolationKind::UnexpectedColumn);
        }
    }
    let mut rows = 0u64;
    while let Some(rec) = csv.next_record()? {
        rows += 1;
        for (i, spec) in specs.iter().enumerate() {
            let (Some(spec), Some(value)) = (spec, rec.field(i)) else {
                continue;
            };
            if let Some(kind) = check_value(spec, value) {
                out.push(rec.line(), &spec.name, kind);
            }
        }
    }
    Ok(Validation {
        file: out.file,
        rows,
        violations: out.violations,
        dropped: out.dropped,
    })
}

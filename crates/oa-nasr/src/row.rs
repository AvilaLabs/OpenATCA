//! Typed access to one CSV row, shared by every file parser.

use crate::{dms_to_decimal, NasrError, Parsed, Position, RowError, RowErrorKind};
use oa_csv::{Reader, Record};
use std::io::BufRead;

/// Why reading a field failed: stops the stream, or rejects just this row.
pub(crate) enum Fail {
    Fatal(NasrError),
    Row(RowError),
}

impl From<oa_csv::CsvError> for Fail {
    fn from(e: oa_csv::CsvError) -> Self {
        Fail::Fatal(NasrError::Csv(e))
    }
}

/// Column names of a DMS position and its published decimals.
pub(crate) struct PosCols {
    pub lat_deg: &'static str,
    pub lat_min: &'static str,
    pub lat_sec: &'static str,
    pub lat_hemis: &'static str,
    pub lat_dec: &'static str,
    pub lon_deg: &'static str,
    pub lon_min: &'static str,
    pub lon_sec: &'static str,
    pub lon_hemis: &'static str,
    pub lon_dec: &'static str,
}

/// The `LAT_*`/`LONG_*` columns used by ARB, FIX, NAV_BASE and APT_BASE.
pub(crate) const STD_POS: PosCols = PosCols {
    lat_deg: "LAT_DEG",
    lat_min: "LAT_MIN",
    lat_sec: "LAT_SEC",
    lat_hemis: "LAT_HEMIS",
    lat_dec: "LAT_DECIMAL",
    lon_deg: "LONG_DEG",
    lon_min: "LONG_MIN",
    lon_sec: "LONG_SEC",
    lon_hemis: "LONG_HEMIS",
    lon_dec: "LONG_DECIMAL",
};

pub(crate) struct Row<'a> {
    pub file: &'a str,
    pub rec: &'a Record,
}

/// Splits a decimal literal into (integer digits excluding leading zeros, fraction digits
/// excluding trailing zeros). Accepts an optional sign, digits, and an optional fraction; no
/// exponent, no `inf`/`nan`, no spaces. `None` if it is not that shape.
pub(crate) fn decimal_shape(s: &str) -> Option<(usize, usize)> {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, f),
        None => (body, ""),
    };
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let int_digits = int.trim_start_matches('0').len();
    let frac_digits = frac.trim_end_matches('0').len();
    Some((int_digits, frac_digits))
}

impl<'a> Row<'a> {
    pub fn new(file: &'a str, rec: &'a Record) -> Row<'a> {
        Row { file, rec }
    }

    pub fn line(&self) -> u64 {
        self.rec.line()
    }

    pub fn problem(&self, column: &str, kind: RowErrorKind) -> Fail {
        Fail::Row(RowError {
            file: self.file.to_string(),
            line: self.rec.line(),
            column: column.to_string(),
            kind,
        })
    }

    /// The trimmed text of a column; a missing column is fatal.
    pub fn text(&self, col: &str) -> Result<&'a str, Fail> {
        Ok(self.rec.get(col)?.trim())
    }

    pub fn req(&self, col: &str) -> Result<String, Fail> {
        let t = self.text(col)?;
        if t.is_empty() {
            return Err(self.problem(col, RowErrorKind::Missing));
        }
        Ok(t.to_string())
    }

    pub fn opt(&self, col: &str) -> Result<Option<String>, Fail> {
        let t = self.text(col)?;
        Ok(if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        })
    }

    fn number(&self, col: &str, t: &str) -> Result<f64, Fail> {
        let bad = || self.problem(col, RowErrorKind::NotANumber(t.to_string()));
        decimal_shape(t).ok_or_else(bad)?;
        match t.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(v),
            _ => Err(bad()),
        }
    }

    pub fn req_f64(&self, col: &str) -> Result<f64, Fail> {
        let t = self.text(col)?;
        if t.is_empty() {
            return Err(self.problem(col, RowErrorKind::Missing));
        }
        self.number(col, t)
    }

    pub fn opt_f64(&self, col: &str) -> Result<Option<f64>, Fail> {
        let t = self.text(col)?;
        if t.is_empty() {
            return Ok(None);
        }
        self.number(col, t).map(Some)
    }

    fn integer(&self, col: &str, t: &str) -> Result<i64, Fail> {
        t.parse::<i64>()
            .map_err(|_| self.problem(col, RowErrorKind::NotANumber(t.to_string())))
    }

    pub fn opt_i64(&self, col: &str) -> Result<Option<i64>, Fail> {
        let t = self.text(col)?;
        if t.is_empty() {
            return Ok(None);
        }
        self.integer(col, t).map(Some)
    }

    /// A non-negative sequence number.
    pub fn req_u32(&self, col: &str) -> Result<u32, Fail> {
        let t = self.text(col)?;
        if t.is_empty() {
            return Err(self.problem(col, RowErrorKind::Missing));
        }
        t.parse::<u32>()
            .map_err(|_| self.problem(col, RowErrorKind::NotANumber(t.to_string())))
    }

    /// A `Y`/`N` flag; an empty value reads as false.
    pub fn flag(&self, col: &str) -> Result<bool, Fail> {
        self.flag_of(col, "Y", Some("N"))
    }

    /// A flag that is `yes` when set and empty (or `no`) otherwise.
    pub fn flag_of(&self, col: &str, yes: &str, no: Option<&str>) -> Result<bool, Fail> {
        let t = self.text(col)?;
        if t == yes {
            Ok(true)
        } else if t.is_empty() || Some(t) == no {
            Ok(false)
        } else {
            Err(self.problem(col, RowErrorKind::BadFlag(t.to_string())))
        }
    }

    /// An altitude column with its direction column.
    pub fn alt_dir(&self, alt: &str, dir: &str) -> Result<crate::awy::AltDir, Fail> {
        Ok(crate::awy::AltDir {
            alt: self.opt_i64(alt)?,
            dir: self.opt(dir)?,
        })
    }

    fn dms(&self, deg: &str, min: &str, sec: &str, hemis: &str, is_lat: bool) -> Result<f64, Fail> {
        let d = self.req_f64(deg)?;
        let m = self.req_f64(min)?;
        let s = self.req_f64(sec)?;
        let h = self.req(hemis)?;
        dms_to_decimal(d, m, s, &h, is_lat)
            .map_err(|msg| self.problem(deg, RowErrorKind::InvalidPosition(msg)))
    }

    /// A required position computed from DMS and compared with the published decimals.
    pub fn position(&self, c: &PosCols) -> Result<Position, Fail> {
        let lat = self.dms(c.lat_deg, c.lat_min, c.lat_sec, c.lat_hemis, true)?;
        let lon = self.dms(c.lon_deg, c.lon_min, c.lon_sec, c.lon_hemis, false)?;
        let plat = self.req_f64(c.lat_dec)?;
        let plon = self.req_f64(c.lon_dec)?;
        if plat.abs() > 90.0 {
            let msg = format!("published latitude {plat} outside [-90, 90]");
            return Err(self.problem(c.lat_dec, RowErrorKind::InvalidPosition(msg)));
        }
        if plon.abs() > 180.0 {
            let msg = format!("published longitude {plon} outside [-180, 180]");
            return Err(self.problem(c.lon_dec, RowErrorKind::InvalidPosition(msg)));
        }
        Ok(crate::position::from_parts(lat, lon, plat, plon))
    }

    /// An optional position: all ten fields empty is `None`, some empty is an error.
    pub fn opt_position(&self, c: &PosCols) -> Result<Option<Position>, Fail> {
        let cols = [
            c.lat_deg,
            c.lat_min,
            c.lat_sec,
            c.lat_hemis,
            c.lat_dec,
            c.lon_deg,
            c.lon_min,
            c.lon_sec,
            c.lon_hemis,
            c.lon_dec,
        ];
        let mut present = 0;
        for col in cols {
            if !self.text(col)?.is_empty() {
                present += 1;
            }
        }
        match present {
            0 => Ok(None),
            10 => self.position(c).map(Some),
            _ => Err(self.problem(c.lat_deg, RowErrorKind::PartialPosition)),
        }
    }
}

/// Reads every row of a file through `f`, collecting rejected rows. CSV-level errors are fatal.
pub(crate) fn parse_rows<R, T>(
    file: &str,
    reader: R,
    mut f: impl FnMut(&Row<'_>) -> Result<T, Fail>,
) -> Result<Parsed<T>, NasrError>
where
    R: BufRead,
{
    let mut csv = Reader::new(file, reader)?;
    let mut out = Parsed::default();
    while let Some(rec) = csv.next_record()? {
        match f(&Row::new(file, &rec)) {
            Ok(v) => out.records.push(v),
            Err(Fail::Row(e)) => out.rejected.push(e),
            Err(Fail::Fatal(e)) => return Err(e),
        }
    }
    Ok(out)
}

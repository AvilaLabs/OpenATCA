//! Checked fixed-width field access. Nothing here can panic: all slicing is `get`.

use crate::error::ErrorKind;
use crate::types::{parse_cycle, Altitude, Meta, Position};

/// One 132-character ASCII record. Columns are 1-based and inclusive, as in the layouts.
// A7.2: cold struct, size guard waived
pub(crate) struct Rec<'a> {
    s: &'a str,
}

impl<'a> Rec<'a> {
    pub fn new(s: &'a str) -> Self {
        Rec { s }
    }

    /// Raw text of columns `a..=b` (empty if outside the record).
    pub fn raw(&self, a: usize, b: usize) -> &'a str {
        self.s.get(a.saturating_sub(1)..b).unwrap_or("")
    }

    pub fn trim(&self, a: usize, b: usize) -> &'a str {
        self.raw(a, b).trim_matches(' ')
    }

    pub fn text(&self, a: usize, b: usize) -> String {
        self.trim(a, b).to_string()
    }

    pub fn opt_text(&self, a: usize, b: usize) -> Option<String> {
        let t = self.trim(a, b);
        (!t.is_empty()).then(|| t.to_string())
    }

    /// The character in column `c`, `' '` if absent.
    pub fn ch(&self, c: usize) -> char {
        self.raw(c, c).chars().next().unwrap_or(' ')
    }

    pub fn opt_ch(&self, c: usize) -> Option<char> {
        match self.ch(c) {
            ' ' => None,
            x => Some(x),
        }
    }

    pub fn is_blank(&self, a: usize, b: usize) -> bool {
        self.raw(a, b).bytes().all(|x| x == b' ')
    }

    pub fn bad(&self, field: &'static str, a: usize, b: usize, reason: &'static str) -> ErrorKind {
        ErrorKind::Field {
            field,
            first_col: a,
            last_col: b,
            value: self.raw(a, b).to_string(),
            reason,
        }
    }

    /// Unsigned integer of digits only; blank gives `None`.
    pub fn uint(&self, field: &'static str, a: usize, b: usize) -> Result<Option<u32>, ErrorKind> {
        let t = self.raw(a, b);
        if t.bytes().all(|x| x == b' ') {
            return Ok(None);
        }
        parse_digits(t)
            .map(Some)
            .ok_or_else(|| self.bad(field, a, b, "expected digits"))
    }

    pub fn req_uint(&self, field: &'static str, a: usize, b: usize) -> Result<u32, ErrorKind> {
        self.uint(field, a, b)?
            .ok_or_else(|| self.bad(field, a, b, "required but blank"))
    }

    /// Integer with an optional leading `-` (or `+`); blank gives `None`.
    pub fn signed(
        &self,
        field: &'static str,
        a: usize,
        b: usize,
    ) -> Result<Option<i32>, ErrorKind> {
        let t = self.raw(a, b);
        if t.bytes().all(|x| x == b' ') {
            return Ok(None);
        }
        let (neg, digits) = match t.as_bytes().first() {
            Some(b'-') => (true, t.get(1..).unwrap_or("")),
            Some(b'+') | Some(b' ') => (false, t.get(1..).unwrap_or("")),
            _ => (false, t),
        };
        match parse_digits(digits) {
            Some(v) => {
                let v = i32::try_from(v).map_err(|_| self.bad(field, a, b, "out of range"))?;
                Ok(Some(if neg { -v } else { v }))
            }
            None => Err(self.bad(field, a, b, "expected a signed integer")),
        }
    }

    /// A bearing or course in tenths of a degree (`0000` to `3600`): digits only, 0 to 360
    /// degrees; blank gives `None`. A larger value is a field that is not a bearing.
    pub fn bearing(
        &self,
        field: &'static str,
        a: usize,
        b: usize,
    ) -> Result<Option<f64>, ErrorKind> {
        match self.uint(field, a, b)? {
            None => Ok(None),
            Some(t) if t <= 3600 => Ok(Some(f64::from(t) / 10.0)),
            Some(_) => Err(self.bad(field, a, b, "expected 0 to 360 degrees")),
        }
    }

    /// An identifier: letters and digits only (upper case, as the FAA codes them); blank gives
    /// the empty string. Anything else is a field in the wrong place or a damaged record.
    pub fn ident(&self, field: &'static str, a: usize, b: usize) -> Result<String, ErrorKind> {
        let t = self.trim(a, b);
        if t.bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            Ok(t.to_string())
        } else {
            Err(self.bad(field, a, b, "expected upper-case letters and digits"))
        }
    }

    /// The section and subsection codes of a referenced record: a letter, and a letter, digit
    /// or blank.
    pub fn section_codes(
        &self,
        field: &'static str,
        sec_col: usize,
        sub_col: usize,
    ) -> Result<(char, char), ErrorKind> {
        let (sec, sub) = (self.ch(sec_col), self.ch(sub_col));
        if !sec.is_ascii_uppercase() {
            return Err(self.bad(field, sec_col, sec_col, "section code must be a letter"));
        }
        if !(sub.is_ascii_uppercase() || sub.is_ascii_digit() || sub == ' ') {
            return Err(self.bad(
                field,
                sub_col,
                sub_col,
                "subsection code must be a letter, digit or blank",
            ));
        }
        Ok((sec, sub))
    }

    /// Digits scaled by `1/div`; blank gives `None`.
    pub fn scaled(
        &self,
        field: &'static str,
        a: usize,
        b: usize,
        div: f64,
    ) -> Result<Option<f64>, ErrorKind> {
        Ok(self.uint(field, a, b)?.map(|v| f64::from(v) / div))
    }

    /// Signed digits scaled by `1/div` (sign is the first column: `+`, `-` or blank).
    pub fn signed_scaled(
        &self,
        field: &'static str,
        a: usize,
        b: usize,
        div: f64,
    ) -> Result<Option<f64>, ErrorKind> {
        Ok(self.signed(field, a, b)?.map(|v| f64::from(v) / div))
    }

    /// Magnetic variation / station declination `E0134` or `W0030`: tenths of a degree,
    /// east positive. Blank gives `None`.
    pub fn variation(&self, field: &'static str, a: usize) -> Result<Option<f64>, ErrorKind> {
        let b = a + 4;
        if self.is_blank(a, b) {
            return Ok(None);
        }
        let digits = self.raw(a + 1, b);
        let v = parse_digits(digits).ok_or_else(|| self.bad(field, a, b, "expected digits"))?;
        if v > 1800 {
            return Err(self.bad(field, a, b, "expected at most 180 degrees"));
        }
        match self.ch(a) {
            'E' => Ok(Some(f64::from(v) / 10.0)),
            'W' => Ok(Some(-(f64::from(v) / 10.0))),
            _ => Err(self.bad(field, a, b, "expected E or W")),
        }
    }

    /// A latitude/longitude pair: latitude at `a..a+8`, longitude at `a+9..a+18`.
    /// All blank gives `None`; partly blank is an error.
    pub fn position(&self, field: &'static str, a: usize) -> Result<Option<Position>, ErrorKind> {
        let b = a + 18;
        if self.is_blank(a, b) {
            return Ok(None);
        }
        let lat = parse_lat(self.raw(a, a + 8))
            .ok_or_else(|| self.bad(field, a, a + 8, "bad latitude"))?;
        let lon = parse_lon(self.raw(a + 9, b))
            .ok_or_else(|| self.bad(field, a + 9, b, "bad longitude"))?;
        Ok(Some(Position { lat, lon }))
    }

    /// An altitude: digits, `-` digits, `FLnnn`, `GND`, `UNLTD`, `UNKNN`; blank gives `None`.
    pub fn altitude(
        &self,
        field: &'static str,
        a: usize,
        b: usize,
    ) -> Result<Option<Altitude>, ErrorKind> {
        let t = self.trim(a, b);
        if t.is_empty() {
            return Ok(None);
        }
        match t {
            "GND" => return Ok(Some(Altitude::Ground)),
            "UNLTD" => return Ok(Some(Altitude::Unlimited)),
            "UNKNN" => return Ok(Some(Altitude::Unknown)),
            _ => {}
        }
        if let Some(level) = t.strip_prefix("FL") {
            return match parse_digits(level).and_then(|v| u16::try_from(v).ok()) {
                Some(l) => Ok(Some(Altitude::FlightLevel(l))),
                None => Err(self.bad(field, a, b, "bad flight level")),
            };
        }
        match self.signed(field, a, b)? {
            Some(f) => Ok(Some(Altitude::Feet(f))),
            None => Ok(None),
        }
    }

    /// The common record prefix and trailer (customer area, record number, cycle).
    pub fn meta(&self, line: usize) -> Result<Meta, ErrorKind> {
        let cycle = parse_cycle(self.raw(129, 132))
            .ok_or_else(|| self.bad("cycle date", 129, 132, "expected YYRR with release 01-14"))?;
        Ok(Meta {
            line,
            customer_area: self.text(2, 4),
            file_record_number: self.text(124, 128),
            cycle,
        })
    }
}

/// Parses a non-empty run of ASCII digits (at most nine).
pub(crate) fn parse_digits(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 9 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `N41583699` / `S` + `DDMMSSss` into signed degrees. Minutes and seconds below 60,
/// latitude not above 90.
pub fn parse_lat(s: &str) -> Option<f64> {
    parse_angle(s, 2, ['N', 'S'], 90)
}

/// `W087542934` / `E` + `DDDMMSSss` into signed degrees. Longitude not above 180.
pub fn parse_lon(s: &str) -> Option<f64> {
    parse_angle(s, 3, ['E', 'W'], 180)
}

fn parse_angle(s: &str, deg_digits: usize, hemis: [char; 2], max_deg: u32) -> Option<f64> {
    let total = 1 + deg_digits + 2 + 4;
    if s.len() != total || !s.is_ascii() {
        return None;
    }
    let first = s.chars().next()?;
    let [north_east, south_west] = hemis;
    let neg = if first == north_east {
        false
    } else if first == south_west {
        true
    } else {
        return None;
    };
    let d = parse_digits(s.get(1..1 + deg_digits)?)?;
    let m = parse_digits(s.get(1 + deg_digits..3 + deg_digits)?)?;
    let sec = parse_digits(s.get(3 + deg_digits..total)?)?; // seconds in hundredths
    if m >= 60 || sec >= 6000 || d > max_deg || (d == max_deg && (m > 0 || sec > 0)) {
        return None;
    }
    // Exact integer hundredths of an arc second, one division at the end.
    let hundredths = (u64::from(d) * 60 + u64::from(m)) * 6000 + u64::from(sec);
    let v = hundredths as f64 / 360_000.0;
    Some(if neg { -v } else { v })
}

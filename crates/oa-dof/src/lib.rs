#![forbid(unsafe_code)]
#![deny(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
//! oa-dof: parser for the FAA Digital Obstacle File (DOF), the fixed-width `.Dat` files the
//! FAA's Aeronautical Information Services publishes every 56 days. Core (pure): bytes in,
//! struct-of-arrays out; [`read_file`] is the one shell function.
//!
//! The layout is the one in the FAA's own `DOF_README` ("The DOF Format"): a currency-date line
//! (`  CURRENCY DATE = MM/DD/YY`), two column-header lines, a line of dashes, then one obstacle
//! per line with columns (1-based, inclusive):
//!
//! | columns | field |
//! |---|---|
//! | 1-2, 3, 4-9 | OAS code (state number or country letters), `-`, obstacle number |
//! | 11 | verification status, `O` verified, `U` unverified |
//! | 13-14, 16-17, 19-34 | country, state (blank outside the US), city |
//! | 36-37, 39-40, 42-46, 47 | latitude degrees, minutes, seconds (`SS.SS`), hemisphere |
//! | 49-51, 53-54, 56-60, 61 | longitude degrees, minutes, seconds, hemisphere |
//! | 63-80, 82 | obstacle type, quantity (1 digit) |
//! | 84-88, 90-94 | AGL height and AMSL height, feet (AMSL may be negative: `-0008`) |
//! | 96 | lighting |
//! | 98, 100 | horizontal and vertical accuracy codes (blank for unverified obstacles with none) |
//! | 102 | marking |
//! | 104-117, 119, 121-127 | FAA study number (may be blank), action `A`/`C`, Julian date `YYYYDDD` |
//!
//! Every separator column must be a blank. A line that breaks any rule is rejected with a typed
//! [`LineError`] and the file carries on: [`Dof::lines`] and [`Dof::rejected_total`] report how many
//! lines were read and refused, so a gap is never silent (RULESET E5). Problems that make the file
//! as a whole unusable (no header, too large) are a [`DofError`].
//!
//! Positions are stored as 1e-7 degree integers (RULESET A9), exact for seconds to 0.01. The
//! horizontal datum is WGS 84 (README, "Special Notice"). Heights are feet, AMSL relative to the
//! FAA's mean sea level (NAVD 88 for most of the CONUS). Text in `city` is Windows-1252.
//!
//! Limits: [`MAX_FILE_BYTES`], [`MAX_LINES`], [`MAX_LINE_BYTES`], [`MAX_REJECTED`].

use std::fmt;
use std::path::Path;

/// Limit: largest file, 512 MiB. The whole-country `DOF.DAT` is 85 MB.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Limit: most lines, 4 million. The whole-country file has 656,889.
pub const MAX_LINES: u64 = 4_000_000;
/// Limit: longest line, 1024 bytes. Real lines are 128 (129 with the carriage return).
pub const MAX_LINE_BYTES: usize = 1024;
/// Limit: most rejected lines kept with their reason; further ones are only counted.
pub const MAX_REJECTED: usize = 10_000;
/// Shortest data line: the last column is 127 (column 128 is blank and may be trimmed).
pub const MIN_DATA_LINE: usize = 127;
/// Header lines before the first obstacle.
pub const HEADER_LINES: u64 = 4;
/// Feet per metre divisor: 1 ft = 0.3048 m exactly.
pub const METRES_PER_FOOT: f64 = 0.3048;
/// Nautical mile in metres, exact.
pub const METRES_PER_NM: f64 = 1852.0;

/// Index of an obstacle in a [`Dof`] (RULESET A4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObstacleIx(pub u32);

impl ObstacleIx {
    /// The shared "no such obstacle" sentinel.
    pub const NONE: ObstacleIx = ObstacleIx(u32::MAX);
}

/// The currency date of a file (`CURRENCY DATE = 09/27/26`). Plain data (A1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Currency {
    /// Four-digit year (the file gives two digits; 00 to 79 are read as 20xx, 80 to 99 as 19xx).
    pub year: u16,
    /// Month 1 to 12.
    pub month: u8,
    /// Day 1 to 31.
    pub day: u8,
}

/// Which field of a line was wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// A column that must be blank.
    Separator(u8),
    /// The dash after the OAS code (column 3).
    OasDash,
    /// OAS code, columns 1-2.
    OasCode,
    /// Obstacle number, columns 4-9.
    OasNumber,
    /// Verification status, column 11.
    Verification,
    /// Country, columns 13-14.
    Country,
    /// State, columns 16-17.
    State,
    /// City, columns 19-34.
    City,
    /// Latitude degrees, minutes, seconds or hemisphere, or the whole value beyond 90 degrees.
    Latitude,
    /// Longitude degrees, minutes, seconds or hemisphere, or the whole value beyond 180 degrees.
    Longitude,
    /// Obstacle type, columns 63-80.
    Type,
    /// Quantity, column 82.
    Quantity,
    /// AGL height, columns 84-88.
    Agl,
    /// AMSL height, columns 90-94.
    Amsl,
    /// Lighting, column 96.
    Lighting,
    /// Horizontal accuracy, column 98.
    HorizontalAccuracy,
    /// Vertical accuracy, column 100.
    VerticalAccuracy,
    /// Marking, column 102.
    Marking,
    /// FAA study number, columns 104-117.
    Study,
    /// Action, column 119.
    Action,
    /// Julian date, columns 121-127.
    Julian,
}

/// Why a line was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineErrorKind {
    /// Fewer than [`MIN_DATA_LINE`] bytes.
    TooShort(usize),
    /// More than [`MAX_LINE_BYTES`] bytes.
    TooLong(usize),
    /// A field failed validation.
    Field(Field),
}

/// A rejected line. Plain data (A1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct LineError {
    /// 1-based line number in the file.
    pub line: u64,
    /// What was wrong.
    pub kind: LineErrorKind,
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            LineErrorKind::TooShort(n) => {
                write!(
                    f,
                    "line {}: {n} bytes, fewer than the {MIN_DATA_LINE} a data line needs",
                    self.line
                )
            }
            LineErrorKind::TooLong(n) => {
                write!(
                    f,
                    "line {}: {n} bytes, over the {MAX_LINE_BYTES} limit",
                    self.line
                )
            }
            LineErrorKind::Field(fl) => write!(f, "line {}: invalid {fl:?}", self.line),
        }
    }
}

/// The file as a whole is unusable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DofError {
    /// The file could not be opened or read.
    Io(String),
    /// Larger than [`MAX_FILE_BYTES`].
    FileTooLarge(u64),
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
    /// The first line is not `CURRENCY DATE = MM/DD/YY` with a real date.
    BadCurrency,
    /// Fewer than the four header lines, or the fourth is not a line of dashes.
    BadHeader,
    /// More than [`u32::MAX`] obstacles (cannot be indexed).
    TooManyObstacles,
    /// A bounding box with a bound that is not a finite number.
    BadBox,
}

impl fmt::Display for DofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DofError::Io(m) => write!(f, "i/o error: {m}"),
            DofError::FileTooLarge(n) => {
                write!(f, "file is {n} bytes, over the {MAX_FILE_BYTES} limit")
            }
            DofError::TooManyLines => write!(f, "more than {MAX_LINES} lines"),
            DofError::BadCurrency => write!(f, "first line is not a CURRENCY DATE header"),
            DofError::BadHeader => write!(f, "the four header lines are missing or malformed"),
            DofError::TooManyObstacles => write!(f, "too many obstacles to index"),
            DofError::BadBox => write!(f, "bounding box bound is not a finite number"),
        }
    }
}

impl std::error::Error for DofError {}

/// A parsed DOF file, struct-of-arrays: every `Vec` has one entry per accepted obstacle and
/// the same index ([`ObstacleIx`]). Text fields are fixed byte arrays padded with blanks.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived (the columns are the hot data; this holds only the counts)
pub struct Dof {
    /// Currency date from line 1.
    pub currency: Currency,
    /// OAS code, 2 characters: a state number (`01`) or country letters (`AG`).
    pub oas_code: Vec<[u8; 2]>,
    /// Obstacle number within the OAS code, 0 to 999,999.
    pub oas_number: Vec<u32>,
    /// `b'O'` verified, `b'U'` unverified.
    pub verification: Vec<u8>,
    /// Country, 2 letters.
    pub country: Vec<[u8; 2]>,
    /// State, 2 letters, blanks outside the US.
    pub state: Vec<[u8; 2]>,
    /// City, 16 bytes, blank padded, Windows-1252.
    pub city: Vec<[u8; 16]>,
    /// Latitude in 1e-7 degrees, north positive.
    pub lat_e7: Vec<i32>,
    /// Longitude in 1e-7 degrees, east positive.
    pub lon_e7: Vec<i32>,
    /// Obstacle type, 18 bytes, blank padded (`TOWER`, `T-L TWR`, `WINDMILL`).
    pub kind: Vec<[u8; 18]>,
    /// Quantity 1 to 9 (a group of obstacles listed as one).
    pub quantity: Vec<u8>,
    /// Height above ground level, feet.
    pub agl_ft: Vec<u32>,
    /// Height above mean sea level, feet (negative below sea level).
    pub amsl_ft: Vec<i32>,
    /// Lighting letter, see [`lighting_name`].
    pub lighting: Vec<u8>,
    /// Horizontal accuracy code `b'1'`..`b'9'`, or `b' '` when the file gives none.
    pub h_acc: Vec<u8>,
    /// Vertical accuracy code `b'A'`..`b'I'`, or `b' '` when the file gives none.
    pub v_acc: Vec<u8>,
    /// Marking letter, see [`marking_name`].
    pub marking: Vec<u8>,
    /// FAA study number, 14 bytes, blank padded (all blanks when none).
    pub study: Vec<[u8; 14]>,
    /// `b'A'` add or `b'C'` change.
    pub action: Vec<u8>,
    /// Date of the action as `YYYYDDD` (year and day of year).
    pub julian: Vec<u32>,
    /// Physical lines read, including the header and blank lines.
    pub lines: u64,
    /// Rejected lines with reasons, at most [`MAX_REJECTED`], in file order.
    pub rejected: Vec<LineError>,
    /// All rejected lines, including those past [`MAX_REJECTED`].
    pub rejected_total: u64,
}

/// Number of obstacles.
pub fn len(d: &Dof) -> usize {
    d.lat_e7.len()
}

/// Latitude in degrees.
pub fn lat_deg(d: &Dof, i: ObstacleIx) -> Option<f64> {
    d.lat_e7.get(i.0 as usize).map(|&v| f64::from(v) / 1e7)
}

/// Longitude in degrees.
pub fn lon_deg(d: &Dof, i: ObstacleIx) -> Option<f64> {
    d.lon_e7.get(i.0 as usize).map(|&v| f64::from(v) / 1e7)
}

/// `NN-NNNNNN`, the OAS number as the FAA prints it.
pub fn oas_number(d: &Dof, i: ObstacleIx) -> Option<String> {
    let code = d.oas_code.get(i.0 as usize)?;
    let n = d.oas_number.get(i.0 as usize)?;
    Some(format!(
        "{}{}-{n:06}",
        char::from(code[0]),
        char::from(code[1])
    ))
}

/// Text of a blank-padded field with the padding removed. Bytes 0x80 and up are read as
/// Windows-1252 (the file's own encoding for names like `PE\xd1UELAS`).
pub fn text(field: &[u8]) -> String {
    let end = field
        .iter()
        .rposition(|&b| b != b' ')
        .map_or(0, |p| p.saturating_add(1));
    field
        .get(..end)
        .unwrap_or(&[])
        .iter()
        .map(|&b| match b {
            0x91 => '\u{2018}',
            0x92 => '\u{2019}',
            0x93 => '\u{201c}',
            0x94 => '\u{201d}',
            0x96 => '\u{2013}',
            0x97 => '\u{2014}',
            b => char::from(b),
        })
        .collect()
}

/// Horizontal position accuracy in metres for a code (README table); `None` for unknown (`9`)
/// and for a blank or invalid code.
pub fn horizontal_accuracy_m(code: u8) -> Option<f64> {
    match code {
        b'1' => Some(20.0 * METRES_PER_FOOT),
        b'2' => Some(50.0 * METRES_PER_FOOT),
        b'3' => Some(100.0 * METRES_PER_FOOT),
        b'4' => Some(250.0 * METRES_PER_FOOT),
        b'5' => Some(500.0 * METRES_PER_FOOT),
        b'6' => Some(1000.0 * METRES_PER_FOOT),
        b'7' => Some(0.5 * METRES_PER_NM),
        b'8' => Some(METRES_PER_NM),
        _ => None,
    }
}

/// Horizontal position accuracy in feet; see [`horizontal_accuracy_m`].
pub fn horizontal_accuracy_ft(code: u8) -> Option<f64> {
    horizontal_accuracy_m(code).map(|m| m / METRES_PER_FOOT)
}

/// Vertical accuracy in feet for a code (README table); `None` for unknown (`I`) and for a blank
/// or invalid code.
pub fn vertical_accuracy_ft(code: u8) -> Option<f64> {
    match code {
        b'A' => Some(3.0),
        b'B' => Some(10.0),
        b'C' => Some(20.0),
        b'D' => Some(50.0),
        b'E' => Some(125.0),
        b'F' => Some(250.0),
        b'G' => Some(500.0),
        b'H' => Some(1000.0),
        _ => None,
    }
}

/// Vertical accuracy in metres; see [`vertical_accuracy_ft`].
pub fn vertical_accuracy_m(code: u8) -> Option<f64> {
    vertical_accuracy_ft(code).map(|f| f * METRES_PER_FOOT)
}

/// The README's name for a lighting letter.
pub fn lighting_name(code: u8) -> Option<&'static str> {
    Some(match code {
        b'R' => "Red",
        b'D' => "Medium intensity white strobe & red",
        b'H' => "High intensity white strobe & red",
        b'M' => "Medium intensity white strobe",
        b'S' => "High intensity white strobe",
        b'F' => "Flood",
        b'C' => "Dual medium catenary",
        b'W' => "Synchronized red lighting",
        b'L' => "Lighted (type unknown)",
        b'N' => "None",
        b'U' => "Unknown",
        _ => return None,
    })
}

/// The README's name for a marking letter.
pub fn marking_name(code: u8) -> Option<&'static str> {
    Some(match code {
        b'P' => "Orange or orange and white paint",
        b'W' => "White paint only",
        b'M' => "Marked",
        b'F' => "Flag marker",
        b'S' => "Spherical marker",
        b'N' => "None",
        b'U' => "Unknown",
        _ => return None,
    })
}

/// Shell: reads a whole file into memory, refusing one over [`MAX_FILE_BYTES`]. The file is opened
/// through the sealed-data guard (OA-SEAL-013): a sealed path is refused as an I/O error.
pub fn read_file(path: &Path) -> Result<Vec<u8>, DofError> {
    use std::io::Read;
    let io = |e: std::io::Error| DofError::Io(e.to_string());
    let mut f = oa_sealed::open(path).map_err(io)?;
    let len = f.metadata().map_err(io)?.len();
    if len > MAX_FILE_BYTES {
        return Err(DofError::FileTooLarge(len));
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(io)?;
    Ok(buf)
}

/// 1-based inclusive column range of a line; empty if outside it.
fn cols(line: &[u8], from: usize, to: usize) -> &[u8] {
    line.get(from.saturating_sub(1)..to).unwrap_or(&[])
}

fn digits(b: &[u8]) -> Option<u32> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    b.iter().try_fold(0u32, |a, &c| {
        a.checked_mul(10)?
            .checked_add(u32::from(c.saturating_sub(b'0')))
    })
}

/// `SS.SS` as hundredths of a second, under 60.00.
fn seconds_hundredths(b: &[u8]) -> Option<u32> {
    let (whole, dot, frac) = (cols(b, 1, 2), cols(b, 3, 3), cols(b, 4, 5));
    if b.len() != 5 || dot != b"." {
        return None;
    }
    let v = digits(whole)?
        .checked_mul(100)?
        .checked_add(digits(frac)?)?;
    (v < 6000).then_some(v)
}

/// Degrees, minutes, seconds and hemisphere to 1e-7 degrees; `max_deg` bounds the total.
fn dms_e7(
    deg: &[u8],
    min: &[u8],
    sec: &[u8],
    hemi: u8,
    pos: u8,
    neg: u8,
    max_deg: u32,
) -> Option<i32> {
    let (d, m, s) = (digits(deg)?, digits(min)?, seconds_hundredths(sec)?);
    if m >= 60 {
        return None;
    }
    let hundredths = u64::from(d)
        .checked_mul(3600)?
        .checked_add(u64::from(m).checked_mul(60)?)?
        .checked_mul(100)?
        .checked_add(u64::from(s))?;
    if hundredths > u64::from(max_deg).checked_mul(360_000)? {
        return None;
    }
    // 1e-7 degrees per hundredth of an arc second is 1e7 / 360000 = 250 / 9; round half up.
    let e7 = hundredths.checked_mul(500)?.checked_add(9)? / 18;
    let v = i32::try_from(e7).ok()?;
    match hemi {
        h if h == pos => Some(v),
        h if h == neg => v.checked_neg(),
        _ => None,
    }
}

fn is_leap(y: u32) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

fn pad<const N: usize>(b: &[u8]) -> [u8; N] {
    let mut a = [b' '; N];
    for (d, s) in a.iter_mut().zip(b) {
        *d = *s;
    }
    a
}

/// One validated line.
struct Row {
    oas_code: [u8; 2],
    oas_number: u32,
    verification: u8,
    country: [u8; 2],
    state: [u8; 2],
    city: [u8; 16],
    lat_e7: i32,
    lon_e7: i32,
    kind: [u8; 18],
    quantity: u8,
    agl_ft: u32,
    amsl_ft: i32,
    lighting: u8,
    h_acc: u8,
    v_acc: u8,
    marking: u8,
    study: [u8; 14],
    action: u8,
    julian: u32,
}

fn first(b: &[u8]) -> u8 {
    b.first().copied().unwrap_or(b' ')
}

fn parse_line(line: &[u8]) -> Result<Row, Field> {
    for c in [
        10usize, 12, 15, 18, 35, 38, 41, 48, 52, 55, 62, 81, 83, 89, 95, 97, 99, 101, 103, 118, 120,
    ] {
        if cols(line, c, c) != b" " {
            return Err(Field::Separator(u8::try_from(c).unwrap_or(u8::MAX)));
        }
    }
    if cols(line, 3, 3) != b"-" {
        return Err(Field::OasDash);
    }
    let code = cols(line, 1, 2);
    if code.len() != 2
        || !code
            .iter()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
    {
        return Err(Field::OasCode);
    }
    let oas_number = digits(cols(line, 4, 9)).ok_or(Field::OasNumber)?;
    let verification = first(cols(line, 11, 11));
    if verification != b'O' && verification != b'U' {
        return Err(Field::Verification);
    }
    let country = cols(line, 13, 14);
    if !country.iter().all(u8::is_ascii_uppercase) || country.len() != 2 {
        return Err(Field::Country);
    }
    let state = cols(line, 16, 17);
    if !state.iter().all(|&b| b == b' ' || b.is_ascii_uppercase()) {
        return Err(Field::State);
    }
    let city = cols(line, 19, 34);
    if city.iter().any(|&b| b < b' ' || b == 0x7f) {
        return Err(Field::City);
    }
    let lat_e7 = dms_e7(
        cols(line, 36, 37),
        cols(line, 39, 40),
        cols(line, 42, 46),
        first(cols(line, 47, 47)),
        b'N',
        b'S',
        90,
    )
    .ok_or(Field::Latitude)?;
    let lon_e7 = dms_e7(
        cols(line, 49, 51),
        cols(line, 53, 54),
        cols(line, 56, 60),
        first(cols(line, 61, 61)),
        b'E',
        b'W',
        180,
    )
    .ok_or(Field::Longitude)?;
    let kind = cols(line, 63, 80);
    if kind.iter().all(|&b| b == b' ') || kind.iter().any(|&b| b < b' ' || b == 0x7f) {
        return Err(Field::Type);
    }
    let quantity = digits(cols(line, 82, 82))
        .filter(|&q| q >= 1)
        .and_then(|q| u8::try_from(q).ok())
        .and_then(|q| q.checked_add(b'0'))
        .ok_or(Field::Quantity)?;
    let agl_ft = digits(cols(line, 84, 88)).ok_or(Field::Agl)?;
    let amsl = cols(line, 90, 94);
    let amsl_ft = match amsl.split_first() {
        Some((b'-', rest)) => digits(rest)
            .and_then(|v| i32::try_from(v).ok())
            .and_then(i32::checked_neg),
        _ => digits(amsl).and_then(|v| i32::try_from(v).ok()),
    }
    .ok_or(Field::Amsl)?;
    let lighting = first(cols(line, 96, 96));
    if lighting_name(lighting).is_none() {
        return Err(Field::Lighting);
    }
    let h_acc = first(cols(line, 98, 98));
    if h_acc != b' ' && !(b'1'..=b'9').contains(&h_acc) {
        return Err(Field::HorizontalAccuracy);
    }
    let v_acc = first(cols(line, 100, 100));
    if v_acc != b' ' && !(b'A'..=b'I').contains(&v_acc) {
        return Err(Field::VerticalAccuracy);
    }
    let marking = first(cols(line, 102, 102));
    if marking_name(marking).is_none() {
        return Err(Field::Marking);
    }
    let study = cols(line, 104, 117);
    if study.iter().any(|&b| !(b' '..0x7f).contains(&b)) {
        return Err(Field::Study);
    }
    let action = first(cols(line, 119, 119));
    if action != b'A' && action != b'C' {
        return Err(Field::Action);
    }
    let julian = digits(cols(line, 121, 127)).ok_or(Field::Julian)?;
    let (year, day) = (julian / 1000, julian % 1000);
    let days = if is_leap(year) { 366 } else { 365 };
    if !(1900..=2100).contains(&year) || day < 1 || day > days {
        return Err(Field::Julian);
    }
    Ok(Row {
        oas_code: pad(code),
        oas_number,
        verification,
        country: pad(country),
        state: pad(state),
        city: pad(city),
        lat_e7,
        lon_e7,
        kind: pad(kind),
        quantity,
        agl_ft,
        amsl_ft,
        lighting,
        h_acc,
        v_acc,
        marking,
        study: pad(study),
        action,
        julian,
    })
}

fn parse_currency(line: &[u8]) -> Option<Currency> {
    // Columns 3-26: `CURRENCY DATE = MM/DD/YY`.
    let body = line.get(2..)?;
    let body = std::str::from_utf8(body).ok()?.trim_end();
    let date = body.strip_prefix("CURRENCY DATE = ")?.as_bytes();
    if date.len() != 8 || date.get(2) != Some(&b'/') || date.get(5) != Some(&b'/') {
        return None;
    }
    let (m, d, y) = (
        digits(date.get(0..2)?)?,
        digits(date.get(3..5)?)?,
        digits(date.get(6..8)?)?,
    );
    let year = if y < 80 {
        y.saturating_add(2000)
    } else {
        y.saturating_add(1900)
    };
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days).contains(&d) {
        return None;
    }
    Some(Currency {
        year: u16::try_from(year).ok()?,
        month: u8::try_from(m).ok()?,
        day: u8::try_from(d).ok()?,
    })
}

/// Parses a whole DOF file (`DOF.DAT`, a state file, ...). Lines end in LF or CRLF.
pub fn parse_dof(data: &[u8]) -> Result<Dof, DofError> {
    if data.len() as u64 > MAX_FILE_BYTES {
        return Err(DofError::FileTooLarge(data.len() as u64));
    }
    let mut lines = data.split(|&b| b == b'\n');
    // A trailing newline leaves one empty piece, which is not a line.
    let strip = |l: &[u8]| -> Vec<u8> { l.strip_suffix(b"\r").unwrap_or(l).to_vec() };
    let first = lines.next().map(strip).ok_or(DofError::BadCurrency)?;
    let currency = parse_currency(&first).ok_or(DofError::BadCurrency)?;
    let mut header = Vec::new();
    for _ in 0..HEADER_LINES - 1 {
        header.push(lines.next().map(strip).ok_or(DofError::BadHeader)?);
    }
    match header.last() {
        Some(dashes) if dashes.len() >= 100 && dashes.iter().all(|&b| b == b'-') => {}
        _ => return Err(DofError::BadHeader),
    }
    let mut d = Dof {
        currency,
        oas_code: Vec::new(),
        oas_number: Vec::new(),
        verification: Vec::new(),
        country: Vec::new(),
        state: Vec::new(),
        city: Vec::new(),
        lat_e7: Vec::new(),
        lon_e7: Vec::new(),
        kind: Vec::new(),
        quantity: Vec::new(),
        agl_ft: Vec::new(),
        amsl_ft: Vec::new(),
        lighting: Vec::new(),
        h_acc: Vec::new(),
        v_acc: Vec::new(),
        marking: Vec::new(),
        study: Vec::new(),
        action: Vec::new(),
        julian: Vec::new(),
        lines: HEADER_LINES,
        rejected: Vec::new(),
        rejected_total: 0,
    };
    let mut rest = lines.peekable();
    while let Some(raw) = rest.next() {
        let last_empty = raw.is_empty() && rest.peek().is_none();
        if last_empty {
            break;
        }
        d.lines = d.lines.saturating_add(1);
        if d.lines > MAX_LINES {
            return Err(DofError::TooManyLines);
        }
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        let kind = if line.len() > MAX_LINE_BYTES {
            LineErrorKind::TooLong(line.len())
        } else if line.len() < MIN_DATA_LINE {
            LineErrorKind::TooShort(line.len())
        } else {
            match parse_line(line) {
                Ok(r) => {
                    if d.lat_e7.len() >= u32::MAX as usize {
                        return Err(DofError::TooManyObstacles);
                    }
                    d.oas_code.push(r.oas_code);
                    d.oas_number.push(r.oas_number);
                    d.verification.push(r.verification);
                    d.country.push(r.country);
                    d.state.push(r.state);
                    d.city.push(r.city);
                    d.lat_e7.push(r.lat_e7);
                    d.lon_e7.push(r.lon_e7);
                    d.kind.push(r.kind);
                    d.quantity.push(r.quantity);
                    d.agl_ft.push(r.agl_ft);
                    d.amsl_ft.push(r.amsl_ft);
                    d.lighting.push(r.lighting);
                    d.h_acc.push(r.h_acc);
                    d.v_acc.push(r.v_acc);
                    d.marking.push(r.marking);
                    d.study.push(r.study);
                    d.action.push(r.action);
                    d.julian.push(r.julian);
                    continue;
                }
                Err(f) => LineErrorKind::Field(f),
            }
        };
        d.rejected_total = d.rejected_total.saturating_add(1);
        if d.rejected.len() < MAX_REJECTED {
            d.rejected.push(LineError {
                line: d.lines,
                kind,
            });
        }
    }
    Ok(d)
}

/// Obstacles inside a latitude/longitude box (degrees, edges inclusive). A box whose
/// `lon_min` is greater than `lon_max` crosses the antimeridian. The file is ordered by latitude
/// within each state, not globally, so this is a linear scan (650,000 obstacles take a
/// millisecond or two). A bound that is not finite is refused with [`DofError::BadBox`]; a finite
/// one past the world is clamped, which keeps its order against every stored position.
pub fn in_bbox(
    d: &Dof,
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
) -> Result<Vec<ObstacleIx>, DofError> {
    if ![lat_min, lat_max, lon_min, lon_max]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(DofError::BadBox);
    }
    // Clamped to plus or minus 2e9 first, which fits an i32, so the cast does not truncate.
    #[allow(clippy::cast_possible_truncation)]
    let e7 = |v: f64| (v * 1e7).round().clamp(-2e9, 2e9) as i32;
    let (la0, la1, lo0, lo1) = (e7(lat_min), e7(lat_max), e7(lon_min), e7(lon_max));
    let mut out = Vec::new();
    for (i, (&la, &lo)) in d.lat_e7.iter().zip(d.lon_e7.iter()).enumerate() {
        let lon_in = if lo0 <= lo1 {
            lo >= lo0 && lo <= lo1
        } else {
            lo >= lo0 || lo <= lo1
        };
        if la >= la0 && la <= la1 && lon_in {
            // The parser refuses a file of u32::MAX obstacles or more, so the index fits.
            out.push(ObstacleIx(
                u32::try_from(i).map_err(|_| DofError::TooManyObstacles)?,
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

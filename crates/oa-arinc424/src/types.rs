//! Small value types shared by the record modules.

use std::fmt;

/// An AIRAC-style cycle: two-digit year and release number (`2611` = release 11 of 2026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cycle {
    pub year: u8,
    pub release: u8,
}

/// Parses four digits `YYRR`. Release must be 1..=14 (a year can hold 14 cycles).
pub fn parse_cycle(s: &str) -> Option<Cycle> {
    let mut it = s.bytes().map(|b| b.is_ascii_digit().then(|| b - b'0'));
    let (a, b, c, d) = (it.next()??, it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    let release = c * 10 + d;
    if !(1..=14).contains(&release) {
        return None;
    }
    Some(Cycle {
        year: a * 10 + b,
        release,
    })
}

/// True when `a` is later than `b`. Years are two digits, so a difference of 1..=50 years
/// forward counts as later and anything else as earlier.
pub fn cycle_is_after(a: Cycle, b: Cycle) -> bool {
    if a.year == b.year {
        return a.release > b.release;
    }
    let forward = (u16::from(a.year) + 100 - u16::from(b.year)) % 100;
    (1..=50).contains(&forward)
}

/// Feet, with flight levels converted (FL360 = 36000). `None` for ground/unlimited/unknown.
pub fn altitude_feet(a: Altitude) -> Option<i32> {
    match a {
        Altitude::Feet(f) => Some(f),
        Altitude::FlightLevel(l) => Some(i32::from(l) * 100),
        _ => None,
    }
}

impl fmt::Display for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}{:02}", self.year, self.release)
    }
}

/// A WGS-84 position in signed decimal degrees (north and east positive).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
}

/// Fields every record carries: its line, customer/area code (cols 2-4), file record
/// number (cols 124-128, may be alphanumeric or blank) and cycle (cols 129-132).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub line: usize,
    pub customer_area: String,
    pub file_record_number: String,
    pub cycle: Cycle,
}

/// A reference from one record to another by identifier, ICAO region, section and
/// subsection (e.g. a leg's fix, or an airway's fix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixRef {
    pub ident: String,
    /// ICAO region code, trimmed (`K5`, or `K`/`P` for offshore fixes).
    pub icao_region: String,
    /// Section code of the referenced record (`E`, `P`, `D`, ...).
    pub section: char,
    /// Subsection code of the referenced record (`A`, `C`, `G`, blank for VOR).
    pub subsection: char,
}

/// An altitude or altitude limit as coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Altitude {
    /// Feet (may be negative).
    Feet(i32),
    /// Flight level (`FL360` is 360).
    FlightLevel(u16),
    /// `GND`.
    Ground,
    /// `UNLTD`.
    Unlimited,
    /// `UNKNN`.
    Unknown,
}

/// Altitude description (cols 83 of a procedure leg).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltitudeDescription {
    /// `+` at or above altitude 1.
    AtOrAbove,
    /// `-` at or below altitude 1.
    AtOrBelow,
    /// `@` at altitude 1.
    At,
    /// `B` between: at or below altitude 1 and at or above altitude 2 (in this data
    /// altitude 1 is always the higher one).
    Between,
    /// Any other code, kept raw.
    Other(char),
}

/// Turn direction (col 44 of a procedure leg).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnDirection {
    Left,
    Right,
    Either,
}

/// Altitude unit indicator of an airspace limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltitudeUnit {
    /// `A`: above ground level.
    Agl,
    /// `M`: mean sea level.
    Msl,
    Other(char),
}

/// A lower or upper airspace limit with its unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AltitudeLimit {
    pub altitude: Altitude,
    pub unit: AltitudeUnit,
}

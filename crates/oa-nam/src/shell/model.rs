//! The message model: plain data for every message of the Rev G core set (RULESET A1). Shell (input-format adapter, B1, B7, C6): allocates per message, so not real-time core (C1). Its
//! `String` and `Vec` fields are what allocate. A value
//! built by hand may be invalid; [`crate::encode`] refuses it, and [`crate::parse`] only ever
//! returns values that encode again.

pub use crate::core::identity::{Facility, Header, Kind, MsgId, Profile, ALL_KINDS};

/// Field 07(b) and (c): SSR mode and code. Both present or both absent (Part II 2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Ssr {
    /// Always `b'A'`.
    pub mode: u8,
    /// Four octal digits as ASCII.
    pub code: [u8; 4],
}

/// Field 07(a): an aircraft identification, or in a MIS a functional address.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Ident {
    /// Begins with a letter, two to seven letters or digits.
    Acid(String),
    /// The text after the oblique stroke, one to six letters or digits.
    Functional(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field07 {
    pub ident: Ident,
    pub ssr: Option<Ssr>,
}

/// Field 08: flight rules (I, V, Y, Z) and type of flight (S, N, G, M, X).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field08 {
    pub rules: u8,
    pub flight_type: Option<u8>,
}

/// Field 09: number of aircraft, type designator, wake turbulence category (L, M, H, J).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field09 {
    pub count: Option<u8>,
    pub aircraft_type: String,
    pub wake: u8,
}

/// Field 10: radio, comm, nav and approach aid equipment (10a, up to 50 characters) and
/// surveillance equipment (10b, up to 20).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field10 {
    pub equipment: String,
    pub surveillance: String,
}

/// An aerodrome designator: four characters, the first a letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Aerodrome(pub [u8; 4]);

/// A time of day (Part I 3.1) or, in Field 16(b) and EET, an elapsed time: HHMM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Hhmm {
    pub hour: u8,
    pub minute: u8,
}

/// Field 13: departure aerodrome and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field13 {
    pub aerodrome: Aerodrome,
    pub time: Option<Hhmm>,
}

/// A level (Part I 3.4). `value` is hundreds of feet for A and F, tens of metres for S and M.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Level {
    /// `Annn`, below 18,000 ft.
    Altitude(u16),
    /// `Fnnn`, 18,000 ft and above.
    FlightLevel(u16),
    /// `Snnnn`, standard metric level (not permitted in NAM Fields 14, 15(b), 32).
    MetricLevel(u16),
    /// `Mnnnn`, metric altitude (not permitted in the same places).
    MetricAltitude(u16),
}

/// Field 14(c): the cleared level, or the block altitude of Attachment 2 2.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Cleared {
    Level(Level),
    /// `AnnnBmmm` or `FnnnBmmm`: low and high in hundreds of feet.
    Block {
        flight_level: bool,
        low: u16,
        high: u16,
    },
}

/// Field 14(e): crossing condition, `A` at or above, `B` at or below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Condition {
    AtOrAbove,
    AtOrBelow,
}

/// Field 14: estimate data. The boundary point is kept as its text (a designator, a latitude
/// and longitude, or a fix-radial-distance, Part I 3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field14 {
    pub point: String,
    pub time: Hhmm,
    pub cleared: Cleared,
    /// 14(d) and 14(e), both or neither.
    pub crossing: Option<(Level, Condition)>,
}

/// A speed (Part I 3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Speed {
    /// `Nnnnn`, true airspeed in knots.
    Knots(u16),
    /// `Mnnn`, Mach number in hundredths.
    Mach(u16),
    /// `Knnnn`, kilometres per hour (not permitted in 15(a)).
    Kmh(u16),
}

/// Field 15: cruising speed, requested level, route. The route is its elements in order, each
/// one token of 15(c) (a designator, `DCT`, a point with a speed and level change, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field15 {
    pub speed: Speed,
    pub level: Level,
    pub route: Vec<String>,
}

/// Field 16: destination aerodrome, total estimated elapsed time, alternates (at most two).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field16 {
    pub aerodrome: Aerodrome,
    pub eet: Option<Hhmm>,
    pub alternates: Vec<Aerodrome>,
}

/// One element of Field 18: a three or four letter indicator and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Indicator {
    pub name: String,
    pub text: String,
}

/// Field 18: `0` (nothing to report) or one or more indicator elements, each at most once.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Field18 {
    Nil,
    Items(Vec<Indicator>),
}

/// The data of one Field 22 amendment, one per amendable field.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum AmendData {
    F07(Field07),
    F08(Field08),
    F09(Field09),
    F10(Field10),
    F13(Field13),
    F14(Field14),
    F15(Field15),
    F16(Field16),
    F18(Field18),
}

/// Field 22: field indicator and amended data. `padded` records whether the indicator was
/// written with a leading zero (`07`) or without (`7`): the ICD prints both (CHG `07/AAL73`,
/// ABI `9/B744/H`), see the README.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Amendment {
    pub padded: bool,
    pub data: AmendData,
}

/// Field 31: destination facility, sector and optional text.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field31 {
    pub facility: Facility,
    /// Two letters or digits; `00` means none designated.
    pub sector: Option<[u8; 2]>,
    /// 31(c); only with a sector. Letters, digits, space, `.` and `/`, at most 28 characters.
    pub text: Option<String>,
}

/// Field 32(a): HHMMSSDD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Time32 {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub hundredths: u8,
}

/// A latitude or longitude to the second, hemisphere as `N`/`S` or `E`/`W`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Angle {
    pub degrees: u16,
    pub minutes: u8,
    pub seconds: u8,
    /// `N`/`S` for a latitude, `E`/`W` for a longitude.
    pub hemisphere: u8,
}

/// Field 32(c): ground speed in knots, or unknown (`N9999`, Attachment 2 2.15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum GroundSpeed {
    Knots(u16),
    Unknown,
}

/// Field 32(d): heading in hundredths of a degree true (0 to 35999), or unknown (`99999`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Heading {
    Hundredths(u16),
    Unknown,
}

/// Field 32: aircraft position and velocity vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Field32 {
    pub time: Time32,
    pub latitude: Angle,
    pub longitude: Angle,
    pub speed: GroundSpeed,
    pub heading: Heading,
    pub altitude: Level,
}

/// A whole message: the header and the fields the message type carries, the rest `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub struct Message {
    pub header: Header,
    pub f07: Option<Field07>,
    pub f08: Option<Field08>,
    pub f09: Option<Field09>,
    pub f10: Option<Field10>,
    pub f13: Option<Field13>,
    pub f14: Option<Field14>,
    pub f15: Option<Field15>,
    pub f16: Option<Field16>,
    pub f18: Option<Field18>,
    pub f22: Vec<Amendment>,
    pub f31: Option<Field31>,
    pub f32: Option<Field32>,
}

/// A message with only a header; the other fields are for the caller to fill.
pub fn bare(header: Header) -> Message {
    Message {
        header,
        f07: None,
        f08: None,
        f09: None,
        f10: None,
        f13: None,
        f14: None,
        f15: None,
        f16: None,
        f18: None,
        f22: Vec::new(),
        f31: None,
        f32: None,
    }
}

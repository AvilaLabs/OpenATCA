//! Field and element parsers and formatters (Part II section 2, Part I section 3). Core (pure):
//! functions of bytes and values (RULESET B1, B2). Every `parse_*` takes the bytes of one field
//! (no leading hyphen) and refuses what the field definition does not allow; every `fmt_*`
//! writes the one canonical form. Nothing here indexes a slice: lengths are checked and slice
//! patterns or `get` are used, so no input can panic (E3).

use crate::error::{ErrorKind as K, NamError};
use crate::model::*;
use std::fmt::Write;

/// The longest message the interface carries, header excluded (NAS-IC-82422100 3.2.2.3.1-a).
pub const MAX_MESSAGE_LEN: usize = 1800;
/// The longest Field 15(c) route in elements; the message limit makes this unreachable, it is
/// the stated budget for the loop (C7).
pub const MAX_ROUTE_ELEMENTS: usize = 900;
/// The longest Field 31(c) text (Attachment 2 2.17).
pub const MAX_F31_TEXT: usize = 28;
/// The longest Field 31(c) text elsewhere: 20 alphanumeric characters (Part II 2.12).
pub const MAX_F31_PLAIN: usize = 20;

fn e(kind: K, field: u8) -> NamError {
    NamError::new(kind, field)
}

pub(crate) fn is_upper(c: u8) -> bool {
    c.is_ascii_uppercase()
}

pub(crate) fn is_alnum(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

pub(crate) fn all(b: &[u8], f: fn(u8) -> bool) -> bool {
    b.iter().all(|&c| f(c))
}

/// Digits only, non-empty, at most nine of them.
pub(crate) fn num(b: &[u8]) -> Option<u32> {
    if b.is_empty() || b.len() > 9 || !all(b, |c| c.is_ascii_digit()) {
        return None;
    }
    let mut v: u32 = 0;
    for &c in b {
        v = v * 10 + u32::from(c - b'0');
    }
    Some(v)
}

pub(crate) fn text(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn sub(b: &[u8], from: usize, to: usize) -> &[u8] {
    b.get(from..to).unwrap_or(&[])
}

fn tail(b: &[u8], from: usize) -> &[u8] {
    b.get(from..).unwrap_or(&[])
}

fn split_at_byte(b: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let i = b.iter().position(|&c| c == sep)?;
    Some((sub(b, 0, i), tail(b, i + 1)))
}

/// Printable ASCII other than the three characters with a framing meaning: `(`, `)`, `-`
/// (Part III 2.4, B.2.8).
pub(crate) fn is_free_text(c: u8) -> bool {
    (0x20..=0x7e).contains(&c) && c != b'(' && c != b')' && c != b'-'
}

// ---------------------------------------------------------------------------------- units

/// HHMM, hours at most 23 and minutes at most 59 (Part I 3.1).
pub fn parse_hhmm(b: &[u8], kind: K, field: u8) -> Result<Hhmm, NamError> {
    match (b.len(), num(b)) {
        (4, Some(v)) if v / 100 <= 23 && v % 100 <= 59 => Ok(Hhmm {
            hour: (v / 100) as u8,
            minute: (v % 100) as u8,
        }),
        _ => Err(e(kind, field)),
    }
}

pub fn fmt_hhmm(t: &Hhmm, out: &mut String) {
    let _ = write!(out, "{:02}{:02}", t.hour, t.minute);
}

/// A level: `Annn` (below 18,000 ft), `Fnnn` (18,000 ft and up), and, where `metric` is true,
/// `Snnnn` and `Mnnnn` (Part I 3.4).
pub fn parse_level(b: &[u8], metric: bool) -> Option<Level> {
    match b {
        [c @ (b'A' | b'F'), d @ ..] if d.len() == 3 => {
            let v = num(d)? as u16;
            match c {
                b'A' if v < 180 => Some(Level::Altitude(v)),
                b'F' if v >= 180 => Some(Level::FlightLevel(v)),
                _ => None,
            }
        }
        [c @ (b'S' | b'M'), d @ ..] if metric && d.len() == 4 => {
            let v = num(d)? as u16;
            if *c == b'S' {
                Some(Level::MetricLevel(v))
            } else {
                Some(Level::MetricAltitude(v))
            }
        }
        _ => None,
    }
}

pub fn fmt_level(l: &Level, out: &mut String) {
    match l {
        Level::Altitude(v) => {
            let _ = write!(out, "A{v:03}");
        }
        Level::FlightLevel(v) => {
            let _ = write!(out, "F{v:03}");
        }
        Level::MetricLevel(v) => {
            let _ = write!(out, "S{v:04}");
        }
        Level::MetricAltitude(v) => {
            let _ = write!(out, "M{v:04}");
        }
    }
}

/// A speed: `Nnnnn`, `Mnnn`, and `Knnnn` where `kmh` is true (Part I 3.5). Zero is refused.
pub fn parse_speed(b: &[u8], kmh: bool) -> Option<Speed> {
    match b {
        [b'N', d @ ..] if d.len() == 4 => {
            let v = num(d)? as u16;
            (v > 0).then_some(Speed::Knots(v))
        }
        [b'M', d @ ..] if d.len() == 3 => {
            let v = num(d)? as u16;
            (v > 0).then_some(Speed::Mach(v))
        }
        [b'K', d @ ..] if kmh && d.len() == 4 => {
            let v = num(d)? as u16;
            (v > 0).then_some(Speed::Kmh(v))
        }
        _ => None,
    }
}

pub fn fmt_speed(s: &Speed, out: &mut String) {
    match s {
        Speed::Knots(v) => {
            let _ = write!(out, "N{v:04}");
        }
        Speed::Mach(v) => {
            let _ = write!(out, "M{v:03}");
        }
        Speed::Kmh(v) => {
            let _ = write!(out, "K{v:04}");
        }
    }
}

// ---------------------------------------------------------------------------------- points

fn lat_lon_ok(b: &[u8]) -> bool {
    match b.len() {
        // DDNDDDE
        7 => {
            let (Some(d1), Some(d2)) = (num(sub(b, 0, 2)), num(sub(b, 3, 6))) else {
                return false;
            };
            matches!(b.get(2), Some(b'N' | b'S'))
                && matches!(b.get(6), Some(b'E' | b'W'))
                && d1 <= 90
                && d2 <= 180
        }
        // DDMMNDDDMME
        11 => {
            let (Some(d1), Some(m1), Some(d2), Some(m2)) = (
                num(sub(b, 0, 2)),
                num(sub(b, 2, 4)),
                num(sub(b, 5, 8)),
                num(sub(b, 8, 10)),
            ) else {
                return false;
            };
            matches!(b.get(4), Some(b'N' | b'S'))
                && matches!(b.get(10), Some(b'E' | b'W'))
                && m1 < 60
                && m2 < 60
                && (d1 < 90 || (d1 == 90 && m1 == 0))
                && (d2 < 180 || (d2 == 180 && m2 == 0))
        }
        _ => false,
    }
}

fn name_ok(b: &[u8], min: usize, max: usize) -> bool {
    (min..=max).contains(&b.len()) && b.first().is_some_and(|&c| is_upper(c)) && all(b, is_alnum)
}

/// A significant point (Part I 3.2): a two to five character designator, a latitude and
/// longitude in degrees or degrees and minutes, or a fix-radial-distance.
pub fn valid_point(b: &[u8]) -> bool {
    if name_ok(b, 2, 5) || lat_lon_ok(b) {
        return true;
    }
    // Fix, bearing (000 to 360) and distance: two to five characters then six digits.
    if (8..=11).contains(&b.len()) {
        let split = b.len() - 6;
        let (name, rest) = (sub(b, 0, split), tail(b, split));
        return name_ok(name, 2, 5)
            && num(sub(rest, 0, 3)).is_some_and(|v| v <= 360)
            && num(sub(rest, 3, 6)).is_some();
    }
    false
}

// ---------------------------------------------------------------------------------- field 07

fn acid_ok(b: &[u8]) -> bool {
    name_ok(b, 2, 7)
}

pub fn parse_f07(b: &[u8], functional_ok: bool) -> Result<Field07, NamError> {
    if let [b'/', rest @ ..] = b {
        if functional_ok && (1..=6).contains(&rest.len()) && all(rest, is_alnum) {
            return Ok(Field07 {
                ident: Ident::Functional(text(rest)),
                ssr: None,
            });
        }
        return Err(e(K::FunctionalAddress, 7));
    }
    let (acid, ssr) = match split_at_byte(b, b'/') {
        Some((a, s)) => (a, Some(s)),
        None => (b, None),
    };
    if !acid_ok(acid) {
        return Err(e(K::Acid, 7));
    }
    let ssr = match ssr {
        None => None,
        Some([m, code @ ..]) => {
            if *m != b'A' {
                return Err(e(K::SsrMode, 7));
            }
            match <[u8; 4]>::try_from(code) {
                Ok(c) if c.iter().all(|d| (b'0'..=b'7').contains(d)) => Some(Ssr {
                    mode: b'A',
                    code: c,
                }),
                _ => return Err(e(K::SsrCode, 7)),
            }
        }
        Some([]) => return Err(e(K::SsrMode, 7)),
    };
    Ok(Field07 {
        ident: Ident::Acid(text(acid)),
        ssr,
    })
}

pub fn fmt_f07(f: &Field07, out: &mut String) {
    match &f.ident {
        Ident::Acid(a) => out.push_str(a),
        Ident::Functional(a) => {
            out.push('/');
            out.push_str(a);
        }
    }
    if let Some(s) = &f.ssr {
        out.push('/');
        out.push(s.mode as char);
        out.push_str(&text(&s.code));
    }
}

// ---------------------------------------------------------------------------------- 08, 09, 10

pub fn parse_f08(b: &[u8]) -> Result<Field08, NamError> {
    let (rules, ty) = match b {
        [r] => (*r, None),
        [r, t] => (*r, Some(*t)),
        _ => return Err(e(K::FlightRules, 8)),
    };
    if !matches!(rules, b'I' | b'V' | b'Y' | b'Z') {
        return Err(e(K::FlightRules, 8));
    }
    if let Some(t) = ty {
        if !matches!(t, b'S' | b'N' | b'G' | b'M' | b'X') {
            return Err(e(K::FlightType, 8));
        }
    }
    Ok(Field08 {
        rules,
        flight_type: ty,
    })
}

pub fn fmt_f08(f: &Field08, out: &mut String) {
    out.push(f.rules as char);
    if let Some(t) = f.flight_type {
        out.push(t as char);
    }
}

pub fn parse_f09(b: &[u8]) -> Result<Field09, NamError> {
    let Some((ty, wake)) = split_at_byte(b, b'/') else {
        return Err(e(K::WakeCategory, 9));
    };
    let digits = ty.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 2 {
        return Err(e(K::AircraftCount, 9));
    }
    let (count, ac) = (sub(ty, 0, digits), tail(ty, digits));
    let count = if count.is_empty() {
        None
    } else {
        match num(count) {
            Some(v) if v >= 1 && !(count.len() == 2 && count.first() == Some(&b'0')) => {
                Some(v as u8)
            }
            _ => return Err(e(K::AircraftCount, 9)),
        }
    };
    if !name_ok(ac, 2, 4) {
        return Err(e(K::AircraftType, 9));
    }
    let wake = match wake {
        [w @ (b'L' | b'M' | b'H' | b'J')] => *w,
        _ => return Err(e(K::WakeCategory, 9)),
    };
    Ok(Field09 {
        count,
        aircraft_type: text(ac),
        wake,
    })
}

pub fn fmt_f09(f: &Field09, out: &mut String) {
    if let Some(c) = f.count {
        let _ = write!(out, "{c}");
    }
    out.push_str(&f.aircraft_type);
    out.push('/');
    out.push(f.wake as char);
}

pub fn parse_f10(b: &[u8]) -> Result<Field10, NamError> {
    let Some((a, s)) = split_at_byte(b, b'/') else {
        return Err(e(K::EquipmentSurveillance, 10));
    };
    if a.is_empty() || !all(a, is_alnum) {
        return Err(e(K::EquipmentComm, 10));
    }
    if a.len() > 50 {
        return Err(e(K::EquipmentCommLength, 10));
    }
    if s.is_empty() || !all(s, is_alnum) {
        return Err(e(K::EquipmentSurveillance, 10));
    }
    if s.len() > 20 {
        return Err(e(K::EquipmentSurveillanceLength, 10));
    }
    equipment_ok(a)?;
    surveillance_ok(s)?;
    Ok(Field10 {
        equipment: text(a),
        surveillance: text(s),
    })
}

/// Splits 10(a) into designators: a letter, or `E`, `J`, `M`, `P` with the digit ICAO Doc 4444
/// (2012) gives them (E1 to E3, J1 to J7, M1 to M3, P1 to P9). `None` for a stray digit or a
/// letter that takes a digit and has none.
fn equipment_items(a: &[u8]) -> Option<Vec<[u8; 2]>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = a.get(i) {
        let max = match c {
            b'E' => b'3',
            b'J' => b'7',
            b'M' => b'3',
            b'P' => b'9',
            b'A'..=b'D' | b'F'..=b'I' | b'K' | b'L' | b'N' | b'O' | b'R' | b'S' | b'T'..=b'Z' => {
                out.push([c, 0]);
                i += 1;
                continue;
            }
            _ => return None,
        };
        let d = *a.get(i + 1)?;
        if !(b'1'..=max).contains(&d) {
            return None;
        }
        out.push([c, d]);
        i += 2;
    }
    Some(out)
}

/// Syntax of 10(a) (Table A-1 codes 71, 73, 75; the ICD defers the values to Doc 4444):
/// designators valid, none repeated, `N` alone, and `S` not together with the V, O, L it
/// stands for. Whether the aircraft really has the equipment is not checkable here.
fn equipment_ok(a: &[u8]) -> Result<(), NamError> {
    let items = equipment_items(a).ok_or_else(|| e(K::EquipmentComm, 10))?;
    for (i, x) in items.iter().enumerate() {
        if items.iter().take(i).any(|y| y[0] == x[0]) {
            return Err(e(K::EquipmentCommDuplicate, 10));
        }
    }
    let has = |c: u8| items.iter().any(|x| x[0] == c);
    if (has(b'N') && items.len() > 1) || (has(b'S') && (has(b'V') || has(b'O') || has(b'L'))) {
        return Err(e(K::EquipmentCommCombination, 10));
    }
    Ok(())
}

/// Syntax of 10(b) (codes 65, 66, 72, 74, 76): `N` alone, or Mode A and C, at most one Mode S
/// letter (E H I L P S X), ADS-B `B1 B2 U1 U2 V1 V2` (one value per letter), ADS-C `D1 G1`.
fn surveillance_ok(s: &[u8]) -> Result<(), NamError> {
    let mut seen: Vec<u8> = Vec::new();
    let mut count = 0usize;
    let mut mode_s = 0usize;
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        match c {
            b'N' | b'A' | b'C' => i += 1,
            b'E' | b'H' | b'I' | b'L' | b'P' | b'S' | b'X' => {
                mode_s += 1;
                i += 1;
            }
            b'B' | b'U' | b'V' | b'D' | b'G' => {
                let d = s.get(i + 1).copied();
                let ok = match c {
                    b'D' | b'G' => d == Some(b'1'),
                    _ => matches!(d, Some(b'1' | b'2')),
                };
                if !ok {
                    let kind = if matches!(c, b'D' | b'G') {
                        K::AdsC
                    } else {
                        K::AdsB
                    };
                    return Err(e(kind, 10));
                }
                i += 2;
            }
            _ => return Err(e(K::EquipmentSurveillance, 10)),
        }
        count += 1;
        if seen.contains(&c) {
            return Err(e(K::EquipmentSurveillanceDuplicate, 10));
        }
        seen.push(c);
    }
    if mode_s > 1 || (seen.contains(&b'N') && count > 1) {
        return Err(e(K::EquipmentSurveillanceCombination, 10));
    }
    Ok(())
}

/// Items 10 and 18 against each other (codes 64, 86, 91), where one message carries both:
/// * `N` (no equipment) with COM/, NAV/ or DAT/: 64;
/// * PBN/ without R in 10(a): 86;
/// * R without PBN/, Z without COM/, NAV/ or DAT/, or G without NAV/: 91 ("R, G, Z with no
///   specification in field 18", Attachment 2 Table 2-2). G needing NAV/ is this crate's
///   reading of Doc 4444 and is marked for review in the README.
pub fn check_items_10_18(f10: &Field10, f18: &Field18) -> Result<(), NamError> {
    let Field18::Items(items) = f18 else {
        // Field 18 of `0` says nothing: R, G and Z are then unspecified.
        return check_unspecified(f10, &[]);
    };
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    check_unspecified(f10, &names)
}

fn check_unspecified(f10: &Field10, names: &[&str]) -> Result<(), NamError> {
    let a = f10.equipment.as_bytes();
    let Some(items) = equipment_items(a) else {
        return Ok(());
    };
    let has = |c: u8| items.iter().any(|x| x[0] == c);
    let named = |n: &str| names.contains(&n);
    if has(b'N') && (named("COM") || named("NAV") || named("DAT")) {
        return Err(e(K::ItemInconsistent, 10));
    }
    if named("PBN") && !has(b'R') {
        return Err(e(K::PbnNotInEquipment, 18));
    }
    if (has(b'R') && !named("PBN"))
        || (has(b'Z') && !(named("COM") || named("NAV") || named("DAT")))
        || (has(b'G') && !named("NAV"))
    {
        return Err(e(K::ItemUnspecified, 10));
    }
    Ok(())
}

pub fn fmt_f10(f: &Field10, out: &mut String) {
    out.push_str(&f.equipment);
    out.push('/');
    out.push_str(&f.surveillance);
}

// ---------------------------------------------------------------------------------- 13, 16

fn aerodrome(b: &[u8]) -> Option<Aerodrome> {
    if name_ok(b, 4, 4) {
        <[u8; 4]>::try_from(b).ok().map(Aerodrome)
    } else {
        None
    }
}

pub fn parse_f13(b: &[u8]) -> Result<Field13, NamError> {
    if b.len() != 4 && b.len() != 8 {
        return Err(e(K::TimeInvalid, 13));
    }
    let Some(ad) = aerodrome(sub(b, 0, 4)) else {
        return Err(e(K::DepartureAerodrome, 13));
    };
    let time = if b.len() == 8 {
        Some(parse_hhmm(tail(b, 4), K::TimeInvalid, 13)?)
    } else {
        None
    };
    Ok(Field13 {
        aerodrome: ad,
        time,
    })
}

pub fn fmt_f13(f: &Field13, out: &mut String) {
    out.push_str(&text(&f.aerodrome.0));
    if let Some(t) = &f.time {
        fmt_hhmm(t, out);
    }
}

pub fn parse_f16(b: &[u8]) -> Result<Field16, NamError> {
    let Some(ad) = aerodrome(sub(b, 0, 4)) else {
        return Err(e(K::DestinationAerodrome, 16));
    };
    if b.len() == 4 {
        return Ok(Field16 {
            aerodrome: ad,
            eet: None,
            alternates: Vec::new(),
        });
    }
    if b.len() < 8 {
        return Err(e(K::TimeInvalid, 16));
    }
    let eet = parse_hhmm(sub(b, 4, 8), K::TimeInvalid, 16)?;
    let mut alts = Vec::new();
    let mut rest = tail(b, 8);
    while !rest.is_empty() {
        let [b' ', a @ ..] = rest else {
            return Err(e(K::AlternateAerodrome, 16));
        };
        let Some(alt) = aerodrome(sub(a, 0, 4)) else {
            return Err(e(K::AlternateAerodrome, 16));
        };
        if alts.len() == 2 {
            return Err(e(K::AlternateUnexpected, 16));
        }
        alts.push(alt);
        rest = tail(a, 4);
    }
    Ok(Field16 {
        aerodrome: ad,
        eet: Some(eet),
        alternates: alts,
    })
}

pub fn fmt_f16(f: &Field16, out: &mut String) {
    out.push_str(&text(&f.aerodrome.0));
    if let Some(t) = &f.eet {
        fmt_hhmm(t, out);
    }
    for a in &f.alternates {
        out.push(' ');
        out.push_str(&text(&a.0));
    }
}

// ---------------------------------------------------------------------------------- 14

pub fn parse_f14(b: &[u8], profile: Profile) -> Result<Field14, NamError> {
    let Some((point, rest)) = split_at_byte(b, b'/') else {
        return Err(e(K::BoundaryPoint, 14));
    };
    if !valid_point(point) {
        return Err(e(K::BoundaryPoint, 14));
    }
    let time = parse_hhmm(sub(rest, 0, 4), K::TimeInvalid, 14)?;
    let lv = tail(rest, 4);
    if lv.is_empty() {
        return Err(e(K::LevelMissing, 14));
    }
    let Some(first) = parse_level(sub(lv, 0, 4), false) else {
        return Err(e(K::Level, 14));
    };
    let mut pos = 4;
    let mut cleared = Cleared::Level(first);
    if lv.get(pos) == Some(&b'B') {
        // Block altitude AnnnBmmm or FnnnBmmm: Attachment 2 2.6 only, not Part II.
        if profile != Profile::EramAttachment2 {
            return Err(e(K::Level, 14));
        }
        let high = num(sub(lv, pos + 1, pos + 4)).filter(|_| lv.len() >= pos + 4);
        let (Some(high), Level::Altitude(low) | Level::FlightLevel(low)) = (high, first) else {
            return Err(e(K::Level, 14));
        };
        let high = high as u16;
        let flight_level = matches!(first, Level::FlightLevel(_));
        let ok = low < high
            && if flight_level {
                high <= 999
            } else {
                high < 180
            };
        if !ok {
            return Err(e(K::Level, 14));
        }
        cleared = Cleared::Block {
            flight_level,
            low,
            high,
        };
        pos += 4;
    }
    let mut crossing = None;
    if pos < lv.len() {
        let Some(level) = parse_level(sub(lv, pos, pos + 4), false) else {
            return Err(e(K::SupplementaryLevel, 14));
        };
        let cond = match tail(lv, pos + 4) {
            [] => return Err(e(K::CrossingConditionMissing, 14)),
            [b'A'] => Condition::AtOrAbove,
            [b'B'] => Condition::AtOrBelow,
            _ => return Err(e(K::CrossingCondition, 14)),
        };
        crossing = Some((level, cond));
    }
    Ok(Field14 {
        point: text(point),
        time,
        cleared,
        crossing,
    })
}

pub fn fmt_f14(f: &Field14, out: &mut String) {
    out.push_str(&f.point);
    out.push('/');
    fmt_hhmm(&f.time, out);
    match &f.cleared {
        Cleared::Level(l) => fmt_level(l, out),
        Cleared::Block {
            flight_level,
            low,
            high,
        } => {
            let c = if *flight_level { 'F' } else { 'A' };
            let _ = write!(out, "{c}{low:03}B{high:03}");
        }
    }
    if let Some((l, c)) = &f.crossing {
        fmt_level(l, out);
        out.push(match c {
            Condition::AtOrAbove => 'A',
            Condition::AtOrBelow => 'B',
        });
    }
}

// ---------------------------------------------------------------------------------- 15

/// Speed then up to `max_levels` levels (or `PLUS` where `plus` is true), all metric forms
/// allowed: the form of 15(c4) and 15(c6) (Part II 2.8).
fn speed_levels_ok(b: &[u8], max_levels: usize, plus: bool) -> bool {
    let mut rest = b;
    if let Some(&c) = rest.first() {
        // `M` then four digits is a metric altitude, not a Mach number.
        let metric_altitude = c == b'M' && rest.get(4).is_some_and(|d| d.is_ascii_digit());
        if matches!(c, b'N' | b'M' | b'K') && !metric_altitude {
            let n = if c == b'M' { 4 } else { 5 };
            if parse_speed(sub(rest, 0, n), true).is_none() {
                return false;
            }
            rest = tail(rest, n);
        }
    }
    let mut levels = 0;
    while !rest.is_empty() {
        if plus && rest == b"PLUS" {
            return levels > 0;
        }
        let n = if matches!(rest.first(), Some(b'S' | b'M')) {
            5
        } else {
            4
        };
        if parse_level(sub(rest, 0, n), true).is_none() || levels == max_levels {
            return false;
        }
        levels += 1;
        rest = tail(rest, n);
    }
    levels > 0 || b.len() >= 4
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tok {
    Point,
    Dct,
    Designator,
    Rules,
    Truncation,
    Other,
}

fn route_token(t: &[u8]) -> Result<Tok, K> {
    if t == b"DCT" {
        return Ok(Tok::Dct);
    }
    if t == b"IFR" || t == b"VFR" {
        return Ok(Tok::Rules);
    }
    if t == b"T" {
        return Ok(Tok::Truncation);
    }
    if let Some((first, rest)) = split_at_byte(t, b'/') {
        if first == b"C" {
            // Cruise climb: C/point/speed level level (or PLUS).
            let Some((p, sl)) = split_at_byte(rest, b'/') else {
                return Err(K::CruiseClimb);
            };
            if !valid_point(p) || !speed_levels_ok(sl, 2, true) {
                return Err(K::CruiseClimb);
            }
            return Ok(Tok::Other);
        }
        // Point with a speed and level change (c6).
        if !valid_point(first) {
            return Err(K::RoutePoint);
        }
        if rest.is_empty() || !speed_levels_ok(rest, 1, false) {
            return Err(K::RouteElement);
        }
        return Ok(Tok::Point);
    }
    if valid_point(t) {
        return Ok(Tok::Point);
    }
    if name_ok(t, 2, 7) {
        return Ok(Tok::Designator);
    }
    Err(K::RouteElement)
}

pub fn parse_f15(b: &[u8]) -> Result<Field15, NamError> {
    if b.len() > MAX_MESSAGE_LEN {
        return Err(e(K::Length, 15));
    }
    let Some((head, route)) = split_at_byte(b, b' ') else {
        return Err(e(K::RouteElement, 15));
    };
    let (speed, level) = match head.first() {
        Some(b'N') => (sub(head, 0, 5), tail(head, 5)),
        Some(b'M') => (sub(head, 0, 4), tail(head, 4)),
        _ => return Err(e(K::Speed, 15)),
    };
    let Some(speed) = parse_speed(speed, false) else {
        return Err(e(K::Speed, 15));
    };
    if level.is_empty() {
        return Err(e(K::LevelMissing, 15));
    }
    let Some(level) = parse_level(level, false) else {
        return Err(e(K::Level, 15));
    };
    let mut elements = Vec::new();
    let mut prev: Option<Tok> = None;
    for t in route.split(|&c| c == b' ') {
        if elements.len() == MAX_ROUTE_ELEMENTS {
            return Err(e(K::Length, 15));
        }
        if prev == Some(Tok::Truncation) {
            return Err(e(K::AfterTruncation, 15));
        }
        let k = route_token(t).map_err(|k| e(k, 15))?;
        if k == Tok::Rules && !matches!(prev, Some(Tok::Point | Tok::Designator | Tok::Other)) {
            return Err(e(K::FlightRulesIndicator, 15));
        }
        prev = Some(k);
        elements.push(text(t));
    }
    Ok(Field15 {
        speed,
        level,
        route: elements,
    })
}

pub fn fmt_f15(f: &Field15, out: &mut String) {
    fmt_speed(&f.speed, out);
    fmt_level(&f.level, out);
    for t in &f.route {
        out.push(' ');
        out.push_str(t);
    }
}

// ---------------------------------------------------------------------------------- 18

fn days_in_month(yy: u32, mm: u32) -> u32 {
    match mm {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        // 2000 to 2099: a leap year when divisible by four.
        2 => {
            if yy & 3 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// `DOF/YYMMDD` value: six digits, a real calendar date in 2000 to 2099 (Part I 3.1).
pub fn dof_ok(b: &[u8]) -> bool {
    let (Some(yy), Some(mm), Some(dd)) = (num(sub(b, 0, 2)), num(sub(b, 2, 4)), num(sub(b, 4, 6)))
    else {
        return false;
    };
    b.len() == 6 && dd >= 1 && dd <= days_in_month(yy, mm)
}

fn eet_ok(b: &[u8]) -> bool {
    b.split(|&c| c == b' ').all(|t| {
        t.len() >= 6
            && t.len() <= 15
            && all(sub(t, 0, t.len() - 4), is_alnum)
            && parse_hhmm(tail(t, t.len() - 4), K::Eet, 18).is_ok()
    })
}

/// PBN/ is up to eight two-character values (letter, digit): more is code 84, a malformed one 77.
fn pbn_check(b: &[u8]) -> Result<(), K> {
    let shaped = !b.is_empty()
        && b.len() & 1 == 0
        && b.chunks(2)
            .all(|p| matches!(p, [l, d] if is_upper(*l) && d.is_ascii_digit()));
    match (shaped, b.len() > 16) {
        (true, false) => Ok(()),
        (true, true) => Err(K::PbnTooMany),
        _ => Err(K::Pbn),
    }
}

/// Does `b` start a new indicator at `i`: three or four letters and a slash.
fn indicator_at(b: &[u8], i: usize) -> Option<usize> {
    let rest = tail(b, i);
    let n = rest.iter().take_while(|c| c.is_ascii_uppercase()).count();
    ((3..=4).contains(&n) && rest.get(n) == Some(&b'/')).then_some(n)
}

pub fn parse_f18(b: &[u8]) -> Result<Field18, NamError> {
    if b.len() > MAX_MESSAGE_LEN {
        return Err(e(K::Length, 18));
    }
    if b == b"0" {
        return Ok(Field18::Nil);
    }
    let mut items: Vec<Indicator> = Vec::new();
    let mut pos = 0;
    while pos < b.len() {
        let Some(n) = indicator_at(b, pos) else {
            return Err(e(K::OtherInfo, 18));
        };
        let name = sub(b, pos, pos + n);
        let start = pos + n + 1;
        // The text runs to the next " XXX/" or the end.
        let mut end = start;
        while end < b.len() && !(b.get(end) == Some(&b' ') && indicator_at(b, end + 1).is_some()) {
            end += 1;
        }
        let body = sub(b, start, end);
        if body.is_empty()
            || !all(body, is_free_text)
            || body.first() == Some(&b' ')
            || body.last() == Some(&b' ')
        {
            return Err(e(K::OtherInfo, 18));
        }
        if items.iter().any(|i| i.name.as_bytes() == name) {
            return Err(e(K::OtherInfo, 18));
        }
        match name {
            b"DOF" if !dof_ok(body) => return Err(e(K::DateOfFlight, 18)),
            b"EET" if !eet_ok(body) => return Err(e(K::Eet, 18)),
            b"PBN" => pbn_check(body).map_err(|k| e(k, 18))?,
            _ => {}
        }
        items.push(Indicator {
            name: text(name),
            text: text(body),
        });
        pos = end + 1;
    }
    if items.is_empty() {
        return Err(e(K::OtherInfo, 18));
    }
    Ok(Field18::Items(items))
}

pub fn fmt_f18(f: &Field18, out: &mut String) {
    match f {
        Field18::Nil => out.push('0'),
        Field18::Items(items) => {
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&it.name);
                out.push('/');
                out.push_str(&it.text);
            }
        }
    }
}

/// The Field 18 of a MIS or LRM: `RMK/` and free text, nothing split (B.2.8).
pub fn parse_remark(b: &[u8]) -> Result<Field18, NamError> {
    if b.len() > MAX_MESSAGE_LEN {
        return Err(e(K::Length, 18));
    }
    match b {
        [b'R', b'M', b'K', b'/', body @ ..] if !body.is_empty() && all(body, is_free_text) => {
            Ok(Field18::Items(vec![Indicator {
                name: "RMK".into(),
                text: text(body),
            }]))
        }
        _ => Err(e(K::OtherInfo, 18)),
    }
}

/// The remark text of a Field 18 that holds only `RMK/`.
pub fn remark_text(f: &Field18) -> Option<&str> {
    match f {
        Field18::Items(v) => match v.as_slice() {
            [Indicator { name, text }] if name == "RMK" => Some(text),
            _ => None,
        },
        Field18::Nil => None,
    }
}

/// An LRM's remark: error code (two or three digits, 1 to 255), field in error (two digits, 00
/// when not field-specific) and text, separated by oblique strokes (Table 17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LrmText {
    pub code: u8,
    pub field: u8,
    pub text: String,
}

pub fn parse_lrm_text(t: &str) -> Result<LrmText, NamError> {
    let bad = || e(K::LrmText, 18);
    let b = t.as_bytes();
    let (code, rest) = split_at_byte(b, b'/').ok_or_else(bad)?;
    let (field, text_b) = split_at_byte(rest, b'/').ok_or_else(bad)?;
    if !(2..=3).contains(&code.len()) || field.len() != 2 || text_b.is_empty() {
        return Err(bad());
    }
    let code = num(code)
        .filter(|&c| (1..=255).contains(&c))
        .ok_or_else(bad)?;
    let field = num(field).ok_or_else(bad)?;
    Ok(LrmText {
        code: code as u8,
        field: field as u8,
        text: text(text_b),
    })
}

pub fn fmt_lrm_text(l: &LrmText) -> String {
    format!("{:02}/{:02}/{}", l.code, l.field, l.text)
}

// ---------------------------------------------------------------------------------- 22

/// The field numbers a Field 22 can amend.
pub const AMENDABLE: [u8; 9] = [7, 8, 9, 10, 13, 14, 15, 16, 18];

pub fn amend_field_number(d: &AmendData) -> u8 {
    match d {
        AmendData::F07(_) => 7,
        AmendData::F08(_) => 8,
        AmendData::F09(_) => 9,
        AmendData::F10(_) => 10,
        AmendData::F13(_) => 13,
        AmendData::F14(_) => 14,
        AmendData::F15(_) => 15,
        AmendData::F16(_) => 16,
        AmendData::F18(_) => 18,
    }
}

/// One `NN/data` (or `N/data`) amendment, the bytes after the hyphen.
pub fn parse_amendment(b: &[u8], profile: Profile) -> Result<Amendment, NamError> {
    let bad = || e(K::AmendmentField, 22);
    let (ind, data) = split_at_byte(b, b'/').ok_or_else(bad)?;
    if ind.is_empty() || ind.len() > 2 {
        return Err(bad());
    }
    let n = num(ind).ok_or_else(bad)? as u8;
    let padded = ind.len() == 2;
    let data = match n {
        7 => AmendData::F07(parse_f07(data, false)?),
        8 => AmendData::F08(parse_f08(data)?),
        9 => AmendData::F09(parse_f09(data)?),
        10 => AmendData::F10(parse_f10(data)?),
        13 => AmendData::F13(parse_f13(data)?),
        14 => AmendData::F14(parse_f14(data, profile)?),
        15 => AmendData::F15(parse_f15(data)?),
        16 => AmendData::F16(parse_f16(data)?),
        18 => AmendData::F18(parse_f18(data)?),
        _ => return Err(bad()),
    };
    Ok(Amendment { padded, data })
}

pub fn fmt_amendment(a: &Amendment, out: &mut String) {
    let n = amend_field_number(&a.data);
    if a.padded {
        let _ = write!(out, "{n:02}/");
    } else {
        let _ = write!(out, "{n}/");
    }
    match &a.data {
        AmendData::F07(f) => fmt_f07(f, out),
        AmendData::F08(f) => fmt_f08(f, out),
        AmendData::F09(f) => fmt_f09(f, out),
        AmendData::F10(f) => fmt_f10(f, out),
        AmendData::F13(f) => fmt_f13(f, out),
        AmendData::F14(f) => fmt_f14(f, out),
        AmendData::F15(f) => fmt_f15(f, out),
        AmendData::F16(f) => fmt_f16(f, out),
        AmendData::F18(f) => fmt_f18(f, out),
    }
}

// ---------------------------------------------------------------------------------- 31

/// What Field 31(c) may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text31 {
    /// Part II 2.12: up to 20 alphanumeric characters (POI, PLA, POA, POJ, RLA).
    Plain,
    /// Attachment 2 2.17, the RTA: the CPDLC contact `frequency/facility name/function`, up
    /// to 28 characters.
    Contact,
}

pub fn parse_f31(b: &[u8], kind: Text31) -> Result<Field31, NamError> {
    let bad = || e(K::Field31, 31);
    let fac = sub(b, 0, 4);
    if b.len() < 4 || !(fac.len() == 4 && all(fac, is_upper)) {
        return Err(bad());
    }
    let facility = Facility(<[u8; 4]>::try_from(fac).map_err(|_| bad())?);
    let rest = tail(b, 4);
    let (sector, text_b) = match rest.len() {
        0 => (None, &[][..]),
        1 => return Err(bad()),
        _ => (
            Some(<[u8; 2]>::try_from(sub(rest, 0, 2)).map_err(|_| bad())?),
            tail(rest, 2),
        ),
    };
    if let Some(s) = sector {
        if !all(&s, is_alnum) {
            return Err(bad());
        }
    }
    let text_opt = if text_b.is_empty() {
        None
    } else {
        let ok = match kind {
            Text31::Plain => text_b.len() <= MAX_F31_PLAIN && all(text_b, is_alnum),
            Text31::Contact => {
                text_b.len() <= MAX_F31_TEXT
                    && std::str::from_utf8(text_b).is_ok_and(|t| parse_contact(t).is_ok())
            }
        };
        if !ok {
            return Err(bad());
        }
        Some(text(text_b))
    };
    Ok(Field31 {
        facility,
        sector,
        text: text_opt,
    })
}

pub fn fmt_f31(f: &Field31, out: &mut String) {
    out.push_str(&text(&f.facility.0));
    if let Some(s) = &f.sector {
        out.push_str(&text(s));
    }
    if let Some(t) = &f.text {
        out.push_str(t);
    }
}

/// The CPDLC contact data of an RTA acceptance in 31(c) (Attachment 2 2.17): frequency,
/// facility name, facility function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    /// `ddd.ddd` MHz, or `dddd`/`ddddd` kHz, or `000.000` for none.
    pub frequency: String,
    pub facility_name: String,
    /// 0 center, 1 approach.
    pub function: u8,
}

pub fn parse_contact(t: &str) -> Result<Contact, NamError> {
    let bad = || e(K::Field31, 31);
    let mut parts = t.split('/');
    let (Some(freq), Some(name), Some(func), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad());
    };
    let fb = freq.as_bytes();
    let freq_ok = if fb.len() == 7 {
        // VHF 118.000 to 136.975 in steps of 0.025, or 000.000.
        let (Some(mhz), Some(khz)) = (num(sub(fb, 0, 3)), num(sub(fb, 4, 7))) else {
            return Err(bad());
        };
        fb.get(3) == Some(&b'.')
            && ((mhz == 0 && khz == 0)
                || ((118..=136).contains(&mhz) && khz % 25 == 0 && (mhz < 136 || khz <= 975)))
    } else {
        // HF 2850 to 28000 kHz.
        (4..=5).contains(&fb.len()) && num(fb).is_some_and(|v| (2850..=28000).contains(&v))
    };
    let nb = name.as_bytes();
    let name_ok = (3..=18).contains(&nb.len()) && all(nb, |c| is_alnum(c) || c == b' ');
    let function = match func.as_bytes() {
        [c @ (b'0' | b'1')] => *c - b'0',
        _ => return Err(bad()),
    };
    if !freq_ok || !name_ok {
        return Err(bad());
    }
    Ok(Contact {
        frequency: freq.into(),
        facility_name: name.into(),
        function,
    })
}

// ---------------------------------------------------------------------------------- 32

fn angle(b: &[u8], deg_digits: usize, hemis: [u8; 2], max_deg: u16) -> Option<Angle> {
    let n = deg_digits + 4;
    if b.len() != n + 1 {
        return None;
    }
    let degrees = num(sub(b, 0, deg_digits))? as u16;
    let minutes = num(sub(b, deg_digits, deg_digits + 2))? as u8;
    let seconds = num(sub(b, deg_digits + 2, n))? as u8;
    let h = *b.get(n)?;
    if !hemis.contains(&h) || minutes > 59 || seconds > 59 || degrees > max_deg {
        return None;
    }
    if degrees == max_deg && (minutes != 0 || seconds != 0) {
        return None;
    }
    Some(Angle {
        degrees,
        minutes,
        seconds,
        hemisphere: h,
    })
}

/// Total length of Field 32: 8 + 15 + 5 + 5 + 4 (Part II 2.13, fixed-length elements).
pub const F32_LEN: usize = 37;

pub fn parse_f32(b: &[u8], profile: Profile) -> Result<Field32, NamError> {
    let bad = || e(K::Field32, 32);
    // 37 bytes; Rev E's metric altitude (`S` or `M` and four digits) makes it 38.
    if b.len() != F32_LEN && !(profile == Profile::RevE && b.len() == F32_LEN + 1) {
        return Err(bad());
    }
    let t = sub(b, 0, 8);
    let (h, m, s, d) = (
        num(sub(t, 0, 2)).ok_or_else(bad)?,
        num(sub(t, 2, 4)).ok_or_else(bad)?,
        num(sub(t, 4, 6)).ok_or_else(bad)?,
        num(sub(t, 6, 8)).ok_or_else(bad)?,
    );
    if h > 23 || m > 59 || s > 59 {
        return Err(bad());
    }
    let latitude = angle(sub(b, 8, 15), 2, *b"NS", 90).ok_or_else(bad)?;
    let longitude = angle(sub(b, 15, 23), 3, *b"EW", 180).ok_or_else(bad)?;
    let speed = match sub(b, 23, 28) {
        b"N9999" if profile == Profile::EramAttachment2 => GroundSpeed::Unknown,
        // `N9999` means unknown only where Attachment 2 2.15 says so.
        [b'N', d @ ..] => match num(d).ok_or_else(bad)? {
            9999 => return Err(bad()),
            v => GroundSpeed::Knots(v as u16),
        },
        _ => return Err(bad()),
    };
    let heading = {
        let h = sub(b, 28, 33);
        match num(h).ok_or_else(bad)? {
            99999 if profile == Profile::EramAttachment2 => Heading::Unknown,
            v if v <= 35999 => Heading::Hundredths(v as u16),
            _ => return Err(bad()),
        }
    };
    // Rev E allows the metric designators here; Rev G 2.13 does not (Attachment 2 2.15).
    let altitude = parse_level(tail(b, 33), profile == Profile::RevE).ok_or_else(bad)?;
    Ok(Field32 {
        time: Time32 {
            hour: h as u8,
            minute: m as u8,
            second: s as u8,
            hundredths: d as u8,
        },
        latitude,
        longitude,
        speed,
        heading,
        altitude,
    })
}

pub fn fmt_f32(f: &Field32, out: &mut String) {
    let _ = write!(
        out,
        "{:02}{:02}{:02}{:02}",
        f.time.hour, f.time.minute, f.time.second, f.time.hundredths
    );
    let _ = write!(
        out,
        "{:02}{:02}{:02}{}",
        f.latitude.degrees, f.latitude.minutes, f.latitude.seconds, f.latitude.hemisphere as char
    );
    let _ = write!(
        out,
        "{:03}{:02}{:02}{}",
        f.longitude.degrees,
        f.longitude.minutes,
        f.longitude.seconds,
        f.longitude.hemisphere as char
    );
    match f.speed {
        GroundSpeed::Knots(v) => {
            let _ = write!(out, "N{v:04}");
        }
        GroundSpeed::Unknown => out.push_str("N9999"),
    }
    match f.heading {
        Heading::Hundredths(v) => {
            let _ = write!(out, "{v:05}");
        }
        Heading::Unknown => out.push_str("99999"),
    }
    fmt_level(&f.altitude, out);
}

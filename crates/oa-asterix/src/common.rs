//! Helpers and value types shared by the three categories: bounds-checked reads, packing with
//! range checks, the six-bit character code, and the fixed-point conversions of positions and
//! times. Core (pure).

use crate::error::ErrorKind;

/// A system identification: SAC and SIC, the two octets of the data source identifier
/// (Part 1 6.2: I021/010, I048/010, I062/010).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct SourceId {
    pub sac: u8,
    pub sic: u8,
}

/// A position as raw counts of the item's LSB (latitude north positive, longitude east
/// positive, both two's complement). The LSB depends on the item; see [`crate::units`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct RawPosition {
    pub lat: i32,
    pub lon: i32,
}

/// A Mode 3/A code with its three flags (I048/070 V, G, L; I062/060 V, G, CH). `code` is the
/// 12-bit reply, four octal digits packed three bits each, so `0o7700` is squawk 7700.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Mode3a {
    /// V: code not validated.
    pub not_validated: bool,
    /// G: garbled code.
    pub garbled: bool,
    /// L (I048/070: not extracted during the last scan) or CH (I062/060: code changed).
    pub l_or_change: bool,
    pub code: u16,
}

/// A target identification: eight characters, each `A` to `Z`, `0` to `9` or a space.
pub type Ident = [u8; 8];

/// An item the typed layer does not interpret: the FRN and the exact bytes after the FSPEC.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct RawItem {
    pub frn: u8,
    pub bytes: Vec<u8>,
}

/// Big-endian unsigned read of `n` octets (n at most 4) at `*pos`, advancing it.
pub(crate) fn rd(buf: &[u8], pos: &mut usize, n: usize) -> Result<u32, ErrorKind> {
    let end = pos.checked_add(n).ok_or(ErrorKind::Truncated)?;
    let s = buf.get(*pos..end).ok_or(ErrorKind::Truncated)?;
    *pos = end;
    Ok(s.iter().fold(0u32, |a, &b| (a << 8) | u32::from(b)))
}

/// `n` octets at `*pos`, advancing it.
pub(crate) fn take<'a>(buf: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], ErrorKind> {
    let end = pos.checked_add(n).ok_or(ErrorKind::Truncated)?;
    let s = buf.get(*pos..end).ok_or(ErrorKind::Truncated)?;
    *pos = end;
    Ok(s)
}

/// The item must be used up.
pub(crate) fn done(buf: &[u8], pos: usize) -> Result<(), ErrorKind> {
    if pos == buf.len() {
        Ok(())
    } else {
        Err(ErrorKind::InvalidValue)
    }
}

/// Sign-extends the low `bits` bits of `raw`.
pub(crate) fn sext(raw: u32, bits: u32) -> i32 {
    let shift = 32 - bits;
    ((raw << shift) as i32) >> shift
}

/// Two's complement of `v` in `bits` bits, refusing a value that does not fit.
pub(crate) fn pack_s(v: i32, bits: u32) -> Result<u32, ErrorKind> {
    let lim = 1i64 << (bits - 1);
    if i64::from(v) < -lim || i64::from(v) >= lim {
        return Err(ErrorKind::ValueOutOfRange);
    }
    Ok((v as u32) & mask(bits))
}

/// `v` in `bits` bits unsigned, refusing a value that does not fit.
pub(crate) fn pack_u(v: u32, bits: u32) -> Result<u32, ErrorKind> {
    if bits < 32 && v >> bits != 0 {
        return Err(ErrorKind::ValueOutOfRange);
    }
    Ok(v)
}

/// A flag bit at `shift`.
pub(crate) fn flag(b: bool, shift: u32) -> u32 {
    u32::from(b) << shift
}

fn mask(bits: u32) -> u32 {
    if bits >= 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    }
}

/// Appends the low `n` octets of `v`, big-endian.
pub(crate) fn put(out: &mut Vec<u8>, v: u32, n: usize) {
    let b = v.to_be_bytes();
    out.extend_from_slice(b.get(4 - n..).unwrap_or(&[]));
}

/// A latitude in raw counts must be within +-90 degrees: `quarter` is the count of 90 degrees,
/// `2^(bits - 1)` of an LSB of 180/2^bits degrees. Part 1 5.2.5.2.1: values outside a stated
/// range make the record invalid.
pub(crate) fn check_lat(lat: i32, bits: u32) -> Result<(), ErrorKind> {
    if i64::from(lat).abs() > 1i64 << (bits - 1) {
        return Err(ErrorKind::InvalidValue);
    }
    Ok(())
}

/// A longitude in raw counts must be within +-180 degrees: `2^bits` counts of `180 / 2^bits`
/// degrees. Same rule as [`check_lat`].
pub(crate) fn check_lon(lon: i32, bits: u32) -> Result<(), ErrorKind> {
    if i64::from(lon).abs() > 1i64 << bits {
        return Err(ErrorKind::InvalidValue);
    }
    Ok(())
}

/// Six-bit character to ASCII (ICAO Annex 10 Vol IV 3.1.2.9.1.2 Table 3-8, which the category
/// specifications cite): 1 to 26 are `A` to `Z`, 32 is a space, 48 to 57 are `0` to `9`.
/// Anything else is refused.
pub(crate) fn char6_to_ascii(c: u8) -> Result<u8, ErrorKind> {
    match c {
        1..=26 => Ok(c + 0x40),
        32 | 48..=57 => Ok(c),
        _ => Err(ErrorKind::InvalidValue),
    }
}

/// ASCII to the six-bit character; the inverse of [`char6_to_ascii`].
pub(crate) fn ascii_to_char6(a: u8) -> Result<u8, ErrorKind> {
    match a {
        b'A'..=b'Z' => Ok(a - 0x40),
        b' ' | b'0'..=b'9' => Ok(a),
        _ => Err(ErrorKind::InvalidValue),
    }
}

/// Eight six-bit characters from six octets.
pub(crate) fn decode_ident(b: &[u8]) -> Result<Ident, ErrorKind> {
    let mut pos = 0;
    let hi = u64::from(rd(b, &mut pos, 3)?);
    let lo = u64::from(rd(b, &mut pos, 3)?);
    let v = (hi << 24) | lo;
    let mut out = [b' '; 8];
    for (i, o) in out.iter_mut().enumerate() {
        let c = ((v >> (42 - 6 * i)) & 0x3F) as u8;
        *o = char6_to_ascii(c)?;
    }
    Ok(out)
}

/// Like [`decode_ident`], but six octets of all zero bits give `None`. Real radars and trackers
/// send them for "no identification" (every character the unassigned code 0); the item is then
/// kept as raw bytes so that it round-trips, and the typed field stays empty.
pub(crate) fn decode_ident_opt(b: &[u8]) -> Result<Option<Ident>, ErrorKind> {
    if b.len() == 6 && b.iter().all(|&x| x == 0) {
        return Ok(None);
    }
    decode_ident(b).map(Some)
}

/// Six octets from eight characters.
pub(crate) fn encode_ident(id: &Ident, out: &mut Vec<u8>) -> Result<(), ErrorKind> {
    let mut v = 0u64;
    for &a in id {
        v = (v << 6) | u64::from(ascii_to_char6(a)?);
    }
    out.extend_from_slice(&v.to_be_bytes()[2..]);
    Ok(())
}

/// Latitude or longitude: raw counts of `180 / 2^bits` degrees to 1e-7 degree, rounded to the
/// nearest (half away from zero). `None` when the result is not a geographic angle (past 180
/// degrees, or a width the conversion does not support), so a raw count out of range never turns
/// into a plausible position; a latitude is further limited to 90 degrees by the caller.
pub fn raw_to_e7(raw: i32, bits: u32) -> Option<i32> {
    if bits > 32 {
        return None;
    }
    let num = i64::from(raw) * 1_800_000_000;
    let den = 1i64 << bits;
    let half = den / 2;
    let q = if num >= 0 {
        (num + half) / den
    } else {
        (num - half) / den
    };
    if q.abs() > 1_800_000_000 {
        return None;
    }
    i32::try_from(q).ok()
}

/// The inverse of [`raw_to_e7`]: 1e-7 degree to the nearest count of `180 / 2^bits` degrees.
/// Counts are at least 1.67e-7 degree apart for `bits` up to 30, so for those widths
/// `raw_to_e7(r)` is `Some(e)` with `e7_to_raw(e) == r`; the error of the other direction is at most half an LSB.
pub fn e7_to_raw(e7: i32, bits: u32) -> i32 {
    let num = i64::from(e7) * (1i64 << bits);
    let den = 1_800_000_000i64;
    let half = den / 2;
    let q = if num >= 0 {
        (num + half) / den
    } else {
        (num - half) / den
    };
    q as i32
}

/// A time in 1/128 s counts to whole milliseconds, rounded to nearest.
pub fn time128_to_ms(raw: u32) -> u32 {
    ((u64::from(raw) * 1000 + 64) / 128) as u32
}

/// Milliseconds to 1/128 s counts, rounded to nearest.
pub fn ms_to_time128(ms: u32) -> u32 {
    ((u64::from(ms) * 128 + 500) / 1000) as u32
}

/// A selected altitude (I021/146, I062/380 subitem 6): the source flag and code and the 13-bit
/// two's complement altitude in counts of 25 ft.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct SelectedAltitude {
    /// SAS: source information provided.
    pub source_provided: bool,
    /// Source, two bits: 0 unknown, 1 aircraft altitude, 2 MCP/FCU, 3 FMS.
    pub source: u8,
    pub altitude: i16,
}

/// Reads a [`SelectedAltitude`] from two octets.
pub(crate) fn decode_selected_altitude(b: &[u8]) -> Result<SelectedAltitude, ErrorKind> {
    let mut pos = 0;
    let v = rd(b, &mut pos, 2)?;
    done(b, pos)?;
    Ok(SelectedAltitude {
        source_provided: v >> 15 & 1 == 1,
        source: (v >> 13 & 3) as u8,
        altitude: sext(v & 0x1FFF, 13) as i16,
    })
}

/// Writes a [`SelectedAltitude`] as two octets.
pub(crate) fn encode_selected_altitude(
    s: &SelectedAltitude,
    out: &mut Vec<u8>,
) -> Result<(), ErrorKind> {
    let v = flag(s.source_provided, 15)
        | pack_u(u32::from(s.source), 2)? << 13
        | pack_s(i32::from(s.altitude), 13)?;
    put(out, v, 2);
    Ok(())
}

/// Packs unsigned `v` into `bits` bits and appends it as `octets` octets.
pub(crate) fn emit_u(out: &mut Vec<u8>, v: u32, bits: u32, octets: usize) -> Result<(), ErrorKind> {
    put(out, pack_u(v, bits)?, octets);
    Ok(())
}

/// Packs two's complement `v` into `bits` bits and appends it as `octets` octets.
pub(crate) fn emit_s(out: &mut Vec<u8>, v: i32, bits: u32, octets: usize) -> Result<(), ErrorKind> {
    put(out, pack_s(v, bits)?, octets);
    Ok(())
}

/// Appends one octet.
pub(crate) fn emit_b(out: &mut Vec<u8>, v: u8) -> Result<(), ErrorKind> {
    out.push(v);
    Ok(())
}

/// The encoded items of a record: `(FRN, item bytes)`.
pub(crate) type Items = Vec<(u8, Vec<u8>)>;
/// An encode failure with the FRN of the item it belongs to (0 if none).
pub(crate) type ItemError = (u8, ErrorKind);

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-AST-090
    #[test]
    fn pack_u_accepts_exactly_the_values_that_fit() {
        assert_eq!(pack_u(0, 4), Ok(0));
        assert_eq!(pack_u(15, 4), Ok(15));
        assert_eq!(pack_u(16, 4), Err(ErrorKind::ValueOutOfRange));
        assert_eq!(pack_u(31, 4), Err(ErrorKind::ValueOutOfRange));
        assert_eq!(pack_u(1, 1), Ok(1));
        assert_eq!(pack_u(2, 1), Err(ErrorKind::ValueOutOfRange));
        assert_eq!(pack_u(0, 0), Ok(0));
        assert_eq!(pack_u(1, 0), Err(ErrorKind::ValueOutOfRange));
        assert_eq!(pack_u(0x00FF_FFFF, 24), Ok(0x00FF_FFFF));
        assert_eq!(pack_u(0x0100_0000, 24), Err(ErrorKind::ValueOutOfRange));
        assert_eq!(pack_u(0x7FFF_FFFF, 31), Ok(0x7FFF_FFFF));
        assert_eq!(pack_u(0x8000_0000, 31), Err(ErrorKind::ValueOutOfRange));
        // A 32-bit element holds every value.
        assert_eq!(pack_u(u32::MAX, 32), Ok(u32::MAX));
    }
}

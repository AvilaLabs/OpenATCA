//! ASTERIX category 062, system track data, Ed 1.21 (Part 9). Typed decode and encode of the
//! items the tracker needs; every other item is kept as raw bytes. Core (pure).
//!
//! The fields hold raw counts of the LSBs in [`crate::units`] (A9). Aircraft Derived Data
//! (I062/380) is decoded as a compound: the target address, target identification and selected
//! altitude subitems are typed, every other subitem is kept as raw bytes.

use crate::common::*;
use crate::error::ErrorKind;
use crate::uap::{compound_subs, UAP_062};
use crate::wire::{compound_join, compound_parts};

/// I062/100 calculated track position, Cartesian (5.2.7): X and Y in 0.5 m, 24-bit two's
/// complement each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct CartPosition {
    pub x: i32,
    pub y: i32,
}

/// I062/185 calculated track velocity, Cartesian (5.2.14): Vx and Vy in 0.25 m/s (y to north).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Velocity {
    pub vx: i16,
    pub vy: i16,
}

/// I062/080 track status (5.2.6): the first part, and the extent octets as on the wire (SIM,
/// TSE, TSB, FPC, AFF, STP, KOS; then MD4, MD5, ...; the sixth extent has spare bits).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct TrackStatus062 {
    /// MON: monosensor track (0 multisensor).
    pub monosensor: bool,
    /// SPI present.
    pub spi: bool,
    /// MRH: geometric altitude more reliable (0 barometric).
    pub geometric_more_reliable: bool,
    /// SRC, three bits, source of the calculated track altitude: 0 none, 1 GNSS, 2 3D radar,
    /// 3 triangulation, 4 height from coverage, 5 speed look-up table, 6 default height,
    /// 7 multilateration.
    pub altitude_source: u8,
    /// CNF: tentative track.
    pub tentative: bool,
    pub ext: Vec<u8>,
}

/// I062/135 calculated track barometric altitude (5.2.12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct BaroAltitude {
    /// QNH correction applied.
    pub qnh: bool,
    /// 15-bit two's complement, 1/4 FL.
    pub raw: i16,
}

/// I062/290 system track update ages (5.2.20), 1/4 s each, absent where the subitem is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct UpdateAges {
    pub track: Option<u8>,
    pub psr: Option<u8>,
    pub ssr: Option<u8>,
    pub mode_s: Option<u8>,
    /// ADS-C age, two octets.
    pub ads_c: Option<u16>,
    pub ads_b_es: Option<u8>,
    pub ads_b_vdl: Option<u8>,
    pub ads_b_uat: Option<u8>,
    pub loop_age: Option<u8>,
    pub mlat: Option<u8>,
}

/// A subitem of a compound item that is kept raw: its one-based number and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct RawSub {
    pub number: u8,
    pub bytes: Vec<u8>,
}

/// I062/380 Aircraft Derived Data (5.2.24).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AircraftDerived {
    /// Subitem 1, target address, 24 bits.
    pub address: Option<u32>,
    /// Subitem 2, target identification.
    pub ident: Option<Ident>,
    /// Subitem 6, selected altitude.
    pub selected_altitude: Option<SelectedAltitude>,
    /// The other subitems (3 to 5, 7 to 28), sorted by number.
    pub other: Vec<RawSub>,
}

/// One CAT062 data record. A7.2 and A3: a short-lived decode result, not a hot collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Cat062 {
    /// I062/010 (mandatory).
    pub source: SourceId,
    /// I062/070 time of track information, 1/128 s since midnight (24 bits).
    pub time: Option<u32>,
    /// I062/105, 180/2^25 degrees.
    pub position: Option<RawPosition>,
    /// I062/100.
    pub cartesian: Option<CartPosition>,
    /// I062/185.
    pub velocity: Option<Velocity>,
    /// I062/060.
    pub mode3a: Option<Mode3a>,
    /// I062/380.
    pub derived: Option<AircraftDerived>,
    /// I062/040 track number.
    pub track_number: Option<u16>,
    /// I062/080.
    pub track_status: Option<TrackStatus062>,
    /// I062/290.
    pub update_ages: Option<UpdateAges>,
    /// I062/136 measured flight level, 1/4 FL.
    pub measured_fl: Option<i16>,
    /// I062/130 calculated track geometric altitude, 6.25 ft.
    pub geometric_altitude: Option<i16>,
    /// I062/135.
    pub baro_altitude: Option<BaroAltitude>,
    /// I062/220 calculated rate of climb/descent, 6.25 ft/min.
    pub climb_rate: Option<i16>,
    /// Items not decoded above (015, 210, 245, 200, 295, 390, 270, 300, 110, 120, 510, 500, 340,
    /// RE, SP).
    pub raw: Vec<RawItem>,
}

fn tail(b: &[u8], from: usize) -> Vec<u8> {
    b.get(from..).unwrap_or(&[]).to_vec()
}

fn first(b: &[u8]) -> Result<u8, ErrorKind> {
    b.first().copied().ok_or(ErrorKind::Truncated)
}

fn fixed(b: &[u8], n: usize) -> Result<u32, ErrorKind> {
    let mut p = 0;
    let v = rd(b, &mut p, n)?;
    done(b, p)?;
    Ok(v)
}

fn decode_derived(b: &[u8]) -> Result<AircraftDerived, ErrorKind> {
    let mut d = AircraftDerived::default();
    for (i, s, e) in compound_parts(b, compound_subs(&UAP_062, 11))? {
        let sub = b.get(s..e).ok_or(ErrorKind::Truncated)?;
        match i {
            0 => d.address = Some(fixed(sub, 3)?),
            1 => d.ident = Some(decode_ident(sub)?),
            5 => d.selected_altitude = Some(decode_selected_altitude(sub)?),
            _ => d.other.push(RawSub {
                number: u8::try_from(i + 1).map_err(|_| ErrorKind::UnknownSubitem)?,
                bytes: sub.to_vec(),
            }),
        }
    }
    Ok(d)
}

fn decode_ages(b: &[u8]) -> Result<UpdateAges, ErrorKind> {
    let mut a = UpdateAges::default();
    for (i, s, e) in compound_parts(b, compound_subs(&UAP_062, 14))? {
        let sub = b.get(s..e).ok_or(ErrorKind::Truncated)?;
        let v = fixed(sub, sub.len())?;
        match i {
            0 => a.track = Some(v as u8),
            1 => a.psr = Some(v as u8),
            2 => a.ssr = Some(v as u8),
            3 => a.mode_s = Some(v as u8),
            4 => a.ads_c = Some(v as u16),
            5 => a.ads_b_es = Some(v as u8),
            6 => a.ads_b_vdl = Some(v as u8),
            7 => a.ads_b_uat = Some(v as u8),
            8 => a.loop_age = Some(v as u8),
            _ => a.mlat = Some(v as u8),
        }
    }
    Ok(a)
}

/// Decodes the item `bytes` of FRN `frn` into `rec`; `Ok(false)` if the FRN is not typed here.
pub(crate) fn decode_item(rec: &mut Cat062, frn: u8, b: &[u8]) -> Result<bool, ErrorKind> {
    match frn {
        1 => {
            rec.source = SourceId {
                sac: first(b)?,
                sic: *b.get(1).ok_or(ErrorKind::Truncated)?,
            }
        }
        4 => rec.time = Some(fixed(b, 3)?),
        5 => {
            let mut p = 0;
            let lat = sext(rd(b, &mut p, 4)?, 32);
            let lon = sext(rd(b, &mut p, 4)?, 32);
            done(b, p)?;
            check_lat(lat, 25)?;
            rec.position = Some(RawPosition { lat, lon });
        }
        6 => {
            let mut p = 0;
            let x = sext(rd(b, &mut p, 3)?, 24);
            let y = sext(rd(b, &mut p, 3)?, 24);
            done(b, p)?;
            rec.cartesian = Some(CartPosition { x, y });
        }
        7 => {
            let v = fixed(b, 4)?;
            rec.velocity = Some(Velocity {
                vx: sext(v >> 16, 16) as i16,
                vy: sext(v & 0xFFFF, 16) as i16,
            });
        }
        9 => {
            let v = fixed(b, 2)?;
            rec.mode3a = Some(Mode3a {
                not_validated: v >> 15 & 1 == 1,
                garbled: v >> 14 & 1 == 1,
                l_or_change: v >> 13 & 1 == 1,
                code: (v & 0xFFF) as u16,
            });
        }
        11 => rec.derived = Some(decode_derived(b)?),
        12 => rec.track_number = Some(fixed(b, 2)? as u16),
        13 => {
            let p = first(b)?;
            rec.track_status = Some(TrackStatus062 {
                monosensor: p >> 7 & 1 == 1,
                spi: p >> 6 & 1 == 1,
                geometric_more_reliable: p >> 5 & 1 == 1,
                altitude_source: p >> 2 & 7,
                tentative: p >> 1 & 1 == 1,
                ext: tail(b, 1),
            });
        }
        14 => rec.update_ages = Some(decode_ages(b)?),
        17 => rec.measured_fl = Some(sext(fixed(b, 2)?, 16) as i16),
        18 => rec.geometric_altitude = Some(sext(fixed(b, 2)?, 16) as i16),
        19 => {
            let v = fixed(b, 2)?;
            rec.baro_altitude = Some(BaroAltitude {
                qnh: v >> 15 == 1,
                raw: sext(v & 0x7FFF, 15) as i16,
            });
        }
        20 => rec.climb_rate = Some(sext(fixed(b, 2)?, 16) as i16),
        _ => return Ok(false),
    }
    Ok(true)
}

fn one(
    out: &mut Items,
    frn: u8,
    f: impl FnOnce(&mut Vec<u8>) -> Result<(), ErrorKind>,
) -> Result<(), ItemError> {
    let mut v = Vec::new();
    f(&mut v).map_err(|k| (frn, k))?;
    out.push((frn, v));
    Ok(())
}

fn encode_derived(d: &AircraftDerived) -> Result<Vec<u8>, ErrorKind> {
    let mut parts: Vec<(usize, Vec<u8>)> = Vec::new();
    if let Some(a) = d.address {
        let mut v = Vec::new();
        emit_u(&mut v, a, 24, 3)?;
        parts.push((0, v));
    }
    if let Some(id) = &d.ident {
        let mut v = Vec::new();
        encode_ident(id, &mut v)?;
        parts.push((1, v));
    }
    if let Some(s) = &d.selected_altitude {
        let mut v = Vec::new();
        encode_selected_altitude(s, &mut v)?;
        parts.push((5, v));
    }
    for r in &d.other {
        let i = usize::from(r.number)
            .checked_sub(1)
            .ok_or(ErrorKind::UnknownSubitem)?;
        if matches!(i, 0 | 1 | 5) {
            return Err(ErrorKind::DuplicateItem);
        }
        parts.push((i, r.bytes.clone()));
    }
    parts.sort_by_key(|p| p.0);
    if parts
        .windows(2)
        .any(|w| w.first().map(|a| a.0) == w.get(1).map(|b| b.0))
    {
        return Err(ErrorKind::DuplicateItem);
    }
    if parts.is_empty() {
        return Err(ErrorKind::EmptyCompound);
    }
    Ok(compound_join(&parts))
}

fn encode_ages(a: &UpdateAges) -> Result<Vec<u8>, ErrorKind> {
    let one8 = |x: Option<u8>| x.map(|v| vec![v]);
    let slots = [
        one8(a.track),
        one8(a.psr),
        one8(a.ssr),
        one8(a.mode_s),
        a.ads_c.map(|v| v.to_be_bytes().to_vec()),
        one8(a.ads_b_es),
        one8(a.ads_b_vdl),
        one8(a.ads_b_uat),
        one8(a.loop_age),
        one8(a.mlat),
    ];
    let parts: Vec<(usize, Vec<u8>)> = slots
        .into_iter()
        .enumerate()
        .filter_map(|(i, x)| x.map(|b| (i, b)))
        .collect();
    if parts.is_empty() {
        return Err(ErrorKind::EmptyCompound);
    }
    Ok(compound_join(&parts))
}

/// The typed items of `rec` as `(FRN, bytes)` in FRN order.
pub(crate) fn encode_items(rec: &Cat062) -> Result<Items, ItemError> {
    let mut o = Vec::new();
    one(&mut o, 1, |v| {
        v.push(rec.source.sac);
        v.push(rec.source.sic);
        Ok(())
    })?;
    if let Some(t) = rec.time {
        one(&mut o, 4, |v| emit_u(v, t, 24, 3))?;
    }
    if let Some(p) = &rec.position {
        one(&mut o, 5, |v| {
            check_lat(p.lat, 25)?;
            emit_s(v, p.lat, 32, 4)?;
            emit_s(v, p.lon, 32, 4)
        })?;
    }
    if let Some(c) = &rec.cartesian {
        one(&mut o, 6, |v| {
            emit_s(v, c.x, 24, 3)?;
            emit_s(v, c.y, 24, 3)
        })?;
    }
    if let Some(c) = &rec.velocity {
        one(&mut o, 7, |v| {
            emit_s(v, i32::from(c.vx), 16, 2)?;
            emit_s(v, i32::from(c.vy), 16, 2)
        })?;
    }
    if let Some(m) = &rec.mode3a {
        one(&mut o, 9, |v| {
            let p = flag(m.not_validated, 15)
                | flag(m.garbled, 14)
                | flag(m.l_or_change, 13)
                | pack_u(u32::from(m.code), 12)?;
            put(v, p, 2);
            Ok(())
        })?;
    }
    if let Some(d) = &rec.derived {
        o.push((11, encode_derived(d).map_err(|k| (11, k))?));
    }
    if let Some(n) = rec.track_number {
        one(&mut o, 12, |v| emit_u(v, u32::from(n), 16, 2))?;
    }
    if let Some(t) = &rec.track_status {
        one(&mut o, 13, |v| {
            let p = flag(t.monosensor, 7)
                | flag(t.spi, 6)
                | flag(t.geometric_more_reliable, 5)
                | pack_u(u32::from(t.altitude_source), 3)? << 2
                | flag(t.tentative, 1)
                | u32::from(!t.ext.is_empty());
            put(v, p, 1);
            v.extend_from_slice(&t.ext);
            Ok(())
        })?;
    }
    if let Some(a) = &rec.update_ages {
        o.push((14, encode_ages(a).map_err(|k| (14, k))?));
    }
    if let Some(f) = rec.measured_fl {
        one(&mut o, 17, |v| emit_s(v, i32::from(f), 16, 2))?;
    }
    if let Some(a) = rec.geometric_altitude {
        one(&mut o, 18, |v| emit_s(v, i32::from(a), 16, 2))?;
    }
    if let Some(b) = &rec.baro_altitude {
        one(&mut o, 19, |v| {
            put(v, flag(b.qnh, 15) | pack_s(i32::from(b.raw), 15)?, 2);
            Ok(())
        })?;
    }
    if let Some(r) = rec.climb_rate {
        one(&mut o, 20, |v| emit_s(v, i32::from(r), 16, 2))?;
    }
    Ok(o)
}

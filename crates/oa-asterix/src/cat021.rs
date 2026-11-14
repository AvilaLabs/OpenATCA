//! ASTERIX category 021, ADS-B target reports, Ed 2.7 (Part 12). Typed decode and encode of the
//! items the tracker needs; every other item is kept as raw bytes. Core (pure).
//!
//! The fields hold raw counts of the LSBs in [`crate::units`] (A9), so `encode(decode(x))` is
//! exact. Enumerations are kept as their codes; reserved codes are not refused (Part 1 5.2.5.2.3
//! asks encoders not to set them, and a decoder of a later edition must still read them).

use crate::common::*;
use crate::error::ErrorKind;

/// I021/040 Target Report Descriptor (5.2.6): the primary subfield, and the extension octets as
/// they are on the wire (first extension DCR, GBS, SIM, TST, SAA, CL; second the error
/// conditions; third and fourth the bit-correction counts), each with its FX bit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Trd021 {
    /// ATP, address type: 0 ICAO, 1 duplicate, 2 surface vehicle, 3 anonymous.
    pub atp: u8,
    /// ARC, altitude reporting capability: 0 25 ft, 1 100 ft, 2 unknown, 3 invalid.
    pub arc: u8,
    /// RC: range check passed, CPR validation pending.
    pub range_check: bool,
    /// RAB: report from a field monitor.
    pub field_monitor: bool,
    /// Extension octets, empty when the primary subfield has no FX.
    pub ext: Vec<u8>,
}

/// I021/090 Quality Indicators (5.2.16): the primary subfield, and the extension octets as they
/// are on the wire (first: NICBARO, SIL, NACp; second: SILS, SDA, GVA; third: PIC, SRC; the
/// rest validation data). Read them with [`quality_nacp`] and the other accessors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Quality021 {
    /// NUCr (MOPS version 0) or NACv, three bits.
    pub nacv: u8,
    /// NUCp (version 0) or NIC, four bits.
    pub nic: u8,
    pub ext: Vec<u8>,
}

/// I021/210 MOPS Version (5.2.35).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Mops {
    /// VNS: the version is not supported by the ground station.
    pub not_supported: bool,
    /// VN: 0 ED-102/DO-260, 1 DO-260A, 2 ED-102A/DO-260B, 3 ED-102B/DO-260C.
    pub version: u8,
    /// LTT, link technology: 0 other, 1 UAT, 2 1090 ES, 3 VDL 4.
    pub link: u8,
}

/// A 15-bit unsigned value with the flag bit above it: I021/150 (IM: 0 IAS in 2^-14 NM/s,
/// 1 Mach in 0.001) and I021/151 (RE: range exceeded; 1 kt).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct FlagU15 {
    pub flag: bool,
    pub raw: u16,
}

/// A 15-bit two's complement value with the range-exceeded bit above it: I021/155, 157 (6.25
/// ft/min).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct FlagS15 {
    pub range_exceeded: bool,
    pub raw: i16,
}

/// I021/160 Airborne Ground Vector (5.2.30).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct GroundVector {
    /// RE: range exceeded.
    pub range_exceeded: bool,
    /// Ground speed, 15 bits, 2^-14 NM/s.
    pub speed: u16,
    /// Track angle clockwise from true north, 360/2^16 degrees.
    pub track: u16,
}

/// I021/148 Final State Selected Altitude (5.2.24).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct FinalState {
    pub manage_vertical: bool,
    pub altitude_hold: bool,
    pub approach: bool,
    /// 13-bit two's complement, 25 ft.
    pub altitude: i16,
}

/// I021/074 and 076: the full second indication (0 same, 1 plus one, 2 minus one, 3 reserved)
/// and the 30-bit fraction of the second (2^-30 s).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct HighPrecisionTime {
    pub fsi: u8,
    pub fraction: u32,
}

/// One CAT021 data record. The typed items are the ones the tracker uses; the rest are in
/// `raw`, sorted by FRN. A7.2 and A3: a record is a short-lived decode result that the tracker
/// adapter moves into its own columns, not a hot collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Cat021 {
    /// I021/010 (mandatory).
    pub source: SourceId,
    /// I021/040.
    pub descriptor: Option<Trd021>,
    /// I021/161, 12 bits.
    pub track_number: Option<u16>,
    /// I021/071 time of applicability for position, 1/128 s since midnight (24 bits).
    pub toa_position: Option<u32>,
    /// I021/130, 180/2^23 degrees.
    pub position: Option<RawPosition>,
    /// I021/131, 180/2^30 degrees.
    pub position_hr: Option<RawPosition>,
    /// I021/072 time of applicability for velocity.
    pub toa_velocity: Option<u32>,
    /// I021/150.
    pub air_speed: Option<FlagU15>,
    /// I021/151.
    pub true_airspeed: Option<FlagU15>,
    /// I021/080 target address, 24 bits.
    pub address: Option<u32>,
    /// I021/073 time of message reception for position.
    pub tomr_position: Option<u32>,
    /// I021/074.
    pub tomr_position_hp: Option<HighPrecisionTime>,
    /// I021/075 time of message reception for velocity.
    pub tomr_velocity: Option<u32>,
    /// I021/076.
    pub tomr_velocity_hp: Option<HighPrecisionTime>,
    /// I021/140 geometric height, 6.25 ft.
    pub geometric_height: Option<i16>,
    /// I021/090.
    pub quality: Option<Quality021>,
    /// I021/210.
    pub mops: Option<Mops>,
    /// I021/070 Mode 3/A code, 12 bits (see [`Mode3a::code`]).
    pub mode3a: Option<u16>,
    /// I021/230 roll angle, 0.01 degree.
    pub roll_angle: Option<i16>,
    /// I021/145 flight level (barometric altitude), 1/4 FL.
    pub flight_level: Option<i16>,
    /// I021/152 magnetic heading, 360/2^16 degrees.
    pub magnetic_heading: Option<u16>,
    /// I021/200 target status, the octet as on the wire.
    pub target_status: Option<u8>,
    /// I021/155 barometric vertical rate.
    pub baro_rate: Option<FlagS15>,
    /// I021/157 geometric vertical rate.
    pub geometric_rate: Option<FlagS15>,
    /// I021/160.
    pub ground_vector: Option<GroundVector>,
    /// I021/165 track angle rate, 10-bit two's complement, 1/32 degree per second.
    pub track_angle_rate: Option<i16>,
    /// I021/077 time of report transmission.
    pub time_transmission: Option<u32>,
    /// I021/170 target identification.
    pub target_id: Option<Ident>,
    /// I021/020 emitter category.
    pub emitter_category: Option<u8>,
    /// I021/146.
    pub selected_altitude: Option<SelectedAltitude>,
    /// I021/148.
    pub final_state: Option<FinalState>,
    /// Items not decoded above (015, 220, 110, 016, 008, 271, 132, 250, 260, 400, 295, RE, SP).
    pub raw: Vec<RawItem>,
}

/// NACp of a quality item, if the first extension is present.
pub fn quality_nacp(q: &Quality021) -> Option<u8> {
    q.ext.first().map(|b| b >> 1 & 0xF)
}
/// SIL of a quality item (first extension), two bits.
pub fn quality_sil(q: &Quality021) -> Option<u8> {
    q.ext.first().map(|b| b >> 5 & 3)
}
/// NICBARO of a quality item (first extension).
pub fn quality_nicbaro(q: &Quality021) -> Option<u8> {
    q.ext.first().map(|b| b >> 7 & 1)
}
/// SDA of a quality item (second extension), two bits.
pub fn quality_sda(q: &Quality021) -> Option<u8> {
    q.ext.get(1).map(|b| b >> 3 & 3)
}
/// GVA of a quality item (second extension), two bits.
pub fn quality_gva(q: &Quality021) -> Option<u8> {
    q.ext.get(1).map(|b| b >> 1 & 3)
}
/// SILS of a quality item (second extension): 1 means per sample.
pub fn quality_sils(q: &Quality021) -> Option<u8> {
    q.ext.get(1).map(|b| b >> 5 & 1)
}
/// PIC of a quality item (third extension), four bits.
pub fn quality_pic(q: &Quality021) -> Option<u8> {
    q.ext.get(2).map(|b| b >> 4 & 0xF)
}
/// GBS, the ground bit, of a descriptor (first extension).
pub fn descriptor_ground(t: &Trd021) -> Option<bool> {
    t.ext.first().map(|b| b >> 6 & 1 == 1)
}
/// SIM, simulated target, of a descriptor (first extension).
pub fn descriptor_simulated(t: &Trd021) -> Option<bool> {
    t.ext.first().map(|b| b >> 5 & 1 == 1)
}
/// TST, test target, of a descriptor (first extension).
pub fn descriptor_test(t: &Trd021) -> Option<bool> {
    t.ext.first().map(|b| b >> 4 & 1 == 1)
}
/// CL, confidence level (0 valid, 1 suspect, 2 no information), of a descriptor.
pub fn descriptor_confidence(t: &Trd021) -> Option<u8> {
    t.ext.first().map(|b| b >> 1 & 3)
}

fn tail(b: &[u8], from: usize) -> Vec<u8> {
    b.get(from..).unwrap_or(&[]).to_vec()
}

fn u24(b: &[u8]) -> Result<u32, ErrorKind> {
    let mut p = 0;
    let v = rd(b, &mut p, 3)?;
    done(b, p)?;
    Ok(v)
}

fn one_octet(b: &[u8]) -> Result<u8, ErrorKind> {
    let mut p = 0;
    let v = rd(b, &mut p, 1)?;
    done(b, p)?;
    Ok(v as u8)
}

fn u16_of(b: &[u8]) -> Result<u32, ErrorKind> {
    let mut p = 0;
    let v = rd(b, &mut p, 2)?;
    done(b, p)?;
    Ok(v)
}

fn hp(b: &[u8]) -> Result<HighPrecisionTime, ErrorKind> {
    let mut p = 0;
    let v = rd(b, &mut p, 4)?;
    done(b, p)?;
    Ok(HighPrecisionTime {
        fsi: (v >> 30) as u8,
        fraction: v & 0x3FFF_FFFF,
    })
}

fn flag_u15(b: &[u8]) -> Result<FlagU15, ErrorKind> {
    let v = u16_of(b)?;
    Ok(FlagU15 {
        flag: v >> 15 == 1,
        raw: (v & 0x7FFF) as u16,
    })
}

fn flag_s15(b: &[u8]) -> Result<FlagS15, ErrorKind> {
    let v = u16_of(b)?;
    Ok(FlagS15 {
        range_exceeded: v >> 15 == 1,
        raw: sext(v & 0x7FFF, 15) as i16,
    })
}

fn position(b: &[u8], bits: usize, lat_bits: u32) -> Result<RawPosition, ErrorKind> {
    let mut p = 0;
    let lat = rd(b, &mut p, bits / 8)?;
    let lon = rd(b, &mut p, bits / 8)?;
    done(b, p)?;
    let lat = sext(lat, bits as u32);
    check_lat(lat, lat_bits)?;
    Ok(RawPosition {
        lat,
        lon: sext(lon, bits as u32),
    })
}

fn put_position(
    pos: &RawPosition,
    bits: usize,
    lat_bits: u32,
    out: &mut Vec<u8>,
) -> Result<(), ErrorKind> {
    check_lat(pos.lat, lat_bits)?;
    put(out, pack_s(pos.lat, bits as u32)?, bits / 8);
    put(out, pack_s(pos.lon, bits as u32)?, bits / 8);
    Ok(())
}

/// Decodes the item `bytes` of FRN `frn` into `rec`; `Ok(false)` if the FRN is not typed here.
pub(crate) fn decode_item(rec: &mut Cat021, frn: u8, b: &[u8]) -> Result<bool, ErrorKind> {
    match frn {
        1 => {
            rec.source = SourceId {
                sac: *b.first().ok_or(ErrorKind::Truncated)?,
                sic: *b.get(1).ok_or(ErrorKind::Truncated)?,
            };
        }
        2 => {
            let p = *b.first().ok_or(ErrorKind::Truncated)?;
            rec.descriptor = Some(Trd021 {
                atp: p >> 5,
                arc: p >> 3 & 3,
                range_check: p >> 2 & 1 == 1,
                field_monitor: p >> 1 & 1 == 1,
                ext: tail(b, 1),
            });
        }
        3 => rec.track_number = Some(u16_of(b)? as u16),
        5 => rec.toa_position = Some(u24(b)?),
        6 => rec.position = Some(position(b, 24, 23)?),
        7 => rec.position_hr = Some(position(b, 32, 30)?),
        8 => rec.toa_velocity = Some(u24(b)?),
        9 => rec.air_speed = Some(flag_u15(b)?),
        10 => rec.true_airspeed = Some(flag_u15(b)?),
        11 => rec.address = Some(u24(b)?),
        12 => rec.tomr_position = Some(u24(b)?),
        13 => rec.tomr_position_hp = Some(hp(b)?),
        14 => rec.tomr_velocity = Some(u24(b)?),
        15 => rec.tomr_velocity_hp = Some(hp(b)?),
        16 => rec.geometric_height = Some(sext(u16_of(b)?, 16) as i16),
        17 => {
            let p = *b.first().ok_or(ErrorKind::Truncated)?;
            rec.quality = Some(Quality021 {
                nacv: p >> 5,
                nic: p >> 1 & 0xF,
                ext: tail(b, 1),
            });
        }
        18 => {
            let v = one_octet(b)?;
            rec.mops = Some(Mops {
                not_supported: v >> 6 & 1 == 1,
                version: v >> 3 & 7,
                link: v & 7,
            });
        }
        19 => rec.mode3a = Some((u16_of(b)? & 0xFFF) as u16),
        20 => rec.roll_angle = Some(sext(u16_of(b)?, 16) as i16),
        21 => rec.flight_level = Some(sext(u16_of(b)?, 16) as i16),
        22 => rec.magnetic_heading = Some(u16_of(b)? as u16),
        23 => rec.target_status = Some(one_octet(b)?),
        24 => rec.baro_rate = Some(flag_s15(b)?),
        25 => rec.geometric_rate = Some(flag_s15(b)?),
        26 => {
            let mut p = 0;
            let a = rd(b, &mut p, 2)?;
            let t = rd(b, &mut p, 2)?;
            done(b, p)?;
            rec.ground_vector = Some(GroundVector {
                range_exceeded: a >> 15 == 1,
                speed: (a & 0x7FFF) as u16,
                track: t as u16,
            });
        }
        27 => rec.track_angle_rate = Some(sext(u16_of(b)? & 0x3FF, 10) as i16),
        28 => rec.time_transmission = Some(u24(b)?),
        29 => {
            if b.len() != 6 {
                return Err(ErrorKind::InvalidValue);
            }
            rec.target_id = Some(decode_ident(b)?);
        }
        30 => rec.emitter_category = Some(one_octet(b)?),
        32 => rec.selected_altitude = Some(decode_selected_altitude(b)?),
        33 => {
            let v = u16_of(b)?;
            rec.final_state = Some(FinalState {
                manage_vertical: v >> 15 & 1 == 1,
                altitude_hold: v >> 14 & 1 == 1,
                approach: v >> 13 & 1 == 1,
                altitude: sext(v & 0x1FFF, 13) as i16,
            });
        }
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

fn put_flag_u15(v: &FlagU15, out: &mut Vec<u8>) -> Result<(), ErrorKind> {
    put(out, flag(v.flag, 15) | pack_u(u32::from(v.raw), 15)?, 2);
    Ok(())
}

fn put_flag_s15(v: &FlagS15, out: &mut Vec<u8>) -> Result<(), ErrorKind> {
    put(
        out,
        flag(v.range_exceeded, 15) | pack_s(i32::from(v.raw), 15)?,
        2,
    );
    Ok(())
}

fn put_hp(v: &HighPrecisionTime, out: &mut Vec<u8>) -> Result<(), ErrorKind> {
    put(
        out,
        pack_u(u32::from(v.fsi), 2)? << 30 | pack_u(v.fraction, 30)?,
        4,
    );
    Ok(())
}

/// The typed items of `rec` as `(FRN, bytes)` in FRN order.
pub(crate) fn encode_items(rec: &Cat021) -> Result<Items, ItemError> {
    let mut o = Vec::new();
    one(&mut o, 1, |v| {
        v.push(rec.source.sac);
        v.push(rec.source.sic);
        Ok(())
    })?;
    if let Some(t) = &rec.descriptor {
        one(&mut o, 2, |v| {
            let p = pack_u(u32::from(t.atp), 3)? << 5
                | pack_u(u32::from(t.arc), 2)? << 3
                | flag(t.range_check, 2)
                | flag(t.field_monitor, 1)
                | u32::from(!t.ext.is_empty());
            put(v, p, 1);
            v.extend_from_slice(&t.ext);
            Ok(())
        })?;
    }
    if let Some(n) = rec.track_number {
        one(&mut o, 3, |v| {
            put(v, pack_u(u32::from(n), 12)?, 2);
            Ok(())
        })?;
    }
    if let Some(t) = rec.toa_position {
        one(&mut o, 5, |v| emit_u(v, t, 24, 3))?;
    }
    if let Some(p) = &rec.position {
        one(&mut o, 6, |v| put_position(p, 24, 23, v))?;
    }
    if let Some(p) = &rec.position_hr {
        one(&mut o, 7, |v| put_position(p, 32, 30, v))?;
    }
    if let Some(t) = rec.toa_velocity {
        one(&mut o, 8, |v| emit_u(v, t, 24, 3))?;
    }
    if let Some(a) = &rec.air_speed {
        one(&mut o, 9, |v| put_flag_u15(a, v))?;
    }
    if let Some(a) = &rec.true_airspeed {
        one(&mut o, 10, |v| put_flag_u15(a, v))?;
    }
    if let Some(a) = rec.address {
        one(&mut o, 11, |v| emit_u(v, a, 24, 3))?;
    }
    if let Some(t) = rec.tomr_position {
        one(&mut o, 12, |v| emit_u(v, t, 24, 3))?;
    }
    if let Some(t) = &rec.tomr_position_hp {
        one(&mut o, 13, |v| put_hp(t, v))?;
    }
    if let Some(t) = rec.tomr_velocity {
        one(&mut o, 14, |v| emit_u(v, t, 24, 3))?;
    }
    if let Some(t) = &rec.tomr_velocity_hp {
        one(&mut o, 15, |v| put_hp(t, v))?;
    }
    if let Some(h) = rec.geometric_height {
        one(&mut o, 16, |v| emit_s(v, i32::from(h), 16, 2))?;
    }
    if let Some(q) = &rec.quality {
        one(&mut o, 17, |v| {
            let p = pack_u(u32::from(q.nacv), 3)? << 5
                | pack_u(u32::from(q.nic), 4)? << 1
                | u32::from(!q.ext.is_empty());
            put(v, p, 1);
            v.extend_from_slice(&q.ext);
            Ok(())
        })?;
    }
    if let Some(m) = &rec.mops {
        one(&mut o, 18, |v| {
            let p = flag(m.not_supported, 6)
                | pack_u(u32::from(m.version), 3)? << 3
                | pack_u(u32::from(m.link), 3)?;
            put(v, p, 1);
            Ok(())
        })?;
    }
    if let Some(c) = rec.mode3a {
        one(&mut o, 19, |v| emit_u(v, u32::from(c), 12, 2))?;
    }
    if let Some(r) = rec.roll_angle {
        one(&mut o, 20, |v| emit_s(v, i32::from(r), 16, 2))?;
    }
    if let Some(f) = rec.flight_level {
        one(&mut o, 21, |v| emit_s(v, i32::from(f), 16, 2))?;
    }
    if let Some(h) = rec.magnetic_heading {
        one(&mut o, 22, |v| emit_u(v, u32::from(h), 16, 2))?;
    }
    if let Some(s) = rec.target_status {
        one(&mut o, 23, |v| emit_b(v, s))?;
    }
    if let Some(r) = &rec.baro_rate {
        one(&mut o, 24, |v| put_flag_s15(r, v))?;
    }
    if let Some(r) = &rec.geometric_rate {
        one(&mut o, 25, |v| put_flag_s15(r, v))?;
    }
    if let Some(g) = &rec.ground_vector {
        one(&mut o, 26, |v| {
            put(
                v,
                flag(g.range_exceeded, 15) | pack_u(u32::from(g.speed), 15)?,
                2,
            );
            put(v, u32::from(g.track), 2);
            Ok(())
        })?;
    }
    if let Some(r) = rec.track_angle_rate {
        one(&mut o, 27, |v| emit_s(v, i32::from(r), 10, 2))?;
    }
    if let Some(t) = rec.time_transmission {
        one(&mut o, 28, |v| emit_u(v, t, 24, 3))?;
    }
    if let Some(id) = &rec.target_id {
        one(&mut o, 29, |v| encode_ident(id, v))?;
    }
    if let Some(e) = rec.emitter_category {
        one(&mut o, 30, |v| emit_b(v, e))?;
    }
    if let Some(s) = &rec.selected_altitude {
        one(&mut o, 32, |v| encode_selected_altitude(s, v))?;
    }
    if let Some(f) = &rec.final_state {
        one(&mut o, 33, |v| {
            let p = flag(f.manage_vertical, 15)
                | flag(f.altitude_hold, 14)
                | flag(f.approach, 13)
                | pack_s(i32::from(f.altitude), 13)?;
            put(v, p, 2);
            Ok(())
        })?;
    }
    Ok(o)
}

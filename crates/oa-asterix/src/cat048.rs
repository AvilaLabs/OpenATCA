//! ASTERIX category 048, monoradar target reports, Ed 1.32 (Part 4). Typed decode and encode of
//! the items the tracker needs; every other item is kept as raw bytes. Core (pure).
//!
//! The fields hold raw counts of the LSBs in [`crate::units`] (A9). The Reserved Expansion Field
//! (FRN 28, Appendix A Ed 1.13) and the Special Purpose Field (FRN 27) are explicit-length items
//! kept raw.

use crate::common::*;
use crate::error::ErrorKind;
use crate::uap::{compound_subs, UAP_048};
use crate::wire::{compound_join, compound_parts, MAX_REPETITIONS};

/// I048/020 Target Report Descriptor (5.2.2): the primary subfield and the extension octets as
/// on the wire (first extension TST, ERR, XPP, ME, MI, FOE/FRI; the rest external-data and
/// transponder-capability flags), each with its FX bit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Trd048 {
    /// TYP, three bits: 0 no detection, 1 single PSR, 2 single SSR, 3 SSR + PSR, 4 Mode S
    /// all-call, 5 Mode S roll-call, 6 all-call + PSR, 7 roll-call + PSR.
    pub typ: u8,
    /// SIM: simulated target report.
    pub simulated: bool,
    /// RDP: report from RDP chain 2.
    pub rdp_chain2: bool,
    /// SPI: special position identification.
    pub spi: bool,
    /// RAB: report from a field monitor.
    pub field_monitor: bool,
    pub ext: Vec<u8>,
}

/// I048/040 measured position in polar coordinates (5.2.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Polar {
    /// Range, 1/256 NM.
    pub rho: u16,
    /// Azimuth clockwise from north, 360/2^16 degrees.
    pub theta: u16,
}

/// I048/042 calculated position in Cartesian coordinates (5.2.5), 1/128 NM, two's complement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Cartesian {
    pub x: i16,
    pub y: i16,
}

/// I048/090 flight level (5.2.12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct FlightLevel048 {
    /// V: code not validated.
    pub not_validated: bool,
    /// G: garbled code.
    pub garbled: bool,
    /// 14-bit two's complement, 1/4 FL.
    pub raw: i16,
}

/// I048/130 radar plot characteristics (5.2.16): each subitem is one octet and is present or
/// absent. The signed ones are two's complement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct PlotCharacteristics {
    /// SRL, SSR plot runlength, 360/2^13 degrees.
    pub ssr_runlength: Option<u8>,
    /// SRR, number of received SSR replies.
    pub ssr_replies: Option<u8>,
    /// SAM, SSR reply amplitude, 1 dBm.
    pub ssr_amplitude: Option<i8>,
    /// PRL, PSR plot runlength, 360/2^13 degrees.
    pub psr_runlength: Option<u8>,
    /// PAM, PSR amplitude, 1 dBm.
    pub psr_amplitude: Option<i8>,
    /// RPD, PSR minus SSR range, 1/256 NM.
    pub range_diff: Option<i8>,
    /// APD, PSR minus SSR azimuth, 360/2^14 degrees.
    pub azimuth_diff: Option<i8>,
}

/// One Mode S BDS register of I048/250: 56 data bits and the register number (BDS1 high nibble,
/// BDS2 low nibble). Passed on raw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct MbRegister {
    pub data: [u8; 7],
    pub bds: u8,
}

/// I048/170 track status (5.2.19): the first part, and the extension octets as on the wire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct TrackStatus048 {
    /// CNF: tentative track.
    pub tentative: bool,
    /// RAD, two bits: 0 combined, 1 PSR, 2 SSR/Mode S, 3 invalid.
    pub rad: u8,
    /// DOU: low confidence in plot to track association.
    pub doubtful: bool,
    /// MAH: horizontal manoeuvre sensed.
    pub manoeuvre: bool,
    /// CDM, two bits: 0 maintaining, 1 climbing, 2 descending, 3 unknown.
    pub cdm: u8,
    /// First extent (TRE, GHO, SUP, TCC) and later extents.
    pub ext: Vec<u8>,
}

/// One CAT048 data record. A7.2 and A3: a short-lived decode result, not a hot collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Cat048 {
    /// I048/010 (mandatory).
    pub source: SourceId,
    /// I048/140 time of day, 1/128 s since midnight (24 bits).
    pub time_of_day: Option<u32>,
    /// I048/020.
    pub descriptor: Option<Trd048>,
    /// I048/040.
    pub polar: Option<Polar>,
    /// I048/070.
    pub mode3a: Option<Mode3a>,
    /// I048/090.
    pub flight_level: Option<FlightLevel048>,
    /// I048/130.
    pub plot: Option<PlotCharacteristics>,
    /// I048/220 aircraft address, 24 bits.
    pub address: Option<u32>,
    /// I048/240 aircraft identification.
    pub ident: Option<Ident>,
    /// I048/250 Mode S MB data, in wire order.
    pub mb_data: Option<Vec<MbRegister>>,
    /// I048/161 track number, 12 bits.
    pub track_number: Option<u16>,
    /// I048/042.
    pub cartesian: Option<Cartesian>,
    /// I048/170.
    pub track_status: Option<TrackStatus048>,
    /// Items not decoded above (030, 050, 055, 060, 065, 080, 100, 110, 120, 200, 210, 230, 260,
    /// SP, RE).
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

/// Decodes the item `bytes` of FRN `frn` into `rec`; `Ok(false)` if the FRN is not typed here.
pub(crate) fn decode_item(rec: &mut Cat048, frn: u8, b: &[u8]) -> Result<bool, ErrorKind> {
    match frn {
        1 => {
            rec.source = SourceId {
                sac: first(b)?,
                sic: *b.get(1).ok_or(ErrorKind::Truncated)?,
            }
        }
        2 => rec.time_of_day = Some(fixed(b, 3)?),
        3 => {
            let p = first(b)?;
            rec.descriptor = Some(Trd048 {
                typ: p >> 5,
                simulated: p >> 4 & 1 == 1,
                rdp_chain2: p >> 3 & 1 == 1,
                spi: p >> 2 & 1 == 1,
                field_monitor: p >> 1 & 1 == 1,
                ext: tail(b, 1),
            });
        }
        4 => {
            let v = fixed(b, 4)?;
            rec.polar = Some(Polar {
                rho: (v >> 16) as u16,
                theta: (v & 0xFFFF) as u16,
            });
        }
        5 => {
            let v = fixed(b, 2)?;
            rec.mode3a = Some(Mode3a {
                not_validated: v >> 15 & 1 == 1,
                garbled: v >> 14 & 1 == 1,
                l_or_change: v >> 13 & 1 == 1,
                code: (v & 0xFFF) as u16,
            });
        }
        6 => {
            let v = fixed(b, 2)?;
            rec.flight_level = Some(FlightLevel048 {
                not_validated: v >> 15 & 1 == 1,
                garbled: v >> 14 & 1 == 1,
                raw: sext(v & 0x3FFF, 14) as i16,
            });
        }
        7 => {
            let mut pc = PlotCharacteristics::default();
            for (i, s, _) in compound_parts(b, compound_subs(&UAP_048, 7))? {
                let o = *b.get(s).ok_or(ErrorKind::Truncated)?;
                match i {
                    0 => pc.ssr_runlength = Some(o),
                    1 => pc.ssr_replies = Some(o),
                    2 => pc.ssr_amplitude = Some(o as i8),
                    3 => pc.psr_runlength = Some(o),
                    4 => pc.psr_amplitude = Some(o as i8),
                    5 => pc.range_diff = Some(o as i8),
                    _ => pc.azimuth_diff = Some(o as i8),
                }
            }
            rec.plot = Some(pc);
        }
        8 => rec.address = Some(fixed(b, 3)?),
        9 => {
            if b.len() != 6 {
                return Err(ErrorKind::InvalidValue);
            }
            rec.ident = Some(decode_ident(b)?);
        }
        10 => {
            let n = usize::from(first(b)?);
            let mut regs = Vec::with_capacity(n);
            let mut pos = 1;
            for _ in 0..n {
                let d = take(b, &mut pos, 7)?;
                let bds = take(b, &mut pos, 1)?;
                let mut data = [0u8; 7];
                data.copy_from_slice(d);
                regs.push(MbRegister {
                    data,
                    bds: first(bds)?,
                });
            }
            done(b, pos)?;
            rec.mb_data = Some(regs);
        }
        11 => rec.track_number = Some(fixed(b, 2)? as u16 & 0xFFF),
        12 => {
            let v = fixed(b, 4)?;
            rec.cartesian = Some(Cartesian {
                x: sext(v >> 16, 16) as i16,
                y: sext(v & 0xFFFF, 16) as i16,
            });
        }
        14 => {
            let p = first(b)?;
            rec.track_status = Some(TrackStatus048 {
                tentative: p >> 7 & 1 == 1,
                rad: p >> 5 & 3,
                doubtful: p >> 4 & 1 == 1,
                manoeuvre: p >> 3 & 1 == 1,
                cdm: p >> 1 & 3,
                ext: tail(b, 1),
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

/// The typed items of `rec` as `(FRN, bytes)` in FRN order.
pub(crate) fn encode_items(rec: &Cat048) -> Result<Items, ItemError> {
    let mut o = Vec::new();
    one(&mut o, 1, |v| {
        v.push(rec.source.sac);
        v.push(rec.source.sic);
        Ok(())
    })?;
    if let Some(t) = rec.time_of_day {
        one(&mut o, 2, |v| emit_u(v, t, 24, 3))?;
    }
    if let Some(t) = &rec.descriptor {
        one(&mut o, 3, |v| {
            let p = pack_u(u32::from(t.typ), 3)? << 5
                | flag(t.simulated, 4)
                | flag(t.rdp_chain2, 3)
                | flag(t.spi, 2)
                | flag(t.field_monitor, 1)
                | u32::from(!t.ext.is_empty());
            put(v, p, 1);
            v.extend_from_slice(&t.ext);
            Ok(())
        })?;
    }
    if let Some(p) = &rec.polar {
        one(&mut o, 4, |v| {
            put(v, u32::from(p.rho), 2);
            put(v, u32::from(p.theta), 2);
            Ok(())
        })?;
    }
    if let Some(m) = &rec.mode3a {
        one(&mut o, 5, |v| {
            let p = flag(m.not_validated, 15)
                | flag(m.garbled, 14)
                | flag(m.l_or_change, 13)
                | pack_u(u32::from(m.code), 12)?;
            put(v, p, 2);
            Ok(())
        })?;
    }
    if let Some(f) = &rec.flight_level {
        one(&mut o, 6, |v| {
            let p = flag(f.not_validated, 15) | flag(f.garbled, 14) | pack_s(i32::from(f.raw), 14)?;
            put(v, p, 2);
            Ok(())
        })?;
    }
    if let Some(pc) = &rec.plot {
        let vals = [
            pc.ssr_runlength,
            pc.ssr_replies,
            pc.ssr_amplitude.map(|x| x as u8),
            pc.psr_runlength,
            pc.psr_amplitude.map(|x| x as u8),
            pc.range_diff.map(|x| x as u8),
            pc.azimuth_diff.map(|x| x as u8),
        ];
        let parts: Vec<(usize, Vec<u8>)> = vals
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.map(|b| (i, vec![b])))
            .collect();
        if parts.is_empty() {
            return Err((7, ErrorKind::EmptyCompound));
        }
        o.push((7, compound_join(&parts)));
    }
    if let Some(a) = rec.address {
        one(&mut o, 8, |v| emit_u(v, a, 24, 3))?;
    }
    if let Some(id) = &rec.ident {
        one(&mut o, 9, |v| encode_ident(id, v))?;
    }
    if let Some(regs) = &rec.mb_data {
        one(&mut o, 10, |v| {
            let n = u8::try_from(regs.len())
                .ok()
                .filter(|&n| usize::from(n) <= MAX_REPETITIONS)
                .ok_or(ErrorKind::ValueOutOfRange)?;
            if n == 0 {
                return Err(ErrorKind::ZeroRepetition);
            }
            v.push(n);
            for r in regs {
                v.extend_from_slice(&r.data);
                v.push(r.bds);
            }
            Ok(())
        })?;
    }
    if let Some(n) = rec.track_number {
        one(&mut o, 11, |v| emit_u(v, u32::from(n), 12, 2))?;
    }
    if let Some(c) = &rec.cartesian {
        one(&mut o, 12, |v| {
            emit_s(v, i32::from(c.x), 16, 2)?;
            emit_s(v, i32::from(c.y), 16, 2)
        })?;
    }
    if let Some(t) = &rec.track_status {
        one(&mut o, 14, |v| {
            let p = flag(t.tentative, 7)
                | pack_u(u32::from(t.rad), 2)? << 5
                | flag(t.doubtful, 4)
                | flag(t.manoeuvre, 3)
                | pack_u(u32::from(t.cdm), 2)? << 1
                | u32::from(!t.ext.is_empty());
            put(v, p, 1);
            v.extend_from_slice(&t.ext);
            Ok(())
        })?;
    }
    Ok(o)
}

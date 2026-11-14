//! Codestream markers (T.800 Annex A): SOC, SIZ, COD, COC, QCD, QCC, COM, SOT, SOD, EOC. Core
//! (pure). Anything outside the supported profile is refused by its marker or by the field
//! that asks for it.
//!
//! Supported: one tile in one tile-part, one component without subsampling, LRCP progression,
//! the reversible 5-3 wavelet with no quantization, code-block style 0, no precinct
//! partitioning, no SOP or EPH markers. The informative markers TLM, PLM, PLT and CRG are
//! skipped; POC, PPM, PPT, RGN and the rest are refused.

use crate::error::J2kError;
use crate::{MAX_BITS, MAX_DIM, MAX_LAYERS, MAX_LEVELS, MAX_SAMPLES};

const SOC: u16 = 0xFF4F;
const SIZ: u16 = 0xFF51;
const COD: u16 = 0xFF52;
const COC: u16 = 0xFF53;
const QCD: u16 = 0xFF5C;
const QCC: u16 = 0xFF5D;
const COM: u16 = 0xFF64;
const SOT: u16 = 0xFF90;
const SOD: u16 = 0xFF93;
const EOC: u16 = 0xFFD9;
/// Informative marker segments that carry nothing the decoder needs: TLM, PLM, PLT, CRG.
const SKIPPED: [u16; 4] = [0xFF55, 0xFF57, 0xFF58, 0xFF63];

/// Image and tile geometry from SIZ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Siz {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub bits: u8,
    pub signed: bool,
}

/// Coding style (the parts of COD / COC that this decoder reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Style {
    pub levels: u8,
    /// log2 of the code-block width and height.
    pub xcb: u8,
    pub ycb: u8,
}

/// Quantization (QCD / QCC), reversible: guard bits and one exponent per sub-band in the order
/// LL, then HL LH HH of each resolution from the lowest.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Quant {
    pub guard: u8,
    pub exps: Vec<u8>,
}

/// Everything before the tile data, and the tile data itself.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Parsed<'a> {
    pub siz: Siz,
    pub style: Style,
    pub layers: u16,
    pub quant: Quant,
    /// Packet data of the one tile-part, SOD excluded.
    pub body: &'a [u8],
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self, what: &'static str) -> Result<u8, J2kError> {
        let b = *self.data.get(self.pos).ok_or(J2kError::Truncated(what))?;
        self.pos += 1;
        Ok(b)
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, J2kError> {
        Ok(u16::from(self.u8(what)?) << 8 | u16::from(self.u8(what)?))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, J2kError> {
        Ok(u32::from(self.u16(what)?) << 16 | u32::from(self.u16(what)?))
    }
    /// The `len - 2` bytes after a two-byte length field `len` that was just read.
    fn segment(&mut self, len: u16, what: &'static str) -> Result<&'a [u8], J2kError> {
        let n = usize::from(len)
            .checked_sub(2)
            .ok_or(J2kError::BadMarker("marker length below 2"))?;
        let s = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or(J2kError::Truncated(what))?;
        self.pos += n;
        Ok(s)
    }
}

fn parse_siz(seg: &[u8]) -> Result<Siz, J2kError> {
    let mut r = Reader { data: seg, pos: 0 };
    let t = "SIZ";
    let rsiz = r.u16(t)?;
    if rsiz != 0 {
        return Err(J2kError::Unsupported("capabilities other than Part 1"));
    }
    let xsiz = r.u32(t)?;
    let ysiz = r.u32(t)?;
    let xo = r.u32(t)?;
    let yo = r.u32(t)?;
    let xt = r.u32(t)?;
    let yt = r.u32(t)?;
    let xto = r.u32(t)?;
    let yto = r.u32(t)?;
    let csiz = r.u16(t)?;
    if xo >= xsiz || yo >= ysiz {
        return Err(J2kError::BadMarker("image area is empty"));
    }
    let (w, h) = (xsiz - xo, ysiz - yo);
    if w > MAX_DIM || h > MAX_DIM || u64::from(w) * u64::from(h) > MAX_SAMPLES {
        return Err(J2kError::ImageTooLarge {
            width: w,
            height: h,
        });
    }
    if csiz != 1 {
        return Err(J2kError::Unsupported("more than one component"));
    }
    let ssiz = r.u8(t)?;
    let (xr, yr) = (r.u8(t)?, r.u8(t)?);
    if r.pos != seg.len() {
        return Err(J2kError::BadMarker(
            "SIZ length disagrees with its components",
        ));
    }
    if xr != 1 || yr != 1 {
        return Err(J2kError::Unsupported("component subsampling"));
    }
    let bits = (ssiz & 0x7F) + 1;
    if bits > MAX_BITS {
        return Err(J2kError::BadBitDepth(bits));
    }
    // One tile: the tile grid must start at or before the image and cover all of it.
    if xt == 0 || yt == 0 || xto > xo || yto > yo {
        return Err(J2kError::BadMarker(
            "tile grid does not cover the image origin",
        ));
    }
    if u64::from(xto) + u64::from(xt) < u64::from(xsiz)
        || u64::from(yto) + u64::from(yt) < u64::from(ysiz)
    {
        return Err(J2kError::Unsupported("more than one tile"));
    }
    Ok(Siz {
        x0: xo,
        y0: yo,
        x1: xsiz,
        y1: ysiz,
        bits,
        signed: ssiz & 0x80 != 0,
    })
}

/// Code-block and wavelet fields shared by COD and COC (SPcod / SPcoc), after the precinct
/// flag has been checked.
fn parse_sp(r: &mut Reader, precincts: bool) -> Result<Style, J2kError> {
    let t = "coding style";
    let levels = r.u8(t)?;
    let cw = r.u8(t)?;
    let ch = r.u8(t)?;
    let cbstyle = r.u8(t)?;
    let transform = r.u8(t)?;
    if levels > MAX_LEVELS {
        return Err(J2kError::BadMarker(
            "more decomposition levels than allowed",
        ));
    }
    if cw > 8 || ch > 8 || cw + ch > 10 {
        return Err(J2kError::BadMarker("code-block size exponent out of range"));
    }
    if cbstyle != 0 {
        return Err(J2kError::UnsupportedBlockStyle(cbstyle));
    }
    if transform != 1 {
        return Err(J2kError::Unsupported("irreversible 9-7 wavelet"));
    }
    if precincts {
        return Err(J2kError::Unsupported("precinct partitioning"));
    }
    Ok(Style {
        levels,
        xcb: cw + 2,
        ycb: ch + 2,
    })
}

fn parse_quant(seg: &[u8], levels: u8) -> Result<Quant, J2kError> {
    let s = *seg.first().ok_or(J2kError::Truncated("quantization"))?;
    if s & 0x1F != 0 {
        return Err(J2kError::UnsupportedQuantization(s & 0x1F));
    }
    let n = 3 * usize::from(levels) + 1;
    let body = seg.get(1..).unwrap_or(&[]);
    if body.len() != n {
        return Err(J2kError::BadMarker(
            "quantization exponents do not match the decomposition levels",
        ));
    }
    Ok(Quant {
        guard: s >> 5,
        exps: body.iter().map(|b| b >> 3).collect(),
    })
}

/// Parses a whole codestream (`SOC` through `EOC`).
pub fn parse(data: &[u8]) -> Result<Parsed<'_>, J2kError> {
    let mut r = Reader { data, pos: 0 };
    if r.u16("SOC").ok() != Some(SOC) {
        return Err(J2kError::NotCodestream);
    }
    let mut siz: Option<Siz> = None;
    let mut cod: Option<(Style, u16)> = None;
    let mut coc: Option<Style> = None;
    let mut qcd: Option<Vec<u8>> = None;
    let mut qcc: Option<Vec<u8>> = None;
    // Main header.
    loop {
        let marker = r.u16("marker")?;
        if marker == SOT {
            r.pos -= 2;
            break;
        }
        if marker >> 8 != 0xFF {
            return Err(J2kError::BadMarker("not a marker where one is required"));
        }
        let len = r.u16("marker length")?;
        let seg = r.segment(len, "marker segment")?;
        match marker {
            SIZ => {
                if siz.is_some() {
                    return Err(J2kError::BadMarker("two SIZ markers"));
                }
                siz = Some(parse_siz(seg)?);
            }
            COD => {
                if cod.is_some() {
                    return Err(J2kError::BadMarker("two COD markers"));
                }
                let mut s = Reader { data: seg, pos: 0 };
                let scod = s.u8("COD")?;
                if scod & 0x06 != 0 {
                    return Err(J2kError::Unsupported("SOP or EPH markers"));
                }
                let prog = s.u8("COD")?;
                let layers = s.u16("COD")?;
                let mct = s.u8("COD")?;
                if prog != 0 {
                    return Err(J2kError::UnsupportedProgression(prog));
                }
                if layers == 0 || layers > MAX_LAYERS {
                    return Err(J2kError::BadMarker("layer count out of range"));
                }
                if mct != 0 {
                    return Err(J2kError::Unsupported("multiple component transform"));
                }
                let style = parse_sp(&mut s, scod & 1 != 0)?;
                if s.pos != seg.len() {
                    return Err(J2kError::BadMarker("COD length disagrees with its fields"));
                }
                cod = Some((style, layers));
            }
            COC => {
                if coc.is_some() {
                    return Err(J2kError::BadMarker("two COC markers"));
                }
                let mut s = Reader { data: seg, pos: 0 };
                if s.u8("COC")? != 0 {
                    return Err(J2kError::BadMarker(
                        "COC names a component that does not exist",
                    ));
                }
                let scoc = s.u8("COC")?;
                if scoc & 0x06 != 0 {
                    return Err(J2kError::BadMarker("reserved bits in Scoc"));
                }
                let style = parse_sp(&mut s, scoc & 1 != 0)?;
                if s.pos != seg.len() {
                    return Err(J2kError::BadMarker("COC length disagrees with its fields"));
                }
                coc = Some(style);
            }
            QCD => {
                if qcd.is_some() {
                    return Err(J2kError::BadMarker("two QCD markers"));
                }
                qcd = Some(seg.to_vec());
            }
            QCC => {
                if qcc.is_some() {
                    return Err(J2kError::BadMarker("two QCC markers"));
                }
                if seg.first() != Some(&0) {
                    return Err(J2kError::BadMarker(
                        "QCC names a component that does not exist",
                    ));
                }
                qcc = Some(seg.get(1..).unwrap_or(&[]).to_vec());
            }
            COM => {}
            m if SKIPPED.contains(&m) => {}
            m => return Err(J2kError::UnsupportedMarker(m)),
        }
    }
    let siz = siz.ok_or(J2kError::MissingMarker("SIZ"))?;
    let (cod_style, layers) = cod.ok_or(J2kError::MissingMarker("COD"))?;
    // COC and QCC override COD and QCD for their component, whatever their order.
    let style = coc.unwrap_or(cod_style);
    let qseg = qcc.or(qcd).ok_or(J2kError::MissingMarker("QCD"))?;
    let quant = parse_quant(&qseg, style.levels)?;
    // Tile-part: SOT, its header, SOD, data.
    let sot_at = r.pos;
    if r.u16("SOT")? != SOT {
        return Err(J2kError::MissingMarker("SOT"));
    }
    if r.u16("SOT")? != 10 {
        return Err(J2kError::BadMarker("SOT length is not 10"));
    }
    let isot = r.u16("SOT")?;
    let psot = r.u32("SOT")?;
    let tpsot = r.u8("SOT")?;
    let tnsot = r.u8("SOT")?;
    if isot != 0 {
        return Err(J2kError::Unsupported("more than one tile"));
    }
    if tpsot != 0 || tnsot > 1 {
        return Err(J2kError::Unsupported("more than one tile-part"));
    }
    loop {
        let marker = r.u16("tile-part header")?;
        if marker == SOD {
            break;
        }
        if marker >> 8 != 0xFF {
            return Err(J2kError::BadMarker("not a marker where one is required"));
        }
        let len = r.u16("marker length")?;
        r.segment(len, "marker segment")?;
        match marker {
            COM => {}
            m if SKIPPED.contains(&m) => {}
            m => return Err(J2kError::UnsupportedMarker(m)),
        }
    }
    // The tile-part ends where Psot says (0: at the closing EOC), and EOC ends the stream.
    let end = if psot == 0 {
        data.len()
            .checked_sub(2)
            .ok_or(J2kError::Truncated("EOC"))?
    } else {
        sot_at
            .checked_add(psot as usize)
            .ok_or(J2kError::BadMarker("Psot overflows"))?
    };
    if end < r.pos || end > data.len() {
        return Err(J2kError::BadMarker("Psot runs past the codestream"));
    }
    let body = data
        .get(r.pos..end)
        .ok_or(J2kError::Truncated("tile data"))?;
    let tail = data.get(end..).unwrap_or(&[]);
    let mut tr = Reader { data: tail, pos: 0 };
    match tr.u16("EOC")? {
        EOC => {
            if tail.len() > 2 {
                return Err(J2kError::TrailingBytes(tail.len() - 2));
            }
        }
        SOT => return Err(J2kError::Unsupported("more than one tile-part")),
        _ => return Err(J2kError::MissingMarker("EOC")),
    }
    Ok(Parsed {
        siz,
        style,
        layers,
        quant,
        body,
    })
}

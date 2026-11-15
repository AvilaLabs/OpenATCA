//! Test-only TIFF writer: builds classic TIFF / GeoTIFF files in memory with every layout, byte
//! order, compression and predictor the reader supports, so each is checked against known data.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::tiff::adler32;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fmt {
    F32,
    I16,
}

#[derive(Clone)]
pub struct Spec {
    pub big: bool,
    pub width: u32,
    pub height: u32,
    /// (tile width, tile length) or strips of `rows_per_strip`.
    pub tile: Option<(u32, u32)>,
    pub rows_per_strip: u32,
    /// 1 none, 5 LZW, 8 or 32946 deflate.
    pub compression: u16,
    pub predictor: u16,
    pub fmt: Fmt,
    pub data: Vec<f32>,
    pub geo: bool,
    pub point: bool,
    pub scale: (f64, f64),
    /// Tiepoint raster (i, j) and model (x, y).
    pub tie: (f64, f64, f64, f64),
    pub crs: u16,
    pub model_type: u16,
    pub nodata: Option<String>,
    pub vertical: Option<u16>,
    /// Orientation tag (274) written when set.
    pub orientation: Option<u16>,
}

impl Spec {
    pub fn new(width: u32, height: u32, data: Vec<f32>) -> Spec {
        Spec {
            big: false,
            width,
            height,
            tile: None,
            rows_per_strip: height,
            compression: 1,
            predictor: 1,
            fmt: Fmt::F32,
            data,
            geo: true,
            point: false,
            scale: (0.001, 0.001),
            tie: (0.0, 0.0, -90.0, 40.0),
            crs: 4269,
            model_type: 2,
            nodata: None,
            vertical: None,
            orientation: None,
        }
    }
}

struct Ent {
    tag: u16,
    typ: u16,
    count: u32,
    bytes: Vec<u8>,
}

fn w16(big: bool, v: u16) -> [u8; 2] {
    if big {
        v.to_be_bytes()
    } else {
        v.to_le_bytes()
    }
}

fn w32(big: bool, v: u32) -> [u8; 4] {
    if big {
        v.to_be_bytes()
    } else {
        v.to_le_bytes()
    }
}

fn shorts(big: bool, tag: u16, v: &[u16]) -> Ent {
    Ent {
        tag,
        typ: 3,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|&x| w16(big, x)).collect(),
    }
}

fn longs(big: bool, tag: u16, v: &[u32]) -> Ent {
    Ent {
        tag,
        typ: 4,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|&x| w32(big, x)).collect(),
    }
}

fn dbls(big: bool, tag: u16, v: &[f64]) -> Ent {
    Ent {
        tag,
        typ: 12,
        count: v.len() as u32,
        bytes: v
            .iter()
            .flat_map(|&x| {
                if big {
                    x.to_bits().to_be_bytes()
                } else {
                    x.to_bits().to_le_bytes()
                }
            })
            .collect(),
    }
}

fn ascii(tag: u16, s: &str) -> Ent {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    Ent {
        tag,
        typ: 2,
        count: b.len() as u32,
        bytes: b,
    }
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x9c];
    let mut c = oa_deflate::Compressor::new(6);
    let mut sink = |b: &[u8]| -> Result<(), ()> {
        out.extend_from_slice(b);
        Ok(())
    };
    c.write(data, &mut sink).unwrap();
    c.finish(&mut sink).unwrap();
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u16, width: u32) {
        self.acc = (self.acc << width) | u32::from(code);
        self.n += width;
        while self.n >= 8 {
            self.out.push((self.acc >> (self.n - 8)) as u8);
            self.n -= 8;
        }
        self.acc &= (1 << self.n) - 1;
    }
}

fn lzw_width(next: u32) -> u32 {
    match next {
        0..=511 => 9,
        512..=1023 => 10,
        1024..=2047 => 11,
        _ => 12,
    }
}

/// TIFF 6.0 LZW encoder (early change), written from the specification for the tests.
pub fn lzw_encode(data: &[u8]) -> Vec<u8> {
    let mut b = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    let mut table: HashMap<(u16, u8), u16> = HashMap::new();
    let mut next: u32 = 258;
    b.put(256, 9);
    let mut w: Option<u16> = None;
    for &c in data {
        match w {
            None => w = Some(u16::from(c)),
            Some(cur) => {
                if let Some(&code) = table.get(&(cur, c)) {
                    w = Some(code);
                } else {
                    b.put(cur, lzw_width(next));
                    table.insert((cur, c), next as u16);
                    next += 1;
                    if next == 4094 {
                        b.put(256, lzw_width(next));
                        table.clear();
                        next = 258;
                    }
                    w = Some(u16::from(c));
                }
            }
        }
    }
    if let Some(cur) = w {
        b.put(cur, lzw_width(next));
        next += 1;
    }
    b.put(257, lzw_width(next));
    if b.n > 0 {
        b.put(0, 8 - b.n);
    }
    b.out
}

/// Encodes one segment's samples (row-major, `row_w` per row) to raw bytes with the predictor.
fn encode_rows(s: &Spec, vals: &[f32], row_w: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in vals.chunks(row_w) {
        match (s.fmt, s.predictor) {
            (Fmt::F32, 3) => {
                let mut t = vec![0u8; row_w * 4];
                for (i, v) in row.iter().enumerate() {
                    let be = v.to_bits().to_be_bytes();
                    for k in 0..4 {
                        t[k * row_w + i] = be[k];
                    }
                }
                for i in (1..t.len()).rev() {
                    t[i] = t[i].wrapping_sub(t[i - 1]);
                }
                out.extend(t);
            }
            (Fmt::F32, _) => {
                for v in row {
                    out.extend(if s.big {
                        v.to_bits().to_be_bytes()
                    } else {
                        v.to_bits().to_le_bytes()
                    });
                }
            }
            (Fmt::I16, p) => {
                let ints: Vec<i16> = row.iter().map(|&v| v as i16).collect();
                for (i, &v) in ints.iter().enumerate() {
                    let d = if p == 2 && i > 0 {
                        v.wrapping_sub(ints[i - 1])
                    } else {
                        v
                    };
                    out.extend(if s.big {
                        d.to_be_bytes()
                    } else {
                        d.to_le_bytes()
                    });
                }
            }
        }
    }
    out
}

fn compress(s: &Spec, raw: &[u8]) -> Vec<u8> {
    match s.compression {
        1 => raw.to_vec(),
        5 => lzw_encode(raw),
        _ => zlib(raw),
    }
}

pub fn build(s: &Spec) -> Vec<u8> {
    let big = s.big;
    let mut file: Vec<u8> = vec![0; 8];
    file[..2].copy_from_slice(if big { b"MM" } else { b"II" });
    file[2..4].copy_from_slice(&w16(big, 42));
    let (w, h) = (s.width as usize, s.height as usize);
    // Segments.
    let mut offsets = Vec::new();
    let mut counts = Vec::new();
    let mut push_seg = |file: &mut Vec<u8>, bytes: Vec<u8>| {
        if file.len() % 2 == 1 {
            file.push(0);
        }
        offsets.push(file.len() as u32);
        counts.push(bytes.len() as u32);
        file.extend(bytes);
    };
    if let Some((tw, th)) = s.tile {
        let (tw, th) = (tw as usize, th as usize);
        for ty in 0..h.div_ceil(th) {
            for tx in 0..w.div_ceil(tw) {
                let mut vals = vec![0.0f32; tw * th];
                for r in 0..th {
                    for c in 0..tw {
                        let (y, x) = (ty * th + r, tx * tw + c);
                        if y < h && x < w {
                            vals[r * tw + c] = s.data[y * w + x];
                        }
                    }
                }
                let raw = encode_rows(s, &vals, tw);
                push_seg(&mut file, compress(s, &raw));
            }
        }
    } else {
        let rps = s.rows_per_strip as usize;
        for y0 in (0..h).step_by(rps) {
            let y1 = (y0 + rps).min(h);
            let raw = encode_rows(s, &s.data[y0 * w..y1 * w], w);
            push_seg(&mut file, compress(s, &raw));
        }
    }
    let (bits, sf) = match s.fmt {
        Fmt::F32 => (32u16, 3u16),
        Fmt::I16 => (16, 2),
    };
    let mut ents = vec![
        longs(big, 256, &[s.width]),
        longs(big, 257, &[s.height]),
        shorts(big, 258, &[bits]),
        shorts(big, 259, &[s.compression]),
        shorts(big, 262, &[1]),
        shorts(big, 277, &[1]),
        shorts(big, 284, &[1]),
        shorts(big, 339, &[sf]),
    ];
    if s.predictor != 1 {
        ents.push(shorts(big, 317, &[s.predictor]));
    }
    if let Some((tw, th)) = s.tile {
        ents.push(longs(big, 322, &[tw]));
        ents.push(longs(big, 323, &[th]));
        ents.push(longs(big, 324, &offsets));
        ents.push(longs(big, 325, &counts));
    } else {
        ents.push(longs(big, 273, &offsets));
        ents.push(longs(big, 278, &[s.rows_per_strip]));
        ents.push(longs(big, 279, &counts));
    }
    if s.geo {
        ents.push(dbls(big, 33550, &[s.scale.0, s.scale.1, 0.0]));
        ents.push(dbls(
            big,
            33922,
            &[s.tie.0, s.tie.1, 0.0, s.tie.2, s.tie.3, 0.0],
        ));
        let mut keys: Vec<[u16; 4]> = vec![
            [1024, 0, 1, s.model_type],
            [1025, 0, 1, if s.point { 2 } else { 1 }],
            [2048, 0, 1, s.crs],
        ];
        if let Some(v) = s.vertical {
            keys.push([4096, 0, 1, v]);
        }
        let mut dir: Vec<u16> = vec![1, 1, 0, keys.len() as u16];
        dir.extend(keys.iter().flatten());
        ents.push(shorts(big, 34735, &dir));
    }
    if let Some(o) = s.orientation {
        ents.push(shorts(big, 274, &[o]));
    }
    if let Some(nd) = &s.nodata {
        ents.push(ascii(42113, nd));
    }
    ents.sort_by_key(|e| e.tag);
    // Out-of-line values first, then the IFD.
    let mut values_at: Vec<Option<u32>> = Vec::new();
    for e in &ents {
        if e.bytes.len() > 4 {
            if file.len() % 2 == 1 {
                file.push(0);
            }
            values_at.push(Some(file.len() as u32));
            file.extend(&e.bytes);
        } else {
            values_at.push(None);
        }
    }
    if file.len() % 2 == 1 {
        file.push(0);
    }
    let ifd = file.len() as u32;
    file[4..8].copy_from_slice(&w32(big, ifd));
    file.extend(w16(big, ents.len() as u16));
    for (e, at) in ents.iter().zip(values_at) {
        file.extend(w16(big, e.tag));
        file.extend(w16(big, e.typ));
        file.extend(w32(big, e.count));
        match at {
            Some(o) => file.extend(w32(big, o)),
            None => {
                let mut v = e.bytes.clone();
                v.resize(4, 0);
                file.extend(v);
            }
        }
    }
    file.extend(w32(big, 0));
    file
}

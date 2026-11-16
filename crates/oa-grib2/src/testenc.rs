//! Test-only encoder: builds GRIB2 sections and messages, with simple and complex packing, so
//! decoder tests can round-trip exactly. Written independently of the decoder from the template
//! tables; it shares only the sign-magnitude convention.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

pub struct BitWriter {
    pub bytes: Vec<u8>,
    nbits: usize,
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter {
            bytes: Vec::new(),
            nbits: 0,
        }
    }
    pub fn put(&mut self, v: u64, n: u32) {
        for k in (0..n).rev() {
            let bit = (v >> k) & 1;
            if self.nbits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if bit == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.nbits % 8);
            }
            self.nbits += 1;
        }
    }
    pub fn align(&mut self) {
        while !self.nbits.is_multiple_of(8) {
            self.nbits += 1;
        }
    }
}

pub fn be(v: u64, n: usize) -> Vec<u8> {
    (0..n).rev().map(|k| (v >> (8 * k)) as u8).collect()
}

/// Sign-magnitude encoding of `v` in `n` bytes.
pub fn sm(v: i64, n: usize) -> Vec<u8> {
    let mag = v.unsigned_abs();
    assert!(mag < 1u64 << (n * 8 - 1));
    be(mag | if v < 0 { 1u64 << (n * 8 - 1) } else { 0 }, n)
}

fn section(num: u8, body: &[u8]) -> Vec<u8> {
    let mut s = be((body.len() + 5) as u64, 4);
    s.push(num);
    s.extend_from_slice(body);
    s
}

pub fn sect1(year: u16, month: u8, day: u8, hour: u8) -> Vec<u8> {
    let mut b = be(7, 2); // centre NCEP
    b.extend(be(0, 2));
    b.extend([2, 1, 1]); // master table, local table, significance
    b.extend(be(u64::from(year), 2));
    b.extend([month, day, hour, 0, 0, 0, 1]);
    b.push(1); // forecast
    section(1, &b)
}

pub struct LambertSpec {
    pub nx: u32,
    pub ny: u32,
    pub la1_micro: i64,
    pub lo1_micro: i64,
    pub flags: u8,
    pub lad_micro: i64,
    pub lov_micro: i64,
    pub dx_mm: u32,
    pub dy_mm: u32,
    pub scan: u8,
    pub latin1_micro: i64,
    pub latin2_micro: i64,
    pub shape: u8,
}

/// The HRRR grid as published (1799 x 1059), with a reduced size when asked.
pub fn hrrr_spec(nx: u32, ny: u32) -> LambertSpec {
    LambertSpec {
        nx,
        ny,
        la1_micro: 21_138_123,
        lo1_micro: 237_280_472,
        flags: 0x08,
        lad_micro: 38_500_000,
        lov_micro: 262_500_000,
        dx_mm: 3_000_000,
        dy_mm: 3_000_000,
        scan: 0x40,
        latin1_micro: 38_500_000,
        latin2_micro: 38_500_000,
        shape: 6,
    }
}

pub fn sect3_lambert(s: &LambertSpec) -> Vec<u8> {
    let mut b = vec![0u8]; // source
    b.extend(be(u64::from(s.nx) * u64::from(s.ny), 4));
    b.push(0); // optional list octets
    b.push(0); // interpretation
    b.extend(be(30, 2));
    b.push(s.shape);
    b.push(0);
    b.extend(be(0, 4));
    b.push(0);
    b.extend(be(0, 4));
    b.push(0);
    b.extend(be(0, 4));
    b.extend(be(u64::from(s.nx), 4));
    b.extend(be(u64::from(s.ny), 4));
    b.extend(sm(s.la1_micro, 4));
    b.extend(sm(s.lo1_micro, 4));
    b.push(s.flags);
    b.extend(sm(s.lad_micro, 4));
    b.extend(sm(s.lov_micro, 4));
    b.extend(be(u64::from(s.dx_mm), 4));
    b.extend(be(u64::from(s.dy_mm), 4));
    b.push(0); // projection centre
    b.push(s.scan);
    b.extend(sm(s.latin1_micro, 4));
    b.extend(sm(s.latin2_micro, 4));
    b.extend(sm(-90_000_000, 4));
    b.extend(sm(0, 4));
    section(3, &b)
}

pub struct LatLonSpec {
    pub ni: u32,
    pub nj: u32,
    pub la1_micro: i64,
    pub lo1_micro: i64,
    pub la2_micro: i64,
    pub lo2_micro: i64,
    pub di_micro: u32,
    pub dj_micro: u32,
    pub scan: u8,
}

/// GFS 0.25 degree grid, global, north to south.
pub fn gfs_spec() -> LatLonSpec {
    LatLonSpec {
        ni: 1440,
        nj: 721,
        la1_micro: 90_000_000,
        lo1_micro: 0,
        la2_micro: -90_000_000,
        lo2_micro: 359_750_000,
        di_micro: 250_000,
        dj_micro: 250_000,
        scan: 0,
    }
}

pub fn sect3_latlon(s: &LatLonSpec) -> Vec<u8> {
    let mut b = vec![0u8];
    b.extend(be(u64::from(s.ni) * u64::from(s.nj), 4));
    b.push(0);
    b.push(0);
    b.extend(be(0, 2));
    b.push(6);
    b.push(0);
    b.extend(be(0, 4));
    b.push(0);
    b.extend(be(0, 4));
    b.push(0);
    b.extend(be(0, 4));
    b.extend(be(u64::from(s.ni), 4));
    b.extend(be(u64::from(s.nj), 4));
    b.extend(be(0, 4)); // basic angle
    b.extend(be(0xFFFF_FFFF, 4)); // subdivisions: missing -> microdegrees
    b.extend(sm(s.la1_micro, 4));
    b.extend(sm(s.lo1_micro, 4));
    b.push(0x30);
    b.extend(sm(s.la2_micro, 4));
    b.extend(sm(s.lo2_micro, 4));
    b.extend(be(u64::from(s.di_micro), 4));
    b.extend(be(u64::from(s.dj_micro), 4));
    b.push(s.scan);
    section(3, &b)
}

/// Template 4.0 (or 4.8, padded) with one fixed surface.
pub fn sect4(
    template: u16,
    category: u8,
    number: u8,
    level_kind: u8,
    scale: i64,
    scaled: u32,
) -> Vec<u8> {
    let mut b = be(0, 2);
    b.extend(be(u64::from(template), 2));
    b.extend([category, number, 2, 0, 0]);
    b.extend(be(0, 2));
    b.push(0);
    b.push(1); // hours
    b.extend(be(6, 4)); // forecast time
    b.push(level_kind);
    b.extend(sm(scale, 1));
    b.extend(be(u64::from(scaled), 4));
    b.push(255);
    b.push(0);
    b.extend(be(0, 4));
    if template == 8 {
        // end of interval, 1 time range
        b.extend(be(2026, 2));
        b.extend([11, 14, 19, 0, 0, 1]);
        b.extend(be(0, 4));
        b.extend([1, 2, 1]);
        b.extend(be(1, 4));
        b.extend([1]);
        b.extend(be(0, 4));
    }
    section(4, &b)
}

#[derive(Clone)]
pub struct Packed {
    pub sect5: Vec<u8>,
    pub sect7: Vec<u8>,
}

fn bits_for(v: u64) -> u32 {
    64 - v.leading_zeros()
}

fn sect5_head(template: u16, ndata: u32, r: f32, e: i64, d: i64, nbits: u32) -> Vec<u8> {
    let mut b = be(u64::from(ndata), 4);
    b.extend(be(u64::from(template), 2));
    b.extend(r.to_bits().to_be_bytes());
    b.extend(sm(e, 2));
    b.extend(sm(d, 2));
    b.push(nbits as u8);
    b.push(1); // original values are integers
    b
}

/// Simple packing of non-negative integers `x` with reference `r`, binary scale `e`, decimal `d`.
pub fn encode_simple(x: &[u64], r: f32, e: i64, d: i64) -> Packed {
    let nbits = bits_for(x.iter().copied().max().unwrap_or(0));
    let mut w = BitWriter::new();
    for &v in x {
        w.put(v, nbits);
    }
    let b = sect5_head(0, x.len() as u32, r, e, d, nbits);
    Packed {
        sect5: section(5, &b),
        sect7: section(7, &w.bytes),
    }
}

/// Template 5.40 around an existing JPEG 2000 codestream: `ndata` values of `nbits` bits, with
/// compression type `compression` (0 lossless) and no target ratio.
pub fn encode_jpeg2000(
    codestream: &[u8],
    ndata: u32,
    r: f32,
    e: i64,
    d: i64,
    nbits: u32,
    compression: u8,
) -> Packed {
    let mut b = sect5_head(40, ndata, r, e, d, nbits);
    b.push(compression);
    b.push(255);
    Packed {
        sect5: section(5, &b),
        sect7: section(7, codestream),
    }
}

pub struct ComplexOpts {
    /// Lengths of every group; they must add up to the number of points.
    pub group_lens: Vec<usize>,
    pub order: u8,
    pub missing_mgmt: u8,
    /// Octets per spatial-differencing descriptor.
    pub extra_octets: usize,
}

/// Complex packing (5.2) or, with `order > 0`, with spatial differencing (5.3). `x` holds the
/// integers to store, `None` for a missing point.
pub fn encode_complex(x: &[Option<i64>], r: f32, e: i64, d: i64, o: &ComplexOpts) -> Packed {
    assert_eq!(o.group_lens.iter().sum::<usize>(), x.len());
    let mut vals: Vec<Option<i64>> = x.to_vec();
    let mut descriptors: Vec<u8> = Vec::new();
    if o.order > 0 {
        let order = usize::from(o.order);
        let idx: Vec<usize> = (0..vals.len()).filter(|&k| vals[k].is_some()).collect();
        let orig: Vec<i64> = idx.iter().map(|&k| vals[k].unwrap()).collect();
        let mut g = orig.clone();
        for _ in 0..order {
            // difference once, leaving earlier entries for the next pass
            let prev = g.clone();
            for k in (1..g.len()).rev() {
                g[k] = prev[k] - prev[k - 1];
            }
        }
        // After `order` passes entries 0..order hold partial differences; the first `order`
        // values are carried as descriptors and zeroed.
        let min = g.iter().skip(order).copied().min().unwrap_or(0);
        for (k, v) in g.iter_mut().enumerate() {
            *v = if k < order { 0 } else { *v - min };
        }
        for k in 0..order {
            descriptors.extend(sm(orig.get(k).copied().unwrap_or(0), o.extra_octets));
        }
        descriptors.extend(sm(min, o.extra_octets));
        for (n, &k) in idx.iter().enumerate() {
            vals[k] = Some(g[n]);
        }
    }
    // Group by the given lengths.
    let mut start = 0;
    let mut refs: Vec<u64> = Vec::new();
    let mut widths: Vec<u32> = Vec::new();
    let mut groups: Vec<Vec<Option<i64>>> = Vec::new();
    let reserve = u64::from(o.missing_mgmt);
    // First pass: widths and references, nbits for the references.
    for &l in &o.group_lens {
        let g: Vec<Option<i64>> = vals[start..start + l].to_vec();
        start += l;
        let present: Vec<i64> = g.iter().flatten().copied().collect();
        if present.is_empty() {
            refs.push(u64::MAX); // all missing: set below once nbits is known
            widths.push(0);
        } else {
            let mn = *present.iter().min().unwrap();
            let mx = *present.iter().max().unwrap();
            refs.push(mn as u64);
            let span = (mx - mn) as u64;
            let w = if span == 0 && !g.iter().any(Option::is_none) {
                0
            } else {
                bits_for(span + reserve)
            };
            widths.push(w);
        }
        groups.push(g);
    }
    let max_ref = refs
        .iter()
        .copied()
        .filter(|&v| v != u64::MAX)
        .max()
        .unwrap_or(0);
    let nbits = if o.missing_mgmt > 0 {
        bits_for(max_ref + reserve + 1)
    } else {
        bits_for(max_ref)
    };
    let all_ones = if nbits == 0 { 0 } else { (1u64 << nbits) - 1 };
    for v in refs.iter_mut() {
        if *v == u64::MAX {
            *v = all_ones;
        }
    }
    let width_ref = widths.iter().copied().min().unwrap_or(0);
    let width_bits = bits_for(u64::from(
        widths.iter().copied().max().unwrap_or(0) - width_ref,
    ));
    let (last, firsts) = o.group_lens.split_last().unwrap();
    let length_ref = firsts.iter().copied().min().unwrap_or(0);
    let length_bits = bits_for(
        firsts
            .iter()
            .map(|&l| (l - length_ref) as u64)
            .max()
            .unwrap_or(0),
    );
    let mut w = BitWriter::new();
    for b in &descriptors {
        w.put(u64::from(*b), 8);
    }
    for &v in &refs {
        w.put(v, nbits);
    }
    w.align();
    for &gw in &widths {
        w.put(u64::from(gw - width_ref), width_bits);
    }
    w.align();
    for &l in firsts {
        w.put((l - length_ref) as u64, length_bits);
    }
    // The table has NG entries; the last one is a placeholder for the true length in octets 43-46.
    w.put(0, length_bits);
    w.align();
    for (gi, g) in groups.iter().enumerate() {
        let gw = widths[gi];
        if gw == 0 {
            continue;
        }
        let mn = refs[gi];
        for v in g {
            match v {
                Some(v) => w.put(*v as u64 - mn, gw),
                None => w.put((1u64 << gw) - 1, gw),
            }
        }
    }
    let ndata = x.len() as u32;
    let mut b = sect5_head(if o.order > 0 { 3 } else { 2 }, ndata, r, e, d, nbits);
    b.push(1); // general group splitting
    b.push(o.missing_mgmt);
    b.extend(be(0, 4));
    b.extend(be(0, 4));
    b.extend(be(o.group_lens.len() as u64, 4));
    b.push(width_ref as u8);
    b.push(width_bits as u8);
    b.extend(be(length_ref as u64, 4));
    b.push(1);
    b.extend(be(*last as u64, 4));
    b.push(length_bits as u8);
    if o.order > 0 {
        b.push(o.order);
        b.push(o.extra_octets as u8);
    }
    Packed {
        sect5: section(5, &b),
        sect7: section(7, &w.bytes),
    }
}

/// Section 6 for a bitmap (`Some(bits)`, true = value present), "previous" (`None` with
/// `previous`), or none.
pub fn sect6(bitmap: Option<&[bool]>, previous: bool) -> Vec<u8> {
    match bitmap {
        Some(bits) => {
            let mut w = BitWriter::new();
            for &b in bits {
                w.put(u64::from(b), 1);
            }
            let mut body = vec![0u8];
            body.extend(w.bytes);
            section(6, &body)
        }
        None if previous => section(6, &[254]),
        None => section(6, &[255]),
    }
}

/// Assembles a message from sections 1..7 (already built), adding sections 0 and 8.
pub fn message(discipline: u8, sections: &[Vec<u8>]) -> Vec<u8> {
    let body: usize = sections.iter().map(Vec::len).sum();
    let total = 16 + body + 4;
    let mut m = b"GRIB".to_vec();
    m.extend([0, 0, discipline, 2]);
    m.extend(be(total as u64, 8));
    for s in sections {
        m.extend_from_slice(s);
    }
    m.extend_from_slice(b"7777");
    m
}

/// One-field message on the given grid section.
pub fn simple_message(grid: Vec<u8>, product: Vec<u8>, p: &Packed, bitmap: Vec<u8>) -> Vec<u8> {
    message(
        0,
        &[
            sect1(2026, 11, 14, 18),
            grid,
            product,
            p.sect5.clone(),
            bitmap,
            p.sect7.clone(),
        ],
    )
}

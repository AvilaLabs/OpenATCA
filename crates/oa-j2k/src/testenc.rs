//! A small JPEG 2000 encoder for the profile this crate decodes, used only to build test
//! codestreams. It is written independently of the decoder's internals: contexts are found by
//! scanning neighbours directly, the wavelet is the forward 5-3 of T.800 F.4, and packets are
//! written with their own bit-stuffing and tag-tree writer. The real-data tests, not this
//! module, are the independent check of the decoder against another implementation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

// ---- MQ encoder (T.800 C.2) ----

const QE: [u32; 47] = [
    0x5601, 0x3401, 0x1801, 0x0AC1, 0x0521, 0x0221, 0x5601, 0x5401, 0x4801, 0x3801, 0x3001, 0x2401,
    0x1C01, 0x1601, 0x5601, 0x5401, 0x5101, 0x4801, 0x3801, 0x3401, 0x3001, 0x2801, 0x2401, 0x2201,
    0x1C01, 0x1801, 0x1601, 0x1401, 0x1201, 0x1101, 0x0AC1, 0x09C1, 0x08A1, 0x0521, 0x0441, 0x02A1,
    0x0221, 0x0141, 0x0111, 0x0085, 0x0049, 0x0025, 0x0015, 0x0009, 0x0005, 0x0001, 0x5601,
];
const NMPS: [usize; 47] = [
    1, 2, 3, 4, 5, 38, 7, 8, 9, 10, 11, 12, 13, 29, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 45, 46,
];
const NLPS: [usize; 47] = [
    1, 6, 9, 12, 29, 33, 6, 14, 14, 14, 17, 18, 20, 21, 14, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22,
    23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 46,
];

pub struct MqEnc {
    a: u32,
    c: u32,
    ct: u32,
    /// out[0] is the dummy byte before the first one (BP = BPST - 1).
    out: Vec<u8>,
    idx: [usize; 19],
    mps: [u8; 19],
}

impl MqEnc {
    pub fn new() -> Self {
        let mut idx = [0usize; 19];
        idx[0] = 4;
        idx[17] = 3;
        idx[18] = 46;
        MqEnc {
            a: 0x8000,
            c: 0,
            ct: 12,
            out: vec![0],
            idx,
            mps: [0; 19],
        }
    }

    pub fn encode(&mut self, cx: usize, d: u8) {
        let i = self.idx[cx];
        let qe = QE[i];
        if d == self.mps[cx] {
            self.a -= qe;
            if self.a & 0x8000 == 0 {
                if self.a < qe {
                    self.a = qe;
                } else {
                    self.c += qe;
                }
                self.idx[cx] = NMPS[i];
                self.renorm();
            } else {
                self.c += qe;
            }
        } else {
            self.a -= qe;
            if self.a < qe {
                self.c += qe;
            } else {
                self.a = qe;
            }
            if matches!(i, 0 | 6 | 14) {
                self.mps[cx] = 1 - self.mps[cx];
            }
            self.idx[cx] = NLPS[i];
            self.renorm();
        }
    }

    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byteout();
            }
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }

    fn byteout(&mut self) {
        let last = self.out.len() - 1;
        if self.out[last] == 0xFF {
            self.out.push((self.c >> 20) as u8);
            self.c &= 0xFFFFF;
            self.ct = 7;
        } else if self.c < 0x800_0000 {
            self.out.push((self.c >> 19) as u8);
            self.c &= 0x7FFFF;
            self.ct = 8;
        } else {
            self.out[last] += 1;
            if self.out[last] == 0xFF {
                self.c &= 0x7FF_FFFF;
                self.out.push((self.c >> 20) as u8);
                self.c &= 0xFFFFF;
                self.ct = 7;
            } else {
                self.out.push((self.c >> 19) as u8);
                self.c &= 0x7FFFF;
                self.ct = 8;
            }
        }
    }

    /// FLUSH (Figure C.11): the bytes of the codeword segment, without a trailing 0xFF.
    pub fn flush(mut self) -> Vec<u8> {
        let temp = self.c + self.a;
        self.c |= 0xFFFF;
        if self.c >= temp {
            self.c -= 0x8000;
        }
        self.c <<= self.ct;
        self.byteout();
        self.c <<= self.ct;
        self.byteout();
        if *self.out.last().unwrap() == 0xFF {
            self.out.pop();
        }
        self.out.remove(0);
        self.out
    }
}

// ---- Tier-1 encoder, contexts by direct neighbour scan ----

struct Grid {
    w: usize,
    h: usize,
    sig: Vec<bool>,
    neg: Vec<bool>,
}

impl Grid {
    fn at(&self, x: isize, y: isize) -> (bool, bool) {
        if x < 0 || y < 0 || x >= self.w as isize || y >= self.h as isize {
            return (false, false);
        }
        let i = y as usize * self.w + x as usize;
        (self.sig[i], self.neg[i])
    }
    fn s(&self, x: isize, y: isize) -> u32 {
        u32::from(self.at(x, y).0)
    }
    /// (horizontal, vertical, diagonal) significant neighbour counts.
    fn hvd(&self, x: usize, y: usize) -> (u32, u32, u32) {
        let (x, y) = (x as isize, y as isize);
        (
            self.s(x - 1, y) + self.s(x + 1, y),
            self.s(x, y - 1) + self.s(x, y + 1),
            self.s(x - 1, y - 1)
                + self.s(x + 1, y - 1)
                + self.s(x - 1, y + 1)
                + self.s(x + 1, y + 1),
        )
    }
    fn zc(&self, orient: u8, x: usize, y: usize) -> usize {
        let (mut h, mut v, d) = self.hvd(x, y);
        if orient == 1 {
            std::mem::swap(&mut h, &mut v);
        }
        if orient == 3 {
            let hv = h + v;
            return match (d, hv) {
                (3.., _) => 8,
                (2, 1..) => 7,
                (2, 0) => 6,
                (1, 2..) => 5,
                (1, 1) => 4,
                (1, 0) => 3,
                (0, 2..) => 2,
                (0, 1) => 1,
                _ => 0,
            };
        }
        match (h, v, d) {
            (2, _, _) => 8,
            (1, 1.., _) => 7,
            (1, 0, 1..) => 6,
            (1, 0, 0) => 5,
            (0, 2, _) => 4,
            (0, 1, _) => 3,
            (0, 0, 2..) => 2,
            (0, 0, 1) => 1,
            _ => 0,
        }
    }
    /// Sign context label and XOR bit (D.3.2).
    fn sc(&self, x: usize, y: usize) -> (usize, u8) {
        let (x, y) = (x as isize, y as isize);
        let c = |a: (bool, bool), b: (bool, bool)| -> i32 {
            let f = |p: (bool, bool)| {
                if !p.0 {
                    0
                } else if p.1 {
                    -1
                } else {
                    1
                }
            };
            (f(a) + f(b)).clamp(-1, 1)
        };
        let h = c(self.at(x - 1, y), self.at(x + 1, y));
        let v = c(self.at(x, y - 1), self.at(x, y + 1));
        match (h, v) {
            (1, 1) => (13, 0),
            (1, 0) => (12, 0),
            (1, -1) => (11, 0),
            (0, 1) => (10, 0),
            (0, 0) => (9, 0),
            (0, -1) => (10, 1),
            (-1, 1) => (11, 1),
            (-1, 0) => (12, 1),
            _ => (13, 1),
        }
    }
}

/// Encodes a block (`w x h` signed coefficients). Returns (zero bit-planes, passes, bytes), or
/// None when the block is all zero. `mb` is the band's bit-plane count.
pub fn encode_block(
    coefs: &[i32],
    w: usize,
    h: usize,
    orient: u8,
    mb: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    let max = coefs.iter().map(|c| c.unsigned_abs()).max().unwrap();
    if max == 0 {
        return None;
    }
    let msb = 31 - max.leading_zeros();
    assert!(msb < mb, "coefficient does not fit the band's bit-planes");
    let mut g = Grid {
        w,
        h,
        sig: vec![false; w * h],
        neg: vec![false; w * h],
    };
    let mut visited = vec![false; w * h];
    let mut refined = vec![false; w * h];
    let mut mq = MqEnc::new();
    let mut passes = 0;
    for bp in (0..=msb).rev() {
        let bit = |x: usize, y: usize| ((coefs[y * w + x].unsigned_abs() >> bp) & 1) as u8;
        let negc = |x: usize, y: usize| coefs[y * w + x] < 0;
        // Significance propagation (not in the first plane).
        if bp != msb {
            for y0 in (0..h).step_by(4) {
                for x in 0..w {
                    for y in y0..(y0 + 4).min(h) {
                        let i = y * w + x;
                        if g.sig[i] || g.hvd(x, y) == (0, 0, 0) {
                            continue;
                        }
                        let b = bit(x, y);
                        mq.encode(g.zc(orient, x, y), b);
                        visited[i] = true;
                        if b == 1 {
                            let (cx, xr) = g.sc(x, y);
                            mq.encode(cx, u8::from(negc(x, y)) ^ xr);
                            g.sig[i] = true;
                            g.neg[i] = negc(x, y);
                        }
                    }
                }
            }
            passes += 1;
            // Magnitude refinement.
            for y0 in (0..h).step_by(4) {
                for x in 0..w {
                    for y in y0..(y0 + 4).min(h) {
                        let i = y * w + x;
                        if !g.sig[i] || visited[i] {
                            continue;
                        }
                        let cx = if refined[i] {
                            16
                        } else if g.hvd(x, y) != (0, 0, 0) {
                            15
                        } else {
                            14
                        };
                        mq.encode(cx, bit(x, y));
                        refined[i] = true;
                    }
                }
            }
            passes += 1;
        }
        // Cleanup.
        for y0 in (0..h).step_by(4) {
            for x in 0..w {
                let mut y = y0;
                let col_free = y0 + 4 <= h
                    && (y0..y0 + 4).all(|yy| {
                        let i = yy * w + x;
                        !g.sig[i] && !visited[i] && g.hvd(x, yy) == (0, 0, 0)
                    });
                if col_free {
                    let first = (y0..y0 + 4).find(|&yy| bit(x, yy) == 1);
                    match first {
                        None => {
                            mq.encode(17, 0);
                            continue;
                        }
                        Some(f) => {
                            mq.encode(17, 1);
                            let k = (f - y0) as u8;
                            mq.encode(18, k >> 1);
                            mq.encode(18, k & 1);
                            let (cx, xr) = g.sc(x, f);
                            mq.encode(cx, u8::from(negc(x, f)) ^ xr);
                            g.sig[f * w + x] = true;
                            g.neg[f * w + x] = negc(x, f);
                            y = f + 1;
                        }
                    }
                }
                while y < (y0 + 4).min(h) {
                    let i = y * w + x;
                    if !g.sig[i] && !visited[i] {
                        let b = bit(x, y);
                        mq.encode(g.zc(orient, x, y), b);
                        if b == 1 {
                            let (cx, xr) = g.sc(x, y);
                            mq.encode(cx, u8::from(negc(x, y)) ^ xr);
                            g.sig[i] = true;
                            g.neg[i] = negc(x, y);
                        }
                    }
                    y += 1;
                }
            }
        }
        passes += 1;
        visited.iter_mut().for_each(|v| *v = false);
    }
    Some((mb - 1 - msb, passes, mq.flush()))
}

// ---- forward 5-3 ----

/// 1D_SD on an interleaved line (analysis); `p` is the parity of the first coordinate.
pub fn analyze_line(s: &mut [i32], p: usize) {
    let n = s.len();
    if n == 1 {
        if p == 1 {
            s[0] *= 2;
        }
        return;
    }
    let nb = |s: &[i32], i: usize| {
        let l = if i > 0 { s[i - 1] } else { s[1] };
        let r = if i + 1 < n { s[i + 1] } else { s[n - 2] };
        (l, r)
    };
    let mut i = 1 - p;
    while i < n {
        let (l, r) = nb(s, i);
        s[i] -= (l + r) >> 1;
        i += 2;
    }
    let mut i = p;
    while i < n {
        let (l, r) = nb(s, i);
        s[i] += (l + r + 2) >> 2;
        i += 2;
    }
}

/// Forward transform of a `w x h` tile-component with origin (x0, y0), in place, into the same
/// layout the decoder reads. Returns the resolution sizes (width, height), lowest first.
pub fn forward(
    a: &mut [i32],
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    levels: usize,
) -> Vec<(usize, usize)> {
    let mut sizes = vec![(w, h)];
    let (mut cw, mut ch, mut ox, mut oy) = (w, h, x0, y0);
    for _ in 0..levels {
        let (px, py) = (ox & 1, oy & 1);
        // Vertical, then horizontal.
        for x in 0..cw {
            let mut col: Vec<i32> = (0..ch).map(|y| a[y * w + x]).collect();
            analyze_line(&mut col, py);
            let nl = (0..ch).filter(|y| (y + py) % 2 == 0).count();
            let (mut li, mut hi) = (0, nl);
            for (y, v) in col.iter().enumerate() {
                let k = if (y + py) % 2 == 0 {
                    li += 1;
                    li - 1
                } else {
                    hi += 1;
                    hi - 1
                };
                a[k * w + x] = *v;
            }
        }
        for y in 0..ch {
            let mut row: Vec<i32> = a[y * w..y * w + cw].to_vec();
            analyze_line(&mut row, px);
            let nl = (0..cw).filter(|x| (x + px) % 2 == 0).count();
            let (mut li, mut hi) = (0, nl);
            for (x, v) in row.iter().enumerate() {
                let k = if (x + px) % 2 == 0 {
                    li += 1;
                    li - 1
                } else {
                    hi += 1;
                    hi - 1
                };
                a[y * w + k] = *v;
            }
        }
        let nlx = (0..cw).filter(|x| (x + px) % 2 == 0).count();
        let nly = (0..ch).filter(|y| (y + py) % 2 == 0).count();
        cw = nlx;
        ch = nly;
        ox = ox.div_ceil(2);
        oy = oy.div_ceil(2);
        sizes.push((cw, ch));
    }
    sizes.reverse();
    sizes
}

// ---- tag tree and packet writer ----

pub struct TagEnc {
    /// (low, value, known) per node, levels concatenated, plus parents.
    low: Vec<u32>,
    val: Vec<u32>,
    known: Vec<bool>,
    parent: Vec<usize>,
}

impl TagEnc {
    pub fn new(w: usize, h: usize, leaves: &[u32]) -> Self {
        let mut val = leaves.to_vec();
        let mut parent = vec![usize::MAX; w * h];
        let (mut lw, mut lh, mut base) = (w, h, 0usize);
        while lw > 1 || lh > 1 {
            let (pw, ph) = (lw.div_ceil(2), lh.div_ceil(2));
            let next = base + lw * lh;
            val.resize(next + pw * ph, u32::MAX);
            parent.resize(next + pw * ph, usize::MAX);
            for y in 0..lh {
                for x in 0..lw {
                    let p = next + (y / 2) * pw + x / 2;
                    parent[base + y * lw + x] = p;
                    val[p] = val[p].min(val[base + y * lw + x]);
                }
            }
            base = next;
            lw = pw;
            lh = ph;
        }
        let n = val.len();
        TagEnc {
            low: vec![0; n],
            val,
            known: vec![false; n],
            parent,
        }
    }

    pub fn encode(&mut self, leaf: usize, threshold: u32, bw: &mut BitW) {
        let mut path = vec![leaf];
        while self.parent[*path.last().unwrap()] != usize::MAX {
            path.push(self.parent[*path.last().unwrap()]);
        }
        let mut low = 0;
        for &n in path.iter().rev() {
            if low > self.low[n] {
                self.low[n] = low;
            } else {
                low = self.low[n];
            }
            while low < threshold {
                if low >= self.val[n] {
                    if !self.known[n] {
                        bw.bit(1);
                        self.known[n] = true;
                    }
                    break;
                }
                bw.bit(0);
                low += 1;
            }
            self.low[n] = low;
        }
    }
}

/// Packet-header bit writer with stuffing after 0xFF.
pub struct BitW {
    pub bytes: Vec<u8>,
    cur: u8,
    n: u32,
    limit: u32,
}

impl BitW {
    pub fn new() -> Self {
        BitW {
            bytes: Vec::new(),
            cur: 0,
            n: 0,
            limit: 8,
        }
    }
    pub fn bit(&mut self, b: u8) {
        self.cur = (self.cur << 1) | b;
        self.n += 1;
        if self.n == self.limit {
            self.bytes.push(self.cur);
            self.limit = if self.cur == 0xFF { 7 } else { 8 };
            self.cur = 0;
            self.n = 0;
        }
    }
    pub fn bits(&mut self, v: u32, n: u32) {
        for k in (0..n).rev() {
            self.bit(((v >> k) & 1) as u8);
        }
    }
    pub fn finish(mut self) -> Vec<u8> {
        while self.n != 0 {
            self.bit(0);
        }
        if self.bytes.last() == Some(&0xFF) {
            self.bytes.push(0);
        }
        self.bytes
    }
}

fn put_passes(bw: &mut BitW, n: u32) {
    match n {
        1 => bw.bit(0),
        2 => bw.bits(0b10, 2),
        3..=5 => {
            bw.bits(0b11, 2);
            bw.bits(n - 3, 2);
        }
        6..=36 => {
            bw.bits(0b1111, 4);
            bw.bits(n - 6, 5);
        }
        _ => {
            bw.bits(0b1_1111_1111, 9);
            bw.bits(n - 37, 7);
        }
    }
}

// ---- the codestream ----

#[derive(Clone, Debug)]
pub struct Opts {
    pub w: usize,
    pub h: usize,
    pub x0: usize,
    pub y0: usize,
    pub bits: u8,
    pub signed: bool,
    pub levels: usize,
    pub xcb: u32,
    pub ycb: u32,
    pub layers: usize,
    pub guard: u32,
}

impl Opts {
    pub fn new(w: usize, h: usize, bits: u8, levels: usize) -> Self {
        Opts {
            w,
            h,
            x0: 0,
            y0: 0,
            bits,
            signed: false,
            levels,
            xcb: 6,
            ycb: 6,
            layers: 1,
            guard: 2,
        }
    }
}

/// A coded block: zero bit-planes and, per layer, (passes, bytes).
type EncBlock = (u32, Vec<(u32, Vec<u8>)>);

struct BandEnc {
    res: usize,
    nbx: usize,
    nby: usize,
    /// per block: Some((zero planes, per-layer (passes, bytes)))
    blocks: Vec<Option<EncBlock>>,
}

fn marker(out: &mut Vec<u8>, m: u16, body: &[u8]) {
    out.extend(m.to_be_bytes());
    out.extend(((body.len() + 2) as u16).to_be_bytes());
    out.extend(body);
}

/// Encodes `samples` (row-major, raw values, unsigned unless `o.signed`) losslessly.
pub fn encode(samples: &[i32], o: &Opts) -> Vec<u8> {
    assert_eq!(samples.len(), o.w * o.h);
    let shift = if o.signed { 0 } else { 1i32 << (o.bits - 1) };
    let mut a: Vec<i32> = samples.iter().map(|&s| s - shift).collect();
    let sizes = forward(&mut a, o.w, o.h, o.x0, o.y0, o.levels);
    let (cw, ch) = (1usize << o.xcb, 1usize << o.ycb);
    let mut bands: Vec<BandEnc> = Vec::new();
    for r in 0..=o.levels {
        let orients: &[u8] = if r == 0 { &[0] } else { &[1, 2, 3] };
        let nb = if r == 0 { o.levels } else { o.levels - r + 1 };
        for &or in orients {
            let (xob, yob) = (usize::from(or & 1), usize::from(or >> 1));
            let coord = |v: usize, ob: usize| -> usize {
                let half = if nb == 0 { 0 } else { 1usize << (nb - 1) };
                let n = v as i64 - (half * ob) as i64;
                (n.div_euclid(1 << nb) + i64::from(n.rem_euclid(1 << nb) != 0)).max(0) as usize
            };
            let (bx0, bx1) = (coord(o.x0, xob), coord(o.x0 + o.w, xob));
            let (by0, by1) = (coord(o.y0, yob), coord(o.y0 + o.h, yob));
            let (low_w, low_h) = if r == 0 { (0, 0) } else { sizes[r - 1] };
            let (offx, offy) = (
                if xob == 1 { low_w } else { 0 },
                if yob == 1 { low_h } else { 0 },
            );
            let (bw, bh) = (bx1 - bx0, by1 - by0);
            let exp = u32::from(o.bits) + [0, 1, 1, 2][usize::from(or)];
            let mb = o.guard + exp - 1;
            let (gx0, gy0) = (bx0 / cw, by0 / ch);
            let (nbx, nby) = if bw > 0 && bh > 0 {
                (bx1.div_ceil(cw) - gx0, by1.div_ceil(ch) - gy0)
            } else {
                (0, 0)
            };
            let mut blocks = Vec::new();
            for by in 0..nby {
                for bx in 0..nbx {
                    let x0 = ((gx0 + bx) * cw).max(bx0);
                    let x1 = ((gx0 + bx + 1) * cw).min(bx1);
                    let y0 = ((gy0 + by) * ch).max(by0);
                    let y1 = ((gy0 + by + 1) * ch).min(by1);
                    let (w, h) = (x1 - x0, y1 - y0);
                    let mut c = Vec::with_capacity(w * h);
                    for y in 0..h {
                        for x in 0..w {
                            c.push(a[(offy + y0 - by0 + y) * o.w + offx + x0 - bx0 + x]);
                        }
                    }
                    blocks.push(
                        encode_block(&c, w, h, or, mb)
                            .map(|(z, passes, bytes)| (z, split_layers(passes, &bytes, o.layers))),
                    );
                }
            }
            bands.push(BandEnc {
                res: r,
                nbx,
                nby,
                blocks,
            });
        }
    }
    // Packets, LRCP.
    let mut body = Vec::new();
    let mut incl: Vec<TagEnc> = Vec::new();
    let mut zbp: Vec<TagEnc> = Vec::new();
    let mut lblock: Vec<Vec<u32>> = Vec::new();
    let mut included: Vec<Vec<bool>> = Vec::new();
    for b in &bands {
        let first_layer: Vec<u32> = b
            .blocks
            .iter()
            .map(|x| {
                x.as_ref()
                    .and_then(|(_, l)| l.iter().position(|(p, _)| *p > 0))
                    .map_or(u32::MAX, |p| p as u32)
            })
            .collect();
        let zeros: Vec<u32> = b
            .blocks
            .iter()
            .map(|x| x.as_ref().map_or(0, |(z, _)| *z))
            .collect();
        let (nbx, nby) = (b.nbx.max(1), b.nby.max(1));
        incl.push(TagEnc::new(nbx, nby, &pad(&first_layer, nbx * nby)));
        zbp.push(TagEnc::new(nbx, nby, &pad(&zeros, nbx * nby)));
        lblock.push(vec![3; b.blocks.len()]);
        included.push(vec![false; b.blocks.len()]);
    }
    for layer in 0..o.layers {
        for (r, &(rw, rh)) in sizes.iter().enumerate() {
            if rw == 0 || rh == 0 {
                continue;
            }
            let mut bw = BitW::new();
            let mut data: Vec<u8> = Vec::new();
            let any = bands
                .iter()
                .filter(|b| b.res == r)
                .any(|b| b.blocks.iter().flatten().any(|(_, l)| l[layer].0 > 0));
            if !any {
                bw.bit(0);
                body.extend(bw.finish());
                continue;
            }
            bw.bit(1);
            for (s, b) in bands.iter().enumerate().filter(|(_, b)| b.res == r) {
                for k in 0..b.blocks.len() {
                    let contrib = b.blocks[k]
                        .as_ref()
                        .map(|(_, l)| &l[layer])
                        .filter(|c| c.0 > 0);
                    if !included[s][k] {
                        incl[s].encode(k, layer as u32 + 1, &mut bw);
                    } else {
                        bw.bit(u8::from(contrib.is_some()));
                    }
                    let Some((np, bytes)) = contrib else { continue };
                    if !included[s][k] {
                        let z = b.blocks[k].as_ref().unwrap().0;
                        for t in 1..=z + 1 {
                            zbp[s].encode(k, t, &mut bw);
                        }
                        included[s][k] = true;
                    }
                    put_passes(&mut bw, *np);
                    let need = 32 - (bytes.len() as u32).leading_zeros();
                    let lg = 31 - np.leading_zeros();
                    while lblock[s][k] + lg < need {
                        bw.bit(1);
                        lblock[s][k] += 1;
                    }
                    bw.bit(0);
                    bw.bits(bytes.len() as u32, lblock[s][k] + lg);
                    data.extend(bytes);
                }
            }
            body.extend(bw.finish());
            body.extend(data);
        }
    }
    // Markers.
    let mut cs = vec![0xFF, 0x4F];
    let mut siz = Vec::new();
    siz.extend(0u16.to_be_bytes());
    for v in [
        o.x0 + o.w,
        o.y0 + o.h,
        o.x0,
        o.y0,
        o.x0 + o.w,
        o.y0 + o.h,
        0,
        0,
    ] {
        siz.extend((v as u32).to_be_bytes());
    }
    siz.extend(1u16.to_be_bytes());
    siz.extend([(o.bits - 1) | if o.signed { 0x80 } else { 0 }, 1, 1]);
    marker(&mut cs, 0xFF51, &siz);
    let mut cod = vec![0, 0];
    cod.extend((o.layers as u16).to_be_bytes());
    cod.extend([
        0,
        o.levels as u8,
        (o.xcb - 2) as u8,
        (o.ycb - 2) as u8,
        0,
        1,
    ]);
    marker(&mut cs, 0xFF52, &cod);
    let mut qcd = vec![(o.guard as u8) << 5];
    qcd.push((o.bits) << 3);
    for _ in 0..o.levels {
        qcd.extend([(o.bits + 1) << 3, (o.bits + 1) << 3, (o.bits + 2) << 3]);
    }
    marker(&mut cs, 0xFF5C, &qcd);
    marker(&mut cs, 0xFF64, b"\x00\x01test");
    let psot = 12 + 2 + body.len();
    cs.extend([0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend((psot as u32).to_be_bytes());
    cs.extend([0, 1, 0xFF, 0x93]);
    cs.extend(body);
    cs.extend([0xFF, 0xD9]);
    cs
}

fn pad(v: &[u32], n: usize) -> Vec<u32> {
    let mut o = v.to_vec();
    o.resize(n, u32::MAX);
    o
}

/// Splits a block's passes and bytes over layers; layers past the passes get none.
fn split_layers(passes: u32, bytes: &[u8], layers: usize) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let (mut done, mut sent) = (0u32, 0usize);
    for l in 0..layers {
        let np = passes / layers as u32 + u32::from((l as u32) < passes % layers as u32);
        done += np;
        let upto = if l + 1 == layers {
            bytes.len()
        } else {
            bytes.len() * done as usize / passes as usize
        };
        out.push((np, bytes[sent..upto].to_vec()));
        sent = upto;
    }
    out
}

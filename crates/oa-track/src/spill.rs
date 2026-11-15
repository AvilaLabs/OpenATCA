//! A tracker run's output on disk, so its memory does not grow with the run. Shell (files):
//! the tracker core never sees it.
//!
//! The rows are drained from the tracker's bounded output table into one file per column
//! (little-endian, the bytes [`crate::types::output_hash_into`] hashes, column after column), so
//! the digest of the run is the same as the digest of the same rows held in memory, computed at
//! the end by reading the files back in the digest's order. Scoring reads back only the columns
//! it needs, in blocks, one address range at a time (`crate::score::RowSource`).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use oa_sha256::{to_hex, Sha256};

use crate::types::{TrackLog, TrackOutput, TrackerStats, OUTPUT_HASH_TAG, O_TICK};

/// The column files in the order the digest hashes them.
const COLS: [&str; 15] = [
    "t_ns", "addr", "lat", "lon", "ve", "vn", "turn", "alt", "altvar", "vrate", "poscov", "velcov",
    "muh", "muv", "flags",
];
const T_NS: usize = 0;
const ADDR: usize = 1;
const LAT: usize = 2;
const LON: usize = 3;
const VE: usize = 4;
const VN: usize = 5;
const ALT: usize = 7;
const FLAGS: usize = 14;
/// Rows read per block when scoring reads the files back.
const BLOCK_ROWS: usize = 1 << 13;

/// Writes drained output rows to the column files.
// A7.2: cold struct, size guard waived
pub struct SpillWriter {
    dir: PathBuf,
    files: Vec<BufWriter<File>>,
    rows: u64,
}

/// Starts a spill in the new directory `dir` (created, with its parents).
pub fn spill_new(dir: &Path) -> io::Result<SpillWriter> {
    fs::create_dir_all(dir)?;
    let mut files = Vec::with_capacity(COLS.len());
    for c in COLS {
        files.push(BufWriter::with_capacity(
            1 << 16,
            File::create(dir.join(c))?,
        ));
    }
    Ok(SpillWriter {
        dir: dir.to_path_buf(),
        files,
        rows: 0,
    })
}

/// Appends every row of `o` (the caller clears `o` afterwards).
pub fn spill_append(w: &mut SpillWriter, o: &TrackOutput) -> io::Result<()> {
    let f32s =
        |v: &Vec<f32>| -> Vec<u8> { v.iter().flat_map(|x| x.to_bits().to_le_bytes()).collect() };
    let arr = |v: &Vec<[f32; 3]>| -> Vec<u8> {
        v.iter()
            .flat_map(|c| c.iter().flat_map(|x| x.to_bits().to_le_bytes()))
            .collect()
    };
    let arr2 = |v: &Vec<[f32; 2]>| -> Vec<u8> {
        v.iter()
            .flat_map(|c| c.iter().flat_map(|x| x.to_bits().to_le_bytes()))
            .collect()
    };
    let cols: [Vec<u8>; 15] = [
        o.t_ns.iter().flat_map(|x| x.to_le_bytes()).collect(),
        o.addr.iter().flat_map(|x| x.to_le_bytes()).collect(),
        o.lat_e7.iter().flat_map(|x| x.to_le_bytes()).collect(),
        o.lon_e7.iter().flat_map(|x| x.to_le_bytes()).collect(),
        f32s(&o.vel_east_mps),
        f32s(&o.vel_north_mps),
        f32s(&o.turn_rate_rps),
        f32s(&o.alt_ft),
        f32s(&o.alt_var_ft2),
        f32s(&o.vrate_fpm),
        arr(&o.pos_cov_m2),
        arr(&o.vel_cov_m2s2),
        arr2(&o.mu_h),
        arr2(&o.mu_v),
        o.flags.clone(),
    ];
    for (f, c) in w.files.iter_mut().zip(cols.iter()) {
        f.write_all(c)?;
    }
    w.rows += o.t_ns.len() as u64;
    Ok(())
}

/// A finished run whose rows are on disk. Removes its directory when dropped. Cold record
/// (A7.2).
// A7.2: cold struct, size guard waived
pub struct SpilledRun {
    pub dir: PathBuf,
    /// Rows written.
    pub rows: u64,
    pub stats: TrackerStats,
    pub log: TrackLog,
    pub offered: u64,
    /// Digest of the rows, as [`crate::types::output_hash_into`] gives for the same rows in memory.
    pub digest: String,
    /// Rows the tracker's bounded table lost (must be 0 in a scored run).
    pub overflow: u64,
}

impl Drop for SpilledRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Flushes the files, computes the digest and returns the run.
pub fn spill_finish(
    mut w: SpillWriter,
    overflow: u64,
    stats: TrackerStats,
    log: TrackLog,
    offered: u64,
) -> io::Result<SpilledRun> {
    for f in &mut w.files {
        f.flush()?;
    }
    let mut h = Sha256::new();
    let mut put = |b: &[u8]| h.update(b).map_err(io::Error::other);
    put(OUTPUT_HASH_TAG)?;
    put(&w.rows.to_le_bytes())?;
    put(&overflow.to_le_bytes())?;
    let mut buf = vec![0u8; 1 << 16];
    for c in COLS {
        let mut f = File::open(w.dir.join(c))?;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            put(buf.get(..n).unwrap_or(&[]))?;
        }
    }
    let digest = to_hex(&h.finalize());
    Ok(SpilledRun {
        dir: w.dir.clone(),
        rows: w.rows,
        stats,
        log,
        offered,
        digest,
        overflow,
    })
}

/// Reads one column file in blocks of whole rows.
struct ColReader {
    f: BufReader<File>,
    width: usize,
    buf: Vec<u8>,
}

impl ColReader {
    fn open(dir: &Path, col: usize, width: usize) -> io::Result<ColReader> {
        let name = COLS.get(col).copied().unwrap_or("");
        Ok(ColReader {
            f: BufReader::with_capacity(1 << 16, File::open(dir.join(name))?),
            width,
            buf: vec![0u8; width * BLOCK_ROWS],
        })
    }

    /// The next block's bytes (whole rows), empty at the end.
    fn block(&mut self) -> io::Result<&[u8]> {
        let mut at = 0usize;
        while at < self.buf.len() {
            let Some(dst) = self.buf.get_mut(at..) else {
                break;
            };
            let n = self.f.read(dst)?;
            if n == 0 {
                break;
            }
            at += n;
        }
        let whole = at - at % self.width;
        Ok(self.buf.get(..whole).unwrap_or(&[]))
    }
}

fn dec<const N: usize, T>(b: &[u8], f: fn([u8; N]) -> T) -> Vec<T> {
    b.as_chunks::<N>().0.iter().copied().map(f).collect()
}

/// The columns scoring uses, for the rows of one block.
struct Block {
    t_ns: Vec<i64>,
    addr: Vec<u32>,
    lat: Vec<i32>,
    lon: Vec<i32>,
    ve: Vec<f32>,
    vn: Vec<f32>,
    alt: Vec<f32>,
    flags: Vec<u8>,
}

/// Reads the scoring columns of a spilled run block by block.
struct BlockReader {
    cols: Vec<ColReader>,
}

fn block_reader(dir: &Path) -> io::Result<BlockReader> {
    let mut cols = Vec::new();
    for (c, w) in [
        (T_NS, 8),
        (ADDR, 4),
        (LAT, 4),
        (LON, 4),
        (VE, 4),
        (VN, 4),
        (ALT, 4),
        (FLAGS, 1),
    ] {
        cols.push(ColReader::open(dir, c, w)?);
    }
    Ok(BlockReader { cols })
}

impl BlockReader {
    fn next(&mut self) -> io::Result<Option<Block>> {
        let mut raw: Vec<Vec<u8>> = Vec::with_capacity(self.cols.len());
        for c in &mut self.cols {
            raw.push(c.block()?.to_vec());
        }
        let g = |i: usize| raw.get(i).map(Vec::as_slice).unwrap_or(&[]);
        let b = Block {
            t_ns: dec(g(0), i64::from_le_bytes),
            addr: dec(g(1), u32::from_le_bytes),
            lat: dec(g(2), i32::from_le_bytes),
            lon: dec(g(3), i32::from_le_bytes),
            ve: dec(g(4), f32::from_le_bytes),
            vn: dec(g(5), f32::from_le_bytes),
            alt: dec(g(6), f32::from_le_bytes),
            flags: g(7).to_vec(),
        };
        let n = b.t_ns.len();
        if n == 0 {
            return Ok(None);
        }
        if [
            b.addr.len(),
            b.lat.len(),
            b.lon.len(),
            b.ve.len(),
            b.vn.len(),
            b.alt.len(),
            b.flags.len(),
        ]
        .iter()
        .any(|&l| l != n)
        {
            return Err(io::Error::other(
                "spill column files have different lengths",
            ));
        }
        Ok(Some(b))
    }
}

/// The rows of a spilled run in `rows` (ticks or updates) per address.
pub fn spill_counts(run: &SpilledRun, ticks: bool) -> io::Result<BTreeMap<u32, u64>> {
    let mut m = BTreeMap::new();
    let mut a = ColReader::open(&run.dir, ADDR, 4)?;
    let mut f = ColReader::open(&run.dir, FLAGS, 1)?;
    loop {
        let (ab, fb) = (a.block()?.to_vec(), f.block()?.to_vec());
        let addrs = dec(&ab, u32::from_le_bytes);
        if addrs.is_empty() {
            break;
        }
        for (ad, fl) in addrs.iter().zip(fb.iter()) {
            if (fl & O_TICK != 0) == ticks {
                *m.entry(*ad).or_insert(0u64) += 1;
            }
        }
    }
    Ok(m)
}

/// The scoring columns of the rows in `ticks`-or-updates with an address in `lo..=hi`, in time
/// order: `(addr, t_ns, lat_e7, lon_e7, alt_ft (f32 bits as stored), east, north, flags)`.
pub type SpillRows = (
    Vec<u32>,
    Vec<i64>,
    Vec<i32>,
    Vec<i32>,
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
    Vec<u8>,
);

/// Reads the rows of a spilled run in `ticks`-or-updates with an address in `lo..=hi`.
pub fn spill_rows(run: &SpilledRun, ticks: bool, lo: u32, hi: u32) -> io::Result<SpillRows> {
    let mut out: SpillRows = Default::default();
    let mut r = block_reader(&run.dir)?;
    while let Some(b) = r.next()? {
        for i in 0..b.t_ns.len() {
            let (Some(&ad), Some(&fl)) = (b.addr.get(i), b.flags.get(i)) else {
                continue;
            };
            if (fl & O_TICK != 0) != ticks || ad < lo || ad > hi {
                continue;
            }
            let at = |v: &[f32]| v.get(i).copied().unwrap_or(f32::NAN);
            out.0.push(ad);
            out.1.push(b.t_ns.get(i).copied().unwrap_or(0));
            out.2.push(b.lat.get(i).copied().unwrap_or(0));
            out.3.push(b.lon.get(i).copied().unwrap_or(0));
            out.4.push(at(&b.alt));
            out.5.push(at(&b.ve));
            out.6.push(at(&b.vn));
            out.7.push(fl);
        }
    }
    Ok(out)
}

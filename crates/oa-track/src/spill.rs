//! A tracker run's output on disk, so its memory does not grow with the run. Shell (files):
//! the tracker core never sees it.
//!
//! The rows are drained from the tracker's bounded output table into one file per column
//! (little-endian, the bytes [`crate::types::output_hash_into`] hashes, column after column), so
//! the digest of the run is the same as the digest of the same rows held in memory, computed at
//! the end by reading the files back in the digest's order. Scoring reads back only the columns
//! it needs, in blocks, one address range at a time (`oa-track-tools::score::RowSource`).
//!
//! # What it touches
//!
//! A spill makes its own directory: [`spill_new`] refuses one that exists and has anything in
//! it, and creates each column file new (never over an existing file). Dropping the finished run
//! removes the column files it made, by name, and then the directory only if that left it empty;
//! a start that fails partway removes only the files that call made; it never removes a directory tree, so a path handed in wrongly costs nothing but its own
//! files. Every read checks every column's size against the row count first, so a truncated or
//! extended column is a typed error ([`SpillMismatch`], inside the `io::Error`), not rows that
//! quietly disagree.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use oa_sha256::{to_hex, Sha256};

use crate::types::{TrackLog, TrackOutput, TrackerStats, OUTPUT_HASH_TAG, O_HAS_ALT, O_TICK};

/// The column files in the order the digest hashes them.
pub(crate) const COLS: [&str; 15] = [
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
/// Bytes per row of each column file, in [`COLS`] order.
pub(crate) const WIDTHS: [u64; 15] = [8, 4, 4, 4, 4, 4, 4, 4, 4, 4, 12, 12, 8, 8, 1];
/// Rows read per block when scoring reads the files back.
const BLOCK_ROWS: usize = 1 << 13;

/// Writes drained output rows to the column files.
// A7.2: cold struct, size guard waived
pub struct SpillWriter {
    dir: PathBuf,
    files: Vec<BufWriter<File>>,
    rows: u64,
}

/// A column file whose size is not the row count times the column's width. Plain data (A1);
/// carried inside an `io::Error` of kind `InvalidData`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillMismatch {
    pub column: &'static str,
    pub found_bytes: u64,
    pub expected_bytes: u64,
}

impl std::fmt::Display for SpillMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "spill column {} has {} bytes, the row count says {}",
            self.column, self.found_bytes, self.expected_bytes
        )
    }
}

impl std::error::Error for SpillMismatch {}

/// Checks every column file of `dir` against `rows` rows.
pub(crate) fn check_columns(dir: &Path, rows: u64) -> io::Result<()> {
    for (c, w) in COLS.iter().zip(WIDTHS.iter()) {
        // seal-allow: the tool's own spill directory (its output), not input data
        let found = fs::metadata(dir.join(c))?.len();
        let expected = rows * w;
        if found != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                SpillMismatch {
                    column: c,
                    found_bytes: found,
                    expected_bytes: expected,
                },
            ));
        }
    }
    Ok(())
}

/// Removes the column files `dir` holds (only those) and then `dir` if that left it empty.
fn remove_columns(dir: &Path) {
    for c in COLS {
        let _ = fs::remove_file(dir.join(c));
    }
    let _ = fs::remove_dir(dir);
}

/// Starts a spill in `dir`: created (with its parents) if absent; refused if it exists and has
/// anything in it. The column files are created new.
pub fn spill_new(dir: &Path) -> io::Result<SpillWriter> {
    // seal-allow: the tracker's own spill directory (a temp dir it created)
    if dir.exists() && fs::read_dir(dir)?.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("spill directory {} is not empty", dir.display()),
        ));
    }
    spill_create(dir)
}

/// Creates the column files in `dir` without looking at what else is there: [`spill_new`] after
/// its check, and the point where two callers that both passed the check race. A failure removes
/// only what this call made. Public for tests of that race.
#[doc(hidden)]
pub fn spill_create(dir: &Path) -> io::Result<SpillWriter> {
    let made_dir = !dir.exists();
    fs::create_dir_all(dir)?;
    let mut files = Vec::with_capacity(COLS.len());
    let mut made: Vec<PathBuf> = Vec::with_capacity(COLS.len());
    for c in COLS {
        let path = dir.join(c);
        // seal-allow: creates the run's own spill file (create_new); reads nothing
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => {
                made.push(path);
                files.push(BufWriter::with_capacity(1 << 16, f));
            }
            Err(e) => {
                // Only the files this call made (a failed create_new made nothing), and the
                // directory only if this call made it and it is now empty.
                drop(files);
                for p in &made {
                    let _ = fs::remove_file(p);
                }
                if made_dir {
                    let _ = fs::remove_dir(dir);
                }
                return Err(e);
            }
        }
    }
    Ok(SpillWriter {
        dir: dir.to_path_buf(),
        files,
        rows: 0,
    })
}

/// Appends every row of `o` (the caller clears `o` afterwards).
pub fn spill_append(w: &mut SpillWriter, o: &TrackOutput) -> io::Result<()> {
    let f32s = |v: &Vec<f32>| -> Vec<u8> {
        v.iter()
            .flat_map(|x| oa_math::canon_nan_f32(*x).to_bits().to_le_bytes())
            .collect()
    };
    let arr = |v: &Vec<[f32; 3]>| -> Vec<u8> {
        v.iter()
            .flat_map(|c| {
                c.iter()
                    .flat_map(|x| oa_math::canon_nan_f32(*x).to_bits().to_le_bytes())
            })
            .collect()
    };
    let arr2 = |v: &Vec<[f32; 2]>| -> Vec<u8> {
        v.iter()
            .flat_map(|c| {
                c.iter()
                    .flat_map(|x| oa_math::canon_nan_f32(*x).to_bits().to_le_bytes())
            })
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

/// A finished run whose rows are on disk. Removes its column files (and its directory, if that
/// leaves it empty) when dropped. Cold record (A7.2).
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
        remove_columns(&self.dir);
    }
}

/// Reads into `buf` until it is full or the file ends; the bytes read.
fn fill(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut at = 0usize;
    while at < buf.len() {
        let Some(dst) = buf.get_mut(at..) else { break };
        let n = f.read(dst)?;
        if n == 0 {
            break;
        }
        at += n;
    }
    Ok(at)
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
    check_columns(&w.dir, w.rows)?;
    let mut h = Sha256::new();
    let mut put = |b: &[u8]| h.update(b).map_err(io::Error::other);
    put(OUTPUT_HASH_TAG)?;
    put(&w.rows.to_le_bytes())?;
    put(&overflow.to_le_bytes())?;
    let mut buf = vec![0u8; 1 << 16];
    for (ci, c) in COLS.iter().enumerate() {
        // seal-allow: the tracker's own spill file in its temp dir
        let mut f = File::open(w.dir.join(c))?;
        if (ALT..ALT + 3).contains(&ci) {
            // The altitude columns hash as zero for a row with no altitude, as in memory.
            // seal-allow: the tracker's own spill file in its temp dir
            let mut flags = File::open(w.dir.join(COLS.get(FLAGS).copied().unwrap_or("flags")))?;
            let mut fbuf = vec![0u8; buf.len() / 4];
            loop {
                let n = fill(&mut f, &mut buf)?;
                if n == 0 {
                    break;
                }
                let rows = n / 4;
                let got = fill(&mut flags, fbuf.get_mut(..rows).unwrap_or(&mut []))?;
                if got != rows {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "short flags"));
                }
                for (v, fl) in buf
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .take(rows)
                    .zip(fbuf.iter())
                {
                    if fl & O_HAS_ALT == 0 {
                        *v = [0; 4];
                    }
                }
                put(buf.get(..rows * 4).unwrap_or(&[]))?;
            }
            continue;
        }
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
pub(crate) struct ColReader {
    f: BufReader<File>,
    width: usize,
    buf: Vec<u8>,
}

impl ColReader {
    pub(crate) fn open(dir: &Path, col: usize, width: usize) -> io::Result<ColReader> {
        let name = COLS.get(col).copied().unwrap_or("");
        Ok(ColReader {
            // seal-allow: the tracker's own spill file in its temp dir
            f: BufReader::with_capacity(1 << 16, File::open(dir.join(name))?),
            width,
            buf: vec![0u8; width * BLOCK_ROWS],
        })
    }

    /// The next block's bytes (whole rows), empty at the end.
    pub(crate) fn block(&mut self) -> io::Result<&[u8]> {
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
    check_columns(&run.dir, run.rows)?;
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
    check_columns(&run.dir, run.rows)?;
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

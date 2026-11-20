//! Shell (B1): `oa-compass slice`, a time window of the `.ast` files of a finished export. Each
//! file is read block by block with our decoder (CAT021 as Ed 2.4, CAT062 Ed 1.21, by the
//! category octet of the file), the records whose time of day is in `[from, to)` (I070 for
//! CAT062, I073 for CAT021) are re-encoded into blocks, every block is decoded again and
//! compared, and the written file is read back from disk like the main export's files. The
//! window is one UTC day: it cannot cross midnight (E5), because the times are times of day.
//!
//! All files are written as `.partial` and renamed together only after every file has been
//! verified; a refusal at any point removes them all. The output directory must be new or empty
//! and not the input directory.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use oa_asterix::{decode_block_with, encode_block_with, Block, Cat021Edition, SourceId};

use super::adsb_export::{self, EDITION as EDITION_021, RECORDS_PER_BLOCK};
use super::dirs::{check_out, create_tracked, remove_created};
use super::error::{io_err, ExportError};
use super::export;
use super::guard::refuse_holdout;
use super::notes::clock_text;
use crate::core::time::UNITS_PER_DAY;

/// What one file of the slice holds. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceFile {
    pub name: String,
    pub category: u8,
    pub records_in: u64,
    pub records_kept: u64,
    pub bytes: u64,
    pub blocks: u64,
}

/// `HH:MM:SS` as a count of 1/128 s; `24:00:00` is accepted (the end of the day).
pub fn parse_clock(s: &str) -> Option<u32> {
    let mut it = s.split(':');
    let (h, m, sec) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some()
        || [h, m, sec]
            .iter()
            .any(|p| p.len() != 2 || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let (h, m, sec): (i64, i64, i64) = (h.parse().ok()?, m.parse().ok()?, sec.parse().ok()?);
    if m > 59 || sec > 59 || h > 24 {
        return None;
    }
    let units = (h * 3600 + m * 60 + sec) * 128;
    (units <= UNITS_PER_DAY).then_some(u32::try_from(units).ok()?)
}

/// Checks the window: `from` before `to`. A window with `to` at or before `from` would cross
/// midnight (or be empty) and is refused.
pub fn check_window(from: u32, to: u32) -> Result<(), ExportError> {
    if to <= from {
        return Err(ExportError::Input(format!(
            "the window {} to {} is empty or crosses midnight; times of day cannot be sliced across midnight (slice each UTC day on its own)",
            clock_text(from),
            clock_text(to)
        )));
    }
    Ok(())
}

/// Reads the next data block (as bytes) of `r` into `buf`; `false` at the end of the file.
fn next_block(r: &mut impl Read, buf: &mut Vec<u8>, path: &Path) -> Result<bool, ExportError> {
    let mut first = [0u8; 1];
    match r.read(&mut first) {
        Ok(0) => return Ok(false),
        Ok(_) => {}
        Err(e) => return Err(io_err(path, &e)),
    }
    let mut rest = [0u8; 2];
    r.read_exact(&mut rest).map_err(|e| io_err(path, &e))?;
    let len = usize::from(u16::from_be_bytes(rest));
    if len < 3 {
        return Err(ExportError::Verify(format!(
            "{}: LEN {len}",
            path.display()
        )));
    }
    buf.clear();
    buf.extend_from_slice(&first);
    buf.extend_from_slice(&rest);
    buf.resize(len, 0);
    r.read_exact(buf.get_mut(3..).unwrap_or(&mut []))
        .map_err(|e| io_err(path, &e))?;
    Ok(true)
}

/// The edition a file's records are read and written as, by category.
fn edition_of(cat: u8) -> Result<Cat021Edition, ExportError> {
    match cat {
        21 => Ok(EDITION_021),
        62 => Ok(Cat021Edition::Ed27), // unused by CAT062; the CAT062 layout has one edition here
        c => Err(ExportError::Input(format!(
            "category {c}: only CAT062 Ed 1.21 and CAT021 Ed 2.4 files can be sliced"
        ))),
    }
}

/// The time of day of a record, `None` if it has none, and the source.
fn record_times(b: &Block) -> Vec<(Option<u32>, SourceId)> {
    match b {
        Block::Cat021(r) => r.iter().map(|x| (x.tomr_position, x.source)).collect(),
        Block::Cat062(r) => r.iter().map(|x| (x.time, x.source)).collect(),
        Block::Cat048(r) => r.iter().map(|x| (None, x.source)).collect(),
    }
}

/// Keeps the records of `b` whose index is in `keep`.
fn pick(b: Block, keep: &[bool]) -> Block {
    fn sel<T>(v: Vec<T>, keep: &[bool]) -> Vec<T> {
        v.into_iter()
            .zip(keep.iter())
            .filter_map(|(x, &k)| k.then_some(x))
            .collect()
    }
    match b {
        Block::Cat021(r) => Block::Cat021(sel(r, keep)),
        Block::Cat062(r) => Block::Cat062(sel(r, keep)),
        Block::Cat048(r) => Block::Cat048(sel(r, keep)),
    }
}

fn len_of(b: &Block) -> usize {
    match b {
        Block::Cat021(r) => r.len(),
        Block::Cat062(r) => r.len(),
        Block::Cat048(r) => r.len(),
    }
}

/// Slices one file into `out_path`; returns what it holds, or `None` for the records kept if the
/// window holds none (no file is written then).
fn slice_file(
    input: &Path,
    out_path: &Path,
    from: u32,
    to: u32,
) -> Result<(SliceFile, Option<PathBuf>), ExportError> {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    refuse_holdout(input)?;
    let f = File::open(input).map_err(|e| io_err(input, &e))?;
    let mut r = BufReader::with_capacity(1 << 20, f);
    let partial = out_path.with_extension("ast.partial");
    let mut w = BufWriter::with_capacity(
        1 << 20,
        File::create(&partial).map_err(|e| io_err(&partial, &e))?,
    );
    let mut buf = Vec::new();
    let mut pending: Vec<Block> = Vec::new();
    let mut pending_n = 0usize;
    let mut info = SliceFile {
        name: name.clone(),
        category: 0,
        records_in: 0,
        records_kept: 0,
        bytes: 0,
        blocks: 0,
    };
    let mut source = None;
    let mut cat = 0u8;
    let mut late = 0u64;
    let result: Result<(), ExportError> = (|| {
        while next_block(&mut r, &mut buf, input)? {
            let this = buf.first().copied().unwrap_or(0);
            if cat != 0 && this != cat {
                return Err(ExportError::Input(format!(
                    "{name}: a file with blocks of category {cat} and {this}"
                )));
            }
            cat = this;
            let ed = edition_of(cat)?;
            let (block, used) = decode_block_with(&buf, ed)
                .map_err(|e| ExportError::Verify(format!("{}: {e:?}", input.display())))?;
            if used != buf.len() {
                return Err(ExportError::Verify(format!(
                    "{}: block length",
                    input.display()
                )));
            }
            let times = record_times(&block);
            let mut keep = Vec::with_capacity(times.len());
            for (t, src) in &times {
                let Some(t) = t else {
                    return Err(ExportError::Verify(format!(
                        "{name}: a record without its time item"
                    )));
                };
                info.records_in = info.records_in.saturating_add(1);
                if i64::from(*t) >= UNITS_PER_DAY {
                    late = late.saturating_add(1);
                }
                let k = (from..to).contains(t);
                keep.push(k);
                if k {
                    source.get_or_insert(*src);
                }
            }
            let kept = pick(block, &keep);
            let n = len_of(&kept);
            if n == 0 {
                continue;
            }
            pending_n += n;
            pending.push(kept);
            if pending_n >= RECORDS_PER_BLOCK {
                flush(&mut pending, &mut pending_n, cat, &mut w, &mut info)?;
            }
        }
        flush(&mut pending, &mut pending_n, cat, &mut w, &mut info)?;
        if late > 0 {
            return Err(ExportError::Verify(format!(
                "{name}: {late} records with a time of day of 24:00:00 or later (a time of day is below 86400 s)"
            )));
        }
        w.flush().map_err(|e| io_err(&partial, &e))
    })();
    drop(w);
    if let Err(e) = result {
        let _ = fs::remove_file(&partial);
        return Err(e);
    }
    info.category = cat;
    if info.records_kept == 0 {
        let _ = fs::remove_file(&partial);
        return Ok((info, None));
    }
    let src = source.unwrap_or_default();
    let read = if cat == 21 {
        adsb_export::read_back(&partial, src, info.records_kept, info.bytes)
    } else {
        export::read_back(&partial, src, info.records_kept, info.bytes)
    };
    match read {
        Ok((n, b)) if n == info.records_kept && b == info.blocks => {}
        Ok(_) => {
            let _ = fs::remove_file(&partial);
            return Err(ExportError::Verify(format!(
                "{name}: read-back block count differs"
            )));
        }
        Err(e) => {
            let _ = fs::remove_file(&partial);
            return Err(e);
        }
    }
    Ok((info, Some(partial)))
}

/// Merges the pending kept records into blocks of at most [`RECORDS_PER_BLOCK`], encodes each,
/// decodes it again and compares, and writes it.
fn flush(
    pending: &mut Vec<Block>,
    pending_n: &mut usize,
    cat: u8,
    w: &mut impl Write,
    info: &mut SliceFile,
) -> Result<(), ExportError> {
    if pending.is_empty() {
        return Ok(());
    }
    let ed = edition_of(cat)?;
    let (mut r21, mut r62) = (Vec::new(), Vec::new());
    for b in pending.drain(..) {
        match b {
            Block::Cat021(r) => r21.extend(r),
            Block::Cat062(r) => r62.extend(r),
            Block::Cat048(_) => {}
        }
    }
    *pending_n = 0;
    let all: Vec<Block> = if cat == 21 {
        r21.chunks(RECORDS_PER_BLOCK)
            .map(|c| Block::Cat021(c.to_vec()))
            .collect()
    } else {
        r62.chunks(RECORDS_PER_BLOCK)
            .map(|c| Block::Cat062(c.to_vec()))
            .collect()
    };
    for block in all {
        let bytes = encode_block_with(&block, ed)?;
        let (back, used) = decode_block_with(&bytes, ed)?;
        if used != bytes.len() || back != block {
            return Err(ExportError::Verify(format!(
                "{}: block {} does not decode to the records it was encoded from",
                info.name, info.blocks
            )));
        }
        w.write_all(&bytes)
            .map_err(|e| ExportError::Io(format!("{}: write: {e}", info.name)))?;
        info.records_kept = info
            .records_kept
            .saturating_add(u64::try_from(len_of(&block)).unwrap_or(u64::MAX));
        info.blocks = info.blocks.saturating_add(1);
        info.bytes = info
            .bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    }
    Ok(())
}

/// A point in [`slice_dir_with`] at which a test can make the run fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold enum, size guard waived
pub enum Stage {
    /// Every file is sliced and verified, nothing is renamed yet.
    Verified,
    /// This many files (1-based, the README and the data-source file included) are renamed.
    Renamed(usize),
}

/// Slices every `.ast` file of `input` into `out`, copies `data_sources.json` and writes the
/// README. Returns what each file holds.
pub fn slice_dir(
    input: &Path,
    out: &Path,
    from: u32,
    to: u32,
) -> Result<Vec<SliceFile>, ExportError> {
    slice_dir_with(input, out, from, to, &mut |_| Ok(()))
}

/// [`slice_dir`] with a hook called at each [`Stage`]; an error from the hook fails the run as
/// any other error does. On any error every file this run created in `out` (partial or already
/// renamed) is removed.
pub fn slice_dir_with(
    input: &Path,
    out: &Path,
    from: u32,
    to: u32,
    hook: &mut dyn FnMut(Stage) -> Result<(), ExportError>,
) -> Result<Vec<SliceFile>, ExportError> {
    check_window(from, to)?;
    // Before the directory is listed: a sealed source is never read, not even its names.
    refuse_holdout(input)?;
    let mut names: Vec<PathBuf> = fs::read_dir(input)
        .map_err(|e| io_err(input, &e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ast"))
        .collect();
    names.sort();
    if names.is_empty() {
        return Err(ExportError::Input(format!(
            "{}: no .ast files",
            input.display()
        )));
    }
    // Every file of the source is checked on its own as well: a link inside an unsealed
    // directory can lead into a sealed one.
    for n in &names {
        refuse_holdout(n)?;
    }
    // Not the input directory (by canonical path), and new or empty: nothing of an earlier run is
    // mixed in; a directory of only partial files says so.
    if out.exists() {
        let (cin, cout) = (
            fs::canonicalize(input).map_err(|e| io_err(input, &e))?,
            fs::canonicalize(out).map_err(|e| io_err(out, &e))?,
        );
        if cin == cout {
            return Err(ExportError::Input("--out is the --in directory".into()));
        }
    }
    // The source README is needed for the import date line: read before anything is created.
    let src_readme = fs::read_to_string(input.join("README.txt"))
        .map_err(|e| io_err(&input.join("README.txt"), &e))?;
    refuse_holdout(out)?;
    check_out(out)?;
    let mut run_dirs: Vec<PathBuf> = Vec::new();
    create_tracked(out, &mut run_dirs)?;
    // (partial, final) of every file of this run.
    let mut pending: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut renamed: Vec<PathBuf> = Vec::new();
    let result = (|| {
        let mut files = Vec::new();
        for p in &names {
            let dest = out.join(p.file_name().unwrap_or_default());
            let partial = dest.with_extension("ast.partial");
            // The partial exists once slice_file runs; list it first so a failure removes it.
            pending.push((partial, dest.clone()));
            let (info, kept) = slice_file(p, &dest, from, to)?;
            if kept.is_none() {
                pending.pop();
            }
            files.push(info);
        }
        if files.iter().all(|f| f.records_kept == 0) {
            let counts: Vec<String> = files
                .iter()
                .map(|f| format!("{}: {} in, 0 kept", f.name, f.records_in))
                .collect();
            return Err(ExportError::Input(format!(
                "no record of any file is in the window {} to {}: {}",
                clock_text(from),
                clock_text(to),
                counts.join("; ")
            )));
        }
        let ds = input.join("data_sources.json");
        if ds.exists() {
            let dest = out.join("data_sources.json");
            let partial = out.join("data_sources.json.partial");
            pending.push((partial.clone(), dest));
            fs::copy(&ds, &partial).map_err(|e| io_err(&ds, &e))?;
        }
        let readme = slice_readme(input, from, to, &files, &src_readme);
        let (rp, rf) = (out.join("README.txt.partial"), out.join("README.txt"));
        pending.push((rp.clone(), rf));
        fs::write(&rp, readme).map_err(|e| io_err(&rp, &e))?;
        hook(Stage::Verified)?;
        // One pass of renames.
        for (k, (from_p, to_p)) in pending.iter().enumerate() {
            fs::rename(from_p, to_p).map_err(|e| io_err(to_p, &e))?;
            renamed.push(to_p.clone());
            hook(Stage::Renamed(k + 1))?;
        }
        Ok(files)
    })();
    if result.is_err() {
        for (partial, _) in &pending {
            let _ = fs::remove_file(partial);
        }
        for f in &renamed {
            let _ = fs::remove_file(f);
        }
        remove_created(&run_dirs);
    }
    result
}

/// The README of a slice.
pub fn slice_readme(
    input: &Path,
    from: u32,
    to: u32,
    files: &[SliceFile],
    src_readme: &str,
) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "A time slice of an oa-compass export, for OpenATS COMPASS\n"
    );
    let _ = writeln!(
        o,
        "Window: {} to {} UTC time of day, from and including, to and excluding (I070 for CAT062 files, I073 for CAT021 files; 1/128 s).",
        clock_text(from),
        clock_text(to)
    );
    let _ = writeln!(o, "Source directory: {}", input.display());
    if let Some(l) = src_readme
        .lines()
        .find(|l| l.starts_with("UTC date to give at import"))
    {
        let _ = writeln!(o, "{l} (as in the source README)");
    }
    let _ = writeln!(
        o,
        "The window is inside one UTC day; a window across midnight is refused."
    );
    let _ = writeln!(
        o,
        "Method: every file of the source directory was read block by block with this repository's decoder (CAT062 Ed 1.21, CAT021 Ed 2.4), the records inside the window were kept unchanged and written again into data blocks of up to {RECORDS_PER_BLOCK} records, every block was decoded again and equals what was encoded, and every file was read back from disk and equals what was written. No value of any record is changed; the data sources are those of data_sources.json, copied as it is. The source README describes the formats, the items and the omissions; its counts are for the whole day, not for this window."
    );
    let _ = writeln!(
        o,
        "\nFiles (records in the source file, records kept, blocks, octets):"
    );
    for f in files {
        if f.records_kept == 0 {
            let _ = writeln!(
                o,
                "  {}: {} in, 0 kept; no file written (no record in the window)",
                f.name, f.records_in
            );
        } else {
            let _ = writeln!(
                o,
                "  {}: {} in, {} kept, {} blocks, {} octets (CAT{:03})",
                f.name, f.records_in, f.records_kept, f.blocks, f.bytes, f.category
            );
        }
    }
    o
}

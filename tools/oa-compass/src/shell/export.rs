//! Shell (B1): the output files of one system. Rows arrive already converted; this module gives
//! each track its 16-bit track number, routes the row to the file of its UTC day, writes the
//! blocks, and at the end reads every file back and compares it with what was written.
//!
//! A file holds one UTC day because I062/070 is a time of day (E5). In the default layout every
//! row must be on the requested day and the first row that is not refuses the export; with
//! `split` the files of each day go to their own directory `<out>/<YYYY.MM.DD>/` with the same
//! file names. Files are written as `<name>.partial` and renamed only when the whole export
//! has succeeded.
//!
//! Track numbers: one per key (an aircraft address for OpenATCA, a flight for ERAM), counted up
//! from 1 in order of first appearance in the file, stable for the whole file. More than 65 535
//! distinct keys in a file is a refusal, not a wrap (E8).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use oa_asterix::{decode_block, Block, SourceId};

use super::encode::{BlockSink, SinkStats};
use super::error::{io_err, ExportError};
use crate::core::convert::{count_notes, Converted, Refusal, NOTES};
use crate::core::stats::{summarize, IntervalSummary};
use crate::core::time::civil_from_days;

/// Where and how files are laid out. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Layout {
    pub out: PathBuf,
    /// Days since 1970-01-01 of the requested UTC day.
    pub day: i64,
    /// One directory per day instead of refusing rows of other days.
    pub split: bool,
}

/// `YYYY.MM.DD` of a day count.
pub fn date_text(day: i64) -> String {
    match civil_from_days(day) {
        Some((y, m, d)) => format!("{y:04}.{m:02}.{d:02}"),
        None => format!("day {day}"),
    }
}

impl Layout {
    /// The directory of `day`'s files.
    pub fn day_dir(&self, day: i64) -> PathBuf {
        if self.split {
            self.out.join(date_text(day))
        } else {
            self.out.clone()
        }
    }
}

/// What a system's export of one day holds. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct DayReport {
    pub day: i64,
    pub file: String,
    pub sink: SinkStats,
    pub tracks: u64,
    pub notes: [u64; NOTES],
    /// Time between consecutive rows of one track, over all tracks of the file.
    pub intervals: IntervalSummary,
    /// Records and blocks counted again by reading the file back.
    pub read_back_records: u64,
    pub read_back_blocks: u64,
}

/// What a system's export holds in all. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct SystemReport {
    pub name: &'static str,
    pub source: SourceId,
    pub days: Vec<DayReport>,
    /// Rows refused, by [`Refusal`] index.
    pub refused: [u64; 2],
    /// Other counts of the run, in order: label and value.
    pub counts: Vec<(String, u64)>,
    /// Descriptions of the inputs (set by the caller), one line each.
    pub inputs: Vec<String>,
    /// Further lines for the README (set by the caller): what the run found that the counts do
    /// not say, one line each.
    pub details: Vec<String>,
}

struct DayFile {
    sink: BlockSink<BufWriter<File>>,
    partial: PathBuf,
    path: PathBuf,
    /// Key to (track number, time of its last row).
    tracks: BTreeMap<u32, (u16, u32)>,
    notes: [u64; NOTES],
    /// Histogram of the interval between consecutive rows of one track, 1/128 s to count.
    intervals: BTreeMap<u32, u64>,
}

/// The writer of one system's files.
// A7.2: cold struct, size guard waived
pub struct SystemExport {
    name: &'static str,
    file: &'static str,
    source: SourceId,
    layout: Layout,
    files: BTreeMap<i64, DayFile>,
    refused: [u64; 2],
    pub counts: Vec<(String, u64)>,
    /// Added to the latitude of every source position (a verification self-test, 0 normally).
    source_offset_e7: i32,
}

/// The renames that make a finished export visible.
// A7.2: cold struct, size guard waived
pub struct Finished {
    pub report: SystemReport,
    renames: Vec<(PathBuf, PathBuf)>,
}

impl Finished {
    /// The (partial, final) names of the files, for a caller that renames them with other files.
    pub fn renames(&self) -> &[(PathBuf, PathBuf)] {
        &self.renames
    }

    /// Moves the partial files to their final names.
    pub fn commit(self) -> Result<SystemReport, ExportError> {
        for (from, to) in &self.renames {
            fs::rename(from, to).map_err(|e| io_err(to, &e))?;
        }
        Ok(self.report)
    }

    /// Removes the partial files.
    pub fn abort(self) {
        for (from, _) in &self.renames {
            let _ = fs::remove_file(from);
        }
    }
}

impl SystemExport {
    pub fn new(
        name: &'static str,
        file: &'static str,
        source: SourceId,
        layout: Layout,
    ) -> SystemExport {
        SystemExport {
            name,
            file,
            source,
            layout,
            files: BTreeMap::new(),
            refused: [0; 2],
            counts: Vec::new(),
            source_offset_e7: 0,
        }
    }

    /// Sets the verification self-test offset (see `Job::self_test_position_offset_e7`).
    pub fn set_source_offset(&mut self, e7: i32) {
        self.source_offset_e7 = e7;
    }

    /// Counts a row that was not converted.
    pub fn refuse(&mut self, r: Refusal) {
        if let Some(c) = self.refused.get_mut(r as usize) {
            *c = c.saturating_add(1);
        }
    }

    /// Adds a converted row of track `key` (the position it came from is `src_e7`).
    pub fn push(&mut self, c: &Converted, key: u32, src_e7: (i32, i32)) -> Result<(), ExportError> {
        if !self.layout.split && c.day != self.layout.day {
            return Err(ExportError::CrossesMidnight {
                system: self.name,
                wanted: date_text(self.layout.day),
                found: date_text(c.day),
            });
        }
        if !self.files.contains_key(&c.day) {
            let dir = self.layout.day_dir(c.day);
            fs::create_dir_all(&dir).map_err(|e| io_err(&dir, &e))?;
            let path = dir.join(self.file);
            let partial = dir.join(format!("{}.partial", self.file));
            let f = File::create(&partial).map_err(|e| io_err(&partial, &e))?;
            self.files.insert(
                c.day,
                DayFile {
                    sink: BlockSink::new(
                        BufWriter::with_capacity(1 << 20, f),
                        self.source,
                        self.name,
                    ),
                    partial,
                    path,
                    tracks: BTreeMap::new(),
                    notes: [0; NOTES],
                    intervals: BTreeMap::new(),
                },
            );
        }
        let Some(file) = self.files.get_mut(&c.day) else {
            return Err(ExportError::Io(format!("{}: day file missing", self.name)));
        };
        let time = c.values.time;
        let number = match file.tracks.get_mut(&key) {
            Some((n, last)) => {
                // Rows are in time order (the sink refuses otherwise), so this does not wrap.
                let gap = time.saturating_sub(*last);
                *last = time;
                let slot = file.intervals.entry(gap).or_insert(0);
                *slot = slot.saturating_add(1);
                *n
            }
            None => {
                let next = u16::try_from(file.tracks.len() + 1).map_err(|_| {
                    ExportError::TrackNumbers {
                        system: self.name,
                        day: date_text(c.day),
                    }
                })?;
                file.tracks.insert(key, (next, time));
                next
            }
        };
        let mut v = c.values;
        v.track_number = number;
        file.sink.push(
            &v,
            (src_e7.0.saturating_add(self.source_offset_e7), src_e7.1),
        )?;
        count_notes(&mut file.notes, c.notes);
        Ok(())
    }

    /// Rows written so far, over all days.
    pub fn rows(&self) -> u64 {
        self.files.values().map(|f| f.sink.stats.records).sum()
    }

    /// Removes the partial files after a failure.
    pub fn abort(self) {
        for f in self.files.into_values() {
            drop(f.sink);
            let _ = fs::remove_file(&f.partial);
        }
    }

    /// Writes the last blocks, reads every file back and returns what is to be committed.
    pub fn finish(self) -> Result<Finished, ExportError> {
        let mut days = Vec::new();
        let mut renames = Vec::new();
        let mut failure = None;
        for (day, f) in self.files {
            let (partial, path, tracks, notes) = (f.partial, f.path, f.tracks, f.notes);
            let hist: Vec<(u32, u64)> = f.intervals.into_iter().collect();
            renames.push((partial.clone(), path.clone()));
            if failure.is_some() {
                continue;
            }
            match f.sink.finish() {
                Ok((w, sink)) => {
                    drop(w);
                    match read_back(&partial, self.source, sink.records, sink.bytes) {
                        Ok((records, blocks)) => days.push(DayReport {
                            day,
                            file: path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            sink,
                            tracks: tracks.len() as u64,
                            notes,
                            intervals: summarize(&hist),
                            read_back_records: records,
                            read_back_blocks: blocks,
                        }),
                        Err(e) => failure = Some(e),
                    }
                }
                Err(e) => failure = Some(e),
            }
        }
        if let Some(e) = failure {
            for (from, _) in &renames {
                let _ = fs::remove_file(from);
            }
            return Err(e);
        }
        Ok(Finished {
            report: SystemReport {
                name: self.name,
                source: self.source,
                days,
                refused: self.refused,
                counts: self.counts,
                inputs: Vec::new(),
                details: Vec::new(),
            },
            renames,
        })
    }
}

/// Reads a finished file from disk, block by block with the `oa-asterix` decoder, and checks that
/// it holds exactly `records` records in `bytes` octets, all of category 62 with the data source
/// `source`, in non-decreasing time. Returns the records and blocks read.
pub fn read_back(
    path: &Path,
    source: SourceId,
    records: u64,
    bytes: u64,
) -> Result<(u64, u64), ExportError> {
    let bad = |s: String| ExportError::Verify(format!("{}: {s}", path.display()));
    let f = File::open(path).map_err(|e| io_err(path, &e))?;
    let mut r = BufReader::with_capacity(1 << 20, f);
    let (mut n_rec, mut n_blk, mut n_bytes, mut last) = (0u64, 0u64, 0u64, 0u32);
    let mut buf = Vec::new();
    let mut body = Vec::new();
    loop {
        let mut first = [0u8; 1];
        match r.read(&mut first) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return Err(io_err(path, &e)),
        }
        let mut rest = [0u8; 2];
        r.read_exact(&mut rest).map_err(|e| io_err(path, &e))?;
        let len = usize::from(u16::from_be_bytes(rest));
        if len < 3 {
            return Err(bad(format!("block {n_blk}: LEN {len}")));
        }
        body.clear();
        body.resize(len - 3, 0);
        r.read_exact(&mut body).map_err(|e| io_err(path, &e))?;
        buf.clear();
        buf.extend_from_slice(&first);
        buf.extend_from_slice(&rest);
        buf.extend_from_slice(&body);
        let (block, used) = decode_block(&buf).map_err(|e| bad(format!("block {n_blk}: {e:?}")))?;
        if used != len {
            return Err(bad(format!(
                "block {n_blk}: {used} octets decoded of {len}"
            )));
        }
        let Block::Cat062(recs) = block else {
            return Err(bad(format!("block {n_blk}: not category 62")));
        };
        for rec in &recs {
            if rec.source != source {
                return Err(bad(format!("block {n_blk}: wrong data source")));
            }
            let t = rec.time.ok_or_else(|| bad("record without I070".into()))?;
            if t < last {
                return Err(bad(format!("block {n_blk}: time goes back")));
            }
            last = t;
        }
        n_rec += recs.len() as u64;
        n_blk += 1;
        n_bytes += len as u64;
    }
    if n_rec != records || n_bytes != bytes {
        return Err(bad(format!(
            "read back {n_rec} records in {n_bytes} octets, wrote {records} in {bytes}"
        )));
    }
    Ok((n_rec, n_blk))
}

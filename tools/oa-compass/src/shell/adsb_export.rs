//! Shell (B1): the CAT021 Ed 2.4 file of one day, `adsb.ast`: the ADS-B reports
//! offered to the tracker's adapter (a superset of those it used), one record per report, in time order. Same discipline as the CAT062 files:
//! every block is decoded again before it is written and must equal the records encoded, files
//! are written as `.partial` and renamed when everything has succeeded, and the committed files
//! are read back from disk. The values come from [`crate::core::adsb`]; the layout of a record is
//! the typed [`Cat021`] of `oa-asterix`, written as Ed 2.4 ([`Cat021Edition::Ed24`]).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use oa_asterix::cat021::{Cat021, FlagS15, GroundVector, Mops, Quality021, Trd021};
use oa_asterix::{
    decode_block_with, encode_block_with, raw_to_e7, Block, Cat021Edition, RawPosition, SourceId,
};

use super::error::{io_err, ExportError};
use super::export::{date_text, Layout};
use super::guard::refuse_sealed_row;
use crate::core::adsb::{count_adsb_notes, AdsbConverted, AdsbRefusal, AdsbValues, ADSB_NOTES};

/// Edition written and read back.
pub const EDITION: Cat021Edition = Cat021Edition::Ed24;
/// Records per data block (a record is at most 54 octets, so a block is far under the LEN limit).
pub const RECORDS_PER_BLOCK: usize = 256;
/// Largest difference between the report's position and the decoded one, 1e-7 degree: half an
/// LSB of 180/2^30 degrees (0.84) plus half a unit of the source (0.5), which in whole units of
/// the source is 1.
pub const MAX_POSITION_ERROR_E7: i32 = 1;
const SYSTEM: &str = "ADS-B";
const FILE: &str = "adsb.ast";

/// The typed record for `v`.
pub fn cat021_record(v: &AdsbValues, source: SourceId) -> Cat021 {
    // I040 first extension: GBS (bit 7), CL 2 = no information (bits 3/2), FX 0.
    let trd_ext = (u8::from(v.ground) << 6) | (2 << 1);
    Cat021 {
        source,
        descriptor: Some(Trd021 {
            atp: v.atp,
            arc: 2,
            range_check: false,
            field_monitor: false,
            ext: vec![trd_ext],
        }),
        position_hr: Some(RawPosition {
            lat: v.lat,
            lon: v.lon,
        }),
        address: Some(v.address),
        tomr_position: Some(v.time),
        geometric_height: v.geometric_height,
        quality: v.quality.map(|q| {
            // First extension: NICBARO (bit 8), SIL (bits 7/6), NACp (bits 5/2); second: SILS
            // (bit 6), SDA (bits 5/4), GVA (bits 3/2); third: PIC (bits 8/5). The core gives an
            // extension only when it and those before it are fully known. One whose bits are all 0
            // is sent only if a later one is (5.2.16 encoding rule).
            let mut ext: Vec<u8> = Vec::new();
            if let Some(e) = q.ext1 {
                ext.push((e.nicbaro << 7) | (e.sil << 5) | (e.nacp << 1));
                if let Some(e) = q.ext2 {
                    ext.push((u8::from(e.sils) << 5) | (e.sda << 3) | (e.gva << 1));
                    if let Some(pic) = q.pic {
                        ext.push(pic << 4);
                    }
                }
            }
            while ext.last() == Some(&0) {
                ext.pop();
            }
            let n = ext.len();
            for (i, o) in ext.iter_mut().enumerate() {
                if i + 1 < n {
                    *o |= 1;
                }
            }
            Quality021 {
                nacv: q.nacv,
                nic: q.nic,
                ext,
            }
        }),
        mops: v.mops_version.map(|version| Mops {
            not_supported: false,
            version,
            link: v.link,
        }),
        flight_level: v.flight_level,
        baro_rate: v.baro_rate.map(|raw| FlagS15 {
            range_exceeded: false,
            raw,
        }),
        geometric_rate: v.geometric_rate.map(|raw| FlagS15 {
            range_exceeded: false,
            raw,
        }),
        ground_vector: v.ground_vector.map(|(speed, track)| GroundVector {
            range_exceeded: false,
            speed,
            track,
        }),
        target_id: v.ident,
        emitter_category: v.category,
        ..Cat021::default()
    }
}

/// What a file holds. A7.2: cold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct AdsbSinkStats {
    pub records: u64,
    pub blocks: u64,
    pub bytes: u64,
    /// Largest position difference between report and decoded record, 1e-7 degree.
    pub max_lat_error_e7: i32,
    pub max_lon_error_e7: i32,
    pub first_time: u32,
    pub last_time: u32,
}

struct DayFile {
    out: BufWriter<File>,
    partial: PathBuf,
    path: PathBuf,
    pending: Vec<Cat021>,
    src_e7: Vec<(i32, i32)>,
    stats: AdsbSinkStats,
    notes: [u64; ADSB_NOTES],
}

/// One day of the ADS-B file. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct AdsbDayReport {
    pub day: i64,
    pub file: String,
    pub stats: AdsbSinkStats,
    pub notes: [u64; ADSB_NOTES],
    pub read_back_records: u64,
    pub read_back_blocks: u64,
}

/// The whole ADS-B export. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct AdsbReport {
    pub source: SourceId,
    pub days: Vec<AdsbDayReport>,
    /// Reports not converted, by [`AdsbRefusal`] index.
    pub refused: [u64; 2],
    /// Other counts, label and value.
    pub counts: Vec<(String, u64)>,
}

/// The writer of the ADS-B file(s).
// A7.2: cold struct, size guard waived
pub struct AdsbExport {
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
pub struct AdsbFinished {
    pub report: AdsbReport,
    renames: Vec<(PathBuf, PathBuf)>,
}

impl AdsbFinished {
    /// The (partial, final) names of the files, for a caller that renames them with other files.
    pub fn renames(&self) -> &[(PathBuf, PathBuf)] {
        &self.renames
    }

    /// Moves the partial files to their final names.
    pub fn commit(self) -> Result<AdsbReport, ExportError> {
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

impl AdsbExport {
    pub fn new(source: SourceId, layout: Layout) -> AdsbExport {
        AdsbExport {
            source,
            layout,
            files: BTreeMap::new(),
            refused: [0; 2],
            counts: Vec::new(),
            source_offset_e7: 0,
        }
    }

    /// Sets the verification self-test offset (see `Job::self_test_adsb_offset_e7`).
    pub fn set_source_offset(&mut self, e7: i32) {
        self.source_offset_e7 = e7;
    }

    /// Counts a report that was not converted.
    pub fn refuse(&mut self, r: AdsbRefusal) {
        if let Some(c) = self.refused.get_mut(r as usize) {
            *c = c.saturating_add(1);
        }
    }

    /// Adds a converted report whose source position was `src_e7`. Refuses one earlier than the
    /// previous report of its day, and one on another day than the requested one unless the
    /// layout splits days.
    pub fn push(&mut self, c: &AdsbConverted, src_e7: (i32, i32)) -> Result<(), ExportError> {
        refuse_sealed_row(SYSTEM, c.day)?;
        if !self.layout.split && c.day != self.layout.day {
            return Err(ExportError::CrossesMidnight {
                system: SYSTEM,
                wanted: date_text(self.layout.day),
                found: date_text(c.day),
            });
        }
        if !self.files.contains_key(&c.day) {
            let dir = self.layout.day_dir(c.day);
            fs::create_dir_all(&dir).map_err(|e| io_err(&dir, &e))?;
            let path = dir.join(FILE);
            let partial = dir.join(format!("{FILE}.partial"));
            let f = File::create(&partial).map_err(|e| io_err(&partial, &e))?;
            self.files.insert(
                c.day,
                DayFile {
                    out: BufWriter::with_capacity(1 << 20, f),
                    partial,
                    path,
                    pending: Vec::with_capacity(RECORDS_PER_BLOCK),
                    src_e7: Vec::with_capacity(RECORDS_PER_BLOCK),
                    stats: AdsbSinkStats::default(),
                    notes: [0; ADSB_NOTES],
                },
            );
        }
        let Some(file) = self.files.get_mut(&c.day) else {
            return Err(ExportError::Io(format!("{SYSTEM}: day file missing")));
        };
        let t = c.values.time;
        if file.stats.records > 0 && t < file.stats.last_time {
            return Err(ExportError::OutOfOrder {
                system: SYSTEM,
                prev: file.stats.last_time,
                now: t,
            });
        }
        if file.stats.records == 0 {
            file.stats.first_time = t;
        }
        file.stats.last_time = t;
        file.stats.records = file.stats.records.saturating_add(1);
        file.pending.push(cat021_record(&c.values, self.source));
        file.src_e7
            .push((src_e7.0.saturating_add(self.source_offset_e7), src_e7.1));
        count_adsb_notes(&mut file.notes, c.notes);
        if file.pending.len() >= RECORDS_PER_BLOCK {
            flush(file)?;
        }
        Ok(())
    }

    /// Removes the partial files after a failure.
    pub fn abort(self) {
        for f in self.files.into_values() {
            drop(f.out);
            let _ = fs::remove_file(&f.partial);
        }
    }

    /// Writes the last blocks, reads every file back and returns what is to be committed.
    pub fn finish(self) -> Result<AdsbFinished, ExportError> {
        let mut days = Vec::new();
        let mut renames = Vec::new();
        let mut failure = None;
        for (day, mut f) in self.files {
            renames.push((f.partial.clone(), f.path.clone()));
            if failure.is_some() {
                continue;
            }
            let done = flush(&mut f).and_then(|()| {
                f.out
                    .flush()
                    .map_err(|e| ExportError::Io(format!("{SYSTEM}: flush: {e}")))
            });
            if let Err(e) = done {
                failure = Some(e);
                continue;
            }
            let (partial, path, stats, notes) = (f.partial, f.path, f.stats, f.notes);
            drop(f.out);
            match read_back(&partial, self.source, stats.records, stats.bytes) {
                Ok((records, blocks)) => days.push(AdsbDayReport {
                    day,
                    file: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    stats,
                    notes,
                    read_back_records: records,
                    read_back_blocks: blocks,
                }),
                Err(e) => failure = Some(e),
            }
        }
        if let Some(e) = failure {
            for (from, _) in &renames {
                let _ = fs::remove_file(from);
            }
            return Err(e);
        }
        Ok(AdsbFinished {
            report: AdsbReport {
                source: self.source,
                days,
                refused: self.refused,
                counts: self.counts,
            },
            renames,
        })
    }
}

/// Encodes, decodes and compares the pending block, then writes it.
fn flush(f: &mut DayFile) -> Result<(), ExportError> {
    if f.pending.is_empty() {
        return Ok(());
    }
    let block = Block::Cat021(std::mem::take(&mut f.pending));
    let src = std::mem::take(&mut f.src_e7);
    let bytes = encode_block_with(&block, EDITION)?;
    let (back, used) = decode_block_with(&bytes, EDITION)?;
    if used != bytes.len() || back != block {
        return Err(ExportError::Verify(format!(
            "{SYSTEM}: block {} does not decode to the records it was encoded from",
            f.stats.blocks
        )));
    }
    if let Block::Cat021(recs) = &back {
        for (r, s) in recs.iter().zip(src.iter()) {
            let p = r.position_hr.ok_or_else(|| {
                ExportError::Verify(format!("{SYSTEM}: decoded record without position"))
            })?;
            let (la, lo) = (
                raw_to_e7(p.lat, 30).unwrap_or(i32::MAX),
                raw_to_e7(p.lon, 30).unwrap_or(i32::MAX),
            );
            let dla = i64::from(la).abs_diff(i64::from(s.0));
            // A longitude of +180 is written as -180: compare around the circle.
            let dlo = i64::from(lo).abs_diff(i64::from(s.1));
            let dlo = dlo.min(3_600_000_000 - dlo.min(3_600_000_000));
            f.stats.max_lat_error_e7 = f
                .stats
                .max_lat_error_e7
                .max(i32::try_from(dla).unwrap_or(i32::MAX));
            f.stats.max_lon_error_e7 = f
                .stats
                .max_lon_error_e7
                .max(i32::try_from(dlo).unwrap_or(i32::MAX));
            if dla > MAX_POSITION_ERROR_E7.unsigned_abs().into()
                || dlo > MAX_POSITION_ERROR_E7.unsigned_abs().into()
            {
                return Err(ExportError::Verify(format!(
                    "{SYSTEM}: a decoded position differs from its source by {dla} / {dlo} (1e-7 degree, latitude / longitude), over the limit {MAX_POSITION_ERROR_E7}; nothing is committed"
                )));
            }
        }
    }
    f.out
        .write_all(&bytes)
        .map_err(|e| ExportError::Io(format!("{SYSTEM}: write: {e}")))?;
    f.stats.blocks = f.stats.blocks.saturating_add(1);
    f.stats.bytes = f.stats.bytes.saturating_add(bytes.len() as u64);
    f.pending = Vec::with_capacity(RECORDS_PER_BLOCK);
    f.src_e7 = Vec::with_capacity(RECORDS_PER_BLOCK);
    Ok(())
}

/// Reads a finished file from disk, block by block with the Ed 2.4 decoder, and checks that it
/// holds exactly `records` records in `bytes` octets, all of category 21 with the data source
/// `source`, in non-decreasing I073. Returns the records and blocks read.
pub fn read_back(
    path: &Path,
    source: SourceId,
    records: u64,
    bytes: u64,
) -> Result<(u64, u64), ExportError> {
    let bad = |s: String| ExportError::Verify(format!("{}: {s}", path.display()));
    let f = super::guard::open(path).map_err(|e| io_err(path, &e))?;
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
        let (block, used) =
            decode_block_with(&buf, EDITION).map_err(|e| bad(format!("block {n_blk}: {e:?}")))?;
        if used != len {
            return Err(bad(format!(
                "block {n_blk}: {used} octets decoded of {len}"
            )));
        }
        let Block::Cat021(recs) = block else {
            return Err(bad(format!("block {n_blk}: not category 21")));
        };
        for rec in &recs {
            if rec.source != source {
                return Err(bad(format!("block {n_blk}: wrong data source")));
            }
            let t = rec
                .tomr_position
                .ok_or_else(|| bad("record without I073".into()))?;
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

//! Shell (B1): record values into CAT062 records and blocks, written through any `Write`. Every
//! block is decoded again with the `oa-asterix` decoder before it is written, and the decoded
//! records must equal the ones encoded; a difference stops the export (E5).
//!
//! Layout of a record (UAP of 5.2 / Table of CAT062 Ed 1.21), in FRN order: I010 source, I015
//! service identification (OpenATCA only), I070 time, I105 position, I185 velocity, I380
//! aircraft derived data (address, identification, as far as known), I040 track number, I080
//! track status, I136 measured flight level (ERAM) or I135 barometric altitude (OpenATCA), I220
//! rate of climb/descent (OpenATCA), I500 accuracies (OpenATCA, APW only).
//!
//! I080 is one octet when nothing is set; AFF adds the first extent (bit 4), and a coasting track
//! adds three extents (the first, an empty second, and the third with CST, bit 8). The primary
//! octet has SIM, SPI and MRH at their default 0 and CNF 0 (confirmed): the tracker only
//! publishes confirmed tracks (its `O_IDENTITY_UNCONFIRMED` flag is not a tentative state and is
//! not written as CNF 1), and for ERAM the feed carries no such state (an assumption, noted). SRC is 0 (no
//! source) because I130 is never sent. MON is 1 for OpenATCA, whose tracker is fed ADS-B only
//! (one kind of sensor), and 0 for ERAM, where it is an assumption (the feed does not say; counted
//! as a note on every ERAM row). CNF is 0 for ERAM for the same reason. MRH is 0 (barometric more
//! reliable) as a default: no geometric altitude is ever sent.

use std::io::Write;

use oa_asterix::cat062::{AircraftDerived, BaroAltitude, Cat062, TrackStatus062, Velocity};
use oa_asterix::{decode_block, encode_block, raw_to_e7, Block, RawItem, RawPosition, SourceId};

use super::error::ExportError;
use crate::core::convert::RecordValues;

/// Records per data block. A record is at most 49 octets (all items present, see the bounds
/// test), so a block is at most `3 + 256 * 49 = 12 547` octets, under a fifth of the 65 535 a LEN holds.
pub const RECORDS_PER_BLOCK: usize = 256;

/// FRN of I015 and of I500 in the CAT062 UAP (the decoder keeps them as raw items).
const FRN_I015: u8 = 3;
const FRN_I500: u8 = 27;
/// I500 primary subfield with only subfield 3 (APW) present, no extension (5.2.26).
const I500_PRIMARY_APW: u8 = 0x20;

/// Largest difference between a source position and the position decoded from the record, in
/// 1e-7 degree: half an LSB of 180/2^25 degrees (26.8) and half a unit of the source (0.5).
pub const MAX_POSITION_ERROR_E7: i32 = 27;

/// The I080 octets after the primary one, for the flags of a record.
fn status_extents(v: &RecordValues) -> Vec<u8> {
    let aff = if v.aff { 0x08 } else { 0 };
    if v.coasting {
        // First extent (AFF), second extent (empty), third extent (CST, no FX).
        vec![aff | 0x01, 0x01, 0x80]
    } else if v.aff {
        vec![aff]
    } else {
        Vec::new()
    }
}

/// The typed record for `v`. Items the values do not have are absent from the record.
pub fn cat062_record(v: &RecordValues, source: SourceId) -> Cat062 {
    let mut raw = Vec::new();
    if let Some(s) = v.service {
        raw.push(RawItem {
            frn: FRN_I015,
            bytes: vec![s],
        });
    }
    if let Some((la, lo)) = v.accuracy {
        let (la, lo) = (la.to_be_bytes(), lo.to_be_bytes());
        raw.push(RawItem {
            frn: FRN_I500,
            bytes: vec![I500_PRIMARY_APW, la[0], la[1], lo[0], lo[1]],
        });
    }
    let derived = if v.address.is_some() || v.ident.is_some() {
        Some(AircraftDerived {
            address: v.address,
            ident: v.ident,
            ..AircraftDerived::default()
        })
    } else {
        None
    };
    Cat062 {
        source,
        time: Some(v.time),
        position: Some(RawPosition {
            lat: v.lat,
            lon: v.lon,
        }),
        velocity: v.velocity.map(|(vx, vy)| Velocity { vx, vy }),
        derived,
        track_number: Some(v.track_number),
        track_status: Some(TrackStatus062 {
            monosensor: v.monosensor,
            ext: status_extents(v),
            ..TrackStatus062::default()
        }),
        measured_fl: v.measured_fl,
        baro_altitude: v.baro_alt.map(|raw| BaroAltitude {
            qnh: v.baro_qnh,
            raw,
        }),
        climb_rate: v.climb,
        raw,
        ..Cat062::default()
    }
}

/// What a sink has written. A7.2: cold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct SinkStats {
    pub records: u64,
    pub blocks: u64,
    pub bytes: u64,
    /// Largest position difference between the source row and the decoded record, 1e-7 degree.
    pub max_lat_error_e7: i32,
    pub max_lon_error_e7: i32,
    /// First and last I070 written, 1/128 s.
    pub first_time: u32,
    pub last_time: u32,
}

/// Writes records as data blocks of [`RECORDS_PER_BLOCK`], in the order given.
// A7.2: cold struct, size guard waived
pub struct BlockSink<W: Write> {
    out: W,
    source: SourceId,
    system: &'static str,
    pending: Vec<Cat062>,
    src_e7: Vec<(i32, i32)>,
    pub stats: SinkStats,
}

impl<W: Write> BlockSink<W> {
    pub fn new(out: W, source: SourceId, system: &'static str) -> BlockSink<W> {
        BlockSink {
            out,
            source,
            system,
            pending: Vec::with_capacity(RECORDS_PER_BLOCK),
            src_e7: Vec::with_capacity(RECORDS_PER_BLOCK),
            stats: SinkStats::default(),
        }
    }

    /// Adds a record whose source position was `src_e7` (lat, lon, 1e-7 degree). Refuses a
    /// record earlier than the previous one.
    pub fn push(&mut self, v: &RecordValues, src_e7: (i32, i32)) -> Result<(), ExportError> {
        if self.stats.records > 0 && v.time < self.stats.last_time {
            return Err(ExportError::OutOfOrder {
                system: self.system,
                prev: self.stats.last_time,
                now: v.time,
            });
        }
        if self.stats.records == 0 {
            self.stats.first_time = v.time;
        }
        self.stats.last_time = v.time;
        self.stats.records = self.stats.records.saturating_add(1);
        self.pending.push(cat062_record(v, self.source));
        self.src_e7.push(src_e7);
        if self.pending.len() >= RECORDS_PER_BLOCK {
            self.flush()?;
        }
        Ok(())
    }

    /// Encodes, decodes and compares the pending block, then writes it.
    fn flush(&mut self) -> Result<(), ExportError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let block = Block::Cat062(std::mem::take(&mut self.pending));
        let src = std::mem::take(&mut self.src_e7);
        let bytes = encode_block(&block)?;
        let (back, used) = decode_block(&bytes)?;
        if used != bytes.len() || back != block {
            return Err(ExportError::Verify(format!(
                "{}: block {} does not decode to the records it was encoded from",
                self.system, self.stats.blocks
            )));
        }
        if let Block::Cat062(recs) = &back {
            for (r, s) in recs.iter().zip(src.iter()) {
                let p = r.position.ok_or_else(|| {
                    ExportError::Verify(format!("{}: decoded record without position", self.system))
                })?;
                let (la, lo) = (
                    raw_to_e7(p.lat, 25).unwrap_or(i32::MAX),
                    raw_to_e7(p.lon, 25).unwrap_or(i32::MAX),
                );
                let dla = i64::from(la).abs_diff(i64::from(s.0));
                // A longitude of +180 is written as -180: compare around the circle.
                let dlo = i64::from(lo).abs_diff(i64::from(s.1));
                let dlo = dlo.min(3_600_000_000 - dlo.min(3_600_000_000));
                self.stats.max_lat_error_e7 = self
                    .stats
                    .max_lat_error_e7
                    .max(i32::try_from(dla).unwrap_or(i32::MAX));
                self.stats.max_lon_error_e7 = self
                    .stats
                    .max_lon_error_e7
                    .max(i32::try_from(dlo).unwrap_or(i32::MAX));
                if dla > MAX_POSITION_ERROR_E7.unsigned_abs().into()
                    || dlo > MAX_POSITION_ERROR_E7.unsigned_abs().into()
                {
                    return Err(ExportError::Verify(format!(
                        "{}: a decoded position differs from its source by {dla} / {dlo} (1e-7 degree, latitude / longitude), over the limit {MAX_POSITION_ERROR_E7}; nothing is committed",
                        self.system
                    )));
                }
            }
        }
        self.out
            .write_all(&bytes)
            .map_err(|e| ExportError::Io(format!("{}: write: {e}", self.system)))?;
        self.stats.blocks = self.stats.blocks.saturating_add(1);
        self.stats.bytes = self.stats.bytes.saturating_add(bytes.len() as u64);
        // Reuse the allocations' capacity for the next block.
        self.pending = Vec::with_capacity(RECORDS_PER_BLOCK);
        self.src_e7 = Vec::with_capacity(RECORDS_PER_BLOCK);
        Ok(())
    }

    /// Writes the last block and flushes; returns the writer and the totals.
    pub fn finish(mut self) -> Result<(W, SinkStats), ExportError> {
        self.flush()?;
        self.out
            .flush()
            .map_err(|e| ExportError::Io(format!("{}: flush: {e}", self.system)))?;
        Ok((self.out, self.stats))
    }
}

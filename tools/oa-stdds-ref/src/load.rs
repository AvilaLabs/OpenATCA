//! Reading STDDS recordings (`.oaf`, raw or gzip) into [`Rec`]s.

use std::io::{self, Read};
use std::path::Path;

use oa_feed::frame::{Kind as FrameKind, MAX_FRAME_LEN};
use oa_feed::reader::FrameReader;
use oa_feed::replay::REPLAY_LIMITS;
use oa_sealed::Sealed;
use oa_sha256::{to_hex, Sha256};
use oa_stdds::tais::tais_bit;
use oa_stdds::{
    new_stats, parse_message, Limits, Message, Stats, TaisField, TaisMessage, TaisRecord,
};
use oa_xml::{Limits as XmlLimits, Parser};
use oa_zip::GzipReader;

use crate::guard::open_guarded;
use crate::model::{classify_record, Rec};

/// Hard research envelope for STDDS recordings (bytes on disk, before inflation).
pub const MAX_STDDS_FILES: usize = 32;
pub const MAX_STDDS_FILE_BYTES: u64 = 128 * 1024 * 1024;
/// Combined raw STDDS input bytes across all files; ADS-B has its own 128 MiB cap.
pub const MAX_TOTAL_STDDS_BYTES: u64 = 384 * 1024 * 1024;
/// Hard decoded-frame-byte envelope for one STDDS recording.
pub const MAX_STDDS_DECODED_FILE_BYTES: u64 = 128 * 1024 * 1024;
/// Combined decoded-frame-byte envelope for all STDDS files.
pub const MAX_TOTAL_STDDS_DECODED_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum retained STDDS records across all files and the report.
pub const MAX_STDDS_RECORDS: usize = 100_000;

/// What was read from one recording.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub path: String,
    /// SHA-256 of the file's bytes as stored (gzip included), hex.
    pub sha256: String,
    pub frames_message: u64,
    pub frames_other: u64,
    pub frames_damaged: u64,
    /// Message frames of other STDDS products (not TAIS) or that the parser refused whole.
    pub messages_not_tais: u64,
    pub gzip_damaged: bool,
    pub gzip_truncated: bool,
    pub records: u64,
    /// Bytes consumed from the source file, gzip included.
    pub source_bytes: u64,
    /// Bytes decoded into OAF frames.
    pub decoded_bytes: u64,
    /// True if the global record cap was exceeded (no excess rows were retained).
    pub record_limit_exceeded: bool,
    /// Message frames received in a sealed window and TAIS records timed (`mrtTime`) in one,
    /// dropped and counted (OA-SEAL-014).
    pub sealed_window_dropped: u64,
}

/// A STDDS record in the form the scoring uses.
pub fn rec_of(file: u32, index: u32, msg: &TaisMessage, r: &TaisRecord) -> Rec {
    let has = |f: TaisField| r.present & tais_bit(f) != 0;
    let pos = (has(TaisField::Lat) && has(TaisField::Lon))
        .then(|| (f64::from(r.lat_e7) * 1e-7, f64::from(r.lon_e7) * 1e-7));
    let vel = (has(TaisField::Vx) && has(TaisField::Vy))
        .then(|| (f64::from(r.vx_cmps) * 0.01, f64::from(r.vy_cmps) * 0.01));
    Rec {
        file,
        index,
        class: classify_record(r, msg),
        addr: has(TaisField::AcAddress).then_some(r.mode_s_addr),
        src: if msg.src_present { msg.src } else { [0; 8] },
        src_present: msg.src_present,
        track_num: has(TaisField::TrackNum).then_some(r.track_num),
        t_ms: has(TaisField::MrtTime).then_some(r.mrt_time_ms),
        pos,
        alt_ft: has(TaisField::ReportedAltitude).then(|| f64::from(r.reported_alt_ft)),
        vel,
    }
}

struct Ctx<'a> {
    parser: Parser,
    stats: Stats,
    limits: Limits,
    file: u32,
    out: &'a mut Vec<Rec>,
    info: FileInfo,
    /// Decides the damaged frames of the file (see [`oa_sealed::DamagedGate`]).
    gate: oa_sealed::DamagedGate,
    record_limit_exceeded: bool,
}

fn drain(reader: &mut FrameReader<'_>, ctx: &mut Ctx<'_>) {
    while let Some(ev) = reader.next() {
        // A damaged frame has no receive time: the good frames either side of it decide whether
        // it is damage or a drop.
        match &ev {
            Ok(f) => {
                let (kept, gone) = ctx.gate.good(f.recv_unix_ns);
                ctx.info.frames_damaged += kept;
                ctx.info.sealed_window_dropped += gone;
            }
            Err(_) => ctx.gate.damaged(),
        }
        match ev {
            // A frame of any kind (Status and Reject included) received in a sealed window is
            // dropped and counted, never parsed and never counted by kind.
            Ok(f) if oa_sealed::in_sealed_window(f.recv_unix_ns) => {
                ctx.info.sealed_window_dropped += 1;
                oa_sealed::note_window_drops(1);
            }
            Ok(f) if f.kind == FrameKind::Message => {
                let before = ctx.stats.sealed_window_dropped;
                let parsed = parse_message(&mut ctx.parser, f.payload, &ctx.limits, &mut ctx.stats);
                // A message with a record timed in a sealed window was dropped whole by the
                // parser (`Limits::drop_window`), before any count: it is in no count here but
                // the drop.
                if ctx.stats.sealed_window_dropped > before {
                    continue;
                }
                ctx.info.frames_message += 1;
                match parsed {
                    Ok(Message::Tais(m)) => {
                        if ctx.out.len().saturating_add(m.records.len()) > MAX_STDDS_RECORDS {
                            ctx.record_limit_exceeded = true;
                            continue;
                        }
                        for r in &m.records {
                            let index = u32::try_from(ctx.info.records).unwrap_or(u32::MAX);
                            ctx.out.push(rec_of(ctx.file, index, &m, r));
                            ctx.info.records += 1;
                        }
                    }
                    _ => ctx.info.messages_not_tais += 1,
                }
            }
            Ok(_) => ctx.info.frames_other += 1,
            Err(_) => {}
        }
    }
}

fn feed(reader: &mut FrameReader<'_>, ctx: &mut Ctx<'_>, data: &[u8]) {
    let mut off = 0usize;
    while let Some(rest) = data.get(off..).filter(|r| !r.is_empty()) {
        let took = reader.push(rest);
        off += took;
        drain(reader, ctx);
        if took == 0 {
            break;
        }
    }
}

/// The decoder limits of every load in this tool: the sealed window as the drop window, so a
/// record timed in it is dropped before any count (OA-SEAL-014).
pub fn load_limits() -> Limits {
    Limits::new(oa_sealed::sealed_drop_window())
}

/// Reads a recording from `src` (raw or gzip, decided by the gzip magic), appending its TAIS
/// records to `out`. `path` is only the label of the result.
pub fn load_reader<R: Read>(
    src: R,
    path: &str,
    file: u32,
    out: &mut Vec<Rec>,
) -> io::Result<FileInfo> {
    load_reader_counted(src, path, file, out, &mut 0)
}

/// [`load_reader`], with the frames and records dropped in a sealed window put in `sealed` on
/// every exit, a read error included, so a caller can print the count whatever the result.
pub fn load_reader_counted<R: Read>(
    src: R,
    path: &str,
    file: u32,
    out: &mut Vec<Rec>,
    sealed: &mut u64,
) -> io::Result<FileInfo> {
    load_reader_counted_with_decoded_cap(
        src,
        path,
        file,
        out,
        sealed,
        MAX_STDDS_FILE_BYTES,
        MAX_STDDS_DECODED_FILE_BYTES,
    )
}

fn load_reader_counted_with_decoded_cap<R: Read>(
    mut src: R,
    path: &str,
    file: u32,
    out: &mut Vec<Rec>,
    sealed: &mut u64,
    source_cap: u64,
    decoded_cap: u64,
) -> io::Result<FileInfo> {
    let parser =
        Parser::new(XmlLimits::default()).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut ctx = Ctx {
        parser,
        stats: new_stats(),
        limits: load_limits(),
        file,
        out,
        info: FileInfo {
            path: path.to_string(),
            sha256: String::new(),
            frames_message: 0,
            frames_other: 0,
            frames_damaged: 0,
            sealed_window_dropped: 0,
            messages_not_tais: 0,
            gzip_damaged: false,
            gzip_truncated: false,
            records: 0,
            source_bytes: 0,
            decoded_bytes: 0,
            record_limit_exceeded: false,
        },
        gate: oa_sealed::DamagedGate::new(),
        record_limit_exceeded: false,
    };
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut reader =
        FrameReader::new(&mut storage).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut hash = Sha256::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut gz: Option<Box<GzipReader>> = None;
    let mut source_bytes = 0u64;
    let mut decoded_bytes = 0u64;
    let mut decoded_limit_exceeded = false;
    let read = (|| -> io::Result<()> {
        let mut n = src.read(&mut chunk)?;
        gz = (n >= 2 && chunk.first() == Some(&0x1f) && chunk.get(1) == Some(&0x8b))
            .then(|| Box::new(GzipReader::new(REPLAY_LIMITS)));
        while n > 0 {
            source_bytes = source_bytes
                .checked_add(n as u64)
                .ok_or_else(|| io::Error::other("STDDS source byte count overflow"))?;
            if source_bytes > source_cap {
                return Err(io::Error::other(format!(
                    "STDDS file exceeds the {source_cap}-byte remaining source limit"
                )));
            }
            let Some(data) = chunk.get(..n) else { break };
            hash.update(data)
                .map_err(|e| io::Error::other(format!("{e:?}")))?;
            match gz.as_mut() {
                None => {
                    decoded_bytes = decoded_bytes
                        .checked_add(data.len() as u64)
                        .ok_or_else(|| io::Error::other("STDDS decoded byte count overflow"))?;
                    if decoded_bytes > decoded_cap {
                        decoded_limit_exceeded = true;
                        return Err(io::Error::other(
                            "STDDS decoded file exceeds its byte limit",
                        ));
                    }
                    feed(&mut reader, &mut ctx, data)
                }
                Some(g) if !ctx.info.gzip_damaged => {
                    let r = g.push(data, &mut |b: &[u8]| -> Result<(), ()> {
                        decoded_bytes = match decoded_bytes.checked_add(b.len() as u64) {
                            Some(n) => n,
                            None => {
                                decoded_limit_exceeded = true;
                                return Err(());
                            }
                        };
                        if decoded_bytes > decoded_cap {
                            decoded_limit_exceeded = true;
                            return Err(());
                        }
                        feed(&mut reader, &mut ctx, b);
                        Ok(())
                    });
                    ctx.info.gzip_damaged = r.is_err();
                }
                Some(_) => {}
            }
            n = src.read(&mut chunk)?;
        }
        Ok(())
    })();
    if read.is_err() {
        ctx.info.sealed_window_dropped += ctx.gate.finish();
    }
    *sealed = ctx.info.sealed_window_dropped + ctx.stats.sealed_window_dropped;
    read?;
    if decoded_limit_exceeded {
        return Err(io::Error::other(format!(
            "STDDS decoded file exceeds the {}-byte limit",
            decoded_cap
        )));
    }
    drain(&mut reader, &mut ctx);
    if reader.finish().is_some() {
        ctx.gate.damaged();
    }
    ctx.info.sealed_window_dropped += ctx.gate.finish();
    *sealed = ctx.info.sealed_window_dropped + ctx.stats.sealed_window_dropped;
    ctx.info.gzip_truncated = gz.as_ref().is_some_and(|g| g.finish().truncated.is_some());
    ctx.info.source_bytes = source_bytes;
    ctx.info.decoded_bytes = decoded_bytes;
    ctx.info.record_limit_exceeded = ctx.record_limit_exceeded;
    if ctx.record_limit_exceeded {
        return Err(io::Error::other(format!(
            "STDDS records exceed the {}-record limit",
            MAX_STDDS_RECORDS
        )));
    }
    ctx.info.sha256 = to_hex(&hash.finalize());
    // Frames by receive time and records by their own time, in one count.
    ctx.info.sealed_window_dropped += ctx.stats.sealed_window_dropped;
    Ok(ctx.info)
}

/// [`load_reader`] on the file at `path`, opened through the shared guard `g` (check, open, verify
/// the opened file). A refusal is an `Err` whose text names the path and carries `OA-SEAL-001`.
pub fn load_file(g: &Sealed, path: &Path, file: u32, out: &mut Vec<Rec>) -> io::Result<FileInfo> {
    load_file_counted(g, path, file, out, &mut 0)
}

/// [`load_file`], with the sealed-window count put in `sealed` on every exit.
pub fn load_file_counted(
    g: &Sealed,
    path: &Path,
    file: u32,
    out: &mut Vec<Rec>,
    sealed: &mut u64,
) -> io::Result<FileInfo> {
    load_file_counted_with_budgets(g, path, file, out, sealed, 0, 0)
}

/// [`load_file_counted`] with cumulative source and decoded-byte budgets from earlier files.
pub fn load_file_counted_with_budgets(
    g: &Sealed,
    path: &Path,
    file: u32,
    out: &mut Vec<Rec>,
    sealed: &mut u64,
    source_bytes_before: u64,
    decoded_bytes_before: u64,
) -> io::Result<FileInfo> {
    let f = open_guarded(g, path)?;
    let size = f.metadata()?.len();
    if size > MAX_STDDS_FILE_BYTES {
        return Err(io::Error::other(format!(
            "{} exceeds the {}-byte STDDS source limit",
            path.display(),
            MAX_STDDS_FILE_BYTES
        )));
    }
    if source_bytes_before.saturating_add(size) > MAX_TOTAL_STDDS_BYTES {
        return Err(io::Error::other(format!(
            "STDDS inputs exceed the {}-byte combined source limit",
            MAX_TOTAL_STDDS_BYTES
        )));
    }
    let remaining_decoded = MAX_TOTAL_STDDS_DECODED_BYTES.saturating_sub(decoded_bytes_before);
    let decoded_cap = MAX_STDDS_DECODED_FILE_BYTES.min(remaining_decoded);
    if decoded_cap == 0 {
        return Err(io::Error::other(
            "STDDS inputs exceed the combined decoded-byte limit",
        ));
    }
    load_reader_counted_with_decoded_cap(
        f,
        &path.display().to_string(),
        file,
        out,
        sealed,
        MAX_STDDS_FILE_BYTES.min(MAX_TOTAL_STDDS_BYTES.saturating_sub(source_bytes_before)),
        decoded_cap,
    )
}

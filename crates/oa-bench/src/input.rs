//! Reading the recorded message directories. Shell (B3): files, hashing, and the order of reading
//! live here; the decoding is the core's.
//!
//! A directory is either **raw message files** (`*.msg`, one SWIM document per file, read in name
//! order, as the SCDS recorder wrote them; they carry no receive time, so T6 is withheld and there
//! is no replay state hash) or **`.oaf` / `.oaf.gz` recordings** (frames with receive times,
//! merged in the replay order of `oa-replay`; the replay pipeline runs over the same frames and
//! its state hash goes in the report).
//!
//! **One read per file.** The digest of an input file is computed from the bytes that are
//! ingested, in the same read ([`HashRead`] wraps the reader the ingestion pulls from), so a file
//! that changes between listing and reading, or while it is read, cannot give a digest that does
//! not describe what was scored. The `.msg` files, the `.oaf` and `.oaf.gz` recordings (through
//! the replay merge) and the ADS-B clip all go this way; a file that is not read to its end is an
//! error, not a digest. A `.msg` file longer than a document can be is still read to its end (the
//! digest covers every byte) but through a fixed buffer, and is counted as a refused document. [`hash_files`] reads separately and is for stand-alone use only.

use std::cell::RefCell;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use oa_replay::{
    list_recordings, merge_streams, new_pipeline, pipeline_drain_checkpoints, process_frame,
    replay_hash, FeedKind, PipelineConfig, StreamSpec,
};
use oa_sha256::{sha256, sha256_reader, to_hex, Digest, Sha256};

use crate::ingest::{ingest_sfdps, ingest_tbfm, ingest_tfms, new_reader, Reader};
use crate::legacy::{new_legacy, Legacy, NO_TIME};
use crate::report::InputHash;

/// What a directory holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirKind {
    /// `*.msg` files.
    Msg,
    /// `.oaf` or `.oaf.gz` recordings.
    Oaf,
}

/// The input files of a directory in reading order, and what they are.
pub fn list_inputs(dir: &Path) -> io::Result<(DirKind, Vec<PathBuf>)> {
    let mut msgs: Vec<PathBuf> = Vec::new();
    // The sealed hold-out guard sees the directory, and every entry before it is stat-ed.
    oa_sealed::refuse_io(dir)?;
    for e in fs::read_dir(dir)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", dir.display())))?
    {
        let p = e?.path();
        oa_sealed::refuse_io(&p)?;
        if p.is_file() && p.extension().is_some_and(|x| x == "msg") {
            msgs.push(p);
        }
    }
    if !msgs.is_empty() {
        msgs.sort();
        return Ok((DirKind::Msg, msgs));
    }
    Ok((DirKind::Oaf, list_recordings(dir)?))
}

/// The hash of a set of files: for one file, its SHA-256; for several, the SHA-256 of the lines
/// `<file name>:<sha256>\n` in the given order. Also the file count and total size.
pub(crate) fn manifest_hash(label: &str, files: &[PathBuf], sums: &[(Digest, u64)]) -> InputHash {
    let mut bytes = 0u64;
    let mut manifest = String::new();
    let mut single = String::new();
    for (p, (d, len)) in files.iter().zip(sums) {
        bytes += len;
        let h = to_hex(d);
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        manifest.push_str(&format!("{name}:{h}\n"));
        single = h;
    }
    let sha = if files.len() == 1 {
        single
    } else {
        to_hex(&sha256(manifest.as_bytes()))
    };
    InputHash {
        label: label.to_string(),
        files: files.len() as u64,
        bytes,
        sha256: sha,
    }
}

/// The hash of a set of files by a read of its own (see [`manifest_hash`] for the form). Not what
/// [`load`] uses: that hashes the bytes it ingests.
pub fn hash_files(label: &str, files: &[PathBuf]) -> io::Result<InputHash> {
    let mut sums = Vec::with_capacity(files.len());
    for p in files {
        oa_sealed::refuse_io(p)?;
        let len = fs::metadata(p)?.len();
        let f = oa_sealed::open(p)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))?;
        let d = sha256_reader(io::BufReader::new(f)).map_err(io::Error::other)?;
        sums.push((d, len));
    }
    Ok(manifest_hash(label, files, &sums))
}

/// Slack over the parser's own document limit for a `.msg` file (an XML declaration, a byte order
/// mark, trailing white space); a file longer than the limit plus this is not a document.
pub const MSG_MARGIN: u64 = 64 * 1024;

/// The refusal reason counted for a `.msg` file past the limit.
pub const MSG_TOO_LONG: &str = "longer than a document can be";

/// The longest `.msg` file kept: the replay parser's document limit plus [`MSG_MARGIN`].
pub fn msg_limit() -> u64 {
    oa_replay::doc_xml_limits().max_input_bytes as u64 + MSG_MARGIN
}

/// A reader that passes on at most `limit` bytes and then fails with `InvalidData` rather than
/// give one more, so a file that grows or lies about its length cannot be pulled whole into memory.
struct Capped<R: Read> {
    inner: R,
    left: u64,
    limit: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            // At the limit: only a clean end is allowed (one byte is enough to tell).
            let mut one = [0u8; 1];
            return match self.inner.read(&mut one)? {
                0 => Ok(0),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("longer than the limit of {} bytes", self.limit),
                )),
            };
        }
        let room = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(out.len());
        let n = match out.get_mut(..room) {
            Some(w) => self.inner.read(w)?,
            None => 0,
        };
        self.left -= n as u64;
        Ok(n)
    }
}

/// Reads all of `path` up to `max` bytes. A file whose length is past `max`, or that turns out
/// longer while it is read, is refused with `InvalidData` naming the file; nothing near its size
/// is allocated.
pub fn read_capped_file(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let named = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    oa_sealed::refuse_io(path).map_err(named)?;
    let len = fs::metadata(path).map_err(named)?.len();
    if len > max {
        return Err(named(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{len} bytes, past the limit of {max} bytes"),
        )));
    }
    let f = oa_sealed::open(path).map_err(named)?;
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    Capped {
        inner: f,
        left: max,
        limit: max,
    }
    .read_to_end(&mut out)
    .map_err(named)?;
    Ok(out)
}

/// Opens one input file for reading. The seam for [`load_with`] and [`load_adsb_with`]: the real
/// one opens the file; a test hands back other bytes than the disk holds, as a file that changed
/// after it was listed.
pub type Opener = Rc<dyn Fn(&Path) -> io::Result<Box<dyn Read>>>;

/// The opener that reads files from disk.
pub fn file_opener() -> Opener {
    Rc::new(|p: &Path| {
        oa_sealed::open(p)
            .map(|f| Box::new(f) as Box<dyn Read>)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))
    })
}

/// Where a [`HashRead`] leaves its result once the file is read to its end: digest and length.
type Done = Rc<RefCell<Option<Result<(Digest, u64), String>>>>;

fn new_done() -> Done {
    Rc::new(RefCell::new(None))
}

/// A reader that hashes every byte it passes on, however many reads that takes. A read of zero
/// bytes puts the digest of everything read so far, and the length, in `done`; bytes read after
/// that take it away again until the next zero read. A reader dropped with no zero read last leaves
/// `done` empty: the file was not read to its end.
// A7.2: cold struct, size guard waived
struct HashRead<R: Read> {
    inner: R,
    h: Sha256,
    bytes: u64,
    failed: bool,
    done: Done,
}

impl<R: Read> HashRead<R> {
    fn new(inner: R, done: Done) -> HashRead<R> {
        HashRead {
            inner,
            h: Sha256::new(),
            bytes: 0,
            failed: false,
            done,
        }
    }
}

impl<R: Read> Read for HashRead<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(out)?;
        if n == 0 && !out.is_empty() {
            let r = if self.failed {
                Err("the file is too long to hash".to_string())
            } else {
                Ok((self.h.clone().finalize(), self.bytes))
            };
            *self.done.borrow_mut() = Some(r);
            return Ok(0);
        }
        if n > 0 {
            *self.done.borrow_mut() = None;
        }
        if let Some(chunk) = out.get(..n) {
            self.failed |= self.h.update(chunk).is_err();
        }
        self.bytes += n as u64;
        Ok(n)
    }
}

/// The digest and length of a file whose reader reached its end.
fn finished(path: &Path, done: &Done) -> io::Result<(Digest, u64)> {
    match done.borrow_mut().take() {
        Some(Ok(s)) => Ok(s),
        Some(Err(e)) => Err(io::Error::other(format!("{}: {e}", path.display()))),
        None => Err(io::Error::other(format!(
            "{}: not read to its end, so its digest does not describe what was read",
            path.display()
        ))),
    }
}

/// The [`InputHash`] of files all read through [`HashRead`] with the given `done` cells.
fn input_hash(label: &str, files: &[PathBuf], dones: &[Done]) -> io::Result<InputHash> {
    let mut sums = Vec::with_capacity(files.len());
    for (p, d) in files.iter().zip(dones) {
        sums.push(finished(p, d)?);
    }
    Ok(manifest_hash(label, files, &sums))
}

/// A stream over `files`, each opened with `open` and hashed into its `done` cell as it is read.
fn hashed_spec(feed: FeedKind, files: Vec<PathBuf>, dones: Vec<Done>, open: Opener) -> StreamSpec {
    let mut it = files.into_iter().zip(dones);
    StreamSpec {
        feed,
        open: Box::new(move || {
            let (p, done) = it.next()?;
            Some(open(&p).map(|r| Box::new(HashRead::new(r, done)) as Box<dyn Read>))
        }),
    }
}

/// One feed's input directory.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct FeedInput {
    pub feed: FeedKind,
    pub dir: PathBuf,
}

/// What loading produced.
// A7.2: cold struct, size guard waived
pub struct Loaded {
    pub legacy: Legacy,
    pub inputs: Vec<InputHash>,
    /// The replay state hash in hex, or why there is none.
    pub replay_hash: String,
    pub notes: Vec<String>,
}

fn ingest_one(leg: &mut Legacy, r: &mut Reader, feed: FeedKind, bytes: &[u8], recv_ms: i64) {
    match feed {
        FeedKind::Sfdps => ingest_sfdps(leg, r, bytes, recv_ms),
        FeedKind::Tbfm => ingest_tbfm(leg, r, bytes),
        FeedKind::Tfms => ingest_tfms(leg, r, bytes),
        FeedKind::Fns | FeedKind::Adsb => {}
    }
}

/// Loads the given feed directories into the legacy tables.
pub fn load(feeds: &[FeedInput]) -> io::Result<Loaded> {
    load_with(feeds, &file_opener())
}

/// One feed's listed files, what they are, and where each digest will land.
type Listed = (FeedKind, DirKind, Vec<PathBuf>, Vec<Done>);

/// [`load`] with the files opened by `open`.
pub fn load_with(feeds: &[FeedInput], open: &Opener) -> io::Result<Loaded> {
    load_with_max(feeds, open, msg_limit())
}

/// [`load_with`] with `max_msg` as the longest `.msg` file accepted.
pub fn load_with_max(feeds: &[FeedInput], open: &Opener, max_msg: u64) -> io::Result<Loaded> {
    let mut leg = new_legacy();
    let mut reader = new_reader().map_err(io::Error::other)?;
    let mut listed: Vec<Listed> = Vec::new();
    for fi in feeds {
        let (kind, files) = list_inputs(&fi.dir)?;
        let dones = files.iter().map(|_| new_done()).collect();
        listed.push((fi.feed, kind, files, dones));
    }
    let all_oaf = !listed.is_empty() && listed.iter().all(|(_, k, _, _)| *k == DirKind::Oaf);
    let mut notes = Vec::new();
    let replay = if all_oaf {
        let mut p = new_pipeline(PipelineConfig::default())
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let specs: Vec<StreamSpec> = listed
            .iter()
            .map(|(f, _, files, dones)| hashed_spec(*f, files.clone(), dones.clone(), open.clone()))
            .collect();
        let mut sink = |m: &oa_replay::Merged<'_>| {
            process_frame(&mut p, m.feed, m.kind, m.recv_ns, m.payload);
            // Only the final hash is kept: the checkpoints are taken out as they are recorded, so
            // none is ever dropped for want of room.
            pipeline_drain_checkpoints(&mut p, |_| {});
            if m.kind == oa_feed::Kind::Message {
                ingest_one(
                    &mut leg,
                    &mut reader,
                    m.feed,
                    m.payload,
                    m.recv_ns.div_euclid(1_000_000),
                );
            }
        };
        let rep = merge_streams(specs, 1 << 16, &mut sink)?;
        let bad: u64 = rep.errors.iter().sum();
        if bad > 0 {
            notes.push(format!(
                "the recordings had {bad} frame or gzip errors (counted by oa-replay)"
            ));
        }
        to_hex(&replay_hash(&p))
    } else {
        for (feed, kind, files, dones) in &listed {
            match kind {
                DirKind::Msg => {
                    for (f, done) in files.iter().zip(dones) {
                        // Read to the end through a fixed buffer, so the digest covers every
                        // byte; only a document within the limit is kept.
                        let mut hr = HashRead::new(open(f)?, done.clone());
                        let mut b: Vec<u8> = Vec::new();
                        let mut buf = [0u8; 8192];
                        let mut too_long = false;
                        loop {
                            let n = hr.read(&mut buf).map_err(|e| {
                                io::Error::new(e.kind(), format!("{}: {e}", f.display()))
                            })?;
                            let Some(chunk) = buf.get(..n).filter(|c| !c.is_empty()) else {
                                break;
                            };
                            if !too_long {
                                if (b.len() + n) as u64 > max_msg {
                                    too_long = true;
                                    b = Vec::new();
                                } else {
                                    b.extend_from_slice(chunk);
                                }
                            }
                        }
                        if too_long {
                            let fi = match feed {
                                FeedKind::Sfdps => Some(0),
                                FeedKind::Tbfm => Some(1),
                                FeedKind::Tfms => Some(2),
                                FeedKind::Fns | FeedKind::Adsb => None,
                            };
                            if let Some(fi) = fi {
                                if let (Some(d), Some(e)) = (
                                    leg.counts.docs.get_mut(fi),
                                    leg.counts.doc_errors.get_mut(fi),
                                ) {
                                    *d += 1;
                                    *e += 1;
                                }
                            }
                            leg.counts.note(MSG_TOO_LONG.to_string(), 1);
                            continue;
                        }
                        ingest_one(&mut leg, &mut reader, *feed, &b, NO_TIME);
                    }
                }
                DirKind::Oaf => {
                    let specs = vec![hashed_spec(
                        *feed,
                        files.clone(),
                        dones.clone(),
                        open.clone(),
                    )];
                    let mut sink = |m: &oa_replay::Merged<'_>| {
                        if m.kind == oa_feed::Kind::Message {
                            ingest_one(
                                &mut leg,
                                &mut reader,
                                m.feed,
                                m.payload,
                                m.recv_ns.div_euclid(1_000_000),
                            );
                        }
                    };
                    merge_streams(specs, 1 << 16, &mut sink)?;
                }
            }
        }
        notes.push("raw .msg input carries no frame headers: no receive time (T6 withheld) and no replay state hash".to_string());
        "not available: the inputs are not all .oaf recordings".to_string()
    };
    notes.push(
        "input digests: each file is hashed from the bytes ingested, in the same read (.msg files as read, recordings through the replay merge)"
            .to_string(),
    );
    let mut inputs = Vec::new();
    for (fi, (_, _, files, dones)) in feeds.iter().zip(&listed) {
        let label = format!(
            "{} ({})",
            oa_replay::FEED_NAMES
                .get(fi.feed as usize)
                .copied()
                .unwrap_or("?"),
            fi.dir.display()
        );
        inputs.push(input_hash(&label, files, dones)?);
    }
    Ok(Loaded {
        legacy: leg,
        inputs,
        replay_hash: replay,
        notes,
    })
}

/// An ADS-B clip read for truth construction.
// A7.2: cold struct, size guard waived
pub struct AdsbClip {
    pub reports: oa_adsb::Reports,
    /// Frames that did not decode (bad frame, or a payload whose header or length is wrong),
    /// counted not hidden.
    pub bad_frames: u64,
    /// Records dropped from frames that did decode (position or flags out of range); the rest of
    /// such a frame is kept.
    pub dropped_records: u64,
    pub hash: InputHash,
}

/// Calls `f(recv_unix_ns, payload)` for every message frame of an OAF1 file, in file order, and
/// returns the number of frames that were damaged (bad CRC or length, or a partial frame at the
/// end of the file; counted, not hidden).
pub fn for_each_adsb_frame(path: &Path, f: &mut dyn FnMut(i64, &[u8])) -> io::Result<u64> {
    let mut file = oa_sealed::open(path)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    oa_feed::read_frames(&mut file, &mut |fr| {
        if fr.kind == oa_feed::Kind::Message {
            f(fr.recv_unix_ns, fr.payload);
        }
    })
    .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// Reads every ADS-B payload frame of an OAF1 file (docs/ADSB_PAYLOAD.md) into one set of reports
/// and hashes the file, in the same read. A bad record loses only itself
/// (`decode_payload_lenient`).
pub fn load_adsb(path: &Path) -> io::Result<AdsbClip> {
    load_adsb_with(path, &file_opener())
}

/// [`load_adsb`] with the file opened by `open`.
pub fn load_adsb_with(path: &Path, open: &Opener) -> io::Result<AdsbClip> {
    let mut reports = oa_adsb::Reports::default();
    let mut undecodable = 0u64;
    let mut dropped_records = 0u64;
    let done = new_done();
    let mut src = HashRead::new(open(path)?, done.clone());
    let damaged = oa_feed::read_frames(&mut src, &mut |fr| {
        if fr.kind == oa_feed::Kind::Message {
            match oa_adsb::decode_payload_lenient(fr.payload, &mut reports) {
                Ok(c) => dropped_records += c.dropped as u64,
                Err(_) => undecodable += 1,
            }
        }
    })
    .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    let hash = input_hash(
        &format!("adsb ({})", path.display()),
        &[path.to_path_buf()],
        &[done],
    )?;
    Ok(AdsbClip {
        reports,
        bad_frames: damaged + undecodable,
        dropped_records,
        hash,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unused_io_amount
)]
mod tests {
    use super::*;

    /// Hands out its chunks in order; an empty chunk is a read of zero bytes.
    struct Chunks(std::collections::VecDeque<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let c = self.0.pop_front().unwrap_or_default();
            out[..c.len()].copy_from_slice(&c);
            Ok(c.len())
        }
    }

    // REQ: OA-BENCH-062
    #[test]
    fn the_digest_covers_every_byte_of_every_read_even_after_a_zero_read() {
        let chunks = vec![b"abc".to_vec(), vec![], b"def".to_vec(), vec![]];
        let done = new_done();
        let mut r = HashRead::new(Chunks(chunks.into()), done.clone());
        let mut buf = [0u8; 16];
        let mut got = Vec::new();
        for _ in 0..4 {
            let n = r.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"abcdef");
        let (d, n) = finished(Path::new("x"), &done).unwrap();
        assert_eq!((d, n), (sha256(b"abcdef"), 6));
        // Data after the last zero read: not read to its end, so no digest.
        let done = new_done();
        let chunks = vec![b"abc".to_vec(), vec![], b"def".to_vec()];
        let mut r = HashRead::new(Chunks(chunks.into()), done.clone());
        for _ in 0..3 {
            r.read(&mut buf).unwrap();
        }
        assert!(finished(Path::new("x"), &done).is_err());
    }
}

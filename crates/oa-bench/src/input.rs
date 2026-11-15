//! Reading the recorded message directories. Shell (B3): files, hashing, and the order of reading
//! live here; the decoding is the core's.
//!
//! A directory is either **raw message files** (`*.msg`, one SWIM document per file, read in name
//! order, as the SCDS recorder wrote them; they carry no receive time, so T6 is withheld and there
//! is no replay state hash) or **`.oaf` / `.oaf.gz` recordings** (frames with receive times,
//! merged in the replay order of `oa-replay`; the replay pipeline runs over the same frames and
//! its state hash goes in the report).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use oa_replay::{
    list_recordings, merge_streams, new_pipeline, process_frame, replay_hash, spec_from_paths,
    FeedKind, PipelineConfig, StreamSpec,
};
use oa_sha256::{sha256, sha256_reader, to_hex};

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
    for e in fs::read_dir(dir)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", dir.display())))?
    {
        let p = e?.path();
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
pub fn hash_files(label: &str, files: &[PathBuf]) -> io::Result<InputHash> {
    let mut bytes = 0u64;
    let mut manifest = String::new();
    let mut single = String::new();
    for p in files {
        bytes += fs::metadata(p)?.len();
        let f = fs::File::open(p)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))?;
        let d = sha256_reader(io::BufReader::new(f)).map_err(io::Error::other)?;
        let h = to_hex(&d);
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        manifest.push_str(&format!("{name}:{h}\n"));
        single = h;
    }
    let sha = if files.len() == 1 {
        single
    } else {
        to_hex(&sha256(manifest.as_bytes()))
    };
    Ok(InputHash {
        label: label.to_string(),
        files: files.len() as u64,
        bytes,
        sha256: sha,
    })
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
    let mut leg = new_legacy();
    let mut reader = new_reader().map_err(io::Error::other)?;
    let mut inputs = Vec::new();
    let mut listed: Vec<(FeedKind, DirKind, Vec<PathBuf>)> = Vec::new();
    for fi in feeds {
        let (kind, files) = list_inputs(&fi.dir)?;
        inputs.push(hash_files(
            &format!(
                "{} ({})",
                oa_replay::FEED_NAMES
                    .get(fi.feed as usize)
                    .copied()
                    .unwrap_or("?"),
                fi.dir.display()
            ),
            &files,
        )?);
        listed.push((fi.feed, kind, files));
    }
    let all_oaf = !listed.is_empty() && listed.iter().all(|(_, k, _)| *k == DirKind::Oaf);
    let mut notes = Vec::new();
    let replay = if all_oaf {
        let mut p = new_pipeline(PipelineConfig::default())
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let specs: Vec<StreamSpec> = listed
            .iter()
            .map(|(f, _, files)| spec_from_paths(*f, files.clone()))
            .collect();
        let mut sink = |m: &oa_replay::Merged<'_>| {
            process_frame(&mut p, m.feed, m.kind, m.recv_ns, m.payload);
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
        for (feed, kind, files) in &listed {
            match kind {
                DirKind::Msg => {
                    for f in files {
                        let b = fs::read(f).map_err(|e| {
                            io::Error::new(e.kind(), format!("{}: {e}", f.display()))
                        })?;
                        ingest_one(&mut leg, &mut reader, *feed, &b, NO_TIME);
                    }
                }
                DirKind::Oaf => {
                    let specs = vec![spec_from_paths(*feed, files.clone())];
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
/// returns the number of frames that were damaged (bad CRC or length; counted, not hidden).
pub fn for_each_adsb_frame(path: &Path, f: &mut dyn FnMut(i64, &[u8])) -> io::Result<u64> {
    let err = |m: String| io::Error::other(format!("{}: {m}", path.display()));
    let mut file = fs::File::open(path).map_err(|e| err(e.to_string()))?;
    let mut buf = vec![0u8; oa_feed::MAX_FRAME_LEN];
    let mut rd = oa_feed::FrameReader::new(&mut buf).map_err(|e| err(format!("{e:?}")))?;
    let mut bad = 0u64;
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let n = io::Read::read(&mut file, &mut chunk).map_err(|e| err(e.to_string()))?;
        if n == 0 {
            break;
        }
        let mut at = 0usize;
        while at < n {
            let Some(rest) = chunk.get(at..n) else { break };
            let took = rd.push(rest);
            at += took;
            while let Some(fr) = rd.next() {
                match fr {
                    Ok(fr) if fr.kind == oa_feed::Kind::Message => f(fr.recv_unix_ns, fr.payload),
                    Ok(_) => {}
                    Err(_) => bad += 1,
                }
            }
            if took == 0 {
                break;
            }
        }
    }
    Ok(bad)
}

/// Reads every ADS-B payload frame of an OAF1 file (docs/ADSB_PAYLOAD.md) into one set of reports
/// and hashes the file. A bad record loses only itself (`decode_payload_lenient`).
pub fn load_adsb(path: &Path) -> io::Result<AdsbClip> {
    let mut reports = oa_adsb::Reports::default();
    let mut undecodable = 0u64;
    let mut dropped_records = 0u64;
    let damaged = for_each_adsb_frame(
        path,
        &mut |_, payload| match oa_adsb::decode_payload_lenient(payload, &mut reports) {
            Ok(c) => dropped_records += c.dropped as u64,
            Err(_) => undecodable += 1,
        },
    )?;
    let bad = damaged + undecodable;
    let hash = hash_files(&format!("adsb ({})", path.display()), &[path.to_path_buf()])?;
    Ok(AdsbClip {
        reports,
        bad_frames: bad,
        dropped_records,
        hash,
    })
}

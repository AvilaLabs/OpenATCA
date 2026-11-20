//! Shell: reads the recording files of one directory (OAF1/OAF2 frames, raw or gzip, decided by
//! the gzip magic), parses each message frame with oa-stdds and feeds the TAIS records to the
//! census. Files are read in name order (the names carry the start time). A file still being
//! written ends inside a gzip member: that is flagged, never fatal, and what was recovered is
//! used.

use crate::census::Census;
use oa_feed::frame::{Kind as FrameKind, MAX_FRAME_LEN};
use oa_feed::reader::FrameReader;
use oa_feed::replay::REPLAY_LIMITS;
use oa_stdds::{new_stats, parse_message, std_error_index, Limits, Message, Stats};
use oa_xml::{Limits as XmlLimits, Parser};
use oa_zip::GzipReader;
use std::convert::Infallible;
use std::io::Read;
use std::path::{Path, PathBuf};

/// What the read saw, in counts.
// A7.2: cold struct, size guard waived
#[derive(Debug)]
pub struct ReadStats {
    pub files: u64,
    pub files_gzip_truncated: u64,
    pub files_gzip_damaged: u64,
    pub frames: u64,
    pub other_frames: u64,
    pub bad_frames: u64,
    pub tais_messages: u64,
    pub tais_records: u64,
    /// Messages of any other kind (SMES, status, ...), parsed and not used.
    pub other_messages: u64,
    /// Messages refused whole, by [`oa_stdds::STD_ERROR_NAMES`] index.
    pub refused: [u64; 4],
    /// Bytes the frame reader would not take (a stall).
    pub stalled_bytes: u64,
    /// Parse-level stats of every message (field presence, errors).
    pub stats: Stats,
}

struct Ctx<'a> {
    parser: Parser,
    limits: Limits,
    census: &'a mut Census,
    rs: ReadStats,
}

impl ReadStats {
    pub fn new() -> ReadStats {
        ReadStats {
            files: 0,
            files_gzip_truncated: 0,
            files_gzip_damaged: 0,
            frames: 0,
            other_frames: 0,
            bad_frames: 0,
            tais_messages: 0,
            tais_records: 0,
            other_messages: 0,
            refused: [0; 4],
            stalled_bytes: 0,
            stats: new_stats(),
        }
    }
}

impl Default for ReadStats {
    fn default() -> ReadStats {
        ReadStats::new()
    }
}

fn drain(reader: &mut FrameReader<'_>, ctx: &mut Ctx<'_>) {
    while let Some(ev) = reader.next() {
        match ev {
            Ok(f) if f.kind == FrameKind::Message => {
                ctx.rs.frames += 1;
                match parse_message(&mut ctx.parser, f.payload, &ctx.limits, &mut ctx.rs.stats) {
                    Ok(Message::Tais(m)) => {
                        ctx.rs.tais_messages += 1;
                        let src = if m.src_present { m.src } else { [0; 8] };
                        for r in &m.records {
                            ctx.rs.tais_records += 1;
                            ctx.census.observe(src, r);
                        }
                    }
                    Ok(_) => ctx.rs.other_messages += 1,
                    Err(e) => {
                        if let Some(c) = ctx.rs.refused.get_mut(std_error_index(&e)) {
                            *c += 1;
                        }
                    }
                }
            }
            Ok(_) => ctx.rs.other_frames += 1,
            Err(_) => ctx.rs.bad_frames += 1,
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
            ctx.rs.stalled_bytes += rest.len() as u64;
            break;
        }
    }
}

/// Reads one stream (raw frames or gzip of them) into `ctx`.
fn read_stream<R: Read>(mut src: R, ctx: &mut Ctx<'_>) -> std::io::Result<()> {
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let Ok(mut reader) = FrameReader::new(&mut storage) else {
        return Err(std::io::Error::other("frame reader buffer too small"));
    };
    let mut chunk = vec![0u8; 64 * 1024];
    let mut n = 0;
    // Read until two bytes (the gzip magic) or the end of the stream.
    while n < 2 {
        let got = match chunk.get_mut(n..) {
            Some(b) => src.read(b)?,
            None => 0,
        };
        if got == 0 {
            break;
        }
        n += got;
    }
    let mut gz = chunk
        .get(..2)
        .is_some_and(|m| n >= 2 && m == [0x1f, 0x8b])
        .then(|| Box::new(GzipReader::new(REPLAY_LIMITS)));
    let mut damaged = false;
    while n > 0 {
        let data = chunk.get(..n).unwrap_or(&[]);
        match gz.as_mut() {
            None => feed(&mut reader, ctx, data),
            Some(g) if !damaged => {
                let r = g.push(data, &mut |b: &[u8]| -> Result<(), Infallible> {
                    feed(&mut reader, ctx, b);
                    Ok(())
                });
                damaged = r.is_err();
            }
            Some(_) => {}
        }
        n = src.read(&mut chunk)?;
    }
    drain(&mut reader, ctx);
    if reader.finish().is_some() {
        ctx.rs.bad_frames += 1;
    }
    ctx.rs.files += 1;
    ctx.rs.files_gzip_damaged += u64::from(damaged);
    ctx.rs.files_gzip_truncated +=
        u64::from(gz.as_ref().is_some_and(|g| g.finish().truncated.is_some()));
    Ok(())
}

/// Deepest subdirectory level searched below the given directory.
const MAX_DEPTH: usize = 3;

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        // A real subdirectory is searched; a link to one is not (no loops, no jumps elsewhere).
        if e.file_type()?.is_dir() {
            if depth < MAX_DEPTH {
                collect(&p, depth + 1, out)?;
            }
        } else if p.is_file()
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".oaf") || n.ends_with(".oaf.gz"))
        {
            out.push(p);
        }
    }
    Ok(())
}

/// The recording files of `dir` and its subdirectories (up to three levels), in path order:
/// names ending `.oaf` or `.oaf.gz`.
pub fn recording_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut v = Vec::new();
    collect(dir, 0, &mut v)?;
    v.sort();
    Ok(v)
}

/// Reads every recording in `dir` into `census`. The caller has checked the path against the
/// sealed list; each file is checked again here.
pub fn read_dir(dir: &Path, census: &mut Census) -> Result<ReadStats, String> {
    let files = recording_files(dir).map_err(|e| format!("cannot list the directory: {e}"))?;
    let parser = Parser::new(XmlLimits::default()).map_err(|e| format!("xml parser: {e:?}"))?;
    let mut ctx = Ctx {
        parser,
        limits: Limits::default(),
        census,
        rs: ReadStats::new(),
    };
    for f in &files {
        crate::guard::refuse_sealed(f)?;
        let file = std::fs::File::open(f).map_err(|e| format!("cannot open a recording: {e}"))?;
        read_stream(file, &mut ctx).map_err(|e| format!("cannot read a recording: {e}"))?;
    }
    Ok(ctx.rs)
}

/// Reads one in-memory stream (tests).
pub fn read_bytes(bytes: &[u8], census: &mut Census) -> Result<ReadStats, String> {
    let parser = Parser::new(XmlLimits::default()).map_err(|e| format!("xml parser: {e:?}"))?;
    let mut ctx = Ctx {
        parser,
        limits: Limits::default(),
        census,
        rs: ReadStats::new(),
    };
    read_stream(bytes, &mut ctx).map_err(|e| format!("cannot read: {e}"))?;
    Ok(ctx.rs)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use oa_feed::frame::encode_frame;

    fn msg(src: &str, recs: &[(u32, &str, Option<&str>)]) -> String {
        let mut s = format!(
            "<?xml version=\"1.0\"?><ns2:TATrackAndFlightPlan xmlns:ns2=\"urn:us:gov:dot:faa:atm:terminal:entities:v4-0:tais:terminalautomationinformation\"><src>{src}</src>"
        );
        for (n, t, adsb) in recs {
            s += &format!("<record><track><trackNum>{n}</trackNum><mrtTime>{t}</mrtTime>");
            if let Some(a) = adsb {
                s += &format!("<adsb>{a}</adsb>");
            }
            s += "</track></record>";
        }
        s + "</ns2:TATrackAndFlightPlan>"
    }

    fn frames(payloads: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, p) in payloads.iter().enumerate() {
            let mut buf = vec![0u8; MAX_FRAME_LEN];
            let n = encode_frame(&mut buf, FrameKind::Message as u8, 0, i as u64, 0, 0, p).unwrap();
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    // REQ: OA-CEN-005
    #[test]
    fn messages_flow_into_the_census_and_absent_flags_stay_absent() {
        let a = msg(
            "ZZA",
            &[
                (1, "2030-01-02T03:04:00.000Z", Some("false")),
                (2, "2030-01-02T03:04:00.000Z", None),
            ],
        );
        let b = msg(
            "ZZA",
            &[
                (1, "2030-01-02T03:04:04.600Z", Some("0")),
                (2, "2030-01-02T03:04:01.000Z", Some("true")),
            ],
        );
        let junk = b"not xml".to_vec();
        let bytes = frames(&[a.into_bytes(), junk, b.into_bytes()]);
        let mut c = Census::new();
        let rs = read_bytes(&bytes, &mut c).unwrap();
        assert_eq!(rs.frames, 3);
        assert_eq!(rs.tais_messages, 2);
        assert_eq!(rs.tais_records, 4);
        assert_eq!(rs.refused[0], 1);
        assert_eq!(rs.files, 1);
        let r = c.finish();
        assert_eq!(r.global.updates, [2, 1, 1]);
        assert_eq!(r.facs.len(), 1);
        assert_eq!(r.global.iv[0].n, 1);
        assert!((r.global.iv[0].quantile(1, 2).unwrap() - 4.605).abs() < 1e-9);
    }

    // REQ: OA-CEN-005
    #[test]
    fn recordings_are_found_in_subdirectories_in_name_order() {
        let base = std::env::temp_dir().join(format!("oa-census-find-{}", std::process::id()));
        let sub = base.join("rec");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(base.join("log")).unwrap();
        for n in ["b-0002.oaf.gz", "b-0001.oaf", "notes.txt"] {
            std::fs::write(sub.join(n), b"").unwrap();
        }
        std::fs::write(base.join("log").join("x.log"), b"").unwrap();
        let names: Vec<String> = recording_files(&base)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(names, ["b-0001.oaf", "b-0002.oaf.gz"]);
    }

    // REQ: OA-CEN-005
    #[test]
    fn a_frame_cut_short_is_counted_not_fatal() {
        let a = msg("ZZA", &[(1, "2030-01-02T03:04:00.000Z", Some("false"))]);
        let mut bytes = frames(&[a.clone().into_bytes(), a.into_bytes()]);
        bytes.truncate(bytes.len() - 5);
        let mut c = Census::new();
        let rs = read_bytes(&bytes, &mut c).unwrap();
        assert_eq!(rs.tais_records, 1);
        assert_eq!(rs.bad_frames, 1);
    }
}

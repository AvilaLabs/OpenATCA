//! Shell: reads the recording files of one directory (OAF1/OAF2 frames, raw or gzip, decided by
//! the gzip magic), parses each message frame with oa-stdds and feeds the TAIS records to the
//! census. Files are read in name order (the names carry the start time). A file still being
//! written ends inside a gzip member: that is flagged, never fatal, and what was recovered is
//! used.

use crate::census::Census;
use crate::guard::{open_guarded, refuse_sealed, OpenFail};
use oa_feed::frame::{Kind as FrameKind, MAX_FRAME_LEN};
use oa_feed::reader::FrameReader;
use oa_feed::replay::REPLAY_LIMITS;
use oa_sealed::Sealed;
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
    /// The memory estimate went past the budget and the read stopped.
    pub over_budget: bool,
    /// Parse-level stats of every message (field presence, errors).
    pub stats: Stats,
    /// Message frames dropped because their time is in a sealed window (OA-SEAL-014).
    pub sealed_window_dropped: u64,
}

struct Ctx<'a> {
    parser: Parser,
    limits: Limits,
    census: &'a mut Census,
    rs: ReadStats,
    /// Decides the damaged frames of the file being read (see [`oa_sealed::DamagedGate`]).
    gate: oa_sealed::DamagedGate,
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
            over_budget: false,
            stats: new_stats(),
            sealed_window_dropped: 0,
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
        if ctx.rs.over_budget {
            break;
        }
        // Every frame is settled against its neighbours: a damaged frame has no receive time of
        // its own, so the good frames either side of it decide whether it is damage or a drop.
        match &ev {
            Ok(f) => {
                let (kept, gone) = ctx.gate.good(f.recv_unix_ns);
                ctx.rs.bad_frames += kept;
                ctx.rs.sealed_window_dropped += gone;
            }
            Err(_) => ctx.gate.damaged(),
        }
        match ev {
            // A frame of any kind (Status and Reject included) received in a sealed window is
            // dropped and counted, never parsed and never counted by kind.
            Ok(f) if oa_sealed::in_sealed_window(f.recv_unix_ns) => {
                // A frame whose time is in a sealed window is dropped and counted, never parsed.
                ctx.rs.sealed_window_dropped += 1;
                oa_sealed::note_window_drops(1);
            }
            Ok(f) if f.kind == FrameKind::Message => {
                let before = ctx.rs.stats.sealed_window_dropped;
                let parsed =
                    parse_message(&mut ctx.parser, f.payload, &ctx.limits, &mut ctx.rs.stats);
                // A message with a record timed in a sealed window was dropped whole by the
                // parser (`Limits::drop_window`), before any count, even when its frame was
                // received outside the window: it is in no count here but the drop.
                if ctx.rs.stats.sealed_window_dropped > before {
                    continue;
                }
                ctx.rs.frames += 1;
                match parsed {
                    Ok(Message::Tais(m)) => {
                        ctx.rs.tais_messages += 1;
                        let src = if m.src_present { m.src } else { [0; 8] };
                        for r in &m.records {
                            ctx.rs.tais_records += 1;
                            ctx.census.observe(src, r);
                        }
                        ctx.rs.over_budget |= ctx.census.over_budget();
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
        if ctx.rs.over_budget {
            break;
        }
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
        ctx.gate.damaged();
    }
    ctx.rs.sealed_window_dropped += ctx.gate.finish();
    ctx.rs.files += 1;
    ctx.rs.files_gzip_damaged += u64::from(damaged);
    ctx.rs.files_gzip_truncated +=
        u64::from(gz.as_ref().is_some_and(|g| g.finish().truncated.is_some()));
    Ok(())
}

/// Deepest subdirectory level searched below the given directory.
const MAX_DEPTH: usize = 3;

/// Why a directory could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// A path that is or holds a sealed recording; nothing was listed or opened.
    Refused(String),
    /// A directory that is not a STDDS recording folder or a capture day (exit 2).
    Layout(String),
    Failed(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Refused(m) | ReadError::Layout(m) | ReadError::Failed(m) => f.write_str(m),
        }
    }
}

fn collect(
    g: &Sealed,
    dir: &Path,
    depth: usize,
    feed_only: bool,
    out: &mut Vec<PathBuf>,
) -> Result<(), ReadError> {
    // Every directory is checked before it is listed, the given one and each below it.
    refuse_sealed(g, dir).map_err(ReadError::Refused)?;
    let fail = |e: std::io::Error| ReadError::Failed(format!("cannot list the directory: {e}"));
    // seal-allow: every directory is checked by refuse_sealed above, before it is listed
    for e in std::fs::read_dir(dir).map_err(fail)? {
        let e = e.map_err(fail)?;
        let p = e.path();
        // A real subdirectory is searched; a link to one is not (no loops, no jumps elsewhere).
        if e.file_type().map_err(fail)?.is_dir() {
            if depth < MAX_DEPTH {
                collect(g, &p, depth + 1, feed_only, out)?;
            }
        } else if p.is_file()
            && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                (n.ends_with(".oaf") || n.ends_with(".oaf.gz"))
                    && (!feed_only || n.starts_with("scds-stdds-"))
            })
        {
            out.push(p);
        }
    }
    Ok(())
}

/// The recording files of `dir` and its subdirectories (up to three levels), in path order:
/// names ending `.oaf` or `.oaf.gz`.
///
/// If `dir` is a capture day directory (it holds `rec/stdds`), only that feed's folder is read:
/// the day also holds the other feeds' recordings (`rec/sfdps`, ...), which are not STDDS.
pub fn recording_files(dir: &Path) -> Result<Vec<PathBuf>, ReadError> {
    recording_files_in(Sealed::production(), dir)
}

/// [`recording_files`] against a given guard.
pub fn recording_files_in(g: &Sealed, dir: &Path) -> Result<Vec<PathBuf>, ReadError> {
    refuse_sealed(g, dir).map_err(ReadError::Refused)?;
    let rec = dir.join("rec");
    let feed = rec.join("stdds");
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let named_rec = [dir, real.as_path()]
        .iter()
        .any(|p| p.file_name().is_some_and(|n| n == "rec"));
    if named_rec {
        return Err(ReadError::Layout(
            "the directory is a capture `rec` folder, which holds every feed; give the day folder or rec/stdds"
                .to_string(),
        ));
    }
    // The guard first: neither folder is probed unless it is unsealed.
    refuse_sealed(g, &feed).map_err(ReadError::Refused)?;
    refuse_sealed(g, &rec).map_err(ReadError::Refused)?;
    // A capture day: only the STDDS feed's files (the day holds the other feeds too).
    // seal-allow: the guard checked the folder above; the probe reads no file
    let has_feed = std::fs::symlink_metadata(&feed).is_ok();
    // seal-allow: the guard checked the folder above; the probe reads no file
    let has_rec = std::fs::symlink_metadata(&rec).is_ok();
    let (start, feed_only) = if has_feed {
        (feed, true)
    } else if has_rec {
        return Err(ReadError::Layout(
            "the directory has a `rec` folder but no `rec/stdds`; give the STDDS recording folder"
                .to_string(),
        ));
    } else {
        (dir.to_path_buf(), false)
    };
    let mut v = Vec::new();
    collect(g, &start, 0, feed_only, &mut v)?;
    v.sort();
    Ok(v)
}

/// Reads every recording in `dir` into `census`. Every directory is checked against the sealed
/// list before it is listed, and each file again before it is opened.
pub fn read_dir(dir: &Path, census: &mut Census) -> Result<ReadStats, ReadError> {
    read_dir_in(Sealed::production(), dir, census)
}

/// [`read_dir`] against a given guard.
pub fn read_dir_in(g: &Sealed, dir: &Path, census: &mut Census) -> Result<ReadStats, ReadError> {
    read_dir_counted(g, dir, census, &mut 0)
}

/// [`read_dir_in`], with the frames and records dropped in a sealed window put in `dropped` as
/// each file is read, so the caller has the count on an error exit too.
pub fn read_dir_counted(
    g: &Sealed,
    dir: &Path,
    census: &mut Census,
    dropped: &mut u64,
) -> Result<ReadStats, ReadError> {
    let files = recording_files_in(g, dir)?;
    let parser = Parser::new(XmlLimits::default())
        .map_err(|e| ReadError::Failed(format!("xml parser: {e:?}")))?;
    let mut ctx = Ctx {
        parser,
        limits: read_limits(),
        census,
        rs: ReadStats::new(),
        gate: oa_sealed::DamagedGate::new(),
    };
    for f in &files {
        refuse_sealed(g, f).map_err(ReadError::Refused)?;
        let file = open_guarded(g, f).map_err(|e| match e {
            OpenFail::Refused(m) => ReadError::Refused(m),
            OpenFail::Io(m) => ReadError::Failed(format!("cannot open a recording: {m}")),
        })?;
        let read = read_stream(file, &mut ctx);
        *dropped = ctx.rs.sealed_window_dropped + ctx.rs.stats.sealed_window_dropped;
        read.map_err(|e| ReadError::Failed(format!("cannot read a recording: {e}")))?;
        if ctx.rs.over_budget {
            return Err(ReadError::Failed(
                "stopped: the memory estimate is past the budget (use --from and --to)".to_string(),
            ));
        }
    }
    // Frames by receive time and records by their own time, in one count.
    ctx.rs.sealed_window_dropped += ctx.rs.stats.sealed_window_dropped;
    Ok(ctx.rs)
}

/// The decoder limits of every read in this tool: the sealed window as the drop window, so a
/// record timed in it is dropped before any count (OA-SEAL-014).
pub fn read_limits() -> Limits {
    Limits::new(oa_sealed::DropWindow::sealed())
}

/// Reads one in-memory stream (tests).
pub fn read_bytes(bytes: &[u8], census: &mut Census) -> Result<ReadStats, String> {
    let parser = Parser::new(XmlLimits::default()).map_err(|e| format!("xml parser: {e:?}"))?;
    let mut ctx = Ctx {
        parser,
        limits: read_limits(),
        census,
        rs: ReadStats::new(),
        gate: oa_sealed::DamagedGate::new(),
    };
    read_stream(bytes, &mut ctx).map_err(|e| format!("cannot read: {e}"))?;
    ctx.rs.sealed_window_dropped += ctx.rs.stats.sealed_window_dropped;
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
        let sub = base.join("segments");
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

    #[cfg(unix)]
    // REQ: OA-CEN-007
    #[test]
    fn a_walk_never_lists_a_sealed_folder_or_the_data_root() {
        use std::os::unix::fs::PermissionsExt;
        let t = std::env::temp_dir().join(format!("oa-census-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let sealed = t.join("scds").join("rec-20261119");
        let day = t.join("scds").join("stdds-day-20261120").join("segments");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("a-0001.oaf"), b"").unwrap();
        std::fs::write(sealed.join("x.oaf"), b"").unwrap();
        let t = t.canonicalize().unwrap();
        let g = crate::guard::tests::guard(&t);
        let sealed = t.join("scds").join("rec-20261119");
        // A listing of the sealed folder would fail (no permission) as Failed, not Refused.
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut c = Census::new();
        // From the data root and the folder itself: refused, nothing listed.
        assert!(matches!(
            read_dir_in(&g, &t.join("scds"), &mut c),
            Err(ReadError::Refused(_))
        ));
        assert!(matches!(
            recording_files_in(&g, &sealed),
            Err(ReadError::Refused(_))
        ));
        // An existing innocent recording directory is accepted and read.
        let ok = t.join("scds").join("stdds-day-20261120");
        assert_eq!(recording_files_in(&g, &ok).unwrap().len(), 1);
        let rs = read_dir_in(&g, &ok, &mut c).unwrap();
        assert_eq!(rs.files, 1);
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[cfg(unix)]
    // REQ: OA-CEN-007
    #[test]
    fn a_link_to_a_sealed_folder_and_a_sealed_file_inside_a_walk_are_refused() {
        let t = std::env::temp_dir().join(format!("oa-census-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let sealed = t.join("scds").join("stdds-20261119-holdout");
        let day = t
            .join("scds")
            .join("day-20261124")
            .join("rec")
            .join("stdds");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(sealed.join("a.oaf"), b"").unwrap();
        let t = t.canonicalize().unwrap();
        let g = crate::guard::tests::guard(&t);
        let link = t.join("innocent");
        std::os::unix::fs::symlink(t.join("scds").join("stdds-20261119-holdout"), &link).unwrap();
        let mut c = Census::new();
        assert!(matches!(
            read_dir_in(&g, &link, &mut c),
            Err(ReadError::Refused(_))
        ));
        // A link to a sealed file inside an accepted folder is refused when it is opened.
        let day = t
            .join("scds")
            .join("day-20261124")
            .join("rec")
            .join("stdds");
        std::os::unix::fs::symlink(
            t.join("scds").join("stdds-20261119-holdout").join("a.oaf"),
            day.join("scds-stdds-20261124T000000Z-0001.oaf"),
        )
        .unwrap();
        let rs = read_dir_in(&g, &t.join("scds").join("day-20261124"), &mut c);
        assert!(matches!(rs, Err(ReadError::Refused(_))));
        std::fs::remove_dir_all(&t).unwrap();
    }

    // REQ: OA-CEN-005
    #[test]
    fn the_capture_day_layout_is_found_and_only_the_stdds_feed_is_read() {
        // run-window.sh writes DIR/rec/<feed>/<feed-name>-<UTC start>-<NNNN>.oaf.gz and DIR/log.
        let t = std::env::temp_dir().join(format!("oa-census-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let day = t.join("day-20261124");
        for feed in ["stdds", "sfdps", "tfms"] {
            let d = day.join("rec").join(feed);
            std::fs::create_dir_all(&d).unwrap();
            for n in ["0002", "0001"] {
                std::fs::write(
                    d.join(format!("scds-{feed}-20261124T000000Z-{n}.oaf.gz")),
                    b"",
                )
                .unwrap();
            }
        }
        std::fs::create_dir_all(day.join("log")).unwrap();
        std::fs::write(day.join("log").join("run.log"), b"").unwrap();
        let t = t.canonicalize().unwrap();
        let day = t.join("day-20261124");
        let g = crate::guard::tests::guard(&t);
        for dir in [day.clone(), day.join("rec").join("stdds")] {
            let names: Vec<String> = recording_files_in(&g, &dir)
                .unwrap()
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                names,
                [
                    "scds-stdds-20261124T000000Z-0001.oaf.gz",
                    "scds-stdds-20261124T000000Z-0002.oaf.gz"
                ]
            );
        }
        std::fs::remove_dir_all(&t).unwrap();
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
        // Nothing follows the cut frame, so it has no receive time to be told by: it is dropped
        // and counted in the drop line, not as damage (OA-SEAL-014).
        assert_eq!((rs.bad_frames, rs.sealed_window_dropped), (0, 1));
    }

    /// Three status frames received at `t`, the middle one damaged (a payload byte flipped).
    fn damaged_middle(t: [i64; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut at = Vec::new();
        for (i, t) in t.into_iter().enumerate() {
            let mut buf = vec![0u8; MAX_FRAME_LEN];
            let n = encode_frame(&mut buf, 3, 0, i as u64, t, 0, b"xxxx").unwrap();
            at.push(out.len());
            out.extend_from_slice(&buf[..n]);
        }
        out[at[1] + 37] ^= 0xff;
        out
    }

    // REQ: OA-SEAL-014
    #[test]
    fn a_damaged_frame_is_gated_by_the_receive_times_of_the_good_frames_either_side() {
        const FROM: i64 = 1_795_131_900_000_000_000;
        let out = FROM - 3_600_000_000_000;
        let read = |t: [i64; 3]| {
            let mut c = Census::new();
            let rs = read_bytes(&damaged_middle(t), &mut c).unwrap();
            (rs.other_frames, rs.bad_frames, rs.sealed_window_dropped)
        };
        // Both neighbours outside the window: it is damage.
        assert_eq!(read([out, out, out + 1]), (2, 1, 0));
        // Either neighbour inside it: the damaged frame is dropped, with that neighbour.
        assert_eq!(read([out, out, FROM]), (1, 0, 2));
        assert_eq!(read([FROM, out, out]), (1, 0, 2));
    }

    // REQ: OA-CEN-005
    #[test]
    fn a_rec_folder_or_a_day_without_rec_stdds_is_refused_and_other_feeds_are_not_read() {
        let t = std::env::temp_dir().join(format!("oa-census-lay2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let day = t.join("day");
        for feed in ["sfdps", "stdds"] {
            std::fs::create_dir_all(day.join("rec").join(feed)).unwrap();
        }
        std::fs::write(
            day.join("rec").join("sfdps").join("scds-sfdps-1-0001.oaf"),
            b"",
        )
        .unwrap();
        // Only `scds-stdds-*` files count in the layout.
        let stdds = day.join("rec").join("stdds");
        std::fs::write(stdds.join("scds-stdds-1-0001.oaf"), b"").unwrap();
        std::fs::write(stdds.join("other-0001.oaf"), b"").unwrap();
        let only = t.join("onlyrec");
        std::fs::create_dir_all(only.join("rec").join("sfdps")).unwrap();
        std::fs::write(
            only.join("rec").join("sfdps").join("scds-sfdps-1-0001.oaf"),
            b"",
        )
        .unwrap();
        let t = t.canonicalize().unwrap();
        let g = crate::guard::tests::guard(&t);
        let n = |d: &Path| recording_files_in(&g, d);
        assert_eq!(n(&t.join("day")).unwrap().len(), 1);
        assert!(matches!(n(&t.join("onlyrec")), Err(ReadError::Layout(_))));
        assert!(matches!(
            n(&t.join("day").join("rec")),
            Err(ReadError::Layout(_))
        ));
        std::fs::remove_dir_all(&t).unwrap();
    }

    // REQ: OA-CEN-007
    #[test]
    fn a_sealed_named_subdirectory_inside_an_unsealed_walk_is_refused() {
        let t = std::env::temp_dir().join(format!("oa-census-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let sub = t.join("x").join("scds").join("stdds-20261119-holdout");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("f.oaf"), b"").unwrap();
        let t = t.canonicalize().unwrap();
        let g = crate::guard::tests::guard(&t.join("elsewhere"));
        // The top directory is innocent; only the per-directory check in the walk can refuse.
        assert!(refuse_sealed(&g, &t).is_ok());
        assert!(matches!(
            recording_files_in(&g, &t),
            Err(ReadError::Refused(_))
        ));
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[cfg(unix)]
    // REQ: OA-CEN-007
    #[test]
    fn a_rec_stdds_link_into_sealed_data_is_refused() {
        let t = std::env::temp_dir().join(format!("oa-census-recl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let sealed = t.join("scds").join("stdds-20261119-holdout");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::write(sealed.join("scds-stdds-1-0001.oaf"), b"").unwrap();
        std::fs::create_dir_all(t.join("day").join("rec")).unwrap();
        let t = t.canonicalize().unwrap();
        std::os::unix::fs::symlink(
            t.join("scds").join("stdds-20261119-holdout"),
            t.join("day").join("rec").join("stdds"),
        )
        .unwrap();
        let g = crate::guard::tests::guard(&t);
        let mut c = Census::new();
        assert!(matches!(
            read_dir_in(&g, &t.join("day"), &mut c),
            Err(ReadError::Refused(_))
        ));
        std::fs::remove_dir_all(&t).unwrap();
    }

    // REQ: OA-SEAL-014
    #[test]
    fn frames_at_the_window_edges_are_kept_or_dropped_and_counted() {
        // The list's window: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes).
        const FROM: i64 = 1_795_131_900_000_000_000;
        const TO: i64 = 1_795_393_920_000_000_000;
        let m = msg("ZZA", &[(1, "2026-11-19T12:00:00Z", None)]).into_bytes();
        let mut bytes = Vec::new();
        for (i, t) in [FROM - 1, FROM, TO - 1, TO].into_iter().enumerate() {
            let mut buf = vec![0u8; MAX_FRAME_LEN];
            let n =
                encode_frame(&mut buf, FrameKind::Message as u8, 0, i as u64, t, 0, &m).unwrap();
            bytes.extend_from_slice(&buf[..n]);
        }
        let mut census = Census::new();
        let rs = read_bytes(&bytes, &mut census).unwrap();
        assert_eq!((rs.frames, rs.sealed_window_dropped), (2, 2));
        assert_eq!(rs.tais_messages, 2);
    }

    // REQ: OA-SEAL-014
    #[test]
    fn a_record_timed_in_the_window_is_dropped_though_its_frame_was_received_before_it() {
        const FROM: i64 = 1_795_131_900_000_000_000;
        // One message, four records: mrtTime at from - 1 s, at from, at to - 1 s and at to. The
        // two inside the window drop the whole message, not just themselves.
        let m = msg(
            "ZZA",
            &[
                (1, "2026-11-19T23:44:59Z", None),
                (2, "2026-11-19T23:45:00Z", None),
                (3, "2026-11-23T00:31:59Z", None),
                (4, "2026-11-23T00:32:00Z", None),
            ],
        )
        .into_bytes();
        let kept = msg("ZZA", &[(5, "2026-11-19T23:44:59Z", None)]).into_bytes();
        let mut bytes = Vec::new();
        // The frames were received at from - 1 ns, outside the window.
        for (i, x) in [&m, &kept].into_iter().enumerate() {
            let mut buf = vec![0u8; MAX_FRAME_LEN];
            let n = encode_frame(
                &mut buf,
                FrameKind::Message as u8,
                0,
                i as u64,
                FROM - 1,
                0,
                x,
            )
            .unwrap();
            bytes.extend_from_slice(&buf[..n]);
        }
        let mut census = Census::new();
        let rs = read_bytes(&bytes, &mut census).unwrap();
        // Only the second message is counted; the first is in no count but the drop (4 records).
        assert_eq!((rs.frames, rs.tais_messages, rs.tais_records), (1, 1, 1));
        assert_eq!(rs.sealed_window_dropped, 4);
        assert_eq!(rs.stats.records, 1);
    }

    // REQ: OA-SEAL-014
    #[test]
    fn a_status_or_reject_frame_received_in_the_window_is_dropped_and_not_counted_by_kind() {
        const FROM: i64 = 1_795_131_900_000_000_000;
        const TO: i64 = 1_795_393_920_000_000_000;
        let mut bytes = Vec::new();
        // Status (3) and Reject (2) frames at the edges: from - 1 and to are kept, from and
        // to - 1 are in the window.
        for (i, (kind, t)) in [
            (3u8, FROM - 1),
            (3, FROM),
            (2, TO - 1),
            (2, TO),
            (3, FROM + 7),
        ]
        .into_iter()
        .enumerate()
        {
            let mut buf = vec![0u8; MAX_FRAME_LEN];
            let n = encode_frame(&mut buf, kind, 0, i as u64, t, 0, b"x").unwrap();
            bytes.extend_from_slice(&buf[..n]);
        }
        let mut census = Census::new();
        let rs = read_bytes(&bytes, &mut census).unwrap();
        assert_eq!((rs.other_frames, rs.sealed_window_dropped), (2, 3));
    }
}

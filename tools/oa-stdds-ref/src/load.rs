//! Reading STDDS recordings (`.oaf`, raw or gzip) into [`Rec`]s, and the sealed-path refusal.

use std::convert::Infallible;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use oa_feed::frame::{Kind as FrameKind, MAX_FRAME_LEN};
use oa_feed::reader::FrameReader;
use oa_feed::replay::REPLAY_LIMITS;
use oa_sha256::{to_hex, Sha256};
use oa_stdds::tais::tais_bit;
use oa_stdds::{
    new_stats, parse_message, Limits, Message, Stats, TaisField, TaisMessage, TaisRecord,
};
use oa_xml::{Limits as XmlLimits, Parser};
use oa_zip::GzipReader;

use crate::model::{classify_record, Rec};

/// Recordings under `scds/` that are sealed hold-outs or smoke sets and are never read.
const SEALED: [&str; 3] = [
    "rec-20261119",
    "stdds-20261119-holdout",
    "stdds-smoke-20261119",
];

/// Whether the components of `p` contain `scds` immediately followed by a sealed name.
fn names_sealed(p: &Path) -> bool {
    let c: Vec<_> = p
        .components()
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    c.windows(2)
        .any(|w| matches!(w, [a, b] if a == "scds" && SEALED.iter().any(|s| b == *s)))
}

/// Whether `path` names a sealed folder as given, or has a prefix that does with links resolved
/// (a link to the file, to a folder above it, or to another link). Nothing is opened.
pub fn is_sealed(path: &Path) -> bool {
    let mut bad = names_sealed(path);
    let mut prefix = PathBuf::new();
    for comp in path.components() {
        prefix.push(comp);
        if let Ok(real) = std::fs::canonicalize(&prefix) {
            bad |= names_sealed(&real);
        }
    }
    bad
}

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
}

fn drain(reader: &mut FrameReader<'_>, ctx: &mut Ctx<'_>) {
    while let Some(ev) = reader.next() {
        match ev {
            Ok(f) if f.kind == FrameKind::Message => {
                ctx.info.frames_message += 1;
                match parse_message(&mut ctx.parser, f.payload, &ctx.limits, &mut ctx.stats) {
                    Ok(Message::Tais(m)) => {
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
            Err(_) => ctx.info.frames_damaged += 1,
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

/// Reads a recording from `src` (raw or gzip, decided by the gzip magic), appending its TAIS
/// records to `out`. `path` is only the label of the result.
pub fn load_reader<R: Read>(
    mut src: R,
    path: &str,
    file: u32,
    out: &mut Vec<Rec>,
) -> io::Result<FileInfo> {
    let parser =
        Parser::new(XmlLimits::default()).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut ctx = Ctx {
        parser,
        stats: new_stats(),
        limits: Limits::default(),
        file,
        out,
        info: FileInfo {
            path: path.to_string(),
            sha256: String::new(),
            frames_message: 0,
            frames_other: 0,
            frames_damaged: 0,
            messages_not_tais: 0,
            gzip_damaged: false,
            gzip_truncated: false,
            records: 0,
        },
    };
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut reader =
        FrameReader::new(&mut storage).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut hash = Sha256::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut n = src.read(&mut chunk)?;
    let mut gz = (n >= 2 && chunk.first() == Some(&0x1f) && chunk.get(1) == Some(&0x8b))
        .then(|| Box::new(GzipReader::new(REPLAY_LIMITS)));
    while n > 0 {
        let Some(data) = chunk.get(..n) else { break };
        hash.update(data)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        match gz.as_mut() {
            None => feed(&mut reader, &mut ctx, data),
            Some(g) if !ctx.info.gzip_damaged => {
                let r = g.push(data, &mut |b: &[u8]| -> Result<(), Infallible> {
                    feed(&mut reader, &mut ctx, b);
                    Ok(())
                });
                ctx.info.gzip_damaged = r.is_err();
            }
            Some(_) => {}
        }
        n = src.read(&mut chunk)?;
    }
    drain(&mut reader, &mut ctx);
    if reader.finish().is_some() {
        ctx.info.frames_damaged += 1;
    }
    ctx.info.gzip_truncated = gz.as_ref().is_some_and(|g| g.finish().truncated.is_some());
    ctx.info.sha256 = to_hex(&hash.finalize());
    Ok(ctx.info)
}

/// [`load_reader`] on the file at `path`. The caller has already refused sealed paths.
pub fn load_file(path: &Path, file: u32, out: &mut Vec<Rec>) -> io::Result<FileInfo> {
    let f = std::fs::File::open(path)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    load_reader(f, &path.display().to_string(), file, out)
}

//! Counts-only summary of the STDDS messages in a recorded feed: `stdds_summary FILE.oaf[.gz]`.
//! Shell tool: reads the file (raw or gzip, decided by the gzip magic), frames it with oa-feed's
//! reader, parses every message frame and prints per-kind counts, parse-error counts and
//! per-field presence counts. It prints no position, identity, time or other message value;
//! the unknown-path and unknown-root lines are element and namespace names (schema text).
//! Paths under `scds/rec-20261119`, `scds/stdds-20261119-holdout` and `scds/stdds-smoke-20261119` are refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_feed::frame::{Kind as FrameKind, MAX_FRAME_LEN};
use oa_feed::reader::FrameReader;
use oa_feed::replay::REPLAY_LIMITS;
use oa_stdds::{
    new_stats, parse_message, std_error_index, Limits, Stats, ERROR_KIND_NAMES, KIND_NAMES,
    SMES_FIELD_NAMES, STD_ERROR_NAMES, TAIS_FIELD_NAMES,
};
use oa_xml::{Limits as XmlLimits, Parser};
use oa_zip::GzipReader;
use std::convert::Infallible;
use std::io::Read;

struct Ctx {
    parser: Parser,
    stats: Stats,
    limits: Limits,
    frames: u64,
    other_frames: u64,
    bad_frames: u64,
    refused: [u64; 4],
}

fn drain(reader: &mut FrameReader<'_>, ctx: &mut Ctx) {
    while let Some(ev) = reader.next() {
        match ev {
            Ok(f) if f.kind == FrameKind::Message => {
                ctx.frames += 1;
                if let Err(e) =
                    parse_message(&mut ctx.parser, f.payload, &ctx.limits, &mut ctx.stats)
                {
                    if let Some(c) = ctx.refused.get_mut(std_error_index(&e)) {
                        *c += 1;
                    }
                }
            }
            Ok(_) => ctx.other_frames += 1,
            Err(_) => ctx.bad_frames += 1,
        }
    }
}

fn feed(reader: &mut FrameReader<'_>, ctx: &mut Ctx, data: &[u8]) {
    let mut off = 0usize;
    while off < data.len() {
        let took = reader.push(&data[off..]);
        off += took;
        drain(reader, ctx);
        if took == 0 {
            break;
        }
    }
}

/// Recordings under `scds/` that are sealed hold-outs or smoke sets and are never read.
const SEALED: [&str; 3] = [
    "rec-20261119",
    "stdds-20261119-holdout",
    "stdds-smoke-20261119",
];

/// Whether the components of `p` contain `scds` immediately followed by a sealed name.
fn names_sealed(p: &std::path::Path) -> bool {
    let c: Vec<_> = p
        .components()
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    c.windows(2)
        .any(|w| w[0] == "scds" && SEALED.iter().any(|s| w[1] == *s))
}

/// Refuses (exit 2, before the file is opened) a path that names a sealed folder as given, or
/// whose every prefix, with links resolved, does.
fn refuse_sealed(path: &str) {
    let given = std::path::Path::new(path);
    let mut bad = names_sealed(given);
    let mut prefix = std::path::PathBuf::new();
    for comp in given.components() {
        prefix.push(comp);
        if let Ok(real) = std::fs::canonicalize(&prefix) {
            bad |= names_sealed(&real);
        }
    }
    if bad {
        eprintln!("refused: the sealed hold-out recording is not read");
        std::process::exit(2);
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: stdds_summary FILE.oaf[.gz]");
        std::process::exit(2);
    });
    refuse_sealed(&path);
    let mut src = std::fs::File::open(&path).unwrap();
    let mut ctx = Ctx {
        parser: Parser::new(XmlLimits::default()).unwrap(),
        stats: new_stats(),
        limits: Limits::default(),
        frames: 0,
        other_frames: 0,
        bad_frames: 0,
        refused: [0; 4],
    };
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut reader = FrameReader::new(&mut storage).unwrap();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut n = src.read(&mut chunk).unwrap();
    let mut gz =
        (n >= 2 && chunk[..2] == [0x1f, 0x8b]).then(|| Box::new(GzipReader::new(REPLAY_LIMITS)));
    let mut gz_damaged = false;
    while n > 0 {
        match gz.as_mut() {
            None => feed(&mut reader, &mut ctx, &chunk[..n]),
            Some(g) if !gz_damaged => {
                let r = g.push(&chunk[..n], &mut |b: &[u8]| -> Result<(), Infallible> {
                    feed(&mut reader, &mut ctx, b);
                    Ok(())
                });
                gz_damaged = r.is_err();
            }
            Some(_) => {}
        }
        n = src.read(&mut chunk).unwrap();
    }
    drain(&mut reader, &mut ctx);
    if reader.finish().is_some() {
        ctx.bad_frames += 1;
    }
    let truncated = gz.as_ref().is_some_and(|g| g.finish().truncated.is_some());

    let s = &ctx.stats;
    println!(
        "frames: message {} other {} damaged {}",
        ctx.frames, ctx.other_frames, ctx.bad_frames
    );
    println!("gzip: damaged {gz_damaged} truncated {truncated}");
    println!("messages parsed: {}", s.messages);
    for (name, c) in KIND_NAMES.iter().zip(&s.kinds) {
        println!("kind {name}: {c}");
    }
    for (name, c) in STD_ERROR_NAMES.iter().zip(&s.errors) {
        println!("refused whole, {name}: {c}");
    }
    println!("refused whole (this run): {:?}", ctx.refused);
    println!(
        "records: {} with errors {} empty {}",
        s.records, s.records_with_errors, s.empty_records
    );
    println!("fields invalidated by sender: {}", s.invalidated);
    for (name, c) in ERROR_KIND_NAMES.iter().zip(&s.field_error_kinds) {
        println!("field error {name}: {c}");
    }
    for (i, name) in TAIS_FIELD_NAMES.iter().enumerate() {
        println!(
            "tais field {name}: present {} refused {}",
            s.tais_present[i], s.tais_errors[i]
        );
    }
    for (i, name) in SMES_FIELD_NAMES.iter().enumerate() {
        println!(
            "smes field {name}: present {} refused {}",
            s.smes_present[i], s.smes_errors[i]
        );
    }
    for u in &s.unknown {
        println!("unknown element {}: {}", u.text, u.count);
    }
    println!("unknown element overflow: {}", s.unknown_overflow);
    for (t, c) in &s.unknown_roots {
        println!("unknown root {t}: {c}");
    }
    println!("unknown root overflow: {}", s.unknown_roots_overflow);
}

//! Counts-only summary of the STDDS messages in a recorded feed: `stdds_summary FILE.oaf[.gz]`.
//! Shell tool: reads the file (raw or gzip, decided by the gzip magic), frames it with oa-feed's
//! reader, parses every message frame and prints per-kind counts, parse-error counts and
//! per-field presence counts. It prints no position, identity, time or other message value;
//! the unknown-path and unknown-root lines are element and namespace names (schema text).
//! Paths into the sealed hold-out data (`oa-sealed`, `crates/oa-sealed/sealed_paths.txt`) are refused.
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
    new_stats, parse_message, std_error_index, Limits, Message, Stats, ERROR_KIND_NAMES,
    KIND_NAMES, LATENCY_BUCKET_NAMES, SMES_FIELD_NAMES, STD_ERROR_NAMES, TAIS_FIELD_NAMES,
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
    /// Message frames received in a sealed window (OA-SEAL-014): dropped, counted.
    sealed_window_dropped: u64,
    refused: [u64; 4],
    /// Bytes the frame reader would not take after a drain (a stall): counted, never dropped
    /// silently; the run exits non-zero.
    stalled_bytes: u64,
}

/// Whether a record's own time (Unix milliseconds) is in a sealed window.
fn sealed_ms(t: i64) -> bool {
    oa_sealed::in_sealed_window(t.saturating_mul(1_000_000))
}

fn drain(reader: &mut FrameReader<'_>, ctx: &mut Ctx) {
    while let Some(ev) = reader.next() {
        match ev {
            Ok(f)
                if f.kind == FrameKind::Message && oa_sealed::in_sealed_window(f.recv_unix_ns) =>
            {
                ctx.sealed_window_dropped += 1;
                oa_sealed::note_window_drops(1);
            }
            Ok(f) if f.kind == FrameKind::Message => {
                // Parsed into a scratch accumulator: a message with a record timed in a sealed
                // window is dropped whole (its records counted as dropped, nothing merged), so
                // no field, flag, latency or record count includes a hold-out row.
                let mut scratch = new_stats();
                match parse_message(&mut ctx.parser, f.payload, &ctx.limits, &mut scratch) {
                    Ok(m) => {
                        let (n, sealed) = match &m {
                            Message::Tais(t) => (
                                t.records.len(),
                                t.records.iter().any(|r| sealed_ms(r.mrt_time_ms)),
                            ),
                            Message::Smes(t) => (
                                t.records.len(),
                                t.records.iter().any(|r| sealed_ms(r.time_ms)),
                            ),
                            _ => (0, false),
                        };
                        if sealed {
                            ctx.sealed_window_dropped += n as u64;
                            oa_sealed::note_window_drops(n as u64);
                        } else {
                            ctx.frames += 1;
                            ctx.stats.merge(&scratch, &ctx.limits);
                        }
                    }
                    Err(e) => {
                        ctx.frames += 1;
                        ctx.stats.merge(&scratch, &ctx.limits);
                        if let Some(c) = ctx.refused.get_mut(std_error_index(&e)) {
                            *c += 1;
                        }
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
            ctx.stalled_bytes += (data.len() - off) as u64;
            break;
        }
    }
}

/// Whether `t` is 1 to `max` ASCII letters and digits and the `extra` characters.
fn safe_text(t: &str, extra: &[u8], max: usize) -> bool {
    !t.is_empty()
        && t.len() <= max
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: stdds_summary FILE.oaf[.gz]");
        std::process::exit(2);
    });
    // The shared sealed hold-out guard (`oa-sealed`): the path is resolved and checked, the file
    // is opened and the opened file is checked again.
    let mut src = match oa_sealed::open(std::path::Path::new(&path)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("refused or unreadable: {path}: {e}");
            std::process::exit(2);
        }
    };
    let mut ctx = Ctx {
        parser: Parser::new(XmlLimits::default()).unwrap(),
        stats: new_stats(),
        limits: Limits::default(),
        frames: 0,
        other_frames: 0,
        bad_frames: 0,
        sealed_window_dropped: 0,
        refused: [0; 4],
        stalled_bytes: 0,
    };
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut reader = FrameReader::new(&mut storage).unwrap();
    let mut chunk = vec![0u8; 64 * 1024];
    // Read until two bytes (the gzip magic) or the end of the file.
    let mut n = 0;
    while n < 2 {
        let got = src.read(&mut chunk[n..]).unwrap();
        if got == 0 {
            break;
        }
        n += got;
    }
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
    println!(
        "sealed-window rows dropped: {} (frames by receive time; TAIS records by mrtTime and ASDE-X reports by time, dropping the whole message)",
        ctx.sealed_window_dropped
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
            "tais field {name}: present {} present_empty {} refused {}",
            s.tais_present[i], s.tais_present_empty[i], s.tais_errors[i]
        );
    }
    for (i, name) in SMES_FIELD_NAMES.iter().enumerate() {
        println!(
            "smes field {name}: present {} present_empty {} refused {}",
            s.smes_present[i], s.smes_present_empty[i], s.smes_errors[i]
        );
    }
    // Refused-field diagnostics: the text's length and character classes, never a value.
    for h in &s.shapes {
        println!(
            "refused shape {} {} len {} classes {}: {}",
            h.field,
            ERROR_KIND_NAMES
                .get(h.kind as usize)
                .copied()
                .unwrap_or("?"),
            h.len,
            h.pattern,
            h.count
        );
    }
    println!("refused shape overflow: {}", s.shapes_overflow);
    // Track flags are not identifying; their value histogram is printed.
    for (name, c) in ["pseudo", "frozen", "new", "adsb", "coast"]
        .iter()
        .zip(&s.tais_flags)
    {
        let total = c[0] + c[1];
        println!(
            "tais flag track/{name}: false {} true {} of {total}",
            c[0], c[1]
        );
    }
    let adsb = s.tais_flags[3];
    if adsb[0] + adsb[1] > 0 {
        println!(
            "tais adsb false share: {:.4}",
            adsb[0] as f64 / (adsb[0] + adsb[1]) as f64
        );
    }
    for (name, c) in LATENCY_BUCKET_NAMES.iter().zip(&s.latency) {
        println!("latency recSAFAReceiptTime-recSTARSTimestamp ms {name}: {c}");
    }
    println!("latency missing a time: {}", s.latency_missing);
    println!(
        "distinct recSTARSSrcID: {} overflow {}",
        s.src_ids.len(),
        s.src_ids_overflow
    );
    // Service states are not identifying; the value histogram is printed.
    let mut other = 0u64;
    for (t, c) in &s.status_values {
        if safe_text(t, b" _-=", 130) {
            println!("stdds-status {t}: {c}");
        } else {
            other += c;
        }
    }
    println!("stdds-status other (text outside [A-Za-z0-9 _-]): {other}");
    println!(
        "stdds-status dropped {} value too long {} key too long {} table overflow {}",
        s.status_dropped, s.status_value_too_long, s.status_key_too_long, s.status_table_overflow
    );
    // Element and root names are printed only if they are plain schema text.
    let mut other_names = 0u64;
    for u in &s.unknown {
        if safe_text(&u.text, b" _-:./{}", 120) {
            println!("unknown element {}: {}", u.text, u.count);
        } else {
            other_names += u.count;
        }
    }
    println!("unknown element overflow: {}", s.unknown_overflow);
    for (t, c) in &s.unknown_roots {
        if safe_text(t, b" _-:./{}", 260) {
            println!("unknown root {t}: {c}");
        } else {
            other_names += c;
        }
    }
    println!("unknown names printed as other: {other_names}");
    println!("unknown root overflow: {}", s.unknown_roots_overflow);
    println!(
        "frame reader stalled, bytes not read: {}",
        ctx.stalled_bytes
    );
    if ctx.stalled_bytes > 0 {
        eprintln!("error: the frame reader stalled; the summary is incomplete");
        std::process::exit(3);
    }
}

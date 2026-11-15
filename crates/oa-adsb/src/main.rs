#![forbid(unsafe_code)]
//! `oa-adsb-clip`: streams adsb.lol day-archive parts, keeps the reports inside a box and a time
//! window, and writes them as OAF1 frames (docs/ADSB_PAYLOAD.md), sorted by time. Shell: owns
//! the files, the process and stdout (B3).
//!
//! ```text
//! oa-adsb-clip --out <file|-> [--box lat_min,lat_max,lon_min,lon_max]
//!              [--from <time>] [--to <time>] [--frame-ms <n>] [--allow-truncated] <part>...
//! ```
//!
//! The box is in degrees (default: ZAU, 38.5,46.5,-93.5,-83.5). Times are Unix seconds or
//! `YYYY-MM-DDTHH:MM:SSZ`; the window is `[from, to)`. Parts are the pieces of one split tar in
//! order, or a single tar. A frame holds the reports within `--frame-ms` (1 to 60000, default
//! 1000) of its first report, at most 60 000; its receive time is the first report's time. If the parts end early the tool says so and exits 3 unless
//! `--allow-truncated` is given (the frames written are still valid). A summary goes to
//! standard error; standard output carries frames only, and only with `--out -`.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

use oa_adsb::{
    encode_payload, frame_ranges, reports_extend, reports_len, scan_archive, summarize,
    unknown_value_text, Filter, Reports, ScanEnd, ScanError, DEFAULT_FRAME_MS, FIELD_NAMES,
    MAX_FRAME_MS, MIN_FRAME_MS, REJECT_NAMES, ZAU_BOX,
};
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};

/// Limit: reports held before sorting, 40 Mi (about 2.5 GB). A day over the ZAU box is a few
/// million; hitting this means the box or window is far too wide, and the tool stops with an
/// error rather than dropping reports.
const MAX_HELD_ROWS: usize = 40 * 1024 * 1024;

struct Args {
    out: String,
    filter: Filter,
    frame_ms: u32,
    allow_truncated: bool,
    parts: Vec<String>,
}

fn usage() -> &'static str {
    "usage: oa-adsb-clip --out <file|-> [--box lat_min,lat_max,lon_min,lon_max] [--from <time>] [--to <time>] [--frame-ms <n>] [--allow-truncated] <part>..."
}

/// `YYYY-MM-DDTHH:MM:SSZ` to Unix seconds (proleptic Gregorian, UTC).
fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20
        || b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || b.get(10) != Some(&b'T')
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
        || b.get(19) != Some(&b'Z')
    {
        return None;
    }
    let n = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse().ok() };
    let (y, m, d, hh, mm, ss) = (
        n(0, 4)?,
        n(5, 7)?,
        n(8, 10)?,
        n(11, 13)?,
        n(14, 16)?,
        n(17, 19)?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn parse_time(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .ok()
        .or_else(|| parse_iso(s))
        .ok_or_else(|| format!("bad time `{s}` (Unix seconds or YYYY-MM-DDTHH:MM:SSZ)"))
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = None;
    let mut filter = ZAU_BOX;
    let mut frame_ms = DEFAULT_FRAME_MS;
    let mut allow_truncated = false;
    let mut parts = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--out" => out = Some(value("--out")?),
            "--box" => {
                let v = value("--box")?;
                let f: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                let [la0, la1, lo0, lo1] = f[..] else {
                    return Err("--box needs lat_min,lat_max,lon_min,lon_max".into());
                };
                if !(-90.0..=90.0).contains(&la0)
                    || !(-90.0..=90.0).contains(&la1)
                    || !(-180.0..=180.0).contains(&lo0)
                    || !(-180.0..=180.0).contains(&lo1)
                    || la0 > la1
                    || lo0 > lo1
                {
                    return Err("--box values out of range or reversed".into());
                }
                let e7 = |d: f64| (d * 1e7).round() as i32;
                filter.lat_min_e7 = e7(la0);
                filter.lat_max_e7 = e7(la1);
                filter.lon_min_e7 = e7(lo0);
                filter.lon_max_e7 = e7(lo1);
            }
            "--from" => {
                filter.from_ns = parse_time(&value("--from")?)?.saturating_mul(1_000_000_000)
            }
            "--to" => filter.to_ns = parse_time(&value("--to")?)?.saturating_mul(1_000_000_000),
            "--frame-ms" => {
                let v = value("--frame-ms")?;
                frame_ms = v
                    .parse::<u32>()
                    .ok()
                    .filter(|n| (MIN_FRAME_MS..=MAX_FRAME_MS).contains(n))
                    .ok_or(format!(
                        "--frame-ms must be {MIN_FRAME_MS} to {MAX_FRAME_MS}, got `{v}`"
                    ))?;
            }
            "--allow-truncated" => allow_truncated = true,
            "--help" | "-h" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => parts.push(a.clone()),
        }
    }
    let out = out.ok_or("--out is required")?;
    if parts.is_empty() {
        return Err("no archive parts given".into());
    }
    Ok(Args {
        out,
        filter,
        frame_ms,
        allow_truncated,
        parts,
    })
}

/// Writes `r` (already sorted by time) as frames of at most `frame_ms` span; returns frames
/// written.
fn write_frames(r: &Reports, frame_ms: u32, out: &mut dyn Write) -> Result<u64, String> {
    let mut seq = 0u64;
    let mut payload = Vec::new();
    let mut frame = vec![0u8; MAX_FRAME_LEN];
    for (from, to) in frame_ranges(r, frame_ms) {
        payload.clear();
        encode_payload(&mut payload, r, from, to).map_err(|e| e.to_string())?;
        let t_ns = r.time_ns.get(from).copied().unwrap_or(0);
        let len = encode_frame(
            &mut frame,
            1,
            FLAG_BYTES,
            seq,
            t_ns,
            t_ns / 1_000_000,
            &payload,
        )
        .ok_or("frame does not fit")?;
        out.write_all(frame.get(..len).unwrap_or(&[]))
            .map_err(|e| e.to_string())?;
        seq += 1;
    }
    Ok(seq)
}

/// A new `Reports` with the rows of `src` in the order `ix`.
fn reorder(src: &Reports, ix: &[u32]) -> Reports {
    let mut dst = Reports::default();
    for &i in ix {
        if let Some(row) = oa_adsb::reports_row(src, i as usize) {
            oa_adsb::reports_push(&mut dst, &row);
        }
    }
    dst
}

fn run(a: &Args) -> Result<ExitCode, String> {
    let mut files = Vec::new();
    for p in &a.parts {
        files.push(io::BufReader::with_capacity(
            1 << 20,
            File::open(p).map_err(|e| format!("{p}: {e}"))?,
        ));
    }
    let mut held = Reports::default();
    let res = scan_archive(files, &a.filter, &mut |r: &Reports| -> Result<(), String> {
        if reports_len(&held) + reports_len(r) > MAX_HELD_ROWS {
            return Err(format!(
                "more than {MAX_HELD_ROWS} reports kept; narrow --box or the time window"
            ));
        }
        reports_extend(&mut held, r);
        Ok(())
    });
    let res = match res {
        Ok(r) => r,
        Err(ScanError::Sink(e)) => return Err(e),
        Err(ScanError::Tar(e)) => return Err(e.to_string()),
    };
    let mut ix: Vec<u32> = (0..reports_len(&held) as u32).collect();
    ix.sort_by_key(|&i| {
        (
            held.time_ns.get(i as usize).copied().unwrap_or(0),
            held.icao.get(i as usize).copied().unwrap_or(0),
        )
    });
    let sorted = reorder(&held, &ix);
    drop(held);
    let frames = if a.out == "-" {
        let mut w = BufWriter::new(io::stdout().lock());
        let n = write_frames(&sorted, a.frame_ms, &mut w)?;
        w.flush().map_err(|e| e.to_string())?;
        n
    } else {
        let mut w = BufWriter::new(File::create(&a.out).map_err(|e| format!("{}: {e}", a.out))?);
        let n = write_frames(&sorted, a.frame_ms, &mut w)?;
        w.flush().map_err(|e| e.to_string())?;
        n
    };
    let st = &res.stats;
    let sum = summarize(&sorted);
    eprintln!(
        "traces_read={} traces_refused={} trace_entries={} other_entries={} rows_seen={} outside_box={} outside_time={} kept={} frames={frames}",
        st.traces_read, st.traces_refused, res.trace_entries, res.other_entries, st.rows_seen,
        st.outside_box, st.outside_time, st.kept
    );
    eprintln!("measurements_bad {}", st.bad_measurements);
    eprintln!("reserved_address_rows {}", st.reserved_address);
    let unk: Vec<String> = st
        .unknown_values
        .iter()
        .filter(|v| v.count > 0)
        .map(|v| {
            let f = FIELD_NAMES
                .get(usize::from(v.field))
                .copied()
                .unwrap_or("?");
            format!("{f}={:?}:{}", unknown_value_text(v), v.count)
        })
        .collect();
    eprintln!(
        "unknown_values {} (overflow {})",
        if unk.is_empty() {
            "none".to_string()
        } else {
            unk.join(" ")
        },
        st.unknown_overflow
    );
    let rej: Vec<String> = REJECT_NAMES
        .iter()
        .zip(st.rejected.iter())
        .filter(|(_, &c)| c > 0)
        .map(|(n, c)| format!("{n}={c}"))
        .collect();
    eprintln!(
        "rows_rejected {}",
        if rej.is_empty() {
            "none".to_string()
        } else {
            rej.join(" ")
        }
    );
    eprintln!(
        "aircraft={} nac_p_known={} nac_p_ge_8={} pairs={} jumps={} jitter_pairs={} max_implied_kt={:.0}",
        sum.aircraft, sum.nac_p_known, sum.nac_p_ge_8, sum.pairs, sum.jumps,
        sum.jitter, sum.max_implied_kt
    );
    for s in &res.error_samples {
        eprintln!("refused: {s}");
    }
    match &res.end {
        ScanEnd::Complete => {
            eprintln!("archive: complete");
            Ok(ExitCode::SUCCESS)
        }
        ScanEnd::Truncated(t) => {
            eprintln!(
                "archive: TRUNCATED at byte {} ({:?}, entry `{}`, {} of {} data bytes present)",
                t.offset, t.stage, t.entry, t.data_present, t.entry_size
            );
            Ok(if a.allow_truncated {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(3)
            })
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match parse_args(&args) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("oa-adsb-clip: {e}");
            }
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };
    match run(&parsed) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("oa-adsb-clip: {e}");
            ExitCode::FAILURE
        }
    }
}

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

use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

use oa_adsb::cli::{parse_args, usage, Args};
use oa_adsb::{
    encode_payload, frame_ranges, reports_extend, reports_len, scan_archive, summarize,
    unknown_value_text, Reports, ScanEnd, ScanError, FIELD_NAMES, REJECT_NAMES,
};
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};

/// Limit: reports held before sorting, 40 Mi (about 2.5 GB). A day over the ZAU box is a few
/// million; hitting this means the box or window is far too wide, and the tool stops with an
/// error rather than dropping reports.
const MAX_HELD_ROWS: usize = 40 * 1024 * 1024;

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

/// Writes `<out>.source` next to the clip: each input path and its SHA-256, the window
/// (`--from`, `--to`; `none` when open) and the counts.
fn write_source(a: &Args, st: &oa_adsb::Stats, frames: u64, path: &str) -> Result<(), String> {
    let mut text = String::from("oa-adsb-clip source\n");
    text.push_str(&format!("out: {}\n", a.out));
    for p in &a.parts {
        let f = oa_sealed::open(std::path::Path::new(p)).map_err(|e| format!("{p}: {e}"))?;
        let d = oa_sha256::sha256_reader(f).map_err(|e| format!("{p}: {e:?}"))?;
        text.push_str(&format!("input: {p} sha256 {}\n", oa_sha256::to_hex(&d)));
    }
    let bound = |ns: i64| {
        if ns == i64::MIN || ns == i64::MAX {
            "none".to_string()
        } else {
            ns.to_string()
        }
    };
    text.push_str(&format!(
        "window_from_unix_ns: {}\nwindow_to_unix_ns: {}\n",
        bound(a.filter.from_ns),
        bound(a.filter.to_ns)
    ));
    text.push_str(&format!(
        "rows_seen: {}\noutside_box: {}\noutside_time: {}\nkept: {}\nframes: {frames}\nsealed-window rows dropped: {}\n",
        st.rows_seen, st.outside_box, st.outside_time, st.kept, st.sealed_window_dropped
    ));
    std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))
}

fn run(a: &Args) -> Result<ExitCode, String> {
    let mut files = Vec::new();
    for p in &a.parts {
        files.push(io::BufReader::with_capacity(
            1 << 20,
            oa_sealed::open(std::path::Path::new(p)).map_err(|e| format!("{p}: {e}"))?,
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
    // A report whose time is in a sealed window is never clipped (OA-SEAL-015): the archive reader
    // dropped and counted them, and the run is refused before anything is written.
    let sealed = res.stats.sealed_window_dropped;
    eprintln!("sealed-window rows dropped: {sealed}");
    if sealed > 0 {
        eprintln!(
            "oa-adsb-clip: refused: {sealed} report(s) fall in a sealed window (OA-SEAL-001, crates/oa-sealed/sealed_paths.txt); narrow --from and --to; nothing was written"
        );
        return Ok(ExitCode::from(2));
    }
    // An existing output or sidecar is never replaced or truncated; the clip and the sidecar are
    // written to temporary names and renamed into place once both are complete.
    let side = format!("{}.source", a.out);
    let tmp_out = format!("{}.tmp{}", a.out, std::process::id());
    let tmp_side = format!("{side}.tmp{}", std::process::id());
    if a.out != "-" {
        for p in [&a.out, &side] {
            if std::fs::symlink_metadata(p).is_ok() {
                return Err(format!(
                    "{p}: exists; it is never replaced, remove it first"
                ));
            }
        }
    }
    let frames = if a.out == "-" {
        let mut w = BufWriter::new(io::stdout().lock());
        let n = write_frames(&sorted, a.frame_ms, &mut w)?;
        w.flush().map_err(|e| e.to_string())?;
        n
    } else {
        // seal-allow: creates the clip's own temporary output (create_new); reads nothing
        let mut w = BufWriter::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_out)
                .map_err(|e| format!("{tmp_out}: {e}"))?,
        );
        let n = write_frames(&sorted, a.frame_ms, &mut w)?;
        w.flush().map_err(|e| e.to_string())?;
        n
    };
    let st = &res.stats;
    if a.out != "-" {
        write_source(a, st, frames, &tmp_side)?;
        std::fs::rename(&tmp_out, &a.out).map_err(|e| format!("{}: {e}", a.out))?;
        std::fs::rename(&tmp_side, &side).map_err(|e| format!("{side}: {e}"))?;
    }
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

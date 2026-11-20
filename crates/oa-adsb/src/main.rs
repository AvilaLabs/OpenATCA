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
    encode_payload, frame_ranges, reports_extend, reports_len, scan_archive_counted, summarize,
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
fn write_source(
    a: &Args,
    st: &oa_adsb::Stats,
    frames: u64,
    out: &mut impl Write,
    path: &str,
) -> Result<(), String> {
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
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| format!("{path}: {e}"))
}

/// Creates `path` for writing with `create_new` (`O_EXCL`): an existing file or link, even a
/// dangling one, is refused and never replaced, truncated or followed, and no second name for
/// an existing file can appear.
fn create_new(path: &str) -> Result<std::fs::File, String> {
    // seal-allow: creates the tool's own output with create_new; reads nothing
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))
}

/// Writes the sidecar and then the clip, each created new; `made` lists the files this run
/// created, so a failure can remove those and only those.
fn write_outputs(
    a: &Args,
    st: &oa_adsb::Stats,
    sorted: &Reports,
    side: &str,
    made: &mut Vec<String>,
) -> Result<u64, String> {
    let mut sf = create_new(side)?;
    made.push(side.to_string());
    let mut w = BufWriter::new(create_new(&a.out)?);
    made.push(a.out.clone());
    let n = write_frames(sorted, a.frame_ms, &mut w)?;
    w.flush().map_err(|e| format!("{}: {e}", a.out))?;
    write_source(a, st, n, &mut sf, side)?;
    Ok(n)
}

/// The early refusal of an output or sidecar that is already there (the create is the race-free
/// check; this one only saves the work).
fn precheck(a: &Args, side: &str) -> Result<(), String> {
    for p in [a.out.as_str(), side] {
        if std::fs::symlink_metadata(p).is_ok() {
            return Err(format!(
                "{p}: exists; it is never replaced, remove it first"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Run between the pre-check and the create, so a test can make a name appear in between.
    static BETWEEN_CHECK_AND_CREATE: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Writes the outputs after the pre-check; a failure removes the files this run created.
fn produce(a: &Args, st: &oa_adsb::Stats, sorted: &Reports, side: &str) -> Result<u64, String> {
    #[cfg(test)]
    BETWEEN_CHECK_AND_CREATE.with(|h| {
        if let Some(f) = h.borrow().as_ref() {
            f();
        }
    });
    let mut made = Vec::new();
    match write_outputs(a, st, sorted, side, &mut made) {
        Ok(n) => Ok(n),
        Err(e) => {
            remove_made(&made);
            Err(e)
        }
    }
}

/// Removes the files a failed run created, and no others.
fn remove_made(made: &[String]) {
    for p in made {
        let _ = std::fs::remove_file(p);
    }
}

fn run(a: &Args, drops: &mut oa_sealed::DropReport) -> Result<ExitCode, String> {
    let mut files = Vec::new();
    for p in &a.parts {
        files.push(io::BufReader::with_capacity(
            1 << 20,
            oa_sealed::open(std::path::Path::new(p)).map_err(|e| format!("{p}: {e}"))?,
        ));
    }
    let mut held = Reports::default();
    let mut scanned_sealed = 0u64;
    let res = scan_archive_counted(
        files,
        &a.filter,
        &mut |r: &Reports| -> Result<(), String> {
            if reports_len(&held) + reports_len(r) > MAX_HELD_ROWS {
                return Err(format!(
                    "more than {MAX_HELD_ROWS} reports kept; narrow --box or the time window"
                ));
            }
            reports_extend(&mut held, r);
            Ok(())
        },
        &mut scanned_sealed,
    );
    // Added as soon as the scan returns, so the line has it on a tar or sink error too.
    drops.add(scanned_sealed);
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
    if sealed > 0 {
        eprintln!(
            "oa-adsb-clip: refused: {sealed} report(s) fall in a sealed window (OA-SEAL-001, crates/oa-sealed/sealed_paths.txt); narrow --from and --to; nothing was written"
        );
        return Ok(ExitCode::from(2));
    }
    // An existing output or sidecar is never replaced or truncated: both are created new, the
    // sidecar first, and a failure removes the files this run created and no others.
    let side = format!("{}.source", a.out);
    if a.out != "-" {
        precheck(a, &side)?;
    }
    let st = &res.stats;
    let frames = if a.out == "-" {
        let mut w = BufWriter::new(io::stdout().lock());
        let n = write_frames(&sorted, a.frame_ms, &mut w)?;
        w.flush().map_err(|e| e.to_string())?;
        n
    } else {
        produce(a, st, &sorted, &side)?
    };
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
    // Printed when `main` returns: on every exit after the scan, an error and a refusal included.
    let mut drops = oa_sealed::DropReport::new();
    match run(&parsed, &mut drops) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("oa-adsb-clip: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("oa-adsb-main-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn args(out: &str, part: &str) -> Args {
        parse_args(&["--out".into(), out.into(), part.into()]).unwrap()
    }

    // REQ: OA-SEAL-015
    #[test]
    fn create_new_never_truncates_replaces_or_follows_what_is_there() {
        // Reverting create_new to File::create (or dropping create_new from the options) makes
        // this fail: it calls the creator directly, as a name that appears between the check in
        // `run` and the create would.
        let d = dir("excl");
        let p = d.join("x");
        std::fs::write(&p, "mine").unwrap();
        assert!(create_new(p.to_str().unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mine");
        let l = d.join("l");
        std::os::unix::fs::symlink(d.join("target"), &l).unwrap();
        assert!(create_new(l.to_str().unwrap()).is_err());
        assert!(!d.join("target").exists());
        let n = d.join("new");
        assert!(create_new(n.to_str().unwrap()).is_ok());
        std::fs::remove_dir_all(&d).unwrap();
    }

    // REQ: OA-SEAL-015
    #[test]
    fn a_name_that_appears_between_the_pre_check_and_the_create_is_never_truncated() {
        // Reverting the create to File::create makes this fail: the pre-check passes, the clip
        // name then appears (another process), and the create must refuse it, not truncate it.
        let d = dir("between");
        let out = d.join("c.oaf");
        let side = format!("{}.source", out.display());
        let a = args(out.to_str().unwrap(), "/nonexistent/part");
        precheck(&a, &side).unwrap();
        let theirs = out.clone();
        BETWEEN_CHECK_AND_CREATE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || std::fs::write(&theirs, "theirs").unwrap()));
        });
        let r = produce(&a, &oa_adsb::Stats::default(), &Reports::default(), &side);
        BETWEEN_CHECK_AND_CREATE.with(|h| *h.borrow_mut() = None);
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "theirs");
        assert!(
            !std::path::Path::new(&side).exists(),
            "the sidecar of the failed run is removed"
        );
        // The same for the sidecar's name.
        let out2 = d.join("d.oaf");
        let side2 = format!("{}.source", out2.display());
        let a2 = args(out2.to_str().unwrap(), "/nonexistent/part");
        let theirs2 = side2.clone();
        BETWEEN_CHECK_AND_CREATE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::write(&theirs2, "theirs").unwrap()
            }));
        });
        let r = produce(&a2, &oa_adsb::Stats::default(), &Reports::default(), &side2);
        BETWEEN_CHECK_AND_CREATE.with(|h| *h.borrow_mut() = None);
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&side2).unwrap(), "theirs");
        assert!(!out2.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    // REQ: OA-SEAL-015
    #[test]
    fn a_clip_name_that_appears_after_the_sidecar_is_made_fails_and_only_the_sidecar_is_removed() {
        let d = dir("race");
        let out = d.join("c.oaf");
        let side = format!("{}.source", out.display());
        // The clip appears (another process made it) between the check and the create.
        std::fs::write(&out, "theirs").unwrap();
        let a = args(out.to_str().unwrap(), "/nonexistent/part");
        let mut made = Vec::new();
        let r = write_outputs(
            &a,
            &oa_adsb::Stats::default(),
            &Reports::default(),
            &side,
            &mut made,
        );
        assert!(r.is_err());
        assert_eq!(made.as_slice(), std::slice::from_ref(&side));
        remove_made(&made);
        assert!(!std::path::Path::new(&side).exists());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "theirs");
        std::fs::remove_dir_all(&d).unwrap();
    }

    // REQ: OA-SEAL-015
    #[test]
    fn a_write_failure_after_both_files_exist_removes_both_and_nothing_else() {
        let d = dir("fail");
        let out = d.join("c.oaf");
        let side = format!("{}.source", out.display());
        let other = d.join("other");
        std::fs::write(&other, "keep").unwrap();
        // The sidecar names the input to hash; the input is not there, so its write fails after
        // the clip was written whole.
        let a = args(out.to_str().unwrap(), "/nonexistent/part");
        let mut made = Vec::new();
        let r = write_outputs(
            &a,
            &oa_adsb::Stats::default(),
            &Reports::default(),
            &side,
            &mut made,
        );
        assert!(r.is_err());
        assert_eq!(made.len(), 2);
        remove_made(&made);
        assert!(!out.exists() && !std::path::Path::new(&side).exists());
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "keep");
        std::fs::remove_dir_all(&d).unwrap();
    }
}

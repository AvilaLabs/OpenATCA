#![forbid(unsafe_code)]
//! `oa-truth-check`: builds the ADS-B truth of an OAF1 clip (docs/ADSB_PAYLOAD.md) and prints
//! what it kept, what it removed and why, the leave-one-out accuracy table and the share of
//! query times under the uncertainty gate. Shell: owns the file, the clock and stdout (B3).
//!
//! ```text
//! oa-truth-check <clip.oaf> [--accel-q <v>] [--sigma-scale <v>] [--no-loo]
//! ```

use std::fs::File;
use std::process::ExitCode;
use std::time::Instant;

use oa_adsb::{decode_payload_lenient, reports_len, Reports};
use oa_feed::{read_frames, Kind};
use oa_sha256::{to_hex, Sha256};
use oa_truth::{
    build_truth, truth_hash_into, Dist, Fraction, LooStats, TruthConfig, TruthStats,
    DEFAULT_CONFIG, REMOVE_NAMES, SPLIT_NAMES,
};

fn usage() -> &'static str {
    "usage: oa-truth-check <clip.oaf> [--accel-q <v>] [--sigma-scale <v>] [--no-loo]"
}

/// The value of a numeric option: a finite number above zero (the config check holds the caps).
fn finite_positive(v: Option<&String>) -> Option<f64> {
    let x: f64 = v?.parse().ok()?;
    (x.is_finite() && x > 0.0).then_some(x)
}

/// The largest clip read, in bytes: ten times the 330 MB raw ZAU day (2026-11-14). A larger
/// file is refused before any of it is read.
const MAX_CLIP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The most rows a clip may claim in its frame headers: ten times the rows of the same day
/// (4.07 million), well under [`oa_truth::MAX_REPORTS`].
const MAX_CLIP_ROWS: u64 = 50_000_000;

/// Why a clip was not read.
#[derive(Debug, PartialEq, Eq)]
enum ClipError {
    /// The file could not be opened or read.
    Io(String),
    /// The file is larger than the byte cap.
    TooManyBytes { bytes: u64, cap: u64 },
    /// The frame headers claim more rows than the row cap.
    TooManyRows { rows: u64, cap: u64 },
}

impl std::fmt::Display for ClipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipError::Io(e) => write!(f, "{e}"),
            ClipError::TooManyBytes { bytes, cap } => {
                write!(f, "clip is {bytes} bytes, over the cap of {cap}")
            }
            ClipError::TooManyRows { rows, cap } => {
                write!(f, "clip claims {rows} rows, over the cap of {cap}")
            }
        }
    }
}

/// Reads every ADS-B payload frame of an OAF1 file into one set of reports. Returns the reports
/// the number of frames that did not decode and the number of records dropped from frames that
/// did (a bad record does not refuse its frame). A file over `max_bytes` is refused unread; a
/// frame whose header takes the running row count over `max_rows` is refused before it is decoded.
fn read_clip(path: &str, max_bytes: u64, max_rows: u64) -> Result<(Reports, u64, u64), ClipError> {
    let io = |e: std::io::Error| ClipError::Io(format!("{path}: {e}"));
    let mut f = File::open(path).map_err(io)?;
    let bytes = f.metadata().map_err(io)?.len();
    if bytes > max_bytes {
        return Err(ClipError::TooManyBytes {
            bytes,
            cap: max_bytes,
        });
    }
    let mut out = Reports::default();
    let mut undecodable = 0u64;
    let mut dropped = 0u64;
    let mut rows = 0u64;
    let mut over: Option<u64> = None;
    let damaged = read_frames(&mut f, &mut |fr| {
        if fr.kind == Kind::Message && over.is_none() {
            // The row count is the u32 at byte 8 of the payload header; a payload too short to
            // have one fails the decode below.
            let claimed = fr
                .payload
                .get(8..12)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map_or(0, |b| u64::from(u32::from_le_bytes(b)));
            rows = rows.saturating_add(claimed);
            if rows > max_rows {
                over = Some(rows);
                return;
            }
            match decode_payload_lenient(fr.payload, &mut out) {
                Ok(c) => dropped += c.dropped as u64,
                Err(_) => undecodable += 1,
            }
        }
    })
    .map_err(io)?;
    if let Some(rows) = over {
        return Err(ClipError::TooManyRows {
            rows,
            cap: max_rows,
        });
    }
    Ok((out, damaged + undecodable, dropped))
}

fn dist_row(name: &str, unit: &str, d: &Dist) {
    println!(
        "  {name:<14} {:>9} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>10.2}  {unit}",
        d.n, d.mean, d.rms, d.median, d.p95, d.p99, d.max
    );
}

fn pct(f: &Fraction) -> String {
    if f.n == 0 {
        "n/a".to_string()
    } else {
        format!(
            "{:.2} % ({} of {})",
            100.0 * f.hit as f64 / f.n as f64,
            f.hit,
            f.n
        )
    }
}

fn print_loo(l: &LooStats) {
    println!(
        "leave-one-out: every {}-th report hidden, {} hidden, {} not predicted (no real report within the bracket on both sides)",
        l.stride, l.hidden, l.no_bracket
    );
    println!(
        "  {:<14} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>10}",
        "residual", "N", "mean", "RMS", "median", "p95", "p99", "max"
    );
    dist_row("horizontal", "m", &l.horizontal);
    dist_row("cross-track", "m", &l.cross);
    dist_row("along-track", "m", &l.along);
    dist_row("altitude", "ft", &l.alt);
    println!(
        "  (mean is signed; RMS, median, p95, p99, max are of the absolute value; nearest rank)"
    );
    println!(
        "  predicted 95 % horizontal radius within gate at hidden times: {}",
        pct(&l.under_gate)
    );
    println!(
        "  hidden report inside its predicted 95 % radius (calibration, ~95 % if honest): {}",
        pct(&l.coverage95)
    );
}

fn print_stats(s: &TruthStats, secs: f64) {
    println!("rows in            {}", s.rows_in);
    println!("aircraft in        {}", s.aircraft_in);
    println!("rows kept          {}", s.rows_kept);
    println!("aircraft with truth {}", s.aircraft_out);
    println!("legs               {}", s.legs);
    println!("leg starts by split:");
    for (n, c) in SPLIT_NAMES.iter().zip(s.splits.iter()) {
        println!("  {n:<20} {c}");
    }
    println!("rows removed, by first failing rule:");
    let mut total = 0u64;
    for (n, c) in REMOVE_NAMES.iter().zip(s.removed.iter()) {
        println!("  {n:<20} {c}");
        total += c;
    }
    println!(
        "  {:<20} {total}   (kept + removed = {}, rows in = {})",
        "total",
        s.rows_kept + total,
        s.rows_in
    );
    println!(
        "rows that passed on a carried-forward quality value: {}",
        s.quality_carried
    );
    println!(
        "95 % horizontal radius within gate, at report times:      {}",
        pct(&s.unc_at_reports)
    );
    println!(
        "95 % horizontal radius within gate, at interval midpoints: {}",
        pct(&s.unc_at_midpoints)
    );
    print_loo(&s.loo);
    println!("build time {secs:.2} s");
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg: TruthConfig = DEFAULT_CONFIG;
    let mut path: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--accel-q" => {
                cfg.accel_q = finite_positive(it.next()).ok_or(usage())?;
            }
            "--sigma-scale" => {
                cfg.sigma_scale = finite_positive(it.next()).ok_or(usage())?;
            }
            "--no-loo" => cfg.loo = false,
            s if s.starts_with("--") => return Err(usage().to_string()),
            s => {
                if path.replace(s.to_string()).is_some() {
                    return Err(usage().to_string());
                }
            }
        }
    }
    let path = path.ok_or(usage())?;
    let (reports, bad, dropped) =
        read_clip(&path, MAX_CLIP_BYTES, MAX_CLIP_ROWS).map_err(|e| e.to_string())?;
    println!(
        "clip {path}: {} reports decoded, {bad} frames refused",
        reports_len(&reports)
    );
    if dropped > 0 {
        println!(
            "{dropped} records dropped from frames that decoded (position or flags out of range)"
        );
    }
    let start = Instant::now();
    let (truth, stats) = build_truth(&reports, &cfg).map_err(|e| e.to_string())?;
    let secs = start.elapsed().as_secs_f64();
    print_stats(&stats, secs);
    let mut h = Sha256::new();
    truth_hash_into(&mut h, &truth);
    println!("truth digest {}", to_hex(&h.finalize()));
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-truth-check: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use oa_adsb::PAYLOAD_MAGIC;
    use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};

    /// One OAF1 frame whose payload is just a header claiming `claimed` rows (no records).
    fn header_only_frame(claimed: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 16];
        payload[..4].copy_from_slice(&PAYLOAD_MAGIC);
        payload[4..6].copy_from_slice(&80u16.to_le_bytes());
        payload[8..12].copy_from_slice(&claimed.to_le_bytes());
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, 0, 0, 0, &payload).unwrap();
        buf.truncate(n);
        buf
    }

    fn write(name: &str, bytes: &[u8]) -> String {
        let p = std::env::temp_dir().join(format!("oa_truth_{}_{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().to_string()
    }

    // REQ: OA-TRUTH-045
    #[test]
    fn a_header_claiming_more_rows_than_the_cap_is_refused_before_decoding() {
        let path = write("rows_over_cap.oaf", &header_only_frame(1001));
        // The payload is not a whole frame of 1001 records, so decoding it would fail: the
        // refusal is the row cap, not a decode error.
        assert_eq!(
            read_clip(&path, 1 << 20, 1000).unwrap_err(),
            ClipError::TooManyRows {
                rows: 1001,
                cap: 1000
            }
        );
    }

    // REQ: OA-TRUTH-045
    #[test]
    fn a_file_over_the_byte_cap_is_refused_unread() {
        let path = write("bytes_over_cap.oaf", &header_only_frame(0));
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!(
            read_clip(&path, len - 1, 1000).unwrap_err(),
            ClipError::TooManyBytes {
                bytes: len,
                cap: len - 1
            }
        );
        // At the cap it is read.
        assert!(read_clip(&path, len, 1000).is_ok());
    }
}

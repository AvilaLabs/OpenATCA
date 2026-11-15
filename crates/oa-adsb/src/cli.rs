//! Argument parsing of the `oa-adsb-clip` binary, kept in the library so it can be tested.
//! Core (pure).

use crate::{Filter, DEFAULT_FRAME_MS, MAX_FRAME_MS, MIN_FRAME_MS, ZAU_BOX};

/// The parsed command line. Plain record (A1).
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Args {
    /// Output file, or `-` for standard output.
    pub out: String,
    /// Box and time window.
    pub filter: Filter,
    /// Frame time span, milliseconds.
    pub frame_ms: u32,
    /// Exit 0 although the archive parts ended early.
    pub allow_truncated: bool,
    /// Archive parts, in order.
    pub parts: Vec<String>,
}

/// The usage line.
pub fn usage() -> &'static str {
    "usage: oa-adsb-clip --out <file|-> [--box lat_min,lat_max,lon_min,lon_max] [--from <time>] [--to <time>] [--frame-ms <n>] [--allow-truncated] <part>..."
}

/// `YYYY-MM-DDTHH:MM:SSZ` to Unix seconds (proleptic Gregorian, UTC).
pub fn parse_iso(s: &str) -> Option<i64> {
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

/// Unix seconds, or `YYYY-MM-DDTHH:MM:SSZ`, as Unix seconds.
pub fn parse_time(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .ok()
        .or_else(|| parse_iso(s))
        .ok_or_else(|| format!("bad time `{s}` (Unix seconds or YYYY-MM-DDTHH:MM:SSZ)"))
}

/// Parses the arguments after the program name; the error is a message, empty for `--help`.
pub fn parse_args(args: &[String]) -> Result<Args, String> {
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

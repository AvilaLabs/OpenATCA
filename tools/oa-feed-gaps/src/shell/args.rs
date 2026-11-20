//! Shell (B1): the command line.

use std::path::PathBuf;

use crate::core::gaps::Window;
use crate::core::timefmt::parse_iso;

pub const USAGE: &str = "usage: oa-feed-gaps --sfdps-dir <dir> --adsb <clip.oaf> --from <ISO time> --to <ISO time> [--clip-log <file>] [--out <file>]";

/// What a run is asked to do.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Args {
    pub sfdps_dir: PathBuf,
    pub adsb: PathBuf,
    pub window: Window,
    pub clip_log: Option<PathBuf>,
    pub out: Option<PathBuf>,
}

/// Parses the arguments after the program name; the error text is the usage failure.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let (mut sfdps, mut adsb, mut from, mut to, mut out) = (None, None, None, None, None);
    let mut clip_log = None;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let v = it.next().ok_or_else(|| format!("{a} needs a value"))?;
        let slot = match a.as_str() {
            "--sfdps-dir" => &mut sfdps,
            "--adsb" => &mut adsb,
            "--from" => &mut from,
            "--to" => &mut to,
            "--out" => &mut out,
            "--clip-log" => &mut clip_log,
            _ => return Err(format!("unknown option {a}")),
        };
        if slot.replace(v.clone()).is_some() {
            return Err(format!("{a} given twice"));
        }
    }
    let need = |o: Option<String>, n: &str| o.ok_or_else(|| format!("{n} is required"));
    let time = |s: String, n: &str| {
        parse_iso(&s).ok_or_else(|| format!("{n}: not YYYY-MM-DDTHH:MM:SSZ: {s}"))
    };
    let from = time(need(from, "--from")?, "--from")?;
    let to = time(need(to, "--to")?, "--to")?;
    let window = Window::new(from, to).ok_or("--from must be before --to")?;
    Ok(Args {
        sfdps_dir: PathBuf::from(need(sfdps, "--sfdps-dir")?),
        adsb: PathBuf::from(need(adsb, "--adsb")?),
        window,
        clip_log: clip_log.map(PathBuf::from),
        out: out.map(PathBuf::from),
    })
}

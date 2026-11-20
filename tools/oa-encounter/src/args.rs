//! The command line: `oa-encounter --adsb <clip.oaf> --out <dir> [--scorer-half]`.

use std::path::PathBuf;

pub const USAGE: &str =
    "usage: oa-encounter --adsb <clip.oaf> --out <dir> [--scorer-half] [--nasr <NASR_CSV.zip>]\n       [--from <unix s> --to <unix s>] [--sfdps <recording dir> [--lag-floor-s <s>]] [--pick-newest-plan] [--hours]";

/// The parsed arguments.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub adsb: PathBuf,
    pub out: PathBuf,
    /// Track only the scorer's half A of the clip (the reports the scoring harness gives the
    /// tracker), not every report.
    pub scorer_half: bool,
    /// The NASR CSV archive whose `APT_BASE.csv` gives the public airports of the surface rule.
    pub nasr: Option<PathBuf>,
    /// Only the reports from `from_s` up to (excluding) `to_s`, Unix seconds.
    pub window: Option<(i64, i64)>,
    /// The SFDPS recordings to join the flight record from, and the lag floor to apply, seconds.
    pub sfdps: Option<PathBuf>,
    pub lag_floor_s: i64,
    /// Where every flying flight with the callsign shares its departure and arrival, take the one
    /// with the newest plan stamp.
    pub pick_newest: bool,
    /// Print the reports of the clip by UTC hour and stop.
    pub hours: bool,
}

/// Parses the arguments after the program name. A flag value that starts with `--` is a missing
/// value; a repeated flag, an unknown flag and a missing flag are errors.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let (mut adsb, mut out, mut nasr): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) =
        (None, None, None);
    let mut scorer_half = false;
    let mut hours = false;
    let mut pick_newest = false;
    let (mut sfdps, mut from, mut to, mut lag): (
        Option<PathBuf>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = (None, None, None, None);
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        let slot = match flag.as_str() {
            "--scorer-half" => {
                if std::mem::replace(&mut scorer_half, true) {
                    return Err("--scorer-half given twice".to_string());
                }
                continue;
            }
            "--pick-newest-plan" => {
                if std::mem::replace(&mut pick_newest, true) {
                    return Err("--pick-newest-plan given twice".to_string());
                }
                continue;
            }
            "--hours" => {
                if std::mem::replace(&mut hours, true) {
                    return Err("--hours given twice".to_string());
                }
                continue;
            }
            "--from" | "--to" | "--lag-floor-s" => {
                let v = it
                    .next()
                    .and_then(|v| v.parse::<i64>().ok())
                    .ok_or_else(|| format!("{flag} needs a whole number"))?;
                let slot = match flag.as_str() {
                    "--from" => &mut from,
                    "--to" => &mut to,
                    _ => &mut lag,
                };
                if slot.replace(v).is_some() {
                    return Err(format!("{flag} given twice"));
                }
                continue;
            }
            "--sfdps" => &mut sfdps,
            "--adsb" => &mut adsb,
            "--out" => &mut out,
            "--nasr" => &mut nasr,
            _ => return Err(format!("unknown argument {flag}")),
        };
        let value = it
            .next()
            .filter(|v| !v.starts_with("--"))
            .ok_or_else(|| format!("{flag} needs a value"))?;
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    let window = match (from, to) {
        (Some(f), Some(t)) if f < t => Some((f, t)),
        (None, None) => None,
        _ => return Err("--from and --to go together, from before to".to_string()),
    };
    match (adsb, out) {
        (Some(adsb), Some(out)) => Ok(Args {
            adsb,
            out,
            scorer_half,
            nasr,
            window,
            sfdps,
            lag_floor_s: lag.unwrap_or(58),
            pick_newest,
            hours,
        }),
        _ => Err("--adsb and --out are both required".to_string()),
    }
}

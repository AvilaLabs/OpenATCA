//! The command line.

use std::path::PathBuf;

pub const USAGE: &str = "usage: oa-legacy-traj --sfdps <dir> --tbfm <dir> --tfms <dir> --adsb <clip.oaf> [--adsb <clip.oaf> ...] --out <dir>
       [--nasr <zip>] [--places <csv>] [--cut-nm <nm>] [--runway-nm <nm>] [--landing-gap-s <s>] [--step-s <s>]
       oa-legacy-traj --sfdps <dir> --tbfm <dir> --adsb <clip.oaf> --out <dir> --nasr <zip> --arrival-meter-fix <name>
defaults: --cut-nm 2 (the headline crossing cut; sensitivity cuts 0.5, 1, 2 and 5 NM are always run), --runway-nm 5, --landing-gap-s 30, --step-s 5;
          no --nasr and no --places (no point or airport then has a place)
fixed settings: leg-airport distance 10 NM, leg window 3 h, landing persistence 30 s, landing altitude 1000 ft above the airport or 80 m/s, longest look-ahead 120 min, clip box edge margin 10 NM";

/// The parsed arguments.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub sfdps: PathBuf,
    pub tbfm: PathBuf,
    pub tfms: Option<PathBuf>,
    /// The ADS-B clips, in the order given (their reports are put together before truth is built).
    pub adsb: Vec<PathBuf>,
    pub out: PathBuf,
    /// The NASR CSV archive the named points, navaids and airports are read from.
    pub nasr: Option<PathBuf>,
    /// Places that override NASR's (lines `fix,NAME,lat,lon` and `airport,CODE,lat,lon`).
    pub places: Option<PathBuf>,
    /// A single metering fix for the issue-time arrival forecast exercise.
    pub arrival_meter_fix: Option<String>,
    /// A truth path counts as at a point within this, nautical miles.
    pub cut_nm: f64,
    /// A landing counts within this of the airport reference point, nautical miles.
    pub runway_nm: f64,
    /// A landing whose reports have a longer gap than this to the ground one is not timed, s.
    pub landing_gap_s: i64,
    /// The truth path is sampled at this step, s.
    pub step_s: i64,
}

fn number(flag: &str, v: Option<&String>) -> Result<f64, String> {
    v.and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("{flag} needs a positive number"))
}

/// Parses the arguments after the program name. A flag value that starts with `--` is a missing
/// value; a repeated flag (other than `--adsb`), an unknown flag and a missing flag are errors.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let (mut sfdps, mut tbfm, mut tfms): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) =
        (None, None, None);
    let (mut out, mut places, mut nasr): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) =
        (None, None, None);
    let mut adsb: Vec<PathBuf> = Vec::new();
    let mut arrival_meter_fix = None;
    let (mut cut, mut runway, mut gap, mut step): (
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) = (None, None, None, None);
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--arrival-meter-fix" => {
                let fix = it.next().filter(|value| {
                    !value.is_empty()
                        && value.len() <= 8
                        && value.bytes().all(|b| b.is_ascii_alphanumeric())
                });
                let fix = fix.ok_or("--arrival-meter-fix needs a fix identifier")?;
                if arrival_meter_fix
                    .replace(fix.to_ascii_uppercase())
                    .is_some()
                {
                    return Err(format!("{flag} given twice"));
                }
            }
            "--cut-nm" | "--runway-nm" | "--landing-gap-s" | "--step-s" => {
                let v = number(flag, it.next())?;
                let slot = match flag.as_str() {
                    "--cut-nm" => &mut cut,
                    "--runway-nm" => &mut runway,
                    "--landing-gap-s" => &mut gap,
                    _ => &mut step,
                };
                if slot.replace(v).is_some() {
                    return Err(format!("{flag} given twice"));
                }
            }
            "--sfdps" | "--tbfm" | "--tfms" | "--adsb" | "--out" | "--places" | "--nasr" => {
                let value = it
                    .next()
                    .filter(|v| !v.starts_with("--"))
                    .ok_or_else(|| format!("{flag} needs a value"))?;
                let p = PathBuf::from(value);
                let slot = match flag.as_str() {
                    "--sfdps" => &mut sfdps,
                    "--tbfm" => &mut tbfm,
                    "--tfms" => &mut tfms,
                    "--out" => &mut out,
                    "--places" => &mut places,
                    "--nasr" => &mut nasr,
                    _ => {
                        adsb.push(p);
                        continue;
                    }
                };
                if slot.replace(p).is_some() {
                    return Err(format!("{flag} given twice"));
                }
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    let (Some(sfdps), Some(tbfm), Some(out)) = (sfdps, tbfm, out) else {
        return Err("--sfdps, --tbfm and --out are required".to_string());
    };
    if arrival_meter_fix.is_none() && tfms.is_none() {
        return Err("--tfms is required for the legacy comparison".to_string());
    }
    if arrival_meter_fix.is_some() && nasr.is_none() && places.is_none() {
        return Err("--arrival-meter-fix needs --nasr or --places".to_string());
    }
    if adsb.is_empty() {
        return Err("at least one --adsb clip is required".to_string());
    }
    let landing_gap_s = gap.unwrap_or(30.0);
    let step_s = step.unwrap_or(5.0);
    if landing_gap_s.fract() != 0.0 || step_s.fract() != 0.0 {
        return Err("--landing-gap-s and --step-s are whole seconds".to_string());
    }
    Ok(Args {
        sfdps,
        tbfm,
        tfms,
        adsb,
        out,
        nasr,
        places,
        arrival_meter_fix,
        cut_nm: cut.unwrap_or(2.0),
        runway_nm: runway.unwrap_or(5.0),
        landing_gap_s: landing_gap_s as i64,
        step_s: step_s as i64,
    })
}

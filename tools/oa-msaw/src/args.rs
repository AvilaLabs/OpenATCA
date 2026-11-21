//! The command line:
//! `oa-msaw --adsb <clip.oaf> --config <file> --out <dir> [--terrain <dir>] [--dof <zip|dat>]
//! [--mva <dir>] [--cifp <file>] [--metar <dir>] [--sfdps <dir>] [--from-s <n> --to-s <n>] [--scorer-half] [--diagnose] [--landing-of <alerts.csv>]`.

use std::path::PathBuf;

pub const USAGE: &str = "usage: oa-msaw --adsb <clip.oaf> --config <file.cfg> --out <dir> [--terrain <3dep dir>] [--dof <DOF zip or DOF.DAT>] [--mva <dir>] [--cifp <FAACIFP18 or the CIFP zip>] [--metar <dir of metar_NNz_hours1.json, or files, comma separated>] [--sfdps <recording dir>] [--from-s <n> --to-s <n>] [--scorer-half] [--diagnose] [--landing-of <alerts.csv of another run>]";

/// The parsed arguments.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub adsb: PathBuf,
    pub config: PathBuf,
    pub out: PathBuf,
    /// A directory of 3DEP one-arc-second tiles. Without it no point has terrain coverage and the
    /// net says so.
    pub terrain: Option<PathBuf>,
    /// The Digital Obstacle File (the zip that holds `DOF.DAT`, or the file itself). Without it
    /// the net runs on terrain and charts alone and the summary says so.
    pub dof: Option<PathBuf>,
    /// A directory of MVA charts (AIXM). Without it no point has chart coverage and the net says so.
    pub mva: Option<PathBuf>,
    /// The CIFP (`FAACIFP18`, or the zip that holds it), for the corridors of the `cifp_airport`
    /// lines and the airports with an approach.
    pub cifp: Option<PathBuf>,
    /// A directory of METAR files (`metar_00z_hours1.json` to `metar_23z_hours1.json`), a METAR
    /// file, or several of either separated by commas, for the altimeter settings. Without it no altitude below FL180 has a setting and the net says so.
    pub metar: Option<PathBuf>,
    /// The SFDPS recordings, for the flight plans that decide who is alerted. Without them every
    /// track is alerted and the summary says that eligibility was not judged.
    pub sfdps: Option<PathBuf>,
    /// Track only the measurements from `from_s` to `to_s` seconds after the first one.
    pub window_s: Option<(f64, f64)>,
    /// Track only the scorer's half A of the clip (the reports the scoring harness gives the
    /// tracker), not every report.
    pub scorer_half: bool,
    /// Also write `diagnosis.txt`: the altitude, radius, obstacle and terrain behind the alerts
    /// (numbers only, no aircraft named).
    pub diagnose: bool,
    /// The `alerts.csv` of other runs, comma-separated: how many of their alerts have their worst point in the
    /// landing zone, by the same corridors and geometry, written to `landing_of.txt`.
    pub landing_of: Option<String>,
}

fn seconds(flag: &str, v: &str) -> Result<f64, String> {
    let x: f64 = v
        .parse()
        .map_err(|_| format!("{flag}: `{v}` is not a number"))?;
    if x.is_finite() && x >= 0.0 {
        Ok(x)
    } else {
        Err(format!("{flag}: `{v}` is not a time"))
    }
}

/// Parses the arguments after the program name. A flag value that starts with `--` is a missing
/// value; a repeated flag, an unknown flag and a missing flag are errors.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut paths: [Option<PathBuf>; 10] = Default::default();
    let (mut from, mut to): (Option<f64>, Option<f64>) = (None, None);
    let mut scorer_half = false;
    let mut diagnose = false;
    const NAMES: [&str; 10] = [
        "--adsb",
        "--config",
        "--out",
        "--terrain",
        "--dof",
        "--mva",
        "--cifp",
        "--metar",
        "--sfdps",
        "--landing-of",
    ];
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        if flag == "--scorer-half" {
            if std::mem::replace(&mut scorer_half, true) {
                return Err("--scorer-half given twice".to_string());
            }
            continue;
        }
        if flag == "--diagnose" {
            if std::mem::replace(&mut diagnose, true) {
                return Err("--diagnose given twice".to_string());
            }
            continue;
        }
        let value = |it: &mut std::slice::Iter<String>| -> Result<String, String> {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--from-s" | "--to-s" => {
                let v = seconds(flag, &value(&mut it)?)?;
                let slot = if flag == "--from-s" {
                    &mut from
                } else {
                    &mut to
                };
                if slot.replace(v).is_some() {
                    return Err(format!("{flag} given twice"));
                }
            }
            f => {
                let i = NAMES
                    .iter()
                    .position(|n| *n == f)
                    .ok_or_else(|| format!("unknown argument {flag}"))?;
                let v = value(&mut it)?;
                if let Some(slot) = paths.get_mut(i) {
                    if slot.replace(PathBuf::from(v)).is_some() {
                        return Err(format!("{flag} given twice"));
                    }
                }
            }
        }
    }
    let window_s = match (from, to) {
        (None, None) => None,
        (Some(a), Some(b)) if a < b => Some((a, b)),
        (Some(_), Some(_)) => return Err("--from-s must be less than --to-s".to_string()),
        _ => return Err("--from-s and --to-s go together".to_string()),
    };
    let [adsb, config, out, terrain, dof, mva, cifp, metar, sfdps, landing_of] = paths;
    let landing_of = landing_of.map(|p| p.to_string_lossy().into_owned());
    match (adsb, config, out) {
        (Some(adsb), Some(config), Some(out)) => Ok(Args {
            adsb,
            config,
            out,
            terrain,
            dof,
            mva,
            cifp,
            metar,
            sfdps,
            window_s,
            scorer_half,
            diagnose,
            landing_of,
        }),
        _ => Err("--adsb, --config and --out are all required".to_string()),
    }
}

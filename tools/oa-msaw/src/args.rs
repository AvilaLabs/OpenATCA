//! The command line:
//! `oa-msaw --adsb <clip.oaf> --config <file> --out <dir> [--terrain <dir>] [--dof <zip|dat>]
//! [--mva <dir>] [--cifp <file>] [--from-s <n> --to-s <n>] [--scorer-half]`.

use std::path::PathBuf;

pub const USAGE: &str = "usage: oa-msaw --adsb <clip.oaf> --config <file.cfg> --out <dir> [--terrain <3dep dir>] [--dof <DOF zip or DOF.DAT>] [--mva <dir>] [--cifp <FAACIFP18>] [--from-s <n> --to-s <n>] [--scorer-half]";

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
    /// The CIFP file, for the procedure areas of the `cifp_airport` lines.
    pub cifp: Option<PathBuf>,
    /// Track only the measurements from `from_s` to `to_s` seconds after the first one.
    pub window_s: Option<(f64, f64)>,
    /// Track only the scorer's half A of the clip (the reports the scoring harness gives the
    /// tracker), not every report.
    pub scorer_half: bool,
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
    let mut paths: [Option<PathBuf>; 7] = Default::default();
    let (mut from, mut to): (Option<f64>, Option<f64>) = (None, None);
    let mut scorer_half = false;
    const NAMES: [&str; 7] = [
        "--adsb",
        "--config",
        "--out",
        "--terrain",
        "--dof",
        "--mva",
        "--cifp",
    ];
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        if flag == "--scorer-half" {
            if std::mem::replace(&mut scorer_half, true) {
                return Err("--scorer-half given twice".to_string());
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
    let [adsb, config, out, terrain, dof, mva, cifp] = paths;
    match (adsb, config, out) {
        (Some(adsb), Some(config), Some(out)) => Ok(Args {
            adsb,
            config,
            out,
            terrain,
            dof,
            mva,
            cifp,
            window_s,
            scorer_half,
        }),
        _ => Err("--adsb, --config and --out are all required".to_string()),
    }
}

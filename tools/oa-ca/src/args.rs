//! The command line: `oa-ca --adsb <clip.oaf> --config <file> --out <dir> [--scorer-half]`.

use std::path::PathBuf;

pub const USAGE: &str =
    "usage: oa-ca --adsb <clip.oaf> --config <file.cfg> --out <dir> [--scorer-half]";

/// The parsed arguments.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub adsb: PathBuf,
    pub config: PathBuf,
    pub out: PathBuf,
    /// Track only the scorer's half A of the clip (the reports the scoring harness gives the
    /// tracker), not every report.
    pub scorer_half: bool,
}

/// Parses the arguments after the program name. A flag value that starts with `--` is a missing
/// value; a repeated flag, an unknown flag and a missing flag are errors.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let (mut adsb, mut config, mut out): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) =
        (None, None, None);
    let mut scorer_half = false;
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        let slot = match flag.as_str() {
            "--scorer-half" => {
                if std::mem::replace(&mut scorer_half, true) {
                    return Err("--scorer-half given twice".to_string());
                }
                continue;
            }
            "--adsb" => &mut adsb,
            "--config" => &mut config,
            "--out" => &mut out,
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
    match (adsb, config, out) {
        (Some(adsb), Some(config), Some(out)) => Ok(Args {
            adsb,
            config,
            out,
            scorer_half,
        }),
        _ => Err("--adsb, --config and --out are all required".to_string()),
    }
}

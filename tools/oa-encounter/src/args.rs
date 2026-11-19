//! The command line: `oa-encounter --adsb <clip.oaf> --out <dir> [--scorer-half]`.

use std::path::PathBuf;

pub const USAGE: &str = "usage: oa-encounter --adsb <clip.oaf> --out <dir> [--scorer-half]";

/// The parsed arguments.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub adsb: PathBuf,
    pub out: PathBuf,
    /// Track only the scorer's half A of the clip (the reports the scoring harness gives the
    /// tracker), not every report.
    pub scorer_half: bool,
}

/// Parses the arguments after the program name. A flag value that starts with `--` is a missing
/// value; a repeated flag, an unknown flag and a missing flag are errors.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let (mut adsb, mut out): (Option<PathBuf>, Option<PathBuf>) = (None, None);
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
    match (adsb, out) {
        (Some(adsb), Some(out)) => Ok(Args {
            adsb,
            out,
            scorer_half,
        }),
        _ => Err("--adsb and --out are both required".to_string()),
    }
}

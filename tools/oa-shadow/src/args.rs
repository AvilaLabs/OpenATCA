//! The command line: `oa-shadow --adsb <clip.oaf> --sfdps-dir <dir> --out <dir>`.

use std::path::PathBuf;

pub const USAGE: &str = "usage: oa-shadow [screen] --adsb <clip.oaf> --sfdps-dir <dir> --out <dir>";

/// The parsed arguments.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub adsb: PathBuf,
    pub sfdps_dir: PathBuf,
    pub out: PathBuf,
}

/// Parses the arguments after the program name. A flag value that starts with `--` is a missing
/// value; a repeated flag, an unknown flag and a missing flag are errors.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let (mut adsb, mut sfdps, mut out): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) =
        (None, None, None);
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        let slot = match flag.as_str() {
            "--adsb" => &mut adsb,
            "--sfdps-dir" => &mut sfdps,
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
    match (adsb, sfdps, out) {
        (Some(adsb), Some(sfdps_dir), Some(out)) => Ok(Args {
            adsb,
            sfdps_dir,
            out,
        }),
        _ => Err("--adsb, --sfdps-dir and --out are all required".to_string()),
    }
}

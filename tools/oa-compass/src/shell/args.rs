//! Shell (B1): the command lines of `oa-compass` and `oa-compass slice`, as values. A flag that
//! takes a value refuses a missing one, and a value that starts with `--` is a missing value (it
//! is the next flag); a flag given twice is an error, not a silent last-one-wins.

use std::path::PathBuf;

use super::guard::{SEALED_DAY, SEALED_DAY_TEXT};
use super::run::{EramAddress, InputsMode, OpenatcaRows};
use super::slice;
use crate::core::time::days_from_civil;

/// The usage line of the export.
pub const USAGE: &str = "usage: oa-compass --date <YYYY.MM.DD> --out <dir> [--adsb <clip.oaf>] [--sfdps-dir <dir>] [--split-days] [--inputs split|all] [--openatca-rows ticks|updates] [--eram-contradicted-address keep|drop]";

/// The usage line of `slice`.
pub const SLICE_USAGE: &str =
    "usage: oa-compass slice --in <dir> --from <HH:MM:SS> --to <HH:MM:SS> --out <dir>";

/// The arguments of an export. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportArgs {
    pub day: i64,
    pub out: PathBuf,
    pub adsb: Option<PathBuf>,
    pub sfdps: Option<PathBuf>,
    pub split: bool,
    pub mode: InputsMode,
    pub rows: OpenatcaRows,
    pub eram_address: EramAddress,
}

/// The arguments of `slice`. A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceArgs {
    pub input: PathBuf,
    pub out: PathBuf,
    pub from: u32,
    pub to: u32,
}

pub(crate) fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split(['.', '-']);
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    days_from_civil(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

/// The (flag, value) pairs of `args`; `bools` are flags without a value.
fn pairs(args: &[String], bools: &[&str]) -> Result<Vec<(String, Option<String>)>, String> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !a.starts_with("--") {
            return Err(format!("unexpected argument {a}"));
        }
        if out.iter().any(|(f, _)| f == a) {
            return Err(format!("{a} is given twice"));
        }
        if bools.contains(&a.as_str()) {
            out.push((a.clone(), None));
            continue;
        }
        match it.next() {
            Some(v) if !v.starts_with("--") => out.push((a.clone(), Some(v.clone()))),
            _ => return Err(format!("{a} needs a value")),
        }
    }
    Ok(out)
}

/// Parses the arguments after the program name of an export.
pub fn parse_export(args: &[String]) -> Result<ExportArgs, String> {
    let (mut date, mut out, mut adsb, mut sfdps) = (None, None, None, None);
    let mut split = false;
    let mut mode = InputsMode::All;
    let mut rows = OpenatcaRows::Ticks;
    let mut eram_address = EramAddress::Keep;
    for (flag, v) in pairs(args, &["--split-days"])? {
        let v = v.unwrap_or_default();
        match flag.as_str() {
            "--split-days" => split = true,
            "--date" => date = Some(v),
            "--out" => out = Some(PathBuf::from(v)),
            "--adsb" => adsb = Some(PathBuf::from(v)),
            "--sfdps-dir" => sfdps = Some(PathBuf::from(v)),
            "--inputs" => match v.as_str() {
                "all" => mode = InputsMode::All,
                "split" => mode = InputsMode::Split,
                _ => return Err("--inputs takes split or all".into()),
            },
            "--openatca-rows" => match v.as_str() {
                "ticks" => rows = OpenatcaRows::Ticks,
                "updates" => rows = OpenatcaRows::Updates,
                _ => return Err("--openatca-rows takes ticks or updates".into()),
            },
            "--eram-contradicted-address" => match v.as_str() {
                "keep" => eram_address = EramAddress::Keep,
                "drop" => eram_address = EramAddress::Drop,
                _ => return Err("--eram-contradicted-address takes keep or drop".into()),
            },
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    let (Some(date), Some(out)) = (date, out) else {
        return Err("--date and --out are required".into());
    };
    let Some(day) = parse_date(&date) else {
        return Err(format!("--date {date} is not a date (YYYY.MM.DD)"));
    };
    if day == SEALED_DAY {
        return Err(format!(
            "{SEALED_DAY_TEXT} is the sealed hold-out day; it is never exported"
        ));
    }
    if adsb.is_none() && sfdps.is_none() {
        return Err("give --adsb, --sfdps-dir or both".into());
    }
    Ok(ExportArgs {
        day,
        out,
        adsb,
        sfdps,
        split,
        mode,
        rows,
        eram_address,
    })
}

/// Parses the arguments after `slice`.
pub fn parse_slice(args: &[String]) -> Result<SliceArgs, String> {
    let (mut inp, mut out, mut from, mut to) = (None, None, None, None);
    for (flag, v) in pairs(args, &[])? {
        let v = v.unwrap_or_default();
        match flag.as_str() {
            "--in" => inp = Some(PathBuf::from(v)),
            "--out" => out = Some(PathBuf::from(v)),
            "--from" => {
                from = Some(slice::parse_clock(&v).ok_or(format!("--from {v} is not HH:MM:SS"))?)
            }
            "--to" => to = Some(slice::parse_clock(&v).ok_or(format!("--to {v} is not HH:MM:SS"))?),
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    match (inp, out, from, to) {
        (Some(input), Some(out), Some(from), Some(to)) => Ok(SliceArgs {
            input,
            out,
            from,
            to,
        }),
        _ => Err("--in, --out, --from and --to (HH:MM:SS) are required".into()),
    }
}

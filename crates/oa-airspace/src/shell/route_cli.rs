//! Normalized filed-route instructions to independently converted NASR/CIFP points. Shell
//! (B1/B3/C6). This grammar is explicit about identities; it is not a general ICAO route parser.

use crate::core::route::{
    convert_route, empty_converted_route, FixIdentity, Label, RoutePlan, RouteStep, MAX_ROUTE_STEPS,
};
use crate::core::snapshot::{decode, MAX_SNAPSHOT_BYTES};
use crate::shell::cli::CliError;
use std::io::{Read, Write};
use std::path::Path;

/// C6/C7: 32 instructions of up to 128 bytes normally fit; longer input is refused before scan.
pub const MAX_ROUTE_TEXT_BYTES: usize = 4096;
const MAX_TOKEN_BYTES: usize = 256;
const MAX_TOKEN_FIELDS: usize = 12;
const ROUTE_ARGUMENT_COUNT: usize = 4;
const MAX_ROUTE_ARGUMENT_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseReason {
    Empty,
    TextLimit,
    StepLimit,
    TokenLimit,
    UnknownToken,
    FieldCount,
    InvalidLabel,
    InvalidNumber,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteParseError {
    pub reason: ParseReason,
    /// Unknown when the byte cap prevented token scanning.
    pub offered: Option<usize>,
    pub admitted: usize,
    pub refused: usize,
    pub unexamined: usize,
}
// A7.2: cold one-shot parser result; all work is bounded by MAX_ROUTE_TEXT_BYTES.

fn label<const N: usize>(text: &str, empty: bool) -> Result<Label<N>, ParseReason> {
    let text = if empty && text == "-" { "" } else { text };
    if (!empty && text.is_empty())
        || text.len() > N
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ParseReason::InvalidLabel);
    }
    let mut bytes = [0; N];
    for (dst, src) in bytes.iter_mut().zip(text.bytes()) {
        *dst = src;
    }
    Ok(Label {
        bytes,
        len: u8::try_from(text.len()).map_err(|_| ParseReason::InvalidLabel)?,
    })
}
fn field<'a>(fields: &[&'a str], i: usize) -> Result<&'a str, ParseReason> {
    fields.get(i).copied().ok_or(ParseReason::FieldCount)
}
fn number<T: std::str::FromStr>(text: &str) -> Result<T, ParseReason> {
    text.parse().map_err(|_| ParseReason::InvalidNumber)
}
fn one(text: &str) -> Result<u8, ParseReason> {
    if text.len() != 1 {
        return Err(ParseReason::InvalidLabel);
    };
    text.as_bytes()
        .first()
        .copied()
        .ok_or(ParseReason::InvalidLabel)
}
fn fix(fields: &[&str], start: usize) -> Result<FixIdentity, ParseReason> {
    Ok(FixIdentity {
        ident: label(field(fields, start)?, false)?,
        region: label(field(fields, start + 1)?, true)?,
        class: number(field(fields, start + 2)?)?,
        scope: label(field(fields, start + 3)?, true)?,
    })
}
fn token(text: &str) -> Result<RouteStep, ParseReason> {
    if text.len() > MAX_TOKEN_BYTES {
        return Err(ParseReason::TokenLimit);
    }
    let fields: Vec<_> = text.split(':').take(MAX_TOKEN_FIELDS + 1).collect();
    match (field(&fields, 0)?, fields.len()) {
        ("FIX", 5) => Ok(RouteStep::Fix(fix(&fields, 1)?)),
        ("DCT", 5) => Ok(RouteStep::Direct(fix(&fields, 1)?)),
        ("AWY", 12) => Ok(RouteStep::Airway {
            designator: label(field(&fields, 1)?, false)?,
            location: one(field(&fields, 2)?)?,
            entry: fix(&fields, 3)?,
            exit: fix(&fields, 7)?,
            planned_altitude_ft: number(field(&fields, 11)?)?,
        }),
        ("PROC", 7) => Ok(RouteStep::Procedure {
            airport_ident: label(field(&fields, 1)?, false)?,
            airport_region: label(field(&fields, 2)?, false)?,
            kind: number(field(&fields, 3)?)?,
            ident: label(field(&fields, 4)?, false)?,
            route_type: one(field(&fields, 5)?)?,
            transition: label(field(&fields, 6)?, true)?,
        }),
        ("FIX" | "DCT" | "AWY" | "PROC", _) => Err(ParseReason::FieldCount),
        _ => Err(ParseReason::UnknownToken),
    }
}

/// Parse bounded, identity-qualified instructions. No ambiguity is resolved from proximity or
/// implicit region/class preference. On a malformed token its unexamined suffix is counted.
pub fn parse_route_steps(text: &str) -> Result<RoutePlan, RouteParseError> {
    if text.len() > MAX_ROUTE_TEXT_BYTES {
        return Err(RouteParseError {
            reason: ParseReason::TextLimit,
            offered: None,
            admitted: 0,
            refused: 0,
            unexamined: 0,
        });
    }
    let offered = text.split_ascii_whitespace().count(); // at most 4096 byte-delimited tokens
    if offered == 0 {
        return Err(RouteParseError {
            reason: ParseReason::Empty,
            offered: Some(offered),
            admitted: 0,
            refused: 0,
            unexamined: 0,
        });
    }
    if offered > MAX_ROUTE_STEPS {
        return Err(RouteParseError {
            reason: ParseReason::StepLimit,
            offered: Some(offered),
            admitted: 0,
            refused: 0,
            unexamined: offered,
        });
    }
    let empty = FixIdentity {
        ident: Label {
            bytes: [0; 12],
            len: 0,
        },
        region: Label {
            bytes: [0; 4],
            len: 0,
        },
        scope: Label {
            bytes: [0; 12],
            len: 0,
        },
        class: 0,
    };
    let mut plan = RoutePlan {
        steps: [RouteStep::Fix(empty); MAX_ROUTE_STEPS],
        len: 0,
    };
    for (index, text) in text.split_ascii_whitespace().enumerate() {
        let step = token(text).map_err(|reason| RouteParseError {
            reason,
            offered: Some(offered),
            admitted: index,
            refused: 1,
            unexamined: offered - index - 1,
        })?;
        let destination = plan.steps.get_mut(index).ok_or(RouteParseError {
            reason: ParseReason::StepLimit,
            offered: Some(offered),
            admitted: index,
            refused: 1,
            unexamined: offered - index - 1,
        })?;
        *destination = step;
        plan.len = plan.len.checked_add(1).ok_or(RouteParseError {
            reason: ParseReason::StepLimit,
            offered: Some(offered),
            admitted: index,
            refused: 1,
            unexamined: offered - index - 1,
        })?;
    }
    Ok(plan)
}

fn label_text<const N: usize>(label: Label<N>) -> Result<String, CliError> {
    let bytes = label
        .bytes
        .get(..usize::from(label.len))
        .ok_or_else(|| CliError::Failed("invalid converted label length".into()))?;
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| CliError::Failed("invalid converted label encoding".into()))
}
fn row<T: Copy>(values: &[T], index: usize) -> Result<T, CliError> {
    values
        .get(index)
        .copied()
        .ok_or_else(|| CliError::Failed("inconsistent converted route columns".into()))
}

/// `oa-adapt route --snapshot FILE --steps 'FIX:... DCT:... AWY:... PROC:...'`.
/// Writes the complete report to the supplied output stream, including failed conversion counts.
pub fn run_route(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    if args.len() != ROUTE_ARGUMENT_COUNT {
        return Err(CliError::Usage(
            "route needs exactly --snapshot FILE --steps TEXT".into(),
        ));
    }
    if args
        .iter()
        .any(|argument| argument.len() > MAX_ROUTE_ARGUMENT_BYTES)
    {
        return Err(CliError::Usage("route argument exceeds byte limit".into()));
    }
    let mut snapshot = None;
    let mut steps = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let slot = match flag.as_str() {
            "--snapshot" => &mut snapshot,
            "--steps" => &mut steps,
            _ => return Err(CliError::Usage(format!("unknown route argument {flag:?}"))),
        };
        if slot.is_some() {
            return Err(CliError::Usage(format!("duplicate route argument {flag}")));
        }
        *slot = Some(
            iter.next()
                .ok_or_else(|| CliError::Usage(format!("missing value for {flag}")))?,
        );
    }
    let snapshot = snapshot.ok_or_else(|| CliError::Usage("route needs --snapshot".into()))?;
    let steps = steps.ok_or_else(|| CliError::Usage("route needs --steps".into()))?;
    let plan =
        parse_route_steps(steps).map_err(|e| CliError::Usage(format!("route parse {e:?}")))?;
    let guard = oa_sealed::Sealed::production();
    let (file, len) = guard
        .open_regular(Path::new(snapshot))
        .map_err(|e| CliError::Failed(format!("snapshot: {e}")))?;
    let max = u64::try_from(MAX_SNAPSHOT_BYTES)
        .map_err(|_| CliError::Failed("snapshot byte limit width".into()))?;
    if len > max {
        return Err(CliError::Failed("snapshot exceeds byte limit".into()));
    }
    let mut bytes = Vec::new();
    file.take(
        max.checked_add(1)
            .ok_or_else(|| CliError::Failed("snapshot byte limit overflow".into()))?,
    )
    .read_to_end(&mut bytes)
    .map_err(|e| CliError::Failed(format!("snapshot: {e}")))?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(CliError::Failed("snapshot grew beyond byte limit".into()));
    }
    let model = decode(&bytes).map_err(|e| CliError::Failed(format!("snapshot: {e}")))?;
    let mut converted = empty_converted_route();
    let report = convert_route(&model, &plan, &mut converted);
    let write_error = |e| CliError::Failed(format!("write route output: {e}"));
    writeln!(out, "OA_ROUTE_CONVERSION\t1").map_err(write_error)?;
    writeln!(
        out,
        "snapshot_sha256\t{}",
        oa_sha256::to_hex(&oa_sha256::sha256(&bytes))
    )
    .map_err(write_error)?;
    writeln!(
        out,
        "instructions_sha256\t{}",
        oa_sha256::to_hex(&oa_sha256::sha256(steps.as_bytes()))
    )
    .map_err(write_error)?;
    writeln!(out, "report\t{report:?}").map_err(write_error)?;
    writeln!(out, "points\t{}", converted.len).map_err(write_error)?;
    writeln!(out,"ordinal\tident\tregion\tscope\tclass\tlat_e7\tlon_e7\tsource\tpath_term\tconnected\talt_desc\talt1_ft\talt2_ft\tspeed_kt\tturn\tmea_ft\tgps_mea_ft\tmea_dir\tgps_mea_dir\tairway\tairway_location\tprocedure_airport\tprocedure_airport_region\tprocedure\ttransition\tprocedure_kind\tprocedure_route_type").map_err(write_error)?;
    for i in 0..usize::from(converted.len) {
        if i >= crate::core::route::MAX_ROUTE_POINTS {
            return Err(CliError::Failed(
                "converted point count exceeds bound".into(),
            ));
        }
        let r = &converted;
        writeln!(
            out,
            "{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            label_text(row(&r.ident, i)?)?,
            label_text(row(&r.region, i)?)?,
            label_text(row(&r.scope, i)?)?,
            row(&r.class, i)?,
            row(&r.lat_e7, i)?,
            row(&r.lon_e7, i)?,
            row(&r.source, i)?,
            row(&r.path_term, i)?,
            row(&r.connected, i)?,
            row(&r.alt_desc, i)?,
            row(&r.alt1_ft, i)?,
            row(&r.alt2_ft, i)?,
            row(&r.speed_kt, i)?,
            row(&r.turn, i)?,
            row(&r.mea_ft, i)?,
            row(&r.gps_mea_ft, i)?,
            row(&r.mea_dir, i)?,
            row(&r.gps_mea_dir, i)?,
            label_text(row(&r.airway_designator, i)?)?,
            row(&r.airway_location, i)?,
            label_text(row(&r.procedure_airport_ident, i)?)?,
            label_text(row(&r.procedure_airport_region, i)?)?,
            label_text(row(&r.procedure_ident, i)?)?,
            label_text(row(&r.procedure_transition, i)?)?,
            row(&r.procedure_kind, i)?,
            row(&r.procedure_route_type, i)?
        )
        .map_err(write_error)?;
    }
    if report.failure != crate::core::route::RouteFailure::None {
        return Err(CliError::Failed(format!(
            "route conversion refused: {:?}",
            report.failure
        )));
    }
    Ok(())
}

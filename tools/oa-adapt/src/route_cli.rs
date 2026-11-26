//! Normalized filed-route instructions to independently converted NASR/CIFP points. Shell
//! (B1/B3/C6). This grammar is explicit about identities; it is not a general ICAO route parser.

use crate::cli::CliError;
use crate::manifest::{PackageIdentity, VerifiedPackage};
use oa_airspace::route::{
    convert_route, empty_converted_route, Label, RouteFailure, MAX_ROUTE_POINTS,
};
pub use oa_airspace::shell::route_parse::{
    parse_route_steps, ParseReason, RouteParseError, MAX_ROUTE_TEXT_BYTES,
};
use oa_airspace::snapshot::{decode, MAX_SNAPSHOT_BYTES};
use std::io::{Read, Write};
use std::path::Path;

/// C6/C7: 32 instructions of up to 128 bytes normally fit; longer input is refused before scan.
const ROUTE_ARGUMENT_COUNT: usize = 4;
const MAX_ROUTE_ARGUMENT_BYTES: usize = 4096;

// A7.2: cold one-shot route input; no per-cycle or real-time use.
enum RouteInput {
    Legacy {
        model: Box<oa_airspace::model::Model>,
        digest: oa_sha256::Digest,
    },
    Verified(Box<VerifiedPackage>),
}

impl RouteInput {
    fn model(&self) -> &oa_airspace::model::Model {
        match self {
            Self::Legacy { model, .. } => model,
            Self::Verified(package) => package.model(),
        }
    }

    fn snapshot_digest(&self) -> oa_sha256::Digest {
        match self {
            Self::Legacy { digest, .. } => *digest,
            Self::Verified(package) => package.identity().snapshot_sha256,
        }
    }

    fn identity(&self) -> Option<&PackageIdentity> {
        match self {
            Self::Legacy { .. } => None,
            Self::Verified(package) => Some(package.identity()),
        }
    }
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
    if args.len() != ROUTE_ARGUMENT_COUNT && args.len() != ROUTE_ARGUMENT_COUNT + 2 {
        return Err(CliError::Usage(
            "route needs --snapshot FILE --steps TEXT, or --package-root DIR --expected-manifest-sha256 HEX --steps TEXT".into(),
        ));
    }
    if args
        .iter()
        .any(|argument| argument.len() > MAX_ROUTE_ARGUMENT_BYTES)
    {
        return Err(CliError::Usage("route argument exceeds byte limit".into()));
    }
    let mut snapshot = None;
    let mut package_root = None;
    let mut expected_pin = None;
    let mut steps = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let slot = match flag.as_str() {
            "--snapshot" => &mut snapshot,
            "--package-root" => &mut package_root,
            "--expected-manifest-sha256" => &mut expected_pin,
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
    let steps = steps.ok_or_else(|| CliError::Usage("route needs --steps".into()))?;
    let plan =
        parse_route_steps(steps).map_err(|e| CliError::Usage(format!("route parse {e:?}")))?;
    let input = match (snapshot, package_root, expected_pin) {
        (Some(path), None, None) => {
            let guard = oa_sealed::Sealed::production();
            let (file, len) = guard.open_regular(Path::new(path))
                .map_err(|e| CliError::Failed(format!("snapshot: {e}")))?;
            let max = u64::try_from(MAX_SNAPSHOT_BYTES)
                .map_err(|_| CliError::Failed("snapshot byte limit width".into()))?;
            if len > max { return Err(CliError::Failed("snapshot exceeds byte limit".into())); }
            let mut bytes = Vec::new();
            file.take(max.checked_add(1).ok_or_else(|| CliError::Failed("snapshot byte limit overflow".into()))?)
                .read_to_end(&mut bytes)
                .map_err(|e| CliError::Failed(format!("snapshot: {e}")))?;
            if bytes.len() > MAX_SNAPSHOT_BYTES { return Err(CliError::Failed("snapshot grew beyond byte limit".into())); }
            let model = decode(&bytes).map_err(|e| CliError::Failed(format!("snapshot: {e}")))?;
            let digest = oa_sha256::sha256(&bytes);
            RouteInput::Legacy { model: Box::new(model), digest }
        }
        (None, Some(root), Some(pin)) => {
            let pin = crate::manifest::parse_pin(pin)
                .map_err(|e| CliError::Usage(format!("expected manifest pin: {e}")))?;
            let verified = crate::manifest::verify_current(Path::new(root), pin)
                .map_err(|e| CliError::Failed(format!("adaptation refused: {e}")))?;
            RouteInput::Verified(Box::new(verified))
        }
        _ => return Err(CliError::Usage("route snapshot mode and verified package mode are mutually exclusive; verified mode needs both package root and independent manifest pin".into())),
    };
    let mut converted = empty_converted_route();
    let report = convert_route(input.model(), &plan, &mut converted);
    let write_error = |e| CliError::Failed(format!("write route output: {e}"));
    writeln!(out, "OA_ROUTE_CONVERSION\t1").map_err(write_error)?;
    if let Some(identity) = input.identity() {
        writeln!(out, "adaptation_status\tverified_for_this_process").map_err(write_error)?;
        writeln!(
            out,
            "manifest_sha256\t{}",
            oa_sha256::to_hex(&identity.manifest_sha256)
        )
        .map_err(write_error)?;
        writeln!(out, "adaptation_release\t{}", identity.release).map_err(write_error)?;
        writeln!(out, "adaptation_facility\t{}", identity.facility).map_err(write_error)?;
        writeln!(out, "change_ref\t{}", identity.change_ref).map_err(write_error)?;
        writeln!(
            out,
            "integrity_scope\tsha256 consistency only; issuer is not authenticated"
        )
        .map_err(write_error)?;
    } else {
        writeln!(out, "adaptation_status\tunverified_legacy_snapshot").map_err(write_error)?;
    }
    writeln!(
        out,
        "snapshot_sha256\t{}",
        oa_sha256::to_hex(&input.snapshot_digest())
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
        if i >= MAX_ROUTE_POINTS {
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
    if report.failure != RouteFailure::None {
        return Err(CliError::Failed(format!(
            "route conversion refused: {:?}",
            report.failure
        )));
    }
    Ok(())
}

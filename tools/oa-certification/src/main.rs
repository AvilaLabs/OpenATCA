#![forbid(unsafe_code)]
//! Minimal shell commands for the bounded synthetic PVER and verified log report.

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command, mode, package_root, manifest_pin] if command == "pver" => {
            let mode = match mode.as_str() {
                "full" => oa_certification::PverMode::Full,
                "abbreviated" => oa_certification::PverMode::Abbreviated,
                _ => {
                    usage();
                    return ExitCode::FAILURE;
                }
            };
            let Some(pin) = parse_digest(manifest_pin) else {
                eprintln!("manifest pin must be exactly 64 lowercase hexadecimal characters");
                return ExitCode::FAILURE;
            };
            oa_certification::run_bound_performance(mode, std::path::Path::new(package_root), pin).map_err(|error| error.to_string()).and_then(|result| {
                println!("mode={mode:?} passed={} targets={}/{} refusals={} losses={} catalog_version={} catalog_adaptation_version={:?} catalog_sha256={} adaptation_sha256={} profile_binding_sha256={}",
                    result.passed(), result.targets_passed(), result.targets_checked(), result.tracker_refusals(),
                    result.tracker_loss_rows(), result.catalog_version(), result.adaptation_version(), oa_sha256::to_hex(&result.catalog_sha256()),
                    digest_text(result.adaptation_sha256()), digest_text(result.profile_binding_sha256()));
                println!("tracker_config_sha256={} runner_build_claim_sha256={} runner_artifact_state={:?} runner_artifact_sha256={}", oa_sha256::to_hex(&result.tracker_config_sha256()), oa_sha256::to_hex(&result.runner_build_claim_sha256()), result.runner_artifact_state(), digest_text(result.runner_artifact_sha256()));
                for target in result.target_results() {
                    println!("target={} state={:?} output_time_ns={:?} position_error_um={:?} position_tolerance_um={} velocity_error_um_s={:?} velocity_tolerance_um_s={}", target.target_id, target.state, target.output_time_ns, target.position_error_um, target.position_tolerance_um, target.velocity_error_um_s, target.velocity_tolerance_um_s);
                }
                result.passed().then_some(()).ok_or_else(|| "synthetic PVER checks did not pass".to_owned())
            })
        }
        [command, path, count, tail, registry_path, registry_pin, principal]
            if command == "report" =>
        {
            let Some(checkpoint) = parse_checkpoint(count, tail) else {
                eprintln!("invalid record count or tail checkpoint");
                return ExitCode::FAILURE;
            };
            let Some(registry_pin) = parse_digest(registry_pin) else {
                eprintln!("registry pin must be exactly 64 lowercase hexadecimal characters");
                return ExitCode::FAILURE;
            };
            let Some(principal_id) = principal.parse::<u64>().ok().filter(|value| *value != 0)
            else {
                eprintln!("principal must be a nonzero unsigned integer claim");
                return ExitCode::FAILURE;
            };
            let result = (|| -> Result<String, String> {
                let registry = oa_audit_log::shell::load_principal_registry(
                    Path::new(registry_path),
                    registry_pin,
                )
                .map_err(|error| format!("principal registry refused: {error:?}"))?;
                let role = oa_audit_core::authorize_principal(
                    &registry,
                    principal_id,
                    oa_audit_core::Action::Verify,
                )
                .map_err(|error| format!("principal registry refused verification: {error:?}"))?;
                let report = oa_certification::certification_report(Path::new(path), checkpoint)
                    .map_err(|error| error.to_string())?;
                Ok(format!(
                    "verification_principal_claim={principal_id} registry_derived_role={role:?}\n{report}"
                ))
            })();
            result
                .map(|report| print!("{report}"))
                .map_err(|error| error.to_string())
        }
        [command, log, count, tail, registry_path, principal, time_ns, service, mode, expected_path, observations_path, exceptions, max_bytes]
            if command == "certify" =>
        {
            let Some(principal_id) = principal.parse::<u64>().ok().filter(|value| *value != 0)
            else {
                eprintln!("principal must be a nonzero unsigned integer claim");
                return ExitCode::FAILURE;
            };
            let Some(time_ns) = time_ns.parse::<u64>().ok() else {
                eprintln!("time must be unsigned nanoseconds supplied by caller");
                return ExitCode::FAILURE;
            };
            let Some(mode) = parse_mode(mode) else {
                eprintln!("mode must be full or abbreviated");
                return ExitCode::FAILURE;
            };
            let Some(max_software_bytes) = max_bytes.parse::<u64>().ok() else {
                eprintln!("artifact cap must be an unsigned integer");
                return ExitCode::FAILURE;
            };
            if service.is_empty()
                || service.len() > 96
                || exceptions.len() > 1024
                || max_software_bytes == 0
                || max_software_bytes > oa_build_info::MAX_ARTIFACT_BYTES
            {
                eprintln!("request service, exception text, or artifact cap exceeds fixed bounds");
                return ExitCode::FAILURE;
            }
            let expected_checkpoint = match parse_checkpoint(count, tail) {
                Some(value) => value,
                None => {
                    eprintln!("invalid expected checkpoint");
                    return ExitCode::FAILURE;
                }
            };
            let prepared = (|| -> Result<(), String> {
                let registry = read_guarded(
                    registry_path,
                    oa_certification::MAX_AUTH_REGISTRY_BYTES as u64,
                )?;
                let authorization =
                    oa_certification::authorize_configure_claim(&registry, principal_id)
                        .map_err(|error| error.to_string())?;
                let expected_bytes = read_guarded(
                    expected_path,
                    oa_certification::MAX_AUTH_REGISTRY_BYTES as u64,
                )?;
                let (expected_processor_ids, expected_evidence) =
                    parse_expected_processors(&expected_bytes)?;
                let expected_inventory_sha256 = oa_sha256::sha256(&expected_bytes);
                let observations = read_guarded(
                    observations_path,
                    oa_certification::MAX_AUTH_REGISTRY_BYTES as u64,
                )?;
                let processors = parse_observations(&observations, &expected_evidence)?;
                let request = oa_certification::CertificationRequest {
                    authorization,
                    time_ns,
                    service_id: service.clone(),
                    mode,
                    expected_processor_ids,
                    expected_inventory_sha256,
                    processors,
                    exceptions: exceptions.clone(),
                    max_software_bytes,
                };
                let committed = oa_certification::record_certification(
                    Path::new(log),
                    expected_checkpoint,
                    request,
                )
                .map_err(|error| error.to_string())?;
                println!("outcome={:?} checkpoint_count={} checkpoint_tail={} audit_sequence={} audit_digest={} audit_durable=true", committed.outcome, committed.checkpoint.count, oa_sha256::to_hex(&committed.checkpoint.tail), committed.receipt.sequence(), oa_sha256::to_hex(&committed.receipt.digest()));
                Ok(())
            })();
            prepared
        }
        _ => {
            usage();
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oa-certification: {error}");
            ExitCode::FAILURE
        }
    }
}

fn digest_text(value: Option<oa_sha256::Digest>) -> String {
    value.map_or_else(|| "absent".to_owned(), |digest| oa_sha256::to_hex(&digest))
}

fn usage() {
    eprintln!("usage: oa-certification pver <full|abbreviated> <g2-package-dir> <manifest-sha256> | report <log> <count> <tail-hex> <local-registry> <registry-sha256> <principal-claim> | certify <log> <count> <tail-hex> <local-registry> <principal-claim> <time-ns-claim> <service> <full|abbreviated> <expected-roster.tsv> <observations.tsv> <exceptions> <artifact-cap>");
}

fn parse_checkpoint(count: &str, tail: &str) -> Option<oa_audit_core::Checkpoint> {
    let count = count.parse::<u64>().ok()?;
    Some(oa_audit_core::Checkpoint {
        count,
        tail: parse_digest(tail)?,
    })
}

fn parse_mode(value: &str) -> Option<oa_certification::PverMode> {
    match value {
        "full" => Some(oa_certification::PverMode::Full),
        "abbreviated" => Some(oa_certification::PverMode::Abbreviated),
        _ => None,
    }
}

fn read_guarded(path: &str, cap: u64) -> Result<Vec<u8>, String> {
    if cap == 0 {
        return Err("input cap is invalid".to_owned());
    }
    if path.is_empty() || path.len() > 4096 {
        return Err("input path exceeds its fixed bound".to_owned());
    }
    let (file, declared) =
        oa_sealed::open_regular(Path::new(path)).map_err(|error| error.to_string())?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() || before.len() != declared {
        return Err("guarded input descriptor identity is invalid".to_owned());
    }
    if declared > cap {
        return Err("guarded input exceeds its byte cap".to_owned());
    }
    let mut bytes =
        Vec::with_capacity(usize::try_from(declared).map_err(|_| "input size overflow")?);
    let mut bounded = file
        .try_clone()
        .map_err(|error| error.to_string())?
        .take(cap.checked_add(1).ok_or("input cap overflow")?);
    bounded
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let after = file.metadata().map_err(|error| error.to_string())?;
    let actual_bytes = u64::try_from(bytes.len()).map_err(|_| "input byte count overflow")?;
    if actual_bytes > cap || actual_bytes != declared || !same_file_stamp(&before, &after) {
        return Err("guarded input length or descriptor identity changed".to_owned());
    }
    Ok(bytes)
}

#[cfg(unix)]
fn same_file_stamp(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

#[cfg(not(unix))]
fn same_file_stamp(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

type ExpectedEvidence = std::collections::BTreeMap<
    u32,
    (
        oa_certification::DigestEvidence,
        oa_certification::DigestEvidence,
    ),
>;

fn parse_expected_processors(bytes: &[u8]) -> Result<(Vec<u32>, ExpectedEvidence), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "expected roster is not UTF-8")?;
    let mut lines = text.lines();
    if lines.next() != Some("OA-CERT-EXPECTED-PROCESSORS-V1") {
        return Err("expected processor roster schema is unsupported".to_owned());
    }
    let mut ids = Vec::new();
    let mut expected = ExpectedEvidence::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if ids.len() >= 128 {
            return Err("expected processor roster row cap exceeded".to_owned());
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err("expected processor roster row must have three fields".to_owned());
        }
        let id_text = fields.first().copied().ok_or("processor ID missing")?;
        let id = id_text.parse::<u32>().map_err(|_| "processor ID invalid")?;
        if id == 0 || id_text != id.to_string() {
            return Err("processor ID must be canonical and nonzero".to_owned());
        }
        let software = parse_evidence(
            fields
                .get(1)
                .copied()
                .ok_or("expected software digest missing")?,
        )?;
        let adaptation = parse_evidence(
            fields
                .get(2)
                .copied()
                .ok_or("expected adaptation digest missing")?,
        )?;
        if expected.insert(id, (software, adaptation)).is_some() {
            return Err("expected processor roster contains a duplicate ID".to_owned());
        }
        ids.push(id);
    }
    if ids.is_empty() {
        return Err("expected processor roster is empty".to_owned());
    }
    ids.sort_unstable();
    Ok((ids, expected))
}

fn parse_observations(
    bytes: &[u8],
    expected: &ExpectedEvidence,
) -> Result<Vec<oa_certification::ProcessorSource>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "observations are not UTF-8")?;
    let mut lines = text.lines();
    if lines.next() != Some("OA-CERT-OBSERVATIONS-V1") {
        return Err("processor observation schema is unsupported".to_owned());
    }
    let mut out = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if out.len() >= 128 {
            return Err("observation row cap exceeded".to_owned());
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err("observation row must have four fields".to_owned());
        }
        let id_text = fields.first().copied().ok_or("processor ID missing")?;
        let id = id_text.parse::<u32>().map_err(|_| "processor ID invalid")?;
        if id == 0 || id_text != id.to_string() {
            return Err("processor ID must be canonical and nonzero".to_owned());
        }
        let (expected_software, expected_adaptation) = expected.get(&id).copied().unwrap_or((
            oa_certification::DigestEvidence::missing(),
            oa_certification::DigestEvidence::missing(),
        ));
        let software_path = fields.get(1).copied().ok_or("software path missing")?;
        let adaptation_path = fields.get(2).copied().ok_or("adaptation path missing")?;
        let adaptation_pin = fields.get(3).copied().ok_or("adaptation pin missing")?;
        if software_path.len() > 4096 || adaptation_path.len() > 4096 {
            return Err("observation path exceeds its fixed bound".to_owned());
        }
        let software_artifact_path = if software_path == "-" {
            None
        } else {
            Some(software_path.into())
        };
        let adaptation_package = if adaptation_path == "-" {
            if adaptation_pin != "-" {
                return Err("missing adaptation package must use '-' manifest pin".to_owned());
            }
            None
        } else {
            let pin = parse_digest(adaptation_pin)
                .ok_or_else(|| "adaptation manifest pin is invalid".to_owned())?;
            if pin == [0; 32] {
                return Err("adaptation manifest pin cannot be zero".to_owned());
            }
            Some((adaptation_path.into(), pin))
        };
        out.push(oa_certification::ProcessorSource {
            processor_id: id,
            expected_software,
            software_artifact_path,
            expected_adaptation,
            adaptation_package,
        });
    }
    Ok(out)
}

fn parse_evidence(value: &str) -> Result<oa_certification::DigestEvidence, String> {
    if value == "-" {
        return Ok(oa_certification::DigestEvidence::missing());
    }
    parse_digest(value)
        .map(oa_certification::DigestEvidence::present)
        .ok_or_else(|| {
            "processor digest must be '-' or 64 lowercase hexadecimal characters".to_owned()
        })
}

fn parse_digest(text: &str) -> Option<oa_sha256::Digest> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let start = index.checked_mul(2)?;
        let end = start.checked_add(2)?;
        *slot = u8::from_str_radix(text.get(start..end)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    // REQ: OA-CERT-003, OA-CERT-008
    #[test]
    fn expected_roster_is_independent_from_observations_and_both_are_bounded() {
        let digest = "01".repeat(32);
        let input = format!(
            "OA-CERT-EXPECTED-PROCESSORS-V1\n7\t{digest}\t{digest}\n8\t{digest}\t{digest}\n"
        );
        let (ids, expected) =
            parse_expected_processors(input.as_bytes()).expect("separate expected roster");
        assert_eq!(ids, vec![7, 8]);
        let observed = b"OA-CERT-OBSERVATIONS-V1\n7\t/tmp/synthetic.bin\t-\t-\n";
        let parsed = parse_observations(observed, &expected).expect("bounded observations");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].processor_id, 7);
        assert_eq!(
            parsed[0].expected_software.state,
            oa_certification::EvidenceState::Present
        );
        assert!(parse_expected_processors(b"unknown\n").is_err());
        assert!(parse_expected_processors(b"OA-CERT-EXPECTED-PROCESSORS-V1\n7\tx\t-\n").is_err());
        assert!(parse_expected_processors(
            format!("OA-CERT-EXPECTED-PROCESSORS-V1\n7\t{digest}\t-\n7\t{digest}\t-\n").as_bytes()
        )
        .is_err());
        let unknown = parse_observations(
            b"OA-CERT-OBSERVATIONS-V1\n9\t/tmp/unknown\t-\t-\n",
            &expected,
        )
        .expect("unknown remains explicit");
        assert_eq!(
            unknown[0].expected_software.state,
            oa_certification::EvidenceState::Missing
        );
    }
}

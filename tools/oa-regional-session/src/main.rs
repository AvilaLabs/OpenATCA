#![forbid(unsafe_code)]
//! CLI shell for normalized regional-session input staging and composed offline execution.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use oa_regional_session::{
    flush_receipt_group, parse_event_line, push_event, session_cursor_new_with_profile_limits,
    session_total_events, summarize_input_batch_with_limit, FrameEvidence, InputError,
    MAX_EVENTS_PER_RECEIPT, MAX_EVENT_LINE_BYTES, MAX_RECORDED_PROFILE_EVENTS, MAX_SESSION_EVENTS,
    RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
};

const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
/// Explicit one-hour profile input envelope. This bounds the retained byte buffer independently
/// of the existing ten-million-event parser ceiling; input is refused rather than truncated.
const MAX_RECORDED_HOUR_INPUT_BYTES: u64 =
    oa_regional_session::recordings_cli::MAX_RECORDED_HOUR_BYTES as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowAccounting {
    admitted: u64,
    refused: u64,
    unexamined: u64,
    denominator: u64,
}

#[derive(Debug)]
enum CliError {
    Usage,
    Io(io::Error),
    InputTooLarge {
        max_bytes: u64,
    },
    InvalidUtf8,
    BadHeader,
    MissingHeader,
    BadLine {
        line: usize,
        error: InputError,
        accounting: RowAccounting,
    },
    Ordering {
        line: usize,
        message: &'static str,
        accounting: RowAccounting,
    },
    OutputExists,
    TooManyEvents,
    Runtime(String),
}

// A7.2: cold command-line configuration, size guard waived.
struct Arguments {
    input: PathBuf,
    output: PathBuf,
    model: Option<oa_regional_session::cli::ModelSource>,
    safety: oa_regional_session::cli::MsawSource,
    performance: Option<PathBuf>,
    options: oa_regional_session::cli::RegionalSessionOptions,
    quarantine_release: Option<oa_regional_session::cli::QuarantineReleaseOptions>,
}

#[derive(Default)]
struct ReplayArgs {
    profile: Option<oa_regional_session::cli::RegionalCliProfile>,
    start_ns: Option<i64>,
    end_ns: Option<i64>,
    warmup_start_ns: Option<i64>,
}

fn arguments() -> Result<Arguments, CliError> {
    let mut args = env::args_os().skip(1);
    let command = args.next().ok_or(CliError::Usage)?;
    let execute = match command.to_str() {
        Some("run") => true,
        Some("stage") => false,
        _ => return Err(CliError::Usage),
    };
    let mut input = None;
    let mut output = None;
    let mut model = None;
    let mut issued_package = None;
    let mut issued_manifest_pin = None;
    let mut processor_id = None;
    let mut safety = None;
    let mut performance = None;
    let mut replay = ReplayArgs::default();
    let mut quarantine_source = None;
    let mut quarantine_instance = None;
    let mut quarantine_item = None;
    let mut manual_handoff_ref = None;
    let mut audit_registry = None;
    let mut audit_registry_pin = None;
    let mut audit_log = None;
    let mut audit_count = None;
    let mut audit_tail = None;
    let mut audit_principal = None;
    let mut audit_role = None;
    let mut audit_time_ns = None;
    let mut count = 0;
    while let Some(arg) = args.next() {
        count += 1;
        if count > 48 {
            return Err(CliError::Usage);
        }
        match arg.to_str() {
            Some("--input") if input.is_none() => input = args.next().map(PathBuf::from),
            Some("--out") if output.is_none() => output = args.next().map(PathBuf::from),
            Some("--snapshot") if execute && model.is_none() => {
                model = Some(oa_regional_session::cli::ModelSource::Snapshot(
                    args.next().map(PathBuf::from).ok_or(CliError::Usage)?,
                ))
            }
            Some("--synthetic-model") if execute && model.is_none() => {
                model = Some(oa_regional_session::cli::ModelSource::Synthetic)
            }
            Some("--issued-package") if execute && issued_package.is_none() => {
                issued_package = args.next().map(PathBuf::from)
            }
            Some("--issued-manifest-sha256") if execute && issued_manifest_pin.is_none() => {
                issued_manifest_pin = args.next().and_then(|value| value.into_string().ok())
            }
            Some("--processor-id") if execute && processor_id.is_none() => {
                processor_id = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| value.parse::<u32>().ok())
            }
            Some("--synthetic-msaw") if execute && safety.is_none() => {
                safety = Some(oa_regional_session::cli::MsawSource::Synthetic)
            }
            Some("--msaw-inputs") if execute && safety.is_none() => {
                safety = Some(oa_regional_session::cli::MsawSource::Manifest(
                    args.next().map(PathBuf::from).ok_or(CliError::Usage)?,
                ));
            }
            Some("--performance") if execute && performance.is_none() => {
                performance = Some(args.next().map(PathBuf::from).ok_or(CliError::Usage)?);
            }
            Some(
                flag @ ("--profile"
                | "--replay-start-ns"
                | "--replay-end-ns"
                | "--replay-warmup-start-ns"),
            ) if execute => {
                parse_replay_option(flag, &mut args, &mut replay)?;
            }
            Some("--quarantine-release-source") if execute && quarantine_source.is_none() => {
                quarantine_source = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| match value.as_str() {
                        "flight-data" => Some(oa_quarantine::InputSource::FlightData),
                        "surveillance" => Some(oa_quarantine::InputSource::Surveillance),
                        _ => None,
                    });
                if quarantine_source.is_none() {
                    return Err(CliError::Usage);
                }
            }
            Some("--quarantine-release-instance") if execute && quarantine_instance.is_none() => {
                quarantine_instance = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<u16>().ok())
                        .filter(|value| *value > 0)
                        .ok_or(CliError::Usage)?,
                );
            }
            Some("--quarantine-release-item") if execute && quarantine_item.is_none() => {
                quarantine_item = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|value| *value > 0)
                        .ok_or(CliError::Usage)?,
                );
            }
            Some("--manual-handoff-ref") if execute && manual_handoff_ref.is_none() => {
                manual_handoff_ref = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .filter(|value| !value.is_empty() && value.len() <= 256)
                        .ok_or(CliError::Usage)?,
                );
            }
            Some("--audit-registry") if execute && audit_registry.is_none() => {
                audit_registry = args.next().map(PathBuf::from);
            }
            Some("--audit-registry-sha256") if execute && audit_registry_pin.is_none() => {
                audit_registry_pin = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| parse_audit_digest(&value).ok())
                        .ok_or(CliError::Usage)?,
                );
            }
            Some("--audit-log") if execute && audit_log.is_none() => {
                audit_log = args.next().map(PathBuf::from);
            }
            Some("--audit-count") if execute && audit_count.is_none() => {
                audit_count = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .ok_or(CliError::Usage)?,
                );
            }
            Some("--audit-tail") if execute && audit_tail.is_none() => {
                audit_tail = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| parse_audit_digest(&value).ok())
                        .ok_or(CliError::Usage)?,
                );
            }
            Some("--audit-principal") if execute && audit_principal.is_none() => {
                audit_principal = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|value| *value > 0)
                        .ok_or(CliError::Usage)?,
                );
            }
            Some("--audit-role") if execute && audit_role.is_none() => {
                audit_role = Some(
                    match args
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .as_deref()
                    {
                        Some("monitor") => oa_audit_core::Role::Monitor,
                        Some("operate") => oa_audit_core::Role::Operate,
                        Some("configure") => oa_audit_core::Role::Configure,
                        _ => return Err(CliError::Usage),
                    },
                );
            }
            Some("--audit-time-ns") if execute && audit_time_ns.is_none() => {
                audit_time_ns = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|value| *value > 0)
                        .ok_or(CliError::Usage)?,
                );
            }
            _ => return Err(CliError::Usage),
        }
    }
    let input = input.ok_or(CliError::Usage)?;
    let output = output.ok_or(CliError::Usage)?;
    let issued_fields_present =
        issued_package.is_some() || issued_manifest_pin.is_some() || processor_id.is_some();
    if model.is_some() && issued_fields_present {
        return Err(CliError::Usage);
    }
    if issued_fields_present {
        let root = issued_package.ok_or(CliError::Usage)?;
        let pin = issued_manifest_pin.ok_or(CliError::Usage)?;
        let expected_manifest_sha256 =
            oa_adapt::manifest::parse_pin(&pin).map_err(|_| CliError::Usage)?;
        let processor_id = processor_id.ok_or(CliError::Usage)?;
        if processor_id == 0 {
            return Err(CliError::Usage);
        }
        model = Some(oa_regional_session::cli::ModelSource::IssuedPackage {
            root,
            expected_manifest_sha256,
            processor_id,
        });
    }
    if execute && model.is_none() {
        return Err(CliError::Usage);
    }
    let any_quarantine_release = quarantine_source.is_some()
        || quarantine_instance.is_some()
        || quarantine_item.is_some()
        || manual_handoff_ref.is_some()
        || audit_registry.is_some()
        || audit_registry_pin.is_some()
        || audit_log.is_some()
        || audit_count.is_some()
        || audit_tail.is_some()
        || audit_principal.is_some()
        || audit_role.is_some()
        || audit_time_ns.is_some();
    let quarantine_release = if any_quarantine_release {
        let source = quarantine_source.ok_or(CliError::Usage)?;
        let source_instance = quarantine_instance.ok_or(CliError::Usage)?;
        let item_id = quarantine_item.ok_or(CliError::Usage)?;
        let audit_registry_path = audit_registry.ok_or(CliError::Usage)?;
        let registry_pin = audit_registry_pin.ok_or(CliError::Usage)?;
        let principal = audit_principal.ok_or(CliError::Usage)?;
        let principal_registry = load_quarantine_principal_registry(
            &audit_registry_path,
            registry_pin,
            principal,
            audit_role,
        )?;
        Some(oa_regional_session::cli::QuarantineReleaseOptions {
            source,
            identity: oa_quarantine::InputIdentity {
                source,
                source_instance,
                item_id,
            },
            manual_handoff_ref: manual_handoff_ref.ok_or(CliError::Usage)?,
            audit_log: audit_log.ok_or(CliError::Usage)?,
            audit_checkpoint: oa_audit_core::Checkpoint {
                count: audit_count.ok_or(CliError::Usage)?,
                tail: audit_tail.ok_or(CliError::Usage)?,
            },
            principal_registry,
            principal,
            time_ns: audit_time_ns.ok_or(CliError::Usage)?,
        })
    } else {
        None
    };
    let profile = replay
        .profile
        .unwrap_or(oa_regional_session::cli::RegionalCliProfile::Default);
    let options = session_options_with_warmup(
        profile,
        replay.start_ns,
        replay.end_ns,
        replay.warmup_start_ns,
    )?;
    Ok(Arguments {
        input,
        output,
        model,
        safety: safety.unwrap_or(oa_regional_session::cli::MsawSource::Disabled),
        performance,
        options,
        quarantine_release,
    })
}

fn parse_replay_option(
    flag: &str,
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    replay: &mut ReplayArgs,
) -> Result<(), CliError> {
    match flag {
        "--profile" if replay.profile.is_none() => {
            let value = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or(CliError::Usage)?;
            replay.profile = Some(parse_cli_profile(&value)?);
        }
        "--replay-start-ns" if replay.start_ns.is_none() => {
            replay.start_ns = Some(
                args.next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| value.parse::<i64>().ok())
                    .ok_or(CliError::Usage)?,
            );
        }
        "--replay-end-ns" if replay.end_ns.is_none() => {
            replay.end_ns = Some(
                args.next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| value.parse::<i64>().ok())
                    .ok_or(CliError::Usage)?,
            );
        }
        "--replay-warmup-start-ns" if replay.warmup_start_ns.is_none() => {
            replay.warmup_start_ns = Some(
                args.next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| value.parse::<i64>().ok())
                    .ok_or(CliError::Usage)?,
            );
        }
        _ => return Err(CliError::Usage),
    }
    Ok(())
}

fn parse_cli_profile(
    value: &str,
) -> Result<oa_regional_session::cli::RegionalCliProfile, CliError> {
    match value {
        "recorded-peak-rendered" => {
            Ok(oa_regional_session::cli::RegionalCliProfile::RecordedPeakRendered)
        }
        "recorded-hour-rendered" => {
            Ok(oa_regional_session::cli::RegionalCliProfile::RecordedHourRendered)
        }
        _ => Err(CliError::Usage),
    }
}

#[cfg(test)]
fn session_options(
    profile: oa_regional_session::cli::RegionalCliProfile,
    start_ns: Option<i64>,
    end_ns: Option<i64>,
) -> Result<oa_regional_session::cli::RegionalSessionOptions, CliError> {
    session_options_with_warmup(profile, start_ns, end_ns, None)
}

fn session_options_with_warmup(
    profile: oa_regional_session::cli::RegionalCliProfile,
    start_ns: Option<i64>,
    end_ns: Option<i64>,
    warmup_start_ns: Option<i64>,
) -> Result<oa_regional_session::cli::RegionalSessionOptions, CliError> {
    let recorded_hour = match (start_ns, end_ns) {
        (None, None) if warmup_start_ns.is_none() => None,
        (Some(start_ns), Some(end_ns)) => Some(
            oa_regional_session::cli::RecordedHourWindow::new_with_warmup(
                start_ns,
                end_ns,
                warmup_start_ns,
            )
            .map_err(|_| CliError::Usage)?,
        ),
        _ => return Err(CliError::Usage),
    };
    if (profile == oa_regional_session::cli::RegionalCliProfile::RecordedHourRendered)
        != recorded_hour.is_some()
    {
        return Err(CliError::Usage);
    }
    Ok(oa_regional_session::cli::RegionalSessionOptions {
        profile,
        recorded_hour,
    })
}

fn input_byte_limit(profile: oa_regional_session::cli::RegionalCliProfile) -> u64 {
    match profile {
        oa_regional_session::cli::RegionalCliProfile::RecordedHourRendered => {
            MAX_RECORDED_HOUR_INPUT_BYTES
        }
        oa_regional_session::cli::RegionalCliProfile::Default
        | oa_regional_session::cli::RegionalCliProfile::RecordedPeakRendered => MAX_INPUT_BYTES,
    }
}

fn parse_audit_digest(value: &str) -> Result<[u8; 32], ()> {
    if value.len() != 64 {
        return Err(());
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let offset = index.checked_mul(2).ok_or(())?;
        let high = value.as_bytes().get(offset).copied().ok_or(())?;
        let low = value.as_bytes().get(offset + 1).copied().ok_or(())?;
        *byte = (hex_nibble(high).ok_or(())? << 4) | hex_nibble(low).ok_or(())?;
    }
    Ok(digest)
}

fn load_quarantine_principal_registry(
    path: &Path,
    pin: [u8; 32],
    principal: u64,
    role_claim: Option<oa_audit_core::Role>,
) -> Result<oa_audit_core::PrincipalRegistry, CliError> {
    let registry = oa_audit_log::shell::load_principal_registry(path, pin)
        .map_err(|error| CliError::Runtime(format!("quarantine principal registry: {error}")))?;
    let role = oa_audit_core::authorize_principal(
        &registry,
        principal,
        oa_audit_core::Action::QuarantineRelease,
    )
    .map_err(|error| CliError::Runtime(format!("quarantine authorization: {error:?}")))?;
    if role_claim.is_some_and(|claim| claim != role) {
        return Err(CliError::Runtime(
            "quarantine role claim does not match pinned registry".into(),
        ));
    }
    Ok(registry)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => byte
            .checked_sub(b'a')
            .and_then(|value| value.checked_add(10)),
        b'A'..=b'F' => byte
            .checked_sub(b'A')
            .and_then(|value| value.checked_add(10)),
        _ => None,
    }
}

fn read_input(path: &Path, max_bytes: u64) -> Result<Vec<u8>, CliError> {
    let (file, len) = oa_sealed::open_regular(path).map_err(CliError::Io)?;
    if len > max_bytes {
        return Err(CliError::InputTooLarge { max_bytes });
    }
    let bytes = read_bounded_bytes(file, max_bytes)?;
    Ok(bytes)
}

fn read_bounded_bytes(reader: impl Read, max_bytes: u64) -> Result<Vec<u8>, CliError> {
    let take_bytes = max_bytes
        .checked_add(1)
        .ok_or(CliError::InputTooLarge { max_bytes })?;
    let mut bytes = Vec::new();
    reader
        .take(take_bytes)
        .read_to_end(&mut bytes)
        .map_err(CliError::Io)?;
    let byte_count =
        u64::try_from(bytes.len()).map_err(|_| CliError::InputTooLarge { max_bytes })?;
    if byte_count > max_bytes {
        return Err(CliError::InputTooLarge { max_bytes });
    }
    Ok(bytes)
}

fn count_nonempty_rows<'a>(lines: impl Iterator<Item = &'a str>) -> Result<u64, CliError> {
    let mut count = 0u64;
    for line in lines {
        if !line.is_empty() {
            count = count.checked_add(1).ok_or(CliError::TooManyEvents)?;
        }
    }
    Ok(count)
}

fn row_accounting(admitted: u64, unexamined: u64) -> Result<RowAccounting, CliError> {
    let denominator = admitted
        .checked_add(1)
        .and_then(|count| count.checked_add(unexamined))
        .ok_or(CliError::TooManyEvents)?;
    Ok(RowAccounting {
        admitted,
        refused: 1,
        unexamined,
        denominator,
    })
}

fn row_failure_accounting<'a>(
    cursor: &oa_regional_session::SessionCursor,
    remaining_lines: impl Iterator<Item = &'a str>,
) -> Result<RowAccounting, CliError> {
    row_accounting(
        session_total_events(cursor),
        count_nonempty_rows(remaining_lines)?,
    )
}

#[cfg(test)]
fn parse_input(bytes: &[u8]) -> Result<Vec<FrameEvidence>, CliError> {
    parse_input_with_limit(bytes, MAX_SESSION_EVENTS)
}

#[cfg(test)]
fn parse_input_with_limit(bytes: &[u8], max_events: usize) -> Result<Vec<FrameEvidence>, CliError> {
    parse_input_with_profile_limits(bytes, max_events, MAX_EVENTS_PER_RECEIPT)
}

fn parse_input_with_profile_limits(
    bytes: &[u8],
    max_events: usize,
    max_events_per_receipt: usize,
) -> Result<Vec<FrameEvidence>, CliError> {
    let text = std::str::from_utf8(bytes).map_err(|_| CliError::InvalidUtf8)?;
    let mut lines = text.split('\n');
    match lines.next() {
        Some("OA-RS\t1") => {}
        Some(_) => return Err(CliError::BadHeader),
        None => return Err(CliError::MissingHeader),
    }
    let mut cursor = session_cursor_new_with_profile_limits(max_events, max_events_per_receipt)
        .map_err(|_| CliError::TooManyEvents)?;
    let mut evidence = Vec::new();
    let mut line_number = 1usize;
    while let Some(raw) = lines.next() {
        line_number = line_number.checked_add(1).ok_or(CliError::TooManyEvents)?;
        if raw.is_empty() {
            continue;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.len() > MAX_EVENT_LINE_BYTES {
            let accounting = row_failure_accounting(&cursor, lines)?;
            return Err(CliError::BadLine {
                line: line_number,
                error: InputError::RowTooLong,
                accounting,
            });
        }
        if cursor_total(&cursor)
            >= u64::try_from(max_events).map_err(|_| CliError::TooManyEvents)?
        {
            let accounting = row_failure_accounting(&cursor, lines)?;
            return Err(CliError::Ordering {
                line: line_number,
                message: "session event capacity reached",
                accounting,
            });
        }
        let event = match parse_event_line(line) {
            Ok(event) => event,
            Err(error) => {
                let accounting = row_failure_accounting(&cursor, lines)?;
                return Err(CliError::BadLine {
                    line: line_number,
                    error,
                    accounting,
                });
            }
        };
        let completed = match push_event(&mut cursor, event) {
            Ok(completed) => completed,
            Err(error) => {
                let message = match error {
                    oa_regional_session::SessionError::CursorClosed => "session cursor is closed",
                    oa_regional_session::SessionError::InvalidTime => {
                        "event time is negative or source time is after receipt time"
                    }
                    oa_regional_session::SessionError::SealedTimestamp => {
                        "sealed source or receipt timestamp refused"
                    }
                    oa_regional_session::SessionError::ReceiptRegression => {
                        "receipt time moved backward"
                    }
                    oa_regional_session::SessionError::EqualTimeOrderRegression => {
                        "equal-time source order moved backward"
                    }
                    oa_regional_session::SessionError::SequenceRegression => {
                        "source sequence is not increasing"
                    }
                    oa_regional_session::SessionError::ReceiptGroupFull => {
                        "receipt group capacity reached"
                    }
                    oa_regional_session::SessionError::EventCountOverflow => {
                        "event counter overflow"
                    }
                    oa_regional_session::SessionError::SessionFull => {
                        "session event capacity reached"
                    }
                    oa_regional_session::SessionError::NoPendingGroup => {
                        "receipt group state invalid"
                    }
                };
                let accounting = row_failure_accounting(&cursor, lines)?;
                return Err(CliError::Ordering {
                    line: line_number,
                    message,
                    accounting,
                });
            }
        };
        if let Some(batch) = completed {
            evidence.push(
                summarize_input_batch_with_limit(&batch, max_events_per_receipt)
                    .map_err(|_| CliError::TooManyEvents)?,
            );
        }
    }
    if let Some(batch) = flush_receipt_group(&mut cursor).map_err(|_| CliError::TooManyEvents)? {
        evidence.push(
            summarize_input_batch_with_limit(&batch, max_events_per_receipt)
                .map_err(|_| CliError::TooManyEvents)?,
        );
    }
    Ok(evidence)
}

fn cursor_total(cursor: &oa_regional_session::SessionCursor) -> u64 {
    session_total_events(cursor)
}

fn write_report(path: &Path, frames: &[FrameEvidence]) -> Result<(), CliError> {
    oa_sealed::refuse_io(path).map_err(CliError::Io)?;
    if path.exists() {
        return Err(CliError::OutputExists);
    }
    fs::create_dir(path).map_err(CliError::Io)?;
    let report_path = path.join("session-report.tsv");
    oa_sealed::refuse_io(&report_path).map_err(CliError::Io)?;
    // seal-allow: create-new staging report output after its path passed the sealed guard
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&report_path)
        .map_err(CliError::Io)?;
    oa_sealed::Sealed::production()
        .verify_open(&report_path, &file)
        .map_err(CliError::Io)?;
    writeln!(file, "status\tSTAGED_INPUT_ONLY").map_err(CliError::Io)?;
    writeln!(file, "frame_count\t{}", frames.len()).map_err(CliError::Io)?;
    writeln!(
        file,
        "input_events\t{}",
        frames
            .iter()
            .map(|f| u64::from(f.input_events))
            .sum::<u64>()
    )
    .map_err(CliError::Io)?;
    writeln!(file, "subsystem_execution\tNOT_IMPLEMENTED").map_err(CliError::Io)?;
    writeln!(file, "receipt_ns\tinput_events\tsurveillance\tflight\troute\tweather\tadaptation\tsource_clock_regressions").map_err(CliError::Io)?;
    for frame in frames {
        write!(file, "{}\t{}", frame.receipt_ns, frame.input_events).map_err(CliError::Io)?;
        for count in frame.source_events {
            write!(file, "\t{count}").map_err(CliError::Io)?;
        }
        writeln!(file, "\t{}", frame.source_clock_regressions).map_err(CliError::Io)?;
    }
    Ok(())
}

fn run() -> Result<(), CliError> {
    let Arguments {
        input,
        output,
        model,
        safety,
        performance,
        options,
        quarantine_release,
    } = arguments()?;
    oa_sealed::refuse_io(&input).map_err(CliError::Io)?;
    oa_sealed::refuse_io(&output).map_err(CliError::Io)?;
    if let Some(oa_regional_session::cli::ModelSource::Snapshot(path)) = &model {
        oa_sealed::refuse_io(path).map_err(CliError::Io)?;
    }
    if let Some(oa_regional_session::cli::ModelSource::IssuedPackage { root, .. }) = &model {
        oa_sealed::refuse_io(root).map_err(CliError::Io)?;
    }
    if let Some(path) = &performance {
        oa_sealed::refuse_io(path).map_err(CliError::Io)?;
    }
    if let Some(request) = &quarantine_release {
        oa_sealed::refuse_io(&request.audit_log).map_err(CliError::Io)?;
    }
    let bytes = read_input(&input, input_byte_limit(options.profile))?;
    let max_events = match options.profile {
        oa_regional_session::cli::RegionalCliProfile::Default => MAX_SESSION_EVENTS,
        oa_regional_session::cli::RegionalCliProfile::RecordedPeakRendered
        | oa_regional_session::cli::RegionalCliProfile::RecordedHourRendered => {
            MAX_RECORDED_PROFILE_EVENTS
        }
    };
    let max_events_per_receipt = match options.profile {
        oa_regional_session::cli::RegionalCliProfile::RecordedHourRendered => {
            RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
        }
        oa_regional_session::cli::RegionalCliProfile::Default
        | oa_regional_session::cli::RegionalCliProfile::RecordedPeakRendered => {
            MAX_EVENTS_PER_RECEIPT
        }
    };
    let frames = parse_input_with_profile_limits(&bytes, max_events, max_events_per_receipt)?;
    match model {
        Some(source) => oa_regional_session::cli::run_session_with_options_and_quarantine_release(
            &bytes,
            &output,
            &source,
            &safety,
            performance.as_deref(),
            options,
            quarantine_release.as_ref(),
        )
        .map_err(CliError::Runtime),
        None => write_report(&output, &frames),
    }
}

fn main() {
    if let Err(error) = run() {
        let message = match error {
            CliError::Usage => "usage: oa-regional-session run --input FILE --out NEW_DIR (--snapshot FILE | --synthetic-model | --issued-package DIR --issued-manifest-sha256 PIN --processor-id ID) [--msaw-inputs MANIFEST | --synthetic-msaw] [--performance MANIFEST] [--profile recorded-peak-rendered|recorded-hour-rendered --replay-start-ns NS --replay-end-ns NS [--replay-warmup-start-ns NS]] [--quarantine-release-source flight-data|surveillance --quarantine-release-instance N --quarantine-release-item N --manual-handoff-ref TEXT --audit-registry FILE --audit-registry-sha256 SHA256 --audit-log FILE --audit-count N --audit-tail SHA256 --audit-principal N [--audit-role operate|configure] --audit-time-ns NS]; stage --input FILE --out NEW_DIR".to_owned(),
            CliError::Io(error) => format!("I/O error: {error}"),
            CliError::InputTooLarge { max_bytes } => format!(
                "input exceeds the {} MiB profile limit",
                max_bytes / (1024 * 1024)
            ),
            CliError::InvalidUtf8 => "input is not UTF-8".to_owned(),
            CliError::BadHeader => "expected header OA-RS<TAB>1".to_owned(),
            CliError::MissingHeader => "input header is missing".to_owned(),
            CliError::BadLine { line, error, accounting } => format!(
                "invalid protocol row at line {line}: {error:?}; normalized_event_rows admitted={} refused={} unexamined={} denominator={}",
                accounting.admitted, accounting.refused, accounting.unexamined, accounting.denominator,
            ),
            CliError::Ordering { line, message, accounting } => format!(
                "{message} at line {line}; normalized_event_rows admitted={} refused={} unexamined={} denominator={}",
                accounting.admitted, accounting.refused, accounting.unexamined, accounting.denominator,
            ),
            CliError::OutputExists => "output directory must not already exist".to_owned(),
            CliError::TooManyEvents => "session exceeds a bounded event or counter limit".to_owned(),
            CliError::Runtime(message) => message,
        };
        eprintln!("oa-regional-session: {message}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        input_byte_limit, load_quarantine_principal_registry, parse_cli_profile, parse_input,
        parse_input_with_limit, parse_input_with_profile_limits, read_bounded_bytes, read_input,
        session_options, session_options_with_warmup, CliError, FrameEvidence, InputError,
        RowAccounting, MAX_EVENTS_PER_RECEIPT, MAX_INPUT_BYTES, MAX_RECORDED_HOUR_INPUT_BYTES,
        MAX_RECORDED_PROFILE_EVENTS, RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
    };
    use oa_regional_session::cli::RegionalCliProfile;
    use oa_regional_session::surveillance;
    use std::ffi::OsString;
    use std::fs::OpenOptions;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const ROW: &str = "v1\t1000\t900\t{sequence}\tADAPTATION\t00000000000000000000000000000385\tABC123\tZAU1\tFEED\tUP\t1";
    static REGISTRY_TEST_ID: AtomicU64 = AtomicU64::new(0);
    static INPUT_TEST_ID: AtomicU64 = AtomicU64::new(0);
    const HOUR_NS: i64 = 3_600 * 1_000_000_000;

    fn input_test_path(label: &str) -> PathBuf {
        let id = INPUT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "oa-regional-input-{label}-{}-{id}.bin",
            std::process::id()
        ))
    }

    // REQ: OA-REG-019
    #[test]
    fn recorded_hour_has_separate_byte_cap_and_requires_one_exact_window() {
        let default = RegionalCliProfile::Default;
        let peak = parse_cli_profile("recorded-peak-rendered").expect("legacy profile parses");
        let hour = parse_cli_profile("recorded-hour-rendered").expect("hour profile parses");
        assert_eq!(input_byte_limit(default), MAX_INPUT_BYTES);
        assert_eq!(input_byte_limit(peak), MAX_INPUT_BYTES);
        assert_eq!(input_byte_limit(hour), MAX_RECORDED_HOUR_INPUT_BYTES);
        assert!(session_options(default, None, None).is_ok());
        assert!(session_options(peak, None, None).is_ok());
        assert!(session_options(hour, None, None).is_err());
        assert!(session_options(default, Some(0), Some(HOUR_NS)).is_err());
        assert!(session_options(hour, Some(0), None).is_err());
        assert!(session_options(hour, Some(0), Some(HOUR_NS)).is_ok());
        assert!(parse_cli_profile("unbounded").is_err());
    }

    // REQ: OA-REG-019
    #[test]
    fn replay_cli_parser_requires_both_window_flags_only_for_hour_profile() {
        let mut replay = super::ReplayArgs::default();
        let mut profile_value = vec![OsString::from("recorded-hour-rendered")].into_iter();
        super::parse_replay_option("--profile", &mut profile_value, &mut replay)
            .expect("synthetic test value");
        let mut start_value = vec![OsString::from("0")].into_iter();
        super::parse_replay_option("--replay-start-ns", &mut start_value, &mut replay)
            .expect("synthetic test value");
        let mut end_value = vec![OsString::from(HOUR_NS.to_string())].into_iter();
        super::parse_replay_option("--replay-end-ns", &mut end_value, &mut replay)
            .expect("synthetic test value");
        let options = session_options(
            replay.profile.expect("synthetic test value"),
            replay.start_ns,
            replay.end_ns,
        )
        .expect("hour profile has one validated half-open window");
        assert_eq!(options.profile, RegionalCliProfile::RecordedHourRendered);
        assert!(options.recorded_hour.is_some());

        assert!(session_options(RegionalCliProfile::RecordedHourRendered, None, None).is_err());
        assert!(session_options(RegionalCliProfile::Default, Some(0), Some(HOUR_NS)).is_err());
        assert!(session_options(RegionalCliProfile::Default, Some(0), None).is_err());
        assert!(session_options(RegionalCliProfile::Default, None, Some(HOUR_NS)).is_err());
        let mut duplicate_profile = vec![OsString::from("recorded-peak-rendered")].into_iter();
        assert!(
            super::parse_replay_option("--profile", &mut duplicate_profile, &mut replay).is_err()
        );
    }

    // REQ: OA-REG-019. Mutation killed: allowing an unbounded or display-after warmup interval.
    #[test]
    fn warmup_option_requires_a_bounded_whole_second_recorded_window() {
        let display_start = HOUR_NS * 4;
        let display_end = display_start + HOUR_NS;
        let warmup_start = display_start - 600 * surveillance::SECOND_NS;
        let options = session_options_with_warmup(
            RegionalCliProfile::RecordedHourRendered,
            Some(display_start),
            Some(display_end),
            Some(warmup_start),
        )
        .expect("ten-minute warmup interval");
        let window = options.recorded_hour.expect("recorded-hour selection");
        assert_eq!(window.warmup_start_ns, Some(warmup_start));
        assert_eq!(
            ((window.start_ns - window.warmup_start_ns.expect("parsed warmup start"))
                / surveillance::SECOND_NS) as usize,
            ((display_start - warmup_start) / surveillance::SECOND_NS) as usize
        );
        assert_eq!(
            window.frame_count(),
            ((display_end - display_start) / surveillance::SECOND_NS) as usize
        );
        assert!(session_options_with_warmup(
            RegionalCliProfile::RecordedHourRendered,
            Some(display_start),
            Some(display_end),
            Some(warmup_start - surveillance::SECOND_NS),
        )
        .is_err());
        assert!(session_options_with_warmup(
            RegionalCliProfile::RecordedHourRendered,
            Some(display_start),
            Some(display_end),
            Some(display_start + surveillance::SECOND_NS),
        )
        .is_err());
        assert!(session_options_with_warmup(
            RegionalCliProfile::RecordedHourRendered,
            Some(display_start),
            Some(display_end),
            Some(warmup_start + 1),
        )
        .is_err());
        assert!(session_options_with_warmup(
            RegionalCliProfile::RecordedHourRendered,
            None,
            None,
            Some(warmup_start),
        )
        .is_err());
    }

    // REQ: OA-REG-019. Mutation killed: silently ignoring the explicit warmup CLI flag.
    #[test]
    fn replay_cli_parser_preserves_explicit_warmup_start() {
        let mut replay = super::ReplayArgs::default();
        let warmup_start_ns = 1_700_000_000_000_000_000i64;
        let mut warmup = vec![OsString::from(warmup_start_ns.to_string())].into_iter();
        super::parse_replay_option("--replay-warmup-start-ns", &mut warmup, &mut replay)
            .expect("synthetic warmup flag");
        assert_eq!(replay.warmup_start_ns, Some(warmup_start_ns));
        assert!(super::parse_replay_option(
            "--replay-warmup-start-ns",
            &mut Vec::<OsString>::new().into_iter(),
            &mut replay,
        )
        .is_err());
    }

    // REQ: OA-REG-019
    #[test]
    fn bounded_input_reader_accepts_exact_limit_and_refuses_one_more_byte() {
        let limit = 4_u64;
        assert_eq!(
            read_bounded_bytes(Cursor::new(b"abcd"), limit).expect("synthetic test value"),
            b"abcd"
        );
        assert!(matches!(
            read_bounded_bytes(Cursor::new(b"abcde"), limit),
            Err(CliError::InputTooLarge { max_bytes }) if max_bytes == limit
        ));
    }

    // REQ: OA-REG-019
    #[test]
    fn recorded_hour_reader_admits_more_than_legacy_cap_and_refuses_its_own_cap() {
        let path = input_test_path("large");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create synthetic sparse input");
        file.set_len(MAX_INPUT_BYTES + 1)
            .expect("set synthetic input length");
        drop(file);

        assert!(matches!(
            read_input(&path, input_byte_limit(RegionalCliProfile::Default)),
            Err(CliError::InputTooLarge { max_bytes }) if max_bytes == MAX_INPUT_BYTES
        ));
        let hour_bytes = read_input(
            &path,
            input_byte_limit(RegionalCliProfile::RecordedHourRendered),
        )
        .expect("hour profile accepts the same bounded input");
        assert_eq!(
            u64::try_from(hour_bytes.len()).expect("synthetic test value"),
            MAX_INPUT_BYTES + 1
        );
        drop(hour_bytes);
        std::fs::remove_file(&path).expect("remove synthetic input");

        let oversized_path = input_test_path("over-cap");
        let oversized = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&oversized_path)
            .expect("create synthetic over-cap input");
        oversized
            .set_len(MAX_RECORDED_HOUR_INPUT_BYTES + 1)
            .expect("set synthetic over-cap length");
        drop(oversized);
        assert!(matches!(
            read_input(
                &oversized_path,
                input_byte_limit(RegionalCliProfile::RecordedHourRendered)
            ),
            Err(CliError::InputTooLarge { max_bytes })
                if max_bytes == MAX_RECORDED_HOUR_INPUT_BYTES
        ));
        std::fs::remove_file(oversized_path).expect("remove synthetic over-cap input");
    }

    // REQ: OA-AUDIT-002, OA-AUDIT-008
    #[test]
    fn quarantine_cli_derives_release_role_from_pinned_registry() {
        let id = REGISTRY_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "oa-regional-quarantine-registry-{}-{id}.tsv",
            std::process::id()
        ));
        let bytes = b"OACERT-LOCAL-PRINCIPALS-V1\n51\toperate\n52\tmonitor\n";
        std::fs::write(&path, bytes).expect("write synthetic registry");
        let pin = oa_sha256::sha256(bytes);
        assert!(load_quarantine_principal_registry(
            &path,
            pin,
            51,
            Some(oa_audit_core::Role::Operate)
        )
        .is_ok());
        assert!(load_quarantine_principal_registry(
            &path,
            pin,
            51,
            Some(oa_audit_core::Role::Monitor)
        )
        .is_err());
        assert!(load_quarantine_principal_registry(
            &path,
            pin,
            52,
            Some(oa_audit_core::Role::Monitor)
        )
        .is_err());
        assert!(load_quarantine_principal_registry(&path, [0; 32], 51, None).is_err());
        let _ = std::fs::remove_file(path);
    }

    fn bad_line_accounting(
        result: Result<Vec<FrameEvidence>, CliError>,
        expected: InputError,
    ) -> RowAccounting {
        match result {
            Err(CliError::BadLine {
                error, accounting, ..
            }) => {
                assert_eq!(error, expected);
                accounting
            }
            _ => RowAccounting {
                admitted: u64::MAX,
                refused: 0,
                unexamined: 0,
                denominator: 0,
            },
        }
    }

    fn ordering_accounting(
        result: Result<Vec<FrameEvidence>, CliError>,
        expected: &'static str,
    ) -> RowAccounting {
        match result {
            Err(CliError::Ordering {
                message,
                accounting,
                ..
            }) => {
                assert_eq!(message, expected);
                accounting
            }
            _ => RowAccounting {
                admitted: u64::MAX,
                refused: 0,
                unexamined: 0,
                denominator: 0,
            },
        }
    }

    // REQ: OA-REG-019
    #[test]
    fn selected_event_limit_controls_parser_capacity_without_changing_the_default() {
        let first = ROW.replace("{sequence}", "1");
        let second = ROW.replace("{sequence}", "2");
        let input = format!("OA-RS\t1\n{first}\n{second}\n");
        assert!(parse_input_with_limit(input.as_bytes(), 2).is_ok());
        assert_eq!(
            ordering_accounting(
                parse_input_with_limit(input.as_bytes(), 1),
                "session event capacity reached"
            ),
            RowAccounting {
                admitted: 1,
                refused: 1,
                unexamined: 0,
                denominator: 2,
            }
        );
        assert!(parse_input(input.as_bytes()).is_ok());
    }

    #[test]
    fn recorded_hour_preflight_accepts_dense_receipt_and_default_refuses_it() {
        let rows = (1..=MAX_EVENTS_PER_RECEIPT + 1)
            .map(|sequence| ROW.replace("{sequence}", &sequence.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        let input = format!("OA-RS\t1\n{rows}\n");
        let hour = parse_input_with_profile_limits(
            input.as_bytes(),
            MAX_RECORDED_PROFILE_EVENTS,
            RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
        )
        .expect("hour preflight uses the explicit larger receipt bound");
        assert_eq!(hour.len(), 1);
        assert_eq!(
            hour.first()
                .expect("the dense receipt emits one frame")
                .input_events as usize,
            MAX_EVENTS_PER_RECEIPT + 1
        );

        assert_eq!(
            ordering_accounting(
                parse_input_with_profile_limits(
                    input.as_bytes(),
                    MAX_RECORDED_PROFILE_EVENTS,
                    MAX_EVENTS_PER_RECEIPT,
                ),
                "receipt group capacity reached"
            ),
            RowAccounting {
                admitted: MAX_EVENTS_PER_RECEIPT as u64,
                refused: 1,
                unexamined: 0,
                denominator: (MAX_EVENTS_PER_RECEIPT + 1) as u64,
            }
        );
    }

    #[test]
    fn malformed_row_counts_the_refusal_and_unexamined_suffix() {
        let first = ROW.replace("{sequence}", "1");
        let last = ROW.replace("{sequence}", "2");
        let input = format!("OA-RS\t1\n{first}\nbad\n{last}\n");
        assert_eq!(
            bad_line_accounting(parse_input(input.as_bytes()), InputError::BadFieldCount),
            RowAccounting {
                admitted: 1,
                refused: 1,
                unexamined: 1,
                denominator: 3,
            }
        );
    }

    #[test]
    fn misordered_row_counts_the_refusal_and_unexamined_suffix() {
        let first = ROW.replace("1000", "2000").replace("{sequence}", "1");
        let backwards = ROW.replace("{sequence}", "2");
        let later = ROW.replace("1000", "3000").replace("{sequence}", "3");
        let input = format!("OA-RS\t1\n{first}\n{backwards}\n{later}\n");
        assert_eq!(
            ordering_accounting(parse_input(input.as_bytes()), "receipt time moved backward"),
            RowAccounting {
                admitted: 1,
                refused: 1,
                unexamined: 1,
                denominator: 3,
            }
        );
    }

    #[test]
    fn receipt_group_limit_counts_the_refusal_and_unexamined_suffix() {
        let mut input = String::from("OA-RS\t1\n");
        for sequence in 1..=256 {
            input.push_str(&ROW.replace("{sequence}", &sequence.to_string()));
            input.push('\n');
        }
        input.push_str(&ROW.replace("{sequence}", "257"));
        input.push('\n');
        input.push_str(&ROW.replace("{sequence}", "258"));
        input.push('\n');
        assert_eq!(
            ordering_accounting(
                parse_input(input.as_bytes()),
                "receipt group capacity reached"
            ),
            RowAccounting {
                admitted: 256,
                refused: 1,
                unexamined: 1,
                denominator: 258,
            }
        );
    }
}

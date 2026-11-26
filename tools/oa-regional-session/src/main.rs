#![forbid(unsafe_code)]
//! CLI shell for normalized regional-session input staging and composed offline execution.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use oa_regional_session::{
    flush_receipt_group, parse_event_line, push_event, row_matches_stream_version,
    session_cursor_new_with_limit, session_total_events, stream_row_version, summarize_input_batch,
    FrameEvidence, InputError, MAX_EVENT_LINE_BYTES, MAX_RECORDED_PROFILE_EVENTS,
    MAX_SESSION_EVENTS,
};

const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;

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
    InputTooLarge,
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
    profile: oa_regional_session::cli::RegionalCliProfile,
    quarantine_release: Option<oa_regional_session::cli::QuarantineReleaseOptions>,
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
    let mut safety = None;
    let mut performance = None;
    let mut profile = None;
    let mut quarantine_source = None;
    let mut quarantine_instance = None;
    let mut quarantine_item = None;
    let mut manual_handoff_ref = None;
    let mut audit_log = None;
    let mut audit_count = None;
    let mut audit_tail = None;
    let mut audit_principal = None;
    let mut audit_role = None;
    let mut audit_time_ns = None;
    let mut principal_registry_path = None;
    let mut principal_registry_sha256 = None;
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
            Some("--profile") if execute && profile.is_none() => {
                profile = Some(
                    match args
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .as_deref()
                    {
                        Some("recorded-peak-rendered") => {
                            oa_regional_session::cli::RegionalCliProfile::RecordedPeakRendered
                        }
                        _ => return Err(CliError::Usage),
                    },
                );
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
            Some("--principal-registry") if execute && principal_registry_path.is_none() => {
                principal_registry_path =
                    Some(args.next().map(PathBuf::from).ok_or(CliError::Usage)?);
            }
            Some("--principal-registry-sha256")
                if execute && principal_registry_sha256.is_none() =>
            {
                principal_registry_sha256 = Some(
                    args.next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| parse_audit_digest(&value).ok())
                        .ok_or(CliError::Usage)?,
                );
            }
            _ => return Err(CliError::Usage),
        }
    }
    let input = input.ok_or(CliError::Usage)?;
    let output = output.ok_or(CliError::Usage)?;
    if execute && model.is_none() {
        return Err(CliError::Usage);
    }
    let any_quarantine_release = quarantine_source.is_some()
        || quarantine_instance.is_some()
        || quarantine_item.is_some()
        || manual_handoff_ref.is_some()
        || audit_log.is_some()
        || audit_count.is_some()
        || audit_tail.is_some()
        || audit_principal.is_some()
        || audit_role.is_some()
        || audit_time_ns.is_some()
        || principal_registry_path.is_some()
        || principal_registry_sha256.is_some();
    let quarantine_release = if any_quarantine_release {
        let source = quarantine_source.ok_or(CliError::Usage)?;
        let source_instance = quarantine_instance.ok_or(CliError::Usage)?;
        let item_id = quarantine_item.ok_or(CliError::Usage)?;
        let registry_path = principal_registry_path.ok_or(CliError::Usage)?;
        let registry_pin = principal_registry_sha256.ok_or(CliError::Usage)?;
        oa_sealed::Sealed::production()
            .check(&registry_path)
            .map_err(|_| CliError::Usage)?;
        let principal_registry =
            oa_audit_log::shell::load_principal_registry(&registry_path, registry_pin)
                .map_err(|_| CliError::Usage)?;
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
            principal: audit_principal.ok_or(CliError::Usage)?,
            role: audit_role.ok_or(CliError::Usage)?,
            principal_registry,
            time_ns: audit_time_ns.ok_or(CliError::Usage)?,
        })
    } else {
        None
    };
    Ok(Arguments {
        input,
        output,
        model,
        safety: safety.unwrap_or(oa_regional_session::cli::MsawSource::Disabled),
        performance,
        profile: profile.unwrap_or(oa_regional_session::cli::RegionalCliProfile::Default),
        quarantine_release,
    })
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

fn read_input(path: &Path) -> Result<Vec<u8>, CliError> {
    let (file, len) = oa_sealed::open_regular(path).map_err(CliError::Io)?;
    if len > MAX_INPUT_BYTES {
        return Err(CliError::InputTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(CliError::Io)?;
    let byte_count = u64::try_from(bytes.len()).map_err(|_| CliError::InputTooLarge)?;
    if byte_count > MAX_INPUT_BYTES {
        return Err(CliError::InputTooLarge);
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

fn parse_input_with_limit(bytes: &[u8], max_events: usize) -> Result<Vec<FrameEvidence>, CliError> {
    let text = std::str::from_utf8(bytes).map_err(|_| CliError::InvalidUtf8)?;
    let mut lines = text.split('\n');
    let header = lines.next().ok_or(CliError::MissingHeader)?;
    let row_version = stream_row_version(header).ok_or(CliError::BadHeader)?;
    let mut cursor =
        session_cursor_new_with_limit(max_events).map_err(|_| CliError::TooManyEvents)?;
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
        if !row_matches_stream_version(line, row_version) {
            let accounting = row_failure_accounting(&cursor, lines)?;
            return Err(CliError::BadLine {
                line: line_number,
                error: InputError::BadVersion,
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
            evidence.push(summarize_input_batch(&batch).map_err(|_| CliError::TooManyEvents)?);
        }
    }
    if let Some(batch) = flush_receipt_group(&mut cursor).map_err(|_| CliError::TooManyEvents)? {
        evidence.push(summarize_input_batch(&batch).map_err(|_| CliError::TooManyEvents)?);
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
        profile,
        quarantine_release,
    } = arguments()?;
    oa_sealed::refuse_io(&input).map_err(CliError::Io)?;
    oa_sealed::refuse_io(&output).map_err(CliError::Io)?;
    if let Some(oa_regional_session::cli::ModelSource::Snapshot(path)) = &model {
        oa_sealed::refuse_io(path).map_err(CliError::Io)?;
    }
    if let Some(path) = &performance {
        oa_sealed::refuse_io(path).map_err(CliError::Io)?;
    }
    if let Some(request) = &quarantine_release {
        oa_sealed::refuse_io(&request.audit_log).map_err(CliError::Io)?;
    }
    let bytes = read_input(&input)?;
    let max_events = match profile {
        oa_regional_session::cli::RegionalCliProfile::Default => MAX_SESSION_EVENTS,
        oa_regional_session::cli::RegionalCliProfile::RecordedPeakRendered => {
            MAX_RECORDED_PROFILE_EVENTS
        }
    };
    let frames = parse_input_with_limit(&bytes, max_events)?;
    match model {
        Some(source) => {
            oa_regional_session::cli::run_session_with_profile_options_and_quarantine_release(
                &bytes,
                &output,
                &source,
                &safety,
                performance.as_deref(),
                profile,
                quarantine_release.as_ref(),
            )
            .map_err(CliError::Runtime)
        }
        None => write_report(&output, &frames),
    }
}

fn main() {
    if let Err(error) = run() {
        let message = match error {
            CliError::Usage => "usage: oa-regional-session run --input FILE --out NEW_DIR (--snapshot FILE | --synthetic-model) [--msaw-inputs MANIFEST | --synthetic-msaw] [--performance MANIFEST] [--profile recorded-peak-rendered] [--quarantine-release-source flight-data|surveillance --quarantine-release-instance N --quarantine-release-item N --manual-handoff-ref TEXT --audit-log FILE --audit-count N --audit-tail SHA256 --audit-principal N --audit-role monitor|operate|configure --audit-time-ns N --principal-registry FILE --principal-registry-sha256 SHA256]; stage --input FILE --out NEW_DIR".to_owned(),
            CliError::Io(error) => format!("I/O error: {error}"),
            CliError::InputTooLarge => "input exceeds the 16 MiB limit".to_owned(),
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
        parse_input, parse_input_with_limit, CliError, FrameEvidence, InputError, RowAccounting,
    };

    const ROW: &str = "v1\t1000\t900\t{sequence}\tADAPTATION\t00000000000000000000000000000385\tABC123\tZAU1\tFEED\tUP\t1";

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
    fn normalized_stream_header_and_rows_must_share_the_declared_version() {
        let v1 = ROW.replace("{sequence}", "1");
        let v2 = v1.replacen("v1\t", "v2\t", 1);
        assert!(parse_input_with_limit(format!("OA-RS\t1\n{v1}\n").as_bytes(), 2).is_ok());
        assert!(parse_input_with_limit(format!("OA-RS\t2\n{v2}\n").as_bytes(), 2).is_ok());
        assert_eq!(
            bad_line_accounting(
                parse_input_with_limit(format!("OA-RS\t1\n{v2}\n").as_bytes(), 2),
                InputError::BadVersion,
            )
            .admitted,
            0
        );
        assert_eq!(
            bad_line_accounting(
                parse_input_with_limit(format!("OA-RS\t2\n{v1}\n").as_bytes(), 2),
                InputError::BadVersion,
            )
            .admitted,
            0
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

//! Bounded audit log verifier shell, with an opt-in operator-configured watch mode.
use oa_audit_core::{AuditError, Checkpoint};
use oa_audit_log::shell::{self, LogError, MAX_RECORDS};
use oa_techops::{
    record_alarm_transition, Alarm, AlarmSeverity, AlarmState, AlarmTime, AlarmTransition,
    ClockDomainId, ResourceKey, ResourceKind,
};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_ARGUMENT_BYTES: usize = 4096;
const MAX_CONFIG_BYTES: usize = 256;
const MAX_WATCH_POLLS: u64 = 86_400;
const MAX_WATCH_RUNTIME: Duration = Duration::from_secs(86_400);
const MAX_OUTPUT_BYTES: usize = 1_048_576;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const ALARM_COUNT: usize = 5;
const ALARM_CHAIN_BROKEN: u64 = 1;
const ALARM_ROLE_ACTION: u64 = 2;
const ALARM_STALE_CONFIG: u64 = 3;
const ALARM_CHECKPOINT_DIVERGED: u64 = 4;
const ALARM_MONITOR_UNAVAILABLE: u64 = 5;
const REASON_CHAIN_BROKEN: u16 = 0xA101;
const REASON_ROLE_ACTION: u16 = 0xA102;
const REASON_STALE_CONFIG: u16 = 0xA103;
const REASON_CHECKPOINT_DIVERGED: u16 = 0xA104;
const REASON_MONITOR_UNAVAILABLE: u16 = 0xA105;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WatchConfig {
    generation: u64,
    checkpoint: Checkpoint,
    refresh_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchStatus {
    Valid,
    GenerationRegression,
    GenerationChangedWithoutAdvance,
    GenerationAdvancedUnverified,
    CheckpointAhead,
    CheckpointUnmatched,
    IncompleteRecord,
    ChainBroken,
    WrongRoleAction,
    LimitReached,
    VerificationUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WatchSample {
    status: WatchStatus,
    verified_checkpoint: Option<Checkpoint>,
    expected: Option<Checkpoint>,
}

type MismatchSignature = (u64, WatchStatus, Option<Checkpoint>);

#[derive(Clone, Copy)]
struct AlarmSpec {
    instance: u64,
    reason: u16,
    severity: AlarmSeverity,
}

const ALARM_SPECS: [AlarmSpec; ALARM_COUNT] = [
    AlarmSpec {
        instance: ALARM_CHAIN_BROKEN,
        reason: REASON_CHAIN_BROKEN,
        severity: AlarmSeverity::Critical,
    },
    AlarmSpec {
        instance: ALARM_ROLE_ACTION,
        reason: REASON_ROLE_ACTION,
        severity: AlarmSeverity::Critical,
    },
    AlarmSpec {
        instance: ALARM_STALE_CONFIG,
        reason: REASON_STALE_CONFIG,
        severity: AlarmSeverity::Warning,
    },
    AlarmSpec {
        instance: ALARM_CHECKPOINT_DIVERGED,
        reason: REASON_CHECKPOINT_DIVERGED,
        severity: AlarmSeverity::Warning,
    },
    AlarmSpec {
        instance: ALARM_MONITOR_UNAVAILABLE,
        reason: REASON_MONITOR_UNAVAILABLE,
        severity: AlarmSeverity::Critical,
    },
];

const EMPTY_ALARM: Alarm = Alarm {
    instance_id: 0,
    resource: ResourceKey {
        resource_id: 0,
        parent_service_id: 0,
        kind: ResourceKind::Service,
    },
    reason_code: 0,
    severity: AlarmSeverity::Informational,
    state: AlarmState::Empty,
    clock_domain: ClockDomainId(1),
    raised_at_ns: 0,
    cleared_at_ns: 0,
    acknowledged_at_ns: 0,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("oa-audit-verify: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && args.first().map(String::as_str) == Some("--help") {
        print!("{}\n{}\n", usage(), watch_help());
        return Ok(());
    }
    if args
        .iter()
        .any(|argument| argument.len() > MAX_ARGUMENT_BYTES)
    {
        return Err("an argument exceeds the 4096-byte limit".into());
    }
    if args.first().map(String::as_str) == Some("--watch") {
        return run_watch(&args);
    }
    run_once(&args)
}

fn run_once(args: &[String]) -> Result<(), String> {
    if args.len() != 6
        || args.first().map(String::as_str) != Some("--log")
        || args.get(2).map(String::as_str) != Some("--count")
        || args.get(4).map(String::as_str) != Some("--tail")
    {
        return Err(usage().into());
    }
    let path = args.get(1).ok_or_else(|| usage().to_owned())?;
    let count = args
        .get(3)
        .ok_or_else(|| usage().to_owned())?
        .parse::<u64>()
        .map_err(|_| "record count must be an unsigned integer".to_owned())?;
    let tail_text = args.get(5).ok_or_else(|| usage().to_owned())?;
    let checkpoint = Checkpoint {
        count,
        tail: parse_digest(tail_text)?,
    };
    let path = Path::new(path);
    oa_sealed::Sealed::production()
        .check(path)
        .map_err(|refusal| refusal.to_string())?;
    match shell::verify(path, checkpoint) {
        Ok(actual) => {
            println!(
                "valid\t{}\t{}",
                actual.count,
                oa_sha256::to_hex(&actual.tail)
            );
            Ok(())
        }
        Err(error) => Err(format_log_error(&error)),
    }
}

fn run_watch(args: &[String]) -> Result<(), String> {
    if args.len() != 9
        || args.get(1).map(String::as_str) != Some("--log")
        || args.get(3).map(String::as_str) != Some("--checkpoint-config")
        || args.get(5).map(String::as_str) != Some("--max-polls")
        || args.get(7).map(String::as_str) != Some("--max-records")
    {
        return Err(watch_usage().into());
    }
    let log_path = PathBuf::from(args.get(2).ok_or_else(|| watch_usage().to_owned())?);
    let config_path = PathBuf::from(args.get(4).ok_or_else(|| watch_usage().to_owned())?);
    let max_polls = parse_bounded_u64(
        args.get(6).ok_or_else(|| watch_usage().to_owned())?,
        1,
        MAX_WATCH_POLLS,
        "max polls",
    )?;
    let max_records = parse_bounded_u64(
        args.get(8).ok_or_else(|| watch_usage().to_owned())?,
        1,
        MAX_RECORDS,
        "max records",
    )?;
    let started = Instant::now();
    let mut alarms = [EMPTY_ALARM; ALARM_COUNT];
    let mut alarm_generations = [0u32; ALARM_COUNT];
    let mut output_bytes = 0usize;
    let mut accepted_config: Option<WatchConfig> = None;
    let mut generation_seen_at = started;
    let mut mismatch_signature: Option<(MismatchSignature, u8)> = None;
    let mut mismatch_count: u8;
    let mut active_alarms = [false; ALARM_COUNT];

    for poll in 0..max_polls {
        if started.elapsed() > MAX_WATCH_RUNTIME {
            return Err("watch exceeded the 24-hour runtime limit".into());
        }
        let now = Instant::now();
        let now_ns = elapsed_ns(started, now)?;
        let config_result = read_watch_config(&config_path);
        let config_value = config_result.as_ref().ok().copied();
        let (sample, generation, config_refused) = match config_result {
            Ok(config) => {
                let sample = inspect_log(&log_path, config, max_records);
                (sample, Some(config.generation), false)
            }
            Err(error) => {
                write_bounded_line(
                    &mut output_bytes,
                    &format!("poll\t{}\tconfig_refused\t{}", poll + 1, error),
                )?;
                (
                    WatchSample {
                        status: WatchStatus::VerificationUnavailable,
                        verified_checkpoint: None,
                        expected: None,
                    },
                    None,
                    true,
                )
            }
        };

        let sample = if let Some(config) = config_value {
            let rejected = rejected_config_status(accepted_config, config, sample.status);
            if let Some(status) = rejected {
                WatchSample {
                    status,
                    verified_checkpoint: None,
                    expected: Some(config.checkpoint),
                }
            } else {
                if sample.status == WatchStatus::Valid
                    && accepted_config
                        .is_none_or(|accepted| config.generation > accepted.generation)
                {
                    accepted_config = Some(config);
                    generation_seen_at = now;
                }
                sample
            }
        } else {
            sample
        };

        let stale = is_stale(
            accepted_config
                .filter(|config| config.refresh_ms > 0)
                .map(|config| config.refresh_ms),
            generation_seen_at,
            now,
        );
        if let Some(config) = config_value {
            if let Some((signature, count)) = mismatch_signature {
                let current_signature = (config.generation, sample.status, sample.expected);
                if signature == current_signature {
                    mismatch_count = count.saturating_add(1);
                } else {
                    mismatch_signature = Some((current_signature, 1));
                    mismatch_count = 1;
                }
            } else if matches!(
                sample.status,
                WatchStatus::CheckpointAhead | WatchStatus::CheckpointUnmatched
            ) {
                mismatch_signature = Some(((config.generation, sample.status, sample.expected), 1));
                mismatch_count = 1;
            } else {
                mismatch_signature = None;
                mismatch_count = 0;
            }
        } else {
            mismatch_signature = None;
            mismatch_count = 0;
        }
        let divergence = mismatch_count >= 3;
        let chain_broken = sample.status == WatchStatus::ChainBroken;
        let wrong_role = sample.status == WatchStatus::WrongRoleAction;
        let unavailable = config_refused
            || matches!(
                sample.status,
                WatchStatus::VerificationUnavailable
                    | WatchStatus::LimitReached
                    | WatchStatus::GenerationRegression
                    | WatchStatus::GenerationChangedWithoutAdvance
                    | WatchStatus::GenerationAdvancedUnverified
            );
        let verified_fresh_pin = sample.status == WatchStatus::Valid;
        let [old_chain, old_role, old_stale, old_divergence, old_unavailable] = active_alarms;
        active_alarms = [
            chain_broken || (!verified_fresh_pin && old_chain),
            wrong_role || (!verified_fresh_pin && old_role),
            stale
                || (config_refused
                    || sample.status == WatchStatus::GenerationRegression
                    || sample.status == WatchStatus::GenerationChangedWithoutAdvance
                    || sample.status == WatchStatus::GenerationAdvancedUnverified)
                    && old_stale,
            divergence || (!verified_fresh_pin && old_divergence),
            unavailable || (!verified_fresh_pin && old_unavailable),
        ];
        update_alarms(
            &mut alarms,
            &mut alarm_generations,
            active_alarms,
            now_ns,
            &mut output_bytes,
        )?;
        let status = if stale {
            "stale_checkpoint_contract"
        } else if divergence {
            "checkpoint_diverged"
        } else {
            status_name(sample.status)
        };
        let verified_count = sample
            .verified_checkpoint
            .map_or_else(|| "-".into(), |cp| cp.count.to_string());
        let expected_count = sample
            .expected
            .map_or_else(|| "-".into(), |cp| cp.count.to_string());
        write_bounded_line(
            &mut output_bytes,
            &format!(
                "poll\t{}\t{}\tverified_count={}\texpected_count={}\tgeneration={}",
                poll + 1,
                status,
                verified_count,
                expected_count,
                generation.map_or_else(|| "-".into(), |value| value.to_string())
            ),
        )?;
        if poll + 1 < max_polls {
            std::thread::sleep(POLL_INTERVAL);
        }
    }
    Ok(())
}

fn inspect_log(log_path: &Path, config: WatchConfig, max_records: u64) -> WatchSample {
    if config.checkpoint.count > max_records {
        return WatchSample {
            status: WatchStatus::LimitReached,
            verified_checkpoint: None,
            expected: Some(config.checkpoint),
        };
    }
    let result = shell::verify(log_path, config.checkpoint);
    match result {
        Ok(verified_checkpoint) => WatchSample {
            status: WatchStatus::Valid,
            verified_checkpoint: Some(verified_checkpoint),
            expected: Some(config.checkpoint),
        },
        Err(LogError::IncompleteRecord { .. }) => WatchSample {
            status: WatchStatus::IncompleteRecord,
            verified_checkpoint: None,
            expected: Some(config.checkpoint),
        },
        Err(LogError::Truncated { sequence }) if sequence <= config.checkpoint.count => {
            WatchSample {
                status: WatchStatus::CheckpointAhead,
                verified_checkpoint: None,
                expected: Some(config.checkpoint),
            }
        }
        Err(LogError::Truncated { .. }) => WatchSample {
            status: WatchStatus::IncompleteRecord,
            verified_checkpoint: None,
            expected: Some(config.checkpoint),
        },
        Err(LogError::CheckpointMismatch) => WatchSample {
            status: WatchStatus::CheckpointUnmatched,
            verified_checkpoint: None,
            expected: Some(config.checkpoint),
        },
        Err(error) => WatchSample {
            status: status_from_log_error(&error),
            verified_checkpoint: None,
            expected: Some(config.checkpoint),
        },
    }
}

fn status_from_log_error(error: &LogError) -> WatchStatus {
    match error {
        LogError::BrokenRecord {
            error: AuditError::RoleDenied,
            ..
        } => WatchStatus::WrongRoleAction,
        LogError::BrokenRecord {
            error: AuditError::LogLimitReached,
            ..
        }
        | LogError::Audit(AuditError::LogLimitReached) => WatchStatus::LimitReached,
        LogError::BrokenRecord { .. } => WatchStatus::ChainBroken,
        LogError::Truncated { .. } => WatchStatus::IncompleteRecord,
        LogError::IncompleteRecord { .. } => WatchStatus::IncompleteRecord,
        LogError::CheckpointMismatch => WatchStatus::CheckpointUnmatched,
        _ => WatchStatus::VerificationUnavailable,
    }
}

fn read_watch_config(path: &Path) -> Result<WatchConfig, String> {
    let (mut file, _) = oa_sealed::Sealed::production()
        .open_regular(path)
        .map_err(|error| format!("sealed config refusal: {error}"))?;
    let before = file
        .metadata()
        .map_err(|error| format!("config metadata failed: {error}"))?;
    let len = usize::try_from(before.len()).map_err(|_| "config size conversion failed")?;
    if len == 0 || len > MAX_CONFIG_BYTES {
        return Err("checkpoint config size is outside 1..=256 bytes".into());
    }
    let mut first = [0u8; MAX_CONFIG_BYTES];
    let first_slice = first
        .get_mut(..len)
        .ok_or_else(|| "checkpoint config exceeds its fixed buffer".to_owned())?;
    file.read_exact(first_slice)
        .map_err(|error| format!("checkpoint config read failed: {error}"))?;
    let mut extra = [0u8; 1];
    if file
        .read(&mut extra)
        .map_err(|error| format!("checkpoint config trailing read failed: {error}"))?
        != 0
    {
        return Err("checkpoint config changed while being read".into());
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("checkpoint config rewind failed: {error}"))?;
    let mut second = [0u8; MAX_CONFIG_BYTES];
    let second_slice = second
        .get_mut(..len)
        .ok_or_else(|| "checkpoint config exceeds its fixed buffer".to_owned())?;
    file.read_exact(second_slice)
        .map_err(|error| format!("checkpoint config reread failed: {error}"))?;
    let after = file
        .metadata()
        .map_err(|error| format!("checkpoint config metadata reread failed: {error}"))?;
    if first.get(..len) != second.get(..len) || before.len() != after.len() {
        return Err("checkpoint config snapshot changed during read".into());
    }
    let text = std::str::from_utf8(first.get(..len).ok_or("config slice missing")?)
        .map_err(|_| "checkpoint config must be ASCII UTF-8")?;
    if !text.is_ascii() {
        return Err("checkpoint config must be ASCII".into());
    }
    parse_watch_config(text)
}

fn parse_watch_config(text: &str) -> Result<WatchConfig, String> {
    let mut lines = text.lines();
    if lines.next() != Some("OA-AUDIT-WATCH-V1") {
        return Err("checkpoint config header/version is invalid".into());
    }
    let generation = parse_config_field(lines.next(), "generation")?;
    let count = parse_config_field(lines.next(), "count")?;
    let tail = lines
        .next()
        .and_then(|line| line.strip_prefix("tail="))
        .ok_or_else(|| "checkpoint config is missing tail".to_owned())?;
    let refresh_ms = parse_config_field(lines.next(), "refresh_ms")?;
    if lines.next().is_some() || generation == 0 || refresh_ms > 86_400_000 {
        return Err("checkpoint config has extra fields or values outside bounds".into());
    }
    Ok(WatchConfig {
        generation,
        checkpoint: Checkpoint {
            count,
            tail: parse_digest(tail)?,
        },
        refresh_ms,
    })
}

fn parse_config_field(line: Option<&str>, name: &str) -> Result<u64, String> {
    let prefix = format!("{name}=");
    let value = line
        .and_then(|line| line.strip_prefix(&prefix))
        .ok_or_else(|| format!("checkpoint config is missing {name}"))?;
    value
        .parse::<u64>()
        .map_err(|_| format!("checkpoint config {name} must be unsigned integer"))
}

fn update_alarms(
    alarms: &mut [Alarm; ALARM_COUNT],
    generations: &mut [u32; ALARM_COUNT],
    active: [bool; ALARM_COUNT],
    now_ns: i64,
    output_bytes: &mut usize,
) -> Result<(), String> {
    let resource = ResourceKey {
        resource_id: 1,
        parent_service_id: 1,
        kind: ResourceKind::Service,
    };
    for (index, spec) in ALARM_SPECS.iter().enumerate() {
        let is_active = *active
            .get(index)
            .ok_or_else(|| "alarm state index outside fixed list".to_owned())?;
        let slot = alarms
            .get_mut(index)
            .ok_or_else(|| "alarm slot outside fixed list".to_owned())?;
        let generation = generations
            .get_mut(index)
            .ok_or_else(|| "alarm generation outside fixed list".to_owned())?;
        if is_active && slot.state == AlarmState::Empty {
            *generation = generation
                .checked_add(1)
                .ok_or_else(|| "alarm instance generation overflow".to_owned())?;
        }
        let instance_id = (spec.instance << 32) | u64::from(*generation);
        if !is_active && slot.state == AlarmState::Empty {
            continue;
        }
        let transition = record_alarm_transition(
            core::slice::from_mut(slot),
            instance_id,
            resource,
            spec.reason,
            spec.severity,
            is_active,
            AlarmTime {
                clock_domain: ClockDomainId(1),
                at_ns: now_ns,
            },
        )
        .map_err(|error| format!("Tech Ops alarm transition refused: {error:?}"))?;
        if transition != AlarmTransition::Unchanged {
            let state = if is_active { "raised" } else { "cleared" };
            write_bounded_line(
                output_bytes,
                &format!("alarm\t{instance_id}\t{state}\t{}", spec.reason),
            )?;
            if !is_active {
                *slot = EMPTY_ALARM;
            }
        }
    }
    Ok(())
}

fn write_bounded_line(output_bytes: &mut usize, line: &str) -> Result<(), String> {
    let next = output_bytes
        .checked_add(line.len())
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or_else(|| "watch output byte counter overflow".to_owned())?;
    if next > MAX_OUTPUT_BYTES {
        return Err("watch output exceeded its 1 MiB bound".into());
    }
    println!("{line}");
    *output_bytes = next;
    Ok(())
}

fn elapsed_ns(started: Instant, now: Instant) -> Result<i64, String> {
    i64::try_from(now.saturating_duration_since(started).as_nanos())
        .map_err(|_| "watch monotonic timestamp exceeds signed nanoseconds".into())
}

fn is_stale(refresh_ms: Option<u64>, seen: Instant, now: Instant) -> bool {
    refresh_ms.is_some_and(|limit| {
        limit > 0 && now.saturating_duration_since(seen) > Duration::from_millis(limit)
    })
}

fn parse_bounded_u64(text: &str, min: u64, max: u64, field: &str) -> Result<u64, String> {
    let value = text
        .parse::<u64>()
        .map_err(|_| format!("{field} must be an unsigned integer"))?;
    if value < min || value > max {
        return Err(format!("{field} must be in {min}..={max}"));
    }
    Ok(value)
}

fn parse_digest(text: &str) -> Result<[u8; 32], String> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return Err("tail digest must contain exactly 64 lowercase hex digits".into());
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let offset = index
            .checked_mul(2)
            .ok_or_else(|| "digest offset overflow".to_owned())?;
        let high = bytes
            .get(offset)
            .copied()
            .ok_or_else(|| "short digest".to_owned())?;
        let low = bytes
            .get(offset + 1)
            .copied()
            .ok_or_else(|| "short digest".to_owned())?;
        *byte = (nibble(high)? << 4) | nibble(low)?;
    }
    Ok(digest)
}

fn nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("tail digest must use lowercase hexadecimal".into()),
    }
}

fn status_name(status: WatchStatus) -> &'static str {
    match status {
        WatchStatus::Valid => "valid",
        WatchStatus::GenerationRegression => "generation_regression_unverified",
        WatchStatus::GenerationChangedWithoutAdvance => "generation_change_without_advance",
        WatchStatus::GenerationAdvancedUnverified => "generation_advance_unverified",
        WatchStatus::CheckpointAhead => "checkpoint_ahead_pending",
        WatchStatus::CheckpointUnmatched => "checkpoint_unmatched_pending",
        WatchStatus::IncompleteRecord => "incomplete_record_pending",
        WatchStatus::ChainBroken => "chain_broken",
        WatchStatus::WrongRoleAction => "wrong_role_action",
        WatchStatus::LimitReached => "limit_reached",
        WatchStatus::VerificationUnavailable => "verification_unavailable",
    }
}

fn rejected_config_status(
    accepted: Option<WatchConfig>,
    proposed: WatchConfig,
    observed_status: WatchStatus,
) -> Option<WatchStatus> {
    match accepted {
        Some(previous) if proposed.generation < previous.generation => {
            Some(WatchStatus::GenerationRegression)
        }
        Some(previous) if proposed.generation == previous.generation && proposed != previous => {
            Some(WatchStatus::GenerationChangedWithoutAdvance)
        }
        Some(previous)
            if proposed.generation > previous.generation
                && observed_status != WatchStatus::Valid =>
        {
            Some(WatchStatus::GenerationAdvancedUnverified)
        }
        _ => None,
    }
}

fn format_log_error(error: &LogError) -> String {
    match error {
        LogError::BrokenRecord {
            sequence,
            error: AuditError::RoleDenied,
        } => {
            format!("wrong role/action at record {sequence}")
        }
        LogError::BrokenRecord { sequence, error } => {
            format!("first broken record {sequence}: {error:?}")
        }
        LogError::Truncated { sequence } => format!("incomplete record {sequence}"),
        LogError::IncompleteRecord { sequence } => format!("partial record {sequence}"),
        other => format!("verification failed: {other:?}"),
    }
}

fn usage() -> &'static str {
    "usage: oa-audit-verify --log <file> --count <n> --tail <64 lowercase hex>"
}

fn watch_usage() -> &'static str {
    "usage: oa-audit-verify --watch --log <file> --checkpoint-config <file> --max-polls <1..86400> --max-records <1..65536>"
}

fn watch_help() -> &'static str {
    "watch mode polls every 1s and re-reads a stable, sealed regular checkpoint config. Format: OA-AUDIT-WATCH-V1, generation=<u64>, count=<u64>, tail=<64 lowercase hex>, refresh_ms=<0..86400000>, one field per line. The checkpoint pair is an operator-supplied external trust input, not authenticated or HSM-backed. A lagging/ahead checkpoint and incomplete record are pending/race statuses, not proof of tampering. refresh_ms=0 disables stale-checkpoint alarms; a positive value requires the publisher to advance generation as a heartbeat even during a quiet log. Polls cap at 86400, runtime at 24h, expected count at max-records (<=65536) plus one record for longer-log refusal, and output at 1MiB. Only exact checkpoint verification reports a verified count; mismatches never expose a computed head."
}

#[cfg(test)]
mod tests {
    // Tests use `expect` for readable fixture setup only; production paths return typed refusals.
    #![allow(clippy::expect_used)]
    #![allow(clippy::unwrap_used)]
    use super::*;
    use oa_audit_core::genesis;

    fn event() -> oa_audit_core::Event {
        oa_audit_core::Event {
            principal: 7,
            role: oa_audit_core::Role::Configure,
            action: oa_audit_core::Action::Certification,
            target: oa_audit_core::target(b"test").expect("valid target").0,
            target_len: 4,
            time_ns: 1,
            details: [0; oa_audit_core::DETAILS_CAPACITY],
            details_len: 0,
        }
    }

    fn path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "oa-audit-watch-{label}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ))
    }

    fn write_config(path: &Path, checkpoint: Checkpoint, generation: u64, refresh_ms: u64) {
        let text = format!(
            "OA-AUDIT-WATCH-V1\ngeneration={generation}\ncount={}\ntail={}\nrefresh_ms={refresh_ms}\n",
            checkpoint.count,
            oa_sha256::to_hex(&checkpoint.tail)
        );
        std::fs::write(path, text).expect("config write");
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_generation_requires_verified_strict_advance_and_immutable_same_generation_pin() {
        let checkpoint = genesis();
        let accepted = WatchConfig {
            generation: 5,
            checkpoint,
            refresh_ms: 1,
        };
        assert_eq!(
            rejected_config_status(None, accepted, WatchStatus::Valid),
            None
        );
        assert_eq!(
            rejected_config_status(
                Some(accepted),
                WatchConfig {
                    generation: 4,
                    ..accepted
                },
                WatchStatus::Valid,
            ),
            Some(WatchStatus::GenerationRegression)
        );
        assert_eq!(
            rejected_config_status(
                Some(accepted),
                WatchConfig {
                    refresh_ms: 0,
                    ..accepted
                },
                WatchStatus::Valid,
            ),
            Some(WatchStatus::GenerationChangedWithoutAdvance)
        );
        assert_eq!(
            rejected_config_status(
                Some(accepted),
                WatchConfig {
                    generation: 6,
                    ..accepted
                },
                WatchStatus::CheckpointUnmatched,
            ),
            Some(WatchStatus::GenerationAdvancedUnverified)
        );
        assert_eq!(
            rejected_config_status(
                Some(accepted),
                WatchConfig {
                    generation: 6,
                    ..accepted
                },
                WatchStatus::Valid,
            ),
            None
        );
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_parses_exact_bounded_config_and_refuses_unknown_or_extra_fields() {
        let genesis = genesis();
        let good = format!(
            "OA-AUDIT-WATCH-V1\ngeneration=1\ncount=0\ntail={}\nrefresh_ms=0\n",
            oa_sha256::to_hex(&genesis.tail)
        );
        assert_eq!(
            parse_watch_config(&good).expect("valid config").checkpoint,
            genesis
        );
        assert!(parse_watch_config(&good.replace("V1", "V2")).is_err());
        assert!(parse_watch_config(&format!("{good}extra=1\n")).is_err());
        assert!(parse_watch_config(&good.replace("refresh_ms=0", "refresh_ms=86400001")).is_err());
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_reads_the_external_checkpoint_config_through_the_sealed_regular_file_api() {
        let config_path = path("config");
        let _ = std::fs::remove_file(&config_path);
        let expected = genesis();
        write_config(&config_path, expected, 1, 0);
        assert_eq!(
            read_watch_config(&config_path)
                .expect("read config")
                .checkpoint,
            expected
        );
        std::fs::remove_file(&config_path).expect("remove config");
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_inspection_accepts_valid_log_and_classifies_checkpoint_lag_and_lead() {
        let log = path("valid");
        let _ = std::fs::remove_file(&log);
        let expected = shell::append(&log, genesis(), event()).expect("append event");
        let valid = inspect_log(
            &log,
            WatchConfig {
                generation: 1,
                checkpoint: expected,
                refresh_ms: 0,
            },
            2,
        );
        assert_eq!(valid.status, WatchStatus::Valid);
        let lag = inspect_log(
            &log,
            WatchConfig {
                generation: 2,
                checkpoint: genesis(),
                refresh_ms: 0,
            },
            2,
        );
        assert_eq!(lag.status, WatchStatus::CheckpointUnmatched);
        let tail_mismatch = inspect_log(
            &log,
            WatchConfig {
                generation: 3,
                checkpoint: Checkpoint {
                    count: expected.count,
                    tail: [9; 32],
                },
                refresh_ms: 0,
            },
            2,
        );
        assert_eq!(tail_mismatch.status, WatchStatus::CheckpointUnmatched);
        let lead = inspect_log(
            &log,
            WatchConfig {
                generation: 4,
                checkpoint: Checkpoint {
                    count: expected.count.saturating_add(1),
                    tail: [9; 32],
                },
                refresh_ms: 0,
            },
            2,
        );
        assert_eq!(lead.status, WatchStatus::CheckpointAhead);
        let _ = std::fs::remove_file(log);
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_inspection_separates_tamper_and_truncation_from_checkpoint_race() {
        let log = path("tamper");
        let _ = std::fs::remove_file(&log);
        let expected = shell::append(&log, genesis(), event()).expect("append event");
        let original = std::fs::read(&log).expect("read log");
        let mut bytes = original.clone();
        if let Some(byte) = bytes.get_mut(64) {
            *byte ^= 1;
        }
        std::fs::write(&log, &bytes).expect("tamper log");
        assert_eq!(
            inspect_log(
                &log,
                WatchConfig {
                    generation: 1,
                    checkpoint: expected,
                    refresh_ms: 0
                },
                2,
            )
            .status,
            WatchStatus::ChainBroken
        );
        std::fs::write(
            &log,
            original
                .get(..original.len().saturating_sub(1))
                .expect("prefix"),
        )
        .expect("truncate log");
        assert_eq!(
            inspect_log(
                &log,
                WatchConfig {
                    generation: 1,
                    checkpoint: expected,
                    refresh_ms: 0
                },
                2,
            )
            .status,
            WatchStatus::IncompleteRecord
        );
        let _ = std::fs::remove_file(log);
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_inspection_reports_missing_and_reordered_records() {
        let log = path("reorder");
        let _ = std::fs::remove_file(&log);
        let first = shell::append(&log, genesis(), event()).expect("append first");
        let second = shell::append(&log, first, event()).expect("append second");
        let bytes = std::fs::read(&log).expect("read log");
        let record_len = bytes.len() / 2;
        let mut reversed = Vec::with_capacity(bytes.len());
        reversed.extend_from_slice(bytes.get(record_len..).expect("second record"));
        reversed.extend_from_slice(bytes.get(..record_len).expect("first record"));
        std::fs::write(&log, reversed).expect("write reordered log");
        assert_eq!(
            inspect_log(
                &log,
                WatchConfig {
                    generation: 1,
                    checkpoint: second,
                    refresh_ms: 0
                },
                2,
            )
            .status,
            WatchStatus::ChainBroken
        );
        std::fs::remove_file(&log).expect("remove log");
        assert_eq!(
            inspect_log(
                &log,
                WatchConfig {
                    generation: 1,
                    checkpoint: second,
                    refresh_ms: 0
                },
                2,
            )
            .status,
            WatchStatus::VerificationUnavailable
        );
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_classifies_a_role_action_refusal_as_a_distinct_status() {
        let error = LogError::BrokenRecord {
            sequence: 1,
            error: AuditError::RoleDenied,
        };
        assert_eq!(status_from_log_error(&error), WatchStatus::WrongRoleAction);
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_staleness_requires_an_explicit_generation_refresh_contract() {
        let now = Instant::now();
        assert!(!is_stale(Some(0), now, now + Duration::from_secs(2)));
        assert!(is_stale(Some(1000), now, now + Duration::from_millis(1001)));
        assert!(!is_stale(Some(1000), now, now + Duration::from_millis(999)));
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn techops_alarm_instances_raise_idempotently_and_clear_on_recovery() {
        let mut alarms = [EMPTY_ALARM; ALARM_COUNT];
        let mut generations = [0u32; ALARM_COUNT];
        let mut output = 0;
        update_alarms(
            &mut alarms,
            &mut generations,
            [true, false, false, false, false],
            1,
            &mut output,
        )
        .expect("raise");
        update_alarms(
            &mut alarms,
            &mut generations,
            [true, false, false, false, false],
            2,
            &mut output,
        )
        .expect("same alarm");
        let active_count = alarms
            .iter()
            .filter(|alarm| alarm.state == AlarmState::Active)
            .count();
        assert_eq!(active_count, 1);
        update_alarms(
            &mut alarms,
            &mut generations,
            [false, false, false, false, false],
            3,
            &mut output,
        )
        .expect("clear");
        assert!(alarms.iter().all(|alarm| alarm.state != AlarmState::Active));
        update_alarms(
            &mut alarms,
            &mut generations,
            [true, false, false, false, false],
            4,
            &mut output,
        )
        .expect("re-raise after recovery");
        assert_eq!(
            alarms.first().map(|alarm| alarm.state),
            Some(AlarmState::Active)
        );
        assert_eq!(generations.first().copied(), Some(2));
    }

    // REQ: OA-AUDIT-011
    #[test]
    fn watch_output_limit_refuses_before_emitting_an_oversized_line() {
        let mut bytes = MAX_OUTPUT_BYTES - 2;
        assert!(write_bounded_line(&mut bytes, "long").is_err());
        assert_eq!(bytes, MAX_OUTPUT_BYTES - 2);
    }
}
